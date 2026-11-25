//! `<install dir>\config\keybind.toml`: key bindings by action id, only those that differ from
//! the defaults (Tabby's Windows defaults).
//!
//! An action listed in `[bindings]` replaces its default keys entirely; `[]` unbinds it. The file
//! is untrusted input: unknown actions are reported, not fatal, and sizes are bounded.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{Error, Result, template::Line};

/// Where an action's keys are active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyContext {
    /// Anywhere in the window.
    Workspace,
    /// Only while a terminal pane has the focus; such keys never reach the shell.
    Terminal,
}

/// An action that keys can be bound to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSpec {
    /// Stable id used in `keybind.toml` (after Tabby's hotkey ids).
    pub id: &'static str,
    /// Where its keys are active.
    pub context: KeyContext,
    /// Default keys, in gpui syntax (`ctrl-shift-c`).
    pub defaults: &'static [&'static str],
    /// One line for the template.
    pub description: &'static str,
}

const fn spec(
    id: &'static str,
    context: KeyContext,
    defaults: &'static [&'static str],
    description: &'static str,
) -> ActionSpec {
    ActionSpec { id, context, defaults, description }
}

use KeyContext::{Terminal as T, Workspace as W};

/// Every bindable action with its default keys: the single list the keymap, the template and
/// validation come from.
pub const ACTIONS: &[ActionSpec] = &[
    spec("new-window", W, &["ctrl-shift-n"], "open a window with the default shell"),
    spec("new-tab", W, &["ctrl-shift-t"], "open a tab with the default shell"),
    spec("toggle-fullscreen", W, &["f11", "alt-enter"], "enter or leave fullscreen"),
    spec("next-tab", W, &["ctrl-tab", "ctrl-shift-right"], "next tab"),
    spec("previous-tab", W, &["ctrl-shift-tab", "ctrl-shift-left"], "previous tab"),
    spec("move-tab-left", W, &["ctrl-shift-pageup"], "move the tab one place left"),
    spec("move-tab-right", W, &["ctrl-shift-pagedown"], "move the tab one place right"),
    spec("rename-tab", W, &["ctrl-shift-f2"], "rename the active tab (or double-click it)"),
    spec("reload-config", W, &["ctrl-shift-r"], "reload config.toml and keybind.toml"),
    spec("profile-selector", W, &["ctrl-shift-e"], "choose a profile to open in a new tab"),
    spec("tab-1", W, &["alt-1"], "tab 1"),
    spec("tab-2", W, &["alt-2"], "tab 2"),
    spec("tab-3", W, &["alt-3"], "tab 3"),
    spec("tab-4", W, &["alt-4"], "tab 4"),
    spec("tab-5", W, &["alt-5"], "tab 5"),
    spec("tab-6", W, &["alt-6"], "tab 6"),
    spec("tab-7", W, &["alt-7"], "tab 7"),
    spec("tab-8", W, &["alt-8"], "tab 8"),
    spec("tab-9", W, &["alt-9"], "tab 9"),
    spec("tab-10", W, &["alt-0"], "tab 10"),
    spec("split-right", T, &["ctrl-shift-s"], "split, new pane on the right"),
    spec("split-bottom", T, &["ctrl-shift-d"], "split, new pane below"),
    spec("split-left", T, &[], "split, new pane on the left"),
    spec("split-top", T, &[], "split, new pane above"),
    spec("close-pane", T, &["ctrl-shift-w"], "close the pane (its tab with the last pane)"),
    spec("pane-nav-left", T, &["ctrl-alt-left"], "focus the pane to the left"),
    spec("pane-nav-right", T, &["ctrl-alt-right"], "focus the pane to the right"),
    spec("pane-nav-up", T, &["ctrl-alt-up"], "focus the pane above"),
    spec("pane-nav-down", T, &["ctrl-alt-down"], "focus the pane below"),
    spec("pane-nav-previous", T, &["ctrl-alt-["], "focus the previous pane"),
    spec("pane-nav-next", T, &["ctrl-alt-]"], "focus the next pane"),
    spec("copy", T, &["ctrl-shift-c"], "copy the selection; without one the key goes to the shell"),
    spec("paste", T, &["ctrl-shift-v", "shift-insert"], "paste"),
    spec("select-all", T, &["ctrl-shift-a"], "select the screen and scrollback"),
    spec("zoom-in", T, &["ctrl-=", "ctrl-shift-=", "ctrl-+"], "larger font"),
    spec("zoom-out", T, &["ctrl--", "ctrl-shift--"], "smaller font"),
    spec("reset-zoom", T, &["ctrl-0"], "configured font size"),
    spec("scroll-up", T, &["ctrl-shift-up"], "scroll up one line"),
    spec("scroll-down", T, &["ctrl-shift-down"], "scroll down one line"),
    spec("scroll-page-up", T, &["alt-pageup"], "scroll up one page"),
    spec("scroll-page-down", T, &["alt-pagedown"], "scroll down one page"),
    spec("scroll-to-top", T, &["ctrl-pageup"], "scroll to the oldest line"),
    spec("scroll-to-bottom", T, &["ctrl-pagedown"], "scroll to the live screen"),
];

