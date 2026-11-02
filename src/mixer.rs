//! Win7's volume trackbar (the flyout's and the mixer's slider) and the Volume Mixer
//! (SndVol.exe dialog 200), plus Volume Control Options (dialog 203).
//!
//! The slider is SndVol's msctls_trackbar32 (TBS_VERT|TBS_NOTICKS|TBS_TOOLTIPS|TBS_REVERSED,
//! range 0-100, the default page of range/5 = 20): the aero.msstyles TRACKBAR channel and
//! right-pointing thumb, SndVol's wedge and three ticks beside it, and in the mixer a green peak
//! meter in the channel. A click in the channel moves one page towards the pointer (no jump), the
//! value shows in a tooltip only while dragging, and letting go of the mouse or of a navigation
//! key is TB_ENDTRACK (the flyout plays the Default Beep then).
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::sys::{self, AudioCmd, AudioInfo};
use crate::util;

// ---------------------------------------------------------------- the trackbar
const TRACKBAR: &str = "TRACKBAR";
const TKP_TRACKVERT: i32 = 2;
const TKP_THUMBRIGHT: i32 = 8;

type Cb = RefCell<Vec<Box<dyn Fn()>>>;

pub struct Slider {
    pub area: gtk::DrawingArea,
    value: Cell<u32>,
    peak: Cell<f64>,
    link: Cell<Option<u32>>,
    /// pointer offset from the thumb's centre while dragging
    drag: Cell<Option<f64>>,
    hot: Cell<bool>,
    track: i32,
    thumb: (i32, i32),
    changed: RefCell<Vec<Box<dyn Fn(u32)>>>,
    touched: Cb,
    ended: Cb,
    tip: RefCell<Option<(gtk::Window, gtk::Label)>>,
}

