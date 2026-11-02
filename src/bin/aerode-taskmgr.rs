//! Windows Task Manager – Windows 7 Task Manager remake (reads /proc, systemd, EWMH).
//! All sampling runs on a worker thread; the window only applies finished snapshots.
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use aerode::{config, theme, util, x11::Wm};

const HIST: usize = 120;

// ------------------------------------------------------------------ sampling (worker thread)
#[derive(Clone, Default)]
struct Row { pid: u32, ppid: u32, name: String, user: String, uid: u32, cpu: u32, mem_kb: u64, desc: String, nice: i32 }

#[derive(Clone)]
struct Adapter { name: String, util: f64, speed_mbit: u64, up: bool }

#[derive(Default)]
struct Snap {
    rows: Vec<Row>,
    /// (total %, kernel %) for the whole machine, then per core
    cpu: (f64, f64),
    cores: Vec<(f64, f64)>,
    idle: u32,
    mem: HashMap<String, u64>,
    net: Vec<Adapter>,
    threads: u64,
    handles: String,
    uptime: u64,
}

#[derive(Default)]
struct Prev {
    cpu: Vec<(u64, u64, u64)>, // total, idle, kernel jiffies per /proc/stat line
    procs: HashMap<u32, u64>,
    net: HashMap<String, (u64, u64)>,
    net_t: Option<std::time::Instant>,
    users: HashMap<u32, String>,
    descs: Option<HashMap<String, String>>,
}

fn read(p: &str) -> String { std::fs::read_to_string(p).unwrap_or_default() }

fn users_by_uid() -> HashMap<u32, String> {
    read("/etc/passwd").lines().filter_map(|l| { let f: Vec<&str> = l.split(':').collect(); if f.len() > 2 { Some((f[2].parse().ok()?, f[0].to_string())) } else { None } }).collect()
}

/// Program name → the friendly name of its .desktop entry (Win7's "Description" column shows the
/// file description, e.g. "Windows Explorer").
fn desktop_names() -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut dirs: Vec<std::path::PathBuf> = vec![glib::user_data_dir().join("applications")];
    dirs.extend(glib::system_data_dirs().iter().map(|d| d.join("applications")));
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if e.path().extension().map(|x| x != "desktop").unwrap_or(true) { continue; }
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            let mut name = None;
            let mut exec = None;
            let mut in_main = false;
            for l in text.lines() {
                if l.starts_with('[') { in_main = l == "[Desktop Entry]"; continue; }
                if !in_main { continue; }
                if let Some(v) = l.strip_prefix("Name=") { name.get_or_insert(v.to_string()); }
                if let Some(v) = l.strip_prefix("Exec=") { exec.get_or_insert(v.to_string()); }
            }
            let (Some(name), Some(exec)) = (name, exec) else { continue };
            let mut words = exec.split_whitespace().filter(|w| !w.contains('=') || w.starts_with('/'));
            let mut prog = words.next().unwrap_or("");
            if prog == "env" || prog.ends_with("/env") { prog = words.next().unwrap_or(""); }
            let base = prog.rsplit('/').next().unwrap_or(prog).trim_matches('"').to_string();
            if !base.is_empty() { m.entry(base).or_insert(name); }
        }
    }
    m
}

fn sample(prev: &mut Prev) -> Snap {
    if prev.users.is_empty() { prev.users = users_by_uid(); }
    if prev.descs.is_none() { prev.descs = Some(desktop_names()); }
    let mut s = Snap::default();
    // --- CPU (total, idle, kernel)
    let mut cur = vec![];
    for l in read("/proc/stat").lines().filter(|l| l.starts_with("cpu")) {
        let n: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
        let total: u64 = n.iter().take(8).sum();
        let idle = n.get(3).copied().unwrap_or(0) + n.get(4).copied().unwrap_or(0);
        let kernel = n.get(2).copied().unwrap_or(0) + n.get(5).copied().unwrap_or(0) + n.get(6).copied().unwrap_or(0);
        cur.push((total, idle, kernel));
    }
    let pct = |i: usize| -> (f64, f64) {
        let (t, id, k) = cur[i];
        let (pt, pid, pk) = prev.cpu.get(i).copied().unwrap_or((0, 0, 0));
        let dt = t.saturating_sub(pt).max(1) as f64;
        (((1.0 - id.saturating_sub(pid) as f64 / dt) * 100.0).clamp(0.0, 100.0), (k.saturating_sub(pk) as f64 / dt * 100.0).clamp(0.0, 100.0))
    };
    if !cur.is_empty() {
        s.cpu = pct(0);
        s.cores = (1..cur.len()).map(pct).collect();
    }
    let dtot = cur.first().map(|c| c.0).unwrap_or(0).saturating_sub(prev.cpu.first().map(|c| c.0).unwrap_or(0)).max(1);
    s.idle = (100.0 - s.cpu.0).round().clamp(0.0, 99.0) as u32;
    // --- processes
    let descs = prev.descs.as_ref().unwrap();
    let mut ticks_now = HashMap::new();
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
            let stat = read(&format!("/proc/{pid}/stat"));
            let Some(rp) = stat.rfind(')') else { continue };
            let comm = stat[stat.find('(').unwrap_or(0) + 1..rp].to_string();
            let f: Vec<&str> = stat.get(rp + 2..).unwrap_or("").split_whitespace().collect();
            if f.len() < 20 { continue; }
            let ticks = f[11].parse::<u64>().unwrap_or(0) + f[12].parse::<u64>().unwrap_or(0);
            let ppid = f[1].parse().unwrap_or(0);
            let nice = f[16].parse().unwrap_or(0);
            s.threads += f[17].parse::<u64>().unwrap_or(1);
            let status = read(&format!("/proc/{pid}/status"));
            let mut uid = 0;
            let mut mem = 0;
            for l in status.lines() {
                if let Some(r) = l.strip_prefix("Uid:") { uid = r.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0); }
                // Win7 shows "Memory (Private Working Set)": RssAnon is the closest Linux equivalent
                if let Some(r) = l.strip_prefix("RssAnon:") { mem = r.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0); }
            }
            let cmd = read(&format!("/proc/{pid}/cmdline")).replace('\0', " ").trim().to_string();
            let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().trim_end_matches(" (deleted)").to_string()));
            // interpreters (python, sh…) are shown by the script they run, like Win7 shows the image
            let name = exe.filter(|e| !e.is_empty() && !e.starts_with("python") && !e.ends_with("sh") && !e.starts_with("perl")).unwrap_or(comm.clone());
            let desc = descs.get(&name).or_else(|| descs.get(&comm)).cloned()
                .unwrap_or_else(|| if cmd.is_empty() { format!("[{comm}]") } else { cmd.chars().take(200).collect() });
            let prev_t = prev.procs.get(&pid).copied().unwrap_or(ticks);
            let cpu = ((ticks.saturating_sub(prev_t.min(ticks))) as f64 * 100.0 / dtot as f64).round().min(99.0) as u32;
            ticks_now.insert(pid, ticks);
            let user = prev.users.get(&uid).cloned().unwrap_or_else(|| uid.to_string());
            s.rows.push(Row { pid, ppid, name, user, uid, cpu, mem_kb: mem, desc, nice });
        }
    }
    prev.procs = ticks_now;
    prev.cpu = cur;
    // --- memory
    s.mem = read("/proc/meminfo").lines().filter_map(|l| { let (k, v) = l.split_once(':')?; Some((k.to_string(), v.split_whitespace().next()?.parse().ok()?)) }).collect();
    s.handles = read("/proc/sys/fs/file-nr").split_whitespace().next().unwrap_or("0").to_string();
    s.uptime = read("/proc/uptime").split_whitespace().next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) as u64;
    // --- network
    let now = std::time::Instant::now();
    let secs = prev.net_t.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(1.0).max(0.05);
    prev.net_t = Some(now);
    let (mut wired, mut wireless) = (0, 0);
    let mut names: Vec<(String, u64, u64)> = read("/proc/net/dev").lines().skip(2).filter_map(|l| {
        let (name, rest) = l.split_once(':')?;
        let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if f.len() < 9 { return None; }
        Some((name.trim().to_string(), f[0], f[8]))
    }).filter(|(n, _, _)| n != "lo" && !n.starts_with("veth")).collect();
    names.sort();
    for (name, rx, tx) in names {
        let old = prev.net.insert(name.clone(), (rx, tx)).unwrap_or((rx, tx));
        let speed: u64 = read(&format!("/sys/class/net/{name}/speed")).trim().parse::<i64>().ok().filter(|v| *v > 0).map(|v| v as u64).unwrap_or(1000);
        let bps = ((rx.saturating_sub(old.0)) + (tx.saturating_sub(old.1))) as f64 * 8.0 / secs;
        let wifi = std::path::Path::new(&format!("/sys/class/net/{name}/wireless")).exists();
        let physical = std::path::Path::new(&format!("/sys/class/net/{name}/device")).exists();
        // Win7 names its adapters' connections
        let shown = if wifi { wireless += 1; if wireless == 1 { "Wireless Network Connection".to_string() } else { format!("Wireless Network Connection {wireless}") } }
            else if physical { wired += 1; if wired == 1 { "Local Area Connection".to_string() } else { format!("Local Area Connection {wired}") } }
            else { name.clone() };
        let up = read(&format!("/sys/class/net/{name}/operstate")).trim() != "down";
        s.net.push(Adapter { name: shown, util: (bps / (speed as f64 * 1e6) * 100.0).min(100.0), speed_mbit: speed, up });
    }
    s
}

