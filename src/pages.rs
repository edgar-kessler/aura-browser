// Internal pages (aura://start, history, bookmarks, downloads, settings, import):
// builds JSON payloads, handles page commands, implements the Chrome import.
use serde_json::{json, Value};
use windows::core::PCWSTR;
use windows::Win32::UI::Shell::ShellExecuteW;

use crate::app::App;
use crate::storage::Bookmark;
use crate::theme::ThemeMode;
use crate::util::*;

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0] as u32,
            *chunk.get(1).unwrap_or(&0) as u32,
            *chunk.get(2).unwrap_or(&0) as u32,
        ];
        let n = (b[0] << 16) | (b[1] << 8) | b[2];
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn favicon_url(fav: &Option<Vec<u8>>) -> Value {
    match fav {
        Some(bytes) => json!(format!("data:image/png;base64,{}", b64(bytes))),
        None => Value::Null,
    }
}

fn theme_tokens(app: &App) -> Value {
    let (r, g, b) = app.theme.accent;
    json!({
        "dark": app.theme.dark,
        "accent": format!("{r},{g},{b}"),
        "reduce": app.theme.reduce_motion,
        "private": app.private,
        "profile": app.profile,
    })
}

pub fn send_init(app: &App, tab_id: u32) {
    let Some(tab) = app.tabs.iter().find(|t| t.id == tab_id) else { return };
    let Some(wv) = &tab.webview else { return };
    let page = tab.url.trim_start_matches("aura://").to_string();

    let data = match page.as_str() {
        "start" => {
            let tops: Vec<Value> = app
                .storage
                .top_history(8)
                .iter()
                .map(|h| json!({"title": h.title, "url": h.url, "favicon": favicon_url(&h.favicon)}))
                .collect();
            let bms: Vec<Value> = app
                .storage
                .all_bookmarks()
                .iter()
                .filter(|b| !b.is_folder)
                .take(8)
                .map(|b| json!({"title": b.title, "url": b.url, "favicon": favicon_url(&b.favicon)}))
                .collect();
            let (blocked, rules, cosmetic, _) = crate::adblock::stats();
            json!({
                "topsites": tops,
                "bookmarks": bms,
                "shield": {
                    "on": crate::adblock::is_enabled(),
                    "blocked": blocked,
                    "rules": rules,
                    "cosmetic": cosmetic,
                },
                "tabs": app.tabs.len(),
            })
        }
        "history" => history_payload(app, "", 0, 500),
        "bookmarks" => {
            let items: Vec<Value> = app
                .storage
                .all_bookmarks()
                .iter()
                .map(|b: &Bookmark| json!({"id": b.id, "parent": b.parent, "folder": b.is_folder, "title": b.title, "url": b.url, "favicon": favicon_url(&b.favicon)}))
                .collect();
            json!({"items": items})
        }
        "downloads" => {
            let items: Vec<Value> = app
                .storage
                .list_downloads()
                .iter()
                .map(|d| json!({"id": d.id, "path": d.path, "url": d.url, "filename": d.filename, "total": d.total_bytes, "received": d.received_bytes, "finished": d.finished, "started": d.started_at}))
                .collect();
            json!({"items": items})
        }
        "settings" => {
            let s = &app.storage;
            let (blocked, rules, cosmetic, lists) = crate::adblock::stats();
            let lists: Vec<Value> = lists
                .iter()
                .map(|(n, c)| json!({"name": n, "rules": c}))
                .collect();
            json!({
                "theme": s.get_setting("theme", "system"),
                "accent": s.get_setting("accent", "110,91,208"),
                "search_engine": s.get_setting("search_engine", "google"),
                "restore_session": s.get_setting("restore_session", "1"),
                "sleep_tabs": s.get_setting("sleep_tabs", "1"),
                "block_popups": s.get_setting("block_popups", "1"),
                "reduce_motion": s.get_setting("reduce_motion", "0"),
                "passwords": count_passwords(app),
                "homepage": s.get_setting("homepage", "aura://start"),
                "new_tab_page": s.get_setting("new_tab_page", "start"),
                "sleep_after_min": s.get_setting("sleep_after_min", "15"),
                "sidebar_hover": s.get_setting("sidebar_hover", "0"),
                "default_zoom": s.get_setting("default_zoom", "100"),
                "suggestions": s.get_setting("suggestions", "1"),
                "close_last_tab": s.get_setting("close_last_tab", "0"),
                "dnt": s.get_setting("dnt", "1"),
                "download_dir": s.get_setting("download_dir", ""),
                "shield": s.get_setting("shield", "1"),
                "shield_update": s.get_setting("shield_update", "1"),
                "shield_blocked": blocked,
                "shield_rules": rules,
                "shield_cosmetic": cosmetic,
                "shield_lists": lists,
                "shield_allow": crate::adblock::allowlist(),
                "shield_strict": crate::adblock::strict_sites(),
                "https_only": s.get_setting("https_only", "1"),
                "popup_strict": s.get_setting("popup_strict", "0"),
                "version": crate::update::VERSION,
                "repo": crate::update::REPO,
                "auto_update": s.get_setting("auto_update", "1"),
                "update_state": match app.update_state {
                    crate::app::UpdateState::Checking => "checking",
                    crate::app::UpdateState::Downloading => "downloading",
                    crate::app::UpdateState::Ready => "ready",
                    crate::app::UpdateState::Failed => "failed",
                    _ => "idle",
                },
                "update": app.pending_update.as_ref().map(|r| json!({
                    "version": r.version, "notes": r.notes,
                    "size": r.size, "name": r.asset_name,
                })),
                "glass": s.get_setting("glass", "1"),
                "tab_layout": s.get_setting("tab_layout", "side"),
                "glass_style": s.get_setting("glass_style", "acrylic"),
                // Warum Glas gerade (nicht) geht — die Seite formuliert daraus den Hinweis.
                "glass_active": app.glass,
                "glass_possible": app.glass_possible(),
                "effects": app.effects_on(),
                "sidebar_width": s.get_setting("sidebar_width", ""),
            })
        }
        "reading" => {
            let items: Vec<Value> = app
                .storage
                .reading_list()
                .iter()
                .map(|(url, title, fav, added, read)| {
                    json!({"url": url, "title": title, "favicon": favicon_url(fav),
                           "added": added, "read": read})
                })
                .collect();
            json!({"items": items})
        }
        "passwords" => {
            // Sicherheitsüberblick, ohne ein einziges Passwort an die Seite zu
            // geben: entschlüsselt wird nur hier im Prozess, die Seite bekommt
            // je Eintrag eine Stärke (0 schwach, 1 mittel, 2 stark) und ob das
            // Passwort mehrfach verwendet wird.
            let list = app.storage.password_list();
            let mut plains: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
            let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            for (id, _, _) in &list {
                if let Some(p) = app.storage.password_blob(*id).and_then(|b| dpapi_unprotect(&b)) {
                    *seen.entry(p.clone()).or_insert(0) += 1;
                    plains.insert(*id, p);
                }
            }
            let strength = |p: &str| -> u32 {
                let len = p.chars().count();
                let classes = [
                    p.chars().any(|c| c.is_ascii_lowercase()),
                    p.chars().any(|c| c.is_ascii_uppercase()),
                    p.chars().any(|c| c.is_ascii_digit()),
                    p.chars().any(|c| !c.is_ascii_alphanumeric()),
                ]
                .iter()
                .filter(|b| **b)
                .count();
                if len >= 12 && classes >= 3 {
                    2
                } else if len >= 8 && classes >= 2 {
                    1
                } else {
                    0
                }
            };
            let items: Vec<Value> = list
                .iter()
                .map(|(id, origin, user)| {
                    let (st, reused) = match plains.get(id) {
                        Some(p) => (strength(p) as i64, seen.get(p).copied().unwrap_or(1) > 1),
                        None => (-1, false),
                    };
                    json!({"id": id, "origin": origin, "user": user, "strength": st, "reused": reused})
                })
                .collect();
            let weak = items.iter().filter(|i| i["strength"] == 0).count();
            let reused = items.iter().filter(|i| i["reused"] == true).count();
            json!({"items": items, "weak": weak, "reused": reused})
        }
        "tasks" => task_payload(app),
        "import" => {
            let profiles: Vec<Value> = chrome_profiles()
                .iter()
                .map(|(dir, label)| json!({"dir": dir, "label": label}))
                .collect();
            json!({"profiles": profiles})
        }
        _ => json!({}),
    };

    let payload = json!({"type": "init", "page": page, "theme": theme_tokens(app), "data": data});
    let w = wide(&payload.to_string());
    unsafe {
        let _ = wv.PostWebMessageAsJson(PCWSTR(w.as_ptr()));
    }
}

