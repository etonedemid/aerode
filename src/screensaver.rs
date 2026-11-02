//! Windows 7 screen savers – Blank, Bubbles, Mystify, Photos, Ribbons – drawn with cairo.
//!
//! `aerode screensaver` is the command xss-lock runs when the X screen saver timer (`xset s`, set from
//! Screen Saver Settings > Wait) fires: it shows the chosen saver full screen until the mouse moves
//! or a key is pressed, then shows the logon (lock) screen when "On resume, display logon screen"
//! is ticked. Control Panel uses [`Saver`] for the little animated preview in the monitor picture.
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use crate::{config, glass, util};

/// (id, Win7 name) in the order of the Screen saver list
pub const SAVERS: &[(&str, &str)] = &[("", "(None)"), ("blank", "Blank"), ("bubbles", "Bubbles"), ("mystify", "Mystify"), ("photos", "Photos"), ("ribbons", "Ribbons")];

struct Rng(u64);
impl Rng {
    fn new() -> Rng {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Rng(t | 1)
    }
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, a: f64, b: f64) -> f64 { a + (b - a) * self.f() }
}

fn hsv(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let h = (h.rem_euclid(1.0)) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 { 0 => (v, t, p), 1 => (q, v, p), 2 => (p, v, t), 3 => (p, q, v), 4 => (t, p, v), _ => (v, p, q) }
}

struct Bubble { x: f64, y: f64, vx: f64, vy: f64, r: f64, hue: f64 }
struct Poly { pts: Vec<(f64, f64, f64, f64)>, hue: f64, trail: Vec<Vec<(f64, f64)>>, acc: f64 }
struct Ribbon { pts: Vec<(f64, f64, f64)>, x: f64, y: f64, dir: f64, turn: f64, phase: f64, hue: f64, width: f64, max: usize, fade: f64 }
struct Photos { files: Vec<PathBuf>, idx: usize, cur: Option<Pixbuf>, prev: Option<Pixbuf>, shown: f64, hold: f64 }

/// One running screen saver, simulated in screen coordinates (`w` x `h`) and drawn at any size.
pub struct Saver {
    pub kind: String,
    w: f64,
    h: f64,
    rng: Rng,
    bubbles: Vec<Bubble>,
    polys: Vec<Poly>,
    ribbons: Vec<Ribbon>,
    photos: Option<Photos>,
    /// what Bubbles floats over (Win7 shows the desktop behind the bubbles)
    backdrop: Option<Pixbuf>,
}

