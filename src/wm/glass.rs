//! Window-frame glass in the WM: the same Windows 7 DWM formula the shell paints (`dwmcolor`),
//! following the live colourisation the Window Color page publishes on the root window
//! (`_AERODE_COLORIZATION`, see `crate::glass`).
//!
//! With a compositor the frame gets the translucent colour picom needs (`picom_fit`; picom blurs
//! what is behind, frames are tagged `_AERODE_GLASS`); without one the glass is faked from the
//! wallpaper blurred the Win7 way; with "Enable transparency" off it is a solid colour.
use cairo::Context;
use std::cell::RefCell;

use crate::dwmcolor::{Colorization, ShaderConsts, ThemeGlass};

pub struct Glass {
    pub colz: Colorization,
    pub theme: ThemeGlass,
    screen: (i32, i32),
    /// the wallpaper at screen size after Win7's sigma-3 blur (w, h, rgb); built on first use
    backdrop: RefCell<Option<(i32, i32, Vec<u8>)>>,
    /// backdrops glass usually covers, for the picom fit: the downscaled wallpaper, white, grey, black
    sample: Vec<[f64; 3]>,
    fits: RefCell<Vec<(ShaderConsts, ([f64; 3], f64))>>,
}

fn wallpaper_image() -> Option<image::DynamicImage> {
    crate::glass::wallpaper_path().and_then(|p| image::open(p).ok())
}

impl Glass {
    /// Call `set_wallpaper` once the screen size is known.
    pub fn new(colz: Colorization, theme: ThemeGlass) -> Glass {
        Glass { colz, theme, screen: (0, 0), backdrop: RefCell::new(None), sample: vec![], fits: RefCell::new(vec![]) }
    }

    /// The wallpaper or the screen size changed.
    pub fn set_wallpaper(&mut self, screen: (i32, i32)) {
        self.screen = screen;
        self.backdrop.replace(None);
        self.fits.borrow_mut().clear();
        self.sample = vec![[1.0, 1.0, 1.0], [0.94, 0.94, 0.94], [0.0, 0.0, 0.0]];
        if let Some(img) = wallpaper_image() {
            let small = img.resize_exact(32, 18, image::imageops::FilterType::Triangle).to_rgb8();
            self.sample.extend(small.pixels().map(|p| p.0.map(|c| c as f64 / 255.0)));
        }
    }

    pub fn set_colorization(&mut self, c: Colorization) -> bool {
        let changed = self.colz != c;
        self.colz = c;
        changed
    }

    pub fn consts(&self, active: bool, maximized: bool) -> ShaderConsts {
        self.colz.realize(active, maximized, &self.theme)
    }

    /// (premultiplied colour, alpha) to paint so that picom's `S + (1-a) * blur(B)` matches Win7.
    fn fit(&self, k: &ShaderConsts) -> ([f64; 3], f64) {
        if let Some(r) = self.fits.borrow().iter().find(|(kk, _)| kk == k).map(|(_, r)| *r) { return r; }
        let r = k.picom_fit(&self.sample);
        let mut f = self.fits.borrow_mut();
        if f.len() >= 8 { f.remove(0); }
        f.push((*k, r));
        r
    }

    fn with_backdrop<R>(&self, f: impl FnOnce(i32, i32, &[u8]) -> R) -> R {
        if self.backdrop.borrow().is_none() {
            let (sw, sh) = (self.screen.0.max(1), self.screen.1.max(1));
            let rgb = match wallpaper_image() {
                Some(img) => img.resize_to_fill(sw as u32, sh as u32, image::imageops::FilterType::Triangle).to_rgb8().into_raw(),
                // Win7's default background blue
                None => [0x1c, 0x6e, 0xc8].repeat((sw * sh) as usize),
            };
            let blurred = crate::glass::win7_blur(&rgb, sw as usize, sh as usize);
            self.backdrop.replace(Some((sw, sh, blurred)));
        }
        let b = self.backdrop.borrow();
        let (w, h, rgb) = b.as_ref().unwrap();
        f(*w, *h, rgb)
    }

    /// Fill the current clip with glass for a surface whose top-left is at screen (sx, sy), in the
    /// given window state; replaces whatever is under the clip. Returns the reflection opacity.
    pub fn fill(&self, cr: &Context, sx: f64, sy: f64, active: bool, maximized: bool, composited: bool) -> f64 {
        let k = self.consts(active, maximized);
        cr.save().ok();
        cr.set_operator(cairo::Operator::Source);
        if k.solid {
            cr.set_source_rgb(k.color[0], k.color[1], k.color[2]);
            cr.paint().ok();
        } else if composited {
            let (s, a) = self.fit(&k);
            if a > 0.0 { cr.set_source_rgba(s[0] / a, s[1] / a, s[2] / a, a); } else { cr.set_source_rgba(0.0, 0.0, 0.0, 0.0); }
            cr.paint().ok();
        } else {
            self.fake(cr, &k, sx, sy);
        }
        cr.restore().ok();
        k.reflection_opacity
    }

    /// No compositor: colourise the blurred wallpaper behind the clip.
    fn fake(&self, cr: &Context, k: &ShaderConsts, sx: f64, sy: f64) {
        let Ok((cx0, cy0, cx1, cy1)) = cr.clip_extents() else { return };
        let (x0, y0, x1, y1) = (cx0.floor() as i32, cy0.floor() as i32, cx1.ceil() as i32, cy1.ceil() as i32);
        if x1 <= x0 || y1 <= y0 { return; }
        let Ok(mut surf) = cairo::ImageSurface::create(cairo::Format::Rgb24, x1 - x0, y1 - y0) else { return };
        let stride = surf.stride() as usize;
        {
            let Ok(mut d) = surf.data() else { return };
            let bb = k.blur_balance as f32;
            let ag = k.afterglow.map(|v| v as f32);
            let col = k.color.map(|v| v as f32 * 255.0);
            let (ox, oy) = (sx.round() as i32 + x0, sy.round() as i32 + y0);
            self.with_backdrop(|bw, bh, rgb| {
                for y in 0..y1 - y0 {
                    let by = (oy + y).clamp(0, bh - 1) as usize;
                    let line = &mut d[y as usize * stride..];
                    for x in 0..x1 - x0 {
                        let bx = (ox + x).clamp(0, bw - 1) as usize;
                        let o = (by * bw as usize + bx) * 3;
                        let px = [rgb[o] as f32, rgb[o + 1] as f32, rgb[o + 2] as f32];
                        let lum = px[0] * 0.2126 + px[1] * 0.7152 + px[2] * 0.0722;
                        let c = |i: usize| (bb * px[i] + lum * ag[i] + col[i] + 0.5).clamp(0.0, 255.0) as u8;
                        let q = x as usize * 4;
                        line[q..q + 4].copy_from_slice(&[c(2), c(1), c(0), 255]);
                    }
                }
            });
        }
        cr.set_source_surface(&surf, x0 as f64, y0 as f64).ok();
        cr.paint().ok();
    }
}
