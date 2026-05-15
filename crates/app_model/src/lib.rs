use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use config_store::AppConfig;
use file_ready_checker::{wait_for_file_ready, FileReadyInfo, FileReadyOptions};
use history_store::{
    CodeType as StoredCodeType, DecodedKind as StoredDecodedKind, HistoryStore, NewReadHistory,
    NewReadResult, ReadStatus,
};
use photo_watcher::PhotoWatcherEvent;
use qr_core::{
    CodeType as DecodedCodeType, DecodedKind as DecodedCodeKind, QrDecodeOptions, QrDecodeReport,
    QrDecoder, QrPreprocessingOptions, RqrrDecoder,
};
use tokio::task;

#[derive(Debug, Clone)]
pub struct AppModel {
    pub config: AppConfig,
    pub watch_state: WatchState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchState {
    Stopped,
    Running,
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            config: AppConfig::default(),
            watch_state: WatchState::Stopped,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReadPipeline {
    history_store: HistoryStore,
    file_ready_options: FileReadyOptions,
    decoder: RqrrDecoder,
    created_files: CreatedFileQueue,
}

impl ReadPipeline {
    pub fn new(history_store: HistoryStore, file_ready_options: FileReadyOptions) -> Self {
        Self {
            history_store,
            file_ready_options,
            decoder: RqrrDecoder::default(),
            created_files: CreatedFileQueue::default(),
        }
    }

    pub fn from_config(history_store: HistoryStore, config: &AppConfig) -> Self {
        let mut pipeline = Self::new(
            history_store,
            FileReadyOptions {
                timeout_ms: config.decode.file_ready_timeout_ms,
                check_interval_ms: config.decode.file_ready_check_interval_ms,
                required_stable_checks: config.decode.required_stable_checks,
            },
        );
        pipeline.decoder = RqrrDecoder::new(QrDecodeOptions {
            force_preprocessing: QrPreprocessingOptions {
                contrast: config.decode.preprocessing.force_contrast,
                brighten: config.decode.preprocessing.force_brighten,
                threshold: config.decode.preprocessing.force_threshold,
                contrast_threshold: config.decode.preprocessing.force_contrast_threshold,
                invert: config.decode.preprocessing.force_invert,
            },
        });
        pipeline
    }

    pub async fn process_photo_watcher_event(
        &self,
        event: PhotoWatcherEvent,
    ) -> anyhow::Result<Option<ProcessedRead>> {
        match event {
            PhotoWatcherEvent::Created(path) => {
                let Some(_guard) = self.created_files.try_start(&path) else {
                    return Ok(None);
                };

                self.process_created_image(path).await.map(Some)
            }
            PhotoWatcherEvent::Error(error) => Err(anyhow::anyhow!("photo watcher error: {error}")),
        }
    }

    pub async fn process_created_image(
        &self,
        path: impl AsRef<Path>,
    ) -> anyhow::Result<ProcessedRead> {
        let path = path.as_ref().to_path_buf();
        let detected_at = unix_seconds_now();

        match wait_for_file_ready(&path, self.file_ready_options).await {
            Ok(file_info) => {
                let request = ReadyReadRequest {
                    path,
                    detected_at,
                    file_info,
                    history_store: self.history_store.clone(),
                    decoder: self.decoder,
                };

                task::spawn_blocking(move || process_ready_read(request))
                    .await
                    .context("read pipeline worker task failed")?
            }
            Err(error) => {
                let history = failed_history_from_path(&path, detected_at, error.to_string());
                insert_read_blocking(self.history_store.clone(), history, Vec::new()).await
            }
        }
    }
}

const DEFAULT_DUPLICATE_SUPPRESSION_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct CreatedFileQueue {
    state: Arc<Mutex<CreatedFileQueueState>>,
    duplicate_window: Duration,
}

impl Default for CreatedFileQueue {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(CreatedFileQueueState::default())),
            duplicate_window: DEFAULT_DUPLICATE_SUPPRESSION_WINDOW,
        }
    }
}

