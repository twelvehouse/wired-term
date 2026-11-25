//! Rows that scroll sideways: gliding, wheel gestures and the bar under each row.

use super::*;

/// Sideways scroll per wheel line, in px.
const ROW_SCROLL_LINE: f32 = 40.;

/// Share of a row's visible width per Ctrl+Shift+wheel notch.
const ROW_WIDTH_STEP: f32 = 0.1;

impl Tab {
    pub(super) fn forget_gone_rows(&mut self, layout: &Layout) {
        let is_live = |id: &RowId| layout.row(*id).is_some();
        self.glides.retain(|id, _| is_live(id));
        self.bars.retain(|id, _| is_live(id));
    }

    /// Returns whether any row still glides.
    fn advance_glides(&mut self, now: Instant) -> bool {
        for (key, glide) in &self.glides {
            self.panes.set_row_scroll(*key, glide.at(now));
        }
        self.glides.retain(|_, g| progress(Some(g.started), now) < 1.);
        !self.glides.is_empty()
    }
}

impl Workspace {
    fn row_at(&self, position: Point<Pixels>, cx: &App) -> Option<Row> {
        let origin = self.pane_area.get()?.origin;
        let y = f32::from(position.y - origin.y);
        self.layout(self.active, cx).rows().iter().find(|r| r.rect.y <= y && y <= r.rect.y + r.rect.height).cloned()
    }

    /// Scrolls just enough to show the focused pane whole, or its left edge if wider than the row.
    pub(super) fn reveal_focused(&mut self, index: usize, cx: &App) {
        let layout = self.layout(index, cx);
        let Some(tab) = self.tabs.get_mut(index) else { return };
        let focused = tab.panes.focused();
        let (Some(row), Some(rect)) = (layout.row_of(focused), layout.pane(focused)) else { return };
        let visible = row.rect.width;
        // In the row's contents; the layout shows the pane where the row is scrolled now.
        let left = rect.x - row.rect.x + row.scroll;
        let current = tab.glides.get(&row.id).map_or(row.scroll, |g| g.to);
        let target = if left < current || rect.width > visible {
            left
        } else if left + rect.width > current + visible {
            left + rect.width - visible
        } else {
            current
        };
        let target = target.clamp(0., (row.width - visible).max(0.));
        if (target - row.scroll).abs() > 0.5 {
            tab.glides.insert(row.id, Glide { from: row.scroll, to: target, started: Instant::now() });
        }
    }

    /// A row whose fixed width the pane area has caught up with follows the area again. Returns
    /// whether a row is still gliding.
    pub(super) fn settle_rows(&mut self, now: Instant, cx: &App) -> bool {
        let layout = self.layout(self.active, cx);
        let Some(tab) = self.tabs.get_mut(self.active) else { return false };
        tab.panes.settle_rows(&layout);
        tab.advance_glides(now)
    }

    fn scroll_row_by(&mut self, row: &Row, delta: f32, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        tab.glides.remove(&row.id);
        let max = (row.width - row.rect.width).max(0.);
        tab.panes.set_row_scroll(row.id, (row.scroll + delta).clamp(0., max));
        cx.notify();
    }

    /// At or below the visible width the row follows the pane area again.
    fn resize_row(&mut self, row: &Row, notches: f32, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let width = row.width + row.rect.width * ROW_WIDTH_STEP * notches;
        tab.glides.remove(&row.id);
        tab.panes.set_row_width(row.id, (width > row.rect.width + 0.5).then_some(width));
        cx.notify();
    }

