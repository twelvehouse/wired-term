//! Configuration: `<install dir>\config\config.toml`, TOML, all keys optional.
//!
//! The file is untrusted input: unknown keys are ignored, out-of-range values
//! are clamped, and a broken file never panics; callers keep the previous config on error.
#![forbid(unsafe_code)]

mod keybind;
mod profile;
mod template;
mod theme;

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;

pub use crate::keybind::{
    ACTIONS, ActionSpec, Bindings, KeyContext, MOUSE, MouseAction, MouseSpec, add_missing_bindings, gesture,
    keybind_path, load_bindings, write_keybind_template_if_missing,
};
pub use crate::profile::{DEFAULT_ORDER, Profile, ProfileIcon, ProfileId, default_profile};
pub use crate::theme::{
    ColorScheme, DEFAULT_BACKGROUND, DEFAULT_FOREGROUND, MAX_THEME_BYTES, Rgb, Theme, ThemeError, load_theme,
    parse_theme, theme_dirs,
};

/// Errors from reading the config file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file exists but could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// File path.
        path: PathBuf,
        /// I/O error.
        source: std::io::Error,
    },
    /// The file is not valid TOML or has values of the wrong type.
    #[error("{path}{}: {message}", at_line(*.line_column))]
    Parse {
        /// File path.
        path: PathBuf,
        /// Line and column (1-based) the parser stopped at, when it says.
        line_column: Option<(usize, usize)>,
        /// What is wrong, on one line.
        message: String,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

fn at_line(line_column: Option<(usize, usize)>) -> String {
    line_column.map(|(line, column)| format!(", line {line}, column {column}")).unwrap_or_default()
}

/// A parse error of `text` (the contents of `path`) with the line and column it points at.
pub(crate) fn parse_error(path: &Path, text: &str, error: &toml::de::Error) -> Error {
    let line_column = error.span().map(|span| line_column(text, span.start));
    let message = error.message().split('\n').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("; ");
    Error::Parse { path: path.to_path_buf(), line_column, message }
}

/// 1-based line and column (in characters) of byte `offset` in `text`.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let before = &text[..end];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    (before.matches('\n').count() + 1, before[line_start..].chars().count() + 1)
}

/// Ligature mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Ligatures {
    /// Always form ligatures.
    On,
    /// Never form ligatures.
    Off,
    /// Form ligatures except across the cursor.
    #[default]
    BreakAtCursor,
}

/// How glyph coverage is turned into pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Antialiasing {
    /// Skia (Chromium) style, as in xterm.js-based terminals such as Tabby.
    #[default]
    Skia,
    /// gpui's DirectWrite rendering (Windows Terminal style correction).
    System,
}

/// Window backdrop material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Backdrop {
    /// No material: plain transparency when `opacity < 1`, opaque otherwise.
    None,
    /// Blur of what is behind the window, without evening out its brightness.
    #[default]
    Blur,
    /// The Windows 11 acrylic: blur that also evens out the brightness behind the window.
    Acrylic,
    /// Mica.
    Mica,
    /// Mica Alt.
    MicaAlt,
}

/// `[general]`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GeneralSettings {
    /// Profile of the first tab and of new tabs; unset = the first installed of PowerShell 7,
    /// Windows PowerShell, Command Prompt.
    pub default_profile: Option<ProfileId>,
}

/// `[font]`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FontSettings {
    /// Family name.
    pub family: String,
    /// Size in logical pixels.
    pub size: f32,
    /// Ligature mode.
    pub ligatures: Ligatures,
    /// Fallback families.
    pub fallbacks: Vec<String>,
    /// Antialiasing style; applies after restart.
    pub antialiasing: Antialiasing,
    /// Line height as a multiple of the font's natural height.
    pub line_height: f32,
    /// Weight of normal text (100..900).
    #[serde(deserialize_with = "weight")]
    pub weight: u16,
    /// Weight of bold text (100..900).
    #[serde(deserialize_with = "weight")]
    pub bold_weight: u16,
    /// Added to the cell width, in logical pixels.
    pub cell_width_adjust: f32,
    /// `skia` only: blending gamma (1.0..3.0); unset = system DirectWrite value. Applies after
    /// restart.
    pub gamma: Option<f32>,
    /// `skia` only: edge contrast (0..1), higher = bolder; unset = system DirectWrite value.
    /// Applies after restart.
    pub contrast: Option<f32>,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            family: "UDEV Gothic 35NFLG".into(),
            size: 12.,
            ligatures: Ligatures::default(),
            fallbacks: vec!["UDEV Gothic NFLG".into(), "Cascadia Mono".into(), "Consolas".into()],
            antialiasing: Antialiasing::default(),
            line_height: 0.9,
            weight: 400,
            bold_weight: 700,
            cell_width_adjust: 0.,
            gamma: None,
            contrast: None,
        }
    }
}

