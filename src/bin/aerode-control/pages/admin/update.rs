//! Windows Update on pacman: the Win7 status panel (green / yellow / red shield), Check for
//! updates (checkupdates, or pacman -Qu after a sync into a private database; AUR through yay or
//! paru), Select updates to install, Install updates (pkexec pacman -Syu with the progress shown
//! in the panel), View update history (/var/log/pacman.log) and Change settings.
use gtk::prelude::*;
use gtk::glib;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

use super::super::super::{link, section, text, title, Cp};
use super::*;
use aerode::config;

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Pkg { pub name: String, pub old: String, pub new: String, pub size: u64, pub repo: String, pub desc: String, pub aur: bool }

/// The last check, kept in ~/.cache/aerode/updates.json (Action Center reads it too).
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct Check { pub when: i64, pub pkgs: Vec<Pkg>, pub error: String, pub installed: i64 }

pub fn cache_file() -> std::path::PathBuf { cache_dir().join("updates.json") }
pub fn load_check() -> Check { std::fs::read_to_string(cache_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default() }
fn save_check(c: &Check) { let _ = std::fs::write(cache_file(), serde_json::to_string_pretty(c).unwrap_or_default()); }

/// "linux 6.11.1-1 -> 6.11.2-1" lines (checkupdates, pacman -Qu, yay -Qua).
pub fn parse_updates(out: &str, aur: bool) -> Vec<Pkg> {
    out.lines().filter_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() >= 4 && f[2] == "->" { Some(Pkg { name: f[0].into(), old: f[1].into(), new: f[3].into(), aur, ..Default::default() }) }
        else if f.len() == 2 && !l.contains("::") { Some(Pkg { name: f[0].into(), new: f[1].into(), aur, ..Default::default() }) }
        else { None }
    }).collect()
}

/// "12.34 MiB" → bytes
fn size_of(s: &str) -> u64 {
    let mut it = s.split_whitespace();
    let n: f64 = it.next().and_then(|x| x.replace(',', ".").parse().ok()).unwrap_or(0.0);
    (n * match it.next() { Some("KiB") => 1024.0, Some("MiB") => 1048576.0, Some("GiB") => 1073741824.0, _ => 1.0 }) as u64
}

/// Look for updates (worker thread). Never changes the system's package database.
pub fn check(with_aur: bool) -> Check {
    let mut c = Check { when: now(), installed: last_installed(), ..Default::default() };
    if !util::which("pacman") { c.error = "pacman was not found. Windows Update needs an Arch-based system.".into(); return c; }
    let db = cache_dir().join("checkup-db");
    let dbs = db.to_string_lossy().to_string();
    let mut synced = false;
    if util::which("checkupdates") {
        let o = std::process::Command::new("checkupdates").env("CHECKUPDATES_DB", &dbs).output();
        match o {
            Ok(o) if o.status.code() == Some(0) => { c.pkgs = parse_updates(&String::from_utf8_lossy(&o.stdout), false); synced = true; }
            Ok(o) if o.status.code() == Some(2) => synced = true,
            Ok(o) => { c.error = String::from_utf8_lossy(&o.stderr).trim().lines().last().unwrap_or("checkupdates failed").to_string(); }
            Err(e) => c.error = e.to_string(),
        }
    } else if util::which("fakeroot") {
        // what checkupdates does: a private copy of the sync databases next to the real local db
        let _ = std::fs::create_dir_all(&db);
        let local = db.join("local");
        if !local.exists() { let _ = std::os::unix::fs::symlink("/var/lib/pacman/local", &local); }
        let (ok, out) = run(&["fakeroot", "--", "pacman", "-Sy", "--dbpath", &dbs, "--logfile", "/dev/null"]);
        if ok { let (_, q) = run(&["pacman", "-Qu", "--dbpath", &dbs]); c.pkgs = parse_updates(&q, false); synced = true; } else { c.error = out.trim().lines().last().unwrap_or("").to_string(); }
    } else {
        // no way to sync without root: report what the system's (possibly old) databases know
        let (_, q) = run(&["pacman", "-Qu"]);
        c.pkgs = parse_updates(&q, false);
    }
    // sizes, repositories and descriptions
    if !c.pkgs.is_empty() {
        let mut a = vec!["pacman".to_string(), "-Si".into()];
        if synced { a.push("--dbpath".into()); a.push(dbs.clone()); }
        a.extend(c.pkgs.iter().map(|p| p.name.clone()));
        let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        let info = sh(&r);
        for block in info.split("\n\n") {
            let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim_start_matches([' ', ':']).trim().to_string())).unwrap_or_default();
            let name = get("Name");
            if let Some(p) = c.pkgs.iter_mut().find(|p| p.name == name) { p.size = size_of(&get("Download Size")); p.repo = get("Repository"); p.desc = get("Description"); }
        }
    }
    if with_aur {
        for helper in ["paru", "yay"] {
            if util::which(helper) {
                let (_, out) = run(&[helper, "-Qua"]);
                let mut aur = parse_updates(&out, true);
                for p in aur.iter_mut() { p.repo = "aur".into(); p.desc = format!("Built from the AUR with {helper}"); }
                c.pkgs.extend(aur);
                break;
            }
        }
    }
    c
}

