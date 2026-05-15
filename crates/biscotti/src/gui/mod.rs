use std::{collections::VecDeque, path::PathBuf, sync::mpsc, time::Duration};

use app_model::{AppModel, ReadPipeline, WatchState};
use config_store::{
    app_data_dir, default_config_path, load_or_recover_default, save_config_atomic, HistoryLimit,
    LoadOutcome, Theme, WatchMode,
};
use gpui::prelude::*;
use gpui::{
    div, img, point, px, rgb, size, App, Application, Bounds, Context, FontWeight,
    ImgResourceLoader, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    Render, Resource, ScrollHandle, SharedString, Task, Timer, TitlebarOptions, Window,
    WindowBounds, WindowOptions,
};
use history_store::{HistoryStore, ReadHistory, ReadResult, ReadStatus};
use photo_watcher::{PhotoWatcher, PhotoWatcherOptions, DEFAULT_CHANNEL_BOUND};

mod buttons;
mod event_log;
mod palette;
mod scrollbar;
mod settings;
mod toast;
mod util;
mod widgets;
mod worker;

use buttons::{
    auto_start_watch_button, choose_folder_button, close_settings_button, delete_history_button,
    extension_toggle_button, force_preprocessing_button, history_limit_button,
    open_url_confirm_button, open_watch_folder_button, preset_selector_u32, preset_selector_u64,
    recursive_watch_button, setting_row, settings_button, theme_button, watch_button,
    watch_mode_selector,
};
use event_log::{event_log_row, EventLogEntry};
use palette::{palette, set_active_palette};
use scrollbar::{vertical_scrollbar, ScrollbarDragState, ScrollbarTarget, SCROLLBAR_MIN_THUMB};
use settings::{settings_category_row, SettingsCategory};
use toast::{Toast, ToastLevel};
use util::{format_local_time, format_ms, looks_like_image};
use widgets::{history_row, preview_placeholder, result_row, section_title};
use worker::{spawn_watch_worker, PipelineUiEvent, WatchWorkerHandle, WATCH_WORKER_POLL_INTERVAL};

const HISTORY_DB_FILE_NAME: &str = "history.sqlite3";
const EVENT_LOG_LIMIT: usize = 200;
const TOAST_DURATION: Duration = Duration::from_millis(2000);
const APP_NAME: &str = env!("BISCOTTI_APP_DISPLAY_NAME");
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_LICENSE: &str = env!("CARGO_PKG_LICENSE");
const APP_REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const BUILD_UNIX_SECONDS: &str = env!("BISCOTTI_BUILD_UNIX_SECONDS");

pub fn run() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);

        let open_result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("Biscotti")),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(BiscottiWindow::load),
        );

        if let Err(error) = open_result {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("Biscotti")
                .set_description(format!("ウィンドウを開けませんでした: {error}"))
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            cx.quit();
            return;
        }

        cx.activate(true);
    });
}

struct BiscottiWindow {
    model: AppModel,
    history_store: Option<HistoryStore>,
    histories: Vec<ReadHistory>,
    selected_history_id: Option<String>,
    selected_results: Vec<ReadResult>,
    watch_worker: Option<WatchWorkerHandle>,
    watch_poll_task: Option<Task<()>>,
    active_read_path: Option<PathBuf>,
    progress_tick: u8,
    event_logs: VecDeque<EventLogEntry>,
    settings_open: bool,
    selected_settings_category: SettingsCategory,
    history_scroll_handle: ScrollHandle,
    detail_scroll_handle: ScrollHandle,
    settings_nav_scroll_handle: ScrollHandle,
    settings_content_scroll_handle: ScrollHandle,
    event_log_scroll_handle: ScrollHandle,
    toast: Option<Toast>,
    reload_generation: u64,
    dragging_scrollbar: Option<ScrollbarDragState>,
    cached_preview_path: Option<PathBuf>,
}

impl BiscottiWindow {
    fn load(cx: &mut Context<Self>) -> Self {
        let mut initial_event_messages = Vec::new();
        let is_first_launch = default_config_path()
            .ok()
            .map(|path| !path.exists())
            .unwrap_or(false);
        let config = match load_or_recover_default() {
            Ok((config, LoadOutcome::Loaded)) => config,
            Ok((config, LoadOutcome::CreatedDefault)) => config,
            Ok((config, LoadOutcome::RecoveredFromCorrupted { backup_path })) => {
                initial_event_messages.push(format!(
                    "設定ファイルが破損していたためデフォルトで起動しました。元のファイルは {} に退避しました",
                    backup_path.display()
                ));
                config
            }
            Err(error) => {
                initial_event_messages.push(format!("設定を読み込めませんでした: {error}"));
                Default::default()
            }
        };
        let history_db_path = default_history_db_path().unwrap_or_else(|error| {
            initial_event_messages.push(format!("履歴DBの保存先を解決できませんでした: {error}"));
            PathBuf::from(HISTORY_DB_FILE_NAME)
        });
        let history_store = match HistoryStore::open(&history_db_path) {
            Ok(store) => {
                store.set_history_limit(config.history.limit.capped());
                // Trim any existing rows that exceed the cap.
                let _ = store.enforce_history_limit();
                Some(store)
            }
            Err(error) => {
                initial_event_messages.push(format!("履歴DBを開けませんでした: {error}"));
                None
            }
        };
        let _ = history_db_path; // referenced earlier when opening; no longer stored in window
        let mut window = Self {
            model: AppModel {
                config,
                watch_state: WatchState::Stopped,
            },
            history_store,
            histories: Vec::new(),
            selected_history_id: None,
            selected_results: Vec::new(),
            watch_worker: None,
            watch_poll_task: None,
            active_read_path: None,
            progress_tick: 0,
            event_logs: VecDeque::new(),
            settings_open: false,
            selected_settings_category: SettingsCategory::WatchFolder,
            history_scroll_handle: ScrollHandle::new(),
            detail_scroll_handle: ScrollHandle::new(),
            settings_nav_scroll_handle: ScrollHandle::new(),
            settings_content_scroll_handle: ScrollHandle::new(),
            event_log_scroll_handle: ScrollHandle::new(),
            toast: None,
            reload_generation: 0,
            dragging_scrollbar: None,
            cached_preview_path: None,
        };

        for message in initial_event_messages {
            window.push_event_log(message);
        }

        cx.on_app_quit(|this, _| {
            this.stop_watch_silent();
            async move {}
        })
        .detach();

        window.reload_histories(cx);

        let folder_setup_needed = match window.model.config.watch.folder_path.as_ref() {
            None => true,
            Some(path) => !path.is_dir(),
        };

        if folder_setup_needed {
            if is_first_launch {
                window.push_event_log("初回起動: 監視フォルダーを選択してください");
            } else {
                window.push_event_log("監視フォルダーが見つかりません: 再選択してください");
            }
            window.choose_watch_folder(FolderPromptMode::Required, cx);
        } else if window.model.config.behavior.auto_start_watch {
            window.start_watch(cx);
        }
        window
    }

