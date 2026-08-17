// Win32-Helfer: Zeichenketten, Registry, bekannte Ordner, Verknuepfungen,
// Prozesse, Download, Konsole, Protokoll. Alles, was nichts mit dem Zeichnen
// zu tun hat und trotzdem an Windows haengt.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------- Zeichenketten

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn utf16_bytes(s: &str) -> Vec<u8> {
    wide(s).iter().flat_map(|c| c.to_le_bytes()).collect()
}

// ---------------------------------------------------------------- Protokoll

/// `%TEMP%\aura-setup.log` – jede Zeile mit Uhrzeit. Bei einem stillen Lauf
/// ist das die einzige Spur; bei einem Fehler zeigt die Oberflaeche darauf.
pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("aura-setup.log")
}

/// Haelt das Protokoll klein: ab 256 KB faengt es von vorn an.
pub fn log_rotate() {
    if let Ok(meta) = std::fs::metadata(log_path()) {
        if meta.len() > 256 * 1024 {
            let _ = std::fs::remove_file(log_path());
        }
    }
}

pub fn log(line: &str) {
    use std::io::Write;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(
            f,
            "{:02}:{:02}:{:02}  {}",
            t.wHour, t.wMinute, t.wSecond, line
        );
    }
}

pub fn today() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}{:02}{:02}", t.wYear, t.wMonth, t.wDay)
}

// ---------------------------------------------------------------- Registry

pub struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl Key {
    pub fn create(root: HKEY, sub: &str) -> Result<Key, String> {
        let s = wide(sub);
        let mut h = HKEY::default();
        let r = unsafe {
            RegCreateKeyExW(
                root,
                PCWSTR(s.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE | KEY_READ,
                None,
                &mut h,
                None,
            )
        };
        if r.is_ok() {
            Ok(Key(h))
        } else {
            Err(format!("Registry: {sub} liess sich nicht anlegen ({})", r.0))
        }
    }

    pub fn set_str(&self, name: &str, value: &str) -> Result<(), String> {
        let n = wide(name);
        let bytes = utf16_bytes(value);
        let r = unsafe { RegSetValueExW(self.0, PCWSTR(n.as_ptr()), None, REG_SZ, Some(&bytes)) };
        if r.is_ok() {
            Ok(())
        } else {
            Err(format!("Registry: Wert {name} liess sich nicht schreiben ({})", r.0))
        }
    }

    pub fn set_u32(&self, name: &str, value: u32) -> Result<(), String> {
        let n = wide(name);
        let r = unsafe {
            RegSetValueExW(
                self.0,
                PCWSTR(n.as_ptr()),
                None,
                REG_DWORD,
                Some(&value.to_le_bytes()),
            )
        };
        if r.is_ok() {
            Ok(())
        } else {
            Err(format!("Registry: Wert {name} liess sich nicht schreiben ({})", r.0))
        }
    }
}

/// Liest eine Zeichenkette; `name` leer heisst Standardwert.
pub fn reg_str(root: HKEY, sub: &str, name: &str) -> Option<String> {
    let s = wide(sub);
    let n = wide(name);
    let mut size: u32 = 0;
    unsafe {
        let r = RegGetValueW(
            root,
            PCWSTR(s.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        );
        if r != ERROR_SUCCESS || size < 2 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size2 = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            root,
            PCWSTR(s.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size2),
        );
        if r != ERROR_SUCCESS {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// Liest einen DWORD.
pub fn reg_u32(root: HKEY, sub: &str, name: &str) -> Option<u32> {
    let s = wide(sub);
    let n = wide(name);
    let mut v: u32 = 0;
    let mut size = 4u32;
    unsafe {
        let r = RegGetValueW(
            root,
            PCWSTR(s.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        );
        (r == ERROR_SUCCESS).then_some(v)
    }
}

pub fn reg_delete_tree(root: HKEY, sub: &str) {
    let s = wide(sub);
    unsafe {
        let _ = RegDeleteTreeW(root, PCWSTR(s.as_ptr()));
        let _ = RegDeleteKeyW(root, PCWSTR(s.as_ptr()));
    }
}

pub fn reg_delete_value(root: HKEY, sub: &str, name: &str) {
    let s = wide(sub);
    let n = wide(name);
    unsafe {
        let mut h = HKEY::default();
        if RegOpenKeyExW(root, PCWSTR(s.as_ptr()), None, KEY_SET_VALUE, &mut h).is_ok() {
            let _ = RegDeleteValueW(h, PCWSTR(n.as_ptr()));
            let _ = RegCloseKey(h);
        }
    }
}

// ---------------------------------------------------------------- Ordner

pub fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

pub fn local_app_data() -> PathBuf {
    known_folder(&FOLDERID_LocalAppData)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("C:\\"))
}

pub fn start_menu_programs() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs)
}

pub fn desktop() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop)
}

