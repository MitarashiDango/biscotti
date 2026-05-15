use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use thiserror::Error;
use tokio::{
    task,
    time::{sleep, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileReadyOptions {
    pub timeout_ms: u64,
    pub check_interval_ms: u64,
    pub required_stable_checks: u32,
}

impl Default for FileReadyOptions {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            check_interval_ms: 400,
            required_stable_checks: 2,
        }
    }
}

impl FileReadyOptions {
    fn validate(self) -> Result<(), FileReadyWaitError> {
        if self.timeout_ms == 0 {
            return Err(FileReadyWaitError::InvalidOptions(
                "timeout_ms must be greater than 0",
            ));
        }

        if self.check_interval_ms == 0 {
            return Err(FileReadyWaitError::InvalidOptions(
                "check_interval_ms must be greater than 0",
            ));
        }

        if self.required_stable_checks == 0 {
            return Err(FileReadyWaitError::InvalidOptions(
                "required_stable_checks must be greater than 0",
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReadyInfo {
    pub path: PathBuf,
    pub size: u64,
    pub modified_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileReadyState {
    NotChecked,
    Missing,
    MetadataError(String),
    NotRegularFile,
    EmptyFile,
    SizeNotStable {
        size: u64,
        unchanged_checks: u32,
        required_stable_checks: u32,
    },
    ImageOpenFailed {
        size: u64,
        error_message: String,
    },
}

impl fmt::Display for FileReadyState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileReadyState::NotChecked => write!(formatter, "file has not been checked yet"),
            FileReadyState::Missing => write!(formatter, "file does not exist"),
            FileReadyState::MetadataError(error) => {
                write!(formatter, "failed to read file metadata: {error}")
            }
            FileReadyState::NotRegularFile => write!(formatter, "path is not a regular file"),
            FileReadyState::EmptyFile => write!(formatter, "file size is 0"),
            FileReadyState::SizeNotStable {
                size,
                unchanged_checks,
                required_stable_checks,
            } => write!(
                formatter,
                "file size is not stable yet: size={size}, unchanged_checks={unchanged_checks}/{required_stable_checks}"
            ),
            FileReadyState::ImageOpenFailed {
                size,
                error_message,
            } => write!(
                formatter,
                "file is stable but image open failed: size={size}, error={error_message}"
            ),
        }
    }
}

#[derive(Debug, Error)]
pub enum FileReadyWaitError {
    #[error("invalid file ready options: {0}")]
    InvalidOptions(&'static str),
    #[error(
        "timed out waiting for file to become ready: path={path}, timeout_ms={timeout_ms}, last_state={last_state}"
    )]
    TimedOut {
        path: PathBuf,
        timeout_ms: u64,
        last_state: FileReadyState,
    },
}

pub async fn wait_for_file_ready(
    path: impl AsRef<Path>,
    options: FileReadyOptions,
) -> Result<FileReadyInfo, FileReadyWaitError> {
    options.validate()?;

    let path = path.as_ref().to_path_buf();
    let deadline = Instant::now() + Duration::from_millis(options.timeout_ms);
    let check_interval = Duration::from_millis(options.check_interval_ms);
    let mut stability = SizeStability::default();

    loop {
        let last_state =
            match check_file_ready(&path, options.required_stable_checks, &mut stability).await {
                CheckResult::Ready(info) => return Ok(info),
                CheckResult::Waiting(state) => state,
            };

        let now = Instant::now();
        if now >= deadline {
            return Err(FileReadyWaitError::TimedOut {
                path,
                timeout_ms: options.timeout_ms,
                last_state,
            });
        }

        sleep((deadline - now).min(check_interval)).await;
    }
}

#[derive(Debug, Default)]
struct SizeStability {
    last_size: Option<u64>,
    unchanged_checks: u32,
}

impl SizeStability {
    fn reset(&mut self) {
        self.last_size = None;
        self.unchanged_checks = 0;
    }

    fn update(&mut self, size: u64) -> u32 {
        if self.last_size == Some(size) {
            self.unchanged_checks += 1;
        } else {
            // First sample: no previous size exists, so unchanged checks start at 0.
            self.last_size = Some(size);
            self.unchanged_checks = 0;
        }

        self.unchanged_checks
    }
}

enum CheckResult {
    Ready(FileReadyInfo),
    Waiting(FileReadyState),
}

async fn check_file_ready(
    path: &Path,
    required_stable_checks: u32,
    stability: &mut SizeStability,
) -> CheckResult {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            stability.reset();
            return CheckResult::Waiting(FileReadyState::Missing);
        }
        Err(error) => {
            stability.reset();
            return CheckResult::Waiting(FileReadyState::MetadataError(error.to_string()));
        }
    };

    if !metadata.is_file() {
        stability.reset();
        return CheckResult::Waiting(FileReadyState::NotRegularFile);
    }

    let size = metadata.len();
    if size == 0 {
        stability.reset();
        return CheckResult::Waiting(FileReadyState::EmptyFile);
    }

