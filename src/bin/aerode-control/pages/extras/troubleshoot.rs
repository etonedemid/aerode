//! Troubleshooting: the Windows 7 "Troubleshoot computer problems" page and the troubleshooter
//! wizard ("Detecting problems…", "Troubleshooting has completed"). Each troubleshooter checks a
//! part of this desktop (PipeWire, NetworkManager, xrandr, picom, the icon theme, CUPS, pacman…)
//! in a worker thread and repairs what it can, as the user or through pkexec.
use gtk::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use aerode::{config, util};
use crate::{link, section, text, title, Cp};
use super::common::*;

/// How a problem gets repaired.
#[derive(Clone)]
pub(super) enum Fix {
    /// commands run one after the other as the user (in a worker thread)
    Run(Vec<Vec<String>>),
    /// one command through pkexec (UAC prompt)
    Admin(Vec<String>),
    /// done in the Control Panel itself
    Main(fn() -> bool),
}

#[derive(Clone)]
struct Finding { title: String, detail: String, fix: Option<(String, Fix)> }

enum Msg { Step(String), Done(Report) }

/// What a troubleshooter found (built in its worker thread).
pub(super) struct Report {
    tx: Option<async_channel::Sender<Msg>>,
    found: Vec<Finding>,
    /// every potential issue looked at: (name, found)
    checked: Vec<(String, bool)>,
    info: Vec<String>,
    /// the device chosen on "Which device do you want to troubleshoot?" (audio troubleshooters)
    device: Option<String>,
}

impl Report {
    fn step(&self, s: &str) { if let Some(tx) = &self.tx { let _ = tx.send_blocking(Msg::Step(s.to_string())); } }
    fn ok(&mut self, name: &str) { self.checked.push((name.to_string(), false)); }
    fn problem(&mut self, name: &str, detail: impl Into<String>, fix: Option<(&str, Fix)>) {
        self.checked.push((name.to_string(), true));
        self.found.push(Finding { title: name.to_string(), detail: detail.into(), fix: fix.map(|(l, f)| (l.to_string(), f)) });
    }
    fn info(&mut self, s: impl Into<String>) { self.info.push(s.into()); }
}

pub(super) struct Troubleshooter {
    id: &'static str,
    name: &'static str,
    desc: &'static str,
    /// the Windows 7 category
    cat: &'static str,
    icon: &'static str,
    detect: fn(&mut Report),
    /// "Explore additional options" opens this Control Panel page
    more: &'static str,
    /// devices to choose from first ("Which device do you want to troubleshoot?"): (id, name)
    devices: Option<fn() -> Vec<(String, String)>>,
}

const ALL: &[Troubleshooter] = &[
    Troubleshooter { id: "programs", name: "Program Compatibility", desc: "Troubleshoot programs that won't start or that were made for Windows", cat: "Programs", icon: "system-software-install", detect: d_programs, more: "programs", devices: None },
    Troubleshooter { id: "hardware", name: "Hardware and Devices", desc: "Use hardware and access devices connected to the computer.", cat: "Hardware and Sound", icon: "computer", detect: d_hardware, more: "devices", devices: None },
    Troubleshooter { id: "printer", name: "Printer", desc: "Troubleshoot problems preventing printing from completing.", cat: "Hardware and Sound", icon: "printer", detect: d_printer, more: "devices", devices: None },
    Troubleshooter { id: "audio", name: "Playing Audio", desc: "Play sounds and other audio such as music files.", cat: "Hardware and Sound", icon: "audio-volume-high", detect: d_audio, more: "sound", devices: Some(audio_sinks) },
    Troubleshooter { id: "recording", name: "Recording Audio", desc: "Record audio input from a microphone or other source.", cat: "Hardware and Sound", icon: "audio-input-microphone", detect: d_recording, more: "sound", devices: Some(audio_sources) },
    Troubleshooter { id: "keyboard", name: "Keyboard", desc: "Find and fix problems with keyboard layouts and key repeat.", cat: "Hardware and Sound", icon: "input-keyboard", detect: d_keyboard, more: "region", devices: None },
    Troubleshooter { id: "internet", name: "Internet Connections", desc: "Connect to the Internet or to a particular website.", cat: "Network and Internet", icon: "network-wired", detect: d_internet, more: "network", devices: None },
    Troubleshooter { id: "adapter", name: "Network Adapter", desc: "Troubleshoot Ethernet, wireless, or other network adapters.", cat: "Network and Internet", icon: "network-wired", detect: d_adapter, more: "network", devices: None },
    Troubleshooter { id: "homegroup", name: "HomeGroup", desc: "Detect problems that prevent you from viewing computers or shared files in a homegroup.", cat: "Network and Internet", icon: "folder-publicshare", detect: d_homegroup, more: "homegroup", devices: None },
    Troubleshooter { id: "aero", name: "Aero", desc: "Display Aero effects such as transparency.", cat: "Appearance and Personalization", icon: "preferences-desktop-theme", detect: d_aero, more: "display", devices: None },
    Troubleshooter { id: "display", name: "Display Quality", desc: "Find and fix problems with screen resolution and refresh rate.", cat: "Appearance and Personalization", icon: "preferences-desktop-display", detect: d_display, more: "display", devices: None },
    Troubleshooter { id: "icons", name: "Icons and Pointers", desc: "Find and fix missing Windows 7 icons and mouse pointers.", cat: "Appearance and Personalization", icon: "preferences-desktop-wallpaper", detect: d_icons, more: "personalization", devices: None },
    Troubleshooter { id: "update", name: "Windows Update", desc: "Resolve problems that prevent you from updating Windows.", cat: "System and Security", icon: "system-software-update", detect: d_update, more: "update", devices: None },
    Troubleshooter { id: "maintenance", name: "System Maintenance", desc: "Clean up unused files and shortcuts, and perform other maintenance tasks.", cat: "System and Security", icon: "applications-system", detect: d_maintenance, more: "admintools", devices: None },
    Troubleshooter { id: "performance", name: "Performance", desc: "Adjust settings in Windows to help improve overall speed and performance.", cat: "System and Security", icon: "utilities-system-monitor", detect: d_performance, more: "perfinfo", devices: None },
    Troubleshooter { id: "power", name: "Power", desc: "Adjust power settings to help improve battery life and reduce power consumption.", cat: "System and Security", icon: "preferences-system-power", detect: d_power, more: "power", devices: None },
];

fn find(id: &str) -> Option<&'static Troubleshooter> { ALL.iter().find(|t| t.id == id) }

fn cat_icon(cat: &str) -> &'static str {
    match cat {
        "Programs" => "system-software-install",
        "Hardware and Sound" => "printer",
        "Network and Internet" => "network-workgroup",
        "Appearance and Personalization" => "preferences-desktop-wallpaper",
        _ => "security-high",
    }
}

// ------------------------------------------------------------------ the Control Panel pages

pub fn page_troubleshooting(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    let holder = vbox(0);
    {
        let holder = holder.clone();
        side_later(cp, move |cp| {
            let (me, h) = (cp.clone(), holder.clone());
            cp.side.pack_start(&link("View all", move || { let p = list_page(&me, None); subpage(&me, &h, "All Categories", &p); }), false, false, 2);
            let (me, h) = (cp.clone(), holder.clone());
            cp.side.pack_start(&link("View history", move || { let p = history_page(&me); subpage(&me, &h, "Troubleshooting History", &p); }), false, false, 2);
            let (me, h) = (cp.clone(), holder.clone());
            cp.side.pack_start(&link("Change settings", move || { let p = settings_page(&me); subpage(&me, &h, "Change Settings", &p); }), false, false, 2);
            let see = gtk::Label::new(Some("See also"));
            see.set_xalign(0.0);
            see.set_margin_top(24);
            see.style_context().add_class("side-h");
            cp.side.pack_start(&see, false, false, 2);
            cp.side_link("Action Center", "actioncenter");
            cp.side_link("Recovery", "recovery");
        });
    }
    let v = vbox(6);
    v.pack_start(&title("Troubleshoot computer problems"), false, false, 0);
    v.pack_start(&text("Click on a task to automatically troubleshoot and fix common computer problems. To view more troubleshooters, click on a category or use the Search box."), false, false, 0);
    type L = (&'static str, &'static str);
    let tasks: [(&str, &[L]); 5] = [
        ("Programs", &[("Run programs made for previous versions of Windows", "programs")]),
        ("Hardware and Sound", &[("Configure a device", "hardware"), ("Use a printer", "printer"), ("Troubleshoot audio playback", "audio")]),
        ("Network and Internet", &[("Connect to the Internet", "internet"), ("Access shared files and folders on other computers", "homegroup")]),
        ("Appearance and Personalization", &[("Display Aero desktop effects", "aero")]),
        ("System and Security", &[("Fix problems with Windows Update", "update"), ("Run maintenance tasks", "maintenance"), ("Improve power usage", "power")]),
    ];
    for (cat, links) in tasks {
        let h = hbox(10);
        h.set_margin_top(12);
        h.pack_start(&util::image(cat_icon(cat), 48), false, false, 0);
        let col = vbox(1);
        let t = gtk::Button::with_label(cat);
        t.style_context().add_class("cp-cat");
        t.set_halign(gtk::Align::Start);
        t.set_relief(gtk::ReliefStyle::None);
        { let (me, hd) = (cp.clone(), holder.clone()); t.connect_clicked(move |_| { let p = list_page(&me, Some(cat)); subpage(&me, &hd, cat, &p); }); }
        col.pack_start(&t, false, false, 0);
        // "Configure a device | Use a printer | Troubleshoot audio playback"
        let row = hbox(6);
        for (i, (l, id)) in links.iter().enumerate() {
            let me = cp.clone();
            if i > 0 { row.pack_start(&grey("|"), false, false, 0); }
            row.pack_start(&link(l, move || run(Some(&me), id)), false, false, 0);
        }
        col.pack_start(&row, false, false, 0);
        h.pack_start(&col, true, true, 0);
        v.pack_start(&h, false, false, 0);
    }
    holder.pack_start(&v, true, true, 0);
    holder.upcast()
}