/// Task manager: every tab with its state, plus the engine's processes.
fn task_payload(app: &App) -> Value {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use windows::core::Interface;

    let tabs: Vec<Value> = app
        .tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            json!({
                "index": i,
                "id": t.id,
                "title": if t.title.is_empty() { t.domain() } else { t.title.clone() },
                "url": t.url,
                "favicon": favicon_url(&t.favicon_png),
                "active": i == app.active,
                "loaded": t.controller.is_some(),
                "asleep": t.asleep,
                "audio": t.playing_audio,
                "pinned": t.pinned,
                "blocked": crate::adblock::blocked_for(t.id),
            })
        })
        .collect();

    // Process list straight from the WebView2 environment.
    let mut procs: Vec<Value> = vec![];
    if let Some(env) = &app.env {
        if let Ok(env8) = env.cast::<ICoreWebView2Environment8>() {
            if let Ok(list) = unsafe { env8.GetProcessInfos() } {
                let mut count = 0u32;
                let _ = unsafe { list.Count(&mut count) };
                for i in 0..count {
                    let Ok(info) = (unsafe { list.GetValueAtIndex(i) }) else { continue };
                    let mut pid = 0i32;
                    let mut kind = COREWEBVIEW2_PROCESS_KIND_BROWSER;
                    unsafe {
                        let _ = info.ProcessId(&mut pid);
                        let _ = info.Kind(&mut kind);
                    }
                    procs.push(json!({
                        "pid": pid,
                        "kind": process_kind_name(kind.0),
                        "mb": process_memory_mb(pid as u32),
                    }));
                }
            }
        }
    }
    let own = std::process::id();
    procs.insert(
        0,
        json!({"pid": own, "kind": "Aura (Oberfläche)", "mb": process_memory_mb(own)}),
    );
    let total: f64 = procs
        .iter()
        .filter_map(|p| p.get("mb").and_then(|m| m.as_f64()))
        .sum();
    json!({"tabs": tabs, "procs": procs, "total_mb": (total * 10.0).round() / 10.0})
}

