//! Backup and Restore: Windows Backup = an rsync copy of the home folder (or chosen folders) to
//! another drive, on a schedule kept as a systemd user timer; Restore = copy back (never deleting,
//! never overwriting newer files). System images / restore points = Timeshift or Snapper
//! snapshots (listed, created through pkexec; restoring opens their own tool).
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::super::super::{link, section, title, Cp};
use super::*;
use aerode::config::{self, Backup};

const STAMP: &str = ".aerode-backup";
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// When the last backup finished (the stamp file on the backup drive).
pub fn last_backup(b: &Backup) -> Option<i64> { mtime(&Path::new(&b.dest).join(STAMP)) }

fn hour_label(h: u32) -> String { format!("{}:00 {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" }) }

pub fn schedule_text(b: &Backup) -> String {
    match b.schedule.as_str() {
        "daily" => format!("Every day at {}", hour_label(b.hour)),
        "weekly" => format!("Every {} at {}", DAYS[(b.day % 7) as usize], hour_label(b.hour)),
        "monthly" => format!("On day {} of every month at {}", b.day.clamp(1, 28), hour_label(b.hour)),
        _ => "Not scheduled".into(),
    }
}

/// Next run of the schedule (local time).
pub fn next_run(b: &Backup) -> Option<i64> {
    use chrono::{Datelike, Duration, Local, TimeZone, Timelike};
    let now = Local::now();
    let at = |d: chrono::NaiveDate| Local.from_local_datetime(&d.and_hms_opt(b.hour.min(23), 0, 0)?).single();
    let today = now.date_naive();
    for i in 0..62 {
        let d = today + Duration::days(i);
        let ok = match b.schedule.as_str() { "daily" => true, "weekly" => d.weekday().num_days_from_sunday() == b.day % 7, "monthly" => d.day() == b.day.clamp(1, 28), _ => return None };
        if ok { if let Some(t) = at(d) { if t > now || (i == 0 && now.hour() < b.hour) { return Some(t.timestamp()); } } }
    }
    None
}

fn home() -> PathBuf { config::home() }

/// rsync arguments for a backup run (also written into the systemd service).
pub fn rsync_args(b: &Backup, progress: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["rsync".into(), "-a".into(), "--exclude=.cache/".into(), "--exclude=.local/share/Trash/".into(), "--exclude=*.tmp".into()];
    if progress { a.push("--info=progress2".into()); a.push("--no-inc-recursive".into()); }
    let h = home();
    // a backup folder inside home must not copy itself
    if let Ok(rel) = Path::new(&b.dest).strip_prefix(&h) { a.push(format!("--exclude=/{}", rel.to_string_lossy())); }
    if b.folders.is_empty() { a.push(format!("{}/", h.display())); }
    else { a.push("-R".into()); for f in &b.folders { a.push(format!("{}/./{f}", h.display())); } }
    a.push(format!("{}/", b.dest));
    a
}

fn unit_files(b: &Backup) -> (String, String) {
    let args: Vec<String> = rsync_args(b, false).into_iter().map(|x| if x.contains(' ') { format!("\"{x}\"") } else { x }).collect();
    let rsync = format!("/usr/bin/{}", args.join(" "));
    let service = format!("[Unit]\nDescription=Windows Backup (Aerode Backup and Restore)\n\n[Service]\nType=oneshot\nNice=10\nIOSchedulingClass=idle\nExecStart={rsync}\nExecStartPost=/usr/bin/touch \"{}/{STAMP}\"\n", b.dest);
    let cal = match b.schedule.as_str() { "daily" => format!("*-*-* {:02}:00:00", b.hour), "weekly" => format!("{} *-*-* {:02}:00:00", &DAYS[(b.day % 7) as usize][..3], b.hour), _ => format!("*-*-{:02} {:02}:00:00", b.day.clamp(1, 28), b.hour) };
    let timer = format!("[Unit]\nDescription=Windows Backup schedule\n\n[Timer]\nOnCalendar={cal}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n");
    (service, timer)
}

