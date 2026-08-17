// Was der Installer tut – ohne Fenster. Die Oberflaeche und der stille Modus
// rufen dieselben zwei Funktionen: `install` und `uninstall`. Fortschritt geht
// ueber einen Rueckruf; die Texte dazu waehlt der Aufrufer.
//
// Installiert wird pro Benutzer nach %LOCALAPPDATA%\Programs\Aura Browser,
// ohne Administratorrechte. Das ist Absicht: der eingebaute Auto-Updater
// tauscht die Dateien spaeter selbst aus und darf dafuer keine Rechte brauchen.
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::System::Registry::HKEY_CURRENT_USER;

use crate::i18n::{fill, strings, Lang};
use crate::payload::Payload;
use crate::sys::{self, log, Key};

pub const APP_NAME: &str = "Aura Browser";
pub const EXE_NAME: &str = "aura-browser.exe";
pub const UNINSTALLER: &str = "uninstall.exe";
pub const PUBLISHER: &str = "Edgar Kessler";
pub const URL: &str = "https://github.com/edgar-kessler/aura-browser";
pub const WEBVIEW2_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
/// Wo der Browser seine Daten ablegt (siehe src/storage.rs): %LOCALAPPDATA%\AuraBrowser.
pub const DATA_DIR: &str = "AuraBrowser";

const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Aura Browser";
/// Der Eintrag des frueheren Inno-Setup-Installers (AppId aus installer/aura.iss).
const INNO_KEY: &str =
    "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{8E4B1C6A-2F71-4B2E-9E1C-3A9D6F0B7C51}_is1";
const PROG_ID: &str = "AuraHTML";
const CLASSES_KEY: &str = "Software\\Classes\\AuraHTML";
const CLIENT_KEY: &str = "Software\\Clients\\StartMenuInternet\\AuraBrowser";
const REGISTERED_APPS: &str = "Software\\RegisteredApplications";

#[derive(Clone, Debug)]
pub struct Options {
    /// Zielordner, endet immer auf "Aura Browser" (siehe `normalize_dir`).
    pub dir: PathBuf,
    pub desktop_icon: bool,
    pub register_browser: bool,
    /// WebView2-Laufzeit nachinstallieren, falls sie fehlt.
    pub webview2: bool,
    pub lang: Lang,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Closing = 0,
    Files = 1,
    Uninstaller = 2,
    Shortcuts = 3,
    Registry = 4,
    WebView2 = 5,
    Removing = 6,
    Data = 7,
}

