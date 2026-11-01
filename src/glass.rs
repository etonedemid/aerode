//! Wallpaper handling and the Aero glass renderer.
//!
//! Glass surfaces paint the blurred wallpaper that lies behind them, then
//! tint + gloss.  That works with or without a compositor.
use gtk::cairo;
use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
use gtk::glib;
use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

use crate::{config, util};

pub struct Wallpaper {
    pub sharp: Pixbuf,
    pub blurred: Pixbuf,
}

thread_local! {
    static WP: RefCell<Option<Rc<Wallpaper>>> = RefCell::new(None);
    static LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = RefCell::new(Vec::new());
}

pub fn wallpaper() -> Rc<Wallpaper> {
    if let Some(w) = WP.with(|w| w.borrow().clone()) {
        return w;
    }
    reload();
    WP.with(|w| w.borrow().clone().unwrap())
}

pub fn on_change(f: impl Fn() + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

pub fn wallpaper_path() -> Option<String> {
    config::get()
        .wallpaper
        .filter(|p| std::path::Path::new(p).exists())
        .or_else(|| config::data_file("wallpaper.jpg").map(|p| p.to_string_lossy().into_owned()))
}

pub fn reload() {
    let g = util::primary_geometry();
    let (w, h) = (g.width(), g.height());
    let mode = config::get().wallpaper_mode;
    let sharp = wallpaper_path()
        .and_then(|p| Pixbuf::from_file(p).ok())
        .map(|p| fit(&p, w, h, &mode))
        .unwrap_or_else(|| generated(w, h));
    let blurred = blur(&sharp, 6);
    WP.with(|x| *x.borrow_mut() = Some(Rc::new(Wallpaper { sharp, blurred })));
    LISTENERS.with(|l| l.borrow().iter().for_each(|f| f()));
}

fn fit(p: &Pixbuf, w: i32, h: i32, mode: &str) -> Pixbuf {
    let (iw, ih) = (p.width() as f64, p.height() as f64);
    let surf = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h).unwrap();
    {
        let cr = cairo::Context::new(&surf).unwrap();
        cr.set_source_rgb(0.0, 0.18, 0.4);
        cr.paint().ok();
        let (sx, sy) = match mode {
            "stretch" => (w as f64 / iw, h as f64 / ih),
            "center" => (1.0, 1.0),
            "fit" => {
                let s = (w as f64 / iw).min(h as f64 / ih);
                (s, s)
            }
            _ => {
                let s = (w as f64 / iw).max(h as f64 / ih);
                (s, s)
            }
        };
        let scaled = p
            .scale_simple((iw * sx).round().max(1.0) as i32, (ih * sy).round().max(1.0) as i32, InterpType::Hyper)
            .unwrap();
        cr.set_source_pixbuf(&scaled, (w - scaled.width()) as f64 / 2.0, (h - scaled.height()) as f64 / 2.0);
        cr.paint().ok();
    }
    gtk::gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h).unwrap()
}

