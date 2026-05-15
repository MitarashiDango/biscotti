use gpui::prelude::*;
use gpui::{div, px, rgb, IntoElement, Task};

use super::palette::palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastLevel {
    Info,
    Error,
}

pub(super) struct Toast {
    pub message: String,
    pub level: ToastLevel,
    pub _dismiss_task: Task<()>,
}

pub(super) fn render_toast(toast: Option<&Toast>) -> impl IntoElement {
    let Some(toast) = toast else {
        return div().into_any_element();
    };

    let pal = palette();
    let (background, border, text_color) = match toast.level {
        ToastLevel::Info => (pal.accent, pal.accent_text_strong, pal.accent_text),
        ToastLevel::Error => (pal.danger, pal.danger_border_strong, pal.accent_text),
    };

    div()
        .absolute()
        .bottom_4()
        .right_4()
        .max_w(px(420.0))
        .px_4()
        .py_3()
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(background))
        .text_color(rgb(text_color))
        .text_sm()
        .child(toast.message.clone())
        .into_any_element()
}
