//! Sound: the Windows 7 Sound dialog. Playback / Recording list the PipeWire (or PulseAudio) sinks
//! and sources from `pactl -f json`, with device icons, the default and default-communication
//! badges and live level meters (parec); Set Default, Configure (speaker setup = card profile) and
//! Properties (General with device usage, Listen, Levels with balance, Advanced default format =
//! a WirePlumber rule). Sounds is the event scheme editor, Communications is kept in config.
use super::*;
use super::super::admin::{bg, img, scrolled, spawn_tool};
use super::super::desktop::{check, combo, group, radios, Sheet};
use aerode::sounds;
use serde_json::Value;
use std::cell::Cell;

fn vbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, s) }
fn hbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, s) }

// ------------------------------------------------------------------ devices

#[derive(Clone, Debug, Default)]
struct Dev {
    sink: bool,
    name: String,
    /// the endpoint name Windows shows first: the active port ("Speakers", "Headphones") or the description
    title: String,
    /// the hardware below it ("Cannon Lake PCH cAVS")
    hw: String,
    desc: String,
    port: String,
    ports: Vec<(String, String, bool)>,
    icon: &'static [&'static str],
    vol: Vec<u32>,
    mute: bool,
    spec: String,
    card: String,
    monitor: String,
    unplugged: bool,
    disabled: bool,
}

/// pactl output (AERODE_PACTL_DIR replays saved JSON for testing).
fn pactl(args: &[&str]) -> String {
    if let Ok(d) = std::env::var("AERODE_PACTL_DIR") {
        let f = match args { ["-f", "json", "list", what] => format!("{what}.json"), [x] => x.to_string(), _ => String::new() };
        return std::fs::read_to_string(std::path::Path::new(&d).join(f)).unwrap_or_default();
    }
    super::super::admin::sh(&[&["pactl"], args].concat())
}

/// Run a pactl command that changes something (printed instead when testing).
fn pactl_do(args: &[&str]) {
    if std::env::var_os("AERODE_PACTL_DIR").is_some() { eprintln!("aerode-control (test): pactl {}", args.join(" ")); return; }
    let mut a = vec!["pactl"];
    a.extend_from_slice(args);
    util::spawn(&a);
}

/// Icon names for a device, best first (resolved on the GTK thread with `util::first_icon`).
fn icon_for(sink: bool, port_type: &str, port: &str, form: &str, bus: &str) -> &'static [&'static str] {
    let p = format!("{port_type} {port} {form}").to_lowercase();
    if sink {
        if p.contains("headphone") || p.contains("headset") || form == "headset" || bus == "bluetooth" { &["audio-headphones", "audio-speakers"] }
        else if p.contains("hdmi") || p.contains("iec958") || p.contains("spdif") || p.contains("displayport") { &["video-display", "audio-card"] }
        else if p.contains("line") { &["audio-card", "audio-speakers"] }
        else { &["audio-speakers", "audio-volume-high"] }
    } else if p.contains("line") { &["audio-card", "audio-input-microphone"] }
    else if bus == "bluetooth" || p.contains("headset") { &["audio-headset", "audio-input-microphone"] }
    else { &["audio-input-microphone"] }
}

fn parse_devs(json: &str, sink: bool) -> Vec<Dev> {
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(json) else { return vec![] };
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    a.iter().filter_map(|d| {
        let props = d.get("properties").cloned().unwrap_or(Value::Null);
        let p = |k: &str| props.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        // monitors of outputs are not recording devices in Windows
        if !sink && (p("device.class") == "monitor" || s(d, "name").ends_with(".monitor")) { return None; }
        let ports: Vec<(String, String, bool)> = d.get("ports").and_then(|x| x.as_array()).map(|a| a.iter().map(|x| (s(x, "name"), s(x, "description"), s(x, "availability") != "not available")).collect()).unwrap_or_default();
        let active = s(d, "active_port");
        let ap = d.get("ports").and_then(|x| x.as_array()).and_then(|a| a.iter().find(|x| s(x, "name") == active)).cloned().unwrap_or(Value::Null);
        let port_desc = s(&ap, "description");
        let desc = s(d, "description");
        let title = if !port_desc.is_empty() && !port_desc.contains("Analog") { port_desc.clone() } else if sink { "Speakers".into() } else { "Microphone".into() };
        let hw = [p("device.product.name"), p("device.description"), p("alsa.card_name"), desc.clone()].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
        let vol: Vec<u32> = d.get("volume").and_then(|v| v.as_object()).map(|o| o.values().filter_map(|c| c.get("value_percent").and_then(|x| x.as_str()).and_then(|x| x.trim_end_matches('%').parse().ok())).collect()).unwrap_or_default();
        Some(Dev {
            sink, name: s(d, "name"), title, hw, desc, port: active.clone(), ports,
            icon: icon_for(sink, &s(&ap, "type"), &active, &p("device.form_factor"), &p("device.bus")),
            vol, mute: d.get("mute").and_then(|x| x.as_bool()).unwrap_or(false), spec: s(d, "sample_specification"),
            card: p("device.name"), monitor: s(d, "monitor_source"), unplugged: s(&ap, "availability") == "not available", disabled: false,
        })
    }).collect()
}

/// Cards whose profile is Off: Windows lists them as disabled devices.
fn disabled_cards(json: &str, sink: bool) -> Vec<Dev> {
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(json) else { return vec![] };
    a.iter().filter(|c| c.get("active_profile").and_then(|x| x.as_str()) == Some("off")).filter_map(|c| {
        let name = c.get("name")?.as_str()?.to_string();
        let desc = c.get("properties").and_then(|p| p.get("device.description")).and_then(|x| x.as_str()).unwrap_or(&name).to_string();
        let has = c.get("profiles")?.as_object()?.values().any(|p| p.get(if sink { "sinks" } else { "sources" }).and_then(|x| x.as_u64()).unwrap_or(0) > 0);
        has.then(|| Dev { sink, name: String::new(), title: if sink { "Speakers".into() } else { "Microphone".into() }, hw: desc.clone(), desc, card: name,
            icon: icon_for(sink, "", "", "", ""), disabled: true, ..Default::default() })
    }).collect()
}

/// The profiles of a card: (name, description, priority, sinks, sources).
fn card_profiles(cards: &str, card: &str) -> (Vec<(String, String, i64, u64, u64)>, String) {
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(cards) else { return (vec![], String::new()) };
    let Some(c) = a.iter().find(|c| c.get("name").and_then(|x| x.as_str()) == Some(card)) else { return (vec![], String::new()) };
    let active = c.get("active_profile").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut v: Vec<(String, String, i64, u64, u64)> = c.get("profiles").and_then(|p| p.as_object()).map(|o| o.iter()
        .filter(|(_, p)| p.get("available").and_then(|x| x.as_bool()).unwrap_or(true))
        .map(|(k, p)| (k.clone(), p.get("description").and_then(|x| x.as_str()).unwrap_or(k).to_string(), p.get("priority").and_then(|x| x.as_i64()).unwrap_or(0),
            p.get("sinks").and_then(|x| x.as_u64()).unwrap_or(0), p.get("sources").and_then(|x| x.as_u64()).unwrap_or(0))).collect()).unwrap_or_default();
    v.sort_by(|a, b| b.2.cmp(&a.2));
    (v, active)
}

