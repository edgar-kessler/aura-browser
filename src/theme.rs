// Theme: light/dark/system, accent color, DPI-independent cosmetic values.
//
// Die Farben sind Vercels Geist-Skala, ausgelesen von vercel.com selbst.
// Geist ordnet den Stufen feste Aufgaben zu:
//
//   background-200  Fläche der Anwendung   (dunkel reines Schwarz, hell Weiß)
//   background-100  abgesetzte Fläche      (Menüs, Karten)
//   gray-100..300   Bedienelement: ruhend, berührt, gewählt
//   gray-400..600   Ränder: ruhend, berührt, gewählt
//   gray-900        Text, leise
//   gray-1000       Text, deutlich
//   blue-700        die eine kräftige Farbe
//
// Die Haltung dahinter: sehr hoher Kontrast zwischen Grund und Schrift, dünne
// ruhige Ränder, keine Farbe ohne Bedeutung. Tiefe kommt aus der Abstufung der
// Flächen, nicht aus Schatten.
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

/// Ein Satz Geist-Farben für eine Darstellung.
struct Scale {
    /// background-200 — die Fläche, auf der alles liegt.
    bg: u32,
    /// background-100 — was darüber schwebt.
    surface: u32,
    /// gray-100..300 — Bedienelement ruhend, berührt, gewählt.
    ui: [u32; 3],
    /// gray-400..600 — Rand ruhend, berührt, gewählt.
    line: [u32; 3],
    /// gray-900, gray-1000 — Text leise, Text deutlich.
    dim: u32,
    text: u32,
    danger: u32,
}

/// Geist, dunkel. Reines Schwarz als Grund — das ist Vercels Kennzeichen.
const DARK: Scale = Scale {
    bg: 0x000000,
    surface: 0x0a0a0a,
    ui: [0x1a1a1a, 0x1f1f1f, 0x292929],
    line: [0x2e2e2e, 0x454545, 0x878787],
    dim: 0xa1a1a1,
    text: 0xededed,
    danger: 0xff6166,
};

/// Geist, hell.
const LIGHT: Scale = Scale {
    bg: 0xffffff,
    surface: 0xfafafa,
    ui: [0xf2f2f2, 0xebebeb, 0xe6e6e6],
    line: [0xebebeb, 0xc9c9c9, 0xa8a8a8],
    dim: 0x4c4c4c,
    text: 0x171717,
    danger: 0xcb2a2f,
};

/// Vercel-Blau (blue-700) — die Vorgabe, wenn der Nutzer nichts anderes wählt.
pub const ACCENT_DEFAULT: (u8, u8, u8) = (0, 114, 245);

fn rgb(v: u32) -> D2D1_COLOR_F {
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
    /// Stufe 3, immer deckend. Das Adressfeld ist ein echtes Kindfenster und
    /// kann keine durchscheinende Fläche haben — `glassify` lässt diesen Wert
    /// deshalb in Ruhe.
    pub input_solid: D2D1_COLOR_F,
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
        let s = if dark { &DARK } else { &LIGHT };
        let (ar, ag, ab) = accent;
        Theme {
            dark,
            accent,
            reduce_motion,
            bg: rgb(s.bg),
            bg_top: rgb(s.bg),
            sidebar_bg: rgb(s.bg),
            input_bg: rgb(s.ui[0]),
            input_solid: rgb(s.ui[0]),
            hover: rgb(s.ui[1]),
            active: rgb(s.ui[2]),
            border: rgb(s.line[0]),
            border_strong: rgb(s.line[1]),
            text: rgb(s.text),
            text_dim: rgb(s.dim),
            accent_f: color(ar, ag, ab, 1.0),
            on_accent: if luminance(accent) > 0.58 {
                color(0, 0, 0, 1.0)
            } else {
                color(255, 255, 255, 1.0)
            },
            popup_bg: rgb(s.surface),
            danger: rgb(s.danger),
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
            self.input_bg = color(0, 0, 0, 0.05);
            self.hover = color(0, 0, 0, 0.07);
            self.active = color(0, 0, 0, 0.11);
            self.border = color(0, 0, 0, 0.12);
            self.border_strong = color(0, 0, 0, 0.18);
        }
    }

    /// Blend `hover`/`active` in at the given animation intensity (0..1).
    pub fn hover_at(&self, t: f32) -> D2D1_COLOR_F {
        let mut c = self.hover;
        c.a *= t.clamp(0.0, 1.0);
        c
    }

    /// Der Druck auf ein Bedienelement: die Stufe „gewählt“, über die
    /// Berührung gelegt. Deutlich sichtbar, aber dieselbe Familie — kein
    /// Farbwechsel, nur eine Stufe tiefer.
    pub fn press_at(&self, t: f32) -> D2D1_COLOR_F {
        let mut c = self.active;
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
