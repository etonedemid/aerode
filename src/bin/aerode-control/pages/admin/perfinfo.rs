//! Performance Information and Tools: the Windows Experience Index table (Processor, Memory,
//! Graphics, Gaming graphics, Primary hard disk, 1.0–7.9, base score = lowest) from short, safe
//! benchmarks on a worker thread: a floating-point loop on every core, a memory copy, the OpenGL
//! renderer (glxinfo) and a sequential + random read of a temporary file in ~/.cache.
use gtk::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::super::super::{kv_grid, link, section, text, title, Cp};
use super::*;
use aerode::config::{self, Wei};

fn clamp(s: f64) -> f64 { (s.clamp(1.0, 7.9) * 10.0).floor() / 10.0 }

/// Floating-point work on every logical CPU for ~1.5 s → million operations per second.
pub fn bench_cpu(cancel: &AtomicBool) -> f64 {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let dur = Duration::from_millis(1500);
    let counts: Vec<std::thread::JoinHandle<u64>> = (0..n).map(|i| std::thread::spawn(move || {
        let start = Instant::now();
        let mut x = 0.5 + i as f64;
        let mut ops = 0u64;
        while start.elapsed() < dur {
            for _ in 0..2048 { x = (x * 1.000_000_1 + 0.25).sin().abs() + (x + 1.0).sqrt() * 1e-3; }
            ops += 2048;
        }
        std::hint::black_box(x);
        ops
    })).collect();
    let total: u64 = counts.into_iter().map(|h| h.join().unwrap_or(0)).sum();
    if cancel.load(Ordering::Relaxed) { return 0.0; }
    total as f64 / dur.as_secs_f64() / 1e6
}

/// ~10 M ops/s per thread is a 2009 mid-range core; 8 fast threads reach 7.9.
pub fn cpu_score(mops: f64) -> f64 { if mops <= 0.0 { 1.0 } else { clamp(2.0 + 1.25 * (mops / 2.0).log2()) } }

/// Copy a 64 MB buffer back and forth for ~1 s → GB/s.
pub fn bench_mem(cancel: &AtomicBool) -> f64 {
    let size = 64 * 1024 * 1024;
    let a = vec![1u8; size];
    let mut b = vec![0u8; size];
    let start = Instant::now();
    let mut bytes = 0u64;
    while start.elapsed() < Duration::from_millis(1000) && !cancel.load(Ordering::Relaxed) {
        b.copy_from_slice(&a);
        bytes += size as u64;
        std::hint::black_box(&b);
    }
    bytes as f64 / start.elapsed().as_secs_f64() / 1e9
}

/// DDR2 ≈ 3 GB/s → 5.9, every doubling +1; capped by the amount of RAM like Win7.
pub fn mem_score(gbps: f64, ram_gb: f64) -> f64 {
    let s = 5.9 + (gbps / 3.0).max(0.01).log2();
    let cap = if ram_gb < 1.0 { 2.9 } else if ram_gb < 2.0 { 4.9 } else if ram_gb < 4.0 { 5.9 } else { 7.9 };
    clamp(s.min(cap))
}

#[derive(Clone, Debug, Default)]
pub struct Gpu { pub renderer: String, pub gl: f64, pub vram_mb: u64, pub software: bool, pub unified: bool }

pub fn gpu() -> Gpu {
    let out = sh(&["glxinfo", "-B"]);
    if out.trim().is_empty() { return gpu_sysfs(); }
    let g = |k: &str| out.lines().find_map(|l| l.trim().strip_prefix(k)).map(|s| s.trim().to_string()).unwrap_or_default();
    let renderer = g("OpenGL renderer string:");
    let ver = g("OpenGL core profile version string:");
    let ver = if ver.is_empty() { g("OpenGL version string:") } else { ver };
    let gl: f64 = ver.split_whitespace().next().and_then(|v| { let p: Vec<&str> = v.split('.').collect(); format!("{}.{}", p.first()?, p.get(1).unwrap_or(&"0")).parse().ok() }).unwrap_or(0.0);
    let vram = { let v = g("Video memory:"); let v = if v.is_empty() { g("Dedicated video memory:") } else { v }; v.trim_end_matches("MB").trim().split_whitespace().next().and_then(|x| x.trim_end_matches("MB").parse().ok()).unwrap_or(0) };
    let low = renderer.to_lowercase();
    Gpu { software: low.contains("llvmpipe") || low.contains("softpipe") || low.contains("swrast") || g("Accelerated:") == "no", unified: g("Unified memory:") == "yes", renderer, gl, vram_mb: vram }
}

