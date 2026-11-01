//! Windows Task Manager – Windows 7 Task Manager remake (reads /proc, systemd, EWMH).
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use aerode::{config, theme, util, x11::Wm};

const HIST: usize = 120;

#[derive(Default, Clone)]
struct Proc { pid: u32, name: String, user: String, uid: u32, cpu_ticks: u64, mem_kb: u64, desc: String, threads: u64 }

#[derive(Default)]
struct Sample {
    cpu_total: Vec<u64>, // per line of /proc/stat: total jiffies
    cpu_idle: Vec<u64>,
    procs: HashMap<u32, u64>,
    net: HashMap<String, (u64, u64)>,
}

struct Tm {
    win: gtk::ApplicationWindow,
    wm: Option<Rc<Wm>>,
    apps: gtk::ListStore,
    procs: gtk::ListStore,
    proc_view: gtk::TreeView,
    services: gtk::ListStore,
    users: gtk::ListStore,
    status: [gtk::Label; 3],
    perf: RefCell<Perf>,
    perf_area: gtk::DrawingArea,
    perf_labels: RefCell<HashMap<&'static str, gtk::Label>>,
    net_area: gtk::DrawingArea,
    net_hist: RefCell<HashMap<String, VecDeque<f64>>>,
    prev: RefCell<Sample>,
    all_users: Cell<bool>,
    paused: Cell<bool>,
    my_uid: u32,
}

#[derive(Default)]
struct Perf {
    cpu: f64,
    cores: Vec<VecDeque<f64>>,
    mem: VecDeque<f64>,
    mem_pct: f64,
    mem_used_gb: f64,
}

fn read(p: &str) -> String { std::fs::read_to_string(p).unwrap_or_default() }

fn users_by_uid() -> HashMap<u32, String> {
    read("/etc/passwd").lines().filter_map(|l| { let f: Vec<&str> = l.split(':').collect(); if f.len() > 2 { Some((f[2].parse().ok()?, f[0].to_string())) } else { None } }).collect()
}

fn list_procs(users: &HashMap<u32, String>) -> Vec<Proc> {
    let mut v = vec![];
    let Ok(rd) = std::fs::read_dir("/proc") else { return v };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let stat = read(&format!("/proc/{pid}/stat"));
        let Some(rp) = stat.rfind(')') else { continue };
        let name = stat[stat.find('(').unwrap_or(0) + 1..rp].to_string();
        let f: Vec<&str> = stat[rp + 2..].split_whitespace().collect();
        if f.len() < 20 { continue; }
        let ticks = f[11].parse::<u64>().unwrap_or(0) + f[12].parse::<u64>().unwrap_or(0);
        let threads = f[17].parse().unwrap_or(1);
        let status = read(&format!("/proc/{pid}/status"));
        let mut uid = 0;
        let mut mem = 0;
        for l in status.lines() {
            if let Some(r) = l.strip_prefix("Uid:") { uid = r.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0); }
            // Win7 shows "Memory (Private Working Set)": RssAnon is the closest Linux equivalent
            if let Some(r) = l.strip_prefix("RssAnon:") { mem = r.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0); }
        }
        let cmd = read(&format!("/proc/{pid}/cmdline")).replace('\0', " ").trim().to_string();
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let name = exe.filter(|e| !e.is_empty()).unwrap_or(name);
        v.push(Proc { pid, desc: if cmd.is_empty() { format!("[{name}]") } else { cmd }, name, user: users.get(&uid).cloned().unwrap_or_else(|| uid.to_string()), uid, cpu_ticks: ticks, mem_kb: mem, threads });
    }
    v
}

fn cpu_times() -> (Vec<u64>, Vec<u64>) {
    let (mut t, mut i) = (vec![], vec![]);
    for l in read("/proc/stat").lines().filter(|l| l.starts_with("cpu")) {
        let n: Vec<u64> = l.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
        t.push(n.iter().sum());
        i.push(n.get(3).copied().unwrap_or(0) + n.get(4).copied().unwrap_or(0));
    }
    (t, i)
}

