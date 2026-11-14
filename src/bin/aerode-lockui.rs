//! The Windows 7 locked screen: the logon background, the user tile, "Locked"; a key press brings
//! the password box (one dot per character) with the round arrow button, the Caps Lock balloon, and
//! "The user name or password is incorrect." with OK after a wrong password, back to the box like Win7.
//!
//! The compositor owns the lock (`wmipc::Req::Lock`): it shows only this program's surfaces and
//! unlocks when it exits 0, so a crash here leaves the session locked. The password check is PAM in
//! xsecurelock's `authproto_pam`, driven through its packet protocol (helpers/authproto.h:
//! `<type> <len>\n<message>\n`); this exits 0 only when authproto_pam does. Nothing here touches PAM
//! or keeps the password beyond the conversation. `--demo` shows it as an ordinary run: Esc quits.
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::rc::Rc;

use cairo::{Context, ImageSurface};
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

#[derive(Clone, Copy, PartialEq)]
enum Mode { Saver, Prompt, Busy, Error }

struct Ui {
    /// the monitor the tile is centred on (the primary one)
    mon: (f64, f64, f64, f64),
    monitors: Vec<(f64, f64, f64, f64)>,
    bg: Option<ImageSurface>,
    face: Option<ImageSurface>,
    name: String,
    mode: Mode,
    chars: usize,
    echo: Option<String>,
    caps: bool,
    message: String,
    /// what has been typed; wiped as soon as it is sent or dropped
    buf: Vec<u8>,
    /// the running conversation
    auth: Option<ChildStdin>,
    /// authproto is waiting for an answer ('p' password, 'u' user)
    asked: Option<char>,
    answered: u32,
    failed_msg: String,
    windows: Vec<gtk::Window>,
    last_key: std::time::Instant,
}

enum Packet { Msg(u8, String), Done(bool) }

fn main() {
    let demo = std::env::args().nth(1).as_deref() == Some("--demo");
    if gtk::init().is_err() { std::process::exit(1); }
    let Some(display) = gdk::Display::default() else { std::process::exit(1) };
    let mut monitors = vec![];
    let mut mon = None;
    for i in 0..display.n_monitors() {
        let Some(m) = display.monitor(i) else { continue };
        let g = m.geometry();
        let rect = (g.x() as f64, g.y() as f64, g.width() as f64, g.height() as f64);
        if m.is_primary() { mon = Some(rect); }
        monitors.push((rect, m));
    }
    let rects: Vec<_> = monitors.iter().map(|m| m.0).collect();
    let mon = mon.or(rects.first().copied()).unwrap_or((0.0, 0.0, 1366.0, 768.0));
    let data = aerode::config::data_dir();
    let load = |p: std::path::PathBuf| image::open(p).ok().map(|i| aerode::wm::decor::rgba_surface(&i.to_rgba8()));
    let bg = ["lockscreen.png", "lockscreen.jpg", "wallpaper.jpg"].iter().find_map(|f| load(data.join(f)));
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let face = load(home.join(".face")).or_else(|| load(data.join("avatar.png")));
    let ui = Rc::new(RefCell::new(Ui {
        mon, monitors: rects, bg, face, name: user_name(), mode: Mode::Saver, chars: 0, echo: None, caps: false, message: String::new(),
        buf: Vec::new(), auth: None, asked: None, answered: 0, failed_msg: String::new(), windows: vec![], last_key: std::time::Instant::now(),
    }));
    let (tx, rx) = async_channel::unbounded::<Packet>();
    for (rect, m) in &monitors {
        let w = gtk::Window::new(gtk::WindowType::Toplevel);
        w.init_layer_shell();
        w.set_layer(Layer::Overlay);
        w.set_namespace("aerode-lock");
        w.set_monitor(m);
        // the keyboard goes to the window of the monitor with the tile
        w.set_keyboard_mode(if *rect == mon { KeyboardMode::Exclusive } else { KeyboardMode::None });
        w.set_exclusive_zone(-1);
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] { w.set_anchor(e, true); }
        w.set_app_paintable(true);
        w.add_events(gdk::EventMask::KEY_PRESS_MASK | gdk::EventMask::BUTTON_PRESS_MASK);
        let (u, r) = (ui.clone(), *rect);
        w.connect_draw(move |_, cr| {
            cr.translate(-r.0, -r.1);
            u.borrow().paint(cr);
            glib::Propagation::Stop
        });
        let (u, t) = (ui.clone(), tx.clone());
        w.connect_key_press_event(move |_, e| { key(&u, &t, e, demo); glib::Propagation::Stop });
        let (u, t) = (ui.clone(), tx.clone());
        // a click wakes it like a key
        w.connect_button_press_event(move |_, _| { if u.borrow().mode == Mode::Saver { start_conversation(&u, &t); } glib::Propagation::Stop });
        w.show_all();
        ui.borrow_mut().windows.push(w);
    }
    {
        let u = ui.clone();
        glib::spawn_future_local(async move {
            while let Ok(p) = rx.recv().await { packet(&u, p); }
        });
    }
    // left alone at the prompt: back to the picture (and the conversation, with anything typed, gone)
    {
        let u = ui.clone();
        glib::timeout_add_seconds_local(5, move || {
            let idle = u.borrow().last_key.elapsed().as_secs() >= 60;
            if idle && matches!(u.borrow().mode, Mode::Prompt | Mode::Error) { cancel(&u); }
            glib::ControlFlow::Continue
        });
    }
    gtk::main();
}

