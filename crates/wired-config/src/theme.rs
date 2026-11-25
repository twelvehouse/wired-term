//! Color themes in Ghostty's theme file format (the iTerm2-Color-Schemes collection is published
//! in it): `key = value` lines and `#` comments. Only colors are read from a theme.
//!
//! Theme files are untrusted: the name may not leave the themes folders, the file size is capped,
//! and lines that cannot be used are reported and skipped, never fatal.

use std::{
    collections::BTreeMap,
    io::Read as _,
    path::{Path, PathBuf},
};

use crate::{ColorSettings, parse_hex_color};

/// A color as `(r, g, b)`.
pub type Rgb = (u8, u8, u8);

/// Largest theme file read, in bytes; real themes are well under 1 KiB.
pub const MAX_THEME_BYTES: u64 = 64 * 1024;

const MAX_THEME_NAME_CHARS: usize = 128;

/// Built-in default text color, used when the theme sets none.
pub const DEFAULT_FOREGROUND: Rgb = (0xc0, 0xca, 0xf5);
/// Built-in default background color, used when the theme sets none.
pub const DEFAULT_BACKGROUND: Rgb = (0x14, 0x15, 0x1a);

/// The colors a theme file sets; `None` (or an index missing from `palette`) is left to the
/// built-in defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Theme {
    /// `foreground`.
    pub foreground: Option<Rgb>,
    /// `background`.
    pub background: Option<Rgb>,
    /// `cursor-color`.
    pub cursor_color: Option<Rgb>,
    /// `cursor-text`: text under a block cursor.
    pub cursor_text: Option<Rgb>,
    /// `selection-background`.
    pub selection_background: Option<Rgb>,
    /// `selection-foreground`.
    pub selection_foreground: Option<Rgb>,
    /// `palette = N=#rrggbb` entries by index.
    pub palette: BTreeMap<u8, Rgb>,
}

/// Parses a theme file. Also returns the lines that could not be used (unknown keys, bad
/// values), which are skipped.
pub fn parse_theme(text: &str) -> (Theme, Vec<String>) {
    let mut theme = Theme::default();
    let mut problems = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = number + 1;
        let Some((key, value)) = line.split_once('=') else {
            problems.push(format!("line {at}: not a `key = value` line"));
            continue;
        };
        let key = key.trim();
        let value = unquote(value.trim());
        // As in Ghostty, an empty value resets the key, which in a theme means leaving it unset.
        if value.is_empty() {
            continue;
        }
        let slot = match key {
            "foreground" => &mut theme.foreground,
            "background" => &mut theme.background,
            "cursor-color" => &mut theme.cursor_color,
            "cursor-text" => &mut theme.cursor_text,
            "selection-background" => &mut theme.selection_background,
            "selection-foreground" => &mut theme.selection_foreground,
            "palette" => {
                match parse_palette_entry(value) {
                    Some((index, color)) => {
                        theme.palette.insert(index, color);
                    }
                    None => problems.push(format!("line {at}: palette: \"{value}\" is not N=#rrggbb with N 0 .. 255")),
                }
                continue;
            }
            _ => {
                problems.push(format!("line {at}: {key} is not a color setting, ignored"));
                continue;
            }
        };
        match parse_theme_color(value) {
            Some(color) => *slot = Some(color),
            None => problems.push(format!("line {at}: {key}: \"{value}\" is not a #rrggbb color")),
        }
    }
    (theme, problems)
}

/// A value without the double quotes Ghostty allows around it.
fn unquote(value: &str) -> &str {
    value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value)
}

/// `#rrggbb` or `rrggbb`.
fn parse_theme_color(value: &str) -> Option<Rgb> {
    if value.starts_with('#') { parse_hex_color(value) } else { parse_hex_color(&format!("#{value}")) }
}

