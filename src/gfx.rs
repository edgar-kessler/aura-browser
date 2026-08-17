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
pub struct Composition {
    pub dc: ID2D1DeviceContext,
    swap: IDXGISwapChain1,
    comp: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    size: (u32, u32),
    dpi: f32,
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

    /// Attaches a composition swap chain to the window.
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
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swap = factory
                .CreateSwapChainForComposition(&dev.d3d, &desc, None)
                .ok()?;
            let comp: IDCompositionDevice = DCompositionCreateDevice(&dev.dxgi).ok()?;
            let target = comp.CreateTargetForHwnd(hwnd, true).ok()?;
            let visual = comp.CreateVisual().ok()?;
            visual.SetContent(&swap).ok()?;
            target.SetRoot(&visual).ok()?;
            comp.Commit().ok()?;
            Some(Composition {
                dc: dev.context.clone(),
                swap,
                comp,
                _target: target,
                _visual: visual,
                size: (width.max(1), height.max(1)),
                dpi: 96.0 * scale,
            })
        }
    }
}

impl Composition {
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        let (w, h) = (width.max(1), height.max(1));
        self.dpi = 96.0 * scale;
        if self.size == (w, h) {
            return;
        }
        unsafe {
            self.dc.SetTarget(None);
            if self
                .swap
                .ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))
                .is_ok()
            {
                self.size = (w, h);
            }
        }
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
    pub fn end(&self) -> bool {
        unsafe {
            let ok = self.dc.EndDraw(None, None).is_ok();
            self.dc.SetTarget(None);
            let _ = self.swap.Present(1, DXGI_PRESENT(0));
            let _ = self.comp.Commit();
            ok
        }
    }

    pub fn target(&self) -> ID2D1RenderTarget {
        self.dc.cast().unwrap()
    }

    /// Liest den aktuellen Rückpuffer als BGRA-Pixel aus. Muss zwischen
    /// `begin()` und `end()` laufen – nach dem Present ist der Puffer beim
    /// Flip-Modell nicht mehr definiert.
    pub fn read_back(&self) -> Option<(u32, u32, Vec<u8>)> {
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
            let (w, h) = (desc.Width, desc.Height);
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
