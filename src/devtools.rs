// Fernsteuerung für Aura – ein kleiner HTTP-Server auf 127.0.0.1, über den sich
// der Browser vollständig steuern und untersuchen lässt: Zustand abfragen,
// navigieren, klicken, tippen, JavaScript ausführen, das DevTools-Protokoll
// ansprechen und Screenshots ziehen.
//
// Standardmäßig aus. Einschalten mit `--debug-port=PORT` oder der Einstellung
// `debug_server`. Der Server hört nur auf localhost und verlangt ein Token, das
// beim Start in `devcontrol.json` im Profilordner landet.
//
// Alles, was WebView2 oder Direct2D berührt, muss auf den UI-Thread. Der
// Netzwerk-Thread legt den Befehl deshalb ab, weckt das Fenster und wartet.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

pub struct Request {
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Value,
}

pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(v: Value) -> Response {
        Response {
            status: 200,
            content_type: "application/json; charset=utf-8",
            body: v.to_string().into_bytes(),
        }
    }
    pub fn png(bytes: Vec<u8>) -> Response {
        Response { status: 200, content_type: "image/png", body: bytes }
    }
    pub fn error(status: u16, msg: &str) -> Response {
        Response {
            status,
            content_type: "application/json; charset=utf-8",
            body: json!({ "error": msg }).to_string().into_bytes(),
        }
    }
}

/// Übergabepunkt zwischen Netzwerk- und UI-Thread.
struct Slot {
    request: Option<Request>,
    response: Option<Response>,
}

static SLOT: Mutex<Option<Slot>> = Mutex::new(None);
static READY: Condvar = Condvar::new();
/// Startet den Server. Gibt den tatsächlich belegten Port zurück.
pub fn start(hwnd: isize, msg: u32, port: u16, profile_dir: &std::path::Path) -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port)).ok()?;
    let port = listener.local_addr().ok()?.port();

    // Das Token ist die einzige Schranke – jeder Prozess auf dem Rechner kommt
    // an 127.0.0.1 heran. Deshalb 256 Bit aus dem System-Zufall, nicht Zeit
    // und PID (beides ist erratbar).
    let mut bytes = [0u8; 32];
    unsafe {
        use windows::Win32::Security::Cryptography::*;
        if BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG).is_err() {
            return None;
        }
    }
    let secret: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let info = json!({ "port": port, "token": secret, "pid": std::process::id() });
    let _ = std::fs::write(profile_dir.join("devcontrol.json"), info.to_string());

    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let secret = secret.clone();
            let port = port;
            // Ein Thread je Verbindung, aber nicht beliebig viele: die
            // Anfragen laufen ohnehin nacheinander durch den UI-Thread.
            if ACTIVE.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                ACTIVE.fetch_sub(1, Ordering::SeqCst);
                let _ = respond(&stream, &Response::error(503, "zu viele Verbindungen"));
                continue;
            }
            let spawned = std::thread::Builder::new()
                .name("aura-devtools".into())
                .spawn(move || {
                    let _ = serve(stream, hwnd, msg, &secret, port);
                    ACTIVE.fetch_sub(1, Ordering::SeqCst);
                });
            if spawned.is_err() {
                ACTIVE.fetch_sub(1, Ordering::SeqCst);
            }
        }
    });
    Some(port)
}

static ACTIVE: AtomicUsize = AtomicUsize::new(0);
const MAX_CONNECTIONS: usize = 16;
/// Anfragen kommen von einem Skript nebenan; wer länger braucht, hängt.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
/// Kopfzeilen zusammen höchstens so lang, Rumpf höchstens so groß.
const MAX_HEAD: u64 = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;