    /// `bar_row` is the row whose bar is under the pointer, where the plain wheel scrolls too.
    pub(super) fn on_row_wheel(&mut self, event: &ScrollEvent, bar_row: Option<RowId>, cx: &mut Context<Self>) {
        self.on_hint_operation(cx);
        let (x, y, is_lines) = match event.delta {
            ScrollDelta::Lines(d) => (d.x, d.y, true),
            ScrollDelta::Pixels(d) => (f32::from(d.x), f32::from(d.y), false),
        };
        let Some(row) = self.row_at(event.position, cx) else { return };
        let m = &event.modifiers;
        let settings = Settings::get(cx);
        if m.control && m.shift {
            if settings.mouse(gesture::CTRL_SHIFT_WHEEL) != MouseAction::RowWidth {
                return;
            }
            let amount = if x != 0. { x } else { y };
            let notches = if is_lines {
                amount.signum()
            } else {
                // Touchpads send many small pixel deltas: one notch per line's worth.
                self.width_wheel_rest += amount / ROW_SCROLL_LINE;
                let whole = self.width_wheel_rest.trunc();
                self.width_wheel_rest -= whole;
                whole
            };
            if notches != 0. {
                self.resize_row(&row, notches, cx);
            }
            cx.stop_propagation();
            return;
        }
        // Windows turns Shift+wheel into a sideways delta; a tilt wheel sends one without Shift.
        let is_sideways_allowed = !m.shift || settings.mouse(gesture::SHIFT_WHEEL) == MouseAction::RowScroll;
        let delta = if x != 0. && is_sideways_allowed {
            x
        } else if bar_row == Some(row.id) {
            y
        } else {
            0.
        };
        if delta != 0. && row.width > row.rect.width {
            let pixels = if is_lines { delta * ROW_SCROLL_LINE } else { delta };
            // A positive delta moves the contents right, toward their start.
            self.scroll_row_by(&row, -pixels, cx);
            cx.stop_propagation();
        }
    }

    fn row(&self, id: RowId, cx: &App) -> Option<Row> {
        self.layout(self.active, cx).row(id).cloned()
    }

    pub(super) fn drag_row_thumb(&mut self, drag: ThumbDrag, pointer: f32, cx: &mut Context<Self>) {
        let Some(origin) = self.pane_area.get().map(|b| f32::from(b.origin.x)) else { return };
        let Some(row) = self.row(drag.row, cx) else {
            self.end_drags(cx);
            return;
        };
        let Some((_, width)) = row_thumb(&row) else { return };
        let travel = (row.rect.width - scrollbar::MARGIN * 2. - width).max(1.);
        let left = pointer - origin - row.rect.x - drag.grab - scrollbar::MARGIN;
        let scroll = (left / travel).clamp(0., 1.) * (row.width - row.rect.width);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.glides.remove(&drag.row);
            tab.panes.set_row_scroll(drag.row, scroll);
            cx.notify();
        }
    }

    /// A press off the thumb centers the thumb on the pointer first, like the terminal scrollbar.
    pub(super) fn press_row_bar(&mut self, id: RowId, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(origin) = self.pane_area.get().map(|b| f32::from(b.origin.x)) else { return };
        let Some(row) = self.row(id, cx) else { return };
        let Some((left, width)) = row_thumb(&row) else { return };
        let x = f32::from(event.position.x) - origin - row.rect.x;
        let grab = if (left..=left + width).contains(&x) { x - left } else { width / 2. };
        let drag = ThumbDrag { row: id, grab };
        self.thumb_drag = Some(drag);
        if let Some(bar) = self.tabs.get_mut(self.active).and_then(|t| t.bars.get_mut(&id)) {
            bar.set_hot(true);
        }
        self.drag_row_thumb(drag, f32::from(event.position.x), cx);
    }

    /// `x` is `None` when the pointer left the bar zone.
    pub(super) fn hover_row_bar(&mut self, id: RowId, x: Option<f32>, cx: &mut Context<Self>) {
        let origin = self.pane_area.get().map_or(0., |b| f32::from(b.origin.x));
        let Some(row) = self.row(id, cx) else { return };
        let thumb = row_thumb(&row).map(|t| (row.rect.x, t));
        let is_held = self.thumb_drag.is_some_and(|drag| drag.row == id);
        let is_over_thumb = x.zip(thumb).is_some_and(|(x, (row_x, (left, width)))| {
            let x = x - origin - row_x;
            (left..=left + width).contains(&x)
        });
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        let bar = tab.bars.entry(id).or_default();
        let wide = bar.set_wide(x.is_some() || is_held);
        let hot = bar.set_hot(is_over_thumb || is_held);
        if wide || hot {
            cx.notify();
        }
    }
}

/// Left edge from the row's left, and width; `None` when the row fits.
pub(super) fn row_thumb(row: &Row) -> Option<(f32, f32)> {
    let max = row.width - row.rect.width;
    if max < 0.5 {
        return None;
    }
    let track = (row.rect.width - scrollbar::MARGIN * 2.).max(1.);
    let width = (track * row.rect.width / row.width).max(scrollbar::MIN_THUMB).min(track);
    Some((scrollbar::MARGIN + (track - width) * row.scroll / max, width))
}
