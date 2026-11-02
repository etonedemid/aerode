//! Display: "Make it easier to read what's on your screen" (text size = Xft.dpi 96/120/144, custom
//! DPI) and Screen Resolution: the monitor arrangement (drag the numbered screens, Identify,
//! Detect), resolution, orientation, multiple displays, main display, and the Advanced settings
//! (adapter, Monitor tab with the screen refresh rate, List All Modes). Applied with xrandr (always
//! with --rate, so high refresh rates stick), kept for the next logon in ~/.config/aerode/displays.sh,
//! with Win7's "Do you want to keep these display settings?" and its 15 second revert.
use super::*;
use super::super::admin::{columns, img, inform, scrolled, spawn_tool};
use super::super::desktop::{check, combo, group, radios, write_xsettings};
use std::cell::Cell;

fn vbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, s) }
fn hbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, s) }

// ------------------------------------------------------------------ text size (the Display page)

fn side(cp: &Rc<Cp>) {
    cp.side_link("Adjust resolution", "resolution");
    { let cp2 = cp.clone(); super::super::admin::side_action(cp, "Calibrate color", move || calibrate(&cp2)); }
    cp.side_link("Change display settings", "resolution");
    { let cp2 = cp.clone(); super::super::admin::side_action(cp, "Connect to a projector", move || display_switch(&cpwin(&cp2))); }
    { let cp2 = cp.clone(); super::super::admin::side_action(cp, "Adjust ClearType text", move || super::super::desktop::cleartype_tuner(&cp2)); }
    { let cp2 = cp.clone(); super::super::admin::side_action(cp, "Set custom text size (DPI)", move || { if let Some(d) = custom_dpi(&cpwin(&cp2), config::get().hw.dpi) { set_dpi(&cp2, d); } }); }
    super::super::admin::see_also(cp, &[("Personalization", "personalization"), ("Devices and Printers", "devices")]);
}

/// Win7 runs dccw.exe; here the colour calibration tools when installed, else Color Management.
fn calibrate(cp: &Rc<Cp>) {
    if !(util::spawn(&["displaycal"]) || util::spawn(&["gcm-calibrate"])) { cp.open("colormgmt"); }
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    side(cp);
    let v = vbox(8);
    v.pack_start(&title("Make it easier to read what's on your screen"), false, false, 0);
    let intro = text("You can change the size of text and other items on your screen by choosing one of these options. To temporarily enlarge just part of the screen, use the Magnifier tool.");
    v.pack_start(&intro, false, false, 0);
    let cur = config::get().hw.dpi.max(72);
    let row = hbox(30);
    let col = vbox(14);
    col.set_margin_top(10);
    col.set_margin_start(10);
    let geo = util::primary_geometry();
    // Win7 offers "Larger" only on screens of at least 1200 × 900
    let mut opts: Vec<(u32, String)> = vec![(96, "S_maller - 100% (default)".into()), (120, "_Medium - 125%".into())];
    if (geo.width() >= 1200 && geo.height() >= 900) || cur == 144 { opts.push((144, "_Larger - 150%".into())); }
    if !opts.iter().any(|o| o.0 == cur) { opts.push((cur, format!("_Custom - {}%", cur * 100 / 96))); }
    let labels: Vec<&str> = opts.iter().map(|o| o.1.as_str()).collect();
    let rs = radios(&labels, opts.iter().position(|o| o.0 == cur).unwrap_or(0));
    for r in &rs { col.pack_start(r, false, false, 0); }
    row.pack_start(&col, false, false, 0);
    let chosen = Rc::new(Cell::new(cur));
    let preview = gtk::DrawingArea::new();
    preview.set_size_request(280, 180);
    { let c = chosen.clone(); preview.connect_draw(move |w, cr| { draw_preview(w, cr, c.get()); glib::Propagation::Stop }); }
    let pf = gtk::Frame::new(None);
    pf.add(&preview);
    row.pack_start(&pf, false, false, 0);
    v.pack_start(&row, false, false, 0);
    let apply = gtk::Button::with_mnemonic("_Apply");
    apply.set_size_request(75, -1);
    apply.set_halign(gtk::Align::End);
    apply.set_sensitive(false);
    for (r, (dpi, _)) in rs.iter().zip(opts.iter()) {
        let (c, p, a, dpi) = (chosen.clone(), preview.clone(), apply.clone(), *dpi);
        r.connect_toggled(move |r| if r.is_active() { c.set(dpi); p.queue_draw(); a.set_sensitive(dpi != config::get().hw.dpi); });
    }
    { let (cp, c) = (cp.clone(), chosen.clone()); apply.connect_clicked(move |a| { a.set_sensitive(false); set_dpi(&cp, c.get()); }); }
    v.pack_start(&apply, false, false, 8);
    v.upcast()
}

/// A little window with a title bar and some text at the chosen size.
fn draw_preview(w: &gtk::DrawingArea, cr: &gtk::cairo::Context, dpi: u32) {
    let (wd, ht) = (w.allocated_width() as f64, w.allocated_height() as f64);
    cr.set_source_rgb(0.92, 0.95, 0.98);
    cr.paint().ok();
    let s = dpi as f64 / 96.0;
    let (x, y, ww, hh) = (16.0, 16.0, wd - 10.0, ht);
    // window: glass title bar, white client
    let g = gtk::cairo::LinearGradient::new(0.0, y, 0.0, y + 22.0 * s);
    g.add_color_stop_rgb(0.0, 0.73, 0.84, 0.94);
    g.add_color_stop_rgb(1.0, 0.56, 0.71, 0.86);
    cr.rectangle(x, y, ww, hh);
    cr.set_source(&g).ok();
    cr.fill().ok();
    cr.rectangle(x + 6.0 * s, y + 22.0 * s, ww, hh);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.fill().ok();
    cr.rectangle(x + 0.5, y + 0.5, ww, hh);
    cr.set_source_rgb(0.35, 0.45, 0.58);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    let font = |sz: f64| { let mut f = gtk::pango::FontDescription::from_string("Segoe UI"); f.set_absolute_size(sz * gtk::pango::SCALE as f64); f };
    let l = w.create_pango_layout(Some("Preview"));
    l.set_font_description(Some(&font(12.0 * s)));
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.move_to(x + 8.0 * s, y + 3.0 * s);
    pangocairo::functions::show_layout(cr, &l);
    let l = w.create_pango_layout(Some("Text and other items on your screen get bigger. Windows, menus and icons grow with them so everything stays easy to read."));
    l.set_font_description(Some(&font(12.0 * s)));
    l.set_width(((ww - 20.0 * s) * gtk::pango::SCALE as f64) as i32);
    l.set_wrap(gtk::pango::WrapMode::Word);
    cr.move_to(x + 14.0 * s, y + 30.0 * s);
    pangocairo::functions::show_layout(cr, &l);
}

/// Save the text size and ask to log off (programs read it when they start).
fn set_dpi(cp: &Rc<Cp>, dpi: u32) {
    config::update(|c| c.hw.dpi = dpi);
    write_xsettings();
    let d = gtk::MessageDialog::new(Some(&cpwin(cp)), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::None,
        "You must log off your computer to apply these changes");
    d.set_title("Microsoft Windows");
    d.set_secondary_text(Some("Save any open files and close all programs before you log off."));
    d.add_button("_Log off now", gtk::ResponseType::Yes);
    d.add_button("Log off _later", gtk::ResponseType::No);
    if d.run() == gtk::ResponseType::Yes { aerode::sys::log_off(); }
    d.close();
}