fn serve(stream: TcpStream, hwnd: isize, msg: u32, secret: &str, port: u16) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    // Der Kopf wird durch einen begrenzten Leser gezogen – eine endlose
    // Kopfzeile kann so keinen Speicher fressen.
    let mut reader = BufReader::new(stream.try_clone()?.take(MAX_HEAD));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut length = 0usize;
    let mut header_token = String::new();
    let mut host_ok = false;
    for _ in 0..64 {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        let lower = h.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(usize::MAX);
        } else if lower.starts_with("x-aura-token:") {
            header_token = h[13..].trim().to_string();
        } else if let Some(v) = lower.strip_prefix("host:") {
            // Gegen DNS-Rebinding: Anfragen von einer Web-Seite tragen den
            // Namen des Angreifers im Host – wir bedienen nur die eigene Adresse.
            let v = v.trim();
            host_ok = v == format!("127.0.0.1:{port}") || v == format!("localhost:{port}");
        }
    }

    // Erst prüfen, dann lesen: ohne Token wird kein Rumpf angenommen, und
    // gross darf er auch nicht sein.
    let response = if !host_ok {
        Response::error(403, "falscher Host")
    } else if !constant_time_eq(header_token.as_bytes(), secret.as_bytes()) {
        Response::error(403, "falsches oder fehlendes Token")
    } else if length > MAX_BODY {
        Response::error(413, "Anfrage zu gross")
    } else {
        let mut raw = vec![0u8; length];
        if length > 0 {
            reader.get_mut().set_limit(length as u64);
            reader.read_exact(&mut raw)?;
        }
        let (path, query) = split_target(&target);
        // Alles, was den Zustand ändert, nur per POST – ein <img src> oder
        // Link aus einer Seite soll nichts auslösen können.
        if method != "POST" && path != "/state" {
            Response::error(405, "nur POST")
        } else {
            let body = serde_json::from_slice(&raw).unwrap_or(Value::Null);
            dispatch(Request { path, query, body }, hwnd, msg)
        }
    };
    respond(&stream, &response)
}

fn respond(mut stream: &TcpStream, response: &Response) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

/// Vergleich ohne frühen Abbruch – die Dauer verrät nichts über die Stelle,
/// an der es nicht mehr passt.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn split_target(target: &str) -> (String, Vec<(String, String)>) {
    match target.split_once('?') {
        None => (target.to_string(), vec![]),
        Some((p, q)) => {
            let params = q
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|kv| match kv.split_once('=') {
                    Some((k, v)) => (k.to_string(), urldecode(v)),
                    None => (kv.to_string(), String::new()),
                })
                .collect();
            (p.to_string(), params)
        }
    }
}

fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Reicht den Befehl an den UI-Thread weiter und wartet auf das Ergebnis.
fn dispatch(req: Request, hwnd: isize, msg: u32) -> Response {
    {
        let mut slot = match SLOT.lock() {
            Ok(s) => s,
            Err(_) => return Response::error(500, "interner Fehler"),
        };
        // Immer nur ein Befehl gleichzeitig – der Browser ist einthreadig.
        while slot.is_some() {
            slot = match READY.wait(slot) {
                Ok(s) => s,
                Err(_) => return Response::error(500, "interner Fehler"),
            };
        }
        *slot = Some(Slot { request: Some(req), response: None });
    }
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            Some(windows::Win32::Foundation::HWND(hwnd as *mut core::ffi::c_void)),
            msg,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(0),
        );
    }
    let deadline = std::time::Duration::from_secs(20);
    let mut slot = match SLOT.lock() {
        Ok(s) => s,
        Err(_) => return Response::error(500, "interner Fehler"),
    };
    loop {
        let done = slot.as_ref().map(|s| s.response.is_some()).unwrap_or(true);
        if done {
            break;
        }
        let (guard, timeout) = match READY.wait_timeout(slot, deadline) {
            Ok(v) => v,
            Err(_) => return Response::error(500, "interner Fehler"),
        };
        slot = guard;
        if timeout.timed_out() {
            *slot = None;
            READY.notify_all();
            return Response::error(504, "Zeitüberschreitung");
        }
    }
    let response = slot.as_mut().and_then(|s| s.response.take());
    *slot = None;
    READY.notify_all();
    response.unwrap_or_else(|| Response::error(500, "keine Antwort"))
}

/// Holt den wartenden Befehl ab – vom UI-Thread aus aufzurufen.
pub fn take_request() -> Option<Request> {
    let mut slot = SLOT.lock().ok()?;
    slot.as_mut()?.request.take()
}

/// Legt die Antwort ab und weckt den wartenden Netzwerk-Thread.
pub fn put_response(response: Response) {
    if let Ok(mut slot) = SLOT.lock() {
        if let Some(s) = slot.as_mut() {
            s.response = Some(response);
        }
    }
    READY.notify_all();
}

/// Base64 zurück in Bytes – für die Screenshots aus dem DevTools-Protokoll.
pub fn b64_decode(text: &str) -> Vec<u8> {
    let val = |c: u8| -> i32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as i32,
            b'a'..=b'z' => (c - b'a') as i32 + 26,
            b'0'..=b'9' => (c - b'0') as i32 + 52,
            b'+' => 62,
            b'/' => 63,
            _ => -1,
        }
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = val(c);
        if v < 0 {
            continue;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}