/// Without glxinfo (mesa-utils): the kernel driver of the first DRM card tells the GPU family.
fn gpu_sysfs() -> Gpu {
    let rd = |p: std::path::PathBuf| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut cards: Vec<std::path::PathBuf> = std::fs::read_dir("/sys/class/drm").map(|d| d.flatten().map(|e| e.path()).filter(|p| { let n = p.file_name().unwrap().to_string_lossy().to_string(); n.starts_with("card") && !n.contains('-') }).collect()).unwrap_or_default();
    cards.sort();
    // prefer a discrete card when there are two
    let pick = cards.iter().find(|c| matches!(std::fs::read_link(c.join("device/driver")).ok().and_then(|l| l.file_name().map(|s| s.to_string_lossy().into_owned())).as_deref(), Some("amdgpu" | "nvidia" | "nouveau"))).or(cards.first()).cloned();
    let Some(card) = pick else { return Gpu::default() };
    let driver = std::fs::read_link(card.join("device/driver")).ok().and_then(|l| l.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
    let vram = rd(card.join("device/mem_info_vram_total")).parse::<u64>().map(|b| b / 1048576).unwrap_or(0);
    let (gl, unified, name) = match driver.as_str() {
        "i915" | "xe" => (4.6, true, "Intel integrated graphics"),
        "amdgpu" => (4.6, vram > 0 && vram <= 2048, "AMD Radeon graphics"),
        "nvidia" => (4.6, false, "NVIDIA graphics"),
        "nouveau" => (4.3, false, "NVIDIA graphics (nouveau)"),
        "radeon" => (3.3, false, "AMD Radeon graphics (radeon)"),
        "virtio-pci" | "virtio_gpu" | "qxl" | "bochs-drm" | "bochs" | "vmwgfx" | "cirrus" => return Gpu { renderer: format!("virtual graphics ({driver})"), gl: 3.3, software: true, ..Default::default() },
        _ => return Gpu::default(),
    };
    Gpu { renderer: format!("{name} ({driver} driver)"), gl, vram_mb: if vram > 0 { vram } else if driver == "nvidia" { 4096 } else { 0 }, software: false, unified }
}

/// (Graphics = desktop/Aero, Gaming graphics = 3D).
pub fn gpu_scores(g: &Gpu) -> (f64, f64) {
    if g.renderer.is_empty() { return (3.0, 1.0); }
    if g.software { return (clamp(1.9 + 0.3 * g.gl.min(4.5)), 1.0); }
    let vram = if g.vram_mb == 0 { 512.0 } else { g.vram_mb as f64 };
    let desktop = 3.4 + g.gl.min(4.6) + if vram >= 1024.0 { 0.3 } else { 0.0 };
    let mut game = 1.9 + g.gl.min(4.6) + 0.5 * (vram / 512.0).max(0.25).log2();
    if g.unified { game -= 0.8; }
    (clamp(desktop), clamp(game))
}

/// Write a temporary file (fsync), drop it from the page cache, read it back sequentially and at
/// random offsets → (MB/s, 4K reads per second).
pub fn bench_disk(cancel: &AtomicBool) -> Result<(f64, f64), String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::io::AsRawFd;
    let dir = cache_dir();
    let free = fs_space(&dir.to_string_lossy()).map(|f| f.0).unwrap_or(0);
    let size: usize = if free > 5 << 30 { 256 << 20 } else if free > 1 << 30 { 64 << 20 } else { return Err("Not enough free disk space for the disk assessment.".into()) };
    let path = dir.join(format!("winsat-{}.tmp", std::process::id()));
    let res = (|| -> std::io::Result<(f64, f64)> {
        let mut f = std::fs::OpenOptions::new().create(true).write(true).read(true).truncate(true).open(&path)?;
        let chunk: Vec<u8> = (0..1 << 20).map(|i| (i * 7 + 13) as u8).collect();
        for _ in 0..size >> 20 { if cancel.load(Ordering::Relaxed) { return Ok((0.0, 0.0)); } f.write_all(&chunk)?; }
        f.sync_all()?;
        let drop = |f: &std::fs::File| unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED); };
        drop(&f);
        f.seek(SeekFrom::Start(0))?;
        let mut buf = vec![0u8; 1 << 20];
        let t = Instant::now();
        let mut n = 0;
        loop { let r = f.read(&mut buf)?; if r == 0 || cancel.load(Ordering::Relaxed) { break; } n += r; }
        let seq = n as f64 / t.elapsed().as_secs_f64().max(1e-6) / 1048576.0;
        drop(&f);
        let mut rng: u64 = 0x9e3779b97f4a7c15;
        let mut small = [0u8; 4096];
        let t = Instant::now();
        let mut reads = 0;
        while t.elapsed() < Duration::from_millis(1500) && reads < 4000 && !cancel.load(Ordering::Relaxed) {
            rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17;
            let off = (rng % (size as u64 / 4096)) * 4096;
            f.seek(SeekFrom::Start(off))?;
            f.read_exact(&mut small)?;
            reads += 1;
        }
        Ok((seq, reads as f64 / t.elapsed().as_secs_f64().max(1e-6)))
    })();
    let _ = std::fs::remove_file(&path);
    res.map_err(|e| e.to_string())
}

