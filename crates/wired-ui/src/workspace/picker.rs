//! The profile picker: a filterable profile list under the title bar; the chosen one opens in a new tab.

use gpui::{
    Keystroke, OperatingSystem,
    component::input::{Input, InputEvent, InputState, MoveDown, MoveUp},
    hsla,
};

use super::*;

const WIDTH: f32 = 320.;
const ROW_HEIGHT: f32 = 30.;
const PADDING_X: f32 = 12.;
const ICON_SIZE: f32 = 16.;
const DROP: f32 = 12.;

const TEXT_REST: f32 = 0.55;
const TEXT_SELECTED: f32 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Open { since: Instant },
    Closing { since: Instant, shown_at_close: f32 },
}

pub(super) struct ProfilePicker {
    input: Entity<InputState>,
    selected_match: usize,
    phase: Phase,
    _events: Subscription,
}

impl ProfilePicker {
    fn is_open(&self) -> bool {
        matches!(self.phase, Phase::Open { .. })
    }

    fn shown(&self, now: Instant) -> f32 {
        self.phase.shown(now)
    }
}

impl Phase {
    fn shown(self, now: Instant) -> f32 {
        match self {
            Self::Open { since } => progress_over(Some(since), now, OVERLAY_ANIMATION),
            Self::Closing { since, shown_at_close } => {
                shown_at_close * (1. - progress_over(Some(since), now, OVERLAY_ANIMATION))
            }
        }
    }
}

fn matching<'a>(profiles: &'a [Arc<Profile>], filter: &str) -> Vec<&'a Arc<Profile>> {
    let filter = filter.trim().to_lowercase();
    profiles
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&filter) || p.id.as_str().to_lowercase().contains(&filter))
        .collect()
}

fn is_other_operation(keystroke: &Keystroke) -> bool {
    let m = &keystroke.modifiers;
    m.control || m.alt || m.platform
}

fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

impl Workspace {
    pub(super) fn is_picker_open(&self) -> bool {
        self.picker.as_ref().is_some_and(ProfilePicker::is_open)
    }

    pub(super) fn is_dimmed(&self, window: &Window) -> bool {
        self.is_picker_open() || crate::overlay::is_chord_held(window, self.hint.is_held())
    }

