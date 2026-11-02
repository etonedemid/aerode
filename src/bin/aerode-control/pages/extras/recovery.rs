//! Recovery: "Restore this computer to an earlier point in time" – System Restore backed by
//! Timeshift or Snapper snapshots (listed and restored through pkexec, like Windows asks for UAC
//! before System Restore starts), plus the advanced recovery methods.
use gtk::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

use aerode::util;
use crate::{link, title, Cp};
use super::common::*;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tool { Timeshift, Snapper }

impl Tool {
    fn name(self) -> &'static str { match self { Tool::Timeshift => "Timeshift", Tool::Snapper => "Snapper" } }
}

fn tools() -> Vec<Tool> {
    let mut v = vec![];
    if util::which("timeshift") { v.push(Tool::Timeshift); }
    if util::which("snapper") { v.push(Tool::Snapper); }
    v
}

/// One restore point.
#[derive(Clone, Debug)]
struct Point {
    tool: Tool,
    /// timeshift snapshot name / snapper number
    id: String,
    time: Option<chrono::DateTime<chrono::Local>>,
    time_text: String,
    desc: String,
    /// Windows 7 "Type": System / Manual / Install
    kind: &'static str,
    /// only shown with "Show more restore points"
    more: bool,
}

impl Point {
    fn when(&self) -> String { self.time.map(win_time).unwrap_or_else(|| self.time_text.clone()) }
}

fn parse_timeshift(s: &str) -> Vec<Point> {
    let mut v = vec![];
    let mut table = false;
    for l in s.lines() {
        if l.starts_with("---") { table = true; continue; }
        if !table { continue; }
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || f[0].parse::<u32>().is_err() { continue; }
        let mut i = 1;
        if f[i] == ">" { i += 1; }
        let Some(name) = f.get(i) else { continue };
        let mut rest = &f[i + 1..];
        let mut tags = "";
        if let Some(t) = rest.first() { if t.chars().all(|c| "OBHDWM".contains(c)) { tags = t; rest = &rest[1..]; } }
        let desc = rest.join(" ");
        let time = chrono::NaiveDateTime::parse_from_str(name, "%Y-%m-%d_%H-%M-%S").ok().and_then(|n| n.and_local_timezone(chrono::Local).single());
        let manual = tags.contains('O');
        v.push(Point {
            tool: Tool::Timeshift, id: name.to_string(), time, time_text: name.to_string(),
            desc: if desc.is_empty() { if tags.contains('B') { "Automatic Restore Point (startup)".into() } else { "Automatic Restore Point".into() } } else { desc },
            kind: if manual { "Manual" } else { "System" }, more: false,
        });
    }
    v
}