/// What a mouse trigger does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    /// Copy the selection if there is one, otherwise paste (Windows Terminal's right-click).
    CopyOrPaste,
    /// Paste.
    Paste,
    /// Close the tab.
    CloseTab,
    /// Scroll the pane row under the pointer sideways.
    RowScroll,
    /// Widen or narrow the pane row under the pointer.
    RowWidth,
    /// Give every pane of the divider's row or column the same size.
    Equalize,
    /// Nothing.
    None,
}

impl MouseAction {
    #[cfg(test)]
    const ALL: [Self; 7] =
        [Self::CopyOrPaste, Self::Paste, Self::CloseTab, Self::RowScroll, Self::RowWidth, Self::Equalize, Self::None];

    /// Its id in `keybind.toml`.
    pub const fn id(self) -> &'static str {
        match self {
            Self::CopyOrPaste => "copy-or-paste",
            Self::Paste => "paste",
            Self::CloseTab => "close-tab",
            Self::RowScroll => "row-scroll",
            Self::RowWidth => "row-width",
            Self::Equalize => "equalize",
            Self::None => "none",
        }
    }
}

/// A mouse or wheel gesture that can be given an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseSpec {
    /// Stable id used in `keybind.toml`'s `[mouse]` table.
    pub id: &'static str,
    /// Action without an entry in the file.
    pub default: MouseAction,
    /// Actions that fit this gesture (a click cannot widen a row).
    pub allowed: &'static [MouseAction],
    /// One line for the template.
    pub description: &'static str,
}

/// Ids of the mouse gestures, for looking them up ([`Bindings::mouse`]).
pub mod gesture {
    /// Right-click in a terminal.
    pub const RIGHT_CLICK: &str = "right-click";
    /// Middle-click on a tab.
    pub const TAB_MIDDLE_CLICK: &str = "tab-middle-click";
    /// Shift+wheel over the panes.
    pub const SHIFT_WHEEL: &str = "shift-wheel";
    /// Ctrl+Shift+wheel over the panes.
    pub const CTRL_SHIFT_WHEEL: &str = "ctrl-shift-wheel";
    /// Double-click on a divider.
    pub const DIVIDER_DOUBLE_CLICK: &str = "divider-double-click";
}

/// Every configurable mouse gesture: the single list behind the mouse handling, the template
/// and validation.
pub const MOUSE: &[MouseSpec] = &[
    MouseSpec {
        id: gesture::RIGHT_CLICK,
        default: MouseAction::CopyOrPaste,
        allowed: &[MouseAction::CopyOrPaste, MouseAction::Paste, MouseAction::None],
        description: "in a terminal",
    },
    MouseSpec {
        id: gesture::TAB_MIDDLE_CLICK,
        default: MouseAction::CloseTab,
        allowed: &[MouseAction::CloseTab, MouseAction::None],
        description: "on a tab",
    },
    MouseSpec {
        id: gesture::SHIFT_WHEEL,
        default: MouseAction::RowScroll,
        allowed: &[MouseAction::RowScroll, MouseAction::None],
        description: "over the panes (a sideways wheel always scrolls the row)",
    },
    MouseSpec {
        id: gesture::CTRL_SHIFT_WHEEL,
        default: MouseAction::RowWidth,
        allowed: &[MouseAction::RowWidth, MouseAction::None],
        description: "over the panes",
    },
    MouseSpec {
        id: gesture::DIVIDER_DOUBLE_CLICK,
        default: MouseAction::Equalize,
        allowed: &[MouseAction::Equalize, MouseAction::None],
        description: "on the line between panes",
    },
];