    fn reload_histories(&mut self, cx: &mut Context<Self>) {
        self.reload_generation = self.reload_generation.wrapping_add(1);
        let generation = self.reload_generation;
        let Some(store) = self.history_store.clone() else {
            self.push_event_log("履歴DBが利用できないため履歴を読み込めません");
            return;
        };
        let preserved_selected = self.selected_history_id.clone();
        let query_limit = self.model.config.history.limit.as_query_limit();

        cx.spawn(async move |this, cx| {
            let histories_result = cx
                .background_executor()
                .spawn(async move { store.list_histories(query_limit) })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.reload_generation != generation {
                    return;
                }

                match histories_result {
                    Ok(histories) => {
                        let next_selected = if preserved_selected
                            .as_ref()
                            .is_some_and(|id| histories.iter().any(|history| history.id == *id))
                        {
                            preserved_selected
                        } else {
                            histories.first().map(|history| history.id.clone())
                        };

                        this.histories = histories;
                        this.selected_history_id = next_selected;
                        this.refresh_preview_cache(cx);
                        this.reload_selected_results(cx);
                    }
                    Err(error) => {
                        this.push_event_log(format!("履歴を読み込めませんでした: {error}"));
                    }
                }

                cx.notify();
            });
        })
        .detach();
    }

    fn reload_selected_results(&mut self, cx: &mut Context<Self>) {
        let Some(history_id) = self.selected_history_id.clone() else {
            self.selected_results.clear();
            return;
        };
        let Some(store) = self.history_store.clone() else {
            self.selected_results.clear();
            return;
        };
        let captured_id = history_id.clone();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.list_results(&history_id) })
                .await;

            let _ = this.update(cx, |this, cx| {
                if this.selected_history_id.as_deref() != Some(&captured_id) {
                    return;
                }
                match result {
                    Ok(results) => this.selected_results = results,
                    Err(error) => {
                        this.selected_results.clear();
                        this.push_event_log(format!("読み取り結果を読み込めませんでした: {error}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn push_event_log(&mut self, message: impl Into<String>) {
        self.event_logs.push_front(EventLogEntry {
            occurred_at: chrono::Local::now().timestamp(),
            message: message.into(),
        });

        if self.event_logs.len() > EVENT_LOG_LIMIT {
            self.event_logs.truncate(EVENT_LOG_LIMIT);
        }
    }

    fn push_toast(
        &mut self,
        message: impl Into<String>,
        level: ToastLevel,
        cx: &mut Context<Self>,
    ) {
        let dismiss_task = cx.spawn(async move |this, cx| {
            Timer::after(TOAST_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            });
        });

        self.toast = Some(Toast {
            message: message.into(),
            level,
            _dismiss_task: dismiss_task,
        });
    }

    fn select_history(&mut self, history_id: String, cx: &mut Context<Self>) {
        self.selected_history_id = Some(history_id);
        self.settings_open = false;
        self.refresh_preview_cache(cx);
        self.reload_selected_results(cx);
    }

    fn open_settings(&mut self) {
        self.settings_open = true;
    }

    fn close_settings(&mut self) {
        self.settings_open = false;
    }

    fn scroll_handle_for(&self, target: ScrollbarTarget) -> &ScrollHandle {
        match target {
            ScrollbarTarget::History => &self.history_scroll_handle,
            ScrollbarTarget::Detail => &self.detail_scroll_handle,
            ScrollbarTarget::SettingsNav => &self.settings_nav_scroll_handle,
            ScrollbarTarget::SettingsContent => &self.settings_content_scroll_handle,
            ScrollbarTarget::EventLog => &self.event_log_scroll_handle,
        }
    }

    fn start_scrollbar_drag(
        &mut self,
        target: ScrollbarTarget,
        pointer_y: Pixels,
        cx: &mut Context<Self>,
    ) {
        let handle = self.scroll_handle_for(target);
        let start_offset_y = handle.offset().y;
        self.dragging_scrollbar = Some(ScrollbarDragState {
            target,
            start_pointer_y: pointer_y,
            start_offset_y,
        });
        cx.notify();
    }

    fn update_scrollbar_drag(&mut self, pointer_y: Pixels, cx: &mut Context<Self>) {
        let Some(state) = self.dragging_scrollbar else {
            return;
        };
        let handle = self.scroll_handle_for(state.target);
        let viewport = handle.bounds().size.height;
        let max_offset = handle.max_offset().height;
        if viewport <= px(0.0) || max_offset <= px(0.0) {
            return;
        }

        let min_thumb = px(SCROLLBAR_MIN_THUMB).min(viewport);
        let thumb_height =
            (viewport * (viewport / (viewport + max_offset))).clamp(min_thumb, viewport);
        let thumb_track = viewport - thumb_height;
        if thumb_track <= px(0.0) {
            return;
        }

        let delta = pointer_y - state.start_pointer_y;
        let scale = max_offset / thumb_track;
        let new_offset_y = (state.start_offset_y - delta * scale).clamp(-max_offset, px(0.0));
        let offset = handle.offset();
        handle.set_offset(point(offset.x, new_offset_y));
        cx.notify();
    }

    fn end_scrollbar_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging_scrollbar.take().is_some() {
            cx.notify();
        }
    }

    fn preview_path_for_selection(&self) -> Option<PathBuf> {
        let selected_id = self.selected_history_id.as_deref()?;
        self.histories
            .iter()
            .find(|history| history.id == selected_id)
            .map(|history| history.source_path.clone())
    }

    fn refresh_preview_cache(&mut self, cx: &mut Context<Self>) {
        let new_path = self.preview_path_for_selection();
        self.change_preview_path(new_path, cx);
    }

    fn change_preview_path(&mut self, new_path: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.cached_preview_path == new_path {
            return;
        }
        if let Some(old_path) = self.cached_preview_path.take() {
            cx.remove_asset::<ImgResourceLoader>(&Resource::Path(old_path.into()));
        }
        self.cached_preview_path = new_path;
    }

    fn select_settings_category(&mut self, category: SettingsCategory) {
        if self.selected_settings_category == category {
            return;
        }

        self.selected_settings_category = category;
        self.settings_content_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    fn open_watch_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let display = path.display().to_string();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { open::that(&path) })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        let message = format!("監視フォルダーを開きました: {display}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Info, cx);
                    }
                    Err(error) => {
                        let message = format!("監視フォルダーを開けませんでした: {error}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_watch_folder(&mut self, mode: FolderPromptMode, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let folder = rfd::FileDialog::new()
                .set_title("監視フォルダーを選択")
                .pick_folder();

            match folder {
                Some(folder_path) => {
                    let _ = this.update(cx, |this, cx| {
                        this.apply_watch_folder(folder_path, cx);
                        cx.notify();
                    });
                }
                None => {
                    if matches!(mode, FolderPromptMode::Required) {
                        rfd::MessageDialog::new()
                            .set_level(rfd::MessageLevel::Error)
                            .set_title("Biscotti")
                            .set_description(
                                "監視フォルダーが選択されなかったため、アプリケーションを終了します。",
                            )
                            .set_buttons(rfd::MessageButtons::Ok)
                            .show();

                        let _ = cx.update(|app| app.quit());
                    }
                }
            }
        })
        .detach();
    }

    fn apply_watch_folder(&mut self, folder_path: PathBuf, cx: &mut Context<Self>) {
        let was_running = matches!(self.model.watch_state, WatchState::Running);
        if was_running {
            self.stop_watch_silent();
        }

        self.model.config.watch.folder_path = Some(folder_path.clone());
        let success_message = format!("監視フォルダーを保存しました: {}", folder_path.display());
        self.save_config_in_background(cx, success_message, was_running);
    }

    fn save_config_in_background(
        &self,
        cx: &mut Context<Self>,
        success_message: String,
        should_restart_watch: bool,
    ) {
        let config = self.model.config.clone();
        let config_path = default_config_path();

        cx.spawn(async move |this, cx| {
            let result = match config_path {
                Ok(path) => {
                    cx.background_executor()
                        .spawn(async move { save_config_atomic(&path, &config) })
                        .await
                }
                Err(error) => Err(error),
            };

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.push_event_log(success_message.clone());
                        this.push_toast(success_message, ToastLevel::Info, cx);
                        if should_restart_watch {
                            this.start_watch(cx);
                        }
                    }
                    Err(error) => {
                        let message = format!("設定を保存できませんでした: {error}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_recursive_watch(&mut self, recursive: bool, cx: &mut Context<Self>) {
        if self.model.config.watch.recursive == recursive {
            return;
        }

        self.model.config.watch.recursive = recursive;
        self.save_settings_change(
            cx,
            "再帰監視の設定を保存しました",
            RestartPolicy::RestartIfRunning,
        );
    }

    fn set_watch_mode(&mut self, mode: WatchMode, cx: &mut Context<Self>) {
        if self.model.config.watch.mode == mode {
            return;
        }

        self.model.config.watch.mode = mode;
        self.save_settings_change(
            cx,
            "監視モードの設定を保存しました",
            RestartPolicy::RestartIfRunning,
        );
    }

    fn set_auto_start_watch(&mut self, auto_start: bool, cx: &mut Context<Self>) {
        if self.model.config.behavior.auto_start_watch == auto_start {
            return;
        }

        self.model.config.behavior.auto_start_watch = auto_start;
        self.save_settings_change(
            cx,
            "自動監視開始の設定を保存しました",
            RestartPolicy::NoRestart,
        );
    }

    fn set_open_url_after_confirm(&mut self, confirm: bool, cx: &mut Context<Self>) {
        if self.model.config.behavior.open_url_after_confirm == confirm {
            return;
        }

        self.model.config.behavior.open_url_after_confirm = confirm;
        self.save_settings_change(
            cx,
            "URLを開く前の確認設定を保存しました",
            RestartPolicy::NoRestart,
        );
    }

    fn set_force_preprocessing(
        &mut self,
        setting: PreprocessingSetting,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let preprocessing = &mut self.model.config.decode.preprocessing;
        let value = match setting {
            PreprocessingSetting::Contrast => &mut preprocessing.force_contrast,
            PreprocessingSetting::Brighten => &mut preprocessing.force_brighten,
            PreprocessingSetting::Threshold => &mut preprocessing.force_threshold,
            PreprocessingSetting::ContrastThreshold => &mut preprocessing.force_contrast_threshold,
            PreprocessingSetting::Invert => &mut preprocessing.force_invert,
        };

        if *value == enabled {
            return;
        }

        *value = enabled;
        self.save_settings_change(
            cx,
            "画像前処理の設定を保存しました",
            RestartPolicy::RestartIfRunning,
        );
    }

    fn set_decode_timeout_ms(&mut self, value: u64, cx: &mut Context<Self>) {
        if self.model.config.decode.file_ready_timeout_ms == value {
            return;
        }
        self.model.config.decode.file_ready_timeout_ms = value;
        self.save_settings_change(
            cx,
            "書き込み完了待ちタイムアウトを保存しました",
            RestartPolicy::RestartIfRunning,
        );
    }

    fn set_decode_check_interval_ms(&mut self, value: u64, cx: &mut Context<Self>) {
        if self.model.config.decode.file_ready_check_interval_ms == value {
            return;
        }
        self.model.config.decode.file_ready_check_interval_ms = value;
        self.save_settings_change(
            cx,
            "サイズ安定チェック間隔を保存しました",
            RestartPolicy::RestartIfRunning,
        );
    }

    fn set_decode_stable_checks(&mut self, value: u32, cx: &mut Context<Self>) {
        if self.model.config.decode.required_stable_checks == value {
            return;
        }
        self.model.config.decode.required_stable_checks = value;
        self.save_settings_change(
            cx,
            "サイズ安定確認回数を保存しました",
            RestartPolicy::RestartIfRunning,
        );
    }

    fn set_extension_enabled(&mut self, ext: &'static str, enabled: bool, cx: &mut Context<Self>) {
        let normalized = ext.to_ascii_lowercase();
        let current = self
            .model
            .config
            .decode
            .extensions
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&normalized));
        if current == enabled {
            return;
        }

        if enabled {
            self.model.config.decode.extensions.push(normalized);
        } else {
            self.model
                .config
                .decode
                .extensions
                .retain(|e| !e.eq_ignore_ascii_case(&normalized));
        }

        self.save_settings_change(
            cx,
            "対象拡張子の設定を保存しました",
            RestartPolicy::RestartIfRunning,
        );
    }

    fn set_history_limit(&mut self, limit: HistoryLimit, cx: &mut Context<Self>) {
        if self.model.config.history.limit == limit {
            return;
        }
        self.model.config.history.limit = limit;

        if let Some(store) = self.history_store.clone() {
            store.set_history_limit(limit.capped());
            // Apply the cap immediately in the background so the UI never blocks on the DELETE.
            cx.background_executor()
                .spawn(async move {
                    let _ = store.enforce_history_limit();
                })
                .detach();
        }

        self.save_settings_change(cx, "履歴件数上限を保存しました", RestartPolicy::NoRestart);
        self.reload_histories(cx);
    }

    fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.model.config.ui.theme == theme {
            return;
        }
        self.model.config.ui.theme = theme;
        self.save_settings_change(cx, "テーマを保存しました", RestartPolicy::NoRestart);
    }

    fn copy_result_text(&mut self, text: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.push_event_log("クリップボードにコピーしました");
                        this.push_toast("クリップボードにコピーしました", ToastLevel::Info, cx);
                    }
                    Err(error) => {
                        let message = format!("コピーできませんでした: {error}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_result_url(&mut self, url: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let target = url.clone();
            let result = cx
                .background_executor()
                .spawn(async move { open::that(&target) })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        let message = format!("URLを開きました: {url}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Info, cx);
                    }
                    Err(error) => {
                        let message = format!("URLを開けませんでした: {error}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_result(&mut self, result_id: String, cx: &mut Context<Self>) {
        let Some(store) = self.history_store.clone() else {
            self.push_event_log("履歴DBが利用できないため削除できません");
            return;
        };

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.delete_result(&result_id) })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(deleted) => {
                        let message = if deleted == 0 {
                            "読み取り結果は既に削除されています".to_owned()
                        } else {
                            "読み取り結果を削除しました".to_owned()
                        };
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Info, cx);
                        this.reload_selected_results(cx);
                    }
                    Err(error) => {
                        let message = format!("読み取り結果を削除できませんでした: {error}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete_history(&mut self, history_id: String, cx: &mut Context<Self>) {
        let Some(store) = self.history_store.clone() else {
            self.push_event_log("履歴DBが利用できないため削除できません");
            return;
        };

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { store.delete_history(&history_id) })
                .await;

            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(deleted) => {
                        this.selected_history_id = None;
                        this.selected_results.clear();
                        this.change_preview_path(None, cx);
                        let message = if deleted == 0 {
                            "読み取り履歴は既に削除されています".to_owned()
                        } else {
                            "読み取り履歴を削除しました".to_owned()
                        };
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Info, cx);
                        this.reload_histories(cx);
                    }
                    Err(error) => {
                        let message = format!("読み取り履歴を削除できませんでした: {error}");
                        this.push_event_log(message.clone());
                        this.push_toast(message, ToastLevel::Error, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_settings_change(
        &mut self,
        cx: &mut Context<Self>,
        success_message: &'static str,
        restart_policy: RestartPolicy,
    ) {
        let should_restart = matches!(self.model.watch_state, WatchState::Running)
            && matches!(restart_policy, RestartPolicy::RestartIfRunning);

        if should_restart {
            self.stop_watch_silent();
        }

        self.save_config_in_background(cx, success_message.to_owned(), should_restart);
    }

    fn toggle_watch(&mut self, cx: &mut Context<Self>) {
        match self.model.watch_state {
            WatchState::Stopped => self.start_watch(cx),
            WatchState::Running => self.stop_watch(),
        }
    }

    fn start_watch(&mut self, cx: &mut Context<Self>) {
        self.stop_watch_silent();

        let Some(folder_path) = self.model.config.watch.folder_path.clone() else {
            self.push_event_log("監視フォルダーが未設定です");
            return;
        };
        let Some(store) = self.history_store.clone() else {
            self.push_event_log("履歴DBが利用できないため監視を開始できません");
            return;
        };

        let options = PhotoWatcherOptions {
            recursive: self.model.config.watch.recursive,
            extensions: self.model.config.decode.extensions.clone(),
            channel_bound: DEFAULT_CHANNEL_BOUND,
        };

        match PhotoWatcher::start(&folder_path, options) {
            Ok(watcher) => {
                let pipeline = ReadPipeline::from_config(store, &self.model.config);
                self.watch_worker = Some(spawn_watch_worker(watcher, pipeline));
                self.start_watch_result_polling(cx);
                self.model.watch_state = WatchState::Running;
                self.push_event_log(format!("監視中: {}", folder_path.display()));
            }
            Err(error) => {
                self.push_event_log(format!("監視を開始できませんでした: {error}"));
            }
        }
    }

    fn stop_watch(&mut self) {
        self.stop_watch_silent();
        self.push_event_log("監視を停止しました");
    }

    fn stop_watch_silent(&mut self) {
        if let Some(worker) = self.watch_worker.take() {
            let _ = worker.stop_sender.send(());
        }
        self.watch_poll_task = None;
        self.model.watch_state = WatchState::Stopped;
    }

    fn start_watch_result_polling(&mut self, cx: &mut Context<Self>) {
        self.watch_poll_task = Some(cx.spawn(async move |this, cx| loop {
            Timer::after(WATCH_WORKER_POLL_INTERVAL).await;

            let Ok(should_continue) = this.update(cx, |this, cx| {
                let messages = this.drain_watch_worker_messages();
                for message in messages {
                    this.handle_pipeline_ui_event(message, cx);
                }

                if this.active_read_path.is_some() {
                    this.progress_tick = this.progress_tick.wrapping_add(1);
                }

                cx.notify();
                this.watch_worker.is_some()
            }) else {
                break;
            };

            if !should_continue {
                break;
            }
        }));
    }

    fn drain_watch_worker_messages(&mut self) -> Vec<PipelineUiEvent> {
        let Some(worker) = &self.watch_worker else {
            return Vec::new();
        };

        let mut messages = Vec::new();
        loop {
            match worker.result_receiver.try_recv() {
                Ok(message) => messages.push(message),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    messages.push(PipelineUiEvent::Stopped(
                        "監視ワーカーとの通信が終了しました".to_owned(),
                    ));
                    break;
                }
            }
        }

        messages
    }

    fn handle_pipeline_ui_event(&mut self, event: PipelineUiEvent, cx: &mut Context<Self>) {
        match event {
            PipelineUiEvent::Detected(path) => {
                self.active_read_path = Some(path.clone());
                self.progress_tick = 0;
                self.push_event_log(format!("画像作成を検知: {}", path.display()));
            }
            PipelineUiEvent::Processed(Ok(Some(processed))) => {
                self.active_read_path = None;
                self.progress_tick = 0;
                self.selected_history_id = Some(processed.history_id.clone());
                for attempt in processed.decode_attempts.iter().skip(1) {
                    self.push_event_log(format!(
                        "QRデコード fallback を実行: {} ({})",
                        attempt,
                        processed.source_path.display()
                    ));
                }
                let message = match processed.status {
                    ReadStatus::Decoded => format!(
                        "読み取り完了: {} ({}件)",
                        processed.source_path.display(),
                        processed.result_count
                    ),
                    ReadStatus::NoCode => {
                        format!("対象コードなし: {}", processed.source_path.display())
                    }
                    ReadStatus::Failed => {
                        format!("読み取り失敗: {}", processed.source_path.display())
                    }
                };
                self.push_event_log(message);
                self.reload_histories(cx);
            }
            PipelineUiEvent::Processed(Ok(None)) => {
                self.active_read_path = None;
                self.progress_tick = 0;
            }
            PipelineUiEvent::Processed(Err(error)) => {
                self.active_read_path = None;
                self.progress_tick = 0;
                self.push_event_log(format!("読み取り処理に失敗しました: {error}"));
            }
            PipelineUiEvent::Stopped(message) => {
                self.active_read_path = None;
                self.progress_tick = 0;
                self.watch_worker = None;
                self.watch_poll_task = None;
                self.model.watch_state = WatchState::Stopped;
                self.push_event_log(message);
            }
        }
    }

    fn selected_history(&self) -> Option<&ReadHistory> {
        let selected_id = self.selected_history_id.as_deref()?;
        self.histories
            .iter()
            .find(|history| history.id == selected_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartPolicy {
    NoRestart,
    RestartIfRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FolderPromptMode {
    Optional,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreprocessingSetting {
    Contrast,
    Brighten,
    Threshold,
    ContrastThreshold,
    Invert,
}

impl Render for BiscottiWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        set_active_palette(self.model.config.ui.theme);

        div()
            .id("biscotti-root")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(palette().bg))
            .text_color(rgb(palette().text))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.dragging_scrollbar.is_some() {
                    this.update_scrollbar_drag(event.position.y, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.end_scrollbar_drag(cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.end_scrollbar_drag(cx);
                }),
            )
            .child(self.render_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(rgb(palette().border_strong))
                    .child(if self.settings_open {
                        self.render_settings_pane(cx).into_any_element()
                    } else {
                        div()
                            .flex()
                            .size_full()
                            .child(self.render_history_pane(cx))
                            .child(self.render_detail_pane(cx))
                            .into_any_element()
                    }),
            )
            .child(self.render_status_bar())
            .child(self.render_toast())
    }
}

impl BiscottiWindow {
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let watch_label = match self.model.watch_state {
            WatchState::Stopped => "監視開始",
            WatchState::Running => "監視停止",
        };
        let folder_path = self.model.config.watch.folder_path.clone();

        div()
            .flex()
            .items_center()
            .justify_between()
            .h_16()
            .px_5()
            .bg(rgb(palette().surface))
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Biscotti"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(open_watch_folder_button(folder_path, cx))
                    .child(watch_button(watch_label, cx)),
            )
    }

    fn render_history_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap_2();

        if self.histories.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(palette().text_mute))
                    .child("履歴はまだありません"),
            );
        }

        for history in self.histories.iter() {
            let selected = self
                .selected_history_id
                .as_ref()
                .is_some_and(|selected_id| selected_id == &history.id);
            list = list.child(history_row(history, selected, cx));
        }

        div()
            .w(px(280.0))
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(rgb(palette().surface))
            .border_r_1()
            .border_color(rgb(palette().border_strong))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(section_title("読み取り履歴"))
                    .child(
                        div()
                            .id("history-scroll-shell")
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .id("history-scroll")
                                    .size_full()
                                    .pr(px(16.0))
                                    .overflow_y_scroll()
                                    .track_scroll(&self.history_scroll_handle)
                                    .child(list),
                            )
                            .child(vertical_scrollbar(
                                "history-scrollbar",
                                ScrollbarTarget::History,
                                &self.history_scroll_handle,
                                cx,
                            )),
                    ),
            )
            .child(div().p_4().child(settings_button(cx)))
    }

    fn render_detail_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .pr(px(28.0))
            .child(self.render_preview_section())
            .child(section_title("読み取りデータ"))
            .child(self.render_results(cx));

        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .child(
                div()
                    .id("detail-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.detail_scroll_handle)
                    .child(content),
            )
            .child(vertical_scrollbar(
                "detail-scrollbar",
                ScrollbarTarget::Detail,
                &self.detail_scroll_handle,
                cx,
            ))
    }

    fn render_settings_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .child(self.render_settings_nav(cx))
            .child(self.render_settings_content(cx))
    }

    fn render_settings_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap_2();

        for category in SettingsCategory::ALL {
            let selected = self.selected_settings_category == category;
            list = list.child(settings_category_row(category, selected, cx));
        }

        div()
            .w(px(280.0))
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(rgb(palette().surface))
            .border_r_1()
            .border_color(rgb(palette().border_strong))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(section_title("設定"))
                    .child(
                        div()
                            .id("settings-nav-scroll-shell")
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .id("settings-nav-scroll")
                                    .size_full()
                                    .pr(px(16.0))
                                    .overflow_y_scroll()
                                    .track_scroll(&self.settings_nav_scroll_handle)
                                    .child(list),
                            )
                            .child(vertical_scrollbar(
                                "settings-nav-scrollbar",
                                ScrollbarTarget::SettingsNav,
                                &self.settings_nav_scroll_handle,
                                cx,
                            )),
                    ),
            )
            .child(div().p_4().child(close_settings_button(cx)))
    }

    fn render_settings_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let category = self.selected_settings_category;
        let label = category.label();

        let (inner, needs_scroll) = match category {
            SettingsCategory::WatchFolder => (
                self.render_settings_watch_folder(cx).into_any_element(),
                true,
            ),
            SettingsCategory::WatchSettings => {
                (self.render_settings_watch(cx).into_any_element(), true)
            }
            SettingsCategory::AppBehavior => (
                self.render_settings_app_behavior(cx).into_any_element(),
                true,
            ),
            SettingsCategory::ImagePreprocessing => (
                self.render_settings_preprocessing(cx).into_any_element(),
                true,
            ),
            SettingsCategory::Advanced => {
                (self.render_settings_advanced(cx).into_any_element(), true)
            }
            SettingsCategory::HistoryLimit => {
                (self.render_settings_history(cx).into_any_element(), true)
            }
            SettingsCategory::EventLog => (self.render_event_log(cx).into_any_element(), false),
            SettingsCategory::About => (self.render_settings_about(cx).into_any_element(), true),
        };

        if needs_scroll {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .relative()
                .child(
                    div()
                        .id("settings-content-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.settings_content_scroll_handle)
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .p_5()
                                .pr(px(28.0))
                                .child(section_title(label))
                                .child(inner),
                        ),
                )
                .child(vertical_scrollbar(
                    "settings-content-scrollbar",
                    ScrollbarTarget::SettingsContent,
                    &self.settings_content_scroll_handle,
                    cx,
                ))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .flex_col()
                .gap_4()
                .p_5()
                .child(section_title(label))
                .child(inner)
                .into_any_element()
        }
    }

    fn render_settings_watch_folder(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let folder_label = self
            .model
            .config
            .watch
            .folder_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "未設定".to_owned());

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .w_full()
                    .border_1()
                    .border_color(rgb(palette().border_strong))
                    .bg(rgb(palette().surface))
                    .p_3()
                    .text_sm()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(folder_label),
            )
            .child(choose_folder_button(cx))
    }

    fn render_settings_watch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(setting_row(
                "再帰監視",
                recursive_watch_button(self.model.config.watch.recursive, cx),
            ))
            .child(setting_row(
                "監視モード",
                watch_mode_selector(&self.model.config.watch.mode, cx),
            ))
            .child(setting_row(
                "起動時に自動で開始",
                auto_start_watch_button(self.model.config.behavior.auto_start_watch, cx),
            ))
    }

    fn render_settings_app_behavior(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current_theme = self.model.config.ui.theme;
        let theme_row = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .child(theme_button(
                "システムに従う",
                Theme::System,
                current_theme == Theme::System,
                cx,
            ))
            .child(theme_button(
                "ライト",
                Theme::Light,
                current_theme == Theme::Light,
                cx,
            ))
            .child(theme_button(
                "ダーク",
                Theme::Dark,
                current_theme == Theme::Dark,
                cx,
            ));

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(setting_row(
                "URLを開く前に確認",
                open_url_confirm_button(self.model.config.behavior.open_url_after_confirm, cx),
            ))
            .child(section_title("テーマ"))
            .child(theme_row)
    }

    fn render_settings_preprocessing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let preprocessing = &self.model.config.decode.preprocessing;

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(palette().text_dim))
                    .child(
                        "通常はQRデコードが失敗した時のみフォールバック試行される画像前処理を、最初から強制適用します。",
                    ),
            )
            .child(setting_row(
                "コントラスト強調を強制適用",
                force_preprocessing_button(
                    PreprocessingSetting::Contrast,
                    preprocessing.force_contrast,
                    cx,
                ),
            ))
            .child(setting_row(
                "明るさ補正を強制適用",
                force_preprocessing_button(
                    PreprocessingSetting::Brighten,
                    preprocessing.force_brighten,
                    cx,
                ),
            ))
            .child(setting_row(
                "二値化を強制適用",
                force_preprocessing_button(
                    PreprocessingSetting::Threshold,
                    preprocessing.force_threshold,
                    cx,
                ),
            ))
            .child(setting_row(
                "コントラスト強調 + 二値化を強制適用",
                force_preprocessing_button(
                    PreprocessingSetting::ContrastThreshold,
                    preprocessing.force_contrast_threshold,
                    cx,
                ),
            ))
            .child(setting_row(
                "反転を強制適用",
                force_preprocessing_button(
                    PreprocessingSetting::Invert,
                    preprocessing.force_invert,
                    cx,
                ),
            ))
    }

    fn render_settings_advanced(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let decode = &self.model.config.decode;
        let timeout_presets: &[u64] = &[3_000, 5_000, 10_000, 20_000, 30_000, 60_000];
        let interval_presets: &[u64] = &[100, 200, 400, 800, 1_500];
        let stable_presets: &[u32] = &[1, 2, 3, 5];

        let common_extensions: &[&'static str] = &["png", "jpg", "jpeg"];

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(palette().text_dim))
                    .child(
                        "ファイル書き出し完了判定とデコード対象拡張子の詳細設定です。プリセットから選択できます。",
                    ),
            )
            .child(section_title("書き込み完了待ちタイムアウト"))
            .child(preset_selector_u64(
                "advanced-timeout",
                timeout_presets,
                decode.file_ready_timeout_ms,
                cx,
                |this, value, cx| this.set_decode_timeout_ms(value, cx),
                format_ms,
            ))
            .child(section_title("サイズ安定チェック間隔"))
            .child(preset_selector_u64(
                "advanced-interval",
                interval_presets,
                decode.file_ready_check_interval_ms,
                cx,
                |this, value, cx| this.set_decode_check_interval_ms(value, cx),
                format_ms,
            ))
            .child(section_title("サイズ安定確認回数"))
            .child(preset_selector_u32(
                "advanced-stable-checks",
                stable_presets,
                decode.required_stable_checks,
                cx,
                |this, value, cx| this.set_decode_stable_checks(value, cx),
                |value| format!("{value} 回"),
            ))
            .child(section_title("対象拡張子"))
            .child(self.render_extension_toggles(common_extensions, cx))
    }

    fn render_extension_toggles(
        &self,
        extensions: &[&'static str],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut row = div().flex().flex_wrap().gap_2();
        for ext in extensions {
            let enabled = self
                .model
                .config
                .decode
                .extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext));
            row = row.child(extension_toggle_button(ext, enabled, cx));
        }
        row
    }

    fn render_settings_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.model.config.history.limit;
        let capped_presets: &[u32] = &[100, 500, 1_000, 5_000];

        let unlimited_selected = matches!(current, HistoryLimit::Unlimited);
        let unlimited_button = history_limit_button(
            "history-limit-unlimited",
            "無制限",
            HistoryLimit::Unlimited,
            unlimited_selected,
            cx,
        );

        let mut capped_row = div().flex().flex_wrap().gap_2().child(unlimited_button);
        for value in capped_presets {
            let limit = HistoryLimit::Capped { value: *value };
            let selected = current == limit;
            capped_row = capped_row.child(history_limit_button(
                "history-limit-cap",
                format!("{value} 件"),
                limit,
                selected,
                cx,
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(palette().text_dim))
                    .child(
                        "SQLite内に保持する読み取り履歴件数の上限です。上限を超えた古い履歴は順次削除されます。",
                    ),
            )
            .child(section_title("履歴件数上限"))
            .child(capped_row)
    }

    fn render_settings_about(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(info_row("アプリ名", APP_NAME))
            .child(info_row("バージョン", APP_VERSION))
            .child(info_row("ライセンス", APP_LICENSE))
            .child(info_row("ビルド日時", display_build_time()))
            .child(repository_info_row(cx))
    }

    fn render_preview_section(&self) -> impl IntoElement {
        let Some(history) = self.selected_history() else {
            return div()
                .flex()
                .flex_col()
                .gap_4()
                .child(section_title("画像プレビュー"))
                .child(preview_placeholder("履歴を選択してください"))
                .into_any_element();
        };

        if !looks_like_image(&history.source_path) {
            return div().into_any_element();
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(section_title("画像プレビュー"))
            .child(
                div()
                    .id("image-preview")
                    .h(px(260.0))
                    .w_full()
                    .border_1()
                    .border_color(rgb(palette().border))
                    .bg(rgb(palette().divider))
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_2()
                    .child(
                        img(history.source_path.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.selected_history().is_none() {
            return div()
                .text_color(rgb(palette().text_mute))
                .child("履歴を選択してください")
                .into_any_element();
        }

        let delete_history = div().pt_2().child(delete_history_button(cx));

        if self.selected_results.is_empty() {
            return div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_color(rgb(palette().text_mute))
                        .child("読み取りデータはありません"),
                )
                .child(delete_history)
                .into_any_element();
        }

        let mut list = div().flex().flex_col().gap_3();

        for result in &self.selected_results {
            list = list.child(result_row(result, cx));
        }

        list = list.child(delete_history);

        list.into_any_element()
    }

    fn render_event_log(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap_2().pr(px(16.0));

        if self.event_logs.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(rgb(palette().text_mute))
                    .child("イベントはまだありません"),
            );
        }

        for event in &self.event_logs {
            list = list.child(event_log_row(event));
        }

        div()
            .id("event-log-shell")
            .relative()
            .flex_1()
            .min_h(px(140.0))
            .w_full()
            .border_1()
            .border_color(rgb(palette().border_strong))
            .bg(rgb(palette().surface))
            .child(
                div()
                    .id("event-log-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.event_log_scroll_handle)
                    .child(list),
            )
            .child(vertical_scrollbar(
                "event-log-scrollbar",
                ScrollbarTarget::EventLog,
                &self.event_log_scroll_handle,
                cx,
            ))
            .into_any_element()
    }

    fn render_status_bar(&self) -> impl IntoElement {
        let Some(path) = self.active_read_path.as_ref() else {
            return div().into_any_element();
        };

        let file_name = path
            .file_name()
            .map(|file_name| file_name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let active_index = usize::from(self.progress_tick % 3);
        let mut progress = div().flex().items_center().gap_1().flex_none();

        let pal = palette();
        for index in 0..3 {
            let color = if index == active_index {
                pal.accent
            } else {
                pal.border
            };
            progress = progress.child(div().w(px(18.0)).h(px(3.0)).bg(rgb(color)));
        }

        div()
            .id("status-bar")
            .w_full()
            .h_8()
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_t_1()
            .border_color(rgb(palette().status_border))
            .bg(rgb(palette().accent_subtle))
            .text_sm()
            .text_color(rgb(palette().accent))
            .child(div().flex_none().child("読み取り中"))
            .child(progress)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(palette().text_strong))
                    .child(file_name),
            )
            .into_any_element()
    }

    fn render_toast(&self) -> impl IntoElement {
        toast::render_toast(self.toast.as_ref())
    }
}