impl Step {
    pub fn from_u32(v: u32) -> Step {
        match v {
            0 => Step::Closing,
            1 => Step::Files,
            2 => Step::Uninstaller,
            3 => Step::Shortcuts,
            4 => Step::Registry,
            5 => Step::WebView2,
            6 => Step::Removing,
            _ => Step::Data,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub step: Step,
    /// 0..1 ueber den ganzen Vorgang.
    pub fraction: f32,
    /// Dauer nicht absehbar (WebView2-Setup laeuft) – Balken laeuft frei.
    pub indeterminate: bool,
}

pub struct Outcome {
    pub exe: PathBuf,
    pub webview2_ok: bool,
    pub registered: bool,
}

pub struct Installed {
    pub version: String,
    pub dir: PathBuf,
}

/// Was schon da ist – aus dem eigenen Deinstallations-Eintrag oder dem des
/// frueheren Inno-Setups.
pub fn installed() -> Option<Installed> {
    for key in [UNINSTALL_KEY, INNO_KEY] {
        let (Some(version), Some(dir)) = (
            sys::reg_str(HKEY_CURRENT_USER, key, "DisplayVersion"),
            sys::reg_str(HKEY_CURRENT_USER, key, "InstallLocation"),
        ) else {
            continue;
        };
        let dir = dir.trim_end_matches(['\\', '/']).to_string();
        if !dir.is_empty() {
            return Some(Installed {
                version,
                dir: PathBuf::from(dir),
            });
        }
    }
    None
}

pub fn default_dir() -> PathBuf {
    sys::local_app_data().join("Programs").join(APP_NAME)
}

/// Sorgt dafuer, dass das Ziel ein eigener Ordner namens "Aura Browser" ist.
/// Waehlt jemand D:\Programme, wird daraus D:\Programme\Aura Browser – so
/// kann die Deinstallation spaeter den ganzen Ordner wegnehmen, ohne fremde
/// Dateien zu treffen.
pub fn normalize_dir(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let trimmed = s.trim_end_matches(['\\', '/']);
    let p = if trimmed.is_empty() { p.to_path_buf() } else { PathBuf::from(trimmed) };
    match p.file_name() {
        Some(n) if n.to_string_lossy().eq_ignore_ascii_case(APP_NAME) => p,
        _ => p.join(APP_NAME),
    }
}

/// Gleicher Ordner? Ohne Ruecksicht auf Gross-/Kleinschreibung und einen
/// abschliessenden Schraegstrich.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    norm(a) == norm(b)
}

pub fn is_running(dir: &Path) -> bool {
    !sys::processes_in(dir).is_empty()
}

fn stem_old(target: &Path) -> PathBuf {
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = target
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    target.with_file_name(format!("{stem}.old.{ext}"))
}

/// Schreibt eine Datei. Ist die alte in Benutzung – laufender Browser –,
/// wird sie beiseitegeschoben statt ueberschrieben: Windows erlaubt es, eine
/// laufende Programmdatei umzubenennen, nur nicht, sie zu ueberschreiben.
/// Den Rest `aura-browser.old.exe` raeumt der Browser beim naechsten Start
/// selbst weg (src/update.rs).
fn write_replacing(target: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    match std::fs::write(target, data) {
        Ok(()) => Ok(()),
        Err(first) if target.exists() => {
            let old = stem_old(target);
            let _ = std::fs::remove_file(&old);
            std::fs::rename(target, &old)
                .map_err(|e| format!("{}: {first} / {e}", target.display()))?;
            std::fs::write(target, data).map_err(|e| format!("{}: {e}", target.display()))
        }
        Err(e) => Err(format!("{}: {e}", target.display())),
    }
}

/// Reste frueherer Laeufe: `*.old.*` im Zielordner.
fn sweep_old(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.contains(".old.") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Wer Aura frueher mit dem Inno-Setup installiert hat, traegt dessen
/// Deinstallierer, seinen Registry-Eintrag und einen Startmenue-Ordner mit
/// sich herum. Das raeumen wir weg – sonst staenden zwei "Aura Browser" unter
/// Apps & Features.
fn retire_inno(dir: &Path) {
    for name in ["unins000.exe", "unins000.dat", "unins000.msg"] {
        let p = dir.join(name);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
            log(&format!("Inno-Rest entfernt: {name}"));
        }
    }
    // Nur, wenn der alte Eintrag auf denselben Ordner zeigt.
    if let Some(loc) = sys::reg_str(HKEY_CURRENT_USER, INNO_KEY, "InstallLocation") {
        if same_dir(Path::new(&loc), dir) {
            sys::reg_delete_tree(HKEY_CURRENT_USER, INNO_KEY);
            log("Inno-Eintrag in Apps & Features entfernt");
        }
    }
    remove_start_menu_group(dir);
}

/// Der Startmenue-Ordner "Aura Browser" alter Art (mit Verknuepfung und
/// "entfernen"-Eintrag). Heute liegt nur eine .lnk direkt unter Programme.
fn remove_start_menu_group(dir: &Path) {
    let Some(programs) = sys::start_menu_programs() else { return };
    let group = programs.join(APP_NAME);
    if !group.is_dir() {
        return;
    }
    if let Ok(rd) = std::fs::read_dir(&group) {
        for e in rd.flatten() {
            let p = e.path();
            let is_lnk = p
                .extension()
                .map(|x| x.eq_ignore_ascii_case("lnk"))
                .unwrap_or(false);
            if is_lnk {
                remove_shortcut(&p, dir);
            }
        }
    }
    let _ = std::fs::remove_dir(&group);
}

pub fn install(
    payload: &Payload,
    self_image: &[u8],
    opts: &Options,
    report: &mut dyn FnMut(Progress),
) -> Result<Outcome, String> {
    let s = strings(opts.lang);
    let dir = &opts.dir;
    let exe = dir.join(EXE_NAME);
    log(&format!(
        "Installation {} nach {} (Desktop {}, Browser anmelden {}, WebView2 {})",
        payload.version,
        dir.display(),
        opts.desktop_icon,
        opts.register_browser,
        opts.webview2
    ));

    if !payload.files.iter().any(|f| f.path == EXE_NAME) {
        return Err(s.err_no_payload.to_string());
    }
    if let Some(free) = sys::free_space(dir) {
        if free < payload.total_raw() + 20 * 1024 * 1024 {
            return Err(s.err_space.to_string());
        }
    }

    // 1. Laufende Instanz freundlich beenden. Was danach noch laeuft, wird beim
    //    Schreiben umbenannt statt ueberschrieben.
    report(Progress { step: Step::Closing, fraction: 0.02, indeterminate: false });
    let left = sys::close_running(dir, Duration::from_secs(6));
    if !left.is_empty() {
        log(&format!("laeuft noch, wird umbenannt: {left:?}"));
    }

    // 2. Ordner
    std::fs::create_dir_all(dir)
        .map_err(|_| fill(s.err_dir, &[("dir", &dir.to_string_lossy())]))?;
    sweep_old(dir);
    retire_inno(dir);

    // 3. Dateien
    let total = payload.total_raw().max(1) as f32;
    let mut done = 0u64;
    for f in &payload.files {
        report(Progress {
            step: Step::Files,
            fraction: 0.05 + 0.65 * (done as f32 / total),
            indeterminate: false,
        });
        let data = f.unpack()?;
        write_replacing(&f.target(dir), &data)?;
        done += f.raw_len;
    }
    log(&format!("{} Dateien geschrieben", payload.files.len()));

    // 4. Der Deinstallierer ist dieses Programm selbst, mitsamt Nutzlast.
    report(Progress { step: Step::Uninstaller, fraction: 0.72, indeterminate: false });
    write_replacing(&dir.join(UNINSTALLER), self_image)?;

    // 5. Verknuepfungen. Startmenue immer, Desktop auf Wunsch. Kein eigener
    //    Ordner im Startmenue und kein "entfernen"-Eintrag – dafuer gibt es
    //    Apps & Features.
    report(Progress { step: Step::Shortcuts, fraction: 0.78, indeterminate: false });
    if let Some(programs) = sys::start_menu_programs() {
        sys::create_shortcut(&programs.join(format!("{APP_NAME}.lnk")), &exe, APP_NAME)?;
    }
    if opts.desktop_icon {
        if let Some(desktop) = sys::desktop() {
            sys::create_shortcut(&desktop.join(format!("{APP_NAME}.lnk")), &exe, APP_NAME)?;
        }
    }

    // 6. Registry: Eintrag in Apps & Features, auf Wunsch Anmeldung als Browser.
    report(Progress { step: Step::Registry, fraction: 0.85, indeterminate: false });
    let size_kb = ((payload.total_raw() + self_image.len() as u64) / 1024) as u32;
    write_uninstall_entry(dir, &exe, &payload.version, size_kb)?;
    if opts.register_browser {
        register_browser(&exe, opts.lang)?;
    }
    sys::notify_assoc_changed();

    // 7. WebView2, falls sie fehlt und gewuenscht ist. Ein Fehler hier bricht
    //    die Installation nicht ab – Aura ist da, nur die Laufzeit fehlt noch.
    let mut webview2_ok = true;
    if opts.webview2 && !sys::webview2_present() {
        report(Progress { step: Step::WebView2, fraction: 0.9, indeterminate: false });
        match install_webview2(report) {
            Ok(()) => log("WebView2-Laufzeit installiert"),
            Err(e) => {
                log(&format!("WebView2: {e}"));
                webview2_ok = false;
            }
        }
    }

    report(Progress { step: Step::Registry, fraction: 1.0, indeterminate: false });
    log("fertig");
    Ok(Outcome {
        exe,
        webview2_ok,
        registered: opts.register_browser,
    })
}

fn write_uninstall_entry(dir: &Path, exe: &Path, version: &str, size_kb: u32) -> Result<(), String> {
    let k = Key::create(HKEY_CURRENT_USER, UNINSTALL_KEY)?;
    let un = dir.join(UNINSTALLER);
    k.set_str("DisplayName", APP_NAME)?;
    k.set_str("DisplayVersion", version)?;
    k.set_str("Publisher", PUBLISHER)?;
    k.set_str("DisplayIcon", &exe.to_string_lossy())?;
    k.set_str("InstallLocation", &dir.to_string_lossy())?;
    k.set_str("UninstallString", &format!("\"{}\" --uninstall", un.display()))?;
    k.set_str(
        "QuietUninstallString",
        &format!("\"{}\" --uninstall --silent", un.display()),
    )?;
    k.set_str("URLInfoAbout", URL)?;
    k.set_str("HelpLink", &format!("{URL}/issues"))?;
    k.set_str("URLUpdateInfo", &format!("{URL}/releases"))?;
    k.set_str("InstallDate", &sys::today())?;
    k.set_u32("NoModify", 1)?;
    k.set_u32("NoRepair", 1)?;
    k.set_u32("EstimatedSize", size_kb)?;
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    k.set_u32("VersionMajor", parts.next().unwrap_or(0))?;
    k.set_u32("VersionMinor", parts.next().unwrap_or(0))?;
    Ok(())
}

/// Meldet Aura bei Windows als Browser an – damit es unter Einstellungen →
/// Standard-Apps auftaucht. Alles unter HKCU, wie die Installation selbst.
fn register_browser(exe: &Path, lang: Lang) -> Result<(), String> {
    let exe_s = exe.to_string_lossy().to_string();
    let icon = format!("{exe_s},0");
    let description = match lang {
        Lang::De => "Nativer Windows-Browser mit eingebautem Werbeblocker",
        Lang::En => "Native Windows browser with a built-in ad blocker",
    };
    let doc = match lang {
        Lang::De => "Aura HTML-Dokument",
        Lang::En => "Aura HTML Document",
    };

    // Die Dokumentklasse, auf die http/https/.html zeigen.
    Key::create(HKEY_CURRENT_USER, CLASSES_KEY)?.set_str("", doc)?;
    Key::create(HKEY_CURRENT_USER, &format!("{CLASSES_KEY}\\DefaultIcon"))?.set_str("", &icon)?;
    Key::create(HKEY_CURRENT_USER, &format!("{CLASSES_KEY}\\shell\\open\\command"))?
        .set_str("", &format!("\"{exe_s}\" \"%1\""))?;
    let app = Key::create(HKEY_CURRENT_USER, &format!("{CLASSES_KEY}\\Application"))?;
    app.set_str("ApplicationName", APP_NAME)?;
    app.set_str("ApplicationIcon", &icon)?;
    app.set_str("ApplicationCompany", PUBLISHER)?;
    app.set_str("ApplicationDescription", description)?;

    // Der Browser-Eintrag mit seinen Faehigkeiten.
    Key::create(HKEY_CURRENT_USER, CLIENT_KEY)?.set_str("", APP_NAME)?;
    Key::create(HKEY_CURRENT_USER, &format!("{CLIENT_KEY}\\DefaultIcon"))?.set_str("", &icon)?;
    Key::create(HKEY_CURRENT_USER, &format!("{CLIENT_KEY}\\shell\\open\\command"))?
        .set_str("", &format!("\"{exe_s}\""))?;
    let cap = Key::create(HKEY_CURRENT_USER, &format!("{CLIENT_KEY}\\Capabilities"))?;
    cap.set_str("ApplicationName", APP_NAME)?;
    cap.set_str("ApplicationDescription", description)?;
    cap.set_str("ApplicationIcon", &icon)?;
    Key::create(HKEY_CURRENT_USER, &format!("{CLIENT_KEY}\\Capabilities\\StartMenu"))?
        .set_str("StartMenuInternet", "AuraBrowser")?;
    let urls = Key::create(HKEY_CURRENT_USER, &format!("{CLIENT_KEY}\\Capabilities\\URLAssociations"))?;
    urls.set_str("http", PROG_ID)?;
    urls.set_str("https", PROG_ID)?;
    let files = Key::create(HKEY_CURRENT_USER, &format!("{CLIENT_KEY}\\Capabilities\\FileAssociations"))?;
    for ext in [".htm", ".html", ".xhtml", ".shtml"] {
        files.set_str(ext, PROG_ID)?;
    }
    Key::create(HKEY_CURRENT_USER, REGISTERED_APPS)?
        .set_str(APP_NAME, &format!("{CLIENT_KEY}\\Capabilities"))?;
    Ok(())
}

/// Zeigt die Browser-Anmeldung auf eine Programmdatei in `dir`? Nur dann
/// gehoert sie uns – eine zweite Installation woanders lassen wir in Ruhe.
fn registered_for(dir: &Path) -> bool {
    let cmd = sys::reg_str(HKEY_CURRENT_USER, &format!("{CLIENT_KEY}\\shell\\open\\command"), "")
        .or_else(|| sys::reg_str(HKEY_CURRENT_USER, &format!("{CLASSES_KEY}\\shell\\open\\command"), ""));
    match cmd {
        Some(c) => c.to_lowercase().contains(&dir.to_string_lossy().to_lowercase()),
        None => false,
    }
}

fn unregister_browser() {
    sys::reg_delete_value(HKEY_CURRENT_USER, REGISTERED_APPS, APP_NAME);
    sys::reg_delete_tree(HKEY_CURRENT_USER, CLIENT_KEY);
    sys::reg_delete_tree(HKEY_CURRENT_USER, CLASSES_KEY);
}

fn install_webview2(report: &mut dyn FnMut(Progress)) -> Result<(), String> {
    let tmp = std::env::temp_dir().join("MicrosoftEdgeWebview2Setup.exe");
    let _ = std::fs::remove_file(&tmp);
    sys::download(
        "go.microsoft.com",
        "/fwlink/p/?LinkId=2124703",
        &tmp,
        &mut |got, total| {
            let f = match total {
                Some(t) if t > 0 => (got as f32 / t as f32).min(1.0),
                _ => 0.5,
            };
            report(Progress {
                step: Step::WebView2,
                fraction: 0.9 + 0.06 * f,
                indeterminate: total.is_none(),
            });
        },
    )?;
    report(Progress { step: Step::WebView2, fraction: 0.97, indeterminate: true });
    let code = sys::run_and_wait(&tmp, &["/silent", "/install"])?;
    let _ = std::fs::remove_file(&tmp);
    if code == 0 || sys::webview2_present() {
        Ok(())
    } else {
        Err(format!("Setup meldet Code {code}"))
    }
}

/// Loescht eine Verknuepfung, aber nur, wenn sie auf unsere Programmdatei zeigt.
fn remove_shortcut(lnk: &Path, dir: &Path) {
    if !lnk.exists() {
        return;
    }
    let ours = match sys::shortcut_target(lnk) {
        Some(t) => t.starts_with(dir),
        None => true, // kaputte .lnk mit unserem Namen: weg damit
    };
    if ours {
        let _ = std::fs::remove_file(lnk);
    }
}

pub fn uninstall(dir: &Path, purge: bool, report: &mut dyn FnMut(Progress)) -> Result<(), String> {
    log(&format!("Deinstallation aus {} (Daten loeschen: {purge})", dir.display()));
    let dedicated = dir
        .file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case(APP_NAME))
        .unwrap_or(false)
        || dir.join(EXE_NAME).is_file();