/// "Troubleshoot problems - <category>" / "View all": every troubleshooter in a list.
fn list_page(cp: &Rc<Cp>, cat: Option<&str>) -> gtk::Widget {
    let v = vbox(8);
    v.pack_start(&title(&format!("Troubleshoot problems - {}", cat.unwrap_or("All Categories"))), false, false, 0);
    let store = gtk::ListStore::new(&[gtk::gdk_pixbuf::Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    for t in ALL.iter().filter(|t| cat.map(|c| c == t.cat).unwrap_or(true)) {
        let pb = util::pixbuf_or(t.icon, 24, "applications-system");
        store.insert_with_values(None, &[(0, &pb), (1, &t.name), (2, &t.desc), (3, &"Local"), (4, &t.cat), (5, &"Aerode"), (6, &t.id)]);
    }
    let tv = gtk::TreeView::with_model(&store);
    let c = gtk::TreeViewColumn::new();
    c.set_title("Name");
    let pr = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&c, &pr, false);
    TreeViewColumnExt::add_attribute(&c, &pr, "pixbuf", 0);
    let tr = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&c, &tr, true);
    TreeViewColumnExt::add_attribute(&c, &tr, "text", 1);
    c.set_resizable(true);
    tv.append_column(&c);
    for (i, t) in [(2, "Description"), (3, "Location"), (4, "Category"), (5, "Publisher")] {
        let c = gtk::TreeViewColumn::new();
        c.set_title(t);
        let r = gtk::CellRendererText::new();
        if i == 2 { r.set_property("ellipsize", gtk::pango::EllipsizeMode::End); c.set_expand(true); }
        TreeViewColumnExt::pack_start(&c, &r, true);
        TreeViewColumnExt::add_attribute(&c, &r, "text", i);
        c.set_resizable(true);
        tv.append_column(&c);
    }
    tv.set_activate_on_single_click(true);
    let me = cp.clone();
    tv.connect_row_activated(move |tv, path, _| {
        let Some(m) = tv.model() else { return };
        let Some(it) = m.iter(path) else { return };
        let id: String = m.value(&it, 6).get().unwrap_or_default();
        run(Some(&me), &id);
    });
    v.pack_start(&scrolled(&tv, 380), true, true, 0);
    v.upcast()
}

fn history_file() -> PathBuf { config::data_dir().join("troubleshooting-history.tsv") }

fn history_page(_cp: &Rc<Cp>) -> gtk::Widget {
    let v = vbox(8);
    v.pack_start(&title("View Troubleshooting history"), false, false, 0);
    v.pack_start(&text("The troubleshooters you have run on this computer and what they found."), false, false, 0);
    let (tv, store) = table(3, &["Name", "Date", "Result"]);
    let fill_store = { let store = store.clone(); move || {
        store.clear();
        let s = std::fs::read_to_string(history_file()).unwrap_or_default();
        for l in s.lines().rev() {
            let f: Vec<&str> = l.splitn(4, '\t').collect();
            if f.len() < 4 { continue; }
            let when = f[0].parse::<i64>().ok().and_then(unix_time).map(win_time).unwrap_or_default();
            store.insert_with_values(None, &[(0, &f[2]), (1, &when), (2, &f[3])]);
        }
    } };
    fill_store();
    v.pack_start(&scrolled(&tv, 320), true, true, 0);
    let clear = link("Clear history", move || { let _ = std::fs::remove_file(history_file()); fill_store(); });
    v.pack_start(&clear, false, false, 0);
    v.upcast()
}

fn immediate_flag() -> PathBuf { config::conf_dir().join("troubleshoot-immediate") }

fn settings_page(cp: &Rc<Cp>) -> gtk::Widget {
    let v = vbox(8);
    v.pack_start(&title("Change Troubleshooting settings"), false, false, 0);
    v.pack_start(&section("Other settings"), false, false, 0);
    let imm = gtk::CheckButton::with_label("Allow troubleshooting to begin immediately when started");
    imm.set_active(immediate_flag().exists());
    imm.set_margin_start(16);
    v.pack_start(&imm, false, false, 0);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    { let (me, imm) = (cp.clone(), imm.clone()); ok.connect_clicked(move |_| {
        let _ = std::fs::create_dir_all(config::conf_dir());
        if imm.is_active() { let _ = std::fs::write(immediate_flag(), ""); } else { let _ = std::fs::remove_file(immediate_flag()); }
        me.open("troubleshooting");
    }); }
    { let me = cp.clone(); cancel.connect_clicked(move |_| me.open("troubleshooting")); }
    let row = button_row(&[&ok, &cancel]);
    row.set_margin_top(20);
    v.pack_start(&row, false, false, 0);
    v.upcast()
}

// ------------------------------------------------------------------ the wizard

#[derive(Clone, Copy, PartialEq)]
enum State { Detected, Fixed, NotFixed, Skipped }

struct Session {
    wiz: Rc<Wizard>,
    t: &'static Troubleshooter,
    cp: Option<Rc<Cp>>,
    auto: Cell<bool>,
    alive: Cell<bool>,
    results: RefCell<Vec<(Finding, State, String)>>,
    report: RefCell<Option<Report>>,
    device: RefCell<Option<String>>,
}

/// Start a troubleshooter by id ("audio", "homegroup"…).
pub(super) fn run(cp: Option<&Rc<Cp>>, id: &str) {
    let Some(t) = find(id) else { return };
    let parent: Option<gtk::Window> = cp.map(|c| c.win.clone().upcast()).filter(|w: &gtk::Window| w.is_visible());
    let wiz = wizard(t.name, t.icon, parent.as_ref(), (600, 450), true);
    if let Some(app) = cp.and_then(|c| c.win.application()) { wiz.win.set_application(Some(&app)); }
    let s = Rc::new(Session { wiz, t, cp: cp.cloned(), auto: Cell::new(true), alive: Cell::new(true), results: RefCell::new(vec![]), report: RefCell::new(None), device: RefCell::new(None) });
    { let s2 = s.clone(); s.wiz.win.connect_destroy(move |_| s2.alive.set(false)); }
    s.wiz.back.set_sensitive(false);
    if immediate_flag().exists() { start(&s); } else { intro(&s); }
    s.wiz.win.show_all();
    s.wiz.win.present();
}

fn close_button(s: &Rc<Session>, label: &str) -> gtk::Button {
    let b = gtk::Button::with_mnemonic(label);
    let w = s.wiz.win.clone();
    b.connect_clicked(move |_| w.close());
    b
}

fn intro(s: &Rc<Session>) {
    let v = vbox(14);
    v.pack_start(&instr("Troubleshoot and help prevent computer problems"), false, false, 0);
    let h = hbox(14);
    h.set_margin_start(8);
    h.pack_start(&util::image(s.t.icon, 48), false, false, 0);
    let col = vbox(3);
    let n = gtk::Label::new(Some(s.t.name));
    n.set_xalign(0.0);
    n.style_context().add_class("xt-name");
    col.pack_start(&n, false, false, 0);
    col.pack_start(&label(s.t.desc), false, false, 0);
    h.pack_start(&col, true, true, 0);
    v.pack_start(&h, false, false, 0);
    let adv_box = vbox(4);
    adv_box.set_no_show_all(true);
    let auto = gtk::CheckButton::with_mnemonic("Apply _repairs automatically");
    auto.set_active(s.auto.get());
    { let s2 = s.clone(); auto.connect_toggled(move |c| s2.auto.set(c.is_active())); }
    adv_box.pack_start(&auto, false, false, 0);
    auto.show();
    let ab = adv_box.clone();
    let adv = link("Advanced", move || ab.set_visible(!ab.is_visible()));
    adv.set_margin_top(20);
    v.pack_start(&adv, false, false, 0);
    v.pack_start(&adv_box, false, false, 0);
    s.wiz.page(&v);
    s.wiz.set_foot(Some(&grey("Publisher: Aerode").upcast()));
    let next = gtk::Button::with_mnemonic("_Next");
    { let s2 = s.clone(); next.connect_clicked(move |_| start(&s2)); }
    s.wiz.set_buttons(&[&next, &close_button(s, "Cancel")]);
    next.grab_default();
}

fn progress_page(s: &Rc<Session>, head: &str, step: &str) -> (gtk::Label, gtk::ProgressBar) {
    let v = vbox(12);
    v.pack_start(&instr(head), false, false, 0);
    let st = label(step);
    v.pack_start(&st, false, false, 0);
    let pb = gtk::ProgressBar::new();
    pb.set_margin_end(40);
    v.pack_start(&pb, false, false, 0);
    s.wiz.page(&v);
    s.wiz.set_foot(None);
    s.wiz.set_buttons(&[&close_button(s, "Cancel")]);
    (st, pb)
}

/// After "Next": "Which device do you want to troubleshoot?" when there is more than one, then
/// detection.
fn start(s: &Rc<Session>) {
    let Some(list) = s.t.devices else { detect(s); return };
    let (_st, pb) = progress_page(s, "Detecting problems", "Looking for devices…");
    pb.pulse();
    let s = s.clone();
    background(list, move |devs| {
        if !s.alive.get() { return; }
        if devs.len() < 2 { *s.device.borrow_mut() = devs.first().map(|d| d.0.clone()); detect(&s); return; }
        let v = vbox(10);
        v.pack_start(&instr("Which device do you want to troubleshoot?"), false, false, 0);
        let (tv, store) = table(2, &["Name"]);
        tv.set_headers_visible(false);
        for (id, name) in &devs { store.insert_with_values(None, &[(0, name), (1, id)]); }
        if let Some(it) = store.iter_first() { tv.selection().select_iter(&it); }
        v.pack_start(&scrolled(&tv, 160), false, false, 0);
        s.wiz.page(&v);
        let next = gtk::Button::with_mnemonic("_Next");
        let go = { let (s, tv) = (s.clone(), tv.clone()); move || { *s.device.borrow_mut() = selected(&tv, 1); detect(&s); } };
        { let go = go.clone(); next.connect_clicked(move |_| go()); }
        tv.connect_row_activated(move |_, _, _| go());
        s.wiz.set_buttons(&[&next, &close_button(&s, "Cancel")]);
    });
}

