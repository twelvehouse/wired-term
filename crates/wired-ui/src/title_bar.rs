//! Custom title bar: tabs, new-tab and profile split button, drag area, reload and caption buttons.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Div, ElementId, Entity, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Stateful, StatefulInteractiveElement, StyleRefinement, Styled, Window,
    WindowControlArea, canvas,
    component::{
        Icon, IconName, Sizable,
        input::{Input, InputState},
    },
    deferred, div, fill, hsla, point, px, size,
};

pub const HEIGHT: f32 = 36.;
pub(crate) const UI_FONT: &str = "Segoe UI";
const CAPTION_FONT: &str = "Segoe Fluent Icons";
const GLYPH_MINIMIZE: &str = "\u{E921}";
const GLYPH_MAXIMIZE: &str = "\u{E922}";
const GLYPH_RESTORE: &str = "\u{E923}";
const GLYPH_CLOSE: &str = "\u{E8BB}";
const GLYPH_EXIT_FULLSCREEN: &str = "\u{E73F}";
const CAPTION_WIDTH: f32 = 46.;

/// How the window is shown, which decides the buttons at the right end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Windowed {
        is_maximized: bool,
    },
    /// The caption buttons do nothing without a frame; one button leaves fullscreen instead.
    Fullscreen,
}

fn white(alpha: f32) -> Hsla {
    hsla(0., 0., 1., alpha)
}

/// Colors of the title bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarColors {
    /// Every segment except the active tab; also the lines between panes.
    pub bar: Hsla,
    /// Minimize and maximize under the pointer (opaque, so the backdrop does not show through).
    pub caption_hover: Hsla,
}

impl BarColors {
    /// `bar`, with a hover color a little lighter (darker on a light bar).
    pub fn new(bar: Hsla) -> Self {
        let step = if bar.l > 0.5 { -0.07 } else { 0.07 };
        Self { bar, caption_hover: Hsla { l: (bar.l + step).clamp(0., 1.), ..bar } }
    }
}

const ICON_REST: f32 = 0.55;
const ICON_HOVER: f32 = 0.95;
const ICON_PRESSED: f32 = 0.4;

/// Workaround for gpui-pre 0.3.12: hover state is not cleared when the cursor leaves the window
/// straight from an element, so hover styles are attached only while the window is hovered.
fn hover_if(
    element: Stateful<Div>,
    is_window_hovered: bool,
    style: impl Fn(StyleRefinement) -> StyleRefinement + 'static,
) -> Stateful<Div> {
    if is_window_hovered { element.hover(style) } else { element }
}