fn redraw(u: &Rc<RefCell<Ui>>) { for w in &u.borrow().windows { w.queue_draw(); } }

fn wipe(buf: &mut Vec<u8>) { buf.iter_mut().for_each(|b| *b = 0); buf.clear(); }

/// Win7's wording for a failed logon, unless PAM said something more specific (account expired...).
fn incorrect(pam: &str) -> String {
    let l = pam.to_lowercase();
    if pam.is_empty() || l.contains("authentication") || l.contains("incorrect") || l.contains("password") {
        "The user name or password is incorrect.".into()
    } else { pam.to_string() }
}

/// Start authproto_pam, with a thread reading its packets.
fn start_conversation(u: &Rc<RefCell<Ui>>, tx: &async_channel::Sender<Packet>) {
    let path = std::env::var("XSECURELOCK_AUTHPROTO").ok().filter(|p| !p.is_empty()).or_else(|| {
        ["/usr/libexec/xsecurelock/authproto_pam", "/usr/lib/xsecurelock/authproto_pam", "/usr/local/libexec/xsecurelock/authproto_pam"]
            .iter().find(|p| std::path::Path::new(p).exists()).map(|p| p.to_string())
    });
    let mut ui = u.borrow_mut();
    ui.last_key = std::time::Instant::now();
    ui.answered = 0;
    ui.message.clear();
    ui.failed_msg.clear();
    wipe(&mut ui.buf);
    ui.chars = 0;
    ui.mode = Mode::Prompt;
    let child = path.and_then(|p| Command::new(p).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok());
    let Some(mut child) = child else {
        ui.mode = Mode::Error;
        ui.message = "The password can't be checked: xsecurelock's authproto_pam is missing.".into();
        drop(ui);
        redraw(u);
        return;
    };
    ui.auth = child.stdin.take();
    let out = child.stdout.take();
    let tx = tx.clone();
    std::thread::spawn(move || {
        if let Some(out) = out {
            let mut r = BufReader::new(out);
            while let Some((t, m)) = read_packet(&mut r) { let _ = tx.send_blocking(Packet::Msg(t, m)); }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = tx.send_blocking(Packet::Done(ok));
    });
    drop(ui);
    redraw(u);
}

fn read_packet(r: &mut impl BufRead) -> Option<(u8, String)> {
    let mut head = String::new();
    if r.read_line(&mut head).ok()? == 0 { return None; }
    let (ty, len) = head.trim_end().split_once(' ')?;
    let len: usize = len.trim().parse().ok()?;
    if len > 65536 { return None; }
    let mut msg = vec![0u8; len + 1];
    r.read_exact(&mut msg).ok()?;
    msg.pop();
    Some((*ty.as_bytes().first()?, String::from_utf8_lossy(&msg).into_owned()))
}

fn packet(u: &Rc<RefCell<Ui>>, p: Packet) {
    let mut ui = u.borrow_mut();
    match p {
        Packet::Msg(b'i', m) => ui.message = m,
        Packet::Msg(b'e', m) => ui.failed_msg = m,
        Packet::Msg(t @ (b'P' | b'U'), _) => {
            ui.asked = Some(if t == b'U' { 'u' } else { 'p' });
            ui.echo = (t == b'U').then(String::new);
            // authproto_pam asks again after a wrong password instead of failing: Win7 says so first
            if ui.answered > 0 {
                ui.mode = Mode::Error;
                ui.message = incorrect(&ui.failed_msg);
                ui.failed_msg.clear();
            } else {
                ui.mode = Mode::Prompt;
            }
        }
        Packet::Msg(..) => {}
        // the only way out of the lock
        Packet::Done(true) => std::process::exit(0),
        Packet::Done(false) => {
            ui.auth = None;
            ui.asked = None;
            if ui.mode == Mode::Saver { return; }
            ui.mode = Mode::Error;
            ui.message = incorrect(&ui.failed_msg);
        }
    }
    drop(ui);
    redraw(u);
}

fn cancel(u: &Rc<RefCell<Ui>>) {
    let mut ui = u.borrow_mut();
    if let Some(mut inp) = ui.auth.take() { let _ = inp.write_all(b"x 0\n\n"); let _ = inp.flush(); }
    wipe(&mut ui.buf);
    ui.chars = 0;
    ui.asked = None;
    ui.echo = None;
    ui.message.clear();
    ui.mode = Mode::Saver;
    drop(ui);
    redraw(u);
}

fn key(u: &Rc<RefCell<Ui>>, tx: &async_channel::Sender<Packet>, e: &gdk::EventKey, demo: bool) {
    use gdk::keys::constants as k;
    let kv = e.keyval();
    u.borrow_mut().last_key = std::time::Instant::now();
    let mode = u.borrow().mode;
    if demo && kv == k::Escape && mode == Mode::Saver { std::process::exit(1); }
    match mode {
        Mode::Saver => start_conversation(u, tx),
        Mode::Busy => {}
        Mode::Error => {
            if kv == k::Return || kv == k::KP_Enter || kv == k::space || kv == k::Escape {
                let alive = u.borrow().auth.is_some();
                if alive { let mut ui = u.borrow_mut(); ui.mode = Mode::Prompt; ui.message.clear(); drop(ui); redraw(u); }
                else { start_conversation(u, tx); }
            }
        }
        Mode::Prompt => {
            if kv == k::Escape { cancel(u); return; }
            let mut ui = u.borrow_mut();
            if kv == k::Return || kv == k::KP_Enter {
                let Some(t) = ui.asked.take() else { return };
                let mut answer = std::mem::take(&mut ui.buf);
                if let Some(inp) = ui.auth.as_mut() {
                    let _ = write!(inp, "{t} {}\n", answer.len());
                    let _ = inp.write_all(&answer);
                    let _ = inp.write_all(b"\n");
                    let _ = inp.flush();
                }
                wipe(&mut answer);
                ui.answered += 1;
                ui.mode = Mode::Busy;
                ui.chars = 0;
                ui.echo = None;
            } else if kv == k::BackSpace {
                // drop one UTF-8 character
                while let Some(c) = ui.buf.pop() { if c & 0xC0 != 0x80 { break; } }
            } else if let Some(c) = kv.to_unicode().filter(|c| !c.is_control()) {
                if ui.buf.len() < 256 { let mut b = [0u8; 4]; ui.buf.extend_from_slice(c.encode_utf8(&mut b).as_bytes()); }
            }
            let text = String::from_utf8_lossy(&ui.buf).into_owned();
            ui.chars = text.chars().count();
            if let Some(echo) = ui.echo.as_mut() { *echo = text; }
            ui.caps = gdk::Display::default().and_then(|d| gdk::Keymap::for_display(&d)).is_some_and(|k| k.is_caps_locked());
            drop(ui);
            redraw(u);
        }
    }
}

impl Ui {
    fn cx(&self) -> f64 { self.mon.0 + self.mon.2 / 2.0 }
    fn tile_cy(&self) -> f64 { self.mon.1 + self.mon.3 * 0.40 }

    fn paint(&self, cr: &Context) {
        // the logon background on every monitor
        for &(mx, my, mw, mh) in &self.monitors {
            cr.save().ok();
            cr.rectangle(mx, my, mw, mh);
            cr.clip();
            cr.set_source_rgb(0.11, 0.31, 0.48);
            cr.paint().ok();
            if let Some(bg) = &self.bg {
                let k = (mw / bg.width() as f64).max(mh / bg.height() as f64);
                cr.translate(mx + (mw - bg.width() as f64 * k) / 2.0, my + (mh - bg.height() as f64 * k) / 2.0);
                cr.scale(k, k);
                cr.set_source_surface(bg, 0.0, 0.0).ok();
                cr.source().set_filter(cairo::Filter::Good);
                cr.paint().ok();
            }
            cr.restore().ok();
        }
        let (cx, cy) = (self.cx(), self.tile_cy());
        if self.mode == Mode::Error {
            text(cr, &self.message, 15.0, false, cx, cy + 40.0, true);
            button(cr, "OK", cx - 45.0, cy + 80.0, 90.0, 28.0, true);
            return;
        }
        user_tile(cr, self.face.as_ref(), cx, cy);
        text(cr, &self.name, 24.0, false, cx, cy + 100.0, true);
        if self.mode == Mode::Saver {
            text(cr, "Locked", 15.0, false, cx, cy + 135.0, true);
            button(cr, "Switch User", cx - 55.0, self.mon.1 + self.mon.3 * 0.72, 110.0, 28.0, false);
            return;
        }
        text(cr, "Locked", 15.0, false, cx, cy + 135.0, true);
        // password box and the round arrow button
        let (bw, bh) = (225.0, 25.0);
        let (bx, by) = (cx - (bw + 36.0) / 2.0, cy + 165.0);
        glass_rounded(cr, bx, by, bw, bh, 2.0);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.fill_preserve().ok();
        cr.set_source_rgba(0.24, 0.38, 0.52, 0.9);
        cr.set_line_width(1.0);
        cr.stroke().ok();
        let shown = match &self.echo {
            Some(e) => e.clone(),
            None => "●".repeat(self.chars),
        };
        if shown.is_empty() {
            left_text(cr, "Password", 12.0, (0.55, 0.55, 0.55), bx + 6.0, by + bh / 2.0);
        } else {
            left_text(cr, &shown, if self.echo.is_some() { 12.0 } else { 9.0 }, (0.0, 0.0, 0.0), bx + 6.0, by + bh / 2.0);
        }
        arrow_button(cr, bx + bw + 6.0 + 15.0, by + bh / 2.0, self.mode == Mode::Busy);
        button(cr, "Switch User", cx - 55.0, self.mon.1 + self.mon.3 * 0.72, 110.0, 28.0, false);
        if self.mode == Mode::Busy {
            text(cr, "Welcome", 15.0, false, cx, by + 50.0, true);
        } else if self.caps {
            caps_balloon(cr, bx, by + bh + 8.0);
        } else if !self.message.is_empty() {
            text(cr, &self.message, 12.0, false, cx, by + 50.0, true);
        }
    }
}

fn user_name() -> String {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() {
            let gecos = std::ffi::CStr::from_ptr((*pw).pw_gecos).to_string_lossy();
            let full = gecos.split(',').next().unwrap_or("").trim().to_string();
            if !full.is_empty() { return full; }
            return std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        }
    }
    std::env::var("USER").unwrap_or_default()
}

