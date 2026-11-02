//! Disk Management (diskmgmt.msc): the volume list on top and the graphical disk map below –
//! one bar per disk, partitions with their coloured stripe (primary, logical, extended, free,
//! unallocated) – from `lsblk -J`. Mount/unmount ("Change Drive Letter and Paths") through
//! udisksctl, Format through mkfs (pkexec), everything that moves partitions in GParted / KDE
//! Partition Manager / GNOME Disks.
use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf::Pixbuf, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::mmc::{self, act, act_off, Mmc};
use super::*;

#[derive(Clone, Default, Debug)]
pub struct Vol {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub start: u64,
    pub fstype: String,
    pub label: String,
    pub uuid: String,
    pub parttype: String,
    pub parttypename: String,
    pub mounts: Vec<String>,
    pub avail: Option<u64>,
    pub fssize: Option<u64>,
    pub number: u32,
    /// unlocked LUKS / LVM children
    pub inner: Vec<Vol>,
}

#[derive(Clone, Default, Debug)]
pub struct Disk {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub model: String,
    pub tran: String,
    pub removable: bool,
    pub rom: bool,
    pub pttype: String,
    pub parts: Vec<Vol>,
    /// a file system directly on the disk (no partition table)
    pub whole: Option<Vol>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { Primary, Logical, Extended, Free, Unallocated }

/// One box of the graphical view (an extended partition contains its logical drives).
#[derive(Clone, Debug)]
pub struct Region { pub kind: Kind, pub size: u64, pub vol: Option<Vol>, pub inner: Vec<Region> }

fn s(v: &serde_json::Value, k: &str) -> String { v[k].as_str().unwrap_or("").to_string() }
fn n(v: &serde_json::Value, k: &str) -> Option<u64> { v[k].as_u64().or_else(|| v[k].as_str().and_then(|x| x.parse().ok())) }

fn vol(v: &serde_json::Value) -> Vol {
    let mut mounts: Vec<String> = v["mountpoints"].as_array().map(|a| a.iter().filter_map(|m| m.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    if mounts.is_empty() { if let Some(m) = v["mountpoint"].as_str() { mounts.push(m.to_string()); } }
    let name = s(v, "name");
    let start = n(v, "start").map(|x| x * 512).unwrap_or_else(|| std::fs::read_to_string(format!("/sys/class/block/{name}/start")).ok().and_then(|x| x.trim().parse::<u64>().ok()).unwrap_or(0) * 512);
    Vol {
        path: { let p = s(v, "path"); if p.is_empty() { format!("/dev/{name}") } else { p } },
        name, size: n(v, "size").unwrap_or(0), start,
        fstype: s(v, "fstype"), label: { let l = s(v, "label"); if l.is_empty() { s(v, "partlabel") } else { l } }, uuid: s(v, "uuid"),
        parttype: s(v, "parttype").to_lowercase(), parttypename: s(v, "parttypename"), mounts, avail: n(v, "fsavail"), fssize: n(v, "fssize"),
        number: n(v, "partn").map(|x| x as u32).unwrap_or(0),
        inner: v["children"].as_array().map(|c| c.iter().map(vol).collect()).unwrap_or_default(),
    }
}

/// Parse `lsblk -J -b -o …`.
pub fn parse_lsblk(json: &str) -> Vec<Disk> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let mut disks = vec![];
    for d in v["blockdevices"].as_array().into_iter().flatten() {
        let ty = s(d, "type");
        let name = s(d, "name");
        if !(ty == "disk" || ty == "rom") || name.starts_with("zram") || name.starts_with("loop") || name.starts_with("ram") { continue; }
        let mut disk = Disk { name: name.clone(), path: { let p = s(d, "path"); if p.is_empty() { format!("/dev/{name}") } else { p } }, size: n(d, "size").unwrap_or(0),
            model: s(d, "model").trim().to_string(), tran: s(d, "tran"), removable: d["rm"].as_bool().unwrap_or(false) || s(d, "rm") == "1" || d["hotplug"].as_bool().unwrap_or(false),
            rom: ty == "rom", pttype: s(d, "pttype"), parts: vec![], whole: None };
        for c in d["children"].as_array().into_iter().flatten() {
            if s(c, "type") == "part" { disk.parts.push(vol(c)); }
        }
        if disk.parts.is_empty() && (!s(d, "fstype").is_empty() || d["mountpoints"].as_array().map(|a| a.iter().any(|m| m.is_string())).unwrap_or(false)) { disk.whole = Some(vol(d)); }
        disk.parts.sort_by_key(|p| p.start);
        disks.push(disk);
    }
    disks
}

const MIB: u64 = 1024 * 1024;

fn is_extended(p: &Vol) -> bool { matches!(p.parttype.as_str(), "0x5" | "0xf" | "0x85" | "0x05" | "0x0f") }

/// The boxes of a disk's bar, with unallocated gaps (> 1 MB) like Win7.
pub fn regions(d: &Disk) -> Vec<Region> {
    if d.rom || d.size == 0 { return vec![]; }
    if let Some(w) = &d.whole { return vec![Region { kind: Kind::Primary, size: d.size, vol: Some(w.clone()), inner: vec![] }]; }
    let mut out = vec![];
    let mut pos = MIB.min(d.size);
    let first_usable = if d.pttype == "gpt" { 34 * 512 } else { 512 };
    if d.parts.is_empty() { return vec![Region { kind: Kind::Unallocated, size: d.size, vol: None, inner: vec![] }]; }
    pos = pos.min(d.parts[0].start).max(first_usable);
    let logical: Vec<&Vol> = d.parts.iter().filter(|p| d.pttype == "dos" && p.number >= 5).collect();
    for p in d.parts.iter().filter(|p| !(d.pttype == "dos" && p.number >= 5)) {
        if p.start > pos + MIB { out.push(Region { kind: Kind::Unallocated, size: p.start - pos, vol: None, inner: vec![] }); }
        if is_extended(p) {
            let mut inner = vec![];
            let mut ip = p.start;
            for l in logical.iter().filter(|l| l.start >= p.start && l.start < p.start + p.size) {
                if l.start > ip + 2 * MIB { inner.push(Region { kind: Kind::Free, size: l.start - ip, vol: None, inner: vec![] }); }
                inner.push(Region { kind: Kind::Logical, size: l.size, vol: Some((*l).clone()), inner: vec![] });
                ip = l.start + l.size;
            }
            if p.start + p.size > ip + 2 * MIB { inner.push(Region { kind: Kind::Free, size: p.start + p.size - ip, vol: None, inner: vec![] }); }
            out.push(Region { kind: Kind::Extended, size: p.size, vol: Some(p.clone()), inner });
        } else {
            out.push(Region { kind: Kind::Primary, size: p.size, vol: Some(p.clone()), inner: vec![] });
        }
        pos = pos.max(p.start + p.size);
    }
    // GPT keeps 33 sectors of backup table at the end
    let end = if d.pttype == "gpt" { d.size.saturating_sub(33 * 512) } else { d.size };
    if end > pos + MIB { out.push(Region { kind: Kind::Unallocated, size: end - pos, vol: None, inner: vec![] }); }
    out
}

/// "NTFS", "FAT32", "EXT4"… (Win7 shows file systems in capitals).
pub fn fs_name(v: &Vol) -> String {
    match v.fstype.as_str() {
        "vfat" => if v.fssize.unwrap_or(v.size) > 32 * 1024 * MIB { "FAT32".into() } else { "FAT32".into() },
        "crypto_LUKS" => match v.inner.iter().find(|i| !i.fstype.is_empty()) { Some(i) => format!("{} (BitLocker Encrypted)", fs_name(i)), None => "BitLocker Encrypted".into() },
        "swap" => "Linux swap".into(),
        "LVM2_member" => "LVM".into(),
        "" => String::new(),
        f => f.to_uppercase(),
    }
}

const SYSTEM_MOUNTS: [&str; 8] = ["/", "/boot", "/boot/efi", "/efi", "/home", "/usr", "/var", "[SWAP]"];

/// "Healthy (System, Boot, Page File, Primary Partition)".
pub fn status(v: &Vol, kind: Kind) -> String {
    let mut f: Vec<&str> = vec![];
    let all_mounts: Vec<String> = v.mounts.iter().cloned().chain(v.inner.iter().flat_map(|i| i.mounts.clone())).collect();
    if v.parttype == "c12a7328-f81f-11d2-ba4b-00a0c93ec93b" || v.parttype == "0xef" { f.push("EFI System Partition"); }
    if all_mounts.iter().any(|m| m == "/boot" || m == "/boot/efi" || m == "/efi") { f.push("System"); }
    if all_mounts.iter().any(|m| m == "/") { f.push("Boot"); }
    if v.fstype == "swap" || all_mounts.iter().any(|m| m == "[SWAP]") { f.push("Page File"); }
    if v.fstype == "crypto_LUKS" { f.push(if v.inner.is_empty() { "Locked" } else { "Unlocked" }); }
    match kind { Kind::Logical => f.push("Logical Drive"), Kind::Primary => f.push("Primary Partition"), _ => {} }
    if f.is_empty() { "Healthy".into() } else { format!("Healthy ({})", f.join(", ")) }
}

/// The mount point(s) a volume is reachable at (its own or its unlocked child's).
fn mounts(v: &Vol) -> Vec<String> { v.mounts.iter().cloned().chain(v.inner.iter().flat_map(|i| i.mounts.clone())).filter(|m| !m.is_empty() && !m.starts_with('[')).collect() }

/// "Data (/mnt/data)", "(/)", "EFI (/boot)"
pub fn vol_title(v: &Vol) -> String {
    let m = mounts(v);
    let label = if v.label.is_empty() { v.inner.iter().map(|i| i.label.clone()).find(|l| !l.is_empty()).unwrap_or_default() } else { v.label.clone() };
    match (label.is_empty(), m.first()) {
        (false, Some(mp)) => format!("{label} ({mp})"),
        (true, Some(mp)) => format!("({mp})"),
        (false, None) => label,
        (true, None) => String::new(),
    }
}

pub fn load() -> Vec<Disk> {
    let full = ["lsblk", "-J", "-b", "-o", "NAME,PATH,TYPE,SIZE,START,FSTYPE,LABEL,UUID,PARTTYPE,PARTTYPENAME,PARTLABEL,PTTYPE,MOUNTPOINTS,FSAVAIL,FSSIZE,MODEL,TRAN,RM,HOTPLUG,PARTN"];
    let mut out = sh(&full);
    if out.trim().is_empty() { out = sh(&["lsblk", "-J", "-b", "-o", "NAME,TYPE,SIZE,FSTYPE,LABEL,UUID,PARTTYPE,MOUNTPOINT,FSAVAIL,FSSIZE,MODEL,TRAN,RM"]); }
    parse_lsblk(&out)
}

struct State {
    m: Rc<Mmc>,
    disks: RefCell<Vec<Disk>>,
    /// (disk, region path: [top] or [extended, logical]); region usize::MAX = the disk label cell
    sel: Cell<Option<(usize, usize, Option<usize>)>>,
    list: gtk::ListStore,
    view: gtk::TreeView,
    map: gtk::Box,
    legend: gtk::Box,
    holder: gtk::Paned,
    syncing: Cell<bool>,
}

pub fn attach(m: &Rc<Mmc>, parent: Option<&gtk::TreeIter>) {
    m.add(parent, "dk", "Disk Management", &["aerode-diskmgmt", "drive-harddisk"]);
    let st: Rc<RefCell<Option<Rc<State>>>> = Rc::new(RefCell::new(None));
    m.handle("dk", move |m, _| {
        let existing = st.borrow().clone();
        let s = match existing { Some(s) => s, None => { let s = new_state(m); *st.borrow_mut() = Some(s.clone()); s } };
        m.set_center(&s.holder);
        reload(&s);
    });
}

pub fn window(app: &gtk::Application) -> gtk::Window {
    let m = Mmc::new(app, "Disk Management", &["aerode-diskmgmt", "drive-harddisk"], false, true);
    let s = new_state(&m);
    m.keep(s.clone());
    { let w = Rc::downgrade(&s); m.tool(mmc::I_PROPS, "Properties", move || if let Some(s) = w.upgrade() { props_selected(&s) }); }
    { let w = Rc::downgrade(&s); m.tool(mmc::I_REFRESH, "Refresh", move || if let Some(s) = w.upgrade() { reload(&s) }); }
    m.set_center(&s.holder);
    m.show("");
    reload(&s);
    m.win.clone()
}

fn new_state(m: &Rc<Mmc>) -> Rc<State> {
    // icon, Volume, Layout, Type, File System, Status, Capacity, Free Space, % Free, Fault Tolerance, Overhead, disk idx, region idx, inner idx (-1)
    let list = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::I32, glib::Type::I32, glib::Type::I32]);
    let view = gtk::TreeView::with_model(&list);
    icon_column(&view, "Volume", 0, 1, 170);
    columns(&view, &[("Layout", 2, 60), ("Type", 3, 50), ("File System", 4, 90), ("Status", 5, 260), ("Capacity", 6, 80), ("Free Space", 7, 80), ("% Free", 8, 55), ("Fault Tolerance", 9, 100), ("Overhead", 10, 0)]);
    let map = vbox(6);
    map.set_border_width(6);
    let legend = hbox(14);
    legend.set_border_width(4);
    let bottom = vbox(0);
    let msw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    msw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    msw.add(&map);
    bottom.pack_start(&msw, true, true, 0);
    bottom.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
    bottom.pack_start(&legend, false, false, 0);
    let holder = gtk::Paned::new(gtk::Orientation::Vertical);
    holder.pack1(&scrolled(&view), false, false);
    holder.pack2(&bottom, true, false);
    holder.set_position(170);
    let s = Rc::new(State { m: m.clone(), disks: RefCell::new(vec![]), sel: Cell::new(None), list, view, map, legend, holder, syncing: Cell::new(false) });
    { let w = Rc::downgrade(&s); s.view.selection().connect_changed(move |sel| if let Some(s) = w.upgrade() {
        if s.syncing.get() { return; }
        if let Some((m, it)) = sel.selected() {
            let (d, r, i): (i32, i32, i32) = (m.value(&it, 11).get().unwrap_or(0), m.value(&it, 12).get().unwrap_or(0), m.value(&it, 13).get().unwrap_or(-1));
            s.sel.set(Some((d as usize, r as usize, if i < 0 { None } else { Some(i as usize) })));
            for c in s.map.children() { c.queue_draw(); }
            actions(&s);
        }
    }); }
    { let w = Rc::downgrade(&s); s.view.connect_button_press_event(move |tv, ev| {
        if ev.button() == 3 { if let (Some(s), Some((Some(p), _, _, _))) = (w.upgrade(), tv.path_at_pos(ev.position().0 as i32, ev.position().1 as i32)) { tv.selection().select_path(&p); menu(&s, ev); return glib::Propagation::Stop; } }
        glib::Propagation::Proceed
    }); }
    { let w = Rc::downgrade(&s); s.view.connect_row_activated(move |_, _, _| if let Some(s) = w.upgrade() { props_selected(&s) }); }
    s
}

