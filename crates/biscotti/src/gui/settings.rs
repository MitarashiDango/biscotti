use gpui::prelude::*;
use gpui::{div, rgb, Context, FontWeight, IntoElement};

use super::palette::palette;
use super::BiscottiWindow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsCategory {
    WatchFolder,
    WatchSettings,
    AppBehavior,
    ImagePreprocessing,
    Advanced,
    HistoryLimit,
    EventLog,
    About,
}

impl SettingsCategory {
    pub(super) const ALL: [SettingsCategory; 8] = [
        SettingsCategory::WatchFolder,
        SettingsCategory::WatchSettings,
        SettingsCategory::AppBehavior,
        SettingsCategory::ImagePreprocessing,
        SettingsCategory::Advanced,
        SettingsCategory::HistoryLimit,
        SettingsCategory::EventLog,
        SettingsCategory::About,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            SettingsCategory::WatchFolder => "監視フォルダー",
            SettingsCategory::WatchSettings => "監視設定",
            SettingsCategory::AppBehavior => "アプリ動作",
            SettingsCategory::ImagePreprocessing => "画像前処理",
            SettingsCategory::Advanced => "高度な設定",
            SettingsCategory::HistoryLimit => "履歴",
            SettingsCategory::EventLog => "イベントログ",
            SettingsCategory::About => "アプリ情報",
        }
    }

    pub(super) fn id(self) -> &'static str {
        match self {
            SettingsCategory::WatchFolder => "settings-cat-folder",
            SettingsCategory::WatchSettings => "settings-cat-watch",
            SettingsCategory::AppBehavior => "settings-cat-behavior",
            SettingsCategory::ImagePreprocessing => "settings-cat-preprocess",
            SettingsCategory::Advanced => "settings-cat-advanced",
            SettingsCategory::HistoryLimit => "settings-cat-history",
            SettingsCategory::EventLog => "settings-cat-event-log",
            SettingsCategory::About => "settings-cat-about",
        }
    }
}

pub(super) fn settings_category_row(
    category: SettingsCategory,
    selected: bool,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let pal = palette();
    let background = if selected {
        pal.accent_subtle
    } else {
        pal.surface_alt
    };
    let border = if selected {
        pal.accent_border
    } else {
        pal.row_border
    };

    div()
        .id(category.id())
        .h_10()
        .px_3()
        .flex()
        .items_center()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(background))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(pal.text_strong))
        .child(category.label())
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_settings_category(category);
            cx.notify();
        }))
}