fn process_kind_name(kind: i32) -> &'static str {
    match kind {
        0 => "Engine (Browser)",
        1 => "Renderer",
        2 => "Utility",
        3 => "Sandbox-Helfer",
        4 => "GPU",
        5 => "PPAPI-Plugin",
        6 => "PPAPI-Broker",
        _ => "Sonstiges",
    }
}

/// Working set of a process in MB.
fn process_memory_mb(pid: u32) -> f64 {
    use windows::Win32::System::ProcessStatus::*;
    use windows::Win32::System::Threading::*;
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return 0.0;
        };
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        let ok = K32GetProcessMemoryInfo(
            h,
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .as_bool();
        let _ = windows::Win32::Foundation::CloseHandle(h);
        if ok {
            (counters.WorkingSetSize as f64 / 1048576.0 * 10.0).round() / 10.0
        } else {
            0.0
        }
    }
}

/// History slice for the page: filtered, time-ranged, with totals.
fn history_payload(app: &App, query: &str, since: i64, limit: usize) -> Value {
    let items: Vec<Value> = app
        .storage
        .history_page(query, since, limit)
        .iter()
        .map(|h| {
            json!({
                "title": h.title,
                "url": h.url,
                "last": h.last_visit,
                "visits": h.visit_count,
                "favicon": favicon_url(&h.favicon),
            })
        })
        .collect();
    json!({
        "items": items,
        "total": app.storage.history_count(),
        "query": query,
        "since": since,
        "limit": limit,
    })
}

fn count_passwords(app: &App) -> i64 {
    app.storage
        .conn
        .query_row("SELECT COUNT(*) FROM passwords", [], |r| r.get(0))
        .unwrap_or(0)
}