/// Last time pacman upgraded something (from its log).
pub fn last_installed() -> i64 {
    let log = std::fs::read_to_string("/var/log/pacman.log").unwrap_or_default();
    log.lines().rev().find(|l| l.contains("[ALPM] upgraded ") || l.contains("[ALPM] installed ")).and_then(|l| super::eventvwr::parse_pacman(l, "")).map(|e| e.ts / 1_000_000).unwrap_or(0)
}

pub fn aur_helper() -> Option<&'static str> { ["paru", "yay"].into_iter().find(|h| util::which(h)) }

/// Packages that want a restart after an upgrade.
fn needs_restart(names: &[String]) -> bool {
    names.iter().any(|n| n.starts_with("linux") || n == "systemd" || n == "glibc" || n.starts_with("nvidia") || n == "mesa" || n.starts_with("xorg-server") || n == "dbus" || n == "amd-ucode" || n == "intel-ucode")
}

#[derive(Clone)]
enum Phase { Idle, Checking, Installing(String, f64), Done(bool, String, bool) }

struct Ui {
    cp: Rc<Cp>,
    holder: gtk::Box,
    check: RefCell<Check>,
    phase: RefCell<Phase>,
    /// names selected for installing (repo updates go together)
    selected: RefCell<Vec<String>>,
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    let holder = vbox(0);
    holder.style_context().add_class("adm");
    let c = load_check();
    let ui = Rc::new(Ui { cp: cp.clone(), holder: holder.clone(), selected: RefCell::new(c.pkgs.iter().map(|p| p.name.clone()).collect()), check: RefCell::new(c), phase: RefCell::new(Phase::Idle) });
    { let u = ui.clone(); side_action(cp, "Check for updates", move || start_check(&u)); }
    { let u = ui.clone(); side_action(cp, "Change settings", move || settings(&u)); }
    { let u = ui.clone(); side_action(cp, "View update history", move || history(&u)); }
    { let u = ui.clone(); side_action(cp, "Restore hidden updates", move || inform(win_of(&u.holder).as_ref(), "Windows Update", "There are no hidden updates.", "")); }
    side_action(cp, "Updates: frequently asked questions", || { util::open_uri("https://wiki.archlinux.org/title/System_maintenance#Upgrading_the_system"); });
    see_also(cp, &[("Installed Updates", "programs")]);
    main_view(&ui);
    // once a day, like Win7's automatic check (only reads; never installs)
    let mode = config::get().maintenance.update_mode;
    let last = ui.check.borrow().when;
    if mode != "never" && now() - last > 86400 { start_check(&ui); }
    holder.upcast()
}

fn total(pkgs: &[&Pkg]) -> u64 { pkgs.iter().map(|p| p.size).sum() }

fn main_view(ui: &Rc<Ui>) {
    for c in ui.holder.children() { ui.holder.remove(&c); }
    let v = vbox(10);
    v.pack_start(&title("Windows Update"), false, false, 0);
    v.pack_start(&panel(ui), false, false, 0);
    let c = ui.check.borrow().clone();
    let cfg = config::get().maintenance;
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(20);
    g.set_margin_top(8);
    let row = |g: &gtk::Grid, r: i32, k: &str, w: &gtk::Widget| { let l = gtk::Label::new(Some(k)); l.set_xalign(0.0); l.set_yalign(0.0); g.attach(&l, 0, r, 1, 1); g.attach(w, 1, r, 1, 1); };
    let l1 = label(&if c.when > 0 { when_words(c.when) } else { "Never".into() }, "");
    row(&g, 0, "Most recent check for updates:", l1.upcast_ref());
    let inst = vbox(0);
    let installed = if c.installed > 0 { c.installed } else { last_installed() };
    inst.pack_start(&label(&if installed > 0 { when_words(installed) } else { "Never".into() }, ""), false, false, 0);
    { let u = ui.clone(); let l = link("View update history", move || history(&u)); inst.pack_start(&l, false, false, 0); }
    row(&g, 1, "Updates were installed:", inst.upcast_ref());
    let recv = vbox(0);
    let aur = aur_helper();
    recv.pack_start(&label(&match (aur, cfg.update_no_aur) { (Some(h), false) => format!("For Arch Linux and the AUR ({h})."), _ => "For Arch Linux only.".into() }, ""), false, false, 0);
    if aur.is_some() && cfg.update_no_aur { let u = ui.clone(); recv.pack_start(&link("Get updates for AUR packages too. Find out more", move || settings(&u)), false, false, 0); }
    row(&g, 2, "You receive updates:", recv.upcast_ref());
    v.pack_start(&g, false, false, 0);
    ui.holder.pack_start(&v, true, true, 0);
    ui.holder.show_all();
}