#[derive(Clone, Default)]
struct Snapshot { sinks: Vec<Dev>, sources: Vec<Dev>, def_sink: String, def_source: String, cards: String }

fn snapshot() -> Snapshot {
    let cards = pactl(&["-f", "json", "list", "cards"]);
    let mut sinks = parse_devs(&pactl(&["-f", "json", "list", "sinks"]), true);
    let mut sources = parse_devs(&pactl(&["-f", "json", "list", "sources"]), false);
    // pactl without JSON (old PulseAudio): names and descriptions from the plain list
    if sinks.is_empty() && std::env::var_os("AERODE_PACTL_DIR").is_none() {
        sinks = aerode::sys::sinks().into_iter().map(|(n, d, _)| Dev { sink: true, name: n, title: "Speakers".into(), hw: d.clone(), desc: d, icon: icon_for(true, "", "", "", ""), ..Default::default() }).collect();
    }
    sinks.extend(disabled_cards(&cards, true));
    sources.extend(disabled_cards(&cards, false));
    Snapshot { sinks, sources, def_sink: pactl(&["get-default-sink"]).trim().to_string(), def_source: pactl(&["get-default-source"]).trim().to_string(), cards }
}

// ------------------------------------------------------------------ level meters

/// parec readers feeding the meters; killed when the page goes away.
struct Meters { kids: RefCell<Vec<std::process::Child>> }
impl Drop for Meters {
    fn drop(&mut self) { for k in self.kids.borrow_mut().iter_mut() { let _ = k.kill(); let _ = k.wait(); } }
}

/// Peak level 0..1 of a source (a sink's monitor for playback devices), ~20 times a second.
fn meter(m: &Rc<Meters>, source: &str, level: Rc<Cell<f64>>, area: gtk::DrawingArea) {
    if std::env::var_os("AERODE_PACTL_DIR").is_some() {
        // testing: a gentle fake signal so the meters can be seen
        let t0 = std::time::Instant::now();
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            if !area.is_visible() && area.parent().is_none() { return glib::ControlFlow::Break; }
            let t = t0.elapsed().as_secs_f64();
            level.set(((t * 2.3).sin() * 0.5 + 0.5) * 0.8);
            area.queue_draw();
            glib::ControlFlow::Continue
        });
        return;
    }
    if !util::which("parec") { return; }
    let Ok(mut child) = std::process::Command::new("parec").args(["-d", source, "--raw", "--format=s16le", "--channels=1", "--rate=8000", "--latency-msec=40", "--client-name=aerode-meter", "--stream-name=peak"])
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() else { return };
    let Some(mut out) = child.stdout.take() else { return };
    m.kids.borrow_mut().push(child);
    super::super::admin::stream(move |tx| {
        use std::io::Read;
        let mut buf = [0u8; 800];
        while out.read_exact(&mut buf).is_ok() {
            let peak = buf.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap_or(0);
            if tx.send_blocking(peak as f64 / 32768.0).is_err() { break; }
        }
    }, move |v: f64| {
        // fast attack, slow decay like the Windows meters
        let cur = level.get();
        level.set(if v > cur { v } else { cur * 0.7 + v * 0.3 });
        area.queue_draw();
    });
}

/// Win7's vertical meter: ten little bars, green while lit.
fn draw_meter(cr: &gtk::cairo::Context, w: f64, h: f64, level: f64) {
    let n = 10;
    let bh = (h - 2.0) / n as f64;
    let lit = (level.sqrt() * n as f64).round() as i32;
    for i in 0..n {
        let y = h - 1.0 - (i + 1) as f64 * bh;
        cr.rectangle(1.0, y + 1.0, w - 2.0, bh - 2.0);
        if i < lit { cr.set_source_rgb(0.20, 0.72, 0.18); } else { cr.set_source_rgb(0.86, 0.87, 0.88); }
        cr.fill().ok();
    }
}

// ------------------------------------------------------------------ the dialog

pub fn page(cp: &Rc<Cp>) -> gtk::Widget { page_tab(cp, 0) }

pub fn page_tab(cp: &Rc<Cp>, tab: u32) -> gtk::Widget {
    css();
    let sheet = Sheet::new(cp);
    let meters = Rc::new(Meters { kids: RefCell::new(vec![]) });
    let snap: Rc<RefCell<Snapshot>> = Rc::new(RefCell::new(Snapshot::default()));
    let (play, play_fill) = device_tab(cp, true, &snap, &meters);
    let (rec, rec_fill) = device_tab(cp, false, &snap, &meters);
    sheet.tab("Playback", &play);
    sheet.tab("Recording", &rec);
    let reload: Rc<dyn Fn()> = {
        let (snap, pf, rf) = (snap.clone(), play_fill.clone(), rec_fill.clone());
        Rc::new(move || {
            let (snap, pf, rf) = (snap.clone(), pf.clone(), rf.clone());
            bg(snapshot, move |s| { *snap.borrow_mut() = s; pf.fill(); rf.fill(); });
        })
    };
    *play_fill.reload.borrow_mut() = Some(reload.clone());
    *rec_fill.reload.borrow_mut() = Some(reload.clone());
    reload();
    let (sounds_tab, sounds_apply) = sounds_tab(&sheet);
    sheet.tab("Sounds", &sounds_tab);
    let comms = vbox(10);
    let top = hbox(12);
    top.pack_start(&img(&["phone", "audio-headset"], 48), false, false, 0);
    top.pack_start(&wrap("Windows can automatically adjust the volume of different sounds when you are using your PC to place or receive telephone calls.", 40), false, false, 0);
    comms.pack_start(&top, false, false, 0);
    comms.pack_start(&lbl("When Windows detects communications activity:"), false, false, 0);
    let cur = config::get().hw.comms_action;
    let cr = radios(&["_Mute all other sounds", "_Reduce the volume of other sounds by 80%", "Reduce the volume of other sounds by _50%", "_Do nothing"],
        match cur.as_str() { "mute" => 0, "reduce50" => 2, "none" => 3, _ => 1 });
    for r in &cr { r.set_margin_start(16); comms.pack_start(r, false, false, 0); sheet.watch_check(r.upcast_ref()); }
    comms.pack_start(&link("More information about communications features", || {}), false, false, 6);
    sheet.tab("Communications", &comms);
    // the meters stop with the page
    { let m = RefCell::new(Some(meters)); sheet.nb.connect_destroy(move |_| { m.borrow_mut().take(); }); }
    sheet.on_apply(move || {
        sounds_apply();
        let a = ["mute", "reduce80", "reduce50", "none"][cr.iter().position(|r| r.is_active()).unwrap_or(1)];
        config::update(|c| c.hw.comms_action = a.to_string());
        true
    });
    sheet.nb.set_current_page(Some(tab));
    let w = sheet.widget();
    { let nb = sheet.nb.clone(); glib::idle_add_local_once(move || nb.set_current_page(Some(tab))); }
    w
}

