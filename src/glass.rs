//! Wallpaper handling and the Aero glass renderer.
//!
//! Glass is Windows 7's DWM formula (`dwmcolor`): with a compositor (picom) the surface paints the
//! translucent colour from `picom_fit` and picom blurs what is behind it; without one the glass is
//! faked from the wallpaper (Win7's sigma-3 blur, then colourised); with "Enable transparency" off
//! it is a solid colour. The DWM reflection streaks (DWMWindow part 40) go on top.
//!
//! The colourisation is live: whoever previews or saves Personalize > Window Color writes the root
//! property `_AERODE_COLORIZATION` (CARDINAL[7]: color, afterglow, color balance, afterglow balance,
//! blur balance, reflection intensity, flags with bit 0 = opaque), and every process that paints
//! glass watches it (`colorization`, `on_color_change`, `enable_rgba`).
use gtk::cairo;
use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;

use crate::dwmcolor::{Colorization, ShaderConsts, ThemeGlass};
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
    // the glass maths depends on the wallpaper (fake glass, picom fit)
    BACKDROP.with(|b| *b.borrow_mut() = None);
    FITS.with(|f| f.borrow_mut().clear());
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

/// True when a compositor is running: glass surfaces then paint translucent
/// pixels and let the compositor (picom) blur what is behind them.
pub fn composited() -> bool {
    gtk::gdk::Screen::default().map(|s| s.is_composited()).unwrap_or(false)
}

// ------------------------------------------------------------------ live colourisation

/// Root property every glass painter watches (CARDINAL[7], see the module docs).
pub const COLORIZATION_PROP: &str = "_AERODE_COLORIZATION";

/// The `_AERODE_COLORIZATION` value of a colourisation.
pub fn to_prop(c: &Colorization) -> [u32; 7] {
    [c.color, c.afterglow, c.color_balance, c.afterglow_balance, c.blur_balance, c.reflection_intensity, c.opaque as u32]
}

pub fn from_prop(v: &[u32]) -> Option<Colorization> {
    (v.len() >= 7).then(|| Colorization {
        color: v[0], afterglow: v[1], color_balance: v[2].min(100), afterglow_balance: v[3].min(100),
        blur_balance: v[4].min(100), reflection_intensity: v[5].min(100), opaque: v[6] & 1 != 0,
    })
}

/// Our own X connection, only for the root property (its events never mix with GDK's).
struct Watch {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    atom: u32,
}

thread_local! {
    static WATCH: RefCell<Option<Rc<Watch>>> = const { RefCell::new(None) };
    static WATCH_TRIED: Cell<bool> = const { Cell::new(false) };
    static CURRENT: Cell<Option<Colorization>> = const { Cell::new(None) };
    static CHANGE_PENDING: Cell<bool> = const { Cell::new(false) };
    static COLOR_LISTENERS: RefCell<Vec<Box<dyn Fn()>>> = RefCell::new(Vec::new());
    /// glass toplevels: re-tagged and redrawn when the colourisation or the compositor changes
    static GLASS_WINDOWS: RefCell<Vec<glib::WeakRef<gtk::Window>>> = RefCell::new(Vec::new());
}

fn watch() -> Option<Rc<Watch>> {
    use std::os::fd::AsRawFd;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask};
    if let Some(w) = WATCH.with(|w| w.borrow().clone()) { return Some(w); }
    if WATCH_TRIED.with(|t| t.replace(true)) { return None; }
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen].root;
    let atom = conn.intern_atom(false, COLORIZATION_PROP.as_bytes()).ok()?.reply().ok()?.atom;
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE)).ok()?;
    conn.flush().ok()?;
    let fd = conn.stream().as_raw_fd();
    let w = Rc::new(Watch { conn, root, atom });
    WATCH.with(|x| *x.borrow_mut() = Some(w.clone()));
    glib::source::unix_fd_add_local(fd, glib::IOCondition::IN, |_, _| {
        if pump() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
    });
    if let Some(s) = gtk::gdk::Screen::default() {
        // glass is painted differently with and without a compositor
        s.connect_composited_changed(|_| for_glass_windows(|w| { tag_glass(w); gtk::prelude::WidgetExt::queue_draw(w) }));
    }
    Some(w)
}