impl Saver {
    pub fn new(kind: &str, w: f64, h: f64, backdrop: Option<Pixbuf>) -> Saver {
        let mut s = Saver { kind: kind.to_string(), w, h, rng: Rng::new(), bubbles: vec![], polys: vec![], ribbons: vec![], photos: None, backdrop };
        match kind {
            "bubbles" => {
                for _ in 0..14 {
                    let r = s.rng.range(h * 0.045, h * 0.11);
                    let a = s.rng.range(0.0, std::f64::consts::TAU);
                    let sp = s.rng.range(h * 0.08, h * 0.16);
                    let b = Bubble { x: s.rng.range(r, w - r), y: s.rng.range(r, h - r), vx: a.cos() * sp, vy: a.sin() * sp, r, hue: s.rng.f() };
                    s.bubbles.push(b);
                }
            }
            "mystify" => {
                for k in 0..2 {
                    let mut pts = vec![];
                    for _ in 0..4 {
                        let sp = (s.rng.range(h * 0.12, h * 0.24), s.rng.range(h * 0.12, h * 0.24));
                        let sg = (if s.rng.f() < 0.5 { -1.0 } else { 1.0 }, if s.rng.f() < 0.5 { -1.0 } else { 1.0 });
                        pts.push((s.rng.range(0.0, w), s.rng.range(0.0, h), sp.0 * sg.0, sp.1 * sg.1));
                    }
                    s.polys.push(Poly { pts, hue: k as f64 * 0.5 + s.rng.f() * 0.2, trail: vec![], acc: 0.0 });
                }
            }
            "ribbons" => { let r = s.new_ribbon(); s.ribbons.push(r); }
            "photos" => {
                let o = config::get().screensaver;
                let dir = if o.photos_dir.is_empty() { util::special_dir(glib::UserDirectory::Pictures, "Pictures") } else { PathBuf::from(&o.photos_dir) };
                let mut files = vec![];
                let mut stack = vec![(dir, 0)];
                while let Some((d, depth)) = stack.pop() {
                    let Ok(rd) = std::fs::read_dir(&d) else { continue };
                    for e in rd.flatten() {
                        let p = e.path();
                        let n = e.file_name().to_string_lossy().to_lowercase();
                        if n.starts_with('.') { continue; }
                        if p.is_dir() { if depth < 4 { stack.push((p, depth + 1)); } continue; }
                        if [".jpg", ".jpeg", ".png", ".bmp", ".gif", ".tif", ".tiff", ".webp"].iter().any(|x| n.ends_with(x)) { files.push(p); }
                        if files.len() > 2000 { break; }
                    }
                }
                files.sort();
                if o.photos_shuffle {
                    for i in (1..files.len()).rev() { let j = (s.rng.f() * (i + 1) as f64) as usize; files.swap(i, j.min(i)); }
                }
                let hold = match o.photos_speed.as_str() { "slow" => 10.0, "fast" => 3.0, _ => 6.0 };
                s.photos = Some(Photos { files, idx: 0, cur: None, prev: None, shown: 0.0, hold });
                s.next_photo();
            }
            _ => {}
        }
        s
    }

    fn new_ribbon(&mut self) -> Ribbon {
        let (w, h) = (self.w, self.h);
        Ribbon { pts: vec![], x: self.rng.range(w * 0.2, w * 0.8), y: self.rng.range(h * 0.2, h * 0.8), dir: self.rng.range(0.0, std::f64::consts::TAU),
            turn: 0.0, phase: self.rng.range(0.0, 6.0), hue: self.rng.f(), width: self.rng.range(h * 0.035, h * 0.07), max: 260 + (self.rng.f() * 200.0) as usize, fade: 1.0 }
    }

    fn next_photo(&mut self) {
        let (w, h) = (self.w as i32, self.h as i32);
        let Some(p) = self.photos.as_mut() else { return };
        if p.files.is_empty() { return; }
        for _ in 0..p.files.len() {
            let f = p.files[p.idx % p.files.len()].clone();
            p.idx += 1;
            if let Ok(pb) = Pixbuf::from_file_at_scale(&f, w, h, true) {
                p.prev = p.cur.take();
                p.cur = Some(pb);
                p.shown = 0.0;
                return;
            }
        }
    }

