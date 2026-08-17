// Beautiful popup windows (context menus, tooltips, suggestions, dialogs) rendered
// with Direct2D into per-pixel-alpha layered windows.
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::{post, App, AppMsg, APP_PTR};
use crate::gfx::*;
use crate::omnibox::Suggestion;
use crate::theme::{R_LG, R_SM};
use crate::util::*;

pub struct MenuItem {
    pub label: String,
    pub icon: String,
    pub shortcut: Option<String>,
    pub action: String,
    pub enabled: bool,
    pub separator: bool,
}

impl MenuItem {
    pub fn new(label: &str, icon: &str, action: &str) -> MenuItem {
        MenuItem {
            label: label.to_string(),
            icon: icon.to_string(),
            shortcut: None,
            action: action.to_string(),
            enabled: true,
            separator: false,
        }
    }
    pub fn sep() -> MenuItem {
        MenuItem {
            label: String::new(),
            icon: String::new(),
            shortcut: None,
            action: String::new(),
            enabled: false,
            separator: true,
        }
    }
    pub fn shortcut(mut self, s: &str) -> MenuItem {
        self.shortcut = Some(s.to_string());
        self
    }
}

pub enum PopupKind {
    Tooltip { title: String, sub: String },
    Menu { items: Vec<MenuItem>, hover: i32 },
    Suggestions { items: Vec<Suggestion>, selected: usize },
    Permission { kind: String, origin: String, hover: i32, remember: bool },
}

pub struct Popup {
    pub hwnd: HWND,
    pub kind: PopupKind,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    memdc: HDC,
    dib: HBITMAP,
    target: Option<ID2D1DCRenderTarget>,
    /// Fade-in state: 0..255 window alpha and the anchor the popup slides to.
    alpha: u8,
    t0: u64,
    base_y: i32,
}

const FADE_MS: f32 = 130.0;
const SLIDE_PX: f32 = 7.0;
const TIMER_FADE: usize = 1;

fn tick() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

const MENU_ROW: f32 = 32.0;
const SUGG_ROW: f32 = 46.0;

pub fn register_class(hinst: HINSTANCE) -> windows::core::Result<()> {
    let class = wide("AuraPopup");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(popup_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: HICON::default(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: PCWSTR(class.as_ptr()),
        hIconSm: HICON::default(),
    };
    unsafe {
        if RegisterClassExW(&wc) == 0 { return Err(windows::core::Error::from_thread()); }
    }
    Ok(())
}

fn measure(app: &App, kind: &PopupKind) -> (f32, f32) {
    let gfx = &app.gfx;
    match kind {
        PopupKind::Tooltip { title, sub } => {
            let wt = text_width(gfx, &app.fmt_semibold, title, 380.0);
            let ws = text_width(gfx, &app.fmt_small, sub, 380.0);
            // 8 px window inset + 24 px text padding, plus a little slack.
            (wt.max(ws) + 40.0, 56.0)
        }
        PopupKind::Menu { items, .. } => {
            let mut wmax: f32 = 180.0;
            for it in items {
                if it.separator {
                    continue;
                }
                let mut w = text_width(gfx, &app.fmt_ui, &it.label, 500.0) + 62.0;
                if let Some(s) = &it.shortcut {
                    w += text_width(gfx, &app.fmt_small, s, 200.0) + 16.0;
                }
                wmax = wmax.max(w);
            }
            let rows = items.iter().filter(|i| !i.separator).count() as f32;
            let seps = items.iter().filter(|i| i.separator).count() as f32;
            // 4 px window inset + 6 px padding on both sides = 20 px of chrome.
            (wmax.min(460.0), rows * MENU_ROW + seps * 9.0 + 20.0)
        }
        PopupKind::Suggestions { items, .. } => {
            let w = (app.layout.omnibox.right - app.layout.omnibox.left).max(320.0);
            (w, items.len() as f32 * SUGG_ROW + 20.0)
        }
        PopupKind::Permission { .. } => (360.0, 132.0),
    }
}

fn text_width(gfx: &Gfx, fmt: &IDWriteTextFormat, text: &str, max: f32) -> f32 {
    let utf: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        if let Ok(l) = gfx.dwrite.CreateTextLayout(&utf, fmt, max, 100.0) {
            let mut m = DWRITE_TEXT_METRICS::default();
            if l.GetMetrics(&mut m).is_ok() {
                return m.width;
            }
        }
    }
    0.0
}