impl Slider {
    /// A vertical volume trackbar with a `track` px long channel.
    pub fn new(track: i32) -> Rc<Slider> {
        let thumb = crate::shellstyle::size(TRACKBAR, TKP_THUMBRIGHT, 1).unwrap_or((19, 11));
        let area = gtk::DrawingArea::new();
        area.set_can_focus(true);
        area.set_size_request(thumb.0.max(22) + 6, track + thumb.1 + 2);
        area.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::LEAVE_NOTIFY_MASK | gdk::EventMask::SCROLL_MASK | gdk::EventMask::SMOOTH_SCROLL_MASK
            | gdk::EventMask::KEY_PRESS_MASK | gdk::EventMask::KEY_RELEASE_MASK);
        let s = Rc::new(Slider {
            area, value: Cell::new(0), peak: Cell::new(0.0), link: Cell::new(None), drag: Cell::new(None), hot: Cell::new(false),
            track, thumb, changed: RefCell::new(vec![]), touched: RefCell::new(vec![]), ended: RefCell::new(vec![]), tip: RefCell::new(None),
        });
        let w = Rc::downgrade(&s);
        s.area.connect_draw(move |a, cr| { if let Some(s) = w.upgrade() { s.draw(a, cr); } glib::Propagation::Stop });
        let w = Rc::downgrade(&s);
        s.area.connect_button_press_event(move |a, e| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            if e.button() != 1 || e.event_type() != gdk::EventType::ButtonPress || !a.is_sensitive() { return glib::Propagation::Stop; }
            a.grab_focus();
            s.touch();
            let (_, y) = e.position();
            let cy = s.thumb_y();
            if (y - cy).abs() <= s.thumb.1 as f64 / 2.0 + 1.0 {
                s.drag.set(Some(y - cy));
                s.show_tip();
            } else {
                // TB_PAGEUP / TB_PAGEDOWN towards the pointer
                let v = s.value.get() as i32 + if y < cy { 20 } else { -20 };
                s.user_set(v);
            }
            a.queue_draw();
            glib::Propagation::Stop
        });
        let w = Rc::downgrade(&s);
        s.area.connect_motion_notify_event(move |a, e| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            let (_, y) = e.position();
            if let Some(off) = s.drag.get() {
                let v = s.value_at(y - off);
                s.user_set(v as i32);
                s.show_tip();
            } else {
                let hot = (y - s.thumb_y()).abs() <= s.thumb.1 as f64 / 2.0;
                if hot != s.hot.get() { s.hot.set(hot); a.queue_draw(); }
            }
            glib::Propagation::Stop
        });
        let w = Rc::downgrade(&s);
        s.area.connect_leave_notify_event(move |a, _| { if let Some(s) = w.upgrade() { s.hot.set(false); a.queue_draw(); } glib::Propagation::Proceed });
        let w = Rc::downgrade(&s);
        s.area.connect_button_release_event(move |a, e| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            if e.button() != 1 || !a.is_sensitive() { return glib::Propagation::Stop; }
            s.drag.set(None);
            s.hide_tip();
            a.queue_draw();
            s.end();
            glib::Propagation::Stop
        });
        let w = Rc::downgrade(&s);
        let acc = Cell::new(0.0f64);
        s.area.connect_scroll_event(move |a, e| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            if !a.is_sensitive() { return glib::Propagation::Stop; }
            let n = match e.direction() {
                gdk::ScrollDirection::Up => 1,
                gdk::ScrollDirection::Down => -1,
                gdk::ScrollDirection::Smooth => { let v = acc.get() - e.delta().1; let n = v.trunc() as i32; acc.set(v - n as f64); n }
                _ => 0,
            };
            if n != 0 { s.touch(); s.user_set(s.value.get() as i32 + 10 * n); }
            glib::Propagation::Stop
        });
        let w = Rc::downgrade(&s);
        s.area.connect_key_press_event(move |a, e| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            let Some(d) = nav_key(e.keyval()) else { return glib::Propagation::Proceed };
            if !a.is_sensitive() { return glib::Propagation::Stop; }
            s.touch();
            let v = s.value.get() as i32;
            s.user_set(match d { Nav::Line(n) => v + n, Nav::Page(n) => v + 20 * n, Nav::Top => 100, Nav::Bottom => 0 });
            glib::Propagation::Stop
        });
        let w = Rc::downgrade(&s);
        s.area.connect_key_release_event(move |_, e| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            if nav_key(e.keyval()).is_none() { return glib::Propagation::Proceed; }
            s.end();
            glib::Propagation::Stop
        });
        let w = Rc::downgrade(&s);
        s.area.connect_destroy(move |_| { if let Some(s) = w.upgrade() { s.hide_tip(); } });
        s
    }

    pub fn value(&self) -> u32 { self.value.get() }
    /// Set from code (no change callbacks).
    pub fn set_value(&self, v: u32) {
        if self.value.replace(v.min(100)) != v.min(100) { self.area.queue_draw(); }
    }
    pub fn dragging(&self) -> bool { self.drag.get().is_some() }
    /// The peak meter in the channel (0..1).
    pub fn set_peak(&self, p: f64) {
        if (self.peak.replace(p) - p).abs() > 0.002 { self.area.queue_draw(); }
    }
    /// The line from the master volume across the programs' sliders.
    pub fn set_link(&self, v: Option<u32>) { if self.link.replace(v) != v { self.area.queue_draw(); } }
    pub fn set_enabled(&self, on: bool) { self.area.set_sensitive(on); }
    /// The user moved it.
    pub fn connect_changed(&self, f: impl Fn(u32) + 'static) { self.changed.borrow_mut().push(Box::new(f)); }
    /// A press, wheel or navigation key on it (before any change).
    pub fn connect_touched(&self, f: impl Fn() + 'static) { self.touched.borrow_mut().push(Box::new(f)); }
    /// TB_ENDTRACK: the mouse button or a navigation key was released.
    pub fn connect_ended(&self, f: impl Fn() + 'static) { self.ended.borrow_mut().push(Box::new(f)); }

    fn touch(&self) { for f in self.touched.borrow().iter() { f(); } }
    fn end(&self) { for f in self.ended.borrow().iter() { f(); } }
    fn user_set(&self, v: i32) {
        let v = v.clamp(0, 100) as u32;
        if self.value.replace(v) == v { return; }
        self.area.queue_draw();
        for f in self.changed.borrow().iter() { f(v); }
    }

    fn top(&self) -> f64 { self.thumb.1 as f64 / 2.0 + 1.0 }
    fn y_of(&self, v: u32) -> f64 { (self.top() + self.track as f64 * (100 - v.min(100)) as f64 / 100.0).round() }
    fn thumb_y(&self) -> f64 { self.y_of(self.value.get()) }
    fn value_at(&self, y: f64) -> u32 { (100.0 - (y - self.top()) * 100.0 / self.track as f64).round().clamp(0.0, 100.0) as u32 }

    fn draw(&self, a: &gtk::DrawingArea, cr: &gtk::cairo::Context) {
        let (tw, th) = (self.thumb.0 as f64, self.thumb.1 as f64);
        let (y0, y1) = (self.top(), self.top() + self.track as f64);
        let w = a.allocated_width() as f64;
        // centred like SndVol: channel, wedge and ticks span 26 px from the thumb's left edge
        let x = ((w - 26.0) / 2.0).floor().max(0.0);
        let cx = x + 3.0;
        // master-volume link line (mixer)
        if let Some(l) = self.link.get() {
            let ly = self.y_of(l) + 0.5;
            cr.set_source_rgb(0.80, 0.86, 0.93);
            cr.set_line_width(1.0);
            cr.move_to(0.0, ly);
            cr.line_to(w, ly);
            cr.stroke().ok();
        }
        // ticks at 100, 50 and 0 %, then SndVol's wedge (louder = wider) beside the channel
        cr.set_source_rgb(0.86, 0.86, 0.86);
        cr.set_line_width(1.0);
        for ty in [y0, (y0 + y1) / 2.0, y1] {
            let ty = ty.round() + 0.5;
            cr.move_to(cx + 6.0, ty);
            cr.line_to(cx + 22.0, ty);
            cr.stroke().ok();
        }
        cr.move_to(cx + 6.0, y0);
        cr.line_to(cx + 18.0, y0);
        cr.line_to(cx + 7.5, y1);
        cr.line_to(cx + 6.0, y1);
        cr.close_path();
        cr.set_source_rgb(0.835, 0.875, 0.898);
        cr.fill().ok();
        // the channel
        let (chy, chh) = (y0 - 2.0, y1 - y0 + 4.0);
        if !crate::shellstyle::paint(cr, TRACKBAR, TKP_TRACKVERT, 1, cx, chy, 6.0, chh) {
            cr.rectangle(cx, chy, 6.0, chh);
            cr.set_source_rgb(0.69, 0.69, 0.69);
            cr.fill().ok();
            cr.rectangle(cx + 1.0, chy + 1.0, 5.0, chh - 1.0);
            cr.set_source_rgb(0.99, 0.99, 0.99);
            cr.fill().ok();
            cr.rectangle(cx + 1.0, chy + 1.0, 4.0, chh - 2.0);
            cr.set_source_rgb(0.906, 0.918, 0.918);
            cr.fill().ok();
        }
        // peak meter inside the channel
        let p = self.peak.get().clamp(0.0, 1.0);
        if p > 0.0 {
            let h = ((y1 - y0) * p).round();
            let g = gtk::cairo::LinearGradient::new(cx + 1.0, 0.0, cx + 5.0, 0.0);
            g.add_color_stop_rgb(0.0, 0.33, 0.80, 0.33);
            g.add_color_stop_rgb(1.0, 0.13, 0.62, 0.13);
            cr.rectangle(cx + 1.0, y1 + 1.0 - h, 4.0, h);
            cr.set_source(&g).ok();
            cr.fill().ok();
        }
        // the thumb
        let ty = (self.thumb_y() - th / 2.0).round();
        let state = if !a.is_sensitive() { 5 } else if self.dragging() { 3 } else if self.hot.get() { 2 } else if a.has_focus() { 4 } else { 1 };
        if !crate::shellstyle::paint(cr, TRACKBAR, TKP_THUMBRIGHT, state, x, ty, tw, th) {
            let (x0, y0, x1, y1) = (x + 0.5, ty + 0.5, x + tw - 0.5, ty + th - 0.5);
            cr.move_to(x0 + 2.0, y0);
            cr.line_to(x1 - th / 2.0, y0);
            cr.line_to(x1, (y0 + y1) / 2.0);
            cr.line_to(x1 - th / 2.0, y1);
            cr.line_to(x0 + 2.0, y1);
            cr.line_to(x0, y1 - 2.0);
            cr.line_to(x0, y0 + 2.0);
            cr.close_path();
            let g = gtk::cairo::LinearGradient::new(0.0, y0, 0.0, y1);
            g.add_color_stop_rgb(0.0, 0.82, 0.95, 1.0);
            g.add_color_stop_rgb(1.0, 0.45, 0.80, 0.95);
            cr.set_source(&g).ok();
            cr.fill_preserve().ok();
            cr.set_source_rgb(0.17, 0.38, 0.60);
            cr.set_line_width(1.0);
            cr.stroke().ok();
        }
    }

    /// TBS_TOOLTIPS: the position beside the thumb while dragging (left of a vertical trackbar).
    fn show_tip(&self) {
        let Some(gw) = self.area.window() else { return };
        let mut tip = self.tip.borrow_mut();
        let (win, lbl) = tip.get_or_insert_with(|| {
            let win = gtk::Window::new(gtk::WindowType::Popup);
            win.set_type_hint(gdk::WindowTypeHint::Tooltip);
            win.style_context().add_class("vol-tip");
            let lbl = gtk::Label::new(None);
            win.add(&lbl);
            (win, lbl)
        });
        lbl.set_text(&self.value.get().to_string());
        // a DrawingArea has its own GdkWindow: its origin is the widget's top-left on screen
        let (_, ox, oy) = gw.origin();
        let w = self.area.allocated_width();
        win.show_all();
        let (_, nat) = win.preferred_size();
        let x = ox + ((w - 26) / 2).max(0) - nat.width - 2;
        let y = oy + self.thumb_y() as i32 - nat.height / 2;
        win.move_(x, y);
    }

    fn hide_tip(&self) { if let Some((w, _)) = self.tip.borrow_mut().take() { unsafe { w.destroy(); } } }
}

enum Nav { Line(i32), Page(i32), Top, Bottom }

/// Trackbar keys (a vertical trackbar's top is its minimum: Up/Left/PgUp/Home raise the volume).
fn nav_key(k: gdk::keys::Key) -> Option<Nav> {
    use gdk::keys::constants as K;
    Some(match k {
        K::Up | K::KP_Up | K::Left | K::KP_Left => Nav::Line(1),
        K::Down | K::KP_Down | K::Right | K::KP_Right => Nav::Line(-1),
        K::Page_Up | K::KP_Page_Up => Nav::Page(1),
        K::Page_Down | K::KP_Page_Down => Nav::Page(-1),
        K::Home | K::KP_Home => Nav::Top,
        K::End | K::KP_End => Nav::Bottom,
        _ => return None,
    })
}

// ---------------------------------------------------------------- shared bits
/// SndVol.exe's blue speaker glyphs (icon groups 110 / 111) when extracted, else a theme speaker.
pub fn mute_glyph(muted: bool, size: i32) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let name = if muted { "aerode-sndvol-muted" } else { "aerode-sndvol-unmuted" };
    if let Some(p) = gtk::IconTheme::default().filter(|t| t.has_icon(name)).and_then(|_| util::pixbuf(name, size)) { return Some(p); }
    // fallback: the device speaker, with a red "no" sign when muted
    let base = util::pixbuf(util::first_icon(&["audio-speakers", "audio-volume-high"]), size)?;
    if !muted { return Some(base); }
    let surf = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, size, size).ok()?;
    {
        use gtk::gdk::prelude::GdkContextExt;
        let cr = gtk::cairo::Context::new(&surf).ok()?;
        cr.set_source_pixbuf(&base, 0.0, 0.0);
        cr.paint().ok();
        let r = size as f64 * 0.24;
        let (cx, cy) = (size as f64 - r - 0.5, size as f64 - r - 0.5);
        cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(0.85, 0.1, 0.1);
        cr.fill_preserve().ok();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(1.0);
        cr.stroke().ok();
        cr.move_to(cx - r * 0.6, cy + r * 0.6);
        cr.line_to(cx + r * 0.6, cy - r * 0.6);
        cr.set_line_width((size as f64 / 12.0).max(1.2));
        cr.stroke().ok();
    }
    gtk::gdk::pixbuf_get_from_surface(&surf, 0, 0, size, size)
}

