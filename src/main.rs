// Entry point: COM/DPI init, window creation, message loop.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adblock;
mod app;
mod gfx;
mod omnibox;
mod pages;
mod popup;
mod storage;
mod tabs;
mod theme;
mod update;
mod util;

use app::App;

fn main() {
    // DPI awareness before creating any window.
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED
                | windows::Win32::System::Com::COINIT_DISABLE_OLE1DDE,
        );
    }

    match App::new() {
        Ok(mut app) => {
            if let Err(e) = app.run() {
                eprintln!("Aura error: {e}");
            }
        }
        Err(e) => {
            eprintln!("Aura failed to start: {e}");
            util::error_box(&format!("Aura Browser konnte nicht gestartet werden.\n\n{e}"));
        }
    }
}