/// Run the troubleshooter's checks in a worker thread on a progress page.
fn run_detect(s: &Rc<Session>, head: &str, done: impl FnOnce(&Rc<Session>, Report) + 'static) {
    let (st, pb) = progress_page(s, head, "Starting…");
    let pulse = { let pb = pb.clone(); glib::timeout_add_local(std::time::Duration::from_millis(100), move || { pb.pulse(); glib::ControlFlow::Continue }) };
    let (tx, rx) = async_channel::unbounded();
    let f = s.t.detect;
    let device = s.device.borrow().clone();
    std::thread::spawn(move || {
        let mut r = Report { tx: Some(tx.clone()), found: vec![], checked: vec![], info: vec![], device };
        f(&mut r);
        r.tx = None;
        let _ = tx.send_blocking(Msg::Done(r));
    });
    let s = s.clone();
    glib::MainContext::default().spawn_local(async move {
        let mut pulse = Some(pulse);
        let mut done = Some(done);
        while let Ok(m) = rx.recv().await {
            if !s.alive.get() { break; }
            match m {
                Msg::Step(t) => st.set_text(&format!("{t}…")),
                Msg::Done(r) => {
                    if let Some(p) = pulse.take() { p.remove(); }
                    if let Some(d) = done.take() { d(&s, r); }
                    break;
                }
            }
        }
        if let Some(p) = pulse.take() { p.remove(); }
    });
}

fn detect(s: &Rc<Session>) {
    run_detect(s, "Detecting problems", |s, r| {
        *s.results.borrow_mut() = r.found.iter().map(|f| (f.clone(), State::Detected, String::new())).collect();
        *s.report.borrow_mut() = Some(r);
        resolve(s, 0);
    });
}

/// Like Windows, check again after repairing: a repair only counts as "Fixed" when the problem
/// is gone.
fn verify(s: &Rc<Session>) {
    let repaired = s.results.borrow().iter().any(|r| r.1 == State::Fixed);
    if !repaired || dry_run() { done(s); return; }
    run_detect(s, "Resolving problems", |s, r| {
        for res in s.results.borrow_mut().iter_mut() {
            if res.1 == State::Fixed && r.found.iter().any(|f| f.title == res.0.title) {
                res.1 = State::NotFixed;
                if res.2.is_empty() { res.2 = "The problem is still there after the repair.".into(); }
            }
        }
        done(s);
    });
}

/// Repair the findings one by one from index `i`: automatically, or asking "Apply this fix".
fn resolve(s: &Rc<Session>, i: usize) {
    if !s.alive.get() { return; }
    let next = s.results.borrow().iter().enumerate().skip(i).find(|(_, (f, st, _))| f.fix.is_some() && *st == State::Detected).map(|(i, (f, _, _))| (i, f.clone()));
    let Some((i, f)) = next else { verify(s); return };
    let (fix_label, fix) = f.fix.clone().unwrap();
    let apply_it = {
        let s = s.clone();
        move || {
            let (_st, pb) = progress_page(&s, "Resolving problems", &format!("{fix_label}…"));
            let pulse = { let pb = pb.clone(); glib::timeout_add_local(std::time::Duration::from_millis(100), move || { pb.pulse(); glib::ControlFlow::Continue }) };
            let s2 = s.clone();
            apply(&fix, move |ok, why| {
                pulse.remove();
                if let Some(r) = s2.results.borrow_mut().get_mut(i) { r.1 = if ok { State::Fixed } else { State::NotFixed }; r.2 = why; }
                resolve(&s2, i + 1);
            });
        }
    };
    if s.auto.get() { apply_it(); return; }
    let v = vbox(12);
    v.pack_start(&instr(&f.title), false, false, 0);
    v.pack_start(&label(&f.detail), false, false, 0);
    let a = Rc::new(apply_it);
    { let a = a.clone(); v.pack_start(&cmd_link(None, "Apply this fix", &f.fix.as_ref().map(|x| x.0.clone()).unwrap_or_default(), move || a()), false, false, 0); }
    let s2 = s.clone();
    v.pack_start(&cmd_link(None, "Skip this fix", "Continue troubleshooting without applying this fix.", move || {
        if let Some(r) = s2.results.borrow_mut().get_mut(i) { r.1 = State::Skipped; }
        resolve(&s2, i + 1);
    }), false, false, 0);
    s.wiz.page(&v);
    s.wiz.set_buttons(&[&close_button(s, "Cancel")]);
}

fn apply(fix: &Fix, cb: impl FnOnce(bool, String) + 'static) {
    match fix.clone() {
        Fix::Run(cmds) => background(move || {
            for c in &cmds { let o = change_blocking(c, None); if !o.ok { return (false, o.text()); } }
            (true, String::new())
        }, move |(ok, why)| cb(ok, why)),
        Fix::Admin(cmd) => {
            let mut a = vec!["pkexec".to_string()];
            a.extend(cmd);
            change(a, None, move |o| { let why = if o.cancelled() { "You need to be an administrator to apply this fix.".into() } else { o.text() }; cb(o.ok, why) });
        }
        Fix::Main(f) => {
            let ok = if dry_run() { eprintln!("[aerode dry run] in-process repair"); true } else { f() };
            glib::idle_add_local_once(move || cb(ok, String::new()));
        }
    }
}

fn status_row(f: &Finding, st: State) -> gtk::Box {
    let h = hbox(8);
    let t = label(&f.title);
    h.pack_start(&t, true, true, 0);
    let (m, txt) = match st { State::Fixed => (Mark::Ok, "Fixed"), State::NotFixed => (Mark::Bad, "Not fixed"), State::Skipped => (Mark::Warn, "Not fixed"), State::Detected => (Mark::Warn, "Detected") };
    let sb = hbox(4);
    sb.pack_start(&mark(m), false, false, 0);
    let l = gtk::Label::new(Some(txt));
    l.set_width_chars(9);
    l.set_xalign(0.0);
    sb.pack_start(&l, false, false, 0);
    h.pack_end(&sb, false, false, 0);
    h
}

fn explore(s: &Rc<Session>) {
    let page = s.t.more;
    match &s.cp {
        Some(cp) if cp.win.is_visible() => { cp.open(page); cp.win.present(); }
        _ => { util::spawn(&["aerode-control", page]); }
    }
}

fn done(s: &Rc<Session>) {
    let res = s.results.borrow().clone();
    let fixed = res.iter().filter(|r| r.1 == State::Fixed).count();
    let v = vbox(10);
    if res.is_empty() {
        v.pack_start(&instr("Troubleshooting couldn't identify the problem"), false, false, 0);
        v.pack_start(&label("No problems were found. You can explore other options or close the troubleshooter."), false, false, 0);
    } else {
        v.pack_start(&instr("Troubleshooting has completed"), false, false, 0);
        v.pack_start(&label(if fixed > 0 { "The troubleshooter made some changes to your system. Try attempting the task you were trying to do before." }
            else { "The troubleshooter found problems it couldn't fix. See below for what was found." }), false, false, 0);
        let hdr = hbox(8);
        let pf = gtk::Label::new(None);
        pf.set_markup("<b>Problems found</b>");
        hdr.pack_start(&pf, false, false, 0);
        hdr.set_margin_top(6);
        v.pack_start(&hdr, false, false, 0);
        let list = vbox(4);
        list.set_margin_start(10);
        for (f, st, _) in &res { list.pack_start(&status_row(f, *st), false, false, 0); }
        v.pack_start(&list, false, false, 0);
    }
    let cmds = vbox(0);
    cmds.set_margin_top(8);
    { let w = s.wiz.win.clone(); cmds.pack_start(&cmd_link(None, "Close the troubleshooter", "", move || w.close()), false, false, 0); }
    { let s2 = s.clone(); cmds.pack_start(&cmd_link(None, "Explore additional options", "", move || explore(&s2)), false, false, 0); }
    v.pack_start(&cmds, false, false, 0);
    s.wiz.page(&v);
    let s2 = s.clone();
    s.wiz.set_foot(Some(&link("View detailed information", move || details(&s2)).upcast()));
    s.wiz.set_buttons(&[&close_button(s, "_Close")]);
    // history (Troubleshooting ▸ View history)
    let summary = if res.is_empty() { "No problems found".to_string() } else {
        let nf = res.len() - fixed;
        match (fixed, nf) { (f, 0) => format!("{f} fixed"), (0, n) => format!("{n} found, not fixed"), (f, n) => format!("{f} fixed, {n} not fixed") }
    };
    let _ = std::fs::create_dir_all(config::data_dir());
    use std::io::Write;
    if let Ok(mut fh) = std::fs::OpenOptions::new().create(true).append(true).open(history_file()) {
        let _ = writeln!(fh, "{}\t{}\t{}\t{}", chrono::Local::now().timestamp(), s.t.id, s.t.name, summary);
    }
}

/// "Troubleshooting report": issues found, potential issues checked, detection details.
fn details(s: &Rc<Session>) {
    let v = vbox(8);
    v.pack_start(&instr("Troubleshooting report"), false, false, 0);
    v.pack_start(&crate::kv_grid(&[("Troubleshooter:", s.t.name.to_string()), ("Publisher:", "Aerode".into()), ("Date:", win_time(chrono::Local::now()))]), false, false, 0);
    let inner = vbox(6);
    let h = |t: &str| { let l = gtk::Label::new(None); l.set_markup(&format!("<b>{}</b>", esc(t))); l.set_xalign(0.0); l.set_margin_top(8); l };
    inner.pack_start(&h("Issues found"), false, false, 0);
    let res = s.results.borrow().clone();
    if res.is_empty() { inner.pack_start(&grey("None"), false, false, 0); }
    for (f, st, why) in &res {
        inner.pack_start(&status_row(f, *st), false, false, 0);
        let d = grey(&f.detail);
        d.set_margin_start(16);
        inner.pack_start(&d, false, false, 0);
        if let Some((l, _)) = &f.fix { let x = grey(&format!("Repair: {l}")); x.set_margin_start(16); inner.pack_start(&x, false, false, 0); }
        if !why.is_empty() { let x = grey(&format!("Error: {why}")); x.set_margin_start(16); inner.pack_start(&x, false, false, 0); }
    }
    inner.pack_start(&h("Potential issues that were checked"), false, false, 0);
    if let Some(r) = s.report.borrow().as_ref() {
        for (name, found) in &r.checked {
            let row = hbox(8);
            row.pack_start(&label(name), true, true, 0);
            let sb = hbox(4);
            sb.pack_start(&mark(if *found { Mark::Warn } else { Mark::Ok }), false, false, 0);
            sb.pack_start(&gtk::Label::new(Some(if *found { "Issue found" } else { "Issue not present" })), false, false, 0);
            row.pack_end(&sb, false, false, 0);
            inner.pack_start(&row, false, false, 0);
        }
        if !r.info.is_empty() {
            inner.pack_start(&h("Detection details"), false, false, 0);
            for i in &r.info { inner.pack_start(&grey(i), false, false, 0); }
        }
    }
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.add(&inner);
    v.pack_start(&sw, true, true, 0);
    s.wiz.page(&v);
    s.wiz.set_foot(None);
    s.wiz.back.set_sensitive(true);
    let s2 = s.clone();
    // the round back button returns to the summary
    let id = Rc::new(RefCell::new(None::<glib::SignalHandlerId>));
    let id2 = id.clone();
    *id.borrow_mut() = Some(s.wiz.back.connect_clicked(move |b| {
        b.set_sensitive(false);
        if let Some(h) = id2.borrow_mut().take() { b.disconnect(h); }
        done(&s2);
    }));
    s.wiz.set_buttons(&[&close_button(s, "_Close")]);
}

