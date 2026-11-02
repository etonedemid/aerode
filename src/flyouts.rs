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
    /// set while an icon is dragged out of / into a flyout (the drag grab takes the focus)
    static KEEP_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Keep the open flyout open when it loses focus (during drag and drop).
pub fn keep_open(on: bool) {
    KEEP_OPEN.with(|k| k.set(on));
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
    w.connect_focus_out_event(|w, _| {
        if !KEEP_OPEN.with(|k| k.get()) { closed_by_itself(w); }
        glib::Propagation::Proceed
    });
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
    // Win7 fades the flyout in (and closes it at once)
    crate::anim::self_animated(&w);
    let animate = crate::anim::effect("animate_taskbar");
    if animate { w.set_opacity(0.0); }
    w.show_all();
    w.present();
    crate::anim::fade(&w, 1.0, crate::anim::FLYOUT_IN, animate, |_| {});
    // the icon's tooltip would pop up over its own flyout (Win7 suppresses it while open)
    let had_tip = anchor.has_tooltip();
    anchor.set_has_tooltip(false);
    { let a = anchor.clone(); w.connect_destroy(move |_| a.set_has_tooltip(had_tip)); }
    OPEN.with(|o| *o.borrow_mut() = Some(w.clone()));
    w
}

/// A right-click menu on a notification-area icon (Win7: every system icon has one). The press
/// is consumed so it doesn't also open the taskbar's own menu.
pub(crate) fn tray_menu(b: &gtk::Button, items: Rc<dyn Fn() -> Vec<(String, Option<Rc<dyn Fn()>>)>>) {
    b.connect_button_press_event(move |b, e| {
        if e.button() != 3 { return glib::Propagation::Proceed; }
        close_open();
        let m = build_menu(items());
        // no tooltip over the open menu (Win7 hides it)
        b.set_has_tooltip(false);
        let bb = b.clone();
        m.connect_hide(move |_| bb.set_has_tooltip(true));
        m.popup_at_widget(b, gdk::Gravity::North, gdk::Gravity::South, Some(e));
        glib::Propagation::Stop
    });
}

/// A popup menu from (mnemonic label, action) items: "" is a separator, no action greys the item
/// out, and a leading ✓ makes it a checked item.
pub(crate) fn build_menu(items: Vec<(String, Option<Rc<dyn Fn()>>)>) -> gtk::Menu {
    let m = gtk::Menu::new();
    for (label, act) in items {
        if label.is_empty() { m.append(&gtk::SeparatorMenuItem::new()); continue; }
        let it: gtk::MenuItem = match label.strip_prefix('\u{2713}') {
            Some(l) => { let c = gtk::CheckMenuItem::with_mnemonic(l); c.set_active(true); c.upcast() }
            None => gtk::MenuItem::with_mnemonic(&label),
        };
        match act {
            Some(f) => { it.connect_activate(move |_| f()); }
            None => it.set_sensitive(false),
        }
        m.append(&it);
    }
    m.show_all();
    m
}

