//! Event Viewer (eventvwr.msc) on top of the systemd journal.
//! Windows Logs: Application = the user's journal, Security = auth/authpriv messages, Setup =
//! /var/log/pacman.log, System = the system journal; Applications and Services Logs ▸ Kernel;
//! Custom Views ▸ Administrative Events = every Critical/Error/Warning. Events load on a worker
//! thread, newest first, a page at a time (more as you scroll), only when a log is opened.
use gtk::prelude::*;
use gtk::{gdk_pixbuf::Pixbuf, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::mmc::{self, act, act_off, Mmc};
use super::*;

const PAGE: usize = 1000;
const LEVELS: [&str; 5] = ["Critical", "Error", "Warning", "Information", "Verbose"];

#[derive(Clone, Default, Debug)]
pub struct Ev {
    /// index into LEVELS
    pub level: usize,
    /// microseconds since the epoch
    pub ts: i64,
    pub source: String,
    pub id: u32,
    pub category: String,
    pub message: String,
    pub user: String,
    pub pid: String,
    pub host: String,
    pub cursor: String,
    pub fields: Vec<(String, String)>,
}

pub fn level_of(prio: Option<u8>) -> usize {
    match prio { Some(0..=2) => 0, Some(3) => 1, Some(4) => 2, Some(7) => 4, _ => 3 }
}

fn level_icon(l: usize) -> &'static [&'static str] {
    match l { 0 | 1 => &["dialog-error"], 2 => &["dialog-warning"], _ => &["dialog-information"] }
}

/// Windows shows numeric event IDs per message type: the journal's MESSAGE_ID (a catalog UUID)
/// becomes a stable 4-digit number, messages without one get the syslog priority's classic id.
pub fn event_id(msg_id: &str, source: &str) -> u32 {
    if msg_id.len() >= 8 { return u32::from_str_radix(&msg_id[..8], 16).map(|v| v % 10000).unwrap_or(0); }
    // a small stable hash of the source, so the same program keeps the same id
    let mut h: u32 = 2166136261;
    for b in source.bytes() { h = (h ^ b as u32).wrapping_mul(16777619); }
    h % 1000
}

/// One line of `journalctl -o json`.
pub fn parse_json(line: &str) -> Option<Ev> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let o = v.as_object()?;
    let s = |k: &str| -> String {
        match o.get(k) {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(a)) => {
                // binary data, or a field that occurs several times
                if a.iter().all(|x| x.is_u64()) { String::from_utf8_lossy(&a.iter().filter_map(|x| x.as_u64().map(|b| b as u8)).collect::<Vec<u8>>()).into_owned() }
                else { a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ") }
            }
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    };
    let prio = s("PRIORITY").parse::<u8>().ok();
    let mut source = s("SYSLOG_IDENTIFIER");
    if source.is_empty() { source = s("_COMM"); }
    if source.is_empty() && s("_TRANSPORT") == "kernel" { source = "kernel".into(); }
    if source.is_empty() { source = s("_SYSTEMD_UNIT"); }
    let mut category = s("UNIT");
    if category.is_empty() { category = s("USER_UNIT"); }
    if category.is_empty() { category = s("_SYSTEMD_UNIT"); }
    if category.is_empty() { category = "None".into(); }
    let uid = s("_UID");
    let user = if uid.is_empty() { "N/A".into() } else if uid == "0" { "SYSTEM".into() } else { user_name(&uid) };
    let mut fields: Vec<(String, String)> = o.keys().map(|k| (k.clone(), s(k))).collect();
    fields.sort_by(|a, b| a.0.cmp(&b.0));
    Some(Ev {
        level: level_of(prio),
        ts: s("__REALTIME_TIMESTAMP").parse().unwrap_or(0),
        id: event_id(&s("MESSAGE_ID"), &source),
        source,
        category,
        message: s("MESSAGE"),
        user,
        pid: s("_PID"),
        host: s("_HOSTNAME"),
        cursor: s("__CURSOR"),
        fields,
    })
}

fn user_name(uid: &str) -> String {
    thread_local!(static PW: String = std::fs::read_to_string("/etc/passwd").unwrap_or_default());
    PW.with(|pw| pw.lines().find_map(|l| { let f: Vec<&str> = l.split(':').collect(); (f.len() > 2 && f[2] == uid).then(|| f[0].to_string()) })).unwrap_or_else(|| uid.to_string())
}

