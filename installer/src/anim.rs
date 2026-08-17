// Bewegung – derselbe gedaempfte Schwinger wie in src/anim.rs des Browsers.
// Bildratenunabhaengig und je Zeitschritt geschlossen geloest, deshalb bleibt
// er auch nach einem Aussetzer (Fenster verdeckt, Ordnerdialog offen) ruhig.

#[derive(Clone, Copy)]
pub struct Spring {
    pub value: f32,
    pub target: f32,
    pub vel: f32,
    omega: f32,
    zeta: f32,
}

impl Spring {
    /// `response` ist die Dauer einer Schwingung in Sekunden, `damping` unter
    /// 1 laesst es nachfedern.
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

    pub fn jump_to(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.vel = 0.0;
    }

    pub fn at_rest(&self) -> bool {
        (self.value - self.target).abs() <= 0.001 && self.vel.abs() <= 0.008
    }

    pub fn step(&mut self, dt: f32) {
        if dt <= 0.0 || self.at_rest() {
            self.value = self.target;
            self.vel = 0.0;
            return;
        }
        let dt = dt.min(0.1);
        let (w, z) = (self.omega, self.zeta);
        let x = self.value - self.target;
        let v = self.vel;
        if z < 1.0 {
            let wd = w * (1.0 - z * z).sqrt();
            let e = (-z * w * dt).exp();
            let (s, c) = (wd * dt).sin_cos();
            let a = x;
            let b = (v + z * w * x) / wd;
            self.value = self.target + e * (a * c + b * s);
            self.vel = e * ((b * wd - z * w * a) * c - (a * wd + z * w * b) * s);
        } else {
            let e = (-w * dt).exp();
            let a = x;
            let b = v + w * x;
            self.value = self.target + e * (a + b * dt);
            self.vel = e * (b - w * (a + b * dt));
        }
        if self.at_rest() {
            self.value = self.target;
            self.vel = 0.0;
        }
    }
}

/// Nachlaufen mit Zeitkonstante `tau` – fuer Auf- und Abblenden.
pub fn approach(value: f32, target: f32, dt: f32, tau: f32) -> f32 {
    if tau <= 0.0 {
        return target;
    }
    let k = 1.0 - (-dt / tau).exp();
    let v = value + (target - value) * k;
    if (v - target).abs() < 0.002 {
        target
    } else {
        v
    }
}