/// Save the backup settings and (un)install the schedule as a user timer.
fn save(b: Option<Backup>) -> Result<(), String> {
    let dir = glib::user_config_dir().join("systemd/user");
    match &b {
        Some(b) if !b.schedule.is_empty() => {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let (s, t) = unit_files(b);
            std::fs::write(dir.join("aerode-backup.service"), s).map_err(|e| e.to_string())?;
            std::fs::write(dir.join("aerode-backup.timer"), t).map_err(|e| e.to_string())?;
            run(&["systemctl", "--user", "daemon-reload"]);
            run(&["systemctl", "--user", "enable", "--now", "aerode-backup.timer"]);
        }
        _ => {
            run(&["systemctl", "--user", "disable", "--now", "aerode-backup.timer"]);
            let _ = std::fs::remove_file(dir.join("aerode-backup.timer"));
            if b.is_none() { let _ = std::fs::remove_file(dir.join("aerode-backup.service")); }
            run(&["systemctl", "--user", "daemon-reload"]);
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------- snapshots

#[derive(Clone, Debug, Default)]
pub struct Snap { pub id: String, pub when: String, pub desc: String, pub kind: String }

/// Which system snapshot tool is set up: "timeshift" | "snapper" | "".
pub fn snap_tool() -> &'static str {
    if util::which("timeshift") { "timeshift" } else if util::which("snapper") && std::fs::read_dir("/etc/snapper/configs").map(|d| d.count() > 0).unwrap_or(false) { "snapper" } else { "" }
}

/// `timeshift --list` → snapshots (newest first).
pub fn parse_timeshift(out: &str) -> Vec<Snap> {
    let mut v = vec![];
    let mut table = false;
    for l in out.lines() {
        if l.starts_with("---") { table = true; continue; }
        if !table { continue; }
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 { continue; }
        let (num, rest) = (f[0], if f[1] == ">" { &f[2..] } else { &f[1..] });
        let Some(name) = rest.first() else { continue };
        let tags = rest.get(1).copied().unwrap_or("");
        let desc = rest.iter().skip(2).copied().collect::<Vec<_>>().join(" ");
        let when = chrono::NaiveDateTime::parse_from_str(name, "%Y-%m-%d_%H-%M-%S").map(|d| d.format("%-m/%-d/%Y %-I:%M:%S %p").to_string()).unwrap_or_else(|_| name.to_string());
        let kind = if desc.contains("autosnap") || desc.contains("before upgrade") { "Install" } else if tags.contains('O') { "Manual" } else { "System" };
        v.push(Snap { id: num.to_string(), when, desc: if desc.is_empty() { "Timeshift snapshot".into() } else { desc }, kind: kind.into() });
    }
    v.reverse();
    v
}

/// `snapper --jsonout list` → snapshots (newest first).
pub fn parse_snapper(out: &str) -> Vec<Snap> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(out) else { return vec![] };
    let mut snaps = vec![];
    if let Some(o) = v.as_object() {
        for (_, list) in o { for s in list.as_array().into_iter().flatten() {
            let n = s["number"].as_u64().unwrap_or(0);
            if n == 0 { continue; }
            let date = s["date"].as_str().unwrap_or("");
            let desc = s["description"].as_str().unwrap_or("").to_string();
            let kind = match (s["type"].as_str().unwrap_or(""), s["cleanup"].as_str().unwrap_or("")) { (_, "timeline") => "System", ("pre", _) | ("post", _) => "Install", _ => "Manual" };
            let when = chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S").map(|d| d.format("%-m/%-d/%Y %-I:%M:%S %p").to_string()).unwrap_or_else(|_| date.to_string());
            snaps.push(Snap { id: n.to_string(), when, desc: if desc.is_empty() { format!("Snapshot {n}") } else { desc }, kind: kind.into() });
        } }
    }
    snaps.reverse();
    snaps
}

// ------------------------------------------------------------------------------- page

struct Ui { cp: Rc<Cp>, holder: gtk::Box, running: RefCell<Option<(String, f64)>>, log: RefCell<String> }

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    let holder = vbox(0);
    holder.style_context().add_class("adm");
    let ui = Rc::new(Ui { cp: cp.clone(), holder: holder.clone(), running: RefCell::new(None), log: RefCell::new(String::new()) });
    let b = config::get().maintenance.backup;
    if b.as_ref().map(|b| !b.schedule.is_empty()).unwrap_or(false) {
        let u = ui.clone();
        side_action(cp, "Turn off schedule", move || {
            let mut b = config::get().maintenance.backup;
            if let Some(b) = b.as_mut() { b.schedule.clear(); }
            let b2 = b.clone();
            config::update(|c| c.maintenance.backup = b.clone());
            let u2 = u.clone();
            bg(move || save(b2), move |r| { if let Err(e) = r { util::error("The schedule could not be turned off.", &e); } u2.cp.open("backup"); });
        });
    }
    { let u = ui.clone(); side_shield(cp, "Create a system image", move || create_snapshot(&u)); }
    side_action(cp, "Create a system repair disc", || inform(None, "Create a system repair disc", "Use an installation USB stick as your repair disc.", "Download the EndeavourOS or Arch Linux ISO and write it to a USB stick (for example with Disks or `dd`). Start the computer from it to repair the system, mount your drives or restore a snapshot."));
    see_also(cp, &[("Action Center", "actioncenter"), ("Recovery", "recovery")]);
    view(&ui);
    holder.upcast()
}