/// One device tab and the function that refills it from the snapshot.
struct Filler { f: RefCell<Option<Rc<dyn Fn()>>>, reload: RefCell<Option<Rc<dyn Fn()>>> }
impl Filler {
    fn reload(&self) { if let Some(r) = self.reload.borrow().clone() { r(); } }
    fn fill(&self) { if let Some(f) = self.f.borrow().clone() { f(); } }
}

fn device_tab(cp: &Rc<Cp>, sinks: bool, snap: &Rc<RefCell<Snapshot>>, meters: &Rc<Meters>) -> (gtk::Box, Rc<Filler>) {
    let v = vbox(8);
    v.pack_start(&lbl(if sinks { "Select a playback device below to modify its settings:" } else { "Select a recording device below to modify its settings:" }), false, false, 0);
    let list = vbox(0);
    let white = gtk::EventBox::new();
    white.style_context().add_class("view");
    white.add(&list);
    let sw = scrolled(&white);
    sw.set_size_request(380, 290);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    v.pack_start(&sw, true, true, 0);
    let bb = hbox(6);
    let configure = gtk::Button::with_mnemonic("_Configure");
    bb.pack_start(&configure, false, false, 0);
    let props = gtk::Button::with_mnemonic("_Properties");
    let setdef = gtk::Button::with_mnemonic("_Set Default");
    let arrow = gtk::MenuButton::new();
    arrow.set_image(Some(&gtk::Image::from_icon_name(Some("pan-down-symbolic"), gtk::IconSize::Menu)));
    let split = hbox(0);
    split.style_context().add_class("linked");
    split.pack_start(&setdef, false, false, 0);
    split.pack_start(&arrow, false, false, 0);
    bb.pack_end(&props, false, false, 0);
    bb.pack_end(&split, false, false, 0);
    for b in [&configure, &props] { b.set_size_request(75, -1); }
    v.pack_start(&bb, false, false, 0);
    let sel: Rc<RefCell<Option<Dev>>> = Rc::new(RefCell::new(None));
    let rows: Rc<RefCell<Vec<(gtk::EventBox, Dev)>>> = Rc::new(RefCell::new(vec![]));
    let win = cpwin(cp);
    let update_buttons = {
        let (sel, snap, configure, props, setdef, arrow) = (sel.clone(), snap.clone(), configure.clone(), props.clone(), setdef.clone(), arrow.clone());
        Rc::new(move || {
            let s = sel.borrow();
            let d = s.as_ref();
            let def = d.map(|d| d.name == if sinks { snap.borrow().def_sink.clone() } else { snap.borrow().def_source.clone() }).unwrap_or(true);
            let comm = d.map(|d| d.name == comms_default(sinks, &snap.borrow())).unwrap_or(true);
            setdef.set_sensitive(d.map(|d| !d.disabled).unwrap_or(false) && !(def && comm));
            arrow.set_sensitive(d.map(|d| !d.disabled).unwrap_or(false));
            props.set_sensitive(d.is_some());
            configure.set_sensitive(sinks && d.map(|d| !d.card.is_empty() && !d.disabled).unwrap_or(false));
        })
    };
    let filler: Rc<Filler> = Rc::new(Filler { f: RefCell::new(None), reload: RefCell::new(None) });
    let fill: Rc<dyn Fn()> = {
        let (list, snap, sel, rows, meters, ub, win) = (list.clone(), snap.clone(), sel.clone(), rows.clone(), meters.clone(), update_buttons.clone(), win.clone());
        let filler_weak = Rc::downgrade(&filler);
        Rc::new(move || {
            for c in list.children() { list.remove(&c); }
            rows.borrow_mut().clear();
            let s = snap.borrow().clone();
            let devs = if sinks { s.sinks.clone() } else { s.sources.clone() };
            if devs.is_empty() {
                let l = lbl(if sinks { "No audio devices are installed" } else { "No recording devices are installed" });
                l.set_halign(gtk::Align::Center);
                l.set_margin_top(120);
                l.style_context().add_class("hw-dim");
                list.pack_start(&l, false, false, 0);
            }
            let def = if sinks { s.def_sink.clone() } else { s.def_source.clone() };
            let comm = comms_default(sinks, &s);
            let keep = sel.borrow().as_ref().map(|d| (d.name.clone(), d.card.clone()));
            *sel.borrow_mut() = None;
            for d in devs {
                let row = gtk::EventBox::new();
                let h = hbox(10);
                h.style_context().add_class("hw-row");
                h.set_border_width(2);
                let badge = if d.name == def && !d.disabled { 1 } else if d.name == comm && !d.disabled { 2 } else { 0 };
                h.pack_start(&device_icon(util::first_icon(d.icon), badge, d.disabled || d.unplugged), false, false, 0);
                let col = vbox(1);
                col.set_valign(gtk::Align::Center);
                col.pack_start(&lbl(&d.title), false, false, 0);
                let hw = lbl(&d.hw);
                hw.style_context().add_class("hw-dim");
                col.pack_start(&hw, false, false, 0);
                let status = if d.disabled { "Disabled" } else if d.unplugged { "Not plugged in" } else if badge == 1 && d.name == comm { "Default Device" }
                    else if badge == 1 { "Default Device" } else if badge == 2 { "Default Communications Device" } else { "Ready" };
                let st = lbl(status);
                st.style_context().add_class("hw-dim");
                col.pack_start(&st, false, false, 0);
                h.pack_start(&col, true, true, 0);
                if !d.disabled {
                    let level = Rc::new(Cell::new(0.0));
                    let m = gtk::DrawingArea::new();
                    m.set_size_request(12, 46);
                    m.set_valign(gtk::Align::Center);
                    m.set_margin_end(6);
                    { let l = level.clone(); m.connect_draw(move |w, cr| { draw_meter(cr, w.allocated_width() as f64, w.allocated_height() as f64, l.get()); glib::Propagation::Stop }); }
                    h.pack_end(&m, false, false, 0);
                    meter(&meters, if sinks { &d.monitor } else { &d.name }, level, m.clone());
                }
                row.add(&h);
                list.pack_start(&row, false, false, 0);
                {
                    let (sel, rows, d2, ub, win, fw) = (sel.clone(), rows.clone(), d.clone(), ub.clone(), win.clone(), filler_weak.clone());
                    let snap = snap.clone();
                    row.connect_button_press_event(move |row, e| {
                        for (r, _) in rows.borrow().iter() { if let Some(c) = r.child() { c.style_context().remove_class("hw-row-sel"); } }
                        if let Some(c) = row.child() { c.style_context().add_class("hw-row-sel"); }
                        *sel.borrow_mut() = Some(d2.clone());
                        ub();
                        let Some(f) = fw.upgrade() else { return glib::Propagation::Stop };
                        if e.event_type() == gtk::gdk::EventType::DoubleButtonPress { properties(&win, &d2, &snap.borrow().cards, &f); }
                        if e.button() == 3 { context_menu(&win, &d2, &snap.borrow(), &f, e); }
                        glib::Propagation::Stop
                    });
                }
                if keep.as_ref().map(|k| k.0 == d.name && k.1 == d.card).unwrap_or(false) {
                    h.style_context().add_class("hw-row-sel");
                    *sel.borrow_mut() = Some(d.clone());
                }
                rows.borrow_mut().push((row, d));
            }
            list.show_all();
            ub();
        })
    };
    // the rows hold the Filler weakly; the fill function goes in now
    *filler.f.borrow_mut() = Some(fill);
    // menu of the split button
    {
        let (sel, snap, fl) = (sel.clone(), snap.clone(), Rc::downgrade(&filler));
        let menu = gtk::Menu::new();
        for (label, which) in [("Default Device", 1u8), ("Default Communication Device", 2)] {
            let it = gtk::MenuItem::with_label(label);
            let (sel, snap, fl) = (sel.clone(), snap.clone(), fl.clone());
            it.connect_activate(move |_| { if let (Some(d), Some(f)) = (sel.borrow().clone(), fl.upgrade()) { set_default(&d, which, &snap.borrow()); f.reload(); } });
            menu.append(&it);
        }
        menu.show_all();
        arrow.set_popup(Some(&menu));
    }
    { let (sel, snap, fl) = (sel.clone(), snap.clone(), Rc::downgrade(&filler)); setdef.connect_clicked(move |_| {
        // the button itself makes it both the default and the default communication device
        if let (Some(d), Some(f)) = (sel.borrow().clone(), fl.upgrade()) { set_default(&d, 3, &snap.borrow()); f.reload(); }
    }); }
    { let (sel, snap, fl, w) = (sel.clone(), snap.clone(), Rc::downgrade(&filler), win.clone()); props.connect_clicked(move |_| {
        if let (Some(d), Some(f)) = (sel.borrow().clone(), fl.upgrade()) { properties(&w, &d, &snap.borrow().cards, &f); }
    }); }
    { let (sel, snap, fl, w) = (sel.clone(), snap.clone(), Rc::downgrade(&filler), win.clone()); configure.connect_clicked(move |_| {
        if let (Some(d), Some(f)) = (sel.borrow().clone(), fl.upgrade()) { speaker_setup(&w, &d, &snap.borrow().cards, &f); }
    }); }
    (v, filler)
}

