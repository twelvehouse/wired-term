//! Renaming a tab in place.

use gpui::component::input::{InputEvent, InputState};

use super::*;

const MAX_TITLE_CHARS: usize = 256;

pub(super) struct Renaming {
    pub(super) tab_id: u64,
    pub(super) input: Entity<InputState>,
    initial_title: String,
    _events: Subscription,
}

/// `None` gives the tab back its automatic title.
fn custom_title(text: &str) -> Option<String> {
    let plain: String = text.chars().filter(|c| !c.is_control()).collect();
    let title: String = plain.trim().chars().take(MAX_TITLE_CHARS).collect();
    (!title.is_empty()).then_some(title)
}

impl Workspace {
    pub(super) fn start_rename(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(current) = &self.renaming {
            if current.tab_id == tab_id {
                return;
            }
            self.finish_rename(true, true, window, cx);
        }
        let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else { return };
        let shown = self.tab_title(tab);
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            state.set_value(shown.clone(), window, cx);
            state.select_all(window, cx);
            state.focus(window, cx);
        });
        let events = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.finish_rename(true, true, window, cx),
            InputEvent::Blur => {
                if window.is_window_active() {
                    this.finish_rename(true, window.focused(cx).is_none(), window, cx);
                }
            }
            InputEvent::Change | InputEvent::Focus => {}
        });
        self.renaming = Some(Renaming { tab_id, input, initial_title: shown, _events: events });
        cx.notify();
    }

    pub(super) fn finish_rename(
        &mut self,
        is_committed: bool,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(renaming) = self.renaming.take() else { return };
        let value = renaming.input.read(cx).value();
        if is_committed
            && value.as_ref() != renaming.initial_title
            && let Some(tab) = self.tabs.iter_mut().find(|t| t.id == renaming.tab_id)
        {
            tab.custom_title = custom_title(value.as_ref());
        }
        if refocus {
            self.focus_active(window, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_trimmed_and_blank_ones_clear() {
        assert_eq!(custom_title("  build  ").as_deref(), Some("build"));
        assert_eq!(custom_title("   "), None);
        assert_eq!(custom_title(""), None);
    }

    #[test]
    fn titles_are_cut_by_characters_not_bytes() {
        let long = "あ".repeat(MAX_TITLE_CHARS + 40);
        assert_eq!(custom_title(&long).map(|t| t.chars().count()), Some(MAX_TITLE_CHARS));
    }

    #[test]
    fn control_characters_are_dropped() {
        assert_eq!(custom_title("a\nb\u{1b}[31mc").as_deref(), Some("ab[31mc"));
    }
}
