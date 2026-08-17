// Die Oberflaeche: ein Fenster ohne Rahmen, komplett selbst gezeichnet mit
// Direct2D und DirectWrite – dieselben Bausteine wie im Browser, dieselben
// Farben (Vercels Geist-Skala aus src/theme.rs), dieselben Federn fuer die
// Bewegung. Kein Assistent mit sieben Seiten: eine Karte, die nacheinander
// drei Zustaende zeigt – Start, Arbeit, Ergebnis.
//
// Aufbau des Fensters (in DIP, 480 x 600):
//
//   Kopf: Symbol, Name, Version – bleibt ueber alle Zustaende stehen.
//   Inhalt: wechselt mit einer kurzen Ueberblendung.
//   Der wichtigste Knopf sitzt immer an derselben Stelle.
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

use crate::anim::{approach, Spring};
use crate::i18n::{fill, strings, Lang, Strings};
use crate::install::{self, Installed, Options, Outcome, Progress, Step};
use crate::payload::Payload;
use crate::sys::{self, wide};

// ---------------------------------------------------------------- Masse

const WIN_W: f32 = 480.0;
const WIN_H: f32 = 600.0;
const PAD: f32 = 40.0;
const CONTENT_W: f32 = WIN_W - 2.0 * PAD;
const CAP_BTN_W: f32 = 46.0;
const CAP_BTN_H: f32 = 32.0;
const ICON: f32 = 96.0;
const ROW_H: f32 = 44.0;
const BTN_H: f32 = 44.0;
const BTN_Y: f32 = 500.0;
const R_SM: f32 = 6.0;
const R_MD: f32 = 8.0;

const WM_APP_PROGRESS: u32 = WM_APP + 1;
const WM_APP_FINISHED: u32 = WM_APP + 2;
const TIMER_ANIM: usize = 1;
const TIMER_SLOW: usize = 2;

// ---------------------------------------------------------------- Farben

type Color = D2D1_COLOR_F;

fn rgb(v: u32) -> Color {
    Color {
        r: ((v >> 16) & 255) as f32 / 255.0,
        g: ((v >> 8) & 255) as f32 / 255.0,
        b: (v & 255) as f32 / 255.0,
        a: 1.0,
    }
}

fn alpha(mut c: Color, a: f32) -> Color {
    c.a *= a;
    c
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Geist. Die Stufen tragen feste Aufgaben – siehe src/theme.rs.
#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    /// background-200: die Flaeche des Fensters.
    bg: Color,
    /// background-100: die Karte darauf.
    surface: Color,
    /// gray-100..300: Bedienelement ruhend, beruehrt, gewaehlt.
    ui: [Color; 3],
    /// gray-400..600: Rand ruhend, beruehrt, gewaehlt.
    line: [Color; 3],
    /// gray-900 / gray-1000: Text leise, Text deutlich.
    dim: Color,
    text: Color,
    /// blue-700 – die eine kraeftige Farbe.
    accent: Color,
    danger: Color,
    success: Color,
}

fn theme(dark: bool) -> Theme {
    if dark {
        Theme {
            dark,
            bg: rgb(0x000000),
            surface: rgb(0x0a0a0a),
            ui: [rgb(0x1a1a1a), rgb(0x1f1f1f), rgb(0x292929)],
            line: [rgb(0x2e2e2e), rgb(0x454545), rgb(0x878787)],
            dim: rgb(0xa1a1a1),
            text: rgb(0xededed),
            accent: rgb(0x0072f5),
            danger: rgb(0xff6166),
            success: rgb(0x62c073),
        }
    } else {
        Theme {
            dark,
            bg: rgb(0xffffff),
            surface: rgb(0xfafafa),
            ui: [rgb(0xf2f2f2), rgb(0xebebeb), rgb(0xe6e6e6)],
            line: [rgb(0xebebeb), rgb(0xc9c9c9), rgb(0xa8a8a8)],
            dim: rgb(0x4c4c4c),
            text: rgb(0x171717),
            accent: rgb(0x0072f5),
            danger: rgb(0xcb2a2f),
            success: rgb(0x297a3a),
        }
    }
}

/// Windows-Einstellung "App-Modus": hell oder dunkel.
fn system_dark() -> bool {
    sys::reg_u32(
        windows::Win32::System::Registry::HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    ) == Some(0)
}

// ---------------------------------------------------------------- Geometrie

fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: radius,
        radiusY: radius,
    }
}

fn inflate(r: D2D_RECT_F, d: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left - d,
        top: r.top - d,
        right: r.right + d,
        bottom: r.bottom + d,
    }
}

fn shift(r: D2D_RECT_F, dy: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left,
        top: r.top + dy,
        right: r.right,
        bottom: r.bottom + dy,
    }
}

