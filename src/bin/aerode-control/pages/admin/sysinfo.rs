//! System Information (msinfo32): System Summary, Hardware Resources, Components and Software
//! Environment, each a table filled on a worker thread when it is first opened. File ▸ Export
//! writes every category to a text file; the Find bar searches the current or all categories.
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use super::*;

#[derive(Clone, Default)]
pub struct Table { pub cols: Vec<&'static str>, pub rows: Vec<Vec<String>> }

/// (id, title, parent id)
pub const TREE: &[(&str, &str, &str)] = &[
    ("summary", "System Summary", ""),
    ("hw", "Hardware Resources", "summary"),
    ("hw.conflicts", "Conflicts/Sharing", "hw"), ("hw.dma", "DMA", "hw"), ("hw.forced", "Forced Hardware", "hw"), ("hw.io", "I/O", "hw"), ("hw.irq", "IRQs", "hw"), ("hw.mem", "Memory", "hw"),
    ("comp", "Components", "summary"),
    ("comp.mm", "Multimedia", "comp"), ("comp.mm.audio", "Audio Codecs", "comp.mm"), ("comp.mm.video", "Video Codecs", "comp.mm"),
    ("comp.cdrom", "CD-ROM", "comp"), ("comp.sound", "Sound Device", "comp"), ("comp.display", "Display", "comp"), ("comp.ir", "Infrared", "comp"),
    ("comp.input", "Input", "comp"), ("comp.input.kbd", "Keyboard", "comp.input"), ("comp.input.mouse", "Pointing Device", "comp.input"),
    ("comp.modem", "Modem", "comp"),
    ("comp.net", "Network", "comp"), ("comp.net.adapter", "Adapter", "comp.net"), ("comp.net.proto", "Protocol", "comp.net"), ("comp.net.winsock", "WinSock", "comp.net"),
    ("comp.ports", "Ports", "comp"), ("comp.ports.serial", "Serial", "comp.ports"), ("comp.ports.parallel", "Parallel", "comp.ports"),
    ("comp.storage", "Storage", "comp"), ("comp.storage.drives", "Drives", "comp.storage"), ("comp.storage.disks", "Disks", "comp.storage"), ("comp.storage.scsi", "SCSI", "comp.storage"), ("comp.storage.ide", "IDE", "comp.storage"),
    ("comp.printing", "Printing", "comp"), ("comp.problem", "Problem Devices", "comp"), ("comp.usb", "USB", "comp"),
    ("sw", "Software Environment", "summary"),
    ("sw.drivers", "System Drivers", "sw"), ("sw.env", "Environment Variables", "sw"), ("sw.print", "Print Jobs", "sw"), ("sw.netconn", "Network Connections", "sw"),
    ("sw.tasks", "Running Tasks", "sw"), ("sw.modules", "Loaded Modules", "sw"), ("sw.services", "Services", "sw"), ("sw.groups", "Program Groups", "sw"),
    ("sw.startup", "Startup Programs", "sw"), ("sw.ole", "OLE Registration", "sw"), ("sw.wer", "Windows Error Reporting", "sw"),
];

fn rd(p: &str) -> String { std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default() }
fn kv(rows: Vec<(&str, String)>) -> Table { Table { cols: vec!["Item", "Value"], rows: rows.into_iter().map(|(k, v)| vec![k.to_string(), v]).collect() } }
fn na() -> String { "Not Available".into() }
fn meminfo() -> HashMap<String, u64> {
    std::fs::read_to_string("/proc/meminfo").unwrap_or_default().lines().filter_map(|l| { let (k, v) = l.split_once(':')?; Some((k.to_string(), v.split_whitespace().next()?.parse::<u64>().ok()? * 1024)) }).collect()
}
/// msinfo style: "15.9 GB", "512 MB"
fn mb(n: u64) -> String { if n >= 1024 * 1024 * 1024 { format!("{:.1} GB", n as f64 / 1073741824.0) } else { format!("{} MB", n / 1048576) } }

fn summary() -> Table {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let get = |k: &str| os.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
    let cpu = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpuget = |k: &str| cpu.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim_start_matches([' ', '\t', ':']).to_string())).unwrap_or_default();
    let cores: std::collections::HashSet<(String, String)> = cpu.split("\n\n").filter_map(|b| {
        let g = |k: &str| b.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()));
        Some((g("physical id").unwrap_or_default(), g("core id")?))
    }).collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let mhz = rd("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq").parse::<u64>().map(|k| k / 1000).ok().or_else(|| cpuget("cpu MHz").parse::<f64>().ok().map(|m| m as u64)).unwrap_or(0);
    let m = meminfo();
    let g = |k: &str| m.get(k).copied().unwrap_or(0);
    let installed: u64 = {
        // physical size from the memory blocks when available (what the RAM sticks add up to)
        let bs = u64::from_str_radix(rd("/sys/devices/system/memory/block_size_bytes").trim(), 16).unwrap_or(0);
        let blocks = std::fs::read_dir("/sys/devices/system/memory").map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("memory")).count() as u64).unwrap_or(0);
        if bs > 0 && blocks > 0 { bs * blocks } else { g("MemTotal") }
    };
    let swaps: Vec<String> = std::fs::read_to_string("/proc/swaps").unwrap_or_default().lines().skip(1).filter_map(|l| l.split_whitespace().next().map(|s| s.to_string())).collect();
    let root_dev = std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines().find_map(|l| { let f: Vec<&str> = l.split_whitespace().collect(); (f.len() > 1 && f[1] == "/").then(|| f[0].to_string()) }).unwrap_or_default();
    let tz = std::fs::read_link("/etc/localtime").ok().map(|p| p.to_string_lossy().split("zoneinfo/").nth(1).unwrap_or("").to_string()).unwrap_or_else(|| rd("/etc/timezone"));
    let kernel = sh(&["uname", "-r"]).trim().to_string();
    let bios_date = rd("/sys/class/dmi/id/bios_date");
    let secure = std::fs::read_dir("/sys/firmware/efi/efivars").ok().and_then(|d| d.flatten().find(|e| e.file_name().to_string_lossy().starts_with("SecureBoot-"))).and_then(|e| std::fs::read(e.path()).ok()).map(|b| b.last().copied() == Some(1));
    let user = glib::user_name().to_string_lossy().into_owned();
    kv(vec![
        ("OS Name", get("PRETTY_NAME")),
        ("Version", { let b = get("BUILD_ID"); if b.is_empty() || b == "rolling" { format!("{kernel} (rolling release)") } else { format!("{kernel} Build {b}") } }),
        ("Other OS Description", "Aerode desktop".into()),
        ("OS Manufacturer", { let h = get("HOME_URL"); if h.is_empty() { get("NAME") } else { format!("{} ({h})", get("NAME")) } }),
        ("System Name", glib::host_name().to_string().to_uppercase()),
        ("System Manufacturer", { let v = rd("/sys/class/dmi/id/sys_vendor"); if v.is_empty() { na() } else { v } }),
        ("System Model", { let v = rd("/sys/class/dmi/id/product_name"); if v.is_empty() { na() } else { v } }),
        ("System Type", match std::env::consts::ARCH { "x86_64" => "x64-based PC".into(), "x86" => "X86-based PC".into(), a => format!("{a}-based PC") }),
        ("Processor", format!("{}, {} Mhz, {} Core(s), {} Logical Processor(s)", cpuget("model name"), mhz, cores.len().max(1), threads)),
        ("BIOS Version/Date", { let v = format!("{} {}, {}", rd("/sys/class/dmi/id/bios_vendor"), rd("/sys/class/dmi/id/bios_version"), bios_date); if v.trim() == ",".trim() || v.trim().len() < 3 { na() } else { v } }),
        ("BIOS Mode", if Path::new("/sys/firmware/efi").exists() { "UEFI".into() } else { "Legacy".into() }),
        ("Secure Boot State", match secure { Some(true) => "On".into(), Some(false) => "Off".into(), None => "Unsupported".into() }),
        ("BaseBoard Manufacturer", { let v = rd("/sys/class/dmi/id/board_vendor"); if v.is_empty() { na() } else { v } }),
        ("BaseBoard Model", { let v = rd("/sys/class/dmi/id/board_name"); if v.is_empty() { na() } else { v } }),
        ("Windows Directory", "/usr".into()),
        ("System Directory", "/usr/bin".into()),
        ("Boot Device", root_dev),
        ("Locale", std::env::var("LANG").unwrap_or_else(|_| "C".into())),
        ("Hardware Abstraction Layer", format!("Linux {}", sh(&["uname", "-v"]).trim())),
        ("User Name", format!("{}\\{user}", glib::host_name().to_string().to_uppercase())),
        ("Time Zone", tz),
        ("Installed Physical Memory (RAM)", mb(installed)),
        ("Total Physical Memory", mb(g("MemTotal"))),
        ("Available Physical Memory", mb(g("MemAvailable"))),
        ("Total Virtual Memory", mb(g("MemTotal") + g("SwapTotal"))),
        ("Available Virtual Memory", mb(g("MemAvailable") + g("SwapFree"))),
        ("Page File Space", mb(g("SwapTotal"))),
        ("Page File", if swaps.is_empty() { "None".into() } else { swaps.join(", ") }),
    ])
}

