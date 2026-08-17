// WebView2 tab management: environment, controller creation, event wiring.
use std::any::Any;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{Interface, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;

use crate::app::{post, AppMsg};
use crate::util::wide;

pub struct Tab {
    pub id: u32,
    pub title: String,
    pub url: String, // display url (aura://... for internal pages)
    pub favicon_png: Option<Vec<u8>>,
    pub favicon: Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap>,
    pub controller: Option<ICoreWebView2Controller>,
    pub webview: Option<ICoreWebView2>,
    pub guards: Vec<Box<dyn Any>>, // keeps COM event handlers alive
    pub can_back: bool,
    pub can_fwd: bool,
    pub loading: bool,
    pub pinned: bool,
    pub muted: bool,
    pub playing_audio: bool,
    pub asleep: bool,
    pub group: Option<i64>,
    pub zoom: f64,
    pub pending_url: Option<String>,
    pub is_internal: bool,
    /// Last bounds/visibility pushed to the controller — avoids redundant
    /// SetBounds calls, each of which forces a full page reflow.
    pub last_bounds: Option<windows::Win32::Foundation::RECT>,
    pub last_visible: Option<bool>,
    /// Tick count when this tab was last in the foreground (sleep timer).
    pub last_active: u64,
    /// A controller has been requested but has not arrived yet.
    pub spawning: bool,
    /// Row animation: 0 = collapsed, 1 = full height. Grows on open, shrinks on
    /// close; a tab is only really dropped once it reaches 0.
    pub appear: f32,
    pub closing: bool,
    /// Hat die Ansicht schon einmal etwas gezeichnet? Vorher zeigt das
    /// Kindfenster nichts, und durch den durchsichtigen Inhaltsbereich sähe
    /// man ein schwarzes Rechteck. Bis dahin legt das Fenster seine eigene
    /// Farbe darunter.
    pub painted: bool,
}

impl Tab {
    pub fn new(id: u32, url: &str) -> Tab {
        Tab {
            id,
            title: "Neuer Tab".into(),
            url: url.to_string(),
            favicon_png: None,
            favicon: None,
            controller: None,
            webview: None,
            guards: vec![],
            can_back: false,
            can_fwd: false,
            loading: false,
            pinned: false,
            muted: false,
            playing_audio: false,
            asleep: false,
            group: None,
            zoom: 1.0,
            pending_url: Some(url.to_string()),
            is_internal: url.starts_with("aura://"),
            last_bounds: None,
            last_visible: None,
            last_active: unsafe {
                windows::Win32::System::SystemInformation::GetTickCount64()
            },
            spawning: false,
            appear: 0.0,
            closing: false,
            painted: false,
        }
    }

    pub fn domain(&self) -> String {
        if self.is_internal {
            return "Aura".into();
        }
        crate::util::host_of(&self.url)
    }
}

/// Reads a PWSTR out-param into a String (takes ownership, frees the buffer).
pub fn get_str(f: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> String {
    let mut p = PWSTR::null();
    let _ = f(&mut p);
    take_pwstr(p)
}

pub fn get_bool(f: impl FnOnce(*mut BOOL) -> windows::core::Result<()>) -> bool {
    let mut b = BOOL(0);
    let _ = f(&mut b);
    b.as_bool()
}

pub fn create_environment(user_data_dir: &str) -> Result<ICoreWebView2Environment> {
    let dir = wide(user_data_dir);
    let env_slot = std::rc::Rc::new(std::cell::RefCell::new(None::<ICoreWebView2Environment>));
    let env_slot2 = env_slot.clone();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| unsafe {
            CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), PCWSTR(dir.as_ptr()), None, &handler)
                .map_err(Error::WindowsError)
        }),
        Box::new(move |result, environment| {
            result?;
            *env_slot2.borrow_mut() = environment;
            Ok(())
        }),
    )?;
    let env = env_slot.borrow_mut().take();
    env.ok_or(Error::CallbackError("no environment".into()))
}

/// Kicks off async controller creation; completion arrives as AppMsg::ControllerReady.
pub fn spawn_controller(env: &ICoreWebView2Environment, hwnd: HWND, tab_id: u32) {
    let env = env.clone();
    let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
        move |result, controller| {
            match (result, controller) {
                (Ok(()), Some(c)) => post(AppMsg::ControllerReady {
                    tab: tab_id,
                    controller: c,
                }),
                _ => post(AppMsg::ControllerFailed { tab: tab_id }),
            }
            Ok(())
        },
    ));
    unsafe {
        let _ = env.CreateCoreWebView2Controller(hwnd, &handler);
    }
}

