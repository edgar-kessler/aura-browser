// Ein selbstgezeichnetes Textfeld.
//
// Warum nicht das EDIT-Fenster von Windows: das Hauptfenster trägt
// `WS_EX_NOREDIRECTIONBITMAP`, weil die Oberfläche über eine eigene
// Kompositionsfläche läuft. Damit hat das Fenster keine Umleitungsfläche mehr,
// in die ein GDI-Kindfenster zeichnen könnte — ein EDIT-Feld erscheint dort
// schlicht nie. Genau das war der Grund, warum die Adresse beim Tippen
// unsichtbar blieb, obwohl der Text vorhanden war.
//
// Also führen wir den Text selbst: Einfügemarke, Auswahl, die üblichen Tasten,
// Zwischenablage, Ziehen mit der Maus. Alles in Zeichen (nicht Bytes) gerechnet,
// damit Umlaute und Emoji nicht zerfallen.
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

use crate::gfx::{brush, rect_f};
use crate::theme::Theme;

pub struct TextField {
    chars: Vec<char>,
    /// Einfügemarke, als Zeichenindex.
    pub caret: usize,
    /// Ankerpunkt der Auswahl; gleich `caret`, wenn nichts ausgewählt ist.
    pub anchor: usize,
    /// Waagerechte Verschiebung in DIP, wenn der Text breiter ist als das Feld.
    scroll: f32,
    pub focused: bool,
    /// Zeitpunkt der letzten Änderung — die Marke blinkt erst danach wieder.
    pub blink_from: u64,
}