fn table(cols: Vec<&'static str>, rows: Vec<Vec<String>>) -> Table { Table { cols, rows } }

fn irq_names() -> Vec<(String, String)> {
    let mut v = vec![];
    for l in std::fs::read_to_string("/proc/interrupts").unwrap_or_default().lines().skip(1) {
        let (num, rest) = match l.split_once(':') { Some(x) => x, None => continue };
        let num = num.trim();
        if num.parse::<u32>().is_err() { continue; }
        let name = rest.split_whitespace().filter(|t| t.parse::<u64>().is_err()).skip(2).collect::<Vec<_>>().join(" ");
        v.push((num.to_string(), if name.is_empty() { "(unused)".into() } else { name }));
    }
    v
}

fn ranges(path: &str) -> Vec<Vec<String>> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| {
        let (r, n) = l.split_once(" : ")?;
        let (a, b) = r.trim().split_once('-')?;
        Some(vec![format!("0x{} - 0x{}", a.to_uppercase(), b.to_uppercase()), n.trim().to_string(), "OK".into()])
    }).collect()
}

fn input(kind: &str) -> Table {
    let mut rows = vec![];
    for block in std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default().split("\n\n") {
        let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k)).map(|s| s.trim().to_string()).unwrap_or_default();
        let h = get("H: Handlers=");
        let name = get("N: Name=").trim_matches('"').to_string();
        let ok = if kind == "kbd" { h.contains("kbd") && (name.to_lowercase().contains("keyboard") || get("P: Phys=").starts_with("isa0060")) } else { h.contains("mouse") };
        if !ok || name.is_empty() { continue; }
        if !rows.is_empty() { rows.push(vec![String::new(), String::new()]); }
        rows.push(vec!["Name".into(), name]);
        rows.push(vec!["Description".into(), if kind == "kbd" { "Keyboard".into() } else { "Pointing device".into() }]);
        rows.push(vec!["PNP Device ID".into(), format!("INPUT\\{}", get("I: ").replace(' ', "&"))]);
        rows.push(vec!["Location".into(), get("P: Phys=")]);
        rows.push(vec!["Driver".into(), get("S: Sysfs=")]);
        rows.push(vec!["Handlers".into(), h]);
    }
    if rows.is_empty() { rows.push(vec!["Name".into(), "Not Available".into()]); }
    Table { cols: vec!["Item", "Value"], rows }
}