/// Minimal CSV (snapper --csvout): commas, double quotes, "" escapes.
fn csv_line(l: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let (mut q, mut prev_q) = (false, false);
    for c in l.chars() {
        match c {
            '"' if q => { q = false; prev_q = true; continue; }
            '"' => { if prev_q { out.last_mut().unwrap().push('"'); } q = true; }
            ',' if !q => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
        prev_q = false;
    }
    out
}

fn snapper_time(s: &str) -> Option<chrono::DateTime<chrono::Local>> {
    let parse = |s: &str| ["%Y-%m-%d %H:%M:%S", "%a %d %b %Y %I:%M:%S %p", "%a %d %b %Y %H:%M:%S", "%a %b %e %H:%M:%S %Y"].iter()
        .find_map(|f| chrono::NaiveDateTime::parse_from_str(s, f).ok()).and_then(|n| n.and_local_timezone(chrono::Local).single());
    let s = s.trim();
    // "Sat 14 Oct 2023 10:00:01 AM CEST": also try without the zone
    parse(s).or_else(|| s.rsplit_once(' ').and_then(|(a, _)| parse(a)))
}

fn parse_snapper(s: &str) -> Vec<Point> {
    let mut lines = s.lines().filter(|l| !l.trim().is_empty());
    let Some(head) = lines.next() else { return vec![] };
    let csv = !head.contains('|');
    let split = |l: &str| -> Vec<String> { if csv { csv_line(l) } else { l.split('|').map(|x| x.trim().to_string()).collect() } };
    let cols: Vec<String> = split(head).into_iter().map(|c| c.trim().to_lowercase()).collect();
    let col = |names: &[&str]| cols.iter().position(|c| names.contains(&c.as_str()));
    let (n, ty, date, desc, clean) = (col(&["number", "#"]), col(&["type"]), col(&["date"]), col(&["description"]), col(&["cleanup"]));
    let mut v = vec![];
    for l in lines {
        if l.starts_with("---") || l.contains("-+-") { continue; }
        let f = split(l);
        let get = |i: Option<usize>| i.and_then(|i| f.get(i)).map(|s| s.trim().to_string()).unwrap_or_default();
        let num = get(n).trim_end_matches(['*', '-', '+']).to_string();
        if num.is_empty() || num == "0" || num.parse::<u32>().is_err() { continue; }
        let (t, c, d) = (get(ty), get(clean), get(desc));
        let dt = get(date);
        v.push(Point {
            tool: Tool::Snapper, id: num, time: snapper_time(&dt), time_text: dt,
            desc: if d.is_empty() { if c == "timeline" { "Automatic Restore Point".into() } else { "Restore point".into() } } else { d },
            kind: if t == "pre" || t == "post" { "Install" } else if c == "timeline" { "System" } else { "Manual" },
            more: t == "post",
        });
    }
    v
}

/// The newest five stay visible (Windows shows the recent ones, older ones behind "Show more").
fn finish(mut v: Vec<Point>) -> Vec<Point> {
    v.sort_by(|a, b| b.time.cmp(&a.time).then(b.id.cmp(&a.id)));
    let mut shown = 0;
    for p in v.iter_mut() {
        if p.more { continue; }
        shown += 1;
        if shown > 5 { p.more = true; }
    }
    v
}

fn list_argv(t: Tool) -> Vec<&'static str> {
    match t {
        Tool::Timeshift => vec!["timeshift", "--list", "--scripted"],
        Tool::Snapper => vec!["sh", "-c", "snapper --csvout -c root list --columns number,type,date,description,cleanup 2>/dev/null || snapper -c root list"],
    }
}

/// Restore (keeping the current state as a restore point first, so the restore can be undone).
fn restore_argv(p: &Point) -> Vec<String> {
    match p.tool {
        Tool::Timeshift => sv(&["sh", "-c", "timeshift --create --comments 'Before System Restore' --tags O --scripted >/dev/null 2>&1; timeshift --restore --snapshot \"$1\" --skip-grub --scripted --yes", "sh", &p.id]),
        Tool::Snapper => sv(&["sh", "-c", "snapper -c root create --description 'Before System Restore' --cleanup-algorithm number && snapper -c root undochange \"$1..0\"", "sh", &p.id]),
    }
}

fn create_argv(t: Tool, desc: &str) -> Vec<String> {
    match t {
        Tool::Timeshift => sv(&["timeshift", "--create", "--comments", desc, "--tags", "O", "--scripted"]),
        Tool::Snapper => sv(&["snapper", "-c", "root", "create", "--description", desc, "--cleanup-algorithm", "number"]),
    }
}

fn no_tools(parent: Option<&gtk::Window>) {
    message(parent, gtk::MessageType::Info, "System Restore", "System Restore is not set up on this computer.",
        "Restore points are snapshots of the system drive made by Timeshift or Snapper. Install the timeshift package (or snapper together with snap-pac, which makes a restore point before every update), turn on snapshots for the system drive, and then open System Restore again.");
}

pub fn page_recovery(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    side_later(cp, |cp| { let me = cp.clone(); cp.side.pack_start(&link("Create a restore point", move || create_point(Some(&me.win.clone().upcast()))), false, false, 2); });
    let holder = vbox(0);
    holder.pack_start(&main_page(cp, &holder), true, true, 0);
    holder.upcast()
}