fn view(ui: &Rc<Ui>) {
    for c in ui.holder.children() { ui.holder.remove(&c); }
    let v = vbox(8);
    v.pack_start(&title("Back up or restore your files"), false, false, 0);
    let b = config::get().maintenance.backup;
    // ------------------------------------------------ Backup
    v.pack_start(&section("Backup"), false, false, 0);
    let bk = vbox(8);
    bk.set_margin_start(14);
    match &b {
        None => {
            let h = hbox(10);
            h.pack_start(&label("Windows Backup has not been set up.", ""), true, true, 0);
            let u = ui.clone();
            let l = link("Set up backup", move || wizard(&u));
            h.pack_end(&l, false, false, 0);
            bk.pack_start(&h, false, false, 0);
        }
        Some(b) => {
            let g = gtk::Grid::new();
            g.set_row_spacing(8);
            g.set_column_spacing(18);
            let loc = hbox(8);
            loc.pack_start(&img(&["drive-removable-media", "drive-harddisk"], 32), false, false, 0);
            let lc = vbox(1);
            lc.pack_start(&label(&b.dest, ""), false, false, 0);
            let space = label("", "dim");
            lc.pack_start(&space, false, false, 0);
            loc.pack_start(&lc, false, false, 0);
            let k = |t: &str| { let l = label(t, ""); l.set_yalign(0.0); l };
            g.attach(&k("Location:"), 0, 0, 1, 1);
            g.attach(&loc, 1, 0, 1, 1);
            let size = label("Backup size: calculating…", "");
            g.attach(&size, 2, 0, 1, 1);
            let running = ui.running.borrow().clone();
            let right = vbox(4);
            match &running {
                Some((step, frac)) => {
                    right.pack_start(&label("Backup in progress...", ""), false, false, 0);
                    let pb = gtk::ProgressBar::new();
                    if *frac >= 0.0 { pb.set_fraction(*frac); } else { pb.pulse(); }
                    pb.set_size_request(200, -1);
                    right.pack_start(&pb, false, false, 0);
                    right.pack_start(&label(step, "dim"), false, false, 0);
                    let u = ui.clone();
                    right.pack_start(&link("View Details", move || details(&u)), false, false, 0);
                }
                None => {
                    let now_b = gtk::Button::with_mnemonic("_Back up now");
                    let u = ui.clone();
                    now_b.connect_clicked(move |_| back_up_now(&u));
                    right.pack_start(&now_b, false, false, 0);
                }
            }
            right.set_halign(gtk::Align::End);
            right.set_hexpand(true);
            g.attach(&right, 3, 0, 1, 2);
            let last = last_backup(b);
            let rows = [("Next backup:", next_run(b).map(when_words).unwrap_or_else(|| "Not scheduled".into())), ("Last backup:", last.map(when_words).unwrap_or_else(|| "Never".into())),
                ("Contents:", if b.folders.is_empty() { "Your home folder (all files and settings)".to_string() } else { format!("Selected folders: {}", b.folders.join(", ")) }), ("Schedule:", schedule_text(b))];
            for (i, (kk, vv)) in rows.iter().enumerate() { g.attach(&k(kk), 0, i as i32 + 1, 1, 1); let l = label(vv, ""); l.set_max_width_chars(50); g.attach(&l, 1, i as i32 + 1, 2, 1); }
            bk.pack_start(&g, false, false, 0);
            let u = ui.clone();
            bk.pack_start(&link("Change settings", move || wizard(&u)), false, false, 0);
            let dest = b.dest.clone();
            bg(move || {
                let du = sh(&["du", "-sb", &dest]).split_whitespace().next().and_then(|x| x.parse::<u64>().ok());
                (du, fs_space(&dest))
            }, move |(du, sp)| {
                size.set_text(&format!("Backup size: {}", du.map(gb).unwrap_or_else(|| "unknown".into())));
                if let Some((f, t)) = sp { space.set_text(&format!("{} free of {}", gb(f), gb(t))); } else { space.set_text("The backup location is not available. Connect the drive."); }
            });
        }
    }
    v.pack_start(&bk, false, false, 0);
    // ------------------------------------------------ Restore
    v.pack_start(&section("Restore"), false, false, 0);
    let rs = vbox(6);
    rs.set_margin_start(14);
    let have = b.as_ref().map(|b| Path::new(&b.dest).exists() && last_backup(b).is_some()).unwrap_or(false);
    if have {
        let h = hbox(10);
        h.pack_start(&label("You can restore your files that were backed up on the current location.", ""), true, true, 0);
        let rb = gtk::Button::with_mnemonic("_Restore my files");
        let u = ui.clone();
        rb.connect_clicked(move |_| restore(&u, false));
        h.pack_end(&rb, false, false, 0);
        rs.pack_start(&h, false, false, 0);
        let u = ui.clone();
        rs.pack_start(&link("Restore all users' files", move || restore(&u, true)), false, false, 0);
    } else {
        rs.pack_start(&label("Windows could not find a backup for this computer.", ""), false, false, 0);
    }
    rs.pack_start(&link("Select another backup to restore files from", || {
        let fc = gtk::FileChooserNative::new(Some("Select another backup"), None::<&gtk::Window>, gtk::FileChooserAction::SelectFolder, Some("_Open"), Some("_Cancel"));
        if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() { util::explore(&p.to_string_lossy()); } }
    }), false, false, 0);
    { let cp = ui.cp.clone(); rs.pack_start(&link("Recover system settings or your computer", move || cp.open("recovery")), false, false, 0); }
    v.pack_start(&rs, false, false, 0);
    // ------------------------------------------------ system snapshots
    let tool = snap_tool();
    v.pack_start(&section(&format!("System image ({})", match tool { "timeshift" => "Timeshift", "snapper" => "Snapper", _ => "not set up" })), false, false, 0);
    let ss = vbox(6);
    ss.set_margin_start(14);
    if tool.is_empty() {
        ss.pack_start(&label("System snapshots let you return the operating system to an earlier state, for example after a broken update. Install Timeshift to create them.", ""), false, false, 0);
        let u = ui.clone();
        ss.pack_start(&shield_link("Install Timeshift", move || {
            let u2 = u.clone();
            if confirm(win_of(&u.holder).as_ref(), "Backup and Restore", "Install Timeshift?", "Timeshift creates system snapshots (rsync or Btrfs) and can restore them from the running system or a live USB.", "Install") {
                elevate(&["pacman", "-S", "--needed", "--noconfirm", "timeshift"], move |ok, out| { report("Timeshift could not be installed.", ok, &out); view(&u2); });
            }
        }), false, false, 0);
    } else {
        ss.pack_start(&label("Restore points of the operating system. Restoring returns system files and settings to the selected point; your documents are not affected.", ""), false, false, 0);
        let store = gtk::ListStore::new(&[glib::Type::STRING; 4]);
        let tv = gtk::TreeView::with_model(&store);
        columns(&tv, &[("Date and Time", 0, 190), ("Description", 1, 330), ("Type", 2, 0)]);
        let sw = scrolled(&tv);
        sw.set_min_content_height(150);
        ss.pack_start(&sw, false, false, 0);
        let bb = hbox(8);
        let show = shield_button("_Show restore points");
        let create = shield_button("_Create...");
        let restore_b = gtk::Button::with_mnemonic("System _Restore...");
        bb.pack_start(&show, false, false, 0);
        bb.pack_end(&restore_b, false, false, 0);
        bb.pack_end(&create, false, false, 0);
        ss.pack_start(&bb, false, false, 0);
        let fill = { let store = store.clone(); move |snaps: Vec<Snap>| { store.clear(); for s in snaps { store.insert_with_values(None, &[(0, &s.when), (1, &s.desc), (2, &s.kind), (3, &s.id)]); } } };
        if tool == "snapper" {
            // snapper can list without root when ALLOW_USERS/GROUPS permit it
            let (fill, show) = (fill.clone(), show.clone());
            bg(|| run(&["snapper", "--jsonout", "list"]), move |(ok, out)| if ok { fill(parse_snapper(&out)); show.hide(); });
        }
        { let fill = fill.clone(); show.connect_clicked(move |b| {
            let (fill, b) = (fill.clone(), b.clone());
            let argv: Vec<&str> = if tool == "timeshift" { vec!["timeshift", "--list"] } else { vec!["snapper", "--jsonout", "list"] };
            elevate(&argv, move |ok, out| { if report("The restore points could not be read.", ok, &out) { fill(if tool == "timeshift" { parse_timeshift(&out) } else { parse_snapper(&out) }); b.hide(); } });
        }); }
        { let u = ui.clone(); create.connect_clicked(move |_| create_snapshot(&u)); }
        restore_b.connect_clicked(move |_| {
            for t in [&["timeshift-launcher"][..], &["timeshift-gtk"], &["btrfs-assistant"], &["snapper-gui"]] { if util::spawn(t) { return; } }
            inform(None, "System Restore", "Restore a snapshot from a terminal.", if tool == "timeshift" { "Run: sudo timeshift --restore" } else { "Run: sudo snapper undochange N..0  (N = the snapshot number), or boot into the snapshot from the boot menu." });
        });
    }
    v.pack_start(&ss, false, false, 0);
    ui.holder.pack_start(&v, true, true, 0);
    ui.holder.show_all();
}


