//! Task Scheduler (taskschd.msc): systemd timers (system and user) and the user's crontab.
//! Task Scheduler Library lists every task with Win7's columns; the folders System, User and
//! Cron narrow it down. Run / End / Enable / Disable go through systemctl (pkexec for system
//! timers), Create Basic Task writes a user timer.
use gtk::prelude::*;
use gtk::{gdk_pixbuf::Pixbuf, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::mmc::{self, act, act_off, Mmc};
use super::*;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Kind { #[default] System, User, Cron }

#[derive(Clone, Default, Debug)]
pub struct Task {
    pub name: String,
    pub kind: Kind,
    pub timer: String,
    pub service: String,
    pub desc: String,
    pub triggers: Vec<String>,
    pub raw: Vec<String>,
    pub next: i64,
    pub last: i64,
    pub result: String,
    pub status: String,
    pub author: String,
    pub created: i64,
    pub exec: String,
    pub path: String,
    pub persistent: bool,
    pub wake: bool,
    pub user: String,
    pub cron_line: String,
    pub conditions: Vec<String>,
    pub last_end: i64,
}

impl Task {
    fn location(&self) -> &'static str { match self.kind { Kind::System => "\\System", Kind::User => "\\User", Kind::Cron => "\\Cron" } }
    fn enabled(&self) -> bool { self.status != "Disabled" }
    fn ctl(&self) -> Vec<&'static str> { if self.kind == Kind::User { vec!["systemctl", "--user"] } else { vec!["systemctl"] } }
}

/// Parse `systemctl show a b c` output into one map per unit (blocks separated by blank lines).
pub fn parse_show(out: &str) -> Vec<HashMap<String, Vec<String>>> {
    let mut v = vec![];
    for block in out.split("\n\n") {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for l in block.lines() { if let Some((k, val)) = l.split_once('=') { m.entry(k.to_string()).or_default().push(val.to_string()); } }
        if !m.is_empty() { v.push(m); }
    }
    v
}

fn first(m: &HashMap<String, Vec<String>>, k: &str) -> String { m.get(k).and_then(|v| v.first()).cloned().unwrap_or_default() }

/// "@1730592000" (systemctl --timestamp=unix) or "Mon 2026-11-03 00:00:00 UTC" → seconds.
pub fn parse_ts(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() || s == "n/a" || s == "0" { return 0; }
    if let Some(n) = s.strip_prefix('@') { return n.parse().unwrap_or(0); }
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 3 {
        if let Ok(d) = chrono::NaiveDateTime::parse_from_str(&format!("{} {}", parts[1], parts[2]), "%Y-%m-%d %H:%M:%S") {
            use chrono::TimeZone;
            let tz = parts.get(3).copied().unwrap_or("");
            if tz == "UTC" { return d.and_utc().timestamp(); }
            if let chrono::LocalResult::Single(t) = chrono::Local.from_local_datetime(&d) { return t.timestamp(); }
        }
    }
    0
}

fn clock(h: u32, m: u32) -> String { format!("{}:{:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, m, if h < 12 { "AM" } else { "PM" }) }

const DOW: [(&str, &str); 7] = [("Mon", "Monday"), ("Tue", "Tuesday"), ("Wed", "Wednesday"), ("Thu", "Thursday"), ("Fri", "Friday"), ("Sat", "Saturday"), ("Sun", "Sunday")];

/// A systemd OnCalendar expression (normalized or not) in Win7 trigger words.
pub fn calendar_words(expr: &str) -> String {
    let e = expr.trim();
    let named = match e { "minutely" => "*-*-* *:*:00", "hourly" => "*-*-* *:00:00", "daily" => "*-*-* 00:00:00", "weekly" => "Mon *-*-* 00:00:00",
        "monthly" => "*-*-01 00:00:00", "yearly" | "annually" => "*-01-01 00:00:00", _ => e };
    let parts: Vec<&str> = named.split_whitespace().collect();
    let (dow, date, time) = match parts.len() { 3 => (Some(parts[0]), parts[1], parts[2]), 2 => if parts[0].contains(':') { (None, "*-*-*", parts[0]) } else { (None, parts[0], parts[1]) }, 1 => (None, parts[0], "00:00:00"), _ => return format!("On a schedule: {e}") };
    let t: Vec<&str> = time.split(':').collect();
    let (h, m) = (t.first().copied().unwrap_or("0"), t.get(1).copied().unwrap_or("0"));
    if h == "*" && m == "*" { return "Every minute".into(); }
    if h == "*" { return if m == "00" || m == "0" { "Every hour".into() } else { format!("At {m} minutes past every hour") }; }
    if h.contains('/') || m.contains('/') || h.contains(',') { return format!("On a schedule: {e}"); }
    let (Ok(hh), Ok(mm)) = (h.parse::<u32>(), m.parse::<u32>()) else { return format!("On a schedule: {e}") };
    let at = clock(hh, mm);
    let d: Vec<&str> = date.split('-').collect();
    let (y, mo, dd) = match d.len() { 3 => (d[0], d[1], d[2]), 2 => ("*", d[0], d[1]), _ => return format!("On a schedule: {e}") };
    if let Some(dw) = dow {
        let days: Vec<String> = dw.split(',').flat_map(|p| {
            if let Some((a, b)) = p.split_once("..") {
                let (ia, ib) = (DOW.iter().position(|x| x.0 == a).unwrap_or(0), DOW.iter().position(|x| x.0 == b).unwrap_or(6));
                (ia..=ib).map(|i| DOW[i].1.to_string()).collect::<Vec<_>>()
            } else { vec![DOW.iter().find(|x| x.0 == p).map(|x| x.1.to_string()).unwrap_or_else(|| p.to_string())] }
        }).collect();
        return format!("At {at} every {} of every week", days.join(", "));
    }
    if y == "*" && mo == "*" && dd == "*" { return format!("At {at} every day"); }
    if y == "*" && mo == "*" { return format!("At {at} on day {} of every month", dd.trim_start_matches('0')); }
    if y == "*" {
        let months = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        if let (Ok(mi), Ok(di)) = (mo.parse::<usize>(), dd.parse::<u32>()) { if (1..=12).contains(&mi) { return format!("At {at} on {} {di} every year", months[mi - 1]); } }
    }
    if let (Ok(_), Ok(_), Ok(_)) = (y.parse::<u32>(), mo.parse::<u32>(), dd.parse::<u32>()) { return format!("At {at} on {}/{}/{y}", mo.trim_start_matches('0'), dd.trim_start_matches('0')); }
    format!("On a schedule: {e}")
}

/// `{ OnBootUSec=15min ; next_elapse=… }` → words.
pub fn monotonic_words(k: &str, v: &str) -> String {
    match k {
        "OnBootUSec" => if v == "0" { "At system startup".into() } else { format!("At system startup - delay {v}") },
        "OnStartupUSec" => if v == "0" { "At log on".into() } else { format!("At log on - delay {v}") },
        "OnUnitActiveUSec" => format!("Repeat every {v}"),
        "OnUnitInactiveUSec" => format!("Repeat {v} after the last run finished"),
        "OnActiveUSec" => format!("{v} after the timer was activated"),
        _ => format!("{k} {v}"),
    }
}

/// A crontab schedule in words (the common shapes; the rest verbatim).
pub fn cron_words(f: &[&str]) -> String {
    if f.len() == 1 {
        return match f[0] { "@reboot" => "At system startup".into(), "@daily" | "@midnight" => "At 12:00 AM every day".into(), "@hourly" => "Every hour".into(),
            "@weekly" => "At 12:00 AM every Sunday of every week".into(), "@monthly" => "At 12:00 AM on day 1 of every month".into(), "@yearly" | "@annually" => "At 12:00 AM on January 1 every year".into(), s => s.into() };
    }
    if f.len() < 5 { return f.join(" "); }
    let (m, h, dom, mon, dow) = (f[0], f[1], f[2], f[3], f[4]);
    if let Some(n) = m.strip_prefix("*/") { if h == "*" && dom == "*" && mon == "*" && dow == "*" { return format!("Every {n} minutes"); } }
    if let (Ok(mm), Ok(hh)) = (m.parse::<u32>(), h.parse::<u32>()) {
        let at = clock(hh, mm);
        if dom == "*" && mon == "*" && dow == "*" { return format!("At {at} every day"); }
        if dom == "*" && mon == "*" {
            let names = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
            let days: Vec<&str> = dow.split(',').filter_map(|d| d.parse::<usize>().ok().and_then(|i| names.get(i).copied())).collect();
            if !days.is_empty() { return format!("At {at} every {} of every week", days.join(", ")); }
        }
        if mon == "*" && dow == "*" { if let Ok(d) = dom.parse::<u32>() { return format!("At {at} on day {d} of every month"); } }
    }
    if h == "*" && dom == "*" && mon == "*" && dow == "*" { if let Ok(mm) = m.parse::<u32>() { return if mm == 0 { "Every hour".into() } else { format!("At {mm} minutes past every hour") }; } }
    format!("Custom schedule: {}", f[..5].join(" "))
}

fn result_text(t: &Task, svc: &HashMap<String, Vec<String>>) -> String {
    let active = first(svc, "ActiveState");
    if active == "active" || active == "activating" { return "The task is currently running. (0x41301)".into(); }
    if t.last == 0 && parse_ts(&first(svc, "ExecMainStartTimestamp")) == 0 { return "The task has not yet run. (0x41303)".into(); }
    let res = first(svc, "Result");
    let code: i64 = first(svc, "ExecMainStatus").parse().unwrap_or(0);
    if res == "success" || res.is_empty() { "The operation completed successfully. (0x0)".into() }
    else if res == "exit-code" { format!("(0x{code:X})") }
    else { format!("{res} (0x1)") }
}

/// Everything, from a worker thread.
pub fn load_all() -> Vec<Task> {
    let mut tasks = vec![];
    let me = glib::user_name().to_string_lossy().into_owned();
    for kind in [Kind::System, Kind::User] {
        let base: Vec<&str> = if kind == Kind::User { vec!["systemctl", "--user"] } else { vec!["systemctl"] };
        let mut a = base.clone();
        a.extend(["list-unit-files", "--type=timer", "--no-legend", "--plain", "--no-pager"]);
        let files = sh(&a);
        let mut names: Vec<String> = files.lines().filter_map(|l| l.split_whitespace().next()).filter(|n| n.ends_with(".timer") && !n.contains('@')).map(|s| s.to_string()).collect();
        let mut a = base.clone();
        a.extend(["list-units", "--type=timer", "--all", "--no-legend", "--plain", "--no-pager"]);
        for l in sh(&a).lines() { if let Some(n) = l.split_whitespace().next() { if n.ends_with(".timer") && !names.iter().any(|x| x == n) { names.push(n.to_string()); } } }
        if names.is_empty() { continue; }
        let mut a = base.clone();
        a.extend(["show", "--timestamp=unix", "-p", "Id,Description,Unit,TimersCalendar,TimersMonotonic,NextElapseUSecRealtime,LastTriggerUSec,UnitFileState,ActiveState,Persistent,WakeSystem,FragmentPath"]);
        a.extend(names.iter().map(|s| s.as_str()));
        let mut shown = parse_show(&sh(&a));
        if shown.len() != names.len() {
            // old systemd without --timestamp=unix
            let mut a = base.clone();
            a.extend(["show", "-p", "Id,Description,Unit,TimersCalendar,TimersMonotonic,NextElapseUSecRealtime,LastTriggerUSec,UnitFileState,ActiveState,Persistent,WakeSystem,FragmentPath"]);
            a.extend(names.iter().map(|s| s.as_str()));
            shown = parse_show(&sh(&a));
        }
        let services: Vec<String> = shown.iter().map(|m| { let u = first(m, "Unit"); if u.is_empty() { first(m, "Id").replace(".timer", ".service") } else { u } }).collect();
        let mut a = base.clone();
        a.extend(["show", "--timestamp=unix", "-p", "Id,Description,ExecStart,Result,ExecMainStatus,ActiveState,ExecMainStartTimestamp,ExecMainExitTimestamp,User,ConditionACPower,ConditionPathExists,ConditionVirtualization,FragmentPath"]);
        a.extend(services.iter().map(|s| s.as_str()));
        let svcs = parse_show(&sh(&a));
        for (i, m) in shown.iter().enumerate() {
            let id = first(m, "Id");
            if id.is_empty() { continue; }
            let svc = svcs.iter().find(|s| first(s, "Id") == services[i]).cloned().unwrap_or_default();
            let mut t = Task { kind, timer: id.clone(), service: services[i].clone(), name: id.trim_end_matches(".timer").to_string(), ..Default::default() };
            t.desc = { let d = first(&svc, "Description"); if d.is_empty() { first(m, "Description") } else { d } };
            for c in m.get("TimersCalendar").into_iter().flatten() {
                let inner = c.trim().trim_start_matches('{').trim_end_matches('}');
                if let Some(expr) = inner.split(';').next().and_then(|p| p.trim().strip_prefix("OnCalendar=")) { t.raw.push(format!("OnCalendar={expr}")); t.triggers.push(calendar_words(expr)); }
            }
            for c in m.get("TimersMonotonic").into_iter().flatten() {
                let inner = c.trim().trim_start_matches('{').trim_end_matches('}');
                if let Some((k, v)) = inner.split(';').next().and_then(|p| p.trim().split_once('=')) { t.raw.push(format!("{k}={v}")); t.triggers.push(monotonic_words(k, v)); }
            }
            t.next = parse_ts(&first(m, "NextElapseUSecRealtime"));
            t.last = parse_ts(&first(m, "LastTriggerUSec"));
            t.last_end = parse_ts(&first(&svc, "ExecMainExitTimestamp"));
            let state = first(m, "UnitFileState");
            let sactive = first(&svc, "ActiveState");
            t.status = if sactive == "active" || sactive == "activating" { "Running".into() } else if state == "disabled" || state == "masked" || (first(m, "ActiveState") != "active" && state != "static") { "Disabled".into() } else { "Ready".into() };
            t.result = result_text(&t, &svc);
            t.path = first(m, "FragmentPath");
            t.created = mtime(std::path::Path::new(&t.path)).unwrap_or(0);
            t.persistent = first(m, "Persistent") == "yes";
            t.wake = first(m, "WakeSystem") == "yes";
            t.exec = first(&svc, "ExecStart").split(" ; ").find_map(|p| p.trim().strip_prefix("argv[]=")).map(|s| s.to_string())
                .or_else(|| first(&svc, "ExecStart").split(" ; ").find_map(|p| p.trim().strip_prefix("{ path=").map(|s| s.to_string()))).unwrap_or_default();
            let u = first(&svc, "User");
            t.user = if kind == Kind::User { me.clone() } else if u.is_empty() { "SYSTEM".into() } else { u };
            t.author = if kind == Kind::User { me.clone() } else if t.path.starts_with("/etc/") { "Administrator".into() } else { String::new() };
            for (k, label) in [("ConditionACPower", "Start the task only if the computer is on AC power"), ("ConditionPathExists", "Start only if this path exists:"), ("ConditionVirtualization", "Start only on this kind of machine:")] {
                let v = first(&svc, k);
                if !v.is_empty() && v != "n/a" { t.conditions.push(format!("{label} {v}")); }
            }
            tasks.push(t);
        }
    }
    // package that installed each system timer = its "Author"
    let paths: Vec<String> = tasks.iter().filter(|t| t.author.is_empty() && !t.path.is_empty()).map(|t| t.path.clone()).collect();
    if !paths.is_empty() && util::which("pacman") {
        let mut a = vec!["pacman", "-Qo"];
        a.extend(paths.iter().map(|s| s.as_str()));
        let out = std::process::Command::new("pacman").args(&a[1..]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        for l in out.lines() {
            if let Some((p, rest)) = l.split_once(" is owned by ") {
                let pkg = rest.split_whitespace().next().unwrap_or("").to_string();
                for t in tasks.iter_mut().filter(|t| t.path == p) { t.author = pkg.clone(); }
            }
        }
    }
    for t in tasks.iter_mut().filter(|t| t.author.is_empty()) { t.author = "Linux".into(); }
    // the user's crontab
    for (i, l) in sh(&["crontab", "-l"]).lines().enumerate() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') || l.contains('=') && !l.starts_with('@') && l.split_whitespace().count() < 6 { continue; }
        let f: Vec<&str> = l.split_whitespace().collect();
        let n = if f[0].starts_with('@') { 1 } else { 5 };
        if f.len() <= n { continue; }
        let cmd = f[n..].join(" ");
        let prog = std::path::Path::new(f[n]).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        tasks.push(Task { name: format!("cron-{} ({prog})", i + 1), kind: Kind::Cron, triggers: vec![cron_words(&f[..n])], raw: vec![f[..n].join(" ")], exec: cmd, status: "Ready".into(),
            result: "".into(), author: me.clone(), user: me.clone(), cron_line: l.to_string(), desc: "Runs from your crontab".into(), ..Default::default() });
    }
    tasks.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    tasks
}

struct State {
    m: Rc<Mmc>,
    tasks: RefCell<Vec<Task>>,
    folder: RefCell<String>,
    store: gtk::ListStore,
    view: gtk::TreeView,
    details: gtk::Box,
    holder: gtk::Box,
    head: gtk::Label,
    title: gtk::Label,
    loaded: std::cell::Cell<bool>,
}

pub fn attach(m: &Rc<Mmc>, parent: Option<&gtk::TreeIter>) {
    let root_name = if parent.is_some() { "Task Scheduler" } else { "Task Scheduler (Local)" };
    let root = m.add(parent, "ts", root_name, &["aerode-taskschd", "x-office-calendar"]);
    let lib = m.add(Some(&root), "ts:lib", "Task Scheduler Library", &["folder"]);
    for f in ["System", "User", "Cron"] { m.add(Some(&lib), &format!("ts:lib:{f}"), f, &["folder"]); }
    let st: Rc<RefCell<Option<Rc<State>>>> = Rc::new(RefCell::new(None));
    m.handle("ts", move |m, id| {
        let existing = st.borrow().clone();
        let s = match existing { Some(s) => s, None => { let s = new_state(m); *st.borrow_mut() = Some(s.clone()); s } };
        if id == "ts" { overview(&s); } else { *s.folder.borrow_mut() = id.trim_start_matches("ts:lib").trim_start_matches(':').to_string(); show_folder(&s); }
    });
}

pub fn window(app: &gtk::Application) -> gtk::Window {
    let m = Mmc::new(app, "Task Scheduler", &["aerode-taskschd", "x-office-calendar"], true, true);
    attach(&m, None);
    m.show("ts");
    m.win.clone()
}

fn new_state(m: &Rc<Mmc>) -> Rc<State> {
    // name, status, triggers, next, last, result, author, created, next-sort, last-sort, index
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::I64, glib::Type::I64, glib::Type::U32]);
    let view = gtk::TreeView::with_model(&store);
    icon_column(&view, "Name", 0, 1, 190);
    columns(&view, &[("Status", 2, 70), ("Triggers", 3, 230), ("Next Run Time", 4, 150), ("Last Run Time", 5, 150), ("Last Run Result", 6, 230), ("Author", 7, 110), ("Created", 8, 0)]);
    for (i, c) in view.columns().iter().enumerate() { if i == 3 { c.set_sort_column_id(9); } if i == 4 { c.set_sort_column_id(10); } }
    let (h, head) = mmc::head("", "");
    let title = h.children()[0].clone().downcast::<gtk::Label>().unwrap();
    let details = vbox(0);
    let top = vbox(0);
    top.pack_start(&h, false, false, 0);
    top.pack_start(&scrolled(&view), true, true, 0);
    let paned = gtk::Paned::new(gtk::Orientation::Vertical);
    paned.pack1(&top, true, false);
    paned.pack2(&details, false, false);
    paned.set_position(260);
    let holder = vbox(0);
    holder.pack_start(&paned, true, true, 0);
    let s = Rc::new(State { m: m.clone(), tasks: RefCell::new(vec![]), folder: RefCell::new(String::new()), store, view, details, holder, head, title, loaded: std::cell::Cell::new(false) });
    { let w = Rc::downgrade(&s); s.view.selection().connect_changed(move |_| if let Some(s) = w.upgrade() { show_details(&s); actions(&s); }); }
    { let w = Rc::downgrade(&s); s.view.connect_row_activated(move |_, _, _| if let Some(s) = w.upgrade() { if let Some(t) = selected(&s) { properties(&s, &t); } }); }
    { let w = Rc::downgrade(&s); s.view.connect_button_press_event(move |tv, ev| {
        if ev.button() == 3 { if let (Some(s), Some((Some(p), _, _, _))) = (w.upgrade(), tv.path_at_pos(ev.position().0 as i32, ev.position().1 as i32)) { tv.selection().select_path(&p); s.m.popup_actions(ev); return glib::Propagation::Stop; } }
        glib::Propagation::Proceed
    }); }
    s
}

fn reload(s: &Rc<State>, then: impl FnOnce(&Rc<State>) + 'static) {
    s.head.set_text("Loading tasks…");
    let s2 = s.clone();
    bg(load_all, move |tasks| { *s2.tasks.borrow_mut() = tasks; s2.loaded.set(true); then(&s2); });
}

fn visible(s: &State) -> Vec<(usize, Task)> {
    let f = s.folder.borrow().clone();
    s.tasks.borrow().iter().cloned().enumerate().filter(|(_, t)| match f.as_str() { "System" => t.kind == Kind::System, "User" => t.kind == Kind::User, "Cron" => t.kind == Kind::Cron, _ => true }).collect()
}

fn show_folder(s: &Rc<State>) {
    s.m.set_center(&s.holder);
    if !s.loaded.get() { reload(s, |s| fill(s)); } else { fill(s); }
    actions(s);
}

fn fill(s: &Rc<State>) {
    s.store.clear();
    let pb = pix(&["aerode-task", "x-office-calendar"], 16);
    let list = visible(s);
    for (i, t) in &list {
        let next = if t.next > 0 && t.enabled() { when(t.next) } else { String::new() };
        let last = if t.last > 0 { when(t.last) } else if t.kind == Kind::Cron { String::new() } else { "11/30/1999 12:00:00 AM".into() };
        s.store.insert_with_values(None, &[(0, &pb), (1, &t.name), (2, &t.status), (3, &t.triggers.join("; ")), (4, &next), (5, &last), (6, &t.result), (7, &t.author), (8, &if t.created > 0 { when(t.created) } else { String::new() }), (9, &t.next), (10, &t.last), (11, &(*i as u32))]);
    }
    let f = s.folder.borrow().clone();
    s.head.set_text(&format!("{} task{}", list.len(), if list.len() == 1 { "" } else { "s" }));
    s.title.set_text(if f.is_empty() { "Task Scheduler Library" } else { &f });
    show_details(s);
    actions(s);
}

fn selected(s: &State) -> Option<Task> {
    let (m, it) = s.view.selection().selected()?;
    let i: u32 = m.value(&it, 11).get().ok()?;
    s.tasks.borrow().get(i as usize).cloned()
}

/// General / Triggers / Actions / Conditions / Settings / History.
fn task_tabs(t: &Task, history: bool) -> gtk::Notebook {
    let nb = gtk::Notebook::new();
    // General
    let g = gtk::Grid::new();
    g.set_border_width(10);
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    let rows = [("Name:", t.name.clone()), ("Location:", t.location().to_string()), ("Author:", t.author.clone()), ("Description:", t.desc.clone())];
    for (i, (k, v)) in rows.iter().enumerate() {
        let a = gtk::Label::new(Some(k)); a.set_xalign(0.0); a.set_yalign(0.0); g.attach(&a, 0, i as i32, 1, 1);
        let b = gtk::Label::new(Some(v)); b.set_xalign(0.0); b.set_selectable(true); b.set_line_wrap(true); b.set_hexpand(true); g.attach(&b, 1, i as i32, 1, 1);
    }
    let sec = label("Security options", "");
    sec.set_margin_top(8);
    g.attach(&sec, 0, 4, 2, 1);
    g.attach(&label(&format!("When running the task, use the following user account:\n    {}", if t.user == "SYSTEM" { "SYSTEM (root)".to_string() } else { t.user.clone() }), ""), 0, 5, 2, 1);
    let r1 = gtk::RadioButton::with_label("Run only when user is logged on");
    let r2 = gtk::RadioButton::with_label_from_widget(&r1, "Run whether user is logged on or not");
    if t.kind == Kind::System || t.kind == Kind::Cron { r2.set_active(true); }
    r1.set_sensitive(false); r2.set_sensitive(false);
    g.attach(&r1, 0, 6, 2, 1);
    g.attach(&r2, 0, 7, 2, 1);
    let hp = gtk::CheckButton::with_label("Run with highest privileges");
    hp.set_active(t.user == "SYSTEM");
    hp.set_sensitive(false);
    g.attach(&hp, 0, 8, 2, 1);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    // Triggers
    let ts = gtk::ListStore::new(&[glib::Type::STRING; 3]);
    for (w, r) in t.triggers.iter().zip(t.raw.iter()) {
        let kind = if r.starts_with("OnCalendar") || t.kind == Kind::Cron { if w.contains("every day") { "Daily" } else if w.contains("every week") || w.contains("of every week") { "Weekly" } else if w.contains("month") { "Monthly" } else if w.contains("startup") { "At startup" } else { "On a schedule" } }
            else if r.starts_with("OnBoot") { "At startup" } else if r.starts_with("OnStartup") { "At log on" } else { "Repeating" };
        ts.insert_with_values(None, &[(0, &kind), (1, &format!("{w}   ({r})")), (2, &if t.enabled() { "Enabled" } else { "Disabled" })]);
    }
    let tv = gtk::TreeView::with_model(&ts);
    columns(&tv, &[("Trigger", 0, 110), ("Details", 1, 380), ("Status", 2, 0)]);
    let b = vbox(6);
    b.set_border_width(8);
    b.pack_start(&label("When you create a task, you can specify the conditions that will trigger the task.", ""), false, false, 0);
    b.pack_start(&scrolled(&tv), true, true, 0);
    nb.append_page(&b, Some(&gtk::Label::new(Some("Triggers"))));
    // Actions
    let as_ = gtk::ListStore::new(&[glib::Type::STRING; 2]);
    as_.insert_with_values(None, &[(0, &"Start a program"), (1, &if t.exec.is_empty() { t.service.clone() } else { t.exec.clone() })]);
    let atv = gtk::TreeView::with_model(&as_);
    columns(&atv, &[("Action", 0, 130), ("Details", 1, 0)]);
    let b = vbox(6);
    b.set_border_width(8);
    b.pack_start(&label("When you create a task, you must specify the action that will occur when your task starts.", ""), false, false, 0);
    b.pack_start(&scrolled(&atv), true, true, 0);
    nb.append_page(&b, Some(&gtk::Label::new(Some("Actions"))));
    // Conditions
    let b = vbox(6);
    b.set_border_width(8);
    b.pack_start(&label("Specify the conditions that, along with the trigger, determine whether the task should run. The task will not run if any condition specified here is not true.", ""), false, false, 0);
    for (lbl, on) in [("Start the task only if the computer is on AC power", t.conditions.iter().any(|c| c.starts_with("Start the task only if the computer is on AC"))), ("Wake the computer to run this task", t.wake)] {
        let c = gtk::CheckButton::with_label(lbl); c.set_active(on); c.set_sensitive(false); b.pack_start(&c, false, false, 0);
    }
    for c in t.conditions.iter().filter(|c| !c.starts_with("Start the task only if the computer is on AC")) { b.pack_start(&label(c, ""), false, false, 0); }
    nb.append_page(&b, Some(&gtk::Label::new(Some("Conditions"))));
    // Settings
    let b = vbox(6);
    b.set_border_width(8);
    b.pack_start(&label("Specify additional settings that affect the behavior of the task.", ""), false, false, 0);
    for (lbl, on) in [("Allow task to be run on demand", true), ("Run task as soon as possible after a scheduled start is missed", t.persistent), ("If the running task does not end when requested, force it to stop", true)] {
        let c = gtk::CheckButton::with_label(lbl); c.set_active(on); c.set_sensitive(false); b.pack_start(&c, false, false, 0);
    }
    nb.append_page(&b, Some(&gtk::Label::new(Some("Settings"))));
    // History (loaded when the tab is opened)
    if history && t.kind != Kind::Cron {
        let hs = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
        let htv = gtk::TreeView::with_model(&hs);
        icon_column(&htv, "Level", 0, 1, 100);
        columns(&htv, &[("Date and Time", 2, 160), ("Message", 3, 0)]);
        let b = vbox(4);
        b.set_border_width(4);
        let n = label("", "dim");
        b.pack_start(&n, false, false, 0);
        b.pack_start(&scrolled(&htv), true, true, 0);
        let page = nb.append_page(&b, Some(&gtk::Label::new(Some("History"))));
        let (svc, user) = (t.service.clone(), t.kind == Kind::User);
        let done = Rc::new(std::cell::Cell::new(false));
        nb.connect_switch_page(move |_, _, p| {
            if p != page || done.replace(true) { return; }
            n.set_text("Loading…");
            let (svc, hs, n) = (svc.clone(), hs.clone(), n.clone());
            bg(move || sh(&["journalctl", if user { "--user-unit" } else { "--unit" }, &svc, "-o", "json", "-r", "-n", "100", "--no-pager", "-q"]), move |out| {
                let mut k = 0;
                for e in out.lines().filter_map(super::eventvwr::parse_json) {
                    let icon: &[&str] = match e.level { 0 | 1 => &["dialog-error"], 2 => &["dialog-warning"], _ => &["dialog-information"] };
                    hs.insert_with_values(None, &[(0, &pix(icon, 16)), (1, &["Critical", "Error", "Warning", "Information", "Verbose"][e.level]), (2, &when(e.ts / 1_000_000)), (3, &e.message)]);
                    k += 1;
                }
                n.set_text(&format!("Number of events: {k}"));
            });
        });
    }
    nb
}

fn show_details(s: &Rc<State>) {
    for c in s.details.children() { s.details.remove(&c); }
    if let Some(t) = selected(s) { s.details.pack_start(&task_tabs(&t, true), true, true, 0); }
    s.details.show_all();
}

fn actions(s: &Rc<State>) {
    let f = s.folder.borrow().clone();
    let title = if f.is_empty() { "Task Scheduler Library".to_string() } else { f };
    let w = Rc::downgrade(s);
    let g = move || w.upgrade();
    let (a, b, c) = (g.clone(), g.clone(), g.clone());
    let mut groups = vec![(title, vec![
        act("Create Basic Task...", &["aerode-task", "x-office-calendar"], move || if let Some(s) = a() { wizard(&s) }),
        act_off("Create Task..."), act_off("Import Task..."),
        act("Display All Running Tasks", NONE, move || if let Some(s) = b() { running(&s) }),
        act_off("Enable All Tasks History"), act_off("New Folder..."), act_off("View ▸"),
        act("Refresh", mmc::I_REFRESH, move || if let Some(s) = c() { reload(&s, |s| fill(s)); }),
        act_off("Help"),
    ])];
    if let Some(t) = selected(s) {
        let mk = |f: fn(&Rc<State>, &Task)| { let (g, t) = (g.clone(), t.clone()); move || if let Some(s) = g() { f(&s, &t) } };
        let toggle = if t.enabled() { act("Disable", NONE, mk(|s, t| enable(s, t, false))) } else { act("Enable", NONE, mk(|s, t| enable(s, t, true))) };
        let end = if t.status == "Running" { act("End", mmc::I_STOP, mk(end)) } else { act_off("End") };
        let deletable = t.kind != Kind::System;
        groups.push(("Selected Item".into(), vec![
            act("Run", mmc::I_RUN, mk(run_task)), end, toggle,
            act("Export...", mmc::I_SAVE, mk(export)),
            act("Properties", mmc::I_PROPS, mk(|s, t| properties(s, t))),
            if deletable { act("Delete", mmc::I_DELETE, mk(delete)) } else { act_off("Delete") },
            act_off("Help"),
        ]));
    }
    s.m.set_actions(groups);
}
const NONE: &[&str] = &[];

fn overview(s: &Rc<State>) {
    let v = vbox(0);
    v.pack_start(&mmc::head("Task Scheduler Summary", "").0, false, false, 0);
    let inner = vbox(10);
    inner.set_border_width(8);
    let d = vbox(4);
    d.style_context().add_class("mmc-desc");
    d.pack_start(&label("You can use Task Scheduler to create and manage common tasks that your computer will carry out automatically at the times you specify. To begin, click a command in the Action menu.", ""), false, false, 0);
    d.pack_start(&label("Tasks are stored in folders in the Task Scheduler Library. To view or perform an operation on an individual task, select the task in the Task Scheduler Library and click on a command in the Action menu.", ""), false, false, 0);
    inner.pack_start(&mmc::group("Overview of Task Scheduler", &d), false, false, 0);
    // Task Status
    let st_store = gtk::ListStore::new(&[glib::Type::STRING; 5]);
    let stv = gtk::TreeView::with_model(&st_store);
    columns(&stv, &[("Task Name", 0, 200), ("Run Result", 1, 230), ("Run Start", 2, 150), ("Run End", 3, 150), ("Triggered By", 4, 0)]);
    let sum1 = label("Loading…", "");
    let b1 = vbox(4);
    let row = hbox(8);
    row.pack_start(&label("Status of tasks that have started in the following time period:", ""), false, false, 0);
    let period = gtk::ComboBoxText::new();
    for (id, l) in [("3600", "Last hour"), ("86400", "Last 24 hours"), ("604800", "Last 7 days"), ("2592000", "Last 30 days")] { period.append(Some(id), l); }
    period.set_active_id(Some("86400"));
    row.pack_start(&period, false, false, 0);
    b1.pack_start(&row, false, false, 0);
    b1.pack_start(&sum1, false, false, 0);
    let sw1 = scrolled(&stv);
    sw1.set_min_content_height(150);
    b1.pack_start(&sw1, true, true, 0);
    inner.pack_start(&mmc::group("Task Status", &b1), true, true, 0);
    // Active Tasks
    let at = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::I64]);
    let atv = gtk::TreeView::with_model(&at);
    columns(&atv, &[("Task Name", 0, 200), ("Next Run Time", 1, 160), ("Triggers", 2, 260), ("Location", 3, 0)]);
    atv.columns()[1].set_sort_column_id(4);
    let sum2 = label("", "");
    let b2 = vbox(4);
    b2.pack_start(&label("Active tasks are tasks that are currently enabled and have not expired.", ""), false, false, 0);
    b2.pack_start(&sum2, false, false, 0);
    let sw2 = scrolled(&atv);
    sw2.set_min_content_height(150);
    b2.pack_start(&sw2, true, true, 0);
    inner.pack_start(&mmc::group("Active Tasks", &b2), true, true, 0);
    let outer = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    outer.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    outer.add(&inner);
    v.pack_start(&outer, true, true, 0);
    s.m.set_center(&v);
    let fill_ov: Rc<dyn Fn(&Rc<State>)> = { let (st_store, sum1, at, sum2, period) = (st_store.clone(), sum1.clone(), at.clone(), sum2.clone(), period.clone()); Rc::new(move |s: &Rc<State>| {
        st_store.clear(); at.clear();
        let span: i64 = period.active_id().and_then(|p| p.parse().ok()).unwrap_or(86400);
        let t0 = now() - span;
        let tasks = s.tasks.borrow();
        let (mut run, mut ok, mut failed) = (0, 0, 0);
        let recent: Vec<&Task> = tasks.iter().filter(|t| t.last >= t0).collect();
        for t in &recent {
            if t.status == "Running" { run += 1 } else if t.result.contains("(0x0)") { ok += 1 } else { failed += 1 }
            let by = t.triggers.first().cloned().unwrap_or_default();
            st_store.insert_with_values(None, &[(0, &format!("{} (last run)", t.name)), (1, &t.result), (2, &when(t.last)), (3, &if t.last_end >= t.last && t.last_end > 0 { when(t.last_end) } else { String::new() }), (4, &by)]);
        }
        sum1.set_markup(&format!("<b>Summary:</b> {} total - {run} running, {ok} succeeded, 0 stopped, {failed} failed", recent.len()));
        let active: Vec<&Task> = tasks.iter().filter(|t| t.enabled() && (t.next > 0 || t.kind == Kind::Cron)).collect();
        for t in &active { at.insert_with_values(None, &[(0, &t.name), (1, &if t.next > 0 { when(t.next) } else { String::new() }), (2, &t.triggers.join("; ")), (3, &t.location()), (4, &t.next)]); }
        at.set_sort_column_id(gtk::SortColumn::Index(4), gtk::SortType::Ascending);
        sum2.set_markup(&format!("<b>Summary:</b> {} total", active.len()));
    }) };
    { let (w, f) = (Rc::downgrade(s), fill_ov.clone()); period.connect_changed(move |_| if let Some(s) = w.upgrade() { f(&s) }); }
    if s.loaded.get() { fill_ov(s); } else { reload(s, move |s| fill_ov(s)); }
    let w = Rc::downgrade(s);
    let (a, b, c) = (w.clone(), w.clone(), w.clone());
    s.m.set_actions(vec![("Task Scheduler (Local)".into(), vec![
        act_off("Connect to Another Computer..."),
        act("Create Basic Task...", &["aerode-task", "x-office-calendar"], move || if let Some(s) = a.upgrade() { wizard(&s) }),
        act_off("Create Task..."), act_off("Import Task..."),
        act("Display All Running Tasks", NONE, move || if let Some(s) = b.upgrade() { running(&s) }),
        act_off("Enable All Tasks History"), act_off("AT Service Account Configuration"), act_off("View ▸"),
        act("Refresh", mmc::I_REFRESH, move || if let Some(s) = c.upgrade() { s.loaded.set(false); s.m.refresh(); }),
        act_off("Help"),
    ])]);
}

