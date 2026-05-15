use std::path::PathBuf;

use config_store::{HistoryLimit, Theme, WatchMode};
use gpui::prelude::*;
use gpui::{
    div, px, rgb, App, ClickEvent, Context, ElementId, FontWeight, IntoElement, PromptButton,
    PromptLevel, SharedString, Stateful, Window,
};

use super::palette::palette;
use super::{BiscottiWindow, FolderPromptMode, PreprocessingSetting};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ButtonIntent {
    Primary,
    Secondary,
    Danger,
}

pub(super) fn button_chrome(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    intent: ButtonIntent,
) -> Stateful<gpui::Div> {
    let pal = palette();
    let (bg, border, text) = match intent {
        ButtonIntent::Primary => (pal.accent, pal.accent, pal.accent_text),
        ButtonIntent::Secondary => (pal.surface, pal.border, pal.text_strong),
        ButtonIntent::Danger => (pal.danger_bg, pal.danger_border, pal.danger),
    };
    div()
        .id(id)
        .h_9()
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_color(rgb(text))
        .child(label.into())
}

pub(super) fn watch_button(
    label: &'static str,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    button_chrome("watch-toggle", label, ButtonIntent::Primary).on_click(cx.listener(
        |this, _, _, cx| {
            this.toggle_watch(cx);
            cx.notify();
        },
    ))
}

pub(super) fn settings_button(cx: &mut Context<BiscottiWindow>) -> impl IntoElement {
    button_chrome("open-settings", "設定", ButtonIntent::Secondary)
        .w_full()
        .on_click(cx.listener(|this, _, _, cx| {
            this.open_settings();
            cx.notify();
        }))
}

pub(super) fn choose_folder_button(cx: &mut Context<BiscottiWindow>) -> impl IntoElement {
    button_chrome(
        "choose-watch-folder",
        "フォルダーを変更",
        ButtonIntent::Primary,
    )
    .w(px(180.0))
    .on_click(cx.listener(|this, _, _, cx| {
        this.choose_watch_folder(FolderPromptMode::Optional, cx);
        cx.notify();
    }))
}

pub(super) fn close_settings_button(cx: &mut Context<BiscottiWindow>) -> impl IntoElement {
    button_chrome("close-settings", "閉じる", ButtonIntent::Secondary)
        .h_8()
        .px_3()
        .on_click(cx.listener(|this, _, _, cx| {
            this.close_settings();
            cx.notify();
        }))
}

pub(super) fn open_watch_folder_button(
    folder_path: Option<PathBuf>,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let enabled = folder_path.is_some();
    let pal = palette();
    let background = if enabled { pal.surface } else { pal.divider };
    let text_color = if enabled {
        pal.text_strong
    } else {
        pal.text_disabled
    };

    let button = div()
        .id("open-watch-folder")
        .h_9()
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(palette().border))
        .bg(rgb(background))
        .text_color(rgb(text_color))
        .child("監視フォルダーを開く");

    match folder_path {
        Some(path) => button
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_watch_folder(path.clone(), cx);
                cx.notify();
            }))
            .into_any_element(),
        None => button.into_any_element(),
    }
}

pub(super) fn setting_row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
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
        .child(control)
}

pub(super) fn recursive_watch_button(
    enabled: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    toggle_setting_button("recursive-watch", enabled, cx, |this, next, cx| {
        this.set_recursive_watch(next, cx);
    })
}

pub(super) fn auto_start_watch_button(
    enabled: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    toggle_setting_button("auto-start-watch", enabled, cx, |this, next, cx| {
        this.set_auto_start_watch(next, cx);
    })
}

pub(super) fn open_url_confirm_button(
    enabled: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    toggle_setting_button("open-url-confirm", enabled, cx, |this, next, cx| {
        this.set_open_url_after_confirm(next, cx);
    })
}

pub(super) fn force_preprocessing_button(
    setting: PreprocessingSetting,
    enabled: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let id = match setting {
        PreprocessingSetting::Contrast => "force-preprocess-contrast",
        PreprocessingSetting::Brighten => "force-preprocess-brighten",
        PreprocessingSetting::Threshold => "force-preprocess-threshold",
        PreprocessingSetting::ContrastThreshold => "force-preprocess-contrast-threshold",
        PreprocessingSetting::Invert => "force-preprocess-invert",
    };

    toggle_setting_button(id, enabled, cx, move |this, next, cx| {
        this.set_force_preprocessing(setting, next, cx);
    })
}