    /// Advance the animation by `dt` seconds.
    pub fn step(&mut self, dt: f64) {
        let dt = dt.clamp(0.0, 0.1);
        let (w, h) = (self.w, self.h);
        match self.kind.as_str() {
            "bubbles" => {
                for b in self.bubbles.iter_mut() {
                    b.x += b.vx * dt; b.y += b.vy * dt;
                    if b.x < b.r { b.x = b.r; b.vx = b.vx.abs(); }
                    if b.x > w - b.r { b.x = w - b.r; b.vx = -b.vx.abs(); }
                    if b.y < b.r { b.y = b.r; b.vy = b.vy.abs(); }
                    if b.y > h - b.r { b.y = h - b.r; b.vy = -b.vy.abs(); }
                    b.hue = (b.hue + dt * 0.03).rem_euclid(1.0);
                }
                // bubbles bounce off each other (equal-mass elastic collision along the line of centres)
                let n = self.bubbles.len();
                for i in 0..n {
                    for j in i + 1..n {
                        let (a, b) = { let (l, r) = self.bubbles.split_at_mut(j); (&mut l[i], &mut r[0]) };
                        let (dx, dy) = (b.x - a.x, b.y - a.y);
                        let d = (dx * dx + dy * dy).sqrt();
                        let min = a.r + b.r;
                        if d > 0.0 && d < min {
                            let (nx, ny) = (dx / d, dy / d);
                            let (ma, mb) = (a.r * a.r, b.r * b.r);
                            let rel = (a.vx - b.vx) * nx + (a.vy - b.vy) * ny;
                            if rel > 0.0 {
                                let k = 2.0 * rel / (ma + mb);
                                a.vx -= k * mb * nx; a.vy -= k * mb * ny;
                                b.vx += k * ma * nx; b.vy += k * ma * ny;
                            }
                            let push = (min - d) / 2.0;
                            a.x -= nx * push; a.y -= ny * push; b.x += nx * push; b.y += ny * push;
                        }
                    }
                }
            }
            "mystify" => {
                for p in self.polys.iter_mut() {
                    for v in p.pts.iter_mut() {
                        v.0 += v.2 * dt; v.1 += v.3 * dt;
                        if v.0 < 0.0 { v.0 = 0.0; v.2 = v.2.abs(); }
                        if v.0 > w { v.0 = w; v.2 = -v.2.abs(); }
                        if v.1 < 0.0 { v.1 = 0.0; v.3 = v.3.abs(); }
                        if v.1 > h { v.1 = h; v.3 = -v.3.abs(); }
                    }
                    p.hue = (p.hue + dt * 0.04).rem_euclid(1.0);
                    // the trail: a copy of the shape every 1/14 s, the last few kept
                    p.acc += dt;
                    if p.acc >= 1.0 / 14.0 || p.trail.is_empty() {
                        p.acc = 0.0;
                        p.trail.insert(0, p.pts.iter().map(|v| (v.0, v.1)).collect());
                        p.trail.truncate(7);
                    }
                }
            }
            "ribbons" => {
                let speed = h * 0.55;
                let mut spawn = false;
                for r in self.ribbons.iter_mut() {
                    if r.pts.len() < r.max {
                        // a smooth random walk: the turn rate itself drifts
                        r.turn = (r.turn + self.rng.range(-2.5, 2.5) * dt).clamp(-1.6, 1.6);
                        r.dir += r.turn * dt;
                        // steer back towards the middle near the edges
                        let (cx, cy) = (w / 2.0 - r.x, h / 2.0 - r.y);
                        if r.x < w * 0.08 || r.x > w * 0.92 || r.y < h * 0.08 || r.y > h * 0.92 {
                            let want = cy.atan2(cx);
                            let mut d = want - r.dir;
                            while d > std::f64::consts::PI { d -= std::f64::consts::TAU; }
                            while d < -std::f64::consts::PI { d += std::f64::consts::TAU; }
                            r.dir += d * (2.5 * dt).min(1.0);
                        }
                        r.x += r.dir.cos() * speed * dt;
                        r.y += r.dir.sin() * speed * dt;
                        r.phase += dt * 2.2;
                        r.pts.push((r.x, r.y, r.phase));
                        if r.pts.len() == r.max * 2 / 3 { spawn = true; }
                    } else {
                        r.fade -= dt / 2.5;
                    }
                }
                self.ribbons.retain(|r| r.fade > 0.0);
                if spawn || self.ribbons.is_empty() { let r = self.new_ribbon(); self.ribbons.push(r); }
            }
            "photos" => {
                let next = self.photos.as_mut().map(|p| { p.shown += dt; p.shown >= p.hold }).unwrap_or(false);
                if next { self.next_photo(); }
            }
            _ => {}
        }
    }