const MODIFIERS: &[&str] = &["ctrl", "control", "alt", "shift", "cmd", "super", "win", "fn", "secondary"];
/// Key names gpui produces on Windows besides single characters and F1..F24.
const NAMED_KEYS: &[&str] = &[
    "space",
    "backspace",
    "enter",
    "tab",
    "up",
    "down",
    "right",
    "left",
    "home",
    "end",
    "pageup",
    "pagedown",
    "back",
    "forward",
    "escape",
    "insert",
    "delete",
    "menu",
];

/// Whether `text` is a key gpui can produce (`ctrl-shift-c`, `alt-pageup`, `ctrl--`), or a
/// sequence of them separated by spaces. gpui itself accepts any word as a key name, so a typo
/// would silently bind nothing.
fn is_keystroke(text: &str) -> bool {
    let is_key = |key: &str| {
        key.chars().count() == 1
            || NAMED_KEYS.contains(&key)
            || key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n))
    };
    let stroke_ok = |stroke: &str| {
        let mut rest = stroke;
        while let Some((modifier, tail)) = rest.split_once('-')
            && MODIFIERS.contains(&modifier)
            && !tail.is_empty()
        {
            rest = tail;
        }
        is_key(rest)
    };
    !text.trim().is_empty() && text.split_whitespace().all(stroke_ok)
}

const MAX_ENTRIES: usize = 256;
const MAX_KEYS_PER_ACTION: usize = 8;
const MAX_KEY_LEN: usize = 64;

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    bindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    mouse: BTreeMap<String, String>,
    /// Top-level tables and keys other than the two above, to report them.
    #[serde(flatten)]
    other: BTreeMap<String, toml::Value>,
}

/// The user's key and mouse bindings (`keybind.toml`): overrides of the defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bindings {
    keys: BTreeMap<String, Vec<String>>,
    mouse: BTreeMap<&'static str, MouseAction>,
    /// What in the file could not be used, for the user.
    problems: Vec<String>,
}

impl Bindings {
    /// Parses `keybind.toml` text. Unknown tables, entries naming no action or gesture, and
    /// mouse actions that do not fit their gesture are dropped; entries over the limits are cut.
    /// All of it is reported (see [`Self::problems`]).
    pub fn from_toml(text: &str, path: &Path) -> Result<Self> {
        let file: File = toml::from_str(text).map_err(|e| crate::parse_error(path, text, &e))?;
        let mut bindings = Self::default();
        let problems = &mut bindings.problems;
        for name in file.other.keys().take(MAX_ENTRIES) {
            problems.push(format!("unknown table or key \"{name}\" (only [bindings] and [mouse] are read)"));
        }
        for (table, count) in [("bindings", file.bindings.len()), ("mouse", file.mouse.len())] {
            if count > MAX_ENTRIES {
                problems.push(format!("[{table}]: only the first {MAX_ENTRIES} of {count} entries are used"));
            }
        }
        for (id, mut keys) in file.bindings.into_iter().take(MAX_ENTRIES) {
            if ACTIONS.iter().all(|a| a.id != id) {
                problems.push(format!("unknown action \"{id}\""));
                continue;
            }
            let count = keys.len();
            keys.retain(|k| k.len() <= MAX_KEY_LEN);
            if keys.len() != count {
                problems.push(format!("{id}: keys longer than {MAX_KEY_LEN} characters are ignored"));
            }
            for bad in keys.iter().filter(|k| !is_keystroke(k)) {
                problems.push(format!("{id}: \"{bad}\" is not a key"));
            }
            keys.retain(|k| is_keystroke(k));
            if keys.len() > MAX_KEYS_PER_ACTION {
                problems.push(format!("{id}: only the first {MAX_KEYS_PER_ACTION} keys are used"));
                keys.truncate(MAX_KEYS_PER_ACTION);
            }
            bindings.keys.insert(id, keys);
        }
        for (id, action) in file.mouse.into_iter().take(MAX_ENTRIES) {
            let Some(spec) = MOUSE.iter().find(|m| m.id == id) else {
                problems.push(format!("unknown mouse gesture \"{id}\""));
                continue;
            };
            match spec.allowed.iter().find(|a| a.id() == action) {
                Some(allowed) => {
                    bindings.mouse.insert(spec.id, *allowed);
                }
                None => {
                    let names: Vec<&str> = spec.allowed.iter().map(|a| a.id()).collect();
                    problems.push(format!("{id}: \"{action}\" is not one of {}", names.join(", ")));
                }
            }
        }
        Ok(bindings)
    }

