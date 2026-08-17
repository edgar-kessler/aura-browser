// Omnibox: suggestion model with strict ranking:
// 1. Bookmarks  2. Open tabs  3. History  4. Direct URL  5. Web search
// Plus command mode (">" prefix) for the command palette.
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::App;
use crate::popup::PopupKind;
use crate::util::*;

#[derive(Clone, Copy, PartialEq)]
pub enum SuggKind {
    Bookmark,
    Tab,
    History,
    Url,
    Search,
    Command,
}

impl SuggKind {
    pub fn label(&self) -> &'static str {
        match self {
            SuggKind::Bookmark => "Lesezeichen",
            SuggKind::Tab => "Geöffneter Tab",
            SuggKind::History => "Verlauf",
            SuggKind::Url => "Adresse",
            SuggKind::Search => "Suche",
            SuggKind::Command => "Befehl",
        }
    }
}

#[derive(Clone)]
pub struct Suggestion {
    pub kind: SuggKind,
    pub title: String,
    pub url: String,
    pub favicon: Option<Vec<u8>>,
    pub action: Option<String>,
}

pub fn edit_text(edit: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(edit) + 1;
        if len <= 1 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize];
        let n = GetWindowTextW(edit, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

pub fn search_url(app: &App, query: &str) -> String {
    let engine = app.storage.get_setting("search_engine", "google");
    // Alles ausser Leerzeichen prozentkodiert – ein "&" oder "#" in der
    // Suche darf die Adresse nicht umbauen.
    let q = crate::util::urlencode(query).replace("%20", "+");
    match engine.as_str() {
        "bing" => format!("https://www.bing.com/search?q={q}"),
        "duckduckgo" => format!("https://duckduckgo.com/?q={q}"),
        "ecosia" => format!("https://www.ecosia.org/search?q={q}"),
        "brave" => format!("https://search.brave.com/search?q={q}"),
        _ => format!("https://www.google.com/search?q={q}"),
    }
}

/// Resolves raw omnibox input into a navigable URL.
pub fn resolve_input(app: &App, input: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return String::new();
    }
    if input.starts_with("aura://") {
        return input.to_string();
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        return input.to_string();
    }
    if looks_like_url(input) {
        return format!("https://{input}");
    }
    search_url(app, input)
}

const COMMANDS: &[(&str, &str, &str)] = &[
    ("Neuer Tab", "Öffnet einen neuen Tab", "newtab"),
    ("Neues Fenster", "Öffnet ein neues Aura-Fenster", "newwindow"),
    ("Privates Fenster", "Öffnet ein privates Fenster", "private"),
    ("Geteilte Ansicht umschalten", "Zwei Seiten nebeneinander", "split"),
    ("Bild-in-Bild", "Video im schwebenden Fenster", "pip"),
    ("Vollbild umschalten", "F11", "fullscreen"),
    ("Verlauf öffnen", "aura://history", "history"),
    ("Lesezeichen öffnen", "aura://bookmarks", "bookmarks"),
    ("Downloads öffnen", "aura://downloads", "downloads"),
    ("Einstellungen öffnen", "aura://settings", "settings"),
    ("Aus Chrome importieren", "Lesezeichen, Verlauf, Passwörter", "import"),
    ("Leseliste öffnen", "aura://reading", "reading"),
    ("Passwörter öffnen", "aura://passwords", "passwords"),
    ("Task-Manager öffnen", "Speicher pro Tab und Prozess", "tasks"),
    ("Später lesen", "Aktuelle Seite in die Leseliste", "read_later"),
    ("Seite übersetzen", "Über Google Übersetzer", "translate"),
    ("Tabs durchsuchen", "Strg+Umschalt+A", "tabsearch"),
    ("Cookies dieser Seite löschen", "Cookies und Speicher", "clear_site"),
    ("Strengen Modus umschalten", "Keine Drittanbieter auf dieser Seite", "shield_strict"),
    ("Geschlossenen Tab wiederherstellen", "Strg+Umschalt+T", "reopen"),
    ("Seite neu laden", "F5", "reload_page"),
    ("Aktuellen Tab schlafen legen", "Spart Ressourcen", "sleep_active"),
    ("Drucken", "Strg+P", "print"),
    ("Seite durchsuchen", "Strg+F", "find"),
    ("Zoom vergrößern", "Strg++", "zoomin"),
    ("Zoom verkleinern", "Strg+−", "zoomout"),
    ("Zoom zurücksetzen", "Strg+0", "zoomreset"),
];