fn comms_default(sinks: bool, s: &Snapshot) -> String {
    let h = config::get().hw;
    let c = if sinks { h.comms_sink } else { h.comms_source };
    if c.is_empty() || !(if sinks { &s.sinks } else { &s.sources }).iter().any(|d| d.name == c) { if sinks { s.def_sink.clone() } else { s.def_source.clone() } } else { c }
}

/// which: 1 = default device, 2 = default communication device, 3 = both.
fn set_default(d: &Dev, which: u8, _s: &Snapshot) {
    if which & 1 != 0 {
        pactl_do(&[if d.sink { "set-default-sink" } else { "set-default-source" }, &d.name]);
    }
    if which & 2 != 0 {
        let n = d.name.clone();
        config::update(|c| if d.sink { c.hw.comms_sink = n } else { c.hw.comms_source = n });
    }
}

/// The 48 px device icon with Win7's green check (default) or green phone (communications) badge.
fn device_icon(icon: &str, badge: u8, dim: bool) -> gtk::DrawingArea {
    let pb = util::pixbuf(icon, 48);
    let d = gtk::DrawingArea::new();
    d.set_size_request(52, 52);
    d.connect_draw(move |_, cr| {
        use gtk::gdk::prelude::GdkContextExt;
        if let Some(p) = &pb {
            cr.set_source_pixbuf(p, 2.0, 2.0);
            if dim { cr.paint_with_alpha(0.45).ok(); } else { cr.paint().ok(); }
        }
        if badge > 0 {
            let (x, y, r) = (40.0, 40.0, 9.0);
            cr.arc(x, y, r, 0.0, std::f64::consts::TAU);
            let g = gtk::cairo::RadialGradient::new(x - 3.0, y - 3.0, 1.0, x, y, r);
            g.add_color_stop_rgb(0.0, 0.55, 0.90, 0.45);
            g.add_color_stop_rgb(1.0, 0.10, 0.55, 0.10);
            cr.set_source(&g).ok();
            cr.fill_preserve().ok();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.set_line_width(1.5);
            cr.stroke().ok();
            cr.set_line_width(2.4);
            cr.set_line_cap(gtk::cairo::LineCap::Round);
            if badge == 1 {
                cr.move_to(x - 4.5, y + 0.5);
                cr.line_to(x - 1.0, y + 4.0);
                cr.line_to(x + 5.0, y - 3.5);
            } else {
                // a telephone handset
                cr.move_to(x - 5.0, y - 2.0);
                cr.curve_to(x - 5.0, y + 5.0, x + 1.0, y + 5.0, x + 5.0, y + 2.5);
            }
            cr.stroke().ok();
        }
        glib::Propagation::Stop
    });
    d
}

fn context_menu(win: &gtk::Window, d: &Dev, s: &Snapshot, f: &Rc<Filler>, e: &gtk::gdk::EventButton) {
    let m = gtk::Menu::new();
    let add = |label: &str, cb: Box<dyn Fn()>| { let it = gtk::MenuItem::with_label(label); it.connect_activate(move |_| cb()); m.append(&it); it };
    if d.disabled {
        let (d2, cards, f2) = (d.clone(), s.cards.clone(), f.clone());
        add("Enable", Box::new(move || { enable_card(&d2, &cards, true); f2.reload(); }));
    } else {
        if d.sink { let n = d.name.clone(); add("Test", Box::new(move || test_sound(&n))); }
        let (d2, cards, f2) = (d.clone(), s.cards.clone(), f.clone());
        add("Disable", Box::new(move || { enable_card(&d2, &cards, false); f2.reload(); }));
        m.append(&gtk::SeparatorMenuItem::new());
        let (d2, s2, f2) = (d.clone(), s.clone(), f.clone());
        add("Set as Default Device", Box::new(move || { set_default(&d2, 1, &s2); f2.reload(); }));
        let (d2, s2, f2) = (d.clone(), s.clone(), f.clone());
        add("Set as Default Communication Device", Box::new(move || { set_default(&d2, 2, &s2); f2.reload(); }));
    }
    m.append(&gtk::SeparatorMenuItem::new());
    let (d2, cards, f2, w) = (d.clone(), s.cards.clone(), f.clone(), win.clone());
    add("Properties", Box::new(move || properties(&w, &d2, &cards, &f2)));
    m.show_all();
    m.popup_at_pointer(Some(e));
}

/// Device usage: a card's profile Off (disable) or its best profile (enable).
fn enable_card(d: &Dev, cards: &str, on: bool) {
    if d.card.is_empty() { return; }
    if !on { pactl_do(&["set-card-profile", &d.card, "off"]); return; }
    let (profiles, _) = card_profiles(cards, &d.card);
    if let Some(p) = profiles.iter().find(|p| p.0 != "off" && (if d.sink { p.3 } else { p.4 }) > 0) { pactl_do(&["set-card-profile", &d.card, &p.0]); }
}