impl CreatedFileQueue {
    pub fn with_duplicate_window(duplicate_window: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(CreatedFileQueueState::default())),
            duplicate_window,
        }
    }

    pub fn try_start(&self, path: impl AsRef<Path>) -> Option<CreatedFileGuard> {
        let path = path.as_ref().to_path_buf();
        let now = Instant::now();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state
            .recent
            .retain(|_, seen_at| now.saturating_duration_since(*seen_at) < self.duplicate_window);

        if state.in_flight.contains(&path) {
            return None;
        }

        if state
            .recent
            .get(&path)
            .is_some_and(|seen_at| now.saturating_duration_since(*seen_at) < self.duplicate_window)
        {
            return None;
        }

        state.in_flight.insert(path.clone());

        Some(CreatedFileGuard {
            path,
            state: Arc::clone(&self.state),
        })
    }
}

#[derive(Debug, Default)]
struct CreatedFileQueueState {
    in_flight: HashSet<PathBuf>,
    recent: HashMap<PathBuf, Instant>,
}

#[derive(Debug)]
pub struct CreatedFileGuard {
    path: PathBuf,
    state: Arc<Mutex<CreatedFileQueueState>>,
}

impl Drop for CreatedFileGuard {
    fn drop(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state.in_flight.remove(&self.path);
        state.recent.insert(self.path.clone(), Instant::now());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedRead {
    pub history_id: String,
    pub source_path: PathBuf,
    pub status: ReadStatus,
    pub result_count: usize,
    pub decode_attempts: Vec<String>,
}

struct ReadyReadRequest {
    path: PathBuf,
    detected_at: i64,
    file_info: FileReadyInfo,
    history_store: HistoryStore,
    decoder: RqrrDecoder,
}

fn process_ready_read(request: ReadyReadRequest) -> anyhow::Result<ProcessedRead> {
    let mut history = history_from_file_info(
        &request.path,
        request.detected_at,
        &request.file_info,
        ReadStatus::Failed,
        None,
    );

    let mut decode_attempts = Vec::new();
    let results = match request.decoder.decode_path_report(&request.path) {
        Ok(report) => {
            let status = status_from_report(&report);
            history.status = status;
            history.error_message = error_message_from_report(&report);
            decode_attempts = report
                .attempts
                .iter()
                .map(|attempt| attempt.label.clone())
                .collect();

            if status == ReadStatus::Decoded {
                report
                    .decoded_codes
                    .into_iter()
                    .map(new_read_result_from_decoded_code)
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            }
        }
        Err(error) => {
            history.status = ReadStatus::Failed;
            history.error_message = Some(error.to_string());
            Vec::new()
        }
    };

    let status = history.status;
    let result_count = results.len();
    let history_id = request.history_store.insert_read(history, &results)?;

    Ok(ProcessedRead {
        history_id,
        source_path: request.path,
        status,
        result_count,
        decode_attempts,
    })
}

async fn insert_read_blocking(
    history_store: HistoryStore,
    history: NewReadHistory,
    results: Vec<NewReadResult>,
) -> anyhow::Result<ProcessedRead> {
    let source_path = history.source_path.clone();
    let status = history.status;
    let result_count = results.len();

    let history_id = task::spawn_blocking(move || history_store.insert_read(history, &results))
        .await
        .context("history insert worker task failed")??;

    Ok(ProcessedRead {
        history_id,
        source_path,
        status,
        result_count,
        decode_attempts: Vec::new(),
    })
}

fn history_from_file_info(
    path: &Path,
    detected_at: i64,
    file_info: &FileReadyInfo,
    status: ReadStatus,
    error_message: Option<String>,
) -> NewReadHistory {
    NewReadHistory {
        source_path: path.to_path_buf(),
        source_file_name: source_file_name(path),
        source_file_size: i64_from_u64_saturating(file_info.size),
        source_modified_at: file_info.modified_at.and_then(unix_seconds),
        detected_at,
        status,
        error_message,
    }
}

fn failed_history_from_path(
    path: &Path,
    detected_at: i64,
    error_message: String,
) -> NewReadHistory {
    let metadata = std::fs::metadata(path).ok();

    NewReadHistory {
        source_path: path.to_path_buf(),
        source_file_name: source_file_name(path),
        source_file_size: metadata
            .as_ref()
            .map(|metadata| i64_from_u64_saturating(metadata.len()))
            .unwrap_or_default(),
        source_modified_at: metadata
            .and_then(|metadata| metadata.modified().ok())
            .and_then(unix_seconds),
        detected_at,
        status: ReadStatus::Failed,
        error_message: Some(error_message),
    }
}

fn status_from_report(report: &QrDecodeReport) -> ReadStatus {
    if !report.decoded_codes.is_empty() {
        ReadStatus::Decoded
    } else if report.detected_grids == 0 {
        ReadStatus::NoCode
    } else {
        ReadStatus::Failed
    }
}

fn error_message_from_report(report: &QrDecodeReport) -> Option<String> {
    if report.all_detected_grids_failed() {
        Some(format!(
            "detected {} QR grid(s), but failed to decode all of them",
            report.detected_grids
        ))
    } else {
        None
    }
}

fn new_read_result_from_decoded_code(
    decoded_code: qr_core::DecodedCode,
) -> anyhow::Result<NewReadResult> {
    Ok(NewReadResult {
        code_type: stored_code_type(decoded_code.code_type),
        decoded_text: decoded_code.decoded_text,
        decoded_kind: stored_decoded_kind(decoded_code.decoded_kind),
    })
}

fn stored_code_type(code_type: DecodedCodeType) -> StoredCodeType {
    match code_type {
        DecodedCodeType::Qr => StoredCodeType::Qr,
    }
}

fn stored_decoded_kind(decoded_kind: DecodedCodeKind) -> StoredDecodedKind {
    match decoded_kind {
        DecodedCodeKind::Url => StoredDecodedKind::Url,
        DecodedCodeKind::Text => StoredDecodedKind::Text,
    }
}

fn source_file_name(path: &Path) -> String {
    path.file_name()
        .map(|file_name| file_name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn unix_seconds_now() -> i64 {
    unix_seconds(SystemTime::now()).unwrap_or_default()
}

fn unix_seconds(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| i64_from_u64_saturating(duration.as_secs()))
}

fn i64_from_u64_saturating(value: u64) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn process_created_image_decodes_qr_and_stores_results() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let image_path = temp_dir.path().join("qr.png");
        let db_path = temp_dir.path().join("history.sqlite3");
        save_qr_image(&image_path, b"https://example.com");

        let store = HistoryStore::open(&db_path).expect("open history store");
        let pipeline = ReadPipeline::new(store, fast_ready_options());
        let outcome = pipeline
            .process_created_image(&image_path)
            .await
            .expect("process QR image");

        assert_eq!(outcome.status, ReadStatus::Decoded);
        assert_eq!(outcome.result_count, 1);

        let store = HistoryStore::open(&db_path).expect("open history store");
        let history = store
            .get_history(&outcome.history_id)
            .expect("get history")
            .expect("history exists");
        let results = store
            .list_results(&outcome.history_id)
            .expect("list results");

        assert_eq!(history.status, ReadStatus::Decoded);
        assert_eq!(history.source_file_name, "qr.png");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].decoded_text, "https://example.com");
        assert_eq!(results[0].decoded_kind, StoredDecodedKind::Url);
    }

    #[test]
    fn read_pipeline_can_be_built_from_config_decode_options() {
        let mut config = AppConfig::default();
        config.decode.file_ready_timeout_ms = 123;
        config.decode.file_ready_check_interval_ms = 45;
        config.decode.required_stable_checks = 6;

        let store = HistoryStore::open_in_memory().expect("open history store");
        let pipeline = ReadPipeline::from_config(store, &config);

        assert_eq!(pipeline.file_ready_options.timeout_ms, 123);
        assert_eq!(pipeline.file_ready_options.check_interval_ms, 45);
        assert_eq!(pipeline.file_ready_options.required_stable_checks, 6);
    }

    #[tokio::test]
    async fn process_created_image_stores_no_code_for_blank_image() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let image_path = temp_dir.path().join("blank.png");
        let db_path = temp_dir.path().join("history.sqlite3");
        save_blank_image(&image_path);

        let store = HistoryStore::open(&db_path).expect("open history store");
        let pipeline = ReadPipeline::new(store, fast_ready_options());
        let outcome = pipeline
            .process_created_image(&image_path)
            .await
            .expect("process blank image");

        assert_eq!(outcome.status, ReadStatus::NoCode);
        assert_eq!(outcome.result_count, 0);
        assert!(HistoryStore::open(&db_path)
            .expect("open history store")
            .list_results(&outcome.history_id)
            .expect("list results")
            .is_empty());
    }

    #[tokio::test]
    async fn process_created_image_stores_failed_when_file_never_becomes_ready() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let image_path = temp_dir.path().join("invalid.png");
        let db_path = temp_dir.path().join("history.sqlite3");
        std::fs::write(&image_path, b"not an image").expect("write invalid image");

        let store = HistoryStore::open(&db_path).expect("open history store");
        let pipeline = ReadPipeline::new(
            store,
            FileReadyOptions {
                timeout_ms: 20,
                check_interval_ms: 5,
                required_stable_checks: 1,
            },
        );
        let outcome = pipeline
            .process_created_image(&image_path)
            .await
            .expect("process invalid image");

        let store = HistoryStore::open(&db_path).expect("open history store");
        let history = store
            .get_history(&outcome.history_id)
            .expect("get history")
            .expect("history exists");

        assert_eq!(outcome.status, ReadStatus::Failed);
        assert_eq!(history.status, ReadStatus::Failed);
        assert!(history
            .error_message
            .expect("error message")
            .contains("timed out waiting for file to become ready"));
    }

    #[tokio::test]
    async fn process_photo_watcher_event_ignores_no_event_but_processes_created() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let image_path = temp_dir.path().join("blank.png");
        let db_path = temp_dir.path().join("history.sqlite3");
        save_blank_image(&image_path);

        let store = HistoryStore::open(&db_path).expect("open history store");
        let pipeline = ReadPipeline::new(store, fast_ready_options());
        let outcome = pipeline
            .process_photo_watcher_event(PhotoWatcherEvent::Created(image_path))
            .await
            .expect("process watcher event")
            .expect("created image produces outcome");

        assert_eq!(outcome.status, ReadStatus::NoCode);
    }

    #[tokio::test]
    async fn process_photo_watcher_event_suppresses_recent_duplicate_paths() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let image_path = temp_dir.path().join("blank.png");
        let db_path = temp_dir.path().join("history.sqlite3");
        save_blank_image(&image_path);

        let store = HistoryStore::open(&db_path).expect("open history store");
        let pipeline = ReadPipeline::new(store, fast_ready_options());

        assert!(pipeline
            .process_photo_watcher_event(PhotoWatcherEvent::Created(image_path.clone()))
            .await
            .expect("process first event")
            .is_some());
        assert!(pipeline
            .process_photo_watcher_event(PhotoWatcherEvent::Created(image_path))
            .await
            .expect("process duplicate event")
            .is_none());

        assert_eq!(
            HistoryStore::open(&db_path)
                .expect("open history store")
                .list_histories(10)
                .expect("list histories")
                .len(),
            1
        );
    }

    #[test]
    fn created_file_queue_allows_path_after_duplicate_window() {
        let queue = CreatedFileQueue::with_duplicate_window(Duration::ZERO);
        let path = PathBuf::from("created.png");

        drop(queue.try_start(&path).expect("start first path"));

        assert!(queue.try_start(&path).is_some());
    }

    fn fast_ready_options() -> FileReadyOptions {
        FileReadyOptions {
            timeout_ms: 1_000,
            check_interval_ms: 5,
            required_stable_checks: 1,
        }
    }

    fn save_qr_image(path: &Path, text: &[u8]) {
        let code = qrcode::QrCode::new(text).expect("create qr code");
        let image = code
            .render::<image::Luma<u8>>()
            .min_dimensions(256, 256)
            .build();
        image.save(path).expect("save qr image");
    }

    fn save_blank_image(path: &Path) {
        let image = image::GrayImage::from_pixel(64, 64, image::Luma([255]));
        image.save(path).expect("save blank image");
    }
}