fn ctl_run(t: &Task, verb: &[&str], s: &Rc<State>, what: &'static str) {
    let s2 = s.clone();
    let after = move |ok: bool, out: String| { if report(what, ok, &out) { reload(&s2, |s| fill(s)); } };
    let mut argv: Vec<String> = t.ctl().iter().map(|x| x.to_string()).collect();
    argv.extend(verb.iter().map(|x| x.to_string()));
    if t.kind == Kind::User {
        bg(move || { let r: Vec<&str> = argv.iter().map(|s| s.as_str()).collect(); run(&r) }, move |(ok, out)| after(ok, out));
    } else {
        let r: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        elevate(&r, after);
    }
}

fn run_task(s: &Rc<State>, t: &Task) {
    if t.kind == Kind::Cron { let cmd = t.exec.clone(); bg(move || run(&["sh", "-c", &cmd]), |_| {}); return; }
    ctl_run(t, &["start", "--no-block", &t.service.clone()], s, "The task could not be started.");
}

fn end(s: &Rc<State>, t: &Task) {
    if !confirm(Some(&s.m.win), "Task Scheduler", &format!("Do you want to end all instances of this task?"), &format!("{} ({})", t.name, t.service), "Yes") { return; }
    ctl_run(t, &["stop", &t.service.clone()], s, "The task could not be ended.");
}