fn pci_by_class(prefix: &[&str]) -> Vec<(String, String, String, String)> {
    // (slot, name, driver, ids)
    let mut v = vec![];
    let lspci = sh(&["lspci", "-vmm", "-k", "-D", "-nn"]);
    if !lspci.trim().is_empty() {
        for b in lspci.split("\n\n") {
            let g = |k: &str| b.lines().find_map(|l| l.strip_prefix(k)).map(|s| s.trim().to_string()).unwrap_or_default();
            let class = g("Class:");
            if !prefix.iter().any(|p| class.contains(&format!("[{p}"))) { continue; }
            v.push((g("Slot:"), format!("{} {}", g("Vendor:"), g("Device:")), g("Driver:"), g("Device:")));
        }
        return v;
    }
    if let Ok(it) = std::fs::read_dir("/sys/bus/pci/devices") {
        for e in it.flatten() {
            let p = e.path();
            let class = rd(&p.join("class").to_string_lossy());
            if !prefix.iter().any(|x| class.trim_start_matches("0x").starts_with(x)) { continue; }
            let drv = std::fs::read_link(p.join("driver")).ok().and_then(|l| l.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
            v.push((e.file_name().to_string_lossy().into_owned(), format!("PCI device {}:{}", rd(&p.join("vendor").to_string_lossy()), rd(&p.join("device").to_string_lossy())), drv, String::new()));
        }
    }
    v
}

fn disks() -> serde_json::Value {
    sh(&["lsblk", "-J", "-b", "-o", "NAME,PATH,TYPE,SIZE,FSTYPE,LABEL,UUID,MOUNTPOINTS,FSAVAIL,FSSIZE,MODEL,TRAN,RM,VENDOR,SERIAL,LOG-SEC,PHY-SEC,PTTYPE"]).parse::<serde_json::Value>().unwrap_or_default()
}

/// One category's table (worker thread).
pub fn load(id: &str) -> Table {
    match id {
        "summary" => summary(),
        "hw" | "comp" | "sw" | "comp.mm" | "comp.input" | "comp.net" | "comp.ports" | "comp.storage" => Table { cols: vec!["Item", "Value"], rows: vec![] },
        "hw.conflicts" => {
            let rows: Vec<Vec<String>> = irq_names().into_iter().filter(|(_, n)| n.contains(',')).map(|(i, n)| vec![format!("IRQ {i}"), n.replace(", ", "\n")]).collect();
            table(vec!["Resource", "Device"], rows)
        }
        "hw.dma" => table(vec!["Resource", "Device", "Status"], std::fs::read_to_string("/proc/dma").unwrap_or_default().lines().filter_map(|l| { let (a, b) = l.split_once(':')?; Some(vec![format!("Channel {}", a.trim()), b.trim().to_string(), "OK".into()]) }).collect()),
        "hw.forced" => table(vec!["Device", "PNP Device ID"], vec![]),
        "hw.io" => table(vec!["Resource", "Device", "Status"], ranges("/proc/ioports")),
        "hw.irq" => table(vec!["Resource", "Device", "Status"], irq_names().into_iter().map(|(i, n)| vec![format!("IRQ {i}"), n, "OK".into()]).collect()),
        "hw.mem" => table(vec!["Resource", "Device", "Status"], ranges("/proc/iomem")),
        "comp.mm.audio" => {
            let mut rows = vec![];
            if let Ok(it) = std::fs::read_dir("/proc/asound") {
                for e in it.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("card")) {
                    if let Ok(c) = std::fs::read_dir(e.path()) { for f in c.flatten().filter(|f| f.file_name().to_string_lossy().starts_with("codec#")) {
                        let t = std::fs::read_to_string(f.path()).unwrap_or_default();
                        let name = t.lines().find_map(|l| l.strip_prefix("Codec: ")).unwrap_or("").to_string();
                        rows.push(vec![name, f.path().to_string_lossy().into_owned(), "OK".into()]);
                    } }
                }
            }
            for m in std::fs::read_to_string("/proc/modules").unwrap_or_default().lines().filter_map(|l| l.split_whitespace().next()).filter(|m| m.starts_with("snd_hda_codec")) { rows.push(vec![m.to_string(), format!("kernel module {m}"), "OK".into()]); }
            table(vec!["Codec", "Manufacturer / File", "Status"], rows)
        }
        "comp.mm.video" => {
            let rows: Vec<Vec<String>> = sh(&["ffmpeg", "-hide_banner", "-decoders"]).lines().filter(|l| l.starts_with(" V")).take(400).filter_map(|l| { let f: Vec<&str> = l.split_whitespace().collect(); (f.len() >= 3).then(|| vec![f[1].to_string(), f[2..].join(" "), "OK".into()]) }).collect();
            table(vec!["Codec", "Description", "Status"], rows)
        }
        "comp.cdrom" => {
            let v = disks();
            let mut rows = vec![];
            for d in v["blockdevices"].as_array().into_iter().flatten().filter(|d| d["type"] == "rom") {
                let s = |k: &str| d[k].as_str().unwrap_or("").to_string();
                rows.extend([vec!["Drive".into(), s("path")], vec!["Description".into(), "CD-ROM Drive".into()], vec!["Media Loaded".into(), if d["size"].as_u64().unwrap_or(0) > 0 { "Yes".into() } else { "No".into() }], vec!["Name".into(), s("model")], vec!["Manufacturer".into(), s("vendor")], vec![String::new(), String::new()]]);
            }
            Table { cols: vec!["Item", "Value"], rows }
        }
        "comp.sound" => {
            let rows: Vec<Vec<String>> = std::fs::read_to_string("/proc/asound/cards").unwrap_or_default().lines().filter(|l| l.trim_start().chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
                .map(|l| { let name = l.split(" - ").nth(1).unwrap_or(l).trim().to_string(); let id = l.split('[').nth(1).and_then(|x| x.split(']').next()).unwrap_or("").trim().to_string(); vec![name, format!("ALSA card {id}"), "OK".into()] }).collect();
            table(vec!["Name", "PNP Device ID", "Status"], rows)
        }
        "comp.display" => {
            let mut rows = vec![];
            let modes: Vec<String> = sh(&["xrandr", "--current"]).lines().filter(|l| l.contains(" connected")).map(|l| l.to_string()).collect();
            let glx = sh(&["glxinfo", "-B"]);
            let g = |k: &str| glx.lines().find_map(|l| l.trim().strip_prefix(k)).map(|s| s.trim().to_string()).unwrap_or_default();
            for (slot, name, drv, _) in pci_by_class(&["03"]) {
                rows.extend([vec!["Name".into(), name.clone()], vec!["PNP Device ID".into(), format!("PCI\\{slot}")], vec!["Adapter Type".into(), g("OpenGL renderer string:")], vec!["Adapter Description".into(), name],
                    vec!["Adapter RAM".into(), { let v = g("Video memory:"); if v.is_empty() { g("Dedicated video memory:") } else { v } }], vec!["Driver".into(), drv], vec!["Driver Version".into(), g("OpenGL version string:")], vec![String::new(), String::new()]]);
            }
            for m in modes {
                let f: Vec<&str> = m.split_whitespace().collect();
                let geo = f.iter().find(|w| w.contains('x') && w.contains('+')).copied().unwrap_or("");
                rows.extend([vec!["Output".into(), f.first().copied().unwrap_or("").into()], vec!["Resolution".into(), geo.split('+').next().unwrap_or("").replace('x', " x ")], vec!["Bits/Pixel".into(), "24".into()], vec![String::new(), String::new()]]);
            }
            Table { cols: vec!["Item", "Value"], rows }
        }
        "comp.ir" | "comp.modem" | "comp.net.winsock" | "comp.ports.parallel" => Table { cols: vec!["Item", "Value"], rows: vec![] },
        "comp.input.kbd" => input("kbd"),
        "comp.input.mouse" => input("mouse"),
        "comp.net.adapter" => {
            let mut rows = vec![];
            let addrs = sh(&["ip", "-o", "addr"]);
            let mut ifs: Vec<String> = std::fs::read_dir("/sys/class/net").map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
            ifs.sort();
            for n in ifs {
                let base = format!("/sys/class/net/{n}");
                let ips: Vec<String> = addrs.lines().filter(|l| l.split_whitespace().nth(1) == Some(n.as_str())).filter_map(|l| { let f: Vec<&str> = l.split_whitespace().collect(); f.iter().position(|x| *x == "inet" || *x == "inet6").map(|i| f[i + 1].to_string()) }).collect();
                let drv = std::fs::read_link(format!("{base}/device/driver")).ok().and_then(|l| l.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_else(|| "virtual".into());
                rows.extend([vec!["Name".into(), n.clone()], vec!["Adapter Type".into(), match rd(&format!("{base}/type")).as_str() { "1" => "Ethernet 802.3".into(), "772" => "Loopback".into(), "801" | "803" => "Wireless".into(), t => format!("type {t}") }],
                    vec!["Product Type".into(), if Path::new(&format!("{base}/wireless")).exists() { "Wireless LAN".into() } else { drv.clone() }], vec!["Installed".into(), "Yes".into()], vec!["Last Reset".into(), String::new()],
                    vec!["Index".into(), rd(&format!("{base}/ifindex"))], vec!["Service Name".into(), drv], vec!["IP Address".into(), ips.join(", ")], vec!["MAC Address".into(), rd(&format!("{base}/address")).to_uppercase().replace(':', ":")],
                    vec!["Operational State".into(), rd(&format!("{base}/operstate"))], vec!["Speed".into(), { let s = rd(&format!("{base}/speed")); if s.is_empty() || s.starts_with('-') { String::new() } else { format!("{s} Mbit/s") } }], vec![String::new(), String::new()]]);
            }
            Table { cols: vec!["Item", "Value"], rows }
        }
        "comp.net.proto" => {
            let rows: Vec<Vec<String>> = std::fs::read_to_string("/proc/net/protocols").unwrap_or_default().lines().skip(1).filter_map(|l| { let f: Vec<&str> = l.split_whitespace().collect(); (f.len() > 2).then(|| vec![f[0].to_string(), format!("{} sockets", f[2])]) }).collect();
            table(vec!["Name", "Sockets in use"], rows)
        }
        "comp.ports.serial" => {
            let mut rows = vec![];
            if let Ok(it) = std::fs::read_dir("/sys/class/tty") { for e in it.flatten() { let n = e.file_name().to_string_lossy().into_owned(); if (n.starts_with("ttyS") && rd(&e.path().join("type").to_string_lossy()) != "0") || n.starts_with("ttyUSB") || n.starts_with("ttyACM") { rows.push(vec!["Name".into(), format!("/dev/{n}")]); } } }
            Table { cols: vec!["Item", "Value"], rows }
        }
        "comp.storage.drives" => {
            let mut rows = vec![];
            for l in std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines() {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() < 3 || !f[0].starts_with("/dev/") { continue; }
                let Some((free, total)) = fs_space(f[1]) else { continue };
                let label = sh(&["lsblk", "-no", "LABEL,UUID", f[0]]);
                let mut lu = label.split_whitespace();
                rows.extend([vec!["Drive".into(), f[1].to_string()], vec!["Description".into(), "Local Fixed Disk".into()], vec!["Compressed".into(), if l.contains("compress") { "Yes".into() } else { "No".into() }], vec!["File System".into(), f[2].to_uppercase()],
                    vec!["Size".into(), format!("{} ({} bytes)", gb(total), mmc::thousands(total as usize))], vec!["Free Space".into(), format!("{} ({} bytes)", gb(free), mmc::thousands(free as usize))], vec!["Volume Name".into(), lu.next().unwrap_or("").to_string()], vec!["Volume Serial Number".into(), lu.next().unwrap_or("").to_string()], vec![String::new(), String::new()]]);
            }
            Table { cols: vec!["Item", "Value"], rows }
        }
        "comp.storage.disks" => {
            let v = disks();
            let mut rows = vec![];
            for d in v["blockdevices"].as_array().into_iter().flatten().filter(|d| d["type"] == "disk") {
                let s = |k: &str| d[k].as_str().map(|x| x.trim().to_string()).unwrap_or_else(|| d[k].as_u64().map(|n| n.to_string()).unwrap_or_default());
                if s("name").starts_with("zram") || s("name").starts_with("loop") { continue; }
                let size = d["size"].as_u64().unwrap_or(0);
                rows.extend([vec!["Description".into(), if d["rm"].as_bool() == Some(true) { "Removable Media".into() } else { "Disk drive".into() }], vec!["Manufacturer".into(), { let v = s("vendor"); if v.is_empty() { "(Standard disk drives)".into() } else { v } }],
                    vec!["Model".into(), s("model")], vec!["Bytes/Sector".into(), s("log-sec")], vec!["Media Loaded".into(), "Yes".into()], vec!["Media Type".into(), if d["rm"].as_bool() == Some(true) { "Removable Media".into() } else { "Fixed hard disk".into() }],
                    vec!["Partitions".into(), d["children"].as_array().map(|c| c.len()).unwrap_or(0).to_string()], vec!["Partition style".into(), s("pttype").to_uppercase()], vec!["Interface".into(), s("tran").to_uppercase()], vec!["Serial Number".into(), s("serial")],
                    vec!["Size".into(), format!("{} ({} bytes)", gb(size), mmc::thousands(size as usize))], vec!["PNP Device ID".into(), s("path")], vec![String::new(), String::new()]]);
            }
            Table { cols: vec!["Item", "Value"], rows }
        }
        "comp.storage.scsi" | "comp.storage.ide" => {
            let rows: Vec<Vec<String>> = pci_by_class(if id.ends_with("ide") { &["0101", "0106"] } else { &["0100", "0104", "0107", "0108", "0180"] }).into_iter().flat_map(|(slot, name, drv, _)| vec![vec!["Name".into(), name], vec!["Driver".into(), drv], vec!["PNP Device ID".into(), format!("PCI\\{slot}")], vec![String::new(), String::new()]]).collect();
            Table { cols: vec!["Item", "Value"], rows }
        }
        "comp.printing" => table(vec!["Name", "Driver", "Port Name", "Server Name"], sh(&["lpstat", "-v"]).lines().filter_map(|l| { let r = l.strip_prefix("device for ")?; let (n, u) = r.split_once(": ")?; Some(vec![n.to_string(), "CUPS".into(), u.to_string(), "localhost".into()]) }).collect()),
        "comp.problem" => table(vec!["Device", "PNP Device ID", "Error Code"], super::devmgr::collect(false).into_iter().filter(|d| !d.problem.is_empty()).map(|d| vec![d.name, d.hwids.first().cloned().unwrap_or_default(), d.problem]).collect()),
        "comp.usb" => table(vec!["Device", "PNP Device ID"], super::devmgr::collect(false).into_iter().filter(|d| d.bus == "USB").map(|d| vec![d.name, d.hwids.first().cloned().unwrap_or_default()]).collect()),
        "sw.drivers" => {
            let rows: Vec<Vec<String>> = std::fs::read_to_string("/proc/modules").unwrap_or_default().lines().map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                vec![f.first().copied().unwrap_or("").into(), format!("{} KB", f.get(1).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) / 1024), "Kernel Driver".into(), "Yes".into(), f.get(4).copied().unwrap_or("").into(), "OK".into(), f.get(3).map(|u| u.trim_end_matches(',').replace(',', ", ")).filter(|u| u != "-").unwrap_or_default()]
            }).collect();
            table(vec!["Name", "Size", "Type", "Started", "State", "Status", "Used By"], rows)
        }
        "sw.env" => {
            let user = glib::user_name().to_string_lossy().into_owned();
            let mut v: Vec<Vec<String>> = std::env::vars().map(|(k, val)| vec![k, val, user.clone()]).collect();
            v.sort();
            table(vec!["Variable", "Value", "User Name"], v)
        }
        "sw.print" => table(vec!["Document", "Size", "Owner", "Notify", "Status", "Time Submitted"], sh(&["lpstat", "-o"]).lines().map(|l| { let f: Vec<&str> = l.split_whitespace().collect(); vec![f.first().copied().unwrap_or("").into(), f.get(2).copied().unwrap_or("").into(), f.get(1).copied().unwrap_or("").into(), String::new(), "Queued".into(), f[3.min(f.len())..].join(" ")] }).collect()),
        "sw.netconn" => table(vec!["Local Name", "Remote Name", "Type", "Status", "User Name"], std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines().filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() > 2 && matches!(f[2], "nfs" | "nfs4" | "cifs" | "smb3" | "fuse.sshfs" | "davfs")).then(|| vec![f[1].to_string(), f[0].to_string(), f[2].to_uppercase(), "OK".into(), glib::user_name().to_string_lossy().into_owned()])
        }).collect()),
        "sw.tasks" => {
            let mut rows = vec![];
            let hz = 100.0;
            let boot = std::fs::read_to_string("/proc/stat").unwrap_or_default().lines().find_map(|l| l.strip_prefix("btime ").and_then(|v| v.trim().parse::<i64>().ok())).unwrap_or(0);
            if let Ok(it) = std::fs::read_dir("/proc") {
                for e in it.flatten() {
                    let pid = e.file_name().to_string_lossy().into_owned();
                    if pid.parse::<u32>().is_err() { continue; }
                    let stat = std::fs::read_to_string(e.path().join("stat")).unwrap_or_default();
                    let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else { continue };
                    let name = stat[a + 1..b].to_string();
                    let f: Vec<&str> = stat[b + 2..].split_whitespace().collect();
                    let nice = f.get(16).copied().unwrap_or("0");
                    let start = f.get(19).and_then(|x| x.parse::<f64>().ok()).map(|t| boot + (t / hz) as i64).unwrap_or(0);
                    let rss = f.get(21).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) * 4096;
                    let path = std::fs::read_link(e.path().join("exe")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "Not Available".into());
                    rows.push(vec![name, path, pid, nice.to_string(), format!("{} KB", rss / 1024), when(start)]);
                }
            }
            rows.sort_by(|a, b| a[0].to_lowercase().cmp(&b[0].to_lowercase()));
            table(vec!["Name", "Path", "Process ID", "Priority", "Working Set", "Start Time"], rows)
        }
        "sw.modules" => {
            let mut libs: Vec<String> = std::fs::read_to_string("/proc/self/maps").unwrap_or_default().lines().filter_map(|l| l.split_whitespace().nth(5)).filter(|p| p.contains(".so")).map(|s| s.to_string()).collect();
            libs.sort();
            libs.dedup();
            table(vec!["Name", "Path", "Size"], libs.into_iter().map(|p| { let sz = std::fs::metadata(&p).map(|m| gb(m.len())).unwrap_or_default(); vec![Path::new(&p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), p, sz] }).collect())
        }
        "sw.services" => {
            let files = sh(&["systemctl", "list-unit-files", "--type=service", "--no-legend", "--plain", "--no-pager"]);
            let state: HashMap<&str, &str> = files.lines().filter_map(|l| { let mut it = l.split_whitespace(); Some((it.next()?, it.next()?)) }).collect();
            let rows: Vec<Vec<String>> = sh(&["systemctl", "list-units", "--type=service", "--all", "--no-legend", "--plain", "--no-pager"]).lines().filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                (f.len() >= 4).then(|| vec![f[4..].join(" "), f[0].to_string(), if f[3] == "running" { "Running".into() } else { "Stopped".into() }, match state.get(f[0]).copied().unwrap_or("static") { "enabled" => "Auto".to_string(), "disabled" | "masked" => "Disabled".into(), _ => "Manual".into() }, "Own Process".into(), "LocalSystem".into()])
            }).collect();
            table(vec!["Display Name", "Name", "State", "Start Mode", "Service Type", "Start Name"], rows)
        }
        "sw.groups" | "sw.startup" | "sw.ole" => Table { cols: vec![], rows: vec![] }, // filled on the GTK thread (gio)
        "sw.wer" => table(vec!["Time", "Type", "Details"], sh(&["coredumpctl", "list", "--no-legend", "--no-pager", "-r", "-q"]).lines().take(200).map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let exe = f.iter().rev().find(|x| x.starts_with('/')).copied().unwrap_or("");
            vec![f.iter().take(4).copied().collect::<Vec<_>>().join(" "), "Application Error".into(), format!("Faulting application {exe}, PID {}, signal {}", f.get(4).copied().unwrap_or(""), f.get(7).copied().unwrap_or(""))]
        }).collect()),
        _ => Table::default(),
    }
}

