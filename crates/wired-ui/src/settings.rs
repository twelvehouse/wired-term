//! Everything the user configures (`config.toml`, `keybind.toml`), owned by one app-wide [`Settings`].

use std::{rc::Rc, sync::Arc};

use gpui::{Action, App, Global, KeyBinding, KeyBindingContextPredicate, Keystroke};
use wired_config::{ACTIONS, Bindings, ColorScheme, Config, KeyContext, MouseAction, Profile, WindowIcon};

use crate::workspace;

fn action_for(id: &str) -> Option<Box<dyn Action>> {
    use wired_terminal_view as term;
    use workspace as ws;
    let action: Box<dyn Action> = match id {
        "new-window" => Box::new(ws::NewWindow),
        "toggle-fullscreen" => Box::new(ws::ToggleFullscreen),
        "new-tab" => Box::new(ws::NewTab),
        "next-tab" => Box::new(ws::NextTab),
        "previous-tab" => Box::new(ws::PreviousTab),
        "move-tab-left" => Box::new(ws::MoveTabLeft),
        "move-tab-right" => Box::new(ws::MoveTabRight),
        "rename-tab" => Box::new(ws::RenameTab),
        "reload-config" => Box::new(ws::ReloadConfig),
        "profile-selector" => Box::new(ws::ProfileSelector),
        "tab-1" => Box::new(ws::ActivateTab1),
        "tab-2" => Box::new(ws::ActivateTab2),
        "tab-3" => Box::new(ws::ActivateTab3),
        "tab-4" => Box::new(ws::ActivateTab4),
        "tab-5" => Box::new(ws::ActivateTab5),
        "tab-6" => Box::new(ws::ActivateTab6),
        "tab-7" => Box::new(ws::ActivateTab7),
        "tab-8" => Box::new(ws::ActivateTab8),
        "tab-9" => Box::new(ws::ActivateTab9),
        "tab-10" => Box::new(ws::ActivateTab10),
        "split-right" => Box::new(ws::SplitRight),
        "split-bottom" => Box::new(ws::SplitBottom),
        "split-left" => Box::new(ws::SplitLeft),
        "split-top" => Box::new(ws::SplitTop),
        "close-pane" => Box::new(ws::ClosePane),
        "pane-nav-left" => Box::new(ws::FocusPaneLeft),
        "pane-nav-right" => Box::new(ws::FocusPaneRight),
        "pane-nav-up" => Box::new(ws::FocusPaneUp),
        "pane-nav-down" => Box::new(ws::FocusPaneDown),
        "pane-nav-previous" => Box::new(ws::FocusPreviousPane),
        "pane-nav-next" => Box::new(ws::FocusNextPane),
        "copy" => Box::new(term::Copy),
        "paste" => Box::new(term::Paste),
        "select-all" => Box::new(term::SelectAll),
        "zoom-in" => Box::new(term::ZoomIn),
        "zoom-out" => Box::new(term::ZoomOut),
        "reset-zoom" => Box::new(term::ResetZoom),
        "scroll-up" => Box::new(term::ScrollUp),
        "scroll-down" => Box::new(term::ScrollDown),
        "scroll-page-up" => Box::new(term::ScrollPageUp),
        "scroll-page-down" => Box::new(term::ScrollPageDown),
        "scroll-to-top" => Box::new(term::ScrollToTop),
        "scroll-to-bottom" => Box::new(term::ScrollToBottom),
        _ => return None,
    };
    Some(action)
}

fn predicate(context: KeyContext) -> Option<Rc<KeyBindingContextPredicate>> {
    let name = match context {
        KeyContext::Workspace => workspace::KEY_CONTEXT,
        KeyContext::Terminal => wired_terminal_view::KEY_CONTEXT,
    };
    KeyBindingContextPredicate::parse(name).ok().map(Rc::new)
}

static OUR_ACTION_NAMES: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| ACTIONS.iter().filter_map(|s| action_for(s.id)).map(|a| a.name()).collect());