/// The sampler thread: one snapshot per request, delivered to the main loop.
fn sampler() -> (std::sync::mpsc::Sender<()>, async_channel::Receiver<Snap>) {
    let (req_tx, req_rx) = std::sync::mpsc::channel::<()>();
    let (tx, rx) = async_channel::bounded(2);
    std::thread::spawn(move || {
        let mut prev = Prev::default();
        while req_rx.recv().is_ok() {
            if tx.send_blocking(sample(&mut prev)).is_err() { break; }
        }
    });
    (req_tx, rx)
}

/// Run `f` on a helper thread and hand its result to `done` on the main loop.
fn in_background<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || { let _ = tx.send_blocking(f()); });
    glib::MainContext::default().spawn_local(async move { if let Ok(v) = rx.recv().await { done(v); } });
}

// ------------------------------------------------------------------ options
struct Opts { on_top: bool, min_on_use: bool, hide_min: bool, speed_ms: u64, tab: u32, all_users: bool, one_graph: bool, kernel: bool }

impl Opts {
    fn load() -> Opts {
        let text = std::fs::read_to_string(config::conf_dir().join("taskmgr.conf")).unwrap_or_default();
        let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|v| v.trim().to_string());
        let b = |k: &str, d: bool| get(k).map(|v| v == "1").unwrap_or(d);
        Opts {
            on_top: b("always_on_top", true), min_on_use: b("minimize_on_use", true), hide_min: b("hide_when_minimized", false),
            speed_ms: get("update_speed").and_then(|v| v.parse().ok()).unwrap_or(1000), tab: get("tab").and_then(|v| v.parse().ok()).unwrap_or(0),
            all_users: b("all_users", false), one_graph: b("one_graph", false), kernel: b("kernel_times", false),
        }
    }
    fn save(&self) {
        let s = format!("always_on_top={}\nminimize_on_use={}\nhide_when_minimized={}\nupdate_speed={}\ntab={}\nall_users={}\none_graph={}\nkernel_times={}\n",
            self.on_top as u8, self.min_on_use as u8, self.hide_min as u8, self.speed_ms, self.tab, self.all_users as u8, self.one_graph as u8, self.kernel as u8);
        let _ = std::fs::create_dir_all(config::conf_dir());
        let _ = std::fs::write(config::conf_dir().join("taskmgr.conf"), s);
    }
}

// ------------------------------------------------------------------ window
struct Tm {
    win: gtk::ApplicationWindow,
    nb: gtk::Notebook,
    wm: Option<Rc<Wm>>,
    apps: gtk::ListStore,
    apps_view: gtk::TreeView,
    procs: gtk::ListStore,
    proc_view: gtk::TreeView,
    services: gtk::ListStore,
    svc_view: gtk::TreeView,
    users: gtk::ListStore,
    net_list: gtk::ListStore,
    status: [gtk::Label; 3],
    perf: RefCell<Perf>,
    perf_area: gtk::DrawingArea,
    perf_labels: RefCell<HashMap<&'static str, gtk::Label>>,
    net_area: gtk::DrawingArea,
    net_hist: RefCell<Vec<(String, VecDeque<f64>)>>,
    rows: RefCell<Vec<Row>>,
    opts: RefCell<Opts>,
    my_uid: u32,
    req: std::sync::mpsc::Sender<()>,
    in_flight: Cell<bool>,
    last_tick: Cell<std::time::Instant>,
    tray: RefCell<Option<glib::Object>>,
}

#[derive(Default)]
struct Perf {
    cpu: (f64, f64),
    total: VecDeque<(f64, f64)>,
    cores: Vec<VecDeque<(f64, f64)>>,
    mem: VecDeque<(f64, f64)>,
    mem_pct: f64,
    mem_used_gb: f64,
    ticks: usize,
}

fn fmt_k(kb: u64) -> String { format!("{} K", group(kb)) }

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); }
    o
}

fn column(view: &gtk::TreeView, title: &str, col: i32, right: bool, width: i32) -> gtk::TreeViewColumn {
    let r = gtk::CellRendererText::new();
    if right { r.set_xalign(1.0); }
    let c = gtk::TreeViewColumn::new();
    c.set_title(title);
    TreeViewColumnExt::pack_start(&c, &r, true);
    TreeViewColumnExt::add_attribute(&c, &r, "text", col);
    c.set_resizable(true);
    c.set_sort_column_id(col);
    if width > 0 { c.set_fixed_width(width); c.set_sizing(gtk::TreeViewColumnSizing::Fixed); }
    view.append_column(&c);
    c
}

fn scrolled(w: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(w);
    sw
}