fn reload(s: &Rc<State>) {
    s.m.status.set_text("Connecting to Virtual Disk Service...");
    let s2 = s.clone();
    bg(load, move |disks| { *s2.disks.borrow_mut() = disks; s2.sel.set(None); fill(&s2); s2.m.status.set_text(""); actions(&s2); });
}

fn region_at(s: &State, d: usize, r: usize, i: Option<usize>) -> Option<Region> {
    let disks = s.disks.borrow();
    let regs = regions(disks.get(d)?);
    let top = regs.get(r)?.clone();
    match i { Some(i) => top.inner.get(i).cloned(), None => Some(top) }
}

fn sel_region(s: &State) -> Option<Region> { let (d, r, i) = s.sel.get()?; if r == usize::MAX { return None; } region_at(s, d, r, i) }
fn sel_disk(s: &State) -> Option<Disk> { let (d, _, _) = s.sel.get()?; s.disks.borrow().get(d).cloned() }

fn fill(s: &Rc<State>) {
    s.list.clear();
    for c in s.map.children() { s.map.remove(&c); }
    for c in s.legend.children() { s.legend.remove(&c); }
    let disks = s.disks.borrow().clone();
    let mut kinds: Vec<Kind> = vec![];
    let (mut ndisk, mut nrom) = (0, 0);
    for (di, d) in disks.iter().enumerate() {
        let regs = regions(d);
        let add_row = |v: &Vol, kind: Kind, ri: usize, ii: i32| {
            let free = v.avail.or_else(|| v.inner.iter().find_map(|i| i.avail));
            let cap = v.size;
            let pb = pix(if mounts(v).iter().any(|m| m == "/") { &["drive-harddisk-system", "drive-harddisk"] } else if d.removable { &["drive-removable-media", "drive-harddisk"] } else { &["drive-harddisk"] }, 16);
            let title = vol_title(v);
            s.list.insert_with_values(None, &[(0, &pb), (1, &if title.is_empty() { v.name.clone() } else { title }), (2, &"Simple"), (3, &"Basic"), (4, &fs_name(v)), (5, &status(v, kind)),
                (6, &gb(cap)), (7, &free.map(gb).unwrap_or_default()), (8, &free.map(|f| format!("{} %", (f as f64 * 100.0 / cap.max(1) as f64).round())).unwrap_or_default()), (9, &"No"), (10, &"0%"),
                (11, &(di as i32)), (12, &(ri as i32)), (13, &ii)]);
        };
        for (ri, r) in regs.iter().enumerate() {
            if !kinds.contains(&r.kind) { kinds.push(r.kind); }
            if let Some(v) = &r.vol { if r.kind != Kind::Extended { add_row(v, r.kind, ri, -1); } }
            for (ii, x) in r.inner.iter().enumerate() {
                if !kinds.contains(&x.kind) { kinds.push(x.kind); }
                if let Some(v) = &x.vol { add_row(v, x.kind, ri, ii as i32); }
            }
        }
        let label = if d.rom { nrom += 1; format!("CD-ROM {}", nrom - 1) } else { ndisk += 1; format!("Disk {}", ndisk - 1) };
        s.map.pack_start(&disk_row(s, di, &label, d, regs), false, false, 0);
    }
    // legend (Win7 order)
    for (k, name) in [(Kind::Unallocated, "Unallocated"), (Kind::Primary, "Primary partition"), (Kind::Extended, "Extended partition"), (Kind::Free, "Free space"), (Kind::Logical, "Logical drive")] {
        if !kinds.contains(&k) { continue; }
        let h = hbox(4);
        let sw = gtk::DrawingArea::new();
        sw.set_size_request(14, 10);
        sw.set_valign(gtk::Align::Center);
        sw.connect_draw(move |_, cr| { let (r, g, b) = color(k); cr.set_source_rgb(r, g, b); cr.rectangle(0.0, 0.0, 14.0, 10.0); let _ = cr.fill(); cr.set_source_rgb(0.4, 0.4, 0.4); cr.rectangle(0.5, 0.5, 13.0, 9.0); cr.set_line_width(1.0); let _ = cr.stroke(); glib::Propagation::Stop });
        h.pack_start(&sw, false, false, 0);
        h.pack_start(&gtk::Label::new(Some(name)), false, false, 0);
        s.legend.pack_start(&h, false, false, 0);
    }
    s.map.show_all();
    s.legend.show_all();
}

