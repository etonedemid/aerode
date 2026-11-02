//! The Windows 7 locked screen, as modules for xsecurelock. One binary, two roles chosen by the
//! name it is started under (install.sh links both into /usr/lib/aerode):
//!
//! * `saver_aerode` – the idle "Locked" screen: the logon background, the user tile, the name and
//!   "Locked".
//! * `auth_aerode` – what a key press brings up: the password box (● per character) with the round
//!   arrow button, "Switch User", the Caps Lock balloon, and "The user name or password is
//!   incorrect." with OK after a wrong password, back to the password box like Win7.
//!
//! xsecurelock owns the lock: it grabs keyboard and pointer, keeps the screen covered if a module
//! dies, and forwards keystrokes to this module on stdin. The password check is PAM in
//! xsecurelock's own `authproto_pam`, which this module drives through its packet protocol
//! (helpers/authproto.h: `<type> <len>\n<message>\n`); it unlocks only when authproto_pam
//! exits 0. Nothing here touches PAM or keeps the password beyond the conversation.
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cairo::{Context, Format, ImageSurface};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Copy, PartialEq)]
enum Mode { Saver, Prompt, Busy, Error }

struct Ui {
    conn: RustConnection,
    win: u32,
    gc: u32,
    depth: u8,
    w: i32,
    h: i32,
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
}

fn main() {
    let argv0 = std::env::args().next().unwrap_or_default();
    let role = std::path::Path::new(&argv0).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let demo = std::env::args().nth(1).filter(|a| a == "--demo");
    let auth = role.starts_with("auth_") || std::env::args().nth(2).as_deref() == Some("auth");
    let Some(mut ui) = Ui::open(demo.is_some()) else { std::process::exit(1) };
    if auth {
        ui.mode = Mode::Prompt;
        std::process::exit(authenticate(&mut ui));
    }
    ui.mode = Mode::Saver;
    ui.draw_all();
    // the saver just keeps the picture up until xsecurelock sends SIGTERM
    loop {
        match ui.conn.wait_for_event() {
            Ok(Event::Expose(e)) if e.count == 0 => ui.draw_all(),
            Ok(_) => {}
            Err(_) => std::process::exit(0),
        }
    }
}

// ---------------------------------------------------------------- authentication

/// Runs authproto conversations until one succeeds (0) or the user gives up (1).
fn authenticate(ui: &mut Ui) -> i32 {
    let timeout = std::env::var("XSECURELOCK_AUTH_TIMEOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(300u64);
    loop {
        ui.mode = Mode::Prompt;
        ui.message.clear();
        ui.draw_all();
        let Some(mut child) = spawn_authproto() else { return 1 };
        let mut out = child.stdout.take().unwrap();
        let mut inp = child.stdin.take().unwrap();
        let mut failed_msg = String::new();
        let mut cancelled = false;
        let mut answered = 0;
        loop {
            let Some((ty, msg)) = read_packet(ui, &mut out, timeout) else { break };
            match ty {
                b'i' => { ui.message = msg; ui.draw_dynamic(); }
                b'e' => { failed_msg = msg; }
                b'P' | b'U' => {
                    // authproto_pam asks again after a wrong password instead of failing:
                    // Win7 says so first, then shows the box again
                    if answered > 0 {
                        ui.mode = Mode::Error;
                        ui.message = incorrect(&failed_msg);
                        failed_msg.clear();
                        ui.draw_all();
                        if !wait_ok(ui, 60) { let _ = inp.write_all(b"x 0\n\n"); let _ = inp.flush(); cancelled = true; break; }
                        ui.message.clear();
                    }
                    ui.mode = Mode::Prompt;
                    ui.draw_all();
                    ui.echo = (ty == b'U').then(String::new);
                    match read_line(ui, timeout) {
                        Some(mut answer) => {
                            ui.mode = Mode::Busy;
                            ui.draw_dynamic();
                            let t = if ty == b'U' { 'u' } else { 'p' };
                            let _ = write!(inp, "{t} {}\n", answer.len());
                            let _ = inp.write_all(&answer);
                            let _ = inp.write_all(b"\n");
                            let _ = inp.flush();
                            answer.iter_mut().for_each(|b| *b = 0);
                            answered += 1;
                        }
                        None => {
                            let _ = inp.write_all(b"x 0\n\n");
                            let _ = inp.flush();
                            cancelled = true;
                        }
                    }
                    ui.echo = None;
                    ui.chars = 0;
                }
                _ => {}
            }
            if cancelled { break; }
        }
        drop(inp);
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if ok { return 0; }
        if cancelled { return 1; }
        // Win7: the message with an OK button, then the password box again
        ui.mode = Mode::Error;
        ui.message = incorrect(&failed_msg);
        ui.draw_all();
        if !wait_ok(ui, 60) { return 1; }
    }
}

/// Win7's wording for a failed logon, unless PAM said something more specific (account expired…).
fn incorrect(pam: &str) -> String {
    let l = pam.to_lowercase();
    if pam.is_empty() || l.contains("authentication") || l.contains("incorrect") || l.contains("password") {
        "The user name or password is incorrect.".into()
    } else { pam.to_string() }
}

fn spawn_authproto() -> Option<Child> {
    let path = std::env::var("XSECURELOCK_AUTHPROTO").ok().filter(|p| !p.is_empty()).or_else(|| {
        ["/usr/libexec/xsecurelock/authproto_pam", "/usr/lib/xsecurelock/authproto_pam", "/usr/local/libexec/xsecurelock/authproto_pam"]
            .iter().find(|p| std::path::Path::new(p).exists()).map(|p| p.to_string())
    })?;
    Command::new(path).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()
}

/// Wait until `fd` is readable, serving X expose events meanwhile. False on timeout.
fn wait_readable(ui: &mut Ui, fd: i32, until: Instant) -> bool {
    loop {
        // events already read off the X socket by an earlier reply
        while let Ok(Some(ev)) = ui.conn.poll_for_event() { ui.event(ev); }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() { return false; }
        let xfd = ui.conn.stream().as_raw_fd();
        let mut fds = [libc::pollfd { fd, events: libc::POLLIN, revents: 0 }, libc::pollfd { fd: xfd, events: libc::POLLIN, revents: 0 }];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, left.as_millis().min(i32::MAX as u128) as i32) };
        if n < 0 { continue; }
        if fds[0].revents != 0 { return true; }
    }
}

