//! Box drawing, block elements and Powerline separators, drawn to fill the cell exactly: font
//! glyphs fit only the font's own cell size, so in any other cell lines miss and blocks leave seams.
//! Geometry follows Ghostty's sprite font.

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use gpui::{App, Bounds, Hsla, PathBuilder, Pixels, SharedString, TransformationMatrix, Window, fill, point, px, size};

/// Returns whether `text` is a single character drawn by [`paint`] instead of the font.
pub(crate) fn is_custom(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if sprite(c).is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Weight {
    None,
    Light,
    Heavy,
    Double,
}

#[derive(Clone, Copy, Debug)]
struct Lines {
    up: Weight,
    right: Weight,
    down: Weight,
    left: Weight,
}

#[derive(Clone, Copy, Debug)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug)]
enum DashGap {
    AtLeastFour,
    Light,
    Heavy,
}

#[derive(Clone, Copy, Debug)]
enum Sprite {
    Lines(Lines),
    Dash { vertical: bool, count: i32, heavy: bool, gap: DashGap },
    Arc(Corner),
    Diagonal { rising: bool, falling: bool },
    Block { w8: i32, h8: i32, right: bool, bottom: bool },
    Shade(Shade),
    Quadrants(u8),
    Powerline(Shape),
}

/// Falling hatches one dot thick, every 6th, 3rd and 2nd dot from light to dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Shade {
    Light,
    Medium,
    Dark,
}