fn info_row(label: &'static str, value: impl Into<String>) -> impl IntoElement {
    let value = value.into();

    div()
        .w_full()
        .min_h(px(44.0))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .border_1()
        .border_color(rgb(palette().border_strong))
        .bg(rgb(palette().surface))
        .p_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(palette().text_strong))
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .text_sm()
                .text_color(rgb(palette().text_dim))
                .text_ellipsis()
                .overflow_hidden()
                .child(value),
        )
}

fn repository_info_row(cx: &mut Context<BiscottiWindow>) -> impl IntoElement {
    let link_url = APP_REPOSITORY.to_owned();
    let button_url = APP_REPOSITORY.to_owned();

    div()
        .w_full()
        .min_h(px(44.0))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .border_1()
        .border_color(rgb(palette().border_strong))
        .bg(rgb(palette().surface))
        .p_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(palette().text_strong))
                .child("リポジトリーURL"),
        )
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_1()
                .items_center()
                .justify_end()
                .gap_2()
                .child(
                    div()
                        .id("repository-url-link")
                        .min_w_0()
                        .text_sm()
                        .text_color(rgb(palette().accent))
                        .text_ellipsis()
                        .overflow_hidden()
                        .child(APP_REPOSITORY)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_result_url(link_url.clone(), cx);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("open-repository-url")
                        .h_8()
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_1()
                        .border_color(rgb(palette().border))
                        .bg(rgb(palette().surface))
                        .text_sm()
                        .text_color(rgb(palette().text_strong))
                        .child("開く")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_result_url(button_url.clone(), cx);
                            cx.notify();
                        })),
                ),
        )
}