/// `[window]`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WindowSettings {
    /// Opacity of the terminal background over the backdrop (0..1).
    pub opacity: f32,
    /// Backdrop material.
    pub backdrop: Backdrop,
    /// Show the button at the right of the title bar that reloads the settings files.
    pub show_reload_button: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self { opacity: 0.6, backdrop: Backdrop::default(), show_reload_button: false }
    }
}

/// `[colors]`: the color theme and the title bar color.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ColorSettings {
    /// Color theme in Ghostty's format: a file name in `themes` next to `config.toml`, else in
    /// `themes` next to `wired.exe` (the iTerm2-Color-Schemes collection). A theme that cannot be
    /// loaded leaves the built-in colors.
    pub theme: Option<String>,
    /// Title bar color as `#rrggbb` (the active tab takes the terminal background).
    pub title_bar: String,
}

impl Default for ColorSettings {
    fn default() -> Self {
        Self { theme: Some("Lumina Rayleigh Dark".into()), title_bar: "#131313".into() }
    }
}

/// Parses `#rrggbb` into `(r, g, b)`.
pub fn parse_hex_color(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// Scrollbar of the terminal panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ScrollbarVisibility {
    /// Thin overlay that widens when the pointer comes near; hidden without history.
    #[default]
    Overlay,
    /// No scrollbar.
    Never,
}

/// What a pane does when its program exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OnExit {
    /// Close on exit code 0, keep the pane with the code otherwise.
    #[default]
    Auto,
    /// Always close.
    Close,
    /// Always keep the pane, showing the exit code.
    Keep,
}

/// How the application chooses the window icon at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IconPreference {
    /// Choose dark or light from the terminal theme's background.
    #[default]
    Auto,
    /// Use the dark icon.
    Dark,
    /// Use the light icon.
    Light,
}

/// The icon to install on each application window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowIcon {
    #[default]
    Dark,
    Light,
}

impl IconPreference {
    /// Resolve the preference against the terminal theme's effective background: a light theme
    /// gets the light icon, a dark one the dark icon.
    pub fn resolve(self, (r, g, b): Rgb) -> WindowIcon {
        match self {
            Self::Dark => WindowIcon::Dark,
            Self::Light => WindowIcon::Light,
            Self::Auto if is_light_background((r, g, b)) => WindowIcon::Light,
            Self::Auto => WindowIcon::Dark,
        }
    }
}

fn is_light_background((r, g, b): Rgb) -> bool {
    const HALF_SCALED: u32 = 255 * 1000 / 2;
    299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) > HALF_SCALED
}

/// `[appearance]`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AppearanceSettings {
    /// Let the cursor blink when the program asks for a blinking cursor.
    pub cursor_blink: bool,
    /// Window icon. Auto follows the terminal theme's background. Changes require a restart.
    pub icon: IconPreference,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self { cursor_blink: true, icon: IconPreference::default() }
    }
}

/// `[terminal]`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TerminalSettings {
    /// Scrollback limit in lines; applies to new terminals.
    #[serde(deserialize_with = "scrollback_lines")]
    pub scrollback_lines: usize,
    /// Copy the selection to the clipboard when the mouse button is released.
    pub copy_on_select: bool,
    /// Scrollbar of the terminal panes.
    pub scrollbar: ScrollbarVisibility,
    /// Hide the mouse pointer on key presses until it moves.
    pub hide_pointer_while_typing: bool,
    /// What a pane does when its program exits.
    pub on_exit: OnExit,
    /// Play the system alert sound on the bell.
    pub bell_sound: bool,
    /// Shake the text of the pane that rang the bell.
    pub bell_visual: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
            copy_on_select: false,
            scrollbar: ScrollbarVisibility::default(),
            hide_pointer_while_typing: true,
            on_exit: OnExit::default(),
            bell_sound: true,
            bell_visual: true,
        }
    }
}