/// One line of /var/log/pacman.log → a Setup event.
pub fn parse_pacman(line: &str, host: &str) -> Option<Ev> {
    let rest = line.strip_prefix('[')?;
    let (stamp, rest) = rest.split_once("] ")?;
    let (src, msg) = match rest.strip_prefix('[') { Some(r) => { let (s, m) = r.split_once("] ")?; (s.to_string(), m.to_string()) } None => ("PACMAN".to_string(), rest.to_string()) };
    let ts = chrono::DateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%S%z").map(|d| d.timestamp()).ok()
        .or_else(|| chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M").ok().map(|d| d.and_utc().timestamp()))?;
    let lower = msg.to_lowercase();
    let level = if lower.starts_with("error") { 1 } else if lower.starts_with("warning") { 2 } else { 3 };
    let id = if msg.starts_with("transaction started") { 1 } else if msg.starts_with("installed ") { 2 } else if msg.starts_with("upgraded ") || msg.starts_with("downgraded ") || msg.starts_with("reinstalled ") { 3 }
        else if msg.starts_with("removed ") { 4 } else if msg.starts_with("transaction completed") { 5 } else if level == 2 { 6 } else if level == 1 { 7 } else { 0 };
    let category = match src.as_str() { "ALPM" => "Package", "ALPM-SCRIPTLET" => "Install script", "PACMAN" => "Command", _ => "None" }.to_string();
    Some(Ev { level, ts: ts * 1_000_000, source: if src == "ALPM-SCRIPTLET" { "ALPM".into() } else { src.clone() }, id, category, message: msg.clone(), user: "SYSTEM".into(), pid: String::new(), host: host.into(), cursor: String::new(),
        fields: vec![("SOURCE".into(), src), ("MESSAGE".into(), msg), ("TIME".into(), stamp.to_string())] })
}

#[derive(Clone)]
struct Filter {
    /// seconds back from now; 0 = any time
    since: i64,
    levels: [bool; 5],
    sources: String,
    ids: String,
    category: String,
    user: String,
}
impl Default for Filter { fn default() -> Self { Filter { since: 0, levels: [false; 5], sources: String::new(), ids: String::new(), category: String::new(), user: String::new() } } }
impl Filter {
    fn active(&self) -> bool { self.since > 0 || self.levels.iter().any(|l| *l) || !self.sources.trim().is_empty() || !self.ids.trim().is_empty() || !self.category.trim().is_empty() || !self.user.trim().is_empty() }
    fn matches(&self, e: &Ev) -> bool {
        if self.levels.iter().any(|l| *l) && !self.levels[e.level] { return false; }
        if !self.sources.trim().is_empty() && !self.sources.split(',').any(|s| s.trim().eq_ignore_ascii_case(&e.source)) { return false; }
        if !self.category.trim().is_empty() && !e.category.to_lowercase().contains(&self.category.trim().to_lowercase()) { return false; }
        if !self.user.trim().is_empty() && !e.user.eq_ignore_ascii_case(self.user.trim()) { return false; }
        if !self.ids.trim().is_empty() {
            let mut inc = vec![]; let mut exc = vec![];
            for p in self.ids.split(',').map(|p| p.trim()).filter(|p| !p.is_empty()) {
                let (neg, p) = match p.strip_prefix('-') { Some(r) => (true, r), None => (false, p) };
                let (a, b) = match p.split_once('-') { Some((a, b)) => (a.trim().parse().unwrap_or(0), b.trim().parse().unwrap_or(u32::MAX)), None => { let v = p.parse().unwrap_or(u32::MAX); (v, v) } };
                if neg { exc.push((a, b)) } else { inc.push((a, b)) }
            }
            if exc.iter().any(|(a, b)| (*a..=*b).contains(&e.id)) { return false; }
            if !inc.is_empty() && !inc.iter().any(|(a, b)| (*a..=*b).contains(&e.id)) { return false; }
        }
        true
    }
    fn describe(&self, log: &str) -> String {
        let mut s = format!("Log: {log}; ");
        let lv: Vec<&str> = (0..5).filter(|i| self.levels[*i]).map(|i| LEVELS[i]).collect();
        if !lv.is_empty() { s += &format!("Levels: {}; ", lv.join(", ")); }
        if !self.sources.trim().is_empty() { s += &format!("Source: {}; ", self.sources.trim()); }
        if !self.ids.trim().is_empty() { s += &format!("Event IDs: {}; ", self.ids.trim()); }
        if self.since > 0 { s += &format!("Logged: last {}; ", since_label(self.since).trim_start_matches("Last ")); }
        s
    }
}

const SINCE: [(i64, &str); 6] = [(0, "Any time"), (3600, "Last hour"), (12 * 3600, "Last 12 hours"), (24 * 3600, "Last 24 hours"), (7 * 86400, "Last 7 days"), (30 * 86400, "Last 30 days")];
fn since_label(s: i64) -> &'static str { SINCE.iter().find(|x| x.0 == s).map(|x| x.1).unwrap_or("Any time") }

/// journalctl arguments for a log (without paging).
fn journal_args(log: &str, f: &Filter) -> Vec<String> {
    let mut a: Vec<String> = vec!["journalctl".into(), "-o".into(), "json".into(), "-r".into(), "--no-pager".into(), "-q".into()];
    match log {
        "Application" => a.push("--user".into()),
        "System" => a.push("--system".into()),
        "Security" => { a.push("SYSLOG_FACILITY=4".into()); a.push("SYSLOG_FACILITY=10".into()); }
        "Kernel" => a.push("_TRANSPORT=kernel".into()),
        "Administrative Events" => { a.push("-p".into()); a.push("0..4".into()); }
        _ => {}
    }
    // the coarse part of the filter runs in journalctl, the rest on the loaded events
    let sel: Vec<usize> = (0..5).filter(|i| f.levels[*i]).collect();
    if !sel.is_empty() && log != "Administrative Events" {
        let prio = |l: usize, hi: bool| match l { 0 => if hi { 2 } else { 0 }, 1 => 3, 2 => 4, 3 => if hi { 6 } else { 5 }, _ => 7 };
        a.push("-p".into());
        a.push(format!("{}..{}", prio(sel[0], false), prio(*sel.last().unwrap(), true)));
    }
    if f.since > 0 { a.push(format!("--since=-{}s", f.since)); }
    if !f.sources.trim().is_empty() && !f.sources.contains(',') { a.push(format!("SYSLOG_IDENTIFIER={}", f.sources.trim())); }
    a
}

struct State {
    m: Rc<Mmc>,
    log: RefCell<String>,
    filter: RefCell<Filter>,
    events: RefCell<Vec<Ev>>,
    store: gtk::ListStore,
    view: gtk::TreeView,
    header: gtk::Label,
    title: gtk::Label,
    busy: Cell<bool>,
    more: Cell<bool>,
    /// bumps when the log or filter changes so late results of an old query are dropped
    epoch: Cell<u32>,
    details: gtk::Box,
    /// the result pane (list + preview), kept while switching between logs
    holder: gtk::Box,
}

/// Add the Event Viewer nodes (standalone or inside Computer Management).
pub fn attach(m: &Rc<Mmc>, parent: Option<&gtk::TreeIter>) {
    let root = m.add(parent, "ev", if parent.is_some() { "Event Viewer" } else { "Event Viewer (Local)" }, &["aerode-eventvwr", "utilities-system-monitor"]);
    let cv = m.add(Some(&root), "ev:cv", "Custom Views", &["folder"]);
    m.add(Some(&cv), "ev:log:Administrative Events", "Administrative Events", &["aerode-ev-filter", "edit-find"]);
    let wl = m.add(Some(&root), "ev:wl", "Windows Logs", &["folder"]);
    for l in ["Application", "Security", "Setup", "System", "Forwarded Events"] { m.add(Some(&wl), &format!("ev:log:{l}"), l, &["aerode-ev-log", "text-x-generic"]); }
    let asl = m.add(Some(&root), "ev:asl", "Applications and Services Logs", &["folder"]);
    m.add(Some(&asl), "ev:log:Kernel", "Kernel", &["aerode-ev-log", "text-x-generic"]);
    m.add(Some(&root), "ev:subs", "Subscriptions", &["folder"]);
    let state: Rc<RefCell<Option<Rc<State>>>> = Rc::new(RefCell::new(None));
    m.handle("ev", move |m, id| {
        if let Some(log) = id.strip_prefix("ev:log:") {
            let existing = state.borrow().clone();
            let st = match existing { Some(s) => s, None => { let s = new_state(m); *state.borrow_mut() = Some(s.clone()); s } };
            open_log(&st, log);
            return;
        }
        match id {
            "ev" => overview(m),
            "ev:subs" => folder_note(m, "Subscriptions", "Event subscriptions collect events from other computers. This computer does not collect events from other computers."),
            _ => {
                let what = match id { "ev:cv" => "Custom Views", "ev:wl" => "Windows Logs", _ => "Applications and Services Logs" };
                logs_summary(m, what);
            }
        }
    });
}

pub fn window(app: &gtk::Application) -> gtk::Window {
    let m = Mmc::new(app, "Event Viewer", &["aerode-eventvwr", "utilities-system-monitor"], true, true);
    attach(&m, None);
    m.show("ev");
    m.win.clone()
}

fn folder_note(m: &Rc<Mmc>, title: &str, text: &str) {
    let v = vbox(0);
    v.pack_start(&mmc::head(title, "").0, false, false, 0);
    let l = label(text, "");
    l.set_margin_start(12);
    l.set_margin_top(12);
    v.pack_start(&l, false, false, 0);
    m.set_center(&v);
    m.set_actions(vec![(title.into(), vec![act_off("Refresh")])]);
}

/// Windows Logs / Custom Views folder: a table of its logs with their size.
fn logs_summary(m: &Rc<Mmc>, what: &str) {
    let v = vbox(0);
    v.pack_start(&mmc::head(what, "").0, false, false, 0);
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    icon_column(&tv, "Name", 0, 1, 200);
    columns(&tv, &[("Type", 2, 110), ("Number of Events", 3, 130), ("Size", 4, 0)]);
    v.pack_start(&scrolled(&tv), true, true, 0);
    let logs: Vec<&str> = match what { "Custom Views" => vec!["Administrative Events"], "Windows Logs" => vec!["Application", "Security", "Setup", "System", "Forwarded Events"], _ => vec!["Kernel"] };
    let pb = pix(&["aerode-ev-log", "text-x-generic"], 16);
    for l in &logs { store.insert_with_values(None, &[(0, &pb), (1, l), (2, &if *l == "Administrative Events" { "Custom View" } else { "Administrative" }), (3, &""), (4, &"")]); }
    { let me = Rc::downgrade(m); tv.connect_row_activated(move |tv, p, _| { if let (Some(m), Some(md)) = (me.upgrade(), tv.model()) { if let Some(it) = md.iter(p) { m.select(&format!("ev:log:{}", sval(&md, &it, 1))); } } }); }
    m.set_center(&v);
    let mm = m.clone();
    m.set_actions(vec![(what.into(), vec![act("Refresh", mmc::I_REFRESH, move || mm.refresh())])]);
    if what == "Windows Logs" {
        let st = store.clone();
        bg(|| {
            let usage = sh(&["journalctl", "--disk-usage"]);
            let size = usage.split("take up ").nth(1).and_then(|s| s.split_whitespace().next()).unwrap_or("").to_string();
            let pac = std::fs::metadata("/var/log/pacman.log").map(|m| m.len()).unwrap_or(0);
            (size, pac)
        }, move |(size, pac)| {
            st.foreach(|md, _, it| {
                let n = sval(md, it, 1);
                let s = match n.as_str() { "System" | "Application" | "Security" => format!("{size} (journal)"), "Setup" => if pac > 0 { gb(pac) } else { "0 bytes".into() }, _ => "0 bytes".into() };
                st.set_value(it, 4, &s.to_value());
                false
            });
        });
    }
}

/// "Event Viewer (Local)": overview and the summary of administrative events.
fn overview(m: &Rc<Mmc>) {
    let v = vbox(8);
    let (h, _) = mmc::head("Overview and Summary", "");
    v.pack_start(&h, false, false, 0);
    let inner = vbox(10);
    inner.set_border_width(8);
    let desc = label("To view events that have occurred on your computer, select the appropriate source, log or custom view node in the console tree. The Administrative Events custom view contains all the administrative events, regardless of source. An aggregate view of all the logs is shown below.", "");
    let d = vbox(0);
    d.style_context().add_class("mmc-desc");
    d.pack_start(&desc, false, false, 0);
    inner.pack_start(&mmc::group("Overview", &d), false, false, 0);
    // Summary of Administrative Events: level ▸ source rows with counts per period
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Event Type", 0, 100), ("Event ID", 1, 60), ("Source", 2, 110), ("Log", 3, 70), ("Last hour", 4, 66), ("24 hours", 5, 66), ("7 days", 6, 0)]);
    let sw = scrolled(&tv);
    sw.set_min_content_height(220);
    let status = label("Loading…", "dim");
    let sum = vbox(4);
    sum.pack_start(&sw, true, true, 0);
    sum.pack_start(&status, false, false, 0);
    inner.pack_start(&mmc::group("Summary of Administrative Events", &sum), true, true, 0);
    // Log Summary
    let ls = gtk::ListStore::new(&[glib::Type::STRING; 5]);
    let ltv = gtk::TreeView::with_model(&ls);
    columns(&ltv, &[("Log Name", 0, 150), ("Size (Current/Maximum)", 1, 180), ("Modified", 2, 160), ("Enabled", 3, 70), ("Retention Policy", 4, 0)]);
    let lsw = scrolled(&ltv);
    lsw.set_min_content_height(120);
    inner.pack_start(&mmc::group("Log Summary", &lsw), false, false, 0);
    let outer = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    outer.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    outer.add(&inner);
    v.pack_start(&outer, true, true, 0);
    m.set_center(&v);
    let (m1, m2) = (m.clone(), m.clone());
    m.set_actions(vec![("Event Viewer (Local)".into(), vec![
        act("Open Saved Log...", mmc::I_OPEN, move || open_saved(&m1)),
        act_off("Create Custom View..."), act_off("Import Custom View..."), act_off("Connect to Another Computer..."),
        act("Refresh", mmc::I_REFRESH, move || m2.refresh()), act_off("Help ▸"),
    ])]);
    bg(|| {
        let since = now() - 7 * 86400;
        let out = std::process::Command::new("journalctl").args(["-o", "json", "-p", "0..4", "--since", "-7d", "--no-pager", "-q", "-n", "200000",
            "--output-fields=PRIORITY,SYSLOG_IDENTIFIER,_COMM,_TRANSPORT,MESSAGE_ID,__REALTIME_TIMESTAMP,_UID"]).output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let mut evs: Vec<Ev> = out.lines().filter_map(parse_json).filter(|e| e.ts / 1_000_000 >= since).collect();
        // Setup log errors/warnings
        if let Ok(p) = std::fs::read_to_string("/var/log/pacman.log") {
            evs.extend(p.lines().rev().take(20000).filter_map(|l| parse_pacman(l, "")).filter(|e| e.level < 3 && e.ts / 1_000_000 >= since));
        }
        let usage = sh(&["journalctl", "--disk-usage"]);
        let size = usage.split("take up ").nth(1).and_then(|s| s.split_whitespace().next()).unwrap_or("").to_string();
        let jmod = mtime(std::path::Path::new("/var/log/journal")).or_else(|| mtime(std::path::Path::new("/run/log/journal")));
        let pac = std::fs::metadata("/var/log/pacman.log").ok().map(|m| m.len());
        let pmod = mtime(std::path::Path::new("/var/log/pacman.log"));
        let conf = std::fs::read_to_string("/etc/systemd/journald.conf").unwrap_or_default();
        let max = conf.lines().find_map(|l| l.trim().strip_prefix("SystemMaxUse=")).map(|s| s.to_string()).unwrap_or_else(|| "10% of disk (max 4G)".into());
        (evs, size, jmod, pac, pmod, max)
    }, move |(evs, size, jmod, pac, pmod, max)| {
        let t = now();
        let mut total = [[0usize; 3]; 3];
        for lvl in 0..3 {
            let these: Vec<&Ev> = evs.iter().filter(|e| e.level == lvl).collect();
            let counts = |list: &[&Ev]| { let mut c = [0usize; 3]; for e in list { let age = t - e.ts / 1_000_000; if age <= 3600 { c[0] += 1; } if age <= 86400 { c[1] += 1; } c[2] += 1; } c };
            let c = counts(&these);
            total[lvl] = c;
            let n = |x: usize| if x == 0 { "-".to_string() } else { x.to_string() };
            let parent = store.insert_with_values(None, None, &[(0, &LEVELS[lvl]), (1, &"-"), (2, &"-"), (3, &"-"), (4, &n(c[0])), (5, &n(c[1])), (6, &n(c[2]))]);
            // group by (id, source)
            let mut by: std::collections::BTreeMap<(String, u32), Vec<&Ev>> = Default::default();
            for e in &these { by.entry((e.source.clone(), e.id)).or_default().push(e); }
            let mut groups: Vec<_> = by.into_iter().collect();
            groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
            for ((src, id), list) in groups.into_iter().take(200) {
                let c = counts(&list);
                let log = if list[0].category == "Package" || list[0].source == "ALPM" || list[0].source == "PACMAN" { "Setup" } else if list[0].user == "SYSTEM" || list[0].user == "N/A" { "System" } else { "Application" };
                store.insert_with_values(Some(&parent), None, &[(0, &LEVELS[lvl]), (1, &id.to_string()), (2, &src), (3, &log), (4, &n(c[0])), (5, &n(c[1])), (6, &n(c[2]))]);
            }
        }
        store.insert_with_values(None, None, &[(0, &"Information"), (1, &"-"), (2, &"-"), (3, &"-"), (4, &"-"), (5, &"-"), (6, &"-")]);
        store.insert_with_values(None, None, &[(0, &"Audit Success"), (1, &"-"), (2, &"-"), (3, &"-"), (4, &"-"), (5, &"-"), (6, &"-")]);
        store.insert_with_values(None, None, &[(0, &"Audit Failure"), (1, &"-"), (2, &"-"), (3, &"-"), (4, &"-"), (5, &"-"), (6, &"-")]);
        status.set_text(if evs.is_empty() { "No administrative events in the last 7 days (or the journal could not be read – add yourself to the systemd-journal or wheel group)." } else { "Last refreshed: just now" });
        let jm = jmod.map(when).unwrap_or_default();
        for l in ["Application", "Security", "System"] { ls.insert_with_values(None, &[(0, &l), (1, &format!("{size}/{max}")), (2, &jm), (3, &"Enabled"), (4, &"Overwrite events as necessary")]); }
        ls.insert_with_values(None, &[(0, &"Setup"), (1, &pac.map(gb).unwrap_or_else(|| "0 bytes".into())), (2, &pmod.map(when).unwrap_or_default()), (3, &if pac.is_some() { "Enabled" } else { "Disabled" }), (4, &"Never overwrite events")]);
        let _ = total;
    });
}

fn new_state(m: &Rc<Mmc>) -> Rc<State> {
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U32, glib::Type::STRING, glib::Type::I64, glib::Type::U32]);
    let view = gtk::TreeView::with_model(&store);
    // Level column: icon + text, sorted by level
    let c = icon_column(&view, "Level", 0, 1, 110);
    c.set_sort_column_id(7);
    let date = gtk::TreeViewColumn::new();
    date.set_title("Date and Time");
    let r = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&date, &r, true);
    TreeViewColumnExt::add_attribute(&date, &r, "text", 2);
    date.set_sort_column_id(6);
    date.set_resizable(true);
    date.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    date.set_fixed_width(170);
    view.append_column(&date);
    columns(&view, &[("Source", 3, 150)]);
    let idc = gtk::TreeViewColumn::new();
    idc.set_title("Event ID");
    let r = gtk::CellRendererText::new();
    r.set_xalign(1.0);
    TreeViewColumnExt::pack_start(&idc, &r, true);
    TreeViewColumnExt::add_attribute(&idc, &r, "text", 4);
    idc.set_sort_column_id(4);
    idc.set_resizable(true);
    idc.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    idc.set_fixed_width(70);
    view.append_column(&idc);
    columns(&view, &[("Task Category", 5, 0)]);
    // fixed-height mode (fast with thousands of rows) needs every column fixed-size
    if let Some(c) = view.column(4) { c.set_sizing(gtk::TreeViewColumnSizing::Fixed); c.set_fixed_width(160); }
    view.set_fixed_height_mode(true);
    let (h, header) = mmc::head("", "");
    let title = h.children()[0].clone().downcast::<gtk::Label>().unwrap();
    let details = vbox(0);
    let top = vbox(0);
    top.pack_start(&h, false, false, 0);
    let sw = scrolled(&view);
    top.pack_start(&sw, true, true, 0);
    let paned = gtk::Paned::new(gtk::Orientation::Vertical);
    paned.pack1(&top, true, false);
    paned.pack2(&details, false, false);
    paned.set_position(300);
    let holder = vbox(0);
    holder.pack_start(&paned, true, true, 0);
    let st = Rc::new(State { m: m.clone(), log: RefCell::new(String::new()), filter: RefCell::new(Filter::default()), events: RefCell::new(vec![]), store, view, header, title, busy: Cell::new(false), more: Cell::new(false), epoch: Cell::new(0), details, holder });
    // lazy paging: when the list is scrolled to the end, load older events
    { let st2 = Rc::downgrade(&st); sw.vadjustment().connect_value_changed(move |a| {
        if a.value() + a.page_size() >= a.upper() - 40.0 { if let Some(st) = st2.upgrade() { if st.more.get() && !st.busy.get() { load(&st, true); } } }
    }); }
    { let st2 = Rc::downgrade(&st); st.view.selection().connect_changed(move |_| if let Some(st) = st2.upgrade() { show_details(&st); update_actions(&st); }); }
    { let st2 = Rc::downgrade(&st); st.view.connect_row_activated(move |_, _, _| if let Some(st) = st2.upgrade() { if let Some(e) = selected(&st) { properties(&st, &e); } }); }
    { let st2 = Rc::downgrade(&st); st.view.connect_button_press_event(move |tv, ev| {
        if ev.button() == 3 { if let (Some(st), Some((Some(p), _, _, _))) = (st2.upgrade(), tv.path_at_pos(ev.position().0 as i32, ev.position().1 as i32)) { tv.selection().select_path(&p); st.m.popup_actions(ev); return glib::Propagation::Stop; } }
        glib::Propagation::Proceed
    }); }
    st
}

