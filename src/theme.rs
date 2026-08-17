// Theme: light/dark/system, accent color, DPI-independent cosmetic values.
//
// Die Farben stehen nicht einzeln nebeneinander, sondern kommen aus einer
// zwölfstufigen Neutralskala mit festen Rollen — das Modell von Radix Colors,
// das auch shadcn und Geist benutzen:
//
//   1  Fläche der Anwendung        7  Rand
//   2  abgesetzte Fläche           8  starker Rand, Fokusring
//   3  Bedienelement               9  Vollton (Akzent)
//   4  Bedienelement berührt      10  Vollton berührt
//   5  Bedienelement gewählt      11  Text, leise
//   6  Trennlinie                 12  Text, deutlich
//
// Dazu die Haltung von Stripe: eine einzige kräftige Farbe, die sich ihren
// Auftritt verdient, Tiefe aus Flächenabstufung statt aus Schatten, und
// Buchstabenabstand, der mit der Schriftgröße enger wird.
use crate::gfx::color;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[derive(Clone, Copy, PartialEq)]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

/// Eckenradien (DIP), dieselbe Reihe wie im Stylesheet der internen Seiten.
pub const R_XS: f32 = 4.0;
pub const R_SM: f32 = 6.0;
pub const R_MD: f32 = 8.0;
pub const R_LG: f32 = 12.0;

/// Radix „slate“, hell — Stufe 1 bis 12.
const SLATE: [u32; 12] = [
    0xfcfcfd, 0xf9f9fb, 0xf0f0f3, 0xe8e8ec, 0xe0e1e6, 0xd9d9e0, 0xcdced6, 0xb9bbc6, 0x8b8d98,
    0x80838d, 0x60646c, 0x1c2024,
];
/// Radix „slate dark“ — Stufe 1 bis 12.
const SLATE_DARK: [u32; 12] = [
    0x111113, 0x18191b, 0x212225, 0x272a2d, 0x2e3135, 0x363a3f, 0x43484e, 0x5a6169, 0x696e77,
    0x777b84, 0xb0b4ba, 0xedeef0,
];

fn step(scale: &[u32; 12], n: usize) -> D2D1_COLOR_F {
    let v = scale[n - 1];
    color(((v >> 16) & 255) as u8, ((v >> 8) & 255) as u8, (v & 255) as u8, 1.0)
}

#[derive(Clone)]
pub struct Theme {
    pub dark: bool,
    pub accent: (u8, u8, u8),
    pub reduce_motion: bool,

    /// Stufe 1 — Fläche des Fensters.
    pub bg: D2D1_COLOR_F,
    /// Stufe 1, eigener Name, damit die Kopfleiste sich später absetzen darf.
    pub bg_top: D2D1_COLOR_F,
    /// Stufe 1 — die Leiste teilt sich die Fläche, getrennt nur durch die Linie.
    pub sidebar_bg: D2D1_COLOR_F,
    /// Stufe 3 — Eingabefelder, ruhende Bedienelemente.
    pub input_bg: D2D1_COLOR_F,
    /// Stufe 4 — berührt.
    pub hover: D2D1_COLOR_F,
    /// Stufe 5 — gewählt.
    pub active: D2D1_COLOR_F,
    /// Stufe 6 — Trennlinien.
    pub border: D2D1_COLOR_F,
    /// Stufe 7 — Rand um Bedienelemente.
    pub border_strong: D2D1_COLOR_F,
    /// Stufe 12 — Text.
    pub text: D2D1_COLOR_F,
    /// Stufe 11 — Text, leise.
    pub text_dim: D2D1_COLOR_F,
    /// Vollton des Akzents.
    pub accent_f: D2D1_COLOR_F,
    /// Der Akzent als Fläche, sehr zurückhaltend.
    pub accent_soft: D2D1_COLOR_F,
    /// Schrift auf dem Vollton — je nach dessen Helligkeit hell oder dunkel.
    pub on_accent: D2D1_COLOR_F,
    /// Stufe 2 — Menüs und Sprechblasen heben sich um eine Stufe ab.
    pub popup_bg: D2D1_COLOR_F,
    pub danger: D2D1_COLOR_F,
}