fn create(app: &App, kind: PopupKind, x: i32, y: i32, activate: bool) -> Option<Box<Popup>> {
    let scale = app.scale;
    let (w_dip, h_dip) = measure(app, &kind);
    let w = (w_dip * scale).ceil() as i32;
    let h = (h_dip * scale).ceil() as i32;

    // Clamp to work area.
    let mut x = x;
    let mut y = y;
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut info).as_bool() {
            let wa = info.rcWork;
            if x + w > wa.right {
                x = wa.right - w - 4;
            }
            if y + h > wa.bottom {
                y = wa.bottom - h - 4;
            }
            if x < wa.left {
                x = wa.left + 4;
            }
            if y < wa.top {
                y = wa.top + 4;
            }
        }
    }

    let class = wide("AuraPopup");
    let mut ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
    if !activate {
        ex |= WS_EX_NOACTIVATE;
    }
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            x, y, w, h,
            None, None, Some(app.hinst), None,
        )
        .ok()?
    };

    // DIB section for per-pixel alpha.
    let memdc = unsafe { CreateCompatibleDC(None) };
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD::default()],
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let dib = unsafe {
        CreateDIBSection(
            Some(memdc),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
        .ok()?
    };
    unsafe {
        SelectObject(memdc, HGDIOBJ(dib.0));
    }

    let props = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0 * scale,
        dpiY: 96.0 * scale,
        ..Default::default()
    };
    let target = unsafe { app.gfx.factory.CreateDCRenderTarget(&props).ok() };

    let reduce = app.theme.reduce_motion;
    let mut popup = Box::new(Popup {
        hwnd,
        kind,
        w,
        h,
        scale,
        memdc,
        dib,
        target,
        alpha: if reduce { 255 } else { 0 },
        t0: tick(),
        base_y: y,
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, popup.as_mut() as *mut Popup as isize);
        let _ = ShowWindow(hwnd, if activate { SW_SHOW } else { SW_SHOWNA });
        if activate {
            let _ = SetForegroundWindow(hwnd);
        }
        if !reduce {
            // Fade in and settle upwards a few pixels.
            let _ = SetWindowPos(
                hwnd, None, x, y + SLIDE_PX as i32, 0, 0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let _ = SetTimer(Some(hwnd), TIMER_FADE, 8, None);
        }
    }
    popup.repaint();
    Some(popup)
}

