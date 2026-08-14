// Core application: window, chrome rendering (Direct2D), layout, actions, message routing.
use std::cell::{Cell, RefCell};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use windows::core::{w, Interface, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::*;
use crate::omnibox::{self, Suggestion};
use crate::popup::{self, MenuItem, Popup, PopupKind};
use crate::storage::{Session, SessionTab, Storage};
use crate::tabs::{self, Tab};
use crate::theme::{Theme, ThemeMode, R_MD, R_SM, R_XS};
use crate::util::*;

pub const WM_SYNC: u32 = WM_APP + 1;
/// Posted from the filter-list downloader thread when new lists are on disk.
pub const WM_FILTERS: u32 = WM_APP + 2;
/// An update was found (WPARAM ignored) / an update was installed (WPARAM = ok).
pub const WM_UPDATE_FOUND: u32 = WM_APP + 3;
pub const WM_UPDATE_READY: u32 = WM_APP + 4;
const TIMER_ANIM: usize = 1;
const TIMER_TOOLTIP: usize = 2;
const TIMER_SLEEP: usize = 3;
const TIMER_SESSION: usize = 4;

const TOPBAR_H: f32 = 56.0;
const SB_COLLAPSED: f32 = 64.0;
const SB_EXPANDED: f32 = 268.0;
const CAP_W: f32 = 40.0;
const CAP_H: f32 = 34.0;

/// Animation timings (ms).
const ANIM_SIDEBAR: f32 = 260.0;
const ANIM_HOVER: f32 = 130.0;
const ANIM_INDICATOR: f32 = 200.0;
const ANIM_TAB: f32 = 165.0;

/// 12345 -> "12.345" (German grouping).
fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

fn now_ms() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

/// Cubic ease-out — fast start, soft landing.
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let u = 1.0 - t;
    1.0 - u * u * u
}

fn approach(cur: f32, target: f32, dt: f32, dur: f32) -> f32 {
    if dur <= 0.0 {
        return target;
    }
    let step = dt / dur;
    if (target - cur).abs() <= step {
        target
    } else if target > cur {
        cur + step
    } else {
        cur - step
    }
}

thread_local! {
    static MSGQ: RefCell<Vec<AppMsg>> = const { RefCell::new(Vec::new()) };
    static MAIN_HWND: Cell<HWND> = const { Cell::new(HWND(std::ptr::null_mut())) };
    pub static APP_PTR: Cell<*mut App> = const { Cell::new(std::ptr::null_mut()) };
}

pub fn post(msg: AppMsg) {
    MSGQ.with(|q| q.borrow_mut().push(msg));
    MAIN_HWND.with(|h| unsafe {
        let _ = PostMessageW(Some(h.get()), WM_SYNC, WPARAM(0), LPARAM(0));
    });
}

// Some variants carry the originating tab even where the handler acts on the
// active tab — keeps the message log meaningful and the shapes uniform.
#[allow(dead_code)]
pub enum AppMsg {
    ControllerReady { tab: u32, controller: ICoreWebView2Controller },
    ControllerFailed { tab: u32 },
    Title { tab: u32, title: String },
    Source { tab: u32, url: String },
    NavCompleted { tab: u32, url: String, title: String, can_back: bool, can_fwd: bool },
    Favicon { tab: u32, bytes: Vec<u8> },
    NewWindow { tab: u32, uri: String, user_initiated: bool },
    Permission { tab: u32, kind: String, uri: String, args: ICoreWebView2PermissionRequestedEventArgs, deferral: ICoreWebView2Deferral },
    PermissionAnswer { allow: bool, remember: bool },
    WebMessage { tab: u32, json: String },
    Fullscreen { tab: u32, contains: bool },
    Audio { tab: u32, playing: bool },
    DownloadStart { tab: u32, uri: String, args: ICoreWebView2DownloadStartingEventArgs, deferral: ICoreWebView2Deferral, op: ICoreWebView2DownloadOperation },
    DownloadProgress { dl: i64, received: i64, total: i64, state: i32 },
    Accel { tab: u32, vk: u32, ctrl: bool, shift: bool, alt: bool },
    MenuAction { action: String },
    OmniSubmit { edit: HWND },
    OmniCancel { edit: HWND },
    OmniNav { edit: HWND, delta: i32 },
    UpgradeToHttps { tab: u32, url: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    Downloading,
    Ready,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Hot {
    None,
    Back,
    Fwd,
    Reload,
    Omnibox,
    Star,
    Shield,
    Downloads,
    Menu,
    CapMin,
    CapMax,
    CapClose,
    Orb,
    Gear,
    Plus,
    Tab(usize),
    TabClose(usize),
}

#[derive(Default)]
pub struct Layout {
    pub back: D2D_RECT_F,
    pub fwd: D2D_RECT_F,
    pub reload: D2D_RECT_F,
    pub omnibox: D2D_RECT_F,
    pub star: D2D_RECT_F,
    pub shield: D2D_RECT_F,
    pub downloads: D2D_RECT_F,
    pub menu: D2D_RECT_F,
    pub cap_min: D2D_RECT_F,
    pub cap_max: D2D_RECT_F,
    pub cap_close: D2D_RECT_F,
    pub orb: D2D_RECT_F,
    pub gear: D2D_RECT_F,
    pub plus: D2D_RECT_F,
    pub tab_rows: Vec<(usize, D2D_RECT_F)>,
    pub tab_close: Vec<(usize, D2D_RECT_F)>,
    /// Scroll viewport of the tab list.
    pub tab_view: D2D_RECT_F,
    pub content: RECT, // physical px
}

pub struct App {
    pub hwnd: HWND,
    pub hinst: HINSTANCE,
    pub gfx: Gfx,
    pub rt: Option<ID2D1HwndRenderTarget>,
    /// Composition surface (glass). None = classic opaque HWND target.
    comp: Option<crate::gfx::Composition>,
    comp_dev: Option<crate::gfx::CompDevice>,
    pub glass: bool,
    pub scale: f32,
    pub theme: Theme,
    pub theme_mode: ThemeMode,
    pub storage: Storage,
    pub profile: String,
    pub private: bool,
    pub env: Option<ICoreWebView2Environment>,

    pub tabs: Vec<Tab>,
    pub active: usize,
    pub next_tab_id: u32,
    pub split: Option<u32>,

    pub sidebar_w: f32,
    pub sidebar_target: f32,
    /// Scroll offset of the tab list in DIP (0 = top). Grows with many tabs.
    pub tab_scroll: f32,
    pub tab_scroll_target: f32,
    pub tab_scroll_max: f32,
    /// Left edge of the web content in DIP. Held steady while the sidebar
    /// animates so the page reflows exactly once per toggle.
    pub content_left: f32,
    pub expanded: bool,
    pub hot: Hot,
    pub pressed: Hot,
    pub layout: Layout,

    // animation state
    sb_from: f32,
    sb_t0: u64,
    hot_prev: Hot,
    hot_t: f32,
    hot_prev_t: f32,
    ind_y: f32,
    ind_h: f32,
    ind_ready: bool,
    anim_last: u64,
    anim_on: bool,

    pub edit: HWND,
    pub editing: bool,
    pub find_edit: HWND,
    pub find_open: bool,

    pub tooltip: Option<Box<Popup>>,
    pub tooltip_tab: Option<usize>,
    pub menu_popup: Option<Box<Popup>>,
    pub sugg_popup: Option<Box<Popup>>,
    pub dialog_popup: Option<Box<Popup>>,
    pub pending_permission: Option<(ICoreWebView2PermissionRequestedEventArgs, ICoreWebView2Deferral, String, String)>,

    /// Update found on GitHub, plus how far the installation got.
    pub pending_update: Option<crate::update::Release>,
    pub update_state: UpdateState,

    pub fullscreen: bool,
    pub fs_element: bool,
    pub saved_placement: WINDOWPLACEMENT,

    pub fmt_ui: IDWriteTextFormat,
    pub fmt_small: IDWriteTextFormat,
    pub fmt_semibold: IDWriteTextFormat,
    pub fmt_title: IDWriteTextFormat,
    pub fmt_icon: IDWriteTextFormat,
    pub fmt_icon_sm: IDWriteTextFormat,

    pub edit_brush: HBRUSH,
    pub edit_fg: COLORREF,
    pub edit_bg: COLORREF,
    /// Owned by the app so the edit controls keep a valid font handle.
    #[allow(dead_code)]
    pub hfont: HFONT,
    pub dl_guards: Vec<Box<dyn std::any::Any>>,
}

impl App {
    pub fn new() -> Result<Box<App>> {
        let args: Vec<String> = std::env::args().collect();
        let mut profile = "Default".to_string();
        let mut private = false;
        let mut start_url: Option<String> = None;
        let mut i = 1;
        while i < args.len() {
            if let Some(p) = args[i].strip_prefix("--profile=") {
                profile = p.to_string();
            } else if args[i] == "--private" {
                private = true;
                profile = format!("__private_{}", std::process::id());
            } else if let Some(u) = args[i].strip_prefix("--url=") {
                start_url = Some(u.to_string());
            } else if !args[i].starts_with("--") {
                // Bare URL, e.g. when Windows hands us a link to open.
                start_url = Some(args[i].clone());
            }
            i += 1;
        }

        let storage = Storage::open(&profile).map_err(|e| windows::core::Error::new(E_FAIL, e))?;

        let theme_mode = match storage.get_setting("theme", "system").as_str() {
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::System,
        };
        let accent = parse_accent(&storage.get_setting("accent", "110,91,208"));
        let reduce_motion = storage.get_setting("reduce_motion", "0") == "1";
        let theme = Theme::new(theme_mode, accent, reduce_motion);

        // Aura Shield: the built-in list is up instantly; the big cached lists are
        // parsed on a worker thread and swapped in a moment later.
        let shield_on = storage.get_setting("shield", "1") == "1";
        let fdir = crate::adblock::filters_dir(&Storage::data_dir(&profile));
        crate::adblock::load(crate::adblock::BASE_LIST, &[], shield_on);
        crate::adblock::set_allowlist(
            &storage
                .get_setting("shield_allow", "")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
        crate::adblock::set_total(storage.get_setting("shield_total", "0").parse().unwrap_or(0));
        crate::adblock::set_dnt(storage.get_setting("dnt", "1") == "1");
        crate::adblock::set_https_only(storage.get_setting("https_only", "1") == "1");
        crate::adblock::set_strict_popups(storage.get_setting("popup_strict", "0") == "1");
        crate::adblock::set_strict_sites(
            &storage
                .get_setting("shield_strict", "")
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );

        let gfx = Gfx::new()?;
        let hinst = HINSTANCE(unsafe { GetModuleHandleW(None)? }.0);

        let fmt_ui = gfx.text_format(13.5, DWRITE_FONT_WEIGHT_NORMAL)?;
        let fmt_small = gfx.text_format(11.5, DWRITE_FONT_WEIGHT_NORMAL)?;
        let fmt_semibold = gfx.text_format(13.5, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
        let fmt_title = gfx.text_format(15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
        let fmt_icon = icon_font(&gfx, 15.0)?;
        let fmt_icon_sm = icon_font(&gfx, 12.0)?;

        register_classes(hinst)?;

        let (tfg, tbg) = if theme.dark {
            (COLORREF(0x00F8F0F0), COLORREF(0x00241E1E))
        } else {
            (COLORREF(0x00261C1C), COLORREF(0x00FCFAFA))
        };
        let edit_brush = unsafe { CreateSolidBrush(tbg) };
        let hfont = unsafe {
            CreateFontW(
                -15, 0, 0, 0,
                FW_NORMAL.0 as i32, 0, 0, 0,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY, (VARIABLE_PITCH.0 | FF_SWISS.0) as u32,
                w!("Segoe UI"),
            )
        };

        // Try the GPU composition device first: it decides whether the window can
        // be a sheet of glass (no redirection bitmap) or a classic opaque one.
        // Glass needs both a composition device and Windows' transparency effects;
        // without the latter DWM draws no backdrop and the chrome would be a hole
        // through to the desktop.
        let want_glass = storage.get_setting("glass", "1") == "1";
        let comp_dev = gfx.create_comp_device();
        let glass = comp_dev.is_some() && want_glass && crate::theme::system_transparency();

        let title = wide("Aura Browser");
        let class = wide("AuraMainWindow");
        let ex_style = if comp_dev.is_some() {
            WS_EX_NOREDIRECTIONBITMAP
        } else {
            WINDOW_EX_STYLE(0)
        };
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                CW_USEDEFAULT, CW_USEDEFAULT, 1440, 900,
                None, None, Some(hinst), None,
            )?
        };

        // Rounded corners + dark frame + shadow line.
        unsafe {
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const _,
                4,
            );
            let dark = BOOL(theme.dark as i32);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                4,
            );
            let margins = if glass {
                // Sheet of glass: the backdrop covers the whole client area.
                MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 }
            } else {
                MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 }
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            if glass {
                // Windows 11 acrylic behind the chrome (falls back silently on 10).
                const DWMWA_SYSTEMBACKDROP_TYPE: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(38);
                let backdrop: i32 = match storage.get_setting("glass_style", "acrylic").as_str() {
                    "mica" => 2,   // DWMSBT_MAINWINDOW
                    "tabbed" => 4, // DWMSBT_TABBEDWINDOW
                    _ => 3,        // DWMSBT_TRANSIENTWINDOW (acrylic)
                };
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &backdrop as *const _ as *const _,
                    4,
                );
            }
        }

        let scale = dpi_scale(hwnd);

        // Omnibox edit control (hidden until editing).
        let edit = create_edit(hwnd, scale, hfont, 100)?;
        let find_edit = create_edit(hwnd, scale, hfont, 101)?;