fn act(f: impl Fn() + 'static) -> Option<Rc<dyn Fn()>> { Some(Rc::new(f)) }

pub(crate) fn tray_button(tb: &Rc<Taskbar>, icon: &str, tip: &str) -> (gtk::Button, gtk::Image) {
    let b = gtk::Button::new();
    crate::tray::themed_tray_button(&b);
    let img = util::image(icon, 16);
    b.add(&img);
    b.set_tooltip_text(Some(tip));
    tb.tray.pack_end(&b, false, false, 0);
    (b, img)
}

pub(crate) fn link(text: &str, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.style_context().add_class("link");
    b.set_halign(gtk::Align::Start);
    b.connect_clicked(move |_| { close_open(); f(); });
    b
}

pub(crate) fn control(page: &str) {
    if !util::spawn(&["aerode-control", page]) {
        util::error("Control Panel is not installed.", "");
    }
}

pub fn build_tray(tb: &Rc<Taskbar>) {
    // packed from the right: clock, then the system icons in Win7's order (Action Center, power, network, volume)
    clock(tb);
    volume(tb);
    network(tb);
    if sys::battery().is_some() {
        battery(tb);
    }
    crate::actioncenter::attach(tb);
    // programs' icons and the "Show hidden icons" arrow
    crate::tray::attach(tb);
    brightness();
    // the docked language bar sits just left of the notification area
    crate::kbdlayout::attach(&tb.tray);
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
    // Win7: the clock's menu is the taskbar's, with the date and notification-icon entries on top
    {
        let tbw = Rc::downgrade(tb);
        tray_menu(&b, Rc::new(move || {
            let mut v: Vec<(String, Option<Rc<dyn Fn()>>)> = vec![
                ("A_djust date/time".into(), act(|| control("datetime"))),
                ("_Customize notification icons".into(), act(|| { crate::tray::customize(); })),
                (String::new(), None),
            ];
            if let Some(tb) = tbw.upgrade() { v.extend(tb.menu_items()); }
            v
        }));
    }
    // Date and Time > Additional Clocks: (name, offset east of UTC) of the shown ones, refreshed
    // once a minute (offsets change with daylight saving time)
    let extra: Rc<RefCell<Vec<(String, i32)>>> = Rc::new(RefCell::new(vec![]));
    let refresh_extra = {
        let extra = extra.clone();
        move || { *extra.borrow_mut() = extra_clocks(); }
    };
    refresh_extra();
    let update = {
        let (t, d, b, extra) = (t.clone(), d.clone(), b.clone(), extra.clone());
        let last = std::cell::RefCell::new(String::new());
        move || {
            let now = chrono::Local::now();
            let h24 = config::get().clock_24h;
            let time = if h24 { now.format("%H:%M").to_string() } else { now.format("%-I:%M %p").to_string() };
            // only touch the labels when the minute changes (no relayout every second)
            if *last.borrow() == time { return; }
            t.set_text(&time);
            *last.borrow_mut() = time;
            d.set_text(&now.format("%-m/%-d/%Y").to_string());
            b.set_tooltip_text(Some(&clock_tooltip(&extra.borrow(), h24)));
        }
    };
    update();
    let ticks = std::cell::Cell::new(0u32);
    // Date and Time > "Notify me when the clock changes": a balloon when daylight saving time starts or ends
    let last_off = std::cell::Cell::new((crate::tzinfo::local_zone(), chrono::Local::now().offset().local_minus_utc()));
    glib::timeout_add_seconds_local(1, move || {
        ticks.set(ticks.get() + 1);
        if ticks.get() % 60 == 0 {
            refresh_extra();
            let now = (crate::tzinfo::local_zone(), chrono::Local::now().offset().local_minus_utc());
            let before = last_off.replace(now.clone());
            // a different zone is the user's own change, not the clock changing
            if before.0 == now.0 && before.1 != now.1 && config::get().hw.dst_notify { dst_balloon(now.1 - before.1); }
        }
        update();
        glib::ControlFlow::Continue
    });
    b.connect_clicked(move |b| {
        if close_open() { return; }
        // the clocks may have just been changed in Control Panel
        *extra.borrow_mut() = extra_clocks();
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
        let pb = |n: &str| config::data_file(&format!("clock/{n}.png")).and_then(|p| gtk::gdk_pixbuf::Pixbuf::from_file(p).ok());
        let clocks = extra.borrow().clone();
        // Win7: one big clock alone; with additional clocks, the local one and the others side by side
        let (size, dial, glassov) = if clocks.is_empty() { (130, pb("5000"), pb("5001")) } else { (104, pb("5002"), pb("5003")) };
        let mut faces: Vec<(gtk::DrawingArea, gtk::Label, Option<i32>)> = vec![];
        let all: Vec<(String, Option<i32>)> = std::iter::once(("Local time".to_string(), None)).chain(clocks.iter().map(|(n, o)| (n.clone(), Some(*o)))).collect();
        let many = all.len() > 1;
        for (name, off) in all {
            let right = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let face = gtk::DrawingArea::new();
            face.set_size_request(size, size);
            let (dial, glassov) = (dial.clone(), glassov.clone());
            face.connect_draw(move |_, cr| { draw_clock_at(cr, size as f64, dial.as_ref(), glassov.as_ref(), clock_time(off)); glib::Propagation::Proceed });
            right.pack_start(&face, false, false, 0);
            if many {
                let n = gtk::Label::new(Some(&name));
                n.set_max_width_chars(14);
                n.set_ellipsize(gtk::pango::EllipsizeMode::End);
                right.pack_start(&n, false, false, 0);
            }
            let digital = gtk::Label::new(Some(&clock_text(off, many)));
            digital.style_context().add_class("fly-time");
            right.pack_start(&digital, false, false, 0);
            row.pack_start(&right, false, false, 0);
            faces.push((face, digital, off));
        }
        body.pack_start(&row, false, false, 0);
        {
            glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
                if !faces[0].0.is_mapped() { return glib::ControlFlow::Break; }
                for (face, digital, off) in &faces {
                    face.queue_draw();
                    digital.set_text(&clock_text(*off, many));
                }
                glib::ControlFlow::Continue
            });
        }
        let foot = link("Change date and time settings...", || control("datetime"));
        popup(b.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
    });
}

