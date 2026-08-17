// Direct2D / DirectWrite / WIC helpers for GPU-accelerated chrome rendering.
use windows::core::{Interface, Result};
use windows_numerics::Vector2;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

pub struct Gfx {
    pub factory: ID2D1Factory1,
    pub dwrite: IDWriteFactory,
    pub wic: IWICImagingFactory,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                Some(&D2D1_FACTORY_OPTIONS {
                    debugLevel: D2D1_DEBUG_LEVEL_NONE,
                }),
            )?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            Ok(Gfx {
                factory,
                dwrite,
                wic,
            })
        }
    }

    pub fn create_hwnd_rt(&self, hwnd: HWND, scale: f32) -> Result<ID2D1HwndRenderTarget> {
        let mut rc = RECT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
        }
        let dpi = 96.0 * scale;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
                alphaMode: D2D1_ALPHA_MODE_UNKNOWN,
            },
            dpiX: dpi,
            dpiY: dpi,
            ..Default::default()
        };
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U {
                width: rc.right as u32,
                height: rc.bottom as u32,
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        unsafe { self.factory.CreateHwndRenderTarget(&props, &hwnd_props) }
    }

    pub fn text_format(&self, size: f32, weight: DWRITE_FONT_WEIGHT) -> Result<IDWriteTextFormat> {
        unsafe {
            let fmt = self.dwrite.CreateTextFormat(
                windows::core::w!("Segoe UI Variable Text"),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                windows::core::w!("de-de"),
            )?;
            single_line(&self.dwrite, &fmt);
            Ok(fmt)
        }
    }

    #[allow(dead_code)]
    pub fn emoji_format(&self, size: f32) -> Result<IDWriteTextFormat> {
        unsafe {
            self.dwrite.CreateTextFormat(
                windows::core::w!("Segoe UI Emoji"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                windows::core::w!("de-de"),
            )
        }
    }

    /// Breite eines Textes in DIP mit diesem Format, höchstens `max`.
    pub fn text_width(&self, fmt: &IDWriteTextFormat, text: &str, max: f32) -> f32 {
        let utf: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            if let Ok(l) = self.dwrite.CreateTextLayout(&utf, fmt, max.max(1.0), 100.0) {
                let mut m = DWRITE_TEXT_METRICS::default();
                if l.GetMetrics(&mut m).is_ok() {
                    return m.width.min(max);
                }
            }
        }
        0.0
    }

    /// Decode PNG/JPEG/ICO bytes into a D2D bitmap via WIC.
    pub fn bitmap_from_bytes(
        &self,
        rt: &ID2D1RenderTarget,
        bytes: &[u8],
    ) -> Result<ID2D1Bitmap> {
        unsafe {
            let stream = self.wic.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let decoder =
                self.wic
                    .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
            let frame = decoder.GetFrame(0)?;
            let conv = self.wic.CreateFormatConverter()?;
            conv.Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )?;
            rt.CreateBitmapFromWicBitmap(&conv, None)
        }
    }
}

/// Eine Zeile, kein Umbruch, überlanger Text endet in Auslassungspunkten.
/// Ohne das brechen URLs, Tab-Titel und Tooltips mitten im Wort um und laufen
/// aus ihren Feldern heraus.
fn single_line(dwrite: &IDWriteFactory, fmt: &IDWriteTextFormat) {
    unsafe {
        let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        if let Ok(sign) = dwrite.CreateEllipsisTrimmingSign(fmt) {
            let trim = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let _ = fmt.SetTrimming(&trim, &sign);
        }
    }
}

/// GPU device shared by the composition surface. Created once, before the
/// window exists, so we know up front whether the glass path is available.
pub struct CompDevice {
    d3d: ID3D11Device,
    dxgi: IDXGIDevice,
    pub context: ID2D1DeviceContext,
}

