use std::{
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    time::Duration,
};

use anyhow::Context;
use notify::{
    event::{CreateKind, EventKind},
    Event, RecommendedWatcher, RecursiveMode, Watcher,
};

pub const DEFAULT_CHANNEL_BOUND: usize = 256;

#[derive(Debug)]
pub struct PhotoWatcher {
    _watcher: RecommendedWatcher,
    receiver: mpsc::Receiver<PhotoWatcherEvent>,
}

impl PhotoWatcher {
    pub fn start(path: impl AsRef<Path>, options: PhotoWatcherOptions) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let recursive_mode = options.recursive_mode();
        let extensions = Arc::new(options.extensions);
        let channel_bound = options.channel_bound;
        let (sender, receiver) = mpsc::sync_channel(channel_bound);
        let callback_extensions = Arc::clone(&extensions);
        let mut watcher =
            notify::recommended_watcher(move |event_result: notify::Result<Event>| {
                match event_result {
                    Ok(event) => {
                        for path in created_image_paths(&event, &callback_extensions) {
                            // try_send drops events when consumer falls behind by more than
                            // channel_bound; better than unbounded growth under stress.
                            let _ = sender.try_send(PhotoWatcherEvent::Created(path));
                        }
                    }
                    Err(error) => {
                        let _ = sender.try_send(PhotoWatcherEvent::Error(error.to_string()));
                    }
                }
            })
            .context("failed to create photo watcher")?;

        watcher
            .watch(path, recursive_mode)
            .with_context(|| format!("failed to watch folder: {}", path.display()))?;

        Ok(Self {
            _watcher: watcher,
            receiver,
        })
    }

    pub fn try_recv(&self) -> Result<PhotoWatcherEvent, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<PhotoWatcherEvent, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoWatcherOptions {
    pub recursive: bool,
    pub extensions: Vec<String>,
    pub channel_bound: usize,
}

impl PhotoWatcherOptions {
    fn recursive_mode(&self) -> RecursiveMode {
        if self.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        }
    }
}

impl Default for PhotoWatcherOptions {
    fn default() -> Self {
        Self {
            recursive: true,
            extensions: vec!["png".into(), "jpg".into(), "jpeg".into()],
            channel_bound: DEFAULT_CHANNEL_BOUND,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhotoWatcherEvent {
    Created(PathBuf),
    Error(String),
}

pub fn is_supported_image_path(path: &Path, extensions: &[String]) -> bool {
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };

    extensions
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate.trim_start_matches('.')))
}

pub fn created_image_paths(event: &Event, extensions: &[String]) -> Vec<PathBuf> {
    if !is_create_event_kind(&event.kind) {
        return Vec::new();
    }

    event
        .paths
        .iter()
        .filter(|path| is_supported_image_path(path, extensions))
        .cloned()
        .collect()
}

pub fn is_create_event_kind(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::Any | CreateKind::File | CreateKind::Other)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{DataChange, MetadataKind, ModifyKind, RemoveKind};

    #[test]
    fn supported_image_path_matches_extensions_case_insensitively() {
        let extensions = vec!["png".to_owned(), ".jpg".to_owned(), "jpeg".to_owned()];

        assert!(is_supported_image_path(Path::new("a.PNG"), &extensions));
        assert!(is_supported_image_path(Path::new("b.jpg"), &extensions));
        assert!(is_supported_image_path(Path::new("c.JPEG"), &extensions));
        assert!(!is_supported_image_path(Path::new("d.gif"), &extensions));
        assert!(!is_supported_image_path(
            Path::new("no_extension"),
            &extensions
        ));
    }

    #[test]
    fn create_file_event_yields_supported_image_paths() {
        let event = Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![
                PathBuf::from("first.png"),
                PathBuf::from("second.txt"),
                PathBuf::from("third.JPG"),
            ],
            attrs: Default::default(),
        };
        let extensions = vec!["png".to_owned(), "jpg".to_owned(), "jpeg".to_owned()];

        assert_eq!(
            created_image_paths(&event, &extensions),
            [PathBuf::from("first.png"), PathBuf::from("third.JPG")]
        );
    }

    #[test]
    fn create_any_and_other_events_are_supported() {
        assert!(is_create_event_kind(&EventKind::Create(CreateKind::Any)));
        assert!(is_create_event_kind(&EventKind::Create(CreateKind::Other)));
    }

    #[test]
    fn create_folder_event_is_not_a_file_candidate() {
        assert!(!is_create_event_kind(&EventKind::Create(
            CreateKind::Folder
        )));
    }

    #[test]
    fn non_create_events_are_ignored() {
        let extensions = vec!["png".to_owned()];
        let modify_event = Event {
            kind: EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            paths: vec![PathBuf::from("created.png")],
            attrs: Default::default(),
        };
        let metadata_event = Event {
            kind: EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            paths: vec![PathBuf::from("created.png")],
            attrs: Default::default(),
        };
        let remove_event = Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![PathBuf::from("created.png")],
            attrs: Default::default(),
        };

        assert!(created_image_paths(&modify_event, &extensions).is_empty());
        assert!(created_image_paths(&metadata_event, &extensions).is_empty());
        assert!(created_image_paths(&remove_event, &extensions).is_empty());
    }

    #[test]
    fn options_default_matches_spec() {
        let options = PhotoWatcherOptions::default();

        assert!(options.recursive);
        assert_eq!(options.extensions, ["png", "jpg", "jpeg"]);
        assert_eq!(options.recursive_mode(), RecursiveMode::Recursive);
    }

    #[test]
    fn watcher_can_start_on_existing_directory_without_emitting_existing_files() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        std::fs::write(temp_dir.path().join("existing.png"), b"not a real image")
            .expect("write existing file");

        let watcher = PhotoWatcher::start(temp_dir.path(), PhotoWatcherOptions::default())
            .expect("watch dir");

        assert!(matches!(watcher.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}
