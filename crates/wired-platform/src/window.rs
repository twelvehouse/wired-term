//! Top-level windows of the app: where they are, which one is under the pointer, and moving them
//! while a tab is dragged. Points are physical screen pixels.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use wired_config::WindowIcon;

/// A top-level window of this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(isize);

/// The id of `window`, if it is a native window of this platform.
pub fn window_id(window: &impl HasWindowHandle) -> Option<WindowId> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(WindowId(handle.hwnd.get())),
        _ => None,
    }
}

#[cfg(windows)]
mod imp {
    use std::{cell::RefCell, collections::HashMap, ffi::c_void, ptr::NonNull};

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT},
        Graphics::{
            Dwm::{
                DWMSBT_TRANSIENTWINDOW, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE,
                DwmGetWindowAttribute, DwmSetWindowAttribute,
            },
            Gdi::{ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint},
        },
        System::{Diagnostics::Debug::MessageBeep, LibraryLoader::GetModuleHandleW, Threading::GetCurrentProcessId},
        UI::{
            HiDpi::{GetDpiForMonitor, GetDpiForWindow, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::SetCapture,
            WindowsAndMessaging::{
                EnumWindows, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GWL_EXSTYLE, GetCursorPos,
                GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, HWND_NOTOPMOST,
                HWND_TOPMOST, ICON_BIG, ICON_SMALL, IMAGE_ICON, IsIconic, IsWindowVisible, IsZoomed, LoadImageW, MB_OK,
                SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SW_RESTORE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                SWP_NOZORDER, SendMessageW, SetWindowPos, ShowWindow, WM_SETICON, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            },
        },
    };

    use super::{Monitor, WindowIcon, WindowId};

    fn hwnd(id: WindowId) -> HWND {
        id.0 as HWND
    }

    thread_local! {
        // Handles stay installed on each HWND, so keep one per resource and size. LR_SHARED can
        // reuse an image cached at the wrong size.
        static ICONS: RefCell<HashMap<(u16, i32, i32), NonNull<c_void>>> = RefCell::default();
    }

    pub(super) fn set_icon(id: WindowId, icon: WindowIcon) {
        const DARK_ICON_ID: u16 = 1;
        const LIGHT_ICON_ID: u16 = 2;
        let resource = match icon {
            WindowIcon::Dark => DARK_ICON_ID,
            WindowIcon::Light => LIGHT_ICON_ID,
        };
        let name = std::ptr::without_provenance(usize::from(resource));
        let window = hwnd(id);
        // SAFETY: a null name requests the current executable. An invalid HWND returns zero DPI.
        let (module, dpi) = unsafe { (GetModuleHandleW(std::ptr::null()), GetDpiForWindow(window)) };
        if module.is_null() || dpi == 0 {
            return;
        }
        for (kind, cx, cy) in [(ICON_SMALL, SM_CXSMICON, SM_CYSMICON), (ICON_BIG, SM_CXICON, SM_CYICON)] {
            // SAFETY: both metrics accept the window's DPI and return values by value.
            let (width, height) = unsafe { (GetSystemMetricsForDpi(cx, dpi), GetSystemMetricsForDpi(cy, dpi)) };
            let handle = ICONS.with_borrow_mut(|icons| {
                let key = (resource, width, height);
                if let Some(&handle) = icons.get(&key) {
                    return Some(handle);
                }
                // SAFETY: the executable stays loaded and Win32 accepts the resource ID in the
                // low word of the name pointer (MAKEINTRESOURCEW).
                let handle = NonNull::new(unsafe { LoadImageW(module, name, IMAGE_ICON, width, height, 0) })?;
                icons.insert(key, handle);
                Some(handle)
            });
            if let Some(handle) = handle {
                // SAFETY: the app owns the window and retains the icon for the lifetime of its HWND.
                unsafe { SendMessageW(window, WM_SETICON, kind as usize, handle.as_ptr() as LPARAM) };
            }
        }
    }

    pub(super) fn monitor_at(x: i32, y: i32) -> Option<Monitor> {
        // SAFETY: plain call with a point by value; NEAREST never returns a null monitor for a
        // valid desktop.
        let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return None;
        }
        // SAFETY: MONITORINFO is plain data; all zeros is a valid value.
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        // SAFETY: `monitor` came from MonitorFromPoint; `info` is valid for writes.
        if unsafe { GetMonitorInfoW(monitor, &raw mut info) } == 0 {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        // SAFETY: both out pointers are valid; a failure leaves the 96 DPI defaults.
        let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) };
        let work = info.rcWork;
        Some(Monitor {
            id: monitor as usize as u64,
            scale: dpi_x.max(1) as f32 / 96.,
            work_area: (work.right - work.left, work.bottom - work.top),
        })
    }

    pub(super) fn cursor_position() -> Option<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: `point` is a valid out pointer for the call.
        (unsafe { GetCursorPos(&raw mut point) } != 0).then_some((point.x, point.y))
    }

    pub(super) fn client_origin(id: WindowId) -> Option<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: `point` is a valid in/out pointer; an invalid handle only makes the call fail.
        (unsafe { ClientToScreen(hwnd(id), &raw mut point) } != 0).then_some((point.x, point.y))
    }

    pub(super) fn move_client_origin(id: WindowId, x: i32, y: i32) {
        let Some((client_x, client_y)) = client_origin(id) else { return };
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: `rect` is a valid out pointer; an invalid handle only makes the calls fail.
        unsafe {
            if GetWindowRect(hwnd(id), &raw mut rect) != 0 {
                let (left, top) = (rect.left + x - client_x, rect.top + y - client_y);
                SetWindowPos(
                    hwnd(id),
                    std::ptr::null_mut(),
                    left,
                    top,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }

    pub(super) fn restore(id: WindowId) {
        // SAFETY: plain window calls; an invalid handle only makes them fail.
        unsafe {
            if IsZoomed(hwnd(id)) != 0 {
                ShowWindow(hwnd(id), SW_RESTORE);
            }
        }
    }

    pub(super) fn set_topmost(id: WindowId, is_topmost: bool) {
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        // SAFETY: plain window calls; an invalid handle only makes them fail.
        unsafe {
            if is_topmost {
                SetWindowPos(hwnd(id), HWND_TOPMOST, 0, 0, 0, 0, flags);
                return;
            }
            // HWND_NOTOPMOST alone would lift the window above the one the user switched to.
            let foreground = GetForegroundWindow();
            if !foreground.is_null() && foreground != hwnd(id) {
                SetWindowPos(hwnd(id), foreground, 0, 0, 0, 0, flags);
            }
            if GetWindowLongPtrW(hwnd(id), GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 {
                SetWindowPos(hwnd(id), HWND_NOTOPMOST, 0, 0, 0, 0, flags);
            }
        }
    }

    pub(super) fn beep() {
        // SAFETY: plain call; it queues the sound and returns at once.
        let _ = unsafe { MessageBeep(MB_OK) };
    }

    pub(super) fn flash_taskbar(id: WindowId) {
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd: hwnd(id),
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        // SAFETY: `info` is a valid in pointer with its size set; an invalid handle only makes the
        // call fail.
        let _ = unsafe { FlashWindowEx(&raw const info) };
    }

    pub(super) fn capture_pointer(id: WindowId) {
        // SAFETY: the window belongs to this thread (every window of the app does); an invalid
        // handle only makes the call fail.
        unsafe { SetCapture(hwnd(id)) };
    }

    pub(super) fn use_system_acrylic(id: WindowId) {
        let kind = DWMSBT_TRANSIENTWINDOW;
        // SAFETY: `kind` is a valid in buffer of the size given; an invalid handle or an older
        // Windows without the attribute only makes the call fail.
        unsafe {
            DwmSetWindowAttribute(
                hwnd(id),
                DWMWA_SYSTEMBACKDROP_TYPE as u32,
                (&raw const kind).cast::<c_void>(),
                size_of_val(&kind) as u32,
            )
        };
    }

    /// Whether the window takes part in "which window is under the pointer": shown, not
    /// minimized, not cloaked (other virtual desktops, suspended apps) and not click-through.
    fn is_hit_testable(window: HWND) -> bool {
        let mut cloaked = 0_u32;
        // SAFETY: `window` comes from EnumWindows; `cloaked` is a valid out buffer of the size
        // given.
        unsafe {
            if IsWindowVisible(window) == 0 || IsIconic(window) != 0 {
                return false;
            }
            let cloaked_ok = DwmGetWindowAttribute(
                window,
                DWMWA_CLOAKED as u32,
                (&raw mut cloaked).cast::<c_void>(),
                size_of::<u32>() as u32,
            ) >= 0;
            if cloaked_ok && cloaked != 0 {
                return false;
            }
            (GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT) == 0
        }
    }

    /// The visible bounds of `window` (without the invisible resize borders where DWM says so).
    fn frame_bounds(window: HWND) -> Option<RECT> {
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: `rect` is a valid out buffer of the size given.
        let is_ok = unsafe {
            DwmGetWindowAttribute(
                window,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                (&raw mut rect).cast::<c_void>(),
                size_of::<RECT>() as u32,
            ) >= 0
                || GetWindowRect(window, &raw mut rect) != 0
        };
        is_ok.then_some(rect)
    }

    struct Search {
        point: POINT,
        skip: HWND,
        found: Option<HWND>,
    }

    unsafe extern "system" fn visit(window: HWND, data: LPARAM) -> i32 {
        // SAFETY: `data` is the `Search` that `window_at` passed and keeps alive during the call.
        let search = unsafe { &mut *(data as *mut Search) };
        if window == search.skip || !is_hit_testable(window) {
            return 1;
        }
        let Some(rect) = frame_bounds(window) else { return 1 };
        let POINT { x, y } = search.point;
        if x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom {
            search.found = Some(window);
            return 0;
        }
        1
    }

    pub(super) fn window_at(x: i32, y: i32, skip: Option<WindowId>) -> Option<WindowId> {
        let mut search = Search { point: POINT { x, y }, skip: skip.map_or(std::ptr::null_mut(), hwnd), found: None };
        // SAFETY: `visit` only reads `search` during the call, which outlives it. EnumWindows
        // lists top-level windows from the top of the z-order down.
        unsafe { EnumWindows(Some(visit), (&raw mut search) as LPARAM) };
        let window = search.found?;
        let mut process = 0_u32;
        // SAFETY: `process` is a valid out pointer.
        unsafe { GetWindowThreadProcessId(window, &raw mut process) };
        // SAFETY: no preconditions.
        (process == unsafe { GetCurrentProcessId() }).then_some(WindowId(window as isize))
    }
}

/// A monitor: what a window placed on it needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    /// The OS handle; gpui uses the same value as its display id.
    pub id: u64,
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// Size of the area not covered by the taskbar, in device pixels.
    pub work_area: (i32, i32),
}

/// The monitor that holds the screen point (`x`, `y`), or the nearest one.
pub fn monitor_at(x: i32, y: i32) -> Option<Monitor> {
    #[cfg(windows)]
    return imp::monitor_at(x, y);
    #[cfg(not(windows))]
    {
        let _ = (x, y);
        None
    }
}

/// Where the pointer is on the screen.
pub fn cursor_position() -> Option<(i32, i32)> {
    #[cfg(windows)]
    return imp::cursor_position();
    #[cfg(not(windows))]
    None
}

/// Screen position of the window's client area (its content, without frame).
pub fn client_origin(id: WindowId) -> Option<(i32, i32)> {
    #[cfg(windows)]
    return imp::client_origin(id);
    #[cfg(not(windows))]
    {
        let _ = id;
        None
    }
}

/// Moves the window so its client area starts at (`x`, `y`), keeping its size, its place in the
/// z-order and the active window. Sends window messages: call it outside of event handling.
pub fn move_client_origin(id: WindowId, x: i32, y: i32) {
    #[cfg(windows)]
    imp::move_client_origin(id, x, y);
    #[cfg(not(windows))]
    let _ = (id, x, y);
}

/// Gives a maximized window its normal size back. Sends window messages: call it outside of
/// event handling.
pub fn restore(id: WindowId) {
    #[cfg(windows)]
    imp::restore(id);
    #[cfg(not(windows))]
    let _ = id;
}

/// Keeps the window above every window that is not topmost, the taskbar included. Taken back, the
/// window goes behind the foreground window. Sends window messages: call it outside of event handling.
pub fn set_topmost(id: WindowId, is_topmost: bool) {
    #[cfg(windows)]
    imp::set_topmost(id, is_topmost);
    #[cfg(not(windows))]
    let _ = (id, is_topmost);
}

/// Plays the system's default alert sound, without waiting for it.
pub fn beep() {
    #[cfg(windows)]
    imp::beep();
}

/// Flashes the window's taskbar button until the window comes to the front.
pub fn flash_taskbar(id: WindowId) {
    #[cfg(windows)]
    imp::flash_taskbar(id);
    #[cfg(not(windows))]
    let _ = id;
}

/// Sends the mouse input to the window until the button is released, wherever the pointer is.
pub fn capture_pointer(id: WindowId) {
    #[cfg(windows)]
    imp::capture_pointer(id);
    #[cfg(not(windows))]
    let _ = id;
}

/// Switches a window that has a system backdrop (Mica) to the system acrylic, whose luminosity
/// layer evens out how bright the content behind it is.
pub fn use_system_acrylic(id: WindowId) {
    #[cfg(windows)]
    imp::use_system_acrylic(id);
    #[cfg(not(windows))]
    let _ = id;
}

/// Sets both native window icons to the embedded variant at the window's current DPI.
pub fn set_icon(id: WindowId, icon: WindowIcon) {
    #[cfg(windows)]
    imp::set_icon(id, icon);
    #[cfg(not(windows))]
    let _ = (id, icon);
}

/// The topmost window at the screen point, passing over `skip`, if it belongs to this process. A
/// window of another app on top hides the app's windows below it.
pub fn window_at(x: i32, y: i32, skip: Option<WindowId>) -> Option<WindowId> {
    #[cfg(windows)]
    return imp::window_at(x, y, skip);
    #[cfg(not(windows))]
    {
        let _ = (x, y, skip);
        None
    }
}