/// A DirectComposition-backed surface: a per-pixel-alpha swap chain hung off a
/// composition visual. This is what lets Mica/Acrylic shine through the chrome.
///
/// Zwei Bäume am selben Fenster:
///
/// * **Grundplatte** (`CreateTargetForHwnd(…, false)`, hinter den Kindfenstern):
///   eine winzige, einfarbige Fläche, riesig skaliert. Wo weder Oberfläche noch
///   Seite etwas malen — Tab wechselt gerade, Fenster wächst schneller als die
///   Seite nachkommt, Ansicht startet noch — sieht man diese Farbe und nicht den
///   Schreibtisch.
/// * **Oberfläche** (`CreateTargetForHwnd(…, true)`, über den Kindfenstern): die
///   Kette mit der Leiste, der Kopfzeile, den Knöpfen. Der Inhaltsbereich bleibt
///   darin durchsichtig, damit die Seite durchkommt. Weil sie *über* der Seite
///   liegt, darf die Leiste beim Ziehen über die noch nicht nachgezogene Seite
///   malen.
///
/// Die Kette ist größer als das Fenster (Überschuss bis zur Monitorgröße): beim
/// Ziehen am Rand muss sie nicht umgebaut werden, und der Streifen, den das
/// Fenster gerade freilegt, ist schon bemalt — bevor der Fensterverwalter
/// unser nächstes Bild hat. Ohne das schien dort für ein Bild der Schreibtisch
/// durch: „beim Resizen wird der Hintergrund durchsichtig“.
pub struct Composition {
    pub dc: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
    comp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    plate: Option<Backplate>,
    /// Größe der Kette (Bildpunkte) — mindestens das Fenster, meist der Monitor.
    size: (u32, u32),
    dpi: f32,
    /// Das Gerät ist verloren gegangen (Treiber-Reset, Aufwachen, Monitor
    /// umgesteckt). Ab jetzt hilft nur ein Neuaufbau von Gerät und Fläche.
    pub lost: bool,
}

/// Die einfarbige Grundplatte hinter allem (siehe [`Composition`]).
struct Backplate {
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    surface: IDCompositionSurface,
    color: D2D1_COLOR_F,
}

/// Seitenlänge der Grundplattenfläche. Sie wird auf 32768 Bildpunkte gestreckt.
const PLATE_PX: u32 = 4;
const PLATE_SCALE: f32 = 32768.0 / PLATE_PX as f32;

/// Ergebnis eines Bildes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Frame {
    Ok,
    /// Zeichnen fehlgeschlagen, Gerät aber noch da — Bild einfach verwerfen.
    Skipped,
    /// Gerät weg — Aufrufer muss `Composition` und `CompDevice` neu anlegen.
    DeviceLost,
}

/// Fehlercodes, die „Gerät ist weg“ bedeuten.
fn is_device_lost(hr: windows::core::HRESULT) -> bool {
    hr == DXGI_ERROR_DEVICE_REMOVED
        || hr == DXGI_ERROR_DEVICE_RESET
        || hr == DXGI_ERROR_DRIVER_INTERNAL_ERROR
        || hr == windows::Win32::Foundation::D2DERR_RECREATE_TARGET
}