fn main_page(cp: &Rc<Cp>, holder: &gtk::Box) -> gtk::Widget {
    let v = vbox(10);
    v.pack_start(&title("Restore this computer to an earlier point in time"), false, false, 0);
    let row = hbox(16);
    row.set_margin_start(16);
    let open = shield_button("Open System _Restore");
    open.set_valign(gtk::Align::Start);
    { let me = cp.clone(); open.connect_clicked(move |_| system_restore(Some(&me.win.clone().upcast()))); }
    row.pack_start(&open, false, false, 0);
    row.pack_start(&label("Undo recent system changes, but leave files such as documents, pictures, and music unchanged. This might remove recently installed programs and drivers."), true, true, 0);
    v.pack_start(&row, false, false, 6);
    let me = cp.clone();
    let h = holder.clone();
    let adv = cmd_link(None, "Advanced recovery methods", "", move || {
        let page = advanced_page(&me);
        subpage(&me, &h, "Advanced Recovery Methods", &page);
    });
    adv.set_halign(gtk::Align::Start);
    v.pack_start(&adv, false, false, 0);
    // what restore points there are, at a glance (the snapshot tools' configs are world-readable)
    let ts = tools();
    let status = if ts.is_empty() { "System Restore is not set up: neither Timeshift nor Snapper is installed.".to_string() }
        else { format!("Restore points are made by {}.", ts.iter().map(|t| t.name()).collect::<Vec<_>>().join(" and ")) };
    let s = grey(&status);
    s.set_margin_top(18);
    v.pack_start(&s, false, false, 0);
    v.upcast()
}

fn os_name() -> String {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    os.lines().find_map(|l| l.strip_prefix("NAME=")).map(|s| s.trim_matches('"').to_string()).unwrap_or_else(|| "Linux".into())
}

fn os_url() -> String {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    os.lines().find_map(|l| l.strip_prefix("HOME_URL=")).map(|s| s.trim_matches('"').to_string()).unwrap_or_default()
}

fn advanced_page(cp: &Rc<Cp>) -> gtk::Widget {
    let v = vbox(10);
    v.pack_start(&title("Select an advanced recovery method"), false, false, 0);
    let me = cp.clone();
    v.pack_start(&cmd_link(Some("drive-harddisk-system"), "Use a system image you created earlier to recover your computer",
        "If you have created a system image, you can use it to replace everything on this computer, including Windows, your programs, and all files with the information stored on the system image.",
        move || {
            let parent: gtk::Window = me.win.clone().upcast();
            if let Some(gui) = ["timeshift-launcher", "timeshift-gtk"].into_iter().find(|g| util::which(g)) {
                util::spawn(&[gui]);
            } else {
                message(Some(&parent), gtk::MessageType::Info, "Recovery", "Restore a system image",
                    "A full system image (a Timeshift snapshot on another drive, a Clonezilla or dd image) is restored with the computer started from a live USB drive: start the computer from the installation USB drive, then restore the image there. Windows Backup images are not supported.");
            }
        }), false, false, 0);
    let os = os_name();
    let me = cp.clone();
    let os2 = os.clone();
    v.pack_start(&cmd_link(Some("system-software-install"), &format!("Reinstall {os} (requires an installation USB drive or disc)"),
        &format!("This will reinstall {os} on your computer. Afterward, you can restore your files from a backup. Any programs you've installed will need to be reinstalled. If your home folder is on its own partition, the installer can keep it."),
        move || {
            let parent: gtk::Window = me.win.clone().upcast();
            let url = os_url();
            let more = if url.is_empty() { String::new() } else { format!("\n\nThe installation image can be downloaded from {url}") };
            message(Some(&parent), gtk::MessageType::Info, "Recovery", &format!("Reinstall {os2}"),
                &format!("Back up your files first (Backup and Restore), then start the computer from the {os2} installation USB drive or disc and choose to install. To keep your files, choose manual partitioning and do not format the partition that holds /home.{more}"));
        }), false, false, 0);
    v.upcast()
}

// ------------------------------------------------------------------ System Restore (rstrui)

struct Restore {
    wiz: Rc<Wizard>,
    points: RefCell<Vec<Point>>,
    /// None while the restore points are still loading
    error: RefCell<Option<String>>,
    chosen: RefCell<Option<Point>>,
    /// the list page, refreshed when the points arrive
    list: RefCell<Option<(gtk::ListStore, gtk::CheckButton, gtk::Box)>>,
    /// the welcome page's "Recommended restore" area, filled when the points arrive
    rec_slot: RefCell<Option<gtk::Box>>,
    /// "Recommended restore" (true) or "Choose a different restore point"
    use_rec: Cell<bool>,
}