/// The status panel with the coloured band and the shield.
fn panel(ui: &Rc<Ui>) -> gtk::Widget {
    let c = ui.check.borrow().clone();
    let phase = ui.phase.borrow().clone();
    let mode = config::get().maintenance.update_mode;
    let h = hbox(0);
    h.style_context().add_class("wu-panel");
    let band = gtk::Box::new(gtk::Orientation::Vertical, 0);
    h.pack_start(&band, false, false, 0);
    let body = hbox(14);
    body.set_border_width(14);
    h.pack_start(&body, true, true, 0);
    let (color, shield, head, sub): (&str, &[&str], String, String);
    let right = vbox(6);
    right.set_valign(gtk::Align::Center);
    let left = vbox(4);
    let selected: Vec<String> = ui.selected.borrow().clone();
    let important: Vec<&Pkg> = c.pkgs.iter().filter(|p| !p.aur).collect();
    let optional: Vec<&Pkg> = c.pkgs.iter().filter(|p| p.aur).collect();
    match phase {
        Phase::Checking => {
            color = "wu-band-grey"; shield = &["aerode-wu", "system-software-update"]; head = "Checking for updates...".into(); sub = String::new();
            let pb = gtk::ProgressBar::new();
            pb.set_size_request(320, -1);
            let p2 = pb.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(80), move || { p2.pulse(); if p2.is_visible() || p2.parent().is_some() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break } });
            left.pack_end(&pb, false, false, 4);
        }
        Phase::Installing(step, frac) => {
            color = "wu-band-yellow"; shield = &["aerode-shield-warning", "security-medium"]; head = "Installing updates...".into(); sub = step.clone();
            let pb = gtk::ProgressBar::new();
            pb.set_size_request(320, -1);
            if frac < 0.0 { let p2 = pb.clone(); glib::timeout_add_local(std::time::Duration::from_millis(80), move || { p2.pulse(); if p2.parent().is_some() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break } }); } else { pb.set_fraction(frac); }
            left.pack_end(&pb, false, false, 4);
            let stop = gtk::Button::with_label("Stop installation");
            stop.set_sensitive(false);
            right.pack_start(&stop, false, false, 0);
        }
        Phase::Done(true, _, restart) if restart => {
            color = "wu-band-yellow"; shield = &["aerode-shield-warning", "security-medium"]; head = "Restart your computer to finish installing important updates".into();
            sub = "Windows can't update important files and services while the system is using them. Make sure to save your files before restarting.".into();
            let b = gtk::Button::with_mnemonic("_Restart now");
            let w = ui.holder.clone();
            b.connect_clicked(move |_| { if confirm(win_of(&w).as_ref(), "Windows Update", "Restart the computer now?", "Save your work first. Open programs will be closed.", "Restart now") { util::spawn(&["systemctl", "reboot"]); } });
            right.pack_start(&b, false, false, 0);
        }
        Phase::Done(true, _, _) => {
            color = "wu-band-green"; shield = &["aerode-shield-ok", "security-high"]; head = "The updates were successfully installed".into(); sub = format!("Succeeded: {}", when_words(now()));
        }
        Phase::Done(false, err, _) => {
            color = "wu-band-red"; shield = &["aerode-shield-error", "dialog-error"]; head = "Some updates were not installed".into(); sub = format!("Errors found:\n{}", err.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"));
            let l = link("Get help with this error", || { util::open_uri("https://wiki.archlinux.org/title/Pacman#Troubleshooting"); });
            left.pack_end(&l, false, false, 0);
            let b = shield_button("_Try again");
            { let u = ui.clone(); b.connect_clicked(move |_| install(&u)); }
            right.pack_start(&b, false, false, 0);
        }
        Phase::Idle if mode == "never" => {
            color = "wu-band-red"; shield = &["aerode-shield-error", "dialog-error"]; head = "Windows Update is turned off".into(); sub = "To check for updates now and from now on once a day, turn on automatic checking.".into();
            let b = shield_button("_Turn on automatic updates");
            { let u = ui.clone(); b.connect_clicked(move |_| { config::update(|c| c.maintenance.update_mode = String::new()); start_check(&u); }); }
            right.pack_start(&b, false, false, 0);
        }
        Phase::Idle if !c.error.is_empty() => {
            color = "wu-band-red"; shield = &["aerode-shield-error", "dialog-error"]; head = "Windows could not search for new updates".into(); sub = format!("An error occurred while checking for new updates for your computer.\nError(s) found: {}", c.error);
            let l = link("Get help with this error", || { util::open_uri("https://wiki.archlinux.org/title/Pacman#Troubleshooting"); });
            left.pack_end(&l, false, false, 0);
            let b = gtk::Button::with_mnemonic("_Try again");
            { let u = ui.clone(); b.connect_clicked(move |_| start_check(&u)); }
            right.pack_start(&b, false, false, 0);
        }
        Phase::Idle if c.when == 0 => {
            color = "wu-band-red"; shield = &["aerode-shield-error", "dialog-error"]; head = "Check for updates for your computer".into(); sub = "Windows has not checked for updates yet.".into();
            let b = gtk::Button::with_mnemonic("_Check for updates");
            { let u = ui.clone(); b.connect_clicked(move |_| start_check(&u)); }
            right.pack_start(&b, false, false, 0);
        }
        Phase::Idle if c.pkgs.is_empty() => {
            color = "wu-band-green"; shield = &["aerode-shield-ok", "security-high"]; head = "Windows is up to date".into(); sub = "There are no updates available for your computer.".into();
        }
        Phase::Idle => {
            color = "wu-band-yellow"; shield = &["aerode-shield-warning", "security-medium"]; head = "Download and install updates for your computer".into(); sub = String::new();
            let links = vbox(2);
            let n = |k: usize, w: &str| format!("{k} {w} update{} {} available", if k == 1 { "" } else { "s" }, if k == 1 { "is" } else { "are" });
            if !important.is_empty() { let u = ui.clone(); links.pack_start(&link(&n(important.len(), "important"), move || select(&u)), false, false, 0); }
            if !optional.is_empty() { let u = ui.clone(); links.pack_start(&link(&n(optional.len(), "optional"), move || select(&u)), false, false, 0); }
            left.pack_end(&links, false, false, 0);
            let sel: Vec<&Pkg> = c.pkgs.iter().filter(|p| selected.contains(&p.name)).collect();
            let imp = sel.iter().filter(|p| !p.aur).count();
            let opt = sel.len() - imp;
            let mut t = String::new();
            if imp > 0 { t += &format!("{imp} important update{} selected", if imp == 1 { " is" } else { "s" }); }
            if opt > 0 { if !t.is_empty() { t += ", "; } t += &format!("{opt} optional update{} selected", if opt == 1 { "" } else { "s" }); }
            if t.is_empty() { t = "No updates are selected".into(); }
            let sz = total(&sel);
            right.pack_start(&label(&format!("{t}{}", if sz > 0 { format!(", {}", gb(sz)) } else { String::new() }), ""), false, false, 0);
            let b = shield_button("_Install updates");
            b.set_sensitive(!sel.is_empty());
            b.set_halign(gtk::Align::End);
            { let u = ui.clone(); b.connect_clicked(move |_| install(&u)); }
            right.pack_start(&b, false, false, 0);
        }
    }
    band.style_context().add_class(color);
    body.pack_start(&img(shield, 48), false, false, 0);
    let t = label(&head, "wu-title");
    left.pack_start(&t, false, false, 0);
    if !sub.is_empty() { let s = label(&sub, ""); s.set_max_width_chars(60); left.pack_start(&s, false, false, 0); }
    // keep the order: title, text, then links/progress
    body.pack_start(&left, true, true, 0);
    body.pack_end(&right, false, false, 0);
    h.upcast()
}