/// Win7 "Custom DPI Setting": a percentage list and a ruler you can drag.
fn custom_dpi(parent: &gtk::Window, cur: u32) -> Option<u32> {
    let d = ok_cancel(Some(parent), "Custom DPI Setting");
    let c = d.content_area();
    c.pack_start(&wrap("For a custom DPI setting, select a percentage from the list, or drag the ruler with your mouse.", 50), false, false, 0);
    let row = hbox(8);
    row.pack_start(&lbl("_Scale to this percentage of normal size:"), false, false, 0);
    let pct = gtk::ComboBoxText::with_entry();
    for p in ["100%", "125%", "150%", "200%"] { pct.append_text(p); }
    let cur_pct = Rc::new(Cell::new(cur * 100 / 96));
    if let Some(e) = pct.child().and_then(|e| e.downcast::<gtk::Entry>().ok()) { e.set_text(&format!("{}%", cur_pct.get())); e.set_width_chars(6); }
    row.pack_start(&pct, false, false, 0);
    c.pack_start(&row, false, false, 0);
    let ruler = gtk::DrawingArea::new();
    ruler.set_size_request(420, 60);
    ruler.add_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK | gtk::gdk::EventMask::BUTTON1_MOTION_MASK);
    { let p = cur_pct.clone(); ruler.connect_draw(move |w, cr| {
        let (wd, ht) = (w.allocated_width() as f64, w.allocated_height() as f64);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok();
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.set_line_width(1.0);
        let inch = 96.0 * p.get() as f64 / 100.0;
        let mut i = 0;
        loop {
            let x = 4.5 + i as f64 * inch / 10.0;
            if x > wd { break; }
            let len = if i % 10 == 0 { ht * 0.55 } else if i % 5 == 0 { ht * 0.35 } else { ht * 0.2 };
            cr.move_to(x, 0.0);
            cr.line_to(x, len);
            if i % 10 == 0 && i > 0 {
                let l = w.create_pango_layout(Some(&(i / 10).to_string()));
                cr.move_to(x - 3.0, len + 2.0);
                pangocairo::functions::show_layout(cr, &l);
            }
            i += 1;
        }
        cr.stroke().ok();
        glib::Propagation::Stop
    }); }
    let sample = lbl("");
    let upd = { let (sample, p, r) = (sample.clone(), cur_pct.clone(), ruler.clone()); Rc::new(move || { sample.set_text(&format!("9 point Segoe UI at {} pixels per inch.", 96 * p.get() / 100)); r.queue_draw(); }) };
    upd();
    { let (p, u, pct) = (cur_pct.clone(), upd.clone(), pct.clone()); let drag = move |x: f64| {
        // the "1" mark under the pointer sets the size of an inch
        let v = ((x - 4.0) / 96.0 * 100.0).round().clamp(100.0, 500.0) as u32;
        p.set(v);
        if let Some(e) = pct.child().and_then(|e| e.downcast::<gtk::Entry>().ok()) { e.set_text(&format!("{v}%")); }
        u();
    }; let d2 = drag.clone(); ruler.connect_button_press_event(move |_, e| { d2(e.position().0); glib::Propagation::Stop }); ruler.connect_motion_notify_event(move |_, e| { drag(e.position().0); glib::Propagation::Stop }); }
    { let (p, u) = (cur_pct.clone(), upd.clone()); pct.connect_changed(move |c| {
        if let Some(t) = c.active_text() { if let Ok(v) = t.trim().trim_end_matches('%').parse::<u32>() { if (100..=500).contains(&v) { p.set(v); u(); } } }
    }); }
    let f = gtk::Frame::new(None);
    f.add(&ruler);
    c.pack_start(&f, false, false, 0);
    c.pack_start(&sample, false, false, 0);
    let xp = check("_Use Windows XP style DPI scaling", false);
    xp.set_sensitive(false);
    c.pack_start(&xp, false, false, 0);
    d.show_all();
    let r = d.run();
    d.close();
    (r == gtk::ResponseType::Ok).then(|| (96 * cur_pct.get() / 100).max(72))
}

// ------------------------------------------------------------------ xrandr model

#[derive(Clone, Debug, Default, PartialEq)]
struct Mode { name: String, w: i32, h: i32, rates: Vec<String> }

#[derive(Clone, Debug, Default)]
struct Out {
    name: String,
    /// EDID monitor name ("Generic PnP Monitor" without one, like Windows)
    monitor: String,
    primary: bool,
    modes: Vec<Mode>,
    preferred: String,
    /// current mode name and rate ("" when the output is off)
    cur: String,
    rate: String,
    x: i32,
    y: i32,
    rot: String,
}

/// What the page will apply for one output.
#[derive(Clone, Debug, PartialEq)]
struct Pending { on: bool, mode: String, rate: String, rot: String, x: i32, y: i32, primary: bool }

fn edid_name(hex: &str) -> Option<String> {
    let b: Vec<u8> = (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
    for off in [54usize, 72, 90, 108] {
        let d = b.get(off..off + 18)?;
        if d[0] == 0 && d[1] == 0 && d[2] == 0 && d[3] == 0xFC {
            let s: String = d[5..].iter().take_while(|c| **c != b'\n' && **c != 0).map(|c| *c as char).collect();
            let s = s.trim().to_string();
            if !s.is_empty() { return Some(s); }
        }
    }
    None
}

fn parse(text: &str) -> Vec<Out> {
    let mut outs: Vec<Out> = vec![];
    let mut connected = false;
    let mut edid: Option<String> = None;
    for l in text.lines() {
        if !l.starts_with(char::is_whitespace) {
            if let (Some(o), Some(e)) = (outs.last_mut(), edid.take()) { if let Some(n) = edid_name(&e) { o.monitor = n; } }
            connected = l.contains(" connected");
            if !connected { continue; }
            let w: Vec<&str> = l.split_whitespace().collect();
            let mut o = Out { name: w[0].to_string(), monitor: "Generic PnP Monitor".into(), primary: w.contains(&"primary"), rot: "normal".into(), ..Default::default() };
            if let Some(i) = w.iter().position(|t| t.contains('x') && t.contains('+') && t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)) {
                let p: Vec<&str> = w[i].split('+').collect();
                o.x = p.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                o.y = p.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
                if let Some(r) = w.get(i + 1).filter(|r| ["normal", "left", "inverted", "right"].contains(r)) { o.rot = r.to_string(); }
            }
            outs.push(o);
        } else if !connected {
        } else if l.trim_start().starts_with("EDID:") {
            edid = Some(String::new());
        } else if let Some(e) = edid.as_mut().filter(|_| l.starts_with("\t\t") && l.trim().chars().all(|c| c.is_ascii_hexdigit())) {
            e.push_str(l.trim());
        } else if l.starts_with("   ") && !l.starts_with("\t") {
            if let Some(e) = edid.take() { if let (Some(o), Some(n)) = (outs.last_mut(), edid_name(&e)) { o.monitor = n; } }
            let Some(o) = outs.last_mut() else { continue };
            let mut it = l.split_whitespace();
            let Some(name) = it.next() else { continue };
            let Some((w, h)) = name.split_once('x') else { continue };
            let h = h.trim_end_matches(|c: char| !c.is_ascii_digit());
            let (Ok(w), Ok(h)) = (w.parse::<i32>(), h.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("").parse::<i32>()) else { continue };
            if name.ends_with('i') { continue; } // interlaced
            let mut rates: Vec<String> = vec![];
            for t in it {
                if t == "+" { if rates.last().is_some() && o.preferred.is_empty() { o.preferred = name.to_string(); } continue; }
                let r = t.trim_end_matches(['*', '+']).to_string();
                if r.parse::<f64>().map(|v| v <= 0.0).unwrap_or(true) && r != "0.00" { continue; }
                if t.contains('*') { o.cur = name.to_string(); o.rate = r.clone(); }
                if t.contains('+') && o.preferred.is_empty() { o.preferred = name.to_string(); }
                if !rates.contains(&r) { rates.push(r); }
            }
            rates.sort_by(|a, b| b.parse::<f64>().unwrap_or(0.0).partial_cmp(&a.parse::<f64>().unwrap_or(0.0)).unwrap());
            match o.modes.iter_mut().find(|m| m.name == name) {
                Some(m) => for r in rates { if !m.rates.contains(&r) { m.rates.push(r); } },
                None => o.modes.push(Mode { name: name.to_string(), w, h, rates }),
            }
        }
    }
    if let (Some(o), Some(e)) = (outs.last_mut(), edid.take()) { if let Some(n) = edid_name(&e) { o.monitor = n; } }
    for o in outs.iter_mut() { if o.preferred.is_empty() { o.preferred = o.modes.first().map(|m| m.name.clone()).unwrap_or_default(); } }
    outs
}

