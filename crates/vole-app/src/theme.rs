use gpui::*;
use gpui_kit::component::theme::{Theme, ThemeMode};

pub const WORKSPACE: u32 = 0x151c26;
pub const SURFACE: u32 = 0x202b3b;
pub const DIVIDER: u32 = 0x405169;
pub const INK: u32 = 0xe3eaf3;
pub const MUTED: u32 = 0xb2b9c4;
pub const READ: u32 = 0x9cc8f4;
pub const WRITE: u32 = 0xe4b48b;
pub const ERROR: u32 = 0xefb0b2;
/// C syntax colors. Read/Write stay reserved for keywords and literals the
/// machine consumes; the rest are quiet tints that keep 4.5:1 on Workspace.
pub const SYNTAX_TYPE: u32 = 0x93d0c8;
pub const SYNTAX_STRING: u32 = 0xc0d69a;
pub const SYNTAX_COMMENT: u32 = 0x8f9bad;
pub const SYNTAX_PREPROCESSOR: u32 = 0xc8aee6;
pub const MONO: &str = "IBM Plex Mono";
pub const SANS: &str = "IBM Plex Sans";

pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.font_family = SANS.into();
        theme.mono_font_family = MONO.into();
        theme.font_size = px(14.);
        theme.mono_font_size = px(14.);
        theme.radius = px(6.);
        theme.background = rgb(WORKSPACE).into();
        theme.foreground = rgb(INK).into();
        theme.border = rgb(DIVIDER).into();
        theme.input = rgb(SURFACE).into();
        theme.muted = rgb(SURFACE).into();
        theme.muted_foreground = rgb(MUTED).into();
        theme.primary = rgb(READ).into();
        theme.primary_foreground = rgb(WORKSPACE).into();
        theme.button = rgb(SURFACE).into();
        theme.button_foreground = rgb(INK).into();
        theme.button_hover = rgb(0x2b3b51).into();
        theme.button_active = rgb(0x34465d).into();
        theme.button_primary = rgb(READ).into();
        theme.button_primary_foreground = rgb(WORKSPACE).into();
        theme.button_primary_hover = rgb(0xafd4f8).into();
        theme.button_primary_active = rgb(0x80b2e3).into();
        theme.ring = rgb(READ).into();
        theme.caret = rgb(READ).into();
        theme.selection = rgba(0x9cc8f445).into();
        theme.popover = rgb(SURFACE).into();
        theme.popover_foreground = rgb(INK).into();
        theme.colors.list = rgb(WORKSPACE).into();
        theme.list_active = rgb(SURFACE).into();
        theme.list_active_border = rgb(READ).into();
        theme.scrollbar_thumb = rgb(DIVIDER).into();
        theme.scrollbar_thumb_hover = rgb(READ).into();
        theme.shadow = false;
    });
}

pub fn pane() -> Div {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(rgb(WORKSPACE))
        .text_color(rgb(INK))
}
pub fn heading(title: impl Into<SharedString>, detail: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .flex_none()
        .h(px(52.))
        .px(px(20.))
        .gap(px(12.))
        .border_b_1()
        .border_color(rgb(DIVIDER))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .child(detail.into()),
        )
}
pub fn note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text.into())
}
pub fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).child(text.into())
}