    /// Paint the current frame into a `dw` x `dh` area.
    pub fn draw(&self, cr: &cairo::Context, dw: f64, dh: f64) {
        cr.save().ok();
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.paint().ok();
        cr.scale(dw / self.w, dh / self.h);
        let (w, h) = (self.w, self.h);
        match self.kind.as_str() {
            "bubbles" => {
                if let Some(bg) = &self.backdrop {
                    cr.save().ok();
                    cr.scale(w / bg.width() as f64, h / bg.height() as f64);
                    gtk::gdk::prelude::GdkContextExt::set_source_pixbuf(cr, bg, 0.0, 0.0);
                    cr.paint().ok();
                    cr.restore().ok();
                }
                for b in &self.bubbles { paint_bubble(cr, b); }
            }
            "mystify" => {
                cr.set_line_join(cairo::LineJoin::Round);
                for p in &self.polys {
                    for (i, shape) in p.trail.iter().enumerate() {
                        let a = 1.0 - i as f64 / p.trail.len().max(1) as f64;
                        let (r, g, b) = hsv(p.hue - i as f64 * 0.012, 0.85, 1.0);
                        // Win7 Mystify lines glow: a wide faint pass under a thin bright one
                        for (lw, alpha) in [(h * 0.012, 0.18 * a), (h * 0.0032, 0.95 * a)] {
                            cr.new_path();
                            for (k, (x, y)) in shape.iter().enumerate() { if k == 0 { cr.move_to(*x, *y) } else { cr.line_to(*x, *y) } }
                            cr.close_path();
                            cr.set_line_width(lw);
                            cr.set_source_rgba(r, g, b, alpha);
                            cr.stroke().ok();
                        }
                    }
                }
            }
            "ribbons" => {
                for r in &self.ribbons {
                    let n = r.pts.len();
                    if n < 2 { continue; }
                    for i in 1..n {
                        let (x0, y0, p0) = r.pts[i - 1];
                        let (x1, y1, p1) = r.pts[i];
                        let (dx, dy) = (x1 - x0, y1 - y0);
                        let len = (dx * dx + dy * dy).sqrt().max(0.001);
                        let (nx, ny) = (-dy / len, dx / len);
                        // the band twists: its apparent width and shading follow the phase
                        let w0 = r.width * p0.sin().abs().max(0.08);
                        let w1 = r.width * p1.sin().abs().max(0.08);
                        let shade = 0.45 + 0.55 * p1.cos().abs();
                        let along = i as f64 / n as f64;
                        let (cr_, cg, cb) = hsv(r.hue + along * 0.25, 0.8, shade);
                        cr.new_path();
                        cr.move_to(x0 + nx * w0 / 2.0, y0 + ny * w0 / 2.0);
                        cr.line_to(x1 + nx * w1 / 2.0, y1 + ny * w1 / 2.0);
                        cr.line_to(x1 - nx * w1 / 2.0, y1 - ny * w1 / 2.0);
                        cr.line_to(x0 - nx * w0 / 2.0, y0 - ny * w0 / 2.0);
                        cr.close_path();
                        // opaque fill plus a hairline of the same colour hides the seams between quads
                        cr.set_source_rgba(cr_, cg, cb, r.fade);
                        cr.fill_preserve().ok();
                        cr.set_line_width(1.2);
                        cr.stroke().ok();
                    }
                }
            }
            "photos" => {
                if let Some(p) = &self.photos {
                    let fade = (p.shown / 1.0).min(1.0);
                    let show = |pb: &Pixbuf, a: f64| {
                        let (x, y) = ((w - pb.width() as f64) / 2.0, (h - pb.height() as f64) / 2.0);
                        gtk::gdk::prelude::GdkContextExt::set_source_pixbuf(cr, pb, x, y);
                        cr.paint_with_alpha(a).ok();
                    };
                    if let Some(pb) = &p.prev { if fade < 1.0 { show(pb, 1.0 - fade); } }
                    if let Some(pb) = &p.cur { show(pb, fade); }
                    if p.files.is_empty() {
                        cr.set_source_rgb(1.0, 1.0, 1.0);
                        cr.select_font_face("Segoe UI", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
                        cr.set_font_size(h * 0.03);
                        let t = "No pictures were found in the folder you selected.";
                        if let Ok(e) = cr.text_extents(t) { cr.move_to((w - e.width()) / 2.0, h / 2.0); }
                        cr.show_text(t).ok();
                    }
                }
            }
            _ => {}
        }
        cr.restore().ok();
    }
}

fn paint_bubble(cr: &cairo::Context, b: &Bubble) {
    let (x, y, r) = (b.x, b.y, b.r);
    // body: nearly clear in the middle, iridescent towards the rim
    let g = cairo::RadialGradient::new(x, y, 0.0, x, y, r);
    let (r1, g1, b1) = hsv(b.hue, 0.55, 1.0);
    let (r2, g2, b2) = hsv(b.hue + 0.35, 0.6, 1.0);
    g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.02);
    g.add_color_stop_rgba(0.72, r1, g1, b1, 0.06);
    g.add_color_stop_rgba(0.9, r2, g2, b2, 0.28);
    g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.55);
    cr.arc(x, y, r, 0.0, std::f64::consts::TAU);
    cr.set_source(&g).ok();
    cr.fill().ok();
    // thin bright rim
    cr.arc(x, y, r * 0.985, 0.0, std::f64::consts::TAU);
    cr.set_line_width(r * 0.03);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
    cr.stroke().ok();
    // specular highlight (upper left) and the faint reflection opposite
    for (hx, hy, hr, a) in [(-0.38, -0.42, 0.26, 0.85), (0.42, 0.45, 0.12, 0.35)] {
        let (cx, cy) = (x + r * hx, y + r * hy);
        let hg = cairo::RadialGradient::new(cx, cy, 0.0, cx, cy, r * hr);
        hg.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, a);
        hg.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
        cr.save().ok();
        cr.translate(cx, cy);
        cr.scale(1.0, 0.75);
        cr.translate(-cx, -cy);
        cr.arc(cx, cy, r * hr, 0.0, std::f64::consts::TAU);
        cr.set_source(&hg).ok();
        cr.fill().ok();
        cr.restore().ok();
    }
}

