//! Tray buttons with Win7-style flyouts: network, volume, battery, clock.
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;

use crate::taskbar::Taskbar;
use crate::{config, sys, util};

thread_local! {
    static OPEN: RefCell<Option<gtk::Window>> = RefCell::new(None);
    /// when a flyout last closed by itself (focus loss / Esc)
    static CLOSED_AT: std::cell::Cell<Option<std::time::Instant>> = std::cell::Cell::new(None);
}

/// Close the open flyout. Returns true if one was open *or* just closed itself because the
/// user clicked its tray icon (focus moved to the taskbar) – so that click only closes it, like Win7.
pub fn close_open() -> bool {
    let w = OPEN.with(|o| o.borrow_mut().take());
    if let Some(w) = w {
        let vis = w.is_visible();
        w.close();
        if vis { return true; }
    }
    CLOSED_AT.with(|c| c.take()).map(|t| t.elapsed().as_millis() < 350).unwrap_or(false)
}

fn closed_by_itself(w: &gtk::Window) {
    CLOSED_AT.with(|c| c.set(Some(std::time::Instant::now())));
    OPEN.with(|o| { let mut o = o.borrow_mut(); if o.as_ref() == Some(w) { *o = None; } });
    w.close();
}

/// A popup anchored above `anchor`, closed on focus loss.
pub fn popup(anchor: &gtk::Widget, body: &gtk::Widget, foot: Option<&gtk::Widget>) -> gtk::Window {
    close_open();
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_decorated(false);
    w.set_type_hint(gdk::WindowTypeHint::Utility);
    w.set_skip_taskbar_hint(true);
    w.set_skip_pager_hint(true);
    w.set_keep_above(true);
    w.set_app_paintable(true);
    crate::glass::enable_rgba(&w);
    w.style_context().add_class("aerode");
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.set_margin_start(7);
    frame.set_margin_end(7);
    frame.set_margin_top(7);
    frame.set_margin_bottom(7);
    body.style_context().add_class("flyout-body");
    frame.pack_start(body, true, true, 0);
    let foot_box = foot.map(|f| {
        f.style_context().add_class("flyout-foot");
        frame.pack_start(f, false, false, 0);
        f.clone()
    });
    w.add(&frame);
    w.connect_draw(move |w, cr| {
        let (x, y) = w.position();
        let fh = foot_box.as_ref().map(|f| f.allocated_height() as f64).unwrap_or(0.0);
        crate::glass::paint_flyout(cr, w.allocated_width() as f64, w.allocated_height() as f64, x as f64, y as f64, fh);
        glib::Propagation::Proceed
    });
    w.connect_focus_out_event(|w, _| { closed_by_itself(w); glib::Propagation::Proceed });
    w.connect_key_press_event(|w, e| {
        if e.keyval() == gdk::keys::constants::Escape { closed_by_itself(w); }
        glib::Propagation::Proceed
    });
    frame.show_all();
    let (_, nat) = w.preferred_size();
    let geo = util::primary_geometry();
    let (ax, _) = anchor
        .window()
        .map(|gw| { let (_, x, y) = gw.origin(); (x, y) })
        .unwrap_or((0, 0));
    let alloc = anchor.allocation();
    let x = (ax + alloc.x() + alloc.width() / 2 - nat.width / 2).clamp(geo.x() + 4, geo.x() + geo.width() - nat.width - 4);
    let y = geo.y() + geo.height() - config::get().taskbar_height - nat.height - 4;
    w.move_(x, y);
    w.show_all();
    w.present();
    OPEN.with(|o| *o.borrow_mut() = Some(w.clone()));
    w
}

fn tray_button(tb: &Rc<Taskbar>, icon: &str, tip: &str) -> (gtk::Button, gtk::Image) {
    let b = gtk::Button::new();
    b.style_context().add_class("tray");
    let img = util::image(icon, 16);
    b.add(&img);
    b.set_tooltip_text(Some(tip));
    tb.tray.pack_end(&b, false, false, 0);
    (b, img)
}

fn link(text: &str, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.style_context().add_class("link");
    b.set_halign(gtk::Align::Start);
    b.connect_clicked(move |_| { close_open(); f(); });
    b
}

fn control(page: &str) {
    if !util::spawn(&["aerode-control", page]) {
        util::error("Control Panel is not installed.", "");
    }
}

pub fn build_tray(tb: &Rc<Taskbar>) {
    clock(tb);
    if sys::battery().is_some() {
        battery(tb);
    }
    brightness(tb);
    volume(tb);
    network(tb);
    crate::tray::attach(tb);
}