fn meminfo() -> HashMap<String, u64> {
    read("/proc/meminfo").lines().filter_map(|l| { let (k, v) = l.split_once(':')?; Some((k.to_string(), v.split_whitespace().next()?.parse().ok()?)) }).collect()
}

fn fmt_k(kb: u64) -> String {
    let s = kb.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); }
    format!("{o} K")
}

fn column(view: &gtk::TreeView, title: &str, col: i32, right: bool, width: i32) {
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
}

fn scrolled(w: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(w);
    sw
}

/// Green-on-black history graph with the classic grid.
fn draw_graph(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, data: &VecDeque<f64>, color: (f64, f64, f64), scroll: usize) {
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
    cr.set_source_rgb(color.0, color.1, color.2);
    let n = data.len();
    for (i, v) in data.iter().enumerate() {
        let px = x + w - (n - 1 - i) as f64 * 3.0;
        let py = y + h - (v / 100.0).clamp(0.0, 1.0) * (h - 1.0);
        if i == 0 { cr.move_to(px, py) } else { cr.line_to(px, py) }
    }
    cr.stroke().ok();
    cr.restore().ok();
}

/// The LED-style usage meter.
fn draw_meter(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, pct: f64, label: &str) {
    cr.rectangle(x, y, w, h);
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.fill().ok();
    let bar_h = h - 22.0;
    let segs = (bar_h / 3.0) as i32;
    let lit = ((pct / 100.0) * segs as f64).round() as i32;
    for i in 0..segs {
        let sy = y + 4.0 + bar_h - (i + 1) as f64 * 3.0;
        for half in 0..2 {
            let sx = x + w / 2.0 - 16.0 + half as f64 * 17.0;
            if i < lit { cr.set_source_rgb(0.0, 1.0, 0.0) } else { cr.set_source_rgb(0.0, 0.32, 0.0) }
            cr.rectangle(sx, sy, 15.0, 2.0);
            cr.fill().ok();
        }
    }
    cr.set_source_rgb(0.0, 1.0, 0.0);
    cr.select_font_face("Segoe UI", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
    cr.set_font_size(11.0);
    let e = cr.text_extents(label).unwrap();
    cr.move_to(x + (w - e.width()) / 2.0, y + h - 6.0);
    cr.show_text(label).ok();
}

impl Tm {
    fn tick(self: &Rc<Self>) {
        if self.paused.get() { return; }
        let users = users_by_uid();
        let (tot, idle) = cpu_times();
        let procs = list_procs(&users);
        let mut prev = self.prev.borrow_mut();
        let dtot = tot.first().copied().unwrap_or(0).saturating_sub(prev.cpu_total.first().copied().unwrap_or(0)).max(1);
        // --- per-core + total CPU
        let mut perf = self.perf.borrow_mut();
        let ncores = tot.len().saturating_sub(1);
        if perf.cores.len() != ncores { perf.cores = vec![VecDeque::new(); ncores]; }
        for i in 0..tot.len() {
            let dt = tot[i].saturating_sub(*prev.cpu_total.get(i).unwrap_or(&0)).max(1) as f64;
            let di = idle[i].saturating_sub(*prev.cpu_idle.get(i).unwrap_or(&0)) as f64;
            let u = ((1.0 - di / dt) * 100.0).clamp(0.0, 100.0);
            if i == 0 { perf.cpu = u; } else {
                let q = &mut perf.cores[i - 1];
                q.push_back(u);
                if q.len() > HIST { q.pop_front(); }
            }
        }
        let mi = meminfo();
        let total = *mi.get("MemTotal").unwrap_or(&1) as f64;
        let avail = *mi.get("MemAvailable").unwrap_or(&0) as f64;
        perf.mem_pct = (1.0 - avail / total) * 100.0;
        perf.mem_used_gb = (total - avail) / 1048576.0;
        let mp = perf.mem_pct;
        perf.mem.push_back(mp);
        if perf.mem.len() > HIST { perf.mem.pop_front(); }
        let cpu_now = perf.cpu;
        drop(perf);
        // --- processes (update in place to keep selection/scroll)
        let ncpu = ncores.max(1) as u64;
        let mut rows: HashMap<u32, gtk::TreeIter> = HashMap::new();
        if let Some(it) = self.procs.iter_first() {
            loop {
                let pid: u32 = self.procs.value(&it, 7).get().unwrap_or(0);
                rows.insert(pid, it.clone());
                if !self.procs.iter_next(&it) { break; }
            }
        }
        let mut seen = std::collections::HashSet::new();
        let mut shown = 0;
        for p in &procs {
            if !self.all_users.get() && p.uid != self.my_uid { continue; }
            shown += 1;
            seen.insert(p.pid);
            let prev_t = prev.procs.get(&p.pid).copied().unwrap_or(p.cpu_ticks);
            let cpu = (((p.cpu_ticks - prev_t.min(p.cpu_ticks)) * 100 * ncpu) as f64 / dtot as f64 / ncpu as f64).round() as u32;
            let cpu_s = format!("{:02}", cpu.min(99));
            let vals: [(u32, &dyn ToValue); 8] = [(0, &p.name), (1, &p.user), (2, &cpu_s), (3, &fmt_k(p.mem_kb)), (4, &p.desc), (5, &p.mem_kb), (6, &cpu), (7, &p.pid)];
            let it = rows.get(&p.pid).cloned().unwrap_or_else(|| self.procs.append());
            self.procs.set(&it, &vals);
        }
        for (pid, it) in rows { if !seen.contains(&pid) { self.procs.remove(&it); } }
        prev.procs = procs.iter().map(|p| (p.pid, p.cpu_ticks)).collect();
        prev.cpu_total = tot;
        prev.cpu_idle = idle;
        // --- networking
        let mut nethist = self.net_hist.borrow_mut();
        for l in read("/proc/net/dev").lines().skip(2) {
            let Some((name, rest)) = l.split_once(':') else { continue };
            let name = name.trim().to_string();
            if name == "lo" { continue; }
            let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if f.len() < 9 { continue; }
            let bytes = (f[0], f[8]);
            let old = prev.net.insert(name.clone(), bytes).unwrap_or(bytes);
            let speed_mbit: f64 = read(&format!("/sys/class/net/{name}/speed")).trim().parse::<f64>().unwrap_or(1000.0).max(10.0);
            let bps = ((bytes.0 - old.0.min(bytes.0)) + (bytes.1 - old.1.min(bytes.1))) as f64 * 8.0;
            let q = nethist.entry(name).or_default();
            q.push_back((bps / (speed_mbit * 1e6) * 100.0).min(100.0));
            if q.len() > HIST { q.pop_front(); }
        }
        drop(nethist);
        drop(prev);
        // --- status bar
        self.status[0].set_text(&format!("Processes: {shown}"));
        self.status[1].set_text(&format!("CPU Usage: {:.0}%", cpu_now));
        self.status[2].set_text(&format!("Physical Memory: {:.0}%", mp));
        // --- performance text block
        let l = self.perf_labels.borrow();
        let kb = |k: &str| mi.get(k).copied().unwrap_or(0) / 1024;
        let set = |k: &str, v: String| if let Some(x) = l.get(k) { x.set_text(&v) };
        set("total", kb("MemTotal").to_string());
        set("cached", (kb("Cached") + kb("Buffers")).to_string());
        set("available", kb("MemAvailable").to_string());
        set("free", kb("MemFree").to_string());
        set("paged", kb("Slab").to_string());
        set("nonpaged", kb("KernelStack").to_string());
        let threads: u64 = procs.iter().map(|p| p.threads).sum();
        let handles = read("/proc/sys/fs/file-nr").split_whitespace().next().unwrap_or("0").to_string();
        set("handles", handles);
        set("threads", threads.to_string());
        set("processes", procs.len().to_string());
        let up = read("/proc/uptime").split_whitespace().next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) as u64;
        set("uptime", format!("{}:{:02}:{:02}:{:02}", up / 86400, up / 3600 % 24, up / 60 % 60, up % 60));
        set("commit", format!("{} / {}", mi.get("Committed_AS").copied().unwrap_or(0) / 1048576, mi.get("CommitLimit").copied().unwrap_or(0) / 1048576));
        drop(l);
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
        let mut seen = std::collections::HashSet::new();
        for c in clients {
            if c.class.eq_ignore_ascii_case("aerode-taskmgr") && c.title == "Windows Task Manager" { /* still list ourselves like Win7 */ }
            seen.insert(c.id);
            let it = have.get(&c.id).cloned().unwrap_or_else(|| self.apps.append());
            // Win7 shows "Not Responding" for hung apps; we can't know cheaply → Running
            self.apps.set(&it, &[(0, &c.title), (1, &"Running"), (2, &c.id), (3, &c.pid)]);
        }
        for (id, it) in have { if !seen.contains(&id) { self.apps.remove(&it); } }
    }

    fn refresh_services(&self) {
        self.services.clear();
        let out = util::output(&["systemctl", "list-units", "--type=service", "--all", "--no-legend", "--plain", "--no-pager"]).unwrap_or_default();
        let pids: HashMap<String, String> = util::output(&["systemctl", "show", "--type=service", "-p", "Id,MainPID", "--no-pager", "*"])
            .unwrap_or_default().split("\n\n").filter_map(|b| {
                let mut id = None; let mut pid = None;
                for l in b.lines() { if let Some(v) = l.strip_prefix("Id=") { id = Some(v.to_string()) } if let Some(v) = l.strip_prefix("MainPID=") { pid = Some(v.to_string()) } }
                Some((id?, pid.filter(|p| p != "0").unwrap_or_default()))
            }).collect();
        for l in out.lines() {
            let f: Vec<&str> = l.splitn(5, char::is_whitespace).filter(|s| !s.is_empty()).collect();
            if f.len() < 4 { continue; }
            let name = f[0].trim_end_matches(".service");
            let status = if f[3] == "running" { "Running" } else { "Stopped" };
            let desc = l.splitn(5, ' ').nth(4).unwrap_or("").trim();
            let it = self.services.append();
            self.services.set(&it, &[(0, &name), (1, pids.get(f[0]).unwrap_or(&String::new())), (2, &desc), (3, &status), (4, &"")]);
        }
    }

    fn refresh_users(&self) {
        self.users.clear();
        let out = util::output(&["loginctl", "list-sessions", "--no-legend"]).unwrap_or_default();
        for l in out.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 3 { continue; }
            let state = util::output(&["loginctl", "show-session", f[0], "-p", "State", "--value"]).unwrap_or_default();
            let it = self.users.append();
            let st = if state.trim() == "active" { "Active" } else { "Disconnected" };
            self.users.set(&it, &[(0, &f[2]), (1, &f[0]), (2, &st), (3, &f.get(4).copied().unwrap_or("")), (4, &format!("Console"))]);
        }
    }

    fn end_process(self: &Rc<Self>) {
        let Some((m, it)) = self.proc_view.selection().selected() else { return };
        let pid: u32 = m.value(&it, 7).get().unwrap_or(0);
        let name: String = m.value(&it, 0).get().unwrap_or_default();
        let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::None,
            &format!("Do you want to end \"{name}\"?"));
        d.set_title("Windows Task Manager");
        d.set_secondary_text(Some("If an open program is associated with this process, it will close and you will lose any unsaved data. If you end a system process, it might result in system instability. Are you sure you want to continue?"));
        d.add_button("End process", gtk::ResponseType::Ok);
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        aerode::sounds::play(aerode::sounds::Event::Exclamation);
        let r = d.run();
        d.close();
        if r == gtk::ResponseType::Ok {
            let ok = std::process::Command::new("kill").arg(pid.to_string()).status().map(|s| s.success()).unwrap_or(false)
                || std::process::Command::new("pkexec").args(["kill", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false);
            if !ok { util::error("Unable to Terminate Process", "The operation could not be completed. Access is denied."); }
            self.tick();
        }
    }
}