/// Ask xrandr off the GTK thread (AERODE_XRANDR_FILE replays a saved `xrandr --prop` for testing).
fn query(done: impl FnOnce(Vec<Out>) + 'static) {
    if let Ok(f) = std::env::var("AERODE_XRANDR_FILE") { let t = std::fs::read_to_string(f).unwrap_or_default(); glib::idle_add_local_once(move || done(parse(&t))); return; }
    util::output_async(vec!["xrandr".into(), "--prop".into()], move |o| done(parse(&o.unwrap_or_default())));
}

fn pending_of(o: &Out) -> Pending {
    Pending { on: !o.cur.is_empty(), mode: if o.cur.is_empty() { o.preferred.clone() } else { o.cur.clone() }, rate: o.rate.clone(), rot: o.rot.clone(), x: o.x, y: o.y, primary: o.primary }
}

fn mode_size(o: &Out, mode: &str) -> (i32, i32) { o.modes.iter().find(|m| m.name == mode).map(|m| (m.w, m.h)).unwrap_or((1024, 768)) }

/// Size on the desktop (rotated outputs swap width and height).
fn rect_size(o: &Out, p: &Pending) -> (i32, i32) {
    let (w, h) = mode_size(o, &p.mode);
    if p.rot == "left" || p.rot == "right" { (h, w) } else { (w, h) }
}

/// "60 Hertz"; when two rates round alike (59.94 and 60.00) the lower one is shown as 59, like Windows.
fn hertz(rate: &str, all: &[String]) -> String {
    let v: f64 = rate.parse().unwrap_or(0.0);
    let twin = all.iter().any(|r| { let o: f64 = r.parse().unwrap_or(0.0); o > v && o.round() == v.round() });
    format!("{} Hertz", if twin { v.floor() } else { v.round() } as i64)
}

fn xrandr_cmd(outs: &[Out], pend: &[Pending]) -> Vec<String> {
    let mut cmd = vec!["xrandr".to_string()];
    let minx = pend.iter().filter(|p| p.on).map(|p| p.x).min().unwrap_or(0);
    let miny = pend.iter().filter(|p| p.on).map(|p| p.y).min().unwrap_or(0);
    for (o, p) in outs.iter().zip(pend) {
        cmd.extend(["--output".into(), o.name.clone()]);
        if !p.on { cmd.push("--off".into()); continue; }
        cmd.extend(["--mode".into(), p.mode.clone()]);
        // always pass the rate: without it xrandr may fall back to 60 Hz
        if !p.rate.is_empty() { cmd.extend(["--rate".into(), p.rate.clone()]); }
        cmd.extend(["--rotate".into(), p.rot.clone(), "--pos".into(), format!("{}x{}", p.x - minx, p.y - miny)]);
        if p.primary { cmd.push("--primary".into()); }
    }
    cmd
}

fn run_xrandr(cmd: &[String]) {
    if std::env::var_os("AERODE_XRANDR_FILE").is_some() { eprintln!("aerode-control (test): {}", cmd.join(" ")); return; }
    let c = cmd.to_vec();
    super::super::admin::bg(move || { let r: Vec<&str> = c.iter().map(|s| s.as_str()).collect(); super::super::admin::run(&r) },
        |(ok, out)| if !ok { util::error("The display settings could not be applied.", out.trim()); });
}

fn save_displays(cmd: &[String]) {
    let q = |s: &str| if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.x+:".contains(c)) { s.to_string() } else { super::super::admin::q(s) };
    let line: Vec<String> = cmd.iter().map(|s| q(s)).collect();
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(config::conf_dir().join("displays.sh"), format!("#!/bin/sh\n# written by aerode (Control Panel > Display > Screen Resolution)\n{}\n", line.join(" ")));
}

// ------------------------------------------------------------------ Screen Resolution

struct Res {
    outs: Vec<Out>,
    /// what is applied now (Revert goes back here)
    base: Vec<Pending>,
    pend: Vec<Pending>,
    sel: usize,
    /// extend | dup | only:N
    multi: String,
    drag: Option<(f64, f64, i32, i32)>,
    /// the preview's scale and offset, kept while dragging
    frozen: Option<(f64, f64, f64)>,
}

struct ResUi {
    st: RefCell<Res>,
    area: gtk::DrawingArea,
    disp: gtk::ComboBoxText,
    res: gtk::ComboBoxText,
    orient: gtk::ComboBoxText,
    multi: gtk::ComboBoxText,
    multi_row: Vec<gtk::Widget>,
    main: gtk::CheckButton,
    apply: gtk::Button,
    filling: Cell<bool>,
    win: gtk::Window,
}