// ---------------------------------------------------------------- clock
fn clock(tb: &Rc<Taskbar>) {
    let b = gtk::Button::new();
    b.style_context().add_class("clock");
    let v = gtk::Box::new(gtk::Orientation::Vertical, 0);
    v.set_valign(gtk::Align::Center);
    let t = gtk::Label::new(None);
    let d = gtk::Label::new(None);
    v.pack_start(&t, false, false, 0);
    v.pack_start(&d, false, false, 0);
    b.add(&v);
    tb.tray.pack_end(&b, false, false, 0);
    let update = {
        let (t, d, b) = (t.clone(), d.clone(), b.clone());
        move || {
            let now = chrono::Local::now();
            t.set_text(&if config::get().clock_24h { now.format("%H:%M").to_string() } else { now.format("%-I:%M %p").to_string() });
            d.set_text(&now.format("%-m/%-d/%Y").to_string());
            b.set_tooltip_text(Some(&now.format("%A, %B %-d, %Y").to_string()));
        }
    };
    update();
    glib::timeout_add_seconds_local(1, move || { update(); glib::ControlFlow::Continue });
    b.connect_clicked(|b| {
        if close_open() { return; }
        let now = chrono::Local::now();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let date = gtk::Label::new(Some(&now.format("%A, %B %-d, %Y").to_string()));
        date.style_context().add_class("fly-link");
        date.set_halign(gtk::Align::Start);
        body.pack_start(&date, false, false, 0);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        let cal = gtk::Calendar::new();
        cal.style_context().add_class("w7cal");
        row.pack_start(&cal, false, false, 0);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let face = gtk::DrawingArea::new();
        face.set_size_request(130, 130);
        let pb = |n: &str| config::data_file(&format!("clock/{n}.png")).and_then(|p| gtk::gdk_pixbuf::Pixbuf::from_file(p).ok());
        let (dial, glassov) = (pb("5000"), pb("5001"));
        face.connect_draw(move |_, cr| { draw_clock(cr, 130.0, dial.as_ref(), glassov.as_ref()); glib::Propagation::Proceed });
        right.pack_start(&face, false, false, 0);
        let digital = gtk::Label::new(Some(&now.format("%-I:%M:%S %p").to_string()));
        digital.style_context().add_class("fly-time");
        right.pack_start(&digital, false, false, 0);
        row.pack_start(&right, false, false, 0);
        body.pack_start(&row, false, false, 0);
        {
            let (face, digital) = (face.clone(), digital.clone());
            glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
                if !face.is_mapped() { return glib::ControlFlow::Break; }
                face.queue_draw();
                digital.set_text(&chrono::Local::now().format("%-I:%M:%S %p").to_string());
                glib::ControlFlow::Continue
            });
        }
        let foot = link("Change date and time settings...", || control("datetime"));
        popup(b.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
    });
}