pub fn build_suggestions(app: &App, query: &str) -> Vec<Suggestion> {
    let q = query.trim();
    let mut out: Vec<Suggestion> = Vec::new();

    // Command palette mode.
    if let Some(cmd) = q.strip_prefix('>') {
        let needle = cmd.trim().to_lowercase();
        for (name, desc, action) in COMMANDS {
            if needle.is_empty() || name.to_lowercase().contains(&needle) {
                out.push(Suggestion {
                    kind: SuggKind::Command,
                    title: name.to_string(),
                    url: desc.to_string(),
                    favicon: None,
                    action: Some(action.to_string()),
                });
            }
        }
        return out;
    }

    // Tab search mode: "@" lists every open tab, "@foo" filters them.
    if let Some(rest) = q.strip_prefix('@') {
        let needle = rest.trim().to_lowercase();
        for t in &app.tabs {
            let title = if t.title.is_empty() { t.domain() } else { t.title.clone() };
            if needle.is_empty()
                || title.to_lowercase().contains(&needle)
                || t.url.to_lowercase().contains(&needle)
            {
                out.push(Suggestion {
                    kind: SuggKind::Tab,
                    title,
                    url: t.url.clone(),
                    favicon: t.favicon_png.clone(),
                    action: None,
                });
            }
        }
        out.truncate(12);
        return out;
    }

    if q.is_empty() {
        // Fresh omnibox: top bookmarks + most visited.
        for b in app.storage.all_bookmarks().into_iter().filter(|b| !b.is_folder).take(4) {
            out.push(Suggestion {
                kind: SuggKind::Bookmark,
                title: b.title,
                url: b.url,
                favicon: b.favicon,
                action: None,
            });
        }
        for h in app.storage.top_history(4) {
            out.push(Suggestion {
                kind: SuggKind::History,
                title: h.title,
                url: h.url,
                favicon: h.favicon,
                action: None,
            });
        }
        return out;
    }

    let needle = q.to_lowercase();

    // 1. Bookmarks
    for b in app.storage.search_bookmarks(q, 4) {
        out.push(Suggestion {
            kind: SuggKind::Bookmark,
            title: b.title,
            url: b.url,
            favicon: b.favicon,
            action: None,
        });
    }

    // 2. Open tabs
    for t in &app.tabs {
        if t.url.to_lowercase().contains(&needle) || t.title.to_lowercase().contains(&needle) {
            out.push(Suggestion {
                kind: SuggKind::Tab,
                title: if t.title.is_empty() { t.domain() } else { t.title.clone() },
                url: t.url.clone(),
                favicon: t.favicon_png.clone(),
                action: None,
            });
        }
    }

    // 3. History
    for h in app.storage.search_history(q, 5) {
        out.push(Suggestion {
            kind: SuggKind::History,
            title: h.title,
            url: h.url,
            favicon: h.favicon,
            action: None,
        });
    }

    // 4. Direct URL
    if looks_like_url(q) {
        let url = if q.starts_with("http") || q.starts_with("aura://") {
            q.to_string()
        } else {
            format!("https://{q}")
        };
        out.push(Suggestion {
            kind: SuggKind::Url,
            title: format!("{q} öffnen"),
            url,
            favicon: None,
            action: None,
        });
    }

    // 5. Web search
    out.push(Suggestion {
        kind: SuggKind::Search,
        title: format!("„{q}“ suchen"),
        url: search_url(app, q),
        favicon: None,
        action: None,
    });

    out.truncate(9);
    out
}

pub fn navigate_suggestions(app: &mut App, delta: i32) {
    if let Some(p) = &mut app.sugg_popup {
        if let PopupKind::Suggestions { items, selected } = &mut p.kind {
            let n = items.len() as i32;
            if n == 0 {
                return;
            }
            let mut s = *selected as i32 + delta;
            if s < 0 {
                s = n - 1;
            }
            if s >= n {
                s = 0;
            }
            *selected = s as usize;
            p.repaint();
        }
    }
}
