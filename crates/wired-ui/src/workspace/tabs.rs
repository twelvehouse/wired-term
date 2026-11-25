//! Tabs: the strip, opening, closing and switching.

use super::*;

pub(super) struct Tab {
    pub(super) id: u64,
    /// Replaces the automatic title until cleared.
    pub(super) custom_title: Option<String>,
    pub(super) slide: Option<Slide>,
    pub(super) panes: PaneTree<Pane>,
    /// `None` for a tab that appears without the opening animation.
    pub(super) opened_at: Option<Instant>,
    pub(super) motion: Option<PaneMotion>,
    /// Rows scrolling to show the focused pane.
    pub(super) glides: HashMap<RowId, Glide>,
    pub(super) bars: HashMap<RowId, ScrollbarState>,
}

impl Tab {
    fn new(id: u64, pane: Pane, opened_at: Option<Instant>) -> Self {
        Self {
            id,
            custom_title: None,
            slide: None,
            panes: PaneTree::new(pane),
            opened_at,
            motion: None,
            glides: HashMap::new(),
            bars: HashMap::new(),
        }
    }
}

fn tab_icon(tab: &Tab) -> Option<ProfileIcon> {
    tab.panes.get(tab.panes.focused()).and_then(|p| p.profile.icon)
}

fn tab_number(index: usize) -> Option<u8> {
    (index < 10).then(|| u8::try_from((index + 1) % 10).unwrap_or(0))
}

/// Only the look of a closed tab, kept for its closing animation.
pub(super) struct ClosingTab {
    pub(super) id: u64,
    pub(super) index: usize,
    title: String,
    icon: Option<ProfileIcon>,
    number: Option<u8>,
    pub(super) started: Instant,
}

impl Workspace {
    /// Starts the number fade when the tab count crosses between one and two.
    pub(super) fn update_numbers(&mut self) {
        let want = self.tabs.len() > 1;
        if want != self.are_numbers_shown {
            self.are_numbers_shown = want;
            self.numbers_changed_at = Some(Instant::now());
        }
    }

    pub(super) fn open_tab(
        &mut self,
        profile: Option<Arc<Profile>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let id = self.next_tab_id;
        let profile = profile
            .or_else(|| Settings::get(cx).default_profile())
            .ok_or_else(|| anyhow::anyhow!("no shell was found"))?;
        let pane = self.new_pane(id, profile, window, cx)?;
        self.next_tab_id += 1;
        self.end_tab_pointer(cx);
        self.tabs.push(Tab::new(id, pane, Some(Instant::now())));
        self.update_numbers();
        self.activate(self.tabs.len() - 1, window, cx);
        Ok(())
    }