/// A flat (toolbar-style) button holding an image.
pub fn flat_button(img: &gtk::Image) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_relief(gtk::ReliefStyle::None);
    b.style_context().add_class("vol-flat");
    b.add(img);
    b.set_halign(gtk::Align::Center);
    b
}

/// Open a device's Properties (Win7: `mmsys.cpl,,<device>,general`).
pub fn device_properties(sink: &str) {
    // the Playback tab with this device's Properties open, like SndVol's device button
    crate::flyouts::control(&format!("sound-properties:{sink}"));
}

/// Start the Volume Mixer (its own process, like sndvol.exe).
pub fn open() {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aerode".into());
    if !util::spawn(&[&exe, "mixer"]) { util::spawn(&["aerode", "mixer"]); }
}

// ---------------------------------------------------------------- peak meters
/// The peak level of a recording (`parec`), read in a helper thread; the child dies with it.
struct Meter {
    child: std::process::Child,
    peak: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl Meter {
    /// `target`: Err(sink-input index) for a program, Ok(monitor source) for a device.
    fn start(target: Result<&str, u32>) -> Option<Meter> {
        use std::io::Read;
        if !util::which("parec") { return None; }
        let mut c = std::process::Command::new("parec");
        match target {
            Ok(src) => { c.arg("-d").arg(src); }
            Err(idx) => { c.arg(format!("--monitor-stream={idx}")); }
        }
        c.args(["--raw", "--format=s16le", "--channels=1", "--rate=8000", "--latency-msec=30", "--client-name=Volume Mixer"])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        let mut child = c.spawn().ok()?;
        let mut out = child.stdout.take()?;
        let peak = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let p = peak.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 320];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 { break; }
                let m = buf[..n & !1].chunks_exact(2).map(|b| (i16::from_le_bytes([b[0], b[1]]) as i32).unsigned_abs()).max().unwrap_or(0);
                // keep the highest value until the UI reads it
                p.fetch_max(m, std::sync::atomic::Ordering::Relaxed);
            }
        });
        Some(Meter { child, peak })
    }
    /// The peak since the last call (0..1).
    fn take(&self) -> f64 { self.peak.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 32768.0 }
}