/// Überschreibt Chromiums Fehlerseite mit einer eigenen. Das Skript prüft
/// selbst, ob wirklich die eingebaute Seite steht — Seiten mit eigenem
/// 404-Layout bleiben unangetastet.
fn replace_error_page(wv: &ICoreWebView2, url: &str, status: i32, reason: i32) {
    let path = crate::storage::assets_dir().join("errorpage.js");
    let Ok(script) = std::fs::read_to_string(&path) else { return };
    let (accent, dark) = crate::app::theme_hint();
    let script = script
        .replace("__ACCENT__", &accent)
        .replace("__DARK__", if dark { "true" } else { "false" });
    // Die Fehlerseite laeuft unter chromewebdata - die echte Adresse muss also
    // von aussen kommen, sonst steht dort der interne Name.
    let js = format!(
        "window.__auraError={{status:{status},reason:'{}',url:{}}};\n{script}",
        error_name(reason),
        serde_json::to_string(url).unwrap_or_else(|_| "\"\"".into())
    );
    let w = wide(&js);
    unsafe {
        let _ = wv.ExecuteScript(PCWSTR(w.as_ptr()), None);
    }
}

/// COREWEBVIEW2_WEB_ERROR_STATUS als Text, damit das Skript danach unterscheiden kann.
fn error_name(status: i32) -> &'static str {
    match status {
        1 => "CertificateCommonNameIsIncorrect",
        2 => "CertificateExpired",
        3 => "ClientCertificateContainsErrors",
        4 => "CertificateRevoked",
        5 => "CertificateIsInvalid",
        6 => "ServerUnreachable",
        7 => "Timeout",
        8 => "ErrorHttpInvalidServerResponse",
        9 => "ConnectionAborted",
        10 => "ConnectionReset",
        11 => "Disconnected",
        12 => "CannotConnect",
        13 => "HostNameNotResolved",
        14 => "OperationCanceled",
        15 => "RedirectFailed",
        16 => "UnexpectedError",
        _ => "",
    }
}

/// Maps the internal aura:// scheme to the local assets host and back.
pub fn real_url(display: &str) -> String {
    if let Some(page) = display.strip_prefix("aura://") {
        let name = page.trim_end_matches('/');
        let file = match name {
            "start" => "newtab.html",
            "history" => "history.html",
            "bookmarks" => "bookmarks.html",
            "downloads" => "downloads.html",
            "settings" => "settings.html",
            "import" => "import.html",
            "reading" => "reading.html",
            "passwords" => "passwords.html",
            "tasks" => "tasks.html",
            _ => "newtab.html",
        };
        format!("https://aura.internal/{file}")
    } else {
        display.to_string()
    }
}

pub fn display_url(real: &str) -> String {
    if let Some(rest) = real.strip_prefix("https://aura.internal/") {
        let page = rest.split(['?', '#']).next().unwrap_or(rest);
        let name = match page {
            "newtab.html" => "start",
            "history.html" => "history",
            "bookmarks.html" => "bookmarks",
            "downloads.html" => "downloads",
            "settings.html" => "settings",
            "import.html" => "import",
            "reading.html" => "reading",
            "passwords.html" => "passwords",
            "tasks.html" => "tasks",
            other => other,
        };
        format!("aura://{name}")
    } else {
        real.to_string()
    }
}

