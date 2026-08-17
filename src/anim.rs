// Bewegung.
//
// Vorher lag die Bewegung verstreut im Zeichencode: mal eine Zeitkurve, mal
// `wert += (ziel - wert) * dt / 110`. Das Zweite hängt an der Bildrate — bei
// 30 Bildern läuft es doppelt so langsam wie bei 60 — und es lässt sich nicht
// unterbrechen, ohne dass die Bewegung springt.
//
// Hier stehen zwei Bausteine, beide bildratenunabhängig:
//
// * `Spring` — ein gedämpfter Schwinger. Er kennt seine Geschwindigkeit, also
//   darf sich das Ziel mitten in der Bewegung ändern; er biegt weich ab statt
//   neu anzusetzen. Das ist der Unterschied, den man bei einer Leiste sieht,
//   die man auf- und gleich wieder zuklappt.
// * `approach` — einfaches Nachlaufen mit Zeitkonstante, für alles, was nur
//   auf- und abblenden soll.
//
// Der Schwinger wird nicht schrittweise angenähert, sondern je Zeitschritt
// geschlossen gelöst. Damit bleibt er auch bei einem Aussetzer von 200 ms
// ruhig — eine schrittweise Näherung würde dort aufschwingen.
//
// Das Muster (Steifigkeit, Dämpfung, Masse) ist dasselbe wie in den
// Rust-Bibliotheken `animate` und `animato`; hier ausgeschrieben, weil es
// dreißig Zeilen sind und keine Abhängigkeit rechtfertigt.

/// Ein gedämpfter Schwinger mit Ziel und Geschwindigkeit.
#[derive(Clone, Copy)]
pub struct Spring {
    pub value: f32,
    pub target: f32,
    pub vel: f32,
    /// Eigenfrequenz in Radiant je Sekunde. Größer = schneller.
    omega: f32,
    /// Dämpfungsmaß. 1 = kriecht gerade eben ohne Überschwingen heran,
    /// darunter schwingt es sichtbar nach.
    zeta: f32,
}

impl Spring {
    /// `response` ist die Dauer einer Schwingung in Sekunden — anschaulicher
    /// als eine Steifigkeit. `damping` unter 1 lässt es nachfedern.
    pub fn new(value: f32, response: f32, damping: f32) -> Spring {
        Spring {
            value,
            target: value,
            vel: 0.0,
            omega: std::f32::consts::TAU / response.max(0.0001),
            zeta: damping.max(0.0),
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Setzt Wert und Ziel gleich, ohne Bewegung.
    pub fn jump_to(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.vel = 0.0;
    }

    pub fn at_rest(&self, eps: f32) -> bool {
        (self.value - self.target).abs() <= eps && self.vel.abs() <= eps * 8.0
    }

    /// Ein Zeitschritt in Sekunden. Geschlossene Lösung, deshalb bei jeder
    /// Schrittweite stabil.
    pub fn step(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let dt = dt.min(0.1); // ein langer Aussetzer soll nicht katapultieren
        let (w, z) = (self.omega, self.zeta);
        let x = self.value - self.target; // Auslenkung
        let v = self.vel;

        if z < 1.0 {
            // Unterdämpft: schwingt nach.
            let wd = w * (1.0 - z * z).sqrt();
            let e = (-z * w * dt).exp();
            let (s, c) = (wd * dt).sin_cos();
            let a = x;
            let b = (v + z * w * x) / wd;
            self.value = self.target + e * (a * c + b * s);
            self.vel = e * ((b * wd - z * w * a) * c - (a * wd + z * w * b) * s);
        } else {
            // Kritisch oder überdämpft: kriecht heran.
            let e = (-w * dt).exp();
            let a = x;
            let b = v + w * x;
            self.value = self.target + e * (a + b * dt);
            self.vel = e * (b - w * (a + b * dt));
        }

        if self.at_rest(0.01) {
            self.value = self.target;
            self.vel = 0.0;
        }
    }
}

/// Nachlaufen mit Zeitkonstante `tau` (Sekunden bis auf 1/e herangekommen).
/// Anders als `wert += (ziel - wert) * k` hängt das Ergebnis nicht an der
/// Bildrate.
pub fn approach(value: f32, target: f32, dt: f32, tau: f32) -> f32 {
    if tau <= 0.0 {
        return target;
    }
    let k = 1.0 - (-dt / tau).exp();
    value + (target - value) * k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_reaches_target() {
        let mut s = Spring::new(0.0, 0.3, 1.0);
        s.set_target(100.0);
        for _ in 0..200 {
            s.step(1.0 / 60.0);
        }
        assert!((s.value - 100.0).abs() < 0.5, "steht bei {}", s.value);
    }

    #[test]
    fn spring_is_frame_rate_independent() {
        // Derselbe Zeitraum in groben und in feinen Schritten muss praktisch
        // dasselbe ergeben — genau das leistet die alte Formel nicht.
        let mut fast = Spring::new(0.0, 0.4, 0.9);
        let mut slow = Spring::new(0.0, 0.4, 0.9);
        fast.set_target(1.0);
        slow.set_target(1.0);
        for _ in 0..60 {
            fast.step(1.0 / 120.0);
        }
        for _ in 0..30 {
            slow.step(1.0 / 60.0);
        }
        assert!((fast.value - slow.value).abs() < 0.01, "{} vs {}", fast.value, slow.value);
    }

    #[test]
    fn spring_survives_a_long_stall() {
        let mut s = Spring::new(0.0, 0.3, 1.0);
        s.set_target(10.0);
        s.step(2.0); // Fenster war zwei Sekunden verdeckt
        assert!(s.value.is_finite() && s.value <= 10.5, "steht bei {}", s.value);
    }

    #[test]
    fn approach_is_frame_rate_independent() {
        let mut a = 0.0;
        for _ in 0..120 {
            a = approach(a, 1.0, 1.0 / 120.0, 0.1);
        }
        let mut b = 0.0;
        for _ in 0..60 {
            b = approach(b, 1.0, 1.0 / 60.0, 0.1);
        }
        assert!((a - b).abs() < 0.005, "{a} vs {b}");
    }
}