/// Analog clock: Win7 dial + glass overlay with hands drawn in between.
fn draw_clock(cr: &gtk::cairo::Context, size: f64, dial: Option<&gtk::gdk_pixbuf::Pixbuf>, glass_ov: Option<&gtk::gdk_pixbuf::Pixbuf>) {
    use chrono::Timelike;
    use gtk::gdk::prelude::GdkContextExt;
    let c = size / 2.0;
    if let Some(d) = dial { cr.set_source_pixbuf(d, 0.0, 0.0); cr.paint().ok(); } else {
        cr.arc(c, c, c - 2.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(0.97, 0.97, 0.98);
        cr.fill_preserve().ok();
        cr.set_source_rgb(0.6, 0.62, 0.66);
        cr.stroke().ok();
    }
    let t = chrono::Local::now();
    let (h, m, s) = (t.hour() % 12, t.minute(), t.second());
    let hand = |angle: f64, len: f64, width: f64, rgba: (f64, f64, f64, f64)| {
        let a = angle - std::f64::consts::FRAC_PI_2;
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_width(width);
        cr.set_source_rgba(rgba.0, rgba.1, rgba.2, rgba.3);
        cr.move_to(c - a.cos() * len * 0.18, c - a.sin() * len * 0.18);
        cr.line_to(c + a.cos() * len, c + a.sin() * len);
        cr.stroke().ok();
    };
    let tau = std::f64::consts::TAU;
    hand(tau * (h as f64 + m as f64 / 60.0) / 12.0, size * 0.25, 3.5, (0.15, 0.16, 0.18, 0.95));
    hand(tau * (m as f64 + s as f64 / 60.0) / 60.0, size * 0.36, 2.5, (0.15, 0.16, 0.18, 0.95));
    hand(tau * s as f64 / 60.0, size * 0.38, 1.0, (0.75, 0.10, 0.10, 0.95));
    if let Some(g) = glass_ov { cr.set_source_pixbuf(g, 0.0, 0.0); cr.paint().ok(); }
}

// ---------------------------------------------------------------- volume
fn volume(tb: &Rc<Taskbar>) {
    let (b, img) = tray_button(tb, "audio-volume-high", "Speakers");
    let refresh = {
        let (img, b) = (img.clone(), b.clone());
        move || {
            let (img, b) = (img.clone(), b.clone());
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            // run the query off-thread through our own binary to avoid blocking
            util::output_async(vec![exe, "query-volume".into()], move |s| {
                let v = s.and_then(|s| { let mut it = s.split_whitespace(); Some((it.next()?.parse().ok()?, it.next()? == "1")) });
                if let Some(p) = util::pixbuf(sys::volume_icon(v), 16) { img.set_from_pixbuf(Some(&p)); }
                b.set_tooltip_text(Some(&match v { Some((p, m)) => format!("Speakers: {p}%{}", if m { " (muted)" } else { "" }), None => "No audio device".into() }));
            });
        }
    };
    refresh();
    { let r = refresh.clone(); glib::timeout_add_seconds_local(3, move || { r(); glib::ControlFlow::Continue }); }
    b.connect_clicked(move |b| {
        if close_open() { return; }
        let (pct, muted) = sys::volume().unwrap_or((0, false));
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let scale = gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, 100.0, 1.0);
        scale.set_inverted(true);
        scale.set_value(pct as f64);
        scale.set_size_request(-1, 140);
        scale.set_draw_value(false);
        let lbl = gtk::Label::new(Some(&pct.to_string()));
        { let l = lbl.clone(); scale.connect_value_changed(move |s| { let v = s.value() as u32; sys::set_volume(v); l.set_text(&v.to_string()); }); }
        scale.connect_button_release_event(|_, _| { util::sound(crate::sounds::Event::DefaultBeep); glib::Propagation::Proceed });
        let mute = gtk::Button::new();
        mute.add(&util::image(sys::volume_icon(Some((pct, muted))), 32));
        mute.set_relief(gtk::ReliefStyle::None);
        mute.set_tooltip_text(Some("Mute Speakers"));
        { let r = refresh.clone(); mute.connect_clicked(move |m| {
            sys::toggle_mute();
            let v = sys::volume();
            if let Some(img) = m.child().and_then(|c| c.downcast::<gtk::Image>().ok()) {
                if let Some(p) = util::pixbuf(sys::volume_icon(v), 32) { img.set_from_pixbuf(Some(&p)); }
            }
            r();
        }); }
        let title = gtk::Label::new(Some("Speakers"));
        title.style_context().add_class("link");
        body.pack_start(&title, false, false, 0);
        body.pack_start(&scale, true, true, 0);
        body.pack_start(&lbl, false, false, 0);
        body.pack_start(&mute, false, false, 0);
        let foot = link("Mixer", || { if !util::spawn(&["pavucontrol"]) { control("sound"); } });
        popup(b.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
    });
}

// ---------------------------------------------------------------- network
fn network(tb: &Rc<Taskbar>) {
    let (b, img) = tray_button(tb, "network-wired", "Network");
    let refresh = {
        let (img, b) = (img.clone(), b.clone());
        move || {
            let (img, b) = (img.clone(), b.clone());
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            util::output_async(vec![exe, "query-network".into()], move |s| {
                let s = s.unwrap_or_default();
                let mut it = s.trim().splitn(3, '\t');
                let n = sys::NetStatus { kind: it.next().unwrap_or("none").into(), signal: it.next().and_then(|x| x.parse().ok()).unwrap_or(0), name: it.next().unwrap_or("").into() };
                if let Some(p) = util::pixbuf(sys::net_icon(&n), 16) { img.set_from_pixbuf(Some(&p)); }
                b.set_tooltip_text(Some(&if n.kind == "none" { "Not connected".to_string() } else { format!("{}\nInternet access", n.name) }));
            });
        }
    };
    refresh();
    { let r = refresh.clone(); glib::timeout_add_seconds_local(5, move || { r(); glib::ControlFlow::Continue }); }
    b.connect_clicked(move |b| {
        if close_open() { return; }
        let st = sys::net_status();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_size_request(260, -1);
        let h = gtk::Label::new(Some("Currently connected to:"));
        h.set_halign(gtk::Align::Start);
        body.pack_start(&h, false, false, 0);
        let cur = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        cur.pack_start(&util::image(sys::net_icon(&st), 32), false, false, 0);
        let cl = gtk::Label::new(Some(&if st.kind == "none" { "Not connected".to_string() } else { format!("{}\nInternet access", st.name) }));
        cl.set_xalign(0.0);
        cur.pack_start(&cl, false, false, 0);
        body.pack_start(&cur, false, false, 4);
        let wifis = sys::wifi_list();
        if !wifis.is_empty() {
            body.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 2);
            let h = gtk::Label::new(Some("Wireless Network Connection"));
            h.set_halign(gtk::Align::Start);
            h.style_context().add_class("link");
            body.pack_start(&h, false, false, 0);
            let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
            for w in wifis.into_iter().take(12) {
                let row = gtk::Button::new();
                row.set_relief(gtk::ReliefStyle::None);
                let hb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let l = gtk::Label::new(Some(&format!("{}{}", w.ssid, if w.active { "   Connected" } else { "" })));
                l.set_xalign(0.0);
                hb.pack_start(&l, true, true, 0);
                if w.secure { hb.pack_start(&util::image("channel-secure-symbolic", 16), false, false, 0); }
                hb.pack_start(&util::image(sys::net_icon(&sys::NetStatus { kind: "wifi".into(), name: String::new(), signal: w.signal }), 16), false, false, 0);
                row.add(&hb);
                let r = refresh.clone();
                row.connect_clicked(move |_| { close_open(); connect_wifi(&w); r(); });
                list.pack_start(&row, false, false, 0);
            }
            let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
            sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            sw.set_min_content_height(160);
            sw.set_max_content_height(260);
            sw.set_propagate_natural_height(true);
            sw.add(&list);
            body.pack_start(&sw, true, true, 0);
        }
        let foot = link("Open Network and Sharing Center", || {
            if !util::spawn(&["nm-connection-editor"]) { control("network"); }
        });
        popup(b.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
    });
}

fn connect_wifi(w: &sys::Wifi) {
    if w.active {
        util::spawn(&["nmcli", "connection", "down", "id", &w.ssid]);
        return;
    }
    if !w.secure || sys::known_connections().contains(&w.ssid) {
        util::spawn(&["nmcli", "device", "wifi", "connect", &w.ssid]);
        return;
    }
    let d = gtk::Dialog::with_buttons(Some("Connect to a Network"), None::<&gtk::Window>, gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_spacing(8);
    c.set_border_width(12);
    let l = gtk::Label::new(Some(&format!("Type the network security key for \"{}\"", w.ssid)));
    l.set_xalign(0.0);
    let e = gtk::Entry::new();
    e.set_visibility(false);
    e.set_activates_default(true);
    let show = gtk::CheckButton::with_label("Hide characters");
    show.set_active(true);
    { let e = e.clone(); show.connect_toggled(move |s| e.set_visibility(!s.is_active())); }
    c.pack_start(&l, false, false, 0);
    c.pack_start(&e, false, false, 0);
    c.pack_start(&show, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    let ssid = w.ssid.clone();
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok {
            util::spawn(&["nmcli", "device", "wifi", "connect", &ssid, "password", &e.text()]);
        }
        d.close();
    });
    d.set_keep_above(true);
    d.show_all();
}

// ---------------------------------------------------------------- battery
fn battery(tb: &Rc<Taskbar>) {
    let (b, img) = tray_button(tb, "battery-good", "Battery");
    let refresh = {
        let (img, b) = (img.clone(), b.clone());
        move || {
            if let Some(bt) = sys::battery() {
                if let Some(p) = util::pixbuf(&sys::battery_icon(&bt), 16) { img.set_from_pixbuf(Some(&p)); }
                b.set_tooltip_text(Some(&format!("{}% {}", bt.percent, if bt.status == "Charging" { "available (plugged in, charging)" } else { "remaining" })));
            }
        }
    };
    refresh();
    glib::timeout_add_seconds_local(20, move || { refresh(); glib::ControlFlow::Continue });
    b.connect_clicked(|b| {
        if close_open() { return; }
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        if let Some(bt) = sys::battery() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.pack_start(&util::image(&sys::battery_icon(&bt), 32), false, false, 0);
            row.pack_start(&gtk::Label::new(Some(&format!("{}% {}", bt.percent, bt.status.to_lowercase()))), false, false, 0);
            body.pack_start(&row, false, false, 0);
        }
        if let Some(cur) = sys::power_profile() {
            let h = gtk::Label::new(Some("Select a power plan:"));
            h.set_halign(gtk::Align::Start);
            body.pack_start(&h, false, false, 0);
            let mut group: Option<gtk::RadioButton> = None;
            for (id, name) in [("balanced", "Balanced"), ("power-saver", "Power saver"), ("performance", "High performance")] {
                let r = gtk::RadioButton::with_label(name);
                if let Some(g) = &group { r.join_group(Some(g)); } else { group = Some(r.clone()); }
                r.set_active(cur == id);
                r.connect_toggled(move |r| if r.is_active() { sys::set_power_profile(id) });
                body.pack_start(&r, false, false, 0);
            }
        }
        if let Some(br) = sys::backlight() {
            let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
            s.set_value(br as f64);
            s.connect_value_changed(|s| { util::spawn(&["brightnessctl", "set", &format!("{}%", s.value() as u32)]); });
            body.pack_start(&gtk::Label::new(Some("Screen brightness")), false, false, 0);
            body.pack_start(&s, false, false, 0);
        }
        let foot = link("More power options", || control("power"));
        popup(b.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
    });
}

// ---------------------------------------------------------------- brightness (DDC/CI)
/// Tray button for external-monitor brightness over DDC/CI (ddcutil).
/// Detection is slow, so it runs in the background and the button appears when ready.
fn brightness(tb: &Rc<Taskbar>) {
    let tray = tb.tray.clone();
    let tb2 = tb.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(sys::ddc_displays()); });
    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        let displays = match rx.try_recv() { Ok(d) => d, Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue, Err(_) => return glib::ControlFlow::Break };
        if displays.is_empty() { return glib::ControlFlow::Break; }
        let (b, _img) = tray_button(&tb2, util::first_icon(&["display-brightness", "display-brightness-symbolic", "preferences-desktop-display"]), "Screen brightness");
        // keep it to the left of the volume/network icons
        tray.reorder_child(&b, 0);
        b.show_all();
        let displays = Rc::new(displays);
        b.connect_clicked(move |b| {
            if close_open() { return; }
            let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
            body.set_size_request(260, -1);
            let h = gtk::Label::new(Some("Adjust screen brightness"));
            h.style_context().add_class("fly-link");
            h.set_halign(gtk::Align::Start);
            body.pack_start(&h, false, false, 0);
            for (n, model) in displays.iter() {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                let l = gtk::Label::new(Some(&format!("{n}. {model}")));
                l.set_xalign(0.0);
                row.pack_start(&l, false, false, 0);
                let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
                s.set_sensitive(false);
                s.set_value_pos(gtk::PositionType::Right);
                row.pack_start(&s, false, false, 0);
                body.pack_start(&row, false, false, 0);
                // read the current value in the background
                let (tx, rx) = std::sync::mpsc::channel();
                let n = *n;
                std::thread::spawn(move || { let _ = tx.send(sys::ddc_get(n)); });
                let s2 = s.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(100), move || match rx.try_recv() {
                    Ok(v) => { if let Some((cur, max)) = v { s2.set_value((cur * 100 / max.max(1)) as f64); s2.set_sensitive(true); } glib::ControlFlow::Break }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(_) => glib::ControlFlow::Break,
                });
                // debounce: DDC is slow, only send the value once the slider rests
                let pending: Rc<std::cell::Cell<Option<glib::SourceId>>> = Rc::new(std::cell::Cell::new(None));
                s.connect_value_changed(move |s| {
                    if let Some(id) = pending.take() { id.remove(); }
                    let v = s.value() as u32;
                    let p2 = pending.clone();
                    pending.set(Some(glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || { p2.set(None); sys::ddc_set(n, v); })));
                });
            }
            if displays.len() > 1 {
                let all = gtk::Button::with_label("Set all monitors to the same level");
                all.style_context().add_class("link");
                let body2 = body.clone();
                all.connect_clicked(move |_| {
                    let mut first: Option<f64> = None;
                    for row in body2.children() {
                        if let Some(sc) = row.downcast_ref::<gtk::Box>().and_then(|r| r.children().into_iter().find_map(|c| c.downcast::<gtk::Scale>().ok())) {
                            match first { None => first = Some(sc.value()), Some(v) => sc.set_value(v) }
                        }
                    }
                });
                body.pack_start(&all, false, false, 0);
            }
            let foot = link("Adjust display settings", || control("display"));
            popup(b.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
        });
        glib::ControlFlow::Break
    });
}