fn start_check(ui: &Rc<Ui>) {
    if matches!(*ui.phase.borrow(), Phase::Checking | Phase::Installing(..)) { return; }
    *ui.phase.borrow_mut() = Phase::Checking;
    main_view(ui);
    let aur = !config::get().maintenance.update_no_aur;
    let u = ui.clone();
    bg(move || check(aur), move |c| {
        save_check(&c);
        *u.selected.borrow_mut() = c.pkgs.iter().map(|p| p.name.clone()).collect();
        *u.check.borrow_mut() = c;
        *u.phase.borrow_mut() = Phase::Idle;
        if u.holder.parent().is_some() { main_view(&u); }
    });
}

/// "Select the updates you want to install".
fn select(ui: &Rc<Ui>) {
    let c = ui.check.borrow().clone();
    let v = vbox(8);
    v.pack_start(&title("Select the updates you want to install"), false, false, 0);
    let body = hbox(10);
    // left: Important / Optional
    let tabs = vbox(4);
    tabs.set_size_request(130, -1);
    let imp_n = c.pkgs.iter().filter(|p| !p.aur).count();
    let opt_n = c.pkgs.len() - imp_n;
    let show_aur = Rc::new(std::cell::Cell::new(imp_n == 0));
    let ti = gtk::RadioButton::with_label(&format!("Important ({imp_n})"));
    let to = gtk::RadioButton::with_label_from_widget(&ti, &format!("Optional ({opt_n})"));
    ti.set_mode(false);
    to.set_mode(false);
    if imp_n == 0 { to.set_active(true); }
    tabs.pack_start(&ti, false, false, 0);
    tabs.pack_start(&to, false, false, 0);
    body.pack_start(&tabs, false, false, 0);
    // middle: list with checkboxes
    let store = gtk::ListStore::new(&[glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL]);
    let tv = gtk::TreeView::with_model(&store);
    let tog = gtk::CellRendererToggle::new();
    let col = gtk::TreeViewColumn::new();
    TreeViewColumnExt::pack_start(&col, &tog, false);
    TreeViewColumnExt::add_attribute(&col, &tog, "active", 0);
    tv.append_column(&col);
    columns(&tv, &[("Name", 1, 0), ("Size", 2, 80)]);
    let detail = vbox(6);
    detail.set_size_request(230, -1);
    let fill: Rc<dyn Fn()> = { let (store, ui, show_aur) = (store.clone(), ui.clone(), show_aur.clone()); Rc::new(move || {
        store.clear();
        let sel = ui.selected.borrow();
        let c = ui.check.borrow();
        let aur = show_aur.get();
        let list: Vec<&Pkg> = c.pkgs.iter().filter(|p| p.aur == aur).collect();
        if !aur && !list.is_empty() { store.insert_with_values(None, &[(0, &list.iter().all(|p| sel.contains(&p.name))), (1, &format!("Arch Linux ({})", list.len())), (2, &""), (3, &""), (4, &true)]); }
        for p in list {
            let name = format!("{} {}{}", p.name, p.new, if p.old.is_empty() { String::new() } else { format!(" (from {})", p.old) });
            store.insert_with_values(None, &[(0, &sel.contains(&p.name)), (1, &name), (2, &if p.size > 0 { gb(p.size) } else { String::new() }), (3, &p.name), (4, &false)]);
        }
    }) };
    fill();
    { let (f, s) = (fill.clone(), show_aur.clone()); ti.connect_toggled(move |b| { if b.is_active() { s.set(false); f(); } }); }
    { let (f, s) = (fill.clone(), show_aur.clone()); to.connect_toggled(move |b| { if b.is_active() { s.set(true); f(); } }); }
    let note = label("", "dim");
    { let (store, ui, fill, note) = (store.clone(), ui.clone(), fill.clone(), note.clone()); tog.connect_toggled(move |_, path| {
        let Some(it) = store.iter(&path) else { return };
        let on: bool = !store.value(&it, 0).get::<bool>().unwrap_or(false);
        let header: bool = store.value(&it, 4).get().unwrap_or(false);
        let name = sval(&store, &it, 3);
        let is_repo = header || ui.check.borrow().pkgs.iter().any(|p| p.name == name && !p.aur);
        {
            let mut sel = ui.selected.borrow_mut();
            let c = ui.check.borrow();
            if is_repo {
                // pacman upgrades the repositories as a whole (partial upgrades are unsupported)
                sel.retain(|n| c.pkgs.iter().any(|p| &p.name == n && p.aur));
                if on { sel.extend(c.pkgs.iter().filter(|p| !p.aur).map(|p| p.name.clone())); }
                note.set_text("Arch Linux installs the repository updates together – partial upgrades are not supported.");
            } else if on { sel.push(name.clone()); } else { sel.retain(|n| *n != name); }
        }
        fill();
    }); }
    { let (d, ui) = (detail.clone(), ui.clone()); tv.selection().connect_changed(move |s| {
        for c in d.children() { d.remove(&c); }
        let Some((m, it)) = s.selected() else { return };
        let name = sval(&m, &it, 3);
        let c = ui.check.borrow();
        if let Some(p) = c.pkgs.iter().find(|p| p.name == name) {
            d.pack_start(&bold(&format!("{} {}", p.name, p.new)), false, false, 0);
            if !p.desc.is_empty() { d.pack_start(&label(&p.desc, ""), false, false, 0); }
            d.pack_start(&label(&format!("Repository: {}", if p.repo.is_empty() { "unknown" } else { &p.repo }), ""), false, false, 0);
            if !p.old.is_empty() { d.pack_start(&label(&format!("Installed version: {}", p.old), ""), false, false, 0); }
            d.pack_start(&label(if p.aur { "Update is built and installed by the AUR helper" } else { "Update is ready for downloading" }, "dim"), false, false, 0);
            let n = p.name.clone();
            d.pack_start(&link("More information", move || { util::open_uri(&format!("https://archlinux.org/packages/?q={n}")); }), false, false, 0);
        }
        d.show_all();
    }); }
    let sw = scrolled(&tv);
    sw.set_min_content_height(300);
    body.pack_start(&sw, true, true, 0);
    body.pack_start(&detail, false, false, 0);
    v.pack_start(&body, true, true, 0);
    v.pack_start(&note, false, false, 0);
    let bb = hbox(8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    ok.set_size_request(80, -1);
    cancel.set_size_request(80, -1);
    bb.pack_end(&cancel, false, false, 0);
    bb.pack_end(&ok, false, false, 0);
    let saved = ui.selected.borrow().clone();
    { let u = ui.clone(); ok.connect_clicked(move |_| main_view(&u)); }
    { let u = ui.clone(); cancel.connect_clicked(move |_| { *u.selected.borrow_mut() = saved.clone(); main_view(&u); }); }
    v.pack_start(&bb, false, false, 0);
    subpage(&ui.cp, &ui.holder, "update", "Select Updates to Install", &v);
}

/// Install: repository updates with pkexec pacman -Syu (progress in the panel), AUR updates
/// afterwards in a terminal (the helper asks questions while it builds).
fn install(ui: &Rc<Ui>) {
    let c = ui.check.borrow().clone();
    let sel = ui.selected.borrow().clone();
    let repo: Vec<String> = c.pkgs.iter().filter(|p| !p.aur && sel.contains(&p.name)).map(|p| p.name.clone()).collect();
    let aur: Vec<String> = c.pkgs.iter().filter(|p| p.aur && sel.contains(&p.name)).map(|p| p.name.clone()).collect();
    let run_aur = move || if !aur.is_empty() { if let Some(h) = aur_helper() { terminal(&format!("{h} -S --needed {}", aur.join(" "))); } };
    if repo.is_empty() { run_aur(); return; }
    if !util::which("pkexec") { terminal("sudo pacman -Syu"); run_aur(); return; }
    *ui.phase.borrow_mut() = Phase::Installing("Preparing to install updates...".into(), -1.0);
    main_view(ui);
    let total_n = repo.len();
    let u = ui.clone();
    let log = Rc::new(RefCell::new(String::new()));
    let restart = needs_restart(&repo);
    #[derive(Debug)]
    enum Msg { Line(String), End(bool) }
    let log2 = log.clone();
    let run_aur = Rc::new(run_aur);
    stream(move |tx: async_channel::Sender<Msg>| {
        use std::io::BufRead;
        let child = std::process::Command::new("pkexec").args(["env", "LANG=C", "pacman", "-Syu", "--noconfirm", "--noprogressbar"])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn();
        let Ok(mut child) = child else { let _ = tx.send_blocking(Msg::End(false)); return };
        let err = child.stderr.take();
        let tx2 = tx.clone();
        let t = std::thread::spawn(move || { if let Some(e) = err { for l in std::io::BufReader::new(e).lines().map_while(Result::ok) { let _ = tx2.send_blocking(Msg::Line(l)); } } });
        if let Some(o) = child.stdout.take() { for l in std::io::BufReader::new(o).lines().map_while(Result::ok) { let _ = tx.send_blocking(Msg::Line(l)); } }
        let _ = t.join();
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = tx.send_blocking(Msg::End(ok));
    }, move |m| match m {
        Msg::Line(l) => {
            log2.borrow_mut().push_str(&l);
            log2.borrow_mut().push('\n');
            let t = l.trim();
            // "(3/25) upgrading linux"
            let step = if let Some(rest) = t.strip_prefix('(') {
                rest.split_once(')').and_then(|(nn, what)| { let (a, b) = nn.trim().split_once('/')?; let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?); Some((a / b, what.trim().to_string(), a as usize, b as usize)) })
            } else { None };
            let phase = match step {
                Some((f, what, a, b)) if what.starts_with("upgrading") || what.starts_with("installing") || what.starts_with("reinstalling") || what.starts_with("removing") =>
                    Phase::Installing(format!("Installing update {a} of {b} ({})...", what.split_whitespace().nth(1).unwrap_or("")), 0.3 + 0.7 * f),
                Some((f, what, ..)) if what.starts_with("checking") => Phase::Installing(format!("Checking the updates ({})...", what.trim_start_matches("checking ")), 0.25 * f),
                _ if t.starts_with(":: Synchronizing") => Phase::Installing("Looking for the latest versions...".into(), -1.0),
                _ if t.starts_with(":: Retrieving packages") || t.contains("downloading") => Phase::Installing(format!("Downloading {total_n} updates..."), -1.0),
                _ if t.starts_with(":: Running post-transaction hooks") => Phase::Installing("Finishing installation (post-transaction hooks)...".into(), 0.98),
                _ => return,
            };
            if matches!(*u.phase.borrow(), Phase::Installing(..)) { *u.phase.borrow_mut() = phase; if u.holder.parent().is_some() { main_view(&u); } }
        }
        Msg::End(ok) => {
            let out = log.borrow().clone();
            let cancelled = !ok && (out.contains("Not authorized") || out.contains("dismissed") || out.trim().is_empty());
            *u.phase.borrow_mut() = if cancelled { Phase::Idle } else { Phase::Done(ok, out.lines().filter(|l| l.starts_with("error") || l.contains("failed")).collect::<Vec<_>>().join("\n") + if ok { "" } else { "\n(see /var/log/pacman.log)" }, restart) };
            if ok {
                let mut c = u.check.borrow().clone();
                c.pkgs.retain(|p| p.aur);
                c.installed = now();
                save_check(&c);
                *u.check.borrow_mut() = c;
                run_aur();
            }
            if u.holder.parent().is_some() { main_view(&u); }
        }
    });
}