fn create_snapshot(ui: &Rc<Ui>) {
    let tool = snap_tool();
    if tool.is_empty() { inform(win_of(&ui.holder).as_ref(), "Create a system image", "Timeshift is not installed.", "Install Timeshift (Backup and Restore ▸ System image ▸ Install Timeshift) to create system images and restore points."); return; }
    let Some(desc) = aerode::desktop::prompt("Create a restore point", "Type a description to help you identify the restore point. The current date and time are added automatically.", "") else { return };
    let argv: Vec<String> = if tool == "timeshift" { vec!["timeshift".into(), "--create".into(), "--comments".into(), desc, "--tags".into(), "O".into()] } else { vec!["snapper".into(), "create".into(), "--description".into(), desc, "--cleanup-algorithm".into(), "number".into()] };
    let r: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let u = ui.clone();
    elevate(&r, move |ok, out| { if report("The restore point could not be created.", ok, &out) { inform(win_of(&u.holder).as_ref(), "System Protection", "The restore point was created successfully.", ""); view(&u); } });
}

fn details(ui: &Rc<Ui>) {
    let d = gtk::Dialog::with_buttons(Some("Windows Backup is currently in progress"), win_of(&ui.holder).as_ref(), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("Close", gtk::ResponseType::Close)]);
    d.set_default_size(560, 320);
    let tv = text_box(&ui.log.borrow());
    tv.set_monospace(true);
    d.content_area().pack_start(&scrolled(&tv), true, true, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
}