pub fn handle_message(app: &mut App, tab_id: u32, json_str: &str) {
    // Zweite Sicherung neben der Herkunftsprüfung in tabs.rs: Befehle gibt es
    // nur, solange oben im Tab eine unserer Seiten steht. Die Antworten gehen
    // an das oberste Dokument – das darf kein fremdes sein.
    if !app.tabs.iter().any(|t| t.id == tab_id && t.is_internal) {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(json_str) else { return };
    let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
    match cmd {
        "open" => {
            if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
                if let Some(i) = app.tabs.iter().position(|t| t.id == tab_id) {
                    let url = url.to_string();
                    app.navigate(i, &url);
                }
            }
        }
        "open_new" => {
            if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
                app.new_tab(url, true);
            }
        }
        "focus_omnibox" => app.begin_edit(),
        "search" => {
            if let Some(q) = v.get("q").and_then(|q| q.as_str()) {
                let url = crate::omnibox::resolve_input(app, q);
                if !url.is_empty() {
                    if let Some(i) = app.tabs.iter().position(|t| t.id == tab_id) {
                        app.navigate(i, &url);
                    }
                }
            }
        }
        "save_setting" => {
            let key = v.get("key").and_then(|k| k.as_str()).unwrap_or("");
            let value = v.get("value").and_then(|k| k.as_str()).unwrap_or("");
            app.storage.set_setting(key, value);
            match key {
                "theme" => {
                    let mode = match value {
                        "light" => ThemeMode::Light,
                        "dark" => ThemeMode::Dark,
                        _ => ThemeMode::System,
                    };
                    app.apply_theme(mode);
                }
                "accent" => {
                    let accent = crate::app::parse_accent(value);
                    let mode = app.theme_mode;
                    let rm = app.theme.reduce_motion;
                    app.theme = crate::theme::Theme::new(mode, accent, rm);
                    if app.glass {
                        app.theme.glassify();
                    }
                    app.paint();
                }
                "reduce_motion" => {
                    app.theme.reduce_motion = value == "1";
                }
                "shield" => {
                    crate::adblock::set_enabled(value == "1");
                    app.paint();
                }
                "dnt" => crate::adblock::set_dnt(value == "1"),
                "https_only" => crate::adblock::set_https_only(value == "1"),
                "popup_strict" => crate::adblock::set_strict_popups(value == "1"),
                "tab_layout" => {
                    // Sofort umschalten: Kopfflaeche und Inhaltskante aendern sich.
                    app.top_tabs = value == "top";
                    app.ind_ready = false;
                    app.tab_scroll = 0.0;
                    app.tab_scroll_target = 0.0;
                    app.relayout();
                    app.paint();
                }
                "glass" | "glass_style" => {
                    // Sofort: Systemhintergrund und Alphawerte lassen sich im
                    // laufenden Betrieb umstellen, die Kompositionsfläche hat
                    // das Fenster ohnehin.
                    let wanted = app.storage.get_setting("glass", "1") == "1";
                    let style = app.storage.get_setting("glass_style", "acrylic");
                    app.set_glass(wanted, &style);
                }
                "sidebar_hover" => app.set_sidebar_hover(value == "1"),
                "default_zoom" => {
                    if let Ok(z) = value.parse::<f64>() {
                        app.zoom_to(z.clamp(50.0, 250.0) / 100.0);
                    }
                }
                _ => {}
            }
            refresh_settings_pages(app);
        }
        "shield_update" => {
            app.update_filters_now();
        }
        "shield_unallow" => {
            if let Some(site) = v.get("site").and_then(|s| s.as_str()) {
                crate::adblock::toggle_site(site);
                let list = crate::adblock::allowlist().join(",");
                app.storage.set_setting("shield_allow", &list);
                refresh_settings_pages(app);
            }
        }
        "remove_bookmark" => {
            if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
                app.storage.remove_bookmark(id);
                refresh_page(app, "bookmarks");
            }
        }
        "add_folder" => {
            if let Some(title) = v.get("title").and_then(|t| t.as_str()) {
                if !title.trim().is_empty() {
                    app.storage.add_bookmark_folder(title.trim(), 0);
                    refresh_page(app, "bookmarks");
                }
            }
        }
        "clear_history" => {
            app.storage.clear_history();
            refresh_page(app, "history");
        }
        "history_query" => {
            let q = v.get("q").and_then(|q| q.as_str()).unwrap_or("");
            let since = v.get("since").and_then(|s| s.as_i64()).unwrap_or(0);
            let limit = v.get("limit").and_then(|l| l.as_u64()).unwrap_or(500) as usize;
            let data = history_payload(app, q, since, limit.min(5000));
            send_to_page(app, tab_id, &json!({"type": "init", "page": "history", "theme": theme_tokens(app), "data": data}));
        }
        "history_remove" => {
            if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
                app.storage.remove_history_url(url);
            }
            if let Some(urls) = v.get("urls").and_then(|u| u.as_array()) {
                for u in urls.iter().filter_map(|u| u.as_str()) {
                    app.storage.remove_history_url(u);
                }
            }
        }
        "history_remove_since" => {
            let since = v.get("since").and_then(|s| s.as_i64()).unwrap_or(0);
            app.storage.remove_history_since(since);
            refresh_page(app, "history");
        }
        "clear_all_data" => app.clear_site_data(""),
        // ---------- reading list ----------
        "reading_add" => {
            if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
                let title = v.get("title").and_then(|t| t.as_str()).unwrap_or(url);
                app.storage.reading_add(url, title, None);
                refresh_page(app, "reading");
            }
        }
        "reading_remove" => {
            if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
                app.storage.reading_remove(url);
            }
        }
        "reading_read" => {
            if let Some(url) = v.get("url").and_then(|u| u.as_str()) {
                let read = v.get("read").and_then(|r| r.as_bool()).unwrap_or(true);
                app.storage.reading_set_read(url, read);
            }
        }
        // ---------- passwords ----------
        "password_reveal" => {
            if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
                let secret = app
                    .storage
                    .password_blob(id)
                    .and_then(|b| dpapi_unprotect(&b))
                    .unwrap_or_else(|| "— nicht entschlüsselbar —".into());
                send_to_page(app, tab_id, &json!({"type": "password", "id": id, "value": secret}));
            }
        }
        "password_remove" => {
            if let Some(id) = v.get("id").and_then(|i| i.as_i64()) {
                app.storage.password_remove(id);
                refresh_page(app, "passwords");
            }
        }
        // ---------- task manager ----------
        // ---------- updates ----------
        "update_check" => app.check_update(),
        "update_install" => app.install_update(),
        "update_restart" => app.restart_for_update(),
        "tasks_refresh" => {
            let data = task_payload(app);
            send_to_page(app, tab_id, &json!({"type": "init", "page": "tasks", "theme": theme_tokens(app), "data": data}));
        }
        "tab_activate" => {
            if let Some(i) = v.get("index").and_then(|i| i.as_u64()) {
                app.activate_tab(i as usize);
            }
        }
        "tab_close" => {
            if let Some(i) = v.get("index").and_then(|i| i.as_u64()) {
                app.close_tab(i as usize);
                let data = task_payload(app);
                send_to_page(app, tab_id, &json!({"type": "init", "page": "tasks", "theme": theme_tokens(app), "data": data}));
            }
        }
        "open_download" | "show_in_folder" => {
            if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                let path = path.to_string();
                if cmd == "open_download" {
                    let p = wide(&path);
                    unsafe {
                        let _ = ShellExecuteW(None, wstr("open"), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), windows::Win32::UI::WindowsAndMessaging::SW_SHOW);
                    }
                } else {
                    let _ = std::process::Command::new("explorer.exe")
                        .arg(format!("/select,{path}"))
                        .spawn();
                }
            }
        }
        "import" => {
            let bookmarks = v.get("bookmarks").and_then(|b| b.as_bool()).unwrap_or(true);
            let history = v.get("history").and_then(|b| b.as_bool()).unwrap_or(true);
            let passwords = v.get("passwords").and_then(|b| b.as_bool()).unwrap_or(false);
            let csv = v.get("csv").and_then(|c| c.as_str()).map(|s| s.to_string());
            let profile = v.get("profile").and_then(|p| p.as_str()).unwrap_or("Default");
            let result = chrome_import(app, bookmarks, history, passwords, csv, profile);
            send_to_page(app, tab_id, &json!({"type": "import_result", "text": result}));
            refresh_page(app, "bookmarks");
            refresh_page(app, "history");
            refresh_page(app, "start");
        }
        "pick_csv" => {
            if let Some(path) = pick_file(app.hwnd) {
                send_to_page(app, tab_id, &json!({"type": "csv_picked", "path": path}));
            }
        }
        "pick_download_dir" => {
            if let Some(path) = pick_folder(app.hwnd) {
                app.storage.set_setting("download_dir", &path);
                refresh_settings_pages(app);
            }
        }
        _ => {}
    }
}

