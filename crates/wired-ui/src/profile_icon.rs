//! Icons of the built-in profiles (Tabby's icon set, MIT), compiled into the executable.
//!
//! Each icon is rasterized from its SVG at the exact device-pixel size it is shown at and drawn
//! 1:1, as a browser does. Scaling a larger rendering down on the GPU loses the thin details.

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use gpui::{
    AnyElement, App, Bounds, Corners, DevicePixels, IntoElement, ParentElement, RenderImage, Size, Styled, SvgSize,
    canvas,
    component::{Icon, IconName},
    div, point, px,
};
use wired_config::ProfileIcon;

fn svg(icon: ProfileIcon) -> &'static [u8] {
    macro_rules! files {
        ($($variant:ident => $file:literal),* $(,)?) => {
            match icon {
                $(ProfileIcon::$variant => include_bytes!(concat!("../assets/profiles/", $file)),)*
            }
        };
    }
    files! {
        PowerShell => "powershell.svg",
        PowerShellCore => "powershell-core.svg",
        Cmd => "cmd.svg",
        Clink => "clink.svg",
        GitBash => "git-bash.svg",
        Linux => "linux.svg",
        Alma => "alma.svg",
        Alpine => "alpine.svg",
        Debian => "debian.svg",
        Kali => "kali.svg",
        Suse => "suse.svg",
        Ubuntu => "ubuntu.svg",
        OracleLinux => "oracle-linux.svg",
        OpenEuler => "open-euler.svg",
    }
}

fn view_box(svg: &[u8]) -> Option<(f32, f32)> {
    let text = std::str::from_utf8(svg).ok()?;
    let root = &text[text.find("<svg")?..];
    let root = &root[..root.find('>')?];
    let value = root.split("viewBox=\"").nth(1)?.split('"').next()?;
    let numbers: Vec<f32> = value.split([' ', ',']).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
    match numbers[..] {
        [_, _, width, height] if width > 0. && height > 0. => Some((width, height)),
        _ => None,
    }
}

fn fitted(side: i32, (width, height): (f32, f32)) -> (i32, i32) {
    let side_f = side as f32;
    let (w, h) = if width >= height { (side_f, side_f * height / width) } else { (side_f * width / height, side_f) };
    ((w.round() as i32).clamp(1, side), (h.round() as i32).clamp(1, side))
}

type Rendered = HashMap<(ProfileIcon, i32), Option<Arc<RenderImage>>>;

thread_local! {
    static RENDERED: RefCell<Rendered> = RefCell::default();
}

fn rendered(icon: ProfileIcon, side: i32, cx: &App) -> Option<Arc<RenderImage>> {
    RENDERED.with_borrow_mut(|cache| {
        cache
            .entry((icon, side))
            .or_insert_with(|| {
                let bytes = svg(icon);
                let (width, height) = fitted(side, view_box(bytes).unwrap_or((1., 1.)));
                let renderer = cx.svg_renderer();
                let parsed = renderer.parse_svg(bytes).ok()?;
                let size = Size { width: DevicePixels(width), height: DevicePixels(height) };
                renderer.render_parsed(&parsed, SvgSize::ExactSize(size)).ok()
            })
            .clone()
    })
}

/// `icon` in a `size` logical-pixel square; a generic terminal icon when there is none.
pub(crate) fn element(icon: Option<ProfileIcon>, size: f32) -> AnyElement {
    let Some(icon) = icon else {
        return div().flex_none().child(Icon::new(IconName::SquareTerminal).size(px(size))).into_any_element();
    };
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            let scale = window.scale_factor();
            let side = (size * scale).round() as i32;
            let Some(image) = rendered(icon, side.max(1), cx) else { return };
            let pixels = image.size(0);
            let (width, height) = (i32::from(pixels.width) as f32 / scale, i32::from(pixels.height) as f32 / scale);
            let origin = bounds.origin + point(px((size - width) / 2.), px((size - height) / 2.));
            let image_bounds = Bounds::new(origin, gpui::size(px(width), px(height)));
            let _ = window.paint_image(image_bounds, image_bounds, Corners::default(), image, 0, false);
        },
    )
    .size(px(size))
    .flex_none()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_root_view_box() {
        assert_eq!(view_box(br#"<svg viewBox="0 0 120 90.5" width="300"><path/></svg>"#), Some((120., 90.5)));
        assert_eq!(view_box(br#"<?xml?><svg width="3"><g viewBox="0 0 1 1"/></svg>"#), None);
    }

    #[test]
    fn every_icon_has_a_view_box() {
        for icon in [ProfileIcon::PowerShell, ProfileIcon::Cmd, ProfileIcon::Linux, ProfileIcon::OracleLinux] {
            assert!(view_box(svg(icon)).is_some(), "{icon:?}");
        }
    }

    #[test]
    fn fits_the_long_side_and_keeps_the_aspect() {
        assert_eq!(fitted(24, (120., 90.)), (24, 18));
        assert_eq!(fitted(24, (448., 512.)), (21, 24));
        assert_eq!(fitted(16, (10., 10.)), (16, 16));
    }
}