fn new_task(parent: &gtk::Window) {
    let d = gtk::Dialog::with_buttons(Some("Create New Task"), Some(parent), gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(8);
    let l = gtk::Label::new(Some("Type the name of a program, folder, document, or Internet resource, and Windows will open it for you."));
    l.set_line_wrap(true);
    l.set_max_width_chars(48);
    l.set_xalign(0.0);
    let e = gtk::Entry::new();
    e.set_activates_default(true);
    let admin = gtk::CheckButton::with_label("Create this task with administrative privileges.");
    c.pack_start(&l, false, false, 0);
    c.pack_start(&e, false, false, 0);
    c.pack_start(&admin, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        let t = e.text().to_string();
        if admin.is_active() { util::spawn(&["pkexec", "sh", "-c", &t]); } else { util::run_command(&t); }
    }
    d.close();
}

fn build(app: &gtk::Application) {
    theme::init_app();
    theme::load_css(".tm-perf { background-color: #f0f0f0; } .tm-group { color: #000; } .tm-status label { padding: 0 10px; }");
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
    root.pack_start(&nb, true, true, 0);
    let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    statusbar.style_context().add_class("statusbar");
    statusbar.style_context().add_class("tm-status");
    let status = [gtk::Label::new(None), gtk::Label::new(None), gtk::Label::new(None)];
    for (i, s) in status.iter().enumerate() {
        if i > 0 { statusbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 0); }
        statusbar.pack_start(s, false, false, 0);
    }
    root.pack_start(&statusbar, false, false, 0);
    win.add(&root);

    // ---------- Applications
    let apps = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::U32, glib::Type::U32]);
    let apps_view = gtk::TreeView::with_model(&apps);
    column(&apps_view, "Task", 0, false, 320);
    column(&apps_view, "Status", 1, false, 0);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(8);
    page.pack_start(&scrolled(&apps_view), true, true, 0);
    let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let (end, sw_to, newt) = (gtk::Button::with_label("End Task"), gtk::Button::with_label("Switch To"), gtk::Button::with_label("New Task..."));
    bb.pack_end(&newt, false, false, 0);
    bb.pack_end(&sw_to, false, false, 0);
    bb.pack_end(&end, false, false, 0);
    page.pack_start(&bb, false, false, 0);
    nb.append_page(&page, Some(&gtk::Label::new(Some("Applications"))));

    // ---------- Processes
    // name, user, cpu text, mem text, desc, mem sort, cpu sort, pid
    let procs = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U64, glib::Type::U32, glib::Type::U32]);
    let proc_view = gtk::TreeView::with_model(&procs);
    column(&proc_view, "Image Name", 0, false, 130);
    column(&proc_view, "User Name", 1, false, 80);
    column(&proc_view, "CPU", 2, true, 40);
    column(&proc_view, "Memory (...", 3, true, 90);
    column(&proc_view, "Description", 4, false, 0);
    // sort numerically
    proc_view.column(2).unwrap().set_sort_column_id(6);
    proc_view.column(3).unwrap().set_sort_column_id(5);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(8);
    page.pack_start(&scrolled(&proc_view), true, true, 0);
    let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let show_all = gtk::CheckButton::with_label("Show processes from all users");
    let endp = gtk::Button::with_label("End Process");
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
    column(&svc_view, "Status", 3, false, 0);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    page.set_border_width(8);
    page.pack_start(&scrolled(&svc_view), true, true, 0);
    let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let svcb = gtk::Button::with_label("Services...");
    bb.pack_end(&svcb, false, false, 0);
    page.pack_start(&bb, false, false, 0);
    nb.append_page(&page, Some(&gtk::Label::new(Some("Services"))));

    // ---------- Performance
    let perf_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    perf_page.set_border_width(8);
    let perf_area = gtk::DrawingArea::new();
    perf_area.set_size_request(480, 300);
    perf_page.pack_start(&perf_area, true, true, 0);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(24);
    grid.set_row_spacing(1);
    let mut labels = HashMap::new();
    let blocks: [(&str, &[(&str, &'static str)]); 4] = [
        ("Physical Memory (MB)", &[("Total", "total"), ("Cached", "cached"), ("Available", "available"), ("Free", "free")]),
        ("Kernel Memory (MB)", &[("Paged", "paged"), ("Nonpaged", "nonpaged")]),
        ("System", &[("Handles", "handles"), ("Threads", "threads"), ("Processes", "processes"), ("Up Time", "uptime"), ("Commit (GB)", "commit")]),
        ("", &[]),
    ];
    let mut row = 0;
    for (bi, (title, items)) in blocks.iter().enumerate() {
        if title.is_empty() { continue; }
        let col = if bi == 2 { 2 } else { 0 };
        let r0 = if bi == 2 { 0 } else { row };
        let h = gtk::Label::new(Some(title));
        h.set_xalign(0.0);
        h.style_context().add_class("tm-group");
        grid.attach(&h, col, r0, 2, 1);
        for (i, (l, key)) in items.iter().enumerate() {
            let a = gtk::Label::new(Some(l));
            a.set_xalign(0.0);
            a.set_margin_start(8);
            let v = gtk::Label::new(Some("0"));
            v.set_xalign(1.0);
            v.set_width_chars(12);
            grid.attach(&a, col, r0 + 1 + i as i32, 1, 1);
            grid.attach(&v, col + 1, r0 + 1 + i as i32, 1, 1);
            labels.insert(*key, v);
        }
        if bi != 2 { row = r0 + items.len() as i32 + 2; }
    }
    perf_page.pack_start(&grid, false, false, 0);
    let rm = gtk::Button::with_label("Resource Monitor...");
    rm.set_halign(gtk::Align::End);
    rm.connect_clicked(|_| { if !util::spawn(&["gnome-system-monitor"]) && !util::spawn(&["plasma-systemmonitor"]) { util::spawn(&["xterm", "-e", "htop"]); } });
    perf_page.pack_start(&rm, false, false, 0);
    nb.append_page(&perf_page, Some(&gtk::Label::new(Some("Performance"))));

    // ---------- Networking
    let net_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
    net_page.set_border_width(8);
    let net_area = gtk::DrawingArea::new();
    net_area.set_size_request(480, 360);
    net_page.pack_start(&net_area, true, true, 0);
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
    let logoff = gtk::Button::with_label("Logoff");
    bb.pack_end(&logoff, false, false, 0);
    page.pack_start(&bb, false, false, 0);
    nb.append_page(&page, Some(&gtk::Label::new(Some("Users"))));

    let my_uid = std::fs::metadata("/proc/self").map(|m| { use std::os::unix::fs::MetadataExt; m.uid() }).unwrap_or(0);
    let tm = Rc::new(Tm {
        win: win.clone(), wm: Wm::connect().ok().map(Rc::new), apps, procs, proc_view: proc_view.clone(), services, users,
        status, perf: RefCell::new(Perf::default()), perf_area: perf_area.clone(), perf_labels: RefCell::new(labels),
        net_area: net_area.clone(), net_hist: RefCell::new(HashMap::new()), prev: RefCell::new(Sample::default()),
        all_users: Cell::new(false), paused: Cell::new(false), my_uid,
    });

    // drawing
    {
        let t = tm.clone();
        perf_area.connect_draw(move |a, cr| {
            let p = t.perf.borrow();
            let w = a.allocated_width() as f64;
            let lab = |cr: &gtk::cairo::Context, x: f64, y: f64, s: &str| { cr.set_source_rgb(0.0, 0.0, 0.0); cr.select_font_face("Segoe UI", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal); cr.set_font_size(12.0); cr.move_to(x, y); cr.show_text(s).ok(); };
            lab(cr, 0.0, 12.0, "CPU Usage");
            lab(cr, 110.0, 12.0, "CPU Usage History");
            draw_meter(cr, 0.0, 18.0, 90.0, 120.0, p.cpu, &format!("{:.0} %", p.cpu));
            let n = p.cores.len().max(1);
            let gw = (w - 110.0 - (n.min(8) as f64 - 1.0) * 4.0) / n.min(8) as f64;
            for (i, c) in p.cores.iter().enumerate().take(8) {
                draw_graph(cr, 110.0 + i as f64 * (gw + 4.0), 18.0, gw, 120.0, c, (0.0, 1.0, 0.0), c.len());
            }
            lab(cr, 0.0, 160.0, "Memory");
            lab(cr, 110.0, 160.0, "Physical Memory Usage History");
            draw_meter(cr, 0.0, 166.0, 90.0, 120.0, p.mem_pct, &format!("{:.2} GB", p.mem_used_gb));
            draw_graph(cr, 110.0, 166.0, w - 110.0, 120.0, &p.mem, (1.0, 1.0, 0.0), p.mem.len());
            glib::Propagation::Proceed
        });
    }
    {
        let t = tm.clone();
        net_area.connect_draw(move |a, cr| {
            let h = t.net_hist.borrow();
            let w = a.allocated_width() as f64;
            let mut y = 0.0;
            let mut names: Vec<_> = h.keys().cloned().collect();
            names.sort();
            for n in names {
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.select_font_face("Segoe UI", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
                cr.set_font_size(12.0);
                cr.move_to(0.0, y + 12.0);
                cr.show_text(&n).ok();
                let q = &h[&n];
                draw_graph(cr, 0.0, y + 18.0, w, 120.0, q, (0.0, 1.0, 0.0), q.len());
                cr.set_source_rgb(0.0, 0.0, 0.0);
                cr.move_to(0.0, y + 152.0);
                cr.show_text(&format!("Network Utilization: {:.2} %", q.back().copied().unwrap_or(0.0))).ok();
                y += 170.0;
            }
            glib::Propagation::Proceed
        });
    }

    // buttons
    {
        let (t, v) = (tm.clone(), apps_view.clone());
        end.connect_clicked(move |_| {
            if let (Some(wm), Some((m, it))) = (&t.wm, v.selection().selected()) { let id: u32 = m.value(&it, 2).get().unwrap_or(0); wm.close(id); }
        });
    }
    {
        let (t, v) = (tm.clone(), apps_view.clone());
        sw_to.connect_clicked(move |_| {
            if let (Some(wm), Some((m, it))) = (&t.wm, v.selection().selected()) { let id: u32 = m.value(&it, 2).get().unwrap_or(0); wm.activate(id); }
        });
    }
    { let w = win.clone(); newt.connect_clicked(move |_| new_task(w.upcast_ref())); }
    { let t = tm.clone(); endp.connect_clicked(move |_| t.end_process()); }
    { let t = tm.clone(); show_all.connect_toggled(move |c| { t.all_users.set(c.is_active()); t.procs.clear(); t.tick(); }); }
    svcb.connect_clicked(|_| { util::spawn(&["aerode-control", "services"]); });
    logoff.connect_clicked(|_| aerode::sys::log_off());
    {
        let t = tm.clone();
        nb.connect_switch_page(move |_, _, n| match n { 2 => t.refresh_services(), 5 => t.refresh_users(), _ => {} });
    }
    // Processes context menu
    {
        let t = tm.clone();
        proc_view.connect_button_press_event(move |v, e| {
            if e.button() != 3 { return glib::Propagation::Proceed; }
            if let Some((Some(path), ..)) = v.path_at_pos(e.position().0 as i32, e.position().1 as i32) { v.selection().select_path(&path); }
            let m = gtk::Menu::new();
            let it = gtk::MenuItem::with_label("Open File Location");
            let t2 = t.clone();
            it.connect_activate(move |_| {
                if let Some((m, i)) = t2.proc_view.selection().selected() {
                    let pid: u32 = m.value(&i, 7).get().unwrap_or(0);
                    if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/exe")) { if let Some(d) = p.parent() { util::explore(&d.to_string_lossy()); } }
                }
            });
            m.append(&it);
            m.append(&gtk::SeparatorMenuItem::new());
            let it = gtk::MenuItem::with_label("End Process");
            let t2 = t.clone();
            it.connect_activate(move |_| t2.end_process());
            m.append(&it);
            let it = gtk::MenuItem::with_label("End Process Tree");
            let t2 = t.clone();
            it.connect_activate(move |_| {
                if let Some((m, i)) = t2.proc_view.selection().selected() {
                    let pid: u32 = m.value(&i, 7).get().unwrap_or(0);
                    util::spawn(&["pkill", "-TERM", "-P", &pid.to_string()]);
                    util::spawn(&["kill", &pid.to_string()]);
                }
            });
            m.append(&it);
            m.show_all();
            m.popup_at_pointer(Some(e));
            glib::Propagation::Stop
        });
    }

    // menus
    let item = |m: &gtk::Menu, l: &str, f: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.connect_activate(move |_| f()); m.append(&it); };
    let m = gtk::Menu::new();
    { let w = win.clone(); item(&m, "_New Task (Run...)", Box::new(move || new_task(w.upcast_ref()))); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); item(&m, "E_xit Task Manager", Box::new(move || w.close())); }
    let top = gtk::MenuItem::with_mnemonic("_File"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    let aot = gtk::CheckMenuItem::with_mnemonic("_Always On Top");
    aot.set_active(true);
    win.set_keep_above(true);
    { let w = win.clone(); aot.connect_toggled(move |a| w.set_keep_above(a.is_active())); }
    m.append(&aot);
    let mou = gtk::CheckMenuItem::with_mnemonic("_Minimize On Use");
    mou.set_active(true);
    m.append(&mou);
    let top = gtk::MenuItem::with_mnemonic("_Options"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    { let t = tm.clone(); item(&m, "_Refresh Now", Box::new(move || t.tick())); }
    let speed = gtk::Menu::new();
    let speed_ms = Rc::new(Cell::new(1000u64));
    let mut grp: Option<gtk::RadioMenuItem> = None;
    for (l, ms) in [("_High", 500u64), ("_Normal", 1000), ("_Low", 4000), ("_Paused", 0)] {
        let it = match &grp { None => gtk::RadioMenuItem::with_mnemonic(l), Some(g) => gtk::RadioMenuItem::with_mnemonic_from_widget(g, Some(l)) };
        if grp.is_none() { grp = Some(it.clone()); }
        it.set_active(ms == 1000);
        let (t, s) = (tm.clone(), speed_ms.clone());
        it.connect_toggled(move |i| if i.is_active() { t.paused.set(ms == 0); s.set(ms.max(500)); });
        speed.append(&it);
    }
    let sp = gtk::MenuItem::with_mnemonic("_Update Speed"); sp.set_submenu(Some(&speed)); m.append(&sp);
    let top = gtk::MenuItem::with_mnemonic("_View"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    { let w = win.clone(); item(&m, "_About Task Manager", Box::new(move || {
        let d = gtk::AboutDialog::new(); d.set_transient_for(Some(&w)); d.set_program_name("Windows Task Manager");
        d.set_comments(Some("Aerode Task Manager – a Windows 7 style task manager for Linux")); d.set_logo_icon_name(Some(util::first_icon(&["utilities-system-monitor"])));
        d.run(); d.close();
    })); }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);
    let _ = (&accel, gdk::Display::default(), config::conf_dir());

    // start on the Processes tab like many users expect? Win7 opens on Applications.
    root.show_all();
    tm.tick();
    {
        let (t, s) = (tm.clone(), speed_ms.clone());
        let last = Rc::new(Cell::new(std::time::Instant::now()));
        glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            if last.get().elapsed().as_millis() as u64 >= s.get() { last.set(std::time::Instant::now()); t.tick(); }
            glib::ControlFlow::Continue
        });
    }
    win.show();
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::new(Some("org.aerode.TaskManager"), gio::ApplicationFlags::empty());
    app.connect_activate(|app| {
        if let Some(w) = app.windows().first() { w.present(); return; }
        build(app);
    });
    app.run()
}