impl Drop for Meter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------- the Volume Mixer
struct Column {
    root: gtk::Box,
    slider: Rc<Slider>,
    mute: gtk::Button,
    mute_img: gtk::Image,
    name: String,
    muted: Cell<bool>,
    /// sink inputs of this program
    streams: RefCell<Vec<u32>>,
    /// its own level (% of the master)
    rel: Cell<u32>,
    meter: RefCell<Option<Meter>>,
    shown: Cell<f64>,
    last_user: Cell<Option<std::time::Instant>>,
}

struct Mixer {
    win: gtk::Window,
    dev_title: gtk::Label,
    dev_img: gtk::Image,
    master: Rc<Column>,
    system: Rc<Column>,
    apps_box: gtk::Box,
    apps: RefCell<Vec<(String, Rc<Column>)>>,
    info: RefCell<AudioInfo>,
    sys_pending: Cell<Option<glib::SourceId>>,
}

fn column(icon: &gtk::Image, name: &str, slider_len: i32) -> (gtk::Box, gtk::Button, Rc<Slider>, gtk::Button, gtk::Image) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
    col.set_size_request(104, -1);
    col.style_context().add_class("mixer-col");
    let b = flat_button(icon);
    col.pack_start(&b, false, false, 0);
    let l = gtk::Label::new(Some(name));
    l.set_justify(gtk::Justification::Center);
    l.set_line_wrap(true);
    l.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_lines(2);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(14);
    l.set_width_chars(14);
    l.set_size_request(-1, 34);
    l.set_valign(gtk::Align::Start);
    col.pack_start(&l, false, false, 0);
    let s = Slider::new(slider_len);
    s.area.set_halign(gtk::Align::Fill);
    col.pack_start(&s.area, false, false, 4);
    let mi = gtk::Image::new();
    let m = flat_button(&mi);
    col.pack_start(&m, false, false, 0);
    (col, b, s, m, mi)
}

