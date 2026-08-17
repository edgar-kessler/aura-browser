// Aura Browser Setup – ein eigener Installer, ohne Inno, NSIS oder WiX.
//
// Ein Programm, drei Rollen:
//
//   AuraBrowserSetup.exe                 installiert (Fenster)
//   AuraBrowserSetup.exe --silent        installiert ohne Fenster
//   uninstall.exe --uninstall            entfernt (Fenster / --silent)
//   aura-setup.exe pack --payload <dir> --out <exe>
//                                        haengt die Nutzlast an sich selbst
//
// Weitere Schalter: --dir=<Ordner>  --no-desktop-icon  --no-register
// --no-webview2  --purge (Deinstallation loescht auch Browserdaten)
// --lang=de|en  --help
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod i18n;
mod install;
mod payload;
mod sys;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use i18n::{strings, Lang};
use install::Options;

const HELP: &str = "\
Aura Browser Setup

  AuraBrowserSetup.exe [Schalter]         Installation mit Fenster
  AuraBrowserSetup.exe --silent [Schalter] Installation ohne Fenster
  uninstall.exe --uninstall [--silent] [--purge]

Schalter:
  --dir=<Ordner>      Zielordner (Standard: %LOCALAPPDATA%\\Programs\\Aura Browser)
  --no-desktop-icon   keine Verknuepfung auf dem Desktop
  --no-register       nicht als Browser bei Windows anmelden
  --no-webview2       fehlende WebView2-Laufzeit nicht nachinstallieren
  --launch            nach stiller Installation starten
  --purge             beim Entfernen auch Browserdaten loeschen
  --lang=de|en        Sprache der Oberflaeche
  --theme=light|dark  Farbschema (Standard: wie Windows)

Bauen:
  aura-setup.exe pack --payload <Ordner> --out <Datei> [--version <x.y.z>]

Exit-Codes: 0 fertig, 1 fehlgeschlagen, 2 abgebrochen oder falsche Angaben,
3 installiert, aber WebView2 fehlt weiterhin.
";

struct Args {
    silent: bool,
    uninstall: bool,
    purge: bool,
    launch: bool,
    lang: Lang,
    dir: Option<PathBuf>,
    desktop_icon: bool,
    register: bool,
    webview2: bool,
    target: Option<PathBuf>,
    parent_pid: Option<u32>,
    cleanup_self: bool,
    /// --theme=light|dark, sonst wie Windows.
    dark: Option<bool>,
}

fn parse(args: &[String]) -> Args {
    let mut a = Args {
        silent: false,
        uninstall: false,
        purge: false,
        launch: false,
        lang: if sys::ui_is_german() { Lang::De } else { Lang::En },
        dir: None,
        desktop_icon: true,
        register: true,
        webview2: true,
        target: None,
        parent_pid: None,
        cleanup_self: false,
        dark: None,
    };
    for raw in args {
        let arg = raw.trim();
        let low = arg.to_ascii_lowercase();
        let (key, val) = match low.split_once('=') {
            Some((k, _)) => (k.to_string(), Some(&arg[k.len() + 1..])),
            None => (low.clone(), None),
        };
        match key.as_str() {
            "--silent" | "/silent" | "/s" | "-s" | "/verysilent" | "/quiet" | "-q" => a.silent = true,
            "--uninstall" | "/uninstall" => a.uninstall = true,
            "--purge" => a.purge = true,
            "--launch" => a.launch = true,
            "--no-desktop-icon" => a.desktop_icon = false,
            "--no-register" => a.register = false,
            "--no-webview2" => a.webview2 = false,
            "--cleanup-self" => a.cleanup_self = true,
            "--lang" => {
                a.lang = match val.map(|v| v.to_ascii_lowercase()).as_deref() {
                    Some("de") => Lang::De,
                    Some("en") => Lang::En,
                    _ => a.lang,
                }
            }
            "--theme" => {
                a.dark = match val.map(|v| v.to_ascii_lowercase()).as_deref() {
                    Some("dark") => Some(true),
                    Some("light") => Some(false),
                    _ => None,
                }
            }
            "--dir" | "/dir" => a.dir = val.map(|v| PathBuf::from(v.trim_matches('"'))),
            "--target" => a.target = val.map(|v| PathBuf::from(v.trim_matches('"'))),
            "--parent-pid" => a.parent_pid = val.and_then(|v| v.parse().ok()),
            _ => {}
        }
    }
    a
}

