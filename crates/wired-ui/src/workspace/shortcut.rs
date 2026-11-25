//! Shortcuts run once on key release, as in Tabby, so a held split shortcut does not add panes
//! at the key repeat rate.

use gpui::{Action, KeyContext, Keystroke};

use super::*;

pub(super) struct HeldShortcut {
    key: String,
    action: Box<dyn Action>,
}

impl Workspace {
    /// Swallows repeats of the held key. Returns whether the key was taken.
    pub(super) fn hold_shortcut(&mut self, keystroke: &Keystroke, context_stack: &[KeyContext], cx: &App) -> bool {
        if self.held_shortcut.as_ref().is_some_and(|held| held.key == keystroke.key) {
            return true;
        }
        let Some(action) = crate::settings::shortcut_action(keystroke, context_stack, cx) else { return false };
        self.held_shortcut = Some(HeldShortcut { key: keystroke.key.clone(), action });
        true
    }

    pub(super) fn release_shortcut(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(held) = self.held_shortcut.take_if(|held| held.key == key) {
            window.dispatch_action(held.action, cx);
        }
    }
}