/// Handle what arrived on the watch connection (also events queued while waiting for a reply).
/// False once the connection is gone.
fn pump() -> bool {
    use x11rb::connection::Connection;
    let Some(w) = WATCH.with(|w| w.borrow().clone()) else { return false };
    let mut hit = false;
    let alive = loop {
        match w.conn.poll_for_event() {
            Ok(Some(x11rb::protocol::Event::PropertyNotify(p))) => hit |= p.atom == w.atom && p.window == w.root,
            Ok(Some(_)) => {}
            Ok(None) => break true,
            Err(_) => break false,
        }
    };
    if hit && !CHANGE_PENDING.with(|p| p.replace(true)) {
        // a dragged slider writes many values: repaint once per main-loop turn
        glib::idle_add_local_once(|| {
            CHANGE_PENDING.with(|p| p.set(false));
            changed();
        });
    }
    alive
}

/// The live `_AERODE_COLORIZATION`, if somebody published one.
pub fn read_property() -> Option<Colorization> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let w = watch()?;
    let reply = w.conn.get_property(false, w.root, w.atom, AtomEnum::CARDINAL, 0, 16).ok().and_then(|c| c.reply().ok());
    pump();
    from_prop(&reply?.value32()?.collect::<Vec<u32>>())
}

/// Broadcast a colourisation to every glass painter (all processes, the WM included);
/// `None` removes the property, which sends everybody back to config.json.
pub fn publish(c: Option<&Colorization>) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let Some(w) = watch() else {
        // no X connection of our own: at least this process follows
        apply(c.copied().unwrap_or_else(|| config::get().colorization.dwm()));
        return;
    };
    let _ = match c {
        Some(c) => w.conn.change_property32(PropMode::REPLACE, w.root, w.atom, AtomEnum::CARDINAL, &to_prop(c)).map(|_| ()),
        None => w.conn.delete_property(w.root, w.atom).map(|_| ()),
    };
    let _ = w.conn.flush();
}

/// The colourisation glass is painted with: the live root property, else the saved one.
pub fn colorization() -> Colorization {
    if let Some(c) = CURRENT.with(|c| c.get()) { return c; }
    let c = read_property().unwrap_or_else(|| config::get().colorization.dwm());
    CURRENT.with(|x| x.set(Some(c)));
    c
}

/// Run `f` whenever the colourisation changes (glass windows from `enable_rgba` are redrawn anyway).
pub fn on_color_change(f: impl Fn() + 'static) {
    let _ = colorization(); // starts the watch
    COLOR_LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
}

fn changed() {
    let new = read_property().unwrap_or_else(|| {
        config::reload();
        config::get().colorization.dwm()
    });
    apply(new);
}

fn apply(new: Colorization) {
    if CURRENT.with(|c| c.replace(Some(new))) == Some(new) { return; }
    for_glass_windows(|w| { tag_glass(w); gtk::prelude::WidgetExt::queue_draw(w) });
    COLOR_LISTENERS.with(|l| l.borrow().iter().for_each(|f| f()));
}

fn for_glass_windows(f: impl Fn(&gtk::Window)) {
    let live: Vec<gtk::Window> = GLASS_WINDOWS.with(|g| {
        let mut g = g.borrow_mut();
        g.retain(|w| w.upgrade().is_some());
        g.iter().filter_map(|w| w.upgrade()).collect()
    });
    live.iter().for_each(f);
}

/// Tag a window as aerode glass (picom blurs behind tagged windows only), or untag it while
/// "Enable transparency" is off: a solid surface needs no blur behind it.
fn tag_glass(w: &gtk::Window) {
    use gtk::prelude::*;
    let Some(gw) = w.window() else { return };
    let atom = gtk::gdk::Atom::intern("_AERODE_GLASS");
    if colorization().opaque {
        gtk::gdk::property_delete(&gw, &atom);
    } else {
        gtk::gdk::property_change(&gw, &atom, &gtk::gdk::Atom::intern("CARDINAL"), 32,
            gtk::gdk::PropMode::Replace, gtk::gdk::ChangeData::ULongs(&[1]));
    }
}

