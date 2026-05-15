use gpui::prelude::*;
use gpui::{div, px, rgb, IntoElement};

use super::palette::palette;
use super::util::display_log_time;

pub(super) struct EventLogEntry {
    pub occurred_at: i64,
    pub message: String,
}

pub(super) fn event_log_row(event: &EventLogEntry) -> impl IntoElement {
    let pal = palette();
    div()
        .px_3()
        .py_2()
        .flex()
        .items_start()
        .gap_3()
        .border_b_1()
        .border_color(rgb(pal.divider))
        .child(
            div()
                .w(px(92.0))
                .flex_none()
                .text_xs()
                .text_color(rgb(pal.text_mute))
                .child(display_log_time(event.occurred_at)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .child(event.message.clone()),
        )
}