fn glass_rounded(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

fn layout(cr: &Context, s: &str, px: f64, bold: bool) -> pangocairo::pango::Layout {
    let l = pangocairo::functions::create_layout(cr);
    let mut fd = pangocairo::pango::FontDescription::from_string(if bold { "Segoe UI Bold" } else { "Segoe UI" });
    fd.set_absolute_size(px * pangocairo::pango::SCALE as f64);
    l.set_font_description(Some(&fd));
    l.set_text(s);
    l
}

/// White text centred at (cx, cy) with Win7's soft dark shadow.
fn text(cr: &Context, s: &str, px: f64, bold: bool, cx: f64, cy: f64, shadow: bool) {
    let l = layout(cr, s, px, bold);
    let (w, h) = l.pixel_size();
    let (x, y) = ((cx - w as f64 / 2.0).round(), (cy - h as f64 / 2.0).round());
    if shadow {
        for (dx, dy, a) in [(1.0, 1.0, 0.45), (0.0, 1.0, 0.25), (1.0, 2.0, 0.2), (2.0, 2.0, 0.12)] {
            cr.move_to(x + dx, y + dy);
            cr.set_source_rgba(0.0, 0.0, 0.0, a);
            pangocairo::functions::show_layout(cr, &l);
        }
    }
    cr.move_to(x, y);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    pangocairo::functions::show_layout(cr, &l);
}

fn left_text(cr: &Context, s: &str, px: f64, c: (f64, f64, f64), x: f64, cy: f64) {
    let l = layout(cr, s, px, false);
    let (_, h) = l.pixel_size();
    cr.move_to(x, (cy - h as f64 / 2.0).round());
    cr.set_source_rgb(c.0, c.1, c.2);
    pangocairo::functions::show_layout(cr, &l);
}

/// The user picture in Win7's glossy tile frame.
fn user_tile(cr: &Context, face: Option<&ImageSurface>, cx: f64, cy: f64) {
    let (fs, ps) = (190.0, 126.0);
    let (fx, fy) = (cx - fs / 2.0, cy - fs / 2.0);
    // soft shadow
    for i in 0..6 {
        let k = i as f64;
        glass_rounded(cr, fx + 22.0 - k, fy + 26.0 - k, fs - 44.0 + 2.0 * k, fs - 44.0 + 2.0 * k, 8.0 + k);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.05);
        cr.fill().ok();
    }
    // the frame: light glass with a bright rim
    let (gx, gy, gs) = (cx - ps / 2.0 - 11.0, cy - ps / 2.0 - 11.0, ps + 22.0);
    glass_rounded(cr, gx, gy, gs, gs, 7.0);
    let g = cairo::LinearGradient::new(0.0, gy, 0.0, gy + gs);
    g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.85);
    g.add_color_stop_rgba(0.5, 0.86, 0.90, 0.95, 0.75);
    g.add_color_stop_rgba(1.0, 0.70, 0.78, 0.86, 0.8);
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgba(0.1, 0.2, 0.35, 0.6);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    glass_rounded(cr, gx + 1.5, gy + 1.5, gs - 3.0, gs - 3.0, 6.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
    cr.stroke().ok();
    // the picture
    let (px, py) = (cx - ps / 2.0, cy - ps / 2.0);
    cr.save().ok();
    cr.rectangle(px, py, ps, ps);
    cr.clip();
    match face {
        Some(f) => {
            let k = (ps / f.width() as f64).max(ps / f.height() as f64);
            cr.translate(px, py);
            cr.scale(k, k);
            cr.set_source_surface(f, 0.0, 0.0).ok();
            cr.source().set_filter(cairo::Filter::Good);
            cr.paint().ok();
        }
        None => { cr.set_source_rgb(0.33, 0.55, 0.78); cr.paint().ok(); }
    }
    cr.restore().ok();
    cr.rectangle(px - 0.5, py - 0.5, ps + 1.0, ps + 1.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
    cr.stroke().ok();
}

/// Win7's logon buttons: translucent glass with a light rim and white text.
fn button(cr: &Context, label: &str, x: f64, y: f64, w: f64, h: f64, default: bool) {
    glass_rounded(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 3.0);
    let g = cairo::LinearGradient::new(0.0, y, 0.0, y + h);
    g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.35);
    g.add_color_stop_rgba(0.5, 1.0, 1.0, 1.0, 0.18);
    g.add_color_stop_rgba(0.51, 1.0, 1.0, 1.0, 0.08);
    g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.2);
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgba(0.0, 0.1, 0.25, 0.7);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    glass_rounded(cr, x + 1.5, y + 1.5, w - 3.0, h - 3.0, 2.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, if default { 0.85 } else { 0.5 });
    cr.stroke().ok();
    text(cr, label, 12.0, false, x + w / 2.0, y + h / 2.0, true);
}

