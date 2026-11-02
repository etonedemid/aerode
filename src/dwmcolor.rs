//! Windows 7 DWM colourisation maths (port of win7_glass.py; drop-in candidate for src/dwmcolor.rs).
//!
//! * `Colorization::from_cpl`  = themecpl.dll!CColorCplPage::SetDwmColorizationColor
//! * `Colorization::realize`   = uDWM!CGlassColorizationParameters::AdjustWindowColorization
//! * `ShaderConsts::glass`     = the Win7 glass pixel formula (CPU path / tests)
//! * `ShaderConsts::picom_fit` = best `S + (1-a)*B` imitation for picom's fixed blend
//! * `hsb_to_rgb`              = themecpl.dll!CColorizationColor (colour mixer)
//! Sources: github.com/ALTaleX531/OpenGlass, github.com/ALTaleX531/dwm_colorization_calculator.

pub const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

/// Personalize > Window Color swatches (themecpl.dll inventory). Alpha = the swatch's intensity.
pub const SWATCHES: [(&str, u32); 16] = [
    ("Sky", 0x6B74B8FC), ("Twilight", 0xA80046AD), ("Sea", 0x8032CDCD), ("Leaf", 0x6614A600),
    ("Lime", 0x6697D937), ("Sun", 0x54FADC0E), ("Pumpkin", 0x80FF9C00), ("Ruby", 0xA8CE0F0F),
    ("Fuchsia", 0x66FF0099), ("Blush", 0x70FCC7F8), ("Violet", 0x856E3BA1), ("Lavender", 0x528D5A94),
    ("Taupe", 0x6698844C), ("Chocolate", 0xA84F1B1B), ("Slate", 0x80555555), ("Frost", 0x54FCFCFC),
];

fn rgb01(argb: u32) -> [f64; 3] {
    [((argb >> 16) & 0xFF) as f64 / 255.0, ((argb >> 8) & 0xFF) as f64 / 255.0, (argb & 0xFF) as f64 / 255.0]
}
fn alpha01(argb: u32) -> f64 { (argb >> 24) as f64 / 255.0 }

/// What Win7 keeps under HKCU\Software\Microsoft\Windows\DWM (and what config.json should keep).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colorization {
    pub color: u32,
    pub afterglow: u32,
    pub color_balance: u32,
    pub afterglow_balance: u32,
    pub blur_balance: u32,
    pub reflection_intensity: u32,
    /// ColorizationOpaqueBlend = !"Enable transparency"
    pub opaque: bool,
}

impl Default for Colorization {
    fn default() -> Self { Colorization::from_cpl(0x6B74B8FC, false) } // Sky: 8 / 43 / 49
}

/// Values the aero.msstyles DWMWindow class supplies (OpenGlass "Auto" defaults for Win7 glass).
/// Read the real ones from the user's msstyles: TOPFRAME (part 56) states 1/2 prop 2432
/// COLORIZATIONOPACITY, part 0 prop 2431 COLORIZATIONCOLOR, REFLECTIONMAP (40) prop 2430 OPACITY.
#[derive(Clone, Copy, Debug)]
pub struct ThemeGlass {
    pub opacity: [f64; 4],            // [active, inactive, active maximized, inactive maximized]
    pub reflection: [f64; 4],         // same order
    pub base_transparent: u32,
    pub base_maximized: u32,
    pub base_opaque: u32,
}