fn open_log(st: &Rc<State>, log: &str) {
    if *st.log.borrow() != log { *st.filter.borrow_mut() = Filter::default(); }
    *st.log.borrow_mut() = log.to_string();
    st.title.set_text(log);
    st.m.set_center(&st.holder);
    load(st, false);
    update_actions(st);
}

fn load(st: &Rc<State>, more: bool) {
    let log = st.log.borrow().clone();
    let f = st.filter.borrow().clone();
    if !more {
        st.epoch.set(st.epoch.get() + 1);
        st.store.clear();
        st.events.borrow_mut().clear();
        show_details(st);
    }
    let epoch = st.epoch.get();
    st.busy.set(true);
    st.header.set_text("Loading events…");
    let cursor = if more { st.events.borrow().last().map(|e| e.cursor.clone()).unwrap_or_default() } else { String::new() };
    let skip = st.events.borrow().len();
    let host = glib::host_name().to_string();
    let f2 = f.clone();
    bg(move || -> (Vec<Ev>, bool, String) {
        if log == "Forwarded Events" { return (vec![], false, String::new()); }
        if log == "Setup" {
            let txt = std::fs::read_to_string("/var/log/pacman.log").unwrap_or_default();
            let since = if f2.since > 0 { now() - f2.since } else { 0 };
            let all: Vec<Ev> = txt.lines().rev().filter_map(|l| parse_pacman(l, &host)).filter(|e| e.ts / 1_000_000 >= since && f2.matches(e)).skip(skip).take(PAGE + 1).collect();
            let more = all.len() > PAGE;
            return (all.into_iter().take(PAGE).collect(), more, if txt.is_empty() { "The Setup log (/var/log/pacman.log) could not be read.".into() } else { String::new() });
        }
        let mut a = journal_args(&log, &f2);
        a.push("-n".into());
        if more && !cursor.is_empty() { a.push((PAGE + 1).to_string()); a.push(format!("--cursor={cursor}")); } else { a.push(PAGE.to_string()); }
        let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        let (ok, out) = match std::process::Command::new(refs[0]).args(&refs[1..]).output() {
            Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned() + "\n\u{0}" + &String::from_utf8_lossy(&o.stderr)),
            Err(e) => (false, format!("\n\u{0}{e}")),
        };
        let (stdout, stderr) = out.split_once('\u{0}').unwrap_or((&out, ""));
        let mut evs: Vec<Ev> = stdout.lines().filter_map(parse_json).collect();
        let fetched = evs.len();
        if more && !cursor.is_empty() && evs.first().map(|e| e.cursor == cursor).unwrap_or(false) { evs.remove(0); }
        let has_more = fetched >= PAGE;
        evs.retain(|e| f2.matches(e));
        let err = if !ok && evs.is_empty() { stderr.trim().lines().last().unwrap_or("The log could not be read.").to_string() } else { String::new() };
        (evs, has_more, err)
    }, {
        let st = st.clone();
        move |(evs, has_more, err): (Vec<Ev>, bool, String)| {
            if st.epoch.get() != epoch { return; }
            st.busy.set(false);
            st.more.set(has_more);
            for e in evs.iter() {
                let e: &Ev = e;
                let pb = pix(level_icon(e.level), 16);
                st.store.insert_with_values(None, &[(0, &pb), (1, &LEVELS[e.level]), (2, &when(e.ts / 1_000_000)), (3, &e.source), (4, &e.id), (5, &e.category), (6, &e.ts), (7, &(e.level as u32))]);
            }
            st.events.borrow_mut().extend(evs);
            let n = st.events.borrow().len();
            let log = st.log.borrow().clone();
            let fl = st.filter.borrow().clone();
            let count = format!("Number of events: {}{}", mmc::thousands(n), if has_more { " (scroll down to load more)" } else { "" });
            st.header.set_text(&if !err.is_empty() { err } else if fl.active() { format!("Filtered: {} {count}", fl.describe(&log)) } else { count });
            if st.view.selection().count_selected_rows() == 0 && n > 0 && st.store.iter_n_children(None) == n as i32 && n <= PAGE {
                st.view.selection().select_path(&gtk::TreePath::new_first());
            }
        }
    });
}