/// 100 MB/s sequential ≈ 5.9; random 4K reads separate disks from SSDs (an HDD stays ≤ 5.9).
pub fn disk_score(mbps: f64, iops: f64) -> f64 {
    let seq = 5.9 + (mbps / 100.0).max(0.01).log2();
    let rnd = 4.9 + 0.75 * (iops / 150.0).max(0.01).log2();
    clamp(seq.min(rnd))
}

fn ram_gb() -> f64 {
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next()?.split_whitespace().nth(1)?.parse::<f64>().ok()).unwrap_or(0.0) / 1048576.0
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    let holder = vbox(0);
    holder.style_context().add_class("adm");
    { let cp2 = cp.clone(); side_action(cp, "Adjust visual effects", move || super::super::hw::visual_effects(&cp2)); }
    { let cp2 = cp.clone(); side_action(cp, "Adjust power settings", move || cp2.open("power")); }
    side_action(cp, "Open disk cleanup", || spawn_tool("cleanmgr"));
    { let (cp2, h) = (cp.clone(), holder.clone()); side_action(cp, "Advanced tools", move || advanced(&cp2, &h)); }
    see_also(cp, &[("Action Center", "actioncenter")]);
    main_view(cp, &holder);
    holder.upcast()
}

fn main_view(cp: &Rc<Cp>, holder: &gtk::Box) {
    for c in holder.children() { holder.remove(&c); }
    let wei = config::get().maintenance.wei;
    let v = vbox(8);
    v.pack_start(&title("Rate and improve your computer's performance"), false, false, 0);
    let h = hbox(4);
    h.pack_start(&label("Not sure where to start?", ""), false, false, 0);
    h.pack_start(&link("Learn how you can improve your computer's performance.", || { util::open_uri("https://wiki.archlinux.org/title/Improving_performance"); }), false, false, 0);
    v.pack_start(&h, false, false, 0);
    // the table
    let t = gtk::Grid::new();
    t.style_context().add_class("wei-table");
    t.set_margin_top(8);
    let head = ["Component", "What is rated", "Subscore", "Base score"];
    for (i, hd) in head.iter().enumerate() {
        let l = label(hd, "");
        l.style_context().add_class("wei-head");
        if i == 2 { l.set_xalign(1.0); }
        l.set_hexpand(i == 1);
        t.attach(&l, i as i32, 0, 1, 1);
    }
    let rows: [(&str, &str, Option<f64>); 5] = [
        ("Processor:", "Calculations per second", wei.as_ref().map(|w| w.cpu)),
        ("Memory (RAM):", "Memory operations per second", wei.as_ref().map(|w| w.mem)),
        ("Graphics:", "Desktop performance for Windows Aero", wei.as_ref().map(|w| w.gfx)),
        ("Gaming graphics:", "3D business and gaming graphics performance", wei.as_ref().map(|w| w.game)),
        ("Primary hard disk:", "Disk data transfer rate", wei.as_ref().map(|w| w.disk)),
    ];
    let base = wei.as_ref().map(|w| [w.cpu, w.mem, w.gfx, w.game, w.disk].into_iter().fold(7.9f64, f64::min));
    for (r, (c, what, sc)) in rows.iter().enumerate() {
        let a = label(c, ""); a.style_context().add_class("wei-row");
        let b = label(what, ""); b.style_context().add_class("wei-row");
        let s = label(&sc.map(|x| format!("{x:.1}")).unwrap_or_default(), "wei-score");
        s.style_context().add_class("wei-row");
        s.set_xalign(1.0);
        // the lowest subscore is the one that determines the base score
        if let (Some(x), Some(bs)) = (sc, base) { if (*x - bs).abs() < 0.001 { s.set_markup(&format!("<b>{x:.1}</b>")); } }
        t.attach(&a, 0, r as i32 + 1, 1, 1);
        t.attach(&b, 1, r as i32 + 1, 1, 1);
        t.attach(&s, 2, r as i32 + 1, 1, 1);
    }
    let bx = vbox(4);
    bx.set_margin_start(16);
    bx.set_margin_end(16);
    bx.set_valign(gtk::Align::Center);
    let box_ = vbox(0);
    box_.style_context().add_class("wei-base");
    let num = gtk::Label::new(Some(&base.map(|b| format!("{b:.1}")).unwrap_or_else(|| "–".into())));
    num.style_context().add_class("wei-base-num");
    box_.pack_start(&num, false, false, 0);
    bx.pack_start(&box_, false, false, 0);
    let det = label(if base.is_some() { "Determined by\nlowest subscore" } else { "Not rated" }, "");
    det.set_justify(gtk::Justification::Center);
    det.set_xalign(0.5);
    bx.pack_start(&det, false, false, 0);
    t.attach(&bx, 3, 1, 1, 5);
    v.pack_start(&t, false, false, 0);
    // links and status under the table
    let below = hbox(20);
    let left = vbox(2);
    left.pack_start(&link("What do these numbers mean?", || inform(None, "Windows Experience Index", "What do these numbers mean?", "The Windows Experience Index measures the capability of your computer's hardware and software configuration on a scale of 1.0 to 7.9. A higher base score generally means that your computer will perform better and faster than a computer with a lower base score, especially when performing more advanced and resource-intensive tasks.\n\nThe base score is determined by the lowest subscore, not an average.")), false, false, 0);
    left.pack_start(&link("Tips for improving your computer's performance.", || { util::open_uri("https://wiki.archlinux.org/title/Improving_performance"); }), false, false, 0);
    left.pack_start(&link("Learn more about the scores and software online", || { util::open_uri("https://en.wikipedia.org/wiki/Windows_System_Assessment_Tool"); }), false, false, 0);
    below.pack_start(&left, true, true, 0);
    let right = vbox(2);
    { let (cp2, h) = (cp.clone(), holder.clone()); right.pack_start(&link("View and print detailed performance and system information", move || details(&cp2, &h)), false, false, 0); }
    below.pack_start(&right, false, false, 0);
    v.pack_start(&below, false, false, 0);
    let st = hbox(10);
    st.set_margin_top(10);
    match &wei {
        Some(w) => {
            st.pack_start(&img(&["aerode-shield-ok", "security-high"], 32), false, false, 0);
            st.pack_start(&label(&format!("Your scores are current.\nLast update: {}", when(w.when)), ""), true, true, 0);
            let h = hbox(4);
            h.pack_start(&img(&["view-refresh"], 16), false, false, 0);
            { let (cp2, ho) = (cp.clone(), holder.clone()); h.pack_start(&link("Re-run the assessment", move || assess(&cp2, &ho)), false, false, 0); }
            st.pack_end(&h, false, false, 0);
        }
        None => {
            st.pack_start(&img(&["aerode-shield-warning", "dialog-warning"], 32), false, false, 0);
            st.pack_start(&label("Your computer has not been rated", ""), true, true, 0);
            let b = gtk::Button::with_mnemonic("_Rate this computer");
            { let (cp2, ho) = (cp.clone(), holder.clone()); b.connect_clicked(move |_| assess(&cp2, &ho)); }
            st.pack_end(&b, false, false, 0);
        }
    }
    v.pack_start(&st, false, false, 0);
    holder.pack_start(&v, true, true, 0);
    holder.show_all();
}