pub(super) fn preset_selector_u64(
    base_id: &'static str,
    presets: &[u64],
    current: u64,
    cx: &mut Context<BiscottiWindow>,
    update: impl Fn(&mut BiscottiWindow, u64, &mut Context<BiscottiWindow>) + Clone + 'static,
    format_label: impl Fn(u64) -> String,
) -> impl IntoElement {
    let mut row = div().flex().flex_wrap().gap_2();
    for (index, value) in presets.iter().copied().enumerate() {
        let id_text: SharedString = format!("{base_id}-{index}").into();
        let label: SharedString = format_label(value).into();
        let selected = value == current;
        let update_clone = update.clone();
        row = row.child(preset_pill(
            id_text,
            label,
            selected,
            cx.listener(move |this, _, _, cx| {
                update_clone(this, value, cx);
                cx.notify();
            }),
        ));
    }
    row
}

pub(super) fn preset_selector_u32(
    base_id: &'static str,
    presets: &[u32],
    current: u32,
    cx: &mut Context<BiscottiWindow>,
    update: impl Fn(&mut BiscottiWindow, u32, &mut Context<BiscottiWindow>) + Clone + 'static,
    format_label: impl Fn(u32) -> String,
) -> impl IntoElement {
    let mut row = div().flex().flex_wrap().gap_2();
    for (index, value) in presets.iter().copied().enumerate() {
        let id_text: SharedString = format!("{base_id}-{index}").into();
        let label: SharedString = format_label(value).into();
        let selected = value == current;
        let update_clone = update.clone();
        row = row.child(preset_pill(
            id_text,
            label,
            selected,
            cx.listener(move |this, _, _, cx| {
                update_clone(this, value, cx);
                cx.notify();
            }),
        ));
    }
    row
}

pub(super) fn preset_pill(
    id: SharedString,
    label: SharedString,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    let pal = palette();
    let background = if selected { pal.accent } else { pal.surface };
    let text_color = if selected {
        pal.accent_text
    } else {
        pal.text_strong
    };

    div()
        .id(id)
        .h_8()
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(palette().border))
        .bg(rgb(background))
        .text_color(rgb(text_color))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

pub(super) fn extension_toggle_button(
    ext: &'static str,
    enabled: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let id: SharedString = format!("ext-toggle-{ext}").into();
    let label: SharedString = format!(".{ext}").into();
    preset_pill(
        id,
        label,
        enabled,
        cx.listener(move |this, _, _, cx| {
            this.set_extension_enabled(ext, !enabled, cx);
            cx.notify();
        }),
    )
}

pub(super) fn history_limit_button(
    base_id: &'static str,
    label: impl Into<SharedString>,
    limit: HistoryLimit,
    selected: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let id: SharedString = match limit {
        HistoryLimit::Unlimited => SharedString::from(base_id),
        HistoryLimit::Capped { value } => format!("{base_id}-{value}").into(),
    };
    preset_pill(
        id,
        label.into(),
        selected,
        cx.listener(move |this, _, _, cx| {
            this.set_history_limit(limit, cx);
            cx.notify();
        }),
    )
}

pub(super) fn theme_button(
    label: &'static str,
    theme: Theme,
    selected: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let id: SharedString = format!("theme-{}", theme_id_suffix(theme)).into();
    preset_pill(
        id,
        SharedString::from(label),
        selected,
        cx.listener(move |this, _, _, cx| {
            this.set_theme(theme, cx);
            cx.notify();
        }),
    )
}

pub(super) fn theme_id_suffix(theme: Theme) -> &'static str {
    match theme {
        Theme::System => "system",
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

pub(super) fn toggle_setting_button(
    id: &'static str,
    enabled: bool,
    cx: &mut Context<BiscottiWindow>,
    update: impl Fn(&mut BiscottiWindow, bool, &mut Context<BiscottiWindow>) + 'static,
) -> impl IntoElement {
    let label = if enabled { "有効" } else { "無効" };
    let pal = palette();
    let background = if enabled { pal.accent } else { pal.surface };
    let text_color = if enabled {
        pal.accent_text
    } else {
        pal.text_strong
    };

    div()
        .id(id)
        .h_8()
        .w(px(72.0))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(palette().border))
        .bg(rgb(background))
        .text_color(rgb(text_color))
        .child(label)
        .on_click(cx.listener(move |this, _, _, cx| {
            update(this, !enabled, cx);
            cx.notify();
        }))
}

pub(super) fn watch_mode_selector(
    mode: &WatchMode,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .gap_2()
                .child(watch_mode_button(
                    "watch-mode-strict",
                    "Strict",
                    WatchMode::Strict,
                    matches!(mode, WatchMode::Strict),
                    true,
                    cx,
                ))
                .child(watch_mode_button(
                    "watch-mode-compatible",
                    "Compatible",
                    WatchMode::Compatible,
                    matches!(mode, WatchMode::Compatible),
                    false,
                    cx,
                )),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette().text_mute))
                .child("Compatibleは未対応です"),
        )
}

