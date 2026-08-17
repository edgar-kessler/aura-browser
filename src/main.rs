// Entry point: COM/DPI init, window creation, message loop.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adblock;
mod anim;
mod app;
mod devtools;
mod gfx;
mod omnibox;
mod pages;
mod popup;
mod storage;
mod tabs;
mod textfield;
mod theme;
mod update;
mod util;

use app::App;

fn main() {
    // Liegt ein fertig geladenes Update bereit? Dann jetzt tauschen, bevor
    // irgendetwas anderes passiert - der Nachfolger uebernimmt.
    if update::finish_pending() {
        return;
    }
    // Nach einem Selbst-Update soll Apps & Features die neue Nummer zeigen.
    update::sync_registry_version();

    // Kopfloser Update-Lauf: prueft, laedt, entpackt und beendet sich. Damit
    // laesst sich das Update per Skript ausloesen und ueberhaupt testen.
    if std::env::args_os().any(|a| a == "--self-update") {
        std::process::exit(self_update());
    }

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
            // Ein privates Fenster hinterlaesst nichts: erst die Datenbank
            // schliessen (App fallen lassen), dann den Profilordner loeschen.
            let private_dir = app.private_data_dir();
            drop(app);
            if let Some(dir) = private_dir {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        Err(e) => {
            eprintln!("Aura failed to start: {e}");
            util::error_box(&format!("Aura Browser konnte nicht gestartet werden.\n\n{e}"));
        }
    }
}

/// Prueft und installiert ein Update ohne Oberflaeche. Rueckgabe ist der
/// Exit-Code: 0 = eingespielt, 1 = nichts Neues, 2 = fehlgeschlagen. Der
/// Verlauf landet zusaetzlich in %TEMP%\aura-update\update.log.
fn self_update() -> i32 {
    let log_path = std::env::temp_dir().join("aura-update").join("update.log");
    let _ = std::fs::create_dir_all(log_path.parent().unwrap());
    let mut log = String::new();
    let mut note = |line: String| {
        println!("{line}");
        log.push_str(&line);
        log.push('\n');
    };

    note(format!("installiert: {}", update::VERSION));
    note(format!("quelle:      {}", update::REPO));

    let code = match update::check() {
        None => {
            note("ergebnis:    kein neueres Release".into());
            1
        }
        Some(rel) => {
            note(format!("gefunden:    {} ({}, {} Bytes)", rel.version, rel.asset_name, rel.size));
            match update::download(&rel) {
                None => {
                    note("ergebnis:    Download oder Entpacken fehlgeschlagen".into());
                    2
                }
                Some(dir) => {
                    note(format!("entpackt:    {}", dir.display()));
                    note("ergebnis:    bereit, wird beim naechsten Start eingespielt".into());
                    0
                }
            }
        }
    };
    let _ = std::fs::write(&log_path, log);
    code
}