/// `[pane]`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PaneSettings {
    /// Opacity of the text in panes other than the focused one of their tab, 0..=1.
    pub inactive_text_opacity: f32,
    /// Keep showing a hollow cursor in those panes.
    pub show_inactive_cursor: bool,
    /// Thickness of the line between two panes, in logical pixels.
    pub divider_width: f32,
}

impl Default for PaneSettings {
    fn default() -> Self {
        Self { inactive_text_opacity: 0.7, show_inactive_cursor: false, divider_width: 4. }
    }
}

const MAX_DIVIDER_WIDTH: f32 = 16.;

/// The whole configuration.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Config {
    /// `[general]`.
    pub general: GeneralSettings,
    /// `[font]`.
    pub font: FontSettings,
    /// `[appearance]`.
    pub appearance: AppearanceSettings,
    /// `[window]`.
    pub window: WindowSettings,
    /// `[terminal]`.
    pub terminal: TerminalSettings,
    /// `[pane]`.
    pub pane: PaneSettings,
    /// `[colors]`.
    pub colors: ColorSettings,
}

const MAX_NAME_LEN: usize = 128;
const MAX_FALLBACKS: usize = 16;

impl Config {
    /// Parses TOML text and clamps every value into its valid range. Also returns what was
    /// wrong but could be fixed: values out of range (replaced by the nearest valid value or
    /// the default) and keys no setting has.
    pub fn from_toml_checked(text: &str, path: &Path) -> Result<(Self, Vec<String>)> {
        let mut config: Self = toml::from_str(text).map_err(|e| parse_error(path, text, &e))?;
        let mut problems: Vec<String> =
            template::unknown_keys(text, &config_lines()).into_iter().map(|k| format!("unknown setting {k}")).collect();
        report_integers(text, &mut problems);
        config.validate(&mut problems);
        Ok((config, problems))
    }

    fn validate(&mut self, problems: &mut Vec<String>) {
        let defaults = Self::default();
        if let Some(id) = &self.general.default_profile
            && (id.as_str().trim().is_empty() || id.as_str().len() > MAX_NAME_LEN)
        {
            problems.push("general.default_profile: invalid id, using the automatic choice".into());
            self.general.default_profile = None;
        }
        let font = &mut self.font;
        if font.family.trim().is_empty() || font.family.len() > MAX_NAME_LEN {
            problems.push(format!("font.family: invalid, using \"{}\"", defaults.font.family));
            font.family = defaults.font.family;
        }
        fix_f32(problems, "font.size", &mut font.size, (6., 72.), defaults.font.size);
        fix_option_f32(problems, "font.gamma", &mut font.gamma, (1., 3.));
        fix_option_f32(problems, "font.contrast", &mut font.contrast, (0., 1.));
        fix_f32(problems, "font.line_height", &mut font.line_height, (0.5, 2.), defaults.font.line_height);
        fix_f32(
            problems,
            "font.cell_width_adjust",
            &mut font.cell_width_adjust,
            (-5., 10.),
            defaults.font.cell_width_adjust,
        );
        let count = font.fallbacks.len();
        font.fallbacks.retain(|f| !f.trim().is_empty() && f.len() <= MAX_NAME_LEN);
        font.fallbacks.truncate(MAX_FALLBACKS);
        if font.fallbacks.len() != count {
            problems.push(format!("font.fallbacks: empty or too long names dropped (at most {MAX_FALLBACKS})"));
        }
        fix_f32(problems, "window.opacity", &mut self.window.opacity, (0., 1.), defaults.window.opacity);
        if parse_hex_color(&self.colors.title_bar).is_none() {
            let (value, default) = (&self.colors.title_bar, &defaults.colors.title_bar);
            problems.push(format!("colors.title_bar: \"{value}\" is not a #rrggbb color, using \"{default}\""));
            self.colors.title_bar = defaults.colors.title_bar;
        }
        let pane = &mut self.pane;
        fix_f32(
            problems,
            "pane.inactive_text_opacity",
            &mut pane.inactive_text_opacity,
            (0., 1.),
            defaults.pane.inactive_text_opacity,
        );
        fix_f32(
            problems,
            "pane.divider_width",
            &mut pane.divider_width,
            (0., MAX_DIVIDER_WIDTH),
            defaults.pane.divider_width,
        );
    }
}