/// A Win7 group box: light rounded frame with the caption breaking the top edge.
fn group_box(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, title: &str) {
    cr.save().ok();
    cr.select_font_face("Segoe UI", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(12.0);
    let tw = cr.text_extents(title).map(|e| e.x_advance()).unwrap_or(0.0);
    let top = y + 7.5;
    cr.set_line_width(1.0);
    cr.set_source_rgb(0.835, 0.875, 0.898);
    let (l, r, b) = (x + 0.5, x + w - 0.5, y + h - 0.5);
    cr.move_to(l + 6.0 + tw + 4.0, top);
    cr.line_to(r - 2.0, top);
    cr.arc(r - 2.0, top + 2.0, 2.0, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.line_to(r, b - 2.0);
    cr.arc(r - 2.0, b - 2.0, 2.0, 0.0, std::f64::consts::FRAC_PI_2);
    cr.line_to(l + 2.0, b);
    cr.arc(l + 2.0, b - 2.0, 2.0, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.line_to(l, top + 2.0);
    cr.arc(l + 2.0, top + 2.0, 2.0, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.line_to(l + 4.0, top);
    cr.stroke().ok();
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.move_to(x + 8.0, y + 12.0);
    cr.show_text(title).ok();
    cr.restore().ok();
}

/// Green-on-black history graph with the classic scrolling grid; (value, kernel) pairs, kernel drawn red.
fn draw_graph(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, data: &VecDeque<(f64, f64)>, color: (f64, f64, f64), scroll: usize, kernel: bool, scale: f64) {
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.fill_preserve().ok();
    cr.clip();
    cr.set_line_width(1.0);
    cr.set_source_rgb(0.0, 0.5, 0.25);
    let step = 12.0;
    let off = (scroll as f64 * 3.0) % step;
    let mut gx = x + w - off;
    while gx > x { cr.move_to(gx.floor() + 0.5, y); cr.line_to(gx.floor() + 0.5, y + h); gx -= step; }
    let mut gy = y + h;
    while gy > y { cr.move_to(x, gy.floor() + 0.5); cr.line_to(x + w, gy.floor() + 0.5); gy -= step; }
    cr.stroke().ok();
    let n = data.len();
    let line = |cr: &cairo::Context, pick: &dyn Fn(&(f64, f64)) -> f64| {
        for (i, v) in data.iter().enumerate() {
            let px = x + w - (n - 1 - i) as f64 * 3.0;
            let py = y + h - (pick(v) / scale).clamp(0.0, 1.0) * (h - 1.0) - 0.5;
            if i == 0 { cr.move_to(px, py) } else { cr.line_to(px, py) }
        }
        cr.stroke().ok();
    };
    if kernel { cr.set_source_rgb(1.0, 0.0, 0.0); line(cr, &|v| v.1); }
    cr.set_source_rgb(color.0, color.1, color.2);
    line(cr, &|v| v.0);
    cr.restore().ok();
}

/// The LED-style usage meter (lit segments green, kernel share red).
fn draw_meter(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, pct: f64, kernel: Option<f64>, label: &str) {
    cr.rectangle(x, y, w, h);
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.fill().ok();
    let bar_h = h - 22.0;
    let segs = (bar_h / 3.0) as i32;
    let lit = ((pct / 100.0) * segs as f64).round() as i32;
    let red = kernel.map(|k| ((k / 100.0) * segs as f64).round() as i32).unwrap_or(0);
    for i in 0..segs {
        let sy = y + 4.0 + bar_h - (i + 1) as f64 * 3.0;
        for half in 0..2 {
            let sx = x + w / 2.0 - 16.0 + half as f64 * 17.0;
            if i < red { cr.set_source_rgb(1.0, 0.0, 0.0) } else if i < lit { cr.set_source_rgb(0.0, 1.0, 0.0) } else { cr.set_source_rgb(0.0, 0.32, 0.0) }
            cr.rectangle(sx, sy, 15.0, 2.0);
            cr.fill().ok();
        }
    }
    cr.set_source_rgb(0.0, 1.0, 0.0);
    cr.select_font_face("Segoe UI", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.0);
    let tw = cr.text_extents(label).map(|e| e.width()).unwrap_or(0.0);
    cr.move_to(x + (w - tw) / 2.0, y + h - 6.0);
    cr.show_text(label).ok();
}

/// Win7's notification-area CPU meter.
fn tray_pixbuf(pct: f64) -> Option<Pixbuf> {
    let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, 16, 16).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        cr.rectangle(2.0, 1.0, 12.0, 14.0);
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.fill().ok();
        cr.rectangle(2.5, 1.5, 11.0, 13.0);
        cr.set_source_rgb(0.55, 0.55, 0.55);
        cr.set_line_width(1.0);
        cr.stroke().ok();
        let hgt = (12.0 * pct / 100.0).round().max(if pct > 0.5 { 1.0 } else { 0.0 });
        cr.rectangle(4.0, 14.0 - hgt, 8.0, hgt);
        cr.set_source_rgb(0.0, 1.0, 0.0);
        cr.fill().ok();
    }
    gdk::pixbuf_get_from_surface(&surf, 0, 0, 16, 16)
}

impl Tm {
    /// Ask the sampler for a new snapshot (at most one outstanding).
    fn request(&self) {
        if self.in_flight.get() { return; }
        if self.req.send(()).is_ok() { self.in_flight.set(true); self.last_tick.set(std::time::Instant::now()); }
    }

    fn apply(self: &Rc<Self>, s: Snap) {
        self.in_flight.set(false);
        // --- performance history
        let mut perf = self.perf.borrow_mut();
        perf.ticks += 1;
        perf.cpu = s.cpu;
        let push = |q: &mut VecDeque<(f64, f64)>, v: (f64, f64)| { q.push_back(v); if q.len() > HIST { q.pop_front(); } };
        let cpu = s.cpu;
        push(&mut perf.total, cpu);
        if perf.cores.len() != s.cores.len() { perf.cores = vec![VecDeque::new(); s.cores.len()]; }
        for (q, v) in perf.cores.iter_mut().zip(&s.cores) { push(q, *v); }
        let total = *s.mem.get("MemTotal").unwrap_or(&1) as f64;
        let avail = *s.mem.get("MemAvailable").unwrap_or(&0) as f64;
        perf.mem_pct = ((1.0 - avail / total) * 100.0).clamp(0.0, 100.0);
        perf.mem_used_gb = (total - avail) / 1048576.0;
        let mp = perf.mem_pct;
        push(&mut perf.mem, (mp, 0.0));
        drop(perf);
        // --- processes (update rows in place to keep selection and scroll position)
        let all = self.opts.borrow().all_users;
        let mut shown_rows: Vec<Row> = s.rows.iter().filter(|r| (all || r.uid == self.my_uid) && r.ppid != 2).cloned().collect();
        // kernel threads are one "System" process, as in Windows
        let kcpu: u32 = s.rows.iter().filter(|r| r.ppid == 2).map(|r| r.cpu).sum();
        if let Some(k) = shown_rows.iter_mut().find(|r| r.pid == 2) {
            k.name = "System".into();
            k.user = "SYSTEM".into();
            k.desc = "Linux Kernel & System".into();
            k.cpu = (k.cpu + kcpu).min(99);
        }
        if all {
            shown_rows.push(Row { pid: 0, name: "System Idle Process".into(), user: "SYSTEM".into(), cpu: s.idle, mem_kb: 24, desc: "Percentage of time the processor is idle".into(), ..Default::default() });
        }
        let mut have: HashMap<u32, gtk::TreeIter> = HashMap::new();
        if let Some(it) = self.procs.iter_first() {
            loop {
                let pid: u32 = self.procs.value(&it, 7).get().unwrap_or(0);
                have.insert(pid, it.clone());
                if !self.procs.iter_next(&it) { break; }
            }
        }
        let mut seen = HashSet::new();
        for p in &shown_rows {
            seen.insert(p.pid);
            let cpu_s = format!("{:02}", p.cpu);
            let mem_s = fmt_k(p.mem_kb);
            match have.get(&p.pid) {
                None => { self.procs.insert_with_values(None, &[(0, &p.name), (1, &p.user), (2, &cpu_s), (3, &mem_s), (4, &p.desc), (5, &p.mem_kb), (6, &p.cpu), (7, &p.pid)]); }
                Some(it) => {
                // only touch changed cells: fewer re-sorts and redraws
                let it = it.clone();
                let cur_cpu: u32 = self.procs.value(&it, 6).get().unwrap_or(u32::MAX);
                let cur_mem: u64 = self.procs.value(&it, 5).get().unwrap_or(u64::MAX);
                if cur_cpu != p.cpu { self.procs.set(&it, &[(2, &cpu_s), (6, &p.cpu)]); }
                if cur_mem != p.mem_kb { self.procs.set(&it, &[(3, &mem_s), (5, &p.mem_kb)]); }
                }
            }
        }
        for (pid, it) in have { if !seen.contains(&pid) { self.procs.remove(&it); } }
        // --- status bar (Win7 counts the processes shown)
        self.status[0].set_text(&format!("Processes: {}", shown_rows.len()));
        self.status[1].set_text(&format!("CPU Usage: {:.0}%", cpu.0));
        self.status[2].set_text(&format!("Physical Memory: {:.0}%", mp));
        // --- performance text block
        {
            let l = self.perf_labels.borrow();
            let kb = |k: &str| s.mem.get(k).copied().unwrap_or(0) / 1024;
            let set = |k: &str, v: String| if let Some(x) = l.get(k) { x.set_text(&v) };
            set("total", kb("MemTotal").to_string());
            set("cached", (kb("Cached") + kb("Buffers")).to_string());
            set("available", kb("MemAvailable").to_string());
            set("free", kb("MemFree").to_string());
            set("paged", kb("SReclaimable").to_string());
            set("nonpaged", (kb("SUnreclaim") + kb("KernelStack")).to_string());
            set("handles", s.handles.clone());
            set("threads", s.threads.to_string());
            set("processes", s.rows.len().to_string());
            let up = s.uptime;
            set("uptime", format!("{}:{:02}:{:02}:{:02}", up / 86400, up / 3600 % 24, up / 60 % 60, up % 60));
            set("commit", format!("{} / {}", s.mem.get("Committed_AS").copied().unwrap_or(0) / 1048576, (s.mem.get("CommitLimit").copied().unwrap_or(0) + 1048575) / 1048576));
        }
        // --- networking
        {
            let mut nh = self.net_hist.borrow_mut();
            nh.retain(|(n, _)| s.net.iter().any(|a| &a.name == n));
            for a in &s.net {
                let i = match nh.iter().position(|(n, _)| n == &a.name) { Some(i) => i, None => { nh.push((a.name.clone(), VecDeque::new())); nh.len() - 1 } };
                let q = &mut nh[i].1;
                q.push_back(a.util);
                if q.len() > HIST { q.pop_front(); }
            }
            self.net_list.clear();
            for a in &s.net {
                let speed = if a.speed_mbit >= 1000 { format!("{} Gbps", a.speed_mbit / 1000) } else { format!("{} Mbps", a.speed_mbit) };
                self.net_list.insert_with_values(None, &[(0, &a.name), (1, &format!("{:.2} %", a.util)), (2, &speed), (3, &if a.up { "Connected" } else { "Disconnected" })]);
            }
        }
        *self.rows.borrow_mut() = s.rows;
        if let Some(t) = self.tray.borrow().as_ref() {
            if let Some(p) = tray_pixbuf(cpu.0) { t.set_property("pixbuf", &p); }
            t.set_property("tooltip-text", format!("CPU Usage: {:.0}%", cpu.0));
        }
        self.perf_area.queue_draw();
        self.net_area.queue_draw();
        self.refresh_apps();
    }

    fn refresh_apps(&self) {
        let Some(wm) = &self.wm else { return };
        wm.drain();
        let clients = wm.clients();
        let mut have: HashMap<u32, gtk::TreeIter> = HashMap::new();
        if let Some(it) = self.apps.iter_first() {
            loop { let id: u32 = self.apps.value(&it, 2).get().unwrap_or(0); have.insert(id, it.clone()); if !self.apps.iter_next(&it) { break; } }
        }
        let mut seen = HashSet::new();
        for c in clients {
            seen.insert(c.id);
            match have.get(&c.id) {
                Some(it) => {
                    let t: String = self.apps.value(it, 0).get().unwrap_or_default();
                    if t != c.title { self.apps.set(it, &[(0, &c.title)]); }
                }
                None => {
                    let icon = wm.icon(c.id, 16).or_else(|| util::pixbuf("application-x-executable", 16));
                    // Win7 shows "Not Responding" for hung apps; we can't know cheaply → Running
                    self.apps.insert_with_values(None, &[(0, &c.title), (1, &"Running"), (2, &c.id), (3, &c.pid), (4, &icon)]);
                }
            }
        }
        for (id, it) in have { if !seen.contains(&id) { self.apps.remove(&it); } }
    }

    fn refresh_services(self: &Rc<Self>) {
        let me = self.clone();
        in_background(|| {
            let out = util::output(&["systemctl", "list-units", "--type=service", "--all", "--no-legend", "--plain", "--no-pager"]).unwrap_or_default();
            let info: HashMap<String, (String, String)> = util::output(&["systemctl", "show", "--type=service", "-p", "Id,MainPID,Slice", "--no-pager", "*"])
                .unwrap_or_default().split("\n\n").filter_map(|b| {
                    let (mut id, mut pid, mut slice) = (None, None, String::new());
                    for l in b.lines() {
                        if let Some(v) = l.strip_prefix("Id=") { id = Some(v.to_string()) }
                        if let Some(v) = l.strip_prefix("MainPID=") { pid = Some(v.to_string()) }
                        if let Some(v) = l.strip_prefix("Slice=") { slice = v.trim_end_matches(".slice").to_string() }
                    }
                    Some((id?, (pid.filter(|p| p != "0").unwrap_or_default(), slice)))
                }).collect();
            let mut rows = vec![];
            for l in out.lines() {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() < 4 { continue; }
                let name = f[0].trim_end_matches(".service").to_string();
                let status = if f[3] == "running" { "Running" } else { "Stopped" };
                let desc = f[4..].join(" ");
                let (pid, group) = info.get(f[0]).cloned().unwrap_or_default();
                rows.push((name, pid, desc, status.to_string(), group));
            }
            rows
        }, move |rows| {
            let sel = me.svc_view.selection().selected().map(|(m, it)| m.value(&it, 0).get::<String>().unwrap_or_default());
            me.services.clear();
            for (n, p, d, s, g) in rows {
                let it = me.services.insert_with_values(None, &[(0, &n), (1, &p), (2, &d), (3, &s), (4, &g)]);
                if sel.as_deref() == Some(n.as_str()) { me.svc_view.selection().select_iter(&it); }
            }
        });
    }

    fn refresh_users(self: &Rc<Self>) {
        let me = self.clone();
        in_background(|| {
            let out = util::output(&["loginctl", "list-sessions", "--no-legend"]).unwrap_or_default();
            out.lines().filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() < 3 { return None; }
                let props = util::output(&["loginctl", "show-session", f[0], "-p", "State", "-p", "Type", "-p", "Remote", "-p", "RemoteHost"]).unwrap_or_default();
                let get = |k: &str| props.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
                let st = if get("State") == "active" { "Active" } else { "Disconnected" };
                let session = if get("Remote") == "yes" { "RDP-Tcp".to_string() } else if get("Type") == "tty" { "Console (tty)".to_string() } else { "Console".to_string() };
                Some((f[2].to_string(), f[0].to_string(), st.to_string(), get("RemoteHost"), session))
            }).collect::<Vec<_>>()
        }, move |rows| {
            me.users.clear();
            for (u, id, st, client, sess) in rows { me.users.insert_with_values(None, &[(0, &u), (1, &id), (2, &st), (3, &client), (4, &sess)]); }
        });
    }

    fn selected_proc(&self) -> Option<(u32, String)> {
        let (m, it) = self.proc_view.selection().selected()?;
        Some((m.value(&it, 7).get().unwrap_or(0), m.value(&it, 0).get().unwrap_or_default()))
    }

    fn confirm(&self, primary: &str, secondary: &str, ok: &str) -> bool {
        let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::None, primary);
        d.set_title("Windows Task Manager");
        // above an "Always On Top" Task Manager, or it would hide behind it
        d.set_keep_above(true);
        d.set_secondary_text(Some(secondary));
        d.add_button(ok, gtk::ResponseType::Ok);
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        d.set_default_response(gtk::ResponseType::Cancel);
        aerode::sounds::play(aerode::sounds::Event::Exclamation);
        let r = d.run();
        d.close();
        r == gtk::ResponseType::Ok
    }

    /// End processes: SIGTERM, then SIGKILL for whatever is still there after 3 s (Win7's End Process
    /// is unconditional). Processes of other users go through pkexec on a helper thread.
    fn terminate(self: &Rc<Self>, pids: Vec<u32>) {
        let start_times: HashMap<u32, String> = pids.iter().map(|p| (*p, start_time(*p))).collect();
        let mut denied = vec![];
        for &p in &pids {
            if p == 0 { continue; }
            if unsafe { libc::kill(p as i32, libc::SIGTERM) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) { denied.push(p); }
        }
        {
            let pids = pids.clone();
            glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
                for p in pids { if start_time(p) == start_times[&p] && !start_times[&p].is_empty() { unsafe { libc::kill(p as i32, libc::SIGKILL); } } }
            });
        }
        if !denied.is_empty() {
            let me = self.clone();
            in_background(move || {
                let mut args = vec!["kill".to_string()];
                args.extend(denied.iter().map(|p| p.to_string()));
                std::process::Command::new("pkexec").args(&args).stdin(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
            }, move |ok| {
                if !ok { util::error("Unable to Terminate Process", "The operation could not be completed.\n\nAccess is denied."); }
                me.request();
            });
        }
        let me = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || me.request());
    }

    fn end_process(self: &Rc<Self>) {
        let Some((pid, name)) = self.selected_proc() else { return };
        if pid == 0 { return; }
        if self.confirm(&format!("Do you want to end \"{name}\"?"),
            "If an open program is associated with this process, it will close and you will lose any unsaved data. If you end a system process, it might result in system instability. Are you sure you want to continue?",
            "End process") { self.terminate(vec![pid]); }
    }

    fn end_tree(self: &Rc<Self>) {
        let Some((pid, name)) = self.selected_proc() else { return };
        if pid == 0 { return; }
        if !self.confirm(&format!("Do you want to end the process tree of \"{name}\"?"),
            "If open programs are associated with this process tree, they will close and you will lose any unsaved data. If you end a system process, it might result in system instability. Are you sure you want to continue?",
            "End process tree") { return; }
        // children before parents
        let rows = self.rows.borrow();
        let mut order = vec![];
        let mut stack = vec![pid];
        while let Some(p) = stack.pop() {
            order.push(p);
            stack.extend(rows.iter().filter(|r| r.ppid == p && r.pid != p).map(|r| r.pid));
        }
        drop(rows);
        order.reverse();
        self.terminate(order);
    }

    fn set_priority(self: &Rc<Self>, nice: i32) {
        let Some((pid, name)) = self.selected_proc() else { return };
        if !self.confirm(&format!("Do you want to change the priority of \"{name}\"?"),
            "Changing the priority of certain processes could cause system instability.", "Change priority") { return; }
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, pid, nice) } != 0 {
            let me = self.clone();
            in_background(move || std::process::Command::new("pkexec").args(["renice", "-n", &nice.to_string(), "-p", &pid.to_string()]).stdin(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false),
                move |ok| { if !ok { util::error("Unable to Change Priority", "The operation could not be completed.\n\nAccess is denied."); } me.request(); });
        }
    }

    fn select_pid(&self, pid: u32) {
        if pid == 0 { return; }
        if let Some(it) = self.procs.iter_first() {
            loop {
                let p: u32 = self.procs.value(&it, 7).get().unwrap_or(0);
                if p == pid {
                    self.proc_view.selection().select_iter(&it);
                    if let Some(path) = self.procs.path(&it) { self.proc_view.scroll_to_cell(Some(&path), None::<&gtk::TreeViewColumn>, true, 0.5, 0.0); }
                    return;
                }
                if !self.procs.iter_next(&it) { break; }
            }
        }
    }

    fn selected_window(&self) -> Option<(u32, u32)> {
        let (m, it) = self.apps_view.selection().selected()?;
        Some((m.value(&it, 2).get().unwrap_or(0), m.value(&it, 3).get().unwrap_or(0)))
    }

    fn switch_to(&self) {
        let (Some(wm), Some((id, _))) = (&self.wm, self.selected_window()) else { return };
        wm.activate(id);
        if self.opts.borrow().min_on_use { self.win.iconify(); }
    }
}

