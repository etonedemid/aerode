//! Windows Mobility Center: the tile window with display brightness (backlight or DDC/CI),
//! volume, battery and power plan, wireless on/off, external display and presentation settings
//! (screen saver and display power management off while presenting).
use gtk::prelude::*;
use gtk::glib;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use aerode::{config, sys, util};
use crate::Cp;
use super::common::*;

const CSS: &str = r#"
.mbl-win { background-image: linear-gradient(to bottom, #ffffff, #eaf1fa); }
.mbl-tile { background-image: linear-gradient(to bottom, #ffffff, #f0f5fc); border: 1px solid #c3d3ea; border-radius: 4px; padding: 6px 8px 6px 6px; }
.mbl-title { color: #1e3287; font-size: 10pt; }
.mbl-status { color: #333333; }
button.mbl-icon { background: none; border: 1px solid transparent; box-shadow: none; border-radius: 3px; padding: 2px; }
button.mbl-icon:hover { background-image: linear-gradient(to bottom, #f5faff, #dcecfd); border-color: #b5d3f6; }
"#;

pub fn page_mobility(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if !DONE.with(|d| d.replace(true)) { aerode::theme::load_css(CSS); }
    let me = cp.clone();
    standalone(cp, move |parent| window(&me, parent))
}

fn open_cp(cp: &Rc<Cp>, page: &str) {
    if cp.win.is_visible() { cp.open(page); cp.win.present(); } else { util::spawn(&["aerode-control", page]); }
}

struct Tile { root: gtk::Box, icon: gtk::Button, status: gtk::Label, body: gtk::Box }

fn tile(icon: &str, name: &str) -> Tile {
    let root = vbox(4);
    root.style_context().add_class("mbl-tile");
    root.set_size_request(158, 128);
    let top = hbox(6);
    let ib = gtk::Button::new();
    ib.style_context().add_class("mbl-icon");
    ib.set_relief(gtk::ReliefStyle::None);
    ib.set_valign(gtk::Align::Start);
    ib.add(&util::image(icon, 32));
    top.pack_start(&ib, false, false, 0);
    let right = vbox(6);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_line_wrap(true);
    status.set_max_width_chars(16);
    status.style_context().add_class("mbl-status");
    right.pack_start(&status, false, false, 0);
    let body = vbox(4);
    right.pack_start(&body, false, false, 0);
    top.pack_start(&right, true, true, 0);
    root.pack_start(&top, true, true, 0);
    let t = gtk::Label::new(Some(name));
    t.set_xalign(0.0);
    t.style_context().add_class("mbl-title");
    root.pack_end(&t, false, false, 0);
    Tile { root, icon: ib, status, body }
}

fn set_icon(b: &gtk::Button, icon: &str) {
    if let Some(c) = b.child() { b.remove(&c); }
    b.add(&util::image(icon, 32));
    b.show_all();
}

fn window(cp: &Rc<Cp>, parent: Option<&gtk::Window>) -> gtk::Window {
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title("Windows Mobility Center");
    w.set_icon_name(Some(util::first_icon(&["computer-laptop", "computer"])));
    w.set_resizable(false);
    if let Some(p) = parent { w.set_transient_for(Some(p)); w.set_position(gtk::WindowPosition::CenterOnParent); } else { w.set_position(gtk::WindowPosition::Center); }
    let bg = vbox(0);
    bg.style_context().add_class("mbl-win");
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(8);
    grid.set_border_width(12);
    // every tile the same size, four to a row (Windows 7 order)
    grid.set_column_homogeneous(true);
    grid.set_row_homogeneous(true);
    let tiles = [brightness(cp), volume(cp), battery(cp), wireless(cp), external(cp), sync(), presentation(&w)];
    for (i, t) in tiles.iter().enumerate() { grid.attach(t, (i % 4) as i32, (i / 4) as i32, 1, 1); }
    bg.pack_start(&grid, true, true, 0);
    w.add(&bg);
    w
}

// ------------------------------------------------------------------ brightness

fn brightness(cp: &Rc<Cp>) -> gtk::Widget {
    let t = tile(util::first_icon(&["display-brightness", "video-display", "preferences-desktop-display"]), "Display brightness");
    t.status.set_text("Checking…");
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 5.0);
    s.set_draw_value(false);
    s.set_size_request(100, -1);
    s.set_sensitive(false);
    t.body.pack_start(&s, false, false, 0);
    { let me = cp.clone(); t.icon.connect_clicked(move |_| open_cp(&me, "power")); }
    // what can be dimmed: the laptop panel and/or monitors that answer DDC/CI (slow to find)
    let targets: Rc<RefCell<(bool, Vec<u32>)>> = Rc::new(RefCell::new((false, vec![])));
    let ready = Rc::new(Cell::new(false));
    {
        let (s, st, targets, ready) = (s.clone(), t.status.clone(), targets.clone(), ready.clone());
        background(|| {
            let bl = sys::backlight();
            let ddc: Vec<u32> = sys::ddc_displays().into_iter().map(|(n, _)| n).collect();
            let val = bl.or_else(|| ddc.first().and_then(|n| sys::ddc_get(*n)).map(|(c, m)| c * 100 / m.max(1)));
            (bl.is_some(), ddc, val)
        }, move |(bl, ddc, val)| {
            match val {
                Some(v) => { s.set_value(v as f64); s.set_sensitive(true); st.set_text(&format!("{v}%")); }
                None => st.set_text("Not available"),
            }
            *targets.borrow_mut() = (bl, ddc);
            ready.set(true);
        });
    }
    let pending: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));
    let st = t.status.clone();
    s.connect_value_changed(move |s| {
        if !ready.get() { return; }
        let v = s.value().round() as u32;
        st.set_text(&format!("{v}%"));
        if let Some(id) = pending.take() { id.remove(); }
        let (targets, p2) = (targets.clone(), pending.clone());
        // ddcutil is slow: only send the value the slider settles on
        pending.set(Some(glib::timeout_add_local_once(Duration::from_millis(180), move || {
            p2.set(None);
            let (bl, ddc) = targets.borrow().clone();
            background(move || {
                if bl { exec(&sv(&["brightnessctl", "set", &format!("{}%", v.clamp(1, 100))]), None); }
                for n in ddc { sys::ddc_set(n, v); }
            }, |_| {});
        })));
    });
    t.root.upcast()
}

// ------------------------------------------------------------------ volume

fn volume(cp: &Rc<Cp>) -> gtk::Widget {
    // the sound server is asked off the GTK thread (sys::query_async); the tile fills in when
    // the answer comes
    let t = tile(sys::volume_icon(Some((50, false))), "Volume");
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 2.0);
    s.set_draw_value(false);
    s.set_size_request(100, -1);
    let mute = gtk::CheckButton::with_mnemonic("_Mute");
    t.body.pack_start(&s, false, false, 0);
    t.body.pack_start(&mute, false, false, 0);
    { let me = cp.clone(); t.icon.connect_clicked(move |_| open_cp(&me, "sound")); }
    let loading = Rc::new(Cell::new(true));
    {
        let (s, mute, st, ib, loading) = (s.clone(), mute.clone(), t.status.clone(), t.icon.clone(), loading.clone());
        sys::query_async(false, move |info| {
            match info.and_then(|i| i.level()) {
                Some((v, m)) => { s.set_value(v as f64); mute.set_active(m); st.set_text(if m { "Muted" } else { "" }); }
                None => { s.set_sensitive(false); mute.set_sensitive(false); st.set_text("No audio device"); }
            }
            set_icon(&ib, sys::volume_icon(Some((s.value() as u32, mute.is_active()))));
            loading.set(false);
        });
    }
    { let (ib, st, mute, loading) = (t.icon.clone(), t.status.clone(), mute.clone(), loading.clone()); s.connect_value_changed(move |s| {
        if loading.get() { return; }
        let v = s.value().round() as u32;
        sys::set_volume(v);
        set_icon(&ib, sys::volume_icon(Some((v, mute.is_active()))));
        st.set_text(if mute.is_active() { "Muted" } else { "" });
    }); }
    { let (ib, st, s, loading) = (t.icon.clone(), t.status.clone(), s.clone(), loading.clone()); mute.connect_toggled(move |m| {
        if loading.get() { return; }
        // set the state explicitly: another program may have toggled it meanwhile
        sys::set_mute(m.is_active());
        set_icon(&ib, sys::volume_icon(Some((s.value() as u32, m.is_active()))));
        st.set_text(if m.is_active() { "Muted" } else { "" });
    }); }
    t.root.upcast()
}

// ------------------------------------------------------------------ battery

fn battery_text(b: &Option<sys::Battery>) -> String {
    match b {
        None => "No battery is detected".into(),
        Some(b) => match b.status.as_str() {
            "Full" => format!("Fully charged ({}%)", b.percent),
            "Charging" => format!("{}% available (plugged in, charging)", b.percent),
            "Not charging" => format!("{}% available (plugged in, not charging)", b.percent),
            _ => format!("{}% remaining", b.percent),
        },
    }
}

fn battery(cp: &Rc<Cp>) -> gtk::Widget {
    let b = sys::battery();
    let t = tile(&b.as_ref().map(sys::battery_icon).unwrap_or_else(|| util::first_icon(&["battery-missing", "battery"]).to_string()), "Battery Status");
    t.status.set_text(&battery_text(&b));
    let plan = gtk::ComboBoxText::new();
    for (id, l) in [("balanced", "Balanced"), ("power-saver", "Power saver"), ("performance", "High performance")] { plan.append(Some(id), l); }
    match sys::power_profile() {
        Some(p) => { plan.set_active_id(Some(&p)); }
        None => { plan.set_active_id(Some("balanced")); plan.set_sensitive(false); plan.set_tooltip_text(Some("Install power-profiles-daemon to change the power plan here.")); }
    }
    plan.connect_changed(|c| if let Some(id) = c.active_id() { sys::set_power_profile(&id); });
    t.body.pack_start(&plan, false, false, 0);
    { let me = cp.clone(); t.icon.connect_clicked(move |_| open_cp(&me, "power")); }
    let (st, ib) = (t.status.downgrade(), t.icon.downgrade());
    glib::timeout_add_seconds_local(5, move || {
        let (Some(st), Some(ib)) = (st.upgrade(), ib.upgrade()) else { return glib::ControlFlow::Break };
        let b = sys::battery();
        st.set_text(&battery_text(&b));
        if let Some(b) = &b { set_icon(&ib, &sys::battery_icon(b)); }
        glib::ControlFlow::Continue
    });
    t.root.upcast()
}

// ------------------------------------------------------------------ wireless

fn wireless(cp: &Rc<Cp>) -> gtk::Widget {
    let t = tile(util::first_icon(&["network-wireless", "network-wireless-signal-excellent"]), "Wireless Network");
    let btn = gtk::Button::with_label("Turn wireless off");
    btn.set_sensitive(false);
    t.body.pack_start(&btn, false, false, 0);
    { let me = cp.clone(); t.icon.connect_clicked(move |_| open_cp(&me, "network")); }
    let on = Rc::new(Cell::new(true));
    let refresh: Rc<dyn Fn()> = {
        let (st, btn, on) = (t.status.clone(), btn.clone(), on.clone());
        Rc::new(move || {
            let (st, btn, on) = (st.clone(), btn.clone(), on.clone());
            background(|| {
                if !util::which("nmcli") { return None; }
                let devs = util::output(&["nmcli", "-t", "-f", "TYPE", "device"]).unwrap_or_default();
                let has = devs.lines().any(|l| l.trim() == "wifi");
                Some((has, sys::wifi_enabled(), sys::net_status()))
            }, move |r| {
                match r {
                    None => { st.set_text("Not available"); btn.set_sensitive(false); }
                    Some((false, _, _)) => { st.set_text("No wireless adapter"); btn.set_sensitive(false); }
                    Some((true, enabled, net)) => {
                        on.set(enabled);
                        st.set_text(if !enabled { "Wireless is turned off" } else if net.kind == "wifi" { "Connected" } else { "Not connected" });
                        btn.set_label(if enabled { "Turn wireless off" } else { "Turn wireless on" });
                        btn.set_sensitive(true);
                    }
                }
            });
        })
    };
    refresh();
    { let (on, refresh) = (on.clone(), refresh.clone()); btn.connect_clicked(move |b| {
        b.set_sensitive(false);
        let r = refresh.clone();
        change(sv(&["nmcli", "radio", "wifi", if on.get() { "off" } else { "on" }]), None, move |_| r());
    }); }
    t.root.upcast()
}

// ------------------------------------------------------------------ external display

fn external(cp: &Rc<Cp>) -> gtk::Widget {
    let t = tile(util::first_icon(&["video-display", "preferences-desktop-display"]), "External Display");
    let n = connected_outputs().len();
    t.status.set_text(if n > 1 { "Display connected" } else { "No display connected" });
    // like Windows: "Connect display" opens the display switch (Win+P) chooser
    let b = gtk::Button::with_label("Connect display");
    t.body.pack_start(&b, false, false, 0);
    { let st = t.status.clone(); b.connect_clicked(move |b| display_switch(b.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok()).as_ref(), st.clone())); }
    { let me = cp.clone(); t.icon.connect_clicked(move |_| open_cp(&me, "display")); }
    t.root.upcast()
}

/// Connected xrandr outputs, the laptop's own panel first.
fn connected_outputs() -> Vec<String> {
    let q = util::output(&["xrandr", "--query"]).unwrap_or_default();
    let mut v: Vec<(bool, bool, String)> = q.lines().filter(|l| l.contains(" connected")).map(|l| {
        let name = l.split_whitespace().next().unwrap_or("").to_string();
        let internal = ["eDP", "LVDS", "DSI"].iter().any(|p| name.starts_with(p));
        (!internal, !l.contains(" primary"), name)
    }).collect();
    v.sort();
    v.into_iter().map(|(_, _, n)| n).collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Switch { ComputerOnly, Duplicate, Extend, ProjectorOnly }

/// The xrandr command for a display switch mode: the first output is the computer's own screen.
fn switch_argv(mode: Switch, outs: &[String]) -> Vec<String> {
    let mut a = sv(&["xrandr"]);
    let Some(main) = outs.first() else { return a };
    let rest = &outs[1..];
    match mode {
        Switch::ComputerOnly => {
            a.extend(sv(&["--output", main, "--auto", "--primary"]));
            for o in rest { a.extend(sv(&["--output", o, "--off"])); }
        }
        Switch::Duplicate => {
            a.extend(sv(&["--output", main, "--auto", "--primary"]));
            for o in rest { a.extend(sv(&["--output", o, "--auto", "--same-as", main])); }
        }
        Switch::Extend => {
            a.extend(sv(&["--output", main, "--auto", "--primary"]));
            let mut prev = main.clone();
            for o in rest { a.extend(sv(&["--output", o, "--auto", "--right-of", &prev])); prev = o.clone(); }
        }
        Switch::ProjectorOnly => {
            if rest.is_empty() { a.extend(sv(&["--output", main, "--auto"])); return a; }
            for (i, o) in rest.iter().enumerate() {
                a.extend(sv(&["--output", o, "--auto"]));
                if i == 0 { a.push("--primary".into()); } else { a.extend(sv(&["--same-as", &rest[0]])); }
            }
            a.extend(sv(&["--output", main, "--off"]));
        }
    }
    a
}

/// A small pictogram of the mode: the computer's screen on the left, the projector on the right.
fn switch_picture(mode: Switch) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_size_request(96, 64);
    d.connect_draw(move |_, cr| {
        let screen = |cr: &gtk::cairo::Context, x: f64, on: bool, wide: f64| {
            cr.rectangle(x, 10.0, wide, 34.0);
            if on {
                let g = gtk::cairo::LinearGradient::new(0.0, 10.0, 0.0, 44.0);
                g.add_color_stop_rgb(0.0, 0.45, 0.75, 0.98);
                g.add_color_stop_rgb(1.0, 0.10, 0.38, 0.78);
                let _ = cr.set_source(&g);
            } else { cr.set_source_rgb(0.20, 0.22, 0.26); }
            let _ = cr.fill_preserve();
            cr.set_source_rgb(0.85, 0.88, 0.92);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        };
        match mode {
            Switch::ComputerOnly => { screen(cr, 6.0, true, 38.0); screen(cr, 52.0, false, 38.0); }
            Switch::Duplicate => { screen(cr, 6.0, true, 38.0); screen(cr, 52.0, true, 38.0); }
            Switch::Extend => { screen(cr, 6.0, true, 84.0); cr.set_source_rgb(0.85, 0.88, 0.92); cr.move_to(48.0, 10.0); cr.line_to(48.0, 44.0); let _ = cr.stroke(); }
            Switch::ProjectorOnly => { screen(cr, 6.0, false, 38.0); screen(cr, 52.0, true, 38.0); }
        }
        // stands
        cr.set_source_rgb(0.85, 0.88, 0.92);
        cr.rectangle(20.0, 47.0, 10.0, 3.0);
        cr.rectangle(66.0, 47.0, 10.0, 3.0);
        let _ = cr.fill();
        glib::Propagation::Stop
    });
    d
}

const SWITCH_CSS: &str = r#"
.mbl-switch { background-color: rgba(20, 30, 45, 0.92); border: 1px solid rgba(255,255,255,0.35); border-radius: 6px; padding: 14px; }
.mbl-switch button { background: none; border: 1px solid transparent; box-shadow: none; border-radius: 4px; padding: 6px; }
.mbl-switch button:hover, .mbl-switch button:focus { background-color: rgba(120, 170, 230, 0.30); border-color: rgba(180, 215, 255, 0.70); }
.mbl-switch label { color: #ffffff; }
.mbl-switch button:disabled label { color: rgba(255,255,255,0.45); }
"#;

/// Windows 7's display switch (Win+P): Computer only, Duplicate, Extend, Projector only.
fn display_switch(parent: Option<&gtk::Window>, status: gtk::Label) {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if !DONE.with(|d| d.replace(true)) { aerode::theme::load_css(SWITCH_CSS); }
    let outs = connected_outputs();
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_decorated(false);
    w.set_modal(true);
    w.set_keep_above(true);
    w.set_skip_taskbar_hint(true);
    w.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    w.set_transient_for(parent);
    w.set_position(gtk::WindowPosition::CenterAlways);
    let row = hbox(10);
    row.style_context().add_class("mbl-switch");
    for (mode, text) in [(Switch::ComputerOnly, "Computer only"), (Switch::Duplicate, "Duplicate"), (Switch::Extend, "Extend"), (Switch::ProjectorOnly, "Projector only")] {
        let b = gtk::Button::new();
        let v = vbox(6);
        v.pack_start(&switch_picture(mode), false, false, 0);
        v.pack_start(&gtk::Label::new(Some(text)), false, false, 0);
        b.add(&v);
        b.set_sensitive(outs.len() > 1 || mode == Switch::ComputerOnly);
        let (w2, outs, st) = (w.clone(), outs.clone(), status.clone());
        b.connect_clicked(move |_| {
            w2.close();
            let st = st.clone();
            change(switch_argv(mode, &outs), None, move |o| {
                if !o.ok { message(None, gtk::MessageType::Error, "Display Switch", "The display could not be changed.", &o.text()); }
                st.set_text(if connected_outputs().len() > 1 { "Display connected" } else { "No display connected" });
            });
        });
        row.pack_start(&b, false, false, 0);
    }
    w.add(&row);
    // Esc or clicking elsewhere closes it
    w.connect_key_press_event(|w, e| { if e.keyval() == gtk::gdk::keys::constants::Escape { w.close(); } glib::Propagation::Proceed });
    w.connect_focus_out_event(|w, _| { w.close(); glib::Propagation::Proceed });
    w.show_all();
    w.present();
}

// ------------------------------------------------------------------ sync center

/// The file sync programs Sync Center stands for: (name, how to check it runs, how to open its settings).
fn sync_clients() -> Vec<(&'static str, bool, Vec<String>)> {
    let mut v = vec![];
    let running = |p: &str| util::output(&["pgrep", "-u", &uid(), "-x", p]).map(|s| !s.trim().is_empty()).unwrap_or(false);
    if util::which("syncthing") {
        let on = util::output(&["systemctl", "--user", "is-active", "syncthing"]).map(|s| s.trim() == "active").unwrap_or(false) || running("syncthing");
        v.push(("Syncthing", on, sv(&["xdg-open", "http://127.0.0.1:8384/"])));
    }
    for (bin, name) in [("nextcloud", "Nextcloud"), ("owncloud", "ownCloud"), ("megasync", "MEGA"), ("dropbox", "Dropbox"), ("insync", "Insync")] {
        if util::which(bin) { v.push((name, running(bin), sv(&[bin]))); }
    }
    v
}

fn uid() -> String { util::output(&["id", "-u"]).map(|s| s.trim().to_string()).unwrap_or_default() }

fn sync() -> gtk::Widget {
    let t = tile(util::first_icon(&["network-transmit-receive", "folder-remote", "emblem-synchronizing"]), "Sync Center");
    t.status.set_text("Checking…");
    let b = gtk::Button::with_label("Sync settings");
    b.set_sensitive(false);
    t.body.pack_start(&b, false, false, 0);
    let open: Rc<RefCell<Option<Vec<String>>>> = Rc::new(RefCell::new(None));
    {
        let (st, b, open) = (t.status.clone(), b.clone(), open.clone());
        background(sync_clients, move |cl| {
            match cl.iter().find(|c| c.1).or(cl.first()) {
                None => st.set_text("No sync partnerships"),
                Some((name, on, argv)) => {
                    st.set_text(&if *on { format!("{name} is syncing your files") } else { format!("{name} is not running") });
                    *open.borrow_mut() = Some(argv.clone());
                    b.set_sensitive(true);
                }
            }
        });
    }
    let go = { let open = open.clone(); move || if let Some(a) = open.borrow().as_ref() { let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect(); util::spawn(&r); } };
    { let go = go.clone(); b.connect_clicked(move |_| go()); }
    t.icon.connect_clicked(move |_| go());
    t.root.upcast()
}

// ------------------------------------------------------------------ presentation

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
struct Pres {
    active: bool,
    /// what was in effect before presenting: screen saver (timeout, cycle), DPMS on, DPMS times
    saver: (u32, u32),
    dpms: bool,
    dpms_t: (u32, u32, u32),
    set_volume: bool,
    volume: u32,
    /// "Show this background": a picture while presenting ("" = no change), and the one it replaced
    background: String,
    saved_wallpaper: Option<String>,
    /// systemd-inhibit keeping the computer awake while presenting
    inhibit_pid: u32,
}

fn pres_file() -> PathBuf { config::conf_dir().join("presentation.json") }
fn load_pres() -> Pres { std::fs::read_to_string(pres_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Pres { volume: 50, ..Default::default() }) }
fn save_pres(p: &Pres) { let _ = std::fs::create_dir_all(config::conf_dir()); let _ = std::fs::write(pres_file(), serde_json::to_string_pretty(p).unwrap_or_default()); }

fn xset_values() -> ((u32, u32), bool, (u32, u32, u32)) {
    let q = util::output(&["xset", "q"]).unwrap_or_default();
    let num = |key: &str| q.split_whitespace().skip_while(|w| *w != key).nth(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    ((num("timeout:"), num("cycle:")), q.contains("DPMS is Enabled"), (num("Standby:"), num("Suspend:"), num("Off:")))
}

/// Start or stop presenting: no screen saver, no display power-off, optionally a set volume.
fn set_presenting(on: bool) -> Pres {
    let mut p = load_pres();
    if on == p.active { return p; }
    if on {
        let (saver, dpms, t) = xset_values();
        p.saver = saver; p.dpms = dpms; p.dpms_t = t;
        util::spawn(&["xset", "s", "off"]);
        util::spawn(&["xset", "-dpms"]);
        if p.set_volume { sys::set_volume(p.volume); }
        // stay awake: no automatic sleep or idle actions until presenting stops
        if util::which("systemd-inhibit") {
            if let Ok(c) = std::process::Command::new("systemd-inhibit").args(["--what=idle:sleep", "--who=Windows Mobility Center", "--why=Giving a presentation", "--mode=block", "sleep", "infinity"])
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() { p.inhibit_pid = c.id(); }
        }
        if !p.background.is_empty() {
            p.saved_wallpaper = Some(config::get().wallpaper.clone().unwrap_or_default());
            let bg = p.background.clone();
            config::update(|c| c.wallpaper = Some(bg));
            util::spawn(&["aerode", "reload"]);
        }
    } else {
        util::spawn(&["xset", "s", &p.saver.0.to_string(), &p.saver.1.to_string()]);
        if p.dpms { util::spawn(&["xset", "+dpms", "dpms", &p.dpms_t.0.to_string(), &p.dpms_t.1.to_string(), &p.dpms_t.2.to_string()]); }
        if p.inhibit_pid > 0 {
            // only our own systemd-inhibit (the PID may have been reused after a restart)
            let cmd = std::fs::read_to_string(format!("/proc/{}/cmdline", p.inhibit_pid)).unwrap_or_default();
            if cmd.starts_with("systemd-inhibit") { util::spawn(&["kill", &p.inhibit_pid.to_string()]); }
            p.inhibit_pid = 0;
        }
        if let Some(w) = p.saved_wallpaper.take() {
            config::update(|c| c.wallpaper = if w.is_empty() { None } else { Some(w) });
            util::spawn(&["aerode", "reload"]);
        }
    }
    p.active = on;
    save_pres(&p);
    p
}

fn presentation(w: &gtk::Window) -> gtk::Widget {
    let t = tile(util::first_icon(&["video-projector", "preferences-desktop-display", "video-display"]), "Presentation Settings");
    let b = gtk::Button::new();
    t.body.pack_start(&b, false, false, 0);
    let show = { let (st, b) = (t.status.clone(), b.clone()); move |p: &Pres| {
        st.set_text(if p.active { "Presenting" } else { "Not presenting" });
        b.set_label(if p.active { "Turn off" } else { "Turn on" });
    } };
    show(&load_pres());
    { let show = show.clone(); b.connect_clicked(move |_| { let now = !load_pres().active; show(&set_presenting(now)); }); }
    { let (w, show) = (w.clone(), show.clone()); t.icon.connect_clicked(move |_| { settings_dialog(&w); show(&load_pres()); }); }
    t.root.upcast()
}

/// The desktop backgrounds (Personalize's list): the Windows 7 ones and the system's.
fn backgrounds() -> Vec<PathBuf> {
    let mut v = vec![];
    for root in [config::data_dir().join("wallpapers"), PathBuf::from("/usr/share/backgrounds"), PathBuf::from("/usr/share/wallpapers")] {
        let mut stack = vec![(root, 0)];
        while let Some((d, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut items: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            items.sort();
            for p in items {
                if p.is_dir() { if depth < 3 { stack.push((p, depth + 1)); } continue; }
                let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") { v.push(p); }
                if v.len() > 300 { return v; }
            }
        }
    }
    v
}

/// "Presentation Settings".
fn settings_dialog(parent: &gtk::Window) {
    let p = load_pres();
    let d = gtk::Dialog::with_buttons(Some("Presentation Settings"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_resizable(false);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    let on = gtk::CheckButton::with_mnemonic("_I am currently giving a presentation");
    on.set_active(p.active);
    c.pack_start(&on, false, false, 0);
    c.pack_start(&grey("When you are giving a presentation, your computer stays awake and the display doesn't turn off."), false, false, 0);
    let g = vbox(6);
    let saver = gtk::CheckButton::with_mnemonic("Turn off the _screen saver");
    saver.set_active(true);
    saver.set_sensitive(false);
    g.pack_start(&saver, false, false, 0);
    let vr = hbox(6);
    let setv = gtk::CheckButton::with_mnemonic("Set the _volume to:");
    setv.set_active(p.set_volume);
    vr.pack_start(&setv, false, false, 0);
    vr.pack_start(&gtk::Label::new(Some("Low")), false, false, 0);
    let vs = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 5.0);
    vs.set_draw_value(false);
    vs.set_value(p.volume as f64);
    vs.set_size_request(120, -1);
    vs.set_sensitive(p.set_volume);
    vr.pack_start(&vs, true, true, 0);
    vr.pack_start(&gtk::Label::new(Some("High")), false, false, 0);
    { let vs = vs.clone(); setv.connect_toggled(move |c| vs.set_sensitive(c.is_active())); }
    g.pack_start(&vr, false, false, 0);
    // "Show this background:" (None) and the desktop backgrounds, Browse... for another picture
    let bgr = hbox(6);
    let showbg = gtk::CheckButton::with_mnemonic("Show this _background:");
    showbg.set_valign(gtk::Align::Start);
    bgr.pack_start(&showbg, false, false, 0);
    let bgcol = vbox(4);
    let (tv, store) = table(2, &["Background"]);
    tv.set_headers_visible(false);
    store.insert_with_values(None, &[(0, &"(None)"), (1, &"")]);
    for f in backgrounds() {
        let name = f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = f.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        store.insert_with_values(None, &[(0, &format!("{dir} - {name}")), (1, &f.to_string_lossy().into_owned())]);
    }
    let pick = |tv: &gtk::TreeView, store: &gtk::ListStore, path: &str| {
        let mut it = store.iter_first();
        while let Some(i) = it {
            if store.value(&i, 1).get::<String>().ok().as_deref() == Some(path) { tv.selection().select_iter(&i); return true; }
            it = if store.iter_next(&i) { Some(i) } else { None };
        }
        false
    };
    if !pick(&tv, &store, &p.background) && !p.background.is_empty() {
        store.insert_with_values(None, &[(0, &p.background.clone()), (1, &p.background.clone())]);
        pick(&tv, &store, &p.background);
    }
    showbg.set_active(!p.background.is_empty());
    let sw = scrolled(&tv, 90);
    sw.set_size_request(230, -1);
    bgcol.pack_start(&sw, false, false, 0);
    let browse = gtk::Button::with_mnemonic("B_rowse...");
    browse.set_halign(gtk::Align::End);
    {
        let (d2, tv, store) = (d.clone(), tv.clone(), store.clone());
        browse.connect_clicked(move |_| {
            let fc = gtk::FileChooserNative::new(Some("Browse"), Some(&d2), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
            let f = gtk::FileFilter::new();
            f.set_name(Some("Pictures"));
            f.add_pixbuf_formats();
            fc.add_filter(f);
            if fc.run() != gtk::ResponseType::Accept { return; }
            let Some(file) = fc.filename() else { return };
            let s = file.to_string_lossy().into_owned();
            if !pick(&tv, &store, &s) { store.insert_with_values(None, &[(0, &s), (1, &s)]); pick(&tv, &store, &s); }
        });
    }
    bgcol.pack_start(&browse, false, false, 0);
    bgr.pack_start(&bgcol, true, true, 0);
    { let bgcol = bgcol.clone(); showbg.connect_toggled(move |c| bgcol.set_sensitive(c.is_active())); }
    bgcol.set_sensitive(showbg.is_active());
    g.pack_start(&bgr, false, false, 0);
    c.pack_start(&group("When I am giving a presentation:", &g), false, false, 0);
    let conn = gtk::Button::with_mnemonic("_Connected displays");
    conn.set_halign(gtk::Align::Start);
    conn.connect_clicked(|_| { util::spawn(&["aerode-control", "display"]); });
    c.pack_start(&conn, false, false, 0);
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        let was_on = load_pres().active;
        // a different background while presenting: stop and start again so it is shown
        if was_on { set_presenting(false); }
        let mut np = load_pres();
        np.set_volume = setv.is_active();
        np.volume = vs.value().round() as u32;
        np.background = if showbg.is_active() { selected(&tv, 1).unwrap_or_default() } else { String::new() };
        save_pres(&np);
        set_presenting(on.is_active());
        if on.is_active() && np.set_volume { sys::set_volume(np.volume); }
    }
    d.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_switch_commands() {
        let o = vec!["eDP-1".to_string(), "HDMI-1".to_string()];
        assert_eq!(switch_argv(Switch::ComputerOnly, &o).join(" "), "xrandr --output eDP-1 --auto --primary --output HDMI-1 --off");
        assert_eq!(switch_argv(Switch::Duplicate, &o).join(" "), "xrandr --output eDP-1 --auto --primary --output HDMI-1 --auto --same-as eDP-1");
        assert_eq!(switch_argv(Switch::Extend, &o).join(" "), "xrandr --output eDP-1 --auto --primary --output HDMI-1 --auto --right-of eDP-1");
        assert_eq!(switch_argv(Switch::ProjectorOnly, &o).join(" "), "xrandr --output HDMI-1 --auto --primary --output eDP-1 --off");
        assert_eq!(switch_argv(Switch::ProjectorOnly, &o[..1]).join(" "), "xrandr --output eDP-1 --auto");
    }
}
