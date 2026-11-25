//! Animations of panes moving after a split, a close or an equalize.

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) struct Glide {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) started: Instant,
}

impl Glide {
    pub(super) fn at(&self, now: Instant) -> f32 {
        self.from + (self.to - self.from) * progress(Some(self.started), now)
    }
}

/// A closed pane still drawn while it shrinks away; its program ends when the animation does.
pub(super) struct PaneGhost {
    pub(super) view: Entity<TerminalView>,
    pub(super) from: Rect,
    pub(super) to: Rect,
}

pub(super) struct PaneMotion {
    /// A pane missing here does not move.
    from: HashMap<PaneId, Rect>,
    pub(super) ghosts: Vec<PaneGhost>,
    pub(super) started: Instant,
}

pub(super) fn lerp_rect(from: Rect, to: Rect, t: f32) -> Rect {
    let mix = |a: f32, b: f32| a + (b - a) * t;
    Rect::new(mix(from.x, to.x), mix(from.y, to.y), mix(from.width, to.width), mix(from.height, to.height))
}

/// The zero-thickness rectangle on `side` that a pane grows from or shrinks to.
pub(super) fn edge(rect: Rect, side: Side) -> Rect {
    match side {
        Side::Left => Rect::new(rect.x, rect.y, 0., rect.height),
        Side::Right => Rect::new(rect.x + rect.width, rect.y, 0., rect.height),
        Side::Top => Rect::new(rect.x, rect.y, rect.width, 0.),
        Side::Bottom => Rect::new(rect.x, rect.y + rect.height, rect.width, 0.),
    }
}

/// Shrinks onto the seam where the panes taking its space meet, so both dividers converge, or
/// to its far edge when nothing comes from that side.
pub(super) fn collapse_target(old: Rect, before: &HashMap<PaneId, Rect>, after: &[(PaneId, Rect)]) -> Rect {
    const EPSILON: f32 = 0.5;
    let overlaps = |r: &Rect| {
        r.x < old.x + old.width && old.x < r.x + r.width && r.y < old.y + old.height && old.y < r.y + r.height
    };
    let takers: Vec<(Rect, Rect)> =
        after.iter().filter(|(_, r)| overlaps(r)).filter_map(|(id, r)| before.get(id).map(|b| (*b, *r))).collect();
    let is_beside = |b: &Rect| b.x + b.width <= old.x + EPSILON || b.x >= old.x + old.width - EPSILON;
    let shares_rows = |b: &Rect| b.y < old.y + old.height - EPSILON && old.y < b.y + b.height - EPSILON;
    let is_side_by_side = match takers.first() {
        Some((b, _)) => is_beside(b),
        None => before.values().any(|b| is_beside(b) && shares_rows(b)),
    };
    if is_side_by_side {
        let from_right = takers.iter().filter(|(b, _)| b.x >= old.x + old.width - EPSILON);
        let x = from_right.map(|(_, a)| a.x).reduce(f32::min);
        x.map_or_else(|| edge(old, Side::Right), |x| Rect::new(x, old.y, 0., old.height))
    } else {
        let from_below = takers.iter().filter(|(b, _)| b.y >= old.y + old.height - EPSILON);
        let y = from_below.map(|(_, a)| a.y).reduce(f32::min);
        y.map_or_else(|| edge(old, Side::Bottom), |y| Rect::new(old.x, y, old.width, 0.))
    }
}

impl Workspace {
    pub(super) fn equalize_divider(&mut self, id: DividerId, cx: &mut Context<Self>) {
        let index = self.active;
        let before = self.drawn_rects(index, cx);
        let Some(tab) = self.tabs.get_mut(index) else { return };
        if !tab.panes.equalize(id) {
            return;
        }
        if let Some(from) = before {
            self.start_motion(index, from, Vec::new(), cx);
        }
        self.reveal_focused(index, cx);
        cx.notify();
    }

    /// `None` when the tab is not on screen.
    pub(super) fn drawn_rects(&self, index: usize, cx: &App) -> Option<HashMap<PaneId, Rect>> {
        if index != self.active || self.pane_area.get().is_none() {
            return None;
        }
        let tab = self.tabs.get(index)?;
        Some(animated_rects(tab, &self.layout(index, cx), Instant::now()))
    }

    /// Terminals are sized for the final layout and clipped while their pane moves.
    pub(super) fn start_motion(
        &mut self,
        index: usize,
        from: HashMap<PaneId, Rect>,
        mut ghosts: Vec<PaneGhost>,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let tab = &mut self.tabs[index];
        if let Some(previous) = tab.motion.take() {
            let t = progress(Some(previous.started), now);
            ghosts.extend(previous.ghosts.into_iter().map(|g| PaneGhost { from: lerp_rect(g.from, g.to, t), ..g }));
        }
        tab.motion = Some(PaneMotion { from, ghosts, started: now });
        let tab_id = tab.id;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(ANIMATION).await;
            let _ = this.update(cx, |this, cx| this.finish_motion(tab_id, now, cx));
        })
        .detach();
        cx.notify();
    }

    /// A motion started later than `started` keeps running.
    fn finish_motion(&mut self, tab_id: u64, started: Instant, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) else { return };
        if tab.motion.as_ref().is_none_or(|m| m.started != started) {
            return;
        }
        tab.motion = None;
        cx.notify();
    }
}