fn contains(r: D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

// ---------------------------------------------------------------- Zustand

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hot {
    None,
    Close,
    Min,
    CheckDesktop,
    CheckRegister,
    CheckPurge,
    Change,
    Primary,
    Secondary,
    LinkLicense,
    LinkSource,
    LinkDefault,
    LinkLog,
    LinkWebView2,
}

const HOT_COUNT: usize = 14;

impl Hot {
    fn idx(self) -> usize {
        self as usize
    }
    fn is_link(self) -> bool {
        matches!(
            self,
            Hot::Secondary
                | Hot::LinkLicense
                | Hot::LinkSource
                | Hot::LinkDefault
                | Hot::LinkLog
                | Hot::LinkWebView2
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Start,
    Working,
    Done,
    Failed,
}

pub enum Mode {
    Install {
        payload: Arc<Payload>,
        image: Arc<Vec<u8>>,
        opts: Options,
        installed: Option<Installed>,
    },
    Uninstall {
        dir: PathBuf,
        purge: bool,
    },
}

/// Ergebnis der Arbeit, vom Hintergrund-Thread abgelegt.
type WorkResult = Result<Option<Outcome>, String>;
static RESULT: Mutex<Option<WorkResult>> = Mutex::new(None);

struct Fonts {
    title: IDWriteTextFormat,
    h2: IDWriteTextFormat,
    body: IDWriteTextFormat,
    body_wrap: IDWriteTextFormat,
    body_center: IDWriteTextFormat,
    small: IDWriteTextFormat,
    small_center: IDWriteTextFormat,
    button: IDWriteTextFormat,
    link: IDWriteTextFormat,
}

struct Ui {
    hwnd: HWND,
    hinst: HINSTANCE,
    scale: f32,
    theme: Theme,
    /// Erzwungenes Farbschema (--theme=light|dark), sonst das von Windows.
    forced_dark: Option<bool>,
    s: &'static Strings,
    /// Versionszeile unter dem Namen – beim Start bestimmt.
    version: String,

    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    wic: IWICImagingFactory,
    rt: Option<ID2D1HwndRenderTarget>,
    icon: Option<ID2D1Bitmap>,
    fonts: Fonts,

    mode: Mode,
    screen: Screen,
    prev_screen: Option<Screen>,
    trans: Spring,

    hits: Vec<(Hot, D2D_RECT_F)>,
    focus_order: Vec<Hot>,
    hot: Hot,
    pressed: Hot,
    focus: Option<Hot>,
    focus_visible: bool,
    hover_t: [f32; HOT_COUNT],
    check_t: [f32; 3],
    mouse_in: bool,

    progress: Spring,
    indeterminate: bool,
    indet_phase: f32,
    status: &'static str,
    work_started: Option<Instant>,
    finish_pending: Option<WorkResult>,
    outcome: Option<Outcome>,
    error: Option<String>,
    running: bool,

    anim_last: Instant,
    timer_on: bool,
    exit_code: i32,
}

// ---------------------------------------------------------------- Einstieg

pub fn run(mode: Mode, lang: Lang, forced_dark: Option<bool>) -> i32 {
    let ui = match Ui::new(mode, lang, forced_dark) {
        Ok(ui) => ui,
        Err(e) => {
            sys::log(&format!("Oberflaeche: {e}"));
            sys::message_box("Aura Browser Setup", &e, true);
            return 1;
        }
    };
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    ui.exit_code
}

impl Ui {
    fn new(mode: Mode, lang: Lang, forced_dark: Option<bool>) -> Result<Box<Ui>, String> {
        let s = strings(lang);
        let err = |what: &str, e: windows::core::Error| format!("{what}: {e}");
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                Some(&D2D1_FACTORY_OPTIONS {
                    debugLevel: D2D1_DEBUG_LEVEL_NONE,
                }),
            )
            .map_err(|e| err("Direct2D", e))?;
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(|e| err("DirectWrite", e))?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| err("WIC", e))?;
            let fonts = Fonts::new(&dwrite, lang).map_err(|e| err("Schriften", e))?;
            let hinst = HINSTANCE(GetModuleHandleW(None).map_err(|e| err("Modul", e))?.0);

            let dark = forced_dark.unwrap_or_else(system_dark);
            let running = match &mode {
                Mode::Install { opts, .. } => install::is_running(&opts.dir),
                Mode::Uninstall { dir, .. } => install::is_running(dir),
            };
            let version = match &mode {
                Mode::Install { payload, .. } => payload.version.clone(),
                Mode::Uninstall { dir, .. } => install::installed()
                    .filter(|i| install::same_dir(&i.dir, dir))
                    .map(|i| i.version)
                    .unwrap_or_default(),
            };
            let mut ui = Box::new(Ui {
                hwnd: HWND::default(),
                hinst,
                scale: 1.0,
                theme: theme(dark),
                forced_dark,
                s,
                version,
                factory,
                dwrite,
                wic,
                rt: None,
                icon: None,
                fonts,
                mode,
                screen: Screen::Start,
                prev_screen: None,
                trans: Spring::new(1.0, 0.5, 1.0),
                hits: Vec::new(),
                focus_order: Vec::new(),
                hot: Hot::None,
                pressed: Hot::None,
                focus: None,
                focus_visible: false,
                hover_t: [0.0; HOT_COUNT],
                check_t: [0.0; 3],
                mouse_in: false,
                progress: Spring::new(0.0, 0.8, 1.0),
                indeterminate: false,
                indet_phase: 0.0,
                status: "",
                work_started: None,
                finish_pending: None,
                outcome: None,
                error: None,
                running,
                anim_last: Instant::now(),
                timer_on: false,
                exit_code: 2,
            });
            ui.check_t = ui.check_targets();

            // Fensterklasse mit dem Programmsymbol.
            let icon = LoadIconW(Some(hinst), PCWSTR(1 as *const u16)).unwrap_or_default();
            let class = wide("AuraSetupWindow");
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hIcon: icon,
                hIconSm: icon,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH::default(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            if RegisterClassExW(&wc) == 0 {
                return Err("Fensterklasse liess sich nicht anmelden".into());
            }

            let title = wide(match &ui.mode {
                Mode::Install { .. } => "Aura Browser Setup",
                Mode::Uninstall { .. } => s.un_title,
            });
            let ptr: *mut Ui = ui.as_mut();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WIN_W as i32,
                WIN_H as i32,
                None,
                None,
                Some(hinst),
                Some(ptr as *const core::ffi::c_void),
            )
            .map_err(|e| err("Fenster", e))?;
            ui.hwnd = hwnd;

            // Runde Ecken, dunkler Rahmen, Schattenlinie – wie beim Browser.
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &corner as *const _ as *const _,
                4,
            );
            let dark_flag = BOOL(dark as i32);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark_flag as *const _ as *const _,
                4,
            );
            let margins = MARGINS {
                cxLeftWidth: 0,
                cxRightWidth: 0,
                cyTopHeight: 1,
                cyBottomHeight: 0,
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

            // Groesse nach DPI, mittig auf dem Bildschirm mit dem Mauszeiger.
            ui.scale = GetDpiForWindow(hwnd) as f32 / 96.0;
            let (w, h) = (
                (WIN_W * ui.scale).round() as i32,
                (WIN_H * ui.scale).round() as i32,
            );
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let (mut x, mut y) = (CW_USEDEFAULT, CW_USEDEFAULT);
            if GetMonitorInfoW(mon, &mut info).as_bool() {
                let wa = info.rcWork;
                x = wa.left + (wa.right - wa.left - w) / 2;
                y = wa.top + (wa.bottom - wa.top - h) / 2;
            }
            let _ = SetWindowPos(
                hwnd,
                None,
                x,
                y,
                w,
                h,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            if matches!(ui.mode, Mode::Install { .. }) {
                SetTimer(Some(hwnd), TIMER_SLOW, 2000, None);
            }
            Ok(ui)
        }
    }

    fn check_targets(&self) -> [f32; 3] {
        match &self.mode {
            Mode::Install { opts, .. } => [
                opts.desktop_icon as u8 as f32,
                opts.register_browser as u8 as f32,
                0.0,
            ],
            Mode::Uninstall { purge, .. } => [0.0, 0.0, *purge as u8 as f32],
        }
    }

    // ------------------------------------------------------------ Fensternachrichten

    unsafe fn handle(&mut self, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                self.paint();
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            WM_SIZE => {
                if let Some(rt) = &self.rt {
                    let mut rc = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rc);
                    let _ = rt.Resize(&D2D_SIZE_U {
                        width: rc.right as u32,
                        height: rc.bottom as u32,
                    });
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let dpi = (w.0 & 0xFFFF) as f32;
                self.scale = dpi / 96.0;
                let r = &*(l.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                if let Some(rt) = &self.rt {
                    rt.SetDpi(dpi, dpi);
                }
                self.invalidate();
                LRESULT(0)
            }
            WM_SETTINGCHANGE => {
                let dark = self.forced_dark.unwrap_or_else(system_dark);
                if dark != self.theme.dark {
                    self.theme = theme(dark);
                    let flag = BOOL(dark as i32);
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_USE_IMMERSIVE_DARK_MODE,
                        &flag as *const _ as *const _,
                        4,
                    );
                    self.invalidate();
                }
                LRESULT(0)
            }
            WM_NCHITTEST => {
                let mut pt = POINT {
                    x: (l.0 & 0xFFFF) as i16 as i32,
                    y: ((l.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                let _ = ScreenToClient(hwnd, &mut pt);
                let (x, y) = (pt.x as f32 / self.scale, pt.y as f32 / self.scale);
                if self.hit(x, y) != Hot::None {
                    LRESULT(HTCLIENT as isize)
                } else {
                    // Alles, was kein Bedienelement ist, zieht das Fenster.
                    LRESULT(HTCAPTION as isize)
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = (
                    (l.0 & 0xFFFF) as i16 as f32 / self.scale,
                    ((l.0 >> 16) & 0xFFFF) as i16 as f32 / self.scale,
                );
                if !self.mouse_in {
                    self.mouse_in = true;
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                }
                let hot = self.hit(x, y);
                self.set_hot(hot);
                LRESULT(0)
            }
            WM_MOUSELEAVE | WM_NCMOUSEMOVE => {
                if msg == WM_MOUSELEAVE {
                    self.mouse_in = false;
                }
                self.set_hot(Hot::None);
                if msg == WM_NCMOUSEMOVE {
                    return DefWindowProcW(hwnd, msg, w, l);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                self.pressed = self.hot;
                self.focus_visible = false;
                if self.hot != Hot::None {
                    self.focus = Some(self.hot);
                    SetCapture(hwnd);
                }
                self.invalidate();
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let (x, y) = (
                    (l.0 & 0xFFFF) as i16 as f32 / self.scale,
                    ((l.0 >> 16) & 0xFFFF) as i16 as f32 / self.scale,
                );
                let hot = self.hit(x, y);
                let pressed = self.pressed;
                self.pressed = Hot::None;
                if pressed != Hot::None && pressed == hot {
                    self.activate(hot);
                }
                self.invalidate();
                LRESULT(0)
            }
            WM_SETCURSOR => {
                if (l.0 & 0xFFFF) as u32 == HTCLIENT {
                    let id = if self.hot.is_link() { IDC_HAND } else { IDC_ARROW };
                    if let Ok(c) = LoadCursorW(None, id) {
                        SetCursor(Some(c));
                    }
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, w, l)
            }
            WM_KEYDOWN => {
                self.key(w.0 as u16);
                LRESULT(0)
            }
            WM_TIMER => {
                if w.0 == TIMER_ANIM {
                    self.tick();
                } else if w.0 == TIMER_SLOW {
                    if self.screen == Screen::Start {
                        let dir = match &self.mode {
                            Mode::Install { opts, .. } => opts.dir.clone(),
                            Mode::Uninstall { dir, .. } => dir.clone(),
                        };
                        let running = install::is_running(&dir);
                        if running != self.running {
                            self.running = running;
                            self.invalidate();
                        }
                    }
                }
                LRESULT(0)
            }
            WM_APP_PROGRESS => {
                let step = Step::from_u32((w.0 & 0xFF) as u32);
                let indeterminate = w.0 & 0x100 != 0;
                let fraction = l.0 as f32 / 10_000.0;
                self.on_progress(Progress {
                    step,
                    fraction,
                    indeterminate,
                });
                LRESULT(0)
            }
            WM_APP_FINISHED => {
                let result = RESULT.lock().ok().and_then(|mut r| r.take());
                if let Some(result) = result {
                    self.progress.set_target(1.0);
                    self.indeterminate = false;
                    self.finish_pending = Some(result);
                    self.animate();
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                if self.screen == Screen::Working {
                    // Waehrend der Arbeit nicht abbrechen – ein halber Stand
                    // waere schlimmer als zehn Sekunden Warten.
                    return LRESULT(0);
                }
                sys::log(&format!("Fenster zu ({:?}, Code {})", self.screen, self.exit_code));
                let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                let _ = KillTimer(Some(hwnd), TIMER_SLOW);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }

    fn key(&mut self, vk: u16) {
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        match VIRTUAL_KEY(vk) {
            VK_ESCAPE => {
                if self.screen != Screen::Working {
                    self.close();
                }
            }
            VK_TAB => {
                if self.focus_order.is_empty() {
                    return;
                }
                let n = self.focus_order.len();
                let cur = self
                    .focus
                    .and_then(|f| self.focus_order.iter().position(|&h| h == f));
                let next = match (cur, shift) {
                    (None, false) => 0,
                    (None, true) => n - 1,
                    (Some(i), false) => (i + 1) % n,
                    (Some(i), true) => (i + n - 1) % n,
                };
                self.focus = Some(self.focus_order[next]);
                self.focus_visible = true;
                self.invalidate();
            }
            VK_SPACE => {
                if let Some(f) = self.focus {
                    self.activate(f);
                }
            }
            VK_RETURN => {
                let target = match self.focus {
                    Some(f) if f != Hot::Primary => f,
                    _ => Hot::Primary,
                };
                self.activate(target);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ Aktionen

    fn activate(&mut self, hot: Hot) {
        match hot {
            Hot::None => {}
            Hot::Close => {
                if self.screen != Screen::Working {
                    self.close();
                }
            }
            Hot::Min => unsafe {
                let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
            },
            Hot::CheckDesktop | Hot::CheckRegister => {
                if let Mode::Install { opts, .. } = &mut self.mode {
                    if hot == Hot::CheckDesktop {
                        opts.desktop_icon = !opts.desktop_icon;
                    } else {
                        opts.register_browser = !opts.register_browser;
                    }
                }
                self.animate();
            }
            Hot::CheckPurge => {
                if let Mode::Uninstall { purge, .. } = &mut self.mode {
                    *purge = !*purge;
                }
                self.animate();
            }
            Hot::Change => self.pick_folder(),
            Hot::Primary => match self.screen {
                Screen::Start | Screen::Failed => self.start_work(),
                Screen::Working => {}
                Screen::Done => {
                    if let Some(o) = &self.outcome {
                        if let Err(e) = sys::spawn(&o.exe, &[]) {
                            sys::log(&format!("Start: {e}"));
                        }
                    }
                    self.close();
                }
            },
            Hot::Secondary => self.close(),
            Hot::LinkLicense => sys::shell_open(&format!("{}/blob/main/LICENSE", install::URL)),
            Hot::LinkSource => sys::shell_open(install::URL),
            Hot::LinkDefault => sys::shell_open("ms-settings:defaultapps"),
            Hot::LinkLog => sys::shell_open(&sys::log_path().to_string_lossy()),
            Hot::LinkWebView2 => sys::shell_open(install::WEBVIEW2_URL),
        }
    }

    fn close(&mut self) {
        sys::log(&format!("Fenster wird geschlossen (Code {})", self.exit_code));
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    fn pick_folder(&mut self) {
        let Mode::Install { opts, .. } = &mut self.mode else { return };
        let title = wide(self.s.folder_dialog_title);
        let picked: Option<String> = unsafe {
            (|| {
                let dlg: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
                let flags = dlg.GetOptions().unwrap_or_default();
                dlg.SetOptions(flags | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
                    .ok()?;
                dlg.SetTitle(PCWSTR(title.as_ptr())).ok()?;
                // Im Elternordner des bisherigen Ziels anfangen.
                if let Some(parent) = opts.dir.parent().filter(|p| p.is_dir()) {
                    let p = sys::wide_path(parent);
                    if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(p.as_ptr()), None) {
                        let _ = dlg.SetFolder(&item);
                    }
                }
                dlg.Show(Some(self.hwnd)).ok()?;
                let item = dlg.GetResult().ok()?;
                let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
                let s = name.to_string().ok();
                CoTaskMemFree(Some(name.0 as *const _));
                s
            })()
        };
        if let Some(p) = picked {
            opts.dir = install::normalize_dir(std::path::Path::new(&p));
            sys::log(&format!("Zielordner gewaehlt: {}", opts.dir.display()));
        }
        self.set_hot(Hot::None);
        self.invalidate();
    }

    fn start_work(&mut self) {
        RESULT.lock().map(|mut r| *r = None).ok();
        self.error = None;
        self.outcome = None;
        self.progress.jump_to(0.0);
        self.indeterminate = false;
        self.work_started = Some(Instant::now());
        self.status = match &self.mode {
            Mode::Install { .. } => self.s.status_closing,
            Mode::Uninstall { .. } => self.s.status_closing,
        };
        self.set_screen(Screen::Working);
        let hwnd = self.hwnd.0 as isize;
        let post = move |msg: u32, w: usize, l: isize| unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd as *mut core::ffi::c_void)),
                msg,
                WPARAM(w),
                LPARAM(l),
            );
        };
        let mut report = move |p: Progress| {
            let w = p.step as usize | if p.indeterminate { 0x100 } else { 0 };
            post(WM_APP_PROGRESS, w, (p.fraction * 10_000.0) as isize);
        };
        match &self.mode {
            Mode::Install {
                payload,
                image,
                opts,
                ..
            } => {
                let (payload, image, opts) = (Arc::clone(payload), Arc::clone(image), opts.clone());
                std::thread::spawn(move || {
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                    }
                    let res = install::install(&payload, &image, &opts, &mut report).map(Some);
                    if let Ok(mut r) = RESULT.lock() {
                        *r = Some(res);
                    }
                    post(WM_APP_FINISHED, 0, 0);
                });
            }
            Mode::Uninstall { dir, purge } => {
                let (dir, purge) = (dir.clone(), *purge);
                std::thread::spawn(move || {
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                    }
                    let res = install::uninstall(&dir, purge, &mut report).map(|_| None);
                    if let Ok(mut r) = RESULT.lock() {
                        *r = Some(res);
                    }
                    post(WM_APP_FINISHED, 0, 0);
                });
            }
        }
    }

    fn on_progress(&mut self, p: Progress) {
        let s = self.s;
        self.status = match p.step {
            Step::Closing => s.status_closing,
            Step::Files => s.status_files,
            Step::Uninstaller => s.status_uninstaller,
            Step::Shortcuts => s.status_shortcuts,
            Step::Registry => s.status_registry,
            Step::WebView2 => s.status_webview2,
            Step::Removing => s.status_removing,
            Step::Data => s.status_data,
        };
        self.progress.set_target(p.fraction.clamp(0.0, 1.0));
        self.indeterminate = p.indeterminate;
        self.animate();
    }

    fn finish(&mut self, result: WorkResult) {
        match result {
            Ok(outcome) => {
                self.outcome = outcome;
                self.exit_code = 0;
                self.set_screen(Screen::Done);
            }
            Err(e) => {
                sys::log(&format!("Fehler: {e}"));
                self.error = Some(e);
                self.exit_code = 1;
                self.set_screen(Screen::Failed);
            }
        }
    }

    fn set_screen(&mut self, screen: Screen) {
        if screen == self.screen {
            return;
        }
        self.prev_screen = Some(self.screen);
        self.screen = screen;
        self.trans.jump_to(0.0);
        self.trans.set_target(1.0);
        self.hot = Hot::None;
        self.pressed = Hot::None;
        self.focus = None;
        self.hover_t = [0.0; HOT_COUNT];
        self.animate();
    }

    fn set_hot(&mut self, hot: Hot) {
        if hot != self.hot {
            self.hot = hot;
            self.animate();
        }
    }

    fn hit(&self, x: f32, y: f32) -> Hot {
        for (h, r) in &self.hits {
            if contains(*r, x, y) {
                return *h;
            }
        }
        Hot::None
    }

    // ------------------------------------------------------------ Bewegung

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn animate(&mut self) {
        if !self.timer_on {
            self.timer_on = true;
            self.anim_last = Instant::now();
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_ANIM, 16, None);
            }
        }
        self.invalidate();
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = (now - self.anim_last).as_secs_f32().min(0.1);
        self.anim_last = now;
        let mut busy = false;

        self.trans.step(dt);
        busy |= !self.trans.at_rest();
        if self.trans.at_rest() {
            self.prev_screen = None;
        }

        self.progress.step(dt);
        busy |= !self.progress.at_rest();

        for i in 0..HOT_COUNT {
            let target = if self.hot.idx() == i && self.hot != Hot::None { 1.0 } else { 0.0 };
            let tau = if target > 0.0 { 0.06 } else { 0.16 };
            self.hover_t[i] = approach(self.hover_t[i], target, dt, tau);
            busy |= self.hover_t[i] != target;
        }
        let targets = self.check_targets();
        for i in 0..3 {
            self.check_t[i] = approach(self.check_t[i], targets[i], dt, 0.09);
            busy |= self.check_t[i] != targets[i];
        }
        if self.indeterminate && self.screen == Screen::Working {
            self.indet_phase = (self.indet_phase + dt * 0.6) % 1.0;
            busy = true;
        }

        // Fertig, sobald der Balken angekommen ist und der Zustand wenigstens
        // kurz zu sehen war – sonst blitzt er nur auf.
        if self.finish_pending.is_some() {
            let shown = self
                .work_started
                .map(|t| now.duration_since(t).as_secs_f32())
                .unwrap_or(9.0);
            if self.progress.value > 0.985 && shown > 0.9 {
                if let Some(r) = self.finish_pending.take() {
                    self.finish(r);
                    busy = true;
                }
            } else {
                busy = true;
            }
        }

        if busy {
            self.invalidate();
        } else {
            self.timer_on = false;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
            }
            self.invalidate();
        }
    }

    // ------------------------------------------------------------ Zeichnen

    fn ensure_target(&mut self) {
        if self.rt.is_some() {
            return;
        }
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let dpi = 96.0 * self.scale;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_UNKNOWN,
                    alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
                },
                dpiX: dpi,
                dpiY: dpi,
                ..Default::default()
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U {
                    width: rc.right.max(1) as u32,
                    height: rc.bottom.max(1) as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            if let Ok(rt) = self.factory.CreateHwndRenderTarget(&props, &hprops) {
                self.icon = self.load_icon(&rt);
                self.rt = Some(rt);
            }
        }
    }

    /// Das echte Programmsymbol (256 px aus der Ressource) als D2D-Bitmap.
    fn load_icon(&self, rt: &ID2D1HwndRenderTarget) -> Option<ID2D1Bitmap> {
        unsafe {
            let h = LoadImageW(
                Some(self.hinst),
                PCWSTR(1 as *const u16),
                IMAGE_ICON,
                256,
                256,
                LR_DEFAULTCOLOR,
            )
            .ok()?;
            let hicon = HICON(h.0);
            let result = (|| {
                let src = self.wic.CreateBitmapFromHICON(hicon).ok()?;
                let conv = self.wic.CreateFormatConverter().ok()?;
                conv.Initialize(
                    &src,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .ok()?;
                rt.CreateBitmapFromWicBitmap(&conv, None).ok()
            })();
            let _ = DestroyIcon(hicon);
            result
        }
    }

    fn paint(&mut self) {
        self.ensure_target();
        let Some(rt) = self.rt.clone() else { return };
        self.hits.clear();
        self.focus_order.clear();
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&self.theme.bg));
        }

        self.draw_header();
        let t = self.trans.value;
        if let Some(prev) = self.prev_screen {
            if t < 0.999 {
                self.draw_content(prev, 1.0 - t, -10.0 * t, false);
            }
        }
        self.draw_content(self.screen, t, 10.0 * (1.0 - t), true);
        self.draw_caption();
        self.draw_focus();

        unsafe {
            if rt.EndDraw(None, None).is_err() {
                // Geraet weg (Treiberwechsel, Fernsitzung): beim naechsten Mal neu.
                self.rt = None;
                self.icon = None;
                self.invalidate();
            }
        }
    }

    fn draw_header(&mut self) {
        let th = self.theme;
        let s = self.s;
        let version = self.version.clone();
        self.draw_icon(rect((WIN_W - ICON) / 2.0, 64.0, ICON, ICON), 1.0);
        self.text(s.app_name, &self.fonts.title.clone(), rect(PAD, 176.0, CONTENT_W, 36.0), th.text);
        if !version.is_empty() {
            self.text(
                &fill(s.version_line, &[("v", &version)]),
                &self.fonts.small_center.clone(),
                rect(PAD, 214.0, CONTENT_W, 20.0),
                th.dim,
            );
        }
    }

    fn draw_content(&mut self, screen: Screen, a: f32, dy: f32, live: bool) {
        match (&self.mode, screen) {
            (Mode::Install { .. }, Screen::Start) => self.draw_install_start(a, dy, live),
            (Mode::Uninstall { .. }, Screen::Start) => self.draw_uninstall_start(a, dy, live),
            (_, Screen::Working) => self.draw_working(a, dy),
            (Mode::Install { .. }, Screen::Done) => self.draw_install_done(a, dy, live),
            (Mode::Uninstall { .. }, Screen::Done) => self.draw_uninstall_done(a, dy, live),
            (_, Screen::Failed) => self.draw_failed(a, dy, live),
        }
    }

    fn draw_install_start(&mut self, a: f32, dy: f32, live: bool) {
        let th = self.theme;
        let s = self.s;
        let (desktop, register, dir, installed, version) = match &self.mode {
            Mode::Install {
                opts,
                installed,
                payload,
                ..
            } => (
                opts.desktop_icon,
                opts.register_browser,
                opts.dir.clone(),
                installed.as_ref().map(|i| i.version.clone()),
                payload.version.clone(),
            ),
            _ => return,
        };

        self.text(s.lead, &self.fonts.body_center.clone(), shift(rect(PAD, 248.0, CONTENT_W, 48.0), dy), alpha(th.dim, a));

        // Die Karte mit den drei Zeilen.
        let card = shift(rect(PAD, 308.0, CONTENT_W, ROW_H * 3.0 + 2.0), dy);
        self.fill_rr(card, R_MD, alpha(th.surface, a));
        self.stroke_rr(card, R_MD, alpha(th.line[0], a), 1.0);
        let _ = (desktop, register); // der Ankreuz-Grad steckt in check_t
        let rows = [
            (Hot::CheckDesktop, s.opt_desktop, 0usize),
            (Hot::CheckRegister, s.opt_register, 1usize),
        ];
        for (i, (hot, label, ci)) in rows.iter().enumerate() {
            let row = rect(card.left + 1.0, card.top + 1.0 + ROW_H * i as f32, CONTENT_W - 2.0, ROW_H);
            self.draw_check_row(*hot, row, label, self.check_t[*ci], a, live);
            self.line(
                v2(row.left + 16.0, row.bottom + 0.5),
                v2(row.right - 16.0, row.bottom + 0.5),
                alpha(th.line[0], a),
            );
        }
        // Zeile 3: der Ordner.
        let row = rect(card.left + 1.0, card.top + 1.0 + ROW_H * 2.0, CONTENT_W - 2.0, ROW_H);
        self.text(s.folder_label, &self.fonts.small.clone(), rect(row.left + 16.0, row.top, 60.0, ROW_H), alpha(th.dim, a));
        let change_w = 20.0 + self.measure(s.change, &self.fonts.link.clone()).0.ceil();
        let change = rect(row.right - 12.0 - change_w, row.top + 8.0, change_w, ROW_H - 16.0);
        let path_r = rect(row.left + 72.0, row.top, change.left - 8.0 - (row.left + 72.0), ROW_H);
        self.text(&dir.to_string_lossy(), &self.fonts.body.clone(), path_r, alpha(th.text, a));
        let ht = self.hover_t[Hot::Change.idx()];
        if ht > 0.0 {
            self.fill_rr(change, R_SM, alpha(th.ui[1], a * ht));
        }
        self.text(s.change, &self.fonts.link.clone(), change, alpha(mix(th.dim, th.text, ht), a));
        if live {
            self.hits.push((Hot::Change, change));
            self.focus_order.push(Hot::Change);
        }

        // Hinweise unter der Karte.
        let mut notes: Vec<String> = Vec::new();
        if let Some(old) = &installed {
            if old != &version {
                notes.push(fill(s.notice_update, &[("old", old)]));
            } else {
                notes.push(fill(s.notice_same, &[("v", &version)]));
            }
        }
        if !sys::webview2_present() {
            notes.push(s.notice_webview2.to_string());
        }
        if self.running {
            notes.push(s.notice_running.to_string());
        }
        if !notes.is_empty() {
            self.text(&notes.join("\n"), &self.fonts.small_center.clone(), shift(rect(PAD, 452.0, CONTENT_W, 44.0), dy), alpha(th.dim, a));
        }

        let label = match &installed {
            Some(old) if old == &version => s.btn_reinstall,
            Some(_) => s.btn_update,
            None => s.btn_install,
        };
        self.draw_primary(label, shift(rect(PAD, BTN_Y, CONTENT_W, BTN_H), dy), a, live);

        // Fusszeile: zwei leise Links.
        let (w1, _) = self.measure(s.footer_license, &self.fonts.link.clone());
        let (w2, _) = self.measure(s.footer_source, &self.fonts.link.clone());
        let gap = 24.0;
        let total = w1 + w2 + gap;
        let x0 = (WIN_W - total) / 2.0;
        let y = 560.0 + dy;
        self.draw_link(Hot::LinkLicense, s.footer_license, rect(x0, y, w1, 20.0), a, live);
        self.text("·", &self.fonts.link.clone(), rect(x0 + w1, y, gap, 20.0), alpha(th.line[2], a));
        self.draw_link(Hot::LinkSource, s.footer_source, rect(x0 + w1 + gap, y, w2, 20.0), a, live);
    }

    fn draw_uninstall_start(&mut self, a: f32, dy: f32, live: bool) {
        let th = self.theme;
        let s = self.s;
        let purge = matches!(&self.mode, Mode::Uninstall { purge: true, .. });
        self.text(s.un_lead, &self.fonts.body_center.clone(), shift(rect(PAD, 248.0, CONTENT_W, 72.0), dy), alpha(th.dim, a));

        let row_h = 60.0;
        let card = shift(rect(PAD, 332.0, CONTENT_W, row_h + 2.0), dy);
        self.fill_rr(card, R_MD, alpha(th.surface, a));
        self.stroke_rr(card, R_MD, alpha(th.line[0], a), 1.0);
        let _ = purge;
        let row = rect(card.left + 1.0, card.top + 1.0, CONTENT_W - 2.0, row_h);
        self.draw_check_row(Hot::CheckPurge, row, s.un_opt_data, self.check_t[2], a, live);
        if self.running {
            self.text(s.notice_running, &self.fonts.small_center.clone(), shift(rect(PAD, 452.0, CONTENT_W, 24.0), dy), alpha(th.dim, a));
        }
        self.draw_primary(s.btn_remove, shift(rect(PAD, BTN_Y, CONTENT_W, BTN_H), dy), a, live);
        self.draw_centered_link(Hot::Secondary, s.btn_cancel, 560.0 + dy, a, live);
    }

    fn draw_working(&mut self, a: f32, dy: f32) {
        let th = self.theme;
        let bar = shift(rect(PAD, 372.0, CONTENT_W, 4.0), dy);
        self.fill_rr(bar, 2.0, alpha(th.ui[0], a));
        if self.indeterminate {
            let seg = CONTENT_W * 0.35;
            let x = bar.left - seg + (CONTENT_W + seg) * self.indet_phase;
            let l = x.max(bar.left);
            let r = (x + seg).min(bar.right);
            if r > l {
                self.fill_rr(rect(l, bar.top, r - l, 4.0), 2.0, alpha(th.accent, a));
            }
        } else {
            let w = (CONTENT_W * self.progress.value.clamp(0.0, 1.0)).max(4.0);
            self.fill_rr(rect(bar.left, bar.top, w, 4.0), 2.0, alpha(th.accent, a));
        }
        self.text(self.status, &self.fonts.small_center.clone(), shift(rect(PAD, 392.0, CONTENT_W, 44.0), dy), alpha(th.dim, a));
    }

    fn draw_install_done(&mut self, a: f32, dy: f32, live: bool) {
        let th = self.theme;
        let s = self.s;
        let (version, dir) = match &self.mode {
            Mode::Install { payload, opts, .. } => (payload.version.clone(), opts.dir.clone()),
            _ => return,
        };
        let (webview2_ok, registered) = self
            .outcome
            .as_ref()
            .map(|o| (o.webview2_ok, o.registered))
            .unwrap_or((true, false));

        self.draw_badge(rect(WIN_W / 2.0 - 16.0, 252.0 + dy, 32.0, 32.0), a);
        self.text(s.done_title, &self.fonts.h2.clone(), shift(rect(PAD, 296.0, CONTENT_W, 28.0), dy), alpha(th.text, a));
        self.text(
            &fill(s.done_sub, &[("v", &version), ("dir", &dir.to_string_lossy())]),
            &self.fonts.small_center.clone(),
            shift(rect(PAD, 328.0, CONTENT_W, 40.0), dy),
            alpha(th.dim, a),
        );
        let mut y = 384.0;
        if !webview2_ok {
            self.text(s.done_webview2_failed, &self.fonts.small_center.clone(), shift(rect(PAD, y, CONTENT_W, 60.0), dy), alpha(th.danger, a));
            y += 64.0;
            self.draw_centered_link(Hot::LinkWebView2, s.link_webview2, y + dy, a, live);
        } else if registered {
            self.draw_centered_link(Hot::LinkDefault, s.link_default, 440.0 + dy, a, live);
        }
        self.draw_primary(s.btn_launch, shift(rect(PAD, BTN_Y, CONTENT_W, BTN_H), dy), a, live);
        self.draw_centered_link(Hot::Secondary, s.btn_close, 560.0 + dy, a, live);
    }

    fn draw_uninstall_done(&mut self, a: f32, dy: f32, live: bool) {
        let th = self.theme;
        let s = self.s;
        self.draw_badge(rect(WIN_W / 2.0 - 16.0, 252.0 + dy, 32.0, 32.0), a);
        self.text(s.un_done, &self.fonts.h2.clone(), shift(rect(PAD, 296.0, CONTENT_W, 28.0), dy), alpha(th.text, a));
        self.draw_primary(s.btn_close, shift(rect(PAD, BTN_Y, CONTENT_W, BTN_H), dy), a, live);
    }

    fn draw_failed(&mut self, a: f32, dy: f32, live: bool) {
        let th = self.theme;
        let s = self.s;
        let error = self.error.clone().unwrap_or_default();
        self.text(s.fail_title, &self.fonts.h2.clone(), shift(rect(PAD, 264.0, CONTENT_W, 28.0), dy), alpha(th.text, a));
        self.text(&error, &self.fonts.small_center.clone(), shift(rect(PAD, 300.0, CONTENT_W, 100.0), dy), alpha(th.dim, a));
        self.draw_centered_link(Hot::LinkLog, s.link_log, 420.0 + dy, a, live);
        self.draw_primary(s.btn_retry, shift(rect(PAD, BTN_Y, CONTENT_W, BTN_H), dy), a, live);
        self.draw_centered_link(Hot::Secondary, s.btn_close, 560.0 + dy, a, live);
    }

    // ------------------------------------------------------------ Bausteine

    /// Eine Zeile der Karte: Kaestchen links, Text daneben. `t` ist der
    /// Ankreuz-Grad (0..1), damit das Haekchen kurz eingezeichnet wird.
    fn draw_check_row(&mut self, hot: Hot, row: D2D_RECT_F, label: &str, t: f32, a: f32, live: bool) {
        let th = self.theme;
        let ht = self.hover_t[hot.idx()];
        if ht > 0.0 {
            self.fill_rect(row, alpha(th.ui[0], a * ht * 0.9));
        }
        let cy = (row.top + row.bottom) / 2.0;
        let bx = rect(row.left + 16.0, cy - 9.0, 18.0, 18.0);
        // Rahmen, der beim Ankreuzen von der Fuellung verdeckt wird.
        self.stroke_rr(bx, 5.0, alpha(mix(th.line[1], th.line[2], ht), a), 1.0);
        if t > 0.001 {
            let inner = inflate(bx, -1.5 * (1.0 - t));
            self.fill_rr(inner, 5.0, alpha(th.text, a * t));
            // Haekchen, kurz eingezeichnet.
            let c = alpha(th.bg, a * t);
            let (x, y) = (bx.left, bx.top);
            let p0 = v2(x + 4.5, y + 9.5);
            let p1 = v2(x + 7.5, y + 12.5);
            let p2 = v2(x + 13.5, y + 5.5);
            let k = (t * 1.6).min(1.0);
            let m1 = if k < 0.5 { lerp_v(p0, p1, k * 2.0) } else { p1 };
            self.line_w(p0, m1, c, 2.0);
            if k > 0.5 {
                let m2 = lerp_v(p1, p2, (k - 0.5) * 2.0);
                self.line_w(p1, m2, c, 2.0);
            }
        }
        let label_r = rect(row.left + 46.0, row.top, row.right - 16.0 - (row.left + 46.0), row.bottom - row.top);
        let font = if row.bottom - row.top > ROW_H + 1.0 { self.fonts.body_wrap.clone() } else { self.fonts.body.clone() };
        self.text(label, &font, label_r, alpha(th.text, a));
        if live {
            self.hits.push((hot, row));
            self.focus_order.push(hot);
        }
    }

    fn draw_primary(&mut self, label: &str, r: D2D_RECT_F, a: f32, live: bool) {
        let th = self.theme;
        let ht = self.hover_t[Hot::Primary.idx()];
        let pressed = self.pressed == Hot::Primary && self.hot == Hot::Primary;
        // Umgekehrt eingefaerbt – schwarz auf Weiss, weiss auf Schwarz.
        let mut fill = mix(th.text, th.bg, 0.16 * ht);
        if pressed {
            fill = mix(th.text, th.bg, 0.3);
        }
        self.fill_rr(r, R_MD, alpha(fill, a));
        self.text(label, &self.fonts.button.clone(), r, alpha(th.bg, a));
        if live {
            self.hits.push((Hot::Primary, r));
            self.focus_order.push(Hot::Primary);
        }
    }

    fn draw_link(&mut self, hot: Hot, label: &str, r: D2D_RECT_F, a: f32, live: bool) {
        let th = self.theme;
        let ht = self.hover_t[hot.idx()];
        let col = mix(th.dim, th.text, ht);
        self.text(label, &self.fonts.link.clone(), r, alpha(col, a));
        if ht > 0.0 {
            let y = r.bottom - 3.5;
            self.line(v2(r.left, y), v2(r.left + (r.right - r.left) * ht.min(1.0), y), alpha(col, a * ht));
        }
        if live {
            self.hits.push((hot, inflate(r, 6.0)));
            self.focus_order.push(hot);
        }
    }

    fn draw_centered_link(&mut self, hot: Hot, label: &str, y: f32, a: f32, live: bool) {
        let (w, _) = self.measure(label, &self.fonts.link.clone());
        self.draw_link(hot, label, rect((WIN_W - w) / 2.0, y, w, 20.0), a, live);
    }

    /// Gruener Punkt mit Haken – "fertig".
    fn draw_badge(&mut self, r: D2D_RECT_F, a: f32) {
        let th = self.theme;
        let cx = (r.left + r.right) / 2.0;
        let cy = (r.top + r.bottom) / 2.0;
        if let Some(rt) = &self.rt {
            unsafe {
                if let Ok(b) = rt.CreateSolidColorBrush(&alpha(th.success, a), None) {
                    rt.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: v2(cx, cy),
                            radiusX: 16.0,
                            radiusY: 16.0,
                        },
                        &b,
                    );
                }
            }
        }
        let c = alpha(rgb(0xffffff), a);
        self.line_w(v2(cx - 6.5, cy + 0.5), v2(cx - 2.0, cy + 5.0), c, 2.2);
        self.line_w(v2(cx - 2.0, cy + 5.0), v2(cx + 7.0, cy - 5.0), c, 2.2);
    }

    fn draw_caption(&mut self) {
        let th = self.theme;
        let close = rect(WIN_W - CAP_BTN_W - 8.0, 8.0, CAP_BTN_W, CAP_BTN_H);
        let min = rect(WIN_W - CAP_BTN_W * 2.0 - 8.0, 8.0, CAP_BTN_W, CAP_BTN_H);
        for (hot, r, is_close) in [(Hot::Min, min, false), (Hot::Close, close, true)] {
            let t = self.hover_t[hot.idx()];
            let disabled = is_close && self.screen == Screen::Working;
            if t > 0.0 && !disabled {
                let c = if is_close { alpha(th.danger, t) } else { alpha(th.ui[1], t * 1.4) };
                self.fill_rr(r, R_SM, c);
            }
            let fg = if is_close && t > 0.5 && !disabled { rgb(0xffffff) } else if disabled { th.line[2] } else { th.text };
            let cx = ((r.left + r.right) / 2.0).round() + 0.5;
            let cy = ((r.top + r.bottom) / 2.0).round() + 0.5;
            let s = 5.0;
            if is_close {
                self.line_w(v2(cx - s, cy - s), v2(cx + s, cy + s), fg, 1.2);
                self.line_w(v2(cx + s, cy - s), v2(cx - s, cy + s), fg, 1.2);
            } else {
                self.line_w(v2(cx - s, cy), v2(cx + s, cy), fg, 1.2);
            }
            self.hits.push((hot, r));
        }
    }

    fn draw_focus(&mut self) {
        if !self.focus_visible {
            return;
        }
        let Some(f) = self.focus else { return };
        let Some((_, r)) = self.hits.iter().find(|(h, _)| *h == f).copied() else { return };
        let radius = if f == Hot::Primary { R_MD + 2.0 } else { R_SM + 2.0 };
        let r = if f.is_link() { inflate(r, -3.0) } else { inflate(r, 2.0) };
        self.stroke_rr(r, radius, self.theme.accent, 2.0);
    }

    fn draw_icon(&mut self, r: D2D_RECT_F, a: f32) {
        let Some(rt) = &self.rt else { return };
        unsafe {
            if let Some(bmp) = &self.icon {
                rt.DrawBitmap(bmp, Some(&r), a, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                return;
            }
            // Ohne Ressource: die Kugel selbst zeichnen, wie in der Leiste.
            let cx = (r.left + r.right) / 2.0;
            let cy = (r.top + r.bottom) / 2.0;
            let rad = (r.right - r.left) / 2.0 - 4.0;
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: alpha(rgb(0xc6b8f5), a) },
                D2D1_GRADIENT_STOP { position: 1.0, color: alpha(rgb(0x6e5bd0), a) },
            ];
            if let Ok(gs) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
                if let Ok(gb) = rt.CreateRadialGradientBrush(
                    &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                        center: v2(cx - rad * 0.3, cy - rad * 0.35),
                        gradientOriginOffset: v2(0.0, 0.0),
                        radiusX: rad * 1.4,
                        radiusY: rad * 1.4,
                    },
                    None,
                    &gs,
                ) {
                    rt.FillEllipse(
                        &D2D1_ELLIPSE { point: v2(cx, cy), radiusX: rad, radiusY: rad },
                        &gb,
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------ Primitive

    fn text(&self, s: &str, fmt: &IDWriteTextFormat, r: D2D_RECT_F, c: Color) {
        let Some(rt) = &self.rt else { return };
        if s.is_empty() || c.a <= 0.002 {
            return;
        }
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            if let Ok(b) = rt.CreateSolidColorBrush(&c, None) {
                rt.DrawText(&w, fmt, &r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn measure(&self, s: &str, fmt: &IDWriteTextFormat) -> (f32, f32) {
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            if let Ok(layout) = self.dwrite.CreateTextLayout(&w, fmt, 4096.0, 4096.0) {
                let mut m = DWRITE_TEXT_METRICS::default();
                if layout.GetMetrics(&mut m).is_ok() {
                    return (m.widthIncludingTrailingWhitespace, m.height);
                }
            }
        }
        (0.0, 0.0)
    }

    fn fill_rect(&self, r: D2D_RECT_F, c: Color) {
        let Some(rt) = &self.rt else { return };
        unsafe {
            if let Ok(b) = rt.CreateSolidColorBrush(&c, None) {
                rt.FillRectangle(&r, &b);
            }
        }
    }

    fn fill_rr(&self, r: D2D_RECT_F, radius: f32, c: Color) {
        let Some(rt) = &self.rt else { return };
        unsafe {
            if let Ok(b) = rt.CreateSolidColorBrush(&c, None) {
                rt.FillRoundedRectangle(&rounded(r, radius), &b);
            }
        }
    }

    fn stroke_rr(&self, r: D2D_RECT_F, radius: f32, c: Color, w: f32) {
        let Some(rt) = &self.rt else { return };
        unsafe {
            if let Ok(b) = rt.CreateSolidColorBrush(&c, None) {
                rt.DrawRoundedRectangle(&rounded(inflate(r, -w / 2.0), radius), &b, w, None);
            }
        }
    }

    fn line(&self, p0: Vector2, p1: Vector2, c: Color) {
        self.line_w(p0, p1, c, 1.0);
    }

    fn line_w(&self, p0: Vector2, p1: Vector2, c: Color, w: f32) {
        let Some(rt) = &self.rt else { return };
        unsafe {
            if let Ok(b) = rt.CreateSolidColorBrush(&c, None) {
                rt.DrawLine(p0, p1, &b, w, None);
            }
        }
    }
}

fn lerp_v(a: Vector2, b: Vector2, t: f32) -> Vector2 {
    v2(a.X + (b.X - a.X) * t, a.Y + (b.Y - a.Y) * t)
}

// ---------------------------------------------------------------- Schriften

impl Fonts {
    fn new(dwrite: &IDWriteFactory, lang: Lang) -> windows::core::Result<Fonts> {
        // Windows 11 bringt Segoe UI Variable mit – Display fuer Grosses, Text
        // fuer den Rest. Windows 10 hat nur Segoe UI.
        let display = if has_family(dwrite, "Segoe UI Variable Display") {
            "Segoe UI Variable Display"
        } else {
            "Segoe UI"
        };
        let text = if has_family(dwrite, "Segoe UI Variable Text") {
            "Segoe UI Variable Text"
        } else {
            "Segoe UI"
        };
        let locale = match lang {
            Lang::De => "de-de",
            Lang::En => "en-us",
        };
        let mk = |family: &str, size: f32, weight: DWRITE_FONT_WEIGHT, center: bool, wrap: bool, vcenter: bool| -> windows::core::Result<IDWriteTextFormat> {
            let fam = wide(family);
            let loc = wide(locale);
            unsafe {
                let f = dwrite.CreateTextFormat(
                    PCWSTR(fam.as_ptr()),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    PCWSTR(loc.as_ptr()),
                )?;
                f.SetTextAlignment(if center { DWRITE_TEXT_ALIGNMENT_CENTER } else { DWRITE_TEXT_ALIGNMENT_LEADING })?;
                f.SetParagraphAlignment(if vcenter { DWRITE_PARAGRAPH_ALIGNMENT_CENTER } else { DWRITE_PARAGRAPH_ALIGNMENT_NEAR })?;
                if !wrap {
                    f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                    if let Ok(sign) = dwrite.CreateEllipsisTrimmingSign(&f) {
                        let trim = DWRITE_TRIMMING {
                            granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                            delimiter: 0,
                            delimiterCount: 0,
                        };
                        let _ = f.SetTrimming(&trim, &sign);
                    }
                }
                Ok(f)
            }
        };
        Ok(Fonts {
            title: mk(display, 26.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true, false, true)?,
            h2: mk(text, 17.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true, false, true)?,
            body: mk(text, 14.0, DWRITE_FONT_WEIGHT_NORMAL, false, false, true)?,
            body_wrap: mk(text, 14.0, DWRITE_FONT_WEIGHT_NORMAL, false, true, true)?,
            body_center: mk(text, 14.0, DWRITE_FONT_WEIGHT_NORMAL, true, true, false)?,
            small: mk(text, 13.0, DWRITE_FONT_WEIGHT_NORMAL, false, false, true)?,
            small_center: mk(text, 13.0, DWRITE_FONT_WEIGHT_NORMAL, true, true, false)?,
            button: mk(text, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, true, false, true)?,
            link: mk(text, 13.0, DWRITE_FONT_WEIGHT_NORMAL, true, false, true)?,
        })
    }
}

fn has_family(dwrite: &IDWriteFactory, name: &str) -> bool {
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        if dwrite.GetSystemFontCollection(&mut coll, false).is_err() {
            return false;
        }
        let Some(coll) = coll else { return false };
        let n = wide(name);
        let mut index = 0u32;
        let mut exists = BOOL(0);
        coll.FindFamilyName(PCWSTR(n.as_ptr()), &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}

// ---------------------------------------------------------------- Fensterprozedur

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        // Kein Rahmen: der Client-Bereich ist das ganze Fenster. Schatten und
        // runde Ecken zeichnet DWM trotzdem, weil der Stil WS_CAPTION traegt.
        WM_NCCALCSIZE if w.0 != 0 => return LRESULT(0),
        WM_NCCREATE => {
            let cs = &*(l.0 as *const CREATESTRUCTW);
            let ui = cs.lpCreateParams as *mut Ui;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ui as isize);
            if !ui.is_null() {
                (*ui).hwnd = hwnd;
            }
            return DefWindowProcW(hwnd, msg, w, l);
        }
        _ => {}
    }
    let ui = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Ui;
    if ui.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    (*ui).handle(hwnd, msg, w, l)
}