impl Gfx {
    /// Builds the D3D11 + D2D device. Returns None when the machine cannot do
    /// hardware composition; the caller then falls back to an HWND target.
    pub fn create_comp_device(&self) -> Option<CompDevice> {
        unsafe {
            let mut d3d: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )
            .ok()?;
            let d3d = d3d?;
            let dxgi: IDXGIDevice = d3d.cast().ok()?;
            let d2d_device = self.factory.CreateDevice(&dxgi).ok()?;
            let context = d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .ok()?;
            Some(CompDevice { d3d, dxgi, context })
        }
    }

    /// Attaches a composition swap chain to the window. `width`/`height` is
    /// the size the chain is allocated at — the caller passes the monitor size,
    /// so later window growth needs no reallocation (see [`Composition`]).
    pub fn create_composition(
        &self,
        dev: &CompDevice,
        hwnd: HWND,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Option<Composition> {
        unsafe {
            let adapter = dev.dxgi.GetAdapter().ok()?;
            let factory: IDXGIFactory2 = adapter.GetParent().ok()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width.max(1),
                Height: height.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                // Drei Puffer: einer beim DWM, einer wartet, einer wird
                // gezeichnet — so blockiert Present(0) praktisch nie.
                BufferCount: 3,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swap = factory
                .CreateSwapChainForComposition(&dev.d3d, &desc, None)
                .ok()?;
            let comp: IDCompositionDevice = DCompositionCreateDevice(&dev.dxgi).ok()?;
            // Oberfläche über den Kindfenstern.
            let target = comp.CreateTargetForHwnd(hwnd, true).ok()?;
            let visual = comp.CreateVisual().ok()?;
            visual.SetContent(&swap).ok()?;
            target.SetRoot(&visual).ok()?;

            // Grundplatte dahinter. Geht sie nicht (älteres Windows), malt die
            // Oberfläche den Inhaltsbereich selbst deckend.
            let plate = (|| -> Option<Backplate> {
                let target = comp.CreateTargetForHwnd(hwnd, false).ok()?;
                let visual = comp.CreateVisual().ok()?;
                let surface = comp
                    .CreateSurface(PLATE_PX, PLATE_PX, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_ALPHA_MODE_PREMULTIPLIED)
                    .ok()?;
                visual.SetContent(&surface).ok()?;
                let m = windows_numerics::Matrix3x2 {
                    M11: PLATE_SCALE, M12: 0.0, M21: 0.0, M22: PLATE_SCALE, M31: 0.0, M32: 0.0,
                };
                visual.SetTransform2(&m).ok()?;
                let _ = visual.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR);
                let _ = visual.SetBorderMode(DCOMPOSITION_BORDER_MODE_HARD);
                target.SetRoot(&visual).ok()?;
                Some(Backplate { _target: target, _visual: visual, surface, color: color(0, 0, 0, 0.0) })
            })();
            comp.Commit().ok()?;
            Some(Composition {
                dc: dev.context.clone(),
                swap,
                comp,
                _target: target,
                _visual: visual,
                plate,
                size: (width.max(1), height.max(1)),
                dpi: 96.0 * scale,
                lost: false,
            })
        }
    }
}