/// Cheap blur: downscale ×8, 3× box blur, upscale.
pub fn blur(p: &Pixbuf, radius: usize) -> Pixbuf {
    let (w, h) = (p.width(), p.height());
    let sw = (w / 8).max(1);
    let sh = (h / 8).max(1);
    let small = p.add_alpha(false, 0, 0, 0).unwrap().scale_simple(sw, sh, InterpType::Bilinear).unwrap();
    let rs = small.rowstride() as usize;
    let bytes = small.read_pixel_bytes();
    let (sw, sh) = (sw as usize, sh as usize);
    let mut buf = vec![0f32; sw * sh * 4];
    for y in 0..sh {
        for x in 0..sw {
            for c in 0..4 {
                buf[(y * sw + x) * 4 + c] = bytes[y * rs + x * 4 + c] as f32;
            }
        }
    }
    let mut tmp = buf.clone();
    for _ in 0..3 {
        box_pass(&buf, &mut tmp, sw, sh, radius, true);
        box_pass(&tmp, &mut buf, sw, sh, radius, false);
    }
    let out: Vec<u8> = buf.iter().map(|v| v.clamp(0.0, 255.0) as u8).collect();
    let pb = Pixbuf::from_bytes(&glib::Bytes::from_owned(out), Colorspace::Rgb, true, 8, sw as i32, sh as i32, sw as i32 * 4);
    pb.scale_simple(w, h, InterpType::Bilinear).unwrap()
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horiz: bool) {
    let (len, lines) = if horiz { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horiz { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        for c in 0..4 {
            let mut acc = 0.0;
            for k in 0..=2 * r {
                let i = (k as isize - r as isize).clamp(0, len as isize - 1) as usize;
                acc += src[idx(line, i) + c];
            }
            for i in 0..len {
                dst[idx(line, i) + c] = acc * norm;
                let add = (i + r + 1).min(len - 1);
                let sub = (i as isize - r as isize).max(0) as usize;
                acc += src[idx(line, add) + c] - src[idx(line, sub) + c];
            }
        }
    }
}

/// Original blue Aero-style wallpaper used when nothing was extracted.
pub fn generated(w: i32, h: i32) -> Pixbuf {
    let surf = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h).unwrap();
    let cr = cairo::Context::new(&surf).unwrap();
    let (fw, fh) = (w as f64, h as f64);
    let g = cairo::LinearGradient::new(0.0, 0.0, 0.0, fh);
    g.add_color_stop_rgb(0.0, 0.02, 0.16, 0.36);
    g.add_color_stop_rgb(0.45, 0.08, 0.38, 0.66);
    g.add_color_stop_rgb(1.0, 0.01, 0.10, 0.24);
    cr.set_source(&g).ok();
    cr.paint().ok();
    let r = cairo::RadialGradient::new(fw * 0.55, fh * 0.42, 0.0, fw * 0.55, fh * 0.42, fw * 0.45);
    r.add_color_stop_rgba(0.0, 0.55, 0.88, 1.0, 0.75);
    r.add_color_stop_rgba(0.35, 0.25, 0.62, 0.92, 0.35);
    r.add_color_stop_rgba(1.0, 0.0, 0.2, 0.5, 0.0);
    cr.set_source(&r).ok();
    cr.paint().ok();
    // light ribbons
    let mut seed = 7u32;
    let mut rnd = move || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        ((seed >> 8) & 0xffff) as f64 / 65535.0
    };
    for i in 0..10 {
        let off = (rnd() - 0.5) * 0.3;
        cr.move_to(-20.0, fh * (0.78 + off));
        cr.curve_to(fw * 0.3, fh * (0.55 + off + rnd() * 0.1), fw * 0.6, fh * (0.75 + off - rnd() * 0.2),
                    fw + 20.0, fh * (0.35 + off + i as f64 * 0.01));
        cr.set_line_width(2.0 + rnd() * 9.0);
        cr.set_source_rgba(0.7, 0.92, 1.0, 0.06 + rnd() * 0.16);
        cr.stroke().ok();
    }
    for _ in 0..30 {
        let (x, y, rr) = (rnd() * fw, fh * (0.2 + rnd() * 0.6), 6.0 + rnd() * 30.0);
        let b = cairo::RadialGradient::new(x, y, 0.0, x, y, rr);
        b.add_color_stop_rgba(0.0, 0.85, 0.96, 1.0, 0.10 + rnd() * 0.15);
        b.add_color_stop_rgba(1.0, 0.85, 0.96, 1.0, 0.0);
        cr.set_source(&b).ok();
        cr.arc(x, y, rr, 0.0, 2.0 * PI);
        cr.fill().ok();
    }
    drop(cr);
    gtk::gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h).unwrap()
}

