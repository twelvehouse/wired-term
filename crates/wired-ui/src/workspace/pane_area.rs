//! The pane area of the active tab: layout, dividers and the drawn panes.

use gpui::{Div, Stateful};
use wired_layout::pane_tree::Divider;

use super::*;
use crate::overlay;

const DIVIDER_GRAB: f32 = 4.;

/// Columns and rows of each pane's own font kept when space allows.
const MIN_PANE_GRID: (u16, u16) = (10, 3);

/// Minimum pane size before its font has been measured.
const FALLBACK_MIN_PANE: (f32, f32) = (100., 60.);

impl Workspace {
    fn viewport(&self) -> Rect {
        self.pane_area.get().map_or(Rect::new(0., 0., 1000., 1000.), |b| {
            Rect::new(0., 0., f32::from(b.size.width), f32::from(b.size.height))
        })
    }

    pub(super) fn pane_geometry(&self, index: usize, cx: &App) -> (Rect, LayoutParams) {
        let min_sizes = self.tabs.get(index).map_or_else(HashMap::new, |tab| {
            let size = |pane: &Pane| pane.view.read(cx).size_for_grid(MIN_PANE_GRID.0, MIN_PANE_GRID.1);
            tab.panes.panes().into_iter().filter_map(|(id, pane)| size(pane).map(|s| (id, s))).collect()
        });
        let params = LayoutParams { gap: self.appearance.divider_width, min_sizes, default_min: FALLBACK_MIN_PANE };
        (self.viewport(), params)
    }

    pub(super) fn layout(&self, index: usize, cx: &App) -> Layout {
        let (area, params) = self.pane_geometry(index, cx);
        self.tabs.get(index).map(|tab| tab.panes.layout(area, &params)).unwrap_or_default()
    }

    fn on_drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            // Released where we could not see it.
            self.end_drags(cx);
            return;
        }
        if let Some(drag) = self.divider_drag {
            self.drag_divider(drag, event, cx);
        } else if let Some(drag) = self.thumb_drag {
            self.drag_row_thumb(drag, f32::from(event.position.x), cx);
        }
    }

    pub(super) fn end_drags(&mut self, cx: &mut Context<Self>) {
        self.divider_drag = None;
        if let Some(drag) = self.thumb_drag.take()
            && let Some(bar) = self.tabs.get_mut(self.active).and_then(|t| t.bars.get_mut(&drag.row))
        {
            bar.set_hot(false);
            bar.set_wide(false);
        }
        cx.notify();
    }

    fn drag_divider(&mut self, drag: DividerDrag, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(origin) = self.pane_area.get().map(|b| b.origin) else { return };
        let index = self.active;
        let (area, params) = self.pane_geometry(index, cx);
        let Some(tab) = self.tabs.get_mut(index) else { return };
        // Panes were added or removed under the pointer: the divider may now be another one.
        if tab.panes.revision() != drag.revision {
            self.end_drags(cx);
            return;
        }
        let layout = tab.panes.layout(area, &params);
        let Some(divider) = layout.dividers().iter().find(|d| d.id == drag.id) else { return };
        let pointer = event.position - origin;
        let center = match divider.axis {
            Axis::Horizontal => f32::from(pointer.x),
            Axis::Vertical => f32::from(pointer.y),
        };
        if tab.panes.drag_divider(drag.id, center - params.gap / 2., area, &params) {
            cx.notify();
        }
    }
}

fn placed(r: Rect) -> Div {
    div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.width)).h(px(r.height))
}