/// Win7 stripe colours.
fn color(k: Kind) -> (f64, f64, f64) {
    match k {
        Kind::Primary => (0.09, 0.17, 0.55),
        Kind::Logical => (0.27, 0.53, 0.86),
        Kind::Extended => (0.0, 0.62, 0.0),
        Kind::Free => (0.55, 0.88, 0.55),
        Kind::Unallocated => (0.0, 0.0, 0.0),
    }
}

const LABEL_W: f64 = 150.0;
const ROW_H: i32 = 78;

/// Horizontal layout of the boxes: proportional, but every box at least `min` wide.
pub fn layout(sizes: &[u64], width: f64, min: f64) -> Vec<(f64, f64)> {
    let n = sizes.len();
    if n == 0 { return vec![]; }
    let total: u64 = sizes.iter().sum::<u64>().max(1);
    let mut w: Vec<f64> = sizes.iter().map(|s| width * *s as f64 / total as f64).collect();
    // raise the small ones to the minimum and take the space from the big ones
    for _ in 0..4 {
        let deficit: f64 = w.iter().filter(|x| **x < min).map(|x| min - x).sum();
        if deficit <= 0.0 { break; }
        let big: f64 = w.iter().filter(|x| **x > min).map(|x| x - min).sum();
        if big <= 0.0 { let e = width / n as f64; w = vec![e; n]; break; }
        w = w.iter().map(|x| if *x < min { min } else { x - (x - min) / big * deficit }).collect();
    }
    let mut x = 0.0;
    w.iter().map(|wi| { let r = (x, *wi); x += wi; r }).collect()
}