/// Seconds until the display turns off (Power Options > Turn off the display), if it does.
fn display_off_secs() -> Option<u64> {
    let s = std::fs::read_to_string(config::conf_dir().join("display-off")).ok()?;
    s.trim().parse::<u64>().ok().filter(|v| *v > 0)
}

/// Show `kind` full screen until the user moves the mouse or presses a key (blocking).
/// `as_saver`: started by the idle timer – keeps the X screen saver from blanking it and turns the
/// display off when Power Options says so.
pub fn run(kind: &str, as_saver: bool) {
    let Some(screen) = gdk::Screen::default() else { return };
    let display = screen.display();
    let mut rect: Option<gdk::Rectangle> = None;
    for i in 0..display.n_monitors() {
        if let Some(m) = display.monitor(i) {
            let g = m.geometry();
            rect = Some(match rect { None => g, Some(r) => r.union(&g) });
        }
    }
    let r = rect.unwrap_or_else(util::primary_geometry);
    let (w, h) = (r.width(), r.height());
    // Bubbles float over a picture of the desktop
    let backdrop = if kind == "bubbles" {
        screen.root_window().and_then(|root| root.pixbuf(r.x(), r.y(), w, h))
            .or_else(|| Some(glass::wallpaper().sharp.clone()))
    } else { None };
    let saver = Rc::new(RefCell::new(Saver::new(kind, w as f64, h as f64, backdrop)));
    let win = gtk::Window::new(gtk::WindowType::Popup);
    win.set_app_paintable(true);
    win.move_(r.x(), r.y());
    win.set_default_size(w, h);
    win.set_size_request(w, h);
    win.add_events(gdk::EventMask::POINTER_MOTION_MASK | gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::KEY_PRESS_MASK);
    { let s = saver.clone(); win.connect_draw(move |w, cr| { s.borrow().draw(cr, w.allocated_width() as f64, w.allocated_height() as f64); glib::Propagation::Stop }); }
    let main = glib::MainLoop::new(None, false);
    let started = std::time::Instant::now();
    let origin: Rc<Cell<Option<(f64, f64)>>> = Rc::new(Cell::new(None));
    let quit = { let m = main.clone(); move || if m.is_running() { m.quit() } };
    // ignore the first moments (the click on Preview, the mouse settling) and small jitters
    let armed = move || started.elapsed().as_millis() > 600;
    { let q = quit.clone(); win.connect_key_press_event(move |_, _| { if armed() { q(); } glib::Propagation::Stop }); }
    { let q = quit.clone(); win.connect_button_press_event(move |_, _| { if armed() { q(); } glib::Propagation::Stop }); }
    {
        let (q, o) = (quit.clone(), origin.clone());
        win.connect_motion_notify_event(move |_, e| {
            let (x, y) = e.root();
            match o.get() {
                None => o.set(Some((x, y))),
                Some((ox, oy)) => if armed() && ((x - ox).abs() > 12.0 || (y - oy).abs() > 12.0) { q(); },
            }
            glib::Propagation::Stop
        });
    }
    {
        let s = saver.clone();
        let last = Cell::new(None::<i64>);
        win.add_tick_callback(move |w, clock| {
            let t = clock.frame_time();
            if let Some(l) = last.get() { s.borrow_mut().step((t - l) as f64 / 1e6); }
            last.set(Some(t));
            if s.borrow().kind != "blank" && !s.borrow().kind.is_empty() { w.queue_draw(); }
            glib::ControlFlow::Continue
        });
    }
    win.connect_realize(|w| {
        if let Some(gw) = w.window() {
            gw.set_cursor(gdk::Cursor::for_display(&gw.display(), gdk::CursorType::BlankCursor).as_ref());
        }
    });
    win.show_all();
    // take the keyboard and pointer like a real screen saver (retry: a menu may hold a grab)
    {
        let w = win.clone();
        let tries = Cell::new(0);
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            let ok = w.window().and_then(|gw| gw.display().default_seat().map(|seat| seat.grab(&gw, gdk::SeatCapabilities::ALL, false, None, None, None) == gdk::GrabStatus::Success)).unwrap_or(false);
            tries.set(tries.get() + 1);
            if ok || tries.get() > 40 { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
    }
    if as_saver {
        // the X server's own saver window covers ours: switch it off now, keep it off until the display
        // is due to turn off (Power Options), then let DPMS take the screen dark
        let idle_at_start = config::get().screensaver.wait as u64 * 60;
        let off_after = display_off_secs().map(|d| d.saturating_sub(idle_at_start));
        if off_after != Some(0) {
            util::spawn(&["xset", "s", "reset"]);
            let t0 = std::time::Instant::now();
            glib::timeout_add_local(std::time::Duration::from_secs(20), move || {
                match off_after {
                    Some(s) if t0.elapsed().as_secs() >= s => { util::spawn(&["xset", "dpms", "force", "off"]); glib::ControlFlow::Break }
                    _ => { util::spawn(&["xset", "s", "reset"]); glib::ControlFlow::Continue }
                }
            });
        }
    }
    main.run();
    if let Some(seat) = win.display().default_seat() { seat.ungrab(); }
    win.close();
    while gtk::events_pending() { gtk::main_iteration(); }
}

/// `aerode screensaver [--preview NAME]`: the idle action (xss-lock), or a full-screen preview.
pub fn command(args: &[String]) {
    let o = config::get().screensaver;
    if let Some(i) = args.iter().position(|a| a == "--preview") {
        let kind = args.get(i + 1).cloned().unwrap_or(o.name.clone());
        if !kind.is_empty() { run(&kind, false); }
        return;
    }
    // xss-lock runs us before suspend too: lock straight away then
    if std::env::var_os("XSS_SLEEP_LOCK_FD").is_some() { crate::lock::lock(); return; }
    if !o.name.is_empty() { run(&o.name, true); }
    if o.lock { crate::lock::lock(); }
}