fn test_sound(sink: &str) {
    let Some(f) = config::data_file("sounds/Windows Background.wav").or_else(|| sounds::file_for(sounds::Event::DefaultBeep)) else { return };
    if std::env::var_os("AERODE_PACTL_DIR").is_some() { eprintln!("aerode-control (test): paplay -d {sink}"); return; }
    if !util::spawn(&["paplay", "-d", sink, &f.to_string_lossy()]) { util::spawn(&["pw-play", "--target", sink, &f.to_string_lossy()]); }
}

/// Configure = Win7 "Speaker Setup": the audio channels (card profile).
fn speaker_setup(win: &gtk::Window, d: &Dev, cards: &str, f: &Rc<Filler>) {
    let (profiles, active) = card_profiles(cards, &d.card);
    let outs: Vec<_> = profiles.into_iter().filter(|p| p.0 != "off" && p.3 > 0).collect();
    let dlg = ok_cancel(Some(win), "Speaker Setup");
    let c = dlg.content_area();
    let h = gtk::Label::new(None);
    h.set_markup("<span size='large' foreground='#1e3287'>Choose your configuration</span>");
    h.set_xalign(0.0);
    c.pack_start(&h, false, false, 0);
    c.pack_start(&wrap("Select the speaker setup below that is most like the configuration on your computer.", 50), false, false, 0);
    c.pack_start(&lbl("Audio channels:"), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    super::super::admin::columns(&tv, &[("Audio channels", 0, 0)]);
    tv.set_headers_visible(false);
    for p in &outs {
        let it = store.insert_with_values(None, &[(0, &p.1), (1, &p.0)]);
        if p.0 == active { tv.selection().select_iter(&it); }
    }
    let sw = scrolled(&tv);
    sw.set_size_request(320, 140);
    c.pack_start(&sw, false, false, 0);
    let test = gtk::Button::with_mnemonic("_Test");
    test.set_halign(gtk::Align::Start);
    { let n = d.name.clone(); test.connect_clicked(move |_| test_sound(&n)); }
    c.pack_start(&test, false, false, 0);
    if let Some(b) = dlg.widget_for_response(gtk::ResponseType::Ok).and_then(|b| b.downcast::<gtk::Button>().ok()) { b.set_label("Next"); }
    dlg.show_all();
    if dlg.run() == gtk::ResponseType::Ok {
        if let Some((m, it)) = tv.selection().selected() {
            let p = super::super::admin::sval(&m, &it, 1);
            if p != active { pactl_do(&["set-card-profile", &d.card, &p]); f.reload(); }
        }
    }
    dlg.close();
}

// ------------------------------------------------------------------ device properties

/// Win7's default formats: (label, bits, rate).
const FORMATS: &[(&str, u32, u32)] = &[
    ("16 bit, 44100 Hz (CD Quality)", 16, 44100), ("16 bit, 48000 Hz (DVD Quality)", 16, 48000),
    ("24 bit, 44100 Hz (Studio Quality)", 24, 44100), ("24 bit, 48000 Hz (Studio Quality)", 24, 48000),
    ("24 bit, 96000 Hz (Studio Quality)", 24, 96000), ("24 bit, 192000 Hz (Studio Quality)", 24, 192000),
];

fn rules_file() -> std::path::PathBuf { glib::user_config_dir().join("wireplumber/wireplumber.conf.d/51-aerode-devices.conf") }

/// Per-device overrides we keep in the WirePlumber rule file: node name -> (description, format, rate).
fn read_rules() -> Vec<(String, String, String, u32)> {
    let s = std::fs::read_to_string(rules_file()).unwrap_or_default();
    // one rule per line pair written by `write_rules`: "# node <name>\t<desc>\t<format>\t<rate>"
    s.lines().filter_map(|l| l.strip_prefix("# node ")).filter_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        Some((f.first()?.to_string(), f.get(1).unwrap_or(&"").to_string(), f.get(2).unwrap_or(&"").to_string(), f.get(3).and_then(|v| v.parse().ok()).unwrap_or(0)))
    }).collect()
}

fn write_rules(rules: &[(String, String, String, u32)]) {
    let mut s = String::from("# written by aerode (Control Panel > Sound > Properties): device names and default formats\nmonitor.alsa.rules = [\n");
    for (name, desc, fmt, rate) in rules {
        s += &format!("# node {name}\t{desc}\t{fmt}\t{rate}\n");
        let mut props = vec![];
        if !desc.is_empty() { props.push(format!("node.description = \"{}\"", desc.replace('"', "'"))); }
        if !fmt.is_empty() { props.push(format!("audio.format = \"{fmt}\"")); }
        if *rate > 0 { props.push(format!("audio.rate = {rate}")); }
        s += &format!("  {{ matches = [ {{ node.name = \"{name}\" }} ]\n    actions = {{ update-props = {{ {} }} }} }}\n", props.join(", "));
    }
    s += "]\n";
    let p = rules_file();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let _ = std::fs::write(&p, s);
}