fn disk_row(s: &Rc<State>, di: usize, label: &str, d: &Disk, regs: Vec<Region>) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::new();
    da.set_size_request(-1, ROW_H);
    da.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
    let lines: Vec<String> = if d.rom {
        let media = d.parts.first().or(d.whole.as_ref()).map(|v| vol_title(v)).unwrap_or_default();
        vec![label.into(), if d.model.to_lowercase().contains("dvd") { "DVD".into() } else { "CD-ROM".into() }, if d.size == 0 { "No Media".into() } else { gb(d.size) }, if media.is_empty() { String::new() } else { media }]
    } else {
        vec![label.into(), if d.removable { "Removable".into() } else { "Basic".into() }, gb(d.size), "Online".into()]
    };
    let icon = pix(if d.rom { &["aerode-dm-cdrom", "drive-optical"] } else if d.removable { &["aerode-dm-removable", "drive-removable-media"] } else { &["aerode-dm-disk", "drive-harddisk"] }, 16);
    let w = Rc::downgrade(s);
    let regs2 = regs.clone();
    da.connect_draw(move |da, cr| {
        let Some(s) = w.upgrade() else { return glib::Propagation::Stop };
        let width = da.allocated_width() as f64;
        let h = ROW_H as f64 - 4.0;
        let sel = s.sel.get();
        // disk label cell: grey gradient
        let g = gtk::cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
        g.add_color_stop_rgb(0.0, 0.96, 0.96, 0.96);
        g.add_color_stop_rgb(1.0, 0.86, 0.87, 0.89);
        cr.rectangle(0.5, 0.5, LABEL_W - 3.0, h);
        let _ = cr.set_source(&g);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(0.55, 0.55, 0.58);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        if sel == Some((di, usize::MAX, None)) { hatch(cr, 1.0, 1.0, LABEL_W - 4.0, h - 1.0); }
        if let Some(pb) = &icon { cr.set_source_pixbuf(pb, 5.0, 5.0); let _ = cr.paint(); }
        let layout = da.create_pango_layout(None);
        let attrs = |bold: bool| { let a = gtk::pango::AttrList::new(); if bold { a.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold)); } a };
        for (i, l) in lines.iter().enumerate() {
            layout.set_text(l);
            layout.set_attributes(Some(&attrs(i == 0)));
            cr.set_source_rgb(0.0, 0.0, 0.0);
            cr.move_to(26.0, 3.0 + i as f64 * 16.0);
            pangocairo::functions::show_layout(cr, &layout);
        }
        layout.set_attributes(None);
        // partitions
        let x0 = LABEL_W;
        let area = width - x0 - 2.0;
        let boxes = layout_regions(&regs2, area);
        for (ri, (r, (bx, bw))) in regs2.iter().zip(boxes.iter()).enumerate() {
            let (x, wdt) = (x0 + bx, *bw - 2.0);
            draw_box(cr, &layout, r, x, 0.5, wdt, h, sel == Some((di, ri, None)));
            if r.kind == Kind::Extended {
                let inner = layout_regions(&r.inner, wdt - 8.0);
                for (ii, (x2, (ix, iw))) in r.inner.iter().zip(inner.iter()).enumerate() {
                    draw_box(cr, &layout, x2, x + 4.0 + ix, 5.0, iw - 2.0, h - 9.0, sel == Some((di, ri, Some(ii))));
                }
            }
        }
        if regs2.is_empty() && !lines.is_empty() {
            cr.set_source_rgb(0.55, 0.55, 0.58);
            cr.rectangle(x0 + 0.5, 0.5, area, h);
            let _ = cr.stroke();
        }
        glib::Propagation::Stop
    });
    let w = Rc::downgrade(s);
    da.connect_button_press_event(move |da, ev| {
        let Some(s) = w.upgrade() else { return glib::Propagation::Stop };
        let (x, y) = ev.position();
        let width = da.allocated_width() as f64;
        let mut hit = (di, usize::MAX, None);
        if x > LABEL_W {
            let boxes = layout_regions(&regs, width - LABEL_W - 2.0);
            for (ri, (r, (bx, bw))) in regs.iter().zip(boxes.iter()).enumerate() {
                let rx = x - LABEL_W;
                if rx >= *bx && rx < bx + bw {
                    hit = (di, ri, None);
                    if r.kind == Kind::Extended && y > 5.0 {
                        for (ii, (ix, iw)) in layout_regions(&r.inner, bw - 10.0).iter().enumerate() { if rx - bx - 4.0 >= *ix && rx - bx - 4.0 < ix + iw { hit = (di, ri, Some(ii)); } }
                    }
                }
            }
        }
        s.sel.set(Some(hit));
        // select the matching volume row on top
        s.syncing.set(true);
        s.view.selection().unselect_all();
        s.list.foreach(|m, p, it| {
            let (d, r, i): (i32, i32, i32) = (m.value(it, 11).get().unwrap_or(-1), m.value(it, 12).get().unwrap_or(-1), m.value(it, 13).get().unwrap_or(-1));
            if d as usize == hit.0 && r as usize == hit.1 && (i < 0 && hit.2.is_none() || hit.2 == Some(i as usize)) { s.view.selection().select_path(p); true } else { false }
        });
        s.syncing.set(false);
        for c in s.map.children() { c.queue_draw(); }
        actions(&s);
        if ev.button() == 3 { menu(&s, ev); }
        else if ev.event_type() == gdk::EventType::DoubleButtonPress { props_selected(&s); }
        glib::Propagation::Stop
    });
    da
}

fn layout_regions(r: &[Region], width: f64) -> Vec<(f64, f64)> { layout(&r.iter().map(|x| x.size).collect::<Vec<_>>(), width, 90.0) }

fn hatch(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.set_source_rgba(0.45, 0.45, 0.45, 0.55);
    cr.set_line_width(1.0);
    let mut i = -h;
    while i < w { cr.move_to(x + i, y + h); cr.line_to(x + i + h, y); i += 4.0; }
    let _ = cr.stroke();
    cr.restore().ok();
}

fn draw_box(cr: &gtk::cairo::Context, layout: &gtk::pango::Layout, r: &Region, x: f64, y: f64, w: f64, h: f64, selected: bool) {
    let (cr_, cg, cb) = color(r.kind);
    if r.kind == Kind::Extended {
        cr.set_source_rgb(cr_, cg, cb);
        cr.set_line_width(2.0);
        cr.rectangle(x + 1.0, y + 0.5, w - 1.0, h);
        let _ = cr.stroke();
        return;
    }
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();
    if selected { hatch(cr, x, y, w, h); }
    cr.set_source_rgb(cr_, cg, cb);
    cr.rectangle(x, y, w, 7.0);
    let _ = cr.fill();
    cr.set_source_rgb(0.55, 0.55, 0.58);
    cr.set_line_width(1.0);
    cr.rectangle(x + 0.5, y, w - 1.0, h);
    let _ = cr.stroke();
    let lines: Vec<String> = match (&r.vol, r.kind) {
        (_, Kind::Unallocated) => vec![String::new(), gb(r.size), "Unallocated".into()],
        (_, Kind::Free) => vec![String::new(), gb(r.size), "Free space".into()],
        (Some(v), k) => { let t = vol_title(v); vec![t, format!("{} {}", gb(v.size), fs_name(v)).trim().to_string(), status(v, k)] }
        _ => vec![],
    };
    layout.set_width(((w - 8.0).max(10.0) * gtk::pango::SCALE as f64) as i32);
    layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
    for (i, l) in lines.iter().enumerate() {
        if l.is_empty() { continue; }
        layout.set_text(l);
        let a = gtk::pango::AttrList::new();
        if i == 0 { a.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold)); }
        layout.set_attributes(Some(&a));
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.move_to(x + 4.0, y + 10.0 + i as f64 * 16.0);
        pangocairo::functions::show_layout(cr, layout);
    }
    layout.set_width(-1);
    layout.set_attributes(None);
}