fn selected(st: &State) -> Option<Ev> {
    let (m, it) = st.view.selection().selected()?;
    let ts: i64 = m.value(&it, 6).get().ok()?;
    let src = sval(&m, &it, 3);
    st.events.borrow().iter().find(|e| e.ts == ts && e.source == src).cloned()
}

/// The General / Details tabs (preview pane and Event Properties).
fn event_tabs(e: &Ev, log: &str) -> gtk::Notebook {
    let nb = gtk::Notebook::new();
    let g = vbox(8);
    g.set_border_width(8);
    let msg = text_box(&e.message);
    let msw = scrolled(&msg);
    msw.set_min_content_height(80);
    g.pack_start(&msw, true, true, 0);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(4);
    grid.set_column_spacing(12);
    let host = if e.host.is_empty() { glib::host_name().to_string() } else { e.host.clone() };
    let cells: [(&str, String, &str, String); 6] = [
        ("Log Name:", log.to_string(), "", String::new()),
        ("Source:", e.source.clone(), "Logged:", when(e.ts / 1_000_000)),
        ("Event ID:", e.id.to_string(), "Task Category:", e.category.clone()),
        ("Level:", LEVELS[e.level].to_string(), "Keywords:", "Classic".into()),
        ("User:", e.user.clone(), "Computer:", host),
        ("OpCode:", "Info".into(), "Process ID:", e.pid.clone()),
    ];
    for (r, (k1, v1, k2, v2)) in cells.iter().enumerate() {
        let add = |t: &str, c: i32, sel: bool| { let l = gtk::Label::new(Some(t)); l.set_xalign(0.0); l.set_selectable(sel); l.set_ellipsize(gtk::pango::EllipsizeMode::End); grid.attach(&l, c, r as i32, 1, 1); };
        add(k1, 0, false); add(v1, 1, true); add(k2, 2, false); add(v2, 3, true);
    }
    let r = cells.len() as i32;
    let l = gtk::Label::new(Some("More Information:"));
    l.set_xalign(0.0);
    grid.attach(&l, 0, r, 1, 1);
    let src = e.source.clone();
    grid.attach(&super::super::super::link("Event Log Online Help", move || { util::open_uri(&format!("https://wiki.archlinux.org/index.php?search={}", src.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_string() } else { format!("%{:02X}", c as u32 & 0xff) }).collect::<String>())); }), 1, r, 3, 1);
    g.pack_start(&grid, false, false, 0);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    // Details: Friendly view / XML view
    let d = vbox(6);
    d.set_border_width(8);
    let rb = hbox(16);
    let friendly = gtk::RadioButton::with_label("Friendly View");
    let xml = gtk::RadioButton::with_label_from_widget(&friendly, "XML View");
    rb.pack_start(&friendly, false, false, 0);
    rb.pack_start(&xml, false, false, 0);
    d.pack_start(&rb, false, false, 0);
    let mut fr = String::from("+ System\n");
    for (k, v) in e.fields.iter().filter(|(k, _)| k.starts_with('_')) { fr += &format!("    {k}  {v}\n"); }
    fr += "\n+ EventData\n";
    for (k, v) in e.fields.iter().filter(|(k, _)| !k.starts_with('_')) { fr += &format!("    {k}  {v}\n"); }
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut x = String::from("<Event xmlns=\"http://schemas.microsoft.com/win/2004/08/events/event\">\n  <System>\n");
    x += &format!("    <Provider Name=\"{}\" />\n    <EventID>{}</EventID>\n    <Level>{}</Level>\n    <TimeCreated SystemTime=\"{}\" />\n    <Execution ProcessID=\"{}\" />\n    <Channel>{}</Channel>\n    <Computer>{}</Computer>\n  </System>\n  <EventData>\n",
        esc(&e.source), e.id, e.level + 1, chrono::DateTime::from_timestamp_micros(e.ts).map(|d| d.to_rfc3339()).unwrap_or_default(), esc(&e.pid), esc(log), esc(&e.host));
    for (k, v) in e.fields.iter().filter(|(k, _)| !k.starts_with('_')) { x += &format!("    <Data Name=\"{}\">{}</Data>\n", esc(k), esc(v)); }
    x += "  </EventData>\n</Event>\n";
    let tvw = text_box(&fr);
    tvw.set_monospace(true);
    tvw.set_wrap_mode(gtk::WrapMode::None);
    { let (t, fr, x) = (tvw.clone(), fr.clone(), x.clone()); xml.connect_toggled(move |b| { if let Some(buf) = t.buffer() { buf.set_text(if b.is_active() { &x } else { &fr }); } }); }
    d.pack_start(&scrolled(&tvw), true, true, 0);
    nb.append_page(&d, Some(&gtk::Label::new(Some("Details"))));
    nb
}