impl Workspace {
    pub(super) fn pane_area(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let index = self.active;
        let Some(tab) = self.tabs.get(index) else { return div().into_any_element() };
        let now = Instant::now();
        // Before layout: a zoom, font reload or monitor scale change alters the minimum sizes.
        for (_, pane) in tab.panes.panes() {
            pane.view.read(cx).measure(window);
        }
        let (area, params) = self.pane_geometry(index, cx);
        let layout = tab.panes.layout(area, &params);
        // Every tab's terminals get their final grid so background programs follow a resize at
        // once; scrolling and animations only move and clip what is drawn.
        if self.pane_area.get().is_some() {
            for (i, each) in self.tabs.iter().enumerate() {
                let other;
                let each_layout = if i == index {
                    &layout
                } else {
                    other = self.layout(i, cx);
                    &other
                };
                for (id, pane) in each.panes.panes() {
                    if let Some(r) = each_layout.pane(id) {
                        pane.view.read(cx).fit(r.width, r.height);
                    }
                }
            }
        }
        let drawn = self.pane_area.get().map(|_| animated_rects(tab, &layout, now));
        let moving = tab.motion.as_ref().filter(|_| drawn.is_some());
        let bar_widths: HashMap<RowId, (f32, bool)> = tab.bars.iter().map(|(id, bar)| (*id, bar.width(now))).collect();
        if moving.is_some() || bar_widths.values().any(|(_, is_animating)| *is_animating) {
            window.request_animation_frame();
        }
        let rects = drawn.unwrap_or_else(|| layout.panes().iter().copied().collect());
        let focused = tab.panes.focused();
        let (hint_opacity, is_hint_changing) = self.hint.opacity(now);
        let hint_target = self.hint.target(tab.id, now);
        let mut is_resizing = false;
        let mut sizes: Vec<Div> = Vec::new();
        for (id, pane) in tab.panes.panes() {
            let (Some(grid), Some(rect)) = (pane.view.read(cx).grid_change(), rects.get(&id)) else { continue };
            let recent = overlay::hold_then_fade(grid.changed_at, now);
            is_resizing |= recent.is_some();
            let opacity = recent.unwrap_or(0.).max(hint_opacity);
            if opacity > 0. {
                let labels = (Some(id) == hint_target).then(|| (self.pane_labels(cx), hint_opacity));
                sizes.push(overlay::size_box(*rect, format!("{} × {}", grid.cols, grid.rows), opacity, labels));
            }
        }
        if is_resizing || is_hint_changing {
            window.request_animation_frame();
        }
        let row_width = hint_target
            .and_then(|id| rects.get(&id))
            .and_then(|rect| self.row_width_hint(area, *rect, hint_opacity, cx));
        let is_visible = |r: &Rect| r.x < area.width && r.x + r.width > 0.;
        // Off-screen panes keep running; only their drawing is skipped.
        let mut views: Vec<(Entity<TerminalView>, Rect)> = tab
            .panes
            .panes()
            .into_iter()
            .filter_map(|(id, pane)| {
                let r = *rects.get(&id)?;
                (id == focused || is_visible(&r)).then(|| (pane.view.clone(), r))
            })
            .collect();
        let lines = match moving {
            Some(motion) => {
                let t = progress(Some(motion.started), now);
                views.extend(motion.ghosts.iter().map(|g| (g.view.clone(), lerp_rect(g.from, g.to, t))));
                let all: Vec<Rect> = views.iter().map(|(_, r)| *r).collect();
                edge_lines(&all, params.gap)
            }
            None => layout.dividers().iter().map(|d| d.rect).collect(),
        };
        let dividers = if moving.is_some() { &[][..] } else { layout.dividers() };
        let grabs = self.divider_grabs(dividers, DIVIDER_GRAB.max(params.gap), tab.panes.revision(), cx);
        let tracker = self.drag_tracker(cx);
        let bars = self.row_bars(tab, &layout, &bar_widths, cx);
        // Only the translucent terminal is dimmed: the opaque divider lines go on top again.
        let dim = overlay::background(self.dim.level(now).0);
        let bar = self.appearance.bar.bar;
        let lines_over_dim = if dim.is_some() { lines.clone() } else { Vec::new() };
        div()
            .id("pane-area")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(self.background)
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| this.on_row_wheel(event, None, cx)))
            .children(lines.into_iter().map(|r| placed(r).bg(bar)))
            .children(views.into_iter().map(|(view, r)| placed(r).overflow_hidden().child(view)))
            .children(dim)
            .children(lines_over_dim.into_iter().map(|r| placed(r).bg(bar)))
            .children(sizes)
            .children(row_width)
            .children(grabs)
            .child(tracker)
            .children(bars)
            .into_any_element()
    }

    fn divider_grabs(
        &self,
        dividers: &[Divider],
        grab: f32,
        revision: u64,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        dividers
            .iter()
            .map(|divider| {
                let r = divider.rect;
                let (element, cursor) = match divider.axis {
                    Axis::Horizontal => (
                        div().left(px(r.x + (r.width - grab) / 2.)).top(px(r.y)).w(px(grab)).h(px(r.height)),
                        CursorStyle::ResizeLeftRight,
                    ),
                    Axis::Vertical => (
                        div().left(px(r.x)).top(px(r.y + (r.height - grab) / 2.)).w(px(r.width)).h(px(grab)),
                        CursorStyle::ResizeUpDown,
                    ),
                };
                let id = divider.id;
                element
                    .id(ElementId::Name(format!("divider-{:?}-{}-{revision}", id.group, id.index).into()))
                    .absolute()
                    .cursor(cursor)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if event.click_count >= 2 {
                                if Settings::get(cx).mouse(gesture::DIVIDER_DOUBLE_CLICK) == MouseAction::Equalize {
                                    this.equalize_divider(id, cx);
                                }
                            } else {
                                this.divider_drag = Some(DividerDrag { id, revision });
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
            })
            .collect()
    }

    fn drag_tracker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let measured = Rc::clone(&self.pane_area);
        let workspace = cx.entity().downgrade();
        let is_dragging = self.divider_drag.is_some() || self.thumb_drag.is_some();
        canvas(
            move |bounds, window, _| {
                // The divider areas are placed from the previous frame's size; redraw when it changed.
                if measured.replace(Some(bounds)) != Some(bounds) {
                    window.refresh();
                }
            },
            move |_, _, window, _| {
                if !is_dragging {
                    return;
                }
                let on_move = workspace.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = on_move.update(cx, |this, cx| this.on_drag_move(event, cx));
                    }
                });
                let on_up = workspace.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                        let _ = on_up.update(cx, |this, cx| this.end_drags(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    fn row_bars(
        &self,
        tab: &Tab,
        layout: &Layout,
        bar_widths: &HashMap<RowId, (f32, bool)>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut bars = Vec::new();
        for row in layout.rows() {
            let Some((left, width)) = row_thumb(row) else { continue };
            let key = row.id;
            let state = tab.bars.get(&key);
            let thickness = bar_widths.get(&key).map_or(scrollbar::THIN_WIDTH, |(w, _)| *w);
            let alpha = state.map_or(ScrollbarState::default().alpha(), ScrollbarState::alpha);
            let color = Hsla { a: alpha, ..self.appearance.foreground };
            let r = row.rect;
            let thumb = Rect::new(r.x + left, r.y + r.height - scrollbar::MARGIN - thickness, width, thickness);
            bars.push(placed(thumb).rounded(px(thickness / 2.)).bg(color).into_any_element());
            let zone = div()
                .id(ElementId::Name(format!("row-bar-{key:?}").into()))
                .absolute()
                .left(px(r.x))
                .top(px(r.y + r.height - scrollbar::HOVER_ZONE))
                .w(px(r.width))
                .h(px(scrollbar::HOVER_ZONE))
                .occlude()
                .on_hover(cx.listener(move |this, is_hovered: &bool, _, cx| {
                    if !*is_hovered {
                        this.hover_row_bar(key, None, cx);
                    }
                }))
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    this.hover_row_bar(key, Some(f32::from(event.position.x)), cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.press_row_bar(key, event, cx);
                        cx.stop_propagation();
                    }),
                )
                .on_scroll(
                    cx.listener(move |this, event: &ScrollEvent, _, cx| this.on_row_wheel(event, Some(key), cx)),
                );
            bars.push(zone.into_any_element());
        }
        bars
    }
}