fn actions(s: &Rc<State>) {
    let w = Rc::downgrade(s);
    let g = move || w.upgrade();
    let r = g.clone();
    let mut groups = vec![("Disk Management".to_string(), vec![
        act("Rescan Disks", mmc::I_REFRESH, move || if let Some(s) = r() { reload(&s) }),
        act("Open partition editor", &["gparted", "drive-harddisk"], || partition_tool("")),
        act_off("Create VHD"), act_off("Attach VHD"),
        act_off("More Actions ▸"),
    ])];
    if let Some(reg) = sel_region(s) {
        let disk = sel_disk(s).unwrap_or_default();
        let mut v = vec![];
        if let Some(vol) = reg.vol.clone() {
            let m = mounts(&vol);
            if let Some(mp) = m.first().cloned() { let mp2 = mp.clone(); v.push(act("Open", mmc::I_OPEN, move || util::explore(&mp2))); v.push(act("Explore", NONE, move || util::explore(&mp))); }
            let (g1, vol1) = (g.clone(), vol.clone());
            v.push(act("Change Drive Letter and Paths...", NONE, move || if let Some(s) = g1() { paths_dialog(&s, &vol1) }));
            let (g2, vol2) = (g.clone(), vol.clone());
            if reg.kind != Kind::Extended { v.push(act("Format...", NONE, move || if let Some(s) = g2() { format_dialog(&s, &vol2) })); }
            let p = disk.path.clone();
            v.push(act("Extend Volume...", NONE, { let p = p.clone(); move || partition_tool(&p) }));
            v.push(act("Shrink Volume...", NONE, { let p = p.clone(); move || partition_tool(&p) }));
            v.push(act("Delete Volume...", mmc::I_DELETE, move || partition_tool(&p)));
            let (g3, vol3, k) = (g.clone(), vol.clone(), reg.kind);
            v.push(act("Properties", mmc::I_PROPS, move || if let Some(s) = g3() { vol_properties(&s, &vol3, k) }));
            groups.push((if vol_title(&vol).is_empty() { vol.name.clone() } else { vol_title(&vol) }, v));
        } else {
            let p = disk.path.clone();
            groups.push(("Unallocated".into(), vec![act("New Simple Volume...", NONE, move || partition_tool(&p)), act_off("New Spanned Volume..."), act_off("New Striped Volume...")]));
        }
    }
    s.m.set_actions(groups);
}
const NONE: &[&str] = &[];

fn props_selected(s: &Rc<State>) {
    match (sel_region(s), s.sel.get()) {
        (Some(Region { vol: Some(v), kind, .. }), _) => vol_properties(s, &v, kind),
        (_, Some((d, usize::MAX, _))) => if let Some(d) = s.disks.borrow().get(d).cloned() { disk_properties(s, &d) },
        _ => {}
    }
}

fn menu(s: &Rc<State>, ev: &gdk::EventButton) {
    let m = gtk::Menu::new();
    let add = |l: &str, f: Option<Box<dyn Fn()>>| { let mi = gtk::MenuItem::with_label(l); match f { Some(f) => { mi.connect_activate(move |_| f()); } None => mi.set_sensitive(false) } m.append(&mi); };
    let w = Rc::downgrade(s);
    match (sel_region(s), s.sel.get()) {
        (Some(reg), _) if reg.vol.is_some() && reg.kind != Kind::Extended => {
            let v = reg.vol.clone().unwrap();
            let disk = sel_disk(s).unwrap_or_default();
            let mp = mounts(&v).first().cloned();
            { let mp = mp.clone(); add("Open", mp.map(|p| Box::new(move || util::explore(&p)) as Box<dyn Fn()>)); }
            { let mp = mp.clone(); add("Explore", mp.map(|p| Box::new(move || util::explore(&p)) as Box<dyn Fn()>)); }
            m.append(&gtk::SeparatorMenuItem::new());
            add("Mark Partition as Active", None);
            { let (w, v) = (w.clone(), v.clone()); add("Change Drive Letter and Paths...", Some(Box::new(move || if let Some(s) = w.upgrade() { paths_dialog(&s, &v) }))); }
            { let (w, v) = (w.clone(), v.clone()); add("Format...", Some(Box::new(move || if let Some(s) = w.upgrade() { format_dialog(&s, &v) }))); }
            m.append(&gtk::SeparatorMenuItem::new());
            for l in ["Extend Volume...", "Shrink Volume..."] { let p = disk.path.clone(); add(l, Some(Box::new(move || partition_tool(&p)))); }
            add("Add Mirror...", None);
            { let p = disk.path.clone(); add("Delete Volume...", Some(Box::new(move || partition_tool(&p)))); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let (w, v, k) = (w.clone(), v.clone(), reg.kind); add("Properties", Some(Box::new(move || if let Some(s) = w.upgrade() { vol_properties(&s, &v, k) }))); }
            add("Help", None);
        }
        (Some(reg), _) if reg.kind == Kind::Unallocated || reg.kind == Kind::Free => {
            let p = sel_disk(s).map(|d| d.path).unwrap_or_default();
            add("New Simple Volume...", Some(Box::new(move || partition_tool(&p))));
            add("New Spanned Volume...", None);
            add("New Striped Volume...", None);
            add("New Mirrored Volume...", None);
            add("New RAID-5 Volume...", None);
            m.append(&gtk::SeparatorMenuItem::new());
            add("Properties", None);
            add("Help", None);
        }
        (_, Some((d, usize::MAX, _))) => {
            let disk = s.disks.borrow().get(d).cloned().unwrap_or_default();
            add("New Spanned Volume...", None);
            add("Convert to Dynamic Disk...", None);
            add(if disk.pttype == "gpt" { "Convert to MBR Disk" } else { "Convert to GPT Disk" }, None);
            m.append(&gtk::SeparatorMenuItem::new());
            if disk.removable || disk.rom { let p = disk.path.clone(); add("Eject", Some(Box::new(move || { let p = p.clone(); bg(move || run(&["udisksctl", "power-off", "-b", &p]), |(ok, out)| { report("The device could not be ejected.", ok, &out); }); }))); }
            else { add("Offline", None); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let (w, disk) = (w.clone(), disk.clone()); add("Properties", Some(Box::new(move || if let Some(s) = w.upgrade() { disk_properties(&s, &disk) }))); }
            add("Help", None);
        }
        _ => return,
    }
    m.show_all();
    m.popup_at_pointer(Some(ev));
}

/// GParted, KDE Partition Manager or GNOME Disks for everything that changes partitions.
pub fn partition_tool(dev: &str) {
    let tries: Vec<Vec<&str>> = vec![
        if dev.is_empty() { vec!["gparted"] } else { vec!["gparted", dev] },
        vec!["partitionmanager"],
        if dev.is_empty() { vec!["gnome-disks"] } else { vec!["gnome-disks", "--block-device", dev] },
        vec!["kde-partitionmanager"],
    ];
    for t in tries { if util::spawn(&t) { return; } }
    let d = gtk::MessageDialog::new(None::<&gtk::Window>, gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::None, "Creating, deleting, resizing and moving partitions is done in a partition editor.");
    d.set_title("Disk Management");
    d.set_secondary_text(Some("No partition editor is installed. GParted is recommended (it can also shrink and extend volumes)."));
    let b = d.add_button("Install GParted", gtk::ResponseType::Ok);
    if let Ok(b) = b.downcast::<gtk::Button>() { shield(&b); }
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let r = d.run();
    d.close();
    if r == gtk::ResponseType::Ok {
        elevate(&["pacman", "-S", "--needed", "--noconfirm", "gparted"], |ok, out| { if report("GParted could not be installed.", ok, &out) { util::spawn(&["gparted"]); } });
    }
}

