//! Panes: creating, splitting, closing and focusing them.

use super::*;

pub(super) struct Pane {
    pub(super) view: Entity<TerminalView>,
    /// Splits and restarts of this pane run the same profile.
    pub(super) profile: Arc<Profile>,
    /// Set by the program; empty until it sets one.
    pub(super) title: String,
    _events: Subscription,
    _focus: Subscription,
}

impl Workspace {
    /// A program that cannot start leaves a pane that says why; only a failure to create the
    /// screen itself is an error.
    pub(super) fn new_pane(
        &self,
        tab_id: u64,
        profile: Arc<Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Pane> {
        let (started, failure) =
            match wired_terminal_view::spawn_terminal(crate::terminal_config(&profile, self.vt_options.clone())) {
                Ok(started) => (started, None),
                Err(error) => (
                    wired_terminal_view::screen_without_program(&self.vt_options)?,
                    Some(format!("Cannot start {}: {error}", profile.program.display())),
                ),
            };
        let (terminal, events) = started;
        let font = self.appearance.font.clone();
        let options = self.appearance.view;
        let view = cx.new(|cx| TerminalView::new(terminal, events, font, options, window, cx));
        if let Some(message) = failure {
            view.update(cx, |view, cx| view.show_start_failure(message, cx));
        }
        let (events, focus) = self.pane_subscriptions(tab_id, &view, window, cx);
        Ok(Pane { view, profile, title: String::new(), _events: events, _focus: focus })
    }

    fn pane_subscriptions(
        &self,
        tab_id: u64,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Subscription, Subscription) {
        let entity = view.clone();
        let events = cx.subscribe_in(view, window, move |this: &mut Self, _, event, window, cx| {
            this.on_pane_event(tab_id, &entity, event, window, cx);
        });
        let entity = view.clone();
        let focus = cx.on_focus(&view.focus_handle(cx), window, move |this, _, cx| {
            if let Some((index, pane)) = this.locate(tab_id, &entity) {
                this.tabs[index].panes.focus(pane);
                this.update_dimming(index, cx);
                this.reveal_focused(index, cx);
                cx.notify();
            }
        });
        (events, focus)
    }

    pub(super) fn adopt_panes(&self, tab: &mut Tab, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<PaneId> = tab.panes.panes().into_iter().map(|(id, _)| id).collect();
        for id in ids {
            let Some(pane) = tab.panes.get_mut(id) else { continue };
            pane.view.update(cx, |view, cx| view.attach_to_window(window, cx));
            let (events, focus) = self.pane_subscriptions(tab_id, &pane.view, window, cx);
            pane._events = events;
            pane._focus = focus;
        }
    }

    fn update_dimming(&self, index: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(index) else { return };
        let (focused, is_split) = (tab.panes.focused(), tab.panes.len() > 1);
        let views: Vec<_> = tab.panes.panes().into_iter().map(|(id, p)| (id, p.view.clone())).collect();
        for (id, view) in views {
            view.update(cx, |view, cx| view.set_dimmed(is_split && id != focused, cx));
        }
    }

    fn focus_pane(&mut self, index: usize, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs[index].panes.focus(pane) {
            return;
        }
        self.update_dimming(index, cx);
        self.reveal_focused(index, cx);
        if index == self.active {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(super) fn split(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(tab) = self.tabs.get(index) else { return };
        let (tab_id, target) = (tab.id, tab.panes.focused());
        let Some(profile) = tab.panes.get(target).map(|p| Arc::clone(&p.profile)) else { return };
        let pane = match self.new_pane(tab_id, profile, window, cx) {
            Ok(pane) => pane,
            Err(error) => {
                self.show_error(format!("Cannot start the shell: {error:#}"), cx);
                return;
            }
        };
        let before = self.drawn_rects(index, cx);
        let Ok(id) = self.tabs[index].panes.split(target, side, pane) else { return };
        if let Some(mut from) = before
            && let Some(&old) = from.get(&target)
        {
            from.insert(id, edge(old, side));
            self.start_motion(index, from, Vec::new(), cx);
        }
        self.focus_pane(index, id, window, cx);
    }

    pub(super) fn focus_direction(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.active;
        let layout = self.layout(index, cx);
        let Some(tab) = self.tabs.get(index) else { return };
        if let Some(next) = tab.panes.neighbor(tab.panes.focused(), direction, &layout) {
            self.focus_pane(index, next, window, cx);
        }
    }

    pub(super) fn focus_step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(tab) = self.tabs.get(index) else { return };
        let from = tab.panes.focused();
        let next = if delta > 0 { tab.panes.next(from) } else { tab.panes.previous(from) };
        self.focus_pane(index, next, window, cx);
    }

    fn locate(&self, tab_id: u64, view: &Entity<TerminalView>) -> Option<(usize, PaneId)> {
        let index = self.tabs.iter().position(|t| t.id == tab_id)?;
        let pane = self.tabs[index].panes.panes().into_iter().find(|(_, p)| &p.view == view).map(|(id, _)| id)?;
        Some((index, pane))
    }

    fn on_pane_event(
        &mut self,
        tab_id: u64,
        view: &Entity<TerminalView>,
        event: &TerminalViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((index, pane)) = self.locate(tab_id, view) else { return };
        match event {
            TerminalViewEvent::TitleChanged(title) => {
                if let Some(p) = self.tabs[index].panes.get_mut(pane) {
                    p.title.clone_from(title);
                }
                cx.notify();
            }
            TerminalViewEvent::Exited(_) => self.close_pane(index, pane, window, cx),
            TerminalViewEvent::Restart => self.restart_pane(index, pane, window, cx),
            TerminalViewEvent::Bell => self.ring_bell(window, cx),
        }
    }

    fn restart_pane(&mut self, index: usize, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let tab_id = self.tabs[index].id;
        let Some(profile) = self.tabs[index].panes.get(pane).map(|p| Arc::clone(&p.profile)) else { return };
        let Ok(fresh) = self.new_pane(tab_id, profile, window, cx) else { return };
        if let Some(slot) = self.tabs[index].panes.get_mut(pane) {
            *slot = fresh;
        }
        self.update_dimming(index, cx);
        if index == self.active {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    /// The last pane closes its tab, and the last tab the window.
    pub(super) fn close_pane(&mut self, index: usize, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.drawn_rects(index, cx);
        let (area, params) = self.pane_geometry(index, cx);
        match self.tabs[index].panes.close(pane, area, &params) {
            // The program ends when the last view handle drops, after the ghost's animation.
            Closed::Removed { item, .. } => {
                let after = self.layout(index, cx);
                self.tabs[index].forget_gone_rows(&after);
                if let Some(from) = before
                    && let Some(&old) = from.get(&pane)
                {
                    // The ghost's terminal keeps the size it was last fitted to.
                    let to = collapse_target(old, &from, after.panes());
                    let ghost = PaneGhost { view: item.view.clone(), from: old, to };
                    self.start_motion(index, from, vec![ghost], cx);
                }
                drop(item);
                self.update_dimming(index, cx);
                if index == self.active {
                    self.focus_active(window, cx);
                }
                cx.notify();
            }
            Closed::LastPane => self.close_tab(index, window, cx),
            Closed::NotFound => {}
        }
    }
}