/// Freier Platz auf dem Laufwerk des tiefsten schon vorhandenen Elternordners.
pub fn free_space(dir: &Path) -> Option<u64> {
    let mut p = dir;
    while !p.exists() {
        p = p.parent()?;
    }
    let w = wide_path(p);
    let mut free: u64 = 0;
    unsafe {
        windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            PCWSTR(w.as_ptr()),
            Some(&mut free),
            None,
            None,
        )
        .ok()?;
    }
    Some(free)
}

// ---------------------------------------------------------------- Verknuepfungen

pub fn create_shortcut(lnk: &Path, target: &Path, description: &str) -> Result<(), String> {
    let err = |what: &str, e: windows::core::Error| format!("Verknuepfung {what}: {e}");
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| err("anlegen", e))?;
        let t = wide_path(target);
        link.SetPath(PCWSTR(t.as_ptr())).map_err(|e| err("Ziel", e))?;
        let wd = wide_path(target.parent().unwrap_or(target));
        link.SetWorkingDirectory(PCWSTR(wd.as_ptr()))
            .map_err(|e| err("Ordner", e))?;
        link.SetIconLocation(PCWSTR(t.as_ptr()), 0)
            .map_err(|e| err("Symbol", e))?;
        let d = wide(description);
        link.SetDescription(PCWSTR(d.as_ptr()))
            .map_err(|e| err("Beschreibung", e))?;
        let file: IPersistFile = link.cast().map_err(|e| err("speichern", e))?;
        if let Some(parent) = lnk.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let l = wide_path(lnk);
        file.Save(PCWSTR(l.as_ptr()), true)
            .map_err(|e| err("speichern", e))?;
    }
    Ok(())
}

/// Wohin eine .lnk zeigt – um beim Entfernen nur unsere eigenen zu loeschen.
pub fn shortcut_target(lnk: &Path) -> Option<PathBuf> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let l = wide_path(lnk);
        file.Load(PCWSTR(l.as_ptr()), STGM_READ).ok()?;
        let mut buf = [0u16; 2048];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }
}

pub fn notify_assoc_changed() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

// ---------------------------------------------------------------- Prozesse