fn start_time(pid: u32) -> String {
    let s = read(&format!("/proc/{pid}/stat"));
    s.rfind(')').and_then(|r| s.get(r + 2..)).and_then(|f| f.split_whitespace().nth(19)).unwrap_or("").to_string()
}

fn new_task(parent: &gtk::Window) {
    let d = gtk::Dialog::with_buttons(Some("Create New Task"), Some(parent), gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel), ("_Browse...", gtk::ResponseType::Other(1))]);
    d.set_resizable(false);
    d.set_keep_above(true);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(10);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.pack_start(&util::image("system-run", 32), false, false, 0);
    let l = gtk::Label::new(Some("Type the name of a program, folder, document, or Internet resource, and Windows will open it for you."));
    l.set_line_wrap(true);
    l.set_max_width_chars(44);
    l.set_xalign(0.0);
    top.pack_start(&l, true, true, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.pack_start(&gtk::Label::with_mnemonic("_Open:"), false, false, 0);
    let e = gtk::Entry::new();
    e.set_activates_default(true);
    row.pack_start(&e, true, true, 0);
    let admin = gtk::CheckButton::with_label("Create this task with administrative privileges.");
    c.pack_start(&top, false, false, 0);
    c.pack_start(&row, false, false, 0);
    c.pack_start(&admin, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    loop {
        match d.run() {
            gtk::ResponseType::Ok => {
                let t = e.text().to_string();
                if !t.trim().is_empty() { if admin.is_active() { util::spawn(&["pkexec", "sh", "-c", &t]); } else { util::run_command(&t); } }
                break;
            }
            gtk::ResponseType::Other(1) => {
                let fc = gtk::FileChooserDialog::with_buttons(Some("Browse"), Some(&d), gtk::FileChooserAction::Open, &[("_Open", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
                fc.set_keep_above(true);
                fc.set_default_size(700, 480);
                if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() { e.set_text(&glib::shell_quote(&p).to_string_lossy()); } }
                fc.close();
            }
            _ => break,
        }
    }
    d.close();
}

fn build(app: &gtk::Application) {
    theme::init_app();
    theme::load_css(".tm-status label { padding: 0 10px; } .tm-perf label { font-size: 9pt; }");
    let opts = Opts::load();
    let win = gtk::ApplicationWindow::new(app);
    win.set_title("Windows Task Manager");
    win.set_default_size(520, 560);
    win.set_icon_name(Some(util::first_icon(&["utilities-system-monitor"])));
    let accel = gtk::AccelGroup::new();
    win.add_accel_group(&accel);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menubar = gtk::MenuBar::new();
    root.pack_start(&menubar, false, false, 0);
    let nb = gtk::Notebook::new();
    nb.set_margin_start(6);
    nb.set_margin_end(6);
    nb.set_margin_top(4);
    nb.set_margin_bottom(4);
    root.pack_start(&nb, true, true, 0);
    let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    statusbar.style_context().add_class("statusbar");
    statusbar.style_context().add_class("tm-status");
    let status = [gtk::Label::new(None), gtk::Label::new(None), gtk::Label::new(None)];
    for (i, s) in status.iter().enumerate() {
        if i > 0 { statusbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 0); }
        s.set_xalign(0.0);
        s.set_width_chars(if i == 0 { 14 } else { 18 });
        statusbar.pack_start(s, false, false, 0);
    }
    root.pack_start(&statusbar, false, false, 0);
    win.add(&root);

    // ---------- Applications: task, status, window id, pid, icon
    let apps = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::U32, glib::Type::U32, Pixbuf::static_type()]);
    let apps_view = gtk::TreeView::with_model(&apps);
    {
        let c = gtk::TreeViewColumn::new();
        c.set_title("Task");
        let ri = gtk::CellRendererPixbuf::new();
        let rt = gtk::CellRendererText::new();
        TreeViewColumnExt::pack_start(&c, &ri, false);
        TreeViewColumnExt::pack_start(&c, &rt, true);
        TreeViewColumnExt::add_attribute(&c, &ri, "pixbuf", 4);
        TreeViewColumnExt::add_attribute(&c, &rt, "text", 0);
        c.set_resizable(true);
        c.set_sort_column_id(0);
        c.set_fixed_width(320);
        c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        apps_view.append_column(&c);
    }
    column(&apps_view, "Status", 1, false, 0);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(8);
    page.pack_start(&scrolled(&apps_view), true, true, 0);
    let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let (end, sw_to, newt) = (gtk::Button::with_mnemonic("_End Task"), gtk::Button::with_mnemonic("_Switch To"), gtk::Button::with_mnemonic("_New Task..."));
    bb.pack_end(&newt, false, false, 0);
    bb.pack_end(&sw_to, false, false, 0);
    bb.pack_end(&end, false, false, 0);
    page.pack_start(&bb, false, false, 0);
    nb.append_page(&page, Some(&gtk::Label::new(Some("Applications"))));

    // ---------- Processes: name, user, cpu text, mem text, desc, mem sort, cpu sort, pid
    let procs = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U64, glib::Type::U32, glib::Type::U32]);
    let proc_view = gtk::TreeView::with_model(&procs);
    column(&proc_view, "Image Name", 0, false, 130);
    column(&proc_view, "User Name", 1, false, 80);
    column(&proc_view, "CPU", 2, true, 40).set_sort_column_id(6);
    column(&proc_view, "Memory (Private Working Set)", 3, true, 90).set_sort_column_id(5);
    column(&proc_view, "Description", 4, false, 0);
    proc_view.set_search_column(0);
    // Windows sorts names without regard to case
    for col in [0u32, 1, 4] {
        procs.set_sort_func(gtk::SortColumn::Index(col), move |m, a, b| {
            let (x, y) = (m.value(a, col as i32).get::<String>().unwrap_or_default().to_lowercase(), m.value(b, col as i32).get::<String>().unwrap_or_default().to_lowercase());
            x.cmp(&y)
        });
    }
    procs.set_sort_column_id(gtk::SortColumn::Index(0), gtk::SortType::Ascending);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(8);
    page.pack_start(&scrolled(&proc_view), true, true, 0);
    let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let show_all = gtk::CheckButton::with_mnemonic("_Show processes from all users");
    show_all.set_active(opts.all_users);
    let endp = gtk::Button::with_mnemonic("_End Process");
    bb.pack_start(&show_all, false, false, 0);
    bb.pack_end(&endp, false, false, 0);
    page.pack_start(&bb, false, false, 0);
    nb.append_page(&page, Some(&gtk::Label::new(Some("Processes"))));

    // ---------- Services
    let services = gtk::ListStore::new(&[glib::Type::STRING; 5]);
    let svc_view = gtk::TreeView::with_model(&services);
    column(&svc_view, "Name", 0, false, 140);
    column(&svc_view, "PID", 1, true, 50);
    column(&svc_view, "Description", 2, false, 200);
    column(&svc_view, "Status", 3, false, 70);
    column(&svc_view, "Group", 4, false, 0);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(8);
    page.pack_start(&scrolled(&svc_view), true, true, 0);
    let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let svcb = gtk::Button::with_mnemonic("_Services...");
    bb.pack_end(&svcb, false, false, 0);
    page.pack_start(&bb, false, false, 0);
    nb.append_page(&page, Some(&gtk::Label::new(Some("Services"))));

    // ---------- Performance
    let perf_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    perf_page.set_border_width(8);
    perf_page.style_context().add_class("tm-perf");
    let perf_area = gtk::DrawingArea::new();
    perf_area.set_size_request(480, 300);
    perf_page.pack_start(&perf_area, true, true, 0);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(6);
    grid.set_row_spacing(1);
    grid.set_column_homogeneous(true);
    let mut labels = HashMap::new();
    let blocks: [(&str, &[(&str, &'static str)], i32, i32); 3] = [
        ("Physical Memory (MB)", &[("Total", "total"), ("Cached", "cached"), ("Available", "available"), ("Free", "free")], 0, 0),
        ("Kernel Memory (MB)", &[("Paged", "paged"), ("Nonpaged", "nonpaged")], 0, 1),
        ("System", &[("Handles", "handles"), ("Threads", "threads"), ("Processes", "processes"), ("Up Time", "uptime"), ("Commit (GB)", "commit")], 1, 0),
    ];
    for (title, items, col, row) in blocks.iter() {
        let fr = gtk::Frame::new(Some(title));
        let g = gtk::Grid::new();
        g.set_border_width(4);
        g.set_column_spacing(12);
        for (i, (l, key)) in items.iter().enumerate() {
            let a = gtk::Label::new(Some(l));
            a.set_xalign(0.0);
            a.set_hexpand(true);
            let v = gtk::Label::new(Some("0"));
            v.set_xalign(1.0);
            g.attach(&a, 0, i as i32, 1, 1);
            g.attach(&v, 1, i as i32, 1, 1);
            labels.insert(*key, v);
        }
        fr.add(&g);
        fr.set_valign(gtk::Align::Start);
        grid.attach(&fr, *col, *row, 1, 1);
    }
    perf_page.pack_start(&grid, false, false, 0);
    let rm = gtk::Button::with_mnemonic("_Resource Monitor...");
    rm.set_halign(gtk::Align::End);
    rm.connect_clicked(|_| { if !util::spawn(&["gnome-system-monitor"]) && !util::spawn(&["plasma-systemmonitor"]) { util::spawn(&["aerode-cmd", "-e", "htop"]); } });
    perf_page.pack_start(&rm, false, false, 0);
    nb.append_page(&perf_page, Some(&gtk::Label::new(Some("Performance"))));

    // ---------- Networking
    let net_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    net_page.set_border_width(8);
    let net_area = gtk::DrawingArea::new();
    net_area.set_size_request(480, 200);
    net_page.pack_start(&net_area, true, true, 0);
    let net_list = gtk::ListStore::new(&[glib::Type::STRING; 4]);
    let net_view = gtk::TreeView::with_model(&net_list);
    column(&net_view, "Adapter Name", 0, false, 200);
    column(&net_view, "Network Utilization", 1, true, 120);
    column(&net_view, "Link Speed", 2, true, 80);
    column(&net_view, "State", 3, false, 0);
    let nsw = scrolled(&net_view);
    nsw.set_size_request(-1, 90);
    net_page.pack_start(&nsw, false, false, 0);
    nb.append_page(&net_page, Some(&gtk::Label::new(Some("Networking"))));

    // ---------- Users
    let users = gtk::ListStore::new(&[glib::Type::STRING; 5]);
    let users_view = gtk::TreeView::with_model(&users);
    column(&users_view, "User", 0, false, 120);
    column(&users_view, "ID", 1, false, 40);
    column(&users_view, "Status", 2, false, 90);
    column(&users_view, "Client Name", 3, false, 100);
    column(&users_view, "Session", 4, false, 0);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(8);
    page.pack_start(&scrolled(&users_view), true, true, 0);
    let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let logoff = gtk::Button::with_mnemonic("_Logoff");
    let disconnect = gtk::Button::with_mnemonic("_Disconnect");
    let sendmsg = gtk::Button::with_mnemonic("Send _Message...");
    disconnect.set_sensitive(false);
    sendmsg.set_sensitive(false);
    bb.pack_end(&sendmsg, false, false, 0);
    bb.pack_end(&logoff, false, false, 0);
    bb.pack_end(&disconnect, false, false, 0);
    page.pack_start(&bb, false, false, 0);
    nb.append_page(&page, Some(&gtk::Label::new(Some("Users"))));

    let my_uid = unsafe { libc::getuid() };
    let (req, snaps) = sampler();
    let tm = Rc::new(Tm {
        win: win.clone(), nb: nb.clone(), wm: Wm::connect().ok().map(Rc::new), apps, apps_view: apps_view.clone(), procs, proc_view: proc_view.clone(),
        services, svc_view: svc_view.clone(), users, net_list,
        status, perf: RefCell::new(Perf::default()), perf_area: perf_area.clone(), perf_labels: RefCell::new(labels),
        net_area: net_area.clone(), net_hist: RefCell::new(vec![]), rows: RefCell::new(vec![]), opts: RefCell::new(opts), my_uid,
        req, in_flight: Cell::new(false), last_tick: Cell::new(std::time::Instant::now()), tray: RefCell::new(None),
    });
    {
        let t = Rc::downgrade(&tm);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(s) = snaps.recv().await { let Some(t) = t.upgrade() else { break }; t.apply(s); }
        });
    }

    // drawing
    {
        let t = tm.clone();
        perf_area.connect_draw(move |a, cr| {
            let p = t.perf.borrow();
            let (one, kernel) = { let o = t.opts.borrow(); (o.one_graph, o.kernel) };
            let w = a.allocated_width() as f64;
            let h = a.allocated_height() as f64;
            let half = (h / 2.0).floor();
            let gh = half - 30.0;
            // CPU Usage | CPU Usage History
            group_box(cr, 0.0, 0.0, 100.0, half - 6.0, "CPU Usage");
            group_box(cr, 106.0, 0.0, w - 106.0, half - 6.0, "CPU Usage History");
            draw_meter(cr, 8.0, 18.0, 84.0, gh, p.cpu.0, kernel.then_some(p.cpu.1), &format!("{:.0} %", p.cpu.0));
            let gx = 114.0;
            let gw_all = w - 106.0 - 16.0;
            if one || p.cores.is_empty() {
                draw_graph(cr, gx, 18.0, gw_all, gh, &p.total, (0.0, 1.0, 0.0), p.ticks, kernel, 1.0);
            } else {
                let n = p.cores.len();
                let rows = n.div_ceil(8);
                let cols = n.div_ceil(rows);
                let cw = (gw_all - (cols as f64 - 1.0) * 4.0) / cols as f64;
                let ch = (gh - (rows as f64 - 1.0) * 4.0) / rows as f64;
                for (i, c) in p.cores.iter().enumerate() {
                    let (cx, cy) = ((i % cols) as f64, (i / cols) as f64);
                    draw_graph(cr, gx + cx * (cw + 4.0), 18.0 + cy * (ch + 4.0), cw, ch, c, (0.0, 1.0, 0.0), p.ticks, kernel, 1.0);
                }
            }
            // Memory | Physical Memory Usage History
            group_box(cr, 0.0, half, 100.0, half - 6.0, "Memory");
            group_box(cr, 106.0, half, w - 106.0, half - 6.0, "Physical Memory Usage History");
            draw_meter(cr, 8.0, half + 18.0, 84.0, gh, p.mem_pct, None, &if p.mem_used_gb >= 1.0 { format!("{:.2} GB", p.mem_used_gb) } else { format!("{:.0} MB", p.mem_used_gb * 1024.0) });
            draw_graph(cr, gx, half + 18.0, gw_all, gh, &p.mem, (0.0, 1.0, 0.0), p.ticks, false, 1.0);
            glib::Propagation::Proceed
        });
    }
    {
        let t = tm.clone();
        net_area.connect_draw(move |a, cr| {
            let h = t.net_hist.borrow();
            let w = a.allocated_width() as f64;
            let ah = a.allocated_height() as f64;
            if h.is_empty() { return glib::Propagation::Proceed; }
            let each = (ah / h.len() as f64).max(120.0);
            for (i, (name, q)) in h.iter().enumerate() {
                let y = i as f64 * each;
                group_box(cr, 0.0, y, w, each - 6.0, name);
                // Win7 rescales the axis to the traffic: 100 %, 50 %, 25 %, 10 %, 5 %, 1 %, 0.5 %
                let max = q.iter().copied().fold(0.0, f64::max);
                let scale = [0.5, 1.0, 5.0, 10.0, 25.0, 50.0, 100.0].into_iter().find(|s| max <= *s).unwrap_or(100.0);
                cr.select_font_face("Segoe UI", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
                cr.set_font_size(10.0);
                cr.set_source_rgb(0.0, 0.0, 0.0);
                let gh = each - 34.0;
                let lab = |v: f64| if v < 1.0 { format!("{v} %") } else { format!("{v:.0} %") };
                for (frac, v) in [(0.0, scale), (0.5, scale / 2.0), (1.0, 0.0)] {
                    let s = lab(v);
                    let tw = cr.text_extents(&s).map(|e| e.width()).unwrap_or(0.0);
                    cr.move_to(52.0 - tw, y + 22.0 + frac * (gh - 8.0) + 4.0);
                    cr.show_text(&s).ok();
                }
                let data: VecDeque<(f64, f64)> = q.iter().map(|v| (*v, 0.0)).collect();
                draw_graph(cr, 58.0, y + 18.0, w - 66.0, gh, &data, (0.0, 1.0, 0.0), q.len(), false, scale);
            }
            glib::Propagation::Proceed
        });
    }

    // buttons
    {
        let (t, v) = (tm.clone(), apps_view.clone());
        end.connect_clicked(move |_| { if let (Some(wm), Some((m, it))) = (&t.wm, v.selection().selected()) { let id: u32 = m.value(&it, 2).get().unwrap_or(0); wm.close(id); } });
    }
    { let t = tm.clone(); sw_to.connect_clicked(move |_| t.switch_to()); }
    { let t = tm.clone(); apps_view.connect_row_activated(move |_, _, _| t.switch_to()); }
    { let w = win.clone(); newt.connect_clicked(move |_| new_task(w.upcast_ref())); }
    { let t = tm.clone(); endp.connect_clicked(move |_| t.end_process()); }
    {
        let t = tm.clone();
        proc_view.connect_key_press_event(move |_, e| {
            if e.keyval() == gdk::keys::constants::Delete { t.end_process(); return glib::Propagation::Stop; }
            glib::Propagation::Proceed
        });
    }
    {
        let t = tm.clone();
        show_all.connect_toggled(move |c| {
            { let mut o = t.opts.borrow_mut(); o.all_users = c.is_active(); o.save(); }
            // re-filter the last snapshot at once; the next one follows
            t.in_flight.set(false);
            t.request();
        });
    }
    svcb.connect_clicked(|_| { util::spawn(&["aerode-control", "services"]); });
    {
        let w = win.clone();
        logoff.connect_clicked(move |_| {
            let d = gtk::MessageDialog::new(Some(&w), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::None, "Do you want to log off the selected user?");
            d.set_title("Windows Task Manager");
            d.set_keep_above(true);
            d.set_secondary_text(Some("Any unsaved data of the selected user will be lost."));
            d.add_button("Log off user", gtk::ResponseType::Ok);
            d.add_button("Cancel", gtk::ResponseType::Cancel);
            let r = d.run();
            d.close();
            if r == gtk::ResponseType::Ok { aerode::sys::log_off(); }
        });
    }
    {
        let t = tm.clone();
        nb.connect_switch_page(move |_, _, n| {
            match n { 2 => t.refresh_services(), 5 => t.refresh_users(), _ => {} }
            // (pages are switched while the window is destroyed too)
            if !t.win.is_visible() { return; }
            let mut o = t.opts.borrow_mut();
            o.tab = n;
            o.save();
        });
    }
    // Applications context menu
    {
        let t = tm.clone();
        apps_view.connect_button_press_event(move |v, e| {
            if e.button() != 3 { return glib::Propagation::Proceed; }
            if let Some((Some(path), ..)) = v.path_at_pos(e.position().0 as i32, e.position().1 as i32) { v.selection().select_path(&path); } else { return glib::Propagation::Stop; }
            let m = gtk::Menu::new();
            let add = |l: &str, f: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.connect_activate(move |_| f()); m.append(&it); };
            { let t = t.clone(); add("_Switch To", Box::new(move || t.switch_to())); }
            { let t = t.clone(); add("_Bring To Front", Box::new(move || { if let (Some(wm), Some((id, _))) = (&t.wm, t.selected_window()) { wm.activate(id); } })); }
            { let t = t.clone(); add("Mi_nimize", Box::new(move || { if let (Some(wm), Some((id, _))) = (&t.wm, t.selected_window()) { wm.minimize(id); } })); }
            { let t = t.clone(); add("Ma_ximize", Box::new(move || { if let (Some(wm), Some((id, _))) = (&t.wm, t.selected_window()) { wm.set_maximized(id, true); } })); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let t = t.clone(); add("_End Task", Box::new(move || { if let (Some(wm), Some((id, _))) = (&t.wm, t.selected_window()) { wm.close(id); } })); }
            m.append(&gtk::SeparatorMenuItem::new());
            {
                let t = t.clone();
                add("_Go To Process", Box::new(move || {
                    let Some((_, pid)) = t.selected_window() else { return };
                    t.nb.set_current_page(Some(1));
                    t.select_pid(pid);
                }));
            }
            m.show_all();
            m.popup_at_pointer(Some(e));
            glib::Propagation::Stop
        });
    }
    // Processes context menu
    {
        let t = tm.clone();
        proc_view.connect_button_press_event(move |v, e| {
            if e.button() != 3 { return glib::Propagation::Proceed; }
            if let Some((Some(path), ..)) = v.path_at_pos(e.position().0 as i32, e.position().1 as i32) { v.selection().select_path(&path); } else { return glib::Propagation::Stop; }
            let Some((pid, _)) = t.selected_proc() else { return glib::Propagation::Stop };
            let m = gtk::Menu::new();
            let add = |m: &gtk::Menu, l: &str, f: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.connect_activate(move |_| f()); m.append(&it); it };
            add(&m, "_Open File Location", Box::new(move || {
                if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/exe")) { util::explore(&p.to_string_lossy()); }
            })).set_sensitive(pid != 0);
            m.append(&gtk::SeparatorMenuItem::new());
            { let t = t.clone(); add(&m, "_End Process", Box::new(move || t.end_process())).set_sensitive(pid != 0); }
            { let t = t.clone(); add(&m, "End Process _Tree", Box::new(move || t.end_tree())).set_sensitive(pid != 0); }
            m.append(&gtk::SeparatorMenuItem::new());
            let pm = gtk::Menu::new();
            let nice = t.rows.borrow().iter().find(|r| r.pid == pid).map(|r| r.nice).unwrap_or(0);
            let mut grp: Option<gtk::RadioMenuItem> = None;
            for (l, n) in [("_Realtime", -20), ("_High", -10), ("_AboveNormal", -5), ("_Normal", 0), ("_BelowNormal", 5), ("_Low", 19)] {
                let it = match &grp { None => gtk::RadioMenuItem::with_mnemonic(l), Some(g) => gtk::RadioMenuItem::with_mnemonic_from_widget(g, Some(l)) };
                if grp.is_none() { grp = Some(it.clone()); }
                let closest = [-20, -10, -5, 0, 5, 19].into_iter().min_by_key(|v| (v - nice).abs()).unwrap_or(0);
                it.set_active(n == closest);
                let t = t.clone();
                it.connect_activate(move |i| if i.is_active() && n != closest { t.set_priority(n); });
                pm.append(&it);
            }
            let sp = gtk::MenuItem::with_mnemonic("Set _Priority");
            sp.set_submenu(Some(&pm));
            sp.set_sensitive(pid != 0);
            m.append(&sp);
            m.append(&gtk::SeparatorMenuItem::new());
            {
                let t = t.clone();
                add(&m, "_Go to Service(s)", Box::new(move || {
                    t.nb.set_current_page(Some(2));
                    let (t2, pid) = (t.clone(), pid.to_string());
                    // the services list loads in the background: select once it is there
                    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                        let Some(it) = t2.services.iter_first() else { return glib::ControlFlow::Continue };
                        t2.svc_view.selection().unselect_all();
                        loop {
                            let p: String = t2.services.value(&it, 1).get().unwrap_or_default();
                            if p == pid { t2.svc_view.selection().select_iter(&it); }
                            if !t2.services.iter_next(&it) { break; }
                        }
                        glib::ControlFlow::Break
                    });
                })).set_sensitive(pid != 0);
            }
            m.show_all();
            m.popup_at_pointer(Some(e));
            glib::Propagation::Stop
        });
    }
    // Services context menu
    {
        let t = tm.clone();
        svc_view.connect_button_press_event(move |v, e| {
            if e.button() != 3 { return glib::Propagation::Proceed; }
            if let Some((Some(path), ..)) = v.path_at_pos(e.position().0 as i32, e.position().1 as i32) { v.selection().select_path(&path); } else { return glib::Propagation::Stop; }
            let Some((m0, it)) = v.selection().selected() else { return glib::Propagation::Stop };
            let name: String = m0.value(&it, 0).get().unwrap_or_default();
            let running = m0.value(&it, 3).get::<String>().unwrap_or_default() == "Running";
            let pid: u32 = m0.value(&it, 1).get::<String>().unwrap_or_default().parse().unwrap_or(0);
            let m = gtk::Menu::new();
            for (l, verb, on) in [("_Start Service", "start", !running), ("S_top Service", "stop", running)] {
                let it = gtk::MenuItem::with_mnemonic(l);
                it.set_sensitive(on);
                let (t, name) = (t.clone(), name.clone());
                it.connect_activate(move |_| {
                    let (t2, name, verb) = (t.clone(), name.clone(), verb.to_string());
                    let (unit, v2) = (format!("{name}.service"), verb.clone());
                    in_background(move || std::process::Command::new("systemctl").args([v2.as_str(), unit.as_str()]).stdin(std::process::Stdio::null()).output().map(|o| (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())).unwrap_or((false, String::new())),
                        move |(ok, err)| {
                            if !ok { util::error(&format!("Windows could not {} the {} service.", if verb == "start" { "start" } else { "stop" }, name), err.trim()); }
                            t2.refresh_services();
                        });
                });
                m.append(&it);
            }
            m.append(&gtk::SeparatorMenuItem::new());
            let go = gtk::MenuItem::with_mnemonic("_Go to Process");
            go.set_sensitive(pid != 0);
            { let t = t.clone(); go.connect_activate(move |_| { t.nb.set_current_page(Some(1)); t.select_pid(pid); }); }
            m.append(&go);
            m.show_all();
            m.popup_at_pointer(Some(e));
            glib::Propagation::Stop
        });
    }

    // menus
    let item = |m: &gtk::Menu, l: &str, acc: &str, f: Box<dyn Fn()>| {
        let it = gtk::MenuItem::with_mnemonic(l);
        if !acc.is_empty() { let (k, md) = gtk::accelerator_parse(acc); it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE); }
        it.connect_activate(move |_| f());
        m.append(&it);
        it
    };
    let m = gtk::Menu::new();
    { let w = win.clone(); item(&m, "_New Task (Run...)", "", Box::new(move || new_task(w.upcast_ref()))); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); item(&m, "E_xit Task Manager", "", Box::new(move || w.close())); }
    let top = gtk::MenuItem::with_mnemonic("_File"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    let check = |m: &gtk::Menu, l: &str, on: bool, f: Box<dyn Fn(bool)>| { let it = gtk::CheckMenuItem::with_mnemonic(l); it.set_active(on); it.connect_toggled(move |i| f(i.is_active())); m.append(&it); it };
    let (on_top, min_use, hide_min) = { let o = tm.opts.borrow(); (o.on_top, o.min_on_use, o.hide_min) };
    win.set_keep_above(on_top);
    { let (w, t) = (win.clone(), tm.clone()); check(&m, "_Always On Top", on_top, Box::new(move |on| { w.set_keep_above(on); let mut o = t.opts.borrow_mut(); o.on_top = on; o.save(); })); }
    { let t = tm.clone(); check(&m, "_Minimize On Use", min_use, Box::new(move |on| { let mut o = t.opts.borrow_mut(); o.min_on_use = on; o.save(); })); }
    { let t = tm.clone(); check(&m, "_Hide When Minimized", hide_min, Box::new(move |on| { let mut o = t.opts.borrow_mut(); o.hide_min = on; o.save(); })); }
    let top = gtk::MenuItem::with_mnemonic("_Options"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    { let t = tm.clone(); item(&m, "_Refresh Now", "F5", Box::new(move || { t.in_flight.set(false); t.request(); t.refresh_apps(); match t.nb.current_page() { Some(2) => t.refresh_services(), Some(5) => t.refresh_users(), _ => {} } })); }
    let speed = gtk::Menu::new();
    let cur_speed = tm.opts.borrow().speed_ms;
    let mut grp: Option<gtk::RadioMenuItem> = None;
    for (l, ms) in [("_High", 500u64), ("_Normal", 1000), ("_Low", 4000), ("_Paused", 0)] {
        let it = match &grp { None => gtk::RadioMenuItem::with_mnemonic(l), Some(g) => gtk::RadioMenuItem::with_mnemonic_from_widget(g, Some(l)) };
        if grp.is_none() { grp = Some(it.clone()); }
        it.set_active(ms == cur_speed);
        let t = tm.clone();
        it.connect_toggled(move |i| if i.is_active() { let mut o = t.opts.borrow_mut(); o.speed_ms = ms; o.save(); });
        speed.append(&it);
    }
    let sp = gtk::MenuItem::with_mnemonic("_Update Speed"); sp.set_submenu(Some(&speed)); m.append(&sp);
    m.append(&gtk::SeparatorMenuItem::new());
    let hist = gtk::Menu::new();
    let one_graph = tm.opts.borrow().one_graph;
    let og = gtk::RadioMenuItem::with_mnemonic("_One Graph, All CPUs");
    let pg = gtk::RadioMenuItem::with_mnemonic_from_widget(&og, Some("One Graph _Per CPU"));
    if one_graph { og.set_active(true) } else { pg.set_active(true) }
    { let t = tm.clone(); og.connect_toggled(move |i| { let mut o = t.opts.borrow_mut(); o.one_graph = i.is_active(); o.save(); drop(o); t.perf_area.queue_draw(); }); }
    hist.append(&og);
    hist.append(&pg);
    let ch = gtk::MenuItem::with_mnemonic("_CPU History"); ch.set_submenu(Some(&hist)); m.append(&ch);
    let kt = { let t = tm.clone(); let on = t.opts.borrow().kernel; check(&m, "_Show Kernel Times", on, Box::new(move |on| { let mut o = t.opts.borrow_mut(); o.kernel = on; o.save(); drop(o); t.perf_area.queue_draw(); })) };
    let top = gtk::MenuItem::with_mnemonic("_View"); top.set_submenu(Some(&m)); menubar.append(&top);
    {
        // Win7 offers the graph options on the Performance tab only
        let (t, ch, kt) = (tm.clone(), ch.clone(), kt.clone());
        top.connect_activate(move |_| { let perf = t.nb.current_page() == Some(3); ch.set_sensitive(perf); kt.set_sensitive(perf); });
    }
    let m = gtk::Menu::new();
    item(&m, "Task Manager _Help Topics", "", Box::new(|| { util::open_uri("https://wiki.archlinux.org/title/Core_utilities"); }));
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); item(&m, "_About Task Manager", "", Box::new(move || {
        let d = gtk::AboutDialog::new(); d.set_transient_for(Some(&w)); d.set_keep_above(true); d.set_program_name("Windows Task Manager");
        d.set_comments(Some("Aerode Task Manager – a Windows 7 style task manager for Linux")); d.set_logo_icon_name(Some(util::first_icon(&["utilities-system-monitor"])));
        d.run(); d.close();
    })); }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);
    let _ = gdk::Display::default();

    // the CPU meter in the notification area (double-click restores, like Win7)
    // (GtkStatusIcon = XEmbed tray icon; gtk-rs no longer wraps it, the C API still exists in GTK 3)
    {
        let icon: glib::Object = unsafe { glib::translate::from_glib_full(gtk::ffi::gtk_status_icon_new() as *mut glib::gobject_ffi::GObject) };
        if let Some(p) = tray_pixbuf(0.0) { icon.set_property("pixbuf", &p); }
        icon.set_property("title", "Windows Task Manager");
        let w = win.clone();
        icon.connect_local("activate", false, move |_| { w.show(); w.deiconify(); w.present(); None });
        *tm.tray.borrow_mut() = Some(icon);
    }
    {
        let t = tm.clone();
        win.connect_window_state_event(move |w, e| {
            if e.changed_mask().contains(gdk::WindowState::ICONIFIED) && e.new_window_state().contains(gdk::WindowState::ICONIFIED) && t.opts.borrow().hide_min { w.hide(); }
            glib::Propagation::Proceed
        });
    }
    {
        // the tray icon keeps the process alive otherwise
        let a = app.clone();
        win.connect_destroy(move |_| a.quit());
    }

    root.show_all();
    let tab = tm.opts.borrow().tab.min(5);
    nb.set_current_page(Some(tab));
    tm.request();
    {
        let t = Rc::downgrade(&tm);
        glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            let Some(t) = t.upgrade() else { return glib::ControlFlow::Break };
            let ms = t.opts.borrow().speed_ms;
            if ms > 0 && t.last_tick.get().elapsed().as_millis() as u64 >= ms { t.request(); }
            glib::ControlFlow::Continue
        });
    }
    win.show();
}

fn main() -> glib::ExitCode {
    util::log_panics("taskmgr");
    let app = gtk::Application::new(Some("org.aerode.TaskManager"), gio::ApplicationFlags::empty());
    app.connect_activate(|app| {
        // one Task Manager: starting it again brings the window back
        if let Some(w) = app.windows().first() { w.show(); w.deiconify(); w.present(); return; }
        build(app);
    });
    app.run()
}
