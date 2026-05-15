use gpui::prelude::*;
use gpui::{div, px, rgb, Context, IntoElement, MouseButton, MouseDownEvent, Pixels, ScrollHandle};

use super::palette::palette;
use super::BiscottiWindow;

pub(super) const SCROLLBAR_THICKNESS: f32 = 8.0;
pub(super) const SCROLLBAR_MIN_THUMB: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollbarTarget {
    History,
    Detail,
    SettingsNav,
    SettingsContent,
    EventLog,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ScrollbarDragState {
    pub target: ScrollbarTarget,
    pub start_pointer_y: Pixels,
    pub start_offset_y: Pixels,
}

pub(super) fn vertical_scrollbar(
    id: &'static str,
    target: ScrollbarTarget,
    handle: &ScrollHandle,
    cx: &mut Context<BiscottiWindow>,
) -> impl IntoElement {
    let bounds = handle.bounds();
    let viewport = bounds.size.height;
    let max_offset = handle.max_offset().height;

    if viewport <= px(0.0) || max_offset <= px(0.0) {
        return div().id(id).into_any_element();
    }

    let min_thumb = px(SCROLLBAR_MIN_THUMB).min(viewport);
    let thumb_height = (viewport * (viewport / (viewport + max_offset))).clamp(min_thumb, viewport);
    let progress = (-handle.offset().y / max_offset).clamp(0.0, 1.0);
    let thumb_top = (viewport - thumb_height) * progress;

    div()
        .id(id)
        .absolute()
        .top(px(0.0))
        .right_1()
        .bottom(px(0.0))
        .w(px(SCROLLBAR_THICKNESS))
        .bg(rgb(palette().scrollbar_track))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                this.start_scrollbar_drag(target, event.position.y, cx);
            }),
        )
        .child(
            div()
                .absolute()
                .top(thumb_top)
                .right(px(0.0))
                .h(thumb_height)
                .w(px(SCROLLBAR_THICKNESS))
                .bg(rgb(palette().scrollbar_thumb)),
        )
        .into_any_element()
}