/// The daylight saving time balloon (through our own notification daemon on the session bus).
fn dst_balloon(delta: i32) {
    let amount = if delta.abs() % 3600 == 0 { format!("{} hour{}", delta.abs() / 3600, if delta.abs() == 3600 { "" } else { "s" }) } else { format!("{} minutes", delta.abs() / 60) };
    let body = if delta < 0 { format!("Daylight Saving Time has ended. Your clock was set back {amount}.") } else { format!("Daylight Saving Time has begun. Your clock was set forward {amount}.") };
    util::spawn(&["gdbus", "call", "--session", "--dest", "org.freedesktop.Notifications", "--object-path", "/org/freedesktop/Notifications",
        "--method", "org.freedesktop.Notifications.Notify", "Date and Time", "0", "preferences-system-time", "The clock has changed", &body, "[]", "{}", "15000"]);
}

/// The additional clocks to show: (display name, current offset east of UTC in seconds).
fn extra_clocks() -> Vec<(String, i32)> {
    config::get().hw.clocks.iter().filter(|c| c.show && !c.tz.is_empty())
        .map(|c| (if c.name.is_empty() { "Clock".to_string() } else { c.name.clone() }, crate::tzinfo::offset_now(&c.tz))).collect()
}

/// Wall time of a clock: local time (None) or UTC + offset.
fn clock_time(off: Option<i32>) -> chrono::NaiveDateTime {
    match off {
        None => chrono::Local::now().naive_local(),
        Some(o) => chrono::Utc::now().naive_utc() + chrono::Duration::seconds(o as i64),
    }
}

/// Text under a clock: "10:30:15 AM", or with several clocks "Today 10:30 AM" / "Tomorrow 2:30 AM".
fn clock_text(off: Option<i32>, many: bool) -> String {
    let t = clock_time(off);
    if !many { return t.format("%-I:%M:%S %p").to_string(); }
    let today = chrono::Local::now().date_naive();
    let day = match (t.date() - today).num_days() { 0 => "Today".to_string(), 1 => "Tomorrow".to_string(), -1 => "Yesterday".to_string(), _ => t.format("%A").to_string() };
    format!("{day} {}", t.format("%-I:%M %p"))
}

/// The taskbar clock's tooltip: the long date, and with additional clocks every clock's time.
fn clock_tooltip(extra: &[(String, i32)], h24: bool) -> String {
    let now = chrono::Local::now();
    if extra.is_empty() { return now.format("%A, %B %-d, %Y").to_string(); }
    let fmt = if h24 { "%H:%M" } else { "%-I:%M %p" };
    let mut s = format!("{}\nLocal time: {}", now.format("%A, %B %-d, %Y"), now.format(fmt));
    for (name, off) in extra {
        let t = clock_time(Some(*off));
        s += &format!("\n{name}: {} {}", t.format("%A"), t.format(fmt));
    }
    s
}