impl TextField {
    pub fn new() -> TextField {
        TextField {
            chars: Vec::new(),
            caret: 0,
            anchor: 0,
            scroll: 0.0,
            focused: false,
            blink_from: 0,
        }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn set_text(&mut self, text: &str, now: u64) {
        self.chars = text.chars().collect();
        self.caret = self.chars.len();
        self.anchor = self.caret;
        self.scroll = 0.0;
        self.blink_from = now;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.chars.len();
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection();
        self.chars[a..b].iter().collect()
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.chars.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }

    pub fn insert(&mut self, text: &str, now: u64) {
        self.delete_selection();
        for c in text.chars().filter(|c| !c.is_control()) {
            self.chars.insert(self.caret, c);
            self.caret += 1;
        }
        self.anchor = self.caret;
        self.blink_from = now;
    }

    /// Wortgrenze links bzw. rechts der Marke — für Strg+Pfeil und Strg+Rück.
    fn word_edge(&self, from: usize, right: bool) -> usize {
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '%';
        let mut i = from;
        if right {
            while i < self.chars.len() && !is_word(self.chars[i]) {
                i += 1;
            }
            while i < self.chars.len() && is_word(self.chars[i]) {
                i += 1;
            }
        } else {
            while i > 0 && !is_word(self.chars[i - 1]) {
                i -= 1;
            }
            while i > 0 && is_word(self.chars[i - 1]) {
                i -= 1;
            }
        }
        i
    }

    /// Verarbeitet eine Taste. Gibt zurück, ob sich der Text geändert hat.
    pub fn key(&mut self, vk: u32, ctrl: bool, shift: bool, now: u64) -> KeyResult {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let vk = VIRTUAL_KEY(vk as u16);
        let len = self.chars.len();
        let mut changed = false;
        self.blink_from = now;
        match vk {
            VK_LEFT | VK_RIGHT => {
                let right = vk == VK_RIGHT;
                let (a, b) = self.selection();
                self.caret = if ctrl {
                    self.word_edge(self.caret, right)
                } else if !shift && a != b {
                    // Ohne Umschalt springt die Marke an den Rand der Auswahl.
                    if right { b } else { a }
                } else if right {
                    (self.caret + 1).min(len)
                } else {
                    self.caret.saturating_sub(1)
                };
                if !shift {
                    self.anchor = self.caret;
                }
            }
            VK_HOME | VK_END => {
                self.caret = if vk == VK_END { len } else { 0 };
                if !shift {
                    self.anchor = self.caret;
                }
            }
            VK_BACK => {
                if !self.delete_selection() && self.caret > 0 {
                    let to = if ctrl { self.word_edge(self.caret, false) } else { self.caret - 1 };
                    self.chars.drain(to..self.caret);
                    self.caret = to;
                    self.anchor = to;
                }
                changed = true;
            }
            VK_DELETE => {
                if !self.delete_selection() && self.caret < len {
                    let to = if ctrl { self.word_edge(self.caret, true) } else { self.caret + 1 };
                    self.chars.drain(self.caret..to);
                }
                changed = true;
            }
            VK_A if ctrl => self.select_all(),
            VK_C | VK_X if ctrl => {
                let sel = self.selected_text();
                if !sel.is_empty() {
                    copy_to_clipboard(&sel);
                    if vk == VK_X {
                        self.delete_selection();
                        changed = true;
                    }
                }
            }
            VK_V if ctrl => {
                if let Some(text) = clipboard_text() {
                    // Mehrzeiliges aus der Zwischenablage wird zu einer Zeile.
                    let flat: String = text.replace(['\r', '\n'], " ");
                    self.insert(flat.trim(), now);
                    changed = true;
                }
            }
            _ => return KeyResult { handled: false, changed: false },
        }
        KeyResult { handled: true, changed }
    }

    /// Ein Zeichen von WM_CHAR. Steuerzeichen werden verworfen.
    pub fn char_input(&mut self, c: char, now: u64) -> bool {
        if c.is_control() {
            return false;
        }
        self.insert(&c.to_string(), now);
        true
    }

    // ---------------- Darstellung ----------------

    fn layout(
        &self,
        dwrite: &IDWriteFactory,
        fmt: &IDWriteTextFormat,
        width: f32,
        text: &[u16],
    ) -> Option<IDWriteTextLayout> {
        unsafe { dwrite.CreateTextLayout(text, fmt, width.max(1.0), 64.0).ok() }
    }

    /// Waagerechte Lage eines Zeichenindex im Text.
    fn caret_x(&self, layout: &IDWriteTextLayout, index_utf16: u32) -> f32 {
        unsafe {
            let (mut x, mut y) = (0.0f32, 0.0f32);
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            let _ = layout.HitTestTextPosition(index_utf16, false, &mut x, &mut y, &mut m);
            x
        }
    }

    /// Zeichenindex an einer Bildschirmlage — fürs Klicken und Ziehen.
    pub fn index_at(
        &self,
        dwrite: &IDWriteFactory,
        fmt: &IDWriteTextFormat,
        r: D2D_RECT_F,
        x: f32,
    ) -> usize {
        let text: Vec<u16> = self.text().encode_utf16().collect();
        let Some(layout) = self.layout(dwrite, fmt, r.right - r.left, &text) else {
            return self.caret;
        };
        unsafe {
            let mut trailing = windows::core::BOOL(0);
            let mut inside = windows::core::BOOL(0);
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            let _ = layout.HitTestPoint(
                x - r.left + self.scroll,
                (r.bottom - r.top) / 2.0,
                &mut trailing,
                &mut inside,
                &mut m,
            );
            let mut idx = m.textPosition as usize;
            if trailing.as_bool() {
                idx += m.length as usize;
            }
            utf16_to_char_index(&self.chars, idx)
        }
    }

    /// Zeichnet Auswahl, Text und Einfügemarke in `r`.
    pub fn paint(
        &mut self,
        rt: &ID2D1RenderTarget,
        dwrite: &IDWriteFactory,
        fmt: &IDWriteTextFormat,
        r: D2D_RECT_F,
        theme: &Theme,
        now: u64,
    ) {
        let text: Vec<u16> = self.text().encode_utf16().collect();
        let width = r.right - r.left;
        let Some(layout) = self.layout(dwrite, fmt, f32::MAX / 4.0, &text) else { return };

        let caret16 = char_to_utf16_index(&self.chars, self.caret) as u32;
        let caret_x = self.caret_x(&layout, caret16);

        // Mitlaufen lassen, damit die Marke immer im Feld bleibt.
        if caret_x - self.scroll > width - 2.0 {
            self.scroll = caret_x - width + 2.0;
        }
        if caret_x - self.scroll < 0.0 {
            self.scroll = caret_x;
        }
        let mut total = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut total);
        }
        // Kein Leerraum rechts, wenn der Text kürzer geworden ist.
        self.scroll = self.scroll.clamp(0.0, (total.width - width).max(0.0));