pub(super) fn system_restore(parent: Option<&gtk::Window>) {
    let ts = tools();
    if ts.is_empty() { no_tools(parent); return; }
    let wiz = wizard("System Restore", "drive-harddisk-system", parent, (620, 470), false);
    let r = Rc::new(Restore { wiz, points: RefCell::new(vec![]), error: RefCell::new(None), chosen: RefCell::new(None), list: RefCell::new(None),
        rec_slot: RefCell::new(None), use_rec: Cell::new(true) });
    welcome(&r);
    r.wiz.win.show_all();
    r.wiz.win.present();
    // like rstrui: elevation first, the list loads while the welcome page is shown
    load_points(&r, ts);
}

fn header(text: &str, sub: &str) -> gtk::Box {
    let v = vbox(4);
    let h = gtk::Label::new(None);
    h.set_markup(&format!("<b>{}</b>", esc(text)));
    h.set_xalign(0.0);
    h.set_line_wrap(true);
    v.pack_start(&h, false, false, 0);
    if !sub.is_empty() { v.pack_start(&label(sub), false, false, 0); }
    v
}

fn nav_buttons(r: &Rc<Restore>, next_label: &str) -> (gtk::Button, gtk::Button, gtk::Button) {
    let back = gtk::Button::with_mnemonic("< _Back");
    let next = gtk::Button::with_mnemonic(next_label);
    let cancel = gtk::Button::with_label("Cancel");
    { let w = r.wiz.win.clone(); cancel.connect_clicked(move |_| w.close()); }
    r.wiz.set_buttons(&[&back, &next, &cancel]);
    (back, next, cancel)
}

/// The restore point Windows recommends: the newest one (usually made before the last update).
fn recommended(r: &Restore) -> Option<Point> { r.points.borrow().first().cloned() }

/// "Recommended restore" / "Choose a different restore point" (rstrui shows them once it knows
/// the restore points).
fn fill_recommended(r: &Rc<Restore>) {
    let Some(slot) = r.rec_slot.borrow().clone() else { return };
    for c in slot.children() { slot.remove(&c); }
    if r.error.borrow().is_none() { slot.pack_start(&loading("Looking for restore points…"), false, false, 0); slot.show_all(); return; }
    let Some(p) = recommended(r) else { r.use_rec.set(false); return };
    let rec = gtk::RadioButton::with_mnemonic("_Recommended restore:");
    let other = gtk::RadioButton::with_mnemonic_from_widget(&rec, "C_hoose a different restore point");
    rec.set_active(r.use_rec.get());
    other.set_active(!r.use_rec.get());
    slot.pack_start(&rec, false, false, 0);
    let d = vbox(4);
    d.set_margin_start(22);
    d.pack_start(&label("Select this option to undo the most recent update, driver, or software installation if you think it is causing problems."), false, false, 0);
    let g = gtk::Grid::new();
    g.set_column_spacing(16);
    g.set_margin_top(4);
    let when = gtk::Label::new(Some(&p.when()));
    when.set_xalign(0.0);
    when.set_valign(gtk::Align::Start);
    g.attach(&when, 0, 0, 1, 1);
    g.attach(&label(&format!("{}: {}", p.kind, p.desc)), 1, 0, 1, 1);
    d.pack_start(&g, false, false, 0);
    d.pack_start(&label(&format!("Current time zone: {}", time_zone())), false, false, 0);
    slot.pack_start(&d, false, false, 0);
    slot.pack_start(&other, false, false, 6);
    { let r2 = r.clone(); rec.connect_toggled(move |b| r2.use_rec.set(b.is_active())); }
    slot.show_all();
}