/// Categories that need gio (desktop files) are built on the GTK thread.
fn load_gtk(id: &str) -> Option<Table> {
    match id {
        "sw.groups" => {
            let mut rows: Vec<Vec<String>> = vec![];
            let user = glib::user_name().to_string_lossy().into_owned();
            for a in util::all_apps() {
                let cat = util::categories(&a);
                let group = ["Accessories:Utility", "Games:Game", "Graphics:Graphics", "Internet:Network", "Office:Office", "Multimedia:AudioVideo", "Development:Development", "System Tools:System", "Settings:Settings", "Education:Education"]
                    .iter().find_map(|g| { let (n, c) = g.split_once(':').unwrap(); cat.split(';').any(|x| x == c).then_some(n) }).unwrap_or("Other");
                rows.push(vec![format!("{user}:{group}"), a.display_name().to_string(), user.clone()]);
            }
            rows.sort();
            Some(table(vec!["Program Group Name", "Name", "User Name"], rows))
        }
        "sw.startup" => {
            let mut rows = vec![];
            let user = glib::user_name().to_string_lossy().into_owned();
            let dirs: Vec<std::path::PathBuf> = glib::system_config_dirs().iter().map(|d| d.join("autostart")).chain(std::iter::once(glib::user_config_dir().join("autostart"))).collect();
            for d in dirs {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    if let Some(app) = gtk::gio::DesktopAppInfo::from_filename(e.path()) {
                        if app.is_hidden() { continue; }
                        rows.push(vec![app.display_name().to_string(), app.commandline().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default(), if d.starts_with(glib::user_config_dir()) { user.clone() } else { "Public".into() }, d.to_string_lossy().into_owned()]);
                    }
                }
            }
            Some(table(vec!["Program", "Command", "User Name", "Location"], rows))
        }
        "sw.ole" => {
            let mut rows = vec![];
            for mime in ["text/plain", "text/html", "application/pdf", "image/png", "image/jpeg", "audio/mpeg", "video/mp4", "application/zip", "inode/directory", "x-scheme-handler/http", "x-scheme-handler/https", "x-scheme-handler/mailto",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "application/vnd.oasis.opendocument.text", "application/x-shellscript"] {
                let app = util::default_for(mime).map(|a| a.display_name().to_string()).unwrap_or_else(|| "Not Available".into());
                rows.push(vec![mime.to_string(), app]);
            }
            Some(table(vec!["Object", "Local Server"], rows))
        }
        _ => None,
    }
}