        unsafe {
            rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            let ox = r.left - self.scroll;
            let oy = r.top + (r.bottom - r.top - total.height) / 2.0;

            // Auswahl
            let (a, b) = self.selection();
            if a != b && self.focused {
                let a16 = char_to_utf16_index(&self.chars, a) as u32;
                let b16 = char_to_utf16_index(&self.chars, b) as u32;
                let (x0, x1) = (self.caret_x(&layout, a16), self.caret_x(&layout, b16));
                let mut c = theme.accent_f;
                c.a = 0.30;
                if let Ok(br) = brush(rt, c) {
                    rt.FillRectangle(
                        &rect_f(ox + x0, r.top + 2.0, x1 - x0, r.bottom - r.top - 4.0),
                        &br,
                    );
                }
            }

            if let Ok(br) = brush(rt, theme.text) {
                rt.DrawTextLayout(
                    crate::gfx::pt(ox, oy),
                    &layout,
                    &br,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }

            // Einfügemarke: blinkt im Sekundentakt, steht nach jeder Eingabe
            // erst einmal still.
            if self.focused {
                let since = now.saturating_sub(self.blink_from);
                if since < 500 || (since / 500) % 2 == 0 {
                    if let Ok(br) = brush(rt, theme.text) {
                        rt.FillRectangle(
                            &rect_f(ox + caret_x.round(), r.top + 2.0, 1.0, r.bottom - r.top - 4.0),
                            &br,
                        );
                    }
                }
            }
            rt.PopAxisAlignedClip();
        }
    }
}

pub struct KeyResult {
    pub handled: bool,
    pub changed: bool,
}

fn char_to_utf16_index(chars: &[char], idx: usize) -> usize {
    chars[..idx.min(chars.len())].iter().map(|c| c.len_utf16()).sum()
}

fn utf16_to_char_index(chars: &[char], idx16: usize) -> usize {
    let mut n = 0;
    for (i, c) in chars.iter().enumerate() {
        if n >= idx16 {
            return i;
        }
        n += c.len_utf16();
    }
    chars.len()
}

pub fn copy_to_clipboard(text: &str) {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        let bytes = wide.len() * 2;
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes) {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(h);
                let _ = SetClipboardData(13u32, Some(windows::Win32::Foundation::HANDLE(h.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

pub fn clipboard_text() -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let data = GetClipboardData(13u32).ok();
        let out = data.and_then(|h| {
            let p = GlobalLock(windows::Win32::Foundation::HGLOBAL(h.0)) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(h.0));
            Some(s)
        });
        let _ = CloseClipboard();
        out
    }
}

/// Damit ungenutzte Einbindungen nicht stören.
#[allow(dead_code)]
fn _unused(_: HWND, _: RECT, _: PCWSTR) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(s: &str) -> TextField {
        let mut f = TextField::new();
        f.set_text(s, 0);
        f
    }

    #[test]
    fn typing_and_backspace() {
        let mut f = field("");
        f.insert("abc", 0);
        assert_eq!(f.text(), "abc");
        f.key(0x08, false, false, 0); // Rück
        assert_eq!(f.text(), "ab");
    }

    #[test]
    fn select_all_then_type_replaces() {
        let mut f = field("https://example.com");
        f.select_all();
        f.insert("x", 0);
        assert_eq!(f.text(), "x");
    }

    #[test]
    fn umlauts_survive_backspace() {
        let mut f = field("Größe");
        f.key(0x08, false, false, 0);
        assert_eq!(f.text(), "Größ");
    }

    #[test]
    fn word_jump_skips_whole_word() {
        let mut f = field("https://example.com/pfad");
        f.caret = f.chars.len();
        f.key(0x25, true, false, 0); // Strg+Links
        assert_eq!(f.caret, "https://example.com/".chars().count());
    }

    #[test]
    fn home_end_and_shift_select() {
        let mut f = field("abcdef");
        f.key(0x24, false, false, 0); // Pos1
        assert_eq!(f.caret, 0);
        f.key(0x23, false, true, 0); // Umschalt+Ende
        assert_eq!(f.selected_text(), "abcdef");
    }
}