pub fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    if r <= 0.0 {
        cr.rectangle(x, y, w, h);
        return;
    }
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// Paint an Aero glass panel of size w×h whose top-left is at screen (sx, sy).
pub fn paint_glass(cr: &cairo::Context, w: f64, h: f64, sx: f64, sy: f64, radius: f64, border: bool) {
    let wp = wallpaper();
    let t = config::get().tint;
    cr.save().ok();
    rounded(cr, 0.0, 0.0, w, h, radius);
    cr.clip();
    cr.set_source_pixbuf(&wp.blurred, -sx, -sy);
    cr.paint().ok();
    cr.set_source_rgba(t[0], t[1], t[2], t[3]);
    cr.paint().ok();
    // Aero diagonal streaks
    let mut x = -h;
    let mut i = 0;
    while x < w {
        let a = if i % 2 == 0 { 0.035 } else { 0.02 };
        let g = cairo::LinearGradient::new(x, 0.0, x + h * 0.6, h);
        g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.0);
        g.add_color_stop_rgba(0.5, 1.0, 1.0, 1.0, a);
        g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
        cr.set_source(&g).ok();
        cr.move_to(x, 0.0);
        cr.line_to(x + 30.0, 0.0);
        cr.line_to(x + 30.0 + h * 0.6, h);
        cr.line_to(x + h * 0.6, h);
        cr.close_path();
        cr.fill().ok();
        x += 70.0;
        i += 1;
    }
    let g = cairo::LinearGradient::new(0.0, 0.0, 0.0, h.min(80.0));
    g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.22);
    g.add_color_stop_rgba(0.49, 1.0, 1.0, 1.0, 0.06);
    g.add_color_stop_rgba(0.5, 1.0, 1.0, 1.0, 0.0);
    g.add_color_stop_rgba(1.0, 0.0, 0.0, 0.0, 0.08);
    cr.set_source(&g).ok();
    cr.paint().ok();
    cr.restore().ok();
    if border {
        cr.set_line_width(1.0);
        rounded(cr, 0.5, 0.5, w - 1.0, h - 1.0, radius);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
        cr.stroke().ok();
        rounded(cr, 1.5, 1.5, w - 3.0, h - 3.0, (radius - 1.0).max(0.0));
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.32);
        cr.stroke().ok();
    }
}

thread_local! {
    static ORB: Option<Pixbuf> = config::data_file("orb.png").and_then(|p| Pixbuf::from_file(p).ok());
}

/// Draw the start orb (state 0 normal, 1 hover, 2 pressed) centred in a size×size box.
pub fn paint_orb(cr: &cairo::Context, size: f64, state: i32) {
    if let Some(strip) = ORB.with(|o| o.clone()) {
        let fh = strip.height() / 3;
        if let Some(frame) = Pixbuf::new_subpixbuf(&strip, 0, fh * state, strip.width(), fh).into() {
            let s = size / strip.width().max(fh) as f64;
            cr.save().ok();
            cr.scale(s, s);
            cr.set_source_pixbuf(&frame, 0.0, 0.0);
            cr.paint().ok();
            cr.restore().ok();
            return;
        }
    }
    let c = size / 2.0;
    let r = size * 0.40;
    if state > 0 {
        let g = cairo::RadialGradient::new(c, c, r * 0.6, c, c, size / 2.0);
        let a = if state == 1 { 0.55 } else { 0.3 };
        g.add_color_stop_rgba(0.0, 0.5, 0.9, 1.0, a);
        g.add_color_stop_rgba(1.0, 0.5, 0.9, 1.0, 0.0);
        cr.set_source(&g).ok();
        cr.arc(c, c, size / 2.0, 0.0, 2.0 * PI);
        cr.fill().ok();
    }
    let d = if state == 2 { 0.75 } else { 1.0 };
    let g = cairo::RadialGradient::new(c, c + r * 0.6, r * 0.1, c, c, r);
    g.add_color_stop_rgb(0.0, 0.35 * d, 0.78 * d, 0.98 * d);
    g.add_color_stop_rgb(0.7, 0.05 * d, 0.32 * d, 0.64 * d);
    g.add_color_stop_rgb(1.0, 0.02, 0.12, 0.3);
    cr.set_source(&g).ok();
    cr.arc(c, c, r, 0.0, 2.0 * PI);
    cr.fill_preserve().ok();
    cr.set_source_rgba(0.0, 0.05, 0.15, 0.9);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    let g = cairo::LinearGradient::new(0.0, c - r, 0.0, c);
    g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.75);
    g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.05);
    cr.set_source(&g).ok();
    cr.save().ok();
    cr.translate(c, c - r * 0.42);
    cr.scale(r * 0.78, r * 0.5);
    cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
    cr.restore().ok();
    cr.fill().ok();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    let s = r * 0.55;
    cr.move_to(c, c - s);
    for k in 1..8 {
        let a = k as f64 * PI / 4.0 - PI / 2.0;
        let rr = if k % 2 == 0 { s } else { s * 0.22 };
        cr.line_to(c + rr * a.cos(), c + rr * a.sin());
    }
    cr.close_path();
    cr.fill().ok();
}