struct State {
    win: gtk::Window,
    store: gtk::TreeStore,
    tree: gtk::TreeView,
    list: gtk::TreeView,
    cache: RefCell<HashMap<String, Table>>,
    current: RefCell<String>,
    status: gtk::Label,
    find: gtk::Entry,
    only_sel: gtk::CheckButton,
    only_names: gtk::CheckButton,
}

pub fn window(app: &gtk::Application) -> gtk::Window {
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_application(Some(app));
    win.set_title("System Information");
    win.set_default_size(900, 600);
    win.set_icon(pix(&["aerode-msinfo", "dialog-information"], 48).as_ref());
    win.style_context().add_class("adm");
    let root = vbox(0);
    let bar = gtk::MenuBar::new();
    let menu = |name: &str| { let mi = gtk::MenuItem::with_mnemonic(name); let m = gtk::Menu::new(); mi.set_submenu(Some(&m)); bar.append(&mi); m };
    let file = menu("_File");
    let edit = menu("_Edit");
    let view = menu("_View");
    let tools = menu("_Tools");
    let help = menu("_Help");
    root.pack_start(&bar, false, false, 0);
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let mut iters: HashMap<&str, gtk::TreeIter> = HashMap::new();
    for (id, title, parent) in TREE {
        let p = iters.get(parent).cloned();
        let it = store.insert_with_values(p.as_ref(), None, &[(0, title), (1, id)]);
        iters.insert(id, it);
    }
    let tree = gtk::TreeView::with_model(&store);
    tree.set_headers_visible(false);
    columns(&tree, &[("", 0, 0)]);
    let list = gtk::TreeView::new();
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    let tsw = scrolled(&tree);
    tsw.set_size_request(230, -1);
    paned.pack1(&tsw, false, false);
    paned.pack2(&scrolled(&list), true, false);
    paned.set_position(230);
    root.pack_start(&paned, true, true, 0);
    // Find bar
    let fb = gtk::Grid::new();
    fb.style_context().add_class("si-find");
    fb.set_column_spacing(8);
    fb.set_row_spacing(4);
    fb.attach(&gtk::Label::new(Some("Find what:")), 0, 0, 1, 1);
    let find = gtk::Entry::new();
    find.set_hexpand(true);
    fb.attach(&find, 1, 0, 1, 1);
    let fbtn = gtk::Button::with_mnemonic("_Find");
    let close = gtk::Button::with_mnemonic("_Close Find");
    fb.attach(&fbtn, 2, 0, 1, 1);
    fb.attach(&close, 3, 0, 1, 1);
    let only_sel = gtk::CheckButton::with_mnemonic("_Search selected category only");
    let only_names = gtk::CheckButton::with_mnemonic("Search category _names only");
    let chk = hbox(20);
    chk.pack_start(&only_sel, false, false, 0);
    chk.pack_start(&only_names, false, false, 0);
    fb.attach(&chk, 0, 1, 4, 1);
    root.pack_start(&fb, false, false, 0);
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.style_context().add_class("mmc-status");
    root.pack_start(&status, false, false, 0);
    win.add(&root);
    let s = Rc::new(State { win: win.clone(), store, tree: tree.clone(), list, cache: RefCell::new(HashMap::new()), current: RefCell::new(String::new()), status, find: find.clone(), only_sel, only_names });
    { let keep = RefCell::new(Some(s.clone())); win.connect_destroy(move |_| { keep.borrow_mut().take(); }); }
    { let w = Rc::downgrade(&s); tree.selection().connect_changed(move |sel| if let (Some(s), Some((m, it))) = (w.upgrade(), sel.selected()) { let id = sval(&m, &it, 1); show(&s, &id); }); }
    { let w = Rc::downgrade(&s); fbtn.connect_clicked(move |_| if let Some(s) = w.upgrade() { do_find(&s) }); }
    { let w = Rc::downgrade(&s); find.connect_activate(move |_| if let Some(s) = w.upgrade() { do_find(&s) }); }
    { let fb2 = fb.clone(); close.connect_clicked(move |_| fb2.hide()); }
    // menus
    let item = |m: &gtk::Menu, l: &str, f: Option<Box<dyn Fn()>>| { let mi = gtk::MenuItem::with_mnemonic(l); match f { Some(f) => { mi.connect_activate(move |_| f()); } None => mi.set_sensitive(false) } m.append(&mi); };
    item(&file, "_Open...", None);
    item(&file, "_Close", None);
    item(&file, "_Save", None);
    { let w = Rc::downgrade(&s); item(&file, "_Export...", Some(Box::new(move || if let Some(s) = w.upgrade() { export(&s) }))); }
    file.append(&gtk::SeparatorMenuItem::new());
    item(&file, "_Print...", None);
    file.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); item(&file, "E_xit", Some(Box::new(move || w.close()))); }
    { let w = Rc::downgrade(&s); item(&edit, "_Copy", Some(Box::new(move || if let Some(s) = w.upgrade() { copy_rows(&s) }))); }
    { let w = Rc::downgrade(&s); item(&edit, "Select _All", Some(Box::new(move || if let Some(s) = w.upgrade() { s.list.selection().select_all(); }))); }
    { let (fb2, f2) = (fb.clone(), find.clone()); item(&edit, "_Find", Some(Box::new(move || { fb2.show(); f2.grab_focus(); }))); }
    { let w = Rc::downgrade(&s); item(&view, "_System Information", Some(Box::new(move || if let Some(s) = w.upgrade() { s.tree.selection().select_path(&gtk::TreePath::new_first()); }))); }
    { let w = Rc::downgrade(&s); item(&view, "_Refresh", Some(Box::new(move || if let Some(s) = w.upgrade() { s.cache.borrow_mut().clear(); let id = s.current.borrow().clone(); s.current.borrow_mut().clear(); show(&s, &id); }))); }
    item(&tools, "_Disk Cleanup", Some(Box::new(|| spawn_tool("cleanmgr"))));
    item(&tools, "_System Restore", Some(Box::new(|| spawn_tool("backup"))));
    item(&tools, "_Task Manager", Some(Box::new(|| { util::spawn(&["aerode-taskmgr"]); })));
    item(&tools, "Device _Manager", Some(Box::new(|| spawn_tool("devmgr"))));
    { let w = win.clone(); item(&help, "_About System Information", Some(Box::new(move || inform(Some(&w), "About System Information", "System Information", "Shows what the Linux system reports about its hardware and software (sysfs, /proc, lsblk, lspci, systemd).")))); }
    win.show_all();
    tree.expand_row(&gtk::TreePath::new_first(), false);
    tree.selection().select_path(&gtk::TreePath::new_first());
    win
}