/// `N=color` with N in 0..=255, written in decimal or with a `0x`, `0o` or `0b` prefix (as Ghostty
/// reads it).
fn parse_palette_entry(value: &str) -> Option<(u8, Rgb)> {
    let (index, color) = value.split_once('=')?;
    let index = index.trim();
    let (digits, radix) = match index.get(..2) {
        Some("0x" | "0X") => (&index[2..], 16),
        Some("0o" | "0O") => (&index[2..], 8),
        Some("0b" | "0B") => (&index[2..], 2),
        _ => (index, 10),
    };
    let index = u8::from_str_radix(digits, radix).ok()?;
    Some((index, parse_theme_color(color.trim())?))
}

/// Why a theme could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    /// The name could reach outside the themes folders or is not a file name.
    #[error("not a theme name (a file name of at most {MAX_THEME_NAME_CHARS} characters, without \\ / or ..)")]
    InvalidName,
    /// No themes folder has the file.
    #[error("not found, looked for {}", .searched.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" and "))]
    NotFound {
        /// The paths tried.
        searched: Vec<PathBuf>,
    },
    /// The path is a folder or a device, not a file.
    #[error("{} is not a file", .path.display())]
    NotAFile {
        /// The path.
        path: PathBuf,
    },
    /// The file is larger than [`MAX_THEME_BYTES`].
    #[error("{} is larger than {} KiB", .path.display(), MAX_THEME_BYTES / 1024)]
    TooLarge {
        /// The path.
        path: PathBuf,
    },
    /// The file exists but could not be read.
    #[error("cannot read {}: {source}", .path.display())]
    Read {
        /// The path.
        path: PathBuf,
        /// I/O error.
        source: std::io::Error,
    },
}

/// The file name a theme name is stored under, `None` when the name is not allowed. Themes live in
/// flat folders, so a name can never hold a path.
fn theme_file_name(name: &str) -> Option<String> {
    let is_allowed = !name.trim().is_empty()
        && name.chars().count() <= MAX_THEME_NAME_CHARS
        && !name.contains(['/', '\\'])
        && !name.contains("..")
        && !name.chars().any(char::is_control);
    // Windows file names cannot hold ':'; tar.exe extracts such themes ("Paper Crane: Midnight
    // Ink") with '_' instead, and the replacement also rules out drive letters and streams.
    is_allowed.then(|| name.replace(':', "_"))
}

/// The folders searched for themes, in order: `themes` in the settings folder (the user's own),
/// then `themes` next to `wired.exe` (the shipped collection). Never the current directory.
pub fn theme_dirs() -> Vec<PathBuf> {
    let user = crate::config_location().path.join("themes");
    let shipped = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("themes")));
    std::iter::once(user).chain(shipped).collect()
}

/// Loads theme `name` from the first of [`theme_dirs`] that has it. Also returns the lines of
/// the file that could not be used.
pub fn load_theme(name: &str) -> Result<(Theme, Vec<String>), ThemeError> {
    load_theme_from(name, &theme_dirs())
}