/// Rounded rectangle with radius `r` on top corners and `rb` on the bottom ones.
pub fn rounded_top(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64, rb: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r.max(0.01), y + r.max(0.01), r.max(0.01), -PI / 2.0, 0.0);
    cr.arc(x + w - rb.max(0.01), y + h - rb.max(0.01), rb.max(0.01), 0.0, PI / 2.0);
    cr.arc(x + rb.max(0.01), y + h - rb.max(0.01), rb.max(0.01), PI / 2.0, PI);
    cr.arc(x + r.max(0.01), y + r.max(0.01), r.max(0.01), PI, 1.5 * PI);
    cr.close_path();
}

thread_local! {
    static THEME: Option<std::rc::Rc<crate::msstyles::Theme>> =
        crate::msstyles::Theme::open(&config::data_dir().join("theme/aero.msstyles")).ok().map(std::rc::Rc::new);
}

/// The user's extracted aero.msstyles, if present.
pub fn theme() -> Option<std::rc::Rc<crate::msstyles::Theme>> { THEME.with(|t| t.clone()) }

/// An IMAGE resource from aero.msstyles as a pixbuf.
pub fn theme_pixbuf(id: u16) -> Option<Pixbuf> {
    let img = theme()?.image_res(id)?;
    let (w, h) = (img.width() as i32, img.height() as i32);
    Some(Pixbuf::from_bytes(&glib::Bytes::from_owned(img.into_raw()), Colorspace::Rgb, true, 8, w, h, w * 4))
}

/// Win7 flyout chrome: glass border, white body, footer band (Flyout part 7 image).
pub fn paint_flyout(cr: &cairo::Context, w: f64, h: f64, sx: f64, sy: f64, foot_h: f64) {
    const B: f64 = 7.0;
    paint_glass(cr, w, h, sx, sy, 6.0, true);
    // white content panel with a 1px dark edge, like the Win7 flyout
    cr.rectangle(B - 0.5, B - 0.5, w - 2.0 * B + 1.0, h - 2.0 * B + 1.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    cr.rectangle(B, B, w - 2.0 * B, h - 2.0 * B);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.fill().ok();
    if foot_h > 0.0 {
        let fy = h - B - foot_h;
        if let Some(band) = theme_pixbuf(780).and_then(|p| p.scale_simple((w - 2.0 * B) as i32, foot_h as i32, InterpType::Bilinear)) {
            cr.set_source_pixbuf(&band, B, fy);
            cr.paint().ok();
        } else {
            cr.rectangle(B, fy, w - 2.0 * B, foot_h);
            cr.set_source_rgb(0.945, 0.961, 0.984);
            cr.fill().ok();
            cr.rectangle(B, fy, w - 2.0 * B, 1.0);
            cr.set_source_rgb(0.85, 0.88, 0.92);
            cr.fill().ok();
        }
    }
}