fn properties(win: &gtk::Window, d: &Dev, cards: &str, f: &Rc<Filler>) {
    let s = DlgSheet::new(Some(win), &format!("{} Properties", d.title), true);
    let rules = read_rules();
    let mine = rules.iter().find(|r| r.0 == d.name).cloned();
    // General
    let t = vbox(10);
    let top = hbox(10);
    top.pack_start(&util::image(util::first_icon(d.icon), 32), false, false, 0);
    let name = gtk::Entry::new();
    name.set_text(&mine.as_ref().map(|r| r.1.clone()).filter(|x| !x.is_empty()).unwrap_or_else(|| d.title.clone()));
    name.set_hexpand(true);
    name.set_sensitive(!d.name.is_empty());
    name.set_tooltip_text(Some(&d.desc));
    top.pack_start(&name, true, true, 0);
    t.pack_start(&top, false, false, 0);
    t.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
    let ci = gtk::Grid::new();
    ci.set_row_spacing(4);
    ci.set_column_spacing(10);
    let hwl = lbl(&d.hw);
    hwl.set_hexpand(true);
    ci.attach(&hwl, 0, 0, 1, 1);
    let cprops = gtk::Button::with_mnemonic("P_roperties");
    cprops.set_hexpand(false);
    cprops.set_halign(gtk::Align::End);
    cprops.connect_clicked(|_| spawn_tool("devmgr"));
    ci.attach(&cprops, 1, 0, 1, 1);
    t.pack_start(&group("Controller Information", &ci), false, false, 0);
    let jacks = vbox(2);
    if d.ports.is_empty() { jacks.pack_start(&lbl("No jack information available"), false, false, 0); }
    for (pname, desc, avail) in &d.ports {
        let state = if *pname == d.port { " (in use)" } else if *avail { "" } else { " (not plugged in)" };
        jacks.pack_start(&lbl(&format!("{desc}{state}")), false, false, 0);
    }
    t.pack_start(&group("Jack Information", &jacks), false, false, 0);
    let ur = hbox(10);
    ur.pack_start(&lbl("Device usage:"), false, false, 0);
    let usage = combo(&[("on", "Use this device (enable)"), ("off", "Don't use this device (disable)")], if d.disabled { "off" } else { "on" });
    usage.set_sensitive(!d.card.is_empty());
    ur.pack_start(&usage, true, true, 0);
    t.pack_start(&ur, false, false, 0);
    s.watch_entry(&name);
    s.watch_combo(&usage);
    s.tab("General", &t);
    // Listen (recording devices): play the device through a playback device (module-loopback)
    let listen_chk = check("_Listen to this device", loopback_of(&d.name).is_some());
    if !d.sink {
        let t = vbox(8);
        t.pack_start(&wrap("You can listen to a portable music player or other device through this microphone jack. If you connect a microphone, you might hear feedback.", 50), false, false, 0);
        t.pack_start(&listen_chk, false, false, 0);
        t.pack_start(&lbl("Playback through this device:"), false, false, 0);
        let pb = gtk::ComboBoxText::new();
        pb.append(Some(""), "Default Playback Device");
        pb.set_active(Some(0));
        pb.set_sensitive(false);
        t.pack_start(&pb, false, false, 0);
        s.watch_check(&listen_chk);
        s.tab("Listen", &t);
    }
    // Levels
    let t = vbox(10);
    let lv = hbox(8);
    lv.pack_start(&lbl(&d.title), false, false, 0);
    let avg = if d.vol.is_empty() { 100 } else { d.vol.iter().sum::<u32>() / d.vol.len() as u32 };
    let sc = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    sc.set_draw_value(false);
    sc.set_value(avg.min(100) as f64);
    sc.set_hexpand(true);
    lv.pack_start(&sc, true, true, 0);
    let val = gtk::SpinButton::with_range(0.0, 100.0, 1.0);
    val.set_value(avg.min(100) as f64);
    val.set_width_chars(3);
    lv.pack_start(&val, false, false, 0);
    let mute = gtk::ToggleButton::new();
    mute.set_active(d.mute);
    let mute_img = gtk::Image::new();
    let set_mute_img = { let i = mute_img.clone(); move |m: bool| { if let Some(p) = util::pixbuf(if m { "audio-volume-muted" } else { "audio-volume-high" }, 16) { i.set_from_pixbuf(Some(&p)); } } };
    set_mute_img(d.mute);
    mute.add(&mute_img);
    mute.set_tooltip_text(Some(if d.sink { "Mute" } else { "Mute" }));
    lv.pack_start(&mute, false, false, 0);
    let bal = gtk::Button::with_mnemonic("_Balance");
    lv.pack_start(&bal, false, false, 0);
    let lvbox = vbox(4);
    lvbox.pack_start(&lv, false, false, 0);
    t.pack_start(&lvbox, false, false, 0);
    let kind = if d.sink { "sink" } else { "source" };
    let (n1, n2, n3) = (d.name.clone(), d.name.clone(), d.name.clone());
    { let v = val.clone(); sc.connect_value_changed(move |s| { v.set_value(s.value()); pactl_do(&[&format!("set-{kind}-volume"), &n1, &format!("{}%", s.value() as u32)]); }); }
    { let s2 = sc.clone(); val.connect_value_changed(move |v| if (s2.value() - v.value()).abs() >= 1.0 { s2.set_value(v.value()); }); }
    mute.connect_toggled(move |b| { set_mute_img(b.is_active()); pactl_do(&[&format!("set-{kind}-mute"), &n2, if b.is_active() { "1" } else { "0" }]); });
    { let (w, vols) = (s.d.clone(), d.vol.clone()); bal.connect_clicked(move |_| balance(w.upcast_ref(), kind, &n3, &vols)); }
    if d.disabled || d.name.is_empty() { lvbox.set_sensitive(false); }
    s.tab("Levels", &t);
    // Advanced
    let t = vbox(10);
    let df = vbox(6);
    df.pack_start(&wrap("Select the sample rate and bit depth to be used when running in shared mode.", 50), false, false, 0);
    let fr = hbox(8);
    let fmt = gtk::ComboBoxText::new();
    for (i, (l, _, _)) in FORMATS.iter().enumerate() { fmt.append(Some(&i.to_string()), l); }
    // the device's current format ("s32le 2ch 48000Hz") or what we set
    let rate = mine.as_ref().map(|r| r.3).filter(|r| *r > 0).or_else(|| d.spec.split_whitespace().last().and_then(|r| r.trim_end_matches("Hz").parse().ok())).unwrap_or(48000);
    let bits = match mine.as_ref().map(|r| r.2.as_str()).filter(|x| !x.is_empty()).unwrap_or(d.spec.split_whitespace().next().unwrap_or("s16le")) { f if f.starts_with("s16") || f.starts_with("S16") => 16, _ => 24 };
    let cur = FORMATS.iter().position(|x| x.1 == bits && x.2 == rate).or_else(|| FORMATS.iter().position(|x| x.2 == rate)).unwrap_or(3);
    fmt.set_active(Some(cur as u32));
    fmt.set_hexpand(true);
    fr.pack_start(&fmt, true, true, 0);
    let test = gtk::Button::with_mnemonic("_Test");
    { let n = d.name.clone(); test.connect_clicked(move |_| test_sound(&n)); }
    test.set_sensitive(d.sink);
    fr.pack_start(&test, false, false, 0);
    df.pack_start(&fr, false, false, 0);
    t.pack_start(&group("Default Format", &df), false, false, 0);
    let ex = vbox(6);
    let ex1 = check("_Allow applications to take exclusive control of this device", true);
    let ex2 = check("_Give exclusive mode applications priority", true);
    for c in [&ex1, &ex2] { c.set_sensitive(false); ex.pack_start(c, false, false, 0); }
    t.pack_start(&group("Exclusive Mode", &ex), false, false, 0);
    let rd = gtk::Button::with_mnemonic("_Restore Defaults");
    rd.set_halign(gtk::Align::End);
    { let f2 = fmt.clone(); rd.connect_clicked(move |_| f2.set_active(Some(3))); }
    t.pack_start(&rd, false, false, 0);
    s.watch_combo(&fmt);
    if d.disabled || d.name.is_empty() { t.set_sensitive(false); }
    s.tab("Advanced", &t);

    let (d2, cards, f2, orig_fmt) = (d.clone(), cards.to_string(), f.clone(), cur);
    s.on_apply(move || {
        let want_on = usage.active_id().as_deref() == Some("on");
        if want_on == d2.disabled { enable_card(&d2, &cards, want_on); }
        if !d2.sink && !d2.name.is_empty() {
            let on = listen_chk.is_active();
            if on != loopback_of(&d2.name).is_some() { set_loopback(&d2.name, on); }
        }
        // name and default format: a WirePlumber rule, read when WirePlumber starts
        let new_name = name.text().trim().to_string();
        let fi = fmt.active().unwrap_or(3) as usize;
        let name_changed = !d2.name.is_empty() && new_name != d2.title && !new_name.is_empty();
        if name_changed || fi != orig_fmt {
            let mut rules = read_rules();
            let old = rules.iter().find(|r| r.0 == d2.name).cloned().unwrap_or((d2.name.clone(), String::new(), String::new(), 0));
            rules.retain(|r| r.0 != d2.name);
            let (_, b, r) = FORMATS[fi];
            rules.push((d2.name.clone(), if name_changed { new_name.clone() } else { old.1 }, if fi != orig_fmt { if b == 16 { "S16LE".into() } else { "S24LE".into() } } else { old.2 }, if fi != orig_fmt { r } else { old.3 }));
            write_rules(&rules);
            if std::env::var_os("AERODE_PACTL_DIR").is_none() && util::which("wpctl") {
                // WirePlumber reads its rules at start: restart it (audio pauses for a moment)
                util::spawn(&["systemctl", "--user", "restart", "wireplumber.service"]);
            }
        }
        f2.reload();
        true
    });
    s.show(0);
}

