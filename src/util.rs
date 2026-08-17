// Small Win32 helpers shared across modules.
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn pwstr(s: &str) -> (Vec<u16>, PCWSTR) {
    let v = wide(s);
    let p = PCWSTR(v.as_ptr());
    (v, p)
}

pub fn x_lparam(lp: LPARAM) -> i32 {
    ((lp.0 as isize & 0xFFFF) as i16) as i32
}

pub fn y_lparam(lp: LPARAM) -> i32 {
    (((lp.0 as isize >> 16) & 0xFFFF) as i16) as i32
}

pub fn client_rect(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    r
}

pub fn cursor_pos() -> POINT {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}

pub fn dpi_scale(hwnd: HWND) -> f32 {
    unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as f32 / 96.0 }
}

/// Extract domain/host from an URL for display purposes.
pub fn host_of(url: &str) -> String {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("aura://"))
        .unwrap_or(url);
    let end = rest
        .find(|c| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

/// Returns true if the input looks like a navigable URL or domain.
pub fn looks_like_url(input: &str) -> bool {
    if input.starts_with("http://") || input.starts_with("https://") || input.starts_with("aura://")
        || input.starts_with("file://") || input.starts_with("about:")
    {
        return true;
    }
    if input.contains(' ') || input.is_empty() {
        return false;
    }
    // domain-like: something.tld
    if let Some(dot) = input.rfind('.') {
        let tld = &input[dot + 1..];
        let before = &input[..dot];
        return tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphanumeric()) && !before.is_empty();
    }
    false
}

/// Percent-encodes everything outside the unreserved URL set.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Löst %XX-Folgen auf; ungültige Bytes werden ersetzt statt zu scheitern.
pub fn urldecode_lossy(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn error_box(msg: &str) {
    unsafe {
        let (_t, tp) = pwstr("Aura Browser");
        let (_m, mp) = pwstr(msg);
        let _ = MessageBoxW(None, mp, tp, MB_ICONERROR | MB_OK);
    }
}

pub fn copy_to_clipboard(hwnd: HWND, text: &str) {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    let w = wide(text);
    let bytes = w.len() * 2;
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let ptr = GlobalLock(h) as *mut u16;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), ptr, w.len());
                let _ = GlobalUnlock(h);
                let _ = SetClipboardData(
                    windows::Win32::System::Ole::CF_UNICODETEXT.0 as u32,
                    Some(windows::Win32::Foundation::HANDLE(h.0 as *mut core::ffi::c_void)),
                );
            }
        }
        let _ = CloseClipboard();
    }
}


pub fn send_msg(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
    unsafe { SendMessageW(hwnd, msg, Some(w), Some(l)).0 }
}
