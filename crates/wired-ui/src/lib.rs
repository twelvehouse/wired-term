//! Window contents of wired: title bar, tabs, split panes of terminal views, and the settings
//! and key bindings they follow.
#![forbid(unsafe_code)]

mod appearance;
mod fonts;
mod overlay;
mod profile_icon;
mod settings;
mod title_bar;
mod workspace;

use std::path::PathBuf;

use anyhow::Context as _;
use gpui::{
    AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
    component::{Theme, ThemeMode},
    hsla, px, size,
};
use wired_config::{ColorScheme, Profile};
use wired_core::TerminalConfig;
use wired_pty::{PtyDimensions, SpawnConfig};
use wired_vt::VtOptions;

use crate::{
    appearance::Appearance,
    settings::{LoadReport, Settings},
    workspace::{MovedTab, Workspace},
};

/// Sets up the UI (theme, bundled fonts, settings and key bindings) and opens the first window.
pub fn start(cx: &mut gpui::App) -> anyhow::Result<()> {
    gpui::init(cx);
    Theme::change(ThemeMode::Dark, None, cx);
    // The component Root paints the theme background over the whole window; make it
    // transparent so the backdrop shows through.
    let theme = Theme::global_mut(cx);
    let clear = hsla(0., 0., 0., 0.);
    theme.tokens.background.color = clear;
    theme.tokens.background.background = clear.into();
    theme.background = clear;

    fonts::load_bundled(cx);
    cx.bind_keys(wired_terminal_view::passthrough_key_bindings());
    let report = Settings::load(cx);
    // Process-wide and read when glyphs are rasterized: set once, before the first frame (glyphs
    // already in the atlas would keep the old rasterization). Changing it needs a restart.
    let font = &Settings::get(cx).config().font;
    wired_platform::set_antialiasing(font.antialiasing, font.gamma, font.contrast);
    open_window(Some(report), cx)
}

fn terminal_config(profile: &Profile, vt: VtOptions) -> TerminalConfig {
    TerminalConfig {
        spawn: SpawnConfig {
            program: profile.program.clone(),
            args: profile.args.clone(),
            cwd: std::env::var_os("USERPROFILE").map(PathBuf::from),
            env: profile.env.clone(),
        },
        size: PtyDimensions { cols: 120, rows: 30, ..PtyDimensions::default() },
        vt,
    }
}

fn vt_options(settings: &Settings) -> VtOptions {
    VtOptions {
        scrollback_lines: settings.config().terminal.scrollback_lines,
        colors: default_colors(settings.colors()),
    }
}

fn rgb((r, g, b): wired_config::Rgb) -> wired_vt::Rgb {
    wired_vt::Rgb::new(r, g, b)
}

/// Like Ghostty, entries 16..255 are not derived from the theme's first 16.
fn default_colors(scheme: &ColorScheme) -> wired_vt::DefaultColors {
    let mut palette = wired_vt::default_palette();
    for (&index, &color) in &scheme.palette {
        palette[usize::from(index)] = rgb(color);
    }
    wired_vt::DefaultColors {
        foreground: rgb(scheme.foreground),
        background: rgb(scheme.background),
        cursor: scheme.cursor.map(rgb),
        palette,
    }
}

/// gpui hides the pointer by itself for keys that type text or run an action; the terminal view
/// covers the other keys. Both follow the one setting.
fn apply_pointer_hiding(hides_while_typing: bool, cx: &mut gpui::App) {
    cx.set_cursor_hide_mode(if hides_while_typing {
        gpui::CursorHideMode::OnTypingAndAction
    } else {
        gpui::CursorHideMode::Never
    });
}

fn open_window(report: Option<LoadReport>, cx: &mut gpui::App) -> anyhow::Result<()> {
    let bounds = Bounds::centered(None, size(px(1000.), px(620.)), cx);
    let (_, workspace) = open_workspace(&mut None, bounds, None, cx)?;
    if let Some(report) = &report {
        workspace.update(cx, |workspace, cx| workspace.show_report(report, cx));
    }
    Ok(())
}

/// `moved` is taken only once the window exists, so it stays with the caller on failure.
fn open_workspace(
    moved: &mut Option<MovedTab>,
    bounds: Bounds<gpui::Pixels>,
    display: Option<gpui::DisplayId>,
    cx: &mut gpui::App,
) -> anyhow::Result<(gpui::AnyWindowHandle, gpui::Entity<Workspace>)> {
    let settings = Settings::get(cx);
    let vt_options = vt_options(settings);
    let appearance = Appearance::from_settings(settings, &cx.text_system().all_font_names());
    apply_pointer_hiding(settings.config().terminal.hide_pointer_while_typing, cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("wired".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_background: appearance.backdrop,
        display_id: display,
        ..Default::default()
    };
    gpui::open_window(options, cx, move |window, cx| {
        cx.new(|cx| Workspace::new(vt_options, appearance, moved.take(), window, cx))
    })
    .context("open window")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_palette_entries_replace_only_their_own_slots() {
        let (theme, _) =
            wired_config::parse_theme("palette = 1=#ff757f\npalette = 255=#000001\ncursor-color = #010203\n");
        let colors = default_colors(&ColorScheme::from_theme(&theme));
        let builtin = wired_vt::default_palette();
        assert_eq!(colors.palette[1], wired_vt::Rgb::new(0xff, 0x75, 0x7f));
        assert_eq!(colors.palette[255], wired_vt::Rgb::new(0, 0, 1));
        assert_eq!(colors.palette[0], builtin[0]);
        assert_eq!(colors.palette[16..255], builtin[16..255]);
        assert_eq!(colors.cursor, Some(wired_vt::Rgb::new(1, 2, 3)));
        assert_eq!(default_colors(&ColorScheme::default()), wired_vt::DefaultColors::default());
    }
}