fn main() {
    // Ein Absturz soll wenigstens im Protokoll stehen – ein Programm ohne
    // Konsole stirbt sonst stumm.
    std::panic::set_hook(Box::new(|info| {
        sys::log(&format!("Absturz: {info}"));
    }));
    let argv: Vec<String> = std::env::args().skip(1).collect();

    if argv.first().map(|a| a == "pack").unwrap_or(false) {
        sys::attach_console();
        std::process::exit(pack(&argv[1..]));
    }
    if argv.iter().any(|a| a == "--help" || a == "-h" || a == "/?") {
        if sys::attach_console() {
            sys::console_print(HELP);
        } else {
            sys::message_box("Aura Browser Setup", HELP, false);
        }
        std::process::exit(0);
    }
    sys::log_rotate();

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

    let args = parse(&argv);
    let exe = std::env::current_exe().unwrap_or_default();
    let named_uninstall = exe
        .file_stem()
        .map(|s| s.to_string_lossy().eq_ignore_ascii_case("uninstall"))
        .unwrap_or(false);

    let code = if args.uninstall || named_uninstall {
        run_uninstall(&args, &exe)
    } else {
        run_install(&args, &exe)
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------- Installation

fn run_install(args: &Args, exe: &Path) -> i32 {
    let s = strings(args.lang);
    let image = match std::fs::read(exe) {
        Ok(b) => b,
        Err(e) => {
            fail(args, &format!("{}: {e}", exe.display()));
            return 1;
        }
    };
    let Some((_, blob)) = payload::detach(&image) else {
        fail(args, s.err_no_payload);
        return 2;
    };
    let payload = match payload::Payload::decode(blob) {
        Ok(p) => p,
        Err(e) => {
            fail(args, &e);
            return 2;
        }
    };

    let installed = install::installed();
    let dir = match &args.dir {
        Some(d) => install::normalize_dir(d),
        None => installed
            .as_ref()
            .map(|i| i.dir.clone())
            .filter(|d| d.join(install::EXE_NAME).is_file() || d.is_dir())
            .unwrap_or_else(install::default_dir),
    };
    let opts = Options {
        dir,
        desktop_icon: args.desktop_icon,
        register_browser: args.register,
        webview2: args.webview2,
        lang: args.lang,
    };

    if args.silent {
        let mut report = |p: install::Progress| {
            sys::log(&format!("  {:?} {:.0}%", p.step, p.fraction * 100.0));
        };
        return match install::install(&payload, &image, &opts, &mut report) {
            Ok(outcome) => {
                if args.launch {
                    let _ = sys::spawn(&outcome.exe, &[]);
                }
                if outcome.webview2_ok {
                    0
                } else {
                    3
                }
            }
            Err(e) => {
                sys::log(&format!("Fehler: {e}"));
                1
            }
        };
    }

    ui::run(
        ui::Mode::Install {
            payload: Arc::new(payload),
            image: Arc::new(image),
            opts,
            installed,
        },
        args.lang,
        args.dark,
    )
}

// ---------------------------------------------------------------- Deinstallation

fn run_uninstall(args: &Args, exe: &Path) -> i32 {
    let s = strings(args.lang);
    let dir = args
        .target
        .clone()
        .or_else(|| install::installed().map(|i| i.dir))
        .or_else(|| {
            // uninstall.exe im Installationsordner, ohne Registry-Eintrag.
            exe.parent()
                .filter(|p| p.join(install::EXE_NAME).is_file())
                .map(|p| p.to_path_buf())
        });
    let Some(dir) = dir else {
        fail(args, s.err_not_installed);
        return 2;
    };

    // Liegen wir selbst im Ordner, der verschwinden soll? Dann eine Kopie in
    // %TEMP% starten und ihr die Arbeit ueberlassen. Sie wartet, bis wir weg
    // sind, damit auch uninstall.exe geloescht werden kann.
    if exe.starts_with(&dir) && args.target.is_none() {
        let tmp = std::env::temp_dir().join(format!("aura-uninstall-{}.exe", std::process::id()));
        if let Err(e) = std::fs::copy(exe, &tmp) {
            fail(args, &format!("{}: {e}", tmp.display()));
            return 1;
        }
        let mut forward: Vec<String> = vec![
            "--uninstall".into(),
            format!("--target={}", dir.display()),
            format!("--parent-pid={}", std::process::id()),
            "--cleanup-self".into(),
            format!("--lang={}", if args.lang == Lang::De { "de" } else { "en" }),
        ];
        if args.silent {
            forward.push("--silent".into());
        }
        if args.purge {
            forward.push("--purge".into());
        }
        let refs: Vec<&str> = forward.iter().map(|s| s.as_str()).collect();
        return match sys::spawn(&tmp, &refs) {
            Ok(()) => 0,
            Err(e) => {
                fail(args, &e);
                1
            }
        };
    }

    if let Some(pid) = args.parent_pid {
        sys::wait_for_exit(pid, std::time::Duration::from_secs(10));
    }
    if args.cleanup_self {
        sys::delete_after_exit(exe);
    }

    if args.silent {
        let mut report = |p: install::Progress| {
            sys::log(&format!("  {:?} {:.0}%", p.step, p.fraction * 100.0));
        };
        return match install::uninstall(&dir, args.purge, &mut report) {
            Ok(()) => 0,
            Err(e) => {
                sys::log(&format!("Fehler: {e}"));
                1
            }
        };
    }
    ui::run(
        ui::Mode::Uninstall {
            dir,
            purge: args.purge,
        },
        args.lang,
        args.dark,
    )
}

fn fail(args: &Args, msg: &str) {
    sys::log(&format!("Abbruch: {msg}"));
    if !args.silent {
        sys::message_box("Aura Browser Setup", msg, true);
    }
}

// ---------------------------------------------------------------- Packen

/// `pack --payload <Ordner> --out <Datei> [--version <x.y.z>]`
///
/// Nimmt die eigene Programmdatei (ohne eine womoeglich schon angehaengte
/// Nutzlast), packt den Ordner und schreibt beides zusammen als fertiges Setup.
fn pack(args: &[String]) -> i32 {
    let mut payload_dir: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut version = env!("CARGO_PKG_VERSION").to_string();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--payload" => payload_dir = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--version" => {
                if let Some(v) = it.next() {
                    version = v.trim_start_matches('v').to_string();
                }
            }
            _ => {}
        }
    }
    let (Some(payload_dir), Some(out)) = (payload_dir, out) else {
        sys::console_print("Aufruf: aura-setup.exe pack --payload <Ordner> --out <Datei> [--version <x.y.z>]\n");
        return 2;
    };
    if !payload_dir.join(install::EXE_NAME).is_file() {
        sys::console_print(&format!(
            "Fehler: {} enthaelt keine {}\n",
            payload_dir.display(),
            install::EXE_NAME
        ));
        return 2;
    }
    let exe = match std::env::current_exe().and_then(std::fs::read) {
        Ok(b) => b,
        Err(e) => {
            sys::console_print(&format!("Fehler: eigene Programmdatei nicht lesbar: {e}\n"));
            return 1;
        }
    };
    let payload = match payload::Payload::pack_dir(&payload_dir, &version) {
        Ok(p) => p,
        Err(e) => {
            sys::console_print(&format!("Fehler beim Packen: {e}\n"));
            return 1;
        }
    };
    let blob = payload.encode();
    let full = payload::attach(&exe, &blob);
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&out, &full) {
        sys::console_print(&format!("Fehler: {}: {e}\n", out.display()));
        return 1;
    }
    let mb = |n: u64| n as f64 / 1024.0 / 1024.0;
    sys::console_print(&format!(
        "{}  Version {}  {} Dateien, {:.2} MB gepackt aus {:.2} MB, Setup {:.2} MB\n",
        out.display(),
        version,
        payload.files.len(),
        mb(payload.total_packed()),
        mb(payload.total_raw()),
        mb(full.len() as u64)
    ));
    0
}