fn show_details(st: &Rc<State>) {
    for c in st.details.children() { st.details.remove(&c); }
    let Some(e) = selected(st) else { return };
    let (h, _) = mmc::head(&format!("Event {}, {}", e.id, e.source), "");
    st.details.pack_start(&h, false, false, 0);
    st.details.pack_start(&event_tabs(&e, &st.log.borrow()), true, true, 0);
    st.details.show_all();
}

fn as_text(e: &Ev, log: &str) -> String {
    format!("Log Name:      {log}\nSource:        {}\nDate:          {}\nEvent ID:      {}\nTask Category: {}\nLevel:         {}\nKeywords:      Classic\nUser:          {}\nComputer:      {}\nDescription:\n{}\n",
        e.source, when(e.ts / 1_000_000), e.id, e.category, LEVELS[e.level], e.user, e.host, e.message)
}

fn properties(st: &Rc<State>, e: &Ev) {
    let d = gtk::Dialog::with_buttons(Some(&format!("Event Properties - Event {}, {}", e.id, e.source)), Some(&st.m.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("_Copy", gtk::ResponseType::Other(1)), ("_Close", gtk::ResponseType::Close)]);
    d.set_default_size(640, 470);
    d.content_area().pack_start(&event_tabs(e, &st.log.borrow()), true, true, 0);
    let txt = as_text(e, &st.log.borrow());
    d.connect_response(move |d, r| { if r == gtk::ResponseType::Other(1) { copy(&txt); } else { d.close(); } });
    d.show_all();
}