fn caption_button(
    id: &'static str,
    glyph: &'static str,
    area: WindowControlArea,
    (bar, hover): (Hsla, Hsla),
    is_window_hovered: bool,
) -> Stateful<Div> {
    let button = div()
        .id(id)
        .w(px(CAPTION_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .font_family(CAPTION_FONT)
        .text_size(px(10.))
        .text_color(white(0.9))
        .bg(bar)
        .window_control_area(area)
        .child(glyph);
    hover_if(button, is_window_hovered, move |s| s.bg(hover))
}

fn exit_fullscreen_button(bar: Hsla, is_window_hovered: bool) -> Stateful<Div> {
    let button = div()
        .id("exit-fullscreen")
        .w(px(CAPTION_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .font_family(CAPTION_FONT)
        .text_size(px(10.))
        .bg(bar)
        .text_color(white(ICON_REST))
        .active(|s| s.text_color(white(ICON_PRESSED)))
        .child(GLYPH_EXIT_FULLSCREEN)
        .on_click(|_, window, _| window.toggle_fullscreen());
    hover_if(button, is_window_hovered, |s| s.text_color(white(ICON_HOVER)))
}

fn window_buttons(state: WindowState, colors: BarColors, is_window_hovered: bool) -> Vec<Stateful<Div>> {
    let is_maximized = match state {
        WindowState::Fullscreen => return vec![exit_fullscreen_button(colors.bar, is_window_hovered)],
        WindowState::Windowed { is_maximized } => is_maximized,
    };
    let maximize_glyph = if is_maximized { GLYPH_RESTORE } else { GLYPH_MAXIMIZE };
    let close_hover = hsla(355. / 360., 0.86, 0.47, 1.0);
    let hover = (colors.bar, colors.caption_hover);
    vec![
        caption_button("min", GLYPH_MINIMIZE, WindowControlArea::Min, hover, is_window_hovered),
        caption_button("max", maximize_glyph, WindowControlArea::Max, hover, is_window_hovered),
        caption_button("close", GLYPH_CLOSE, WindowControlArea::Close, (colors.bar, close_hover), is_window_hovered),
    ]
}

fn icon_button(
    id: &'static str,
    icon: IconName,
    (width, icon_size): (f32, f32),
    bar: Hsla,
    is_window_hovered: bool,
) -> Stateful<Div> {
    let button = div()
        .id(id)
        .w(px(width))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(bar)
        .text_color(white(ICON_REST))
        .active(|s| s.text_color(white(ICON_PRESSED)))
        .child(Icon::new(icon).size(px(icon_size)));
    hover_if(button, is_window_hovered, |s| s.text_color(white(ICON_HOVER)))
}

/// What a tab shows as its title.
pub enum TabTitle {
    Text(String),
    Editing(Entity<InputState>),
}

/// Where the tab strip and its tabs were drawn last, in window coordinates before tab offsets.
#[derive(Debug, Default)]
pub struct StripMetrics {
    pub left: f32,
    pub top: f32,
    /// Window x the tabs and the buttons after them may not pass.
    pub limit_right: f32,
    /// Width of every tab by id, closing ones included while they still take room.
    pub widths: HashMap<u64, f32>,
    /// Window x range the active tab takes this frame, offset included.
    pub active_span: Option<(f32, f32)>,
}

/// [`StripMetrics`] shared with the drawing code that fills it in.
pub type SharedMetrics = Rc<RefCell<StripMetrics>>;

/// What a tab shows.
pub struct TabLook {
    /// Stable id of the tab (element id).
    pub id: u64,
    /// Shortcut number (1..9, 0) shown before the icon; `None` hides it.
    pub number: Option<u8>,
    /// Icon of the focused pane's profile.
    pub icon: Option<wired_config::ProfileIcon>,
    pub title: TabTitle,
    /// The active tab merges with the terminal (no shade, no hover).
    pub is_active: bool,
    /// Progress of the opening animation, eased, 0..=1 (1 = settled).
    pub appear: f32,
    /// How far the number is shown, eased, 0..=1 (it fades and grows in and out).
    pub number_reveal: f32,
    /// How far right of its place in the strip the tab is drawn.
    pub offset: f32,
    /// Held by the pointer, or settling after it let go: drawn above its neighbors.
    pub is_lifted: bool,
    /// Where to record this tab's drawn width.
    pub metrics: SharedMetrics,
}

/// The tabs of the strip and what is drawn with them.
pub struct TabStrip {
    pub tabs: Vec<(TabLook, TabHandlers)>,
    /// Room after the last tab for a tab held past it.
    pub tail: f32,
    /// Where to record the strip's right limit.
    pub metrics: SharedMetrics,
    /// Dims the active tab, the only translucent part of the bar.
    pub dim: Option<Div>,
}

/// Full width of a tab.
pub const TAB_WIDTH: f32 = 200.;
const TAB_MIN_WIDTH: f32 = 80.;
const NUMBER_WIDTH: f32 = 16.;
const APPEAR_START_OPACITY: f32 = 0.3;

/// Called with a mouse event over a tab.
pub type MouseHandler<E> = Box<dyn Fn(&E, &mut Window, &mut App)>;

/// Mouse handlers of one tab.
pub struct TabHandlers {
    /// Left button pressed on the tab.
    pub on_press: MouseHandler<MouseDownEvent>,
    /// Closes the tab: middle button released (when `middle_closes`) or the × clicked.
    pub on_close: Callback,
    /// Whether a middle-click closes the tab.
    pub middle_closes: bool,
}

/// Called without an event.
pub type Callback = Box<dyn Fn(&mut Window, &mut App)>;

fn rename_field(input: &Entity<InputState>) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .rounded(px(3.))
        .bg(white(0.1))
        .text_color(white(0.92))
        .child(Input::new(input).xsmall().appearance(false))
        .into_any_element()
}

fn tab(
    look: TabLook,
    handlers: TabHandlers,
    background: Hsla,
    is_window_hovered: bool,
    dim: Option<Div>,
) -> AnyElement {
    let on_close = Rc::new(handlers.on_close);
    let close_by_middle = Rc::clone(&on_close);
    let middle_closes = handlers.middle_closes;
    let close_button = div()
        .id(ElementId::Name(format!("tab-close-{}", look.id).into()))
        .flex_none()
        .w(px(HEIGHT))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(white(ICON_REST))
        .active(|s| s.text_color(white(ICON_PRESSED)))
        .child(Icon::new(IconName::Close).size(px(12.)))
        // The tab below must not see this press as the start of a click or drag on it.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Only a press that began on this ×, so releasing a drag over it closes nothing.
        .on_click(move |_, window, cx| on_close(window, cx));
    let close_button = hover_if(close_button, is_window_hovered, |s| s.text_color(white(ICON_HOVER)));
    // The number has no flex gap of its own, so it can grow from zero width without a jump.
    let number = look.number.filter(|_| look.number_reveal > 0.).map(|n| {
        div()
            .flex_none()
            .w(px(NUMBER_WIDTH * look.number_reveal))
            .overflow_hidden()
            .whitespace_nowrap()
            .opacity(look.number_reveal)
            .text_color(white(0.35))
            .child(n.to_string())
    });
    let title = match look.title {
        TabTitle::Text(text) => {
            div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text).into_any_element()
        }
        TabTitle::Editing(input) => rename_field(&input),
    };
    let is_opening = look.appear < 1.;
    // While opening, the content keeps the full tab width so the title does not squeeze as the slot widens.
    let content = div()
        .relative()
        .top(px(HEIGHT * (1. - look.appear)))
        .opacity(APPEAR_START_OPACITY + (1. - APPEAR_START_OPACITY) * look.appear)
        .h_full()
        .flex_none()
        // No right padding: the close button's square hit area reaches the tab's right edge.
        .pl(px(12.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().flex().items_center().children(number).child(crate::profile_icon::element(look.icon, 14.)))
        .child(title)
        .child(close_button);
    let content = if is_opening { content.w(px(TAB_WIDTH)) } else { content.w_full() };
    let id = look.id;
    let is_active = look.is_active;
    let metrics = look.metrics;
    let measure = canvas(
        move |bounds, _, _| {
            let mut metrics = metrics.borrow_mut();
            metrics.widths.insert(id, f32::from(bounds.size.width));
            if is_active {
                metrics.active_span = Some((f32::from(bounds.left()), f32::from(bounds.right())));
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    let element = div()
        .id(ElementId::Name(format!("tab-{}", look.id).into()))
        .relative()
        .left(px(look.offset))
        .w(px(TAB_WIDTH * look.appear))
        .min_w(px(TAB_MIN_WIDTH * look.appear))
        .flex_shrink(1.)
        .h_full()
        .overflow_hidden()
        .text_size(px(12.))
        .on_mouse_down(MouseButton::Left, handlers.on_press)
        .on_mouse_up(MouseButton::Middle, move |_, window, cx| {
            if middle_closes {
                close_by_middle(window, cx);
            }
        })
        .child(content)
        .child(measure)
        .children(dim);
    // Inactive tabs have no background: the strip's background paints the bar color and must not
    // be covered where the active tab passes under or over them.
    let element = if look.is_active {
        element.bg(background).text_color(white(0.92))
    } else {
        hover_if(element.text_color(white(0.7)), is_window_hovered, |s| s.text_color(white(0.92)))
    };
    // Drawn after the tabs next to it, so it passes over them. It cannot leave the strip, so it
    // needs no clipping.
    if look.is_lifted { deferred(element).priority(1).into_any_element() } else { element.into_any_element() }
}

fn strip_background(metrics: SharedMetrics, bar: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (left, right) = (f32::from(bounds.left()), f32::from(bounds.right()));
            let (hole_left, hole_right) = metrics
                .borrow()
                .active_span
                .map_or((right, right), |(l, r)| (l.clamp(left, right), r.clamp(left, right)));
            for (from, to) in [(left, hole_left), (hole_right, right)] {
                if to > from {
                    let part = Bounds::new(point(px(from), bounds.top()), size(px(to - from), bounds.size.height));
                    window.paint_quad(fill(part, bar));
                }
            }
        },
    )
    .absolute()
    .size_full()
}

const NEW_TAB_SIZE: (f32, f32) = (30., 16.);
const PROFILES_SIZE: (f32, f32) = (18., 12.);
const STRIP_BUTTONS_WIDTH: f32 = NEW_TAB_SIZE.0 + PROFILES_SIZE.0;

fn profiles_button(bar: Hsla, is_window_hovered: bool) -> Stateful<Div> {
    let divider = div().absolute().left_0().top(px(10.)).w(px(1.)).h(px(16.)).bg(white(0.08));
    icon_button("profiles", IconName::ChevronDown, PROFILES_SIZE, bar, is_window_hovered).relative().child(divider)
}

/// What the buttons next to the tabs and at the right end do.
pub struct BarButtons {
    /// `+`: a tab with the default profile.
    pub on_new_tab: MouseHandler<ClickEvent>,
    /// The arrow next to `+`: the profile picker.
    pub on_profiles: MouseHandler<ClickEvent>,
    /// The reload button, which reads the settings files again; `None` hides it.
    pub on_reload: Option<MouseHandler<ClickEvent>>,
}

/// Builds the title bar; `tracker` fills the strip, measuring it and following the pointer during tab drags.
pub fn title_bar(
    strip: TabStrip,
    tracker: impl IntoElement,
    background: Hsla,
    colors: BarColors,
    state: WindowState,
    is_window_hovered: bool,
    buttons: BarButtons,
) -> AnyElement {
    let BarButtons { on_new_tab, on_profiles, on_reload } = buttons;
    let bar = colors.bar;
    // The active tab records its span again in this frame's prepaint, which runs before any paint.
    strip.metrics.borrow_mut().active_span = None;
    let strip_background = strip_background(Rc::clone(&strip.metrics), bar);
    let metrics = strip.metrics;
    let mut dim = strip.dim;
    let measure_limit = canvas(
        move |bounds, _, _| metrics.borrow_mut().limit_right = f32::from(bounds.right()) - STRIP_BUTTONS_WIDTH,
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    div()
        .h(px(HEIGHT))
        .w_full()
        .flex()
        .flex_row()
        .flex_none()
        .font_family(UI_FONT)
        .child(
            div()
                .h_full()
                .flex()
                .flex_row()
                .flex_shrink(1.)
                .min_w_0()
                .overflow_hidden()
                .relative()
                .child(strip_background)
                .children(strip.tabs.into_iter().map(|(look, handlers)| {
                    let dim = if look.is_active { dim.take() } else { None };
                    tab(look, handlers, background, is_window_hovered, dim)
                }))
                .children((strip.tail > 0.).then(|| div().flex_none().h_full().w(px(strip.tail))))
                .child(tracker),
        )
        .child(icon_button("new-tab", IconName::Plus, NEW_TAB_SIZE, bar, is_window_hovered).on_click(on_new_tab))
        .child(profiles_button(bar, is_window_hovered).on_click(on_profiles))
        // Empty space: OS drag area (double-click maximizes, Aero Snap).
        .child(
            div()
                .relative()
                .flex_1()
                .h_full()
                .bg(bar)
                .window_control_area(WindowControlArea::Drag)
                .child(measure_limit),
        )
        .children(on_reload.map(|on_reload| {
            icon_button("reload", IconName::RefreshCw, (HEIGHT, 16.), bar, is_window_hovered).on_click(on_reload)
        }))
        .children(window_buttons(state, colors, is_window_hovered))
        .into_any_element()
}