pub fn page_resolution(cp: &Rc<Cp>) -> gtk::Widget {
    let v = vbox(8);
    v.pack_start(&title("Change the appearance of your displays"), false, false, 0);
    let top = hbox(8);
    let area = gtk::DrawingArea::new();
    area.set_size_request(440, 170);
    area.add_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK | gtk::gdk::EventMask::BUTTON_RELEASE_MASK | gtk::gdk::EventMask::BUTTON1_MOTION_MASK);
    let af = gtk::Frame::new(None);
    af.set_shadow_type(gtk::ShadowType::In);
    af.add(&area);
    top.pack_start(&af, false, false, 0);
    let bcol = vbox(6);
    bcol.set_valign(gtk::Align::End);
    let detect = gtk::Button::with_mnemonic("D_etect");
    let identify = gtk::Button::with_mnemonic("_Identify");
    for b in [&detect, &identify] { b.set_size_request(75, -1); bcol.pack_start(b, false, false, 0); }
    top.pack_start(&bcol, false, false, 0);
    v.pack_start(&top, false, false, 6);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    g.set_margin_start(20);
    let disp = gtk::ComboBoxText::new();
    let res = gtk::ComboBoxText::new();
    let orient = combo(&[("normal", "Landscape"), ("left", "Portrait"), ("inverted", "Landscape (flipped)"), ("right", "Portrait (flipped)")], "normal");
    let multi = gtk::ComboBoxText::new();
    for w in [&disp, &res, &orient, &multi] { w.set_size_request(260, -1); }
    let mut multi_row: Vec<gtk::Widget> = vec![];
    for (i, (l, w)) in [("Displa_y:", disp.clone().upcast::<gtk::Widget>()), ("_Resolution:", res.clone().upcast()), ("_Orientation:", orient.clone().upcast()), ("_Multiple displays:", multi.clone().upcast())].into_iter().enumerate() {
        let lab = lbl(l);
        lab.set_mnemonic_widget(Some(&w));
        g.attach(&lab, 0, i as i32, 1, 1);
        g.attach(&w, 1, i as i32, 1, 1);
        if i == 3 { multi_row = vec![lab.upcast(), w.clone()]; }
    }
    v.pack_start(&g, false, false, 0);
    let mrow = hbox(10);
    mrow.set_margin_start(20);
    let main = check("Ma_ke this my main display", false);
    mrow.pack_start(&main, false, false, 0);
    let adv = link("Advanced settings", || {});
    mrow.pack_end(&adv, false, false, 0);
    v.pack_start(&mrow, false, false, 4);
    let links = vbox(4);
    links.set_margin_top(10);
    { let w = cpwin(cp); links.pack_start(&link("Connect to a projector (or press the Windows logo key + P and tap P)", move || display_switch(&w)), false, false, 0); }
    { let cp2 = cp.clone(); links.pack_start(&link("Make text and other items larger or smaller", move || cp2.open("display")), false, false, 0); }
    links.pack_start(&link("What display settings should I choose?", || { util::open_uri("https://wiki.archlinux.org/title/Xrandr"); }), false, false, 0);
    v.pack_start(&links, false, false, 0);
    let bb = hbox(6);
    bb.set_halign(gtk::Align::End);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_mnemonic("_Apply");
    for b in [&ok, &cancel, &apply] { b.set_size_request(75, -1); bb.pack_start(b, false, false, 0); }
    apply.set_sensitive(false);
    v.pack_start(&bb, false, false, 12);

    let ui = Rc::new(ResUi { st: RefCell::new(Res { outs: vec![], base: vec![], pend: vec![], sel: 0, multi: "extend".into(), drag: None, frozen: None }),
        area: area.clone(), disp, res, orient, multi, multi_row, main, apply: apply.clone(), filling: Cell::new(false), win: cpwin(cp) });
    { let u = ui.clone(); area.connect_draw(move |w, cr| { u.draw(w, cr); glib::Propagation::Stop }); }
    { let u = ui.clone(); area.connect_button_press_event(move |w, e| { u.press(w, e.position()); glib::Propagation::Stop }); }
    { let u = ui.clone(); area.connect_motion_notify_event(move |w, e| { u.motion(w, e.position()); glib::Propagation::Stop }); }
    { let u = ui.clone(); area.connect_button_release_event(move |_, _| { u.release(); glib::Propagation::Stop }); }
    { let u = ui.clone(); ui.disp.connect_changed(move |c| { if u.filling.get() { return; } if let Some(i) = c.active() { u.st.borrow_mut().sel = i as usize; u.fill(); } }); }
    { let u = ui.clone(); ui.res.connect_changed(move |c| { if u.filling.get() { return; } if let Some(m) = c.active_id() { u.set_mode(&m); } }); }
    { let u = ui.clone(); ui.orient.connect_changed(move |c| {
        if u.filling.get() { return; }
        let Some(r) = c.active_id() else { return };
        { let mut st = u.st.borrow_mut(); let i = st.sel; if let Some(p) = st.pend.get_mut(i) { p.rot = r.to_string(); } }
        u.relayout(); u.changed();
    }); }
    { let u = ui.clone(); ui.multi.connect_changed(move |c| {
        if u.filling.get() { return; }
        let Some(m) = c.active_id() else { return };
        u.set_multi(&m);
    }); }
    { let u = ui.clone(); ui.main.connect_toggled(move |c| {
        if u.filling.get() || !c.is_active() { return; }
        { let mut st = u.st.borrow_mut(); let i = st.sel; for (j, p) in st.pend.iter_mut().enumerate() { p.primary = j == i; } }
        u.fill(); u.changed();
    }); }
    { let u = ui.clone(); detect.connect_clicked(move |_| u.load()); }
    { let u = ui.clone(); identify.connect_clicked(move |_| u.identify()); }
    { let u = ui.clone(); adv.connect_clicked(move |_| u.advanced()); }
    { let u = ui.clone(); apply.connect_clicked(move |_| u.apply(None)); }
    { let (u, cp2) = (ui.clone(), cp.clone()); ok.connect_clicked(move |_| {
        if u.apply.is_sensitive() { let cp3 = cp2.clone(); u.apply(Some(Box::new(move || super::super::desktop::leave(&cp3)))); } else { super::super::desktop::leave(&cp2); }
    }); }
    { let cp2 = cp.clone(); cancel.connect_clicked(move |_| super::super::desktop::leave(&cp2)); }
    ui.load();
    v.upcast()
}

impl ResUi {
    fn load(self: &Rc<Self>) {
        let u = self.clone();
        query(move |outs| {
            {
                let mut st = u.st.borrow_mut();
                st.pend = outs.iter().map(pending_of).collect();
                st.base = st.pend.clone();
                let on: Vec<usize> = st.pend.iter().enumerate().filter(|(_, p)| p.on).map(|(i, _)| i).collect();
                st.multi = if on.len() == 1 && outs.len() > 1 { format!("only:{}", on[0]) }
                    else if on.len() > 1 && on.iter().all(|i| st.pend[*i].x == st.pend[on[0]].x && st.pend[*i].y == st.pend[on[0]].y) { "dup".into() }
                    else { "extend".into() };
                st.sel = st.pend.iter().position(|p| p.primary).unwrap_or(0);
                st.outs = outs;
            }
            u.fill();
            u.apply.set_sensitive(false);
        });
    }

    fn changed(&self) { self.apply.set_sensitive(true); self.area.queue_draw(); }

    /// Fill the combos for the selected display.
    fn fill(&self) {
        self.filling.set(true);
        let st = self.st.borrow();
        self.disp.remove_all();
        for (i, o) in st.outs.iter().enumerate() { self.disp.append(Some(&o.name), &format!("{}. {}", i + 1, o.monitor)); }
        if st.outs.is_empty() { self.disp.append(Some(""), "No displays found (xrandr is not available)"); }
        self.disp.set_active(Some(st.sel as u32));
        self.res.remove_all();
        if let (Some(o), Some(p)) = (st.outs.get(st.sel), st.pend.get(st.sel)) {
            // duplicating: only the resolutions every display has
            let common = |m: &Mode| st.multi != "dup" || st.outs.iter().all(|x| x.modes.iter().any(|y| y.w == m.w && y.h == m.h));
            for m in o.modes.iter().filter(|m| common(m)) {
                self.res.append(Some(&m.name), &format!("{} × {}{}", m.w, m.h, if m.name == o.preferred { " (Recommended)" } else { "" }));
            }
            self.res.set_active_id(Some(&p.mode));
            self.orient.set_active_id(Some(&p.rot));
            self.main.set_active(p.primary);
            self.main.set_sensitive(!p.primary && p.on);
            self.main.set_label(if p.primary { "This is currently your main display." } else { "Ma_ke this my main display" });
            for w in [self.res.upcast_ref::<gtk::Widget>(), self.orient.upcast_ref()] { w.set_sensitive(p.on); }
        }
        self.multi.remove_all();
        self.multi.append(Some("dup"), "Duplicate these displays");
        self.multi.append(Some("extend"), "Extend these displays");
        for i in 0..st.outs.len() { self.multi.append(Some(&format!("only:{i}")), &format!("Show desktop only on {}", i + 1)); }
        self.multi.set_active_id(Some(&st.multi));
        for w in &self.multi_row { w.set_visible(st.outs.len() > 1); w.set_no_show_all(st.outs.len() <= 1); }
        drop(st);
        self.area.queue_draw();
        self.filling.set(false);
    }