/// "Change Drive Letter and Paths": the mount points; Add = mount, Remove = unmount (udisks).
fn paths_dialog(s: &Rc<State>, v: &Vol) {
    let target = v.inner.iter().find(|i| !i.fstype.is_empty() && i.fstype != "crypto_LUKS").cloned().unwrap_or_else(|| v.clone());
    let title = vol_title(v);
    let d = gtk::Dialog::with_buttons(Some(&format!("Change Drive Letter and Paths for {}", if title.is_empty() { &v.name } else { &title })), Some(&s.m.win), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(420, 260);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(8);
    c.pack_start(&label("Allow access to this volume by using the following drive letter and paths:", ""), false, false, 0);
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING]);
    for m in mounts(v) { store.insert_with_values(None, &[(0, &pix(&["drive-harddisk"], 16)), (1, &m)]); }
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    icon_column(&tv, "", 0, 1, 0);
    let sw = scrolled(&tv);
    sw.set_min_content_height(120);
    c.pack_start(&sw, true, true, 0);
    let bb = hbox(6);
    let add = gtk::Button::with_mnemonic("A_dd...");
    let change = gtk::Button::with_mnemonic("_Change...");
    let remove = gtk::Button::with_mnemonic("_Remove");
    change.set_sensitive(false);
    let mounted = !mounts(v).is_empty();
    add.set_sensitive(!mounted && !target.fstype.is_empty() && target.fstype != "swap");
    remove.set_sensitive(mounted);
    for b in [&add, &change, &remove] { bb.pack_start(b, false, false, 0); }
    c.pack_start(&bb, false, false, 0);
    let (path, s2, d2) = (target.path.clone(), s.clone(), d.clone());
    add.connect_clicked(move |_| {
        let (s3, d3) = (s2.clone(), d2.clone());
        let p = path.clone();
        // udisks asks polkit (our UAC prompt) when the volume is not the user's to mount
        bg(move || run(&["udisksctl", "mount", "-b", &p]), move |(ok, out)| {
            if report("The volume could not be mounted.", ok, &out) { reload(&s3); d3.close(); }
        });
    });
    let (path, s2, d2, mts) = (target.path.clone(), s.clone(), d.clone(), mounts(v));
    remove.connect_clicked(move |_| {
        if mts.iter().any(|m| SYSTEM_MOUNTS.contains(&m.as_str())) {
            inform(Some(d2.upcast_ref()), "Disk Management", "The parameter is incorrect.", "This volume is in use by the system (it holds the operating system, the boot files, the page file or your home folder) and cannot be removed.");
            return;
        }
        if !confirm(Some(d2.upcast_ref()), "Disk Management", "Programs that rely on drive letters might not run correctly. Do you want to remove this drive letter?", "The volume will be unmounted.", "Yes") { return; }
        let (s3, d3, p) = (s2.clone(), d2.clone(), path.clone());
        bg(move || run(&["udisksctl", "unmount", "-b", &p]), move |(ok, out)| { if report("The volume could not be unmounted.", ok, &out) { reload(&s3); d3.close(); } });
    });
    d.show_all();
    d.run();
    d.close();
}