fn display_build_time() -> String {
    BUILD_UNIX_SECONDS
        .parse::<i64>()
        .ok()
        .map(|timestamp| format_local_time(timestamp, "%Y/%m/%d %H:%M:%S", "不明"))
        .unwrap_or_else(|| "不明".to_owned())
}

fn default_history_db_path() -> anyhow::Result<PathBuf> {
    Ok(app_data_dir()?.join(HISTORY_DB_FILE_NAME))
}

#[cfg(test)]
mod tests {
    use super::buttons::theme_id_suffix;
    use super::util::format_local_time;
    use super::widgets::read_status_label;
    use super::*;
    use std::io::Write;

    #[test]
    fn read_status_label_returns_localized_strings() {
        assert_eq!(read_status_label(ReadStatus::Decoded), "成功");
        assert_eq!(read_status_label(ReadStatus::NoCode), "対象なし");
        assert_eq!(read_status_label(ReadStatus::Failed), "失敗");
    }

    #[test]
    fn format_ms_uses_seconds_for_round_values() {
        assert_eq!(format_ms(1000), "1 秒");
        assert_eq!(format_ms(5000), "5 秒");
        assert_eq!(format_ms(60_000), "60 秒");
    }

    #[test]
    fn format_ms_uses_milliseconds_for_non_round_values() {
        assert_eq!(format_ms(100), "100 ms");
        assert_eq!(format_ms(400), "400 ms");
        assert_eq!(format_ms(1500), "1500 ms");
    }