fn back_up_now(ui: &Rc<Ui>) {
    let Some(b) = config::get().maintenance.backup else { return };
    if !util::which("rsync") { util::error("Windows Backup needs rsync.", "Install the rsync package and try again."); return; }
    if !Path::new(&b.dest).parent().map(|p| p.exists()).unwrap_or(false) { util::error("The backup location is not available.", &format!("Connect the drive that holds {} and try again.", b.dest)); return; }
    *ui.running.borrow_mut() = Some(("Preparing...".into(), -1.0));
    ui.log.borrow_mut().clear();
    view(ui);
    enum M { P(String, f64), Line(String), End(bool) }
    let u = ui.clone();
    let args = rsync_args(&b, true);
    let dest = b.dest.clone();
    stream(move |tx: async_channel::Sender<M>| {
        use std::io::Read;
        let _ = std::fs::create_dir_all(&dest);
        let child = std::process::Command::new(&args[0]).args(&args[1..]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn();
        let Ok(mut child) = child else { let _ = tx.send_blocking(M::End(false)); return };
        let err = child.stderr.take();
        let tx2 = tx.clone();
        let t = std::thread::spawn(move || { let mut s = String::new(); if let Some(mut e) = err { let _ = e.read_to_string(&mut s); } for l in s.lines() { let _ = tx2.send_blocking(M::Line(l.to_string())); } });
        if let Some(mut o) = child.stdout.take() {
            let mut buf = [0u8; 4096];
            let mut line = String::new();
            loop {
                let n = o.read(&mut buf).unwrap_or(0);
                if n == 0 { break; }
                for ch in String::from_utf8_lossy(&buf[..n]).chars() {
                    if ch == '\r' || ch == '\n' {
                        // "  1,234,567  45%   12.34MB/s    0:00:10 (xfr#3, to-chk=10/20)"
                        let f: Vec<&str> = line.split_whitespace().collect();
                        if let Some(p) = f.get(1).and_then(|p| p.strip_suffix('%')).and_then(|p| p.parse::<f64>().ok()) {
                            let _ = tx.send_blocking(M::P(format!("{} copied, {}", f[0], f.get(2).copied().unwrap_or("")), p / 100.0));
                        } else if !line.trim().is_empty() { let _ = tx.send_blocking(M::Line(line.clone())); }
                        line.clear();
                    } else { line.push(ch); }
                }
            }
        }
        let _ = t.join();
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
        // 24 = some files vanished while copying: still a good backup
        let ok = code == 0 || code == 24;
        if ok { let _ = std::fs::write(Path::new(&dest).join(STAMP), now().to_string()); }
        let _ = tx.send_blocking(M::End(ok));
    }, move |m| match m {
        M::P(t, f) => { *u.running.borrow_mut() = Some((t, f)); if u.holder.parent().is_some() { view(&u); } }
        M::Line(l) => { u.log.borrow_mut().push_str(&l); u.log.borrow_mut().push('\n'); }
        M::End(ok) => {
            *u.running.borrow_mut() = None;
            if u.holder.parent().is_some() { view(&u); }
            if !ok { util::error("The backup did not complete successfully.", u.log.borrow().lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n").as_str()); }
        }
    });
}

/// "Restore Files": pick files/folders from the backup, restore to the original place or elsewhere.
fn restore(ui: &Rc<Ui>, all: bool) {
    let Some(b) = config::get().maintenance.backup else { return };
    let d = gtk::Dialog::with_buttons(Some("Restore Files"), win_of(&ui.holder).as_ref(), gtk::DialogFlags::MODAL, &[("_Restore", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(560, 440);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    let t = gtk::Label::new(None);
    t.set_markup(&format!("<span size='large' foreground='#1e3287'>{}</span>", if all { "Restore all users' files" } else { "Browse or search your backup for files and folders to restore" }));
    t.set_xalign(0.0);
    c.pack_start(&t, false, false, 0);
    c.pack_start(&label(&format!("All files will be restored to their latest version.\nLast backup: {}", last_backup(&b).map(when_words).unwrap_or_default()), ""), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Name", 0, 220), ("In Folder", 1, 0)]);
    let sw = scrolled(&tv);
    sw.set_min_content_height(180);
    let row = hbox(8);
    row.pack_start(&sw, true, true, 0);
    let btns = vbox(6);
    let bfiles = gtk::Button::with_mnemonic("Browse for _files");
    let bfolders = gtk::Button::with_mnemonic("Browse for f_olders");
    let bremove = gtk::Button::with_mnemonic("_Remove");
    for x in [&bfiles, &bfolders, &bremove] { btns.pack_start(x, false, false, 0); }
    row.pack_start(&btns, false, false, 0);
    c.pack_start(&row, true, true, 0);
    if all { store.insert_with_values(None, &[(0, &"(everything in the backup)"), (1, &b.dest)]); row.set_sensitive(false); }
    let dest = b.dest.clone();
    for (btn, folders) in [(&bfiles, false), (&bfolders, true)] {
        let (store, dest, d2) = (store.clone(), dest.clone(), d.clone());
        btn.connect_clicked(move |_| {
            let fc = gtk::FileChooserNative::new(Some(if folders { "Browse the backup for folders" } else { "Browse the backup for files" }), Some(&d2), if folders { gtk::FileChooserAction::SelectFolder } else { gtk::FileChooserAction::Open }, Some("_Add"), Some("_Cancel"));
            fc.set_select_multiple(true);
            let _ = fc.set_current_folder(&dest);
            if fc.run() == gtk::ResponseType::Accept {
                for f in fc.filenames() { if f.starts_with(&dest) { store.insert_with_values(None, &[(0, &f.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()), (1, &f.to_string_lossy().to_string())]); } }
            }
        });
    }
    { let (tv, store) = (tv.clone(), store.clone()); bremove.connect_clicked(move |_| { if let Some((_, it)) = tv.selection().selected() { store.remove(&it); } }); }
    c.pack_start(&bold("Where do you want to restore your files?"), false, false, 0);
    let orig = gtk::RadioButton::with_label("In the original location");
    let other = gtk::RadioButton::with_label_from_widget(&orig, "In the following location:");
    let place = gtk::FileChooserButton::new("Restore to", gtk::FileChooserAction::SelectFolder);
    let _ = place.set_current_folder(home());
    c.pack_start(&orig, false, false, 0);
    let h = hbox(8);
    h.pack_start(&other, false, false, 0);
    h.pack_start(&place, true, true, 0);
    c.pack_start(&h, false, false, 0);
    c.pack_start(&label("Files that are newer on this computer are kept (nothing is deleted).", "dim"), false, false, 0);
    d.show_all();
    let r = d.run();
    let mut items: Vec<String> = vec![];
    store.foreach(|m, _, it| { items.push(sval(m, it, 1)); false });
    let target = if other.is_active() { place.filename().map(|p| p.to_string_lossy().into_owned()) } else { None };
    d.close();
    if r != gtk::ResponseType::Ok || items.is_empty() { return; }
    let home_s = home().to_string_lossy().into_owned();
    let dest = b.dest.clone();
    let u = ui.clone();
    bg(move || {
        let mut errs = String::new();
        for it in items {
            let src = if all { format!("{dest}/") } else { it.clone() };
            let rel = Path::new(&src).strip_prefix(&dest).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let to = match &target { Some(t) => if all { format!("{t}/") } else { format!("{t}/") }, None => { let p = Path::new(&home_s).join(&rel); format!("{}/", p.parent().unwrap_or(Path::new(&home_s)).display()) } };
            let to = if all && target.is_none() { format!("{home_s}/") } else { to };
            let (ok, out) = run(&["rsync", "-a", "--update", &src, &to]);
            if !ok { errs += &out; }
        }
        errs
    }, move |errs| {
        if errs.is_empty() { inform(win_of(&u.holder).as_ref(), "Restore Files", "Your files have been restored.", ""); } else { util::error("Some files could not be restored.", errs.trim()); }
    });
}

/// "Set up backup": where, what, schedule, review.
fn wizard(ui: &Rc<Ui>) {
    let cur = config::get().maintenance.backup.unwrap_or(Backup { schedule: "weekly".into(), day: 0, hour: 19, ..Default::default() });
    let d = gtk::Dialog::new();
    d.set_title("Set up backup");
    d.set_transient_for(win_of(&ui.holder).as_ref());
    d.set_modal(true);
    d.set_default_size(600, 470);
    let next = d.add_button("_Next", gtk::ResponseType::Other(2));
    let save_b = d.add_button("_Save settings and run backup", gtk::ResponseType::Ok).downcast::<gtk::Button>().unwrap();
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let stack = gtk::Stack::new();
    stack.set_border_width(14);
    d.content_area().pack_start(&stack, true, true, 0);
    let head = |t: &str| { let l = gtk::Label::new(None); l.set_markup(&format!("<span size='large' foreground='#1e3287'>{}</span>", glib::markup_escape_text(t))); l.set_xalign(0.0); l };
    // 1: where
    let p1 = vbox(8);
    p1.pack_start(&head("Select where you want to save your backup"), false, false, 0);
    p1.pack_start(&label("We recommend that you save your backup on an external hard drive.", ""), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Backup Destination", 0, 260), ("Free Space", 1, 100), ("Total Size", 2, 0)]);
    let sw = scrolled(&tv);
    sw.set_min_content_height(200);
    p1.pack_start(&sw, true, true, 0);
    let fill = { let store = store.clone(); move || {
        store.clear();
        let home_dev = std::fs::metadata(home()).ok().map(|m| { use std::os::unix::fs::MetadataExt; m.dev() });
        for l in std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 3 || !f[0].starts_with("/dev/") { continue; }
            let mp = f[1].replace("\\040", " ");
            if ["/", "/boot", "/boot/efi", "/efi", "/home", "/usr", "/var"].contains(&mp.as_str()) || mp.starts_with("/snap") || matches!(f[2], "squashfs" | "iso9660") { continue; }
            let same = std::fs::metadata(&mp).ok().map(|m| { use std::os::unix::fs::MetadataExt; Some(m.dev()) == home_dev }).unwrap_or(false);
            if same { continue; }
            let Some((free, total)) = fs_space(&mp) else { continue };
            let name = format!("{} ({})", Path::new(&mp).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or(mp.clone()), mp);
            store.insert_with_values(None, &[(0, &name), (1, &gb(free)), (2, &gb(total)), (3, &mp)]);
        }
    } };
    fill();
    let bb = hbox(8);
    let refresh = gtk::Button::with_mnemonic("_Refresh");
    let network = gtk::Button::with_mnemonic("Save on a ne_twork / folder...");
    bb.pack_start(&refresh, false, false, 0);
    bb.pack_end(&network, false, false, 0);
    p1.pack_start(&bb, false, false, 0);
    { let f = fill.clone(); refresh.connect_clicked(move |_| f()); }
    let chosen = Rc::new(RefCell::new(if cur.dest.is_empty() { String::new() } else { cur.dest.clone() }));
    let chosen_l = label(&if cur.dest.is_empty() { String::new() } else { format!("Current location: {}", cur.dest) }, "dim");
    p1.pack_start(&chosen_l, false, false, 0);
    { let (chosen, cl, d2) = (chosen.clone(), chosen_l.clone(), d.clone()); network.connect_clicked(move |_| {
        let fc = gtk::FileChooserNative::new(Some("Select a backup folder"), Some(&d2), gtk::FileChooserAction::SelectFolder, Some("_Select"), Some("_Cancel"));
        if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() { *chosen.borrow_mut() = p.to_string_lossy().into_owned(); cl.set_text(&format!("Selected: {}", p.display())); } }
    }); }
    { let (chosen, cl) = (chosen.clone(), chosen_l.clone()); tv.selection().connect_changed(move |s| if let Some((m, it)) = s.selected() {
        let mp = sval(&m, &it, 3);
        let dest = format!("{mp}/AerodeBackup/{}/{}", glib::host_name(), glib::user_name().to_string_lossy());
        cl.set_text(&format!("Selected: {dest}"));
        *chosen.borrow_mut() = dest;
    }); }
    stack.add_named(&p1, "1");
    // 2: what
    let p2 = vbox(8);
    p2.pack_start(&head("What do you want to back up?"), false, false, 0);
    let auto = gtk::RadioButton::with_label("Let Windows choose (recommended)");
    p2.pack_start(&auto, false, false, 0);
    let l = label("Windows will back up your whole home folder: Desktop, Documents, Pictures, Music, Videos, Downloads and your program settings. Temporary files and the Recycle Bin are skipped.", "dim");
    l.set_margin_start(24);
    p2.pack_start(&l, false, false, 0);
    let mine = gtk::RadioButton::with_label_from_widget(&auto, "Let me choose");
    p2.pack_start(&mine, false, false, 0);
    let l = label("You can select folders to back up.", "dim");
    l.set_margin_start(24);
    p2.pack_start(&l, false, false, 0);
    let fl = vbox(2);
    fl.set_margin_start(24);
    let mut folders: Vec<String> = std::fs::read_dir(home()).map(|d| d.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.') || n == ".config" || n == ".local" || n == ".mozilla" || n == ".ssh").collect()).unwrap_or_default();
    folders.sort_by_key(|f| (f.starts_with('.'), f.to_lowercase()));
    let checks: Vec<(String, gtk::CheckButton)> = folders.iter().map(|f| { let c = gtk::CheckButton::with_label(&match f.as_str() { ".config" => "Program settings (.config)".to_string(), ".local" => "Program data (.local)".into(), ".ssh" => "SSH keys (.ssh)".into(), ".mozilla" => "Firefox profile (.mozilla)".into(), _ => f.clone() }); c.set_active(cur.folders.contains(f)); (f.clone(), c) }).collect();
    for (_, c) in &checks { fl.pack_start(c, false, false, 0); }
    let fsw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    fsw.set_min_content_height(170);
    fsw.add(&fl);
    p2.pack_start(&fsw, true, true, 0);
    if !cur.folders.is_empty() { mine.set_active(true); }
    { let fsw = fsw.clone(); fsw.set_sensitive(mine.is_active()); mine.connect_toggled(move |m| fsw.set_sensitive(m.is_active())); }
    stack.add_named(&p2, "2");
    // 3: review + schedule
    let p3 = vbox(8);
    p3.pack_start(&head("Review your backup settings"), false, false, 0);
    let review = label("", "");
    p3.pack_start(&review, false, false, 0);
    p3.pack_start(&bold("How often do you want to back up?"), false, false, 0);
    let on = gtk::CheckButton::with_label("Run backup on a schedule (recommended)");
    on.set_active(!cur.schedule.is_empty());
    p3.pack_start(&on, false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    g.set_margin_start(24);
    let how = gtk::ComboBoxText::new();
    for (id, l) in [("daily", "Daily"), ("weekly", "Weekly"), ("monthly", "Monthly")] { how.append(Some(id), l); }
    how.set_active_id(Some(if cur.schedule.is_empty() { "weekly" } else { &cur.schedule }));
    let dayc = gtk::ComboBoxText::new();
    let hourc = gtk::ComboBoxText::new();
    for h in 0..24 { hourc.append(Some(&h.to_string()), &hour_label(h)); }
    hourc.set_active_id(Some(&cur.hour.to_string()));
    let fill_days = { let (dayc, how) = (dayc.clone(), how.clone()); let day = cur.day; move || {
        dayc.remove_all();
        match how.active_id().as_deref() { Some("weekly") => for (i, dn) in DAYS.iter().enumerate() { dayc.append(Some(&i.to_string()), dn); }, Some("monthly") => for i in 1..=28 { dayc.append(Some(&i.to_string()), &i.to_string()); }, _ => {} }
        if !dayc.set_active_id(Some(&day.to_string())) { dayc.set_active(Some(0)); }
        dayc.set_sensitive(how.active_id().as_deref() != Some("daily"));
    } };
    fill_days();
    { let f = fill_days.clone(); how.connect_changed(move |_| f()); }
    for (i, (l, w)) in [("How often:", how.clone().upcast::<gtk::Widget>()), ("What day:", dayc.clone().upcast()), ("What time:", hourc.clone().upcast())].into_iter().enumerate() {
        g.attach(&label(l, ""), 0, i as i32, 1, 1);
        g.attach(&w, 1, i as i32, 1, 1);
    }
    { let g = g.clone(); g.set_sensitive(on.is_active()); on.connect_toggled(move |c| g.set_sensitive(c.is_active())); }
    p3.pack_start(&g, false, false, 0);
    stack.add_named(&p3, "3");
    let page = Rc::new(std::cell::Cell::new(1));
    let build = { let (auto, checks, chosen, on, how, dayc, hourc) = (auto.clone(), checks.clone(), chosen.clone(), on.clone(), how.clone(), dayc.clone(), hourc.clone()); move || Backup {
        dest: chosen.borrow().clone(),
        folders: if auto.is_active() { vec![] } else { checks.iter().filter(|(_, c)| c.is_active()).map(|(f, _)| f.clone()).collect() },
        schedule: if on.is_active() { how.active_id().map(|s| s.to_string()).unwrap_or_default() } else { String::new() },
        day: dayc.active_id().and_then(|s| s.parse().ok()).unwrap_or(0),
        hour: hourc.active_id().and_then(|s| s.parse().ok()).unwrap_or(19),
    } };
    let update = { let (stack, page, next, save_b, review, build) = (stack.clone(), page.clone(), next.clone(), save_b.clone(), review.clone(), build.clone()); move || {
        stack.set_visible_child_name(&page.get().to_string());
        next.set_visible(page.get() < 3);
        save_b.set_visible(page.get() == 3);
        if page.get() == 3 {
            let b = build();
            review.set_text(&format!("Backup Location:  {}\n\nBackup Summary:\n    {}\n\nSchedule:  {}", b.dest, if b.folders.is_empty() { "Your home folder (all files and settings)".to_string() } else { b.folders.join(", ") }, schedule_text(&b)));
        }
    } };
    update();
    { let (page, update, chosen, d2) = (page.clone(), update.clone(), chosen.clone(), d.clone()); let (on, how, dayc, hourc) = (on.clone(), how.clone(), dayc.clone(), hourc.clone());
      let refresh_review = update.clone();
      for w in [on.upcast_ref::<gtk::Widget>(), how.upcast_ref(), dayc.upcast_ref(), hourc.upcast_ref()] { let r = refresh_review.clone(); if let Some(c) = w.downcast_ref::<gtk::ComboBoxText>() { c.connect_changed(move |_| r()); } else if let Some(c) = w.downcast_ref::<gtk::CheckButton>() { c.connect_toggled(move |_| r()); } }
      d.connect_response(move |d, r| match r {
        gtk::ResponseType::Other(2) => {
            if page.get() == 1 && chosen.borrow().is_empty() { inform(Some(d2.upcast_ref()), "Set up backup", "Select a backup destination first.", ""); return; }
            page.set(page.get() + 1); update();
        }
        _ => d.close(),
    }); }
    let u = ui.clone();
    let build2 = build.clone();
    save_b.connect_clicked(move |b| {
        let nb = build2();
        if nb.dest.is_empty() { return; }
        config::update(|c| c.maintenance.backup = Some(nb.clone()));
        let (u2, nb2) = (u.clone(), nb.clone());
        bg(move || save(Some(nb2)), move |r| { if let Err(e) = r { util::error("The backup schedule could not be saved.", &e); } u2.cp.open("backup"); });
        if let Some(w) = b.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()) { w.close(); }
        // run it right away like Win7's "Save settings and run backup"
        let u4 = u.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || back_up_now(&u4));
    });
    d.show_all();
    update();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeshift_list() {
        let out = "Device : /dev/sda2\nUUID   : x\nPath   : /run/timeshift/backup\nMode   : RSYNC\nStatus : OK\n2 snapshots, 100 GB free\n\nNum     Name                 Tags  Description\n------------------------------------------------------------------------------\n0    >  2024-05-01_12-00-01  O     before kernel\n1    >  2024-05-02_02-00-01  D\n";
        let s = parse_timeshift(out);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].desc, "before kernel");
        assert_eq!(s[1].kind, "Manual");
        assert_eq!(s[0].kind, "System");
    }
    #[test]
    fn snapper_list() {
        let out = r#"{"root":[{"number":0,"type":"single","date":"","description":"current","cleanup":""},{"number":5,"type":"pre","date":"2024-05-01 12:00:01","description":"pacman -Syu","cleanup":"number"},{"number":9,"type":"single","date":"2024-05-03 10:00:00","description":"","cleanup":"timeline"}]}"#;
        let s = parse_snapper(out);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, "System");
        assert_eq!(s[1].kind, "Install");
    }
    #[test]
    fn schedule() {
        let b = Backup { dest: "/mnt/b".into(), schedule: "weekly".into(), day: 0, hour: 19, folders: vec![] };
        assert_eq!(schedule_text(&b), "Every Sunday at 7:00 PM");
        assert!(next_run(&b).is_some());
        let a = rsync_args(&b, false);
        assert!(a.last().unwrap() == "/mnt/b/");
        let (s, t) = unit_files(&b);
        assert!(s.contains("ExecStart=/usr/bin/rsync -a"));
        assert!(t.contains("OnCalendar=Sun *-*-* 19:00:00"));
    }
}