/// The round blue "go" button with a white arrow (dimmed while checking).
fn arrow_button(cr: &Context, cx: f64, cy: f64, busy: bool) {
    let r = 14.0;
    cr.new_path();
    cr.arc(cx, cy, r, 0.0, 2.0 * std::f64::consts::PI);
    let g = cairo::LinearGradient::new(0.0, cy - r, 0.0, cy + r);
    if busy {
        g.add_color_stop_rgb(0.0, 0.62, 0.70, 0.78);
        g.add_color_stop_rgb(1.0, 0.40, 0.48, 0.58);
    } else {
        g.add_color_stop_rgb(0.0, 0.50, 0.78, 0.98);
        g.add_color_stop_rgb(0.5, 0.15, 0.48, 0.85);
        g.add_color_stop_rgb(1.0, 0.10, 0.36, 0.72);
    }
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgba(0.02, 0.15, 0.35, 0.9);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    // gloss
    cr.save().ok();
    cr.arc(cx, cy, r - 1.5, 0.0, 2.0 * std::f64::consts::PI);
    cr.clip();
    cr.arc(cx, cy - r * 0.9, r * 1.05, 0.0, 2.0 * std::f64::consts::PI);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.28);
    cr.fill().ok();
    cr.restore().ok();
    // arrow
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.move_to(cx - 6.0, cy - 2.0);
    cr.line_to(cx + 1.0, cy - 2.0);
    cr.line_to(cx + 1.0, cy - 6.0);
    cr.line_to(cx + 7.0, cy);
    cr.line_to(cx + 1.0, cy + 6.0);
    cr.line_to(cx + 1.0, cy + 2.0);
    cr.line_to(cx - 6.0, cy + 2.0);
    cr.close_path();
    cr.fill().ok();
}