const WEIGHT_RANGE: (i64, i64) = (100, 900);
const SCROLLBACK_LINES_RANGE: (i64, i64) = (0, 1_000_000);

/// The integer settings and their ranges. They are clamped while deserializing (so a value that
/// does not fit the Rust type, such as `-1` for a `u16`, does not refuse the whole file), and
/// [`report_integers`] reports them with the value as written.
const INTEGER_SETTINGS: [(&str, &str, (i64, i64)); 3] = [
    ("font", "weight", WEIGHT_RANGE),
    ("font", "bold_weight", WEIGHT_RANGE),
    ("terminal", "scrollback_lines", SCROLLBACK_LINES_RANGE),
];

fn clamped<'de, D: serde::Deserializer<'de>, T: TryFrom<i64>>(
    deserializer: D,
    (lo, hi): (i64, i64),
) -> std::result::Result<T, D::Error> {
    use serde::de::Error as _;
    let value = i64::deserialize(deserializer)?.clamp(lo, hi);
    T::try_from(value).map_err(|_| D::Error::custom(format!("{value} does not fit")))
}

fn weight<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<u16, D::Error> {
    clamped(deserializer, WEIGHT_RANGE)
}

fn scrollback_lines<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<usize, D::Error> {
    clamped(deserializer, SCROLLBACK_LINES_RANGE)
}

/// Reports every [`INTEGER_SETTINGS`] value in `text` outside its range, as written there.
fn report_integers(text: &str, problems: &mut Vec<String>) {
    let Ok(table) = text.parse::<toml::Table>() else { return };
    for (section, key, (lo, hi)) in INTEGER_SETTINGS {
        if let Some(value) = table.get(section).and_then(|s| s.get(key)).and_then(toml::Value::as_integer)
            && !(lo..=hi).contains(&value)
        {
            problems.push(format!("{section}.{key}: {value} is outside {lo} .. {hi}, using {}", value.clamp(lo, hi)));
        }
    }
}

/// Keeps `value` in `range`; not a number becomes `default`. Reports any change.
fn fix_f32(problems: &mut Vec<String>, name: &str, value: &mut f32, (lo, hi): (f32, f32), default: f32) {
    let fixed = if value.is_finite() { value.clamp(lo, hi) } else { default };
    if fixed.to_bits() != value.to_bits() {
        problems.push(format!("{name}: {value} is outside {lo} .. {hi}, using {fixed}"));
        *value = fixed;
    }
}

/// Like [`fix_f32`] for a setting that may be unset; not a number unsets it.
fn fix_option_f32(problems: &mut Vec<String>, name: &str, value: &mut Option<f32>, (lo, hi): (f32, f32)) {
    if let Some(v) = *value {
        let fixed = v.is_finite().then(|| v.clamp(lo, hi));
        if fixed.map(f32::to_bits) != Some(v.to_bits()) {
            problems.push(format!("{name}: {v} is outside {lo} .. {hi}"));
            *value = fixed;
        }
    }
}

/// Where the settings files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDir {
    /// The folder.
    pub path: PathBuf,
    /// Why it is not `config` next to `wired.exe`, to tell the user; `None` when it is.
    pub fallback_reason: Option<String>,
}

/// `config` next to `wired.exe` (portable), or `%APPDATA%\wired` when that cannot be written
/// to (an install under Program Files). Decided once per process.
pub fn config_location() -> &'static ConfigDir {
    static LOCATION: std::sync::OnceLock<ConfigDir> = std::sync::OnceLock::new();
    LOCATION.get_or_init(|| {
        let install = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("config")));
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).filter(|p| p.is_absolute());
        choose_config_dir(install, appdata, can_write)
    })
}