fn new_column(icon: &gtk::Image, name: &str) -> (Rc<Column>, gtk::Button) {
    let (root, b, slider, mute, mute_img) = column(icon, name, 110);
    (Rc::new(Column { root, slider, mute, mute_img, name: name.to_string(), muted: Cell::new(false), streams: RefCell::new(vec![]),
        rel: Cell::new(100), meter: RefCell::new(None), shown: Cell::new(0.0), last_user: Cell::new(None) }), b)
}

impl Column {
    fn set_muted(&self, m: bool) {
        self.muted.set(m);
        if let Some(p) = mute_glyph(m, 16) { self.mute_img.set_from_pixbuf(Some(&p)); }
        self.mute.set_tooltip_text(Some(&format!("{} {}", if m { "Unmute" } else { "Mute" }, self.name)));
    }
    fn busy(&self) -> bool {
        self.slider.dragging() || self.last_user.get().map(|t| t.elapsed().as_millis() < 600).unwrap_or(false)
    }
    fn tick_meter(&self) {
        let p = self.meter.borrow().as_ref().map(|m| m.take()).unwrap_or(0.0);
        // fall back smoothly like Win7's meters
        let v = if self.muted.get() { 0.0 } else { p.max(self.shown.get() * 0.82) };
        self.shown.set(if v < 0.01 { 0.0 } else { v });
        self.slider.set_peak(self.shown.get());
    }
}

/// `aerode mixer`: the Volume Mixer window (runs until closed).
pub fn run() {
    if gtk::init().is_err() { eprintln!("aerode mixer: cannot open the display"); return; }
    // one mixer: starting it again brings the open one forward
    let app = gtk::Application::new(Some("org.aerode.VolumeMixer"), gtk::gio::ApplicationFlags::empty());
    app.connect_activate(|app| {
        if let Some(w) = app.windows().first() { w.present(); return; }
        crate::theme::init_app();
        crate::theme::load_css(crate::theme::SHELL_CSS);
        build(app);
    });
    app.run_with_args::<&str>(&[]);
}

fn build(app: &gtk::Application) {
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    app.add_window(&win);
    win.set_title("Volume Mixer");
    win.set_icon_name(Some("multimedia-volume-control"));
    win.set_type_hint(gdk::WindowTypeHint::Dialog);
    win.set_default_size(490, -1);
    // sndvol has only a close box (no minimize / maximize)
    win.connect_realize(|w| { if let Some(gw) = w.window() { gw.set_functions(gdk::WMFunction::MOVE | gdk::WMFunction::RESIZE | gdk::WMFunction::CLOSE); } });
    win.style_context().add_class("mixer");
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    outer.set_border_width(10);
    // "Device"
    let dev_frame = gtk::Frame::new(None);
    let dl = gtk::Label::with_mnemonic("_Device");
    dev_frame.set_label_widget(Some(&dl));
    let dev_img = util::image("audio-speakers", 32);
    let (master, dev_btn) = new_column(&dev_img, "Speakers");
    let dev_title = master.root.children().get(1).and_then(|w| w.clone().downcast::<gtk::Label>().ok()).unwrap_or_else(|| gtk::Label::new(None));
    master.root.set_border_width(4);
    dev_frame.add(&master.root);
    dl.set_mnemonic_widget(Some(&master.slider.area));
    outer.pack_start(&dev_frame, false, false, 0);
    // "Applications"
    let app_frame = gtk::Frame::new(Some("Applications"));
    let apps_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    apps_box.set_border_width(4);
    let sys_img = util::image(util::first_icon(&["multimedia-volume-control", "audio-speakers", "preferences-desktop-sound"]), 32);
    let (system, sys_btn) = new_column(&sys_img, "System Sounds");
    apps_box.pack_start(&system.root, false, false, 0);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    sw.add(&apps_box);
    app_frame.add(&sw);
    outer.pack_start(&app_frame, true, true, 0);
    win.add(&outer);
    let m = Rc::new(Mixer { win: win.clone(), dev_title, dev_img, master, system, apps_box, apps: RefCell::new(vec![]), info: RefCell::new(AudioInfo::default()), sys_pending: Cell::new(None) });
    // device column
    { let m2 = Rc::downgrade(&m); dev_btn.connect_clicked(move |_| { if let Some(m) = m2.upgrade() { device_properties(&m.info.borrow().default); } }); }
    {
        let w = Rc::downgrade(&m);
        m.master.slider.connect_touched(move || {
            let Some(m) = w.upgrade() else { return };
            // Win7: touching a muted device's slider unmutes it
            if m.master.muted.get() { m.master.set_muted(false); sys::send(AudioCmd::Mute { sink: None, mute: Some(false) }); }
        });
        let w = Rc::downgrade(&m);
        m.master.slider.connect_changed(move |v| {
            let Some(m) = w.upgrade() else { return };
            m.master.last_user.set(Some(std::time::Instant::now()));
            sys::send(AudioCmd::Level { sink: Some(m.info.borrow().default.clone()), pct: v });
            m.relink(v);
        });
        let w = Rc::downgrade(&m);
        m.master.mute.connect_clicked(move |_| {
            let Some(m) = w.upgrade() else { return };
            let to = !m.master.muted.get();
            m.master.set_muted(to);
            sys::send(AudioCmd::Mute { sink: Some(m.info.borrow().default.clone()), mute: Some(to) });
        });
    }
    // System Sounds
    sys_btn.connect_clicked(|_| crate::flyouts::control("sound-sounds"));
    m.wire_app(&m.system.clone(), true);
    // follow the sound server
    let refresh = {
        let w = Rc::downgrade(&m);
        move || {
            let w = w.clone();
            sys::query_async(true, move |i| { if let (Some(m), Some(i)) = (w.upgrade(), i) { m.update(i); } });
        }
    };
    refresh();
    let r = refresh.clone();
    let events = util::watch_lines(&["pactl", "subscribe"], |l| l.contains(" on sink") || l.contains(" on server") || l.contains(" on card "),
        std::time::Duration::from_millis(80), move || r());
    { let r = refresh.clone(); glib::timeout_add_seconds_local(if events { 30 } else { 2 }, move || { r(); glib::ControlFlow::Continue }); }
    {
        let w = Rc::downgrade(&m);
        glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
            let Some(m) = w.upgrade() else { return glib::ControlFlow::Break };
            m.master.tick_meter();
            m.system.tick_meter();
            for (_, c) in m.apps.borrow().iter() { c.tick_meter(); }
            glib::ControlFlow::Continue
        });
    }
    // the meters' recordings end with the window
    { let m2 = m.clone(); win.connect_destroy(move |_| { m2.apps.borrow_mut().clear(); m2.master.meter.borrow_mut().take(); m2.system.meter.borrow_mut().take(); }); }
    win.show_all();
}