/// Give a toplevel an RGBA visual so it can be truly translucent, tag it as glass, and keep it
/// painted with the live colourisation.
pub fn enable_rgba(w: &gtk::Window) {
    use gtk::prelude::*;
    let _ = colorization(); // starts the watch
    if w.is_realized() { tag_glass(w); }
    w.connect_realize(tag_glass);
    if let Some(v) = WidgetExt::screen(w).and_then(|s| s.rgba_visual()) { w.set_visual(Some(&v)); }
    w.connect_screen_changed(|w, _| {
        if let Some(v) = WidgetExt::screen(w).and_then(|s| s.rgba_visual()) { w.set_visual(Some(&v)); }
    });
    GLASS_WINDOWS.with(|g| {
        let mut g = g.borrow_mut();
        g.retain(|w| w.upgrade().is_some());
        g.push(w.downgrade());
    });
}

// ------------------------------------------------------------------ painting

thread_local! {
    static THEME_GLASS: ThemeGlass = {
        // DWMWindow TOPFRAME (56) COLORIZATIONOPACITY and REFLECTIONMAP (40) OPACITY per state
        // (1 active, 2 inactive, 3 active maximized, 4 inactive maximized)
        let mut t = ThemeGlass::default();
        if let Some(th) = theme() {
            for st in 1..=4 {
                if let Some(v) = th.int("DWMWindow", 56, st, 2432) { t.opacity[st as usize - 1] = v as f64 / 100.0; }
                if let Some(v) = th.int("DWMWindow", 40, st, 2430) { t.reflection[st as usize - 1] = v as f64 / 100.0; }
            }
        }
        t
    };
    /// DWMWindow part 40 (REFLECTIONMAP): the streaks DWM lays over all glass, premultiplied
    static REFLECTION: Option<cairo::ImageSurface> = load_reflection();
    /// the wallpaper blurred the way Win7 blurs glass backdrops (no compositor: fake glass)
    static BACKDROP: RefCell<Option<Rc<Backdrop>>> = const { RefCell::new(None) };
    /// picom_fit results per state, for the current wallpaper
    static FITS: RefCell<Vec<(ShaderConsts, ([f64; 3], f64))>> = const { RefCell::new(Vec::new()) };
}

/// The theme's glass constants (opacity and reflection per window state).
pub fn theme_glass() -> ThemeGlass { THEME_GLASS.with(|t| *t) }

fn load_reflection() -> Option<cairo::ImageSurface> {
    let t = theme()?;
    let r = t.rect("DWMWindow", 40, 0, crate::msstyles::ATLASRECT)?;
    let atlas = t.stream(971)?;
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    if w <= 0 || h <= 0 || r[2] as u32 > atlas.width() || r[3] as u32 > atlas.height() { return None; }
    let mut s = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    let stride = s.stride() as usize;
    {
        let mut d = s.data().ok()?;
        for y in 0..h {
            for x in 0..w {
                let [r_, g, b, a] = atlas.get_pixel((r[0] + x) as u32, (r[1] + y) as u32).0;
                let pm = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
                let o = y as usize * stride + x as usize * 4;
                d[o..o + 4].copy_from_slice(&[pm(b), pm(g), pm(r_), a]);
            }
        }
    }
    Some(s)
}

struct Backdrop {
    w: i32,
    h: i32,
    rgb: Vec<u8>,
}

fn backdrop() -> Rc<Backdrop> {
    if let Some(b) = BACKDROP.with(|b| b.borrow().clone()) { return b; }
    let wp = wallpaper();
    let p = &wp.sharp;
    let (w, h, n, rs) = (p.width(), p.height(), p.n_channels() as usize, p.rowstride() as usize);
    let bytes = p.read_pixel_bytes();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            rgb.extend_from_slice(&bytes[y * rs + x * n..y * rs + x * n + 3]);
        }
    }
    let b = Rc::new(Backdrop { w, h, rgb: win7_blur(&rgb, w as usize, h as usize) });
    BACKDROP.with(|x| *x.borrow_mut() = Some(b.clone()));
    b
}

