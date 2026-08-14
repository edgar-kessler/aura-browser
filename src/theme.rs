// Theme: light/dark/system, accent color, DPI-independent cosmetic values.
use crate::gfx::color;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[derive(Clone, Copy, PartialEq)]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

/// Corner radii (DIP), HeroUI-inspired: soft, generous, consistent.
pub const R_XS: f32 = 8.0;
pub const R_SM: f32 = 11.0;
pub const R_MD: f32 = 14.0;
pub const R_LG: f32 = 18.0;

#[derive(Clone)]
pub struct Theme {
    pub dark: bool,
    pub accent: (u8, u8, u8),
    pub reduce_motion: bool,

    pub bg: D2D1_COLOR_F,
    pub bg_top: D2D1_COLOR_F,
    pub sidebar_bg: D2D1_COLOR_F,
    pub input_bg: D2D1_COLOR_F,
    pub hover: D2D1_COLOR_F,
    pub active: D2D1_COLOR_F,
    pub border: D2D1_COLOR_F,
    pub text: D2D1_COLOR_F,
    pub text_dim: D2D1_COLOR_F,
    pub accent_f: D2D1_COLOR_F,
    pub accent_soft: D2D1_COLOR_F,
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
        let (ar, ag, ab) = accent;
        if dark {
            Theme {
                dark,
                accent,
                reduce_motion,
                bg: color(9, 9, 11, 1.0),          // zinc-950
                bg_top: color(9, 9, 11, 1.0),
                sidebar_bg: color(9, 9, 11, 1.0),
                input_bg: color(255, 255, 255, 0.06),
                hover: color(255, 255, 255, 0.08),
                active: color(255, 255, 255, 0.13),
                border: color(255, 255, 255, 0.10),
                text: color(236, 237, 238, 1.0),
                text_dim: color(150, 150, 162, 1.0),
                accent_f: color(ar, ag, ab, 1.0),
                accent_soft: color(ar, ag, ab, 0.18),
                popup_bg: color(24, 24, 27, 1.0),  // zinc-900
                danger: color(243, 18, 96, 1.0),
            }
        } else {
            Theme {
                dark,
                accent,
                reduce_motion,
                bg: color(255, 255, 255, 1.0),
                bg_top: color(255, 255, 255, 1.0),
                sidebar_bg: color(250, 250, 250, 1.0),
                input_bg: color(24, 24, 27, 0.05),
                hover: color(24, 24, 27, 0.055),
                active: color(24, 24, 27, 0.10),
                border: color(24, 24, 27, 0.09),
                text: color(17, 24, 28, 1.0),
                text_dim: color(113, 113, 122, 1.0),
                accent_f: color(ar, ag, ab, 1.0),
                accent_soft: color(ar, ag, ab, 0.14),
                popup_bg: color(255, 255, 255, 1.0),
                danger: color(241, 70, 104, 1.0),
            }
        }
    }

    /// Makes the chrome translucent so the system backdrop (Mica/Acrylic) reads
    /// through it. Only applied when the composition surface is available.
    pub fn glassify(&mut self) {
        if self.dark {
            self.bg = color(9, 9, 11, 0.55);
            self.bg_top = color(16, 16, 20, 0.55);
            self.sidebar_bg = color(9, 9, 11, 0.32);
            self.input_bg = color(255, 255, 255, 0.09);
            self.border = color(255, 255, 255, 0.12);
        } else {
            self.bg = color(255, 255, 255, 0.55);
            self.bg_top = color(255, 255, 255, 0.55);
            self.sidebar_bg = color(250, 250, 250, 0.32);
            self.input_bg = color(24, 24, 27, 0.06);
            self.border = color(24, 24, 27, 0.12);
        }
    }

    /// Blend `hover`/`active` in at the given animation intensity (0..1).
    pub fn hover_at(&self, t: f32) -> D2D1_COLOR_F {
        let mut c = self.hover;
        c.a *= t.clamp(0.0, 1.0);
        c
    }


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