    report(Progress { step: Step::Closing, fraction: 0.05, indeterminate: false });
    let left = sys::close_running(dir, Duration::from_secs(6));
    if !left.is_empty() {
        log(&format!("laeuft noch: {left:?}"));
    }

    report(Progress { step: Step::Registry, fraction: 0.2, indeterminate: false });
    if registered_for(dir) {
        unregister_browser();
    }
    if installed().map(|i| same_dir(&i.dir, dir)).unwrap_or(false) {
        sys::reg_delete_tree(HKEY_CURRENT_USER, UNINSTALL_KEY);
    }
    sys::notify_assoc_changed();

    report(Progress { step: Step::Shortcuts, fraction: 0.3, indeterminate: false });
    retire_inno(dir);
    if let Some(programs) = sys::start_menu_programs() {
        remove_shortcut(&programs.join(format!("{APP_NAME}.lnk")), dir);
    }
    if let Some(desktop) = sys::desktop() {
        remove_shortcut(&desktop.join(format!("{APP_NAME}.lnk")), dir);
    }

    report(Progress { step: Step::Removing, fraction: 0.4, indeterminate: false });
    let mut leftover: Option<String> = None;
    if dedicated && dir.is_dir() {
        // Erst alles einzeln, damit ein einzelner Fehler nicht den Rest
        // stehen laesst; dann den Ordner selbst.
        remove_tree(dir, &mut leftover);
        if let Some(l) = &leftover {
            log(&format!("blieb stehen: {l}"));
        }
    } else {
        log("kein Aura-Ordner, Dateien bleiben unangetastet");
    }

    if purge {
        report(Progress { step: Step::Data, fraction: 0.9, indeterminate: false });
        let data = sys::local_app_data().join(DATA_DIR);
        if data.is_dir() {
            let mut lo = None;
            remove_tree(&data, &mut lo);
            if let Some(l) = lo {
                log(&format!("Daten teilweise geblieben: {l}"));
            }
        }
    }

    report(Progress { step: Step::Removing, fraction: 1.0, indeterminate: false });
    log("entfernt");
    match leftover {
        Some(l) if dir.join(EXE_NAME).exists() => Err(l),
        _ => Ok(()),
    }
}

fn remove_tree(dir: &Path, leftover: &mut Option<String>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                remove_tree(&p, leftover);
            } else if let Err(err) = std::fs::remove_file(&p) {
                // Eine noch laufende Programmdatei laesst sich wenigstens
                // umbenennen; der Ordner wird dann beim naechsten Mal frei.
                let old = stem_old(&p);
                if std::fs::rename(&p, &old).is_err() && leftover.is_none() {
                    *leftover = Some(format!("{}: {err}", p.display()));
                }
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}