impl Composition {
    /// Sorgt dafür, dass die Kette mindestens `width`×`height` misst. Reicht
    /// sie, passiert nichts — das ist beim Ziehen am Rand der Normalfall, weil
    /// sie von vornherein Monitorgröße hat. Muss sie wachsen (größerer
    /// Monitor), dann gleich auf `hint` (Monitorgröße), nicht nur knapp.
    pub fn resize(&mut self, width: u32, height: u32, hint: (u32, u32), scale: f32) {
        let (w, h) = (width.max(1), height.max(1));
        self.dpi = 96.0 * scale;
        if w <= self.size.0 && h <= self.size.1 {
            return;
        }
        let (nw, nh) = (w.max(hint.0).max(self.size.0), h.max(hint.1).max(self.size.1));
        unsafe {
            self.dc.SetTarget(None);
            match self
                .swap
                .ResizeBuffers(0, nw, nh, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))
            {
                Ok(()) => self.size = (nw, nh),
                Err(e) if is_device_lost(e.code()) => self.lost = true,
                Err(_) => {}
            }
        }
    }

    /// Größe der Kette in DIP — so weit reicht die Malfläche.
    pub fn canvas_dip(&self) -> (f32, f32) {
        let s = self.dpi / 96.0;
        (self.size.0 as f32 / s, self.size.1 as f32 / s)
    }

    /// Gibt es die Grundplatte? Sonst muss die Oberfläche selbst grundieren.
    pub fn has_plate(&self) -> bool {
        self.plate.is_some()
    }

    /// Färbt die Grundplatte. Nur außerhalb von `begin`/`end` aufrufen.
    pub fn set_plate_color(&mut self, c: D2D1_COLOR_F) {
        let Some(plate) = self.plate.as_mut() else { return };
        let same = |a: f32, b: f32| (a - b).abs() < 0.002;
        if same(plate.color.r, c.r) && same(plate.color.g, c.g) && same(plate.color.b, c.b) && same(plate.color.a, c.a) {
            return;
        }
        unsafe {
            let mut offset = windows::Win32::Foundation::POINT::default();
            let Ok(surface) = plate.surface.BeginDraw::<IDXGISurface>(None, &mut offset) else { return };
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            if let Ok(bitmap) = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props)) {
                self.dc.SetTarget(&bitmap);
                self.dc.SetDpi(96.0, 96.0);
                self.dc.BeginDraw();
                // Die Fläche kann Teil eines Atlas sein — nur unser Stück
                // an `offset` färben, nichts daneben.
                self.dc.PushAxisAlignedClip(
                    &rect_f(offset.x as f32, offset.y as f32, PLATE_PX as f32, PLATE_PX as f32),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
                self.dc.Clear(Some(&c));
                self.dc.PopAxisAlignedClip();
                let _ = self.dc.EndDraw(None, None);
                self.dc.SetTarget(None);
            }
            let _ = plate.surface.EndDraw();
            let _ = self.comp.Commit();
        }
        plate.color = c;
    }

    /// Binds the back buffer and opens a draw batch.
    pub fn begin(&self) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: self.dpi,
                dpiY: self.dpi,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let bitmap = self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.dc.SetTarget(&bitmap);
            self.dc.SetDpi(self.dpi, self.dpi);
            self.dc.BeginDraw();
            Ok(())
        }
    }

    /// Ends the batch and pushes the frame to the compositor.
    ///
    /// `Present(0)`: nicht auf den nächsten Bildwechsel warten. Bei einer
    /// Kompositionskette entscheidet ohnehin der DWM, wann das Bild auf den
    /// Schirm kommt; ein neueres Bild ersetzt ein noch nicht gezeigtes. Mit
    /// Sync-Intervall 1 stand der UI-Thread bei jedem Bild bis zu 16 ms —
    /// bei jeder Mausbewegung, jedem Animationsschritt, jedem WM_SIZE. Das war
    /// der Grund, warum sich alles zäh anfühlte.
    ///
    /// `sync`: nach dem Abschicken warten, bis der DWM das nächste Bild
    /// zusammengesetzt hat (`DwmFlush`). Beim Ziehen am Fensterrand hält das
    /// Rahmen und Inhalt zusammen — sonst hinkt die Fläche dem Rahmen ein,
    /// zwei Bilder hinterher, und man sieht am Rand die Fläche dahinter.
    /// Dazu `DXGI_PRESENT_RESTART`: ein noch wartendes Bild in alter Größe
    /// wird verworfen statt gezeigt. (So macht es Raph Levien in
    /// „Smooth resize“, und so machen es druid und Zed.)
    pub fn end(&mut self, sync: bool) -> Frame {
        unsafe {
            let drawn = self.dc.EndDraw(None, None);
            self.dc.SetTarget(None);
            if let Err(e) = &drawn {
                if is_device_lost(e.code()) {
                    self.lost = true;
                    return Frame::DeviceLost;
                }
                return Frame::Skipped;
            }
            let flags = if sync { DXGI_PRESENT_RESTART } else { DXGI_PRESENT(0) };
            let hr = self.swap.Present(0, flags);
            if hr.is_err() && is_device_lost(hr) {
                self.lost = true;
                return Frame::DeviceLost;
            }
            if self.comp.Commit().is_err() {
                // Der Kompositionsdienst kennt unser Gerät nicht mehr.
                self.lost = true;
                return Frame::DeviceLost;
            }
            if sync {
                let _ = windows::Win32::Graphics::Dwm::DwmFlush();
            }
            Frame::Ok
        }
    }

    /// Lebt das Gerät noch? Der Kompositionsdienst meldet einen Neustart
    /// (etwa nach einem Absturz des DWM) nur so — Present liefert dann noch
    /// lange Erfolg, während auf dem Schirm nichts mehr ankommt.
    pub fn alive(&self) -> bool {
        unsafe { self.comp.CheckDeviceState().map(|b| b.as_bool()).unwrap_or(false) }
    }

    pub fn target(&self) -> ID2D1RenderTarget {
        self.dc.cast().unwrap()
    }

    /// Liest den aktuellen Rückpuffer als BGRA-Pixel aus, beschnitten auf
    /// `cw`×`ch` (das Fenster — die Kette selbst ist größer). Muss zwischen
    /// `begin()` und `end()` laufen – nach dem Present ist der Puffer beim
    /// Flip-Modell nicht mehr definiert.
    pub fn read_back(&self, cw: u32, ch: u32) -> Option<(u32, u32, Vec<u8>)> {
        unsafe {
            let back: ID3D11Texture2D = self.swap.GetBuffer(0).ok()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            back.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;

            let device: ID3D11Device = back.GetDevice().ok()?;
            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging)).ok()?;
            let staging = staging?;

            let ctx = device.GetImmediateContext().ok()?;
            ctx.CopyResource(&staging, &back);

            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).ok()?;
            let (w, h) = (cw.min(desc.Width).max(1), ch.min(desc.Height).max(1));
            let mut out = vec![0u8; (w * h * 4) as usize];
            for y in 0..h as usize {
                let src = (map.pData as *const u8).add(y * map.RowPitch as usize);
                let dst = out.as_mut_ptr().add(y * w as usize * 4);
                std::ptr::copy_nonoverlapping(src, dst, w as usize * 4);
            }
            ctx.Unmap(&staging, 0);
            Some((w, h, out))
        }
    }
}

