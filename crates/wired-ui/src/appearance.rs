//! Maps `wired-config` settings to the window and terminal view.

use gpui::{Hsla, Rgba, WindowBackgroundAppearance};
use wired_config::{Backdrop, MouseAction, gesture};
use wired_terminal_view::{FontConfig, Ligatures, OnExit, RightClick, ViewColors, ViewOptions};
use wired_vt::Rgb;

use crate::{rgb, settings::Settings, title_bar::BarColors};

/// Settings resolved for rendering.
#[derive(Debug, Clone)]
pub struct Appearance {
    pub font: FontConfig,
    pub view: ViewOptions,
    pub backdrop: WindowBackgroundAppearance,
    /// The Mica of `backdrop` is to be switched to the system acrylic.
    pub is_system_acrylic: bool,
    /// Terminal background color with the configured opacity.
    pub background: Hsla,
    /// Space between two panes, filled by the divider line, in logical pixels.
    pub divider_width: f32,
    /// Terminal text color, which overlay scrollbars are drawn in (at their own alpha).
    pub foreground: Hsla,
    /// Title bar colors.
    pub bar: BarColors,
    /// Whether the title bar shows the reload button.
    pub show_reload_button: bool,
}

impl Appearance {
    /// `available_fonts`: installed and bundled family names, to pick the first usable family.
    pub fn from_settings(settings: &Settings, available_fonts: &[String]) -> Self {
        let config = settings.config();
        let colors = settings.colors();
        let right_click = match settings.mouse(gesture::RIGHT_CLICK) {
            MouseAction::CopyOrPaste => RightClick::CopyOrPaste,
            MouseAction::Paste => RightClick::Paste,
            _ => RightClick::Nothing,
        };
        let (family, fallbacks) =
            crate::fonts::choose_family(&config.font.family, &config.font.fallbacks, available_fonts);
        let opacity = config.window.opacity;
        let backdrop = match config.window.backdrop {
            Backdrop::Blur => WindowBackgroundAppearance::Blurred,
            // gpui has no system acrylic; the workspace switches the Mica it sets up to it.
            Backdrop::Mica | Backdrop::Acrylic => WindowBackgroundAppearance::MicaBackdrop,
            Backdrop::MicaAlt => WindowBackgroundAppearance::MicaAltBackdrop,
            Backdrop::None if opacity < 1. => WindowBackgroundAppearance::Transparent,
            Backdrop::None => WindowBackgroundAppearance::Opaque,
        };
        Self {
            font: FontConfig {
                family: family.into(),
                fallbacks,
                size: config.font.size,
                ligatures: match config.font.ligatures {
                    wired_config::Ligatures::On => Ligatures::On,
                    wired_config::Ligatures::Off => Ligatures::Off,
                    wired_config::Ligatures::BreakAtCursor => Ligatures::BreakAtCursor,
                },
                line_height: config.font.line_height,
                weight: f32::from(config.font.weight),
                bold_weight: f32::from(config.font.bold_weight),
                cell_width_adjust: config.font.cell_width_adjust,
            },
            view: ViewOptions {
                copy_on_select: config.terminal.copy_on_select,
                cursor_blink: config.appearance.cursor_blink,
                scrollbar: config.terminal.scrollbar == wired_config::ScrollbarVisibility::Overlay,
                hide_pointer_while_typing: config.terminal.hide_pointer_while_typing,
                on_exit: match config.terminal.on_exit {
                    wired_config::OnExit::Auto => OnExit::Auto,
                    wired_config::OnExit::Close => OnExit::Close,
                    wired_config::OnExit::Keep => OnExit::Keep,
                },
                inactive_text_opacity: config.pane.inactive_text_opacity,
                show_inactive_cursor: config.pane.show_inactive_cursor,
                right_click,
                bell_visual: config.terminal.bell_visual,
                colors: ViewColors {
                    cursor_text: colors.cursor_text.map(rgb),
                    selection_background: colors.selection_background.map(rgb),
                    selection_foreground: colors.selection_foreground.map(rgb),
                },
            },
            backdrop,
            is_system_acrylic: config.window.backdrop == Backdrop::Acrylic,
            background: to_hsla(rgb(colors.background), opacity),
            divider_width: config.pane.divider_width,
            foreground: to_hsla(rgb(colors.foreground), 1.),
            bar: BarColors::new(hex_to_hsla(&config.colors.title_bar)),
            show_reload_button: config.window.show_reload_button,
        }
    }
}

fn hex_to_hsla(value: &str) -> Hsla {
    to_hsla(rgb(wired_config::parse_hex_color(value).unwrap_or((0, 0, 0))), 1.)
}

fn to_hsla(c: Rgb, alpha: f32) -> Hsla {
    let mut color: Hsla =
        Rgba { r: f32::from(c.r) / 255., g: f32::from(c.g) / 255., b: f32::from(c.b) / 255., a: 1. }.into();
    color.a = alpha;
    color
}