    /// The tab now at the same place becomes active, as in Tabby.
    pub(super) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.tabs.len() {
            return;
        }
        self.end_tab_pointer(cx);
        let was_active = index == self.active;
        let tab = self.tabs.remove(index);
        if self.renaming.as_ref().is_some_and(|r| r.tab_id == tab.id) {
            self.renaming = None;
        }
        if self.tabs.is_empty() {
            window.remove_window();
            return;
        }
        self.closing.push(ClosingTab {
            id: tab.id,
            index,
            title: self.tab_title(&tab),
            icon: tab_icon(&tab),
            number: tab_number(index),
            started: Instant::now(),
        });
        // Closes its panes and programs now, before the animation ends.
        drop(tab);
        self.update_numbers();
        let next = if was_active {
            index.min(self.tabs.len() - 1)
        } else if index < self.active {
            self.active - 1
        } else {
            self.active
        };
        self.activate(next, window, cx);
    }

    pub(super) fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            // Drags belong to the tab they started in (ids of another tab's tree can coincide).
            self.end_drags(cx);
            self.active = index;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(super) fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active)
            && let Some(pane) = tab.panes.get(tab.panes.focused())
        {
            window.focus(&pane.view.focus_handle(cx), cx);
        }
    }

    pub(super) fn move_active(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.tabs.len();
        let Some(target) = self.active.checked_add_signed(delta).filter(|t| *t < len) else { return };
        self.end_tab_pointer(cx);
        self.move_tab(self.active, target, true, Instant::now());
        cx.notify();
    }

    pub(super) fn step_tab(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let len = isize::try_from(self.tabs.len()).unwrap_or(1);
        let current = isize::try_from(self.active).unwrap_or(0);
        let next = usize::try_from((current + delta).rem_euclid(len)).unwrap_or(0);
        self.activate(next, window, cx);
    }

    pub(super) fn tab_title(&self, tab: &Tab) -> String {
        tab.custom_title.clone().unwrap_or_else(|| {
            let focused = tab.panes.get(tab.panes.focused());
            focused
                .map(|p| p.title.clone())
                .filter(|t| !t.is_empty())
                .or_else(|| focused.map(|p| p.profile.name.clone()))
                .unwrap_or_default()
        })
    }

    /// Also returns whether any tab is still animating.
    pub(super) fn tab_strip(&self, cx: &mut Context<Self>) -> (TabStrip, bool) {
        let now = Instant::now();
        let numbers = progress(self.numbers_changed_at, now);
        let number_reveal = if self.are_numbers_shown { numbers } else { 1. - numbers };
        let mut is_animating = numbers < 1.;
        let middle_closes = Settings::get(cx).mouse(gesture::TAB_MIDDLE_CLICK) == MouseAction::CloseTab;
        let (shifts, tail) = self.tab_shifts(now);
        let mut tabs = self
            .tabs
            .iter()
            .zip(shifts)
            .enumerate()
            .map(|(index, (tab, shift))| {
                let id = tab.id;
                let appear = progress(tab.opened_at, now);
                is_animating |= appear < 1.;
                let number = tab_number(index);
                is_animating |= shift.is_sliding;
                let title = match &self.renaming {
                    Some(r) if r.tab_id == id => TabTitle::Editing(r.input.clone()),
                    _ => TabTitle::Text(self.tab_title(tab)),
                };
                let look = TabLook {
                    id,
                    number,
                    icon: tab_icon(tab),
                    title,
                    is_active: index == self.active,
                    appear,
                    number_reveal,
                    offset: shift.offset,
                    is_lifted: shift.is_lifted,
                    metrics: Rc::clone(&self.strip),
                };
                let entity = cx.entity();
                let press = entity.downgrade();
                let close = entity.downgrade();
                let handlers = TabHandlers {
                    on_press: Box::new(move |event, window, cx| {
                        let _ = press.update(cx, |this, cx| this.press_tab(id, event, window, cx));
                    }),
                    on_close: Box::new(move |window, cx| {
                        let _ = close.update(cx, |this, cx| {
                            if let Some(index) = this.tabs.iter().position(|t| t.id == id) {
                                this.close_tab(index, window, cx);
                            }
                        });
                    }),
                    middle_closes,
                };
                Some((look, handlers))
            })
            .collect::<Vec<_>>();
        // Closed tabs play their opening animation backwards where they were.
        let ghosts = self.sorted_ghosts();
        is_animating |= !ghosts.is_empty();
        let order = strip_order(tabs.len(), &ghosts.iter().map(|g| g.index).collect::<Vec<_>>());
        let strip = order
            .into_iter()
            .filter_map(|item| match item {
                StripItem::Tab(index) => tabs[index].take(),
                StripItem::Ghost(index) => {
                    let ghost = ghosts[index];
                    let look = TabLook {
                        id: ghost.id,
                        number: ghost.number,
                        icon: ghost.icon,
                        title: TabTitle::Text(ghost.title.clone()),
                        is_active: false,
                        appear: 1. - progress(Some(ghost.started), now),
                        number_reveal,
                        offset: 0.,
                        is_lifted: false,
                        metrics: Rc::clone(&self.strip),
                    };
                    let handlers = TabHandlers {
                        on_press: Box::new(|_, _, _| {}),
                        on_close: Box::new(|_, _| {}),
                        middle_closes: false,
                    };
                    Some((look, handlers))
                }
            })
            .collect();
        (TabStrip { tabs: strip, tail, metrics: Rc::clone(&self.strip), dim: None }, is_animating)
    }

    pub(super) fn sorted_ghosts(&self) -> Vec<&ClosingTab> {
        let mut ghosts: Vec<&ClosingTab> = self.closing.iter().collect();
        ghosts.sort_by_key(|c| c.index);
        ghosts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StripItem {
    /// Index into `Workspace::tabs`.
    Tab(usize),
    /// Index into the sorted closing tabs.
    Ghost(usize),
}

/// Each sorted ghost index counts among the items placed so far.
pub(super) fn strip_order(tab_count: usize, ghost_indexes: &[usize]) -> Vec<StripItem> {
    let mut order: Vec<StripItem> = (0..tab_count).map(StripItem::Tab).collect();
    for (ghost, &index) in ghost_indexes.iter().enumerate() {
        order.insert(index.min(order.len()), StripItem::Ghost(ghost));
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_tabs_are_put_back_where_they_were() {
        use StripItem::{Ghost, Tab};
        assert_eq!(strip_order(2, &[]), [Tab(0), Tab(1)]);
        assert_eq!(strip_order(2, &[1]), [Tab(0), Ghost(0), Tab(1)]);
        assert_eq!(strip_order(2, &[0, 3]), [Ghost(0), Tab(0), Tab(1), Ghost(1)]);
    }
}