/// The "Windows Experience Index" assessment window (cancelable; never blocks the UI).
fn assess(cp: &Rc<Cp>, holder: &gtk::Box) {
    let d = gtk::Dialog::new();
    d.set_title("Windows Experience Index");
    d.set_transient_for(Some(&cp.win));
    d.set_modal(true);
    d.set_default_size(440, -1);
    d.set_deletable(false);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(10);
    let t = gtk::Label::new(None);
    t.set_markup("<span size='large' foreground='#1e3287'>Rating your computer...</span>");
    t.set_xalign(0.0);
    c.pack_start(&t, false, false, 0);
    c.pack_start(&label("Windows is assessing the performance of your computer. This takes a few seconds; the computer may seem slow while it runs.", ""), false, false, 0);
    let pb = gtk::ProgressBar::new();
    c.pack_start(&pb, false, false, 0);
    let step = label("Running the Processor performance assessment...", "dim");
    c.pack_start(&step, false, false, 0);
    let cancel_b = d.add_button("Cancel", gtk::ResponseType::Cancel).downcast::<gtk::Button>().unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    { let c2 = cancel.clone(); cancel_b.connect_clicked(move |b| { c2.store(true, Ordering::Relaxed); b.set_sensitive(false); }); }
    d.show_all();
    enum M { Step(String, f64), Done(Option<Wei>, String) }
    let (cp2, holder2, d2) = (cp.clone(), holder.clone(), d.clone());
    let cancel2 = cancel.clone();
    stream(move |tx: async_channel::Sender<M>| {
        let c = &cancel2;
        let _ = tx.send_blocking(M::Step("Running the Processor performance assessment...".into(), 0.05));
        let mops = bench_cpu(c);
        if c.load(Ordering::Relaxed) { let _ = tx.send_blocking(M::Done(None, String::new())); return; }
        let _ = tx.send_blocking(M::Step("Running the Memory performance assessment...".into(), 0.3));
        let gbps = bench_mem(c);
        let _ = tx.send_blocking(M::Step("Running the Direct3D / OpenGL assessment...".into(), 0.5));
        let g = gpu();
        let _ = tx.send_blocking(M::Step("Running the Disk performance assessment...".into(), 0.65));
        let disk = bench_disk(c);
        if c.load(Ordering::Relaxed) { let _ = tx.send_blocking(M::Done(None, String::new())); return; }
        let (gfx, game) = gpu_scores(&g);
        let (disk_s, note) = match disk { Ok((mbps, iops)) => (disk_score(mbps, iops), format!("disk {mbps:.0} MB/s, {iops:.0} reads/s")), Err(e) => (1.0, e) };
        let w = Wei { cpu: cpu_score(mops), mem: mem_score(gbps, ram_gb()), gfx, game, disk: disk_s, when: now() };
        let _ = tx.send_blocking(M::Done(Some(w), format!("cpu {mops:.1} Mops/s, memory {gbps:.1} GB/s, {} (OpenGL {:.1}), {note}", g.renderer, g.gl)));
    }, move |m| match m {
        M::Step(s, f) => { step.set_text(&s); pb.set_fraction(f); }
        M::Done(w, info) => {
            d2.close();
            if let Some(w) = w {
                eprintln!("aerode-control: Windows Experience Index: {info}");
                config::update(|c| c.maintenance.wei = Some(w.clone()));
                main_view(&cp2, &holder2);
            }
        }
    });
}

