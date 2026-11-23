//! Farbraum-Hilfen: sRGB <-> linear <-> Oklab.
//! Oklab ist wahrnehmungsnah, daher liefern Mittelwerte und Abstände dort
//! deutlich bessere Ergebnisse als in RGB.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

impl Lab {
    #[inline]
    pub fn dist2(&self, o: &Lab) -> f32 {
        let dl = self.l - o.l;
        let da = self.a - o.a;
        let db = self.b - o.b;
        dl * dl + da * da + db * db
    }

    #[inline]
    pub fn add(&mut self, o: &Lab) {
        self.l += o.l;
        self.a += o.a;
        self.b += o.b;
    }

    #[inline]
    pub fn scale(&self, f: f32) -> Lab {
        Lab { l: self.l * f, a: self.a * f, b: self.b * f }
    }
}

fn lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })
}

#[inline]
fn lin_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

#[inline]
pub fn rgb_to_lab(r: u8, g: u8, b: u8) -> Lab {
    let t = lut();
    let (r, g, b) = (t[r as usize], t[g as usize], t[b as usize]);
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    Lab {
        l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    }
}

#[inline]
pub fn lab_to_rgb(c: Lab) -> [u8; 3] {
    let l = c.l + 0.396_337_78 * c.a + 0.215_803_76 * c.b;
    let m = c.l - 0.105_561_346 * c.a - 0.063_854_17 * c.b;
    let s = c.l - 0.089_484_18 * c.a - 1.291_485_5 * c.b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        lin_to_srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        lin_to_srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        lin_to_srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    ]
}