/// Win7's glass blur (uDWM, sigma 3): a horizontal 15-tap Gaussian (8 bilinear taps) sampled into a
/// quarter-width buffer, a vertical one, then linear upsampling back to full width. `rgb` is w×h×3.
pub fn win7_blur(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (wt, of) = crate::dwmcolor::dwm_blur_taps();
    let wq = (w >> 2).max(1);
    let lerp_at = |t: f32, n: usize| -> (usize, usize, f32) {
        let i0 = t.floor();
        let f = t - i0;
        let i0 = i0 as isize;
        (i0.clamp(0, n as isize - 1) as usize, (i0 + 1).clamp(0, n as isize - 1) as usize, f)
    };
    // pass 1: horizontal, quarter width (texel k sits at full-res x = 4k + 1.5)
    let mut q = vec![0f32; h * wq * 3];
    let taps1: Vec<Vec<(usize, usize, f32, f32)>> = (0..wq)
        .map(|k| (0..8).map(|i| { let (a, b, f) = lerp_at(4.0 * k as f32 + 1.0 + of[i], w); (a, b, f, wt[i]) }).collect())
        .collect();
    for y in 0..h {
        let row = &rgb[y * w * 3..(y + 1) * w * 3];
        for (k, taps) in taps1.iter().enumerate() {
            let mut acc = [0f32; 3];
            for &(a, b, f, wgt) in taps {
                for c in 0..3 {
                    acc[c] += wgt * (row[a * 3 + c] as f32 * (1.0 - f) + row[b * 3 + c] as f32 * f);
                }
            }
            q[(y * wq + k) * 3..(y * wq + k) * 3 + 3].copy_from_slice(&acc);
        }
    }
    // pass 2: vertical on the quarter-width buffer (linear in x, so it commutes with the upsampling)
    let mut v = vec![0f32; h * wq * 3];
    for y in 0..h {
        let taps: Vec<(usize, usize, f32, f32)> = (0..8).map(|i| { let (a, b, f) = lerp_at(y as f32 + of[i], h); (a, b, f, wt[i]) }).collect();
        for k in 0..wq {
            let mut acc = [0f32; 3];
            for &(a, b, f, wgt) in &taps {
                for c in 0..3 {
                    acc[c] += wgt * (q[(a * wq + k) * 3 + c] * (1.0 - f) + q[(b * wq + k) * 3 + c] * f);
                }
            }
            v[(y * wq + k) * 3..(y * wq + k) * 3 + 3].copy_from_slice(&acc);
        }
    }
    // linear upsampling to full width
    let up: Vec<(usize, usize, f32)> = (0..w).map(|x| lerp_at((x as f32 + 0.5) * 0.25 - 0.5, wq)).collect();
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        for (x, &(a, b, f)) in up.iter().enumerate() {
            for c in 0..3 {
                let s = v[(y * wq + a) * 3 + c] * (1.0 - f) + v[(y * wq + b) * 3 + c] * f;
                out[(y * w + x) * 3 + c] = (s + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// (S, a) for picom: premultiplied colour + alpha whose `S + (1-a) * blur(B)` best matches Win7.
fn picom_fit(k: &ShaderConsts) -> ([f64; 3], f64) {
    if let Some(r) = FITS.with(|f| f.borrow().iter().find(|(kk, _)| kk == k).map(|(_, r)| *r)) { return r; }
    // the backdrops glass usually covers: the (downscaled) wallpaper, white and grey windows, black
    let mut sample = vec![[1.0, 1.0, 1.0], [0.94, 0.94, 0.94], [0.0, 0.0, 0.0]];
    if let Some(small) = wallpaper().sharp.scale_simple(32, 18, InterpType::Tiles) {
        let (n, rs, bytes) = (small.n_channels() as usize, small.rowstride() as usize, small.read_pixel_bytes());
        for y in 0..small.height() as usize {
            for x in 0..small.width() as usize {
                let o = y * rs + x * n;
                sample.push([bytes[o] as f64 / 255.0, bytes[o + 1] as f64 / 255.0, bytes[o + 2] as f64 / 255.0]);
            }
        }
    }
    let r = k.picom_fit(&sample);
    FITS.with(|f| {
        let mut f = f.borrow_mut();
        if f.len() >= 8 { f.remove(0); }
        f.push((*k, r));
    });
    r
}

/// No compositor: colourise the Win7-blurred wallpaper behind the visible part of a w×h surface
/// whose top-left is at screen (sx, sy).
fn fake_glass(cr: &cairo::Context, k: &ShaderConsts, w: f64, h: f64, sx: f64, sy: f64) {
    let (cx0, cy0, cx1, cy1) = cr.clip_extents().unwrap_or((0.0, 0.0, w, h));
    let (x0, y0) = (cx0.max(0.0).floor() as i32, cy0.max(0.0).floor() as i32);
    let (x1, y1) = (cx1.min(w).ceil() as i32, cy1.min(h).ceil() as i32);
    if x1 <= x0 || y1 <= y0 { return; }
    let b = backdrop();
    let Ok(mut surf) = cairo::ImageSurface::create(cairo::Format::Rgb24, x1 - x0, y1 - y0) else { return };
    let stride = surf.stride() as usize;
    {
        let Ok(mut d) = surf.data() else { return };
        let bb = k.blur_balance as f32;
        let ag = k.afterglow.map(|v| v as f32);
        let col = k.color.map(|v| v as f32 * 255.0);
        let (ox, oy) = (sx.round() as i32 + x0, sy.round() as i32 + y0);
        for y in 0..y1 - y0 {
            let by = (oy + y).clamp(0, b.h - 1) as usize;
            let line = &mut d[y as usize * stride..];
            for x in 0..x1 - x0 {
                let bx = (ox + x).clamp(0, b.w - 1) as usize;
                let o = (by * b.w as usize + bx) * 3;
                let px = [b.rgb[o] as f32, b.rgb[o + 1] as f32, b.rgb[o + 2] as f32];
                let lum = px[0] * 0.2126 + px[1] * 0.7152 + px[2] * 0.0722;
                let c = |i: usize| (bb * px[i] + lum * ag[i] + col[i] + 0.5).clamp(0.0, 255.0) as u8;
                let q = x as usize * 4;
                line[q..q + 4].copy_from_slice(&[c(2), c(1), c(0), 255]);
            }
        }
    }
    cr.set_source_surface(&surf, x0 as f64, y0 as f64).ok();
    cr.paint().ok();
}

/// The DWM reflection over glass: the map stretched over the whole screen, shifted by 13 % of the
/// surface's distance from the screen centre (parallax). Without a theme: plain diagonal streaks.
fn paint_reflection(cr: &cairo::Context, opacity: f64, w: f64, h: f64, sx: f64, sy: f64) {
    if opacity <= 0.0 { return; }
    let screen = gtk::gdk::Screen::default().and_then(|s| s.root_window()).map(|r| (r.width() as f64, r.height() as f64));
    if let (Some(map), Some((scr_w, scr_h))) = (REFLECTION.with(|r| r.clone()), screen) {
        let shift = ((sx + w / 2.0) - scr_w / 2.0) * 0.13;
        cr.save().ok();
        cr.translate(shift - sx, -sy);
        cr.scale(scr_w / map.width() as f64, scr_h / map.height() as f64);
        cr.set_source_surface(&map, 0.0, 0.0).ok();
        cr.source().set_extend(cairo::Extend::Pad);
        cr.source().set_filter(cairo::Filter::Bilinear);
        cr.paint_with_alpha(opacity).ok();
        cr.restore().ok();
        return;
    }
    let mut x = -h;
    let mut i = 0;
    while x < w {
        let a = if i % 2 == 0 { 0.035 } else { 0.02 } * opacity / 0.9;
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
}

/// Fill the current clip with Win7 glass (no gloss, no border): a surface of w×h whose top-left is
/// at screen (sx, sy), in the given window state. Replaces whatever is under the clip.
pub fn paint_glass_fill(cr: &cairo::Context, w: f64, h: f64, sx: f64, sy: f64, active: bool, maximized: bool) {
    let k = colorization().realize(active, maximized, &theme_glass());
    cr.save().ok();
    cr.set_operator(cairo::Operator::Source);
    if k.solid {
        cr.set_source_rgb(k.color[0], k.color[1], k.color[2]);
        cr.paint().ok();
    } else if composited() {
        let (s, a) = picom_fit(&k);
        if a > 0.0 { cr.set_source_rgba(s[0] / a, s[1] / a, s[2] / a, a); } else { cr.set_source_rgba(0.0, 0.0, 0.0, 0.0); }
        cr.paint().ok();
    } else {
        fake_glass(cr, &k, w, h, sx, sy);
    }
    cr.set_operator(cairo::Operator::Over);
    paint_reflection(cr, k.reflection_opacity, w, h, sx, sy);
    cr.restore().ok();
}

/// Frame glass extended into a window's client area (the Explorer and Control Panel headers): fills
/// `widget` in its toplevel's state. Active = the WM's active window (_NET_WM_STATE_FOCUSED, GDK's
/// FOCUSED state), which the WM-drawn caption above follows too; is_active() is the X keyboard focus
/// and also flips on the NotifyGrab focus-outs of the WM's Windows-key grab.
/// The toplevel needs `enable_rgba` to be translucent under a compositor.
pub fn paint_widget_glass(widget: &impl gtk::prelude::IsA<gtk::Widget>, cr: &cairo::Context) {
    use gtk::prelude::*;
    let wd = widget.as_ref();
    let a = wd.allocation();
    let (ox, oy) = wd.window().map(|gw| { let (_, x, y) = gw.origin(); (x, y) }).unwrap_or((0, 0));
    let (sx, sy) = if wd.has_window() { (ox, oy) } else { (ox + a.x(), oy + a.y()) };
    let st = wd.toplevel().and_then(|t| t.window()).map(|gw| gw.state()).unwrap_or(gtk::gdk::WindowState::empty());
    paint_glass_fill(cr, a.width() as f64, a.height() as f64, sx as f64, sy as f64,
        st.contains(gtk::gdk::WindowState::FOCUSED), st.contains(gtk::gdk::WindowState::MAXIMIZED));
}

/// Paint an Aero glass panel of size w×h whose top-left is at screen (sx, sy) (taskbar, Start
/// menu, flyouts): active glass, the panel gloss and optionally the double border.
pub fn paint_glass(cr: &cairo::Context, w: f64, h: f64, sx: f64, sy: f64, radius: f64, border: bool) {
    cr.save().ok();
    cr.set_operator(cairo::Operator::Source);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    cr.paint().ok();
    cr.set_operator(cairo::Operator::Over);
    rounded(cr, 0.0, 0.0, w, h, radius);
    cr.clip();
    paint_glass_fill(cr, w, h, sx, sy, true, false);
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
    // explorer.exe bitmap 6801: three 54×54 states stacked, premultiplied alpha like every 32-bit
    // bitmap Windows feeds to AlphaBlend (read as straight alpha its glow came out too dark)
    static ORB: Option<cairo::ImageSurface> = config::data_file("orb.png")
        .and_then(|p| image::open(p).ok())
        .and_then(|i| crate::shellstyle::premul_surface(&i.to_rgba8()));
}

/// Draw the start orb (state 0 normal, 1 hover, 2 pressed) centred in a size×size box.
pub fn paint_orb(cr: &cairo::Context, size: f64, state: i32) {
    if let Some(strip) = ORB.with(|o| o.clone()) {
        let (fw, fh) = (strip.width(), strip.height() / 3);
        let s = size / fw.max(fh) as f64;
        cr.save().ok();
        cr.scale(s, s);
        cr.rectangle(0.0, 0.0, fw as f64, fh as f64);
        cr.clip();
        cr.set_source_surface(&strip, 0.0, -(fh * state.clamp(0, 2)) as f64).ok();
        if (s - 1.0).abs() > 0.01 { cr.source().set_filter(cairo::Filter::Good); }
        cr.paint().ok();
        cr.restore().ok();
        return;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorization_property_round_trip() {
        for opaque in [false, true] {
            let c = Colorization::from_cpl(0xA8CE0F0F, opaque);
            assert_eq!(from_prop(&to_prop(&c)), Some(c));
        }
        assert_eq!(from_prop(&[1, 2, 3]), None);
    }

    #[test]
    fn win7_blur_keeps_flat_and_softens_edges() {
        let (w, h) = (64, 32);
        let flat = vec![77u8; w * h * 3];
        assert!(win7_blur(&flat, w, h).iter().all(|v| *v == 77));
        // vertical step at x = 32: blurred over roughly +-7 px, monotonic, ends untouched
        let step: Vec<u8> = (0..w * h).flat_map(|i| { let v = if i % w < 32 { 0 } else { 255 }; [v, v, v] }).collect();
        let out = win7_blur(&step, w, h);
        let row: Vec<u8> = (0..w).map(|x| out[(16 * w + x) * 3]).collect();
        assert_eq!(row[2], 0);
        assert_eq!(row[w - 3], 255);
        assert!(row.windows(2).all(|p| p[0] <= p[1]), "{row:?}");
        assert!(row[26] > 0 && row[26] < 128 && row[38] > 128 && row[38] < 255, "{row:?}");
    }
}