/// Divider lines for panes at animated positions, where the tree's dividers do not apply.
pub(super) fn edge_lines(rects: &[Rect], divider: f32) -> Vec<Rect> {
    let mut lines = Vec::new();
    for r in rects {
        if r.x > 0.5 {
            lines.push(Rect::new(r.x - divider, r.y, divider, r.height));
        }
        if r.y > 0.5 {
            lines.push(Rect::new(r.x, r.y - divider, r.width, divider));
        }
    }
    lines
}

pub(super) fn animated_rects(tab: &Tab, layout: &Layout, now: Instant) -> HashMap<PaneId, Rect> {
    let motion = tab.motion.as_ref();
    let at = |id: &PaneId, to: Rect| {
        motion
            .and_then(|m| m.from.get(id).map(|from| lerp_rect(*from, to, progress(Some(m.started), now))))
            .unwrap_or(to)
    };
    layout.panes().iter().map(|(id, to)| (*id, at(id, *to))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wired_layout::pane_tree::LayoutParams;

    fn closing_frames(tree: &mut PaneTree<&'static str>, closed: PaneId, bounds: Rect) -> Vec<Vec<f32>> {
        let params = LayoutParams { gap: 4., min_sizes: HashMap::new(), default_min: (50., 30.) };
        let before: HashMap<PaneId, Rect> = tree.layout(bounds, &params).panes().iter().copied().collect();
        let old = before[&closed];
        tree.close(closed, bounds, &params);
        let after = tree.layout(bounds, &params);
        let to = collapse_target(old, &before, after.panes());
        (0..=10)
            .map(|step| {
                let t = step as f32 / 10.;
                let mut rects: Vec<Rect> = after.panes().iter().map(|(id, r)| lerp_rect(before[id], *r, t)).collect();
                rects.push(lerp_rect(old, to, t));
                let mut lines: Vec<f32> = edge_lines(&rects, params.gap).iter().map(|l| l.x).collect();
                lines.sort_by(f32::total_cmp);
                lines
            })
            .collect()
    }

    #[test]
    fn closing_a_middle_pane_brings_both_dividers_together() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Right, "c").expect("split");
        let bounds = Rect::new(0., 0., 908., 600.);
        let before = tree.layout(bounds, &LayoutParams { gap: 4., ..Default::default() });
        let (left, right) = (before.pane(b).expect("b").x - 4., before.pane(c).expect("c").x - 4.);
        let frames = closing_frames(&mut tree, b, bounds);
        assert_eq!(frames[0], [left, right]);
        for pair in frames.windows(2) {
            // The left divider only moves right, the right one only left; they never pass.
            assert!(pair[1][0] >= pair[0][0] && pair[1][1] <= pair[0][1], "{pair:?}");
            assert!(pair[1][0] <= pair[1][1]);
        }
        let last = frames.last().expect("frames");
        assert_eq!(last[0], last[1], "one divider at the end: {last:?}");
    }

    #[test]
    fn closing_the_last_pane_of_a_widened_row_shrinks_it_sideways() {
        let params = LayoutParams { gap: 4., min_sizes: HashMap::new(), default_min: (50., 30.) };
        let bounds = Rect::new(0., 0., 908., 600.);
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let c = tree.split(b, Side::Right, "c").expect("split");
        let row = tree.layout(bounds, &params).rows()[0].id;
        tree.set_row_width(row, Some(3000.));
        let before: HashMap<PaneId, Rect> = tree.layout(bounds, &params).panes().iter().copied().collect();
        let old = before[&c];
        tree.close(c, bounds, &params);
        let after = tree.layout(bounds, &params);
        // Rounding may move them by a pixel; none reaches into c's place.
        for (id, r) in after.panes() {
            assert!((r.x - before[id].x).abs() <= 1. && r.x + r.width <= old.x, "{id:?} {r:?}");
        }
        let to = collapse_target(old, &before, after.panes());
        assert_eq!(to, Rect::new(old.x + old.width, old.y, 0., old.height));
    }

    #[test]
    fn closing_the_last_pane_shrinks_it_to_the_far_edge() {
        let mut tree = PaneTree::new("a");
        let a = tree.focused();
        let b = tree.split(a, Side::Right, "b").expect("split");
        let bounds = Rect::new(0., 0., 804., 600.);
        let frames = closing_frames(&mut tree, b, bounds);
        // Only the closed pane's own divider, moving to the window's right edge.
        assert_eq!(frames.last().expect("frames"), &[800.]);
    }
}