// ------------------------------------------------------------------ detection helpers

fn sh(args: &[&str]) -> Out { exec(&sv(args), None) }
fn have(cmd: &str) -> bool { util::which(cmd) }
fn active(unit: &str, user: bool) -> bool {
    let o = if user { sh(&["systemctl", "--user", "is-active", unit]) } else { sh(&["systemctl", "is-active", unit]) };
    o.stdout.trim() == "active"
}
fn run1(a: &[&str]) -> Fix { Fix::Run(vec![sv(a)]) }
fn admin1(a: &[&str]) -> Fix { Fix::Admin(sv(a)) }
fn gb(n: u64) -> String { util::human_size(n) }

fn dir_size(p: &Path, budget: &mut u32) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    let mut n = 0;
    for e in rd.flatten() {
        if *budget == 0 { break; }
        *budget -= 1;
        let Ok(m) = e.metadata() else { continue };
        if m.is_dir() { n += dir_size(&e.path(), budget); } else { n += m.len(); }
    }
    n
}

// ------------------------------------------------------------------ troubleshooters

/// PipeWire/PulseAudio devices as (name, description) from `pactl list sinks|sources`.
fn pactl_devices(kind: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vec![];
    for l in sh(&["pactl", "list", kind]).stdout.lines() {
        let l = l.trim();
        if let Some(n) = l.strip_prefix("Name:") { v.push((n.trim().to_string(), n.trim().to_string())); }
        else if let (Some(d), Some(last)) = (l.strip_prefix("Description:"), v.last_mut()) { last.1 = d.trim().to_string(); }
    }
    v.retain(|(n, _)| n != "auto_null" && !n.ends_with(".monitor"));
    v
}
fn audio_sinks() -> Vec<(String, String)> { pactl_devices("sinks") }
fn audio_sources() -> Vec<(String, String)> { pactl_devices("sources") }

fn d_audio(r: &mut Report) {
    r.step("Checking audio services");
    let info = sh(&["pactl", "info"]);
    let wp = have("wpctl") && sh(&["wpctl", "status"]).ok;
    if !info.ok && !wp {
        r.problem("Audio services not responding", "The sound service (PipeWire with WirePlumber, or PulseAudio) is not running or does not answer, so no program can play sound.",
            Some(("Restart the audio services", Fix::Run(vec![sv(&["systemctl", "--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"])]))));
        return;
    }
    r.ok("Audio services not responding");
    if let Some(l) = info.stdout.lines().find(|l| l.starts_with("Server Name:")) { r.info(l.trim().to_string()); }
    r.step("Checking for audio devices");
    let sinks: Vec<String> = sh(&["pactl", "list", "short", "sinks"]).stdout.lines().filter_map(|l| l.split('\t').nth(1).map(String::from)).filter(|n| n != "auto_null").collect();
    let cards = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default();
    r.info(format!("Playback devices: {}", if sinks.is_empty() { "none".into() } else { sinks.join(", ") }));
    if sinks.is_empty() {
        let n = cards.lines().filter(|l| l.trim_start().chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false)).count();
        r.problem("No audio output device is installed", format!("No speakers or headphones were found ({n} sound card(s) known to the kernel). Make sure they are plugged in and turned on; restarting the session manager makes it look for them again."),
            Some(("Look for audio devices again", run1(&["systemctl", "--user", "restart", "wireplumber"]))));
        return;
    }
    r.ok("No audio output device is installed");
    let def = sh(&["pactl", "get-default-sink"]).stdout.trim().to_string();
    let dev = r.device.clone().filter(|d| sinks.contains(d));
    match &dev {
        // the device chosen on the first page should be the one programs play to
        Some(d) if *d != def => r.problem("The device is not the default playback device", format!("Programs play sound on {}, not on {d}.", if def.is_empty() { "no device" } else { &def }),
            Some(("Set the device as the default playback device", run1(&["pactl", "set-default-sink", d])))),
        Some(_) => { r.ok("The device is not the default playback device"); r.info(format!("Default device: {def}")); }
        None if def.is_empty() || !sinks.contains(&def) =>
            r.problem("No default playback device", "Sound has nowhere to go because no default playback device is set.", Some(("Set the first device as the default", run1(&["pactl", "set-default-sink", &sinks[0]])))),
        None => { r.ok("No default playback device"); r.info(format!("Default device: {def}")); }
    }
    let target = dev.clone().unwrap_or_else(|| "@DEFAULT_SINK@".to_string());
    r.step("Checking the audio device");
    let state = sh(&["pactl", "list", "short", "sinks"]).stdout.lines().find(|l| l.split('\t').nth(1) == Some(target.as_str()) || (target == "@DEFAULT_SINK@" && l.split('\t').nth(1) == Some(def.as_str())))
        .and_then(|l| l.split('\t').nth(4).map(String::from)).unwrap_or_default();
    if !state.is_empty() { r.info(format!("Device state: {state}")); }
    r.step("Checking volume and mute");
    let muted = sh(&["pactl", "get-sink-mute", &target]).stdout.contains("yes");
    if muted { r.problem("Speakers/headphones are muted", "The playback device is muted.", Some(("Turn off mute", run1(&["pactl", "set-sink-mute", &target, "0"])))); }
    else { r.ok("Speakers/headphones are muted"); }
    let vol = sh(&["pactl", "get-sink-volume", &target]).stdout;
    if let Some(v) = vol.split('/').nth(1).and_then(|x| x.trim().trim_end_matches('%').parse::<u32>().ok()) {
        r.info(format!("Volume: {v}%"));
        if v < 10 { r.problem("Volume is low", format!("The volume is set to {v}%."), Some(("Set the volume to 50%", run1(&["pactl", "set-sink-volume", &target, "50%"])))); }
        else { r.ok("Volume is low"); }
    }
}

fn d_recording(r: &mut Report) {
    r.step("Checking audio services");
    if !sh(&["pactl", "info"]).ok {
        r.problem("Audio services not responding", "The sound service (PipeWire / PulseAudio) is not running.", Some(("Restart the audio services", Fix::Run(vec![sv(&["systemctl", "--user", "restart", "pipewire", "pipewire-pulse", "wireplumber"])]))));
        return;
    }
    r.ok("Audio services not responding");
    r.step("Checking for recording devices");
    let srcs: Vec<String> = sh(&["pactl", "list", "short", "sources"]).stdout.lines().filter_map(|l| l.split('\t').nth(1).map(String::from)).filter(|n| !n.ends_with(".monitor")).collect();
    r.info(format!("Recording devices: {}", if srcs.is_empty() { "none".into() } else { srcs.join(", ") }));
    if srcs.is_empty() { r.problem("No recording device is installed", "No microphone or line-in was found. Plug in a microphone and try again.", None); return; }
    r.ok("No recording device is installed");
    let def = sh(&["pactl", "get-default-source"]).stdout.trim().to_string();
    let dev = r.device.clone().filter(|d| srcs.contains(d));
    match &dev {
        Some(d) if *d != def => r.problem("The device is not the default recording device", format!("Programs record from {}, not from {d}.", if def.is_empty() { "no device" } else { &def }),
            Some(("Set the device as the default recording device", run1(&["pactl", "set-default-source", d])))),
        Some(_) => r.ok("The device is not the default recording device"),
        None if def.is_empty() || !srcs.contains(&def) =>
            r.problem("No default recording device", "Programs don't know which microphone to use.", Some(("Set the first microphone as the default", run1(&["pactl", "set-default-source", &srcs[0]])))),
        None => r.ok("No default recording device"),
    }
    let target = dev.unwrap_or_else(|| "@DEFAULT_SOURCE@".to_string());
    r.step("Checking microphone volume");
    if sh(&["pactl", "get-source-mute", &target]).stdout.contains("yes") {
        r.problem("Microphone is muted", "The recording device is muted.", Some(("Turn off mute", run1(&["pactl", "set-source-mute", &target, "0"]))));
    } else { r.ok("Microphone is muted"); }
    let vol = sh(&["pactl", "get-source-volume", &target]).stdout;
    if let Some(p) = vol.split('/').nth(1).and_then(|x| x.trim().trim_end_matches('%').parse::<u32>().ok()) {
        if p < 10 { r.problem("Microphone volume is low", format!("The recording level is {p}%."), Some(("Set the recording level to 70%", run1(&["pactl", "set-source-volume", &target, "70%"])))); }
        else { r.ok("Microphone volume is low"); }
    }
}