    pub(super) fn toggle_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_picker_open() {
            self.close_picker(true, window, cx);
        } else {
            self.open_picker(window, cx);
        }
    }

    fn open_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            self.finish_rename(true, false, window, cx);
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Open a profile"));
        input.update(cx, |state, cx| state.focus(window, cx));
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let selected = this.picker.as_ref().map_or(0, |p| p.selected_match);
                this.confirm_picker(selected, window, cx);
            }
            InputEvent::Change => {
                if let Some(picker) = &mut this.picker {
                    picker.selected_match = 0;
                }
                cx.notify();
            }
            InputEvent::Blur => {
                if window.is_window_active() {
                    this.close_picker(window.focused(cx).is_none(), window, cx);
                }
            }
            InputEvent::Focus => {}
        });
        let phase = Phase::Open { since: Instant::now() };
        self.picker = Some(ProfilePicker { input, selected_match: 0, phase, _events: events });
        cx.notify();
    }

    pub(super) fn close_picker(&mut self, refocus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let Some(picker) = self.picker.as_mut().filter(|p| p.is_open()) else { return };
        picker.phase = Phase::Closing { since: now, shown_at_close: picker.shown(now) };
        if refocus {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// Closes the picker on a key that is not its own; returns whether the key was taken.
    pub(super) fn on_picker_key(&mut self, keystroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // The key that toggles the picker closes it through its action.
        let is_toggle = Settings::get(cx).keys("profile-selector").iter().any(|bound| {
            Keystroke::parse_with_os(bound, OperatingSystem::Windows)
                .is_ok_and(|k| k.modifiers == keystroke.modifiers && k.key == keystroke.key)
        });
        if !self.is_picker_open() || !is_other_operation(keystroke) || is_toggle {
            return false;
        }
        self.close_picker(true, window, cx);
        // gpui chose the input's dispatch path before the interceptor changed focus.
        // Replay after this update so the terminal's bindings receive the key once.
        let handle = window.window_handle();
        let keystroke = keystroke.clone();
        cx.defer(move |cx| {
            let _ = handle.update(cx, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            });
        });
        true
    }

    fn picker_matches(&self, cx: &App) -> Vec<Arc<Profile>> {
        let Some(picker) = &self.picker else { return Vec::new() };
        let filter = picker.input.read(cx).value();
        matching(Settings::get(cx).profiles(), &filter).into_iter().cloned().collect()
    }

    fn confirm_picker(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_picker_open() {
            return;
        }
        let Some(profile) = self.picker_matches(cx).get(index).cloned() else { return };
        self.close_picker(false, window, cx);
        if let Err(error) = self.open_tab(Some(profile), window, cx) {
            self.show_error(format!("Cannot start the shell: {error:#}"), cx);
            self.focus_active(window, cx);
        }
    }

    fn step_picker(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.picker_matches(cx).len();
        let Some(picker) = &mut self.picker else { return };
        let (Ok(len), Ok(current)) = (isize::try_from(len), isize::try_from(picker.selected_match)) else { return };
        if len > 0 {
            picker.selected_match = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
            cx.notify();
        }
    }

    pub(super) fn picker_element(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let now = Instant::now();
        let picker = self.picker.as_ref()?;
        let shown = picker.shown(now);
        let is_open = picker.is_open();
        if !is_open && shown <= 0. {
            self.picker = None;
            return None;
        }
        if shown < 1. {
            window.request_animation_frame();
        }
        let found = self.picker_matches(cx);
        let default_id = Settings::get(cx).default_profile().map(|p| p.id.clone());
        let selected = picker.selected_match.min(found.len().saturating_sub(1));
        let rows = found.iter().enumerate().map(|(index, profile)| {
            let is_default = default_id.as_ref() == Some(&profile.id);
            div()
                .id(("profile", index))
                .h(px(ROW_HEIGHT))
                .px(px(PADDING_X))
                .flex()
                .items_center()
                .gap(px(10.))
                .text_color(white(if index == selected { TEXT_SELECTED } else { TEXT_REST }))
                .child(crate::profile_icon::element(profile.icon, ICON_SIZE))
                .child(div().flex_1().min_w_0().truncate().child(profile.name.clone()))
                .children(
                    is_default.then(|| div().flex_none().text_size(px(12.)).text_color(white(0.4)).child("default")),
                )
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if let Some(picker) = this.picker.as_mut().filter(|p| p.is_open() && p.selected_match != index) {
                        picker.selected_match = index;
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.confirm_picker(index, window, cx);
                    }),
                )
        });
        let empty = found.is_empty().then(|| {
            div().h(px(ROW_HEIGHT)).px(px(PADDING_X)).flex().items_center().text_color(white(0.4)).child("No profile")
        });
        let panel = crate::overlay::panel()
            .id("profile-picker")
            .w(px(WIDTH))
            .py(px(6.))
            .opacity(shown)
            .occlude()
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                cx.stop_propagation();
                this.step_picker(-1, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                cx.stop_propagation();
                this.step_picker(1, cx);
            }))
            .child(div().px(px(PADDING_X - 4.)).pb(px(4.)).child(Input::new(&picker.input).appearance(false)))
            .children(rows)
            .children(empty);
        let catcher = is_open.then(|| {
            div()
                .id("profile-picker-outside")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .occlude()
                .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.close_picker(true, window, cx);
                }))
                .on_scroll(cx.listener(|this, _, window, cx| this.close_picker(true, window, cx)))
        });
        let placed = div()
            .absolute()
            .top(px(title_bar::HEIGHT + 8. - DROP * (1. - shown)))
            .left_0()
            .w_full()
            .flex()
            .justify_center()
            .child(panel);
        Some(div().absolute().top_0().left_0().size_full().children(catcher).child(placed).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use wired_config::ProfileId;

    use super::*;

    fn profile(id: &str, name: &str) -> Arc<Profile> {
        Arc::new(Profile {
            id: ProfileId::new(id),
            name: name.into(),
            icon: None,
            program: PathBuf::from(r"C:\x.exe"),
            args: Vec::new(),
            env: Vec::new(),
        })
    }

    #[test]
    fn filter_matches_name_or_id_ignoring_case() {
        let profiles = [profile("builtin:pwsh", "PowerShell"), profile("builtin:git-bash", "Git Bash")];
        let names = |filter: &str| matching(&profiles, filter).iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(""), ["PowerShell", "Git Bash"]);
        assert_eq!(names("BASH"), ["Git Bash"]);
        assert_eq!(names("pwsh"), ["PowerShell"]);
        assert!(names("zsh").is_empty());
    }

    #[test]
    fn only_modified_keys_count_as_other_operations() {
        let key = |s: &'static str| Keystroke::parse(s).expect("keystroke");
        assert!(!is_other_operation(&key("a")));
        assert!(!is_other_operation(&key("down")));
        assert!(!is_other_operation(&key("shift-a")));
        assert!(is_other_operation(&key("ctrl-shift-t")));
        assert!(is_other_operation(&key("alt-1")));
    }

    #[test]
    fn closing_starts_where_opening_was() {
        let now = Instant::now();
        assert!((Phase::Open { since: now - OVERLAY_ANIMATION }.shown(now) - 1.).abs() < f32::EPSILON);
        assert!(Phase::Open { since: now }.shown(now).abs() < f32::EPSILON);
        let half = Phase::Closing { since: now, shown_at_close: 0.5 }.shown(now);
        assert!((half - 0.5).abs() < f32::EPSILON, "closing starts from where it was");
        assert!(Phase::Closing { since: now - OVERLAY_ANIMATION, shown_at_close: 0.5 }.shown(now).abs() < f32::EPSILON);
    }
}