impl Shade {
    fn period(self) -> i32 {
        match self {
            Shade::Light => 6,
            Shade::Medium => 3,
            Shade::Dark => 2,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    Fill(&'static [(f32, f32)]),
    Stroke(&'static [(f32, f32)]),
    HalfDisc(Facing),
    HalfCircle(Facing),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Facing {
    Left,
    Right,
}

const TOP_LEFT: u8 = 1 << 0;
const TOP_RIGHT: u8 = 1 << 1;
const BOTTOM_LEFT: u8 = 1 << 2;
const BOTTOM_RIGHT: u8 = 1 << 3;

/// `spec` is up, right, down, left, each `.` none, `l` light, `h` heavy or `d` double.
const fn parse_lines(spec: &[u8; 4]) -> Lines {
    const fn weight(c: u8) -> Weight {
        match c {
            b'l' => Weight::Light,
            b'h' => Weight::Heavy,
            b'd' => Weight::Double,
            _ => Weight::None,
        }
    }
    Lines { up: weight(spec[0]), right: weight(spec[1]), down: weight(spec[2]), left: weight(spec[3]) }
}

const fn lines(spec: &[u8; 4]) -> Sprite {
    Sprite::Lines(parse_lines(spec))
}

const fn dash(vertical: bool, count: i32, heavy: bool, gap: DashGap) -> Sprite {
    Sprite::Dash { vertical, count, heavy, gap }
}

const fn block(w8: i32, h8: i32, right: bool, bottom: bool) -> Sprite {
    Sprite::Block { w8, h8, right, bottom }
}

fn sprite(c: char) -> Option<Sprite> {
    use DashGap::{AtLeastFour, Heavy, Light};
    Some(match c {
        '─' => lines(b".l.l"),
        '━' => lines(b".h.h"),
        '│' => lines(b"l.l."),
        '┃' => lines(b"h.h."),
        '┄' => dash(false, 3, false, AtLeastFour),
        '┅' => dash(false, 3, true, AtLeastFour),
        '┆' => dash(true, 3, false, AtLeastFour),
        '┇' => dash(true, 3, true, AtLeastFour),
        '┈' => dash(false, 4, false, AtLeastFour),
        '┉' => dash(false, 4, true, AtLeastFour),
        '┊' => dash(true, 4, false, AtLeastFour),
        '┋' => dash(true, 4, true, AtLeastFour),
        '┌' => lines(b".ll."),
        '┍' => lines(b".hl."),
        '┎' => lines(b".lh."),
        '┏' => lines(b".hh."),
        '┐' => lines(b"..ll"),
        '┑' => lines(b"..lh"),
        '┒' => lines(b"..hl"),
        '┓' => lines(b"..hh"),
        '└' => lines(b"ll.."),
        '┕' => lines(b"lh.."),
        '┖' => lines(b"hl.."),
        '┗' => lines(b"hh.."),
        '┘' => lines(b"l..l"),
        '┙' => lines(b"l..h"),
        '┚' => lines(b"h..l"),
        '┛' => lines(b"h..h"),
        '├' => lines(b"lll."),
        '┝' => lines(b"lhl."),
        '┞' => lines(b"hll."),
        '┟' => lines(b"llh."),
        '┠' => lines(b"hlh."),
        '┡' => lines(b"hhl."),
        '┢' => lines(b"lhh."),
        '┣' => lines(b"hhh."),
        '┤' => lines(b"l.ll"),
        '┥' => lines(b"l.lh"),
        '┦' => lines(b"h.ll"),
        '┧' => lines(b"l.hl"),
        '┨' => lines(b"h.hl"),
        '┩' => lines(b"h.lh"),
        '┪' => lines(b"l.hh"),
        '┫' => lines(b"h.hh"),
        '┬' => lines(b".lll"),
        '┭' => lines(b".llh"),
        '┮' => lines(b".hll"),
        '┯' => lines(b".hlh"),
        '┰' => lines(b".lhl"),
        '┱' => lines(b".lhh"),
        '┲' => lines(b".hhl"),
        '┳' => lines(b".hhh"),
        '┴' => lines(b"ll.l"),
        '┵' => lines(b"ll.h"),
        '┶' => lines(b"lh.l"),
        '┷' => lines(b"lh.h"),
        '┸' => lines(b"hl.l"),
        '┹' => lines(b"hl.h"),
        '┺' => lines(b"hh.l"),
        '┻' => lines(b"hh.h"),
        '┼' => lines(b"llll"),
        '┽' => lines(b"lllh"),
        '┾' => lines(b"lhll"),
        '┿' => lines(b"lhlh"),
        '╀' => lines(b"hlll"),
        '╁' => lines(b"llhl"),
        '╂' => lines(b"hlhl"),
        '╃' => lines(b"hllh"),
        '╄' => lines(b"hhll"),
        '╅' => lines(b"llhh"),
        '╆' => lines(b"lhhl"),
        '╇' => lines(b"hhlh"),
        '╈' => lines(b"lhhh"),
        '╉' => lines(b"hlhh"),
        '╊' => lines(b"hhhl"),
        '╋' => lines(b"hhhh"),
        '╌' => dash(false, 2, false, Light),
        '╍' => dash(false, 2, true, Heavy),
        '╎' => dash(true, 2, false, Heavy),
        '╏' => dash(true, 2, true, Heavy),
        '═' => lines(b".d.d"),
        '║' => lines(b"d.d."),
        '╒' => lines(b".dl."),
        '╓' => lines(b".ld."),
        '╔' => lines(b".dd."),
        '╕' => lines(b"..ld"),
        '╖' => lines(b"..dl"),
        '╗' => lines(b"..dd"),
        '╘' => lines(b"ld.."),
        '╙' => lines(b"dl.."),
        '╚' => lines(b"dd.."),
        '╛' => lines(b"l..d"),
        '╜' => lines(b"d..l"),
        '╝' => lines(b"d..d"),
        '╞' => lines(b"ldl."),
        '╟' => lines(b"dld."),
        '╠' => lines(b"ddd."),
        '╡' => lines(b"l.ld"),
        '╢' => lines(b"d.dl"),
        '╣' => lines(b"d.dd"),
        '╤' => lines(b".dld"),
        '╥' => lines(b".ldl"),
        '╦' => lines(b".ddd"),
        '╧' => lines(b"ld.d"),
        '╨' => lines(b"dl.l"),
        '╩' => lines(b"dd.d"),
        '╪' => lines(b"ldld"),
        '╫' => lines(b"dldl"),
        '╬' => lines(b"dddd"),
        '╭' => Sprite::Arc(Corner::BottomRight),
        '╮' => Sprite::Arc(Corner::BottomLeft),
        '╯' => Sprite::Arc(Corner::TopLeft),
        '╰' => Sprite::Arc(Corner::TopRight),
        '╱' => Sprite::Diagonal { rising: true, falling: false },
        '╲' => Sprite::Diagonal { rising: false, falling: true },
        '╳' => Sprite::Diagonal { rising: true, falling: true },
        '╴' => lines(b"...l"),
        '╵' => lines(b"l..."),
        '╶' => lines(b".l.."),
        '╷' => lines(b"..l."),
        '╸' => lines(b"...h"),
        '╹' => lines(b"h..."),
        '╺' => lines(b".h.."),
        '╻' => lines(b"..h."),
        '╼' => lines(b".h.l"),
        '╽' => lines(b"l.h."),
        '╾' => lines(b".l.h"),
        '╿' => lines(b"h.l."),

        '▀' => block(8, 4, false, false),
        '▁' => block(8, 1, false, true),
        '▂' => block(8, 2, false, true),
        '▃' => block(8, 3, false, true),
        '▄' => block(8, 4, false, true),
        '▅' => block(8, 5, false, true),
        '▆' => block(8, 6, false, true),
        '▇' => block(8, 7, false, true),
        '█' => block(8, 8, false, false),
        '▉' => block(7, 8, false, false),
        '▊' => block(6, 8, false, false),
        '▋' => block(5, 8, false, false),
        '▌' => block(4, 8, false, false),
        '▍' => block(3, 8, false, false),
        '▎' => block(2, 8, false, false),
        '▏' => block(1, 8, false, false),
        '▐' => block(4, 8, true, false),
        '░' => Sprite::Shade(Shade::Light),
        '▒' => Sprite::Shade(Shade::Medium),
        '▓' => Sprite::Shade(Shade::Dark),
        '▔' => block(8, 1, false, false),
        '▕' => block(1, 8, true, false),
        '▖' => Sprite::Quadrants(BOTTOM_LEFT),
        '▗' => Sprite::Quadrants(BOTTOM_RIGHT),
        '▘' => Sprite::Quadrants(TOP_LEFT),
        '▙' => Sprite::Quadrants(TOP_LEFT | BOTTOM_LEFT | BOTTOM_RIGHT),
        '▚' => Sprite::Quadrants(TOP_LEFT | BOTTOM_RIGHT),
        '▛' => Sprite::Quadrants(TOP_LEFT | TOP_RIGHT | BOTTOM_LEFT),
        '▜' => Sprite::Quadrants(TOP_LEFT | TOP_RIGHT | BOTTOM_RIGHT),
        '▝' => Sprite::Quadrants(TOP_RIGHT),
        '▞' => Sprite::Quadrants(TOP_RIGHT | BOTTOM_LEFT),
        '▟' => Sprite::Quadrants(TOP_RIGHT | BOTTOM_LEFT | BOTTOM_RIGHT),

        '\u{E0B0}' => Sprite::Powerline(Shape::Fill(&[(0., 0.), (1., 0.5), (0., 1.)])),
        '\u{E0B1}' => Sprite::Powerline(Shape::Stroke(&[(0., 0.), (1., 0.5), (0., 1.)])),
        '\u{E0B2}' => Sprite::Powerline(Shape::Fill(&[(1., 0.), (0., 0.5), (1., 1.)])),
        '\u{E0B3}' => Sprite::Powerline(Shape::Stroke(&[(1., 0.), (0., 0.5), (1., 1.)])),
        '\u{E0B4}' => Sprite::Powerline(Shape::HalfDisc(Facing::Right)),
        '\u{E0B5}' => Sprite::Powerline(Shape::HalfCircle(Facing::Right)),
        '\u{E0B6}' => Sprite::Powerline(Shape::HalfDisc(Facing::Left)),
        '\u{E0B7}' => Sprite::Powerline(Shape::HalfCircle(Facing::Left)),
        '\u{E0B8}' => Sprite::Powerline(Shape::Fill(&[(0., 0.), (1., 1.), (0., 1.)])),
        '\u{E0BA}' => Sprite::Powerline(Shape::Fill(&[(1., 0.), (1., 1.), (0., 1.)])),
        '\u{E0BC}' => Sprite::Powerline(Shape::Fill(&[(0., 0.), (1., 0.), (0., 1.)])),
        '\u{E0BE}' => Sprite::Powerline(Shape::Fill(&[(0., 0.), (1., 0.), (1., 1.)])),
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

fn rect(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// A cell in device pixels. `x` and `y` place it on the screen so shade patterns continue across
/// cells; `t` is the light line width.
#[derive(Clone, Copy, Debug)]
struct Cell {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    t: i32,
}

/// Dot size in device pixels, as in Windows Terminal.
fn shade_dot(w: i32, h: i32) -> i32 {
    (w as f32 / 12.).max(h as f32 / 24.).round().max(1.) as i32
}

/// Dots on a grid fixed to the screen, merged into runs per row.
fn shade_rects(Cell { x, y, w, h, .. }: Cell, shade: Shade, out: &mut Vec<Rect>) {
    let dot = shade_dot(w, h);
    let lit = |gx: i32, gy: i32| (gx - gy).rem_euclid(shade.period()) == 0;
    let (first_x, last_x) = (x.div_euclid(dot), (x + w - 1).div_euclid(dot));
    for gy in y.div_euclid(dot)..=(y + h - 1).div_euclid(dot) {
        let (y0, y1) = ((gy * dot).max(y) - y, ((gy + 1) * dot).min(y + h) - y);
        let mut run: Option<i32> = None;
        for gx in first_x..=last_x + 1 {
            match (run, gx <= last_x && lit(gx, gy)) {
                (None, true) => run = Some(gx),
                (Some(start), false) => {
                    out.push(rect((start * dot).max(x) - x, y0, (gx * dot).min(x + w) - x, y1));
                    run = None;
                }
                _ => {}
            }
        }
    }
}

/// The smallest cell position with the same shade pixels as `cell`.
fn shade_phase(Cell { x, y, w, h, .. }: Cell, shade: Shade) -> (i32, i32) {
    let dot = shade_dot(w, h);
    let x = x.rem_euclid(dot) + dot * (x.div_euclid(dot) - y.div_euclid(dot)).rem_euclid(shade.period());
    (x, y.rem_euclid(dot))
}

/// The cell's shade as an SVG alpha mask, cached by size and [`shade_phase`] so a few masks cover a screen.
fn shade_svg(cell: Cell, shade: Shade) -> (SharedString, Arc<[u8]>) {
    type Key = (Shade, i32, i32, i32, i32);
    type Mask = (SharedString, Arc<[u8]>);
    thread_local! {
        static CACHE: RefCell<HashMap<Key, Mask>> = RefCell::default();
    }
    let Cell { w, h, .. } = cell;
    let (phase_x, phase_y) = shade_phase(cell, shade);
    let key = (shade, w, h, phase_x, phase_y);
    CACHE.with_borrow_mut(|cache| {
        cache
            .entry(key)
            .or_insert_with(|| {
                let mut rects = Vec::new();
                shade_rects(Cell { x: phase_x, y: phase_y, ..cell }, shade, &mut rects);
                let mut svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" shape-rendering="crispEdges"><path d=""#
                );
                for r in rects {
                    svg.push_str(&format!("M{} {}h{}v{}h-{}z", r.x0, r.y0, r.x1 - r.x0, r.y1 - r.y0, r.x1 - r.x0));
                }
                svg.push_str(r#""/></svg>"#);
                (format!("wired-shade/{shade:?}/{w}x{h}/{phase_x},{phase_y}").into(), svg.into_bytes().into())
            })
            .clone()
    })
}

fn lines_rects(Cell { w, h, t, .. }: Cell, l: Lines, out: &mut Vec<Rect>) {
    use Weight::{Double, Heavy, None};
    let heavy = t * 2;
    let h_light_top = (h - t).max(0) / 2;
    let h_light_bottom = h_light_top + t;
    let h_heavy_top = (h - heavy).max(0) / 2;
    let h_heavy_bottom = h_heavy_top + heavy;
    let h_double_top = (h_light_top - t).max(0);
    let h_double_bottom = h_light_bottom + t;
    let v_light_left = (w - t).max(0) / 2;
    let v_light_right = v_light_left + t;
    let v_heavy_left = (w - heavy).max(0) / 2;
    let v_heavy_right = v_heavy_left + heavy;
    let v_double_left = (v_light_left - t).max(0);
    let v_double_right = v_light_right + t;

    let up_bottom = if l.left == Heavy || l.right == Heavy {
        h_heavy_bottom
    } else if l.left != l.right || l.down == l.up {
        if l.left == Double || l.right == Double { h_double_bottom } else { h_light_bottom }
    } else if l.left == None && l.right == None {
        h_light_bottom
    } else {
        h_light_top
    };
    let down_top = if l.left == Heavy || l.right == Heavy {
        h_heavy_top
    } else if l.left != l.right || l.up == l.down {
        if l.left == Double || l.right == Double { h_double_top } else { h_light_top }
    } else if l.left == None && l.right == None {
        h_light_top
    } else {
        h_light_bottom
    };
    let left_right = if l.up == Heavy || l.down == Heavy {
        v_heavy_right
    } else if l.up != l.down || l.left == l.right {
        if l.up == Double || l.down == Double { v_double_right } else { v_light_right }
    } else if l.up == None && l.down == None {
        v_light_right
    } else {
        v_light_left
    };
    let right_left = if l.up == Heavy || l.down == Heavy {
        v_heavy_left
    } else if l.up != l.down || l.right == l.left {
        if l.up == Double || l.down == Double { v_double_left } else { v_light_left }
    } else if l.up == None && l.down == None {
        v_light_left
    } else {
        v_light_right
    };

    match l.up {
        Weight::None => {}
        Weight::Light => out.push(rect(v_light_left, 0, v_light_right, up_bottom)),
        Weight::Heavy => out.push(rect(v_heavy_left, 0, v_heavy_right, up_bottom)),
        Weight::Double => {
            let left_bottom = if l.left == Double { h_light_top } else { up_bottom };
            let right_bottom = if l.right == Double { h_light_top } else { up_bottom };
            out.push(rect(v_double_left, 0, v_light_left, left_bottom));
            out.push(rect(v_light_right, 0, v_double_right, right_bottom));
        }
    }
    match l.right {
        Weight::None => {}
        Weight::Light => out.push(rect(right_left, h_light_top, w, h_light_bottom)),
        Weight::Heavy => out.push(rect(right_left, h_heavy_top, w, h_heavy_bottom)),
        Weight::Double => {
            let top_left = if l.up == Double { v_light_right } else { right_left };
            let bottom_left = if l.down == Double { v_light_right } else { right_left };
            out.push(rect(top_left, h_double_top, w, h_light_top));
            out.push(rect(bottom_left, h_light_bottom, w, h_double_bottom));
        }
    }
    match l.down {
        Weight::None => {}
        Weight::Light => out.push(rect(v_light_left, down_top, v_light_right, h)),
        Weight::Heavy => out.push(rect(v_heavy_left, down_top, v_heavy_right, h)),
        Weight::Double => {
            let left_top = if l.left == Double { h_light_bottom } else { down_top };
            let right_top = if l.right == Double { h_light_bottom } else { down_top };
            out.push(rect(v_double_left, left_top, v_light_left, h));
            out.push(rect(v_light_right, right_top, v_double_right, h));
        }
    }
    match l.left {
        Weight::None => {}
        Weight::Light => out.push(rect(0, h_light_top, left_right, h_light_bottom)),
        Weight::Heavy => out.push(rect(0, h_heavy_top, left_right, h_heavy_bottom)),
        Weight::Double => {
            let top_right = if l.up == Double { v_light_left } else { left_right };
            let bottom_right = if l.down == Double { v_light_left } else { left_right };
            out.push(rect(0, h_double_top, top_right, h_light_top));
            out.push(rect(0, h_light_bottom, bottom_right, h_double_bottom));
        }
    }
}

fn dash_rects(cell: Cell, vertical: bool, count: i32, heavy: bool, gap: DashGap, out: &mut Vec<Rect>) {
    let Cell { w, h, t, .. } = cell;
    let thick = if heavy { t * 2 } else { t };
    let desired_gap = match gap {
        DashGap::AtLeastFour => t.max(4),
        DashGap::Light => t,
        DashGap::Heavy => t * 2,
    };
    let (length, across) = if vertical { (h, w) } else { (w, h) };
    if length < count * 2 {
        let solid = match (vertical, heavy) {
            (true, true) => b"h.h.",
            (true, false) => b"l.l.",
            (false, true) => b".h.h",
            (false, false) => b".l.l",
        };
        lines_rects(cell, parse_lines(solid), out);
        return;
    }
    let gap = desired_gap.min(length / (2 * count));
    let dashes = length - count * gap;
    let (dash, mut extra) = (dashes / count, dashes % count);
    let offset = (across - thick).max(0) / 2;
    let mut start = if vertical { 0 } else { gap / 2 };
    for _ in 0..count {
        let mut end = start + dash;
        if extra > 0 {
            extra -= 1;
            end += 1;
        }
        out.push(if vertical {
            rect(offset, start, offset + thick, end)
        } else {
            rect(start, offset, end, offset + thick)
        });
        start = end + gap;
    }
}

fn rects(sprite: Sprite, cell: Cell) -> Option<Vec<Rect>> {
    let Cell { w, h, .. } = cell;
    let mut out = Vec::new();
    match sprite {
        Sprite::Lines(l) => lines_rects(cell, l, &mut out),
        Sprite::Dash { vertical, count, heavy, gap } => dash_rects(cell, vertical, count, heavy, gap, &mut out),
        Sprite::Block { w8, h8, right, bottom } => {
            let bw = (w as f32 * w8 as f32 / 8.).round() as i32;
            let bh = (h as f32 * h8 as f32 / 8.).round() as i32;
            let x = if right { w - bw } else { 0 };
            let y = if bottom { h - bh } else { 0 };
            out.push(rect(x, y, x + bw, y + bh));
        }
        Sprite::Shade(shade) => shade_rects(cell, shade, &mut out),
        Sprite::Quadrants(bits) => {
            // Rounded from opposite ends so both halves of an odd size get the middle pixel.
            let (mid_x0, mid_x1) = (w - (w as f32 / 2.).round() as i32, (w as f32 / 2.).round() as i32);
            let (mid_y0, mid_y1) = (h - (h as f32 / 2.).round() as i32, (h as f32 / 2.).round() as i32);
            let quarters = [
                rect(0, 0, mid_x1, mid_y1),
                rect(mid_x0, 0, w, mid_y1),
                rect(0, mid_y0, mid_x1, h),
                rect(mid_x0, mid_y0, w, h),
            ];
            out.extend((0..4).filter(|i| bits & (1 << i) != 0).map(|i| quarters[i]));
        }
        Sprite::Arc(_) | Sprite::Diagonal { .. } | Sprite::Powerline(_) => return None,
    }
    Some(out)
}

/// Paints `text` (see [`is_custom`]) into `cell`; does nothing if it is not a custom glyph.
/// `thickness_px` is the light line width in device pixels.
pub(crate) fn paint(window: &mut Window, cx: &App, text: &str, cell: Bounds<Pixels>, color: Hsla, thickness_px: u32) {
    let Some(sprite) = text.chars().next().and_then(sprite) else { return };
    let scale = window.scale_factor();
    // Rounded like gpui snaps bounds, so the shade phase matches where the mask lands.
    let to_device = |v: Pixels| {
        let d = f32::from(v) * scale;
        ((d.abs() - 0.5).ceil().copysign(d)) as i32
    };
    let device = Cell {
        x: to_device(cell.origin.x),
        y: to_device(cell.origin.y),
        w: (f32::from(cell.size.width) * scale).round() as i32,
        h: (f32::from(cell.size.height) * scale).round() as i32,
        t: thickness_px.max(1) as i32,
    };
    // One cached mask per cell: as rectangles, a shade cell is up to one quad per dot.
    if let Sprite::Shade(shade) = sprite {
        let (path, svg) = shade_svg(device, shade);
        let painted = window.paint_svg(cell, path, Some(&svg), TransformationMatrix::default(), color, cx);
        debug_assert!(painted.is_ok(), "shade mask not painted: {painted:?}");
        return;
    }
    if let Some(rects) = rects(sprite, device) {
        let o = cell.origin;
        for r in rects.into_iter().filter(|r| r.x1 > r.x0 && r.y1 > r.y0) {
            let bounds = Bounds::new(
                point(o.x + px(r.x0 as f32 / scale), o.y + px(r.y0 as f32 / scale)),
                size(px((r.x1 - r.x0) as f32 / scale), px((r.y1 - r.y0) as f32 / scale)),
            );
            window.paint_quad(fill(bounds, color));
        }
        return;
    }
    if let Some(path) = path(sprite, cell, device, scale) {
        window.paint_path(path, color);
    }
}

fn path(sprite: Sprite, cell: Bounds<Pixels>, device: Cell, scale: f32) -> Option<gpui::Path<Pixels>> {
    let o = cell.origin;
    let (w, h) = (cell.size.width, cell.size.height);
    let at = |(x, y): (f32, f32)| point(o.x + w * x, o.y + h * y);
    let dev = |x: f32, y: f32| point(o.x + px(x / scale), o.y + px(y / scale));
    let mut builder = match sprite {
        Sprite::Powerline(Shape::Fill(_) | Shape::HalfDisc(_)) => PathBuilder::fill(),
        _ => PathBuilder::stroke(px(device.t as f32 / scale)),
    };
    match sprite {
        Sprite::Powerline(shape @ (Shape::Fill(points) | Shape::Stroke(points))) => {
            builder.move_to(at(points[0]));
            for &p in &points[1..] {
                builder.line_to(at(p));
            }
            if matches!(shape, Shape::Fill(_)) {
                builder.close();
            }
        }
        Sprite::Powerline(shape @ (Shape::HalfDisc(facing) | Shape::HalfCircle(facing))) => {
            let right = facing == Facing::Right;
            let x = if right { 0. } else { 1. };
            builder.move_to(at((x, 0.)));
            builder.arc_to(point(w, h / 2.), px(0.), false, right, at((x, 1.)));
            if matches!(shape, Shape::HalfDisc(_)) {
                builder.close();
            }
        }
        Sprite::Arc(corner) => {
            let Cell { w, h, t, .. } = device;
            let (w, h, t) = (w as f32, h as f32, t as f32);
            let cx = ((w - t).max(0.) / 2.).floor() + t / 2.;
            let cy = ((h - t).max(0.) / 2.).floor() + t / 2.;
            let r = w.min(h) / 2.;
            let s = 0.25;
            let (edge_y, dy) = match corner {
                Corner::TopLeft | Corner::TopRight => (0., -1.),
                Corner::BottomLeft | Corner::BottomRight => (h, 1.),
            };
            let (edge_x, dx) = match corner {
                Corner::TopLeft | Corner::BottomLeft => (0., -1.),
                Corner::TopRight | Corner::BottomRight => (w, 1.),
            };
            builder.move_to(dev(cx, edge_y));
            builder.line_to(dev(cx, cy + dy * r));
            builder.cubic_bezier_to(dev(cx + dx * r, cy), dev(cx, cy + dy * s * r), dev(cx + dx * s * r, cy));
            builder.line_to(dev(edge_x, cy));
        }
        Sprite::Diagonal { rising, falling } => {
            let Cell { w, h, .. } = device;
            let (w, h) = (w as f32, h as f32);
            // Overshoot the corners along the slope so diagonals in adjacent cells join.
            let (sx, sy) = ((w / h).min(1.) * 0.5, (h / w).min(1.) * 0.5);
            if rising {
                builder.move_to(dev(w + sx, -sy));
                builder.line_to(dev(-sx, h + sy));
            }
            if falling {
                builder.move_to(dev(-sx, -sy));
                builder.line_to(dev(w + sx, h + sy));
            }
        }
        Sprite::Lines(_) | Sprite::Dash { .. } | Sprite::Block { .. } | Sprite::Shade(_) | Sprite::Quadrants(_) => {
            return None;
        }
    }
    builder.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELLS: [Cell; 4] = [
        Cell { x: 0, y: 0, w: 7, h: 15, t: 1 },
        Cell { x: 0, y: 0, w: 8, h: 16, t: 1 },
        Cell { x: 0, y: 0, w: 11, h: 22, t: 2 },
        Cell { x: 0, y: 0, w: 9, h: 17, t: 1 },
    ];

    fn all_sprites() -> impl Iterator<Item = (char, Sprite)> {
        ('\u{2500}'..='\u{259F}').chain('\u{E0B0}'..='\u{E0BF}').filter_map(|c| sprite(c).map(|s| (c, s)))
    }

    fn sprite_rects(c: char, cell: Cell) -> Vec<Rect> {
        rects(sprite(c).expect("custom glyph"), cell).expect("drawn with rectangles")
    }

    fn edge_spans(c: char, cell: Cell, edge: u8) -> Vec<(i32, i32)> {
        let mut spans: Vec<_> = sprite_rects(c, cell)
            .into_iter()
            .filter_map(|r| match edge {
                b'u' if r.y0 == 0 => Some((r.x0, r.x1)),
                b'd' if r.y1 == cell.h => Some((r.x0, r.x1)),
                b'l' if r.x0 == 0 => Some((r.y0, r.y1)),
                b'r' if r.x1 == cell.w => Some((r.y0, r.y1)),
                _ => None,
            })
            .collect();
        spans.sort_unstable_by_key(|s| s.0);
        spans
    }

    #[test]
    fn covers_box_drawing_and_block_elements() {
        assert!(('\u{2500}'..='\u{259F}').all(|c| is_custom(&c.to_string())), "every box and block char");
        assert!(is_custom("\u{E0B0}"));
        assert!(!is_custom("\u{E0A0}"));
        assert!(!is_custom("a"));
        assert!(!is_custom("─a"));
    }

    #[test]
    fn lines_meet_their_neighbors_in_every_cell_size() {
        let straight = |weight: Weight, horizontal: bool| match (weight, horizontal) {
            (Weight::None, _) => None,
            (Weight::Light, true) => Some('─'),
            (Weight::Light, false) => Some('│'),
            (Weight::Heavy, true) => Some('━'),
            (Weight::Heavy, false) => Some('┃'),
            (Weight::Double, true) => Some('═'),
            (Weight::Double, false) => Some('║'),
        };
        for cell in CELLS {
            for (c, s) in all_sprites() {
                let Sprite::Lines(l) = s else { continue };
                for (weight, edge, horizontal) in
                    [(l.up, b'u', false), (l.down, b'd', false), (l.left, b'l', true), (l.right, b'r', true)]
                {
                    let Some(straight) = straight(weight, horizontal) else {
                        assert!(edge_spans(c, cell, edge).is_empty(), "{c} draws nothing at edge {}", edge as char);
                        continue;
                    };
                    assert_eq!(
                        edge_spans(c, cell, edge),
                        edge_spans(straight, cell, edge),
                        "{c} meets {straight} at edge {} in {cell:?}",
                        edge as char
                    );
                }
            }
        }
    }

    #[test]
    fn rectangles_stay_inside_the_cell() {
        for cell in CELLS {
            for (c, s) in all_sprites() {
                for r in rects(s, cell).unwrap_or_default() {
                    assert!(r.x0 >= 0 && r.y0 >= 0 && r.x1 <= cell.w && r.y1 <= cell.h, "{c} {r:?} in {cell:?}");
                }
            }
        }
    }

    #[test]
    fn full_and_half_blocks_tile_without_gaps() {
        for cell in CELLS {
            assert_eq!(sprite_rects('█', cell), [rect(0, 0, cell.w, cell.h)]);
            let upper = sprite_rects('▀', cell)[0];
            let lower = sprite_rects('▄', cell)[0];
            assert_eq!((upper.y0, lower.y1), (0, cell.h));
            assert!(upper.y1 >= lower.y0, "▀ and ▄ cover the cell together in {cell:?}");
            let quarters: i32 = sprite_rects('▚', cell)
                .iter()
                .chain(&sprite_rects('▞', cell))
                .map(|r| (r.x1 - r.x0) * (r.y1 - r.y0))
                .sum();
            assert!(quarters >= cell.w * cell.h, "▚ and ▞ cover the cell together in {cell:?}");
        }
    }

    #[test]
    fn cached_shade_phases_draw_the_same_pixels() {
        for shade in [Shade::Light, Shade::Medium, Shade::Dark] {
            for (w, h) in [(7, 15), (24, 48), (36, 30)] {
                for (x, y) in [(0, 0), (5, 3), (130, 77), (-9, 41), (311, 290)] {
                    let cell = Cell { x, y, w, h, t: 1 };
                    let (px, py) = shade_phase(cell, shade);
                    let (mut real, mut cached) = (Vec::new(), Vec::new());
                    shade_rects(cell, shade, &mut real);
                    shade_rects(Cell { x: px, y: py, ..cell }, shade, &mut cached);
                    assert_eq!(real, cached, "{shade:?} at ({x}, {y}) in {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn a_line_spans_the_rect_it_is_given() {
        let narrow = Cell { x: 0, y: 0, w: 7, h: 15, t: 1 };
        let wide = Cell { w: 14, ..narrow };
        assert_eq!(edge_spans('─', narrow, b'r'), edge_spans('─', wide, b'r'));
        assert_eq!(sprite_rects('─', narrow).iter().map(|r| r.x1).max(), Some(7));
        assert_eq!(
            sprite_rects('─', wide).iter().map(|r| r.x1).max(),
            Some(14),
            "wide ─ reaches the second cell's edge"
        );
    }

    #[test]
    fn dashes_too_small_for_gaps_keep_their_weight() {
        let cell = Cell { x: 0, y: 0, w: 7, h: 15, t: 1 };
        let thickness = |c| {
            let rects = sprite_rects(c, cell);
            let covered = (0..cell.w).all(|x| rects.iter().any(|r| (r.x0..r.x1).contains(&x)));
            assert!(covered, "{c} is a solid line across the cell");
            rects[0].y1 - rects[0].y0
        };
        assert_eq!(thickness('┉'), 2);
        assert_eq!(thickness('┈'), 1);

        let short = Cell { x: 0, y: 0, w: 7, h: 7, t: 1 };
        let width = |c| {
            let rects = sprite_rects(c, short);
            let covered = (0..short.h).all(|y| rects.iter().any(|r| (r.y0..r.y1).contains(&y)));
            assert!(covered, "{c} is a solid line down the cell");
            rects[0].x1 - rects[0].x0
        };
        assert_eq!(width('┋'), 2);
        assert_eq!(width('┊'), 1);
    }

    #[test]
    fn shade_masks_hold_the_cell_pattern() {
        let cell = Cell { x: 0, y: 0, w: 12, h: 24, t: 1 };
        let (path, svg) = shade_svg(cell, Shade::Medium);
        let svg = std::str::from_utf8(&svg).expect("utf-8");
        assert!(svg.contains(r#"width="12" height="24" viewBox="0 0 12 24""#), "{svg}");
        let mut rects = Vec::new();
        shade_rects(cell, Shade::Medium, &mut rects);
        assert_eq!(svg.matches('M').count(), rects.len(), "one subpath per run");
        assert!(svg.starts_with("<svg") && svg.contains("M0 0h1v1h-1z"), "{svg}");
        let (shifted, _) = shade_svg(Cell { x: 1, ..cell }, Shade::Medium);
        let (same, _) = shade_svg(Cell { x: 3, ..cell }, Shade::Medium);
        assert_ne!(path, shifted, "another phase is another mask");
        assert_eq!(path, same, "a whole period later is the same mask");
    }

    #[test]
    fn dashes_are_evenly_spaced_within_the_cell() {
        let cell = Cell { x: 0, y: 0, w: 12, h: 24, t: 1 };
        let dashes = sprite_rects('┈', cell);
        assert_eq!(dashes.len(), 4);
        assert!(dashes.windows(2).all(|p| p[1].x0 - p[0].x1 == dashes[1].x0 - dashes[0].x1));
        assert!(dashes[3].x1 <= cell.w);
    }

    /// Lit device pixels of `c` drawn in `cell`, in screen coordinates.
    fn lit_pixels(c: char, cell: Cell) -> std::collections::BTreeSet<(i32, i32)> {
        sprite_rects(c, cell)
            .into_iter()
            .flat_map(|r| (r.y0..r.y1).flat_map(move |y| (r.x0..r.x1).map(move |x| (cell.x + x, cell.y + y))))
            .collect()
    }

    #[test]
    fn shades_are_falling_hatches_closer_for_darker_shades() {
        let cell = Cell { x: 0, y: 0, w: 12, h: 24, t: 1 };
        let area = (cell.w * cell.h) as usize;
        assert_eq!(lit_pixels('░', cell).len() * 6, area);
        assert_eq!(lit_pixels('▒', cell).len() * 3, area);
        assert_eq!(lit_pixels('▓', cell).len() * 2, area);
        let medium = lit_pixels('▒', cell);
        assert!(medium.contains(&(0, 0)) && medium.contains(&(1, 1)) && !medium.contains(&(1, 0)), "1px \\ lines");
    }

    #[test]
    fn shade_patterns_continue_across_cells() {
        for c in ['░', '▒', '▓'] {
            // Pairs whose union has the same dot size as each half (1, 1, 2 and 3 pixels).
            for (w, h, beside) in [(7, 15, true), (5, 11, true), (24, 20, false), (36, 30, false)] {
                let first = Cell { x: 5, y: 3, w, h, t: 1 };
                let (second, union) = if beside {
                    (Cell { x: 5 + w, ..first }, Cell { w: w * 2, ..first })
                } else {
                    (Cell { y: 3 + h, ..first }, Cell { h: h * 2, ..first })
                };
                let mut joined = lit_pixels(c, first);
                joined.extend(lit_pixels(c, second));
                assert_eq!(joined, lit_pixels(c, union), "{c} continues across cells of {w}x{h}");
            }
        }
    }
}