fn welcome(r: &Rc<Restore>) {
    let h = hbox(18);
    let img = util::image("drive-harddisk-system", 48);
    img.set_valign(gtk::Align::Start);
    h.pack_start(&img, false, false, 0);
    let v = vbox(12);
    v.pack_start(&header("Restore system files and settings", ""), false, false, 0);
    v.pack_start(&label("System Restore can help fix problems that might be making your computer run slowly or stop responding."), false, false, 0);
    v.pack_start(&label("System Restore does not affect any of your documents, pictures, or other personal data. Recently installed programs and drivers might be uninstalled."), false, false, 0);
    let w = r.wiz.win.clone();
    v.pack_start(&link("Is this process reversible?", move || message(Some(&w), gtk::MessageType::Info, "System Restore", "Is this process reversible?",
        "Yes. Before your computer is restored, the current state is saved as a restore point named \"Before System Restore\". If the restore doesn't fix the problem, open System Restore again and choose that restore point to undo it.")), false, false, 0);
    let slot = vbox(4);
    slot.set_margin_top(8);
    v.pack_start(&slot, false, false, 0);
    *r.rec_slot.borrow_mut() = Some(slot);
    fill_recommended(r);
    h.pack_start(&v, true, true, 0);
    r.wiz.page(&h);
    let (back, next, _) = nav_buttons(r, "_Next >");
    back.set_sensitive(false);
    let r2 = r.clone();
    next.connect_clicked(move |_| {
        match recommended(&r2).filter(|_| r2.use_rec.get() && r2.error.borrow().is_some()) {
            Some(p) => { *r2.chosen.borrow_mut() = Some(p); confirm(&r2, true); }
            None => choose(&r2),
        }
    });
}

fn load_points(r: &Rc<Restore>, ts: Vec<Tool>) {
    let r = r.clone();
    // one tool after the other (usually there is only one)
    fn step(r: Rc<Restore>, mut rest: Vec<Tool>, mut acc: Vec<Point>, mut errors: Vec<String>) {
        if rest.is_empty() {
            let pts = finish(acc);
            let err = if pts.is_empty() { errors.join("\n") } else { String::new() };
            *r.points.borrow_mut() = pts;
            *r.error.borrow_mut() = Some(err);
            refresh_list(&r);
            return;
        }
        let t = rest.remove(0);
        admin_read(&list_argv(t), move |o| {
            if o.cancelled() { errors.push("You must be an administrator to see the restore points.".into()); }
            else if !o.ok && o.stdout.trim().is_empty() { errors.push(format!("{}: {}", t.name(), o.text())); }
            match t { Tool::Timeshift => acc.extend(parse_timeshift(&o.stdout)), Tool::Snapper => acc.extend(parse_snapper(&o.stdout)) }
            step(r, rest, acc, errors);
        });
    }
    step(r, ts, vec![], vec![]);
}

fn refresh_list(r: &Rc<Restore>) {
    fill_recommended(r);
    let Some((store, more, status)) = r.list.borrow().clone() else { return };
    let Some(msg) = r.error.borrow().clone() else { return };
    store.clear();
    let pts = r.points.borrow();
    for (i, p) in pts.iter().enumerate() {
        if p.more && !more.is_active() { continue; }
        store.insert_with_values(None, &[(0, &p.when()), (1, &p.desc), (2, &p.kind), (3, &i.to_string())]);
    }
    let t = if !msg.is_empty() { msg } else if pts.is_empty() { "There are no restore points on this computer yet.".into() } else { String::new() };
    for c in status.children() { status.remove(&c); }
    if !t.is_empty() { status.pack_start(&grey(&t), false, false, 0); }
    status.show_all();
}