impl Default for ThemeGlass {
    fn default() -> Self {
        ThemeGlass {
            opacity: [1.0, 0.4, 1.0, 0.4],
            reflection: [0.9, 0.75, 0.9, 0.75],
            base_transparent: 0x0000_0000,
            base_maximized: 0x0000_0000,
            base_opaque: 0xFFDF_DFDF,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShaderConsts {
    pub color: [f64; 3],      // u_color     (already x colour balance)
    pub afterglow: [f64; 3],  // u_afterglow (already x afterglow balance)
    pub blur_balance: f64,    // u_blur_balance
    pub solid: bool,          // u_solid: transparency off -> paint `color`, no blur
    pub reflection_opacity: f64,
}

impl Colorization {
    /// ARGB from a swatch, the intensity slider (alpha) or the colour mixer (rgb).
    pub fn from_cpl(argb: u32, opaque: bool) -> Colorization {
        let bal = ((alpha01(argb) - 0.1) / 0.75 * 100.0 + 10.0) as i32; // trunc like the original
        let (c, a, b) = if opaque {
            (bal - 10, 10, 100 - bal)
        } else if bal < 50 {
            (5, bal - 5, 100 - bal)
        } else if bal >= 95 {
            (bal - 25, 0, 100 - (bal - 25))
        } else {
            let a = 95 - bal;
            let b = 50 - ((bal - 50) >> 1);
            (100 - a - b, a, b)
        };
        let cl = |v: i32| v.clamp(0, 100) as u32;
        Colorization { color: argb, afterglow: argb, color_balance: cl(c), afterglow_balance: cl(a),
                       blur_balance: cl(b), reflection_intensity: 50, opaque }
    }

    /// Shader constants for one window state.
    pub fn realize(&self, active: bool, maximized: bool, theme: &ThemeGlass) -> ShaderConsts {
        let base = if self.opaque { theme.base_opaque } else if maximized { theme.base_maximized } else { theme.base_transparent };
        let (ba, brgb) = (alpha01(base), rgb01(base));
        let i = match (active, maximized) { (true, false) => 0, (false, false) => 1, (true, true) => 2, (false, true) => 3 };
        let opac = theme.opacity[i];
        let ab = self.afterglow_balance as f64 / 100.0 * (1.0 - ba);
        let mut bb = self.blur_balance as f64 / 100.0 * (1.0 - ba);
        let cb = self.color_balance as f64 / 100.0 * opac;
        let src = rgb01(self.color);
        let ca = (1.0 - cb) * ba + cb;
        let crgb: [f64; 3] = std::array::from_fn(|k| if ca > 0.0 { ((1.0 - cb) * brgb[k] * ba + src[k] * cb) / ca } else { src[k] });
        bb = if self.opaque { 0.0 } else { 1.0 - (1.0 - bb) * opac };
        let ag = rgb01(self.afterglow);
        ShaderConsts {
            color: std::array::from_fn(|k| crgb[k] * ca),
            afterglow: std::array::from_fn(|k| ag[k] * ab),
            blur_balance: bb,
            solid: bb == 0.0 && ab == 0.0,
            reflection_opacity: (theme.reflection[i] * self.reflection_intensity as f64 / 100.0 / 0.5).clamp(0.0, 1.0),
        }
    }
}

impl ShaderConsts {
    /// One glass pixel from an (already Win7-blurred) backdrop pixel, sRGB-encoded 0..1.
    pub fn glass(&self, b: [f64; 3]) -> [f64; 3] {
        if self.solid { return self.color; }
        let lum = b[0] * LUMA[0] + b[1] * LUMA[1] + b[2] * LUMA[2];
        std::array::from_fn(|k| (self.blur_balance * b[k] + lum * self.afterglow[k] + self.color[k]).clamp(0.0, 1.0))
    }

    /// picom can only do `S + (1-a) * blur(B)` (premultiplied over). Least-squares (S, a) over
    /// `sample` (downscaled blurred wallpaper + white/#F0F0F0/black); S is kept a valid
    /// premultiplied colour (0 <= S <= a) so cairo can paint it. Returns ([S], a).
    pub fn picom_fit(&self, sample: &[[f64; 3]]) -> ([f64; 3], f64) {
        if self.solid { return (self.color, 1.0); }
        let n = sample.len().max(1) as f64;
        let t: Vec<[f64; 3]> = sample.iter().map(|b| self.glass(*b)).collect();
        let mut best = (f64::MAX, [0.0; 3], 0.0);
        for step in 0..=100 {
            let keep = step as f64 / 100.0;
            let a = 1.0 - keep;
            let mut s = [0.0; 3];
            for (b, tt) in sample.iter().zip(&t) { for k in 0..3 { s[k] += (tt[k] - keep * b[k]) / n; } }
            for v in &mut s { *v = v.clamp(0.0, a); }
            let mut e = 0.0;
            for (b, tt) in sample.iter().zip(&t) { for k in 0..3 { e += ((s[k] + keep * b[k]).clamp(0.0, 1.0) - tt[k]).abs(); } }
            if e < best.0 { best = (e, s, a); }
        }
        (best.1, best.2)
    }
}

/// Colour intensity slider (10..=85 %) -> alpha byte.
pub fn intensity_to_alpha(percent: u32) -> u32 { (percent.clamp(10, 85) * 255 + 50) / 100 }

/// Colour mixer: hue 0..360, saturation 0..1, brightness 0..1 -> rgb 0..1 (themecpl variant).
pub fn hsb_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    if s == 0.0 { return [v; 3]; }
    let h = if h <= 0.0 { 360.0 } else { h };
    let f = h / 60.0 - (h / 60.0).trunc();
    match h {
        h if h <= 60.0 => [v, (1.0 - (1.0 - f) * s) * v, (1.0 - s) * v],
        h if h <= 120.0 => [(1.0 - f * s) * v, v, (1.0 - s) * v],
        h if h <= 180.0 => [(1.0 - s) * v, v, (1.0 - (1.0 - f) * s) * v],
        h if h <= 240.0 => [(1.0 - s) * v, (1.0 - f * s) * v, v],
        h if h <= 300.0 => [(1.0 - (1.0 - f) * s) * v, (1.0 - s) * v, v],
        _ => [v, (1.0 - s) * v, (1.0 - f * s) * v],
    }
}

/// Win7 blur taps (sigma 3): 15 gaussian taps folded into 8 bilinear (weight, offset) pairs.
pub fn dwm_blur_taps() -> ([f32; 8], [f32; 8]) {
    let sigma = 3.0f64;
    let r = (sigma * 2.3).ceil() as i32; // 7
    let n = 2 * r + 1;
    let w = |i: i32| if i < n { (-((i - r) * (i - r)) as f64 / (2.0 * sigma * sigma)).exp() } else { 0.0 };
    let mut ws = [0.0f64; 8];
    let mut os = [0.0f64; 8];
    for hw in 0..8 {
        let (w0, w1) = (w(2 * hw), w(2 * hw + 1));
        ws[hw as usize] = w0 + w1;
        os[hw as usize] = (2 * hw - r) as f64 + if w0 + w1 > 0.0 { w1 / (w0 + w1) } else { 0.0 };
    }
    let tot: f64 = ws.iter().sum();
    (ws.map(|x| (x / tot) as f32), os.map(|x| x as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_balances_match_win7() {
        let s = Colorization::from_cpl(0x6B74B8FC, false);
        assert_eq!((s.color_balance, s.afterglow_balance, s.blur_balance), (8, 43, 49));
        let o = Colorization::from_cpl(0x6B74B8FC, true);
        assert_eq!((o.color_balance, o.afterglow_balance, o.blur_balance), (42, 10, 48));
    }

    #[test]
    fn swatch_table_matches_python_reference() {
        // (color, afterglow, blur) balances printed by win7_glass.py
        let want = [(8, 43, 49), (56, 11, 33), (24, 32, 44), (5, 45, 50), (5, 45, 50), (5, 35, 60), (24, 32, 44), (56, 11, 33),
                    (5, 45, 50), (12, 40, 48), (29, 29, 42), (5, 34, 61), (5, 45, 50), (56, 11, 33), (24, 32, 44), (5, 35, 60)];
        for ((_, argb), w) in SWATCHES.iter().zip(want) {
            let c = Colorization::from_cpl(*argb, false);
            assert_eq!((c.color_balance, c.afterglow_balance, c.blur_balance), w, "{argb:08X}");
        }
    }

    #[test]
    fn sky_active_inactive_constants() {
        let t = ThemeGlass::default();
        let a = Colorization::default().realize(true, false, &t);
        assert!((a.blur_balance - 0.49).abs() < 1e-9);
        assert!((a.color[2] - 0.08 * 252.0 / 255.0).abs() < 1e-9);
        let i = Colorization::default().realize(false, false, &t);
        assert!((i.blur_balance - (1.0 - 0.51 * 0.4)).abs() < 1e-9);       // 0.796
        assert!((i.afterglow[1] - 0.43 * 184.0 / 255.0).abs() < 1e-9);       // afterglow unchanged
        // the screenshot check from REPORT.md: inactive Sky over (9,108,199) -> about (29,121,206)
        let px = i.glass([9.0 / 255.0, 108.0 / 255.0, 199.0 / 255.0]).map(|v| (v * 255.0).round());
        assert_eq!(px, [29.0, 121.0, 206.0]);
    }

    #[test]
    fn opaque_is_solid_mix_with_dfdfdf() {
        let o = Colorization::from_cpl(0x6B74B8FC, true).realize(true, false, &ThemeGlass::default());
        assert!(o.solid);
        let px = o.color.map(|v| (v * 255.0).round());
        assert_eq!(px, [178.0, 207.0, 235.0]);
    }

    #[test]
    fn blur_taps() {
        let (w, o) = dwm_blur_taps();
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((o[0] + 6.3269).abs() < 1e-3 && (o[7] - 7.0).abs() < 1e-6);
    }

    #[test]
    fn picom_fit_is_valid_premultiplied() {
        let sample = [[1.0, 1.0, 1.0], [0.94, 0.94, 0.94], [0.0, 0.0, 0.0], [0.05, 0.45, 0.8]];
        let k = Colorization::default().realize(true, false, &ThemeGlass::default());
        let (s, a) = k.picom_fit(&sample);
        assert!(a > 0.0 && a < 1.0);
        assert!(s.iter().all(|v| *v >= 0.0 && *v <= a));
    }
}