fn set_table(s: &State, t: &Table) {
    for c in s.list.columns() { s.list.remove_column(&c); }
    let n = t.cols.len().max(1);
    let store = gtk::ListStore::new(&vec![glib::Type::STRING; n]);
    for r in &t.rows {
        let it = store.append();
        for (i, v) in r.iter().enumerate().take(n) { store.set_value(&it, i as u32, &v.to_value()); }
    }
    s.list.set_model(Some(&store));
    let widths: Vec<i32> = if t.cols == ["Item", "Value"] { vec![240, 0] } else { (0..n).map(|i| if i + 1 == n { 0 } else { 170 }).collect() };
    let cols: Vec<(&str, i32, i32)> = t.cols.iter().enumerate().map(|(i, c)| (*c, i as i32, widths[i])).collect();
    columns(&s.list, &cols);
    s.list.selection().set_mode(gtk::SelectionMode::Multiple);
}

fn show(s: &Rc<State>, id: &str) {
    if id.is_empty() || *s.current.borrow() == id { return; }
    *s.current.borrow_mut() = id.to_string();
    if let Some(t) = s.cache.borrow().get(id).cloned() { set_table(s, &t); return; }
    if let Some(t) = load_gtk(id) { s.cache.borrow_mut().insert(id.to_string(), t.clone()); set_table(s, &t); return; }
    set_table(s, &Table { cols: vec!["Item", "Value"], rows: vec![] });
    s.status.set_text("Refreshing system information...");
    let (s2, idc) = (s.clone(), id.to_string());
    let idw = id.to_string();
    bg(move || load(&idw), move |t| {
        s2.status.set_text("");
        s2.cache.borrow_mut().insert(idc.clone(), t.clone());
        if *s2.current.borrow() == idc { set_table(&s2, &t); }
    });
}