fn wstr(s: &str) -> PCWSTR {
    // leaked small string; used rarely (ShellExecute verb)
    let v: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let b = v.into_boxed_slice();
    PCWSTR(Box::leak(b).as_ptr())
}

fn send_to_page(app: &App, tab_id: u32, payload: &Value) {
    if let Some(tab) = app.tabs.iter().find(|t| t.id == tab_id) {
        if let Some(wv) = &tab.webview {
            let w = wide(&payload.to_string());
            unsafe {
                let _ = wv.PostWebMessageAsJson(PCWSTR(w.as_ptr()));
            }
        }
    }
}

fn refresh_page(app: &App, page: &str) {
    let url = format!("aura://{page}");
    let ids: Vec<u32> = app.tabs.iter().filter(|t| t.url == url).map(|t| t.id).collect();
    for id in ids {
        send_init(app, id);
    }
}

fn refresh_settings_pages(app: &App) {
    refresh_page(app, "settings");
    refresh_page(app, "start");
}

/// Pushes fresh update state to any open settings page.
pub fn refresh_about_pages(app: &App) {
    refresh_page(app, "settings");
}

fn pick_file(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::UI::Controls::Dialogs::*;
    let mut buf = vec![0u16; 1024];
    let filter = wide("CSV-Dateien\0*.csv\0Alle Dateien\0*.*\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: hwnd,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: wstr("Chrome-Passwort-CSV auswählen"),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    unsafe {
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            return Some(String::from_utf16_lossy(&buf[..end]));
        }
    }
    None
}