    let unchanged_checks = stability.update(size);
    match image_opens(path).await {
        Ok(()) => CheckResult::Ready(FileReadyInfo {
            path: path.to_path_buf(),
            size,
            modified_at: metadata.modified().ok(),
        }),
        Err(_) if unchanged_checks < required_stable_checks => {
            CheckResult::Waiting(FileReadyState::SizeNotStable {
                size,
                unchanged_checks,
                required_stable_checks,
            })
        }
        Err(error) => CheckResult::Waiting(FileReadyState::ImageOpenFailed {
            size,
            error_message: error.to_string(),
        }),
    }
}

async fn image_opens(path: &Path) -> Result<(), String> {
    let path = path.to_path_buf();

    task::spawn_blocking(move || {
        image::open(path)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_match_spec() {
        let options = FileReadyOptions::default();

        assert_eq!(options.timeout_ms, 10_000);
        assert_eq!(options.check_interval_ms, 400);
        assert_eq!(options.required_stable_checks, 2);
    }

    #[test]
    fn metadata_error_state_display_includes_error_message() {
        let state = FileReadyState::MetadataError("access denied".to_owned());

        assert_eq!(
            state.to_string(),
            "failed to read file metadata: access denied"
        );
    }

    #[tokio::test]
    async fn valid_image_becomes_ready_when_image_can_be_opened() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("ready.png");
        save_test_image(&path);

        let info = wait_for_file_ready(&path, fast_options())
            .await
            .expect("wait for ready image");

        assert_eq!(info.path, path);
        assert!(info.size > 0);
        assert!(info.modified_at.is_some());
    }

    #[tokio::test]
    async fn valid_image_does_not_wait_for_every_stable_check() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("ready-fast.png");
        save_test_image(&path);

        let info = wait_for_file_ready(
            &path,
            FileReadyOptions {
                timeout_ms: 40,
                check_interval_ms: 10,
                required_stable_checks: 10,
            },
        )
        .await
        .expect("valid image should use image-open fast path");

        assert_eq!(info.path, path);
    }

    #[tokio::test]
    async fn waits_until_image_can_be_opened() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("eventual.png");
        fs::write(&path, b"incomplete").expect("write incomplete image");

        let writer_path = path.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(20)).await;
            save_test_image(&writer_path);
        });

        let info = wait_for_file_ready(
            &path,
            FileReadyOptions {
                timeout_ms: 1_000,
                check_interval_ms: 5,
                required_stable_checks: 1,
            },
        )
        .await
        .expect("wait for rewritten image");

        assert!(info.size > b"incomplete".len() as u64);
    }

    #[tokio::test]
    async fn missing_file_times_out_with_last_state() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("missing.png");

        let error = wait_for_file_ready(&path, tiny_timeout_options())
            .await
            .expect_err("missing file should time out");

        assert_timed_out_with(error, FileReadyState::Missing);
    }

    #[tokio::test]
    async fn empty_file_times_out_with_last_state() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("empty.png");
        fs::write(&path, []).expect("write empty file");

        let error = wait_for_file_ready(&path, tiny_timeout_options())
            .await
            .expect_err("empty file should time out");

        assert_timed_out_with(error, FileReadyState::EmptyFile);
    }

    #[tokio::test]
    async fn invalid_image_times_out_after_stable_size() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("invalid.png");
        fs::write(&path, b"not an image").expect("write invalid image");

        let error = wait_for_file_ready(
            &path,
            FileReadyOptions {
                timeout_ms: 50,
                check_interval_ms: 5,
                required_stable_checks: 1,
            },
        )
        .await
        .expect_err("invalid image should time out");

        match error {
            FileReadyWaitError::TimedOut { last_state, .. } => {
                assert!(matches!(last_state, FileReadyState::ImageOpenFailed { .. }));
            }
            FileReadyWaitError::InvalidOptions(message) => {
                panic!("unexpected invalid options error: {message}");
            }
        }
    }

    #[tokio::test]
    async fn directory_times_out_as_not_regular_file() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");

        let error = wait_for_file_ready(temp_dir.path(), tiny_timeout_options())
            .await
            .expect_err("directory should time out");

        assert_timed_out_with(error, FileReadyState::NotRegularFile);
    }

    #[tokio::test]
    async fn invalid_options_are_rejected() {
        let error = wait_for_file_ready(
            "anything.png",
            FileReadyOptions {
                timeout_ms: 0,
                check_interval_ms: 1,
                required_stable_checks: 1,
            },
        )
        .await
        .expect_err("zero timeout should be rejected");

        assert!(matches!(error, FileReadyWaitError::InvalidOptions(_)));
    }

    fn fast_options() -> FileReadyOptions {
        FileReadyOptions {
            timeout_ms: 1_000,
            check_interval_ms: 5,
            required_stable_checks: 1,
        }
    }

    fn tiny_timeout_options() -> FileReadyOptions {
        FileReadyOptions {
            timeout_ms: 20,
            check_interval_ms: 5,
            required_stable_checks: 1,
        }
    }

    fn save_test_image(path: &Path) {
        let image = image::RgbImage::from_pixel(2, 2, image::Rgb([255, 255, 255]));
        image.save(path).expect("save test image");
    }

    fn assert_timed_out_with(error: FileReadyWaitError, expected_state: FileReadyState) {
        match error {
            FileReadyWaitError::TimedOut { last_state, .. } => {
                assert_eq!(last_state, expected_state);
            }
            FileReadyWaitError::InvalidOptions(message) => {
                panic!("unexpected invalid options error: {message}");
            }
        }
    }
}