    fn set_mode(&self, mode: &str) {
        {
            let mut st = self.st.borrow_mut();
            let i = st.sel;
            let o = st.outs[i].clone();
            let Some(m) = o.modes.iter().find(|m| m.name == mode).cloned() else { return };
            let dup = st.multi == "dup";
            let outs = st.outs.clone();
            for (j, p) in st.pend.iter_mut().enumerate() {
                let oo = &outs[j];
                let target = if j == i { Some(m.clone()) } else if dup { oo.modes.iter().find(|x| x.w == m.w && x.h == m.h).cloned() } else { None };
                let Some(t) = target else { continue };
                // keep the refresh rate when the new mode has it, else take the highest
                let cur: f64 = p.rate.parse().unwrap_or(0.0);
                p.rate = t.rates.iter().find(|r| (r.parse::<f64>().unwrap_or(0.0) - cur).abs() < 0.5).cloned().or_else(|| t.rates.first().cloned()).unwrap_or_default();
                p.mode = t.name.clone();
            }
        }
        self.relayout();
        self.changed();
    }

    fn set_multi(&self, m: &str) {
        {
            let mut st = self.st.borrow_mut();
            st.multi = m.to_string();
            let outs = st.outs.clone();
            match m {
                "dup" => {
                    let first = st.sel;
                    let (w, h) = mode_size(&outs[first], &st.pend[first].mode);
                    for (j, p) in st.pend.iter_mut().enumerate() {
                        p.on = true;
                        p.x = 0;
                        p.y = 0;
                        if let Some(t) = outs[j].modes.iter().find(|x| x.w == w && x.h == h) { p.mode = t.name.clone(); p.rate = t.rates.first().cloned().unwrap_or_default(); }
                    }
                }
                "extend" => {
                    for p in st.pend.iter_mut() { p.on = true; }
                    // side by side, left to right
                    let mut x = 0;
                    for j in 0..st.pend.len() {
                        let (w, _) = rect_size(&outs[j], &st.pend[j]);
                        st.pend[j].x = x;
                        st.pend[j].y = 0;
                        x += w;
                    }
                }
                m => {
                    let only: usize = m.trim_start_matches("only:").parse().unwrap_or(0);
                    for (j, p) in st.pend.iter_mut().enumerate() { p.on = j == only; p.primary = j == only; if j == only { p.x = 0; p.y = 0; } }
                }
            }
        }
        self.fill();
        self.changed();
    }

    /// After a size change keep the screens touching: pack the extended layout again left to right
    /// in their current order.
    fn relayout(&self) {
        let mut st = self.st.borrow_mut();
        if st.multi != "extend" { return; }
        let outs = st.outs.clone();
        let mut order: Vec<usize> = (0..st.pend.len()).filter(|i| st.pend[*i].on).collect();
        order.sort_by_key(|i| (st.pend[*i].x, st.pend[*i].y));
        // only re-pack a simple row; free arrangements are left as they are unless they now overlap
        let overlap = order.iter().any(|a| order.iter().any(|b| a != b && {
            let (pa, pb) = (&st.pend[*a], &st.pend[*b]);
            let (wa, ha) = rect_size(&outs[*a], pa);
            let (wb, hb) = rect_size(&outs[*b], pb);
            pa.x < pb.x + wb && pb.x < pa.x + wa && pa.y < pb.y + hb && pb.y < pa.y + ha
        }));
        let row = order.windows(2).all(|w| st.pend[w[0]].y == st.pend[w[1]].y);
        if !(overlap || row) { return; }
        let mut x = 0;
        for i in order { let (w, _) = rect_size(&outs[i], &st.pend[i]); st.pend[i].x = x; x += w; if row { st.pend[i].y = 0; } }
    }

    /// Screen rectangles in desktop coordinates and the transform to the drawing area.
    fn layout(&self, aw: f64, ah: f64) -> (Vec<(usize, i32, i32, i32, i32)>, f64, f64, f64) {
        let st = self.st.borrow();
        let mut rects = vec![];
        let mut off_x = st.pend.iter().zip(&st.outs).filter(|(p, _)| p.on).map(|(p, o)| p.x + rect_size(o, p).0).max().unwrap_or(0);
        for (i, (o, p)) in st.outs.iter().zip(&st.pend).enumerate() {
            let (w, h) = rect_size(o, p);
            if p.on { rects.push((i, p.x, p.y, w, h)); } else { rects.push((i, off_x + w / 10, 0, w, h)); off_x += w + w / 10; }
        }
        if rects.is_empty() { return (rects, 1.0, 0.0, 0.0); }
        if let Some((s, ox, oy)) = st.frozen { return (rects, s, ox, oy); }
        let minx = rects.iter().map(|r| r.1).min().unwrap();
        let miny = rects.iter().map(|r| r.2).min().unwrap();
        let maxx = rects.iter().map(|r| r.1 + r.3).max().unwrap();
        let maxy = rects.iter().map(|r| r.2 + r.4).max().unwrap();
        let (bw, bh) = ((maxx - minx) as f64, (maxy - miny) as f64);
        let s = ((aw - 40.0) / bw).min((ah - 30.0) / bh).min(0.12);
        let ox = (aw - bw * s) / 2.0 - minx as f64 * s;
        let oy = (ah - bh * s) / 2.0 - miny as f64 * s;
        (rects, s, ox, oy)
    }

    fn draw(&self, w: &gtk::DrawingArea, cr: &gtk::cairo::Context) {
        let (aw, ah) = (w.allocated_width() as f64, w.allocated_height() as f64);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok();
        let (rects, s, ox, oy) = self.layout(aw, ah);
        let st = self.st.borrow();
        let dup = st.multi == "dup";
        for (k, &(i, x, y, rw, rh)) in rects.iter().enumerate() {
            let on = st.pend[i].on;
            // duplicated screens are drawn as one with both numbers, offset a little
            let shift = if dup { k as f64 * 6.0 } else { 0.0 };
            let (x, y, w, h) = (ox + x as f64 * s + shift, oy + y as f64 * s + shift, rw as f64 * s, rh as f64 * s);
            let g = gtk::cairo::LinearGradient::new(x, y, x, y + h);
            if on { g.add_color_stop_rgb(0.0, 0.42, 0.65, 0.89); g.add_color_stop_rgb(1.0, 0.13, 0.36, 0.67); }
            else { g.add_color_stop_rgb(0.0, 0.82, 0.84, 0.87); g.add_color_stop_rgb(1.0, 0.68, 0.71, 0.75); }
            cr.rectangle(x + 1.0, y + 1.0, w - 2.0, h - 2.0);
            cr.set_source(&g).ok();
            cr.fill().ok();
            let sel = i == st.sel;
            cr.set_line_width(if sel { 3.0 } else { 1.0 });
            if sel { cr.set_source_rgb(0.20, 0.47, 0.85); } else { cr.set_source_rgb(0.25, 0.29, 0.35); }
            cr.rectangle(x + 1.5, y + 1.5, w - 3.0, h - 3.0);
            cr.stroke().ok();
            let l = w_layout(&self.area, &(i + 1).to_string(), (h * 0.5).clamp(12.0, 46.0));
            let (lw, lh) = l.pixel_size();
            let (tx, ty) = (x + (w - lw as f64) / 2.0, y + (h - lh as f64) / 2.0);
            cr.move_to(tx + 1.5, ty + 1.5);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
            pangocairo::functions::show_layout(cr, &l);
            cr.move_to(tx, ty);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            pangocairo::functions::show_layout(cr, &l);
        }
    }

    fn hit(&self, pos: (f64, f64)) -> Option<usize> {
        let (aw, ah) = (self.area.allocated_width() as f64, self.area.allocated_height() as f64);
        let (rects, s, ox, oy) = self.layout(aw, ah);
        rects.iter().rev().find(|&&(_, x, y, w, h)| {
            let (x, y, w, h) = (ox + x as f64 * s, oy + y as f64 * s, w as f64 * s, h as f64 * s);
            pos.0 >= x && pos.0 < x + w && pos.1 >= y && pos.1 < y + h
        }).map(|r| r.0)
    }