/// Bittet laufende Aura-Fenster, sich zu schliessen, und wartet, bis kein
/// `aura-browser.exe` aus `dir` mehr laeuft. Zurueck kommen die PIDs, die
/// nach Ablauf der Frist noch da sind.
pub fn close_running(dir: &Path, wait: Duration) -> Vec<u32> {
    let ours = processes_in(dir);
    if ours.is_empty() {
        return ours;
    }
    // Nur Fenster der Prozesse aus dem Zielordner – eine Aura-Instanz, die
    // woanders liegt (etwa ein Entwicklungsstand), geht uns nichts an.
    let class = wide("AuraMainWindow");
    unsafe {
        let mut prev: Option<HWND> = None;
        for _ in 0..64 {
            match FindWindowExW(None, prev, PCWSTR(class.as_ptr()), PCWSTR::null()) {
                Ok(h) if !h.is_invalid() => {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(h, Some(&mut pid));
                    if ours.contains(&pid) {
                        let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    prev = Some(h);
                }
                _ => break,
            }
        }
    }
    let deadline = Instant::now() + wait;
    loop {
        let pids = processes_in(dir);
        if pids.is_empty() || Instant::now() >= deadline {
            return pids;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// PIDs aller `aura-browser.exe`, deren Programmdatei in `dir` liegt.
pub fn processes_in(dir: &Path) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::*;
    let want = dir.to_string_lossy().to_lowercase();
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
                if name == "aura-browser.exe" {
                    if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, e.th32ProcessID) {
                        let mut buf = [0u16; 2048];
                        let mut n = buf.len() as u32;
                        if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok() {
                            let full = String::from_utf16_lossy(&buf[..n as usize]).to_lowercase();
                            if full.starts_with(&want) {
                                out.push(e.th32ProcessID);
                            }
                        }
                        let _ = CloseHandle(h);
                    }
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Wartet, bis ein Prozess weg ist (hoechstens `wait`).
pub fn wait_for_exit(pid: u32, wait: Duration) {
    use windows::Win32::System::Threading::*;
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, wait.as_millis() as u32);
            let _ = CloseHandle(h);
        }
    }
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Startet ein Programm ohne Fenster und wartet auf den Exit-Code.
pub fn run_and_wait(exe: &Path, args: &[&str]) -> Result<i32, String> {
    use std::os::windows::process::CommandExt;
    let st = std::process::Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    Ok(st.code().unwrap_or(-1))
}

/// Startet ein Programm und laesst es laufen.
pub fn spawn(exe: &Path, args: &[&str]) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(args)
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", exe.display()))
}

/// Oeffnet eine Adresse oder Datei mit dem, was Windows dafuer vorsieht.
pub fn shell_open(target: &str) {
    let t = wide(target);
    unsafe {
        ShellExecuteW(None, windows::core::w!("open"), PCWSTR(t.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Loescht eine Datei, sobald der aufrufende Prozess weg ist – fuer die Kopie
/// des Deinstallierers, die sich nicht selbst loeschen kann, solange sie
/// laeuft. cmd.exe erledigt das im Hintergrund, ohne Fenster.
pub fn delete_after_exit(path: &Path) {
    use std::os::windows::process::CommandExt;
    let pid = std::process::id();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // Ein Stapelskript, weil `goto` nur dort funktioniert. Es wartet, bis
    // unser Prozess (PID und Name) verschwunden ist, loescht die Datei und
    // sich selbst – derselbe Kniff wie in src/update.rs des Browsers.
    let script = std::env::temp_dir().join(format!("aura-cleanup-{pid}.cmd"));
    let body = format!(
        "@echo off\r\n\
         :w\r\n\
         tasklist /FI \"PID eq {pid}\" 2>nul | find /I \"{name}\" >nul\r\n\
         if not errorlevel 1 (\r\n\
           timeout /t 1 /nobreak >nul\r\n\
           goto w\r\n\
         )\r\n\
         del /q \"{path}\"\r\n\
         (goto) 2>nul & del \"%~f0\"\r\n",
        path = path.display(),
    );
    if std::fs::write(&script, body).is_err() {
        return;
    }
    let _ = std::process::Command::new("cmd.exe")
        .args(["/c", &script.to_string_lossy()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

// ---------------------------------------------------------------- Konsole

/// Haengt sich an die Konsole des Aufrufers, falls es eine gibt. Ein Programm
/// mit Fenster-Subsystem hat sonst keine – und `pack` soll etwas sagen.
pub fn attach_console() -> bool {
    use windows::Win32::System::Console::*;
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS).is_ok() }
}

pub fn console_print(text: &str) {
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::Console::*;
    unsafe {
        // Geerbte Standardausgabe zuerst: eine Pipe bekommt UTF-8, eine
        // Konsole die Zeichen direkt.
        if let Ok(h) = GetStdHandle(STD_OUTPUT_HANDLE) {
            if !h.is_invalid() && !h.0.is_null() {
                let mut mode = CONSOLE_MODE(0);
                if GetConsoleMode(h, &mut mode).is_ok() {
                    let w: Vec<u16> = text.encode_utf16().collect();
                    let _ = WriteConsoleW(h, &w, None, None);
                } else {
                    let _ = WriteFile(h, Some(text.as_bytes()), None, None);
                }
                return;
            }
        }
        // Sonst die Konsole des Aufrufers, an die wir uns gehaengt haben.
        let Ok(h) = CreateFileW(
            windows::core::w!("CONOUT$"),
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        ) else {
            return;
        };
        let w: Vec<u16> = text.encode_utf16().collect();
        let _ = WriteConsoleW(h, &w, None, None);
        let _ = CloseHandle(h);
    }
}

// ---------------------------------------------------------------- Sonstiges

pub fn ui_is_german() -> bool {
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    (lang & 0x3FF) == 0x07
}

pub fn message_box(title: &str, text: &str, error: bool) {
    let t = wide(title);
    let m = wide(text);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(m.as_ptr()),
            PCWSTR(t.as_ptr()),
            if error { MB_ICONERROR | MB_OK } else { MB_ICONINFORMATION | MB_OK },
        );
    }
}

/// Ist die WebView2-Laufzeit da? Ohne sie zeigt Aura keine Seite. Windows 11
/// bringt sie mit; auf Windows 10 haengt es davon ab, ob Edge je aktualisiert
/// wurde. Die Schluessel sind die von Microsoft dokumentierten.
pub fn webview2_present() -> bool {
    const CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let places = [
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}")),
        (HKEY_LOCAL_MACHINE, format!("SOFTWARE\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}")),
        (HKEY_CURRENT_USER, format!("Software\\Microsoft\\EdgeUpdate\\Clients\\{CLIENT}")),
    ];
    places.iter().any(|(root, sub)| {
        matches!(reg_str(*root, sub, "pv"), Some(v) if !v.is_empty() && v != "0.0.0.0")
    })
}

// ---------------------------------------------------------------- Download

/// Laedt `https://host/path` nach `to` und meldet (geladen, gesamt).
/// WinHTTP folgt Weiterleitungen von selbst – wichtig fuer den fwlink von
/// Microsoft, hinter dem die WebView2-Laufzeit liegt.
pub fn download(host: &str, path: &str, to: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String> {
    use std::io::Write;
    use windows::Win32::Networking::WinHttp::*;
    unsafe {
        let session = WinHttpOpen(
            windows::core::w!("Aura Browser Setup"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        );
        if session.is_null() {
            return Err("WinHTTP nicht verfuegbar".into());
        }
        let _ = WinHttpSetTimeouts(session, 15_000, 15_000, 30_000, 30_000);
        let h = wide(host);
        let conn = WinHttpConnect(session, PCWSTR(h.as_ptr()), 443, 0);
        if conn.is_null() {
            let _ = WinHttpCloseHandle(session);
            return Err(format!("Keine Verbindung zu {host}"));
        }
        let p = wide(path);
        let req = WinHttpOpenRequest(
            conn,
            windows::core::w!("GET"),
            PCWSTR(p.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        let close_all = |req: *mut core::ffi::c_void| {
            if !req.is_null() {
                let _ = WinHttpCloseHandle(req);
            }
            let _ = WinHttpCloseHandle(conn);
            let _ = WinHttpCloseHandle(session);
        };
        if req.is_null() {
            close_all(req);
            return Err("Anfrage liess sich nicht anlegen".into());
        }
        let result: Result<(), String> = (|| {
            WinHttpSendRequest(req, None, None, 0, 0, 0).map_err(|e| format!("Senden: {e}"))?;
            WinHttpReceiveResponse(req, std::ptr::null_mut()).map_err(|e| format!("Antwort: {e}"))?;

            let mut status: u32 = 0;
            let mut len = 4u32;
            let _ = WinHttpQueryHeaders(
                req,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut status as *mut u32 as *mut _),
                &mut len,
                std::ptr::null_mut(),
            );
            if status != 200 {
                return Err(format!("Server antwortet mit HTTP {status}"));
            }
            let mut total: u32 = 0;
            let mut len = 4u32;
            let has_total = WinHttpQueryHeaders(
                req,
                WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some(&mut total as *mut u32 as *mut _),
                &mut len,
                std::ptr::null_mut(),
            )
            .is_ok();
            let total = if has_total && total > 0 { Some(total as u64) } else { None };

            let mut file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
            let mut got: u64 = 0;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut avail: u32 = 0;
                WinHttpQueryDataAvailable(req, &mut avail).map_err(|e| format!("Lesen: {e}"))?;
                if avail == 0 {
                    break;
                }
                let want = (avail as usize).min(buf.len()) as u32;
                let mut read: u32 = 0;
                WinHttpReadData(req, buf.as_mut_ptr() as *mut _, want, &mut read)
                    .map_err(|e| format!("Lesen: {e}"))?;
                if read == 0 {
                    break;
                }
                file.write_all(&buf[..read as usize]).map_err(|e| format!("Schreiben: {e}"))?;
                got += read as u64;
                progress(got, total);
            }
            if got == 0 {
                return Err("Leere Antwort".into());
            }
            Ok(())
        })();
        close_all(req);
        result
    }
}
