//! The shortcut hint shown while Ctrl and Shift are held alone.

use gpui::{Div, Modifiers};

use super::*;
use crate::overlay::{self, Edge, Icon, Label, Labels};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Chord {
    #[default]
    Released,
    Held {
        since: Instant,
    },
    /// An operation or another modifier dismissed the hint until Ctrl and Shift are let go.
    Used,
}

/// The pane a hint belongs to, independent of subsequent focus changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HintTarget {
    pub(super) tab_id: u64,
    pub(super) pane_id: PaneId,
}

#[derive(Debug, Clone, Copy)]
struct Fade {
    from_opacity: f32,
    started: Instant,
}

/// Tracks Ctrl+Shift held alone and the hint's opacity; input methods return whether to redraw.
#[derive(Debug, Default)]
pub(super) struct ShortcutHint {
    chord: Chord,
    target: Option<HintTarget>,
    fading: Option<Fade>,
}

impl ShortcutHint {
    /// Whether the chord is held without an operation dismissing it.
    pub(super) fn is_held(&self) -> bool {
        matches!(self.chord, Chord::Held { .. })
    }

    /// The original pane in this tab while the hint remains visible.
    pub(super) fn target(&self, tab_id: u64, now: Instant) -> Option<PaneId> {
        self.target.filter(|target| target.tab_id == tab_id && self.opacity(now).0 > 0.).map(|target| target.pane_id)
    }

    /// Opacity of the hint at `now`, and whether it is still changing (frames are needed).
    pub(super) fn opacity(&self, now: Instant) -> (f32, bool) {
        match (self.chord, self.fading) {
            (Chord::Held { since }, _) => {
                (progress_over(Some(since), now, OVERLAY_ANIMATION), now < since + OVERLAY_ANIMATION)
            }
            (_, Some(fade)) => (
                fade.from_opacity * (1. - progress_over(Some(fade.started), now, OVERLAY_ANIMATION)),
                now < fade.started + OVERLAY_ANIMATION,
            ),
            _ => (0., false),
        }
    }

    fn start_fading(&mut self, now: Instant) {
        let (opacity, _) = self.opacity(now);
        self.fading = (opacity > 0.).then_some(Fade { from_opacity: opacity, started: now });
    }

    pub(super) fn modifiers_changed(
        &mut self,
        modifiers: &Modifiers,
        now: Instant,
        target: Option<HintTarget>,
    ) -> bool {
        let both = modifiers.control && modifiers.shift;
        let is_alone = both && !modifiers.alt && !modifiers.platform && !modifiers.function;
        match self.chord {
            Chord::Released if is_alone => {
                if self.opacity(now).0 == 0. {
                    self.target = target;
                }
                self.chord = Chord::Held { since: now };
                self.fading = None;
                true
            }
            Chord::Released if both => {
                self.chord = Chord::Used;
                false
            }
            Chord::Held { .. } if !is_alone => {
                self.start_fading(now);
                self.chord = if both { Chord::Used } else { Chord::Released };
                true
            }
            Chord::Used if !modifiers.control && !modifiers.shift => {
                self.chord = Chord::Released;
                false
            }
            _ => false,
        }
    }

    pub(super) fn operation_performed(&mut self, now: Instant) -> bool {
        let was_held = self.is_held();
        if was_held {
            self.start_fading(now);
            self.chord = Chord::Used;
        }
        was_held
    }

    pub(super) fn window_deactivated(&mut self, now: Instant) -> bool {
        let was_held = self.is_held();
        if was_held {
            self.start_fading(now);
        }
        self.chord = Chord::Released;
        was_held
    }
}

fn describe_keys(keystrokes: &str) -> String {
    keystrokes.split_whitespace().map(describe_stroke).collect::<Vec<_>>().join(" ")
}

fn describe_stroke(stroke: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut rest = stroke;
    while let Some((modifier, tail)) = rest.split_once('-')
        && !tail.is_empty()
        && let Some(name) = modifier_name(modifier)
    {
        parts.push(name.to_owned());
        rest = tail;
    }
    parts.push(match rest {
        "pageup" => "PageUp".to_owned(),
        "pagedown" => "PageDown".to_owned(),
        _ => {
            let mut chars = rest.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        }
    });
    parts.join("+")
}

