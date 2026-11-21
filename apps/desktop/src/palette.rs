use gpui_kit::component::Theme;
use gpui_kit::{App, Hsla};
use std::rc::Rc;

const CANVAS: &str = "#16161f";
const PANEL: &str = "#1e1e2a";
const CONTROL: &str = "#282836";
const BORDER: &str = "#393948";
const TEXT: &str = "#f5f3fa";
const MUTED_TEXT: &str = "#bdb9cc";
const VIOLET: &str = "#b398fa";
const VIOLET_INK: &str = "#211633";
const SELECTED: &str = "#443466";
const GREEN: &str = "#68efa0";
const RED: &str = "#ff666f";
const RED_INK: &str = "#2e1118";

/// Customize the registered Dark theme, so switching modes always reloads
/// the right palette (including GPUI Kit widgets) and leaves Light untouched.
pub fn install_dark(cx: &mut App) {
    Theme::update(cx, |theme| {
        let mut dark = (*theme.dark_theme).clone();
        dark.name = "Demolition Dark".into();
        let c = &mut dark.colors;
        c.background = Some(CANVAS.into());
        c.sidebar = Some(PANEL.into());
        c.title_bar = Some(PANEL.into());
        c.title_bar_border = Some(BORDER.into());
        c.popover = Some(PANEL.into());
        c.popover_foreground = Some(TEXT.into());
        c.border = Some(BORDER.into());
        c.input = Some("#474759".into());
        c.foreground = Some(TEXT.into());
        c.muted_foreground = Some(MUTED_TEXT.into());
        c.muted = Some("#292b36".into());

        c.button = Some(CONTROL.into());
        c.button_hover = Some("#343447".into());
        c.button_active = Some(SELECTED.into());
        c.button_foreground = Some(TEXT.into());
        c.secondary = c.button.clone();
        c.secondary_hover = c.button_hover.clone();
        c.secondary_active = c.button_active.clone();
        c.secondary_foreground = c.button_foreground.clone();
        c.primary = Some(VIOLET.into());
        c.primary_hover = Some("#c5b0ff".into());
        c.primary_active = Some("#9878e9".into());
        c.primary_foreground = Some(VIOLET_INK.into());
        c.accent = Some(SELECTED.into());
        c.accent_foreground = Some(TEXT.into());
        c.selection = Some(SELECTED.into());
        c.ring = Some(VIOLET.into());
        c.caret = Some(VIOLET.into());
        c.list = Some(PANEL.into());
        c.list_hover = Some(CONTROL.into());
        c.list_active = Some(SELECTED.into());
        c.list_active_border = Some("#8c6bdc".into());
        c.list_highlight_foreground = Some(VIOLET.into());
        c.link = Some(VIOLET.into());

        c.success = Some(GREEN.into());
        c.success_foreground = Some("#12271b".into());
        c.success_hover = Some("#8ef6b6".into());
        c.success_active = Some("#54d287".into());
        c.danger = Some(RED.into());
        c.danger_foreground = Some(RED_INK.into());
        c.danger_hover = Some("#ff858b".into());
        c.danger_active = Some("#ee515e".into());
        theme.dark_theme = Rc::new(dark);
    });
}

pub fn panel_surface(cx: &App) -> Hsla {
    let theme = Theme::global(cx);
    if theme.is_dark() {
        theme.colors.sidebar
    } else {
        theme.colors.background
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_text_and_transport_labels_have_readable_contrast() {
        fn luminance(color: &str) -> f64 {
            let channel = |start| {
                let s =
                    f64::from(u8::from_str_radix(&color[start..start + 2], 16).unwrap()) / 255.0;
                if s <= 0.04045 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5)
        }
        for (text, surface) in [
            (TEXT, CONTROL),
            (TEXT, SELECTED),
            (MUTED_TEXT, PANEL),
            (MUTED_TEXT, CANVAS),
            (VIOLET_INK, VIOLET),
            (VIOLET_INK, "#9878e9"),
            (RED_INK, RED),
            (RED_INK, "#ee515e"),
            (RED, PANEL),
            (GREEN, PANEL),
        ] {
            let a = luminance(text);
            let b = luminance(surface);
            assert!(
                (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5,
                "{text} on {surface}"
            );
        }
    }
}
