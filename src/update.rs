// Auto-Update über GitHub Releases.
//
// Ablauf: Manifest holen → Version vergleichen → ZIP herunterladen → entpacken →
// Austauschskript schreiben → beim Neustart tauscht das Skript die Dateien.
// Netzwerk und Entpacken laufen über curl.exe bzw. PowerShell, beides ist in
// Windows enthalten – keine zusätzlichen Abhängigkeiten für einen Browser, der
// klein bleiben soll.
use std::path::{Path, PathBuf};

/// Wohin der Updater schaut. Wird beim Veröffentlichen einmal gesetzt.
pub const REPO: &str = "edgar-kessler/aura-browser";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Default)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub asset_url: String,
    pub asset_name: String,
    pub size: u64,
}

/// Vergleicht zwei Versionen wie 0.2.10 – Zahl für Zahl, nicht als Text.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split(['.', '-', '+'])
            .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn curl(args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("curl.exe")
        .args(["-sSL", "--max-time", "45", "-A", "Aura Browser Updater"])
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// Fragt das neueste Release ab. `None`, wenn nichts Neueres da ist.
pub fn check() -> Option<Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&[
        "-H",
        "Accept: application/vnd.github+json",
        &url,
    ])?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = v.get("tag_name")?.as_str()?.to_string();
    if !is_newer(&tag, VERSION) {
        return None;
    }
    // Bevorzugt das Windows-x64-Paket.
    let assets = v.get("assets")?.as_array()?;
    let asset = assets
        .iter()
        .find(|a| {
            a.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n.ends_with(".zip") && n.contains("windows"))
                .unwrap_or(false)
        })
        .or_else(|| {
            assets.iter().find(|a| {
                a.get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| n.ends_with(".zip"))
                    .unwrap_or(false)
            })
        })?;
    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        notes: v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .lines()
            .take(12)
            .collect::<Vec<_>>()
            .join("\n"),
        asset_url: asset.get("browser_download_url")?.as_str()?.to_string(),
        asset_name: asset.get("name")?.as_str()?.to_string(),
        size: asset.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
    })
}

fn work_dir() -> PathBuf {
    std::env::temp_dir().join("aura-update")
}

/// Lädt das Paket und entpackt es. Ergebnis: Ordner mit den neuen Dateien.
pub fn download(rel: &Release) -> Option<PathBuf> {
    let dir = work_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let zip = dir.join(&rel.asset_name);
    curl(&["-o", &zip.to_string_lossy(), &rel.asset_url])?;
    let meta = std::fs::metadata(&zip).ok()?;
    if meta.len() < 100_000 {
        return None; // offensichtlich kein vollständiges Paket
    }
    let unpacked = dir.join("new");
    let ps = format!(
        "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
        zip.to_string_lossy(),
        unpacked.to_string_lossy()
    );
    run_hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &ps])?;
    // Manche Pakete haben einen Wurzelordner – dann eine Ebene tiefer gehen.
    let exe_here = unpacked.join("aura-browser.exe").exists();
    if exe_here {
        return Some(unpacked);
    }
    let sub = std::fs::read_dir(&unpacked)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("aura-browser.exe").exists())?;
    Some(sub)
}

fn run_hidden(program: &str, args: &[&str]) -> Option<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let st = std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .ok()?;
    st.success().then_some(())
}

/// Tauscht beim Start die Dateien, wenn ein entpacktes Update bereitliegt, und
/// startet den neuen Stand. Rückgabe `true` heißt: der Aufrufer soll sich sofort
/// beenden, der Nachfolger läuft schon.
///
/// Bewusst ohne Hilfsprozess: ein gestartetes Skript stirbt mit uns, sobald der
/// Browser in einem Job-Objekt läuft (Launcher, Testrunner). Windows erlaubt
/// dagegen, die eigene laufende Programmdatei umzubenennen — genau das nutzen
/// wir hier.
pub fn finish_pending() -> bool {
    let staged = work_dir().join("new");
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(install) = exe.parent() else { return false };

    if !staged.join("aura-browser.exe").is_file() {
        // Reste eines früheren Updates aufräumen.
        let _ = std::fs::remove_file(exe.with_extension("old.exe"));
        return false;
    }
    // Nicht in den Staging-Ordner selbst kopieren.
    if install == staged {
        return false;
    }

    let old = exe.with_extension("old.exe");
    let _ = std::fs::remove_file(&old);
    if std::fs::rename(&exe, &old).is_err() {
        return false;
    }
    if copy_tree(&staged, install).is_err() {
        let _ = std::fs::rename(&old, &exe); // Rückzieher
        return false;
    }
    let _ = std::fs::remove_dir_all(work_dir());

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new(&exe)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    true
}

