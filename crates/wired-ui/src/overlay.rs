//! Transient information drawn over a pane or the window: a size box at the center of a pane with
//! labels around it, and single labels at the edge of an area.

use std::time::{Duration, Instant};

use gpui::{BoxShadow, Div, FontWeight, ParentElement, Styled, Window, div, hsla, point, px, rems};
use wired_layout::pane_tree::Rect;

use crate::title_bar::UI_FONT;

/// Dims content behind an overlay (the shortcut hint, the profile picker) by `level` (0 to 1),
/// without taking input.
pub(crate) fn background(level: f32) -> Option<Div> {
    (level > 0.).then(|| div().absolute().top_0().left_0().size_full().bg(hsla(0., 0., 0., 0.45 * level)))
}

/// Whether the Ctrl+Shift chord of a held shortcut hint is still down in the active window.
pub(crate) fn is_chord_held(window: &Window, is_hint_held: bool) -> bool {
    let modifiers = window.modifiers();
    is_hint_held && window.is_window_active() && modifiers.control && modifiers.shift
}

/// The panel of a transient box: no frame, translucent.
pub(crate) fn panel() -> Div {
    div()
        .rounded(px(6.))
        .bg(hsla(0., 0., 0.1, 0.8))
        .text_color(hsla(0., 0., 1., 0.92))
        .text_size(px(14.))
        .font_family(UI_FONT)
}

const HOLD: Duration = Duration::from_millis(1500);
const FADE: Duration = Duration::from_millis(500);

const LABEL_TEXT_SIZE: f32 = 0.875;
const LABEL_GAP: f32 = LABEL_TEXT_SIZE * 2.;
const EDGE_MARGIN: f32 = 12.;

const ICON_FONT: &str = "Segoe Fluent Icons";
const GLYPH_LEFT: &str = "\u{E72B}";
const GLYPH_RIGHT: &str = "\u{E72A}";
const GLYPH_UP: &str = "\u{E74A}";
const GLYPH_DOWN: &str = "\u{E74B}";
const GLYPH_CLOSE: &str = "\u{E8BB}";
const GLYPH_MOUSE: &str = "\u{E962}";

/// Opacity of an overlay shown at `since` (1 while held, then fading); `None` once it is gone.
pub(crate) fn hold_then_fade(since: Instant, now: Instant) -> Option<f32> {
    let past_hold = now.saturating_duration_since(since).checked_sub(HOLD).unwrap_or_default();
    let t = past_hold.as_secs_f32() / FADE.as_secs_f32();
    (t < 1.).then_some(1. - t)
}

/// The picture in front of a label, drawn from icon-font glyphs and a thin line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    SplitRight,
    SplitDown,
    Close,
    Mouse,
}

/// What to do and how: an icon, the keys or gesture for it, and an optional description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Label {
    pub icon: Icon,
    pub keys: String,
    pub text: Option<&'static str>,
}

/// The labels around a size box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Labels {
    pub above: Option<Label>,
    pub right: Option<Label>,
    pub below: Option<Label>,
}

/// The edge of an area a label is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Top,
    Bottom,
}

/// A box with `text` at the center of `area`, with `labels` (and their opacity) on its sides.
pub(crate) fn size_box(area: Rect, text: String, opacity: f32, labels: Option<(Labels, f32)>) -> Div {
    let (Labels { above, right, below }, label_opacity) = labels.unwrap_or_default();
    let shown = |label: Option<Label>| label.map(|l| label_view(l).opacity(label_opacity));
    let middle = div()
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .child(div().flex_1().min_w_0())
        .child(panel().flex_none().px(px(14.)).py(px(8.)).opacity(opacity).child(text))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .child(div().w(rems(LABEL_GAP)).min_w_0().flex_initial())
                .children(shown(right)),
        );
    placed(area)
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_end()
                .children(shown(above))
                .child(div().h(rems(LABEL_GAP)).min_h_0().flex_initial()),
        )
        .child(middle)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_start()
                .child(div().h(rems(LABEL_GAP)).min_h_0().flex_initial())
                .children(shown(below)),
        )
}

/// `label` at the top or bottom of `area`, horizontally centered, at `opacity`.
pub(crate) fn pinned(area: Rect, edge: Edge, label: Label, opacity: f32) -> Div {
    let at = placed(area).flex().flex_col().items_center().opacity(opacity);
    let at = match edge {
        Edge::Top => at.justify_start().pt(px(EDGE_MARGIN)),
        Edge::Bottom => at.justify_end().pb(px(EDGE_MARGIN)),
    };
    at.child(label_view(label))
}

fn placed(area: Rect) -> Div {
    div().absolute().left(px(area.x)).top(px(area.y)).w(px(area.width)).h(px(area.height))
}

fn label_view(label: Label) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .py(px(4.))
        .rounded(px(6.))
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.2),
            offset: point(px(0.), px(1.)),
            blur_radius: px(36.),
            spread_radius: px(0.),
            inset: false,
        }])
        .text_color(hsla(0., 0., 0.92, 1.))
        .text_size(rems(LABEL_TEXT_SIZE))
        .font_family(UI_FONT)
        .whitespace_nowrap()
        .child(icon(label.icon))
        .child(div().font_weight(FontWeight::MEDIUM).child(label.keys))
        .children(label.text.map(|text| div().font_weight(FontWeight::MEDIUM).child(text)))
}

fn icon(icon: Icon) -> Div {
    let glyph = |code: &'static str| div().font_family(ICON_FONT).text_size(px(12.)).line_height(px(12.)).child(code);
    let line = |width: f32, height: f32| div().w(px(width)).h(px(height)).bg(hsla(0., 0., 0.92, 1.));
    match icon {
        Icon::SplitRight => div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(glyph(GLYPH_LEFT))
            .child(line(1., 14.))
            .child(glyph(GLYPH_RIGHT)),
        Icon::SplitDown => {
            div().flex().flex_col().items_center().child(glyph(GLYPH_UP)).child(line(14., 1.)).child(glyph(GLYPH_DOWN))
        }
        Icon::Close => glyph(GLYPH_CLOSE),
        Icon::Mouse => glyph(GLYPH_MOUSE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_opaque_while_held_then_fades_out() {
        let start = Instant::now();
        assert_eq!(hold_then_fade(start, start), Some(1.));
        assert_eq!(hold_then_fade(start, start + HOLD), Some(1.));
        let halfway = hold_then_fade(start, start + HOLD + FADE / 2).expect("fading");
        assert!((halfway - 0.5).abs() < 0.01);
        assert_eq!(hold_then_fade(start, start + HOLD + FADE), None);
    }
}