fn d_internet(r: &mut Report) {
    r.step("Checking the network service");
    let nm = have("nmcli");
    if nm {
        if !active("NetworkManager", false) {
            r.problem("The network service is not running", "NetworkManager, which connects this computer to networks, is not running.", Some(("Start the network service", admin1(&["systemctl", "restart", "NetworkManager"]))));
            return;
        }
        r.ok("The network service is not running");
        if sh(&["nmcli", "networking"]).stdout.trim() == "disabled" {
            r.problem("Networking is turned off", "All network connections are turned off.", Some(("Turn on networking", run1(&["nmcli", "networking", "on"]))));
            return;
        }
        r.ok("Networking is turned off");
        r.step("Checking network adapters");
        let devs: Vec<Vec<String>> = sh(&["nmcli", "-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).stdout.lines().map(aerode::sys::nm_split).filter(|f| f.len() >= 4).collect();
        for d in &devs { if d[1] == "wifi" || d[1] == "ethernet" { r.info(format!("{} ({}): {}{}", d[0], d[1], d[2], if d[3].is_empty() { String::new() } else { format!(" – {}", d[3]) })); } }
        let connected = devs.iter().any(|d| (d[1] == "wifi" || d[1] == "ethernet") && d[2] == "connected");
        let has_wifi = devs.iter().any(|d| d[1] == "wifi");
        if !connected && has_wifi && sh(&["nmcli", "radio", "wifi"]).stdout.trim() == "disabled" {
            r.problem("Wireless is turned off", "The wireless adapter is turned off.", Some(("Turn on wireless", run1(&["nmcli", "radio", "wifi", "on"]))));
            return;
        }
        r.ok("Wireless is turned off");
        if !connected {
            let cand = devs.iter().find(|d| (d[1] == "ethernet" || d[1] == "wifi") && d[2] == "disconnected").map(|d| d[0].clone());
            r.problem("This computer is not connected to a network", "No network adapter is connected. Plug in a network cable, or connect to a wireless network from the network icon in the notification area.",
                cand.map(|d| ("Connect the network adapter", Fix::Run(vec![sv(&["nmcli", "device", "connect", &d])]))));
            return;
        }
        r.ok("This computer is not connected to a network");
    }
    r.step("Checking the default gateway");
    if have("ip") {
        let route = sh(&["ip", "route", "show", "default"]).stdout;
        if route.trim().is_empty() {
            r.problem("The default gateway is not available", "This computer has no route to the Internet.", nm.then(|| ("Reset the network connection", admin1(&["systemctl", "restart", "NetworkManager"]))));
            return;
        }
        r.ok("The default gateway is not available");
        r.info(format!("Route: {}", route.lines().next().unwrap_or("").trim()));
    }
    r.step("Checking name resolution (DNS)");
    let dns = sh(&["timeout", "6", "getent", "hosts", "archlinux.org"]).ok || sh(&["timeout", "6", "getent", "hosts", "example.com"]).ok;
    r.step("Checking Internet access");
    let ping = if have("ping") { ["1.1.1.1", "9.9.9.9", "8.8.8.8"].iter().any(|h| sh(&["ping", "-c", "1", "-W", "3", h]).ok) } else { dns };
    if !dns {
        let fix = if active("systemd-resolved", false) { Some(("Restart the DNS client", admin1(&["systemctl", "restart", "systemd-resolved"]))) }
            else if nm { Some(("Reset the network connection", admin1(&["systemctl", "restart", "NetworkManager"]))) } else { None };
        r.problem("The DNS server isn't responding", if ping { "The Internet can be reached, but names of websites can't be looked up." } else { "Names of websites can't be looked up." }, fix);
    } else { r.ok("The DNS server isn't responding"); }
    if !ping {
        r.problem("Can't reach the Internet", "This computer is connected to a network, but the Internet can't be reached. The problem might be with your router, modem or Internet service provider – try turning them off and on again.", None);
    } else { r.ok("Can't reach the Internet"); }
}

/// Network Adapter: adapters NetworkManager doesn't use, cables unplugged, links down, missing drivers.
fn d_adapter(r: &mut Report) {
    r.step("Checking network adapters");
    // (name, type) of the hardware adapters the kernel knows
    let mut adapters: Vec<(String, String)> = vec![];
    if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
        for e in rd.flatten() {
            let p = e.path();
            if !p.join("device").exists() { continue; } // lo, bridges, VPN tunnels…
            let name = e.file_name().to_string_lossy().into_owned();
            let ty = if p.join("wireless").exists() || p.join("phy80211").exists() { "wifi" } else { "ethernet" };
            adapters.push((name, ty.to_string()));
        }
    }
    adapters.sort();
    if adapters.is_empty() {
        // a network controller without a driver
        let lspci = sh(&["lspci", "-k"]).stdout;
        let mut no_driver = vec![];
        let mut cur: Option<String> = None;
        let mut has_driver = false;
        for l in lspci.lines().chain(std::iter::once("")) {
            if !l.starts_with(char::is_whitespace) {
                if let Some(c) = cur.take() { if !has_driver { no_driver.push(c); } }
                has_driver = false;
                if l.contains("Network controller") || l.contains("Ethernet controller") { cur = Some(l.splitn(2, ": ").nth(1).unwrap_or(l).to_string()); }
            } else if l.contains("Kernel driver in use") { has_driver = true; }
        }
        if let Some(c) = no_driver.first() {
            r.problem("The network adapter driver is missing", format!("{c} has no driver. Install the linux-firmware package, or the driver for this adapter, and restart the computer."), None);
        } else {
            r.problem("Windows couldn't find a network adapter", "No Ethernet or wireless adapter was found. If the computer has one, it may be turned off in the computer's setup (BIOS/UEFI).", None);
        }
        return;
    }
    r.ok("Windows couldn't find a network adapter");
    let nm: Vec<Vec<String>> = if have("nmcli") { sh(&["nmcli", "-t", "-f", "DEVICE,TYPE,STATE", "device"]).stdout.lines().map(aerode::sys::nm_split).filter(|f| f.len() >= 3).collect() } else { vec![] };
    for (name, ty) in &adapters {
        let kind = if ty == "wifi" { "wireless" } else { "Ethernet" };
        let state = nm.iter().find(|d| d[0] == *name).map(|d| d[2].clone()).unwrap_or_default();
        let oper = std::fs::read_to_string(format!("/sys/class/net/{name}/operstate")).unwrap_or_default().trim().to_string();
        let carrier = std::fs::read_to_string(format!("/sys/class/net/{name}/carrier")).unwrap_or_default().trim().to_string();
        r.info(format!("{name} ({kind}): {}{}", if state.is_empty() { &oper } else { &state }, if carrier == "0" { ", no link" } else { "" }));
        if state == "unmanaged" {
            r.problem(&format!("The {kind} adapter {name} is not used"), "NetworkManager is told to leave this adapter alone, so it is never connected.",
                Some(("Let Windows use the adapter", run1(&["nmcli", "device", "set", name, "managed", "yes"]))));
            continue;
        }
        r.ok(&format!("The {kind} adapter {name} is not used"));
        if ty == "ethernet" && (carrier == "0" || state == "unavailable") {
            r.problem(&format!("An Ethernet cable is not properly plugged in or might be broken ({name})"), "Make sure the cable is plugged into the computer and the router, and try another cable.", None);
            continue;
        }
        if oper == "down" && state != "unavailable" && state != "disconnected" && !state.is_empty() {
            r.problem(&format!("The adapter {name} is turned off"), "The network adapter is disabled.", Some(("Enable the network adapter", admin1(&["ip", "link", "set", name, "up"]))));
            continue;
        }
        if state == "disconnected" {
            r.problem(&format!("The adapter {name} is not connected"), "The adapter works, but it isn't connected to a network.", Some(("Connect the adapter", run1(&["nmcli", "device", "connect", name]))));
        } else { r.ok(&format!("The adapter {name} is not connected")); }
    }
}

/// Performance: memory, virtual memory, busy processes, startup programs, visual effects.
fn d_performance(r: &mut Report) {
    r.step("Checking memory");
    let mi = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |k: &str| mi.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let (total, avail, swap) = (kb("MemTotal:"), kb("MemAvailable:"), kb("SwapTotal:"));
    r.info(format!("Memory: {} available of {}; virtual memory: {}", gb(avail * 1024), gb(total * 1024), if swap > 0 { gb(swap * 1024) } else { "none".into() }));
    let top = |sort: &str| sh(&["ps", "-eo", "comm=,pid=,%cpu=,rss=", &format!("--sort=-{sort}")]).stdout.lines().take(3).map(|l| l.split_whitespace().next().unwrap_or("").to_string()).collect::<Vec<_>>().join(", ");
    if total > 0 && avail * 100 / total < 10 {
        r.problem("The computer is low on memory", format!("Only {} of memory is free. Close programs you aren't using; the ones using the most memory are: {}.", gb(avail * 1024), top("rss")), None);
    } else { r.ok("The computer is low on memory"); }
    if swap == 0 {
        r.problem("Virtual memory is turned off", "When memory runs out, programs are closed instead of being moved to virtual memory. Set up a swap file or zram (install zram-generator).", None);
    } else { r.ok("Virtual memory is turned off"); }
    r.step("Checking for busy programs");
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let load: f64 = std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok())).unwrap_or(0.0);
    r.info(format!("Load: {load:.2} on {cores} processor(s)"));
    if load > cores * 1.5 {
        r.problem("Programs are keeping the processor busy", format!("The processor is very busy. The programs using it most are: {}. Close them in Task Manager if you don't need them.", top("%cpu")), None);
    } else { r.ok("Programs are keeping the processor busy"); }
    r.step("Checking startup programs");
    let auto: Vec<String> = std::fs::read_dir(glib::user_config_dir().join("autostart")).map(|rd| rd.flatten()
        .filter(|e| { let s = std::fs::read_to_string(e.path()).unwrap_or_default(); !s.lines().any(|l| l.trim() == "Hidden=true" || l.trim() == "X-GNOME-Autostart-enabled=false") })
        .map(|e| e.file_name().to_string_lossy().trim_end_matches(".desktop").to_string()).collect()).unwrap_or_default();
    r.info(format!("Startup programs: {}", auto.len()));
    if auto.len() > 10 {
        r.problem("Too many programs start when Windows starts", format!("{} programs start when you log on ({}…). Turn off the ones you don't need.", auto.len(), auto.iter().take(4).cloned().collect::<Vec<_>>().join(", ")),
            Some(("Open Startup Programs", Fix::Main(|| util::spawn(&["aerode-control", "startup"])))));
    } else { r.ok("Too many programs start when Windows starts"); }
    r.step("Checking visual effects");
    let gl = sh(&["glxinfo", "-B"]).stdout;
    let software = gl.lines().find_map(|l| l.trim().strip_prefix("OpenGL renderer string:")).map(|s| s.contains("llvmpipe") || s.contains("softpipe")).unwrap_or(false);
    let composited = sh(&["pgrep", "-x", "picom"]).ok;
    if software && composited {
        r.problem("Visual effects slow down this computer", "Aero glass and animations are drawn without a graphics driver, which makes everything slower.",
            Some(("Turn off desktop composition", Fix::Main(|| {
                let _ = std::fs::create_dir_all(config::conf_dir());
                let _ = std::fs::write(config::conf_dir().join("no-compositor"), "");
                util::spawn(&["aerode", "restart"])
            }))));
    } else { r.ok("Visual effects slow down this computer"); }
}