        let mut app = Box::new(App {
            hwnd,
            hinst,
            gfx,
            rt: None,
            comp: None,
            comp_dev,
            glass,
            scale,
            theme,
            theme_mode,
            storage,
            profile,
            private,
            env: None,
            tabs: vec![],
            active: 0,
            next_tab_id: 1,
            split: None,
            sidebar_w: SB_COLLAPSED,
            sidebar_target: SB_COLLAPSED,
            tab_scroll: 0.0,
            tab_scroll_target: 0.0,
            tab_scroll_max: 0.0,
            content_left: SB_COLLAPSED,
            expanded: false,
            hot: Hot::None,
            pressed: Hot::None,
            layout: Layout::default(),
            sb_from: SB_COLLAPSED,
            sb_t0: 0,
            hot_prev: Hot::None,
            hot_t: 0.0,
            hot_prev_t: 0.0,
            ind_y: 0.0,
            ind_h: 0.0,
            ind_ready: false,
            anim_last: 0,
            anim_on: false,
            edit,
            editing: false,
            find_edit,
            find_open: false,
            tooltip: None,
            tooltip_tab: None,
            menu_popup: None,
            sugg_popup: None,
            dialog_popup: None,
            pending_permission: None,
            pending_update: None,
            update_state: UpdateState::Idle,
            fullscreen: false,
            fs_element: false,
            saved_placement: WINDOWPLACEMENT::default(),
            fmt_ui,
            fmt_small,
            fmt_semibold,
            fmt_title,
            fmt_icon,
            fmt_icon_sm,
            edit_brush,
            edit_fg: tfg,
            edit_bg: tbg,
            hfont,
            dl_guards: vec![],
        });

        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, app.as_mut() as *mut App as isize);
        }
        MAIN_HWND.with(|h| h.set(hwnd));
        APP_PTR.with(|p| p.set(app.as_mut() as *mut App));

        // The first WM_NCCALCSIZE ran before the App pointer existed, so the window
        // still carries the default frame. Force a recalc now that we handle it.
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }

        // Hang the composition swap chain off the window now that it exists.
        // The surface is used even without glass — it just paints opaque then.
        if let Some(dev) = &app.comp_dev {
            let rc = client_rect(hwnd);
            app.comp = app.gfx.create_composition(
                dev,
                hwnd,
                rc.right as u32,
                rc.bottom as u32,
                app.scale,
            );
            if app.comp.is_none() {
                app.glass = false;
            }
        }
        if app.glass {
            app.theme.glassify();
        }

        // Show the chrome before the (slow) WebView2 environment comes up, so the
        // window is on screen in a few dozen milliseconds instead of half a second.
        app.relayout();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        app.paint();

        // Filter lists load in the background from here on.
        crate::adblock::load_cached_async(fdir.clone(), shield_on, hwnd.0 as isize, WM_FILTERS);

        // WebView2 environment (blocking init with message pump).
        let data_dir = if private {
            std::env::temp_dir().join(format!("aura_priv_{}", std::process::id()))
        } else {
            crate::storage::dir_webview(&app.profile)
        };
        let _ = std::fs::create_dir_all(&data_dir);
        let env = tabs::create_environment(&data_dir.to_string_lossy())
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("WebView2: {e}")))?;
        app.env = Some(env);

        // Restore session or open start tab.
        let restored = if private {
            false
        } else {
            app.restore_session()
        };
        // A URL on the command line always opens, session or not.
        if let Some(url) = start_url {
            app.new_tab(&url, true);
        } else if !restored {
            app.new_tab("aura://start", true);
        }

        unsafe {
            let _ = SetTimer(Some(hwnd), TIMER_SLEEP, 60_000, None);
            let _ = SetTimer(Some(hwnd), TIMER_SESSION, 30_000, None);
        }

        // Look for a new release shortly after start, once per day at most.
        if app.storage.get_setting("auto_update", "1") == "1" {
            let last: i64 = app.storage.get_setting("update_checked", "0").parse().unwrap_or(0);
            if now_unix() - last > 24 * 3600 {
                app.storage.set_setting("update_checked", &now_unix().to_string());
                crate::update::check_async(hwnd.0 as isize, WM_UPDATE_FOUND);
            }
        }

        // Refresh the filter lists at most every three days, in the background.
        if shield_on && app.storage.get_setting("shield_update", "1") == "1" {
            if crate::adblock::cache_age(&fdir) > 3 * 24 * 3600 {
                crate::adblock::update_async(fdir.clone(), hwnd.0 as isize, WM_FILTERS);
            }
        }

        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        unsafe {
            // Windows applies the launcher's STARTUPINFO show command to the very
            // first ShowWindow call, which can leave us minimised. The second call
            // is ours, so state it explicitly.
            let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = UpdateWindow(self.hwnd);
        }
        self.relayout();
        let mut msg = MSG::default();
        loop {
            let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }
        Ok(())
    }

    // ---------------- window procedure ----------------
    fn wndproc(&mut self, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_NCCALCSIZE => {
                    if w.0 != 0 {
                        // No non-client frame: the client area is the whole window.
                        // When maximized Windows oversizes the window by the (now
                        // invisible) frame, which would push the chrome off-screen —
                        // clamp the client area back to the work area.
                        if IsZoomed(hwnd).as_bool() {
                            let params = &mut *(l.0 as *mut NCCALCSIZE_PARAMS);
                            let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                            let mut info = MONITORINFO {
                                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                                ..Default::default()
                            };
                            if GetMonitorInfoW(mon, &mut info).as_bool() {
                                let r = &mut params.rgrc[0];
                                r.left = r.left.max(info.rcWork.left);
                                r.top = r.top.max(info.rcWork.top);
                                r.right = r.right.min(info.rcWork.right);
                                r.bottom = r.bottom.min(info.rcWork.bottom);
                            }
                        }
                        return LRESULT(0);
                    }
                    return DefWindowProcW(hwnd, msg, w, l);
                }
                WM_GETMINMAXINFO => {
                    let mmi = &mut *(l.0 as *mut MINMAXINFO);
                    let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    if GetMonitorInfoW(mon, &mut info).as_bool() {
                        mmi.ptMaxPosition.x = info.rcWork.left - info.rcMonitor.left;
                        mmi.ptMaxPosition.y = info.rcWork.top - info.rcMonitor.top;
                        mmi.ptMaxSize.x = info.rcWork.right - info.rcWork.left;
                        mmi.ptMaxSize.y = info.rcWork.bottom - info.rcWork.top;
                    }
                    return LRESULT(0);
                }
                WM_NCHITTEST => return self.hit_test_nchittest(hwnd, l),
                WM_ERASEBKGND => return LRESULT(1),
                WM_PAINT => {
                    self.paint();
                    let _ = ValidateRect(Some(hwnd), None);
                    return LRESULT(0);
                }
                WM_SIZE => {
                    let rc = client_rect(hwnd);
                    if let Some(rt) = &self.rt {
                        let _ = rt.Resize(&D2D_SIZE_U {
                            width: rc.right as u32,
                            height: rc.bottom as u32,
                        });
                    }
                    let scale = self.scale;
                    if let Some(c) = self.comp.as_mut() {
                        c.resize(rc.right as u32, rc.bottom as u32, scale);
                    }
                    self.relayout();
                    self.paint();
                    return LRESULT(0);
                }
                WM_DPICHANGED => {
                    self.scale = GetDpiForWindow(hwnd) as f32 / 96.0;
                    let rc = &*(l.0 as *const RECT);
                    let _ = SetWindowPos(
                        hwnd, None,
                        rc.left, rc.top,
                        rc.right - rc.left, rc.bottom - rc.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    self.relayout();
                    return LRESULT(0);
                }
                WM_MOUSEMOVE => {
                    self.on_mouse_move(x_lparam(l) as f32 / self.scale, y_lparam(l) as f32 / self.scale);
                    return LRESULT(0);
                }
                WM_MOUSELEAVE => {
                    self.set_hot(Hot::None);
                    self.hide_tooltip();
                    return LRESULT(0);
                }
                WM_LBUTTONDOWN => {
                    let hot = self.hot;
                    self.pressed = hot;
                    self.on_click(hot);
                    return LRESULT(0);
                }
                WM_RBUTTONUP => {
                    self.on_right_click(x_lparam(l) as f32 / self.scale, y_lparam(l) as f32 / self.scale);
                    return LRESULT(0);
                }
                WM_MBUTTONUP => {
                    if let Hot::Tab(i) = self.hot {
                        self.close_tab(i);
                    }
                    return LRESULT(0);
                }
                WM_LBUTTONDBLCLK => {
                    if self.hot == Hot::Omnibox {
                        self.begin_edit();
                    } else if self.hot == Hot::None && y_lparam(l) as f32 / self.scale < TOPBAR_H {
                        // Double-click on empty chrome toggles maximize, like a titlebar.
                        let cmd = if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE };
                        let _ = ShowWindow(hwnd, cmd);
                    }
                    return LRESULT(0);
                }
                WM_MOUSEWHEEL => {
                    let delta = ((w.0 >> 16) & 0xFFFF) as i16;
                    // Ctrl+Wheel over the chrome zooms the page.
                    if (w.0 & 0x0008) != 0 {
                        self.zoom(if delta > 0 { 0.1 } else { -0.1 });
                        return LRESULT(0);
                    }
                    // Wheel over the sidebar scrolls the tab list.
                    let mut p = POINT { x: x_lparam(l), y: y_lparam(l) }; // screen coords
                    let _ = ScreenToClient(hwnd, &mut p);
                    let xd = p.x as f32 / self.scale;
                    if xd < self.sidebar_w && self.tab_scroll_max > 0.0 {
                        let step = if self.sidebar_w < 150.0 { 48.0 } else { 40.0 } * 3.0;
                        let before = self.tab_scroll_target;
                        self.tab_scroll_target =
                            (self.tab_scroll_target - delta as f32 / 120.0 * step).clamp(0.0, self.tab_scroll_max);
                        if (self.tab_scroll_target - before).abs() > 0.01 {
                            self.start_anim();
                        }
                        return LRESULT(0);
                    }
                    return DefWindowProcW(hwnd, msg, w, l);
                }
                WM_XBUTTONUP => {
                    // Mouse thumb buttons: back / forward.
                    match ((w.0 >> 16) & 0xFFFF) as u16 {
                        1 => self.go_back(),
                        2 => self.go_fwd(),
                        _ => {}
                    }
                    return LRESULT(1);
                }
                WM_COMMAND => {
                    let notif = ((w.0 >> 16) & 0xFFFF) as u16;
                    let ctrl = HWND(l.0 as *mut core::ffi::c_void);
                    if notif == EN_CHANGE as u16 {
                        if ctrl == self.edit {
                            self.on_omnibox_changed();
                        } else if ctrl == self.find_edit {
                            self.on_find_changed();
                        }
                    }
                    return LRESULT(0);
                }
                WM_CTLCOLOREDIT => {
                    let hdc = HDC(w.0 as *mut core::ffi::c_void);
                    SetTextColor(hdc, self.edit_fg);
                    SetBkColor(hdc, self.edit_bg);
                    SetBkMode(hdc, TRANSPARENT);
                    return LRESULT(self.edit_brush.0 as isize);
                }
                WM_KEYDOWN | WM_SYSKEYDOWN => {
                    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    let alt = GetKeyState(VK_MENU.0 as i32) < 0;
                    if self.shortcut(w.0 as u32, ctrl, shift, alt) {
                        return LRESULT(0);
                    }
                    return DefWindowProcW(hwnd, msg, w, l);
                }
                WM_TIMER => {
                    match w.0 {
                        TIMER_ANIM => self.anim_tick(),
                        TIMER_TOOLTIP => self.tooltip_tick(),
                        TIMER_SLEEP => self.sleep_tick(),
                        TIMER_SESSION => self.save_session(),
                        _ => {}
                    }
                    return LRESULT(0);
                }
                WM_FILTERS => {
                    self.reload_filters();
                    return LRESULT(0);
                }
                WM_UPDATE_FOUND => {
                    if let Some(rel) = crate::update::take_pending() {
                        self.pending_update = Some(rel);
                        self.paint();
                        crate::pages::refresh_about_pages(self);
                    }
                    return LRESULT(0);
                }
                WM_UPDATE_READY => {
                    self.update_state = if w.0 == 1 { UpdateState::Ready } else { UpdateState::Failed };
                    crate::pages::refresh_about_pages(self);
                    self.paint();
                    return LRESULT(0);
                }
                WM_SYNC => {
                    self.on_sync();
                    return LRESULT(0);
                }
                WM_SETTINGCHANGE => {
                    if self.theme_mode == ThemeMode::System {
                        self.apply_theme(ThemeMode::System);
                    }
                    return DefWindowProcW(hwnd, msg, w, l);
                }
                WM_CLOSE => {
                    self.shutdown();
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    return LRESULT(0);
                }
                _ => DefWindowProcW(hwnd, msg, w, l),
            }
        }
    }

    fn hit_test_nchittest(&self, hwnd: HWND, _l: LPARAM) -> LRESULT {
        let pt = cursor_pos();
        let rc = client_rect(hwnd);
        let mut win = RECT::default();
        unsafe {
            let _ = GetWindowRect(hwnd, &mut win);
        }
        let x = pt.x - win.left;
        let y = pt.y - win.top;
        let w = rc.right;
        let h = rc.bottom;
        let border = 6;
        let zoomed = unsafe { IsZoomed(hwnd) }.as_bool();
        if !zoomed {
            let left = x < border;
            let right = x >= w - border;
            let top = y < border;
            let bottom = y >= h - border;
            let code = match (left, right, top, bottom) {
                (true, _, true, _) => HTTOPLEFT,
                (_, true, true, _) => HTTOPRIGHT,
                (true, _, _, true) => HTBOTTOMLEFT,
                (_, true, _, true) => HTBOTTOMRIGHT,
                (true, _, _, _) => HTLEFT,
                (_, true, _, _) => HTRIGHT,
                (_, _, true, _) => HTTOP,
                (_, _, _, true) => HTBOTTOM,
                _ => HTNOWHERE,
            };
            if code != HTNOWHERE {
                return LRESULT(code as isize);
            }
        }
        // Caption area: topbar, but not over interactive elements.
        let xd = x as f32 / self.scale;
        let yd = y as f32 / self.scale;
        if yd < TOPBAR_H && xd >= self.sidebar_w {
            let hot = self.hit(xd, yd);
            if hot == Hot::None {
                return LRESULT(HTCAPTION as isize);
            }
        }
        LRESULT(HTCLIENT as isize)
    }

    // ---------------- layout ----------------
    pub fn relayout(&mut self) {
        let rc = client_rect(self.hwnd);
        let w = rc.right as f32 / self.scale;
        let h = rc.bottom as f32 / self.scale;
        let sw = self.sidebar_w;
        let mut l = Layout::default();

        // Topbar: nav buttons.
        let by = (TOPBAR_H - 32.0) / 2.0;
        l.back = rect_f(sw + 10.0, by, 32.0, 32.0);
        l.fwd = rect_f(sw + 48.0, by, 32.0, 32.0);
        l.reload = rect_f(sw + 86.0, by, 32.0, 32.0);

        // Caption buttons — rounded pills that sit inside the topbar rather than
        // full-height Windows boxes glued to the corner.
        let caps_w = CAP_W * 3.0 + 12.0;
        let cy = (TOPBAR_H - CAP_H) / 2.0;
        l.cap_close = rect_f(w - CAP_W - 8.0, cy, CAP_W, CAP_H);
        l.cap_max = rect_f(w - CAP_W * 2.0 - 8.0, cy, CAP_W, CAP_H);
        l.cap_min = rect_f(w - CAP_W * 3.0 - 8.0, cy, CAP_W, CAP_H);

        // Right-side topbar buttons.
        l.menu = rect_f(w - caps_w - 42.0, by, 32.0, 32.0);
        l.downloads = rect_f(w - caps_w - 80.0, by, 32.0, 32.0);
        l.shield = rect_f(w - caps_w - 118.0, by, 32.0, 32.0);

        // Omnibox pill.
        let ob_left = sw + 132.0;
        let ob_right = w - caps_w - 134.0;
        l.omnibox = rect_f(ob_left, 9.0, (ob_right - ob_left).max(200.0), 38.0);
        l.star = rect_f(ob_right - 42.0, 12.0, 32.0, 32.0);

        // Sidebar. The tab list scrolls: with many tabs it would otherwise run
        // straight past the bottom edge and off the window.
        l.orb = rect_f(12.0, 10.0, 40.0, 40.0);
        let collapsed = self.sidebar_w < 150.0;
        let list_top = 66.0;
        let list_bottom = h - 104.0; // room for "new tab" + settings
        let row_h = if collapsed { 48.0 } else { 40.0 };
        let content_h: f32 = self.tabs.iter().map(|t| row_h * t.appear).sum();
        let view_h = (list_bottom - list_top).max(row_h);
        self.tab_scroll_max = (content_h - view_h).max(0.0);
        self.tab_scroll_target = self.tab_scroll_target.clamp(0.0, self.tab_scroll_max);
        self.tab_scroll = self.tab_scroll.clamp(0.0, self.tab_scroll_max);
        l.tab_view = rect_f(0.0, list_top, sw, view_h);

        let mut y = list_top - self.tab_scroll;
        for (i, tab) in self.tabs.iter().enumerate() {
            // Opening/closing rows take proportionally less height.
            let slot = row_h * tab.appear;
            // Rows outside the viewport are not laid out at all, so hit-testing
            // and painting stay O(visible) even with hundreds of tabs.
            let visible = y + slot > list_top && y < list_bottom && tab.appear > 0.02;
            if visible {
                let inner = (slot - 4.0).max(1.0);
                if collapsed || tab.pinned {
                    l.tab_rows.push((i, rect_f(10.0, y, sw - 20.0, inner.min(44.0))));
                } else {
                    l.tab_rows.push((i, rect_f(8.0, y, sw - 16.0, inner.min(36.0))));
                    if tab.appear > 0.9 {
                        l.tab_close.push((i, rect_f(sw - 42.0, y + 4.0, 28.0, 28.0)));
                    }
                }
            }
            y += slot;
        }
        let list_end = (list_top + content_h - self.tab_scroll).min(list_bottom);
        l.plus = rect_f(10.0, list_end + 6.0, sw - 20.0, 40.0);
        l.gear = rect_f(10.0, h - 52.0, sw - 20.0, 40.0);

        // Place the active-tab indicator without animating on the first layout.
        if !self.ind_ready {
            if let Some((_, r)) = l.tab_rows.iter().find(|(i, _)| *i == self.active) {
                let inset = (r.bottom - r.top) * 0.22;
                self.ind_y = r.top + inset;
                self.ind_h = (r.bottom - r.top) - inset * 2.0;
                self.ind_ready = true;
            }
        }

        // Content area (physical px) for webviews — pinned to content_left, not
        // to the animating sidebar width.
        let cx = (self.content_left * self.scale).round() as i32;
        let cy = (TOPBAR_H * self.scale).round() as i32;
        l.content = if self.fs_element {
            RECT { left: 0, top: 0, right: rc.right, bottom: rc.bottom }
        } else {
            RECT { left: cx, top: cy, right: rc.right, bottom: rc.bottom }
        };
        self.layout = l;

        // Slide the active-tab indicator to its new row.
        let (ty, th) = self.indicator_target();
        if self.theme.reduce_motion {
            self.ind_y = ty;
            self.ind_h = th;
        } else if (self.ind_y - ty).abs() > 0.5 || (self.ind_h - th).abs() > 0.5 {
            self.start_anim();
        }

        // Position edit controls (physical px).
        let ob = self.layout.omnibox;
        let s = self.scale;
        unsafe {
            let _ = MoveWindow(
                self.edit,
                ((ob.left + 38.0) * s) as i32,
                ((ob.top + 7.0) * s) as i32,
                ((ob.right - ob.left - 80.0) * s) as i32,
                (24.0 * s) as i32,
                true,
            );
            let fw = 320.0 * s;
            let _ = MoveWindow(
                self.find_edit,
                rc.right - (fw + 16.0 * s) as i32,
                ((TOPBAR_H + 8.0) * s) as i32,
                fw as i32,
                (28.0 * s) as i32,
                true,
            );
        }
        self.layout_webviews();
    }

    pub fn layout_webviews(&mut self) {
        let c = self.layout.content;
        let cw = c.right - c.left;
        let active_id = self.tabs.get(self.active).map(|t| t.id).unwrap_or(0);
        let split = self.split;
        let mut reclip = false;
        for tab in &mut self.tabs {
            let Some(ctl) = tab.controller.clone() else { continue };
            let is_active = tab.id == active_id;
            let is_split = split == Some(tab.id);
            let target = if is_active {
                Some(if split.is_some() {
                    RECT { left: c.left, top: c.top, right: c.left + cw / 2, bottom: c.bottom }
                } else {
                    c
                })
            } else if is_split {
                Some(RECT { left: c.left + cw / 2 + 1, top: c.top, right: c.right, bottom: c.bottom })
            } else {
                None
            };
            unsafe {
                match target {
                    // Skip redundant SetBounds: every call reflows the page.
                    Some(rect) => {
                        if tab.last_bounds != Some(rect) {
                            let _ = ctl.SetBounds(rect);
                            tab.last_bounds = Some(rect);
                            reclip = true;
                        }
                        if tab.last_visible != Some(true) {
                            let _ = ctl.SetIsVisible(true);
                            tab.last_visible = Some(true);
                        }
                    }
                    None => {
                        if tab.last_visible != Some(false) {
                            let _ = ctl.SetIsVisible(false);
                            tab.last_visible = Some(false);
                        }
                    }
                }
            }
        }
        if reclip {
            // Window regions clip without anti-aliasing, so rounding the web view
            // looks jagged — keep it square and let the chrome do the styling.
            self.unclip_webviews();
        }
    }

    fn unclip_webviews(&self) {
        unsafe extern "system" fn cb(child: HWND, _lp: LPARAM) -> BOOL {
            unsafe {
                let mut cls = [0u16; 64];
                let n = GetClassNameW(child, &mut cls) as usize;
                if String::from_utf16_lossy(&cls[..n]).starts_with("Chrome_WidgetWin") {
                    SetWindowRgn(child, None, true);
                }
            }
            BOOL(1)
        }
        unsafe {
            let _ = EnumChildWindows(Some(self.hwnd), Some(cb), LPARAM(0));
        }
    }

    // ---------------- painting ----------------
    pub fn paint(&mut self) {
        // Glass path: a DirectComposition swap chain with per-pixel alpha, so the
        // Mica/Acrylic backdrop shows through the chrome.
        if self.comp.is_some() {
            let Some(c) = self.comp.as_ref() else { return };
            if c.begin().is_err() {
                return;
            }
            let rt = c.target();
            self.paint_chrome(&rt);
            let ok = match self.comp.as_ref() {
                Some(c) => c.end(),
                None => true,
            };
            if !ok {
                self.comp = None; // device lost: fall back next frame
                for t in &mut self.tabs {
                    t.favicon = None;
                }
            }
            return;
        }

        if self.rt.is_none() {
            match self.gfx.create_hwnd_rt(self.hwnd, self.scale) {
                Ok(rt) => self.rt = Some(rt),
                Err(_) => return,
            }
        }
        let rt = self.rt.clone().unwrap();
        let target: ID2D1RenderTarget = rt.cast().unwrap();
        unsafe {
            rt.BeginDraw();
        }
        self.paint_chrome(&target);
        let ok = unsafe { rt.EndDraw(None, None) }.is_ok();
        if !ok {
            self.rt = None; // device lost: recreate next paint
            for t in &mut self.tabs {
                t.favicon = None;
            }
        }
    }

    fn paint_chrome(&mut self, rt: &ID2D1RenderTarget) {
        let theme = self.theme.clone();
        let rc = client_rect(self.hwnd);
        let w = rc.right as f32 / self.scale;
        let h = rc.bottom as f32 / self.scale;
        let sw = self.sidebar_w;
        // On the composition surface our visual sits *above* the WebView child
        // window, so the content area has to stay untouched — otherwise the
        // chrome paints straight over the page.
        let overlay = self.comp.is_some();
        let content_x = if self.fs_element { w } else { self.content_left };
        unsafe {
            if overlay {
                rt.Clear(None);
            } else {
                rt.Clear(Some(&theme.bg));
            }

            // ---- topbar ----
            if let Ok(b) = brush(rt, theme.bg_top) {
                rt.FillRectangle(&rect_f(sw, 0.0, w - sw, TOPBAR_H), &b);
            }

            // ---- sidebar ----
            if let Ok(b) = brush(rt, theme.sidebar_bg) {
                rt.FillRectangle(&rect_f(0.0, 0.0, sw, h), &b);
            }

            // Gap between the sidebar and the page while the sidebar animates.
            if overlay && content_x > sw {
                if let Ok(b) = brush(rt, theme.bg) {
                    rt.FillRectangle(&rect_f(sw, TOPBAR_H, content_x - sw, h - TOPBAR_H), &b);
                }
            }

            // ---- separators ----
            if let Ok(b) = brush(rt, theme.border) {
                rt.FillRectangle(&rect_f(sw - 1.0, 0.0, 1.0, h), &b);
                if !self.fs_element {
                    let cl = self.content_left.min(sw);
                    rt.FillRectangle(&rect_f(cl, TOPBAR_H - 1.0, w - cl, 1.0), &b);
                }
            }
            self.paint_progress(rt, &theme, w, sw);

            self.paint_nav(rt, &theme);
            self.paint_omnibox(rt, &theme);
            self.paint_caption(rt, &theme, w);
            self.paint_sidebar(rt, &theme, h);
        }
    }

    /// Indeterminate loading sweep along the bottom edge of the topbar.
    fn paint_progress(&self, rt: &ID2D1RenderTarget, theme: &Theme, w: f32, sw: f32) {
        let loading = self.tabs.get(self.active).map(|t| t.loading).unwrap_or(false);
        let track = rect_f(sw, TOPBAR_H - 2.0, w - sw, 2.0);
        if !loading {
            return;
        }
        let span = (w - sw).max(1.0);
        let seg = (span * 0.28).max(60.0);
        // 0..1 sweep with an eased in/out so it never looks mechanical.
        let p = ((now_ms() % 1400) as f32) / 1400.0;
        let e = if p < 0.5 {
            4.0 * p * p * p
        } else {
            1.0 - (-2.0 * p + 2.0).powi(3) / 2.0
        };
        let x = sw - seg + e * (span + seg);
        let left = x.max(track.left);
        let right = (x + seg).min(track.right);
        if right <= left {
            return;
        }
        unsafe {
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: color(0, 0, 0, 0.0) },
                D2D1_GRADIENT_STOP { position: 0.5, color: theme.accent_f },
                D2D1_GRADIENT_STOP { position: 1.0, color: color(0, 0, 0, 0.0) },
            ];
            if let Ok(gs) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
                if let Ok(gb) = rt.CreateLinearGradientBrush(
                    &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: pt(x, 0.0),
                        endPoint: pt(x + seg, 0.0),
                    },
                    None,
                    &gs,
                ) {
                    rt.FillRectangle(&rect_f(left, track.top, right - left, 2.0), &gb);
                }
            }
        }
    }

    fn paint_nav(&self, rt: &ID2D1RenderTarget, theme: &Theme) {
        let l = &self.layout;
        let active_tab = self.tabs.get(self.active);
        let can_back = active_tab.map(|t| t.can_back).unwrap_or(false);
        let can_fwd = active_tab.map(|t| t.can_fwd).unwrap_or(false);
        let loading = active_tab.map(|t| t.loading).unwrap_or(false);
        self.icon_button(rt, theme, l.back, "\u{E72B}", can_back, self.hover_t(Hot::Back));
        self.icon_button(rt, theme, l.fwd, "\u{E72A}", can_fwd, self.hover_t(Hot::Fwd));
        let glyph = if loading { "\u{E711}" } else { "\u{E72C}" };
        self.icon_button(rt, theme, l.reload, glyph, true, self.hover_t(Hot::Reload));
        self.icon_button(rt, theme, l.downloads, "\u{E896}", true, self.hover_t(Hot::Downloads));
        self.icon_button(rt, theme, l.menu, "\u{E712}", true, self.hover_t(Hot::Menu));
        self.paint_shield(rt, theme);
    }

    /// Shield button with a live "blocked on this page" badge.
    fn paint_shield(&self, rt: &ID2D1RenderTarget, theme: &Theme) {
        let r = self.layout.shield;
        let t = self.hover_t(Hot::Shield);
        let tab = self.tabs.get(self.active);
        let host = tab.map(|t| host_of(&t.url)).unwrap_or_default();
        let on = crate::adblock::is_enabled() && !crate::adblock::is_allowlisted(&host);
        let n = tab.map(|t| crate::adblock::blocked_for(t.id)).unwrap_or(0);
        unsafe {
            if t > 0.0 {
                if let Ok(b) = brush(rt, theme.hover_at(t)) {
                    rt.FillRoundedRectangle(&rounded(r, R_SM), &b);
                }
            }
            let c = if on { theme.accent_f } else { theme.text_dim };
            if let Ok(b) = brush(rt, c) {
                let g: Vec<u16> = if on { "\u{EA18}" } else { "\u{F140}" }.encode_utf16().collect();
                self.fmt_icon.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                self.fmt_icon.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                rt.DrawText(&g, &self.fmt_icon, &r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
            if on && n > 0 {
                let label = if n > 99 { "99+".to_string() } else { n.to_string() };
                let bw = 14.0 + label.len() as f32 * 4.0;
                let badge = rect_f(r.right - bw - 1.0, r.top - 1.0, bw, 14.0);
                if let Ok(b) = brush(rt, theme.accent_f) {
                    rt.FillRoundedRectangle(&rounded(badge, 7.0), &b);
                }
                if let Ok(b) = brush(rt, color(255, 255, 255, 1.0)) {
                    let t: Vec<u16> = label.encode_utf16().collect();
                    self.fmt_small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                    self.fmt_small.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                    rt.DrawText(&t, &self.fmt_small, &badge, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
            }
        }
    }

    fn icon_button(&self, rt: &ID2D1RenderTarget, theme: &Theme, r: D2D_RECT_F, glyph: &str, enabled: bool, hot: f32) {
        unsafe {
            if hot > 0.0 && enabled {
                if let Ok(b) = brush(rt, theme.hover_at(hot)) {
                    rt.FillRoundedRectangle(&rounded(r, R_SM), &b);
                }
            }
            let c = if enabled { theme.text } else { theme.text_dim };
            if let Ok(b) = brush(rt, c) {
                let text: Vec<u16> = glyph.encode_utf16().collect();
                self.fmt_icon.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                self.fmt_icon.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                rt.DrawText(
                    &text,
                    &self.fmt_icon,
                    &r,
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
    }

    fn paint_omnibox(&mut self, rt: &ID2D1RenderTarget, theme: &Theme) {
        let r = self.layout.omnibox;
        let pill = (r.bottom - r.top) / 2.0; // fully rounded pill
        let ht = self.hover_t(Hot::Omnibox);
        unsafe {
            // pill background (lifts slightly on hover)
            if let Ok(b) = brush(rt, theme.input_bg) {
                rt.FillRoundedRectangle(&rounded(r, pill), &b);
            }
            if ht > 0.0 && !self.editing {
                if let Ok(b) = brush(rt, theme.hover_at(ht * 0.7)) {
                    rt.FillRoundedRectangle(&rounded(r, pill), &b);
                }
            }
            if self.editing {
                // focus ring
                if let Ok(b) = brush(rt, theme.accent_soft) {
                    rt.DrawRoundedRectangle(&rounded(inflate(r, 2.5), pill + 2.5), &b, 3.0, None);
                }
                if let Ok(b) = brush(rt, theme.accent_f) {
                    rt.DrawRoundedRectangle(&rounded(r, pill), &b, 1.5, None);
                }
            } else if let Ok(b) = brush(rt, theme.border) {
                rt.DrawRoundedRectangle(&rounded(r, pill), &b, 1.0, None);
            }

            // left glyph: lock / globe / search
            let tab = self.tabs.get(self.active);
            let glyph = if self.editing {
                "\u{E721}"
            } else if tab.map(|t| t.is_internal).unwrap_or(true) {
                "\u{E774}"
            } else if tab.map(|t| t.url.starts_with("https://")).unwrap_or(false) {
                "\u{E72E}"
            } else {
                "\u{E774}"
            };
            if let Ok(b) = brush(rt, theme.text_dim) {
                let text: Vec<u16> = glyph.encode_utf16().collect();
                self.fmt_icon_sm.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                self.fmt_icon_sm.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                rt.DrawText(
                    &text,
                    &self.fmt_icon_sm,
                    &rect_f(r.left + 8.0, r.top, 26.0, r.bottom - r.top),
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // URL text (only when not editing; edit control shows text otherwise)
            if !self.editing {
                if let Some(tab) = tab {
                    let url = &tab.url;
                    if !url.is_empty() {
                        // Split "https://host/path?q#f" into host (bright) and remainder (dimmed).
                        let (shown_host, rest) = if tab.is_internal {
                            (url.clone(), String::new())
                        } else {
                            let after = url
                                .strip_prefix("https://")
                                .or_else(|| url.strip_prefix("http://"))
                                .unwrap_or(url.as_str());
                            let end = after
                                .find(|c| c == '/' || c == '?' || c == '#')
                                .unwrap_or(after.len());
                            (after[..end].to_string(), after[end..].to_string())
                        };
                        // host part
                        if let Ok(b) = brush(rt, theme.text) {
                            let t: Vec<u16> = shown_host.encode_utf16().collect();
                            self.fmt_ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
                            self.fmt_ui.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                            rt.DrawText(
                                &t,
                                &self.fmt_ui,
                                &rect_f(r.left + 38.0, r.top, r.right - r.left - 80.0, r.bottom - r.top),
                                &b,
                                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                        // rest of url, dimmed, after measured host width
                        if !rest.is_empty() {
                            if let Ok(layout) = self.gfx.dwrite.CreateTextLayout(
                                &shown_host.encode_utf16().collect::<Vec<u16>>(),
                                &self.fmt_ui,
                                10000.0,
                                100.0,
                            ) {
                                let mut m = DWRITE_TEXT_METRICS::default();
                                let _ = layout.GetMetrics(&mut m);
                                if let Ok(b) = brush(rt, theme.text_dim) {
                                    let t: Vec<u16> = rest.encode_utf16().collect();
                                    rt.DrawText(
                                        &t,
                                        &self.fmt_ui,
                                        &rect_f(
                                            r.left + 38.0 + m.width,
                                            r.top,
                                            r.right - r.left - 80.0 - m.width,
                                            r.bottom - r.top,
                                        ),
                                        &b,
                                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                                        DWRITE_MEASURING_MODE_NATURAL,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // bookmark star
            let bookmarked = tab
                .and_then(|t| self.storage.is_bookmarked(&t.url))
                .is_some();
            let star_glyph = if bookmarked { "\u{E735}" } else { "\u{E734}" };
            let star_color = if bookmarked { theme.accent_f } else { theme.text_dim };
            let star_t = self.hover_t(Hot::Star);
            if star_t > 0.0 {
                if let Ok(b) = brush(rt, theme.hover_at(star_t)) {
                    rt.FillRoundedRectangle(&rounded(self.layout.star, R_SM), &b);
                }
            }
            if let Ok(b) = brush(rt, star_color) {
                let t: Vec<u16> = star_glyph.encode_utf16().collect();
                self.fmt_icon_sm.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                rt.DrawText(
                    &t,
                    &self.fmt_icon_sm,
                    &self.layout.star,
                    &b,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
        }
    }

    fn paint_caption(&self, rt: &ID2D1RenderTarget, theme: &Theme, w: f32) {
        let zoomed = unsafe { IsZoomed(self.hwnd) }.as_bool();
        let caps = [
            (self.layout.cap_min, Hot::CapMin, 0),
            (self.layout.cap_max, Hot::CapMax, if zoomed { 2 } else { 1 }),
            (self.layout.cap_close, Hot::CapClose, 3),
        ];
        for (r, hot, kind) in caps {
            unsafe {
                let hovered = self.hot == hot;
                let t = self.hover_t(hot);
                if t > 0.0 {
                    let mut c = if kind == 3 { theme.danger } else { theme.hover };
                    c.a *= if kind == 3 { t } else { t * 1.4 };
                    if let Ok(b) = brush(rt, c) {
                        rt.FillRoundedRectangle(&rounded(r, R_SM), &b);
                    }
                }
                let fg = if hovered && kind == 3 { color(255, 255, 255, 1.0) } else { theme.text };
                if let Ok(b) = brush(rt, fg) {
                    let cx = ((r.left + r.right) / 2.0).round() + 0.5;
                    let cy = ((r.top + r.bottom) / 2.0).round() + 0.5;
                    let s = 5.5; // half-size of the glyphs
                    match kind {
                        0 => rt.DrawLine(pt(cx - s, cy), pt(cx + s, cy), &b, 1.2, None),
                        1 => rt.DrawRoundedRectangle(
                            &rounded(rect_f(cx - s, cy - s, s * 2.0, s * 2.0), 2.0),
                            &b, 1.2, None,
                        ),
                        2 => {
                            rt.DrawRoundedRectangle(
                                &rounded(rect_f(cx - s, cy - s + 2.0, s * 2.0 - 2.0, s * 2.0 - 2.0), 2.0),
                                &b, 1.2, None,
                            );
                            rt.DrawRoundedRectangle(
                                &rounded(rect_f(cx - s + 2.0, cy - s, s * 2.0 - 2.0, s * 2.0 - 2.0), 2.0),
                                &b, 1.2, None,
                            );
                        }
                        _ => {
                            rt.DrawLine(pt(cx - s, cy - s), pt(cx + s, cy + s), &b, 1.3, None);
                            rt.DrawLine(pt(cx + s, cy - s), pt(cx - s, cy + s), &b, 1.3, None);
                        }
                    }
                }
            }
        }
        let _ = w;
    }

    fn paint_sidebar(&mut self, rt: &ID2D1RenderTarget, theme: &Theme, h: f32) {
        let collapsed = self.sidebar_w < 150.0;
        unsafe {
            // Aura orb (radial gradient) — expand/collapse button.
            let orb = self.layout.orb;
            let cx = (orb.left + orb.right) / 2.0;
            let cy = (orb.top + orb.bottom) / 2.0;
            let orb_t = self.hover_t(Hot::Orb);
            if orb_t > 0.0 {
                if let Ok(b) = brush(rt, theme.hover_at(orb_t)) {
                    rt.FillRoundedRectangle(&rounded(orb, R_MD), &b);
                }
            }
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: color(198, 184, 245, 1.0) },
                D2D1_GRADIENT_STOP { position: 1.0, color: theme.accent_f },
            ];
            if let Ok(gs) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
                if let Ok(gb) = rt.CreateRadialGradientBrush(
                    &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                        center: pt(cx - 4.0, cy - 5.0),
                        gradientOriginOffset: pt(0.0, 0.0),
                        radiusX: 18.0,
                        radiusY: 18.0,
                    },
                    None,
                    &gs,
                ) {
                    // Orb breathes a touch on hover.
                    rt.FillEllipse(&ellipse(cx, cy, 13.0 + orb_t * 1.5), &gb);
                }
            }
            if !collapsed {
                if let Ok(b) = brush(rt, theme.text) {
                    let t: Vec<u16> = "Aura".encode_utf16().collect();
                    self.fmt_title.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
                    self.fmt_title.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                    rt.DrawText(
                        &t,
                        &self.fmt_title,
                        &rect_f(orb.right + 10.0, orb.top, 120.0, orb.bottom - orb.top),
                        &b,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            // Tabs. Decode pending favicons into device bitmaps first (needs &mut self).
            for i in 0..self.tabs.len() {
                if self.tabs[i].favicon.is_some() {
                    continue;
                }
                let png = match &self.tabs[i].favicon_png {
                    Some(p) => p.clone(),
                    None => continue,
                };
                if let Ok(bmp) = self.gfx.bitmap_from_bytes(rt, &png) {
                    self.tabs[i].favicon = Some(bmp);
                }
            }
            let groups = self.storage.list_groups();
            // Clip the list so scrolled rows never bleed over the footer buttons.
            let view = self.layout.tab_view;
            rt.PushAxisAlignedClip(&view, D2D1_ANTIALIAS_MODE_ALIASED);
            // Active-tab pill indicator, sliding between rows.
            if self.ind_ready && self.ind_h > 1.0 {
                if let Some((_, ar)) = self.layout.tab_rows.iter().find(|(i, _)| *i == self.active) {
                    let inset = ((ar.bottom - ar.top) - self.ind_h) / 2.0;
                    let pill = rect_f(
                        ar.left,
                        self.ind_y - inset,
                        ar.right - ar.left,
                        self.ind_h + inset * 2.0,
                    );
                    if let Ok(b) = brush(rt, theme.active) {
                        rt.FillRoundedRectangle(&rounded(pill, R_MD), &b);
                    }
                    if let Ok(b) = brush(rt, theme.accent_f) {
                        rt.FillRoundedRectangle(
                            &rounded(rect_f(pill.left - 7.0, self.ind_y, 3.0, self.ind_h), 1.5),
                            &b,
                        );
                    }
                }
            }
            for (i, r) in self.layout.tab_rows.clone() {
                let tab = &self.tabs[i];
                let is_active = i == self.active;
                let hovered = self.hot == Hot::Tab(i);
                // row background (the active pill is drawn above, animated)
                let row_t = self.hover_t(Hot::Tab(i));
                if !is_active && row_t > 0.0 {
                    if let Ok(b) = brush(rt, theme.hover_at(row_t)) {
                        rt.FillRoundedRectangle(&rounded(r, R_MD), &b);
                    }
                }
                // Rows fade while they grow in or shrink away.
                let fade = tab.appear.clamp(0.0, 1.0);
                if fade < 0.999 {
                    rt.PushLayer(
                        &D2D1_LAYER_PARAMETERS {
                            contentBounds: rect_f(r.left, r.top, r.right - r.left, r.bottom - r.top),
                            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                            opacity: fade,
                            ..Default::default()
                        },
                        None,
                    );
                }
                // group color dot
                if let Some(gid) = tab.group {
                    if let Some(g) = groups.iter().find(|g| g.id == gid) {
                        let c = parse_hex_color(&g.color).unwrap_or(theme.accent_f);
                        if let Ok(b) = brush(rt, c) {
                            let gy = (r.top + r.bottom) / 2.0;
                            rt.FillEllipse(&ellipse(r.left + 7.0, gy, 3.5), &b);
                        }
                    }
                }
                // favicon
                let icon_size = if collapsed || tab.pinned { 24.0 } else { 19.0 };
                let ix = if collapsed || tab.pinned {
                    (r.left + r.right) / 2.0 - icon_size / 2.0
                } else {
                    r.left + 16.0
                };
                let iy = (r.top + r.bottom) / 2.0 - icon_size / 2.0;
                let icon_rect = rect_f(ix, iy, icon_size, icon_size);
                if let Some(bmp) = &tab.favicon {
                    rt.DrawBitmap(
                        bmp,
                        Some(&icon_rect),
                        1.0,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                } else {
                    // fallback: rounded square with first letter
                    let letter_bg = if tab.is_internal { theme.accent_soft } else { theme.hover };
                    if let Ok(b) = brush(rt, letter_bg) {
                        rt.FillRoundedRectangle(&rounded(icon_rect, R_XS * 0.75), &b);
                    }
                    let ch = tab
                        .domain()
                        .chars()
                        .next()
                        .unwrap_or('A')
                        .to_uppercase()
                        .to_string();
                    if let Ok(b) = brush(rt, if tab.is_internal { theme.accent_f } else { theme.text_dim }) {
                        let t: Vec<u16> = ch.encode_utf16().collect();
                        self.fmt_small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                        self.fmt_small.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                        rt.DrawText(
                            &t,
                            &self.fmt_small,
                            &icon_rect,
                            &b,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
                // audio/mute indicator
                if tab.playing_audio || tab.muted {
                    let g = if tab.muted { "\u{E74F}" } else { "\u{E767}" };
                    if let Ok(b) = brush(rt, theme.text_dim) {
                        let t: Vec<u16> = g.encode_utf16().collect();
                        rt.DrawText(
                            &t,
                            &self.fmt_icon_sm,
                            &rect_f(icon_rect.right - 4.0, icon_rect.bottom - 8.0, 12.0, 12.0),
                            &b,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
                // title + close (expanded, not pinned)
                if !collapsed && !tab.pinned {
                    let title = if tab.title.is_empty() { tab.domain() } else { tab.title.clone() };
                    if let Ok(b) = brush(rt, if is_active { theme.text } else { theme.text_dim }) {
                        let t: Vec<u16> = title.encode_utf16().collect();
                        self.fmt_ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
                        self.fmt_ui.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                        rt.DrawText(
                            &t,
                            &self.fmt_ui,
                            &rect_f(r.left + 40.0, r.top, r.right - r.left - 72.0, r.bottom - r.top),
                            &b,
                            D2D1_DRAW_TEXT_OPTIONS_CLIP,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                    if hovered || self.hot == Hot::TabClose(i) {
                        if let Some((_, cr)) = self.layout.tab_close.iter().find(|(ti, _)| *ti == i) {
                            let cr = *cr;
                            let ct = self.hover_t(Hot::TabClose(i));
                            if ct > 0.0 {
                                let mut c = theme.danger;
                                c.a = 0.16 * ct;
                                if let Ok(b) = brush(rt, c) {
                                    rt.FillRoundedRectangle(&rounded(cr, R_XS), &b);
                                }
                            }
                            if let Ok(b) = brush(rt, theme.text_dim) {
                                let t: Vec<u16> = "\u{E711}".encode_utf16().collect();
                                self.fmt_icon_sm.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                                rt.DrawText(
                                    &t,
                                    &self.fmt_icon_sm,
                                    &cr,
                                    &b,
                                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                            }
                        }
                    }
                }
                if fade < 0.999 {
                    rt.PopLayer();
                }
            }

            rt.PopAxisAlignedClip(); // end of the scrolled tab list

            // Scrollbar hint: a slim thumb that only shows when the list overflows.
            if self.tab_scroll_max > 0.5 {
                let view = self.layout.tab_view;
                let vh = view.bottom - view.top;
                let frac = vh / (vh + self.tab_scroll_max);
                let th = (vh * frac).max(28.0);
                let ty = view.top + (vh - th) * (self.tab_scroll / self.tab_scroll_max);
                if let Ok(b) = brush(rt, theme.border) {
                    rt.FillRoundedRectangle(
                        &rounded(rect_f(view.right - 4.0, ty, 3.0, th), 1.5),
                        &b,
                    );
                }
            }

            // plus button
            let plus = self.layout.plus;
            let plus_t = self.hover_t(Hot::Plus);
            if plus_t > 0.0 {
                if let Ok(b) = brush(rt, theme.hover_at(plus_t)) {
                    rt.FillRoundedRectangle(&rounded(plus, R_MD), &b);
                }
            }
            if let Ok(b) = brush(rt, theme.text_dim) {
                let t: Vec<u16> = "\u{E710}".encode_utf16().collect();
                self.fmt_icon.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                self.fmt_icon.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                let r = if collapsed { plus } else { rect_f(plus.left, plus.top, 40.0, plus.bottom - plus.top) };
                rt.DrawText(&t, &self.fmt_icon, &r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                if !collapsed {
                    if let Ok(b2) = brush(rt, theme.text) {
                        let t2: Vec<u16> = "Neuer Tab".encode_utf16().collect();
                        self.fmt_ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
                        rt.DrawText(
                            &t2,
                            &self.fmt_ui,
                            &rect_f(plus.left + 40.0, plus.top, plus.right - plus.left - 44.0, plus.bottom - plus.top),
                            &b2,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
            }

            // settings gear
            let gear = self.layout.gear;
            let gear_t = self.hover_t(Hot::Gear);
            if gear_t > 0.0 {
                if let Ok(b) = brush(rt, theme.hover_at(gear_t)) {
                    rt.FillRoundedRectangle(&rounded(gear, R_MD), &b);
                }
            }
            if let Ok(b) = brush(rt, theme.text_dim) {
                let t: Vec<u16> = "\u{E713}".encode_utf16().collect();
                let r = if collapsed { gear } else { rect_f(gear.left, gear.top, 40.0, gear.bottom - gear.top) };
                rt.DrawText(&t, &self.fmt_icon, &r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                if !collapsed {
                    if let Ok(b2) = brush(rt, theme.text) {
                        let t2: Vec<u16> = "Einstellungen".encode_utf16().collect();
                        self.fmt_ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
                        rt.DrawText(
                            &t2,
                            &self.fmt_ui,
                            &rect_f(gear.left + 40.0, gear.top, gear.right - gear.left - 44.0, gear.bottom - gear.top),
                            &b2,
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                            DWRITE_MEASURING_MODE_NATURAL,
                        );
                    }
                }
            }
        }
        let _ = h;
    }

    // ---------------- input handling ----------------
    fn hit(&self, x: f32, y: f32) -> Hot {
        let l = &self.layout;
        let inside = |r: &D2D_RECT_F| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inside(&l.cap_close) { return Hot::CapClose; }
        if inside(&l.cap_max) { return Hot::CapMax; }
        if inside(&l.cap_min) { return Hot::CapMin; }
        if inside(&l.back) { return Hot::Back; }
        if inside(&l.fwd) { return Hot::Fwd; }
        if inside(&l.reload) { return Hot::Reload; }
        if inside(&l.downloads) { return Hot::Downloads; }
        if inside(&l.shield) { return Hot::Shield; }
        if inside(&l.menu) { return Hot::Menu; }
        if inside(&l.star) { return Hot::Star; }
        if inside(&l.omnibox) { return Hot::Omnibox; }
        if inside(&l.orb) { return Hot::Orb; }
        if inside(&l.plus) { return Hot::Plus; }
        if inside(&l.gear) { return Hot::Gear; }
        for (i, r) in &l.tab_close {
            if inside(r) && self.hot == Hot::Tab(*i) {
                return Hot::TabClose(*i);
            }
        }
        for (i, r) in &l.tab_rows {
            if inside(r) {
                return Hot::Tab(*i);
            }
        }
        Hot::None
    }

    fn set_hot(&mut self, hot: Hot) {
        if self.hot != hot {
            if self.theme.reduce_motion {
                self.hot = hot;
                self.hot_t = 1.0;
                self.hot_prev = Hot::None;
                self.hot_prev_t = 0.0;
                self.paint();
                return;
            }
            // Carry the outgoing element's intensity into the fade-out slot.
            self.hot_prev = self.hot;
            self.hot_prev_t = self.hot_t;
            self.hot = hot;
            self.hot_t = 0.0;
            self.start_anim();
            self.paint();
        }
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        // Optional: sidebar unfolds while the pointer rests on it.
        if self.storage.get_setting("sidebar_hover", "0") == "1" && !self.editing {
            let inside = x < self.sidebar_w + 4.0;
            if inside && !self.expanded {
                self.toggle_sidebar();
            } else if !inside && self.expanded && x > SB_EXPANDED {
                self.toggle_sidebar();
            }
        }
        let hot = self.hit(x, y);
        if hot != self.hot {
            // arm tooltip for collapsed tab icons
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_TOOLTIP);
            }
            self.hide_tooltip();
            if let Hot::Tab(i) = hot {
                if self.sidebar_w < 150.0 {
                    self.tooltip_tab = Some(i);
                    unsafe {
                        let _ = SetTimer(Some(self.hwnd), TIMER_TOOLTIP, 450, None);
                    }
                }
            }
            self.set_hot(hot);
        }
        // track leave
        unsafe {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
        }
    }

    fn tooltip_tick(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TOOLTIP);
        }
        let Some(i) = self.tooltip_tab else { return };
        let Some(tab) = self.tabs.get(i) else { return };
        let title = if tab.title.is_empty() { tab.domain() } else { tab.title.clone() };
        let sub = tab.domain();
        // Anchor beside the tab row and centre it vertically, instead of dropping
        // it under the cursor where it overlaps the sidebar.
        let row = self.layout.tab_rows.iter().find(|(ti, _)| *ti == i).map(|(_, r)| *r);
        let mut pt = match row {
            Some(r) => POINT {
                x: ((self.sidebar_w + 8.0) * self.scale) as i32,
                y: (((r.top + r.bottom) / 2.0 - 26.0) * self.scale) as i32,
            },
            None => {
                let p = cursor_pos();
                POINT { x: p.x + 16, y: p.y + 12 }
            }
        };
        if row.is_some() {
            unsafe {
                let _ = ClientToScreen(self.hwnd, &mut pt);
            }
        }
        popup::show_tooltip(self, pt.x, pt.y, &title, &sub);
    }

    fn hide_tooltip(&mut self) {
        if let Some(t) = self.tooltip.take() {
            t.close();
        }
        self.tooltip_tab = None;
    }

    fn on_click(&mut self, hot: Hot) {
        match hot {
            Hot::CapMin => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_SYSCOMMAND, WPARAM(SC_MINIMIZE as usize), LPARAM(0));
            },
            Hot::CapMax => unsafe {
                let cmd = if IsZoomed(self.hwnd).as_bool() { SC_RESTORE } else { SC_MAXIMIZE };
                let _ = PostMessageW(Some(self.hwnd), WM_SYSCOMMAND, WPARAM(cmd as usize), LPARAM(0));
            },
            Hot::CapClose => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Hot::Back => self.go_back(),
            Hot::Fwd => self.go_fwd(),
            Hot::Reload => self.reload(),
            Hot::Downloads => self.open_internal("aura://downloads"),
            Hot::Shield => self.show_shield_menu(),
            Hot::Menu => self.show_main_menu(),
            Hot::Star => self.toggle_bookmark(),
            Hot::Omnibox => self.begin_edit(),
            Hot::Orb => self.toggle_sidebar(),
            Hot::Plus => { self.new_tab("aura://start", true); }
            Hot::Gear => self.open_internal("aura://settings"),
            Hot::Tab(i) => self.activate_tab(i),
            Hot::TabClose(i) => self.close_tab(i),
            Hot::None => {
                if self.editing {
                    self.end_edit(false);
                }
            }
        }
    }

    fn on_right_click(&mut self, x: f32, y: f32) {
        if let Hot::Tab(i) = self.hit(x, y) {
            self.show_tab_menu(i);
        }
    }

    // ---------------- sidebar ----------------
    fn toggle_sidebar(&mut self) {
        self.expanded = !self.expanded;
        self.sidebar_target = if self.expanded { SB_EXPANDED } else { SB_COLLAPSED };
        // Reflow the page once, at the wider of the two edges, so the sidebar
        // always animates over empty chrome instead of pushing the web content.
        self.content_left = self.sidebar_w.max(self.sidebar_target);
        if self.theme.reduce_motion {
            self.sidebar_w = self.sidebar_target;
            self.content_left = self.sidebar_target;
            self.relayout();
            self.paint();
        } else {
            self.sb_from = self.sidebar_w;
            self.sb_t0 = now_ms();
            self.relayout();
            self.start_anim();
        }
    }

    // ---------------- shield ----------------
    // ---------------- updates ----------------
    /// Asks GitHub whether a newer release exists.
    pub fn check_update(&mut self) {
        self.update_state = UpdateState::Checking;
        crate::update::check_async(self.hwnd.0 as isize, WM_UPDATE_FOUND);
    }

    /// Downloads and stages the update; the swap happens on the next start.
    pub fn install_update(&mut self) {
        let Some(rel) = self.pending_update.clone() else { return };
        self.update_state = UpdateState::Downloading;
        crate::pages::refresh_about_pages(self);
        crate::update::install_async(rel, self.hwnd.0 as isize, WM_UPDATE_READY);
    }

    /// Closes the browser so the staged update can replace the files.
    pub fn restart_for_update(&mut self) {
        self.save_session();
        self.close_window();
    }

    /// Installs freshly parsed lists (background load or after an update).
    pub fn reload_filters(&mut self) {
        if crate::adblock::install_pending() {
            self.paint();
            return;
        }
        // A download finished: re-parse in the background, don't stall the UI.
        let dir = crate::adblock::filters_dir(&Storage::data_dir(&self.profile));
        let on = crate::adblock::is_enabled();
        crate::adblock::load_cached_async(dir, on, self.hwnd.0 as isize, WM_FILTERS);
    }

    pub fn update_filters_now(&mut self) {
        let dir = crate::adblock::filters_dir(&Storage::data_dir(&self.profile));
        crate::adblock::update_async(dir, self.hwnd.0 as isize, WM_FILTERS);
    }

    pub fn set_shield(&mut self, on: bool) {
        crate::adblock::set_enabled(on);
        self.storage.set_setting("shield", if on { "1" } else { "0" });
        self.reload_active();
    }

    /// Toggles the shield for the active tab's site and reloads it.
    pub fn toggle_shield_site(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        if tab.is_internal {
            return;
        }
        let host = host_of(&tab.url);
        crate::adblock::toggle_site(&host);
        let list = crate::adblock::allowlist().join(",");
        self.storage.set_setting("shield_allow", &list);
        self.reload_active();
    }

    /// Clears cookies and web storage for one site (or everything when empty).
    /// WebView2 has no per-origin clear, so cookies go through the cookie
    /// manager and the rest through a small script in the page itself.
    pub fn clear_site_data(&mut self, host: &str) {
        let Some(wv) = self.tabs.get(self.active).and_then(|t| t.webview.clone()) else { return };

        if host.is_empty() {
            if let Ok(wv13) = wv.cast::<ICoreWebView2_13>() {
                if let Ok(profile) = unsafe { wv13.Profile() } {
                    if let Ok(p2) = profile.cast::<ICoreWebView2Profile2>() {
                        let h = webview2_com::ClearBrowsingDataCompletedHandler::create(
                            Box::new(|_| Ok(())),
                        );
                        unsafe {
                            let _ = p2.ClearBrowsingDataAll(&h);
                        }
                    }
                }
            }
            self.reload_active();
            return;
        }

        // Cookies: the domain plus its subdomains.
        if let Ok(wv2) = wv.cast::<ICoreWebView2_2>() {
            if let Ok(cm) = unsafe { wv2.CookieManager() } {
                let dom = wide(host);
                let dot = wide(&format!(".{host}"));
                let path = wide("/");
                let empty = wide("");
                unsafe {
                    let _ = cm.DeleteCookiesWithDomainAndPath(
                        PCWSTR(empty.as_ptr()),
                        PCWSTR(dom.as_ptr()),
                        PCWSTR(path.as_ptr()),
                    );
                    let _ = cm.DeleteCookiesWithDomainAndPath(
                        PCWSTR(empty.as_ptr()),
                        PCWSTR(dot.as_ptr()),
                        PCWSTR(path.as_ptr()),
                    );
                }
            }
        }
        // localStorage / sessionStorage / IndexedDB / Cache API for this origin.
        let js = "(async()=>{try{localStorage.clear();sessionStorage.clear();\
                  if(window.indexedDB?.databases){for(const d of await indexedDB.databases())\
                  if(d.name)indexedDB.deleteDatabase(d.name);}\
                  if(window.caches){for(const k of await caches.keys())await caches.delete(k);}\
                  }catch(e){}})()";
        let w = wide(js);
        unsafe {
            let _ = wv.ExecuteScript(PCWSTR(w.as_ptr()), None);
        }
        self.reload_active();
    }

    fn reload_active(&mut self) {
        let i = self.active;
        if let Some(tab) = self.tabs.get(i) {
            if let Some(wv) = &tab.webview {
                unsafe {
                    let _ = wv.Reload();
                }
            }
        }
        self.paint();
    }

    fn persist_shield_total(&self) {
        self.storage
            .set_setting("shield_total", &crate::adblock::total_blocked().to_string());
    }

    // ---------------- animation ----------------
    fn start_anim(&mut self) {
        if self.anim_on {
            return;
        }
        self.anim_on = true;
        self.anim_last = now_ms();
        unsafe {
            let _ = SetTimer(Some(self.hwnd), TIMER_ANIM, 8, None);
        }
    }

    fn stop_anim(&mut self) {
        if !self.anim_on {
            return;
        }
        self.anim_on = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
        }
    }

    /// Hover intensity (0..1) for a chrome element; cross-fades on change.
    fn hover_t(&self, h: Hot) -> f32 {
        if h == Hot::None {
            0.0
        } else if self.hot == h {
            self.hot_t
        } else if self.hot_prev == h {
            self.hot_prev_t
        } else {
            0.0
        }
    }

    fn anim_tick(&mut self) {
        let now = now_ms();
        let dt = (now.saturating_sub(self.anim_last)) as f32;
        self.anim_last = now;
        let mut busy = false;
        let mut needs_layout = false;

        // sidebar slide
        if (self.sidebar_w - self.sidebar_target).abs() > 0.05 {
            let p = ((now.saturating_sub(self.sb_t0)) as f32 / ANIM_SIDEBAR).clamp(0.0, 1.0);
            self.sidebar_w = self.sb_from + (self.sidebar_target - self.sb_from) * ease_out(p);
            if p >= 1.0 {
                self.sidebar_w = self.sidebar_target;
            } else {
                busy = true;
            }
            needs_layout = true;
        }
        // The easing lands within a hair of the target well before p reaches 1,
        // so release the held content edge here rather than inside the branch —
        // otherwise the page keeps the wider gap forever.
        if (self.sidebar_w - self.sidebar_target).abs() <= 0.05
            && (self.content_left - self.sidebar_target).abs() > 0.05
        {
            self.sidebar_w = self.sidebar_target;
            self.content_left = self.sidebar_target;
            needs_layout = true;
        }

        // smooth tab-list scrolling
        if (self.tab_scroll - self.tab_scroll_target).abs() > 0.3 {
            let k = (dt / 110.0).clamp(0.0, 1.0);
            self.tab_scroll += (self.tab_scroll_target - self.tab_scroll) * k;
            needs_layout = true;
            busy = true;
        } else if self.tab_scroll != self.tab_scroll_target {
            self.tab_scroll = self.tab_scroll_target;
            needs_layout = true;
        }

        // tab rows growing in / shrinking away
        let step = dt / ANIM_TAB;
        let mut drop_any = false;
        for t in &mut self.tabs {
            if t.closing {
                t.appear -= step;
                if t.appear <= 0.0 {
                    t.appear = 0.0;
                    drop_any = true;
                } else {
                    busy = true;
                }
                needs_layout = true;
            } else if t.appear < 1.0 {
                t.appear = (t.appear + step).min(1.0);
                busy |= t.appear < 1.0;
                needs_layout = true;
            }
        }
        if drop_any {
            self.drop_closed_tabs();
            needs_layout = true;
        }

        // keep ticking while a page is loading (progress sweep)
        if self.tabs.get(self.active).map(|t| t.loading).unwrap_or(false) {
            busy = true;
        }

        // hover cross-fade
        if self.hot_t < 1.0 && self.hot != Hot::None {
            self.hot_t = approach(self.hot_t, 1.0, dt, ANIM_HOVER);
            busy |= self.hot_t < 1.0;
        }
        if self.hot_prev_t > 0.0 {
            self.hot_prev_t = approach(self.hot_prev_t, 0.0, dt, ANIM_HOVER);
            busy |= self.hot_prev_t > 0.0;
        }

        // active-tab indicator slide
        let (ty, th) = self.indicator_target();
        if !self.ind_ready {
            self.ind_y = ty;
            self.ind_h = th;
            self.ind_ready = true;
        } else if (self.ind_y - ty).abs() > 0.2 || (self.ind_h - th).abs() > 0.2 {
            let k = (dt / ANIM_INDICATOR).clamp(0.0, 1.0) * 2.2;
            let k = k.min(1.0);
            self.ind_y += (ty - self.ind_y) * k;
            self.ind_h += (th - self.ind_h) * k;
            busy = true;
        }

        if needs_layout {
            self.relayout();
        }
        self.paint();
        if !busy {
            self.stop_anim();
        }
    }

    /// Where the accent bar of the active tab should sit (y, height in DIP).
    fn indicator_target(&self) -> (f32, f32) {
        for (i, r) in &self.layout.tab_rows {
            if *i == self.active {
                let inset = (r.bottom - r.top) * 0.22;
                return (r.top + inset, (r.bottom - r.top) - inset * 2.0);
            }
        }
        (self.ind_y, 0.0)
    }

    // ---------------- tabs ----------------
    pub fn new_tab(&mut self, url: &str, activate: bool) -> usize {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, url);
        tab.zoom = self
            .storage
            .get_setting("default_zoom", "100")
            .parse::<f64>()
            .unwrap_or(100.0)
            .clamp(50.0, 250.0)
            / 100.0;
        if self.theme.reduce_motion {
            tab.appear = 1.0;
        }
        self.tabs.push(tab);
        let idx = self.tabs.len() - 1;
        self.ensure_controller(idx);
        self.start_anim(); // row grows in
        if activate {
            self.active = idx;
            self.layout_webviews();
        }
        self.relayout();
        self.paint();
        idx
    }

    /// Adds a tab without booting a WebView2 controller — used for session
    /// restore, where most tabs are never looked at.
    fn new_tab_lazy(&mut self, url: &str) -> usize {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = Tab::new(id, url);
        tab.zoom = self
            .storage
            .get_setting("default_zoom", "100")
            .parse::<f64>()
            .unwrap_or(100.0)
            .clamp(50.0, 250.0)
            / 100.0;
        // Restored rows are there from the start — no wave of animations.
        tab.appear = 1.0;
        self.tabs.push(tab);
        self.tabs.len() - 1
    }

    /// Keeps the given tab row inside the scrolled viewport.
    fn scroll_into_view(&mut self, idx: usize) {
        let rc = client_rect(self.hwnd);
        let h = rc.bottom as f32 / self.scale;
        let row_h = if self.sidebar_w < 150.0 { 48.0 } else { 40.0 };
        let view_h = ((h - 104.0) - 66.0).max(row_h);
        let y: f32 = self.tabs.iter().take(idx).map(|t| row_h * t.appear).sum();
        if y < self.tab_scroll_target {
            self.tab_scroll_target = y;
        } else if y + row_h > self.tab_scroll_target + view_h {
            self.tab_scroll_target = y + row_h - view_h;
        }
        self.tab_scroll_target = self.tab_scroll_target.max(0.0);
        if self.theme.reduce_motion {
            self.tab_scroll = self.tab_scroll_target;
        } else {
            self.start_anim();
        }
    }

    /// Boots the WebView2 controller for a tab if it does not have one yet.
    fn ensure_controller(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        if tab.controller.is_some() || tab.spawning {
            return;
        }
        tab.spawning = true;
        let id = tab.id;
        if let Some(env) = &self.env {
            tabs::spawn_controller(env, self.hwnd, id);
        }
    }

    pub fn activate_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() || idx == self.active {
            return;
        }
        let prev = self.active;
        if let Some(t) = self.tabs.get_mut(prev) {
            t.last_active = now_ms();
        }
        self.active = idx;
        self.scroll_into_view(idx);
        // Session-restored tabs boot on first use.
        self.ensure_controller(idx);
        let tab = &mut self.tabs[idx];
        tab.last_active = now_ms();
        if tab.asleep {
            if let Some(wv) = &tab.webview {
                if let Ok(wv3) = wv.cast::<ICoreWebView2_3>() {
                    unsafe {
                        let _ = wv3.Resume();
                    }
                }
            }
            tab.asleep = false;
        }
        self.layout_webviews();
        if let Some(t) = self.tabs.get(idx) {
            if let Some(ctl) = &t.controller {
                unsafe {
                    let _ = ctl.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                }
            }
        }
        self.paint();
    }

    /// Closes a tab. The row shrinks away first; the entry is dropped in
    /// `drop_closed_tabs` once the animation has finished.
    pub fn close_tab(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        if tab.closing {
            return;
        }
        tab.closing = true;
        let (id, url, title) = (tab.id, tab.url.clone(), tab.title.clone());
        // Tear the engine down right away — only the row lingers.
        if let Some(ctl) = tab.controller.take() {
            unsafe {
                let _ = ctl.Close();
            }
        }
        tab.webview = None;
        tab.guards.clear();
        tab.last_bounds = None;
        tab.last_visible = None;
        crate::adblock::forget_tab(id);
        if !self.private {
            self.storage.push_closed_tab(&url, &title);
        }
        if self.split == Some(id) {
            self.split = None;
        }
        // Move focus off the dying tab immediately.
        if self.active == idx {
            let next = self
                .tabs
                .iter()
                .enumerate()
                .filter(|(i, t)| !t.closing && *i != idx)
                .min_by_key(|(i, _)| (*i as i32 - idx as i32).abs())
                .map(|(i, _)| i);
            if let Some(n) = next {
                self.active = n;
                self.ensure_controller(n);
                if let Some(t) = self.tabs.get_mut(n) {
                    t.last_active = now_ms();
                }
            }
        }
        if self.theme.reduce_motion {
            if let Some(t) = self.tabs.get_mut(idx) {
                t.appear = 0.0;
            }
            self.drop_closed_tabs();
        } else {
            self.start_anim();
        }
        self.layout_webviews();
        self.relayout();
        self.paint();
    }

    /// Removes finished-closing tabs and repairs the active index.
    fn drop_closed_tabs(&mut self) {
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        self.tabs.retain(|t| !(t.closing && t.appear <= 0.0));
        self.active = active_id
            .and_then(|id| self.tabs.iter().position(|t| t.id == id))
            .unwrap_or_else(|| self.active.min(self.tabs.len().saturating_sub(1)));

        if self.tabs.iter().all(|t| t.closing) {
            if self.storage.get_setting("close_last_tab", "0") == "1" {
                self.close_window();
            } else {
                let u = self.new_tab_url();
                self.new_tab(&u, true);
            }
        }
        self.layout_webviews();
    }

    fn tab_index_by_id(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    pub fn navigate(&mut self, idx: usize, display_url: &str) {
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        tab.is_internal = display_url.starts_with("aura://");
        tab.loading = true;
        let real = tabs::real_url(display_url);
        if let Some(wv) = &tab.webview {
            let wurl = wide(&real);
            unsafe {
                let _ = wv.Navigate(PCWSTR(wurl.as_ptr()));
            }
        } else {
            tab.pending_url = Some(display_url.to_string());
        }
        tab.url = display_url.to_string();
        self.start_anim(); // drives the loading sweep
        self.paint();
    }

    fn go_back(&mut self) {
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                unsafe {
                    let _ = wv.GoBack();
                }
            }
        }
    }

    fn go_fwd(&mut self) {
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                unsafe {
                    let _ = wv.GoForward();
                }
            }
        }
    }

    fn reload(&mut self) {
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                unsafe {
                    if t.loading {
                        let _ = wv.Stop();
                    } else {
                        let _ = wv.Reload();
                    }
                }
            }
        }
    }

    pub fn open_internal(&mut self, url: &str) {
        // Reuse an existing internal tab of this kind if present.
        if let Some(i) = self.tabs.iter().position(|t| t.url == url) {
            self.activate_tab(i);
        } else {
            self.new_tab(url, true);
        }
    }

    // ---------------- async message pump ----------------
    fn on_sync(&mut self) {
        let msgs: Vec<AppMsg> = MSGQ.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for msg in msgs {
            self.handle_msg(msg);
        }
    }

    fn handle_msg(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::ControllerReady { tab, controller } => self.on_controller_ready(tab, controller),
            AppMsg::ControllerFailed { tab } => {
                if let Some(i) = self.tab_index_by_id(tab) {
                    self.tabs[i].title = "Fehler".into();
                    self.paint();
                }
            }
            AppMsg::Title { tab, title } => {
                if let Some(i) = self.tab_index_by_id(tab) {
                    if !title.is_empty() {
                        self.tabs[i].title = title;
                        self.paint();
                    }
                }
            }
            AppMsg::Source { tab, url } => {
                if let Some(i) = self.tab_index_by_id(tab) {
                    let display = tabs::display_url(&url);
                    self.tabs[i].url = display.clone();
                    self.tabs[i].is_internal = display.starts_with("aura://");
                    if i == self.active {
                        self.paint();
                    }
                }
            }
            AppMsg::NavCompleted { tab, url, title, can_back, can_fwd } => {
                if let Some(i) = self.tab_index_by_id(tab) {
                    let display = tabs::display_url(&url);
                    let t = &mut self.tabs[i];
                    t.url = display.clone();
                    t.is_internal = display.starts_with("aura://");
                    t.loading = false;
                    t.can_back = can_back;
                    t.can_fwd = can_fwd;
                    if !title.is_empty() {
                        t.title = title.clone();
                    }
                    if !self.private && !t.is_internal && !display.is_empty() {
                        self.storage.add_history(&display, &t.title, t.favicon_png.as_deref());
                    }
                    if t.is_internal {
                        crate::pages::send_init(self, tab);
                    }
                    self.paint();
                }
            }
            AppMsg::Favicon { tab, bytes } => {
                if let Some(i) = self.tab_index_by_id(tab) {
                    self.tabs[i].favicon = None;
                    self.tabs[i].favicon_png = Some(bytes);
                    self.paint();
                }
            }
            AppMsg::NewWindow { uri, user_initiated, tab } => {
                // Popunders on streaming sites are click-triggered, so "user
                // initiated" is not enough — check the target against the lists.
                if crate::adblock::is_blocked_popup(tab, &uri) {
                    return;
                }
                let block = self.storage.get_setting("block_popups", "1") == "1"
                    || crate::adblock::tab_is_strict(tab);
                if !block || user_initiated {
                    // Ctrl+click / middle-click opens in the background, like every
                    // other browser; a plain target=_blank comes to the front.
                    let bg = unsafe {
                        GetKeyState(VK_CONTROL.0 as i32) < 0 || GetKeyState(VK_MBUTTON.0 as i32) < 0
                    };
                    self.new_tab(&uri, !bg);
                }
            }
            AppMsg::Permission { kind, uri, args, deferral, .. } => {
                let origin = host_of(&uri);
                // Hardened sites never get camera, microphone or location.
                if crate::adblock::is_strict(&origin) {
                    unsafe {
                        let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                        let _ = deferral.Complete();
                    }
                    return;
                }
                match self.storage.permission(&origin, &kind) {
                    Some(true) => unsafe {
                        let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW);
                        let _ = deferral.Complete();
                    },
                    Some(false) => unsafe {
                        let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                        let _ = deferral.Complete();
                    },
                    None => {
                        self.pending_permission = Some((args, deferral, kind.clone(), origin.clone()));
                        popup::show_permission(self, &kind, &origin);
                    }
                }
            }
            AppMsg::PermissionAnswer { allow, remember } => {
                if let Some(p) = self.dialog_popup.take() {
                    p.close();
                }
                if let Some((args, deferral, kind, origin)) = self.pending_permission.take() {
                    if remember {
                        self.storage.set_permission(&origin, &kind, allow);
                    }
                    unsafe {
                        let _ = args.SetState(if allow {
                            COREWEBVIEW2_PERMISSION_STATE_ALLOW
                        } else {
                            COREWEBVIEW2_PERMISSION_STATE_DENY
                        });
                        let _ = deferral.Complete();
                    }
                }
            }
            AppMsg::WebMessage { tab, json } => crate::pages::handle_message(self, tab, &json),
            AppMsg::Fullscreen { contains, .. } => {
                self.fs_element = contains;
                self.relayout();
                self.paint();
            }
            AppMsg::Audio { tab, playing } => {
                if let Some(i) = self.tab_index_by_id(tab) {
                    self.tabs[i].playing_audio = playing;
                    self.paint();
                }
            }
            AppMsg::DownloadStart { uri, args, deferral, op, tab } => {
                // Hardened sites may not put files on disk.
                if crate::adblock::tab_is_strict(tab) {
                    unsafe {
                        let _ = args.SetCancel(true);
                        let _ = deferral.Complete();
                    }
                    return;
                }
                self.on_download_start(&uri, &args, &deferral, &op);
            }
            AppMsg::DownloadProgress { dl, received, total, state } => {
                let finished = state == 1; // COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
                self.storage.update_download(dl, received, total, finished);
            }
            AppMsg::Accel { vk, ctrl, shift, alt, .. } => {
                self.shortcut(vk, ctrl, shift, alt);
            }
            AppMsg::UpgradeToHttps { tab, url } => {
                if let Some(i) = self.tab_index_by_id(tab) {
                    self.navigate(i, &url);
                }
            }
            AppMsg::MenuAction { action } => {
                if let Some(p) = self.menu_popup.take() {
                    p.close();
                }
                self.exec_action(&action);
            }
            AppMsg::OmniSubmit { edit } => {
                if edit == self.edit {
                    self.submit_omnibox();
                } else if edit == self.find_edit {
                    self.find_next(false);
                }
            }
            AppMsg::OmniCancel { edit } => {
                if edit == self.edit {
                    self.end_edit(false);
                } else if edit == self.find_edit {
                    self.close_find();
                }
            }
            AppMsg::OmniNav { edit, delta } => {
                if edit == self.edit {
                    omnibox::navigate_suggestions(self, delta);
                } else if edit == self.find_edit {
                    self.find_next(delta < 0);
                }
            }
        }
    }

    fn on_controller_ready(&mut self, tab_id: u32, controller: ICoreWebView2Controller) {
        let Some(i) = self.tab_index_by_id(tab_id) else { return };
        let Ok(webview) = (unsafe { controller.CoreWebView2() }) else { return };

        // Map aura.internal to the local assets folder.
        if let Ok(wv3) = webview.cast::<ICoreWebView2_3>() {
            let dir = crate::storage::assets_dir();
            let d = wide(&dir.to_string_lossy());
            unsafe {
                let _ = wv3.SetVirtualHostNameToFolderMapping(
                    w!("aura.internal"),
                    PCWSTR(d.as_ptr()),
                    COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
                );
            }
        }

        let Some(env) = self.env.clone() else { return };
        let guards = tabs::attach_events(tab_id, &webview, &controller, &env);
        let tab = &mut self.tabs[i];
        tab.spawning = false;
        tab.guards = guards;
        tab.webview = Some(webview.clone());
        tab.controller = Some(controller);

        // Apply zoom.
        if let Some(ctl) = &tab.controller {
            unsafe {
                let _ = ctl.SetZoomFactor(tab.zoom);
            }
        }

        let pending = tab.pending_url.take();
        if let Some(url) = pending {
            let real = tabs::real_url(&url);
            let wurl = wide(&real);
            if let Some(wv) = &tab.webview {
                unsafe {
                    let _ = wv.Navigate(PCWSTR(wurl.as_ptr()));
                }
            }
            tab.loading = true;
        }
        self.layout_webviews();
        self.paint();
    }

    // ---------------- downloads ----------------
    fn on_download_start(
        &mut self,
        uri: &str,
        args: &ICoreWebView2DownloadStartingEventArgs,
        deferral: &ICoreWebView2Deferral,
        op: &ICoreWebView2DownloadOperation,
    ) {
        let filename = uri
            .rsplit('/')
            .next()
            .unwrap_or("download")
            .split(['?', '#'])
            .next()
            .unwrap_or("download")
            .to_string();
        let filename = if filename.is_empty() { "download".into() } else { filename };
        let configured = self.storage.get_setting("download_dir", "");
        let dir = if configured.is_empty() {
            std::env::var("USERPROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join("Downloads")
                .join("Aura")
        } else {
            std::path::PathBuf::from(configured)
        };
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(&filename);
        let dl_id = self.storage.add_download(&path.to_string_lossy(), uri, &filename);

        // Track progress.
        let mut token: i64 = 0;
        let h = webview2_com::BytesReceivedChangedEventHandler::create(Box::new(move |op, _| {
            if let Some(op) = op {
                let mut r: i64 = 0;
                let mut t: i64 = 0;
                unsafe {
                    let _ = op.BytesReceived(&mut r);
                    let _ = op.TotalBytesToReceive(&mut t);
                }
                post(AppMsg::DownloadProgress { dl: dl_id, received: r, total: t, state: 0 });
            }
            Ok(())
        }));
        unsafe {
            let _ = op.add_BytesReceivedChanged(&h, &mut token);
        }
        self.dl_guards.push(Box::new(h));
        let h = webview2_com::StateChangedEventHandler::create(Box::new(move |op, _| {
            if let Some(op) = op {
                let mut state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
                let mut r: i64 = 0;
                let mut t: i64 = 0;
                unsafe {
                    let _ = op.State(&mut state);
                    let _ = op.BytesReceived(&mut r);
                    let _ = op.TotalBytesToReceive(&mut t);
                }
                let code = match state {
                    COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED => 1,
                    COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED => 2,
                    _ => 0,
                };
                post(AppMsg::DownloadProgress { dl: dl_id, received: r, total: t, state: code });
            }
            Ok(())
        }));
        unsafe {
            let _ = op.add_StateChanged(&h, &mut token);
        }
        self.dl_guards.push(Box::new(h));

        let p = wide(&path.to_string_lossy());
        unsafe {
            let _ = args.SetResultFilePath(PCWSTR(p.as_ptr()));
            let _ = args.SetHandled(true);
            let _ = deferral.Complete();
        }
    }

    // ---------------- menus ----------------
    fn show_tab_menu(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let tab = &self.tabs[idx];
        let pinned = tab.pinned;
        let muted = tab.muted;
        let groups = self.storage.list_groups();
        let mut items = vec![
            MenuItem::new("Tab schließen", "E711", &format!("close:{idx}")),
            MenuItem::new("Andere Tabs schließen", "E8BB", &format!("closeothers:{idx}")),
            MenuItem::new("Tabs rechts schließen", "E8BB", &format!("closeright:{idx}")),
            MenuItem::sep(),
            MenuItem::new("Tab duplizieren", "E8C8", &format!("duplicate:{idx}")),
            MenuItem::new(if pinned { "Tab lösen" } else { "Tab anheften" }, "E718", &format!("pin:{idx}")),
            MenuItem::new(if muted { "Stummschaltung aufheben" } else { "Tab stummschalten" }, "E74F", &format!("mute:{idx}")),
        ];
        if groups.is_empty() {
            items.push(MenuItem::new("Neue Gruppe aus Tab", "E8A5", &format!("groupnew:{idx}")));
        } else {
            for g in &groups {
                items.push(MenuItem::new(
                    &format!("Gruppe: {}", g.name),
                    "E8A5",
                    &format!("group:{idx}:{}", g.id),
                ));
            }
            items.push(MenuItem::new("Neue Gruppe…", "E710", &format!("groupnew:{idx}")));
        }
        items.push(MenuItem::sep());
        items.push(MenuItem::new("Seite neu laden", "E72C", &format!("reload:{idx}")));
        items.push(MenuItem::new("Adresse kopieren", "E8C8", &format!("copyurl:{idx}")));
        items.push(MenuItem::new("DevTools öffnen", "EC7A", &format!("devtools:{idx}")));
        let p = cursor_pos();
        popup::show_menu(self, p.x, p.y, items);
    }

    fn show_main_menu(&mut self) {
        let items = vec![
            MenuItem::new("Neuer Tab", "E710", "newtab").shortcut("Strg+T"),
            MenuItem::new("Neues Fenster", "E8A7", "newwindow").shortcut("Strg+N"),
            MenuItem::new("Privates Fenster", "EA9B", "private"),
            MenuItem::sep(),
            MenuItem::new("Geteilte Ansicht", "F584", "split"),
            MenuItem::new("Bild-in-Bild", "E91D", "pip"),
            MenuItem::new("Vollbild", "E740", "fullscreen").shortcut("F11"),
            MenuItem::sep(),
            MenuItem::new("Verlauf", "E81C", "history").shortcut("Strg+H"),
            MenuItem::new("Lesezeichen", "E8A4", "bookmarks").shortcut("Strg+Umschalt+B"),
            MenuItem::new("Leseliste", "E8F1", "reading").shortcut("Strg+Umschalt+E"),
            MenuItem::new("Downloads", "E896", "downloads").shortcut("Strg+J"),
            MenuItem::new("Passwörter", "E8D7", "passwords"),
            MenuItem::sep(),
            MenuItem::new(
                if self
                    .tabs
                    .get(self.active)
                    .map(|t| self.storage.reading_has(&t.url))
                    .unwrap_or(false)
                {
                    "Aus Leseliste entfernen"
                } else {
                    "Später lesen"
                },
                "E7B8",
                "read_later",
            )
            .shortcut("Strg+Umschalt+L"),
            MenuItem::new("Seite übersetzen", "F2B7", "translate"),
            MenuItem::new("Tabs durchsuchen", "E721", "tabsearch").shortcut("Strg+Umschalt+A"),
            MenuItem::new("Task-Manager", "E9D9", "tasks").shortcut("Umschalt+Esc"),
            MenuItem::sep(),
            MenuItem::new("Zoom +", "E71E", "zoomin").shortcut("Strg++"),
            MenuItem::new("Zoom −", "E71F", "zoomout").shortcut("Strg+−"),
            MenuItem::new("Zoom zurücksetzen", "E7A2", "zoomreset").shortcut("Strg+0"),
            MenuItem::sep(),
            MenuItem::new("Seite durchsuchen", "E721", "find").shortcut("Strg+F"),
            MenuItem::new("Drucken", "E749", "print").shortcut("Strg+P"),
            MenuItem::new("Geschlossenen Tab öffnen", "E777", "reopen").shortcut("Strg+Umschalt+T"),
            MenuItem::sep(),
            MenuItem::new("Aus Chrome importieren", "E782", "import"),
            MenuItem::new("Einstellungen", "E713", "settings").shortcut("Strg+,"),
        ];
        let r = self.layout.menu;
        let mut pt = POINT { x: ((r.right) * self.scale) as i32, y: ((r.bottom + 4.0) * self.scale) as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        popup::show_menu(self, pt.x, pt.y, items);
    }

    fn show_shield_menu(&mut self) {
        let tab = self.tabs.get(self.active);
        let host = tab.map(|t| host_of(&t.url)).unwrap_or_default();
        let site = crate::adblock::base_domain(&host).to_string();
        let n = tab.map(|t| crate::adblock::blocked_for(t.id)).unwrap_or(0);
        let (total, rules, cosmetic, lists) = crate::adblock::stats();
        let on = crate::adblock::is_enabled();
        let site_on = on && !crate::adblock::is_allowlisted(&host);

        let mut items = vec![
            MenuItem::new(&format!("Auf dieser Seite blockiert: {n}"), "EA18", "noop"),
            MenuItem::new(&format!("Insgesamt blockiert: {total}"), "E9D9", "noop"),
            MenuItem::sep(),
        ];
        if !site.is_empty() && site != "aura" {
            items.push(MenuItem::new(
                &if site_on {
                    format!("Für {site} deaktivieren")
                } else {
                    format!("Für {site} aktivieren")
                },
                if site_on { "E7B3" } else { "EA18" },
                "shield_site",
            ));
            let strict = crate::adblock::is_strict(&host);
            items.push(MenuItem::new(
                if strict {
                    "Strenger Modus: an"
                } else {
                    "Strenger Modus für diese Seite"
                },
                if strict { "E72E" } else { "E785" },
                "shield_strict",
            ));
            if crate::adblock::https_only() && !crate::adblock::http_allowed(&host) {
                items.push(MenuItem::new("HTTP für diese Seite erlauben", "E785", "allow_http"));
            }
        }
        items.push(MenuItem::new(
            if on { "Shield ausschalten" } else { "Shield einschalten" },
            if on { "E7B3" } else { "EA18" },
            "shield_toggle",
        ));
        items.push(MenuItem::sep());
        items.push(MenuItem::new(
            &format!("{} Regeln · {} Kosmetik", fmt_thousands(rules as u64), fmt_thousands(cosmetic as u64)),
            "E8A5",
            "noop",
        ));
        for (name, n) in lists.iter().take(8) {
            items.push(MenuItem::new(&format!("{name} · {}", fmt_thousands(*n as u64)), "E8A5", "noop"));
        }
        items.push(MenuItem::sep());
        if !site.is_empty() && site != "aura" {
            items.push(MenuItem::new(
                &format!("Cookies & Daten von {site} löschen"),
                "E74D",
                "clear_site",
            ));
        }
        items.push(MenuItem::new("Filterlisten aktualisieren", "E72C", "shield_update"));

        let r = self.layout.shield;
        let mut pt = POINT { x: ((r.right + 60.0) * self.scale) as i32, y: ((r.bottom + 4.0) * self.scale) as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        popup::show_menu(self, pt.x, pt.y, items);
    }

    // ---------------- actions ----------------
    pub fn exec_action(&mut self, action: &str) {
        let mut parts = action.split(':');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().and_then(|s| s.parse::<usize>().ok());
        match cmd {
            "close" => { if let Some(i) = arg { self.close_tab(i); } }
            "closeothers" => {
                if let Some(i) = arg {
                    let keep = self.tabs[i].id;
                    let mut j = self.tabs.len();
                    while j > 0 {
                        j -= 1;
                        if self.tabs[j].id != keep {
                            self.close_tab(j);
                        }
                    }
                }
            }
            "closeright" => {
                if let Some(i) = arg {
                    while self.tabs.len() > i + 1 {
                        self.close_tab(self.tabs.len() - 1);
                    }
                }
            }
            "duplicate" => {
                if let Some(i) = arg {
                    let url = self.tabs[i].url.clone();
                    self.new_tab(&url, true);
                }
            }
            "pin" => {
                if let Some(i) = arg {
                    self.tabs[i].pinned = !self.tabs[i].pinned;
                    // Pinned tabs float to the top.
                    let tab = self.tabs.remove(i);
                    let pos = self.tabs.iter().take_while(|t| t.pinned).count();
                    self.tabs.insert(pos, tab);
                    self.active = pos;
                    self.relayout();
                    self.layout_webviews();
                    self.paint();
                }
            }
            "mute" => {
                if let Some(i) = arg {
                    let muted = !self.tabs[i].muted;
                    self.tabs[i].muted = muted;
                    if let Some(wv) = &self.tabs[i].webview {
                        if let Ok(wv8) = wv.cast::<ICoreWebView2_8>() {
                            unsafe {
                                let _ = wv8.SetIsMuted(muted);
                            }
                        }
                    }
                    self.paint();
                }
            }
            "group" => {
                if let (Some(i), Some(gid)) = (arg, parts.next().and_then(|s| s.parse::<i64>().ok())) {
                    self.tabs[i].group = Some(gid);
                    self.paint();
                }
            }
            "groupnew" => {
                if let Some(i) = arg {
                    let n = self.storage.list_groups().len() + 1;
                    let color = crate::storage::GROUP_COLORS[(n - 1) % crate::storage::GROUP_COLORS.len()].1;
                    let gid = self.storage.add_group(&format!("Gruppe {n}"), color);
                    self.tabs[i].group = Some(gid);
                    self.paint();
                }
            }
            "reload" => { if let Some(i) = arg { self.reload_tab(i); } }
            "copyurl" => {
                if let Some(i) = arg {
                    let url = self.tabs[i].url.clone();
                    copy_to_clipboard(self.hwnd, &url);
                }
            }
            "devtools" => {
                if let Some(i) = arg {
                    if let Some(wv) = &self.tabs[i].webview {
                        unsafe {
                            let _ = wv.OpenDevToolsWindow();
                        }
                    }
                }
            }
            "sugg" => {
                // Click on a suggestion row in the omnibox dropdown.
                if let Some(i) = arg {
                    let s = if let Some(p) = &self.sugg_popup {
                        if let crate::popup::PopupKind::Suggestions { items, .. } = &p.kind {
                            items.get(i).cloned()
                        } else { None }
                    } else { None };
                    if let Some(p) = self.sugg_popup.take() {
                        p.close();
                    }
                    if let Some(s) = s {
                        self.end_edit(true);
                        self.accept_suggestion(&s);
                    }
                }
            }
            "newtab" => { let u = self.new_tab_url(); self.new_tab(&u, true); }
            "newwindow" => self.spawn_window(&[]),
            "private" => self.spawn_window(&["--private"]),
            "split" => self.toggle_split(),
            "pip" => self.picture_in_picture(),
            "fullscreen" => self.toggle_fullscreen(),
            "history" => self.open_internal("aura://history"),
            "bookmarks" => self.open_internal("aura://bookmarks"),
            "downloads" => self.open_internal("aura://downloads"),
            "settings" => self.open_internal("aura://settings"),
            "import" => self.open_internal("aura://import"),
            "zoomin" => self.zoom(0.1),
            "zoomout" => self.zoom(-0.1),
            "zoomreset" => self.zoom_to(1.0),
            "find" => self.open_find(),
            "print" => self.print_page(),
            "reopen" => self.reopen_closed(),
            "palette" => self.open_palette(),
            "sleep_active" => self.sleep_active_tab(),
            "shield_site" => self.toggle_shield_site(),
            "shield_toggle" => {
                let on = !crate::adblock::is_enabled();
                self.set_shield(on);
            }
            "shield_update" => self.update_filters_now(),
            "reading" => self.open_internal("aura://reading"),
            "passwords" => self.open_internal("aura://passwords"),
            "tasks" => self.open_internal("aura://tasks"),
            "read_later" => self.add_to_reading_list(),
            "translate" => self.translate_page(),
            "tabsearch" => self.open_tab_search(),
            "allow_http" => {
                let host = self.tabs.get(self.active).map(|t| host_of(&t.url)).unwrap_or_default();
                if !host.is_empty() {
                    crate::adblock::allow_http(&host);
                    let idx = self.active;
                    let url = format!("http://{host}/");
                    self.navigate(idx, &url);
                }
            }
            "shield_strict" => {
                let host = self.tabs.get(self.active).map(|t| host_of(&t.url)).unwrap_or_default();
                if !host.is_empty() {
                    crate::adblock::toggle_strict(&host);
                    let list = crate::adblock::strict_sites().join(",");
                    self.storage.set_setting("shield_strict", &list);
                    self.reload_active();
                }
            }
            "clear_site" => {
                let host = self.tabs.get(self.active).map(|t| host_of(&t.url)).unwrap_or_default();
                let site = crate::adblock::base_domain(&host).to_string();
                self.clear_site_data(&site);
            }
            _ => {}
        }
    }

    fn reload_tab(&mut self, i: usize) {
        if let Some(wv) = &self.tabs[i].webview {
            unsafe {
                let _ = wv.Reload();
            }
        }
    }

    fn spawn_window(&self, extra: &[&str]) {
        if let Ok(exe) = std::env::current_exe() {
            let mut cmd = std::process::Command::new(exe);
            cmd.arg(format!("--profile={}", self.profile));
            for e in extra {
                cmd.arg(e);
            }
            let _ = cmd.spawn();
        }
    }

    fn toggle_split(&mut self) {
        if self.split.is_some() {
            self.split = None;
        } else if self.tabs.len() > 1 {
            let other = (self.active + 1) % self.tabs.len();
            self.split = Some(self.tabs[other].id);
        }
        self.layout_webviews();
        self.paint();
    }

    fn picture_in_picture(&self) {
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                let script = w!("(function(){const v=document.querySelector('video');if(v){if(document.pictureInPictureElement){document.exitPictureInPicture();}else{v.requestPictureInPicture();}}})()");
                unsafe {
                    let _ = wv.ExecuteScript(script, None);
                }
            }
        }
    }

    fn toggle_fullscreen(&mut self) {
        unsafe {
            if !self.fullscreen {
                let mut wp = WINDOWPLACEMENT {
                    length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                let _ = GetWindowPlacement(self.hwnd, &mut wp);
                self.saved_placement = wp;
                let style = GetWindowLongPtrW(self.hwnd, GWL_STYLE);
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, style & !(WS_OVERLAPPEDWINDOW.0 as isize) | WS_POPUP.0 as isize | WS_VISIBLE.0 as isize);
                let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(mon, &mut info);
                let r = info.rcMonitor;
                let _ = SetWindowPos(
                    self.hwnd, Some(HWND_TOP),
                    r.left, r.top,
                    r.right - r.left, r.bottom - r.top,
                    SWP_FRAMECHANGED | SWP_SHOWWINDOW,
                );
                self.fullscreen = true;
            } else {
                let style = GetWindowLongPtrW(self.hwnd, GWL_STYLE);
                SetWindowLongPtrW(
                    self.hwnd, GWL_STYLE,
                    (style & !(WS_POPUP.0 as isize)) | WS_OVERLAPPEDWINDOW.0 as isize | WS_CLIPCHILDREN.0 as isize,
                );
                let _ = SetWindowPlacement(self.hwnd, &self.saved_placement);
                let _ = SetWindowPos(
                    self.hwnd, None, 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
                );
                self.fullscreen = false;
            }
        }
    }

    fn zoom(&mut self, delta: f64) {
        let t = &mut self.tabs[self.active];
        t.zoom = (t.zoom + delta).clamp(0.25, 5.0);
        if let Some(ctl) = &t.controller {
            unsafe {
                let _ = ctl.SetZoomFactor(t.zoom);
            }
        }
    }

    pub fn zoom_to(&mut self, z: f64) {
        let t = &mut self.tabs[self.active];
        t.zoom = z;
        if let Some(ctl) = &t.controller {
            unsafe {
                let _ = ctl.SetZoomFactor(z);
            }
        }
    }

    fn print_page(&self) {
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                unsafe {
                    let _ = wv.ExecuteScript(w!("window.print()"), None);
                }
            }
        }
    }

    fn reopen_closed(&mut self) {
        if let Some((url, _)) = self.storage.pop_closed_tab() {
            self.new_tab(&url, true);
        }
    }

    fn open_palette(&mut self) {
        self.begin_edit();
        omnibox::set_edit_text(self.edit, ">");
        self.on_omnibox_changed();
    }

    fn sleep_active_tab(&mut self) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            if let Some(wv) = &t.webview {
                if let Ok(wv3) = wv.cast::<ICoreWebView2_3>() {
                    let h = webview2_com::TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
                    unsafe {
                        let _ = wv3.TrySuspend(&h);
                    }
                    t.asleep = true;
                }
            }
        }
    }

    fn sleep_tick(&mut self) {
        if self.storage.get_setting("sleep_tabs", "1") != "1" {
            return;
        }
        // Grace period before a background tab is suspended.
        let after_ms = self
            .storage
            .get_setting("sleep_after_min", "15")
            .parse::<u64>()
            .unwrap_or(15)
            .max(1)
            * 60_000;
        let now = now_ms();
        let active_id = self.tabs.get(self.active).map(|t| t.id).unwrap_or(0);
        let eligible: Vec<u32> = self
            .tabs
            .iter()
            .filter(|t| {
                t.id != active_id
                    && !t.pinned
                    && !t.playing_audio
                    && !t.asleep
                    && !t.is_internal
                    && t.webview.is_some()
                    && now.saturating_sub(t.last_active) >= after_ms
            })
            .map(|t| t.id)
            .collect();
        for id in eligible {
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) {
                if let Some(wv) = &t.webview {
                    if let Ok(wv3) = wv.cast::<ICoreWebView2_3>() {
                        let h = webview2_com::TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
                        unsafe {
                            let _ = wv3.TrySuspend(&h);
                        }
                        t.asleep = true;
                    }
                }
            }
        }
    }

    // ---------------- omnibox ----------------
    pub fn begin_edit(&mut self) {
        self.editing = true;
        let url = self.tabs.get(self.active).map(|t| t.url.clone()).unwrap_or_default();
        omnibox::set_edit_text(self.edit, &url);
        unsafe {
            let _ = ShowWindow(self.edit, SW_SHOW);
            let _ = SetFocus(Some(self.edit));
        }
        send_msg(self.edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
        self.on_omnibox_changed();
        self.paint();
    }

    pub fn end_edit(&mut self, keep: bool) {
        let _ = keep;
        self.editing = false;
        unsafe {
            let _ = ShowWindow(self.edit, SW_HIDE);
            let _ = SetFocus(Some(self.hwnd));
        }
        if let Some(p) = self.sugg_popup.take() {
            p.close();
        }
        self.paint();
    }

    fn on_omnibox_changed(&mut self) {
        if self.storage.get_setting("suggestions", "1") != "1" {
            if let Some(p) = self.sugg_popup.take() {
                p.close();
            }
            return;
        }
        let text = omnibox::edit_text(self.edit);
        let items = omnibox::build_suggestions(self, &text);
        if items.is_empty() {
            if let Some(p) = self.sugg_popup.take() {
                p.close();
            }
        } else {
            popup::show_suggestions(self, items);
        }
    }

    fn submit_omnibox(&mut self) {
        // Selected suggestion wins; otherwise treat input as URL/search.
        if let Some(p) = &self.sugg_popup {
            if let PopupKind::Suggestions { items, selected } = &p.kind {
                if let Some(s) = items.get(*selected) {
                    let s = s.clone();
                    self.end_edit(true);
                    self.accept_suggestion(&s);
                    return;
                }
            }
        }
        let text = omnibox::edit_text(self.edit);
        self.end_edit(true);
        let url = omnibox::resolve_input(self, &text);
        if !url.is_empty() {
            let idx = self.active;
            self.navigate(idx, &url);
        }
    }

    pub fn accept_suggestion(&mut self, s: &Suggestion) {
        match s.kind {
            omnibox::SuggKind::Tab => {
                if let Some(i) = self.tabs.iter().position(|t| t.url == s.url) {
                    self.activate_tab(i);
                    return;
                }
                let idx = self.active;
                self.navigate(idx, &s.url);
            }
            omnibox::SuggKind::Command => {
                if let Some(a) = &s.action {
                    let a = a.clone();
                    self.exec_action(&a);
                }
            }
            _ => {
                let idx = self.active;
                self.navigate(idx, &s.url);
            }
        }
    }

    // ---------------- find bar ----------------
    fn open_find(&mut self) {
        self.find_open = true;
        unsafe {
            let _ = ShowWindow(self.find_edit, SW_SHOW);
            let _ = SetFocus(Some(self.find_edit));
        }
        send_msg(self.find_edit, EM_SETSEL, WPARAM(0), LPARAM(-1));
    }

    fn close_find(&mut self) {
        self.find_open = false;
        unsafe {
            let _ = ShowWindow(self.find_edit, SW_HIDE);
            let _ = SetFocus(Some(self.hwnd));
        }
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                unsafe {
                    let _ = wv.ExecuteScript(w!("window.getSelection()&&window.getSelection().removeAllRanges()"), None);
                }
            }
        }
    }

    fn on_find_changed(&mut self) {
        let term = omnibox::edit_text(self.find_edit);
        self.find_script(&term, false);
    }

    fn find_next(&mut self, backwards: bool) {
        let term = omnibox::edit_text(self.find_edit);
        self.find_script(&term, backwards);
    }

    fn find_script(&self, term: &str, backwards: bool) {
        if term.is_empty() {
            return;
        }
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                let json = serde_json::to_string(term).unwrap_or_default();
                let script = format!("window.find({json},false,{backwards},true)");
                let w = wide(&script);
                unsafe {
                    let _ = wv.ExecuteScript(PCWSTR(w.as_ptr()), None);
                }
            }
        }
    }

    // ---------------- bookmarks ----------------
    fn toggle_bookmark(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        let url = t.url.clone();
        if url.is_empty() || url.starts_with("aura://") {
            return;
        }
        if let Some(id) = self.storage.is_bookmarked(&url) {
            self.storage.remove_bookmark(id);
        } else {
            self.storage.add_bookmark(&t.title, &url, t.favicon_png.as_deref(), 0);
        }
        self.paint();
    }

    // ---------------- shortcuts ----------------
    pub fn shortcut(&mut self, vk: u32, ctrl: bool, shift: bool, alt: bool) -> bool {
        match (vk, ctrl, shift, alt) {
            (0x54, true, false, false) => {                                                  // Ctrl+T
                let u = self.new_tab_url();
                self.new_tab(&u, true);
                true
            }
            (0x57, true, false, false) => { let i = self.active; self.close_tab(i); true }  // Ctrl+W
            (0x4E, true, false, false) => { self.spawn_window(&[]); true }                  // Ctrl+N
            (0x4E, true, true, false) => { self.spawn_window(&["--private"]); true }        // Ctrl+Shift+N
            (0x4C, true, false, false) => { self.begin_edit(); true }                       // Ctrl+L
            (0x4B, true, false, false) => { self.open_palette(); true }                     // Ctrl+K
            (0x50, true, true, false) => { self.open_palette(); true }                      // Ctrl+Shift+P
            (0x44, true, false, false) => { self.toggle_bookmark(); true }                  // Ctrl+D
            (0x48, true, false, false) => { self.open_internal("aura://history"); true }    // Ctrl+H
            (0x4A, true, false, false) => { self.open_internal("aura://downloads"); true }  // Ctrl+J
            (0x46, true, false, false) => { self.open_find(); true }                        // Ctrl+F
            (0x50, true, false, false) => { self.print_page(); true }                       // Ctrl+P
            (0x54, true, true, false) => { self.reopen_closed(); true }                     // Ctrl+Shift+T
            (0x09, true, false, false) => {                                               // Ctrl+Tab
                let n = self.tabs.len();
                if n > 1 { self.activate_tab((self.active + 1) % n); }
                true
            }
            (0x09, true, true, false) => {
                let n = self.tabs.len();
                if n > 1 { self.activate_tab((self.active + n - 1) % n); }
                true
            }
            (0x74, false, false, false) => { self.reload(); true }                          // F5
            (0x7B, false, false, false) => {                                              // F12
                if let Some(t) = self.tabs.get(self.active) {
                    if let Some(wv) = &t.webview {
                        unsafe { let _ = wv.OpenDevToolsWindow(); }
                    }
                }
                true
            }
            (0x7A, false, false, false) => { self.toggle_fullscreen(); true }               // F11
            (0x25, false, false, true) => { self.go_back(); true }                          // Alt+Left
            (0x27, false, false, true) => { self.go_fwd(); true }                           // Alt+Right
            (0x08, false, false, false) if !self.editing && !self.find_open => {            // Backspace
                self.go_back();
                true
            }
            (0xBB, true, false, false) | (0x6B, true, false, false) => { self.zoom(0.1); true }
            (0xBD, true, false, false) | (0x6D, true, false, false) => { self.zoom(-0.1); true }
            (0x30, true, false, false) | (0x60, true, false, false) => { self.zoom_to(1.0); true }
            (0x31..=0x38, true, false, false) => {                                        // Ctrl+1..8
                let i = (vk - 0x31) as usize;
                if i < self.tabs.len() { self.activate_tab(i); }
                true
            }
            (0x39, true, false, false) => {                                               // Ctrl+9 = letzter Tab
                if !self.tabs.is_empty() {
                    let last = self.tabs.len() - 1;
                    self.activate_tab(last);
                }
                true
            }
            // --- navigation ---
            (0x52, true, false, false) => { self.reload(); true }                           // Ctrl+R
            (0x52, true, true, false) | (0x74, true, false, false) => {                     // Ctrl+Shift+R / Ctrl+F5
                self.hard_reload();
                true
            }
            (0x1B, false, false, false) if !self.editing && !self.find_open => {            // Esc = Stopp
                self.stop_loading();
                true
            }
            (0x22, true, false, false) => {                                                 // Ctrl+PageDown
                let n = self.tabs.len();
                if n > 1 { self.activate_tab((self.active + 1) % n); }
                true
            }
            (0x21, true, false, false) => {                                                 // Ctrl+PageUp
                let n = self.tabs.len();
                if n > 1 { self.activate_tab((self.active + n - 1) % n); }
                true
            }
            (0x24, false, false, true) => { self.go_home(); true }                          // Alt+Home
            // --- tabs & fenster ---
            (0x57, true, true, false) => { self.close_window(); true }                      // Ctrl+Shift+W
            (0x44, true, true, false) => { self.duplicate_active(); true }                  // Ctrl+Shift+D
            (0x4D, true, true, false) => { self.toggle_mute_active(); true }                // Ctrl+Shift+M
            (0x53, true, true, false) => { self.toggle_split(); true }                      // Ctrl+Shift+S
            (0x41, true, true, false) => { self.open_tab_search(); true }                   // Ctrl+Shift+A (wie Chrome)
            (0x50, true, true, true) => { self.toggle_pin_active(); true }                  // Strg+Umschalt+Alt+P
            // --- ansicht ---
            (0x4F, true, true, false) => { self.open_internal("aura://bookmarks"); true }   // Ctrl+Shift+O
            (0x2C, true, false, false) | (0xBC, true, false, false) => {                    // Ctrl+,
                self.open_internal("aura://settings");
                true
            }
            (0x55, true, false, false) => { self.view_source(); true }                      // Ctrl+U
            (0x47, true, false, false) | (0x72, false, false, false) => {                   // Ctrl+G / F3
                self.find_next(shift);
                true
            }
            (0x53, true, false, false) => { self.save_page(); true }                        // Ctrl+S
            (0x45, true, true, false) => { self.open_internal("aura://reading"); true }     // Ctrl+Shift+E
            (0x4C, true, true, false) => { self.add_to_reading_list(); true }               // Ctrl+Shift+L
            (0x1B, false, true, false) => { self.open_internal("aura://tasks"); true }      // Shift+Esc
            _ => false,
        }
    }

    fn hard_reload(&mut self) {
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                unsafe {
                    let script = wide("location.reload(true)");
                    let _ = wv.ExecuteScript(PCWSTR(script.as_ptr()), None);
                }
            }
        }
    }

    fn stop_loading(&mut self) {
        if let Some(t) = self.tabs.get_mut(self.active) {
            if let Some(wv) = &t.webview {
                unsafe {
                    let _ = wv.Stop();
                }
            }
            t.loading = false;
        }
        self.paint();
    }

    fn go_home(&mut self) {
        let idx = self.active;
        let home = self.storage.get_setting("homepage", "aura://start");
        self.navigate(idx, &home);
    }

    /// URL for a freshly opened tab, per the "Neuer Tab" setting.
    pub fn new_tab_url(&self) -> String {
        match self.storage.get_setting("new_tab_page", "start").as_str() {
            "blank" => "about:blank".to_string(),
            "home" => self.storage.get_setting("homepage", "aura://start"),
            _ => "aura://start".to_string(),
        }
    }

    fn close_window(&mut self) {
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn duplicate_active(&mut self) {
        let url = self.tabs.get(self.active).map(|t| t.url.clone()).unwrap_or_default();
        if !url.is_empty() {
            self.new_tab(&url, true);
        }
    }

    fn toggle_mute_active(&mut self) {
        let i = self.active;
        self.exec_action(&format!("mute:{i}"));
    }

    fn toggle_pin_active(&mut self) {
        let i = self.active;
        self.exec_action(&format!("pin:{i}"));
    }

    /// Toggles the current page in the reading list.
    fn add_to_reading_list(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        if t.url.is_empty() || t.is_internal {
            return;
        }
        let (url, title, fav) = (t.url.clone(), t.title.clone(), t.favicon_png.clone());
        if self.storage.reading_has(&url) {
            self.storage.reading_remove(&url);
        } else {
            self.storage.reading_add(&url, &title, fav.as_deref());
        }
        self.paint();
    }

    /// No translate API in WebView2 — hand the page to Google Translate.
    fn translate_page(&mut self) {
        let Some(t) = self.tabs.get(self.active) else { return };
        if !t.url.starts_with("http") {
            return;
        }
        let target = self.storage.get_setting("translate_to", "de");
        let url = format!(
            "https://translate.google.com/translate?sl=auto&tl={target}&u={}",
            urlencode(&t.url)
        );
        self.new_tab(&url, true);
    }

    /// Opens the omnibox in tab-search mode ("@" lists every open tab).
    fn open_tab_search(&mut self) {
        self.begin_edit();
        omnibox::set_edit_text(self.edit, "@");
        send_msg(self.edit, EM_SETSEL, WPARAM(1), LPARAM(1));
        self.on_omnibox_changed();
    }

    fn view_source(&mut self) {
        let url = self.tabs.get(self.active).map(|t| t.url.clone()).unwrap_or_default();
        if url.starts_with("http") {
            self.new_tab(&format!("view-source:{url}"), true);
        }
    }

    fn save_page(&mut self) {
        if let Some(t) = self.tabs.get(self.active) {
            if let Some(wv) = &t.webview {
                // Chromium's own "save page" dialog.
                unsafe {
                    let js = wide("document.execCommand('SaveAs')");
                    let _ = wv.ExecuteScript(PCWSTR(js.as_ptr()), None);
                }
            }
        }
    }

    // ---------------- session / theme / shutdown ----------------
    fn restore_session(&mut self) -> bool {
        let Some(session) = self.storage.load_session() else { return false };
        if session.tabs.is_empty() {
            return false;
        }
        for st in &session.tabs {
            let idx = self.new_tab_lazy(&st.url);
            self.tabs[idx].pinned = st.pinned;
            self.tabs[idx].group = st.group;
            self.tabs[idx].title = st.title.clone();
            // Favicon from history so restored tabs are recognisable before load.
            self.tabs[idx].favicon_png = self.storage.favicon_for(&st.url);
        }
        self.active = session.active.min(self.tabs.len() - 1);
        // Only the visible tab actually boots a renderer.
        let active = self.active;
        self.ensure_controller(active);
        self.layout_webviews();
        true
    }

    pub fn save_session(&self) {
        self.persist_shield_total();
        if self.private {
            return;
        }
        let session = Session {
            tabs: self
                .tabs
                .iter()
                .filter(|t| !t.closing)
                .map(|t| SessionTab {
                    url: t.url.clone(),
                    title: t.title.clone(),
                    pinned: t.pinned,
                    group: t.group,
                })
                .collect(),
            active: self.active,
        };
        self.storage.save_session(&session);
    }

    pub fn apply_theme(&mut self, mode: ThemeMode) {
        self.theme_mode = mode;
        let accent = self.theme.accent;
        let rm = self.theme.reduce_motion;
        self.theme = Theme::new(mode, accent, rm);
        if self.glass {
            self.theme.glassify();
        }
        unsafe {
            let dark = BOOL(self.theme.dark as i32);
            let _ = DwmSetWindowAttribute(
                self.hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as *const _,
                4,
            );
            let _ = DeleteObject(HGDIOBJ(self.edit_brush.0));
            let (fg, bg) = if self.theme.dark {
                (COLORREF(0x00F8F0F0), COLORREF(0x00241E1E))
            } else {
                (COLORREF(0x00261C1C), COLORREF(0x00FCFAFA))
            };
            self.edit_fg = fg;
            self.edit_bg = bg;
            self.edit_brush = CreateSolidBrush(bg);
        }
        self.paint();
    }

    fn shutdown(&mut self) {
        self.save_session();
        for tab in &self.tabs {
            if let Some(ctl) = &tab.controller {
                unsafe {
                    let _ = ctl.Close();
                }
            }
        }
        if self.private {
            let dir = std::env::temp_dir().join(format!("aura_priv_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

// ---------------- helpers ----------------
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        if app.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        (*app).wndproc(hwnd, msg, w, l)
    }
}

fn register_classes(hinst: HINSTANCE) -> Result<()> {
    let class = wide("AuraMainWindow");
    let icon = unsafe { LoadIconW(Some(hinst), PCWSTR(1 as *const u16)) }.unwrap_or_default();
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class.as_ptr()),
        hIconSm: icon,
    };
    unsafe {
        if RegisterClassExW(&wc) == 0 { return Err(windows::core::Error::from_thread()); }
    }
    popup::register_class(hinst)?;
    Ok(())
}

fn create_edit(parent: HWND, scale: f32, font: HFONT, _id: i32) -> Result<HWND> {
    let class = wide("EDIT");
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(WS_CHILD.0 | ES_AUTOHSCROLL as u32),
            0, 0, 10, 10,
            Some(parent), None, None, None,
        )?
    };
    unsafe {
        SendMessageW(hwnd, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let _ = SetWindowSubclass(hwnd, Some(edit_proc), 0, 0);
    }
    let _ = scale;
    Ok(hwnd)
}

unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        if msg == WM_KEYDOWN {
            match w.0 as u32 {
                0x0D => { post(AppMsg::OmniSubmit { edit: hwnd }); return LRESULT(0); } // Enter
                0x1B => { post(AppMsg::OmniCancel { edit: hwnd }); return LRESULT(0); } // Esc
                0x26 => { post(AppMsg::OmniNav { edit: hwnd, delta: -1 }); return LRESULT(0); } // Up
                0x28 => { post(AppMsg::OmniNav { edit: hwnd, delta: 1 }); return LRESULT(0); } // Down
                _ => {}
            }
        }
        if msg == WM_CHAR && (w.0 as u32 == 0x0D || w.0 as u32 == 0x1B) {
            return LRESULT(0);
        }
        // Losing focus closes the field — it must never linger over the page.
        if msg == WM_KILLFOCUS {
            post(AppMsg::OmniCancel { edit: hwnd });
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

fn icon_font(gfx: &Gfx, size: f32) -> Result<IDWriteTextFormat> {
    unsafe {
        gfx.dwrite.CreateTextFormat(
            w!("Segoe MDL2 Assets"),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("de-de"),
        )
    }
}

pub fn parse_accent(s: &str) -> (u8, u8, u8) {
    let mut it = s.split(',');
    let r = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(110);
    let g = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(91);
    let b = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(208);
    (r, g, b)
}

pub fn parse_hex_color(s: &str) -> Option<D2D1_COLOR_F> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(color(r, g, b, 1.0))
}