/// The install folder's `config` if `can_write` it, else `<appdata>\wired`. Never the current
/// directory.
fn choose_config_dir(
    install: Option<PathBuf>,
    appdata: Option<PathBuf>,
    can_write: impl Fn(&Path) -> std::io::Result<()>,
) -> ConfigDir {
    let refused = match &install {
        Some(dir) => match can_write(dir) {
            Ok(()) => return ConfigDir { path: dir.clone(), fallback_reason: None },
            Err(error) => format!("{} cannot be written to ({error})", dir.display()),
        },
        None => "the folder of wired.exe is unknown".to_owned(),
    };
    match appdata {
        Some(appdata) => {
            let path = appdata.join("wired");
            let fallback_reason = Some(format!("{refused}, so the settings are in {}", path.display()));
            ConfigDir { path, fallback_reason }
        }
        // Without %APPDATA%: the install folder, where writing fails and says so; the temporary
        // folder only when even that is unknown.
        None => {
            ConfigDir { path: install.unwrap_or_else(|| std::env::temp_dir().join("wired")), fallback_reason: None }
        }
    }
}

/// Creates `dir` if needed and writes and removes a file in it. The file is named after the
/// process, so two instances starting at once do not remove each other's.
fn can_write(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

fn config_dir() -> &'static Path {
    &config_location().path
}

/// `config_dir()\config.toml`.
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Loads the config; a missing file means all defaults. Also returns what was wrong in the
/// file but could be fixed ([`Config::from_toml_checked`]).
pub fn load_checked(path: &Path) -> Result<(Config, Vec<String>)> {
    match read_optional(path) {
        Ok(Some(text)) => Config::from_toml_checked(&text, path),
        Ok(None) => Ok((Config::default(), Vec::new())),
        Err(source) => Err(Error::Read { path: path.to_path_buf(), source }),
    }
}