impl Theme {
    pub fn new(mode: ThemeMode, accent: (u8, u8, u8), reduce_motion: bool) -> Theme {
        let dark = match mode {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => system_dark(),
        };
        let s = if dark { &SLATE_DARK } else { &SLATE };
        let (ar, ag, ab) = accent;
        Theme {
            dark,
            accent,
            reduce_motion,
            bg: step(s, 1),
            bg_top: step(s, 1),
            sidebar_bg: step(s, 1),
            input_bg: step(s, 3),
            hover: step(s, 4),
            active: step(s, 5),
            border: step(s, 6),
            border_strong: step(s, 7),
            text: step(s, 12),
            text_dim: step(s, 11),
            accent_f: color(ar, ag, ab, 1.0),
            accent_soft: color(ar, ag, ab, if dark { 0.18 } else { 0.13 }),
            on_accent: if luminance(accent) > 0.58 {
                color(9, 9, 11, 1.0)
            } else {
                color(255, 255, 255, 1.0)
            },
            popup_bg: step(s, 2),
            danger: if dark { color(255, 99, 105, 1.0) } else { color(206, 44, 49, 1.0) },
        }
    }

    /// Makes the chrome translucent so the system backdrop (Mica/Acrylic) reads
    /// through it. Only applied when the composition surface is available.
    pub fn glassify(&mut self) {
        // Die Flächen werden durchlässig, die Linien dafür eine Spur kräftiger —
        // sonst verschwinden sie im Hintergrund.
        let a = if self.dark { 0.62 } else { 0.66 };
        self.bg.a = a;
        self.bg_top.a = a;
        self.sidebar_bg.a = a * 0.78;
        self.popup_bg.a = 0.94;
        if self.dark {
            self.input_bg = color(255, 255, 255, 0.07);
            self.hover = color(255, 255, 255, 0.09);
            self.active = color(255, 255, 255, 0.13);
            self.border = color(255, 255, 255, 0.12);
            self.border_strong = color(255, 255, 255, 0.18);
        } else {
            self.input_bg = color(28, 32, 36, 0.05);
            self.hover = color(28, 32, 36, 0.07);
            self.active = color(28, 32, 36, 0.11);
            self.border = color(28, 32, 36, 0.12);
            self.border_strong = color(28, 32, 36, 0.18);
        }
    }

    /// Blend `hover`/`active` in at the given animation intensity (0..1).
    pub fn hover_at(&self, t: f32) -> D2D1_COLOR_F {
        let mut c = self.hover;
        c.a *= t.clamp(0.0, 1.0);
        c
    }

    /// Der Fokusring: der Akzent, deutlich sichtbar aber nicht grell.
    pub fn ring(&self) -> D2D1_COLOR_F {
        let (r, g, b) = self.accent;
        color(r, g, b, 0.45)
    }
}

/// Wahrgenommene Helligkeit (0..1), um Schrift auf dem Vollton zu wählen.
fn luminance((r, g, b): (u8, u8, u8)) -> f32 {
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

/// Windows setting "Transparenzeffekte". When it is off, DWM draws no Mica or
/// Acrylic — a translucent window would then show the desktop straight through,
/// so the glass look has to stay off as well.
pub fn system_transparency() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            None,
            KEY_READ,
            &mut key,
        )
        .is_err()
        {
            return true;
        }
        let mut value: u32 = 1;
        let mut size = 4u32;
        let r = RegQueryValueExW(
            key,
            w!("EnableTransparency"),
            None,
            None,
            Some(&mut value as *mut u32 as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        !r.is_ok() || value == 1
    }
}

fn system_dark() -> bool {
    // Reads AppsUseLightTheme from the registry (HKCU\...\Personalize).
    use windows::core::w;
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            None,
            KEY_READ,
            &mut key,
        )
        .is_err()
        {
            return false;
        }
        let mut value: u32 = 1;
        let mut size = 4u32;
        let r = RegQueryValueExW(
            key,
            w!("AppsUseLightTheme"),
            None,
            None,
            Some(&mut value as *mut u32 as *mut u8),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        r.is_ok() && value == 0
    }
}