/// "Review your update history" from pacman.log.
fn history(ui: &Rc<Ui>) {
    let v = vbox(8);
    v.pack_start(&title("Review your update history"), false, false, 0);
    v.pack_start(&text("Check the Status column to ensure all important updates were successful. To remove an update, see Installed Updates."), false, false, 0);
    { let cp = ui.cp.clone(); v.pack_start(&link("Installed Updates", move || cp.open("programs")), false, false, 0); }
    v.pack_start(&link("Troubleshoot problems with installing updates", || { util::open_uri("https://wiki.archlinux.org/title/Pacman#Troubleshooting"); }), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::I64]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Name", 0, 380), ("Status", 1, 90), ("Importance", 2, 90), ("Date Installed", 3, 0)]);
    tv.columns()[3].set_sort_column_id(4);
    let sw = scrolled(&tv);
    sw.set_min_content_height(360);
    v.pack_start(&sw, true, true, 0);
    let status = label("Loading…", "dim");
    v.pack_start(&status, false, false, 0);
    let ok = gtk::Button::with_label("OK");
    ok.set_halign(gtk::Align::End);
    ok.set_size_request(80, -1);
    { let u = ui.clone(); ok.connect_clicked(move |_| main_view(&u)); }
    v.pack_start(&ok, false, false, 0);
    subpage(&ui.cp, &ui.holder, "update", "View update history", &v);
    bg(|| {
        let log = std::fs::read_to_string("/var/log/pacman.log").unwrap_or_default();
        let mut rows = vec![];
        for l in log.lines().rev() {
            let Some(e) = super::eventvwr::parse_pacman(l, "") else { continue };
            if e.source != "ALPM" { continue; }
            let m = &e.message;
            let (verb, rest) = match m.split_once(' ') { Some(x) => x, None => continue };
            let (name, vers) = match rest.split_once(" (") { Some((n, v)) => (n.to_string(), v.trim_end_matches(')').to_string()), None => continue };
            let (status, title) = match verb {
                "upgraded" => ("Successful", { let (a, b) = vers.split_once(" -> ").unwrap_or(("", &vers)); format!("{name} {b} (updated from {a})") }),
                "downgraded" => ("Successful", format!("{name} {} (downgraded)", vers.split(" -> ").last().unwrap_or(""))),
                "installed" => ("Successful", format!("{name} {vers}")),
                "reinstalled" => ("Successful", format!("{name} {vers} (reinstalled)")),
                "removed" => ("Removed", format!("{name} {vers}")),
                _ => continue,
            };
            let imp = if name.starts_with("linux") || name == "systemd" || name == "glibc" || name == "openssl" { "Important" } else if verb == "installed" { "Optional" } else { "Recommended" };
            rows.push((title, status.to_string(), imp.to_string(), day(e.ts / 1_000_000), e.ts));
            if rows.len() >= 3000 { break; }
        }
        rows
    }, move |rows| {
        for (a, b, c, d, t) in &rows { store.insert_with_values(None, &[(0, a), (1, b), (2, c), (3, d), (4, t)]); }
        status.set_text(&if rows.is_empty() { "No updates have been installed (or /var/log/pacman.log could not be read).".to_string() } else { format!("{} entries", rows.len()) });
    });
}