fn read_packet(ui: &mut Ui, out: &mut std::process::ChildStdout, timeout: u64) -> Option<(u8, String)> {
    let mut head = Vec::new();
    loop {
        // authproto may take a while (PAM delays after a wrong password): keep the screen alive
        if !wait_readable(ui, out.as_raw_fd(), Instant::now() + Duration::from_secs(timeout.max(60))) { return None; }
        let b = read_byte(out.as_raw_fd())?;
        if b == b'\n' { break; }
        head.push(b);
        if head.len() > 32 { return None; }
    }
    let head = String::from_utf8_lossy(&head).to_string();
    let (ty, len) = head.split_once(' ')?;
    let len: usize = len.trim().parse().ok()?;
    if len > 65536 { return None; }
    let mut msg = vec![0u8; len + 1];
    out.read_exact(&mut msg).ok()?;
    msg.pop();
    Some((*ty.as_bytes().first()?, String::from_utf8_lossy(&msg).into_owned()))
}

/// One byte straight from the fd (no std buffering: poll() must see everything that is pending).
fn read_byte(fd: i32) -> Option<u8> {
    let mut b = 0u8;
    loop {
        let n = unsafe { libc::read(fd, &mut b as *mut u8 as *mut libc::c_void, 1) };
        if n == 1 { return Some(b); }
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue; }
        return None;
    }
}

/// Keystrokes from xsecurelock until Enter (the answer) or Escape/timeout/EOF (None).
fn read_line(ui: &mut Ui, timeout: u64) -> Option<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut deadline = Instant::now() + Duration::from_secs(timeout);
    ui.chars = 0;
    ui.caps = caps_lock(&ui.conn, ui.win);
    ui.draw_dynamic();
    loop {
        if !wait_readable(ui, 0, deadline) { buf.iter_mut().for_each(|x| *x = 0); return None; }
        let Some(b) = read_byte(0) else { buf.iter_mut().for_each(|x| *x = 0); return None };
        deadline = Instant::now() + Duration::from_secs(timeout);
        match b {
            b'\r' | b'\n' => return Some(buf),
            0 | 0x1b => { buf.iter_mut().for_each(|x| *x = 0); return None; }
            0x08 | 0x7f => {
                // drop one UTF-8 character
                while let Some(c) = buf.pop() { if c & 0xC0 != 0x80 { break; } }
            }
            0x01 | 0x15 => { buf.iter_mut().for_each(|x| *x = 0); buf.clear(); }
            c if c < 0x20 => {}
            c => { if buf.len() < 256 { buf.push(c); } }
        }
        ui.chars = String::from_utf8_lossy(&buf).chars().count();
        if let Some(e) = ui.echo.as_mut() { *e = String::from_utf8_lossy(&buf).into_owned(); }
        ui.caps = caps_lock(&ui.conn, ui.win);
        ui.draw_dynamic();
    }
}

/// After a wrong password: wait for Enter / Escape / Space (the OK button), or give up.
fn wait_ok(ui: &mut Ui, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if !wait_readable(ui, 0, deadline) { return false; }
        let Some(b) = read_byte(0) else { return false };
        if matches!(b, b'\r' | b'\n' | b' ' | 0x1b) { return true; }
    }
}