    fn press(&self, _w: &gtk::DrawingArea, pos: (f64, f64)) {
        let Some(i) = self.hit(pos) else { return };
        let (aw, ah) = (self.area.allocated_width() as f64, self.area.allocated_height() as f64);
        let (_, sc, ox, oy) = self.layout(aw, ah);
        {
            let mut st = self.st.borrow_mut();
            st.frozen = Some((sc, ox, oy));
            st.sel = i;
            let (px, py) = (st.pend[i].x, st.pend[i].y);
            st.drag = (st.multi == "extend" && st.pend[i].on && st.outs.len() > 1).then_some((pos.0, pos.1, px, py));
        }
        self.fill();
    }

    fn motion(&self, _w: &gtk::DrawingArea, pos: (f64, f64)) {
        let (aw, ah) = (self.area.allocated_width() as f64, self.area.allocated_height() as f64);
        let (_, s, _, _) = self.layout(aw, ah);
        let mut st = self.st.borrow_mut();
        let Some((sx, sy, px, py)) = st.drag else { return };
        let i = st.sel;
        st.pend[i].x = px + ((pos.0 - sx) / s) as i32;
        st.pend[i].y = py + ((pos.1 - sy) / s) as i32;
        drop(st);
        self.area.queue_draw();
    }

    /// Drop: snap the dragged screen against the nearest edge of another one, aligned when close.
    fn release(&self) {
        let mut st = self.st.borrow_mut();
        st.frozen = None;
        let Some((_, _, px, py)) = st.drag.take() else { drop(st); self.area.queue_draw(); return };
        let i = st.sel;
        let outs = st.outs.clone();
        let (w, h) = rect_size(&outs[i], &st.pend[i]);
        let (cx, cy) = (st.pend[i].x, st.pend[i].y);
        let mut best: Option<(i64, i32, i32)> = None;
        for (j, p) in st.pend.iter().enumerate() {
            if j == i || !p.on { continue; }
            let (ow, oh) = rect_size(&outs[j], p);
            let clamp_y = |y: i32| y.clamp(p.y - h + 1, p.y + oh - 1);
            let clamp_x = |x: i32| x.clamp(p.x - w + 1, p.x + ow - 1);
            for (x, y) in [(p.x - w, clamp_y(cy)), (p.x + ow, clamp_y(cy)), (clamp_x(cx), p.y - h), (clamp_x(cx), p.y + oh)] {
                // line edges up when they are within 5 % of each other
                let snap = |v: i32, targets: &[i32], tol: i32| targets.iter().copied().find(|t| (v - t).abs() <= tol).unwrap_or(v);
                let (x, y) = if x == p.x - w || x == p.x + ow { (x, snap(y, &[p.y, p.y + oh - h], oh / 20)) } else { (snap(x, &[p.x, p.x + ow - w], ow / 20), y) };
                let d = ((x - cx) as i64).pow(2) + ((y - cy) as i64).pow(2);
                if best.map(|b| d < b.0).unwrap_or(true) { best = Some((d, x, y)); }
            }
        }
        if let Some((_, x, y)) = best { st.pend[i].x = x; st.pend[i].y = y; }
        // overlapping another screen after all: back to where it was
        let overlaps = st.pend.iter().enumerate().any(|(j, p)| {
            if j == i || !p.on { return false; }
            let (ow, oh) = rect_size(&outs[j], p);
            let q = &st.pend[i];
            q.x < p.x + ow && p.x < q.x + w && q.y < p.y + oh && p.y < q.y + h
        });
        if overlaps { st.pend[i].x = px; st.pend[i].y = py; }
        let moved = st.pend[i].x != px || st.pend[i].y != py;
        drop(st);
        if moved { self.changed(); } else { self.area.queue_draw(); }
    }

    /// Big numbers on every screen for three seconds.
    fn identify(&self) {
        let st = self.st.borrow();
        let Some(screen) = gtk::gdk::Screen::default() else { return };
        let display = screen.display();
        for (i, (o, p)) in st.outs.iter().zip(&st.pend).enumerate() {
            if !p.on { continue; }
            // where the screen is now (the gdk monitor with that connector, else the saved position)
            let base = &st.base[i];
            let (bw, bh) = rect_size(o, base);
            let mut geo = (base.x, base.y, bw, bh);
            for m in 0..display.n_monitors() {
                if let Some(mon) = display.monitor(m) { if mon.model().map(|s| s == o.name).unwrap_or(false) { let g = mon.geometry(); geo = (g.x(), g.y(), g.width(), g.height()); } }
            }
            let win = gtk::Window::new(gtk::WindowType::Popup);
            win.set_app_paintable(true);
            aerode::glass::enable_rgba(win.upcast_ref());
            let size = 220;
            win.set_default_size(size, size);
            win.move_(geo.0 + (geo.2 - size) / 2, geo.1 + (geo.3 - size) / 2);
            let n = (i + 1).to_string();
            win.connect_draw(move |w, cr| {
                cr.set_operator(gtk::cairo::Operator::Source);
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
                cr.paint().ok();
                cr.set_operator(gtk::cairo::Operator::Over);
                let l = w_layout(w, &n, 150.0);
                let (lw, lh) = l.pixel_size();
                let (x, y) = ((w.allocated_width() - lw) as f64 / 2.0, (w.allocated_height() - lh) as f64 / 2.0);
                cr.move_to(x, y);
                pangocairo::functions::layout_path(cr, &l);
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.fill_preserve().ok();
                cr.set_line_width(4.0);
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.stroke().ok();
                glib::Propagation::Stop
            });
            win.show_all();
            glib::timeout_add_seconds_local_once(3, move || win.close());
        }
    }