    /// The keys bound to `action`: the user's list if the file has one, else the defaults.
    pub fn keys(&self, action: &ActionSpec) -> Vec<&str> {
        match self.keys.get(action.id) {
            Some(keys) => keys.iter().map(String::as_str).collect(),
            None => action.defaults.to_vec(),
        }
    }

    /// What mouse gesture `id` does (`None` for an unknown id).
    pub fn mouse(&self, id: &str) -> MouseAction {
        let spec = MOUSE.iter().find(|m| m.id == id);
        self.mouse.get(id).copied().or(spec.map(|s| s.default)).unwrap_or(MouseAction::None)
    }

    /// What in the file could not be used.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}

/// `config_dir()\keybind.toml`.
pub fn keybind_path() -> PathBuf {
    crate::config_dir().join("keybind.toml")
}

/// Loads the bindings; a missing file means the defaults.
pub fn load_bindings(path: &Path) -> Result<Bindings> {
    match crate::read_optional(path) {
        Ok(Some(text)) => Bindings::from_toml(&text, path),
        Ok(None) => Ok(Bindings::default()),
        Err(source) => Err(Error::Read { path: path.to_path_buf(), source }),
    }
}

const KEYBIND_HEADER: &str = r#"# wired key and mouse bindings. Edit and press Ctrl+Shift+R to apply.
#
# [bindings]: list only actions whose keys you want to change: a listed action replaces its
# default keys entirely, `[]` unbinds it. Keys are written like "ctrl-shift-c", "alt-pageup",
# "shift-insert"; several per action are allowed. Terminal keys are taken from the shell
# (Ctrl+R bound here no longer reaches it).
# [mouse]: what mouse gestures do; "none" turns one off.
"#;

/// Every action and mouse gesture with its default, commented out: the single list the
/// template and the "add new entries to an existing file" step come from.
pub(crate) fn keybind_lines() -> Vec<Line> {
    let mut lines = Vec::new();
    for action in ACTIONS {
        let keys: Vec<String> = action.defaults.iter().map(|k| format!("\"{k}\"")).collect();
        let entry = format!("# {} = [{}]", action.id, keys.join(", "));
        lines.push(Line { section: "bindings", key: action.id, text: format!("{entry:<56} # {}", action.description) });
    }
    for spec in MOUSE {
        let entry = format!("# {} = \"{}\"", spec.id, spec.default.id());
        let choices: Vec<&str> = spec.allowed.iter().map(|a| a.id()).collect();
        let text = format!("{entry:<56} # {}: {}", spec.description, choices.join(" | "));
        lines.push(Line { section: "mouse", key: spec.id, text });
    }
    lines
}

/// Text written when no `keybind.toml` exists: every action and gesture with its default,
/// commented out, as a reference.
pub(crate) fn keybind_template() -> String {
    crate::template::render(KEYBIND_HEADER, &keybind_lines())
}

/// Writes [`keybind_template`] to `path` if no file exists there. Returns whether it was written.
pub fn write_keybind_template_if_missing(path: &Path) -> std::io::Result<bool> {
    crate::write_if_missing(path, &keybind_template())
}

/// Adds the actions and mouse gestures `keybind.toml` lacks, commented out; returns the keys
/// added.
pub fn add_missing_bindings(path: &Path) -> std::io::Result<Vec<String>> {
    crate::add_missing_lines(path, &keybind_lines())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Bindings {
        Bindings::from_toml(text, Path::new("keybind.toml")).expect("parse")
    }

    fn action(id: &str) -> &'static ActionSpec {
        ACTIONS.iter().find(|a| a.id == id).expect("action")
    }

    #[test]
    fn the_template_changes_nothing() {
        assert_eq!(parse(&keybind_template()), Bindings::default());
    }