fn caps_lock(conn: &RustConnection, win: u32) -> bool {
    conn.query_pointer(win).ok().and_then(|c| c.reply().ok()).map(|r| u16::from(r.mask) & u16::from(KeyButMask::LOCK) != 0).unwrap_or(false)
}

// ---------------------------------------------------------------- window and drawing

impl Ui {
    fn open(demo: bool) -> Option<Ui> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen].root;
        let parent = if demo { root } else {
            let v = std::env::var("XSCREENSAVER_WINDOW").ok()?;
            let v = v.trim();
            if let Some(h) = v.strip_prefix("0x") { u32::from_str_radix(h, 16).ok()? } else { v.parse().ok()? }
        };
        let g = conn.get_geometry(parent).ok()?.reply().ok()?;
        let (w, h) = if demo { (1366, 768) } else { (g.width as i32, g.height as i32) };
        let win = conn.generate_id().ok()?;
        conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, parent, 0, 0, w as u16, h as u16, 0, WindowClass::INPUT_OUTPUT, 0,
            &CreateWindowAux::new().background_pixel(0x1d4f7a).event_mask(EventMask::EXPOSURE)).ok()?;
        conn.map_window(win).ok()?;
        // xsecurelock leaves its auth window unmapped until the module shows it (auth_x11 does
        // XMapRaised too); the saver window is already mapped, so this changes nothing there
        if !demo {
            conn.map_window(parent).ok()?;
            conn.configure_window(parent, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)).ok()?;
        }
        let gc = conn.generate_id().ok()?;
        conn.create_gc(gc, win, &CreateGCAux::new()).ok()?;
        let depth = conn.get_geometry(win).ok()?.reply().ok()?.depth;
        // monitors in this window's coordinates (the lock window covers the whole screen)
        let mut monitors = vec![];
        let mut mon = None;
        if !demo {
            if let Ok(Ok(r)) = conn.randr_get_monitors(root, true).map(|c| c.reply()) {
                for m in r.monitors {
                    let rect = (m.x as f64, m.y as f64, m.width as f64, m.height as f64);
                    if m.primary { mon = Some(rect); }
                    monitors.push(rect);
                }
            }
        }
        if monitors.is_empty() { monitors.push((0.0, 0.0, w as f64, h as f64)); }
        let mon = mon.unwrap_or(monitors[0]);
        let data = aerode::config::data_dir();
        let load = |p: std::path::PathBuf| image::open(p).ok().map(|i| aerode::wm::decor::rgba_surface(&i.to_rgba8()));
        let bg = ["lockscreen.png", "lockscreen.jpg", "wallpaper.jpg"].iter().find_map(|f| load(data.join(f)));
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
        let face = load(home.join(".face")).or_else(|| load(data.join("avatar.png")));
        let _ = conn.flush();
        Some(Ui { conn, win, gc, depth, w, h, mon, monitors, bg, face, name: user_name(), mode: Mode::Saver, chars: 0, echo: None, caps: false, message: String::new() })
    }

    fn event(&mut self, ev: Event) {
        if let Event::Expose(e) = ev { if e.count == 0 { self.draw_all(); } }
    }

    fn cx(&self) -> f64 { self.mon.0 + self.mon.2 / 2.0 }
    fn tile_cy(&self) -> f64 { self.mon.1 + self.mon.3 * 0.40 }
    /// the part that changes while typing: password box, arrow, balloon, messages
    fn dynamic_rect(&self) -> (i32, i32, i32, i32) {
        let y = self.tile_cy() + 150.0;
        ((self.cx() - 230.0) as i32, y as i32, 460, 240)
    }

    fn draw_all(&mut self) { self.present(None); }
    fn draw_dynamic(&mut self) { let r = self.dynamic_rect(); self.present(Some(r)); }

    fn present(&mut self, area: Option<(i32, i32, i32, i32)>) {
        let (x0, y0, w, h) = area.unwrap_or((0, 0, self.w, self.h));
        let (x0, y0) = (x0.max(0), y0.max(0));
        let (w, h) = (w.min(self.w - x0), h.min(self.h - y0));
        if w <= 0 || h <= 0 { return; }
        let Ok(mut s) = ImageSurface::create(Format::Rgb24, w, h) else { return };
        {
            let Ok(cr) = Context::new(&s) else { return };
            cr.translate(-x0 as f64, -y0 as f64);
            self.paint(&cr);
        }
        s.flush();
        let stride = s.stride() as usize;
        let Ok(data) = s.data().map(|d| d.to_vec()) else { return };
        let rows = (200_000 / stride).max(1);
        for y in (0..h as usize).step_by(rows) {
            let n = rows.min(h as usize - y);
            let _ = self.conn.put_image(ImageFormat::Z_PIXMAP, self.win, self.gc, w as u16, n as u16, x0 as i16, (y0 + y as i32) as i16, 0, self.depth, &data[y * stride..(y + n) * stride]);
        }
        let _ = self.conn.flush();
    }

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
