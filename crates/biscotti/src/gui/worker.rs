use std::{path::PathBuf, sync::mpsc, thread, time::Duration};

use app_model::{ProcessedRead, ReadPipeline};
use photo_watcher::{PhotoWatcher, PhotoWatcherEvent};

pub(super) const WATCH_WORKER_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) struct WatchWorkerHandle {
    pub stop_sender: mpsc::Sender<()>,
    pub result_receiver: mpsc::Receiver<PipelineUiEvent>,
    pub _thread: thread::JoinHandle<()>,
}

pub(super) enum PipelineUiEvent {
    Detected(PathBuf),
    Processed(anyhow::Result<Option<ProcessedRead>>),
    Stopped(String),
}

pub(super) fn spawn_watch_worker(
    watcher: PhotoWatcher,
    pipeline: ReadPipeline,
) -> WatchWorkerHandle {
    let (stop_sender, stop_receiver) = mpsc::channel();
    let (result_sender, result_receiver) = mpsc::channel();

    let thread = thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = result_sender.send(PipelineUiEvent::Stopped(format!(
                    "読み取りワーカーを開始できませんでした: {error}"
                )));
                return;
            }
        };

        loop {
            match stop_receiver.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }

            match watcher.recv_timeout(WATCH_WORKER_POLL_INTERVAL) {
                Ok(event) => {
                    let result = match event {
                        PhotoWatcherEvent::Created(path) => {
                            if result_sender
                                .send(PipelineUiEvent::Detected(path.clone()))
                                .is_err()
                            {
                                break;
                            }

                            runtime.block_on(
                                pipeline
                                    .process_photo_watcher_event(PhotoWatcherEvent::Created(path)),
                            )
                        }
                        PhotoWatcherEvent::Error(error) => runtime.block_on(
                            pipeline.process_photo_watcher_event(PhotoWatcherEvent::Error(error)),
                        ),
                    };

                    if result_sender
                        .send(PipelineUiEvent::Processed(result))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = result_sender.send(PipelineUiEvent::Stopped(
                        "監視イベントの受信が終了しました".to_owned(),
                    ));
                    break;
                }
            }
        }
    });

    WatchWorkerHandle {
        stop_sender,
        result_receiver,
        _thread: thread,
    }
}