/// Bildtakt für Animationen.
///
/// Vorher trieb ein 8-ms-Timer die Bewegung. Windows-Timer ticken aber in
/// Wirklichkeit alle 15,6 ms, und der Schirm wechselt alle 16,7 ms (oder
/// 8,3 bei 120 Hz): zwei Takte, die nichts voneinander wissen. Ergebnis war
/// ein Schwebungsmuster — mal zwei Schritte in einem Bild, mal keiner. Genau
/// das sieht man als „nicht flüssig“.
///
/// Hier wartet ein Hilfsthread auf den Takt des Fensterverwalters (`DwmFlush`
/// kehrt zurück, sobald der DWM ein Bild zusammengesetzt hat) und stößt dann
/// im UI-Thread eine Nachricht an. Ein Schritt je Bild, mit dem echten
/// Zeitabstand — die Federn in [`crate::anim`] laufen dann so glatt, wie der
/// Schirm es hergibt. Solange nichts in Bewegung ist, schläft der Thread.
pub struct FrameClock {
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    wake: windows::Win32::Foundation::HANDLE,
}

// Das Ereignis-Handle darf über Threads wandern; der Thread hält seine eigene
// Kopie und benutzt nur PostMessage/SetEvent, beide threadsicher.
unsafe impl Send for FrameClock {}