    #[test]
    fn format_local_time_returns_fallback_on_invalid_timestamp() {
        let out = format_local_time(i64::MIN, "%H:%M", "--");
        assert_eq!(out, "--");
    }

    #[test]
    fn format_local_time_formats_normal_timestamps() {
        // 2024-01-01 00:00:00 UTC = 1704067200. The Local format will vary by TZ,
        // so just verify the function produces a non-fallback string of expected shape.
        let out = format_local_time(1_704_067_200, "%Y", "fallback");
        assert!(
            out == "2023" || out == "2024",
            "unexpected year output: {out}"
        );
    }

    #[test]
    fn theme_id_suffix_returns_distinct_strings() {
        assert_eq!(theme_id_suffix(Theme::System), "system");
        assert_eq!(theme_id_suffix(Theme::Light), "light");
        assert_eq!(theme_id_suffix(Theme::Dark), "dark");
    }

    #[test]
    fn looks_like_image_rejects_missing_files() {
        let path = std::env::temp_dir().join("biscotti-test-nonexistent.png");
        let _ = std::fs::remove_file(&path);
        assert!(!looks_like_image(&path));
    }

    #[test]
    fn looks_like_image_rejects_empty_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.png");
        std::fs::File::create(&path).expect("create empty file");
        assert!(!looks_like_image(&path));
    }

    #[test]
    fn looks_like_image_rejects_unsupported_extensions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("data.txt");
        let mut file = std::fs::File::create(&path).expect("create text file");
        file.write_all(b"some text").expect("write");
        assert!(!looks_like_image(&path));
    }

    #[test]
    fn looks_like_image_accepts_supported_extensions_case_insensitively() {
        let dir = tempfile::tempdir().expect("tempdir");
        for ext in ["png", "PNG", "jpg", "JPG", "jpeg", "JPEG"] {
            let path = dir.path().join(format!("image.{ext}"));
            let mut file = std::fs::File::create(&path).expect("create file");
            file.write_all(b"placeholder").expect("write");
            assert!(looks_like_image(&path), "expected supported: {ext}");
        }
    }

    #[test]
    fn settings_category_all_includes_every_variant() {
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::WatchFolder));
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::WatchSettings));
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::AppBehavior));
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::ImagePreprocessing));
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::Advanced));
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::HistoryLimit));
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::EventLog));
        assert!(SettingsCategory::ALL.contains(&SettingsCategory::About));
    }

    #[test]
    fn display_build_time_returns_non_empty_string() {
        assert!(!display_build_time().is_empty());
    }

    #[test]
    fn active_palette_switches_with_theme() {
        set_active_palette(Theme::Light);
        let light = palette();
        set_active_palette(Theme::Dark);
        let dark = palette();
        assert_ne!(light.bg, dark.bg);
        assert_ne!(light.text, dark.text);
        // Reset to light for other tests
        set_active_palette(Theme::Light);
    }

    #[test]
    fn active_palette_falls_back_to_light_for_system() {
        set_active_palette(Theme::System);
        let system = palette();
        assert_eq!(system.bg, super::palette::LIGHT_PALETTE.bg);
    }
}