impl Mixer {
    fn master_level(&self) -> u32 { self.master.slider.value() }

    /// The programs' sliders show their level × the master (Win7 links them to the device).
    fn relink(&self, master: u32) {
        for c in std::iter::once(&self.system).chain(self.apps.borrow().iter().map(|(_, c)| c)) {
            c.slider.set_link(Some(master));
            if !c.busy() { c.slider.set_value(c.rel.get() * master / 100); }
        }
    }

    fn wire_app(self: &Rc<Self>, c: &Rc<Column>, system: bool) {
        let (w, cw) = (Rc::downgrade(self), Rc::downgrade(c));
        c.slider.connect_touched(move || {
            let (Some(_m), Some(c)) = (w.upgrade(), cw.upgrade()) else { return };
            if c.muted.get() { c.set_muted(false); set_app_mute(&c, system, false); }
        });
        let (w, cw) = (Rc::downgrade(self), Rc::downgrade(c));
        c.slider.connect_changed(move |v| {
            let (Some(m), Some(c)) = (w.upgrade(), cw.upgrade()) else { return };
            c.last_user.set(Some(std::time::Instant::now()));
            let master = m.master_level();
            // above the master: raise the device and run the program at its full level
            let rel = if v > master {
                m.master.slider.set_value(v);
                m.master.last_user.set(Some(std::time::Instant::now()));
                sys::send(AudioCmd::Level { sink: Some(m.info.borrow().default.clone()), pct: v });
                m.relink(v);
                100
            } else if master == 0 { 100 } else { (v * 100 + master / 2) / master };
            c.rel.set(rel.min(100));
            if system {
                let mut p = sys::volume_prefs();
                p.system_sounds = rel.min(100);
                sys::save_volume_prefs(&p);
                // once the slider rests (it may need a moment of silence played as System Sounds)
                if let Some(id) = m.sys_pending.take() { id.remove(); }
                let w = Rc::downgrade(&m);
                m.sys_pending.set(Some(glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                    if let Some(m) = w.upgrade() { m.sys_pending.set(None); sys::send(AudioCmd::EventLevel(m.system.rel.get())); }
                })));
            } else {
                for &i in c.streams.borrow().iter() { sys::send(AudioCmd::StreamLevel { index: i, pct: rel.min(100) }); }
            }
        });
        let cw = Rc::downgrade(c);
        c.mute.connect_clicked(move |_| {
            let Some(c) = cw.upgrade() else { return };
            let to = !c.muted.get();
            c.set_muted(to);
            set_app_mute(&c, system, to);
        });
    }

    fn update(self: &Rc<Self>, i: AudioInfo) {
        let def = i.default_sink().cloned();
        let ok = def.is_some();
        // title: "Volume Mixer - Speakers (High Definition Audio Device)"
        match &def {
            Some(d) => {
                self.win.set_title(&format!("Volume Mixer - {} ({})", d.title, d.description));
                self.dev_title.set_text(&d.title);
                if let Some(p) = util::pixbuf(util::first_icon(d.icons), 32) { self.dev_img.set_from_pixbuf(Some(&p)); }
                let name = d.title.clone();
                self.master.mute.set_tooltip_text(Some(&format!("{} {name}", if d.muted { "Unmute" } else { "Mute" })));
            }
            None => { self.win.set_title("Volume Mixer"); self.dev_title.set_text(i.state.text()); }
        }
        for c in [&self.master, &self.system] { c.slider.set_enabled(ok); c.mute.set_sensitive(ok); }
        if let Some(d) = &def {
            if !self.master.busy() { self.master.slider.set_value(d.level().min(100)); }
            self.master.muted.set(d.muted);
            if let Some(p) = mute_glyph(d.muted, 16) { self.master.mute_img.set_from_pixbuf(Some(&p)); }
            self.master.mute.set_tooltip_text(Some(&format!("{} {}", if d.muted { "Unmute" } else { "Mute" }, d.title)));
            // the device meter: its monitor
            let want = d.monitor.clone();
            let have = self.master.meter.borrow().is_some();
            if !have && !want.is_empty() { *self.master.meter.borrow_mut() = Meter::start(Ok(&want)); }
        }
        let def_index = def.as_ref().map(|d| d.index);
        // programs on the default device, one column per program (all its streams)
        let mut groups: Vec<(String, Vec<sys::Stream>)> = vec![];
        let mut system: Vec<sys::Stream> = vec![];
        for s in i.streams.iter().filter(|s| Some(s.sink) == def_index) {
            if s.is_system_sounds() { system.push(s.clone()); continue; }
            let key = format!("{}\u{1}{}", s.app, s.pid);
            match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, v)) => v.push(s.clone()), None => groups.push((key, vec![s.clone()])) }
        }
        // System Sounds: its saved level; live event sounds follow it
        let prefs = sys::volume_prefs();
        *self.system.streams.borrow_mut() = system.iter().map(|s| s.index).collect();
        if !self.system.busy() {
            // a live event sound has the level the server keeps for System Sounds
            let l = system.iter().map(|s| s.level()).max().map(|l| l.min(100));
            self.system.rel.set(l.unwrap_or(prefs.system_sounds.min(100)));
            if let Some(l) = l { if l != prefs.system_sounds { let mut p = prefs.clone(); p.system_sounds = l; sys::save_volume_prefs(&p); } }
        }
        self.system.set_muted(prefs.system_sounds_muted);
        if system.is_empty() { self.system.meter.borrow_mut().take(); }
        else if self.system.meter.borrow().is_none() { *self.system.meter.borrow_mut() = Meter::start(Err(system[0].index)); }
        // drop the columns of programs that stopped, add the new ones
        {
            let mut apps = self.apps.borrow_mut();
            apps.retain(|(k, c)| { let keep = groups.iter().any(|(g, _)| g == k); if !keep { unsafe { c.root.destroy(); } } keep });
        }
        for (key, streams) in &groups {
            let existing = self.apps.borrow().iter().find(|(k, _)| k == key).map(|(_, c)| c.clone());
            let c = match existing {
                Some(c) => c,
                None => {
                    let s = &streams[0];
                    let name = if s.app.is_empty() { "Name Not Available".to_string() } else { s.app.clone() };
                    let img = gtk::Image::from_pixbuf(app_icon(s).as_ref());
                    let (c, _b) = new_column(&img, &name);
                    self.apps_box.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 0);
                    self.apps_box.pack_start(&c.root, false, false, 0);
                    c.root.show_all();
                    self.wire_app(&c, false);
                    *c.meter.borrow_mut() = Meter::start(Err(s.index));
                    self.apps.borrow_mut().push((key.clone(), c.clone()));
                    c
                }
            };
            *c.streams.borrow_mut() = streams.iter().map(|s| s.index).collect();
            if !c.busy() { c.rel.set(streams.iter().map(|s| s.level()).max().unwrap_or(100).min(100)); }
            c.set_muted(streams.iter().all(|s| s.muted));
            c.slider.set_enabled(ok);
            c.mute.set_sensitive(ok);
        }
        // separators left behind by removed columns
        let kids = self.apps_box.children();
        let mut prev_sep = true;
        for k in kids.iter() {
            let sep = k.is::<gtk::Separator>();
            if sep && prev_sep { unsafe { k.destroy(); } continue; }
            prev_sep = sep;
        }
        if let Some(last) = self.apps_box.children().last() { if last.is::<gtk::Separator>() { unsafe { last.destroy(); } } }
        *self.info.borrow_mut() = i;
        self.relink(self.master_level());
    }
}

