use gpui::prelude::*;
use gpui::{div, px, rgb, Context, FontWeight, IntoElement, Rgba, SharedString};

use history_store::{DecodedKind, ReadHistory, ReadResult, ReadStatus};

use super::buttons::{copy_button, delete_result_button, open_url_button};
use super::palette::palette;
use super::util::display_time;
use super::BiscottiWindow;

pub(super) fn section_title(text: &'static str) -> impl IntoElement {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(palette().text_secondary))
        .child(text)
}

pub(super) fn preview_placeholder(label: &'static str) -> impl IntoElement {
    div()
        .h(px(220.0))
        .w_full()
        .border_1()
        .border_color(rgb(palette().border))
        .bg(rgb(palette().divider))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(palette().text_mute))
        .child(label)
}

pub(super) fn history_row(
    history: &ReadHistory,
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
    let history_id = history.id.clone();
    let row_id: SharedString = format!("history-row-{}", history.id).into();

    div()
        .id(row_id)
        .h_10()
        .px_3()
        .flex()
        .items_center()
        .justify_between()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(background))
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(display_time(history.detected_at)),
        )
        .child(
            div()
                .text_sm()
                .text_color(status_color(history.status))
                .child(read_status_label(history.status)),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.select_history(history_id.clone(), cx);
            cx.notify();
        }))
}

pub(super) fn result_row(
    result: &ReadResult,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let is_url = matches!(result.decoded_kind, DecodedKind::Url);
    let copy_text = result.decoded_text.clone();
    let open_url = result.decoded_text.clone();
    let result_id = result.id.clone();
    let row_id: SharedString = result.id.clone().into();
    let mut actions = div()
        .flex()
        .gap_2()
        .child(copy_button(row_id.clone(), copy_text, cx));

    if is_url {
        actions = actions.child(open_url_button(row_id.clone(), open_url, cx));
    }

    actions = actions.child(delete_result_button(row_id, result_id, cx));

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .border_1()
        .border_color(rgb(palette().border_strong))
        .bg(rgb(palette().surface))
        .p_3()
        .child(div().w_full().text_sm().child(result.decoded_text.clone()))
        .child(actions)
}

pub(super) fn read_status_label(status: ReadStatus) -> &'static str {
    match status {
        ReadStatus::Decoded => "成功",
        ReadStatus::NoCode => "対象なし",
        ReadStatus::Failed => "失敗",
    }
}

pub(super) fn status_color(status: ReadStatus) -> Rgba {
    match status {
        ReadStatus::Decoded => rgb(palette().success),
        ReadStatus::NoCode => rgb(palette().warning),
        ReadStatus::Failed => rgb(palette().danger),
    }
}