/// The action of wired's own that `keystroke` runs in `context_stack`; these run on key release, as in Tabby.
pub(crate) fn shortcut_action(
    keystroke: &Keystroke,
    context_stack: &[gpui::KeyContext],
    cx: &App,
) -> Option<Box<dyn Action>> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let (bindings, _) = keymap.bindings_for_input(std::slice::from_ref(keystroke), context_stack);
    let action = bindings.first()?.action();
    is_held_until_release(action.name()).then(|| action.boxed_clone())
}

/// Whether wired's action `name` waits for its key to come up. Copy runs on the press: without a
/// selection it passes its key to the program (Ctrl+C), which only the key-down event can carry.
fn is_held_until_release(name: &str) -> bool {
    OUR_ACTION_NAMES.contains(&name) && name != wired_terminal_view::Copy::name_for_type()
}

/// The settings in effect, one per app.
#[derive(Debug, Default)]
pub struct Settings {
    config: Config,
    colors: ColorScheme,
    window_icon: WindowIcon,
    bindings: Bindings,
    profiles: Vec<Arc<Profile>>,
    default_profile: Option<Arc<Profile>>,
}

impl Global for Settings {}

impl Settings {
    /// The settings in effect (the defaults before [`Self::load`]).
    pub fn get(cx: &App) -> &Self {
        cx.try_global::<Self>().unwrap_or(&DEFAULT)
    }

    /// `config.toml`.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The terminal colors in effect.
    pub fn colors(&self) -> &ColorScheme {
        &self.colors
    }

    /// The window icon selected when the app started.
    pub fn window_icon(&self) -> WindowIcon {
        self.window_icon
    }

    /// The profiles a tab can run, in menu order.
    pub fn profiles(&self) -> &[Arc<Profile>] {
        &self.profiles
    }

    /// The profile new tabs use; `None` only when no shell was found.
    pub fn default_profile(&self) -> Option<Arc<Profile>> {
        self.default_profile.clone()
    }

    /// What mouse gesture `id` (an id from [`wired_config::MOUSE`]) does.
    pub fn mouse(&self, id: &str) -> MouseAction {
        self.bindings.mouse(id)
    }

    /// The keys bound to action `id` now (none for an unknown id).
    pub fn keys(&self, id: &str) -> Vec<&str> {
        ACTIONS.iter().find(|a| a.id == id).map_or_else(Vec::new, |spec| self.bindings.keys(spec))
    }