/// The "Caps Lock is on" balloon under the password box (sized to its text).
fn caps_balloon(cr: &Context, x: f64, y: f64) {
    let w = 320.0;
    let body = layout(cr, "Having Caps Lock on may cause you to enter your password incorrectly.\n\nYou should press Caps Lock to turn it off before entering your password.", 11.0, false);
    body.set_width(((w - 50.0) * pangocairo::pango::SCALE as f64) as i32);
    body.set_wrap(pangocairo::pango::WrapMode::Word);
    let h = 40.0 + body.pixel_size().1 as f64 + 10.0;
    let top = y + 12.0;
    glass_rounded(cr, x + 0.5, top + 0.5, w, h, 4.0);
    let g = cairo::LinearGradient::new(0.0, top, 0.0, top + h);
    g.add_color_stop_rgb(0.0, 1.0, 1.0, 1.0);
    g.add_color_stop_rgb(1.0, 0.89, 0.89, 0.93);
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgb(0.46, 0.46, 0.46);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    // the stem pointing at the password box
    cr.move_to(x + 20.5, top + 1.0);
    cr.line_to(x + 20.5, y + 0.5);
    cr.line_to(x + 34.0, top + 1.0);
    cr.close_path();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.fill_preserve().ok();
    cr.set_source_rgb(0.46, 0.46, 0.46);
    cr.stroke().ok();
    cr.rectangle(x + 21.0, top, 12.5, 1.5);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.fill().ok();
    // warning triangle
    let (tx, ty) = (x + 12.0, top + 10.0);
    cr.move_to(tx + 8.0, ty);
    cr.line_to(tx + 16.0, ty + 14.0);
    cr.line_to(tx, ty + 14.0);
    cr.close_path();
    cr.set_source_rgb(0.98, 0.78, 0.15);
    cr.fill_preserve().ok();
    cr.set_source_rgb(0.55, 0.40, 0.0);
    cr.stroke().ok();
    left_text(cr, "!", 11.0, (0.0, 0.0, 0.0), tx + 6.5, ty + 8.5);
    let l = layout(cr, "Caps Lock is on", 12.0, true);
    cr.move_to(x + 38.0, top + 9.0);
    cr.set_source_rgb(0.0, 0.0, 0.0);
    pangocairo::functions::show_layout(cr, &l);
    cr.move_to(x + 38.0, top + 32.0);
    pangocairo::functions::show_layout(cr, &body);
}