/// Folder picker (IFileOpenDialog with FOS_PICKFOLDERS).
fn pick_folder(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_PATHMUSTEXIST | FOS_FORCEFILESYSTEM)
            .ok()?;
        dlg.SetTitle(wstr("Download-Ordner wählen")).ok()?;
        if dlg.Show(Some(hwnd)).is_err() {
            return None;
        }
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok()?;
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        Some(s)
    }
}

// ---------------- Chrome import ----------------
fn chrome_root() -> Option<std::path::PathBuf> {
    let base = std::env::var("LOCALAPPDATA").ok()?;
    let dir = std::path::PathBuf::from(base)
        .join("Google")
        .join("Chrome")
        .join("User Data");
    dir.exists().then_some(dir)
}

/// All Chrome profiles with their display names, newest activity first.
pub fn chrome_profiles() -> Vec<(String, String)> {
    let Some(root) = chrome_root() else { return vec![] };
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(&root) else { return out };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name != "Default" && !name.starts_with("Profile ") {
            continue;
        }
        if !entry.path().join("Bookmarks").exists() && !entry.path().join("History").exists() {
            continue;
        }
        // The human-readable name lives in Preferences → profile.name.
        let label = std::fs::read_to_string(entry.path().join("Preferences"))
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| {
                v.get("profile")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| name.clone());
        out.push((name, label));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn chrome_dir(profile: &str) -> Option<std::path::PathBuf> {
    // Chrome nennt seine Profile "Default" und "Profile N" – mehr Zeichen
    // braucht der Name nicht, und ein Pfad darf er nie werden.
    let clean: String = profile
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
        .take(64)
        .collect();
    let p = if clean.trim().is_empty() { "Default".to_string() } else { clean };
    let dir = chrome_root()?.join(p);
    dir.exists().then_some(dir)
}

fn chrome_import(
    app: &App,
    bookmarks: bool,
    history: bool,
    passwords: bool,
    csv: Option<String>,
    profile: &str,
) -> String {
    let mut report: Vec<String> = Vec::new();
    let Some(dir) = chrome_dir(profile) else {
        return "Chrome wurde auf diesem System nicht gefunden.".to_string();
    };

    if bookmarks {
        match import_chrome_bookmarks(app, &dir) {
            Ok(n) => report.push(format!("{n} Lesezeichen importiert")),
            Err(e) => report.push(format!("Lesezeichen: {e}")),
        }
    }
    if history {
        match import_chrome_history(app, &dir) {
            Ok(n) => report.push(format!("{n} Verlaufseinträge importiert")),
            Err(e) => report.push(format!("Verlauf: {e}")),
        }
    }
    if passwords {
        match csv {
            Some(path) => match import_password_csv(app, &path) {
                Ok(n) => report.push(format!("{n} Passwörter importiert (DPAPI-verschlüsselt gespeichert)")),
                Err(e) => report.push(format!("Passwörter: {e}")),
            },
            None => report.push("Passwörter: keine CSV-Datei ausgewählt (in Chrome: Einstellungen → Passwörter → Exportieren)".into()),
        }
    }
    report.push("Hinweis: Direkt aus Chrome verschlüsselte Passwörter können seit Chrome 127 (App-Bound-Encryption) nicht ausgelesen werden – der CSV-Export ist der offizielle Weg.".into());
    report.join("\n")
}

fn import_chrome_bookmarks(app: &App, dir: &std::path::Path) -> Result<usize, String> {
    let path = dir.join("Bookmarks");
    let data = std::fs::read_to_string(&path).map_err(|e| format!("Bookmarks-Datei nicht lesbar: {e}"))?;
    let v: Value = serde_json::from_str(&data).map_err(|e| e.to_string())?;
    let mut count = 0usize;
    if let Some(roots) = v.get("roots").and_then(|r| r.as_object()) {
        for (_, root) in roots {
            import_bm_node(app, root, 0, &mut count);
        }
    }
    Ok(count)
}

fn import_bm_node(app: &App, node: &Value, parent: i64, count: &mut usize) {
    let ty = node.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if ty == "folder" {
        let name = node.get("name").and_then(|n| n.as_str()).unwrap_or("Ordner");
        let fid = app.storage.add_bookmark_folder(name, parent);
        if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
            for c in children {
                import_bm_node(app, c, fid, count);
            }
        }
    } else if ty == "url" {
        let name = node.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let url = node.get("url").and_then(|u| u.as_str()).unwrap_or("");
        if !url.is_empty() {
            app.storage.add_bookmark(name, url, None, parent);
            *count += 1;
        }
    }
}