    /// Reads `config.toml` and `keybind.toml` and binds the keys; an unreadable file keeps what was in effect.
    pub fn load(cx: &mut App) -> LoadReport {
        let mut report = LoadReport::default();
        let previous = cx.try_global::<Self>();

        let config_path = wired_config::config_path();
        let is_created = prepare_file(
            &mut report,
            "config.toml",
            &config_path,
            wired_config::write_template_if_missing,
            wired_config::add_missing_settings,
        );
        if is_created && let Some(reason) = &wired_config::config_location().fallback_reason {
            report.warnings.push(reason.clone());
        }
        // For completion and checks in TOML editors; wired itself does not read it.
        if let Err(error) = wired_config::write_schema_if_outdated(&wired_config::schema_path()) {
            report.errors.push(format!("cannot write config.schema.json: {error}"));
        }
        match wired_config::add_schema_directive(&config_path) {
            Ok(true) => report.notes.push("config.toml: added #:schema for completion in TOML editors".into()),
            Ok(false) => {}
            Err(error) => report.errors.push(format!("config.toml: cannot add #:schema: {error}")),
        }
        let config = match wired_config::load_checked(&config_path) {
            Ok((config, problems)) => {
                report.warnings.extend(problems.into_iter().map(|p| format!("config.toml: {p}")));
                config
            }
            Err(error) => {
                report.errors.push(format!("{error}; keeping the previous settings"));
                previous.map(|p| p.config.clone()).unwrap_or_default()
            }
        };
        let (colors, problems) = ColorScheme::load(&config.colors);
        report.warnings.extend(problems.into_iter().map(|p| format!("config.toml: colors.{p}")));
        let window_icon = previous.map_or_else(|| config.appearance.icon.resolve(colors.background), Self::window_icon);

        let keybind_path = wired_config::keybind_path();
        prepare_file(
            &mut report,
            "keybind.toml",
            &keybind_path,
            wired_config::write_keybind_template_if_missing,
            wired_config::add_missing_bindings,
        );
        let bindings = match wired_config::load_bindings(&keybind_path) {
            Ok(bindings) => bindings,
            Err(error) => {
                report.errors.push(format!("{error}; keeping the previous bindings"));
                previous.map(|p| p.bindings.clone()).unwrap_or_default()
            }
        };
        report.warnings.extend(bindings.problems().iter().map(|p| format!("keybind.toml: {p}")));

        let mut keys = Vec::new();
        for spec in ACTIONS {
            for keystrokes in bindings.keys(spec) {
                let Some(action) = action_for(spec.id) else { continue };
                match KeyBinding::load(keystrokes, action, predicate(spec.context), false, None, cx) {
                    Ok(binding) => keys.push(binding),
                    Err(error) => {
                        report.warnings.push(format!("keybind.toml: {}: \"{keystrokes}\": {error}", spec.id));
                    }
                }
            }
        }
        // Only wired's own actions are replaced; bindings other crates registered stay.
        cx.retain_key_bindings(|binding| !OUR_ACTION_NAMES.contains(&binding.action().name()));
        cx.bind_keys(keys);

        let profiles: Vec<Arc<Profile>> = wired_platform::detect_shells().into_iter().map(Arc::new).collect();
        let default_profile = match wired_config::default_profile(&profiles, config.general.default_profile.as_ref()) {
            Some((profile, message)) => {
                report.warnings.extend(message.map(|m| format!("config.toml: general.default_profile: {m}")));
                Some(Arc::clone(profile))
            }
            None => {
                report.errors.push("no shell was found".into());
                None
            }
        };

        cx.set_global(Self { config, colors, window_icon, bindings, profiles, default_profile });
        report
    }
}

/// What [`Settings::load`] has to tell the user.
#[derive(Debug, Default)]
pub struct LoadReport {
    /// Files that could not be read or written; the settings in effect before stay.
    pub errors: Vec<String>,
    /// Entries that could not be used as written and were replaced or ignored.
    pub warnings: Vec<String>,
    /// Things done to the files (new settings added).
    pub notes: Vec<String>,
}

/// Returns whether the file was created.
fn prepare_file(
    report: &mut LoadReport,
    name: &str,
    path: &std::path::Path,
    write_template: fn(&std::path::Path) -> std::io::Result<bool>,
    add_missing: fn(&std::path::Path) -> std::io::Result<Vec<String>>,
) -> bool {
    match write_template(path) {
        Ok(true) => return true,
        Ok(false) => {}
        Err(error) => {
            report.errors.push(format!("cannot create {}: {error}", path.display()));
            return false;
        }
    }
    match add_missing(path) {
        Ok(added) if !added.is_empty() => report.notes.push(format!("{name}: added {}", added.join(", "))),
        Ok(_) => {}
        Err(error) => report.errors.push(format!("{name}: cannot add new entries: {error}")),
    }
    false
}

static DEFAULT: std::sync::LazyLock<Settings> = std::sync::LazyLock::new(Settings::default);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_id_has_an_action() {
        for spec in ACTIONS {
            assert!(action_for(spec.id).is_some(), "{}", spec.id);
        }
    }

    #[test]
    fn copy_runs_on_press_so_its_key_can_reach_the_program() {
        let name = |id| action_for(id).expect("known action").name();
        assert!(!is_held_until_release(name("copy")));
        for id in ["split-right", "paste", "new-tab", "reload-config"] {
            assert!(is_held_until_release(name(id)), "{id}");
        }
        assert!(!is_held_until_release("editor::Unrelated"));
    }
}