/// The module-loopback that plays this source, if one is loaded by us.
fn loopback_of(source: &str) -> Option<String> {
    if std::env::var_os("AERODE_PACTL_DIR").is_some() { return None; }
    let s = util::output(&["pactl", "list", "short", "modules"])?;
    s.lines().find(|l| l.contains("module-loopback") && l.contains(&format!("source={source}")) && l.contains("aerode-listen")).and_then(|l| l.split_whitespace().next().map(String::from))
}

fn set_loopback(source: &str, on: bool) {
    if on { pactl_do(&["load-module", "module-loopback", &format!("source={source}"), "sink_input_properties=media.name=aerode-listen", "latency_msec=60"]); }
    else if let Some(id) = loopback_of(source) { pactl_do(&["unload-module", &id]); }
}

/// Win7 "Balance": one slider per channel.
fn balance(parent: &gtk::Window, kind: &'static str, name: &str, vols: &[u32]) {
    let d = ok_cancel(Some(parent), "Balance");
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(10);
    let labels = ["L", "R", "C", "LFE", "RL", "RR", "SL", "SR"];
    let n = vols.len().max(2);
    let scales: Vec<gtk::Scale> = (0..n).map(|i| {
        g.attach(&lbl(labels.get(i).copied().unwrap_or("?")), 0, i as i32, 1, 1);
        let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        s.set_value(*vols.get(i).unwrap_or(&100) as f64);
        s.set_size_request(220, -1);
        s.set_value_pos(gtk::PositionType::Right);
        s.set_digits(0);
        g.attach(&s, 1, i as i32, 1, 1);
        s
    }).collect();
    let apply = { let (scales, name) = (scales.clone(), name.to_string()); Rc::new(move || {
        let mut a: Vec<String> = vec![format!("set-{kind}-volume"), name.clone()];
        a.extend(scales.iter().map(|s| format!("{}%", s.value() as u32)));
        let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        pactl_do(&r);
    }) };
    for s in &scales { let a = apply.clone(); s.connect_value_changed(move |_| a()); }
    d.content_area().pack_start(&g, false, false, 0);
    d.show_all();
    let r = d.run();
    if r != gtk::ResponseType::Ok {
        // Cancel puts the old levels back
        let mut a: Vec<String> = vec![format!("set-{kind}-volume"), name.to_string()];
        a.extend(vols.iter().map(|v| format!("{v}%")));
        let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        if !vols.is_empty() { pactl_do(&r); }
    }
    d.close();
}

// ------------------------------------------------------------------ Sounds tab

