//! wired: terminal emulator for Windows 11.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Before anything can load a DLL by name.
    if !wired_platform::harden_dll_search() {
        wired_platform::show_error("Windows did not allow limiting where DLLs load from, so wired did not start.");
        wired_platform::wait_for_error_boxes();
        return;
    }

    gpui::application().with_assets(gpui::assets::Assets).run(|cx| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                wired_platform::wait_for_error_boxes();
                cx.quit();
            }
        })
        .detach();

        if let Err(error) = wired_ui::start(cx) {
            wired_platform::show_error(&format!("wired could not start: {error:#}"));
            wired_platform::wait_for_error_boxes();
            cx.quit();
        }
    });
}
