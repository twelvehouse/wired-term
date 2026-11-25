//! Overlay scrollbar for the scrollback, Ubuntu style: a thin line that widens when the pointer
//! comes near the right edge. Drawn by the view itself (not gpui-component's scrollbar) so the
//! grab area reaches the window edge while the thumb keeps a gap to it.

use std::time::{Duration, Instant};

use gpui::{Bounds, Hsla, Pixels, Point, Window, fill, point, px, size};
use wired_vt::Scrollbar;

/// Width of the zone at the right edge that widens the bar and accepts presses.
pub const HOVER_ZONE: f32 = 16.;
/// Thickness of the bar away from the pointer.
pub const THIN_WIDTH: f32 = 3.;
const WIDE_WIDTH: f32 = 8.;
/// Gap between the thumb and the right edge, and at the top and bottom of the track.
pub const MARGIN: f32 = 4.;
/// Shortest thumb.
pub const MIN_THUMB: f32 = 32.;
const ANIMATION: Duration = Duration::from_millis(200);
const ALPHA: f32 = 0.35;
const HOT_ALPHA: f32 = 0.55;

/// Vertical extent of the thumb.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Thumb {
    pub(crate) top: Pixels,
    pub(crate) height: Pixels,
}

impl Thumb {
    pub(crate) fn contains_y(&self, y: Pixels) -> bool {
        y >= self.top && y <= self.top + self.height
    }
}

fn track(bounds: Bounds<Pixels>) -> (Pixels, Pixels) {
    (bounds.top() + px(MARGIN), (bounds.size.height - px(MARGIN * 2.)).max(px(1.)))
}

/// The thumb for the viewport `bar` in a pane of `bounds`; `None` without history to scroll.
pub(crate) fn thumb(bounds: Bounds<Pixels>, bar: Scrollbar) -> Option<Thumb> {
    let max_offset = bar.total.checked_sub(bar.len).filter(|&m| m > 0)?;
    let (track_top, track_height) = track(bounds);
    let height = (track_height * (bar.len as f32 / bar.total as f32)).max(px(MIN_THUMB)).min(track_height);
    let progress = (bar.offset.min(max_offset) as f32) / max_offset as f32;
    Some(Thumb { top: track_top + (track_height - height) * progress, height })
}

/// The viewport offset (in rows) that puts the top of a thumb of `height` at `top`.
pub(crate) fn offset_for(bounds: Bounds<Pixels>, bar: Scrollbar, height: Pixels, top: Pixels) -> u64 {
    let max_offset = bar.total.saturating_sub(bar.len);
    let (track_top, track_height) = track(bounds);
    let travel = track_height - height;
    if travel <= px(0.) {
        return 0;
    }
    let progress = ((top - track_top) / travel).clamp(0., 1.);
    (progress * max_offset as f32).round() as u64
}

pub(crate) fn in_zone(bounds: Bounds<Pixels>, position: Point<Pixels>) -> bool {
    position.x >= bounds.right() - px(HOVER_ZONE)
        && position.x <= bounds.right()
        && position.y >= bounds.top()
        && position.y <= bounds.bottom()
}

fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// Interaction and animation state of one overlay scrollbar.
#[derive(Debug)]
pub struct ScrollbarState {
    is_wide: bool,
    /// The pointer is over the thumb or dragging it.
    is_hot: bool,
    /// While dragging: the pointer's distance from the thumb's top.
    pub(crate) drag_grab: Option<Pixels>,
    width_from: f32,
    width_to: f32,
    started: Instant,
}

impl Default for ScrollbarState {
    fn default() -> Self {
        Self {
            is_wide: false,
            is_hot: false,
            drag_grab: None,
            width_from: THIN_WIDTH,
            width_to: THIN_WIDTH,
            started: Instant::now(),
        }
    }
}

impl ScrollbarState {
    /// Marks the thumb as under the pointer or grabbed (drawn brighter). Returns whether it changed.
    pub fn set_hot(&mut self, is_hot: bool) -> bool {
        std::mem::replace(&mut self.is_hot, is_hot) != is_hot
    }

    /// Starts animating toward the wide or thin width. Returns whether anything changed.
    pub fn set_wide(&mut self, is_wide: bool) -> bool {
        if is_wide == self.is_wide {
            return false;
        }
        let now = Instant::now();
        self.width_from = self.width(now).0;
        self.width_to = if is_wide { WIDE_WIDTH } else { THIN_WIDTH };
        self.started = now;
        self.is_wide = is_wide;
        true
    }

    /// Whether the pointer is in the zone (or dragging), so the bar is wide.
    pub fn is_wide(&self) -> bool {
        self.is_wide
    }

    /// Current width and whether the animation is still running.
    pub fn width(&self, now: Instant) -> (f32, bool) {
        let t = now.saturating_duration_since(self.started).as_secs_f32() / ANIMATION.as_secs_f32();
        if t >= 1. || self.width_from == self.width_to {
            (self.width_to, false)
        } else {
            (self.width_from + (self.width_to - self.width_from) * ease_out_cubic(t), true)
        }
    }

    /// Opacity of the thumb.
    pub fn alpha(&self) -> f32 {
        if self.is_hot || self.drag_grab.is_some() { HOT_ALPHA } else { ALPHA }
    }
}

/// Paints the thumb; the radius is half the current width, so the ends stay round while the
/// width animates.
pub(crate) fn paint(window: &mut Window, bounds: Bounds<Pixels>, thumb: Thumb, width: f32, color: Hsla) {
    let width = px(width);
    let rect = Bounds::new(point(bounds.right() - px(MARGIN) - width, thumb.top), size(width, thumb.height));
    window.paint_quad(fill(rect, color).corner_radii(width / 2.));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(0.)), size(px(800.), px(408.)))
    }

    #[test]
    fn no_thumb_without_history() {
        assert_eq!(thumb(pane(), Scrollbar { total: 24, offset: 0, len: 24 }), None);
    }

    #[test]
    fn thumb_tracks_offset_and_round_trips() {
        let bar = Scrollbar { total: 1000, offset: 0, len: 25 };
        let top = thumb(pane(), bar).expect("thumb");
        assert_eq!(top.top, px(MARGIN));
        assert_eq!(top.height, px(MIN_THUMB));
        let bottom = thumb(pane(), Scrollbar { offset: 975, ..bar }).expect("thumb");
        assert_eq!(bottom.top + bottom.height, px(408. - MARGIN));
        let middle = thumb(pane(), Scrollbar { offset: 500, ..bar }).expect("thumb");
        assert_eq!(offset_for(pane(), bar, middle.height, middle.top), 500);
    }

    #[test]
    fn zone_reaches_the_edge() {
        let b = pane();
        assert!(in_zone(b, point(px(800.), px(10.))));
        assert!(in_zone(b, point(px(784.), px(10.))));
        assert!(!in_zone(b, point(px(783.), px(10.))));
    }

    #[test]
    fn width_animates_and_settles() {
        let mut state = ScrollbarState::default();
        assert!(!state.width(Instant::now()).1, "a new scrollbar asks for no frames");
        assert!(state.set_wide(true));
        assert!(!state.set_wide(true));
        let (_, running) = state.width(Instant::now());
        assert!(running);
        assert_eq!(state.width(Instant::now() + ANIMATION).0, WIDE_WIDTH);
    }
}