/// Win7 "Format" dialog → mkfs through pkexec (never the running system's volumes).
fn format_dialog(s: &Rc<State>, v: &Vol) {
    let m = mounts(v);
    if m.iter().any(|x| SYSTEM_MOUNTS.contains(&x.as_str())) {
        inform(Some(&s.m.win), "Virtual Disk Manager", "You cannot format this volume. It contains the version of Windows that you are using. Formatting this volume could cause your computer to stop working.", "");
        return;
    }
    let d = gtk::Dialog::with_buttons(Some(&format!("Format {}", if vol_title(v).is_empty() { v.name.clone() } else { vol_title(v) })), Some(&s.m.win), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let g = gtk::Grid::new();
    g.set_border_width(12);
    g.set_row_spacing(8);
    g.set_column_spacing(10);
    g.attach(&label("Volume label:", ""), 0, 0, 1, 1);
    let lbl = gtk::Entry::new();
    lbl.set_text(&v.label);
    g.attach(&lbl, 1, 0, 1, 1);
    g.attach(&label("File system:", ""), 0, 1, 1, 1);
    let fs = gtk::ComboBoxText::new();
    for (id, l) in [("ext4", "EXT4"), ("btrfs", "BTRFS"), ("xfs", "XFS"), ("ntfs", "NTFS"), ("vfat", "FAT32"), ("exfat", "exFAT")] { if util::which(&format!("mkfs.{id}")) || id == "ext4" { fs.append(Some(id), l); } }
    if !fs.set_active_id(Some(&v.fstype)) { fs.set_active(Some(0)); }
    g.attach(&fs, 1, 1, 1, 1);
    g.attach(&label("Allocation unit size:", ""), 0, 2, 1, 1);
    let au = gtk::ComboBoxText::new();
    au.append_text("Default");
    au.set_active(Some(0));
    g.attach(&au, 1, 2, 1, 1);
    let quick = gtk::CheckButton::with_label("Perform a quick format");
    quick.set_active(true);
    g.attach(&quick, 0, 3, 2, 1);
    let comp = gtk::CheckButton::with_label("Enable file and folder compression");
    comp.set_sensitive(false);
    g.attach(&comp, 0, 4, 2, 1);
    d.content_area().pack_start(&g, true, true, 0);
    d.show_all();
    let r = d.run();
    let (label_s, fs_s, full) = (lbl.text().to_string(), fs.active_id().map(|x| x.to_string()).unwrap_or_else(|| "ext4".into()), !quick.is_active());
    d.close();
    if r != gtk::ResponseType::Ok { return; }
    if !confirm(Some(&s.m.win), "Format", "Formatting this volume will erase all data on it. Back up any data you want to keep before formatting. Do you want to continue?", &v.path, "OK") { return; }
    let lab = match fs_s.as_str() { "vfat" => "-n", "ntfs" => "-L", "exfat" => "-L", _ => "-L" };
    let quickflag = match (fs_s.as_str(), full) { ("ntfs", false) => " -Q", ("ext4", true) => " -c", _ => "" };
    let force = match fs_s.as_str() { "btrfs" | "xfs" => " -f", "ext4" => " -F", _ => "" };
    let mut script = String::new();
    for mp in &m { script += &format!("umount {} && ", q(mp)); }
    script += &format!("wipefs -a {p} && mkfs.{fs_s}{force}{quickflag}{} {p}", if label_s.is_empty() { String::new() } else { format!(" {lab} {}", q(&label_s)) }, p = q(&v.path));
    let s2 = s.clone();
    elevate_sh(&script, move |ok, out| { if report("Windows was unable to complete the format.", ok, &out) { reload(&s2); } });
}

/// Volume Properties: General (pie chart, Disk Cleanup), Tools, Hardware.
fn vol_properties(s: &Rc<State>, v: &Vol, kind: Kind) {
    let title = vol_title(v);
    let d = gtk::Dialog::with_buttons(Some(&format!("{} Properties", if title.is_empty() { v.name.clone() } else { title.clone() })), Some(&s.m.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel), ("_Apply", gtk::ResponseType::Apply)]);
    d.set_default_size(370, 470);
    let nb = gtk::Notebook::new();
    let g = vbox(8);
    g.set_border_width(12);
    let h = hbox(12);
    h.pack_start(&img(&["drive-harddisk"], 32), false, false, 0);
    let name = gtk::Entry::new();
    name.set_text(&v.label);
    name.set_sensitive(false);
    h.pack_start(&name, true, true, 0);
    g.pack_start(&h, false, false, 0);
    g.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
    let fsv = v.inner.iter().find(|i| i.fssize.is_some()).cloned().unwrap_or_else(|| v.clone());
    let cap = fsv.fssize.unwrap_or(v.size);
    let free = fsv.avail.unwrap_or(0);
    let used = cap.saturating_sub(free);
    let known = fsv.avail.is_some();
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(14);
    let rows: Vec<(&str, String, String)> = vec![
        ("Type:", if v.inner.is_empty() { "Local Disk".into() } else { "Local Disk (encrypted)".into() }, String::new()),
        ("File system:", fs_name(&fsv), String::new()),
        ("Used space:", if known { format!("{} bytes", mmc::thousands(used as usize)) } else { "—".into() }, if known { gb(used) } else { String::new() }),
        ("Free space:", if known { format!("{} bytes", mmc::thousands(free as usize)) } else { "—".into() }, if known { gb(free) } else { String::new() }),
        ("Capacity:", format!("{} bytes", mmc::thousands(cap as usize)), gb(cap)),
    ];
    for (i, (k, a, b)) in rows.iter().enumerate() {
        if i == 2 || i == 3 {
            let sw = gtk::DrawingArea::new();
            sw.set_size_request(14, 14);
            let c = if i == 2 { (0.15, 0.35, 0.85) } else { (0.85, 0.2, 0.85) };
            sw.connect_draw(move |_, cr| { cr.set_source_rgb(c.0, c.1, c.2); cr.rectangle(0.0, 0.0, 14.0, 14.0); let _ = cr.fill(); glib::Propagation::Stop });
            grid.attach(&sw, 0, i as i32, 1, 1);
        }
        let l = gtk::Label::new(Some(k)); l.set_xalign(0.0); grid.attach(&l, 1, i as i32, 1, 1);
        let l = gtk::Label::new(Some(a)); l.set_xalign(1.0); grid.attach(&l, 2, i as i32, 1, 1);
        let l = gtk::Label::new(Some(b)); l.set_xalign(1.0); grid.attach(&l, 3, i as i32, 1, 1);
    }
    g.pack_start(&grid, false, false, 0);
    // the pie
    let pie = gtk::DrawingArea::new();
    pie.set_size_request(-1, 110);
    let frac = if cap > 0 && known { used as f64 / cap as f64 } else { 0.0 };
    pie.connect_draw(move |w, cr| {
        let cx = w.allocated_width() as f64 / 2.0;
        let (rx, ry, depth) = (60.0, 32.0, 14.0);
        let cy = 40.0;
        cr.save().ok();
        cr.translate(cx, cy);
        cr.scale(1.0, ry / rx);
        // rim
        for i in (0..depth as i32).rev() {
            cr.save().ok();
            cr.translate(0.0, i as f64 * rx / ry);
            cr.move_to(0.0, 0.0);
            cr.arc(0.0, 0.0, rx, 0.0, std::f64::consts::TAU);
            cr.set_source_rgb(0.6, 0.12, 0.6);
            let _ = cr.fill();
            cr.move_to(0.0, 0.0);
            cr.arc(0.0, 0.0, rx, -std::f64::consts::FRAC_PI_2, -std::f64::consts::FRAC_PI_2 + frac * std::f64::consts::TAU);
            cr.set_source_rgb(0.1, 0.22, 0.6);
            let _ = cr.fill();
            cr.restore().ok();
        }
        cr.move_to(0.0, 0.0);
        cr.arc(0.0, 0.0, rx, 0.0, std::f64::consts::TAU);
        cr.set_source_rgb(0.85, 0.2, 0.85);
        let _ = cr.fill();
        cr.move_to(0.0, 0.0);
        cr.arc(0.0, 0.0, rx, -std::f64::consts::FRAC_PI_2, -std::f64::consts::FRAC_PI_2 + frac * std::f64::consts::TAU);
        cr.set_source_rgb(0.15, 0.35, 0.85);
        let _ = cr.fill();
        cr.restore().ok();
        glib::Propagation::Stop
    });
    g.pack_start(&pie, false, false, 0);
    let hb = hbox(10);
    let mp = mounts(v).first().cloned().unwrap_or_else(|| v.path.clone());
    hb.pack_start(&gtk::Label::new(Some(&format!("Drive {mp}"))), true, true, 0);
    let dc = gtk::Button::with_mnemonic("Disk Clean_up");
    dc.connect_clicked(|_| spawn_tool("cleanmgr"));
    hb.pack_end(&dc, false, false, 0);
    g.pack_start(&hb, false, false, 0);
    for t in ["Compress this drive to save disk space", "Allow files on this drive to have contents indexed in addition to file properties"] { let c = gtk::CheckButton::with_label(t); c.set_sensitive(false); g.pack_start(&c, false, false, 0); }
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    // Tools
    let t = vbox(10);
    t.set_border_width(12);
    let tool = |title: &str, text: &str, btn: &str| { let f = gtk::Frame::new(Some(title)); let b = hbox(10); b.set_border_width(8); b.pack_start(&label(text, ""), true, true, 0); let bt = shield_button(btn); bt.set_valign(gtk::Align::End); b.pack_start(&bt, false, false, 0); f.add(&b); t.pack_start(&f, false, false, 0); bt };
    let chk = tool("Error-checking", "This option will check the drive for errors.", "_Check now...");
    let defrag = tool("Defragmentation", "This option will defragment files on the drive.", "_Defragment now...");
    let back = tool("Backup", "This option will back up files on the drive.", "_Back up now...");
    back.set_image(None::<&gtk::Widget>);
    back.connect_clicked(|_| spawn_tool("backup"));
    let (p, fst, mounted, dw) = (fsv.path.clone(), fsv.fstype.clone(), !mounts(v).is_empty(), d.clone());
    chk.connect_clicked(move |_| {
        if mounted { inform(Some(dw.upcast_ref()), "Checking Disk", "Windows can't check the disk while it's in use", "Unmount the volume first (Change Drive Letter and Paths ▸ Remove), or check it at the next start with: fsck at boot (add fsck.mode=force to the kernel command line)."); return; }
        let dw2 = dw.clone();
        let argv: Vec<&str> = match fst.as_str() { "btrfs" => vec!["btrfs", "check", "--readonly", &p], "xfs" => vec!["xfs_repair", "-n", &p], "ntfs" => vec!["ntfsfix", "-n", &p], _ => vec!["fsck", "-n", &p] };
        elevate(&argv, move |ok, out| inform(Some(dw2.upcast_ref()), "Checking Disk", if ok { "Your device or disk was successfully scanned. Windows found no problems." } else { "Windows found problems on the disk." }, out.trim()));
    });
    let (fst, mp2, dw) = (fsv.fstype.clone(), mounts(v).first().cloned().unwrap_or_default(), d.clone());
    defrag.set_sensitive(fst == "ext4" && !mp2.is_empty());
    defrag.connect_clicked(move |_| { let dw2 = dw.clone(); elevate(&["e4defrag", "-c", &mp2], move |_, out| inform(Some(dw2.upcast_ref()), "Disk Defragmenter", "Analysis complete.", out.trim())); });
    nb.append_page(&t, Some(&gtk::Label::new(Some("Tools"))));
    // Hardware
    let hw = vbox(8);
    hw.set_border_width(12);
    hw.pack_start(&label("All disk drives:", ""), false, false, 0);
    let hs = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
    for dk in s.disks.borrow().iter() { hs.insert_with_values(None, &[(0, &pix(&["drive-harddisk"], 16)), (1, &format!("{} {}", if dk.model.is_empty() { &dk.name } else { &dk.model }, if dk.rom { "CD-ROM" } else { "Disk Device" })), (2, &if dk.rom { "DVD/CD-ROM drives" } else { "Disk drives" })]); }
    let htv = gtk::TreeView::with_model(&hs);
    icon_column(&htv, "Name", 0, 1, 200);
    columns(&htv, &[("Type", 2, 0)]);
    hw.pack_start(&scrolled(&htv), true, true, 0);
    hw.pack_start(&kv(&[("Partition:", v.path.clone()), ("UUID:", v.uuid.clone()), ("Partition type:", v.parttypename.clone()), ("Layout:", format!("{:?}", kind))]), false, false, 0);
    nb.append_page(&hw, Some(&gtk::Label::new(Some("Hardware"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.connect_response(|d, r| if r != gtk::ResponseType::Apply { d.close() });
    d.show_all();
}

fn kv(rows: &[(&str, String)]) -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(4);
    g.set_column_spacing(12);
    for (i, (k, v)) in rows.iter().enumerate() {
        let a = gtk::Label::new(Some(k)); a.set_xalign(0.0); g.attach(&a, 0, i as i32, 1, 1);
        let b = gtk::Label::new(Some(v)); b.set_xalign(0.0); b.set_selectable(true); b.set_ellipsize(gtk::pango::EllipsizeMode::End); g.attach(&b, 1, i as i32, 1, 1);
    }
    g
}

fn disk_properties(s: &Rc<State>, dk: &Disk) {
    let d = gtk::Dialog::with_buttons(Some(&format!("{} Properties", if dk.model.is_empty() { &dk.name } else { &dk.model })), Some(&s.m.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(400, 440);
    let nb = gtk::Notebook::new();
    let g = vbox(10);
    g.set_border_width(12);
    let h = hbox(12);
    h.pack_start(&img(&["drive-harddisk"], 32), false, false, 0);
    h.pack_start(&label(&format!("{} {}", dk.model, if dk.rom { "CD-ROM Drive" } else { "Disk Device" }), ""), true, true, 0);
    g.pack_start(&h, false, false, 0);
    g.pack_start(&kv(&[("Device type:", if dk.rom { "DVD/CD-ROM drives".into() } else { "Disk drives".into() }), ("Manufacturer:", "(Standard disk drives)".into()), ("Location:", format!("{} ({})", dk.path, dk.tran.to_uppercase()))]), false, false, 0);
    let fr = gtk::Frame::new(Some("Device status"));
    let st = text_box("This device is working properly.");
    st.set_border_width(6);
    fr.add(&st);
    g.pack_start(&fr, false, false, 0);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    let vtab = vbox(10);
    vtab.set_border_width(12);
    vtab.pack_start(&label("The volumes contained on this disk are listed below.", ""), false, false, 0);
    let regs = regions(dk);
    let unalloc: u64 = regs.iter().filter(|r| r.kind == Kind::Unallocated).map(|r| r.size).sum();
    vtab.pack_start(&kv(&[("Disk:", dk.name.clone()), ("Type:", if dk.removable { "Removable".into() } else { "Basic".into() }), ("Status:", "Online".into()),
        ("Partition style:", match dk.pttype.as_str() { "gpt" => "GUID Partition Table (GPT)".into(), "dos" => "Master Boot Record (MBR)".into(), "" => "Not Applicable".into(), x => x.to_string() }),
        ("Capacity:", gb(dk.size)), ("Unallocated space:", gb(unalloc)), ("Reserved space:", "1 MB".into())]), false, false, 0);
    let vs = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
    for p in &dk.parts { let t = vol_title(p); vs.insert_with_values(None, &[(0, &pix(&["drive-harddisk"], 16)), (1, &if t.is_empty() { p.name.clone() } else { t }), (2, &gb(p.size))]); }
    let vtv = gtk::TreeView::with_model(&vs);
    icon_column(&vtv, "Volume", 0, 1, 220);
    columns(&vtv, &[("Capacity", 2, 0)]);
    vtab.pack_start(&scrolled(&vtv), true, true, 0);
    nb.append_page(&vtab, Some(&gtk::Label::new(Some("Volumes"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &str = r#"{"blockdevices":[
      {"name":"nvme0n1","path":"/dev/nvme0n1","type":"disk","size":512110190592,"start":null,"pttype":"gpt","model":"Samsung SSD 970 EVO Plus 500GB","tran":"nvme","rm":false,"mountpoints":[null],
       "children":[
        {"name":"nvme0n1p1","path":"/dev/nvme0n1p1","type":"part","size":314572800,"start":4096,"fstype":"vfat","label":"EFI","parttype":"c12a7328-f81f-11d2-ba4b-00a0c93ec93b","mountpoints":["/boot/efi"],"fsavail":300000000,"fssize":314000000,"partn":1},
        {"name":"nvme0n1p2","path":"/dev/nvme0n1p2","type":"part","size":400000000000,"start":618496,"fstype":"btrfs","label":"endeavouros","parttype":"0fc63daf-8483-4772-8e79-3d69d8477de4","mountpoints":["/","/home"],"fsavail":200000000000,"fssize":400000000000,"partn":2}
       ]},
      {"name":"sda","path":"/dev/sda","type":"disk","size":1000204886016,"pttype":"dos","model":"ST1000DM003","tran":"sata","rm":false,"mountpoints":[null],
       "children":[
        {"name":"sda1","type":"part","size":104857600,"start":2048,"fstype":"ntfs","label":"System Reserved","parttype":"0x7","mountpoints":[null],"partn":1},
        {"name":"sda2","type":"part","size":500000000000,"start":206848,"parttype":"0x5","mountpoints":[null],"partn":2},
        {"name":"sda5","type":"part","size":200000000000,"start":208896,"fstype":"ext4","label":"Data","parttype":"0x83","mountpoints":["/mnt/data"],"fsavail":100000000000,"fssize":200000000000,"partn":5}
       ]},
      {"name":"sr0","type":"rom","size":0,"model":"DVD RW DRIVE","rm":true,"mountpoints":[null]},
      {"name":"zram0","type":"disk","size":8000000000,"mountpoints":["[SWAP]"]}
    ]}"#;
    #[test]
    fn parse_and_layout() {
        let d = parse_lsblk(SAMPLE);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].parts.len(), 2);
        let r = regions(&d[0]);
        assert_eq!(r.iter().filter(|x| x.kind == Kind::Primary).count(), 2);
        assert_eq!(r.last().unwrap().kind, Kind::Unallocated);
        assert_eq!(vol_title(&d[0].parts[1]), "endeavouros (/)");
        assert_eq!(status(&d[0].parts[0], Kind::Primary), "Healthy (EFI System Partition, System, Primary Partition)");
        let r = regions(&d[1]);
        let ext = r.iter().find(|x| x.kind == Kind::Extended).unwrap();
        assert_eq!(ext.inner.iter().filter(|x| x.kind == Kind::Logical).count(), 1);
        assert!(ext.inner.iter().any(|x| x.kind == Kind::Free));
        assert!(r.iter().any(|x| x.kind == Kind::Unallocated));
        let l = layout(&[100, 1, 1], 300.0, 60.0);
        assert!(l.iter().all(|(_, w)| *w >= 59.9));
        assert!((l.iter().map(|x| x.1).sum::<f64>() - 300.0).abs() < 0.01);
    }
}