fn watch_mode_button(
    id: &'static str,
    label: &'static str,
    mode: WatchMode,
    selected: bool,
    enabled: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let pal = palette();
    let background = if selected {
        pal.accent
    } else if enabled {
        pal.surface
    } else {
        pal.divider
    };
    let text_color = if selected {
        pal.accent_text
    } else if enabled {
        pal.text_strong
    } else {
        pal.text_disabled
    };

    let button = div()
        .id(id)
        .h_8()
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(palette().border))
        .bg(rgb(background))
        .text_color(rgb(text_color))
        .child(label);

    if enabled {
        button
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_watch_mode(mode, cx);
                cx.notify();
            }))
            .into_any_element()
    } else {
        button.into_any_element()
    }
}

pub(super) fn copy_button(
    row_id: SharedString,
    text: String,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("copy-result-{}", row_id)))
        .h_8()
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(palette().border))
        .bg(rgb(palette().surface))
        .text_color(rgb(palette().text_strong))
        .child("コピー")
        .on_click(cx.listener(move |this, _, _, cx| {
            this.copy_result_text(text.clone(), cx);
            cx.notify();
        }))
}

pub(super) fn open_url_button(
    row_id: SharedString,
    url: String,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("open-url-{}", row_id)))
        .h_8()
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(palette().border))
        .bg(rgb(palette().surface))
        .text_color(rgb(palette().text_strong))
        .child("開く")
        .on_click(cx.listener(move |this, _, window, cx| {
            let url = url.clone();

            if this.model.config.behavior.open_url_after_confirm {
                let answer = window.prompt(
                    PromptLevel::Info,
                    "URLを開きますか？",
                    Some(url.as_str()),
                    &[PromptButton::ok("開く"), PromptButton::cancel("キャンセル")],
                    cx,
                );

                cx.spawn(async move |this, cx| {
                    let message = match answer.await {
                        Ok(0) => None,
                        Ok(_) => Some("URLを開く操作をキャンセルしました".to_owned()),
                        Err(error) => Some(format!("URL確認に失敗しました: {error}")),
                    };

                    let _ = this.update(cx, |this, cx| {
                        if let Some(message) = message {
                            this.push_event_log(message);
                        } else {
                            this.open_result_url(url, cx);
                        }
                        cx.notify();
                    });
                })
                .detach();
            } else {
                this.open_result_url(url, cx);
            }

            cx.notify();
        }))
}

pub(super) fn delete_result_button(
    row_id: SharedString,
    result_id: String,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("delete-result-{}", row_id)))
        .h_8()
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(palette().danger_border))
        .bg(rgb(palette().danger_bg))
        .text_color(rgb(palette().danger))
        .child("削除")
        .on_click(cx.listener(move |_, _, window, cx| {
            let result_id = result_id.clone();
            let answer = window.prompt(
                PromptLevel::Warning,
                "読み取り結果を削除しますか？",
                Some("この読み取り結果だけを削除します。履歴のステータスは変更されません。"),
                &[PromptButton::ok("削除"), PromptButton::cancel("キャンセル")],
                cx,
            );

            cx.spawn(async move |this, cx| {
                let message = match answer.await {
                    Ok(0) => None,
                    Ok(_) => Some("読み取り結果の削除をキャンセルしました".to_owned()),
                    Err(error) => Some(format!("読み取り結果削除の確認に失敗しました: {error}")),
                };

                let _ = this.update(cx, |this, cx| {
                    if let Some(message) = message {
                        this.push_event_log(message);
                    } else {
                        this.delete_result(result_id, cx);
                    }
                    cx.notify();
                });
            })
            .detach();

            cx.notify();
        }))
}

pub(super) fn delete_history_button(cx: &mut Context<BiscottiWindow>) -> impl IntoElement {
    button_chrome("delete-history", "この履歴を削除", ButtonIntent::Danger)
        .w(px(160.0))
        .px_3()
        .on_click(cx.listener(|this, _, window, cx| {
            let Some(history_id) = this.selected_history_id.clone() else {
                return;
            };

            let answer = window.prompt(
                PromptLevel::Warning,
                "読み取り履歴を削除しますか？",
                Some("この履歴に含まれる読み取り結果も削除されます。"),
                &[PromptButton::ok("削除"), PromptButton::cancel("キャンセル")],
                cx,
            );

            cx.spawn(async move |this, cx| {
                let message = match answer.await {
                    Ok(0) => None,
                    Ok(_) => Some("履歴削除をキャンセルしました".to_owned()),
                    Err(error) => Some(format!("履歴削除の確認に失敗しました: {error}")),
                };

                let _ = this.update(cx, |this, cx| {
                    if let Some(message) = message {
                        this.push_event_log(message);
                    } else {
                        this.delete_history(history_id, cx);
                    }
                    cx.notify();
                });
            })
            .detach();

            cx.notify();
        }))
}