/// "Choose how Windows can install updates".
fn settings(ui: &Rc<Ui>) {
    let cfg = config::get().maintenance;
    let v = vbox(10);
    v.pack_start(&title("Choose how Windows can install updates"), false, false, 0);
    v.pack_start(&text("When your computer is online, Windows can automatically check for important updates using these settings. When new updates are available, you can install them from Windows Update. Updates are never installed without asking you."), false, false, 0);
    v.pack_start(&link("How does automatic updating help me?", || { util::open_uri("https://wiki.archlinux.org/title/System_maintenance"); }), false, false, 0);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(14);
    let mut r = 0;
    let head = |g: &gtk::Grid, r: i32, t: &str| { let l = section(t); l.set_margin_top(6); g.attach(&l, 0, r, 2, 1); };
    head(&grid, r, "Important updates"); r += 1;
    let mode = gtk::ComboBoxText::new();
    mode.append(Some("check"), "Check for updates but let me choose whether to download and install them");
    mode.append(Some("never"), "Never check for updates (not recommended)");
    mode.set_active_id(Some(if cfg.update_mode == "never" { "never" } else { "check" }));
    grid.attach(&img(&["aerode-shield-warning", "security-medium"], 32), 0, r, 1, 1);
    grid.attach(&mode, 1, r, 1, 1); r += 1;
    let when = hbox(6);
    when.pack_start(&gtk::Label::new(Some("Install new updates:")), false, false, 0);
    let every = gtk::ComboBoxText::new(); every.append_text("Every day"); every.set_active(Some(0)); every.set_sensitive(false);
    let at = gtk::ComboBoxText::new(); at.append_text("3:00 AM"); at.set_active(Some(0)); at.set_sensitive(false);
    when.pack_start(&every, false, false, 0);
    when.pack_start(&gtk::Label::new(Some("at")), false, false, 0);
    when.pack_start(&at, false, false, 0);
    when.set_sensitive(false);
    grid.attach(&when, 1, r, 1, 1); r += 1;
    head(&grid, r, "Recommended updates"); r += 1;
    let rec = gtk::CheckButton::with_label("Give me recommended updates the same way I receive important updates");
    rec.set_active(true); rec.set_sensitive(false);
    grid.attach(&rec, 1, r, 1, 1); r += 1;
    head(&grid, r, "Who can install updates"); r += 1;
    let who = gtk::CheckButton::with_label("Allow all users to install updates on this computer (administrators confirm with their password)");
    who.set_active(true); who.set_sensitive(false);
    grid.attach(&who, 1, r, 1, 1); r += 1;
    head(&grid, r, "Arch User Repository"); r += 1;
    let helper = aur_helper();
    let aur = gtk::CheckButton::with_label(&format!("Give me updates for AUR packages when I update Windows{}", helper.map(|h| format!(" (using {h})")).unwrap_or_else(|| " (install yay or paru first)".into())));
    aur.set_active(helper.is_some() && !cfg.update_no_aur);
    aur.set_sensitive(helper.is_some());
    grid.attach(&aur, 1, r, 1, 1); r += 1;
    head(&grid, r, "Software notifications"); r += 1;
    let notif = gtk::CheckButton::with_label("Show me detailed notifications when new software is available");
    notif.set_sensitive(false);
    grid.attach(&notif, 1, r, 1, 1);
    v.pack_start(&grid, false, false, 0);
    let bb = hbox(8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    ok.set_size_request(80, -1);
    cancel.set_size_request(80, -1);
    bb.pack_end(&cancel, false, false, 0);
    bb.pack_end(&ok, false, false, 0);
    { let (u, mode, aur) = (ui.clone(), mode.clone(), aur.clone()); ok.connect_clicked(move |_| {
        let m = mode.active_id().map(|s| s.to_string()).unwrap_or_default();
        let no_aur = !aur.is_active();
        config::update(|c| { c.maintenance.update_mode = if m == "never" { "never".into() } else { String::new() }; c.maintenance.update_no_aur = no_aur; });
        main_view(&u);
    }); }
    { let u = ui.clone(); cancel.connect_clicked(move |_| main_view(&u)); }
    v.pack_start(&bb, false, false, 8);
    subpage(&ui.cp, &ui.holder, "update", "Change settings", &v);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse() {
        let p = parse_updates("linux 6.11.1.arch1-1 -> 6.11.2.arch1-1\nfirefox 131.0-1 -> 131.0.2-1\n", false);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "linux");
        assert_eq!(p[1].new, "131.0.2-1");
        assert_eq!(size_of("12.50 MiB"), 13107200);
        assert!(needs_restart(&["linux".into()]));
        assert!(!needs_restart(&["firefox".into()]));
    }
}