fn modifier_name(modifier: &str) -> Option<&'static str> {
    Some(match modifier {
        "ctrl" | "control" | "secondary" => "Ctrl",
        "alt" => "Alt",
        "shift" => "Shift",
        "cmd" | "super" | "win" => "Win",
        "fn" => "Fn",
        _ => return None,
    })
}

fn pane_labels(keys: impl Fn(&str) -> Vec<String>) -> Labels {
    let label = |id: &str, icon| {
        let bound = keys(id);
        let keys = bound.iter().map(|k| describe_keys(k)).collect::<Vec<_>>().join(" / ");
        (!bound.is_empty()).then_some(Label { icon, keys, text: None })
    };
    Labels {
        above: label("close-pane", Icon::Close),
        right: label("split-right", Icon::SplitRight),
        below: label("split-bottom", Icon::SplitDown),
    }
}

/// Its icon cannot tell what the wheel does, so the label says it.
fn row_width_label(action: MouseAction) -> Option<Label> {
    (action == MouseAction::RowWidth).then(|| Label {
        icon: Icon::Mouse,
        keys: "Ctrl+Shift+Wheel".to_owned(),
        text: Some("Adjust scroll group width"),
    })
}

/// The side farther from `focused`'s center, so the label stays clear of the pane in use.
fn row_width_edge(area_height: f32, focused: Rect) -> Edge {
    let center = focused.y + focused.height / 2.;
    if center > area_height - center { Edge::Top } else { Edge::Bottom }
}

impl Workspace {
    pub(super) fn on_modifiers_changed(&mut self, modifiers: &Modifiers, cx: &mut Context<Self>) {
        let target = self.tabs.get(self.active).map(|tab| HintTarget { tab_id: tab.id, pane_id: tab.panes.focused() });
        self.hint.modifiers_changed(modifiers, Instant::now(), target);
        cx.notify();
    }

    pub(super) fn on_hint_operation(&mut self, cx: &mut Context<Self>) {
        if self.hint.operation_performed(Instant::now()) {
            cx.notify();
        }
    }

    pub(super) fn reset_hint(&mut self, cx: &mut Context<Self>) {
        self.hint.window_deactivated(Instant::now());
        cx.notify();
    }

    pub(super) fn pane_labels(&self, cx: &App) -> Labels {
        let settings = Settings::get(cx);
        pane_labels(|id| settings.keys(id).into_iter().map(str::to_owned).collect())
    }