/// The scheme editor; returns the tab and its Apply.
fn sounds_tab(sheet: &Rc<Sheet>) -> (gtk::Box, Box<dyn Fn()>) {
    let v = vbox(6);
    v.pack_start(&wrap("A sound theme is a set of sounds applied to events in Windows and programs. You can select an existing scheme or save one you have modified.", 52), false, false, 0);
    let c0 = config::get();
    // working copies (Apply saves them)
    let map: Rc<RefCell<std::collections::HashMap<String, String>>> = Rc::new(RefCell::new(c0.sounds.clone()));
    let muted = Rc::new(Cell::new(c0.sounds_muted));
    v.pack_start(&lbl("Sound Sc_heme:"), false, false, 0);
    let sr = hbox(6);
    let scheme = gtk::ComboBoxText::new();
    scheme.append(Some("default"), "Windows Default");
    scheme.append(Some("none"), "No Sounds");
    scheme.append(Some("modified"), "Windows Default (modified)");
    let scheme_id = |m: &std::collections::HashMap<String, String>, muted: bool| if muted { "none" } else if m.is_empty() { "default" } else { "modified" };
    scheme.set_active_id(Some(scheme_id(&map.borrow(), muted.get())));
    scheme.set_hexpand(true);
    sr.pack_start(&scheme, true, true, 0);
    let save_as = gtk::Button::with_mnemonic("Sa_ve As...");
    let delete = gtk::Button::with_mnemonic("_Delete");
    save_as.set_sensitive(false);
    delete.set_sensitive(false);
    sr.pack_start(&save_as, false, false, 0);
    sr.pack_start(&delete, false, false, 0);
    v.pack_start(&sr, false, false, 0);
    v.pack_start(&wrap("To change sounds, click a program event in the following list and then select a sound to apply. You can save the changes as a new sound scheme.", 52), false, false, 4);
    v.pack_start(&lbl("Program _Events:"), false, false, 0);
    // columns: label, file shown, event index, has sound (icon)
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::U32, gtk::gdk_pixbuf::Pixbuf::static_type()]);
    let speaker = util::pixbuf(util::first_icon(&["audio-volume-high"]), 16);
    let file_of = { let (map, muted) = (map.clone(), muted.clone()); move |ev: sounds::Event| -> Option<String> {
        if muted.get() { return None; }
        match map.borrow().get(ev.key()) {
            Some(f) if f.is_empty() => None,
            Some(f) => Some(f.clone()),
            None => (!ev.default_file().is_empty()).then(|| ev.default_file().to_string()),
        }
    } };
    let fill = { let (store, f, sp) = (store.clone(), file_of.clone(), speaker.clone()); Rc::new(move || {
        store.clear();
        let root = store.insert_with_values(None, None, &[(0, &"Windows"), (2, &u32::MAX)]);
        for (i, ev) in sounds::Event::ALL.iter().enumerate() {
            let file = f(*ev);
            let shown = file.as_deref().map(|p| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).unwrap_or_default();
            let it = store.insert_with_values(Some(&root), None, &[(0, &ev.label()), (1, &shown), (2, &(i as u32))]);
            if file.is_some() { store.set(&it, &[(3, &sp)]); }
        }
    }) };
    fill();
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let pr = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&col, &pr, false);
    TreeViewColumnExt::add_attribute(&col, &pr, "pixbuf", 3);
    let tr = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &tr, true);
    TreeViewColumnExt::add_attribute(&col, &tr, "text", 0);
    tv.append_column(&col);
    tv.expand_all();
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 150);
    v.pack_start(&sw, true, true, 0);
    let startup = check("_Play Windows Startup sound", map.borrow().get(sounds::Event::Logon.key()).map(|f| !f.is_empty()).unwrap_or(true));
    v.pack_start(&startup, false, false, 0);
    v.pack_start(&lbl("_Sounds:"), false, false, 0);
    let row = hbox(6);
    let pick = gtk::ComboBoxText::new();
    pick.append(Some(""), "(None)");
    let mut names: Vec<String> = std::fs::read_dir(config::data_dir().join("sounds")).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    for n in &names { pick.append(Some(n), n.trim_end_matches(".wav")); }
    pick.set_hexpand(true);
    pick.set_sensitive(false);
    let test = gtk::Button::with_mnemonic("_Test");
    let browse = gtk::Button::with_mnemonic("_Browse...");
    test.set_sensitive(false);
    browse.set_sensitive(false);
    row.pack_start(&pick, true, true, 0);
    row.pack_start(&test, false, false, 0);
    row.pack_start(&browse, false, false, 0);
    v.pack_start(&row, false, false, 0);
    let sel: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let setting = Rc::new(Cell::new(false));
    let dirty = sheet.dirty();
    { let (pick, sel, f, test, browse, setting) = (pick.clone(), sel.clone(), file_of.clone(), test.clone(), browse.clone(), setting.clone()); tv.selection().connect_changed(move |s| {
        let i = s.selected().map(|(m, it)| m.value(&it, 2).get::<u32>().unwrap_or(u32::MAX)).unwrap_or(u32::MAX);
        let ok = i != u32::MAX;
        sel.set(ok.then_some(i as usize));
        for w in [pick.upcast_ref::<gtk::Widget>(), test.upcast_ref(), browse.upcast_ref()] { w.set_sensitive(ok); }
        if !ok { return; }
        let file = f(sounds::Event::ALL[i as usize]).unwrap_or_default();
        let base = std::path::Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        setting.set(true);
        if !pick.set_active_id(Some(&base)) { if file.is_empty() { pick.set_active_id(Some("")); } else { pick.append(Some(&file), &base); pick.set_active_id(Some(&file)); } }
        setting.set(false);
    }); }
    let changed = { let (scheme, map, muted, fill, dirty, setting) = (scheme.clone(), map.clone(), muted.clone(), fill.clone(), dirty.clone(), setting.clone()); Rc::new(move || {
        setting.set(true);
        scheme.set_active_id(Some(scheme_id(&map.borrow(), muted.get())));
        setting.set(false);
        fill();
        dirty();
    }) };
    { let (sel, map, muted, setting, ch, tv) = (sel.clone(), map.clone(), muted.clone(), setting.clone(), changed.clone(), tv.clone()); pick.connect_changed(move |c| {
        if setting.get() { return; }
        let (Some(i), Some(id)) = (sel.get(), c.active_id()) else { return };
        let ev = sounds::Event::ALL[i];
        // a pick in "No Sounds" starts a modified default scheme
        if muted.get() { muted.set(false); map.borrow_mut().clear(); for e in sounds::Event::ALL { map.borrow_mut().insert(e.key().to_string(), String::new()); } }
        if id.as_str() == ev.default_file() { map.borrow_mut().remove(ev.key()); } else { map.borrow_mut().insert(ev.key().to_string(), id.to_string()); }
        let path = tv.selection().selected().and_then(|(m, it)| m.path(&it));
        ch();
        tv.expand_all();
        if let Some(p) = path { tv.selection().select_path(&p); }
    }); }
    { let (scheme, map, muted, setting, fill, dirty, tv) = (scheme.clone(), map.clone(), muted.clone(), setting.clone(), fill.clone(), dirty.clone(), tv.clone()); scheme.connect_changed(move |c| {
        if setting.get() { return; }
        match c.active_id().as_deref() {
            Some("none") => muted.set(true),
            Some("default") => { muted.set(false); map.borrow_mut().clear(); }
            _ => muted.set(false),
        }
        fill();
        tv.expand_all();
        dirty();
    }); }
    { let (pick, sel) = (pick.clone(), sel.clone()); test.connect_clicked(move |_| {
        let Some(_) = sel.get() else { return };
        let Some(id) = pick.active_id().filter(|s| !s.is_empty()) else { return };
        let p = if id.starts_with('/') { std::path::PathBuf::from(id.as_str()) } else { config::data_dir().join("sounds").join(id.as_str()) };
        for pl in ["pw-play", "paplay", "aplay"] { if util::spawn(&[pl, &p.to_string_lossy()]) { break; } }
    }); }
    { let (pick, w) = (pick.clone(), sheet.nb.clone()); browse.connect_clicked(move |_| {
        let parent = w.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok());
        let fc = gtk::FileChooserNative::new(Some("Browse for new Program Event sound"), parent.as_ref(), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
        let ff = gtk::FileFilter::new();
        for p in ["*.wav", "*.ogg", "*.oga", "*.flac"] { ff.add_pattern(p); }
        ff.set_name(Some("Wave Files (*.wav)"));
        fc.add_filter(ff);
        if fc.run() == gtk::ResponseType::Accept {
            if let Some(p) = fc.filename() {
                let s = p.to_string_lossy().into_owned();
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                pick.append(Some(&s), &name);
                pick.set_active_id(Some(&s));
            }
        }
    }); }
    { let (map, d) = (map.clone(), dirty.clone()); startup.connect_toggled(move |c| {
        if c.is_active() { map.borrow_mut().remove(sounds::Event::Logon.key()); } else { map.borrow_mut().insert(sounds::Event::Logon.key().to_string(), String::new()); }
        d();
    }); }
    let apply = Box::new(move || {
        let (m, mu) = (map.borrow().clone(), muted.get());
        config::update(|c| { c.sounds = m; c.sounds_muted = mu; });
    });
    (v, apply)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_pactl_json() {
        let j = r#"[{"index":55,"state":"RUNNING","name":"alsa_output.pci.analog-stereo","description":"Built-in Audio Analog Stereo","sample_specification":"s32le 2ch 48000Hz","mute":false,
            "volume":{"front-left":{"value":42598,"value_percent":"65%","db":"-11 dB"},"front-right":{"value":42598,"value_percent":"64%","db":"-11 dB"}},
            "monitor_source":"alsa_output.pci.analog-stereo.monitor","properties":{"device.product.name":"Cannon Lake PCH cAVS","device.name":"alsa_card.pci"},
            "ports":[{"name":"analog-output-headphones","description":"Headphones","type":"Headphones","availability":"available"}],"active_port":"analog-output-headphones"}]"#;
        let d = parse_devs(j, true);
        assert_eq!(d.len(), 1);
        assert_eq!((d[0].title.as_str(), d[0].hw.as_str(), d[0].card.as_str()), ("Headphones", "Cannon Lake PCH cAVS", "alsa_card.pci"));
        assert_eq!(d[0].vol, vec![65, 64]);
        assert!(!d[0].unplugged);
        let src = r#"[{"name":"x.monitor","description":"Monitor of X","properties":{"device.class":"monitor"}}]"#;
        assert!(parse_devs(src, false).is_empty());
    }
}
