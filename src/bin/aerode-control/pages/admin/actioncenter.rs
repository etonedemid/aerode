//! Action Center: "Review recent messages and resolve problems" with the Security and
//! Maintenance sections – red (important) and yellow message bars with their action button –
//! and the status rows when a section is expanded. Checks: firewall, pending/old updates,
//! antivirus (only when ClamAV is installed), polkit agent (UAC), low disk space, failed systemd
//! units, backup, programs that crashed (coredumps).
use gtk::prelude::*;
use gtk::glib;
use std::rc::Rc;

use super::super::super::{link, text, title, Cp};
use super::*;
use aerode::config;

#[derive(Clone, Default, Debug)]
pub struct Status {
    pub fw: super::firewall::Fw,
    pub fw_installed: bool,
    pub updates: super::update::Check,
    pub update_mode: String,
    pub clamav: Option<bool>,
    pub agent: bool,
    /// (mount, free, total)
    pub low_disks: Vec<(String, u64, u64)>,
    pub disks: Vec<(String, u64, u64)>,
    pub failed: Vec<String>,
    pub backup: Option<String>,
    pub last_backup: i64,
    pub backup_scheduled: bool,
    /// (time, program)
    pub crashes: Vec<(String, String)>,
}

/// Everything Action Center looks at (worker thread, a fraction of a second).
pub fn gather() -> Status {
    let mut s = Status { fw: super::firewall::state(), ..Default::default() };
    s.fw_installed = util::which("ufw") || util::which("firewall-cmd");
    s.updates = super::update::load_check();
    if util::which("clamscan") || util::which("clamd") { s.clamav = Some(sh(&["systemctl", "is-active", "clamav-daemon"]).trim() == "active" || sh(&["systemctl", "is-active", "clamd"]).trim() == "active"); }
    // a polkit authentication agent in this session (ours or another desktop's)
    s.agent = std::fs::read_dir("/proc").map(|d| d.flatten().any(|e| {
        let c = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
        let c = c.trim();
        c.starts_with("aerode-polkit") || c.contains("polkit-gnome") || c.contains("polkit-kde") || c == "lxpolkit" || c.contains("polkit-mate") || c == "xfce-polkit" || c.contains("polkit-agent")
    })).unwrap_or(false);
    // fixed file systems
    let mut seen = vec![];
    for l in std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || !f[0].starts_with("/dev/") || f[1].starts_with("/run/media") || f[1].starts_with("/media") || f[1].starts_with("/snap") || matches!(f[2], "squashfs" | "iso9660" | "udf") || seen.contains(&f[0]) { continue; }
        seen.push(f[0]);
        if let Some((free, total)) = fs_space(f[1]) {
            if total < 512 * 1024 * 1024 { continue; }
            s.disks.push((f[1].to_string(), free, total));
            if free < 2 * 1024 * 1024 * 1024 || (free as f64) < total as f64 * 0.05 { s.low_disks.push((f[1].to_string(), free, total)); }
        }
    }
    for user in [false, true] {
        let mut a = vec!["systemctl"];
        if user { a.push("--user"); }
        a.extend(["--failed", "--no-legend", "--plain", "--no-pager"]);
        for l in sh(&a).lines() { if let Some(u) = l.split_whitespace().next() { s.failed.push(u.trim_start_matches('●').trim().to_string()); } }
    }
    s.failed.retain(|u| !u.is_empty());
    // backup: Timeshift, Snapper or our own home-folder backup
    let m = config::get().maintenance;
    if let Some(b) = &m.backup { s.backup = Some(format!("Windows Backup to {}", b.dest)); s.last_backup = super::backup::last_backup(b).unwrap_or(0); s.backup_scheduled = !b.schedule.is_empty(); }
    else if std::path::Path::new("/etc/timeshift/timeshift.json").exists() && std::fs::read_to_string("/etc/timeshift/timeshift.json").map(|t| t.contains("\"backup_device_uuid\" : \"") && !t.contains("\"backup_device_uuid\" : \"\"")).unwrap_or(false) { s.backup = Some("Timeshift".into()); s.backup_scheduled = true; }
    else if std::fs::read_dir("/etc/snapper/configs").map(|d| d.count() > 0).unwrap_or(false) { s.backup = Some("Snapper".into()); s.backup_scheduled = true; }
    s.update_mode = m.update_mode;
    // programs that stopped working in the last week
    for l in sh(&["coredumpctl", "list", "--since=-7d", "--no-legend", "--no-pager", "-q"]).lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if let Some(exe) = f.iter().rev().find(|x| x.starts_with('/')) { s.crashes.push((f.iter().take(3).copied().collect::<Vec<_>>().join(" "), exe.to_string())); }
    }
    s
}