    /// Apply the pending layout, then Win7's "Do you want to keep these display settings?" with a
    /// 15 second automatic revert. `then` runs after Keep changes.
    fn apply(self: &Rc<Self>, then: Option<Box<dyn Fn()>>) {
        let (cmd, back) = { let st = self.st.borrow(); (xrandr_cmd(&st.outs, &st.pend), xrandr_cmd(&st.outs, &st.base)) };
        if self.st.borrow().outs.is_empty() { return; }
        run_xrandr(&cmd);
        self.apply.set_sensitive(false);
        let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Other, gtk::ButtonsType::None, "Do you want to keep these display settings?");
        d.set_title("Display Settings");
        d.add_button("_Keep changes", gtk::ResponseType::Yes);
        d.add_button("_Revert", gtk::ResponseType::No);
        d.set_default_response(gtk::ResponseType::No);
        d.set_secondary_text(Some("Reverting to previous display settings in 15 seconds."));
        let secs = Rc::new(Cell::new(15));
        let u = self.clone();
        let then = RefCell::new(then);
        // closing the dialog answers it once more (delete-event): only the first answer counts
        let answered = Cell::new(false);
        d.connect_response(move |d, r| {
            if answered.replace(true) { return; }
            if r == gtk::ResponseType::Yes {
                save_displays(&cmd);
                { let mut st = u.st.borrow_mut(); st.base = st.pend.clone(); }
                if let Some(f) = then.borrow_mut().take() { f(); }
            } else {
                run_xrandr(&back);
                { let mut st = u.st.borrow_mut(); st.pend = st.base.clone(); }
                u.fill();
            }
            d.close();
        });
        {
            let (d2, s) = (d.clone(), secs.clone());
            glib::timeout_add_seconds_local(1, move || {
                if !d2.is_visible() { return glib::ControlFlow::Break; }
                let n = s.get() - 1;
                s.set(n);
                if n <= 0 { d2.response(gtk::ResponseType::No); return glib::ControlFlow::Break; }
                d2.set_secondary_text(Some(&format!("Reverting to previous display settings in {n} seconds.")));
                glib::ControlFlow::Continue
            });
        }
        d.show_all();
    }

    /// "<monitor> and <adapter> Properties": Adapter, Monitor (refresh rate), Color Management.
    fn advanced(self: &Rc<Self>) {
        let (o, p) = { let st = self.st.borrow(); match (st.outs.get(st.sel), st.pend.get(st.sel)) { (Some(o), Some(p)) => (o.clone(), p.clone()), _ => return } };
        let s = DlgSheet::new(Some(&self.win), &format!("{} Properties", o.monitor), true);
        // Adapter
        let t = vbox(10);
        let at = hbox(10);
        at.pack_start(&img(&["video-display", "preferences-desktop-display"], 32), false, false, 0);
        let aname = lbl("…");
        at.pack_start(&aname, true, true, 0);
        let props = gtk::Button::with_mnemonic("_Properties");
        props.connect_clicked(|_| spawn_tool("devmgr"));
        props.set_valign(gtk::Align::Center);
        at.pack_end(&props, false, false, 0);
        t.pack_start(&group("Adapter Type", &at), false, false, 0);
        let ig = gtk::Grid::new();
        ig.set_row_spacing(4);
        ig.set_column_spacing(12);
        let keys = ["Chip Type:", "DAC Type:", "Total Available Graphics Memory:", "Dedicated Video Memory:", "System Video Memory:", "Shared System Memory:", "Adapter String:", "BIOS Information:"];
        let vals: Vec<gtk::Label> = keys.iter().enumerate().map(|(i, k)| { ig.attach(&lbl(k), 0, i as i32, 1, 1); let v = lbl(""); ig.attach(&v, 1, i as i32, 1, 1); v }).collect();
        t.pack_start(&group("Adapter Information", &ig), false, false, 0);
        let list_modes = gtk::Button::with_mnemonic("_List All Modes");
        list_modes.set_halign(gtk::Align::Start);
        t.pack_start(&list_modes, false, false, 0);
        s.tab("Adapter", &t);
        {
            let (aname, vals, d) = (aname.clone(), vals.clone(), s.d.clone());
            let mon = o.monitor.clone();
            super::super::admin::bg(adapter_info, move |a| {
                aname.set_text(&a.name);
                d.set_title(&format!("{mon} and {} Properties", a.name));
                for (l, v) in vals.iter().zip([a.chip.clone(), "Internal".into(), a.total.clone(), a.vram.clone(), "0 MB".into(), a.shared.clone(), a.name.clone(), a.bios.clone()]) { l.set_text(&v); }
            });
        }
        // Monitor
        let t = vbox(10);
        let mt = hbox(10);
        mt.pack_start(&img(&["video-display", "preferences-desktop-display"], 32), false, false, 0);
        mt.pack_start(&lbl(&o.monitor), true, true, 0);
        let mp = gtk::Button::with_mnemonic("_Properties");
        mp.set_sensitive(false);
        mp.set_valign(gtk::Align::Center);
        mt.pack_end(&mp, false, false, 0);
        t.pack_start(&group("Monitor Type", &mt), false, false, 0);
        let ms = vbox(6);
        ms.pack_start(&lbl("_Screen refresh rate:"), false, false, 0);
        let rate = gtk::ComboBoxText::new();
        let rates = o.modes.iter().find(|m| m.name == p.mode).map(|m| m.rates.clone()).unwrap_or_default();
        for r in &rates { rate.append(Some(r), &hertz(r, &rates)); }
        if !rate.set_active_id(Some(&p.rate)) { rate.set_active(Some(0)); }
        ms.pack_start(&rate, false, false, 0);
        let hide = check("_Hide modes that this monitor cannot display", true);
        hide.set_sensitive(false);
        ms.pack_start(&hide, false, false, 0);
        ms.pack_start(&wrap("Clearing this check box allows you to select display modes that this monitor cannot display correctly. This may lead to an unusable display and/or damaged hardware.", 52), false, false, 0);
        t.pack_start(&group("Monitor Settings", &ms), false, false, 0);
        let colors = vbox(6);
        let depth = gtk::ComboBoxText::new();
        depth.append_text("True Color (32 bit)");
        depth.set_active(Some(0));
        colors.pack_start(&depth, false, false, 0);
        t.pack_start(&group("Colors", &colors), false, false, 0);
        s.watch_combo(&rate);
        s.tab("Monitor", &t);
        // Color Management
        let t = vbox(10);
        t.pack_start(&wrap("To change color management settings, click Color Management.", 52), false, false, 0);
        let cm = gtk::Button::with_mnemonic("Color _Management...");
        cm.set_halign(gtk::Align::Start);
        { let d = s.d.clone(); cm.connect_clicked(move |_| { d.close(); spawn_tool("colormgmt"); }); }
        t.pack_start(&cm, false, false, 0);
        s.tab("Color Management", &t);
        // List All Modes picks mode + rate
        let picked: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
        { let (o, d, picked, dirty, rate) = (o.clone(), s.d.clone(), picked.clone(), s.dirty(), rate.clone()); list_modes.connect_clicked(move |_| {
            if let Some((m, r)) = list_all_modes(d.upcast_ref(), &o) {
                rate.remove_all();
                let rs = o.modes.iter().find(|x| x.name == m).map(|x| x.rates.clone()).unwrap_or_default();
                for x in &rs { rate.append(Some(x), &hertz(x, &rs)); }
                rate.set_active_id(Some(&r));
                *picked.borrow_mut() = Some((m, r));
                dirty();
            }
        }); }
        let u = self.clone();
        s.on_apply(move || {
            {
                let mut st = u.st.borrow_mut();
                let i = st.sel;
                if let Some((m, _)) = picked.borrow_mut().take() { st.pend[i].mode = m; }
                if let Some(r) = rate.active_id() { st.pend[i].rate = r.to_string(); }
            }
            u.fill();
            u.apply(None);
            true
        });
        s.show(0);
    }
}

fn w_layout(w: &impl IsA<gtk::Widget>, t: &str, px: f64) -> gtk::pango::Layout {
    let l = w.create_pango_layout(Some(t));
    let mut f = gtk::pango::FontDescription::from_string("Segoe UI");
    f.set_weight(gtk::pango::Weight::Bold);
    f.set_absolute_size(px * gtk::pango::SCALE as f64);
    l.set_font_description(Some(&f));
    l
}

#[derive(Default)]
struct Adapter { name: String, chip: String, vram: String, total: String, shared: String, bios: String }