    pub(super) fn row_width_hint(&self, area: Rect, focused: Rect, opacity: f32, cx: &App) -> Option<Div> {
        let label = row_width_label(Settings::get(cx).mouse(gesture::CTRL_SHIFT_WHEEL))?;
        Some(overlay::pinned(area, row_width_edge(area.height, focused), label, opacity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL_SHIFT: Modifiers =
        Modifiers { control: true, alt: false, shift: true, platform: false, function: false };
    const NONE: Modifiers = Modifiers { control: false, alt: false, shift: false, platform: false, function: false };

    #[test]
    fn an_operation_removes_dimming_before_the_hint_finishes_fading() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        assert!(hint.is_held());
        let operated = start + OVERLAY_ANIMATION;
        assert!(hint.operation_performed(operated));
        assert!(!hint.is_held());
        assert_eq!(hint.opacity(operated).0, 1.);
        hint.modifiers_changed(&CTRL_SHIFT, operated + OVERLAY_ANIMATION, None);
        assert!(!hint.is_held(), "holding the chord cannot restore the background");
        hint.modifiers_changed(&NONE, operated + OVERLAY_ANIMATION, None);
        hint.modifiers_changed(&CTRL_SHIFT, operated + OVERLAY_ANIMATION * 2, None);
        assert!(hint.is_held(), "a fresh chord restores the background");
    }

    #[test]
    fn closing_the_target_does_not_move_fading_labels_to_the_next_pane() {
        let start = Instant::now();
        let mut panes = PaneTree::new(());
        let first = panes.focused();
        let second = panes.split(first, Side::Right, ()).expect("split");
        panes.focus(first);
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, Some(HintTarget { tab_id: 7, pane_id: first }));
        let pressed = start + OVERLAY_ANIMATION;
        hint.operation_performed(pressed);
        panes.close(first, Rect::new(0., 0., 800., 600.), &LayoutParams::default());
        assert_eq!(panes.focused(), second);
        let target = hint.target(7, pressed + OVERLAY_ANIMATION / 2);
        assert_eq!(target, Some(first));
        assert!(target.and_then(|id| panes.get(id)).is_none(), "closed pane has no labels");
        assert_eq!(hint.target(8, pressed + OVERLAY_ANIMATION / 2), None, "another tab cannot inherit the labels");
        assert_eq!(hint.target(7, pressed + OVERLAY_ANIMATION), None);
    }

    #[test]
    fn focus_changes_and_represses_keep_the_target_until_the_hint_disappears() {
        let start = Instant::now();
        let mut panes = PaneTree::new(());
        let first = panes.focused();
        let second = panes.split(first, Side::Right, ()).expect("split");
        let first_target = Some(HintTarget { tab_id: 7, pane_id: first });
        let second_target = Some(HintTarget { tab_id: 7, pane_id: second });
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, first_target);
        hint.modifiers_changed(&CTRL_SHIFT, start, second_target);
        assert_eq!(hint.target(7, start + OVERLAY_ANIMATION), Some(first));
        hint.modifiers_changed(&NONE, start + OVERLAY_ANIMATION, second_target);
        hint.modifiers_changed(&CTRL_SHIFT, start + OVERLAY_ANIMATION * 3 / 2, second_target);
        assert_eq!(hint.target(7, start + OVERLAY_ANIMATION * 2), Some(first));
        hint.modifiers_changed(&NONE, start + OVERLAY_ANIMATION * 3, second_target);
        hint.modifiers_changed(&CTRL_SHIFT, start + OVERLAY_ANIMATION * 4, second_target);
        assert_eq!(hint.target(7, start + OVERLAY_ANIMATION * 5), Some(second));
    }