fn d_homegroup(r: &mut Report) {
    r.step("Checking file sharing");
    if !(have("smbd") || Path::new("/usr/bin/smbd").exists() || Path::new("/usr/sbin/smbd").exists()) {
        r.problem("File sharing is not installed", "Sharing files and printers needs Samba. Install the samba package (sudo pacman -S samba), or open HomeGroup and click Install.", None);
        return;
    }
    r.ok("File sharing is not installed");
    r.step("Checking the file sharing service");
    let unit = if Path::new("/usr/lib/systemd/system/smb.service").exists() { "smb" } else { "smbd" };
    if !active(unit, false) {
        r.problem("The file sharing service is not running", "Other computers can't see the folders this computer shares.", Some(("Start the file sharing service", admin1(&["systemctl", "enable", "--now", unit]))));
    } else { r.ok("The file sharing service is not running"); }
    r.step("Checking shared folders");
    let o = sh(&["net", "usershare", "list"]);
    if !o.ok {
        r.problem("Folder sharing is turned off", format!("Sharing folders as a user is not set up ({}).", o.text().lines().next().unwrap_or("usershares are disabled")),
            Some(("Turn on folder sharing", Fix::Admin(super::homegroup::enable_argv()))));
    } else { r.ok("Folder sharing is turned off"); r.info(format!("Shared folders: {}", o.stdout.lines().count())); }
    if sh(&["getent", "group", "sambashare"]).ok {
        let groups = sh(&["id", "-nG"]).stdout;
        if !groups.split_whitespace().any(|g| g == "sambashare") {
            let user = glib::user_name().to_string_lossy().into_owned();
            r.problem("You are not allowed to share folders", "Your account is not in the sambashare group. After the repair, log off and log on again.", Some(("Allow your account to share folders", admin1(&["gpasswd", "-a", &user, "sambashare"]))));
        } else { r.ok("You are not allowed to share folders"); }
    }
    r.step("Checking network discovery");
    let nmb = if Path::new("/usr/lib/systemd/system/nmb.service").exists() { "nmb" } else { "nmbd" };
    if !active(nmb, false) && !active("wsdd", false) && !active("avahi-daemon", false) {
        r.problem("Network discovery is turned off", "Other computers can't find this computer by name in their Network folder.", Some(("Turn on network discovery", admin1(&["systemctl", "enable", "--now", nmb]))));
    } else { r.ok("Network discovery is turned off"); }
}

fn d_aero(r: &mut Report) {
    r.step("Checking desktop composition");
    let flag = config::conf_dir().join("no-compositor");
    if flag.exists() {
        r.problem("Desktop composition is disabled", "Aero glass, shadows and animations are turned off in Display settings.", Some(("Turn on desktop composition", Fix::Main(|| {
            let _ = std::fs::remove_file(config::conf_dir().join("no-compositor"));
            util::spawn(&["aerode", "restart"])
        }))));
        return;
    }
    r.ok("Desktop composition is disabled");
    if !have("picom") { r.problem("The Desktop Window Manager is not installed", "Aero effects need the picom compositor. Install the picom package.", None); return; }
    r.ok("The Desktop Window Manager is not installed");
    if !sh(&["pgrep", "-x", "picom"]).ok {
        r.problem("The Desktop Window Manager is not running", "The compositor that draws Aero glass is not running.", Some(("Restart the Desktop Window Manager", Fix::Main(|| util::spawn(&["aerode", "restart"])))));
    } else { r.ok("The Desktop Window Manager is not running"); }
    r.step("Checking color depth");
    let xd = sh(&["xdpyinfo"]).stdout;
    if let Some(d) = xd.lines().find_map(|l| l.trim().strip_prefix("depth of root window:")).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<u32>().ok()) {
        if d < 24 { r.problem("Color depth is too low", format!("The screen uses {d}-bit color; Aero needs 32-bit (24 bit) color."), None); } else { r.ok("Color depth is too low"); }
    }
    r.step("Checking the video driver");
    let gl = sh(&["glxinfo", "-B"]).stdout;
    if let Some(rend) = gl.lines().find_map(|l| l.trim().strip_prefix("OpenGL renderer string:")).map(|s| s.trim().to_string()) {
        r.info(format!("Renderer: {rend}"));
        if rend.contains("llvmpipe") || rend.contains("softpipe") || rend.contains("Software Rasterizer") {
            r.problem("The video card driver does not support Aero", format!("Graphics are drawn without 3D acceleration ({rend}). Install the driver for your graphics card (mesa, nvidia, vulkan drivers)."), None);
        } else { r.ok("The video card driver does not support Aero"); }
    }
}

/// (output, primary, current mode, current rate, preferred mode, [(mode, [rates])])
type XOut = (String, bool, String, f64, String, Vec<(String, Vec<f64>)>);

fn xrandr_outputs() -> Vec<XOut> {
    let mut outs: Vec<XOut> = vec![];
    for l in sh(&["xrandr", "--query"]).stdout.lines() {
        if !l.starts_with(' ') {
            if l.contains(" connected") {
                let name = l.split_whitespace().next().unwrap_or("").to_string();
                let cur = l.split_whitespace().find(|w| w.contains('x') && w.contains('+')).map(|w| w.split('+').next().unwrap_or("").to_string()).unwrap_or_default();
                outs.push((name, l.contains("primary"), cur, 0.0, String::new(), vec![]));
            } else { outs.push((String::new(), false, String::new(), 0.0, String::new(), vec![])); }
        } else if let Some(o) = outs.last_mut() {
            if o.0.is_empty() { continue; }
            let mut it = l.split_whitespace();
            let Some(mode) = it.next() else { continue };
            if !mode.contains('x') { continue; }
            let mut rates = vec![];
            for t in it {
                let v: f64 = t.trim_end_matches(['*', '+']).parse().unwrap_or(0.0);
                if v <= 0.0 { continue; }
                if t.contains('*') { o.3 = v; }
                if t.contains('+') && o.4.is_empty() { o.4 = mode.to_string(); }
                rates.push(v);
            }
            match o.5.iter_mut().find(|(m, _)| m == mode) { Some(e) => e.1.extend(rates), None => o.5.push((mode.to_string(), rates)) }
        }
    }
    outs.retain(|o| !o.0.is_empty());
    outs
}

fn d_display(r: &mut Report) {
    r.step("Checking displays");
    if !have("xrandr") { r.problem("Display settings can't be read", "The xrandr program is missing (install xorg-xrandr).", None); return; }
    let outs = xrandr_outputs();
    for o in &outs { r.info(format!("{}: {} at {:.0} Hz (native {})", o.0, if o.2.is_empty() { "off" } else { &o.2 }, o.3, o.4)); }
    let saved = config::conf_dir().join("displays.sh");
    if let Ok(script) = std::fs::read_to_string(&saved) {
        // the settings chosen in Control Panel ▸ Display: are they what the screens show?
        r.step("Checking saved display settings");
        let toks: Vec<&str> = script.split_whitespace().collect();
        let mut differs = vec![];
        let mut i = 0;
        while i < toks.len() {
            if toks[i] == "--output" && i + 1 < toks.len() {
                let name = toks[i + 1];
                let get = |flag: &str| toks[i + 2..].iter().take_while(|t| **t != "--output").position(|t| *t == flag).and_then(|p| toks.get(i + 2 + p + 1)).copied();
                if let Some(o) = outs.iter().find(|o| o.0 == name) {
                    if let Some(m) = get("--mode") { if m != o.2 { differs.push(format!("{name} is at {} instead of {m}", if o.2.is_empty() { "off" } else { &o.2 })); } }
                    if let Some(rt) = get("--rate").and_then(|x| x.parse::<f64>().ok()) { if (rt - o.3).abs() > 1.0 && !o.2.is_empty() { differs.push(format!("{name} runs at {:.0} Hz instead of {rt:.0} Hz", o.3)); } }
                }
                i += 2;
            } else { i += 1; }
        }
        if differs.is_empty() { r.ok("Saved display settings are not applied"); }
        else { r.problem("Saved display settings are not applied", format!("{}.", differs.join("; ")), Some(("Apply the saved display settings", run1(&["sh", &saved.to_string_lossy()])))); }
        return;
    }
    r.step("Checking refresh rates");
    for o in &outs {
        if o.2.is_empty() { continue; }
        let max = o.5.iter().find(|(m, _)| *m == o.2).map(|(_, rs)| rs.iter().cloned().fold(0.0, f64::max)).unwrap_or(0.0);
        let name = format!("The refresh rate of {} is lower than it supports", o.0);
        if max > o.3 + 1.5 {
            r.problem(&name, format!("{} runs at {:.0} Hz, but supports {max:.0} Hz at {}.", o.0, o.3, o.2), Some(("Use the highest refresh rate", run1(&["xrandr", "--output", &o.0, "--mode", &o.2, "--rate", &format!("{max:.2}")]))));
        } else { r.ok(&name); }
        if !o.4.is_empty() && o.4 != o.2 {
            let pr = o.5.iter().find(|(m, _)| *m == o.4).map(|(_, rs)| rs.iter().cloned().fold(0.0, f64::max)).unwrap_or(60.0);
            r.problem(&format!("{} is not at its recommended resolution", o.0), format!("{} is at {}; its native resolution is {}. Text and pictures look sharpest at the native resolution.", o.0, o.2, o.4),
                Some(("Reset the resolution", run1(&["xrandr", "--output", &o.0, "--mode", &o.4, "--rate", &format!("{pr:.2}")]))));
        } else { r.ok(&format!("{} is not at its recommended resolution", o.0)); }
    }
}