impl FrameClock {
    /// Startet den Taktgeber. `msg` wird an `hwnd` gesendet, sooft ein neues
    /// Bild fällig ist — aber nie schneller, als der UI-Thread quittiert.
    pub fn start(hwnd: HWND, msg: u32) -> FrameClock {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

        let active = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicBool::new(false));
        let wake: HANDLE = unsafe { CreateEventW(None, false, false, None) }.unwrap_or_default();
        let (a, p) = (active.clone(), pending.clone());
        let hwnd_raw = hwnd.0 as isize;
        let wake_raw = wake.0 as isize;
        std::thread::Builder::new()
            .name("aura-frames".into())
            .spawn(move || {
                let hwnd = HWND(hwnd_raw as *mut _);
                let wake = HANDLE(wake_raw as *mut _);
                loop {
                    if !a.load(Ordering::Acquire) {
                        unsafe {
                            WaitForSingleObject(wake, INFINITE);
                        }
                        continue;
                    }
                    // Auf das nächste zusammengesetzte Bild warten. Kehrt der
                    // Aufruf sofort zurück (kein DWM, Fenster verdeckt, Fehler),
                    // dann selbst takten — sonst liefe die Schleife heiß.
                    let t0 = std::time::Instant::now();
                    let ok = unsafe { windows::Win32::Graphics::Dwm::DwmFlush() }.is_ok();
                    let spent = t0.elapsed();
                    if !ok || spent < std::time::Duration::from_millis(1) {
                        std::thread::sleep(std::time::Duration::from_millis(16).saturating_sub(spent));
                    }
                    if a.load(Ordering::Acquire) && !p.swap(true, Ordering::AcqRel) {
                        unsafe {
                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                Some(hwnd),
                                msg,
                                windows::Win32::Foundation::WPARAM(0),
                                windows::Win32::Foundation::LPARAM(0),
                            );
                        }
                    }
                }
            })
            .ok();
        FrameClock { active, pending, wake }
    }

    /// Bewegung an oder aus. Beim Einschalten wird der Thread geweckt.
    pub fn set_active(&self, on: bool) {
        use std::sync::atomic::Ordering;
        let was = self.active.swap(on, Ordering::AcqRel);
        if on && !was {
            unsafe {
                let _ = windows::Win32::System::Threading::SetEvent(self.wake);
            }
        }
    }

    /// Der UI-Thread hat die Nachricht verarbeitet — das nächste Bild darf
    /// gemeldet werden.
    pub fn ack(&self) {
        self.pending.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Entpackt ein PNG in rohe BGRA-Pixel – Gegenstück zu `encode_png`.
pub fn decode_png_bgra(wic: &IWICImagingFactory, bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let decoder = wic
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).ok()?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut px).ok()?;
        Some((w, h, px))
    }
}

/// Packt BGRA-Pixel (vormultipliziert) als PNG. Der Alphakanal wird auf
/// deckend gesetzt, damit der Glaseffekt im Bild nicht als Loch erscheint.
pub fn encode_png(wic: &IWICImagingFactory, w: u32, h: u32, mut px: Vec<u8>) -> Option<Vec<u8>> {
    for p in px.chunks_exact_mut(4) {
        p[3] = 255;
    }
    unsafe {
        let mem = windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal(
            windows::Win32::Foundation::HGLOBAL(std::ptr::null_mut()),
            true,
        )
        .ok()?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
        encoder.Initialize(&mem, WICBitmapEncoderNoCache).ok()?;
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut opts: Option<windows::Win32::System::Com::StructuredStorage::IPropertyBag2> = None;
        encoder.CreateNewFrame(&mut frame, &mut opts).ok()?;
        let frame = frame?;
        frame.Initialize(opts.as_ref()).ok()?;
        frame.SetSize(w, h).ok()?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt).ok()?;
        frame.WritePixels(h, w * 4, &px).ok()?;
        frame.Commit().ok()?;
        encoder.Commit().ok()?;

        // Den Speicherstrom wieder auslesen.
        let mut stat = windows::Win32::System::Com::STATSTG::default();
        mem.Stat(&mut stat, windows::Win32::System::Com::STATFLAG(1)).ok()?;
        let len = stat.cbSize as usize;
        mem.Seek(0, windows::Win32::System::Com::STREAM_SEEK_SET, None).ok()?;
        let mut out = vec![0u8; len];
        let mut read = 0u32;
        if mem
            .Read(out.as_mut_ptr() as *mut _, len as u32, Some(&mut read))
            .is_err()
        {
            return None;
        }
        out.truncate(read as usize);
        Some(out)
    }
}

pub fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

pub fn rect_f(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

pub fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// Grows a rect by `d` on every side (negative shrinks).
pub fn inflate(r: D2D_RECT_F, d: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left - d,
        top: r.top - d,
        right: r.right + d,
        bottom: r.bottom + d,
    }
}

pub fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect,
        radiusX: radius,
        radiusY: radius,
    }
}

pub fn ellipse(cx: f32, cy: f32, r: f32) -> D2D1_ELLIPSE {
    D2D1_ELLIPSE {
        point: pt(cx, cy),
        radiusX: r,
        radiusY: r,
    }
}

pub fn brush(rt: &ID2D1RenderTarget, c: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
    unsafe { rt.CreateSolidColorBrush(&c, None) }
}