/// Wires all WebView2 events for a tab. Handlers are stored in the returned guard vec.
pub fn attach_events(
    tab_id: u32,
    webview: &ICoreWebView2,
    controller: &ICoreWebView2Controller,
    env: &ICoreWebView2Environment,
) -> Vec<Box<dyn Any>> {
    let mut guards: Vec<Box<dyn Any>> = vec![];
    let mut token: i64 = 0;

    // --- Aura Shield: remember the site, and upgrade plain HTTP ---
    let h = NavigationStartingEventHandler::create(Box::new(move |_wv, args| {
        if let Some(args) = args {
            let uri = get_str(|p| unsafe { args.Uri(p) });
            if crate::adblock::https_only() {
                if let Some(rest) = uri.strip_prefix("http://") {
                    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
                    let local = host.starts_with("localhost")
                        || host.starts_with("127.0.0.1")
                        || host.starts_with("[::1]")
                        || host.ends_with(".localhost");
                    if !local && !crate::adblock::http_allowed(host) {
                        unsafe {
                            let _ = args.SetCancel(true);
                        }
                        post(AppMsg::UpgradeToHttps {
                            tab: tab_id,
                            url: format!("https://{rest}"),
                        });
                        return Ok(());
                    }
                }
            }
            crate::adblock::set_tab_host(tab_id, &uri);
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_NavigationStarting(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- Aura Shield: drop ad/tracker requests before they hit the network ---
    let envc = env.clone();
    let h = WebResourceRequestedEventHandler::create(Box::new(move |_wv, args| {
        if let Some(args) = args {
            if let Ok(req) = unsafe { args.Request() } {
                let uri = get_str(|p| unsafe { req.Uri(p) });
                let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                unsafe {
                    let _ = args.ResourceContext(&mut ctx);
                }
                if crate::adblock::should_block(tab_id, &uri, ctx.0) {
                    unsafe {
                        if let Ok(resp) = envc.CreateWebResourceResponse(
                            None,
                            403,
                            windows::core::w!("Blocked by Aura Shield"),
                            windows::core::w!("Access-Control-Allow-Origin: *"),
                        ) {
                            let _ = args.SetResponse(&resp);
                        }
                    }
                } else if crate::adblock::send_dnt() {
                    unsafe {
                        if let Ok(h) = req.Headers() {
                            let _ = h.SetHeader(
                                windows::core::w!("DNT"),
                                windows::core::w!("1"),
                            );
                            let _ = h.SetHeader(
                                windows::core::w!("Sec-GPC"),
                                windows::core::w!("1"),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }));
    // Routing every request through the host process costs a round trip, so the
    // filter is only installed when something actually needs to inspect requests.
    if crate::adblock::is_enabled() || crate::adblock::send_dnt() {
        unsafe {
            let _ = webview.AddWebResourceRequestedFilter(
                windows::core::w!("*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            );
            let _ = webview.add_WebResourceRequested(&h, &mut token);
        }
        guards.push(Box::new(h));
    }

    // --- Aura Shield: surrogate ad SDKs + popunder guard, before page scripts ---
    if crate::adblock::is_enabled() {
        let mut script = std::fs::read_to_string(crate::storage::assets_dir().join("shield.js"))
            .unwrap_or_default();
        if !script.is_empty() {
            if crate::adblock::strict_popups() {
                script.insert_str(0, "window.__auraStrictPopups=true;\n");
            }
            let w = wide(&script);
            let h = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                |_, _| Ok(()),
            ));
            unsafe {
                let _ = webview.AddScriptToExecuteOnDocumentCreated(PCWSTR(w.as_ptr()), &h);
            }
            guards.push(Box::new(h));
        }
    }

    // --- Aura Shield: hide the empty frames the blocked requests leave behind ---
    let h = ContentLoadingEventHandler::create(Box::new(move |wv, _| {
        // Ab hier zeichnet die Ansicht selbst — der Platzhalter darf weg.
        post(AppMsg::TabPainted { tab: tab_id });
        if let Some(wv) = wv {
            let url = get_str(|p| unsafe { wv.Source(p) });
            let host = crate::adblock::host_of_url(&url).to_string();
            // Seiten, die schon einmal mit Fenstern um sich geworfen haben,
            // bekommen die strenge Regel: nur echte Links dürfen noch öffnen.
            if crate::adblock::is_popup_abuser(&host) {
                let w = wide("window.__auraStrictPopups=true;");
                unsafe {
                    let _ = wv.ExecuteScript(PCWSTR(w.as_ptr()), None);
                }
            }
            let css = crate::adblock::cosmetic_css(&host);
            if !css.is_empty() {
                let lit = serde_json::to_string(&css).unwrap_or_else(|_| "\"\"".into());
                let js = format!(
                    "(function(){{try{{if(document.getElementById('aura-shield'))return;\
                     var s=document.createElement('style');s.id='aura-shield';s.textContent={lit};\
                     (document.head||document.documentElement).appendChild(s);}}catch(e){{}}}})()"
                );
                let w = wide(&js);
                unsafe {
                    let _ = wv.ExecuteScript(PCWSTR(w.as_ptr()), None);
                }
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_ContentLoading(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- title changed ---
    let h = DocumentTitleChangedEventHandler::create(Box::new(move |wv, _| {
        if let Some(wv) = wv {
            let title = get_str(|p| unsafe { wv.DocumentTitle(p) });
            post(AppMsg::Title { tab: tab_id, title });
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_DocumentTitleChanged(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- url changed (redirects, SPA navigation) ---
    let h = SourceChangedEventHandler::create(Box::new(move |wv, _| {
        if let Some(wv) = wv {
            let url = get_str(|p| unsafe { wv.Source(p) });
            post(AppMsg::Source { tab: tab_id, url });
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_SourceChanged(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- navigation completed ---
    let h = NavigationCompletedEventHandler::create(Box::new(move |wv, args| {
        if let Some(wv) = wv {
            let url = get_str(|p| unsafe { wv.Source(p) });
            let title = get_str(|p| unsafe { wv.DocumentTitle(p) });
            let can_back = get_bool(|b| unsafe { wv.CanGoBack(b) });
            let can_fwd = get_bool(|b| unsafe { wv.CanGoForward(b) });

            // Chromiums Fehlerseite traegt „Microsoft Edge“ in der Fusszeile.
            // Wenn die Navigation schieflief, ueberschreiben wir sie mit einer
            // eigenen – aber nur, wenn wirklich die eingebaute Seite steht.
            if let Some(args) = args {
                let ok = get_bool(|b| unsafe { args.IsSuccess(b) });
                let mut status = 0i32;
                if let Ok(a2) = args.cast::<ICoreWebView2NavigationCompletedEventArgs2>() {
                    unsafe {
                        let _ = a2.HttpStatusCode(&mut status);
                    }
                }
                let mut reason = COREWEBVIEW2_WEB_ERROR_STATUS_UNKNOWN;
                unsafe {
                    let _ = args.WebErrorStatus(&mut reason);
                }
                if !ok || status >= 400 {
                    replace_error_page(&wv, &url, status, reason.0);
                }
            }

            post(AppMsg::NavCompleted {
                tab: tab_id,
                url,
                title,
                can_back,
                can_fwd,
            });
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_NavigationCompleted(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- favicon changed ---
    let h = FaviconChangedEventHandler::create(Box::new(move |wv, _| {
        if let Some(wv) = wv {
            if let Ok(wv15) = wv.cast::<ICoreWebView2_15>() {
                let gh = GetFaviconCompletedHandler::create(Box::new(move |result, stream| {
                    if result.is_ok() {
                        if let Some(stream) = stream {
                            if let Some(bytes) = read_stream(&stream) {
                                post(AppMsg::Favicon { tab: tab_id, bytes });
                            }
                        }
                    }
                    Ok(())
                }));
                unsafe {
                    let _ = wv15.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &gh);
                }
            }
        }
        Ok(())
    }));
    if let Ok(wv15) = webview.cast::<ICoreWebView2_15>() {
        unsafe {
            let _ = wv15.add_FaviconChanged(&h, &mut token);
        }
        guards.push(Box::new(h));
    }

    // --- new window requested (target=_blank, window.open) ---
    let h = NewWindowRequestedEventHandler::create(Box::new(move |_wv, args| {
        if let Some(args) = args {
            let uri = get_str(|p| unsafe { args.Uri(p) });
            let initiated = get_bool(|b| unsafe { args.IsUserInitiated(b) });
            unsafe {
                let _ = args.SetHandled(true);
            }
            if !uri.is_empty() {
                post(AppMsg::NewWindow {
                    tab: tab_id,
                    uri,
                    user_initiated: initiated,
                });
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_NewWindowRequested(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- permission requested (camera/mic/location/...) ---
    let h = PermissionRequestedEventHandler::create(Box::new(move |_wv, args| {
        if let Some(args) = args {
            let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
            unsafe {
                let _ = args.PermissionKind(&mut kind);
            }
            let uri = get_str(|p| unsafe { args.Uri(p) });
            if let Ok(deferral) = unsafe { args.GetDeferral() } {
                post(AppMsg::Permission {
                    tab: tab_id,
                    kind: permission_kind_name(kind).to_string(),
                    uri,
                    args: args.clone(),
                    deferral,
                });
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_PermissionRequested(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- messages from internal pages (chrome.webview.postMessage) ---
    let h = WebMessageReceivedEventHandler::create(Box::new(move |_wv, args| {
        if let Some(args) = args {
            let json = get_str(|p| unsafe { args.WebMessageAsJson(p) });
            post(AppMsg::WebMessage { tab: tab_id, json });
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_WebMessageReceived(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- fullscreen element (video fullscreen) ---
    let h = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |wv, _| {
        if let Some(wv) = wv {
            let contains = get_bool(|b| unsafe { wv.ContainsFullScreenElement(b) });
            post(AppMsg::Fullscreen {
                tab: tab_id,
                contains,
            });
        }
        Ok(())
    }));
    unsafe {
        let _ = webview.add_ContainsFullScreenElementChanged(&h, &mut token);
    }
    guards.push(Box::new(h));

    // --- audio playing indicator ---
    if let Ok(wv8) = webview.cast::<ICoreWebView2_8>() {
        let h = IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |wv, _| {
            if let Some(wv) = wv {
                if let Ok(wv8) = wv.cast::<ICoreWebView2_8>() {
                    let playing = get_bool(|b| unsafe { wv8.IsDocumentPlayingAudio(b) });
                    post(AppMsg::Audio {
                        tab: tab_id,
                        playing,
                    });
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = wv8.add_IsDocumentPlayingAudioChanged(&h, &mut token);
        }
        guards.push(Box::new(h));
    }

    // --- downloads ---
    if let Ok(wv4) = webview.cast::<ICoreWebView2_4>() {
        let h = DownloadStartingEventHandler::create(Box::new(move |_wv, args| {
            if let Some(args) = args {
                if let Ok(deferral) = unsafe { args.GetDeferral() } {
                    if let Ok(op) = unsafe { args.DownloadOperation() } {
                        let uri = get_str(|p| unsafe { op.Uri(p) });
                        post(AppMsg::DownloadStart {
                            tab: tab_id,
                            uri,
                            args: args.clone(),
                            deferral,
                            op,
                        });
                    }
                }
            }
            Ok(())
        }));
        unsafe {
            let _ = wv4.add_DownloadStarting(&h, &mut token);
        }
        guards.push(Box::new(h));
    }

    // --- keyboard shortcuts inside web content ---
    let h = AcceleratorKeyPressedEventHandler::create(Box::new(move |_ctl, args| {
        if let Some(args) = args {
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN;
            unsafe {
                let _ = args.KeyEventKind(&mut kind);
            }
            if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN {
                let mut vk = 0u32;
                unsafe {
                    let _ = args.VirtualKey(&mut vk);
                }
                use windows::Win32::UI::Input::KeyboardAndMouse::*;
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 };
                let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 };
                let alt = unsafe { GetKeyState(VK_MENU.0 as i32) < 0 };
                post(AppMsg::Accel {
                    tab: tab_id,
                    vk,
                    ctrl,
                    shift,
                    alt,
                });
            }
        }
        Ok(())
    }));
    unsafe {
        let _ = controller.add_AcceleratorKeyPressed(&h, &mut token);
    }
    guards.push(Box::new(h));

    guards
}

pub fn permission_kind_name(kind: COREWEBVIEW2_PERMISSION_KIND) -> &'static str {
    match kind {
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => "Mikrofon",
        COREWEBVIEW2_PERMISSION_KIND_CAMERA => "Kamera",
        COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION => "Standort",
        COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS => "Benachrichtigungen",
        COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS => "Sensoren",
        COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ => "Zwischenablage",
        _ => "Berechtigung",
    }
}

fn read_stream(stream: &windows::Win32::System::Com::IStream) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    let mut buf = [0u8; 16384];
    loop {
        let mut read: u32 = 0;
        let hr = unsafe {
            stream.Read(
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                buf.len() as u32,
                Some(&mut read),
            )
        };
        if hr.is_err() || read == 0 {
            break;
        }
        data.extend_from_slice(&buf[..read as usize]);
        if data.len() > 4 * 1024 * 1024 {
            return None; // sanity limit
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}