/// "View and print detailed performance and system information".
fn details(cp: &Rc<Cp>, holder: &gtk::Box) {
    let v = vbox(8);
    v.pack_start(&title("More details about my computer"), false, false, 0);
    let wei = config::get().maintenance.wei;
    let base = wei.as_ref().map(|w| [w.cpu, w.mem, w.gfx, w.game, w.disk].into_iter().fold(7.9f64, f64::min));
    v.pack_start(&label(&format!("Windows Experience Index: {}", base.map(|b| format!("{b:.1}")).unwrap_or_else(|| "Not rated".into())), ""), false, false, 0);
    let body = vbox(8);
    body.pack_start(&label("Loading…", "dim"), false, false, 0);
    v.pack_start(&body, false, false, 0);
    let print = gtk::Button::with_mnemonic("_Print this page");
    print.set_halign(gtk::Align::Start);
    v.pack_start(&print, false, false, 0);
    let ok = gtk::Button::with_label("OK");
    ok.set_halign(gtk::Align::End);
    ok.set_size_request(80, -1);
    { let (cp2, h) = (cp.clone(), holder.clone()); ok.connect_clicked(move |_| main_view(&cp2, &h)); }
    v.pack_start(&ok, false, false, 0);
    subpage(cp, holder, "perfinfo", "Performance Information and Tools ▸ Detailed Information", &v);
    bg(|| {
        let dmi = |f: &str| std::fs::read_to_string(format!("/sys/class/dmi/id/{f}")).map(|s| s.trim().to_string()).unwrap_or_default();
        let cpu = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
        let model = cpu.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string())).unwrap_or_default();
        let lm = cpu.lines().find_map(|l| l.strip_prefix("flags")).map(|f| f.split_whitespace().any(|x| x == "lm")).unwrap_or(cfg!(target_pointer_width = "64"));
        let g = gpu();
        let root = fs_space("/");
        let net: Vec<String> = std::fs::read_dir("/sys/class/net").map(|d| d.flatten().filter(|e| e.path().join("device").exists()).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        let total: u64 = sh(&["lsblk", "-b", "-d", "-n", "-o", "SIZE,TYPE"]).lines().filter(|l| l.ends_with("disk")).filter_map(|l| l.split_whitespace().next()?.parse::<u64>().ok()).sum();
        vec![
            ("System", vec![("Manufacturer", dmi("sys_vendor")), ("Model", dmi("product_name")), ("Total amount of system memory", format!("{:.2} GB RAM", ram_gb())), ("System type", format!("{} operating system", if cfg!(target_pointer_width = "64") { "64-bit" } else { "32-bit" })), ("Number of processor cores", std::thread::available_parallelism().map(|n| n.get().to_string()).unwrap_or_default()), ("64-bit capable", if lm { "Yes".into() } else { "No".into() }), ("Processor", model)]),
            ("Storage", vec![("Total size of hard disk(s)", gb(total)), ("Disk partition (/)", root.map(|(f, t)| format!("{} Free ({} Total)", gb(f), gb(t))).unwrap_or_default())]),
            ("Graphics", vec![("Display adapter type", if g.renderer.is_empty() { "Unknown (install mesa-utils for glxinfo)".into() } else { g.renderer.clone() }), ("Total available graphics memory", if g.vram_mb > 0 { format!("{} MB", g.vram_mb) } else { "Unknown".into() }), ("OpenGL version", if g.gl > 0.0 { format!("{:.1}", g.gl) } else { "Unknown".into() })]),
            ("Network", vec![("Network Adapter", net.join(", "))]),
        ]
    }, move |sections: Vec<(&str, Vec<(&str, String)>)>| {
        for c in body.children() { body.remove(&c); }
        for (s, rows) in sections {
            body.pack_start(&section(s), false, false, 0);
            let r: Vec<(&str, String)> = rows.into_iter().map(|(k, v)| (k, if v.is_empty() { "Not Available".into() } else { v })).collect();
            body.pack_start(&kv_grid(&r), false, false, 0);
        }
        body.show_all();
    });
    { let v2 = v.clone(); print.connect_clicked(move |_| {
        let po = gtk::PrintOperation::new();
        po.set_n_pages(1);
        let w = v2.clone();
        po.connect_draw_page(move |_, ctx, _| {
            let Some(cr) = ctx.cairo_context() else { return };
            let sx = ctx.width() / w.allocated_width().max(1) as f64;
            cr.scale(sx.min(1.0), sx.min(1.0));
            w.draw(&cr);
        });
        let _ = po.run(gtk::PrintOperationAction::PrintDialog, win_of(&v2).as_ref());
    }); }
}