fn d_icons(r: &mut Report) {
    r.step("Checking the icon theme");
    let icons = glib::user_data_dir().join("icons/Aerode");
    let cursors = glib::user_data_dir().join("icons/Aerode-cursors/cursors");
    let src = std::fs::read_to_string(config::data_dir().join("source")).unwrap_or_default().trim().to_string();
    if !icons.join("index.theme").exists() {
        r.problem("The Windows 7 icons are missing", "The Aerode icon theme has not been built from your Windows 7 image yet.", Some(("Rebuild the icons", run1(&["aerode-extract", "--icons"]))));
    } else {
        r.ok("The Windows 7 icons are missing");
        r.step("Checking the icon cache");
        let mtime = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
        let cache = mtime(&icons.join("icon-theme.cache"));
        let newest = std::fs::read_dir(&icons).map(|rd| rd.flatten().filter_map(|e| mtime(&e.path())).max()).ok().flatten();
        let stale = match (cache, newest) { (None, _) => true, (Some(c), Some(n)) => n > c + std::time::Duration::from_secs(2), _ => false };
        if stale { r.problem("The icon cache is out of date", "Programs may show old or missing icons.", Some(("Rebuild the icon cache", run1(&["gtk-update-icon-cache", "-f", "-t", &icons.to_string_lossy()])))); }
        else { r.ok("The icon cache is out of date"); }
    }
    r.step("Checking mouse pointers");
    if !cursors.is_dir() {
        let fix = if !src.is_empty() && Path::new(&src).exists() { Some(("Extract the pointers from the Windows 7 image again", run1(&["aerode-extract", &src]))) } else { None };
        r.problem("The Windows 7 mouse pointers are missing", if fix.is_some() { "The Aero pointer scheme is not installed." } else { "The Aero pointer scheme is not installed. Run aerode-extract with your Windows 7 ISO to install it." }, fix);
    } else { r.ok("The Windows 7 mouse pointers are missing"); }
}

fn layout_code(l: &aerode::kbdlayout::Layout) -> String { if l.variant.is_empty() { l.layout.clone() } else { format!("{}({})", l.layout, l.variant) } }

fn d_keyboard(r: &mut Report) {
    r.step("Checking keyboard layouts");
    let cfg = config::get();
    let (cur, opts) = aerode::kbdlayout::current();
    let now: Vec<String> = cur.iter().map(layout_code).collect();
    r.info(format!("Layouts in use: {}", now.join(", ")));
    let sw = cfg.kbd_switch.clone().unwrap_or_else(|| aerode::kbdlayout::DEFAULT_SWITCH.to_string());
    let sw_ok = sw.is_empty() || opts.iter().any(|o| *o == sw);
    if (!cfg.kbd_layouts.is_empty() && cfg.kbd_layouts != now) || !sw_ok {
        r.problem("The keyboard layouts are not the ones you chose", format!("You chose {} in Region and Language, but {} is in use{}.", if cfg.kbd_layouts.is_empty() { now.join(", ") } else { cfg.kbd_layouts.join(", ") }, now.join(", "), if sw_ok { "" } else { ", without the key that switches between them" }),
            Some(("Apply your keyboard layouts again", Fix::Main(|| { aerode::kbdlayout::ensure(); true }))));
    } else { r.ok("The keyboard layouts are not the ones you chose"); }
    r.step("Checking key repeat");
    let q = sh(&["xset", "q"]).stdout;
    if q.lines().any(|l| l.contains("auto repeat:") && l.contains("off")) {
        r.problem("Key repeat is turned off", "Holding down a key types it only once.", Some(("Turn on key repeat", run1(&["xset", "r", "on"]))));
    } else { r.ok("Key repeat is turned off"); }
}

fn d_printer(r: &mut Report) {
    r.step("Checking the print spooler");
    if !have("lpstat") { r.problem("Printing is not installed", "Printing needs CUPS. Install the cups package (and system-config-printer to add printers).", None); return; }
    r.ok("Printing is not installed");
    if !active("cups", false) && !active("cups.socket", false) {
        r.problem("The print spooler service is not running", "CUPS, which sends documents to printers, is not running.", Some(("Start the print spooler service", admin1(&["systemctl", "enable", "--now", "cups"]))));
        return;
    }
    r.ok("The print spooler service is not running");
    r.step("Checking printers");
    let p = sh(&["lpstat", "-p"]).stdout;
    let printers: Vec<String> = p.lines().filter_map(|l| l.strip_prefix("printer ").and_then(|s| s.split_whitespace().next()).map(String::from)).collect();
    if printers.is_empty() { r.problem("No printers are installed", "Add a printer in Devices and Printers.", None); return; }
    r.ok("No printers are installed");
    r.info(format!("Printers: {}", printers.join(", ")));
    if sh(&["lpstat", "-d"]).stdout.contains("no system default") {
        r.problem("No default printer is set", "Programs don't know which printer to use.", Some(("Make the first printer the default", run1(&["lpoptions", "-d", &printers[0]]))));
    } else { r.ok("No default printer is set"); }
    for l in p.lines() {
        let Some(name) = l.strip_prefix("printer ").and_then(|s| s.split_whitespace().next()) else { continue };
        if l.contains("disabled") {
            r.problem(&format!("The printer \"{name}\" is paused"), "The printer does not take new print jobs.", Some(("Resume the printer", Fix::Admin(sv(&["sh", "-c", "cupsenable \"$1\" && cupsaccept \"$1\"", "sh", name])))));
        }
    }
    r.step("Checking the print queue");
    let jobs = sh(&["lpstat", "-o"]).stdout;
    let n = jobs.lines().count();
    if n > 0 {
        r.problem("Print jobs are stuck in the print queue", format!("{n} document(s) are waiting to be printed."), Some(("Cancel the waiting print jobs", run1(&["cancel", "-a"]))));
    } else { r.ok("Print jobs are stuck in the print queue"); }
}

fn d_update(r: &mut Report) {
    r.step("Checking the update database");
    if !have("pacman") { r.info("pacman is not installed: package checks skipped"); }
    let lock = Path::new("/var/lib/pacman/db.lck");
    let running = sh(&["pgrep", "-x", "pacman"]).ok || ["yay", "paru", "pamac"].iter().any(|p| sh(&["pgrep", "-x", p]).ok);
    if lock.exists() && !running {
        r.problem("The update database is locked", "An update was interrupted and left /var/lib/pacman/db.lck behind, so no updates can be installed.", Some(("Unlock the update database", admin1(&["rm", "-f", "/var/lib/pacman/db.lck"]))));
    } else { r.ok("The update database is locked"); }
    r.step("Checking interrupted downloads");
    let parts = std::fs::read_dir("/var/cache/pacman/pkg").map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".part")).count()).unwrap_or(0);
    if parts > 0 {
        r.problem("Some update downloads are incomplete", format!("{parts} partly downloaded package(s) are in the package cache."), Some(("Delete the incomplete downloads", admin1(&["find", "/var/cache/pacman/pkg", "-maxdepth", "1", "-name", "*.part", "-delete"]))));
    } else { r.ok("Some update downloads are incomplete"); }
    r.step("Checking update servers");
    let ml = Path::new("/etc/pacman.d/mirrorlist");
    if let Some(age) = ml.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()) {
        let days = age.as_secs() / 86400;
        r.info(format!("Mirror list updated {days} days ago"));
        if days > 180 {
            let fix = if have("reflector") { Some(("Choose the fastest update servers", admin1(&["reflector", "--latest", "20", "--protocol", "https", "--sort", "rate", "--save", "/etc/pacman.d/mirrorlist"]))) }
                else if have("rankmirrors") || have("eos-rankmirrors") { Some(("Choose the fastest update servers", admin1(&["eos-rankmirrors"]))) } else { None };
            r.problem("The list of update servers is old", format!("The mirror list was last changed {days} days ago; servers in it may be gone or out of date."), fix);
        } else { r.ok("The list of update servers is old"); }
    }
}