    #[test]
    fn appears_at_once_and_fades_in() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        assert!(hint.modifiers_changed(&CTRL_SHIFT, start, None));
        assert_eq!(hint.opacity(start), (0., true));
        let (fading_in, is_changing) = hint.opacity(start + OVERLAY_ANIMATION / 4);
        assert!(fading_in > 0. && fading_in < 1. && is_changing);
        assert_eq!(hint.opacity(start + OVERLAY_ANIMATION), (1., false));
    }

    #[test]
    fn releasing_a_key_fades_it_out() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        let released = start + OVERLAY_ANIMATION * 2;
        let only_ctrl = Modifiers { control: true, ..NONE };
        assert!(hint.modifiers_changed(&only_ctrl, released, None));
        assert_eq!(hint.opacity(released).0, 1.);
        let halfway = hint.opacity(released + OVERLAY_ANIMATION / 2).0;
        assert!(halfway > 0. && halfway < 1.);
        assert_eq!(hint.opacity(released + OVERLAY_ANIMATION), (0., false));
    }

    #[test]
    fn a_key_cancels_it() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        assert!(hint.operation_performed(start + OVERLAY_ANIMATION * 2));
        assert_eq!(hint.opacity(start + OVERLAY_ANIMATION * 3), (0., false));
        assert!(!hint.operation_performed(start + OVERLAY_ANIMATION * 3), "nothing to cancel any more");
    }

    #[test]
    fn a_used_chord_stays_hidden_until_both_keys_are_released() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        hint.operation_performed(start);
        let only_ctrl = Modifiers { control: true, ..NONE };
        let later = start + Duration::from_secs(1);
        hint.modifiers_changed(&only_ctrl, later, None);
        assert!(!hint.modifiers_changed(&CTRL_SHIFT, later, None));
        assert_eq!(hint.opacity(later + OVERLAY_ANIMATION * 2), (0., false));
        hint.modifiers_changed(&NONE, later, None);
        assert!(hint.modifiers_changed(&CTRL_SHIFT, later, None));
    }

    #[test]
    fn another_modifier_keeps_it_away() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        let with_alt = Modifiers { alt: true, ..CTRL_SHIFT };
        assert!(!hint.modifiers_changed(&with_alt, start, None));
        assert_eq!(hint.opacity(start + OVERLAY_ANIMATION * 2), (0., false));

        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        assert!(hint.modifiers_changed(&with_alt, start + OVERLAY_ANIMATION * 2, None));
        assert_eq!(hint.opacity(start + OVERLAY_ANIMATION * 4), (0., false));
    }

    #[test]
    fn losing_the_focus_while_shown_fades_it_out_and_forgets_the_chord() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        let away = start + OVERLAY_ANIMATION * 2;
        assert!(hint.window_deactivated(away));
        assert_eq!(hint.opacity(away).0, 1.);
        assert_eq!(hint.opacity(away + OVERLAY_ANIMATION), (0., false));
        assert!(hint.modifiers_changed(&CTRL_SHIFT, away, None));
    }

    #[test]
    fn losing_the_focus_after_a_chord_was_used_lets_the_next_one_show() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        hint.operation_performed(start);
        assert!(!hint.window_deactivated(start));
        assert!(hint.modifiers_changed(&CTRL_SHIFT, start, None));
    }

    #[test]
    fn hint_stays_visible_while_the_chord_is_held() {
        let start = Instant::now();
        let mut hint = ShortcutHint::default();
        hint.modifiers_changed(&CTRL_SHIFT, start, None);
        let long_after = start + Duration::from_secs(5);
        assert_eq!(hint.opacity(long_after), (1., false));
    }

    #[test]
    fn keys_read_like_labels() {
        assert_eq!(describe_keys("ctrl-shift-s"), "Ctrl+Shift+S");
        assert_eq!(describe_keys("ctrl-alt-left"), "Ctrl+Alt+Left");
        assert_eq!(describe_keys("ctrl-shift-pagedown"), "Ctrl+Shift+PageDown");
        assert_eq!(describe_keys("ctrl--"), "Ctrl+-");
        assert_eq!(describe_keys("f11"), "F11");
        assert_eq!(describe_keys("ctrl-k ctrl-s"), "Ctrl+K Ctrl+S");
    }

    fn defaults(id: &str) -> Vec<String> {
        wired_config::ACTIONS
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.defaults.iter().map(|k| (*k).to_owned()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn labels_follow_the_current_bindings() {
        let labels = pane_labels(defaults);
        let keys = |label: &Option<Label>| label.as_ref().map(|l| (l.icon, l.keys.clone()));
        assert_eq!(keys(&labels.right), Some((Icon::SplitRight, "Ctrl+Shift+S".to_owned())));
        assert_eq!(keys(&labels.below), Some((Icon::SplitDown, "Ctrl+Shift+D".to_owned())));
        assert_eq!(keys(&labels.above), Some((Icon::Close, "Ctrl+Shift+W".to_owned())));

        let rebound = pane_labels(|id| match id {
            "split-right" => vec!["ctrl-alt-e".into(), "f5".into()],
            "split-bottom" => Vec::new(),
            _ => defaults(id),
        });
        assert_eq!(keys(&rebound.right), Some((Icon::SplitRight, "Ctrl+Alt+E / F5".to_owned())));
        assert_eq!(rebound.below, None, "an action without keys gets no label");
        assert_eq!(rebound.above, labels.above);
    }

    #[test]
    fn row_width_is_labeled_only_while_that_gesture_is_on() {
        let label = row_width_label(MouseAction::RowWidth).expect("on");
        assert_eq!((label.icon, label.keys.as_str()), (Icon::Mouse, "Ctrl+Shift+Wheel"));
        assert_eq!(row_width_label(MouseAction::None), None);
    }

    #[test]
    fn row_width_goes_to_the_side_farther_from_the_focused_pane() {
        let pane_at = |y: f32, height: f32| Rect::new(0., y, 500., height);
        assert_eq!(row_width_edge(600., pane_at(0., 200.)), Edge::Bottom, "pane near the top");
        assert_eq!(row_width_edge(600., pane_at(400., 200.)), Edge::Top, "pane near the bottom");
        assert_eq!(row_width_edge(600., pane_at(0., 600.)), Edge::Bottom, "the whole area: equal distances");
        assert_eq!(row_width_edge(600., pane_at(200., 200.)), Edge::Bottom, "centered: equal distances");
    }
}