impl Popup {
    pub fn close(self: Box<Popup>) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteDC(self.memdc);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
        }
    }

    pub fn repaint(&mut self) {
        let app_ptr = APP_PTR.with(|p| p.get());
        if app_ptr.is_null() {
            return;
        }
        let app = unsafe { &*app_ptr };
        let Some(target) = &self.target else { return };
        let theme = &app.theme;
        let w_dip = self.w as f32 / self.scale;
        let h_dip = self.h as f32 / self.scale;
        let rect_px = RECT { left: 0, top: 0, right: self.w, bottom: self.h };
        unsafe {
            let _ = target.BindDC(self.memdc, &rect_px);
            target.BeginDraw();
            let rt: ID2D1RenderTarget = target.cast().unwrap();
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));

            // Schlagschatten aus gestapelten durchscheinenden Rechtecken. Ein
            // schwebendes Menü braucht ihn, damit es sich von der Seite löst —
            // im Fenster selbst kommt Tiefe dagegen aus Flächenabstufung.
            for i in 0..4 {
                let pad = 1.0 + i as f32;
                if let Ok(b) = brush(&rt, color(0, 0, 0, 0.07 - i as f32 * 0.015)) {
                    rt.FillRoundedRectangle(
                        &rounded(rect_f(pad, pad + 1.5, w_dip - pad * 2.0, h_dip - pad * 2.0 - 1.5), R_LG),
                        &b,
                    );
                }
            }
            // body
            let body = rect_f(4.0, 4.0, w_dip - 8.0, h_dip - 8.0);
            if let Ok(b) = brush(&rt, theme.popup_bg) {
                rt.FillRoundedRectangle(&rounded(body, R_LG), &b);
            }
            if let Ok(b) = brush(&rt, theme.border) {
                rt.DrawRoundedRectangle(&rounded(body, R_LG), &b, 1.0, None);
            }

            match &self.kind {
                PopupKind::Tooltip { title, sub } => {
                    self.draw_text(&rt, &app.fmt_semibold, theme.text, title, rect_f(body.left + 12.0, body.top + 8.0, body.right - body.left - 24.0, 18.0));
                    self.draw_text(&rt, &app.fmt_small, theme.text_dim, sub, rect_f(body.left + 12.0, body.top + 26.0, body.right - body.left - 24.0, 15.0));
                }
                PopupKind::Menu { items, hover } => {
                    let mut y = body.top + 6.0;
                    for (i, it) in items.iter().enumerate() {
                        if it.separator {
                            if let Ok(b) = brush(&rt, theme.border) {
                                rt.FillRectangle(&rect_f(body.left + 10.0, y + 4.0, body.right - body.left - 20.0, 1.0), &b);
                            }
                            y += 9.0;
                            continue;
                        }
                        let row = rect_f(body.left + 4.0, y, body.right - body.left - 8.0, MENU_ROW);
                        if *hover == i as i32 && it.enabled {
                            if let Ok(b) = brush(&rt, theme.hover) {
                                rt.FillRoundedRectangle(&rounded(row, R_SM), &b);
                            }
                        }
                        // icon
                        if !it.icon.is_empty() {
                            let ch = u32::from_str_radix(&it.icon, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .unwrap_or(' ');
                            let glyph: Vec<u16> = ch.to_string().encode_utf16().collect();
                            if let Ok(b) = brush(&rt, theme.text_dim) {
                                app.fmt_icon_sm.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                                app.fmt_icon_sm.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                                rt.DrawText(
                                    &glyph,
                                    &app.fmt_icon_sm,
                                    &rect_f(row.left + 6.0, row.top, 24.0, row.bottom - row.top),
                                    &b,
                                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                                    DWRITE_MEASURING_MODE_NATURAL,
                                );
                            }
                        }
                        let fg = if it.enabled { theme.text } else { theme.text_dim };
                        self.draw_text(&rt, &app.fmt_ui, fg, &it.label, rect_f(row.left + 36.0, row.top, row.right - row.left - 36.0, row.bottom - row.top));
                        if let Some(s) = &it.shortcut {
                            let sw = text_width(&app.gfx, &app.fmt_small, s, 200.0);
                            self.draw_text_right(&rt, &app.fmt_small, theme.text_dim, s, rect_f(row.right - sw - 12.0, row.top, sw, row.bottom - row.top));
                        }
                        y += MENU_ROW;
                    }
                }
                PopupKind::Suggestions { items, selected } => {
                    let mut y = body.top + 6.0;
                    for (i, s) in items.iter().enumerate() {
                        let row = rect_f(body.left + 4.0, y, body.right - body.left - 8.0, SUGG_ROW);
                        if i == *selected {
                            if let Ok(b) = brush(&rt, theme.accent_soft) {
                                rt.FillRoundedRectangle(&rounded(row, R_SM), &b);
                            }
                        }
                        // favicon / kind glyph
                        let icon_r = rect_f(row.left + 10.0, row.top + 12.0, 22.0, 22.0);
                        let mut drew = false;
                        if let Some(png) = &s.favicon {
                            if let Ok(bmp) = app.gfx.bitmap_from_bytes(&rt, png) {
                                rt.DrawBitmap(&bmp, Some(&icon_r), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                                drew = true;
                            }
                        }
                        if !drew {
                            let glyph = match s.kind {
                                crate::omnibox::SuggKind::Bookmark => "\u{E734}",
                                crate::omnibox::SuggKind::Tab => "\u{E8A5}",
                                crate::omnibox::SuggKind::History => "\u{E81C}",
                                crate::omnibox::SuggKind::Url => "\u{E774}",
                                crate::omnibox::SuggKind::Search => "\u{E721}",
                                crate::omnibox::SuggKind::Command => "\u{E756}",
                            };
                            if let Ok(b) = brush(&rt, theme.text_dim) {
                                let t: Vec<u16> = glyph.encode_utf16().collect();
                                app.fmt_icon_sm.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                                app.fmt_icon_sm.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                                rt.DrawText(&t, &app.fmt_icon_sm, &icon_r, &b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                            }
                        }
                        // title + url
                        self.draw_text(&rt, &app.fmt_semibold, theme.text, &s.title, rect_f(row.left + 42.0, row.top + 5.0, row.right - row.left - 150.0, 18.0));
                        let sub = if s.kind == crate::omnibox::SuggKind::Command { s.url.clone() } else { host_of(&s.url) };
                        self.draw_text(&rt, &app.fmt_small, theme.text_dim, &sub, rect_f(row.left + 42.0, row.top + 24.0, row.right - row.left - 150.0, 15.0));
                        // kind label
                        let kind = s.kind.label();
                        let kw = text_width(&app.gfx, &app.fmt_small, kind, 120.0);
                        self.draw_text_right(&rt, &app.fmt_small, theme.text_dim, kind, rect_f(row.right - kw - 12.0, row.top + 15.0, kw, 16.0));
                        y += SUGG_ROW;
                    }
                }
                PopupKind::Permission { kind, origin, hover, remember } => {
                    let title = format!("{kind} zulassen?");
                    self.draw_text(&rt, &app.fmt_semibold, theme.text, &title, rect_f(body.left + 14.0, body.top + 10.0, body.right - body.left - 28.0, 20.0));
                    self.draw_text(&rt, &app.fmt_small, theme.text_dim, &format!("{origin} möchte diese Berechtigung verwenden."), rect_f(body.left + 14.0, body.top + 32.0, body.right - body.left - 28.0, 16.0));
                    // remember toggle
                    let chk = rect_f(body.left + 14.0, body.top + 54.0, 14.0, 14.0);
                    if let Ok(b) = brush(&rt, theme.border) {
                        rt.DrawRoundedRectangle(&rounded(chk, 5.0), &b, 1.0, None);
                    }
                    if *remember {
                        if let Ok(b) = brush(&rt, theme.accent_f) {
                            rt.FillRoundedRectangle(&rounded(rect_f(chk.left + 3.0, chk.top + 3.0, 8.0, 8.0), 2.0), &b);
                        }
                    }
                    self.draw_text(&rt, &app.fmt_small, theme.text_dim, "Für diese Website merken", rect_f(body.left + 34.0, body.top + 52.0, 200.0, 16.0));
                    // buttons
                    let bw = 100.0;
                    let deny_r = rect_f(body.right - bw * 2.0 - 22.0, body.bottom - 38.0, bw, 30.0);
                    let allow_r = rect_f(body.right - bw - 12.0, body.bottom - 38.0, bw, 30.0);
                    if let Ok(b) = brush(&rt, if *hover == 1 { theme.active } else { theme.hover }) {
                        rt.FillRoundedRectangle(&rounded(deny_r, R_SM), &b);
                    }
                    if let Ok(b) = brush(&rt, theme.accent_f) {
                        rt.FillRoundedRectangle(&rounded(allow_r, R_SM), &b);
                    }
                    self.draw_text_center(&rt, &app.fmt_ui, theme.text, "Blockieren", deny_r);
                    self.draw_text_center(&rt, &app.fmt_ui, color(255, 255, 255, 1.0), "Zulassen", allow_r);
                }
            }

            let _ = target.EndDraw(None, None);
        }
        // push to screen
        unsafe {
            let mut pos = POINT::default();
            let mut wr = RECT::default();
            let _ = GetWindowRect(self.hwnd, &mut wr);
            pos.x = wr.left;
            pos.y = wr.top;
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: self.alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&pos),
                Some(&size),
                Some(self.memdc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    fn draw_text(&self, rt: &ID2D1RenderTarget, fmt: &IDWriteTextFormat, c: D2D1_COLOR_F, text: &str, r: D2D_RECT_F) {
        unsafe {
            if let Ok(b) = brush(rt, c) {
                let t: Vec<u16> = text.encode_utf16().collect();
                fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
                fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                rt.DrawText(&t, fmt, &r, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn draw_text_right(&self, rt: &ID2D1RenderTarget, fmt: &IDWriteTextFormat, c: D2D1_COLOR_F, text: &str, r: D2D_RECT_F) {
        unsafe {
            if let Ok(b) = brush(rt, c) {
                let t: Vec<u16> = text.encode_utf16().collect();
                fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING).ok();
                fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                rt.DrawText(&t, fmt, &r, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn draw_text_center(&self, rt: &ID2D1RenderTarget, fmt: &IDWriteTextFormat, c: D2D1_COLOR_F, text: &str, r: D2D_RECT_F) {
        unsafe {
            if let Ok(b) = brush(rt, c) {
                let t: Vec<u16> = text.encode_utf16().collect();
                fmt.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER).ok();
                rt.DrawText(&t, fmt, &r, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    fn row_at(&self, y_dip: f32) -> i32 {
        match &self.kind {
            PopupKind::Menu { items, .. } => {
                let mut yy = 10.0;
                for (i, it) in items.iter().enumerate() {
                    let h = if it.separator { 9.0 } else { MENU_ROW };
                    if y_dip >= yy && y_dip < yy + h {
                        return if it.separator { -1 } else { i as i32 };
                    }
                    yy += h;
                }
                -1
            }
            PopupKind::Suggestions { items, .. } => {
                let idx = ((y_dip - 10.0) / SUGG_ROW) as i32;
                if idx >= 0 && (idx as usize) < items.len() {
                    idx
                } else {
                    -1
                }
            }
            _ => -1,
        }
    }
}

// ---------- public show helpers ----------
pub fn show_tooltip(app: &mut App, x: i32, y: i32, title: &str, sub: &str) {
    if let Some(t) = app.tooltip.take() {
        t.close();
    }
    app.tooltip = create(
        app,
        PopupKind::Tooltip { title: title.to_string(), sub: sub.to_string() },
        x, y, false,
    );
}

pub fn show_menu(app: &mut App, x: i32, y: i32, items: Vec<MenuItem>) {
    if let Some(m) = app.menu_popup.take() {
        m.close();
    }
    app.menu_popup = create(app, PopupKind::Menu { items, hover: -1 }, x, y, true);
}

pub fn show_suggestions(app: &mut App, items: Vec<Suggestion>) {
    let r = app.layout.omnibox;
    let mut pt = POINT {
        x: (r.left * app.scale) as i32,
        y: ((r.bottom + 4.0) * app.scale) as i32,
    };
    unsafe {
        let _ = ClientToScreen(app.hwnd, &mut pt);
    }
    let replace = app.sugg_popup.is_some();
    if let Some(p) = app.sugg_popup.take() {
        p.close();
    }
    let _ = replace;
    app.sugg_popup = create(app, PopupKind::Suggestions { items, selected: 0 }, pt.x, pt.y, false);
}

pub fn show_permission(app: &mut App, kind: &str, origin: &str) {
    if let Some(d) = app.dialog_popup.take() {
        d.close();
    }
    let r = app.layout.omnibox;
    let mut pt = POINT {
        x: ((r.right - 360.0) * app.scale) as i32,
        y: ((r.bottom + 6.0) * app.scale) as i32,
    };
    unsafe {
        let _ = ClientToScreen(app.hwnd, &mut pt);
    }
    app.dialog_popup = create(
        app,
        PopupKind::Permission { kind: kind.to_string(), origin: origin.to_string(), hover: -1, remember: false },
        pt.x, pt.y, true,
    );
}

// ---------- window procedure ----------
unsafe extern "system" fn popup_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Popup;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, w, l);
        }
        let popup = &mut *ptr;
        match msg {
            WM_PAINT => {
                popup.repaint();
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            WM_TIMER if w.0 == TIMER_FADE => {
                // Fade in while sliding the last few pixels into place.
                let p = ((tick().saturating_sub(popup.t0)) as f32 / FADE_MS).clamp(0.0, 1.0);
                let e = 1.0 - (1.0 - p) * (1.0 - p) * (1.0 - p);
                popup.alpha = (e * 255.0) as u8;
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                let y = popup.base_y + ((1.0 - e) * SLIDE_PX) as i32;
                if y != wr.top {
                    let _ = SetWindowPos(
                        hwnd, None, wr.left, y, 0, 0,
                        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                popup.repaint();
                if p >= 1.0 {
                    popup.alpha = 255;
                    let _ = KillTimer(Some(hwnd), TIMER_FADE);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let y = y_lparam(l) as f32 / popup.scale;
                let row = popup.row_at(y);
                match &mut popup.kind {
                    PopupKind::Menu { hover, .. } => {
                        if row != *hover {
                            *hover = row;
                            popup.repaint();
                        }
                    }
                    PopupKind::Permission { hover, .. } => {
                        let app_ptr = APP_PTR.with(|p| p.get());
                        let mut new_hover = -1;
                        if !app_ptr.is_null() {
                            let x = x_lparam(l) as f32 / popup.scale;
                            let w_dip = popup.w as f32 / popup.scale;
                            let h_dip = popup.h as f32 / popup.scale;
                            let body = rect_f(4.0, 4.0, w_dip - 8.0, h_dip - 8.0);
                            if y >= body.bottom - 38.0 && y <= body.bottom - 8.0 {
                                if x >= body.right - 112.0 {
                                    new_hover = 0; // allow
                                } else if x >= body.right - 224.0 {
                                    new_hover = 1; // deny
                                }
                            } else if y >= body.top + 50.0 && y <= body.top + 70.0 && x <= body.left + 240.0 {
                                new_hover = 2; // remember
                            }
                        }
                        if new_hover != *hover {
                            *hover = new_hover;
                            popup.repaint();
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let y = y_lparam(l) as f32 / popup.scale;
                let row = popup.row_at(y);
                match &mut popup.kind {
                    PopupKind::Menu { items, .. } => {
                        if row >= 0 {
                            if let Some(it) = items.get(row as usize) {
                                if it.enabled {
                                    post(AppMsg::MenuAction { action: it.action.clone() });
                                }
                            }
                        }
                    }
                    PopupKind::Suggestions { items, .. } => {
                        if row >= 0 && (row as usize) < items.len() {
                            post(AppMsg::MenuAction { action: format!("sugg:{row}") });
                        }
                    }
                    PopupKind::Permission { hover, remember, .. } => {
                        match *hover {
                            0 => post(AppMsg::PermissionAnswer { allow: true, remember: *remember }),
                            1 => post(AppMsg::PermissionAnswer { allow: false, remember: *remember }),
                            2 => {
                                *remember = !*remember;
                                popup.repaint();
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_KEYDOWN => {
                match &mut popup.kind {
                    PopupKind::Menu { items, hover } => {
                        let n = items.len() as i32;
                        match w.0 as u32 {
                            0x1B => post(AppMsg::MenuAction { action: "noop".into() }), // Esc
                            0x0D => {
                                if *hover >= 0 {
                                    if let Some(it) = items.get(*hover as usize) {
                                        if it.enabled {
                                            post(AppMsg::MenuAction { action: it.action.clone() });
                                        }
                                    }
                                }
                            }
                            0x26 | 0x28 => {
                                let dir = if w.0 as u32 == 0x26 { -1 } else { 1 };
                                let mut h = *hover;
                                for _ in 0..n.max(1) {
                                    h += dir;
                                    if h < 0 { h = n - 1; }
                                    if h >= n { h = 0; }
                                    if let Some(it) = items.get(h as usize) {
                                        if !it.separator && it.enabled {
                                            break;
                                        }
                                    }
                                }
                                *hover = h;
                                popup.repaint();
                            }
                            _ => {}
                        }
                    }
                    PopupKind::Permission { .. } => {
                        if w.0 as u32 == 0x1B {
                            post(AppMsg::PermissionAnswer { allow: false, remember: false });
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                match &popup.kind {
                    PopupKind::Menu { .. } => post(AppMsg::MenuAction { action: "noop".into() }),
                    PopupKind::Permission { .. } => {
                        post(AppMsg::PermissionAnswer { allow: false, remember: false })
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}