fn table_text(t: &Table) -> String {
    let mut out = String::new();
    if !t.cols.is_empty() { out += &t.cols.join("\t"); out.push('\n'); }
    for r in &t.rows { out += &r.join("\t"); out.push('\n'); }
    out
}

fn copy_rows(s: &State) {
    let (paths, m) = s.list.selection().selected_rows();
    let mut out = String::new();
    for p in paths { if let Some(it) = m.iter(&p) { let n = m.n_columns(); out += &(0..n).map(|i| sval(&m, &it, i)).collect::<Vec<_>>().join("\t"); out.push('\n'); } }
    copy(&out);
}

fn export(s: &Rc<State>) {
    let fc = gtk::FileChooserNative::new(Some("Export As"), Some(&s.win), gtk::FileChooserAction::Save, Some("_Save"), Some("_Cancel"));
    fc.set_current_name("System Information.txt");
    fc.set_do_overwrite_confirmation(true);
    if fc.run() != gtk::ResponseType::Accept { return; }
    let Some(path) = fc.filename() else { return };
    s.status.set_text("Exporting system information...");
    let gtk_parts: Vec<(String, Table)> = ["sw.groups", "sw.startup", "sw.ole"].iter().filter_map(|id| load_gtk(id).map(|t| (id.to_string(), t))).collect();
    let s2 = s.clone();
    bg(move || {
        let mut out = format!("System Information report written at: {}\nSystem Name: {}\n", when(now()), glib::host_name());
        for (id, title, _) in TREE {
            out += &format!("[{title}]\n\n");
            let t = gtk_parts.iter().find(|(i, _)| i == id).map(|(_, t)| t.clone()).unwrap_or_else(|| load(id));
            out += &table_text(&t);
            out.push('\n');
        }
        std::fs::write(&path, out).map_err(|e| e.to_string())
    }, move |r| { s2.status.set_text(""); if let Err(e) = r { util::error("System Information could not be exported.", &e); } });
}