/// The graphics adapter from lspci and the DRM sysfs (runs on a worker thread).
fn adapter_info() -> Adapter {
    let mut a = Adapter::default();
    let out = super::super::admin::sh(&["lspci", "-mm"]);
    let line = out.lines().find(|l| l.contains("\"VGA compatible controller\"") || l.contains("\"3D controller\"") || l.contains("\"Display controller\""));
    if let Some(l) = line {
        let q: Vec<&str> = l.split('"').skip(1).step_by(2).collect();
        let vendor = q.get(1).copied().unwrap_or("");
        let dev = q.get(2).copied().unwrap_or("");
        let short = if vendor.contains("AMD") || vendor.contains("ATI") { "AMD" } else { vendor.split_whitespace().next().unwrap_or("") };
        let model = dev.rsplit_once('[').map(|(_, m)| m.trim_end_matches(']').to_string()).unwrap_or_else(|| dev.to_string());
        a.name = format!("{short} {model}").trim().to_string();
        a.chip = dev.split('[').next().unwrap_or(dev).trim().to_string();
    } else {
        a.name = "Standard VGA Graphics Adapter".into();
        a.chip = "n/a".into();
    }
    let card = std::fs::read_dir("/sys/class/drm").ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().map(|n| { let n = n.to_string_lossy(); n.starts_with("card") && !n.contains('-') }).unwrap_or(false)));
    let read = |f: &str| card.as_ref().and_then(|c| std::fs::read_to_string(c.join("device").join(f)).ok()).map(|s| s.trim().to_string());
    let mb = |s: Option<String>| s.and_then(|v| v.parse::<u64>().ok()).map(|b| format!("{} MB", b >> 20));
    a.vram = mb(read("mem_info_vram_total")).unwrap_or_else(|| "n/a".into());
    a.shared = mb(read("mem_info_gtt_total")).unwrap_or_else(|| "n/a".into());
    a.total = match (read("mem_info_vram_total").and_then(|v| v.parse::<u64>().ok()), read("mem_info_gtt_total").and_then(|v| v.parse::<u64>().ok())) {
        (Some(v), Some(g)) => format!("{} MB", (v + g) >> 20), (Some(v), None) => format!("{} MB", v >> 20), _ => "n/a".into() };
    a.bios = read("vbios_version").unwrap_or_else(|| read("driver").unwrap_or_default());
    if a.bios.is_empty() {
        a.bios = card.as_ref().and_then(|c| std::fs::read_link(c.join("device/driver")).ok()).and_then(|p| p.file_name().map(|n| format!("Driver: {}", n.to_string_lossy()))).unwrap_or_else(|| "n/a".into());
    }
    a
}

/// "List All Modes": every resolution and refresh rate of the monitor.
fn list_all_modes(parent: &gtk::Window, o: &Out) -> Option<(String, String)> {
    let d = ok_cancel(Some(parent), "List All Modes");
    let c = d.content_area();
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    for m in &o.modes {
        for r in &m.rates { store.insert_with_values(None, &[(0, &format!("{} by {}, True Color (32 bit), {}", m.w, m.h, hertz(r, &m.rates))), (1, &m.name), (2, r)]); }
    }
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("List of valid modes", 0, 0)]);
    tv.set_headers_visible(false);
    let sw = scrolled(&tv);
    sw.set_size_request(340, 220);
    c.pack_start(&group("List of valid modes", &sw), true, true, 0);
    { let d = d.clone(); tv.connect_row_activated(move |_, _, _| d.response(gtk::ResponseType::Ok)); }
    d.show_all();
    let r = d.run();
    let pick = tv.selection().selected().map(|(m, it)| (super::super::admin::sval(&m, &it, 1), super::super::admin::sval(&m, &it, 2)));
    d.close();
    if r == gtk::ResponseType::Ok { pick } else { None }
}

/// Win+P "DisplaySwitch": Computer only / Duplicate / Extend / Projector only.
pub fn display_switch(parent: &gtk::Window) {
    let d = gtk::Dialog::new();
    d.set_title("Connect to a projector");
    d.set_transient_for(Some(parent));
    d.set_modal(true);
    d.set_resizable(false);
    let c = d.content_area();
    c.set_border_width(12);
    let row = hbox(10);
    let pick: Rc<Cell<Option<&'static str>>> = Rc::new(Cell::new(None));
    for (id, label, icon) in [("computer", "Computer only", "video-display"), ("dup", "Duplicate", "video-display"), ("extend", "Extend", "video-display"), ("projector", "Projector only", "video-display")] {
        let b = gtk::Button::new();
        let v = vbox(6);
        v.pack_start(&img(&[icon], 48), false, false, 0);
        v.pack_start(&gtk::Label::new(Some(label)), false, false, 0);
        b.add(&v);
        b.set_size_request(110, 100);
        let (p, d2) = (pick.clone(), d.clone());
        b.connect_clicked(move |_| { p.set(Some(id)); d2.response(gtk::ResponseType::Ok); });
        row.pack_start(&b, false, false, 0);
    }
    c.pack_start(&row, false, false, 0);
    d.show_all();
    let parent = parent.clone();
    d.connect_response(move |d, _| {
        d.close();
        let Some(what) = pick.get() else { return };
        let p = parent.clone();
        query(move |outs| {
            if outs.len() < 2 { inform(Some(&p), "Connect to a projector", "Windows did not detect a second display.", "Connect the projector or monitor, and then try again."); return; }
            let first = outs.iter().position(|o| o.primary).unwrap_or(0);
            let second = (0..outs.len()).find(|i| *i != first).unwrap_or(1);
            let mut pend: Vec<Pending> = outs.iter().map(pending_of).collect();
            let mut x = 0;
            for (i, (o, pp)) in outs.iter().zip(pend.iter_mut()).enumerate() {
                pp.mode = if pp.mode.is_empty() { o.preferred.clone() } else { pp.mode.clone() };
                if pp.rate.is_empty() { pp.rate = o.modes.iter().find(|m| m.name == pp.mode).and_then(|m| m.rates.first().cloned()).unwrap_or_default(); }
                pp.on = match what { "computer" => i == first, "projector" => i == second, _ => true };
                pp.primary = i == if what == "projector" { second } else { first };
                match what { "extend" => { pp.x = x; pp.y = 0; x += rect_size(o, pp).0; } _ => { pp.x = 0; pp.y = 0; } }
            }
            let cmd = xrandr_cmd(&outs, &pend);
            run_xrandr(&cmd);
            save_displays(&cmd);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    const TWO: &str = "Screen 0: minimum 320 x 200, current 4480 x 1440, maximum 16384 x 16384
DP-1 connected primary 2560x1440+0+0 (normal left inverted right x axis y axis) 597mm x 336mm
\tEDID:
\t\t00ffffffffffff000000000000000000
\t\t00000000000000000000000000000000
\t\t00000000000000000000000000000000
\t\t00000000000000000000000000000000
\t\t0000000000000000000000fc0044454c
\t\t4c205332373231444746000000000000
\t\t00000000000000000000000000000000
\t\t00000000000000000000000000000000
\tnon-desktop: 0
   2560x1440    143.97*+ 119.88    99.95    59.95
   1920x1080    143.98   119.98    60.00    59.94
HDMI-1 connected 1080x1920+2560+0 left (normal left inverted right x axis y axis) 527mm x 296mm
   1920x1080     60.00 +  50.00    59.94*
DP-2 disconnected (normal left inverted right x axis y axis)
";

    #[test]
    fn parses_xrandr() {
        let o = parse(TWO);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].monitor, "DELL S2721DGF");
        assert!(o[0].primary);
        assert_eq!((o[0].cur.as_str(), o[0].rate.as_str()), ("2560x1440", "143.97"));
        assert_eq!(o[0].modes[1].rates, vec!["143.98", "119.98", "60.00", "59.94"]);
        assert_eq!(o[1].monitor, "Generic PnP Monitor");
        assert_eq!((o[1].rot.as_str(), o[1].x, o[1].rate.as_str(), o[1].preferred.as_str()), ("left", 2560, "59.94", "1920x1080"));
        let p: Vec<Pending> = o.iter().map(pending_of).collect();
        assert_eq!(rect_size(&o[1], &p[1]), (1080, 1920));
        let cmd = xrandr_cmd(&o, &p).join(" ");
        assert_eq!(cmd, "xrandr --output DP-1 --mode 2560x1440 --rate 143.97 --rotate normal --pos 0x0 --primary --output HDMI-1 --mode 1920x1080 --rate 59.94 --rotate left --pos 2560x0");
        assert_eq!(hertz("59.94", &o[1].modes[0].rates), "59 Hertz");
        assert_eq!(hertz("60.00", &o[1].modes[0].rates), "60 Hertz");
        assert_eq!(hertz("143.97", &o[0].modes[0].rates), "144 Hertz");
    }
}
