use std::cell::Cell;

use config_store::Theme;

#[derive(Debug, Clone, Copy)]
pub(super) struct Palette {
    pub bg: u32,
    pub surface: u32,
    pub surface_alt: u32,
    pub text: u32,
    pub text_strong: u32,
    pub text_secondary: u32,
    pub text_dim: u32,
    pub text_mute: u32,
    pub text_disabled: u32,
    pub border: u32,
    pub border_strong: u32,
    pub divider: u32,
    pub row_border: u32,
    pub scrollbar_track: u32,
    pub scrollbar_thumb: u32,
    pub accent: u32,
    pub accent_text: u32,
    pub accent_subtle: u32,
    pub accent_border: u32,
    pub accent_text_strong: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
    pub danger_bg: u32,
    pub danger_border: u32,
    pub danger_border_strong: u32,
    pub status_border: u32,
}

pub(super) const LIGHT_PALETTE: Palette = Palette {
    bg: 0xF7F7F2,
    surface: 0xFFFFFF,
    surface_alt: 0xFAFAF7,
    text: 0x242422,
    text_strong: 0x30302C,
    text_secondary: 0x4A4A43,
    text_dim: 0x66665D,
    text_mute: 0x77776D,
    text_disabled: 0x888880,
    border: 0xC9C9C0,
    border_strong: 0xD8D8D0,
    divider: 0xEFEFE8,
    row_border: 0xE1E1DA,
    scrollbar_track: 0xD8D8D0,
    scrollbar_thumb: 0x8D8D84,
    accent: 0x25635A,
    accent_text: 0xFFFFFF,
    accent_subtle: 0xEAF4F1,
    accent_border: 0x73A79D,
    accent_text_strong: 0x1B4A43,
    success: 0x25635A,
    warning: 0x816420,
    danger: 0x92352E,
    danger_bg: 0xFFF7F6,
    danger_border: 0xC98982,
    danger_border_strong: 0x6E2723,
    status_border: 0xB8D6D0,
};

pub(super) const DARK_PALETTE: Palette = Palette {
    bg: 0x1A1A1A,
    surface: 0x252525,
    surface_alt: 0x2D2D2D,
    text: 0xE8E8E0,
    text_strong: 0xF0F0E8,
    text_secondary: 0xC8C8C0,
    text_dim: 0xA0A095,
    text_mute: 0x808075,
    text_disabled: 0x606058,
    border: 0x3D3D3A,
    border_strong: 0x4A4A45,
    divider: 0x303030,
    row_border: 0x3A3A37,
    scrollbar_track: 0x3A3A37,
    scrollbar_thumb: 0x7A7A72,
    accent: 0x4D9E91,
    accent_text: 0x0A1614,
    accent_subtle: 0x2A3D3A,
    accent_border: 0x4D7A72,
    accent_text_strong: 0xB8E0D7,
    success: 0x73C7B7,
    warning: 0xD8B25C,
    danger: 0xE8867E,
    danger_bg: 0x3A2624,
    danger_border: 0x7A4541,
    danger_border_strong: 0xB85F58,
    status_border: 0x4D7A72,
};

thread_local! {
    static ACTIVE_PALETTE: Cell<Palette> = const { Cell::new(LIGHT_PALETTE) };
}

pub(super) fn palette() -> Palette {
    ACTIVE_PALETTE.with(|p| p.get())
}

pub(super) fn set_active_palette(theme: Theme) {
    let p = match theme {
        Theme::Dark => DARK_PALETTE,
        _ => LIGHT_PALETTE,
    };
    ACTIVE_PALETTE.with(|c| c.set(p));
}