fn select_id(s: &State, id: &str) {
    let mut path = None;
    s.store.foreach(|m, p, it| { if sval(m, it, 1) == id { path = Some(p.clone()); true } else { false } });
    if let Some(p) = path { s.tree.expand_to_path(&p); s.tree.selection().select_path(&p); s.tree.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0); }
}

fn do_find(s: &Rc<State>) {
    let q = s.find.text().to_lowercase();
    if q.is_empty() { return; }
    let cur = s.current.borrow().clone();
    if s.only_names.is_active() {
        let start = TREE.iter().position(|t| t.0 == cur).map(|i| i + 1).unwrap_or(0);
        for k in 0..TREE.len() { let t = TREE[(start + k) % TREE.len()]; if t.1.to_lowercase().contains(&q) { select_id(s, t.0); return; } }
        inform(Some(&s.win), "System Information", "Windows has finished searching.", "");
        return;
    }
    // next match in the current table after the selected row
    let start_row = s.list.selection().selected_rows().0.first().and_then(|p| p.indices().first().copied()).map(|i| i + 1).unwrap_or(0) as usize;
    if let Some(t) = s.cache.borrow().get(&cur) {
        if let Some(i) = t.rows.iter().enumerate().skip(start_row).find(|(_, r)| r.iter().any(|c| c.to_lowercase().contains(&q))).map(|x| x.0) {
            let p = gtk::TreePath::from_indicesv(&[i as i32]);
            s.list.selection().unselect_all();
            s.list.selection().select_path(&p);
            s.list.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, true, 0.5, 0.0);
            return;
        }
    }
    if s.only_sel.is_active() { inform(Some(&s.win), "System Information", "Windows has finished searching.", ""); return; }
    // the following categories (loaded on a worker thread)
    let from = TREE.iter().position(|t| t.0 == cur).map(|i| i + 1).unwrap_or(0);
    let order: Vec<&'static str> = (0..TREE.len()).map(|k| TREE[(from + k) % TREE.len()].0).filter(|id| *id != cur).collect();
    let cached: HashMap<String, Table> = s.cache.borrow().clone();
    let gtk_parts: HashMap<String, Table> = ["sw.groups", "sw.startup", "sw.ole"].iter().filter_map(|id| load_gtk(id).map(|t| (id.to_string(), t))).collect();
    s.status.set_text("Searching...");
    let s2 = s.clone();
    bg(move || {
        for id in order {
            let t = cached.get(id).cloned().or_else(|| gtk_parts.get(id).cloned()).unwrap_or_else(|| load(id));
            if let Some(i) = t.rows.iter().position(|r| r.iter().any(|c| c.to_lowercase().contains(&q))) { return Some((id, t, i)); }
        }
        None
    }, move |r| {
        s2.status.set_text("");
        match r {
            Some((id, t, i)) => {
                s2.cache.borrow_mut().insert(id.to_string(), t);
                select_id(&s2, id);
                let p = gtk::TreePath::from_indicesv(&[i as i32]);
                s2.list.selection().unselect_all();
                s2.list.selection().select_path(&p);
                s2.list.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, true, 0.5, 0.0);
            }
            None => inform(Some(&s2.win), "System Information", "Windows has finished searching.", ""),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_category_loads() {
        for (id, _, _) in TREE { let _ = load(id); }
        let s = summary();
        assert!(s.rows.iter().any(|r| r[0] == "OS Name" && !r[1].is_empty()));
    }
}