/// "Advanced tools".
fn advanced(cp: &Rc<Cp>, holder: &gtk::Box) {
    let v = vbox(8);
    v.pack_start(&title("Advanced Tools"), false, false, 0);
    v.pack_start(&text("Use these tools to get additional performance information."), false, false, 0);
    let tools: Vec<(&[&str], &str, Box<dyn Fn()>)> = vec![
        (&["aerode-eventvwr", "utilities-system-monitor"], "View performance details in Event log", Box::new(|| spawn_tool("eventvwr"))),
        (&["utilities-system-monitor-perf", "utilities-system-monitor"], "Open Performance Monitor", Box::new(|| { util::spawn(&["aerode-taskmgr"]); })),
        (&["utilities-system-monitor"], "Open Resource Monitor", Box::new(|| { util::spawn(&["aerode-taskmgr"]); })),
        (&["utilities-system-monitor"], "Open Task Manager", Box::new(|| { util::spawn(&["aerode-taskmgr"]); })),
        (&["aerode-msinfo", "dialog-information"], "View advanced system details in System Information", Box::new(|| spawn_tool("sysinfo"))),
        (&["preferences-desktop-display"], "Adjust the appearance and performance of Windows", { let cp = cp.clone(); Box::new(move || super::super::hw::visual_effects(&cp)) }),
        (&["aerode-cleanmgr", "user-trash-full"], "Open Disk Cleanup", Box::new(|| spawn_tool("cleanmgr"))),
    ];
    let list = vbox(6);
    list.set_margin_start(10);
    for (icon, t, f) in tools {
        let h = hbox(8);
        h.pack_start(&img(icon, 32), false, false, 0);
        h.pack_start(&link(t, f), false, false, 0);
        list.pack_start(&h, false, false, 0);
    }
    v.pack_start(&list, false, false, 0);
    subpage(cp, holder, "perfinfo", "Advanced Tools", &v);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scores() {
        assert_eq!(mem_score(12.0, 16.0), 7.9);
        assert_eq!(mem_score(3.0, 16.0), 5.9);
        assert_eq!(mem_score(12.0, 1.5), 4.9);
        assert_eq!(disk_score(100.0, 150.0), 4.9);
        assert_eq!(disk_score(500.0, 8000.0), 7.9);
        let sw = Gpu { renderer: "llvmpipe (LLVM 17.0.6, 256 bits)".into(), gl: 4.5, software: true, ..Default::default() };
        assert_eq!(gpu_scores(&sw).1, 1.0);
        let dg = Gpu { renderer: "NVIDIA GeForce RTX 3070".into(), gl: 4.6, vram_mb: 8192, ..Default::default() };
        assert_eq!(gpu_scores(&dg), (7.9, 7.9));
        assert!(cpu_score(1.0) < cpu_score(100.0));
    }
}