/// Traegt die laufende Version in den Eintrag unter Apps & Features nach.
/// Das Setup (installer/) schreibt ihn bei der Installation; nach einem
/// Selbst-Update stuende dort sonst fuer immer die alte Nummer. Nur, wenn der
/// Eintrag auf unseren eigenen Ordner zeigt.
pub fn sync_registry_version() {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::*;
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let dir = dir.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Aura Browser"),
            None,
            KEY_READ | KEY_SET_VALUE,
            &mut key,
        )
        .is_err()
        {
            return;
        }
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        let loc_ok = RegQueryValueExW(
            key,
            w!("InstallLocation"),
            None,
            None,
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut size),
        )
        .is_ok();
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let loc = String::from_utf16_lossy(&buf[..len]);
        if loc_ok && loc.trim_end_matches(['\\', '/']).to_lowercase() == dir {
            let mut cur = [0u16; 64];
            let mut csize = (cur.len() * 2) as u32;
            let _ = RegQueryValueExW(
                key,
                w!("DisplayVersion"),
                None,
                None,
                Some(cur.as_mut_ptr() as *mut u8),
                Some(&mut csize),
            );
            let clen = cur.iter().position(|&c| c == 0).unwrap_or(cur.len());
            if String::from_utf16_lossy(&cur[..clen]) != VERSION {
                let value: Vec<u16> = VERSION.encode_utf16().chain(std::iter::once(0)).collect();
                let bytes: Vec<u8> = value.iter().flat_map(|c| c.to_le_bytes()).collect();
                let _ = RegSetValueExW(key, PCWSTR(w!("DisplayVersion").0), None, REG_SZ, Some(&bytes));
            }
        }
        let _ = RegCloseKey(key);
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Schreibt das Austauschskript, startet es und meldet zurück, ob es lief.
/// Der Aufrufer beendet danach den Browser; das Skript wartet darauf.
#[allow(dead_code)]
pub fn apply(new_dir: &Path) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(install) = exe.parent() else { return false };
    let pid = std::process::id();
    let script = work_dir().join("apply.cmd");
    // Warten bis der Browser weg ist, kopieren, neu starten, sich selbst löschen.
    let body = format!(
        "@echo off\r\n\
         setlocal\r\n\
         :wait\r\n\
         tasklist /FI \"PID eq {pid}\" 2>nul | find \"{pid}\" >nul\r\n\
         if not errorlevel 1 (\r\n\
           timeout /t 1 /nobreak >nul\r\n\
           goto wait\r\n\
         )\r\n\
         robocopy \"{new}\" \"{install}\" /E /IS /R:3 /W:1 >nul\r\n\
         start \"\" \"{exe}\"\r\n\
         (goto) 2>nul & del \"%~f0\"\r\n",
        pid = pid,
        new = new_dir.display(),
        install = install.display(),
        exe = exe.display(),
    );
    if std::fs::write(&script, body).is_err() {
        return false;
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Der Browser haengt womoeglich in einem Job-Objekt (Launcher, Explorer-Job,
    // Testrunner). Ohne Breakaway stirbt das Austauschskript zusammen mit uns,
    // bevor es kopieren kann. Wenn der Job kein Breakaway erlaubt, faellt der
    // zweite Versuch auf den normalen Start zurueck.
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let spawn = |flags: u32| {
        std::process::Command::new("cmd.exe")
            .args(["/c", &script.to_string_lossy()])
            .creation_flags(flags)
            .spawn()
    };
    spawn(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| spawn(CREATE_NO_WINDOW))
        .is_ok()
}

/// Sucht im Hintergrund und meldet sich per Fensternachricht zurück.
pub fn check_async(hwnd: isize, msg: u32) {
    std::thread::spawn(move || {
        if let Some(rel) = check() {
            if let Ok(mut slot) = PENDING.lock() {
                *slot = Some(rel);
            }
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)),
                    msg,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    });
}

pub static PENDING: std::sync::Mutex<Option<Release>> = std::sync::Mutex::new(None);

pub fn take_pending() -> Option<Release> {
    PENDING.lock().ok().and_then(|mut s| s.take())
}

/// Lädt und installiert im Hintergrund; meldet über `msg`, ob es geklappt hat
/// (WPARAM 1 = bereit zum Neustart, 0 = fehlgeschlagen).
pub fn install_async(rel: Release, hwnd: isize, msg: u32) {
    std::thread::spawn(move || {
        // Nur herunterladen und entpacken – eingespielt wird beim Neustart.
        let ok = download(&rel).is_some();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)),
                msg,
                windows::Win32::Foundation::WPARAM(ok as usize),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
    }
}