fn import_chrome_history(app: &App, dir: &std::path::Path) -> Result<usize, String> {
    let src = dir.join("History");
    let tmp = std::env::temp_dir().join(format!("aura_chrome_history_copy_{}", std::process::id()));
    std::fs::copy(&src, &tmp).map_err(|e| format!("History nicht kopierbar (Chrome läuft?): {e}"))?;
    let conn = rusqlite::Connection::open_with_flags(
        &tmp,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| e.to_string())?;
    let mut st = conn
        .prepare("SELECT url,title,visit_count,last_visit_time FROM urls ORDER BY last_visit_time DESC LIMIT 5000")
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut count = 0;
    for row in rows.flatten() {
        let (url, title, visits, chrome_time) = row;
        let unix = (chrome_time / 1_000_000) - 11_644_473_600;
        let _ = app.storage.conn.execute(
            "INSERT INTO history(url,title,visit_count,last_visit) VALUES(?1,?2,?3,?4)
             ON CONFLICT(url) DO UPDATE SET visit_count=history.visit_count+excluded.visit_count,
                last_visit=MAX(history.last_visit, excluded.last_visit), title=excluded.title",
            rusqlite::params![url, title, visits, unix],
        );
        count += 1;
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(count)
}

fn import_password_csv(app: &App, path: &str) -> Result<usize, String> {
    // Eine Passwort-CSV ist Kilobytes gross, nicht Gigabytes.
    if std::fs::metadata(path).map(|m| m.len() > 64 * 1024 * 1024).unwrap_or(false) {
        return Err("Datei zu gross".into());
    }
    let data = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut count = 0;
    for (i, line) in data.lines().enumerate() {
        if i == 0 {
            continue; // header
        }
        let cols = parse_csv_line(line);
        if cols.len() >= 4 {
            let origin = host_of(&cols[1]);
            let username = &cols[2];
            let password = &cols[3];
            if !password.is_empty() {
                if let Some(blob) = dpapi_protect(password.as_bytes()) {
                    let _ = app.storage.conn.execute(
                        "INSERT INTO passwords(origin,username,password) VALUES(?1,?2,?3)",
                        rusqlite::params![origin, username, blob],
                    );
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                out.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// Reverses `dpapi_protect` — only works for this Windows account.
fn dpapi_unprotect(data: &[u8]) -> Option<String> {
    use windows::Win32::Security::Cryptography::*;
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output).ok()?;
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let text = String::from_utf8_lossy(slice).to_string();
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            output.pbData as *mut core::ffi::c_void,
        )));
        Some(text)
    }
}

fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Security::Cryptography::*;
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&input, windows::core::w!("AuraBrowser"), None, None, None, 0, &mut output).ok()?;
        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let v = slice.to_vec();
        let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(
            output.pbData as *mut core::ffi::c_void,
        )));
        Some(v)
    }
}