fn set_app_mute(c: &Column, system: bool, to: bool) {
    if system {
        let mut p = sys::volume_prefs();
        p.system_sounds_muted = to;
        sys::save_volume_prefs(&p);
    }
    for &i in c.streams.borrow().iter() { sys::send(AudioCmd::StreamMute { index: i, mute: to }); }
}

/// A program's icon: the one it names, else its .desktop file's, else a generic one.
fn app_icon(s: &sys::Stream) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    if !s.icon.is_empty() { if let Some(p) = util::pixbuf(&s.icon, 32) { return Some(p); } }
    let bin = std::path::Path::new(&s.binary).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    if !bin.is_empty() {
        if let Some(a) = util::app_by_id(&format!("{bin}.desktop")).or_else(|| util::all_apps().into_iter().find(|a| {
            a.executable().file_name().map(|f| f.to_string_lossy() == bin).unwrap_or(false)
        })) {
            if let Some(p) = util::gicon_pixbuf(a.icon().as_ref(), 32) { return Some(p); }
        }
        if let Some(p) = util::pixbuf(&bin, 32) { return Some(p); }
    }
    util::pixbuf(util::first_icon(&["application-x-executable", "application-default-icon"]), 32)
}

// ---------------------------------------------------------------- Volume Control Options
/// Volume Control Options (sndvol.exe.mui DIALOG 203): which devices get their own column in the
/// volume flyout.
pub fn options_dialog(info: &AudioInfo) {
    let d = gtk::Window::new(gtk::WindowType::Toplevel);
    d.set_title("Volume Control Options");
    d.set_icon_name(Some("multimedia-volume-control"));
    d.set_type_hint(gdk::WindowTypeHint::Dialog);
    d.set_resizable(false);
    d.set_keep_above(true);
    d.set_position(gtk::WindowPosition::Center);
    d.style_context().add_class("vol-options");
    let v = gtk::Box::new(gtk::Orientation::Vertical, 10);
    v.set_border_width(14);
    let head = gtk::Label::new(Some("Select which devices show their own volume control"));
    head.style_context().add_class("main-instruction");
    head.set_xalign(0.0);
    v.pack_start(&head, false, false, 0);
    let t = gtk::Label::new(Some("The default sound device will always show its own volume control."));
    t.set_xalign(0.0);
    v.pack_start(&t, false, false, 2);
    let p = sys::volume_prefs();
    let all = gtk::CheckButton::with_mnemonic("_All devices currently playing sound");
    all.set_active(p.all_playing);
    v.pack_start(&all, false, false, 0);
    let comm = gtk::CheckButton::with_mnemonic("The default _communication device");
    comm.set_active(p.communication);
    v.pack_start(&comm, false, false, 0);
    let sl = gtk::Label::with_mnemonic("Sound _devices:");
    sl.set_xalign(0.0);
    v.pack_start(&sl, false, false, 0);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    list.set_border_width(4);
    let mut checks: Vec<(String, gtk::CheckButton)> = vec![];
    for s in &info.sinks {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let c = gtk::CheckButton::new();
        c.set_active(p.devices.contains(&s.name));
        row.pack_start(&c, false, false, 0);
        row.pack_start(&util::image(util::first_icon(s.icons), 32), false, false, 0);
        let tv = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let n = gtk::Label::new(Some(&s.title));
        n.set_xalign(0.0);
        let dsc = gtk::Label::new(Some(&s.description));
        dsc.set_xalign(0.0);
        dsc.style_context().add_class("dim-label");
        tv.pack_start(&n, false, false, 0);
        tv.pack_start(&dsc, false, false, 0);
        tv.set_valign(gtk::Align::Center);
        row.pack_start(&tv, true, true, 0);
        list.pack_start(&row, false, false, 0);
        checks.push((s.name.clone(), c));
    }
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.set_size_request(370, 150);
    sw.add(&list);
    sl.set_mnemonic_widget(Some(&sw));
    v.pack_start(&sw, true, true, 0);
    let bb = gtk::ButtonBox::new(gtk::Orientation::Horizontal);
    bb.set_layout(gtk::ButtonBoxStyle::End);
    bb.set_spacing(8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_mnemonic("_Apply");
    apply.set_sensitive(false);
    for b in [&ok, &cancel, &apply] { b.set_size_request(75, -1); bb.add(b); }
    v.pack_start(&bb, false, false, 0);
    d.add(&v);
    let checks = Rc::new(checks);
    let save = {
        let (all, comm, checks) = (all.clone(), comm.clone(), checks.clone());
        move || {
            let mut p = sys::volume_prefs();
            p.all_playing = all.is_active();
            p.communication = comm.is_active();
            p.devices = checks.iter().filter(|(_, c)| c.is_active()).map(|(n, _)| n.clone()).collect();
            sys::save_volume_prefs(&p);
        }
    };
    let dirty = { let a = apply.clone(); move || a.set_sensitive(true) };
    { let f = dirty.clone(); all.connect_toggled(move |_| f()); }
    { let f = dirty.clone(); comm.connect_toggled(move |_| f()); }
    for (_, c) in checks.iter() { let f = dirty.clone(); c.connect_toggled(move |_| f()); }
    { let (s, d) = (save.clone(), d.clone()); ok.connect_clicked(move |_| { s(); d.close(); }); }
    { let d = d.clone(); cancel.connect_clicked(move |_| d.close()); }
    apply.connect_clicked(move |a| { save(); a.set_sensitive(false); });
    ok.set_can_default(true);
    d.set_default(Some(&ok));
    d.show_all();
    d.present();
}
