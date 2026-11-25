//! OS-specific code. Every `cfg(windows)` item of the app lives here.
//! Other platforms get no-op fallbacks so the rest of the code stays platform-neutral.

mod shells;
pub mod window;

pub use shells::detect as detect_shells;

/// Skia (Chromium) style text when `skia`, else gpui's DirectWrite style. `gamma` and
/// `contrast` default to the system DirectWrite values, as in Chromium. Call before the first
/// window is created.
pub fn set_antialiasing(antialiasing: wired_config::Antialiasing, gamma: Option<f32>, contrast: Option<f32>) {
    #[cfg(windows)]
    gpui_windows::set_skia_text(antialiasing == wired_config::Antialiasing::Skia, contrast, gamma);
    #[cfg(not(windows))]
    let _ = (antialiasing, gamma, contrast);
}

/// Hides the mouse pointer until it moves. The window's next `WM_SETCURSOR` (sent when the mouse
/// moves) restores it.
pub fn hide_pointer_until_mouse_moves() {
    #[cfg(windows)]
    // SAFETY: SetCursor with a null cursor only changes the current thread's cursor.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetCursor(std::ptr::null_mut());
    }
}

/// Opens `url` with the handler registered for its scheme (the default browser for `http` and
/// `https`). The URL is handed over as one argument, never through a command line; callers
/// decide which schemes may open. Returns `false` if it could not be opened.
///
/// Blocks, and the shell runs a message loop inside: call it off the UI thread, or that loop
/// dispatches the app's own messages in the middle of the caller's update.
pub fn open_url(url: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize},
            UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        };
        if url.is_empty() || url.contains('\0') {
            return false;
        }
        // ShellExecuteW may hand the work to COM components, which need an apartment.
        // SAFETY: plain call; a thread already in another apartment gets an error and is left as is.
        let com =
            unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) };
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (verb, file) = (wide("open"), wide(url));
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; the other
        // pointers may be null as documented.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if com >= 0 {
            // SAFETY: balances the successful CoInitializeEx above on this thread.
            unsafe { CoUninitialize() };
        }
        // Values above 32 mean success.
        result as isize > 32
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        false
    }
}

static ERROR_BOXES: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Shows `message` in an error box, also on stderr. The box runs on a thread of its own, so the
/// caller's windows get no messages while it is open; see [`wait_for_error_boxes`].
pub fn show_error(message: &str) {
    eprintln!("wired: {message}");
    #[cfg(windows)]
    {
        let wide = |s: &str| s.replace('\0', " ").encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(message), wide("wired"));
        let shown = std::thread::Builder::new().name("wired-error-box".into()).spawn(move || {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};
            // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; no owner window.
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
                )
            };
        });
        if let Ok(handle) = shown {
            ERROR_BOXES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(handle);
        }
    }
}

/// Waits until every box from [`show_error`] is closed. Call it before the process exits, or the
/// boxes vanish unread.
pub fn wait_for_error_boxes() {
    let boxes = std::mem::take(&mut *ERROR_BOXES.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    for handle in boxes {
        let _ = handle.join();
    }
}

/// Restricts DLL loading for the whole process to the application directory and System32.
///
/// Must run first in `main`, before any library loads a DLL by name. It stops search-order
/// hijacking: a `conpty.dll` (loaded by name by `portable-pty`) or any other DLL dropped
/// in the current directory or a PATH entry is never loaded.
///
/// Returns `false` if the OS refused the setting.
pub fn harden_dll_search() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::LibraryLoader::{
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
        };
        // SAFETY: plain Win32 call with constant flags; it only changes the loader search policy.
        unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) != 0 }
    }
    #[cfg(not(windows))]
    {
        true
    }
}