fn d_maintenance(r: &mut Report) {
    r.step("Checking disk space");
    let home = config::home();
    for (label, p) in [("Local Disk (/)", PathBuf::from("/")), ("Home", home.clone())] {
        let o = sh(&["df", "-P", "-B1", &p.to_string_lossy()]);
        let Some(l) = o.stdout.lines().nth(1) else { continue };
        let f: Vec<&str> = l.split_whitespace().collect();
        let (Some(size), Some(avail)) = (f.get(1).and_then(|x| x.parse::<u64>().ok()), f.get(3).and_then(|x| x.parse::<u64>().ok())) else { continue };
        r.info(format!("{label}: {} free of {}", gb(avail), gb(size)));
        let name = format!("{label} is low on disk space");
        if size > 0 && (avail * 100 / size < 10 || avail < 3 << 30) {
            r.problem(&name, format!("Only {} of {} is free. The repairs below free up space.", gb(avail), gb(size)), None);
        } else { r.ok(&name); }
    }
    r.step("Looking for temporary files");
    let cache = glib::user_cache_dir();
    let mut found = vec![];
    let mut total = 0;
    for d in ["thumbnails", "yay", "paru", "pikaur", "pip", "go-build", "electron-builder", "node-gyp", "mesa_shader_cache", "fontconfig"] {
        let p = cache.join(d);
        if !p.is_dir() { continue; }
        let mut budget = 200_000;
        let n = dir_size(&p, &mut budget);
        if n > 50 << 20 { total += n; found.push((p, n)); }
    }
    if total > 1 << 30 {
        let list = found.iter().map(|(p, n)| format!("{} ({})", p.file_name().unwrap_or_default().to_string_lossy(), gb(*n))).collect::<Vec<_>>().join(", ");
        let mut cmd = vec!["rm".to_string(), "-rf".to_string(), "--".to_string()];
        cmd.extend(found.iter().map(|(p, _)| p.to_string_lossy().into_owned()));
        r.problem("Temporary files use a lot of disk space", format!("{} of caches that programs rebuild by themselves: {list}.", gb(total)), Some(("Delete the temporary files", Fix::Run(vec![cmd]))));
    } else { r.ok("Temporary files use a lot of disk space"); }
    r.step("Checking the package cache");
    let mut budget = 400_000;
    let pk = dir_size(Path::new("/var/cache/pacman/pkg"), &mut budget);
    if pk > 3 << 30 {
        let fix = have("paccache").then(|| ("Keep only the two newest versions of each package", admin1(&["paccache", "-rk2"])));
        r.problem("Old update files use a lot of disk space", format!("The package cache holds {} of downloaded packages{}.", gb(pk), if fix.is_none() { " (install pacman-contrib to clean it)" } else { "" }), fix);
    } else { r.ok("Old update files use a lot of disk space"); }
    r.step("Checking the system log");
    let du = sh(&["journalctl", "--disk-usage"]).stdout;
    let size = du.split_whitespace().find(|w| w.ends_with(['G', 'M']) && w[..w.len() - 1].parse::<f64>().is_ok()).map(|w| {
        let v: f64 = w[..w.len() - 1].parse().unwrap_or(0.0);
        if w.ends_with('G') { v * 1024.0 } else { v }
    }).unwrap_or(0.0);
    if size > 1024.0 {
        r.problem("System logs use a lot of disk space", format!("The system log takes {:.1} GB.", size / 1024.0), Some(("Shrink the system log to 500 MB", admin1(&["journalctl", "--vacuum-size=500M"]))));
    } else { r.ok("System logs use a lot of disk space"); }
    r.step("Looking for broken shortcuts");
    let mut broken = vec![];
    for d in [glib::user_data_dir().join("applications"), glib::user_config_dir().join("autostart"), util::special_dir(glib::UserDirectory::Desktop, "Desktop")] {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x != "desktop").unwrap_or(true) { continue; }
            let s = std::fs::read_to_string(&p).unwrap_or_default();
            if s.lines().any(|l| l.trim() == "Hidden=true") || !s.lines().any(|l| l.trim() == "Type=Application") { continue; }
            let exe = s.lines().find_map(|l| l.strip_prefix("TryExec=")).or_else(|| s.lines().find_map(|l| l.strip_prefix("Exec="))).unwrap_or("").trim().to_string();
            let Some(bin) = glib::shell_parse_argv(exe.as_str()).ok().and_then(|a| a.into_iter().find(|t| { let t = t.to_string_lossy(); !t.contains('=') || t.starts_with('/') }))
                .map(|t| t.to_string_lossy().into_owned()) else { continue };
            if bin == "env" || bin.is_empty() { continue; }
            if !util::which(&bin) { broken.push(p); }
        }
    }
    if broken.is_empty() { r.ok("Broken shortcuts"); }
    else {
        let to = config::data_dir().join("broken-shortcuts");
        let mut cmd = vec!["mv".to_string(), "-f".to_string(), "--".to_string()];
        cmd.extend(broken.iter().map(|p| p.to_string_lossy().into_owned()));
        cmd.push(to.to_string_lossy().into_owned());
        r.problem("Broken shortcuts", format!("{} shortcut(s) point to programs that are no longer installed: {}.", broken.len(), broken.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")),
            Some(("Remove the broken shortcuts (they are kept in ~/.local/share/aerode/broken-shortcuts)", Fix::Run(vec![sv(&["mkdir", "-p", &to.to_string_lossy()]), cmd]))));
    }
}

fn d_power(r: &mut Report) {
    r.step("Checking the power plan");
    let bat = aerode::sys::battery();
    let on_battery = bat.as_ref().map(|b| b.status == "Discharging").unwrap_or(false);
    match &bat { Some(b) => r.info(format!("Battery: {}% ({})", b.percent, b.status)), None => r.info("No battery") }
    let prof = aerode::sys::power_profile();
    if let Some(p) = &prof { r.info(format!("Power plan: {p}")); }
    if on_battery && prof.as_deref() == Some("performance") {
        r.problem("The power plan is set to High performance", "On battery, the High performance plan drains the battery quickly.", Some(("Change to the Balanced power plan", run1(&["powerprofilesctl", "set", "balanced"]))));
    } else { r.ok("The power plan is set to High performance"); }
    r.step("Checking display brightness");
    if on_battery {
        if let Some(b) = aerode::sys::backlight() {
            if b >= 95 { r.problem("Display brightness is set to maximum", "A bright screen uses a lot of power.", Some(("Lower the brightness to 70%", run1(&["brightnessctl", "set", "70%"])))); }
            else { r.ok("Display brightness is set to maximum"); }
        }
    }
    r.step("Checking display and sleep settings");
    let presenting = std::fs::read_to_string(config::conf_dir().join("presentation.json")).map(|s| s.contains("\"active\": true")).unwrap_or(false);
    let q = sh(&["xset", "q"]).stdout;
    let dpms_off = q.contains("DPMS is Disabled");
    let saver = q.lines().find_map(|l| l.trim().strip_prefix("timeout:")).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    if !presenting && dpms_off && saver == 0 && !q.is_empty() {
        r.problem("The display never turns off", "The screen stays on when the computer is not used.", Some(("Turn off the display after 10 minutes", Fix::Run(vec![sv(&["xset", "+dpms", "dpms", "600", "600", "600"]), sv(&["xset", "s", "600"])]))));
    } else { r.ok("The display never turns off"); }
}

fn d_hardware(r: &mut Report) {
    r.step("Checking wireless devices");
    if let Ok(rd) = std::fs::read_dir("/sys/class/rfkill") {
        for e in rd.flatten() {
            let p = e.path();
            let rd = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default().trim().to_string();
            let (name, ty, soft, hard) = (rd("name"), rd("type"), rd("soft"), rd("hard"));
            let what = match ty.as_str() { "wlan" => "Wireless", "bluetooth" => "Bluetooth", "wwan" => "Mobile broadband", _ => continue };
            let t = format!("{what} ({name}) is turned off");
            if hard == "1" { r.problem(&t, "It is turned off with a switch or key on the computer. Turn it on with that switch (often Fn+F2 or similar).", None); }
            else if soft == "1" { r.problem(&t, "The device is blocked in software.", Some(("Turn the device on", run1(&["rfkill", "unblock", if ty == "wlan" { "wifi" } else { &ty }])))); }
            else { r.ok(&t); }
        }
    }
    r.step("Checking Bluetooth");
    let bt = std::fs::read_dir("/sys/class/bluetooth").map(|mut r| r.next().is_some()).unwrap_or(false);
    if bt {
        if !active("bluetooth", false) { r.problem("The Bluetooth service is not running", "Bluetooth devices can't be added or used.", Some(("Start the Bluetooth service", admin1(&["systemctl", "enable", "--now", "bluetooth"])))); }
        else { r.ok("The Bluetooth service is not running"); }
    }
    r.step("Checking for device errors");
    let k = sh(&["journalctl", "-k", "-b", "--no-pager", "-o", "cat", "-p", "warning"]).stdout;
    let fw: Vec<&str> = k.lines().filter(|l| l.contains("firmware: failed to load") || (l.contains("Direct firmware load") && l.contains("failed"))).collect();
    if !fw.is_empty() { r.problem("A device is missing its firmware", format!("{}. Install the linux-firmware package (and sof-firmware for sound).", fw[0].trim()), None); }
    else { r.ok("A device is missing its firmware"); }
    let usb: Vec<&str> = k.lines().filter(|l| l.contains("device descriptor read") || l.contains("unable to enumerate USB device")).collect();
    if !usb.is_empty() { r.problem("A USB device is not working", format!("{}. Unplug the device and plug it into another USB port.", usb[0].trim()), None); }
    else { r.ok("A USB device is not working"); }
    if k.is_empty() { r.info("The kernel log can't be read by this account (add yourself to the wheel or systemd-journal group)."); }
}

fn d_programs(r: &mut Report) {
    r.step("Looking for Windows programs");
    let mut exes = vec![];
    for d in [util::special_dir(glib::UserDirectory::Downloads, "Downloads"), util::special_dir(glib::UserDirectory::Desktop, "Desktop")] {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_lowercase();
                if n.ends_with(".exe") || n.ends_with(".msi") { exes.push(e.file_name().to_string_lossy().into_owned()); }
            }
        }
    }
    if exes.is_empty() { r.info("No Windows programs (.exe, .msi) were found in Downloads or on the desktop."); }
    if !exes.is_empty() && !have("wine") {
        r.problem("Programs made for Windows can't run", format!("{} needs Wine to run. Install the wine package (and winetricks).", exes[0]), None);
    } else { r.ok("Programs made for Windows can't run"); }
    if have("wine") {
        r.step("Checking how Windows programs open");
        let cur = sh(&["xdg-mime", "query", "default", "application/x-ms-dos-executable"]).stdout.trim().to_string();
        let wine_desktop = ["/usr/share/applications/wine.desktop", "/usr/local/share/applications/wine.desktop"].iter().any(|p| Path::new(p).exists());
        if !cur.starts_with("wine") && wine_desktop {
            r.problem("Windows programs don't open with Wine", "Double-clicking an .exe file doesn't start it.", Some(("Open Windows programs with Wine", run1(&["xdg-mime", "default", "wine.desktop", "application/x-ms-dos-executable", "application/x-msdownload", "application/x-msi", "application/x-ms-shortcut"]))));
        } else { r.ok("Windows programs don't open with Wine"); }
    }
    r.step("Looking for programs that won't start");
    let mut missing = vec![];
    if let Ok(rd) = std::fs::read_dir(glib::user_data_dir().join("applications")) {
        for e in rd.flatten() {
            let s = std::fs::read_to_string(e.path()).unwrap_or_default();
            if let Some(t) = s.lines().find_map(|l| l.strip_prefix("TryExec=")) { if !util::which(t.trim()) { missing.push(e.file_name().to_string_lossy().into_owned()); } }
        }
    }
    if missing.is_empty() { r.ok("A program's shortcut points to a missing file"); }
    else { r.problem("A program's shortcut points to a missing file", format!("{} – run the System Maintenance troubleshooter to remove broken shortcuts.", missing.join(", ")), None); }
}