/// A message: kind key (for "Turn off messages about …"), important?, title, text, button.
pub struct Msg { pub key: &'static str, pub topic: &'static str, pub red: bool, pub title: String, pub text: String, pub button: Option<(String, bool, Rc<dyn Fn()>)>, pub extra: Option<(String, Rc<dyn Fn()>)> }

/// Messages for the current status (also used for the flag in the notification area).
pub fn messages(cp: &Rc<Cp>, s: &Status, refresh: Rc<dyn Fn()>) -> (Vec<Msg>, Vec<Msg>) {
    let off = config::get().maintenance.ac_off;
    let mut sec = vec![];
    let mut mnt = vec![];
    let r = refresh.clone();
    if !off.contains(&"firewall".to_string()) && !s.fw.on && s.fw.vendor.is_empty() {
        let be = s.fw.backend;
        let r2 = r.clone();
        sec.push(Msg { key: "firewall", topic: "network firewall", red: true, title: "Network firewall  (Important)".into(),
            text: if !s.fw_installed { "No firewall program is installed on this computer.".into() } else { format!("Windows Firewall ({be}) is turned off or set up incorrectly.") },
            button: Some((if s.fw_installed { "Turn on now".into() } else { "Install a firewall".into() }, true, Rc::new(move || {
                let script = match be { "firewalld" => "systemctl enable --now firewalld", "ufw" => "systemctl enable --now ufw && ufw --force enable", _ => "pacman -S --needed --noconfirm ufw && ufw default deny incoming && systemctl enable --now ufw && ufw --force enable" };
                let r3 = r2.clone();
                elevate_sh(script, move |ok, out| { report("The firewall could not be turned on.", ok, &out); r3(); });
            }))),
            extra: Some(("Get a different firewall program online".into(), Rc::new(|| { util::open_uri("https://wiki.archlinux.org/title/Firewall"); }))) });
    }
    if !off.contains(&"updates".to_string()) {
        let imp = s.updates.pkgs.iter().filter(|p| !p.aur).count();
        let cp1 = cp.clone();
        if s.update_mode == "never" {
            sec.push(Msg { key: "updates", topic: "Windows Update", red: true, title: "Windows Update  (Important)".into(), text: "Windows Update is turned off.".into(),
                button: Some(("Change settings...".into(), false, Rc::new(move || cp1.open("update")))), extra: None });
        } else if imp > 0 {
            sec.push(Msg { key: "updates", topic: "Windows Update", red: false, title: "Windows Update  (Important)".into(), text: format!("{imp} important update{} available for your computer.", if imp == 1 { " is" } else { "s are" }),
                button: Some(("View updates".into(), false, Rc::new(move || cp1.open("update")))), extra: None });
        } else if s.updates.when == 0 || now() - s.updates.when > 7 * 86400 {
            let days = if s.updates.when == 0 { String::new() } else { format!(" in {} days", (now() - s.updates.when) / 86400) };
            mnt.push(Msg { key: "check", topic: "Windows Update", red: false, title: "Check for updates".into(), text: format!("Windows has not checked for updates{days}."),
                button: Some(("Check for updates".into(), false, Rc::new(move || cp1.open("update")))), extra: None });
        }
    }
    if !off.contains(&"virus".to_string()) && s.clamav == Some(false) {
        let r2 = r.clone();
        sec.push(Msg { key: "virus", topic: "virus protection", red: true, title: "Virus protection  (Important)".into(), text: "ClamAV reports that it is turned off.".into(),
            button: Some(("Turn on now".into(), true, Rc::new(move || { let r3 = r2.clone(); elevate_sh("systemctl enable --now clamav-daemon", move |ok, out| { report("ClamAV could not be turned on.", ok, &out); r3(); }); }))), extra: None });
    }
    if !off.contains(&"uac".to_string()) && !s.agent {
        let r2 = r.clone();
        sec.push(Msg { key: "uac", topic: "User Account Control", red: false, title: "User Account Control".into(), text: "No authentication agent is running, so programs cannot ask you for administrator permission.".into(),
            button: Some(("Turn on".into(), false, Rc::new(move || { util::spawn(&["aerode-polkit"]); let r3 = r2.clone(); glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || r3()); }))), extra: None });
    }
    if !off.contains(&"disk".to_string()) && !s.low_disks.is_empty() {
        let list = s.low_disks.iter().map(|(m, f, t)| format!("{m} has {} free of {}", gb(*f), gb(*t))).collect::<Vec<_>>().join("; ");
        let very = s.low_disks.iter().any(|(_, f, t)| (*f as f64) < *t as f64 * 0.02 || *f < 512 * 1024 * 1024);
        mnt.push(Msg { key: "disk", topic: "disk space", red: very, title: format!("Disk space{}", if very { "  (Important)" } else { "" }), text: format!("Your computer is low on disk space: {list}."),
            button: Some(("Free up disk space".into(), false, Rc::new(|| spawn_tool("cleanmgr")))), extra: None });
    }
    if !off.contains(&"services".to_string()) && !s.failed.is_empty() {
        let failed = s.failed.clone();
        let n = failed.len();
        mnt.push(Msg { key: "services", topic: "system services", red: true, title: "System services  (Important)".into(),
            text: format!("{n} service{} failed to start: {}.", if n == 1 { "" } else { "s" }, failed.iter().take(4).cloned().collect::<Vec<_>>().join(", ") + if n > 4 { ", …" } else { "" }),
            button: Some(("View details".into(), false, Rc::new(move || failed_details(&failed)))), extra: None });
    }
    if !off.contains(&"backup".to_string()) {
        let cp1 = cp.clone();
        if s.backup.is_none() {
            mnt.push(Msg { key: "backup", topic: "Windows Backup", red: false, title: "Set up backup".into(), text: "Your files are not being backed up.".into(), button: Some(("Set up backup".into(), false, Rc::new(move || cp1.open("backup")))), extra: None });
        } else if s.backup_scheduled && s.last_backup > 0 && now() - s.last_backup > 14 * 86400 {
            mnt.push(Msg { key: "backup", topic: "Windows Backup", red: false, title: "Back up your files".into(), text: format!("The last successful backup was {} days ago.", (now() - s.last_backup) / 86400), button: Some(("Back up now".into(), false, Rc::new(move || cp1.open("backup")))), extra: None });
        }
    }
    if !off.contains(&"crashes".to_string()) && !s.crashes.is_empty() {
        let cp1 = cp.clone();
        let progs: Vec<String> = { let mut v: Vec<String> = s.crashes.iter().map(|(_, e)| std::path::Path::new(e).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()).collect(); v.sort(); v.dedup(); v };
        mnt.push(Msg { key: "crashes", topic: "problem reports", red: false, title: "Check for solutions to problem reports".into(),
            text: format!("{} stopped working in the last 7 days.", if progs.len() == 1 { progs[0].clone() } else { format!("{} programs ({})", progs.len(), progs.iter().take(3).cloned().collect::<Vec<_>>().join(", ")) }),
            button: Some(("View problem reports".into(), false, Rc::new(move || { let _ = &cp1; archived(&cp1); }))), extra: None });
    }
    (sec, mnt)
}