/// Reads a file, `None` when it does not exist.
fn read_optional(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Points TOML editors (Taplo / Even Better TOML, Tombi) at the schema next to the file.
const SCHEMA_DIRECTIVE: &str = "#:schema ./config.schema.json\n";

const CONFIG_HEADER: &str =
    concat!("#:schema ./config.schema.json\n", "# wired configuration. Edit and press Ctrl+Shift+R to apply.\n");

/// `config_dir()\config.schema.json`.
pub fn schema_path() -> PathBuf {
    config_dir().join("config.schema.json")
}

/// The JSON Schema of `config.toml`, from the settings types and their doc comments.
pub fn schema_json() -> String {
    let schema = schemars::schema_for!(Config);
    let mut text = serde_json::to_string_pretty(&schema).expect("a JSON value always serializes");
    text.push('\n');
    text
}

/// Writes [`schema_json`] to `path` unless the file already has exactly that. Returns whether it
/// was written.
pub fn write_schema_if_outdated(path: &Path) -> std::io::Result<bool> {
    let schema = schema_json();
    if read_optional(path)?.as_deref() == Some(schema.as_str()) {
        return Ok(false);
    }
    replace_file(path, &schema)?;
    Ok(true)
}

/// Puts the `#:schema` line at the top of an existing `config.toml` that has none (files from
/// before the schema existed). Returns whether it was added.
pub fn add_schema_directive(path: &Path) -> std::io::Result<bool> {
    let Some(text) = read_optional(path)? else { return Ok(false) };
    match with_schema_directive(&text) {
        Some(updated) => replace_file(path, &updated).map(|()| true),
        None => Ok(false),
    }
}

/// `text` with the `#:schema` line first (after a byte order mark, which must stay the very first
/// character), ending the way the file's lines end; `None` when it already has one.
fn with_schema_directive(text: &str) -> Option<String> {
    if text.lines().any(|line| line.trim_start().starts_with("#:schema")) {
        return None;
    }
    let (bom, body) = text.strip_prefix('\u{feff}').map_or(("", text), |rest| ("\u{feff}", rest));
    let line = SCHEMA_DIRECTIVE.trim_end();
    let newline = if body.contains("\r\n") { "\r\n" } else { "\n" };
    Some(format!("{bom}{line}{newline}{body}"))
}

/// Every setting of `config.toml` with its default and comment: the single list the template,
/// the "add new settings to an existing file" step and the unknown-key check come from.
/// `Config::default()` must agree with it (unit-tested).
fn config_lines() -> Vec<template::Line> {
    use template::line;
    vec![
        line(
            "general",
            "default_profile",
            "\"builtin:pwsh\"",
            "unset = PowerShell 7, else Windows PowerShell, else Command Prompt",
            true,
        ),
        line("font", "family", "\"UDEV Gothic 35NFLG\"", "\"UDEV Gothic NFLG\" for 1:2 width", false),
        line("font", "size", "12.0", "6 .. 72", false),
        line("font", "ligatures", "\"break-at-cursor\"", "\"on\" | \"off\" | \"break-at-cursor\"", false),
        line("font", "fallbacks", "[\"UDEV Gothic NFLG\", \"Cascadia Mono\", \"Consolas\"]", "", false),
        line(
            "font",
            "antialiasing",
            "\"skia\"",
            "\"skia\" (Chromium/Tabby-like) | \"system\" (DirectWrite) (restart)",
            false,
        ),
        line("font", "line_height", "0.9", "0.5 .. 2.0", false),
        line("font", "weight", "400", "100 .. 900", false),
        line("font", "bold_weight", "700", "100 .. 900", false),
        line("font", "cell_width_adjust", "0.0", "px added to the cell width, -5 .. 10", false),
        line("font", "gamma", "1.8", "skia: 1.0 .. 3.0; unset = system DirectWrite value (restart)", true),
        line("font", "contrast", "0.5", "skia: 0 .. 1, higher = bolder; unset = system value (restart)", true),
        line("appearance", "cursor_blink", "true", "", false),
        line(
            "appearance",
            "icon",
            "\"auto\"",
            "\"auto\" (follows terminal theme) | \"dark\" | \"light\" (restart required)",
            false,
        ),
        line("window", "opacity", "0.6", "0.0 .. 1.0", false),
        line("window", "backdrop", "\"blur\"", "\"none\" | \"blur\" | \"acrylic\" | \"mica\" | \"mica-alt\"", false),
        line("window", "show_reload_button", "false", "button at the top right that reloads the settings", false),
        line(
            "colors",
            "theme",
            "\"Lumina Rayleigh Dark\"",
            "file name in themes\\ next to this file or wired.exe",
            false,
        ),
        line("colors", "title_bar", "\"#131313\"", "the active tab takes the background", false),
        line("terminal", "scrollback_lines", "10000", "applies to new terminals", false),
        line("terminal", "copy_on_select", "false", "", false),
        line("terminal", "scrollbar", "\"overlay\"", "\"overlay\" | \"never\"", false),
        line("terminal", "hide_pointer_while_typing", "true", "", false),
        line("terminal", "on_exit", "\"auto\"", "\"auto\" (close on exit code 0) | \"close\" | \"keep\"", false),
        line("terminal", "bell_sound", "true", "system alert sound on the bell", false),
        line("terminal", "bell_visual", "true", "the pane's text shakes on the bell", false),
        line("pane", "inactive_text_opacity", "0.7", "text of unfocused panes, 0.0 .. 1.0", false),
        line("pane", "show_inactive_cursor", "false", "hollow cursor in unfocused panes", false),
        line("pane", "divider_width", "4.0", "line between panes in px, 0 .. 16", false),
    ]
}

/// Text written when no config exists: every setting at its default, as a reference.
pub(crate) fn config_template() -> String {
    template::render(CONFIG_HEADER, &config_lines())
}

/// Writes [`config_template`] to `path` if no file exists there. Returns whether it was written.
pub fn write_template_if_missing(path: &Path) -> std::io::Result<bool> {
    write_if_missing(path, &config_template())
}

fn write_if_missing(path: &Path, text: &str) -> std::io::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)?;
    Ok(true)
}