fn enable(s: &Rc<State>, t: &Task, on: bool) {
    if t.kind == Kind::Cron {
        let (line, s2) = (t.cron_line.clone(), s.clone());
        bg(move || {
            let cur = sh(&["crontab", "-l"]);
            let new: String = cur.lines().map(|l| if l.trim() == line { if on { line.clone() } else { format!("#aerode-disabled# {line}") } } else if on && l.trim() == format!("#aerode-disabled# {line}") { line.clone() } else { l.to_string() }).collect::<Vec<_>>().join("\n") + "\n";
            set_crontab(&new)
        }, move |r| { if let Err(e) = r { util::error("The task could not be changed.", &e); } reload(&s2, |s| fill(s)); });
        return;
    }
    let timer = t.timer.clone();
    ctl_run(t, &[if on { "enable" } else { "disable" }, "--now", &timer], s, if on { "The task could not be enabled." } else { "The task could not be disabled." });
}

fn set_crontab(text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut c = std::process::Command::new("crontab").arg("-").stdin(std::process::Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    c.stdin.take().ok_or("no stdin")?.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    if c.wait().map(|s| s.success()).unwrap_or(false) { Ok(()) } else { Err("crontab refused the change.".into()) }
}

fn unit_text(t: &Task) -> String {
    if t.kind == Kind::Cron { return t.cron_line.clone() + "\n"; }
    let base: Vec<&str> = t.ctl();
    let mut a = base.clone();
    a.extend(["cat", t.timer.as_str(), t.service.as_str()]);
    sh(&a)
}

fn export(s: &Rc<State>, t: &Task) {
    let text = unit_text(t);
    save_text(Some(&s.m.win), "Save As", &format!("{}.{}", t.name, if t.kind == Kind::Cron { "cron" } else { "timer" }), &text);
}

fn delete(s: &Rc<State>, t: &Task) {
    if !confirm(Some(&s.m.win), "Task Scheduler", &format!("Are you sure you want to delete this task?"), &t.name, "Yes") { return; }
    let (t, s2) = (t.clone(), s.clone());
    bg(move || -> Result<(), String> {
        match t.kind {
            Kind::Cron => { let cur = sh(&["crontab", "-l"]); set_crontab(&(cur.lines().filter(|l| l.trim() != t.cron_line).collect::<Vec<_>>().join("\n") + "\n")) }
            _ => {
                run(&["systemctl", "--user", "disable", "--now", &t.timer]);
                let dir = glib::user_config_dir().join("systemd/user");
                for f in [&t.timer, &t.service] { let p = dir.join(f); if p.exists() { std::fs::remove_file(p).map_err(|e| e.to_string())?; } }
                run(&["systemctl", "--user", "daemon-reload"]);
                Ok(())
            }
        }
    }, move |r| { if let Err(e) = r { util::error("The task could not be deleted.", &e); } reload(&s2, |s| fill(s)); });
}

fn properties(s: &Rc<State>, t: &Task) {
    let d = gtk::Dialog::with_buttons(Some(&format!("{} Properties ({})", t.name, if t.kind == Kind::System { "Local Computer" } else { "Local User" })), Some(&s.m.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(620, 470);
    d.content_area().pack_start(&task_tabs(t, true), true, true, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
}

fn running(s: &Rc<State>) {
    let tasks: Vec<Task> = s.tasks.borrow().iter().filter(|t| t.status == "Running").cloned().collect();
    let d = gtk::Dialog::with_buttons(Some("All Running Tasks"), Some(&s.m.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("Close", gtk::ResponseType::Close)]);
    d.set_default_size(560, 300);
    let store = gtk::ListStore::new(&[glib::Type::STRING; 4]);
    for t in &tasks { store.insert_with_values(None, &[(0, &t.name), (1, &t.location()), (2, &if t.last > 0 { format!("{}s", now() - t.last) } else { String::new() }), (3, &t.service)]); }
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Name", 0, 180), ("Location", 1, 90), ("Elapsed Time", 2, 100), ("Current Action", 3, 0)]);
    let c = d.content_area();
    c.set_border_width(8);
    c.set_spacing(6);
    c.pack_start(&label(&format!("Running tasks: {}", tasks.len()), ""), false, false, 0);
    c.pack_start(&scrolled(&tv), true, true, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
}

/// Create Basic Task Wizard → ~/.config/systemd/user/aerode-task-NAME.{service,timer}.
fn wizard(s: &Rc<State>) {
    let d = gtk::Dialog::new();
    d.set_title("Create Basic Task Wizard");
    d.set_transient_for(Some(&s.m.win));
    d.set_modal(true);
    d.set_default_size(620, 440);
    let back = d.add_button("< _Back", gtk::ResponseType::Other(1));
    let next = d.add_button("_Next >", gtk::ResponseType::Other(2));
    let finish = d.add_button("_Finish", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let body = hbox(0);
    let steps = vbox(6);
    steps.set_border_width(10);
    steps.set_size_request(170, -1);
    let step_names = ["Create a Basic Task", "Trigger", "Schedule", "Action", "Start a Program", "Finish"];
    let step_labels: Vec<gtk::Label> = step_names.iter().map(|n| { let l = gtk::Label::new(Some(n)); l.set_xalign(0.0); steps.pack_start(&l, false, false, 0); l }).collect();
    body.pack_start(&steps, false, false, 0);
    body.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 0);
    let stack = gtk::Stack::new();
    stack.set_border_width(14);
    body.pack_start(&stack, true, true, 0);
    d.content_area().pack_start(&body, true, true, 0);
    // 0: name
    let p0 = vbox(6);
    p0.pack_start(&label("Use this wizard to quickly schedule a common task. For more advanced options or settings such as multiple task actions or triggers, use the Create Task command in the Actions pane.", ""), false, false, 0);
    p0.pack_start(&label("Name:", ""), false, false, 0);
    let name = gtk::Entry::new();
    p0.pack_start(&name, false, false, 0);
    p0.pack_start(&label("Description:", ""), false, false, 0);
    let desc = gtk::Entry::new();
    p0.pack_start(&desc, false, false, 0);
    stack.add_named(&p0, "0");
    // 1: trigger
    let p1 = vbox(6);
    p1.pack_start(&bold("When do you want the task to start?"), false, false, 0);
    let daily = gtk::RadioButton::with_label("Daily");
    let weekly = gtk::RadioButton::with_label_from_widget(&daily, "Weekly");
    let monthly = gtk::RadioButton::with_label_from_widget(&daily, "Monthly");
    let once = gtk::RadioButton::with_label_from_widget(&daily, "One time");
    let logon = gtk::RadioButton::with_label_from_widget(&daily, "When I log on");
    for r in [&daily, &weekly, &monthly, &once, &logon] { p1.pack_start(r, false, false, 0); }
    stack.add_named(&p1, "1");
    // 2: schedule
    let p2 = gtk::Grid::new();
    p2.set_row_spacing(8);
    p2.set_column_spacing(8);
    p2.attach(&label("Start:", ""), 0, 0, 1, 1);
    let date = gtk::Entry::new();
    date.set_text(&chrono::Local::now().format("%Y-%m-%d").to_string());
    date.set_width_chars(11);
    let time = gtk::Entry::new();
    time.set_text("03:00");
    time.set_width_chars(6);
    p2.attach(&date, 1, 0, 1, 1);
    p2.attach(&time, 2, 0, 1, 1);
    let rec_l = label("Recur every:", "");
    p2.attach(&rec_l, 0, 1, 1, 1);
    let every = gtk::SpinButton::with_range(1.0, 365.0, 1.0);
    p2.attach(&every, 1, 1, 1, 1);
    let unit = label("days", "");
    p2.attach(&unit, 2, 1, 1, 1);
    let days = hbox(4);
    let day_checks: Vec<gtk::CheckButton> = DOW.iter().map(|(_, n)| { let c = gtk::CheckButton::with_label(n); days.pack_start(&c, false, false, 0); c }).collect();
    day_checks[0].set_active(true);
    p2.attach(&days, 0, 2, 3, 1);
    stack.add_named(&p2, "2");
    // 3: action
    let p3 = vbox(6);
    p3.pack_start(&bold("What action do you want the task to perform?"), false, false, 0);
    let a_prog = gtk::RadioButton::with_label("Start a program");
    let a_mail = gtk::RadioButton::with_label_from_widget(&a_prog, "Send an e-mail");
    a_mail.set_sensitive(false);
    let a_msg = gtk::RadioButton::with_label_from_widget(&a_prog, "Display a message");
    for r in [&a_prog, &a_mail, &a_msg] { p3.pack_start(r, false, false, 0); }
    stack.add_named(&p3, "3");
    // 4: program / message
    let p4 = gtk::Grid::new();
    p4.set_row_spacing(8);
    p4.set_column_spacing(8);
    let l1 = label("Program/script:", "");
    p4.attach(&l1, 0, 0, 1, 1);
    let prog = gtk::Entry::new();
    prog.set_hexpand(true);
    p4.attach(&prog, 0, 1, 1, 1);
    let browse = gtk::Button::with_mnemonic("B_rowse...");
    p4.attach(&browse, 1, 1, 1, 1);
    let l2 = label("Add arguments (optional):", "");
    p4.attach(&l2, 0, 2, 1, 1);
    let args = gtk::Entry::new();
    p4.attach(&args, 0, 3, 1, 1);
    let l3 = label("Start in (optional):", "");
    p4.attach(&l3, 0, 4, 1, 1);
    let dir = gtk::Entry::new();
    p4.attach(&dir, 0, 5, 1, 1);
    { let (prog, d) = (prog.clone(), d.clone()); browse.connect_clicked(move |_| {
        let fc = gtk::FileChooserNative::new(Some("Open"), Some(&d), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
        if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() { prog.set_text(&p.to_string_lossy()); } }
    }); }
    stack.add_named(&p4, "4");
    // 5: finish
    let p5 = vbox(6);
    let summary = label("", "");
    summary.set_selectable(true);
    p5.pack_start(&summary, false, false, 0);
    stack.add_named(&p5, "5");
    let page = Rc::new(std::cell::Cell::new(0usize));
    let update: Rc<dyn Fn()> = {
        let (stack, page, back, next, finish, step_labels, every, unit, days, rec_l, l1, l2, l3, browse, a_msg, daily, weekly, _monthly, once, logon, summary, name, desc, prog, args, date, time) =
            (stack.clone(), page.clone(), back.clone(), next.clone(), finish.clone(), step_labels.clone(), every.clone(), unit.clone(), days.clone(), rec_l.clone(), l1.clone(), l2.clone(), l3.clone(), browse.clone(), a_msg.clone(), daily.clone(), weekly.clone(), monthly.clone(), once.clone(), logon.clone(), summary.clone(), name.clone(), desc.clone(), prog.clone(), args.clone(), date.clone(), time.clone());
        Rc::new(move || {
            let p = page.get();
            stack.set_visible_child_name(&p.to_string());
            for (i, l) in step_labels.iter().enumerate() { l.set_markup(&if i == p { format!("<b>{}</b>", step_names[i]) } else { step_names[i].to_string() }); }
            step_labels[4].set_text(if a_msg.is_active() { "Display a Message" } else { "Start a Program" });
            if p == 4 { step_labels[4].set_markup(&format!("<b>{}</b>", if a_msg.is_active() { "Display a Message" } else { "Start a Program" })); }
            back.set_sensitive(p > 0);
            next.set_sensitive(p < 5);
            finish.set_sensitive(p == 5);
            let w = weekly.is_active();
            days.set_visible(w);
            unit.set_text(if daily.is_active() { "days" } else if w { "weeks on:" } else { "months" });
            every.set_visible(!once.is_active());
            rec_l.set_visible(!once.is_active());
            unit.set_visible(!once.is_active());
            let msg = a_msg.is_active();
            l1.set_text(if msg { "Title:" } else { "Program/script:" });
            l2.set_text(if msg { "Message:" } else { "Add arguments (optional):" });
            l3.set_visible(!msg);
            browse.set_visible(!msg);
            if p == 5 {
                let trig = if logon.is_active() { "When I log on".to_string() } else if once.is_active() { format!("One time; At {} on {}", time.text(), date.text()) }
                    else if daily.is_active() { format!("Daily; At {} every {} day(s)", time.text(), every.value_as_int()) } else if weekly.is_active() { format!("Weekly; At {} every {} week(s)", time.text(), every.value_as_int()) } else { format!("Monthly; At {} on day {}", time.text(), date.text().split('-').nth(2).unwrap_or("1")) };
                summary.set_text(&format!("Name:          {}\nDescription:   {}\n\nTrigger:       {trig}\nAction:        {} {} {}", name.text(), desc.text(), if msg { "Display a message;" } else { "Start a program;" }, prog.text(), args.text()));
            }
        })
    };
    // skip the Schedule page for "When I log on"
    let step = { let (page, update, logon) = (page.clone(), update.clone(), logon.clone()); move |dirn: i32| {
        let mut p = page.get() as i32 + dirn;
        if p == 2 && logon.is_active() { p += dirn; }
        page.set(p.clamp(0, 5) as usize);
        update();
    } };
    let st = s.clone();
    d.connect_response(move |d, r| match r {
        gtk::ResponseType::Other(1) => step(-1),
        gtk::ResponseType::Other(2) => {
            if page.get() == 0 && name.text().trim().is_empty() { inform(Some(d.upcast_ref()), "Task Scheduler", "The task name cannot be empty.", ""); return; }
            step(1)
        }
        gtk::ResponseType::Ok => {
            let slug: String = name.text().chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
            let unit_name = format!("aerode-task-{}", slug.trim_matches('-'));
            let exec = if a_msg.is_active() { format!("/usr/bin/notify-send {} {}", q(&prog.text()), q(&args.text())) } else {
                let p = prog.text().to_string();
                let cmd = if args.text().is_empty() { p.clone() } else { format!("{p} {}", args.text()) };
                if p.starts_with('/') { cmd } else { format!("/bin/sh -c {}", q(&cmd)) }
            };
            let mut service = format!("[Unit]\nDescription={}\n\n[Service]\nType=oneshot\nExecStart={exec}\n", if desc.text().is_empty() { name.text().to_string() } else { desc.text().to_string() });
            if !dir.text().is_empty() && !a_msg.is_active() { service += &format!("WorkingDirectory={}\n", dir.text()); }
            let (h, mi) = { let t = time.text(); let mut it = t.split(':'); (it.next().and_then(|x| x.trim().parse::<u32>().ok()).unwrap_or(3).min(23), it.next().and_then(|x| x.trim().parse::<u32>().ok()).unwrap_or(0).min(59)) };
            let date_s = date.text().to_string();
            let n = every.value_as_int().max(1);
            let cal = if once.is_active() { format!("{date_s} {h:02}:{mi:02}:00") }
                else if daily.is_active() { if n == 1 { format!("*-*-* {h:02}:{mi:02}:00") } else { format!("*-*-1/{n} {h:02}:{mi:02}:00") } }
                else if weekly.is_active() { let ds: Vec<&str> = DOW.iter().zip(day_checks.iter()).filter(|(_, c)| c.is_active()).map(|(d, _)| d.0).collect(); format!("{} *-*-* {h:02}:{mi:02}:00", if ds.is_empty() { "Mon".to_string() } else { ds.join(",") }) }
                else { let dd = date_s.split('-').nth(2).unwrap_or("01"); if n == 1 { format!("*-*-{dd} {h:02}:{mi:02}:00") } else { format!("*-01/{n}-{dd} {h:02}:{mi:02}:00") } };
            let timer = if logon.is_active() { None } else { Some(format!("[Unit]\nDescription={}\n\n[Timer]\nOnCalendar={cal}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n", name.text())) };
            if logon.is_active() { service += "\n[Install]\nWantedBy=default.target\n"; }
            let dirp = glib::user_config_dir().join("systemd/user");
            let write = || -> std::io::Result<()> {
                std::fs::create_dir_all(&dirp)?;
                std::fs::write(dirp.join(format!("{unit_name}.service")), &service)?;
                if let Some(t) = &timer { std::fs::write(dirp.join(format!("{unit_name}.timer")), t)?; }
                Ok(())
            };
            if let Err(e) = write() { util::error("The task could not be created.", &e.to_string()); return; }
            let target = if timer.is_some() { format!("{unit_name}.timer") } else { format!("{unit_name}.service") };
            let st = st.clone();
            bg(move || { run(&["systemctl", "--user", "daemon-reload"]); run(&["systemctl", "--user", "enable", if target.ends_with(".timer") { "--now" } else { "--quiet" }, &target]) },
               move |(ok, out)| { if !ok { util::error("The task was created but could not be enabled.", out.trim()); } reload(&st, |s| fill(s)); });
            d.close();
        }
        _ => d.close(),
    });
    d.show_all();
    update();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar() {
        assert_eq!(calendar_words("*-*-* 00:00:00"), "At 12:00 AM every day");
        assert_eq!(calendar_words("daily"), "At 12:00 AM every day");
        assert_eq!(calendar_words("Mon *-*-* 00:00:00"), "At 12:00 AM every Monday of every week");
        assert_eq!(calendar_words("*-*-01 00:00:00"), "At 12:00 AM on day 1 of every month");
        assert_eq!(calendar_words("*-*-* *:00:00"), "Every hour");
        assert_eq!(calendar_words("*-*-* 15:30:00"), "At 3:30 PM every day");
        assert_eq!(calendar_words("Mon..Fri *-*-* 08:05:00"), "At 8:05 AM every Monday, Tuesday, Wednesday, Thursday, Friday of every week");
        assert!(calendar_words("*-*-* 00/6:00:00").starts_with("On a schedule"));
    }
    #[test]
    fn cron() {
        assert_eq!(cron_words(&["0", "3", "*", "*", "*"]), "At 3:00 AM every day");
        assert_eq!(cron_words(&["*/5", "*", "*", "*", "*"]), "Every 5 minutes");
        assert_eq!(cron_words(&["@reboot"]), "At system startup");
        assert_eq!(cron_words(&["30", "18", "*", "*", "1,5"]), "At 6:30 PM every Monday, Friday of every week");
    }
    #[test]
    fn show() {
        let out = "Id=a.timer\nTimersCalendar={ OnCalendar=*-*-* 00:00:00 ; next_elapse=@1700000000 }\nNextElapseUSecRealtime=@1700000000\n\nId=b.timer\nTimersMonotonic={ OnBootUSec=15min ; next_elapse=0 }\n";
        let v = parse_show(out);
        assert_eq!(v.len(), 2);
        assert_eq!(first(&v[0], "Id"), "a.timer");
        assert_eq!(parse_ts(&first(&v[0], "NextElapseUSecRealtime")), 1700000000);
        assert_eq!(parse_ts("Mon 2023-11-13 00:00:00 UTC"), 1699833600);
    }
}