    #[test]
    fn listed_actions_replace_their_defaults_and_empty_unbinds() {
        let bindings = parse("[bindings]\nsplit-right = [\"ctrl-shift-e\"]\nnew-tab = []\n");
        assert_eq!(bindings.keys(action("split-right")), ["ctrl-shift-e"]);
        assert!(bindings.keys(action("new-tab")).is_empty());
        assert_eq!(bindings.keys(action("paste")), ["ctrl-shift-v", "shift-insert"]);
    }

    #[test]
    fn unknown_actions_are_reported_and_sizes_bounded() {
        let long = "x".repeat(MAX_KEY_LEN + 1);
        let many = ["\"a\""; MAX_KEYS_PER_ACTION + 3].join(", ");
        let bindings =
            parse(&format!("[bindings]\nnope = [\"a\"]\ncopy = [\"{long}\", \"ctrl-c\"]\npaste = [{many}]\n"));
        assert_eq!(
            bindings.problems(),
            [
                "copy: keys longer than 64 characters are ignored",
                "unknown action \"nope\"",
                "paste: only the first 8 keys are used",
            ]
        );
        assert_eq!(bindings.keys(action("copy")), ["ctrl-c"]);
        assert_eq!(bindings.keys(action("paste")).len(), MAX_KEYS_PER_ACTION);
    }

    #[test]
    fn unknown_tables_and_too_many_entries_are_reported() {
        let entries: String = (0..=MAX_ENTRIES).map(|i| format!("a{i} = []\n")).collect();
        let bindings = parse(&format!("theme = 1\n[keys]\nx = 1\n[bindings]\n{entries}"));
        let problems = bindings.problems();
        assert!(problems.iter().any(|p| p.starts_with("unknown table or key \"keys\"")), "{problems:?}");
        assert!(problems.iter().any(|p| p.starts_with("unknown table or key \"theme\"")), "{problems:?}");
        assert!(problems.contains(&format!("[bindings]: only the first 256 of {} entries are used", MAX_ENTRIES + 1)));
    }

    #[test]
    fn mouse_gestures_take_fitting_actions_only() {
        let bindings = parse("[mouse]\nright-click = \"paste\"\nshift-wheel = \"equalize\"\nnope = \"none\"\n");
        assert_eq!(bindings.mouse("right-click"), MouseAction::Paste);
        assert_eq!(bindings.mouse("shift-wheel"), MouseAction::RowScroll);
        assert_eq!(bindings.mouse("divider-double-click"), MouseAction::Equalize);
        assert_eq!(bindings.problems().len(), 2, "{:?}", bindings.problems());
    }

    #[test]
    fn defaults_are_allowed_and_action_ids_unique() {
        for spec in MOUSE {
            assert!(spec.allowed.contains(&spec.default), "{}", spec.id);
        }
        for (i, a) in MouseAction::ALL.iter().enumerate() {
            assert!(MouseAction::ALL[i + 1..].iter().all(|b| b.id() != a.id()));
        }
    }

    #[test]
    fn keys_are_checked() {
        for good in [
            "ctrl-shift-c",
            "ctrl--",
            "ctrl-shift-=",
            "alt-pageup",
            "shift-insert",
            "f11",
            "ctrl-alt-[",
            "ctrl-k ctrl-s",
        ] {
            assert!(is_keystroke(good), "{good}");
        }
        for bad in ["ctrl-shift-nokey", "", "ctrl-", "ctrl-f25", "hyper-c"] {
            assert!(!is_keystroke(bad), "{bad}");
        }
        let bindings = parse("[bindings]\ncopy = [\"ctrl-shift-nokey\", \"ctrl-c\"]\n");
        assert_eq!(bindings.keys(action("copy")), ["ctrl-c"]);
        assert_eq!(bindings.problems().len(), 1);
    }

    #[test]
    fn defaults_are_valid_keys() {
        for action in ACTIONS {
            assert!(action.defaults.iter().all(|k| is_keystroke(k)), "{}", action.id);
        }
    }

    #[test]
    fn ids_are_unique() {
        for (i, a) in ACTIONS.iter().enumerate() {
            assert!(ACTIONS[i + 1..].iter().all(|b| b.id != a.id), "{}", a.id);
        }
    }
}