fn failed_details(units: &[String]) {
    let d = gtk::Dialog::with_buttons(Some("System services"), None::<&gtk::Window>, gtk::DialogFlags::empty(), &[("Close", gtk::ResponseType::Close)]);
    d.set_default_size(640, 420);
    let tv = text_box("Loading…");
    tv.set_monospace(true);
    d.content_area().pack_start(&scrolled(&tv), true, true, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
    let units = units.to_vec();
    bg(move || {
        let mut out = String::new();
        for u in &units {
            let user = sh(&["systemctl", "--user", "is-failed", u]).trim() == "failed";
            let mut a = vec!["systemctl"]; if user { a.push("--user"); } a.extend(["status", "--no-pager", "-n", "12", u.as_str()]);
            let (_, s) = run(&a);
            out += &s;
            out += "\n";
        }
        out
    }, move |t| { if let Some(b) = tv.buffer() { b.set_text(&t); } });
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    let holder = vbox(0);
    holder.style_context().add_class("adm");
    { let cp2 = cp.clone(); let h = holder.clone(); side_action(cp, "Change Action Center settings", move || settings(&cp2, &h)); }
    { let cp2 = cp.clone(); side_action(cp, "View archived messages", move || archived(&cp2)); }
    { let cp2 = cp.clone(); side_action(cp, "View performance information", move || cp2.open("perfinfo")); }
    see_also(cp, &[("Backup and Restore", "backup"), ("Windows Update", "update"), ("Windows Program Compatibility Troubleshooter", "troubleshooting")]);
    let v = vbox(6);
    v.pack_start(&title("Review recent messages and resolve problems"), false, false, 0);
    let intro = text("Checking your computer…");
    v.pack_start(&intro, false, false, 0);
    let content = vbox(10);
    v.pack_start(&content, false, false, 6);
    // "If you don't see your problem listed, try one of these:"
    v.pack_start(&text("If you don't see your problem listed, try one of these:"), false, false, 8);
    let tiles = hbox(40);
    for (icon, t, sub, page) in [(&["aerode-troubleshoot", "help-browser"][..], "Troubleshooting", "Find and fix problems", "troubleshooting"), (&["aerode-recovery", "drive-harddisk-system"][..], "Recovery", "Restore your computer to an earlier time", "recovery")] {
        let h = hbox(10);
        h.pack_start(&img(icon, 32), false, false, 0);
        let c = vbox(0);
        let cp2 = cp.clone();
        c.pack_start(&link(t, move || cp2.open(page)), false, false, 0);
        let l = label(sub, ""); l.set_max_width_chars(26);
        c.pack_start(&l, false, false, 0);
        h.pack_start(&c, false, false, 0);
        tiles.pack_start(&h, false, false, 0);
    }
    tiles.set_margin_start(12);
    v.pack_start(&tiles, false, false, 0);
    holder.pack_start(&v, true, true, 0);
    let refresh: Rc<RefreshCell> = Rc::new(std::cell::RefCell::new(None));
    let r: Rc<dyn Fn()> = { let rc = refresh.clone(); Rc::new(move || { if let Some(f) = rc.borrow().clone() { f(); } }) };
    let f: Rc<dyn Fn()> = { let (cp, content, intro, r) = (cp.clone(), content.clone(), intro.clone(), r.clone()); Rc::new(move || {
        let (cp, content, intro, r) = (cp.clone(), content.clone(), intro.clone(), r.clone());
        bg(gather, move |s| render(&cp, &content, &intro, &s, r.clone()));
    }) };
    *refresh.borrow_mut() = Some(f.clone());
    f();
    holder.upcast()
}
type RefreshCell = std::cell::RefCell<Option<Rc<dyn Fn()>>>;

fn render(cp: &Rc<Cp>, content: &gtk::Box, intro: &gtk::Label, s: &Status, refresh: Rc<dyn Fn()>) {
    for c in content.children() { content.remove(&c); }
    let (sec, mnt) = messages(cp, s, refresh.clone());
    intro.set_text(if sec.is_empty() && mnt.is_empty() { "No issues have been detected by Action Center." } else { "Action Center has detected one or more issues for you to review." });
    content.pack_start(&section_box("Security", sec, status_rows_security(s), refresh.clone()), false, false, 0);
    content.pack_start(&section_box("Maintenance", mnt, status_rows_maintenance(cp, s), refresh), false, false, 0);
    content.show_all();
}

/// "Security ⌄": header line with chevron, the messages, and (expanded) the status rows.
fn section_box(name: &str, msgs: Vec<Msg>, rows: Vec<(String, String, String)>, refresh: Rc<dyn Fn()>) -> gtk::Widget {
    let v = vbox(6);
    let head = hbox(8);
    let t = label(name, "ac-sec");
    head.pack_start(&t, false, false, 0);
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.style_context().add_class("ac-rule");
    rule.set_valign(gtk::Align::Center);
    head.pack_start(&rule, true, true, 0);
    let chev = chevron(false);
    head.pack_start(&chev, false, false, 0);
    v.pack_start(&head, false, false, 0);
    for m in msgs { v.pack_start(&message(m, refresh.clone()), false, false, 0); }
    let details = gtk::Grid::new();
    details.set_row_spacing(10);
    details.set_column_spacing(30);
    details.set_margin_start(14);
    details.set_margin_top(4);
    for (i, (k, val, sub)) in rows.iter().enumerate() {
        let a = label(k, "ac-status-k");
        a.set_yalign(0.0);
        details.attach(&a, 0, i as i32, 1, 1);
        let b = vbox(1);
        let vl = label(val, "");
        b.pack_start(&vl, false, false, 0);
        if !sub.is_empty() { let s = label(sub, "ac-status-v"); s.set_max_width_chars(60); b.pack_start(&s, false, false, 0); }
        details.attach(&b, 1, i as i32, 1, 1);
    }
    v.pack_start(&details, false, false, 0);
    details.set_no_show_all(true);
    let key = format!("ac-open-{name}");
    let open = std::fs::read_to_string(cache_dir().join("actioncenter")).unwrap_or_default().lines().any(|l| l == key);
    if open { details.set_no_show_all(false); details.show_all(); set_chevron(&chev, true); }
    { let (d, c) = (details.clone(), chev.clone()); chev.connect_clicked(move |_| {
        let o = !d.is_visible();
        if o { d.set_no_show_all(false); d.show_all(); } else { d.hide(); }
        set_chevron(&c, o);
        // remember like Win7 does
        let p = cache_dir().join("actioncenter");
        let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().filter(|l| *l != key).map(|s| s.to_string()).collect();
        if o { lines.push(key.clone()); }
        let _ = std::fs::write(p, lines.join("\n"));
    }); }
    v.upcast()
}

/// One message bar.
fn message(m: Msg, refresh: Rc<dyn Fn()>) -> gtk::Widget {
    let h = hbox(0);
    h.style_context().add_class("ac-msg");
    let band = vbox(0);
    band.style_context().add_class(if m.red { "ac-band-red" } else { "ac-band-yellow" });
    h.pack_start(&band, false, false, 0);
    let body = hbox(12);
    body.set_border_width(8);
    let col = vbox(3);
    let t = gtk::Label::new(None);
    t.set_xalign(0.0);
    let (main, imp) = m.title.split_once("  ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((m.title.clone(), String::new()));
    t.set_markup(&format!("<b>{}</b>  {}", glib::markup_escape_text(&main), glib::markup_escape_text(&imp)));
    col.pack_start(&t, false, false, 0);
    let tx = label(&m.text, "");
    tx.set_max_width_chars(55);
    col.pack_start(&tx, false, false, 0);
    let key = m.key;
    let r = refresh.clone();
    col.pack_start(&link(&format!("Turn off messages about {}", m.topic), move || { config::update(|c| if !c.maintenance.ac_off.iter().any(|k| k == key) { c.maintenance.ac_off.push(key.to_string()) }); r(); }), false, false, 0);
    body.pack_start(&col, true, true, 0);
    let right = vbox(4);
    right.set_valign(gtk::Align::Start);
    if let Some((label_, needs_admin, f)) = m.button {
        let b = if needs_admin { shield_button(&label_) } else { gtk::Button::with_label(&label_) };
        b.set_halign(gtk::Align::End);
        b.connect_clicked(move |_| f());
        right.pack_start(&b, false, false, 0);
    }
    if let Some((t, f)) = m.extra { let l = link(&t, move || f()); l.set_halign(gtk::Align::End); if let Some(lb) = l.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { lb.set_max_width_chars(24); lb.set_justify(gtk::Justification::Right); lb.set_xalign(1.0); } right.pack_start(&l, false, false, 0); }
    body.pack_start(&right, false, false, 0);
    h.pack_start(&body, true, true, 0);
    h.upcast()
}

fn status_rows_security(s: &Status) -> Vec<(String, String, String)> {
    let mut v = vec![];
    v.push(("Network firewall".into(), if s.fw.on { "On".into() } else if !s.fw.vendor.is_empty() { "On".into() } else { "Off".into() },
        if s.fw.on { format!("Windows Firewall ({}) is actively protecting your computer.", s.fw.backend) } else if !s.fw.vendor.is_empty() { format!("{} is actively protecting your computer.", s.fw.vendor) } else if s.fw_installed { "Windows Firewall is turned off.".into() } else { "No firewall is installed.".into() }));
    v.push(("Windows Update".into(), if s.update_mode == "never" { "Off".into() } else { "On".into() }, if s.update_mode == "never" { "Windows Update will not check for updates.".into() } else { "Windows will check for updates once a day and let you choose which to install.".into() }));
    v.push(("Virus protection".into(), match s.clamav { Some(true) => "On".into(), Some(false) => "Off".into(), None => "Not found".into() },
        match s.clamav { Some(true) => "ClamAV reports that it is up to date and virus scanning is on.".into(), Some(false) => "ClamAV reports that it is turned off.".into(), None => "No antivirus program was found on this computer. Linux desktops seldom need one; ClamAV is available if you want one.".into() }));
    v.push(("User Account Control".into(), if s.agent { "On".into() } else { "Off".into() }, if s.agent { "UAC will notify when programs try to make changes to the computer.".into() } else { "No authentication agent is running.".into() }));
    v
}

fn status_rows_maintenance(_cp: &Rc<Cp>, s: &Status) -> Vec<(String, String, String)> {
    let mut v = vec![];
    v.push(("Check for solutions to problem reports".into(), "On".into(), if s.crashes.is_empty() { "No programs stopped working in the last 7 days.".into() } else { format!("{} problem report(s) in the last 7 days.", s.crashes.len()) }));
    v.push(("Backup".into(), match &s.backup { Some(b) => b.clone(), None => "Not set up".into() }, if s.last_backup > 0 { format!("Last backup: {}", when_words(s.last_backup)) } else if s.backup.is_some() { String::new() } else { "Your files are not being backed up.".into() }));
    let imp = s.updates.pkgs.iter().filter(|p| !p.aur).count();
    v.push(("Check for updates".into(), if imp > 0 { format!("{imp} important updates available") } else { "No action needed".into() }, if s.updates.when > 0 { format!("Last checked: {}", when_words(s.updates.when)) } else { "Windows has not checked for updates yet.".into() }));
    v.push(("Disk space".into(), if s.low_disks.is_empty() { "OK".into() } else { "Low".into() }, s.disks.iter().map(|(m, f, t)| format!("{m}: {} free of {}", gb(*f), gb(*t))).collect::<Vec<_>>().join("\n")));
    v.push(("System services".into(), if s.failed.is_empty() { "No action needed".into() } else { format!("{} failed", s.failed.len()) }, if s.failed.is_empty() { "All services started correctly.".into() } else { s.failed.join(", ") }));
    v.push(("Troubleshooting: System Maintenance".into(), "No action needed".into(), "Windows is actively analyzing your computer for maintenance problems.".into()));
    v
}

/// "Turn messages on or off".
fn settings(cp: &Rc<Cp>, holder: &gtk::Box) {
    let off = config::get().maintenance.ac_off;
    let v = vbox(8);
    v.pack_start(&title("Turn messages on or off"), false, false, 0);
    v.pack_start(&text("For each selected item, Windows will check for problems and send you a message if problems are found."), false, false, 0);
    v.pack_start(&link("How does Action Center check for problems?", || { util::open_uri("https://wiki.archlinux.org/title/System_maintenance"); }), false, false, 0);
    let mut checks: Vec<(&'static str, gtk::CheckButton)> = vec![];
    for (head, items) in [("Security messages", &[("updates", "Windows Update"), ("firewall", "Network firewall"), ("virus", "Virus protection"), ("uac", "User Account Control")][..]),
                          ("Maintenance messages", &[("backup", "Windows Backup"), ("check", "Check for updates"), ("disk", "Disk space"), ("services", "System services"), ("crashes", "Problem reports")][..])] {
        v.pack_start(&super::super::super::section(head), false, false, 0);
        let g = gtk::Grid::new();
        g.set_column_spacing(40);
        g.set_row_spacing(6);
        g.set_margin_start(14);
        for (i, (k, l)) in items.iter().enumerate() {
            let c = gtk::CheckButton::with_label(l);
            c.set_active(!off.iter().any(|x| x == k));
            g.attach(&c, (i % 2) as i32, (i / 2) as i32, 1, 1);
            checks.push((k, c));
        }
        v.pack_start(&g, false, false, 0);
    }
    v.pack_start(&super::super::super::section("Related settings"), false, false, 0);
    let rel = vbox(2);
    rel.set_margin_start(14);
    { let cp2 = cp.clone(); rel.pack_start(&link("Windows Update settings", move || cp2.open("update")), false, false, 0); }
    { let cp2 = cp.clone(); rel.pack_start(&link("Backup settings", move || cp2.open("backup")), false, false, 0); }
    v.pack_start(&rel, false, false, 0);
    let bb = hbox(8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    ok.set_size_request(80, -1);
    cancel.set_size_request(80, -1);
    bb.pack_end(&cancel, false, false, 0);
    bb.pack_end(&ok, false, false, 0);
    v.pack_start(&bb, false, false, 8);
    { let cp2 = cp.clone(); ok.connect_clicked(move |_| {
        let off: Vec<String> = checks.iter().filter(|(_, c)| !c.is_active()).map(|(k, _)| k.to_string()).collect();
        config::update(|c| c.maintenance.ac_off = off.clone());
        cp2.open("actioncenter");
    }); }
    { let cp2 = cp.clone(); cancel.connect_clicked(move |_| cp2.open("actioncenter")); }
    subpage(cp, holder, "actioncenter", "Change Action Center settings", &v);
}

/// "Review archived messages about computer problems" (coredumps = problem reports).
pub fn archived(cp: &Rc<Cp>) {
    let d = gtk::Dialog::with_buttons(Some("Problem Reports"), Some(&cp.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("OK", gtk::ResponseType::Ok)]);
    d.set_default_size(700, 430);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    let t = gtk::Label::new(None);
    t.set_markup("<span size='large' foreground='#1e3287'>Review archived messages</span>");
    t.set_xalign(0.0);
    c.pack_start(&t, false, false, 0);
    c.pack_start(&label("Archived messages about computer problems: programs that stopped working (core dumps kept by systemd-coredump).", ""), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING; 5]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Source", 0, 170), ("Summary", 1, 160), ("Date", 2, 170), ("Status", 3, 0)]);
    c.pack_start(&scrolled(&tv), true, true, 0);
    let st = label("Loading…", "dim");
    c.pack_start(&st, false, false, 0);
    { let w = cp.win.clone(); tv.connect_row_activated(move |tv, p, _| {
        let Some(m) = tv.model() else { return };
        let Some(it) = m.iter(p) else { return };
        let pid = sval(&m, &it, 4);
        let w = w.clone();
        bg(move || sh(&["coredumpctl", "info", "--no-pager", &pid]), move |out| {
            let d = gtk::Dialog::with_buttons(Some("Problem Details"), Some(&w), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("Close", gtk::ResponseType::Close)]);
            d.set_default_size(640, 420);
            let tvw = text_box(if out.is_empty() { "No details are available." } else { &out });
            tvw.set_monospace(true);
            d.content_area().pack_start(&scrolled(&tvw), true, true, 0);
            d.connect_response(|d, _| d.close());
            d.show_all();
        });
    }); }
    d.connect_response(|d, _| d.close());
    d.show_all();
    bg(|| sh(&["coredumpctl", "list", "--no-legend", "--no-pager", "-r", "-q"]), move |out| {
        let mut n = 0;
        for l in out.lines().take(500) {
            let f: Vec<&str> = l.split_whitespace().collect();
            let exe = f.iter().rev().find(|x| x.starts_with('/')).copied().unwrap_or("");
            let name = std::path::Path::new(exe).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
            let date = f.iter().take(4).copied().collect::<Vec<_>>().join(" ");
            store.insert_with_values(None, &[(0, &name), (1, &"Stopped working"), (2, &date), (3, &"Not reported"), (4, &f.get(4).copied().unwrap_or("").to_string())]);
            n += 1;
        }
        st.set_text(&if n == 0 { "There are no archived messages.".to_string() } else { format!("{n} reports. Double-click a report to see its details.") });
    });
}