/// Adds the declared lines a file lacks (settings or bindings a newer version introduced),
/// each with its comment, at the end of its section. Writes only when something was added;
/// returns the keys added. A missing file is left alone.
fn add_missing_lines(path: &Path, lines: &[template::Line]) -> std::io::Result<Vec<String>> {
    let Some(text) = read_optional(path)? else { return Ok(Vec::new()) };
    let (updated, added) = match template::add_missing(&text, lines) {
        Ok(result) => result,
        // A file that does not parse is left as it is; loading it reports the error.
        Err(_) if toml_edit::Document::parse(text.as_str()).is_err() => return Ok(Vec::new()),
        Err(message) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, message)),
    };
    if !added.is_empty() {
        replace_file(path, &updated)?;
    }
    Ok(added)
}

/// Writes `text` to a file next to `path` and moves it over `path`, so a failed write never
/// leaves a half-written settings file.
fn replace_file(path: &Path, text: &str) -> std::io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let result = std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Adds the settings `config.toml` lacks (see [`add_missing_lines`]); returns the keys added.
pub fn add_missing_settings(path: &Path) -> std::io::Result<Vec<String>> {
    add_missing_lines(path, &config_lines())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        Config::from_toml_checked(text, Path::new("config.toml")).map(|(config, _)| config)
    }

    #[test]
    fn reports_what_it_fixes_and_unknown_keys() {
        let text = "[font]\nsize = 1000\nsizee = 3\n[colors]\nforeground = \"red\"\n";
        let (config, problems) = Config::from_toml_checked(text, Path::new("config.toml")).expect("config");
        assert!((config.font.size - 72.).abs() < f32::EPSILON);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("font.sizee"), "{problems:?}");
        assert!(problems.iter().any(|p| p == "unknown setting colors.foreground"), "{problems:?}");
    }

    #[test]
    fn templates_already_have_every_declared_line() {
        assert!(template::add_missing(&config_template(), &config_lines()).expect("template parses").1.is_empty());
        assert!(
            template::add_missing(&keybind::keybind_template(), &keybind::keybind_lines())
                .expect("template parses")
                .1
                .is_empty()
        );
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(parse(&config_template()).expect("template"), Config::default());
    }

    #[test]
    fn window_icon_follows_the_startup_theme_or_the_explicit_preference() {
        let dark = DEFAULT_BACKGROUND;
        let light = (0xfa, 0xf4, 0xe8);
        for (text, expected_dark, expected_light) in [
            ("", WindowIcon::Dark, WindowIcon::Light),
            ("[appearance]\nicon = \"auto\"", WindowIcon::Dark, WindowIcon::Light),
            ("[appearance]\nicon = \"dark\"", WindowIcon::Dark, WindowIcon::Dark),
            ("[appearance]\nicon = \"light\"", WindowIcon::Light, WindowIcon::Light),
        ] {
            let (config, problems) = Config::from_toml_checked(text, Path::new("config.toml")).expect("config");
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(config.appearance.icon.resolve(dark), expected_dark);
            assert_eq!(config.appearance.icon.resolve(light), expected_light);
        }
        assert!(matches!(parse("[appearance]\nicon = \"blue\""), Err(Error::Parse { .. })));
    }

    #[test]
    fn reads_values_and_ignores_unknown_keys() {
        let config = parse(
            "unknown = 1\n[font]\nsize = 14\nligatures = \"off\"\nantialiasing = \"system\"\n[window]\nbackdrop = \"mica-alt\"\n",
        )
        .expect("config");
        assert!((config.font.size - 14.).abs() < f32::EPSILON);
        assert_eq!(config.font.ligatures, Ligatures::Off);
        assert_eq!(config.font.antialiasing, Antialiasing::System);
        assert_eq!(config.window.backdrop, Backdrop::MicaAlt);
    }

    #[test]
    fn clamps_out_of_range_values() {
        let config = parse(
            "[font]\nsize = 1000\nfamily = \"\"\nweight = 50\nbold_weight = 2000\ncell_width_adjust = 99\n[window]\nopacity = -3\n[pane]\ninactive_text_opacity = 7\n",
        )
        .expect("config");
        assert!((config.pane.inactive_text_opacity - 1.).abs() < f32::EPSILON);
        assert!((config.font.size - 72.).abs() < f32::EPSILON);
        assert_eq!((config.font.weight, config.font.bold_weight), (100, 900));
        assert!((config.font.cell_width_adjust - 10.).abs() < f32::EPSILON);
        assert_eq!(config.font.family, FontSettings::default().family);
        assert!(config.window.opacity.abs() < f32::EPSILON);
    }

    #[test]
    fn integers_outside_their_type_are_clamped_not_fatal() {
        let text = "[font]\nweight = -1\nbold_weight = 70000\nsize = 13\n[terminal]\nscrollback_lines = -5\n";
        let (config, problems) = Config::from_toml_checked(text, Path::new("config.toml")).expect("file is used");
        assert_eq!((config.font.weight, config.font.bold_weight), (100, 900));
        assert_eq!(config.terminal.scrollback_lines, 0);
        assert!((config.font.size - 13.).abs() < f32::EPSILON, "the rest of the file still applies");
        assert_eq!(
            problems,
            [
                "font.weight: -1 is outside 100 .. 900, using 100",
                "font.bold_weight: 70000 is outside 100 .. 900, using 900",
                "terminal.scrollback_lines: -5 is outside 0 .. 1000000, using 0",
            ],
            "each one once, with the value as written"
        );
    }

    #[test]
    fn reports_syntax_errors() {
        assert!(matches!(parse("[font\n"), Err(Error::Parse { .. })));
        assert!(matches!(parse("[font]\nsize = \"big\"\n"), Err(Error::Parse { .. })));
    }

    #[test]
    fn parse_errors_name_the_file_line_and_column_on_one_line() {
        let error = parse("# note\n[font]\nsize = \"big\"\n").expect_err("wrong type");
        let shown = error.to_string();
        assert!(shown.starts_with("config.toml, line 3, column 8: "), "{shown}");
        assert!(!shown.contains('\n'), "{shown}");
    }

    #[test]
    fn falls_back_to_appdata_when_the_install_folder_is_read_only() {
        let install = Some(PathBuf::from(r"C:\Program Files\wired\config"));
        let appdata = Some(PathBuf::from(r"C:\Users\u\AppData\Roaming"));
        let writable = choose_config_dir(install.clone(), appdata.clone(), |_| Ok(()));
        assert_eq!(
            writable,
            ConfigDir { path: PathBuf::from(r"C:\Program Files\wired\config"), fallback_reason: None }
        );
        let denied = || Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let read_only = choose_config_dir(install, appdata.clone(), |_| denied());
        assert_eq!(read_only.path, PathBuf::from(r"C:\Users\u\AppData\Roaming\wired"));
        assert!(read_only.fallback_reason.is_some_and(|r| r.contains(r"C:\Program Files\wired\config")));
        assert_eq!(
            choose_config_dir(None, appdata, |_| Ok(())).path,
            PathBuf::from(r"C:\Users\u\AppData\Roaming\wired")
        );
    }

    #[test]
    fn schema_describes_settings_and_their_choices() {
        let schema: serde_json::Value = serde_json::from_str(&schema_json()).expect("schema is JSON");
        let text = schema.to_string();
        for expected in ["default_profile", "scrollback_lines", "break-at-cursor", "mica-alt", "Size in logical pixels"]
        {
            assert!(text.contains(expected), "{expected} missing");
        }
        assert!(config_template().starts_with(SCHEMA_DIRECTIVE));
    }

    #[test]
    fn schema_line_goes_after_a_bom_and_keeps_the_line_endings() {
        let with_bom = with_schema_directive("\u{feff}[font]\r\nsize = 14\r\n").expect("added");
        assert_eq!(with_bom, "\u{feff}#:schema ./config.schema.json\r\n[font]\r\nsize = 14\r\n");
        assert!(parse(&with_bom).is_ok(), "the file still parses");
        assert_eq!(with_schema_directive("[font]\n").as_deref(), Some("#:schema ./config.schema.json\n[font]\n"));
        assert_eq!(with_schema_directive("# mine\n#:schema other.json\n"), None);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        assert_eq!(line_column("あい = 1\nx", 6), (1, 3));
        assert_eq!(line_column("a\nb", 2), (2, 1));
        assert_eq!(line_column("a", 99), (1, 2));
    }
}