fn choose(r: &Rc<Restore>) {
    *r.rec_slot.borrow_mut() = None;
    let v = vbox(8);
    v.pack_start(&header("Restore your computer to the state it was in before the selected event", ""), false, false, 0);
    let w = r.wiz.win.clone();
    v.pack_start(&link("How do I choose a restore point?", move || message(Some(&w), gtk::MessageType::Info, "System Restore", "How do I choose a restore point?",
        "Choose the most recent restore point made before the problem started. \"Install\" restore points are made just before packages are installed or updated; \"System\" ones on a schedule; \"Manual\" ones when you created them.")), false, false, 0);
    let tz = time_zone();
    v.pack_start(&label(&format!("Current time zone: {tz}")), false, false, 0);
    let (tv, store) = table(4, &["Date and Time", "Description", "Type"]);
    expand_column(&tv, 1);
    if let Some(c) = tv.column(0) { c.set_min_width(165); }
    if let Some(c) = tv.column(2) { c.set_min_width(80); }
    v.pack_start(&scrolled(&tv, 190), true, true, 0);
    let row = hbox(8);
    let more = gtk::CheckButton::with_mnemonic("_Show more restore points");
    row.pack_start(&more, false, false, 0);
    let scan = gtk::Button::with_mnemonic("Scan for _affected programs");
    scan.set_sensitive(false);
    row.pack_end(&scan, false, false, 0);
    v.pack_start(&row, false, false, 0);
    let status = hbox(0);
    v.pack_start(&status, false, false, 0);
    r.wiz.page(&v);
    *r.list.borrow_mut() = Some((store.clone(), more.clone(), status.clone()));
    if r.error.borrow().is_none() { fill(&status, &loading("Loading restore points…")); } else { refresh_list(r); }
    { let r2 = r.clone(); more.connect_toggled(move |_| refresh_list(&r2)); }
    let (back, next, _) = nav_buttons(r, "_Next >");
    next.set_sensitive(false);
    let pick = { let r = r.clone(); let tv = tv.clone(); move || -> Option<Point> { selected(&tv, 3).and_then(|i| i.parse::<usize>().ok()).and_then(|i| r.points.borrow().get(i).cloned()) } };
    { let (next, scan, pick) = (next.clone(), scan.clone(), pick.clone()); tv.selection().connect_changed(move |_| { let s = pick().is_some(); next.set_sensitive(s); scan.set_sensitive(s); }); }
    { let (r2, pick) = (r.clone(), pick.clone()); scan.connect_clicked(move |_| if let Some(p) = pick() { affected(&r2.wiz.win, &p); }); }
    { let r2 = r.clone(); back.connect_clicked(move |_| welcome(&r2)); }
    { let r2 = r.clone(); next.connect_clicked(move |_| { if let Some(p) = pick() { *r2.chosen.borrow_mut() = Some(p); confirm(&r2, false); } }); }
}

/// `from_welcome`: Back returns to the welcome page (Recommended restore) instead of the list.
fn confirm(r: &Rc<Restore>, from_welcome: bool) {
    *r.rec_slot.borrow_mut() = None;
    let Some(p) = r.chosen.borrow().clone() else { return };
    let v = vbox(10);
    v.pack_start(&header("Confirm your restore point", "Your computer will be restored to the state it was in before the event in the Description field below."), false, false, 0);
    v.pack_start(&crate::kv_grid(&[
        ("Time:", format!("{} ({})", p.when(), time_zone())),
        ("Description:", format!("{}: {}", p.kind, p.desc)),
        ("Drives:", format!("Local Disk (/) (System) – {}", p.tool.name())),
    ]), false, false, 0);
    let (w, p2) = (r.wiz.win.clone(), p.clone());
    v.pack_start(&link("Scan for affected programs", move || affected(&w, &p2)), false, false, 0);
    v.pack_start(&label("Documents, pictures and other files in your home folder are not changed."), false, false, 0);
    v.pack_start(&label("System Restore needs to restart your computer to apply these changes. Before you proceed, save any open files and close all programs."), false, false, 0);
    r.wiz.page(&v);
    let (back, next, cancel) = nav_buttons(r, "_Finish");
    { let r2 = r.clone(); back.connect_clicked(move |_| if from_welcome { welcome(&r2) } else { choose(&r2) }); }
    let r2 = r.clone();
    next.connect_clicked(move |_| {
        if !ask(Some(&r2.wiz.win), gtk::MessageType::Warning, "System Restore", "Once started, System Restore cannot be interrupted. Do you want to continue?",
            &format!("Your computer will be restored to {} ({}). Programs and updates installed after that time will be removed.", p.when(), p.desc), "_Yes", "_No") { return; }
        let v = vbox(12);
        v.pack_start(&header("Please wait while your system files and settings are being restored.", ""), false, false, 0);
        v.pack_start(&loading("System Restore is restoring the system drive…"), false, false, 0);
        r2.wiz.page(&v);
        for c in r2.wiz.buttons.children() { c.set_sensitive(false); }
        let _ = &cancel;
        let r3 = r2.clone();
        let mut argv = vec!["pkexec".to_string()];
        argv.extend(restore_argv(&p));
        let when = p.when();
        change(argv, None, move |o| {
            if o.ok {
                let restart = ask(Some(&r3.wiz.win), gtk::MessageType::Info, "System Restore", "System Restore completed successfully.",
                    &format!("The system has been restored to {when}. Your documents have not been affected. Restart your computer now to finish."), "_Restart now", "Restart _later");
                r3.wiz.win.close();
                if restart && !dry_run() { aerode::sys::restart(); }
            } else {
                let why = if o.cancelled() { "You must be an administrator to restore the computer.".to_string() } else { o.text() };
                message(Some(&r3.wiz.win), gtk::MessageType::Error, "System Restore", "System Restore did not complete successfully.",
                    &format!("Your computer's system files and settings were not changed.\n\nDetails: {why}"));
                confirm(&r3, from_welcome);
            }
        });
    });
}