fn update_actions(st: &Rc<State>) {
    let log = st.log.borrow().clone();
    let m = &st.m;
    let w = Rc::downgrade(st);
    let s = move || w.upgrade();
    let mut groups = vec![];
    let (a, b, c, d, e, f, g) = (s.clone(), s.clone(), s.clone(), s.clone(), s.clone(), s.clone(), s.clone());
    let mm = m.clone();
    let journal = log != "Setup" && log != "Forwarded Events";
    groups.push((log.clone(), vec![
        act("Open Saved Log...", mmc::I_OPEN, move || open_saved(&mm)),
        act_off("Create Custom View..."),
        act_off("Import Custom View..."),
        if journal && log != "Administrative Events" { act("Clear Log...", NONE_ICON, move || if let Some(st) = a() { clear_log(&st) }) } else { act_off("Clear Log...") },
        act("Filter Current Log...", mmc::I_FILTER, move || if let Some(st) = b() { filter_dialog(&st) }),
        if st.filter.borrow().active() { act("Clear Filter", NONE_ICON, move || if let Some(st) = c() { *st.filter.borrow_mut() = Filter::default(); load(&st, false); update_actions(&st); }) } else { act_off("Clear Filter") },
        act("Properties", mmc::I_PROPS, move || if let Some(st) = d() { log_properties(&st) }),
        act("Find...", mmc::I_FIND, move || if let Some(st) = e() { find(&st) }),
        act("Save All Events As...", mmc::I_SAVE, move || if let Some(st) = f() { save_all(&st, false) }),
        act_off("Attach a Task To this Log..."),
        act("Refresh", mmc::I_REFRESH, move || if let Some(st) = g() { load(&st, false) }),
        act_off("Help ▸"),
    ]));
    if let Some(ev) = selected(st) {
        let (a, b, c, d) = (s.clone(), s.clone(), s.clone(), s.clone());
        let ev2 = ev.clone();
        groups.push((format!("Event {}, {}", ev.id, ev.source), vec![
            act("Event Properties", mmc::I_PROPS, move || if let Some(st) = a() { properties(&st, &ev2) }),
            act_off("Attach Task To This Event..."),
            act("Copy", mmc::I_COPY, move || if let Some(st) = b() { if let Some(e) = selected(&st) { copy(&as_text(&e, &st.log.borrow())); } }),
            act("Save Selected Events...", mmc::I_SAVE, move || if let Some(st) = c() { save_all(&st, true) }),
            act("Refresh", mmc::I_REFRESH, move || if let Some(st) = d() { load(&st, false) }),
            act_off("Help ▸"),
        ]));
    }
    m.set_actions(groups);
}
const NONE_ICON: &[&str] = &[];