/// Analog clock: Win7 dial + glass overlay with hands drawn in between (local time).
pub fn draw_clock(cr: &gtk::cairo::Context, size: f64, dial: Option<&gtk::gdk_pixbuf::Pixbuf>, glass_ov: Option<&gtk::gdk_pixbuf::Pixbuf>) {
    draw_clock_at(cr, size, dial, glass_ov, chrono::Local::now().naive_local());
}

/// The same for any time (other time zones, Date and Time Settings while editing). The dial and
/// glass images are scaled to `size`.
pub fn draw_clock_at(cr: &gtk::cairo::Context, size: f64, dial: Option<&gtk::gdk_pixbuf::Pixbuf>, glass_ov: Option<&gtk::gdk_pixbuf::Pixbuf>, t: chrono::NaiveDateTime) {
    use chrono::Timelike;
    use gtk::gdk::prelude::GdkContextExt;
    let c = size / 2.0;
    let paint = |p: &gtk::gdk_pixbuf::Pixbuf| {
        cr.save().ok();
        let k = size / p.width().max(1) as f64;
        cr.scale(k, k);
        cr.set_source_pixbuf(p, 0.0, 0.0);
        cr.paint().ok();
        cr.restore().ok();
    };
    if let Some(d) = dial { paint(d); } else {
        cr.arc(c, c, c - 2.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(0.97, 0.97, 0.98);
        cr.fill_preserve().ok();
        cr.set_source_rgb(0.6, 0.62, 0.66);
        cr.stroke().ok();
    }
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
    // hand widths follow the dial size (they were made for the 130 px one)
    let k = (size / 130.0).max(0.6);
    hand(tau * (h as f64 + m as f64 / 60.0) / 12.0, size * 0.25, 3.5 * k, (0.15, 0.16, 0.18, 0.95));
    hand(tau * (m as f64 + s as f64 / 60.0) / 60.0, size * 0.36, 2.5 * k, (0.15, 0.16, 0.18, 0.95));
    hand(tau * s as f64 / 60.0, size * 0.38, 1.0, (0.75, 0.10, 0.10, 0.95));
    if let Some(g) = glass_ov { paint(g); }
}

// ---------------------------------------------------------------- volume
// the speaker icon, its flyout and context menu: src/flyouts/volume.rs
mod volume;
use volume::volume;

// ---------------------------------------------------------------- network
fn network(tb: &Rc<Taskbar>) {
    let (b, img) = tray_button(tb, "network-wired", "Network");
    tray_menu(&b, Rc::new(|| vec![
        ("_Troubleshoot problems".into(), act(|| control("troubleshooting"))),
        ("Open Network and _Sharing Center".into(), act(|| control("network"))),
    ]));
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
    // NetworkManager reports connection changes; Wi-Fi signal strength still needs the slow poll
    let r = refresh.clone();
    let events = util::watch_lines(&["nmcli", "monitor"], |_| true, std::time::Duration::from_millis(300), move || r());
    { let r = refresh.clone(); glib::timeout_add_seconds_local(if events { 30 } else { 5 }, move || { r(); glib::ControlFlow::Continue }); }
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
    tray_menu(&b, Rc::new(|| vec![
        ("P_ower Options".into(), act(|| control("power"))),
        ("Windows _Mobility Center".into(), act(|| control("mobility"))),
    ]));
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
/// External-monitor brightness over DDC/CI (ddcutil) as a notification-area icon; like any new
/// Win7 icon it starts out in the overflow (Customize... can put it on the taskbar).
/// Detection is slow, so it runs in the background and the icon appears when ready.
fn brightness() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(sys::ddc_displays()); });
    glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
        let displays = match rx.try_recv() { Ok(d) => d, Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue, Err(_) => return glib::ControlFlow::Break };
        if displays.is_empty() { return glib::ControlFlow::Break; }
        let displays = Rc::new(displays);
        let icon = util::first_icon(&["display-brightness", "display-brightness-symbolic", "preferences-desktop-display"]);
        crate::tray::add_builtin("aerode-brightness", "Screen brightness", icon, "Screen brightness", move |anchor| {
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
            popup(anchor, body.upcast_ref(), Some(foot.upcast_ref()));
        });
        glib::ControlFlow::Break
    });
}