/// "Scan for affected programs": packages installed after the restore point (removed by the
/// restore) and packages removed since (brought back), from pacman's database in the snapshot.
fn affected(parent: &gtk::Window, p: &Point) {
    let cur: BTreeSet<String> = std::fs::read_dir("/var/lib/pacman/local").map(|r| r.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    let d = gtk::Dialog::with_buttons(Some("System Restore"), Some(parent), gtk::DialogFlags::MODAL, &[("_Close", gtk::ResponseType::Close)]);
    d.set_default_size(520, 460);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    c.pack_start(&crate::kv_grid(&[("Description:", p.desc.clone()), ("Date:", p.when())]), false, false, 0);
    let body = vbox(6);
    body.pack_start(&loading("Scanning for affected programs and drivers…"), false, false, 0);
    c.pack_start(&body, true, true, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
    let tool = p.tool;
    let argv: Vec<String> = match tool {
        Tool::Snapper => sv(&["pkexec", "snapper", "-c", "root", "status", &format!("{}..0", p.id)]),
        Tool::Timeshift => sv(&["pkexec", "sh", "-c", "for d in /timeshift/snapshots/\"$1\" /run/timeshift/*/backup/timeshift/snapshots/\"$1\"; do [ -d \"$d/localhost/var/lib/pacman/local\" ] && exec ls \"$d/localhost/var/lib/pacman/local\"; done; exit 3", "sh", &p.id]),
    };
    exec_async(argv, None, move |o| {
        for ch in body.children() { body.remove(&ch); }
        let (mut deleted, mut restored): (Vec<String>, Vec<String>) = (vec![], vec![]);
        let mut ok = o.ok;
        match tool {
            Tool::Snapper => for l in o.stdout.lines() {
                let Some((flags, path)) = l.split_once(' ') else { continue };
                let Some(pkg) = path.trim().strip_prefix("/var/lib/pacman/local/") else { continue };
                if pkg.contains('/') || pkg.is_empty() { continue; }
                if flags.starts_with('+') { deleted.push(pkg.to_string()); } else if flags.starts_with('-') { restored.push(pkg.to_string()); }
            },
            Tool::Timeshift => {
                let snap: BTreeSet<String> = o.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
                if snap.is_empty() { ok = false; }
                deleted = cur.difference(&snap).cloned().collect();
                restored = snap.difference(&cur).cloned().collect();
            }
        }
        if !ok {
            let why = if o.cancelled() { "You must be an administrator to scan the restore point." } else { "The programs in this restore point cannot be determined (Timeshift snapshots in BTRFS mode are only readable while Timeshift runs)." };
            body.pack_start(&label(why), false, false, 0);
            body.show_all();
            return;
        }
        let show = |t: &str, list: &[String], body: &gtk::Box| {
            body.pack_start(&label(t), false, false, 0);
            let (tv, store) = table(2, &["Description", "Type"]);
            for x in list {
                // pacman's "name-version-release" → "name version-release"
                let parts: Vec<&str> = x.rsplitn(3, '-').collect();
                let shown = if parts.len() == 3 { format!("{} {}-{}", parts[2], parts[1], parts[0]) } else { x.clone() };
                store.insert_with_values(None, &[(0, &shown), (1, &"Program".to_string())]);
            }
            if list.is_empty() { store.insert_with_values(None, &[(0, &"None detected".to_string()), (1, &String::new())]); }
            body.pack_start(&scrolled(&tv, 120), true, true, 0);
        };
        show("Programs and drivers that will be deleted:", &deleted, &body);
        show("Programs and drivers that might be restored. These programs might not work correctly after restore and might need to be reinstalled:", &restored, &body);
        body.show_all();
    });
}

/// System Protection's "Create a restore point".
fn create_point(parent: Option<&gtk::Window>) {
    let ts = tools();
    let Some(&tool) = ts.first() else { no_tools(parent); return };
    let d = gtk::Dialog::with_buttons(Some("System Protection"), parent, gtk::DialogFlags::MODAL, &[("C_reate", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    c.pack_start(&header("Create a restore point", "Type a description to help you identify the restore point. The current date and time are added automatically."), false, false, 0);
    let e = gtk::Entry::new();
    e.set_activates_default(true);
    c.pack_start(&e, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    let r = d.run();
    let desc = e.text().trim().to_string();
    d.close();
    if r != gtk::ResponseType::Ok || desc.is_empty() { return; }
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title("System Protection");
    w.set_transient_for(parent);
    w.set_modal(true);
    w.set_resizable(false);
    w.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    w.set_position(gtk::WindowPosition::CenterOnParent);
    let b = vbox(10);
    b.set_border_width(16);
    b.pack_start(&loading("Creating a restore point…"), false, false, 0);
    let pb = gtk::ProgressBar::new();
    b.pack_start(&pb, false, false, 0);
    w.add(&b);
    w.show_all();
    let pulse = glib::timeout_add_local(std::time::Duration::from_millis(120), { let pb = pb.clone(); move || { pb.pulse(); glib::ControlFlow::Continue } });
    let mut argv = vec!["pkexec".to_string()];
    argv.extend(create_argv(tool, &desc));
    let parent = parent.cloned();
    change(argv, None, move |o| {
        pulse.remove();
        w.close();
        if o.ok { message(parent.as_ref(), gtk::MessageType::Info, "System Protection", "The restore point was created successfully.", ""); }
        else if !o.cancelled() { message(parent.as_ref(), gtk::MessageType::Error, "System Protection", "The restore point could not be created.", &o.text()); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeshift_list() {
        let s = "Device : /dev/sda2\nMode   : BTRFS\nStatus : OK\n3 snapshots, 100.1 GB free\n\nNum     Name                 Tags  Description\n------------------------------------------------------------------------------\n0    >  2023-10-14_10-00-01  D     \n1    >  2023-10-15_10-00-01  O     before update\n";
        let v = parse_timeshift(s);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].id, "2023-10-15_10-00-01");
        assert_eq!(v[1].desc, "before update");
        assert_eq!(v[1].kind, "Manual");
        assert_eq!(v[0].desc, "Automatic Restore Point");
    }
    #[test]
    fn snapper_csv_and_table() {
        let csv = "number,type,date,description,cleanup\n0,single,,current,\n1,pre,2023-10-14 10:00:01,\"pacman -Syu, again\",number\n2,post,2023-10-14 10:01:00,linux,number\n";
        let v = parse_snapper(csv);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].desc, "pacman -Syu, again");
        assert_eq!(v[0].kind, "Install");
        assert!(v[0].time.is_some());
        assert!(v[1].more);
        let table = " # | Type   | Pre # | Date                     | User | Cleanup  | Description | Userdata\n---+--------+-------+--------------------------+------+----------+-------------+---------\n0  | single |       |                          | root |          | current     |\n5  | single |       | Sat Oct 14 10:00:01 2023 | root | timeline | timeline    |\n";
        let v = parse_snapper(table);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "5");
        assert_eq!(v[0].kind, "System");
    }
}