fn filter_dialog(st: &Rc<State>) {
    let log = st.log.borrow().clone();
    let cur = st.filter.borrow().clone();
    let d = gtk::Dialog::with_buttons(Some("Filter Current Log"), Some(&st.m.win), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(470, -1);
    let nb = gtk::Notebook::new();
    let g = gtk::Grid::new();
    g.set_border_width(12);
    g.set_row_spacing(8);
    g.set_column_spacing(10);
    let lab = |t: &str| { let l = gtk::Label::new(Some(t)); l.set_xalign(0.0); l };
    g.attach(&lab("Logged:"), 0, 0, 1, 1);
    let logged = gtk::ComboBoxText::new();
    for (s, l) in SINCE { logged.append(Some(&s.to_string()), l); }
    logged.set_active_id(Some(&cur.since.to_string()));
    g.attach(&logged, 1, 0, 2, 1);
    g.attach(&lab("Event level:"), 0, 1, 1, 1);
    let lv = gtk::Grid::new();
    lv.set_column_spacing(14);
    let checks: Vec<gtk::CheckButton> = LEVELS.iter().enumerate().map(|(i, l)| { let c = gtk::CheckButton::with_label(l); c.set_active(cur.levels[i]); c }).collect();
    // Win7 order: Critical Warning Verbose / Error Information
    lv.attach(&checks[0], 0, 0, 1, 1); lv.attach(&checks[2], 1, 0, 1, 1); lv.attach(&checks[4], 2, 0, 1, 1);
    lv.attach(&checks[1], 0, 1, 1, 1); lv.attach(&checks[3], 1, 1, 1, 1);
    g.attach(&lv, 1, 1, 2, 1);
    let by_log = gtk::RadioButton::with_label("By log");
    let by_src = gtk::RadioButton::with_label_from_widget(&by_log, "By source");
    g.attach(&by_log, 0, 2, 1, 1);
    let logs = gtk::Entry::new();
    logs.set_text(&log);
    logs.set_sensitive(false);
    g.attach(&lab("Event logs:"), 1, 2, 1, 1);
    g.attach(&logs, 2, 2, 1, 1);
    g.attach(&by_src, 0, 3, 1, 1);
    g.attach(&lab("Event sources:"), 1, 3, 1, 1);
    let srcs = gtk::Entry::new();
    srcs.set_text(&cur.sources);
    srcs.set_placeholder_text(Some("e.g. sshd, NetworkManager"));
    g.attach(&srcs, 2, 3, 1, 1);
    if !cur.sources.is_empty() { by_src.set_active(true); }
    let ids = gtk::Entry::new();
    ids.set_text(&cur.ids);
    ids.set_placeholder_text(Some("<All Event IDs>"));
    let note = label("Includes/Excludes Event IDs: Enter ID numbers and/or ID ranges separated by commas. To exclude criteria, type a minus sign first. For example 1,3,5-99,-76", "dim");
    note.set_max_width_chars(60);
    g.attach(&note, 0, 4, 3, 1);
    g.attach(&ids, 0, 5, 3, 1);
    g.attach(&lab("Task category:"), 0, 6, 1, 1);
    let cat = gtk::Entry::new();
    cat.set_text(&cur.category);
    g.attach(&cat, 1, 6, 2, 1);
    g.attach(&lab("User:"), 0, 7, 1, 1);
    let user = gtk::Entry::new();
    user.set_text(&cur.user);
    user.set_placeholder_text(Some("<All Users>"));
    g.attach(&user, 1, 7, 2, 1);
    g.attach(&lab("Computer(s):"), 0, 8, 1, 1);
    let comp = gtk::Entry::new();
    comp.set_placeholder_text(Some("<All Computers>"));
    comp.set_sensitive(false);
    g.attach(&comp, 1, 8, 2, 1);
    let clear = gtk::Button::with_mnemonic("C_lear");
    clear.set_halign(gtk::Align::End);
    { let (logged, checks, srcs, ids, cat, user) = (logged.clone(), checks.clone(), srcs.clone(), ids.clone(), cat.clone(), user.clone()); clear.connect_clicked(move |_| {
        logged.set_active_id(Some("0")); for c in &checks { c.set_active(false); } srcs.set_text(""); ids.set_text(""); cat.set_text(""); user.set_text("");
    }); }
    g.attach(&clear, 2, 9, 1, 1);
    nb.append_page(&g, Some(&gtk::Label::new(Some("Filter"))));
    let x = text_box(&format!("journalctl {}", journal_args(&log, &cur)[1..].join(" ")));
    x.set_monospace(true);
    let xb = vbox(0);
    xb.set_border_width(8);
    xb.pack_start(&scrolled(&x), true, true, 0);
    nb.append_page(&xb, Some(&gtk::Label::new(Some("XML"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        let mut f = Filter::default();
        f.since = logged.active_id().and_then(|s| s.parse().ok()).unwrap_or(0);
        for (i, c) in checks.iter().enumerate() { f.levels[i] = c.is_active(); }
        f.sources = if by_src.is_active() { srcs.text().to_string() } else { String::new() };
        f.ids = ids.text().to_string();
        f.category = cat.text().to_string();
        f.user = user.text().to_string();
        *st.filter.borrow_mut() = f;
        load(st, false);
        update_actions(st);
    }
    d.close();
}

fn find(st: &Rc<State>) {
    let Some(q) = aerode::desktop::prompt("Find", "Find what:", "") else { return };
    let q = q.to_lowercase();
    if q.is_empty() { return; }
    let start = st.view.selection().selected().and_then(|(m, it)| m.path(&it).and_then(|p| p.indices().first().copied())).map(|i| i + 1).unwrap_or(0);
    let n = st.store.iter_n_children(None);
    for k in 0..n {
        let i = (start + k) % n.max(1);
        let Some(it) = st.store.iter_nth_child(None, i) else { continue };
        let ts: i64 = st.store.value(&it, 6).get().unwrap_or(0);
        let src = sval(&st.store, &it, 3);
        let hit = st.events.borrow().iter().any(|e| e.ts == ts && e.source == src && (e.message.to_lowercase().contains(&q) || e.source.to_lowercase().contains(&q) || e.category.to_lowercase().contains(&q) || e.id.to_string() == q));
        if hit {
            let Some(p) = st.store.path(&it) else { return };
            st.view.selection().select_path(&p);
            st.view.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, true, 0.5, 0.0);
            return;
        }
    }
    inform(Some(&st.m.win), "Event Viewer", "Event Viewer has finished searching the log. No match found.", "");
}

fn save_all(st: &Rc<State>, only_selected: bool) {
    let log = st.log.borrow().clone();
    let evs: Vec<Ev> = if only_selected { selected(st).into_iter().collect() } else { st.events.borrow().clone() };
    let fc = gtk::FileChooserNative::new(Some("Save As"), Some(&st.m.win), gtk::FileChooserAction::Save, Some("_Save"), Some("_Cancel"));
    fc.set_current_name(&format!("{log}.txt"));
    fc.set_do_overwrite_confirmation(true);
    for (n, p) in [("Text (Tab delimited) (*.txt)", "*.txt"), ("CSV (Comma Separated) (*.csv)", "*.csv"), ("XML (*.xml)", "*.xml")] { let f = gtk::FileFilter::new(); f.set_name(Some(n)); f.add_pattern(p); fc.add_filter(f); }
    if fc.run() != gtk::ResponseType::Accept { return; }
    let Some(p) = fc.filename() else { return };
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out = String::new();
    if ext == "xml" {
        out += "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Events>\n";
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        for e in &evs { out += &format!("  <Event><Level>{}</Level><TimeCreated>{}</TimeCreated><Provider>{}</Provider><EventID>{}</EventID><Task>{}</Task><Message>{}</Message></Event>\n", LEVELS[e.level], when(e.ts / 1_000_000), esc(&e.source), e.id, esc(&e.category), esc(&e.message)); }
        out += "</Events>\n";
    } else {
        let sep = if ext == "csv" { "," } else { "\t" };
        let cell = |s: &str| if ext == "csv" { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.replace(['\t', '\n'], " ") };
        out += &["Level", "Date and Time", "Source", "Event ID", "Task Category", "Message"].join(sep);
        out.push('\n');
        for e in &evs { out += &[cell(LEVELS[e.level]), cell(&when(e.ts / 1_000_000)), cell(&e.source), e.id.to_string(), cell(&e.category), cell(&e.message)].join(sep); out.push('\n'); }
    }
    if let Err(e) = std::fs::write(&p, out) { util::error("The events could not be saved.", &e.to_string()); }
}

fn clear_log(st: &Rc<State>) {
    let log = st.log.borrow().clone();
    let d = gtk::MessageDialog::new(Some(&st.m.win), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::None, &format!("Do you want to save \"{log}\" before clearing it?"));
    d.set_title("Event Viewer");
    d.set_secondary_text(Some("Clearing removes the archived journal files; the journal keeps only events written from now on."));
    d.add_button("Save and Clear", gtk::ResponseType::Yes);
    let c = d.add_button("Clear", gtk::ResponseType::No);
    if let Ok(c) = c.downcast::<gtk::Button>() { shield(&c); }
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let r = d.run();
    d.close();
    if r == gtk::ResponseType::Cancel || r == gtk::ResponseType::DeleteEvent { return; }
    if r == gtk::ResponseType::Yes { save_all(st, false); }
    let st2 = st.clone();
    elevate_sh("journalctl --rotate && journalctl --vacuum-time=1s", move |ok, out| { if report("The log could not be cleared.", ok, &out) { load(&st2, false); } });
}

fn log_properties(st: &Rc<State>) {
    let log = st.log.borrow().clone();
    let d = gtk::Dialog::with_buttons(Some("Log Properties"), Some(&st.m.win), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok)]);
    let nb = gtk::Notebook::new();
    let g = gtk::Grid::new();
    g.set_border_width(12);
    g.set_row_spacing(6);
    g.set_column_spacing(12);
    let setup = log == "Setup";
    let path = if setup { "/var/log/pacman.log".to_string() } else if std::path::Path::new("/var/log/journal").exists() { "/var/log/journal".into() } else { "/run/log/journal (volatile)".into() };
    let size_l = gtk::Label::new(Some("Calculating…"));
    let conf = std::fs::read_to_string("/etc/systemd/journald.conf").unwrap_or_default();
    let max = conf.lines().find_map(|l| l.trim().strip_prefix("SystemMaxUse=")).unwrap_or("10% of the file system, up to 4 GB").to_string();
    let rows: Vec<(&str, gtk::Widget)> = vec![
        ("Full Name:", gtk::Label::new(Some(&log)).upcast()),
        ("Log path:", gtk::Label::new(Some(&path)).upcast()),
        ("Log size:", size_l.clone().upcast()),
        ("Modified:", gtk::Label::new(Some(&mtime(std::path::Path::new(path.split(' ').next().unwrap_or(""))).map(when).unwrap_or_default())).upcast()),
    ];
    for (i, (k, w)) in rows.iter().enumerate() {
        let l = gtk::Label::new(Some(k)); l.set_xalign(0.0); g.attach(&l, 0, i as i32, 1, 1);
        if let Some(l) = w.downcast_ref::<gtk::Label>() { l.set_xalign(0.0); l.set_selectable(true); }
        g.attach(w, 1, i as i32, 1, 1);
    }
    let en = gtk::CheckButton::with_label("Enable logging");
    en.set_active(true);
    en.set_sensitive(false);
    g.attach(&en, 0, 5, 2, 1);
    let mx = gtk::Label::new(Some(&format!("Maximum log size: {}", if setup { "unlimited" } else { &max })));
    mx.set_xalign(0.0);
    g.attach(&mx, 0, 6, 2, 1);
    let l = label("When maximum event log size is reached:", "");
    g.attach(&l, 0, 7, 2, 1);
    let r1 = gtk::RadioButton::with_label("Overwrite events as needed (oldest events first)");
    let r2 = gtk::RadioButton::with_label_from_widget(&r1, "Do not overwrite events ( Clear logs manually )");
    if setup { r2.set_active(true); }
    r1.set_sensitive(false); r2.set_sensitive(false);
    g.attach(&r1, 0, 8, 2, 1);
    g.attach(&r2, 0, 9, 2, 1);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.show_all();
    bg(move || if setup { std::fs::metadata("/var/log/pacman.log").map(|m| gb(m.len())).unwrap_or_default() } else {
        sh(&["journalctl", "--disk-usage"]).split("take up ").nth(1).and_then(|s| s.split_whitespace().next()).map(|s| format!("{s} (all journal files)")).unwrap_or_else(|| "unknown".into())
    }, move |s| size_l.set_text(&s));
    d.run();
    d.close();
}

/// "Open Saved Log...": a journal file (journalctl --file) shown under Saved Logs.
fn open_saved(m: &Rc<Mmc>) {
    let fc = gtk::FileChooserNative::new(Some("Open Saved Log"), Some(&m.win), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
    let f = gtk::FileFilter::new(); f.set_name(Some("Journal Files (*.journal, *.journal~)")); f.add_pattern("*.journal"); f.add_pattern("*.journal~"); fc.add_filter(f);
    if fc.run() != gtk::ResponseType::Accept { return; }
    let Some(p) = fc.filename() else { return };
    let path = p.to_string_lossy().to_string();
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    // show it in a simple list (loaded once)
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    icon_column(&tv, "Level", 0, 1, 110);
    columns(&tv, &[("Date and Time", 2, 170), ("Source", 3, 150), ("Message", 4, 0)]);
    let v = vbox(0);
    let (h, extra) = mmc::head(&name, "Loading…");
    v.pack_start(&h, false, false, 0);
    v.pack_start(&scrolled(&tv), true, true, 0);
    m.set_center(&v);
    bg(move || sh(&["journalctl", "--file", &path, "-o", "json", "-r", "-n", "5000", "--no-pager", "-q"]), move |out| {
        let mut n = 0;
        for e in out.lines().filter_map(parse_json) {
            store.insert_with_values(None, &[(0, &pix(level_icon(e.level), 16)), (1, &LEVELS[e.level]), (2, &when(e.ts / 1_000_000)), (3, &e.source), (4, &e.message)]);
            n += 1;
        }
        extra.set_text(&format!("Number of events: {}", mmc::thousands(n)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn journal_json() {
        let l = r#"{"__CURSOR":"s=1;i=2","__REALTIME_TIMESTAMP":"1700000000000000","PRIORITY":"3","SYSLOG_IDENTIFIER":"sshd","_PID":"42","_UID":"0","MESSAGE":"error: kex_exchange_identification","_SYSTEMD_UNIT":"sshd.service","_HOSTNAME":"box"}"#;
        let e = parse_json(l).unwrap();
        assert_eq!(e.level, 1);
        assert_eq!(e.source, "sshd");
        assert_eq!(e.category, "sshd.service");
        assert_eq!(e.user, "SYSTEM");
        assert_eq!(e.ts, 1_700_000_000_000_000);
        let bin = r#"{"__REALTIME_TIMESTAMP":"1","MESSAGE":[104,105],"_TRANSPORT":"kernel","PRIORITY":"6"}"#;
        let e = parse_json(bin).unwrap();
        assert_eq!(e.message, "hi");
        assert_eq!(e.source, "kernel");
        assert_eq!(e.level, 3);
    }
    #[test]
    fn pacman_log() {
        let e = parse_pacman("[2026-10-30T12:34:56+0100] [ALPM] upgraded linux (6.11.1-1 -> 6.11.2-1)", "h").unwrap();
        assert_eq!(e.id, 3);
        assert_eq!(e.level, 3);
        assert_eq!(e.source, "ALPM");
        let e = parse_pacman("[2026-10-30T12:34:56+0100] [ALPM] warning: /etc/foo installed as /etc/foo.pacnew", "h").unwrap();
        assert_eq!(e.level, 2);
        let e = parse_pacman("[2019-01-01 10:00] [PACMAN] Running 'pacman -Syu'", "h").unwrap();
        assert_eq!(e.source, "PACMAN");
    }
    #[test]
    fn filter_ids() {
        let mut f = Filter::default();
        f.ids = "1,3,5-99,-76".into();
        let mut e = Ev::default();
        e.id = 76; assert!(!f.matches(&e));
        e.id = 50; assert!(f.matches(&e));
        e.id = 2; assert!(!f.matches(&e));
        f.levels[1] = true;
        e.id = 3; e.level = 2; assert!(!f.matches(&e));
        e.level = 1; assert!(f.matches(&e));
    }
}