fn load_theme_from(name: &str, dirs: &[PathBuf]) -> Result<(Theme, Vec<String>), ThemeError> {
    let file_name = theme_file_name(name).ok_or(ThemeError::InvalidName)?;
    let searched: Vec<PathBuf> = dirs.iter().map(|dir| dir.join(&file_name)).collect();
    for path in &searched {
        match read_theme_file(path) {
            Ok(text) => return Ok(parse_theme(&text)),
            Err(ThemeError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(ThemeError::NotFound { searched })
}

fn read_theme_file(path: &Path) -> Result<String, ThemeError> {
    let read_error = |source| ThemeError::Read { path: path.to_path_buf(), source };
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        // Windows refuses to open a folder as a file (access denied).
        Err(_) if path.is_dir() => return Err(ThemeError::NotAFile { path: path.to_path_buf() }),
        Err(error) => return Err(read_error(error)),
    };
    // Checked on the open handle, so a device name (CON) is refused before anything is read.
    if !file.metadata().map_err(read_error)?.is_file() {
        return Err(ThemeError::NotAFile { path: path.to_path_buf() });
    }
    let mut bytes = Vec::new();
    file.take(MAX_THEME_BYTES + 1).read_to_end(&mut bytes).map_err(read_error)?;
    if bytes.len() as u64 > MAX_THEME_BYTES {
        return Err(ThemeError::TooLarge { path: path.to_path_buf() });
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The terminal colors in effect: each one from the theme, else the built-in default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorScheme {
    /// Default text color.
    pub foreground: Rgb,
    /// Default background color.
    pub background: Rgb,
    /// Cursor color; `None` = the text color.
    pub cursor: Option<Rgb>,
    /// Text under a block cursor; `None` = the background color.
    pub cursor_text: Option<Rgb>,
    /// Background of selected cells; `None` = the built-in translucent highlight.
    pub selection_background: Option<Rgb>,
    /// Text of selected cells; `None` = unchanged.
    pub selection_foreground: Option<Rgb>,
    /// Palette entries that replace the built-in 256-color palette's.
    pub palette: BTreeMap<u8, Rgb>,
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::from_theme(&Theme::default())
    }
}

impl ColorScheme {
    /// The colors `theme` sets, with the built-in defaults for the rest.
    pub fn from_theme(theme: &Theme) -> Self {
        Self {
            foreground: theme.foreground.unwrap_or(DEFAULT_FOREGROUND),
            background: theme.background.unwrap_or(DEFAULT_BACKGROUND),
            cursor: theme.cursor_color,
            cursor_text: theme.cursor_text,
            selection_background: theme.selection_background,
            selection_foreground: theme.selection_foreground,
            palette: theme.palette.clone(),
        }
    }

    /// Loads the theme `config` names (if any) and resolves the colors. Problems with the theme
    /// are returned prefixed with its name; a theme that cannot be loaded leaves the defaults.
    pub fn load(config: &ColorSettings) -> (Self, Vec<String>) {
        let Some(name) = &config.theme else { return (Self::default(), Vec::new()) };
        let (theme, problems) = load_theme(name).unwrap_or_else(|error| (Theme::default(), vec![error.to_string()]));
        let problems = problems.into_iter().map(|p| format!("theme \"{name}\": {p}")).collect();
        (Self::from_theme(&theme), problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOON: &str = "palette = 0=#1b1d2b\npalette = 15=#c8d3f5\nbackground = #222436\nforeground = #c8d3f5\n\
        cursor-color = #c8d3f5\ncursor-text = #222436\nselection-background = #2d3f76\nselection-foreground = #c8d3f5\n";

    #[test]
    fn reads_a_ghostty_theme_file() {
        let (theme, problems) = parse_theme(MOON);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(theme.background, Some((0x22, 0x24, 0x36)));
        assert_eq!(theme.foreground, Some((0xc8, 0xd3, 0xf5)));
        assert_eq!(theme.cursor_color, Some((0xc8, 0xd3, 0xf5)));
        assert_eq!(theme.cursor_text, Some((0x22, 0x24, 0x36)));
        assert_eq!(theme.selection_background, Some((0x2d, 0x3f, 0x76)));
        assert_eq!(theme.selection_foreground, Some((0xc8, 0xd3, 0xf5)));
        assert_eq!(theme.palette, BTreeMap::from([(0, (0x1b, 0x1d, 0x2b)), (15, (0xc8, 0xd3, 0xf5))]));
    }

    #[test]
    fn accepts_comments_quotes_bare_hex_and_prefixed_indices() {
        let text = "# comment\r\n  background = \"112233\"\r\n\r\npalette = 0x10 = #aabbcc\npalette=0b11=#000001\n";
        let (theme, problems) = parse_theme(text);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(theme.background, Some((0x11, 0x22, 0x33)));
        assert_eq!(theme.palette, BTreeMap::from([(3, (0, 0, 1)), (16, (0xaa, 0xbb, 0xcc))]));
    }

    #[test]
    fn skips_and_reports_lines_it_cannot_use() {
        let text = "foreground = #c8d3f5\nfont-size = 20\nbackground = red\njust words\npalette = 256=#000000\n\
            palette = -1=#000000\npalette = 7=#12345\ncursor-color = cell-foreground\n";
        let (theme, problems) = parse_theme(text);
        assert_eq!(theme, Theme { foreground: Some((0xc8, 0xd3, 0xf5)), ..Theme::default() });
        let lines: Vec<&str> = problems.iter().filter_map(|p| p.split(':').next()).collect();
        assert_eq!(lines, ["line 2", "line 3", "line 4", "line 5", "line 6", "line 7", "line 8"]);
    }

    #[test]
    fn theme_names_cannot_leave_the_themes_folders() {
        for bad in ["", "  ", "../config", "..", "a/b", r"a\b", r"C:\Windows\win.ini", "x\u{7}", &"a".repeat(129)] {
            assert_eq!(theme_file_name(bad), None, "{bad:?}");
        }
        assert_eq!(theme_file_name("TokyoNight Moon").as_deref(), Some("TokyoNight Moon"));
        assert_eq!(theme_file_name("Paper Crane: Midnight Ink").as_deref(), Some("Paper Crane_ Midnight Ink"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wired-theme-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn user_themes_come_before_shipped_ones() {
        let (user, shipped) = (temp_dir("user"), temp_dir("shipped"));
        std::fs::write(shipped.join("Mine"), "background = #000001\n").expect("write");
        std::fs::write(shipped.join("Only Shipped"), "background = #000002\n").expect("write");
        std::fs::write(user.join("Mine"), "background = #000003\n").expect("write");
        let dirs = [user.clone(), shipped.clone()];
        let background = |name| load_theme_from(name, &dirs).map(|(theme, _)| theme.background);
        assert_eq!(background("Mine").expect("found"), Some((0, 0, 3)));
        assert_eq!(background("Only Shipped").expect("found"), Some((0, 0, 2)));
        assert!(
            matches!(load_theme_from("Missing", &dirs), Err(ThemeError::NotFound { searched }) if searched.len() == 2)
        );
        assert!(matches!(load_theme_from("../user/Mine", &dirs), Err(ThemeError::InvalidName)));
        let _ = (std::fs::remove_dir_all(user), std::fs::remove_dir_all(shipped));
    }

    #[test]
    fn refuses_folders_and_files_over_the_size_cap() {
        let dir = temp_dir("cap");
        std::fs::create_dir(dir.join("Folder")).expect("mkdir");
        let fits = "# pad\n".repeat(1000) + "background = #000001\n";
        std::fs::write(dir.join("Fits"), &fits).expect("write");
        std::fs::write(dir.join("Huge"), "#".repeat(MAX_THEME_BYTES as usize + 1)).expect("write");
        let dirs = [dir.clone()];
        assert!(matches!(load_theme_from("Folder", &dirs), Err(ThemeError::NotAFile { .. })));
        assert!(matches!(load_theme_from("Huge", &dirs), Err(ThemeError::TooLarge { .. })));
        assert_eq!(load_theme_from("Fits", &dirs).expect("loads").0.background, Some((0, 0, 1)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_theme_beats_the_defaults() {
        let (theme, _) = parse_theme(MOON);
        let from_theme = ColorScheme::from_theme(&theme);
        assert_eq!((from_theme.foreground, from_theme.background), ((0xc8, 0xd3, 0xf5), (0x22, 0x24, 0x36)));
        assert_eq!(from_theme.selection_background, Some((0x2d, 0x3f, 0x76)));
        assert_eq!(from_theme.palette.get(&15), Some(&(0xc8, 0xd3, 0xf5)));

        let (no_background, _) = parse_theme("foreground = #010203\n");
        let mixed = ColorScheme::from_theme(&no_background);
        assert_eq!((mixed.foreground, mixed.background), ((1, 2, 3), DEFAULT_BACKGROUND));

        let defaults = ColorScheme::default();
        assert_eq!((defaults.foreground, defaults.background), (DEFAULT_FOREGROUND, DEFAULT_BACKGROUND));
        assert!(defaults.palette.is_empty() && defaults.cursor.is_none() && defaults.selection_background.is_none());
    }

    #[test]
    fn a_theme_that_cannot_load_is_reported_by_name_and_leaves_the_defaults() {
        let config = ColorSettings { theme: Some("../escape".into()), ..ColorSettings::default() };
        let (scheme, problems) = ColorScheme::load(&config);
        assert_eq!(scheme, ColorScheme::default());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("theme \"../escape\": not a theme name"), "{problems:?}");
    }
}
