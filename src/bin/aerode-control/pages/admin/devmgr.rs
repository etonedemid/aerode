//! Device Manager (devmgmt.msc): the hardware from sysfs (PCI, USB, ACPI, input, block, DRM,
//! power supplies) sorted into the Windows 7 device classes, with the Win7 Properties sheet
//! (General / Driver / Details / Resources). Names come from lspci/lsusb or the pci.ids /
//! usb.ids databases when available.
use gtk::prelude::*;
use gtk::{gdk_pixbuf::Pixbuf, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::mmc::{self, act, act_off, Mmc};
use super::*;

#[derive(Clone, Default, Debug)]
pub struct Dev {
    pub class: &'static str,
    pub name: String,
    pub sys: String,
    pub bus: String,
    pub maker: String,
    pub location: String,
    pub driver: String,
    pub module: String,
    pub hwids: Vec<String>,
    pub compat: Vec<String>,
    pub problem: String,
    pub resources: Vec<(String, String)>,
    pub extra: Vec<(String, String)>,
    pub hidden: bool,
}

/// Win7 device classes (display name, icon names).
pub const CLASSES: &[(&str, &[&str])] = &[
    ("Batteries", &["aerode-dev-battery", "battery"]),
    ("Bluetooth Radios", &["aerode-dev-bluetooth", "bluetooth"]),
    ("Computer", &["aerode-dev-computer", "computer"]),
    ("Disk drives", &["aerode-dev-disk", "drive-harddisk"]),
    ("Display adapters", &["aerode-dev-display", "video-display"]),
    ("DVD/CD-ROM drives", &["aerode-dev-cdrom", "drive-optical"]),
    ("Human Interface Devices", &["aerode-dev-hid", "input-mouse"]),
    ("IDE ATA/ATAPI controllers", &["aerode-dev-ide", "drive-harddisk"]),
    ("IEEE 1394 Bus host controllers", &["aerode-dev-ide", "drive-harddisk"]),
    ("Imaging devices", &["aerode-dev-imaging", "camera-web"]),
    ("Keyboards", &["aerode-dev-keyboard", "input-keyboard"]),
    ("Memory technology devices", &["aerode-dev-memory", "media-flash"]),
    ("Mice and other pointing devices", &["aerode-dev-mouse", "input-mouse"]),
    ("Monitors", &["aerode-dev-monitor", "video-display"]),
    ("Network adapters", &["aerode-dev-network", "network-wired"]),
    ("Other devices", &["aerode-dev-unknown", "dialog-question"]),
    ("Ports (COM & LPT)", &["aerode-dev-ports", "network-wired"]),
    ("Processors", &["aerode-dev-cpu", "computer"]),
    ("Software devices", &["aerode-dev-system", "computer"]),
    ("Sound, video and game controllers", &["aerode-dev-sound", "audio-card", "multimedia-volume-control"]),
    ("Storage controllers", &["aerode-dev-storage", "drive-harddisk"]),
    ("Storage volumes", &["drive-harddisk"]),
    ("System devices", &["aerode-dev-system", "computer"]),
    ("Universal Serial Bus controllers", &["aerode-dev-usb", "drive-removable-media-usb"]),
];

pub fn class_icon(c: &str) -> &'static [&'static str] { CLASSES.iter().find(|x| x.0 == c).map(|x| x.1).unwrap_or(&["computer"]) }

fn rd(p: &Path) -> String { std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default() }
fn link_name(p: &Path) -> String { std::fs::read_link(p).ok().and_then(|l| l.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default() }
fn hex(s: &str) -> u32 { u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0) }

/// Vendor names for when no pci.ids/usb.ids is installed.
fn known_vendor(id: u32) -> &'static str {
    match id {
        0x8086 => "Intel Corporation", 0x1022 => "Advanced Micro Devices, Inc. [AMD]", 0x1002 => "Advanced Micro Devices, Inc. [AMD/ATI]", 0x10de => "NVIDIA Corporation",
        0x10ec => "Realtek Semiconductor Co., Ltd.", 0x1af4 => "Red Hat, Inc.", 0x14e4 => "Broadcom Inc.", 0x168c => "Qualcomm Atheros", 0x14c3 => "MEDIATEK Corp.",
        0x144d => "Samsung Electronics Co Ltd", 0x15b7 => "Sandisk Corp", 0x2646 => "Kingston Technology Company, Inc.", 0x1344 => "Micron Technology Inc", 0x1987 => "Phison Electronics Corporation",
        0x1b21 => "ASMedia Technology Inc.", 0x15ad => "VMware", 0x80ee => "InnoTek Systemberatung GmbH", 0x1234 => "QEMU", 0x046d => "Logitech, Inc.", 0x045e => "Microsoft Corp.",
        0x1d6b => "Linux Foundation", 0x0bda => "Realtek Semiconductor Corp.", 0x8087 => "Intel Corp.", 0x05ac => "Apple, Inc.", 0x1532 => "Razer USA, Ltd", 0x0781 => "SanDisk Corp.",
        _ => "",
    }
}

/// Minimal reader for pci.ids / usb.ids: vendor → (name, device → name).
#[derive(Default)]
pub struct Ids { vendors: HashMap<u32, (String, HashMap<u32, String>)>, classes: HashMap<u32, String> }
impl Ids {
    pub fn load(paths: &[&str]) -> Ids {
        let mut ids = Ids::default();
        let Some(txt) = paths.iter().find_map(|p| std::fs::read(p).ok()) else { return ids };
        let txt = String::from_utf8_lossy(&txt);
        let mut vendor: Option<u32> = None;
        let mut in_class = false;
        for l in txt.lines() {
            if l.starts_with('#') || l.trim().is_empty() { continue; }
            if l.starts_with("C ") { in_class = true; vendor = None; if let Some((c, n)) = l[2..].split_once("  ") { ids.classes.insert(hex(c), n.to_string()); } continue; }
            if !l.starts_with('\t') {
                in_class = false;
                if let Some((v, n)) = l.split_once("  ") { if let Ok(v) = u32::from_str_radix(v.trim(), 16) { ids.vendors.insert(v, (n.to_string(), HashMap::new())); vendor = Some(v); continue; } }
                vendor = None;
            } else if !in_class && !l.starts_with("\t\t") {
                if let (Some(v), Some((d, n))) = (vendor, l[1..].split_once("  ")) { if let Ok(d) = u32::from_str_radix(d.trim(), 16) { if let Some(e) = ids.vendors.get_mut(&v) { e.1.insert(d, n.to_string()); } } }
            }
        }
        ids
    }
    pub fn vendor(&self, v: u32) -> String { self.vendors.get(&v).map(|x| x.0.clone()).unwrap_or_else(|| known_vendor(v).to_string()) }
    pub fn device(&self, v: u32, d: u32) -> String { self.vendors.get(&v).and_then(|x| x.1.get(&d)).cloned().unwrap_or_default() }
}

/// "Intel Corporation" → "Intel", "NVIDIA Corporation" → "NVIDIA" (Win7 device names use the brand).
fn brand(v: &str) -> String {
    let v = v.split(" [").next().unwrap_or(v);
    for suf in [" Corporation", " Corp.", ", Inc.", " Inc.", " Co., Ltd.", " Co., Ltd", " Technology", " Semiconductor", " Electronics", " Co Ltd", " Ltd."] {
        if let Some(s) = v.strip_suffix(suf) { return brand(s); }
    }
    v.to_string()
}

/// "TU116 [GeForce GTX 1660 SUPER]" + "NVIDIA" → "NVIDIA GeForce GTX 1660 SUPER".
pub fn pci_name(vendor: &str, device: &str) -> String {
    let b = brand(vendor);
    let d = match (device.find('['), device.rfind(']')) { (Some(a), Some(z)) if z > a => device[a + 1..z].to_string(), _ => device.to_string() };
    if d.is_empty() { return String::new(); }
    if b.is_empty() || d.to_lowercase().starts_with(&b.to_lowercase()) { d } else { format!("{b} {d}") }
}

/// The Win7 class for a PCI class code (0xCCSSPP).
pub fn pci_class(code: u32, has_driver: bool) -> &'static str {
    let (c, s) = (code >> 16, (code >> 8) & 0xff);
    match (c, s) {
        (0x01, 0x01) | (0x01, 0x06) => "IDE ATA/ATAPI controllers",
        (0x01, _) => "Storage controllers",
        (0x02, _) => "Network adapters",
        (0x03, _) => "Display adapters",
        (0x04, _) => "Sound, video and game controllers",
        (0x07, 0x00) | (0x07, 0x01) => "Ports (COM & LPT)",
        (0x09, _) => "Human Interface Devices",
        (0x0c, 0x00) => "IEEE 1394 Bus host controllers",
        (0x0c, 0x03) => "Universal Serial Bus controllers",
        (0x0d, 0x11) => "Bluetooth Radios",
        (0x0d, _) => "Network adapters",
        (0x05, _) | (0x06, _) | (0x07, _) | (0x08, _) | (0x0b, _) | (0x0c, _) | (0x10, _) | (0x11, _) | (0x12, _) | (0x13, _) => "System devices",
        _ => if has_driver { "System devices" } else { "Other devices" },
    }
}

/// Win7 names of the ACPI devices that end up under System devices.
fn acpi_name(hid: &str) -> Option<&'static str> {
    Some(match hid {
        "PNP0A08" | "PNP0A03" => "PCI Express Root Complex",
        "PNP0B00" => "System CMOS/real time clock",
        "PNP0100" => "System timer",
        "PNP0103" => "High precision event timer",
        "PNP0C02" => "Motherboard resources",
        "PNP0C01" => "System board",
        "PNP0200" => "Direct memory access controller",
        "PNP0000" | "PNP0003" => "Programmable interrupt controller",
        "PNP0800" => "System speaker",
        "PNP0C04" => "Numeric data processor",
        "PNP0C14" => "Microsoft Windows Management Interface for ACPI",
        "PNP0C09" => "Microsoft ACPI-Compliant Embedded Controller",
        "PNP0C0C" => "ACPI Power Button",
        "PNP0C0E" => "ACPI Sleep Button",
        "PNP0C0D" => "ACPI Lid",
        "ACPI0003" => "Microsoft AC Adapter",
        "PNP0C0A" => "Microsoft ACPI-Compliant Control Method Battery",
        "ACPI000E" => "ACPI Wake Alarm",
        "ACPI0010" => "Processor Aggregator",
        "PNP0C0B" => "ACPI Fan",
        "LNXTHERM" | "PNP0C11" => "ACPI Thermal Zone",
        "PNP0C33" => "Error Device",
        "PNP0C40" => "Windows Button Array Device",
        "INT33A1" => "Intel(R) Power Engine Plug-in",
        "PNP0C50" => "I2C HID Device",
        "LNXSYSTM" => "Microsoft ACPI-Compliant System",
        "PNP0A05" | "PNP0A06" => "Generic Bus",
        "PNP0C0F" => "PCI Interrupt Link",
        "LNXVIDEO" => "ACPI Video Device",
        _ => return None,
    })
}

/// Monitor name from an EDID blob (descriptor 0xFC).
pub fn edid_name(e: &[u8]) -> Option<String> {
    if e.len() < 128 { return None; }
    for i in 0..4 {
        let o = 54 + i * 18;
        if e[o] == 0 && e[o + 1] == 0 && e[o + 3] == 0xfc {
            let s: String = e[o + 5..o + 18].iter().take_while(|&&b| b != 0x0a).map(|&b| b as char).collect();
            let s = s.trim().to_string();
            if !s.is_empty() { return Some(s); }
        }
    }
    None
}

fn module_of(sys: &Path) -> (String, String) {
    let driver = link_name(&sys.join("driver"));
    let module = link_name(&sys.join("driver/module"));
    (driver.clone(), if module.is_empty() { driver } else { module })
}

fn pci_resources(sys: &Path) -> Vec<(String, String)> {
    let mut v = vec![];
    for l in rd(&sys.join("resource")).lines() {
        let f: Vec<u64> = l.split_whitespace().map(|x| u64::from_str_radix(x.trim_start_matches("0x"), 16).unwrap_or(0)).collect();
        if f.len() < 3 || f[0] == 0 && f[1] == 0 { continue; }
        if f[2] & 0x100 != 0 { v.push(("I/O Range".into(), format!("{:04X} - {:04X}", f[0], f[1]))); }
        else if f[2] & 0x200 != 0 { v.push(("Memory Range".into(), format!("{:016X} - {:016X}", f[0], f[1]))); }
    }
    let irq: u32 = rd(&sys.join("irq")).parse().unwrap_or(0);
    if irq > 0 { v.push(("IRQ".into(), format!("0x{irq:08X} ({irq})"))); }
    v
}

/// Every device, from a worker thread. `hidden` adds virtual/software devices.
pub fn collect(hidden: bool) -> Vec<Dev> {
    let mut devs: Vec<Dev> = vec![];
    let pci_ids = Ids::load(&["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids"]);
    let usb_ids = Ids::load(&["/usr/share/hwdata/usb.ids", "/usr/share/misc/usb.ids", "/usr/share/usb.ids"]);
    // lspci -vmm gives the nicest names (and knows subsystem names)
    let mut lspci: HashMap<String, (String, String)> = HashMap::new();
    for block in sh(&["lspci", "-vmm", "-D"]).split("\n\n") {
        let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k)).map(|s| s.trim().to_string()).unwrap_or_default();
        let slot = get("Slot:");
        if !slot.is_empty() { lspci.insert(slot, (get("Vendor:"), get("Device:"))); }
    }
    // ---------------------------------------------------------------- Computer
    let acpi = Path::new("/sys/firmware/acpi").exists();
    let arch = std::env::consts::ARCH;
    devs.push(Dev { class: "Computer", name: format!("{}{}-based PC", if acpi { "ACPI " } else { "" }, if arch == "x86_64" { "x64" } else { arch }), bus: "ACPI".into(), maker: "(Standard computers)".into(),
        driver: "kernel".into(), hwids: vec![format!("acpi{}", if arch == "x86_64" { "x64" } else { arch })], ..Default::default() });
    // ---------------------------------------------------------------- PCI
    if let Ok(rd_) = std::fs::read_dir("/sys/bus/pci/devices") {
        let mut entries: Vec<PathBuf> = rd_.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let sys = std::fs::canonicalize(&p).unwrap_or(p.clone());
            let addr = p.file_name().unwrap().to_string_lossy().to_string();
            let (v, d, sv, sd, class, rev) = (hex(&rd(&sys.join("vendor"))), hex(&rd(&sys.join("device"))), hex(&rd(&sys.join("subsystem_vendor"))), hex(&rd(&sys.join("subsystem_device"))), hex(&rd(&sys.join("class"))), hex(&rd(&sys.join("revision"))));
            let (driver, module) = module_of(&sys);
            let (vn, dn) = lspci.get(&addr).cloned().unwrap_or_else(|| (pci_ids.vendor(v), pci_ids.device(v, d)));
            let mut name = pci_name(&vn, &dn);
            if name.is_empty() {
                name = match class >> 8 { 0x0600 => "PCI standard host CPU bridge".into(), 0x0604 => "PCI standard PCI-to-PCI bridge".into(), 0x0601 => "PCI standard ISA bridge".into(), 0x0c05 => "SM Bus Controller".into(),
                    0x0106 => "Standard AHCI 1.0 Serial ATA Controller".into(), 0x0108 => "Standard NVM Express Controller".into(), 0x0c03 => "Standard USB Host Controller".into(), 0x0403 => "High Definition Audio Controller".into(),
                    _ => if v == 0x1af4 { virtio_name(d) } else { format!("PCI Device") } };
                if v == 0x1af4 && name == "PCI Device" { name = virtio_name(d); }
            }
            let cls = pci_class(class, !driver.is_empty());
            let bus = u32::from_str_radix(addr.get(5..7).unwrap_or("0"), 16).unwrap_or(0);
            let dev = u32::from_str_radix(addr.get(8..10).unwrap_or("0"), 16).unwrap_or(0);
            let func = addr.get(11..12).and_then(|f| f.parse::<u32>().ok()).unwrap_or(0);
            let needs_driver = matches!(class >> 16, 0x01 | 0x02 | 0x03 | 0x04 | 0x0c) && (class >> 8) != 0x0c05;
            let problem = if driver.is_empty() && needs_driver { "The drivers for this device are not installed. (Code 28)".to_string() } else { String::new() };
            let class_ = if !problem.is_empty() { "Other devices" } else { cls };
            devs.push(Dev {
                class: class_, name, sys: sys.to_string_lossy().into(), bus: "PCI".into(), maker: brand(&if vn.is_empty() { known_vendor(v).to_string() } else { vn }),
                location: format!("PCI bus {bus}, device {dev}, function {func}"), driver, module,
                hwids: vec![format!("PCI\\VEN_{v:04X}&DEV_{d:04X}&SUBSYS_{sd:04X}{sv:04X}&REV_{rev:02X}"), format!("PCI\\VEN_{v:04X}&DEV_{d:04X}&SUBSYS_{sd:04X}{sv:04X}"), format!("PCI\\VEN_{v:04X}&DEV_{d:04X}&CC_{:06X}", class), format!("PCI\\VEN_{v:04X}&DEV_{d:04X}&CC_{:04X}", class >> 8)],
                compat: vec![format!("PCI\\VEN_{v:04X}&DEV_{d:04X}&REV_{rev:02X}"), format!("PCI\\VEN_{v:04X}&DEV_{d:04X}"), format!("PCI\\VEN_{v:04X}&CC_{:06X}", class), format!("PCI\\CC_{:06X}", class), format!("PCI\\CC_{:04X}", class >> 8)],
                problem, resources: pci_resources(&sys), extra: vec![("Modalias".into(), rd(&sys.join("modalias"))), ("Class".into(), pci_ids.classes.get(&(class >> 16)).cloned().unwrap_or_default())],
                hidden: false,
            });
        }
    }
    // ---------------------------------------------------------------- USB
    if let Ok(rd_) = std::fs::read_dir("/sys/bus/usb/devices") {
        let mut entries: Vec<PathBuf> = rd_.flatten().map(|e| e.path()).filter(|p| !p.file_name().unwrap().to_string_lossy().contains(':')).collect();
        entries.sort();
        for p in entries {
            let sys = std::fs::canonicalize(&p).unwrap_or(p.clone());
            let id = p.file_name().unwrap().to_string_lossy().to_string();
            let (v, d) = (hex(&rd(&sys.join("idVendor"))), hex(&rd(&sys.join("idProduct"))));
            let rev = rd(&sys.join("bcdDevice"));
            let dclass = hex(&rd(&sys.join("bDeviceClass")));
            let product = rd(&sys.join("product"));
            let manuf = rd(&sys.join("manufacturer"));
            // interface classes
            let mut ifaces: Vec<(u32, u32, u32, String)> = vec![];
            if let Ok(it) = std::fs::read_dir(&sys) {
                for e in it.flatten() {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.starts_with(&format!("{id}:")) { let ip = e.path(); ifaces.push((hex(&rd(&ip.join("bInterfaceClass"))), hex(&rd(&ip.join("bInterfaceSubClass"))), hex(&rd(&ip.join("bInterfaceProtocol"))), link_name(&ip.join("driver")))); }
                }
            }
            let root = id.starts_with("usb");
            let (driver, module) = { let (dr, m) = module_of(&sys); let ifd = ifaces.iter().map(|i| i.3.clone()).find(|x| !x.is_empty()).unwrap_or_default(); if ifd.is_empty() { (dr, m) } else { (ifd.clone(), ifd) } };
            let has = |c: u32| ifaces.iter().any(|i| i.0 == c);
            let (class, generic): (&str, String) = if root { ("Universal Serial Bus controllers", "USB Root Hub".into()) }
                else if dclass == 9 || has(9) { ("Universal Serial Bus controllers", "Generic USB Hub".into()) }
                else if has(0xe0) { ("Bluetooth Radios", "Generic Bluetooth Adapter".into()) }
                else if has(1) { ("Sound, video and game controllers", "USB Audio Device".into()) }
                else if has(0x0e) { ("Imaging devices", "USB Video Device".into()) }
                else if has(3) { ("Human Interface Devices", "USB Input Device".into()) }
                else if has(8) { ("Universal Serial Bus controllers", "USB Mass Storage Device".into()) }
                else if has(2) || has(0x0a) { ("Ports (COM & LPT)", "USB Serial Device".into()) }
                else if has(7) { ("Universal Serial Bus controllers", "USB Printing Support".into()) }
                else if driver.is_empty() { ("Other devices", "Unknown device".into()) }
                else { ("Universal Serial Bus controllers", "USB Composite Device".into()) };
            let name = if root { generic.clone() } else if !product.is_empty() { if class == "Human Interface Devices" || class == "Universal Serial Bus controllers" && generic != "Generic USB Hub" { format!("{generic} ({product})") } else { product.clone() } }
                else { let n = usb_ids.device(v, d); if n.is_empty() { generic.clone() } else { n } };
            let vendor = if manuf.is_empty() { usb_ids.vendor(v) } else { manuf };
            let port = id.rsplit(['.', '-']).next().unwrap_or("0").parse::<u32>().unwrap_or(0);
            let busn = rd(&sys.join("busnum"));
            devs.push(Dev {
                class, name, sys: sys.to_string_lossy().into(), bus: "USB".into(), maker: if root { "(Standard USB Host Controller)".into() } else { brand(&vendor) },
                location: if root { String::new() } else { format!("Port_#{port:04}.Hub_#{:04}", busn.parse::<u32>().unwrap_or(0)) }, driver, module,
                hwids: vec![format!("USB\\VID_{v:04X}&PID_{d:04X}&REV_{rev}"), format!("USB\\VID_{v:04X}&PID_{d:04X}")],
                compat: ifaces.iter().map(|i| format!("USB\\Class_{:02X}&SubClass_{:02X}&Prot_{:02X}", i.0, i.1, i.2)).collect(),
                problem: if class == "Other devices" { "The drivers for this device are not installed. (Code 28)".into() } else { String::new() },
                extra: vec![("Speed".into(), format!("{} Mbit/s", rd(&sys.join("speed")))), ("Serial number".into(), rd(&sys.join("serial")))], ..Default::default()
            });
        }
    }
    // ---------------------------------------------------------------- input devices
    let inputs = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
    for block in inputs.split("\n\n") {
        let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k)).map(|s| s.trim().to_string()).unwrap_or_default();
        let name = get("N: Name=").trim_matches('"').to_string();
        let h = get("H: Handlers=");
        let phys = get("P: Phys=");
        let sysfs = get("S: Sysfs=");
        if name.is_empty() { continue; }
        let ps2 = phys.starts_with("isa0060");
        let (class, nm) = if h.contains("mouse") { ("Mice and other pointing devices", if ps2 { "PS/2 Compatible Mouse".to_string() } else if phys.contains("usb") { format!("HID-compliant mouse ({name})") } else { name.clone() }) }
            else if h.contains("kbd") && (name.to_lowercase().contains("keyboard") || ps2) { ("Keyboards", if ps2 { "Standard PS/2 Keyboard".to_string() } else if phys.contains("usb") { format!("HID Keyboard Device ({name})") } else { name.clone() }) }
            else { continue };
        let b = get("I: Bus=");
        devs.push(Dev { class, name: nm, sys: format!("/sys{sysfs}"), bus: if ps2 { "ACPI".into() } else if phys.contains("usb") { "HID".into() } else { "Input".into() }, maker: if ps2 { "(Standard keyboards)".into() } else { String::new() },
            location: phys.clone(), driver: if ps2 { "i8042".into() } else { "hid-generic".into() }, module: if ps2 { "atkbd".into() } else { "usbhid".into() },
            hwids: vec![format!("INPUT\\{}", b.replace(' ', "&"))], ..Default::default() });
    }
    // ---------------------------------------------------------------- processors
    let cpu = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpu.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string())).unwrap_or_else(|| "Processor".into());
    let vendor = cpu.lines().find_map(|l| l.strip_prefix("vendor_id").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string())).unwrap_or_default();
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let cpu_drv = rd(Path::new("/sys/devices/system/cpu/cpufreq/policy0/scaling_driver"));
    for i in 0..n {
        devs.push(Dev { class: "Processors", name: model.clone(), sys: format!("/sys/devices/system/cpu/cpu{i}"), bus: "ACPI".into(), maker: match vendor.as_str() { "GenuineIntel" => "Intel".into(), "AuthenticAMD" => "AMD".into(), v => v.into() },
            location: format!("Processor {i}"), driver: if cpu_drv.is_empty() { "processor".into() } else { cpu_drv.clone() }, module: if cpu_drv.is_empty() { "processor".into() } else { cpu_drv.clone() },
            hwids: vec![format!("ACPI\\{}", vendor), "ACPI\\Processor".into()], ..Default::default() });
    }
    // ---------------------------------------------------------------- disks
    if let Some(v) = util::output(&["lsblk", "-J", "-d", "-o", "NAME,MODEL,TRAN,TYPE,SIZE,VENDOR,SERIAL,REV,RM"]).and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
        for b in v["blockdevices"].as_array().into_iter().flatten() {
            let s = |k: &str| b[k].as_str().unwrap_or("").trim().to_string();
            let (name, ty, tran) = (s("name"), s("type"), s("tran"));
            if name.starts_with("loop") || name.starts_with("zram") || name.starts_with("ram") || name.starts_with("nbd") { continue; }
            let model = { let m = s("model"); if m.is_empty() { format!("{} {}", s("vendor"), name).trim().to_string() } else { m } };
            let suffix = match tran.as_str() { "sata" | "ata" | "pata" => "ATA Device", "usb" => "USB Device", "nvme" => "NVMe Device", "scsi" | "sas" => "SCSI Disk Device", "virtio" => "VirtIO Disk Device", "mmc" => "SD Memory Card", _ => "Disk Device" };
            let class = if ty == "rom" { "DVD/CD-ROM drives" } else { "Disk drives" };
            let sys = std::fs::canonicalize(format!("/sys/block/{name}/device")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            devs.push(Dev { class, name: format!("{model} {suffix}"), sys, bus: tran.to_uppercase(), maker: "(Standard disk drives)".into(), location: format!("/dev/{name}"),
                driver: if tran == "nvme" { "nvme".into() } else if ty == "rom" { "sr".into() } else { "sd".into() }, module: if tran == "nvme" { "nvme".into() } else if ty == "rom" { "sr_mod".into() } else { "sd_mod".into() },
                hwids: vec![format!("{}\\Disk{}", tran.to_uppercase(), model.replace(' ', "_"))], extra: vec![("Serial number".into(), s("serial")), ("Firmware revision".into(), s("rev")), ("Capacity".into(), s("size"))], ..Default::default() });
        }
    }
    // ---------------------------------------------------------------- monitors
    let mut monitors = 0;
    if let Ok(it) = std::fs::read_dir("/sys/class/drm") {
        let mut v: Vec<PathBuf> = it.flatten().map(|e| e.path()).filter(|p| p.file_name().unwrap().to_string_lossy().contains('-')).collect();
        v.sort();
        for p in v {
            if rd(&p.join("status")) != "connected" { continue; }
            let edid = std::fs::read(p.join("edid")).unwrap_or_default();
            let mfg = if edid.len() > 9 { let w = ((edid[8] as u16) << 8) | edid[9] as u16; format!("{}{}{}", (((w >> 10) & 31) as u8 + 64) as char, (((w >> 5) & 31) as u8 + 64) as char, ((w & 31) as u8 + 64) as char) } else { String::new() };
            let conn = p.file_name().unwrap().to_string_lossy().split_once('-').map(|x| x.1.to_string()).unwrap_or_default();
            devs.push(Dev { class: "Monitors", name: edid_name(&edid).unwrap_or_else(|| "Generic PnP Monitor".into()), sys: p.to_string_lossy().into(), bus: "DRM".into(), maker: if mfg.is_empty() { "(Standard monitor types)".into() } else { mfg.clone() },
                location: conn, driver: "drm".into(), module: "drm".into(), hwids: vec![format!("MONITOR\\{}{:02X}{:02X}", mfg, edid.get(11).copied().unwrap_or(0), edid.get(10).copied().unwrap_or(0))], ..Default::default() });
            monitors += 1;
        }
    }
    if monitors == 0 {
        for l in sh(&["xrandr", "--query"]).lines().filter(|l| l.contains(" connected")) {
            devs.push(Dev { class: "Monitors", name: "Generic PnP Monitor".into(), bus: "X11".into(), maker: "(Standard monitor types)".into(), location: l.split_whitespace().next().unwrap_or("").into(), driver: "xrandr".into(), ..Default::default() });
        }
    }
    // ---------------------------------------------------------------- batteries / AC
    if let Ok(it) = std::fs::read_dir("/sys/class/power_supply") {
        for e in it.flatten() {
            let p = e.path();
            let ty = rd(&p.join("type"));
            let model = rd(&p.join("model_name"));
            let (name, ok) = match ty.as_str() { "Battery" if rd(&p.join("scope")) != "Device" => (if model.is_empty() { "Microsoft ACPI-Compliant Control Method Battery".to_string() } else { format!("Microsoft ACPI-Compliant Control Method Battery ({model})") }, true), "Mains" => ("Microsoft AC Adapter".to_string(), true), _ => (String::new(), false) };
            if !ok { continue; }
            devs.push(Dev { class: "Batteries", name, sys: std::fs::canonicalize(&p).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(), bus: "ACPI".into(), maker: { let m = rd(&p.join("manufacturer")); if m.is_empty() { "Microsoft".into() } else { m } },
                location: e.file_name().to_string_lossy().into(), driver: "battery".into(), module: if ty == "Mains" { "ac".into() } else { "battery".into() }, hwids: vec![if ty == "Mains" { "ACPI\\ACPI0003".into() } else { "ACPI\\PNP0C0A".into() }], ..Default::default() });
        }
    }
    // ---------------------------------------------------------------- ACPI system devices
    if let Ok(it) = std::fs::read_dir("/sys/bus/acpi/devices") {
        let mut v: Vec<PathBuf> = it.flatten().map(|e| e.path()).collect();
        v.sort();
        for p in v {
            let hid = rd(&p.join("hid"));
            let Some(name) = acpi_name(&hid) else { continue };
            if matches!(hid.as_str(), "PNP0C0A" | "ACPI0003") { continue; } // listed under Batteries
            let status: u32 = rd(&p.join("status")).parse().unwrap_or(15);
            if status & 1 == 0 { continue; }
            let pp = std::fs::canonicalize(p.join("physical_node")).ok();
            let (driver, module) = pp.as_ref().map(|x| module_of(x)).unwrap_or_default();
            devs.push(Dev { class: "System devices", name: name.into(), sys: std::fs::canonicalize(&p).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(), bus: "ACPI".into(), maker: "(Standard system devices)".into(),
                location: rd(&p.join("path")), driver, module, hwids: vec![format!("ACPI\\{hid}"), format!("*{hid}")], ..Default::default() });
        }
    }
    // ---------------------------------------------------------------- serial ports
    if let Ok(it) = std::fs::read_dir("/sys/class/tty") {
        let mut com = 0;
        let mut v: Vec<PathBuf> = it.flatten().map(|e| e.path()).collect();
        v.sort();
        for p in v {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            if n.starts_with("ttyS") && rd(&p.join("type")) != "0" && p.join("device").exists() {
                com += 1;
                devs.push(Dev { class: "Ports (COM & LPT)", name: format!("Communications Port (COM{com})"), sys: p.to_string_lossy().into(), bus: "ACPI".into(), maker: "(Standard port types)".into(), location: format!("/dev/{n}"), driver: "serial".into(), module: "8250".into(), ..Default::default() });
            } else if n.starts_with("ttyUSB") || n.starts_with("ttyACM") {
                devs.push(Dev { class: "Ports (COM & LPT)", name: format!("USB Serial Port ({n})"), sys: p.to_string_lossy().into(), bus: "USB".into(), location: format!("/dev/{n}"), driver: link_name(&p.join("device/driver")), ..Default::default() });
            }
        }
    }
    // ---------------------------------------------------------------- hidden: virtual devices
    if hidden {
        if let Ok(it) = std::fs::read_dir("/sys/class/net") {
            for e in it.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if e.path().join("device").exists() || n == "lo" { continue; }
                let kind = rd(&e.path().join("uevent")).lines().find_map(|l| l.strip_prefix("DEVTYPE=")).unwrap_or("virtual").to_string();
                devs.push(Dev { class: "Network adapters", name: format!("Virtual {} Adapter ({n})", kind), sys: std::fs::canonicalize(e.path()).map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(), bus: "Virtual".into(), maker: "Linux".into(), hidden: true, ..Default::default() });
            }
        }
        for (name, drv) in [("Composite Bus Enumerator", "compbatt"), ("Microsoft System Management BIOS Driver", "dmi"), ("Volume Manager", "dm_mod"), ("UMBus Root Bus Enumerator", "uinput")] {
            devs.push(Dev { class: "System devices", name: name.into(), bus: "Root".into(), maker: "(Standard system devices)".into(), driver: drv.into(), module: drv.into(), hidden: true, ..Default::default() });
        }
    }
    devs
}

fn virtio_name(d: u32) -> String {
    match d { 0x1000 | 0x1041 => "Red Hat VirtIO Ethernet Adapter", 0x1001 | 0x1042 => "Red Hat VirtIO SCSI controller", 0x1003 | 0x1043 => "VirtIO Serial Driver", 0x1004 | 0x1048 => "Red Hat VirtIO SCSI pass-through controller",
        0x1005 | 0x1044 => "VirtIO RNG Device", 0x1002 | 0x1045 => "VirtIO Balloon Driver", 0x1050 => "Red Hat VirtIO GPU DOD controller", 0x1052 => "VirtIO Input Driver", 0x1053 => "VirtIO Socket Driver", _ => "VirtIO Device" }.into()
}

struct State {
    m: Rc<Mmc>,
    devs: RefCell<Vec<Dev>>,
    store: gtk::TreeStore,
    view: gtk::TreeView,
    by_connection: Cell<bool>,
    hidden: Cell<bool>,
    /// the result pane, kept while switching nodes in Computer Management
    sw: gtk::ScrolledWindow,
}

pub fn attach(m: &Rc<Mmc>, parent: Option<&gtk::TreeIter>) {
    m.add(parent, "dm", "Device Manager", &["aerode-devmgr", "computer"]);
    let st: Rc<RefCell<Option<Rc<State>>>> = Rc::new(RefCell::new(None));
    m.handle("dm", move |m, _| {
        let existing = st.borrow().clone();
        let s = match existing { Some(s) => s, None => { let s = new_state(m); *st.borrow_mut() = Some(s.clone()); s } };
        m.set_center(&s.sw);
        load(&s);
        actions(&s);
    });
}

pub fn window(app: &gtk::Application) -> gtk::Window {
    let m = Mmc::new(app, "Device Manager", &["aerode-devmgr", "computer"], false, false);
    let s = new_state(&m);
    m.keep(s.clone());
    // devmgmt.msc toolbar: Properties, Help | Scan for hardware changes, Update Driver Software, Disable, Uninstall
    { let w = Rc::downgrade(&s); m.tool(mmc::I_PROPS, "Properties", move || if let Some(s) = w.upgrade() { if let Some(d) = selected(&s) { properties(&s, &d); } }); }
    m.tool_sep();
    { let w = Rc::downgrade(&s); m.tool(&["aerode-dev-scan", "view-refresh"], "Scan for hardware changes", move || if let Some(s) = w.upgrade() { load(&s); }); }
    { let w = Rc::downgrade(&s); m.tool(&["aerode-dev-update", "system-software-update"], "Update Driver Software...", move || if let Some(s) = w.upgrade() { update_driver(&s); }); }
    { let w = Rc::downgrade(&s); m.tool(&["aerode-dev-disable", "action-unavailable"], "Disable", move || if let Some(s) = w.upgrade() { if let Some(d) = selected(&s) { disable(&s, &d); } }); }
    { let w = Rc::downgrade(&s); m.tool(&["aerode-dev-uninstall", "edit-delete"], "Uninstall", move || if let Some(s) = w.upgrade() { if let Some(d) = selected(&s) { uninstall(&s, &d); } }); }
    // View menu: Devices by type / by connection, Show hidden devices
    let by_type = gtk::RadioMenuItem::with_mnemonic("Devices by t_ype");
    let by_conn = gtk::RadioMenuItem::with_mnemonic_from_widget(&by_type, Some("Devices by co_nnection"));
    m.view_menu.append(&by_type);
    m.view_menu.append(&by_conn);
    for l in ["Resources by type", "Resources by connection"] { let mi = gtk::MenuItem::with_label(l); mi.set_sensitive(false); m.view_menu.append(&mi); }
    m.view_menu.append(&gtk::SeparatorMenuItem::new());
    let hid = gtk::CheckMenuItem::with_mnemonic("Show hidden de_vices");
    m.view_menu.append(&hid);
    m.view_menu.append(&gtk::SeparatorMenuItem::new());
    { let w = Rc::downgrade(&s); by_conn.connect_toggled(move |b| if let Some(s) = w.upgrade() { s.by_connection.set(b.is_active()); fill(&s); }); }
    { let w = Rc::downgrade(&s); hid.connect_toggled(move |b| if let Some(s) = w.upgrade() { s.hidden.set(b.is_active()); load(&s); }); }
    m.set_center(&s.sw);
    m.show("");
    load(&s);
    actions(&s);
    m.win.set_default_size(720, 560);
    m.win.resize(720, 560);
    m.win.clone()
}

fn new_state(m: &Rc<Mmc>) -> Rc<State> {
    // icon, name, index into devs (-1 = class/root)
    let store = gtk::TreeStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::I32]);
    let view = gtk::TreeView::with_model(&store);
    view.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let pr = gtk::CellRendererPixbuf::new();
    let tr = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &pr, false);
    TreeViewColumnExt::add_attribute(&col, &pr, "pixbuf", 0);
    TreeViewColumnExt::pack_start(&col, &tr, true);
    TreeViewColumnExt::add_attribute(&col, &tr, "text", 1);
    view.append_column(&col);
    let sw = scrolled(&view);
    let s = Rc::new(State { m: m.clone(), devs: RefCell::new(vec![]), store, view, by_connection: Cell::new(false), hidden: Cell::new(false), sw });
    { let w = Rc::downgrade(&s); s.view.connect_row_activated(move |tv, p, _| if let Some(s) = w.upgrade() {
        match selected(&s) { Some(d) => properties(&s, &d), None => { if tv.row_expanded(p) { tv.collapse_row(p); } else { tv.expand_row(p, false); } } }
    }); }
    { let w = Rc::downgrade(&s); s.view.selection().connect_changed(move |_| if let Some(s) = w.upgrade() { actions(&s); }); }
    { let w = Rc::downgrade(&s); s.view.connect_button_press_event(move |tv, ev| {
        if ev.button() == 3 { if let (Some(s), Some((Some(p), _, _, _))) = (w.upgrade(), tv.path_at_pos(ev.position().0 as i32, ev.position().1 as i32)) { tv.selection().select_path(&p); context_menu(&s, ev); return glib::Propagation::Stop; } }
        glib::Propagation::Proceed
    }); }
    s
}

fn load(s: &Rc<State>) {
    let hidden = s.hidden.get();
    let s2 = s.clone();
    s.m.status.set_text("Scanning for hardware changes…");
    bg(move || collect(hidden), move |devs| { *s2.devs.borrow_mut() = devs; fill(&s2); s2.m.status.set_text(""); });
}

fn dev_icon(d: &Dev) -> Option<Pixbuf> {
    let base = pix(class_icon(d.class), 16)?;
    if d.problem.is_empty() && !d.hidden { return Some(base); }
    // Win7 overlays a yellow "!" for problem devices and fades hidden ones
    let out = base.copy()?;
    if !d.problem.is_empty() {
        if let Some(w) = pix(&["dialog-warning"], 10) { w.composite(&out, 6, 6, 10, 10, 6.0, 6.0, 1.0, 1.0, gtk::gdk_pixbuf::InterpType::Bilinear, 255); }
    }
    if d.hidden { return Some(out.add_alpha(false, 0, 0, 0).map(|p| { let f = p.copy().unwrap_or(p.clone()); f.saturate_and_pixelate(&f, 0.5, false); f }).unwrap_or(out)); }
    Some(out)
}

fn fill(s: &Rc<State>) {
    s.store.clear();
    let devs = s.devs.borrow();
    let host = glib::host_name().to_string();
    let root = s.store.insert_with_values(None, None, &[(0, &pix(&["computer"], 16)), (1, &host), (2, &-1i32)]);
    if s.by_connection.get() {
        // nest by sysfs path: each device under the closest device whose path is a prefix of its own
        let pc = devs.iter().position(|d| d.class == "Computer");
        let pc_it = pc.map(|i| s.store.insert_with_values(Some(&root), None, &[(0, &dev_icon(&devs[i])), (1, &devs[i].name), (2, &(i as i32))])).unwrap_or(root.clone());
        let mut order: Vec<usize> = (0..devs.len()).filter(|i| Some(*i) != pc).collect();
        order.sort_by(|a, b| devs[*a].sys.len().cmp(&devs[*b].sys.len()).then(devs[*a].sys.cmp(&devs[*b].sys)));
        let mut iters: HashMap<usize, gtk::TreeIter> = HashMap::new();
        for &i in &order {
            let d = &devs[i];
            let parent = if d.sys.is_empty() { None } else {
                order.iter().filter(|&&j| j != i && !devs[j].sys.is_empty() && d.sys.starts_with(&format!("{}/", devs[j].sys)) && iters.contains_key(&j)).max_by_key(|&&j| devs[j].sys.len()).copied()
            };
            let pit = parent.and_then(|p| iters.get(&p).cloned()).unwrap_or(pc_it.clone());
            let it = s.store.insert_with_values(Some(&pit), None, &[(0, &dev_icon(d)), (1, &d.name), (2, &(i as i32))]);
            iters.insert(i, it);
        }
        s.view.expand_all();
    } else {
        let mut classes: Vec<&str> = devs.iter().map(|d| d.class).collect();
        classes.sort_by_key(|c| c.to_lowercase());
        classes.dedup();
        for c in classes {
            let cit = s.store.insert_with_values(Some(&root), None, &[(0, &pix(class_icon(c), 16)), (1, &c), (2, &-1i32)]);
            let mut list: Vec<(usize, &Dev)> = devs.iter().enumerate().filter(|(_, d)| d.class == c).collect();
            list.sort_by(|a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()));
            let mut problem = false;
            for (i, d) in list { s.store.insert_with_values(Some(&cit), None, &[(0, &dev_icon(d)), (1, &d.name), (2, &(i as i32))]); problem |= !d.problem.is_empty(); }
            // Win7 opens the classes that have a device with a problem
            if problem { if let Some(p) = s.store.path(&cit) { s.view.expand_to_path(&p); s.view.expand_row(&p, false); } }
        }
        if let Some(p) = s.store.path(&root) { s.view.expand_row(&p, false); }
    }
}

fn selected(s: &State) -> Option<Dev> {
    let (m, it) = s.view.selection().selected()?;
    let i: i32 = m.value(&it, 2).get().ok()?;
    if i < 0 { return None; }
    s.devs.borrow().get(i as usize).cloned()
}

fn actions(s: &Rc<State>) {
    if !s.m.actions_visible() && s.m.tree_visible() == false {
        // standalone devmgmt.msc: no Actions pane, but keep the Action menu filled
    }
    let w = Rc::downgrade(s);
    let g = move || w.upgrade();
    let (a, b) = (g.clone(), g.clone());
    let mut groups = vec![("Device Manager".to_string(), vec![
        act("Scan for hardware changes", &["aerode-dev-scan", "view-refresh"], move || if let Some(s) = a() { load(&s) }),
        act_off("Add legacy hardware"),
        act_off("Devices and Printers"),
    ])];
    let _ = b;
    if let Some(d) = selected(s) {
        let mk = |f: fn(&Rc<State>, &Dev)| { let (g, d) = (g.clone(), d.clone()); move || if let Some(s) = g() { f(&s, &d) } };
        groups.push((d.name.clone(), vec![
            act("Update Driver Software...", &["aerode-dev-update", "system-software-update"], mk(|s, _| update_driver(s))),
            if d.driver.is_empty() || d.sys.is_empty() || d.bus != "PCI" && d.bus != "USB" { act_off("Disable") } else { act("Disable", &["aerode-dev-disable", "action-unavailable"], mk(disable)) },
            if d.sys.is_empty() || d.bus != "PCI" && d.bus != "USB" { act_off("Uninstall") } else { act("Uninstall", &["aerode-dev-uninstall", "edit-delete"], mk(uninstall)) },
            act("Properties", mmc::I_PROPS, mk(properties)),
        ]));
    }
    s.m.set_actions(groups);
}

fn context_menu(s: &Rc<State>, ev: &gtk::gdk::EventButton) {
    let menu = gtk::Menu::new();
    let add = |l: &str, f: Option<Box<dyn Fn()>>| { let mi = gtk::MenuItem::with_label(l); match f { Some(f) => { mi.connect_activate(move |_| f()); } None => mi.set_sensitive(false) } menu.append(&mi); };
    match selected(s) {
        Some(d) => {
            let w = Rc::downgrade(s);
            { let w = w.clone(); add("Update Driver Software...", Some(Box::new(move || if let Some(s) = w.upgrade() { update_driver(&s) }))); }
            { let (w, d2) = (w.clone(), d.clone()); add("Disable", if d.bus == "PCI" || d.bus == "USB" { Some(Box::new(move || if let Some(s) = w.upgrade() { disable(&s, &d2) })) } else { None }); }
            { let (w, d2) = (w.clone(), d.clone()); add("Uninstall", if d.bus == "PCI" || d.bus == "USB" { Some(Box::new(move || if let Some(s) = w.upgrade() { uninstall(&s, &d2) })) } else { None }); }
            menu.append(&gtk::SeparatorMenuItem::new());
            { let w = w.clone(); add("Scan for hardware changes", Some(Box::new(move || if let Some(s) = w.upgrade() { load(&s) }))); }
            menu.append(&gtk::SeparatorMenuItem::new());
            let (w, d2) = (w.clone(), d.clone());
            let mi = gtk::MenuItem::new();
            let l = gtk::Label::new(None);
            l.set_markup("<b>Properties</b>");
            l.set_xalign(0.0);
            mi.add(&l);
            mi.connect_activate(move |_| if let Some(s) = w.upgrade() { properties(&s, &d2) });
            menu.append(&mi);
        }
        None => {
            let w = Rc::downgrade(s);
            add("Scan for hardware changes", Some(Box::new(move || if let Some(s) = w.upgrade() { load(&s) })));
            add("Add legacy hardware", None);
            menu.append(&gtk::SeparatorMenuItem::new());
            add("Properties", None);
        }
    }
    menu.show_all();
    menu.popup_at_pointer(Some(ev));
}

fn status_text(d: &Dev) -> String {
    if !d.problem.is_empty() { format!("{}\n\nTo find a driver for this device, click Update Driver.", d.problem) } else { "This device is working properly.".into() }
}

/// The Win7 device Properties sheet.
fn properties(s: &Rc<State>, d: &Dev) {
    let dlg = gtk::Dialog::with_buttons(Some(&format!("{} Properties", d.name)), Some(&s.m.win), gtk::DialogFlags::DESTROY_WITH_PARENT, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    dlg.set_default_size(420, 470);
    let nb = gtk::Notebook::new();
    let header = |d: &Dev| {
        let h = hbox(12);
        h.pack_start(&gtk::Image::from_pixbuf(pix(class_icon(d.class), 32).as_ref()), false, false, 0);
        let l = gtk::Label::new(Some(&d.name));
        l.set_xalign(0.0);
        l.set_line_wrap(true);
        l.set_max_width_chars(40);
        h.pack_start(&l, true, true, 0);
        h
    };
    // General
    let g = vbox(10);
    g.set_border_width(12);
    g.pack_start(&header(d), false, false, 0);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(14);
    grid.set_margin_start(44);
    for (i, (k, v)) in [("Device type:", d.class.to_string()), ("Manufacturer:", d.maker.clone()), ("Location:", if d.location.is_empty() { "on ".to_string() + &d.bus } else { d.location.clone() })].iter().enumerate() {
        let a = gtk::Label::new(Some(k)); a.set_xalign(0.0); grid.attach(&a, 0, i as i32, 1, 1);
        let b = gtk::Label::new(Some(v)); b.set_xalign(0.0); b.set_ellipsize(gtk::pango::EllipsizeMode::End); b.set_selectable(true); grid.attach(&b, 1, i as i32, 1, 1);
    }
    g.pack_start(&grid, false, false, 0);
    let fr = gtk::Frame::new(Some("Device status"));
    let st = text_box(&status_text(d));
    let sw = scrolled(&st);
    sw.set_min_content_height(110);
    sw.set_border_width(6);
    fr.add(&sw);
    g.pack_start(&fr, true, true, 0);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    // Driver
    let dr = vbox(10);
    dr.set_border_width(12);
    dr.pack_start(&header(d), false, false, 0);
    let dg = gtk::Grid::new();
    dg.set_row_spacing(6);
    dg.set_column_spacing(14);
    dg.set_margin_start(44);
    let vals: Vec<gtk::Label> = ["Driver Provider:", "Driver Date:", "Driver Version:", "Digital Signer:"].iter().enumerate().map(|(i, k)| {
        let a = gtk::Label::new(Some(k)); a.set_xalign(0.0); dg.attach(&a, 0, i as i32, 1, 1);
        let b = gtk::Label::new(Some(if d.module.is_empty() { "Not available" } else { "…" })); b.set_xalign(0.0); b.set_selectable(true); b.set_ellipsize(gtk::pango::EllipsizeMode::End); dg.attach(&b, 1, i as i32, 1, 1); b
    }).collect();
    dr.pack_start(&dg, false, false, 0);
    let bgrid = gtk::Grid::new();
    bgrid.set_row_spacing(8);
    bgrid.set_column_spacing(10);
    let rows: [(&str, &str); 5] = [("Driver Details", "To view details about the driver files."), ("Update Driver...", "To update the driver software for this device."), ("Roll Back Driver", "If the device fails after updating the driver, roll back to the previously installed driver."), ("Disable", "Disables the selected device."), ("Uninstall", "To uninstall the driver (Advanced).")];
    let files = Rc::new(RefCell::new(String::new()));
    for (i, (b, t)) in rows.iter().enumerate() {
        let btn = gtk::Button::with_label(b);
        btn.set_size_request(120, -1);
        bgrid.attach(&btn, 0, i as i32, 1, 1);
        let l = label(t, "");
        l.set_max_width_chars(34);
        bgrid.attach(&l, 1, i as i32, 1, 1);
        let (sw, dd) = (Rc::downgrade(s), d.clone());
        match i {
            0 => { let (files, dd) = (files.clone(), d.clone()); let dlg2 = dlg.clone(); btn.set_sensitive(!d.module.is_empty()); btn.connect_clicked(move |_| driver_details(&dlg2, &dd, &files.borrow())); }
            1 => { btn.connect_clicked(move |_| if let Some(s) = sw.upgrade() { update_driver(&s) }); }
            2 => btn.set_sensitive(false),
            3 => { btn.set_sensitive(d.bus == "PCI" || d.bus == "USB"); btn.connect_clicked(move |_| if let Some(s) = sw.upgrade() { disable(&s, &dd) }); }
            _ => { btn.set_sensitive(d.bus == "PCI" || d.bus == "USB"); btn.connect_clicked(move |_| if let Some(s) = sw.upgrade() { uninstall(&s, &dd) }); }
        }
    }
    dr.pack_start(&bgrid, false, false, 0);
    nb.append_page(&dr, Some(&gtk::Label::new(Some("Driver"))));
    if !d.module.is_empty() {
        let module = d.module.clone();
        let files = files.clone();
        bg(move || {
            let info = sh(&["modinfo", &module]);
            let kver = sh(&["uname", "-r"]).trim().to_string();
            let kdate = sh(&["uname", "-v"]).trim().to_string();
            (info, kver, kdate)
        }, move |(info, kver, kdate)| {
            let get = |k: &str| info.lines().filter_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).collect::<Vec<_>>();
            let author = get("author:").first().cloned().unwrap_or_else(|| "The Linux kernel developers".into());
            let ver = get("version:").first().cloned().unwrap_or_else(|| kver.clone());
            let signer = get("signer:").first().cloned().unwrap_or_else(|| if info.is_empty() { "Not available".into() } else { "Not digitally signed".into() });
            let file = get("filename:").first().cloned().unwrap_or_default();
            // the kernel build date ("#1 SMP PREEMPT_DYNAMIC Fri, 24 Oct 2025 …")
            let date = kdate.split_whitespace().collect::<Vec<_>>().windows(4).find_map(|w| chrono::NaiveDate::parse_from_str(&format!("{} {} {}", w[1], w[2], w[3]), "%d %b %Y").ok().or_else(|| chrono::NaiveDate::parse_from_str(&format!("{} {} {}", w[1], w[2], w[3].trim_end_matches(',')), "%b %d %Y").ok()))
                .map(|d| d.format("%-m/%-d/%Y").to_string()).unwrap_or_default();
            let vals_: [String; 4] = [author, date, ver, signer];
            for (l, v) in vals.iter().zip(vals_.iter()) { l.set_text(if v.is_empty() { "Not available" } else { v }); }
            *files.borrow_mut() = if file.is_empty() || file == "(builtin)" { format!("(built into the kernel {kver})\n{info}") } else { format!("{file}\n{info}") };
        });
    }
    // Details
    let det = vbox(8);
    det.set_border_width(12);
    det.pack_start(&header(d), false, false, 0);
    det.pack_start(&label("Property", ""), false, false, 0);
    let combo = gtk::ComboBoxText::new();
    let props: Vec<(String, Vec<String>)> = {
        let mut p = vec![
            ("Device description".to_string(), vec![d.name.clone()]),
            ("Hardware Ids".into(), d.hwids.clone()),
            ("Compatible Ids".into(), d.compat.clone()),
            ("Device class".into(), vec![d.class.to_string()]),
            ("Driver".into(), vec![d.driver.clone()]),
            ("Kernel module".into(), vec![d.module.clone()]),
            ("Manufacturer".into(), vec![d.maker.clone()]),
            ("Location information".into(), vec![d.location.clone()]),
            ("Device instance path".into(), vec![d.sys.trim_start_matches("/sys").to_string()]),
            ("Physical Device Object name".into(), vec![d.sys.clone()]),
            ("Enumerator".into(), vec![d.bus.clone()]),
        ];
        for (k, v) in &d.extra { if !v.is_empty() { p.push((k.clone(), vec![v.clone()])); } }
        p.retain(|(_, v)| v.iter().any(|x| !x.is_empty()));
        p
    };
    for (k, _) in &props { combo.append_text(k); }
    det.pack_start(&combo, false, false, 0);
    det.pack_start(&label("Value", ""), false, false, 0);
    let vs = gtk::ListStore::new(&[glib::Type::STRING]);
    let vtv = gtk::TreeView::with_model(&vs);
    vtv.set_headers_visible(false);
    columns(&vtv, &[("Value", 0, 0)]);
    det.pack_start(&scrolled(&vtv), true, true, 0);
    { let (vs, props) = (vs.clone(), props.clone()); combo.connect_changed(move |c| { vs.clear(); if let Some(i) = c.active() { for v in &props[i as usize].1 { vs.insert_with_values(None, &[(0, v)]); } } }); }
    combo.set_active(Some(0));
    nb.append_page(&det, Some(&gtk::Label::new(Some("Details"))));
    // Resources
    if !d.resources.is_empty() {
        let r = vbox(8);
        r.set_border_width(12);
        r.pack_start(&header(d), false, false, 0);
        r.pack_start(&label("Resource settings:", ""), false, false, 0);
        let rs = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
        for (k, v) in &d.resources { rs.insert_with_values(None, &[(0, k), (1, v)]); }
        let rtv = gtk::TreeView::with_model(&rs);
        columns(&rtv, &[("Resource type", 0, 130), ("Setting", 1, 0)]);
        r.pack_start(&scrolled(&rtv), true, true, 0);
        let set = gtk::CheckButton::with_label("Use automatic settings");
        set.set_active(true);
        set.set_sensitive(false);
        r.pack_start(&set, false, false, 0);
        r.pack_start(&label("Conflicting device list:", ""), false, false, 0);
        let c = text_box("No conflicts.");
        let csw = scrolled(&c);
        csw.set_min_content_height(50);
        r.pack_start(&csw, false, false, 0);
        nb.append_page(&r, Some(&gtk::Label::new(Some("Resources"))));
    }
    dlg.content_area().pack_start(&nb, true, true, 0);
    dlg.connect_response(|d, _| d.close());
    dlg.show_all();
}

fn driver_details(parent: &gtk::Dialog, d: &Dev, info: &str) {
    let dlg = gtk::Dialog::with_buttons(Some("Driver File Details"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok)]);
    dlg.set_default_size(420, 380);
    let c = dlg.content_area();
    c.set_border_width(10);
    c.set_spacing(6);
    let h = hbox(10);
    h.pack_start(&gtk::Image::from_pixbuf(pix(class_icon(d.class), 32).as_ref()), false, false, 0);
    h.pack_start(&label(&d.name, ""), true, true, 0);
    c.pack_start(&h, false, false, 0);
    c.pack_start(&label("Driver files:", ""), false, false, 0);
    let first = info.lines().next().unwrap_or("").to_string();
    let fs = text_box(&first);
    let fsw = scrolled(&fs);
    fsw.set_min_content_height(50);
    c.pack_start(&fsw, false, false, 0);
    let get = |k: &str| info.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_else(|| "Not available".into());
    c.pack_start(&kvs(&[("Provider:", get("author:")), ("File version:", get("version:")), ("Copyright:", get("license:")), ("Digital Signer:", get("signer:"))]), false, false, 0);
    let all = text_box(info.lines().skip(1).collect::<Vec<_>>().join("\n").as_str());
    all.set_monospace(true);
    c.pack_start(&scrolled(&all), true, true, 0);
    dlg.show_all();
    dlg.run();
    dlg.close();
}

fn kvs(rows: &[(&str, String)]) -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(4);
    g.set_column_spacing(12);
    for (i, (k, v)) in rows.iter().enumerate() {
        let a = gtk::Label::new(Some(k)); a.set_xalign(0.0); g.attach(&a, 0, i as i32, 1, 1);
        let b = gtk::Label::new(Some(v)); b.set_xalign(0.0); b.set_selectable(true); b.set_ellipsize(gtk::pango::EllipsizeMode::End); g.attach(&b, 1, i as i32, 1, 1);
    }
    g
}

fn update_driver(s: &Rc<State>) {
    let d = gtk::Dialog::with_buttons(Some("Update Driver Software"), Some(&s.m.win), gtk::DialogFlags::MODAL, &[("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(520, 300);
    let c = d.content_area();
    c.set_border_width(16);
    c.set_spacing(12);
    let t = gtk::Label::new(None);
    t.set_markup("<span size='large' foreground='#1e3287'>How do you want to search for driver software?</span>");
    t.set_xalign(0.0);
    c.pack_start(&t, false, false, 0);
    let b1 = gtk::Button::new();
    let v1 = vbox(2);
    v1.pack_start(&bold("➜  Search automatically for updated driver software"), false, false, 0);
    v1.pack_start(&label("Linux drivers come with the kernel and firmware packages. Windows will check Windows Update for newer kernel and linux-firmware packages.", "dim"), false, false, 0);
    b1.add(&v1);
    b1.set_relief(gtk::ReliefStyle::None);
    let b2 = gtk::Button::new();
    let v2 = vbox(2);
    v2.pack_start(&bold("➜  Browse my computer for driver software"), false, false, 0);
    v2.pack_start(&label("Locate and load a kernel module (driver) manually.", "dim"), false, false, 0);
    b2.add(&v2);
    b2.set_relief(gtk::ReliefStyle::None);
    c.pack_start(&b1, false, false, 0);
    c.pack_start(&b2, false, false, 0);
    { let d = d.clone(); b1.connect_clicked(move |_| { spawn_tool("update"); d.close(); }); }
    { let d = d.clone(); b2.connect_clicked(move |_| {
        let fc = gtk::FileChooserNative::new(Some("Browse For Driver Software"), Some(&d), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
        let f = gtk::FileFilter::new(); f.set_name(Some("Kernel modules (*.ko, *.ko.zst, *.ko.xz)")); for p in ["*.ko", "*.ko.zst", "*.ko.xz", "*.ko.gz"] { f.add_pattern(p); } fc.add_filter(f);
        if fc.run() == gtk::ResponseType::Accept {
            if let Some(p) = fc.filename() {
                let p = p.to_string_lossy().to_string();
                if confirm(Some(d.upcast_ref()), "Update Driver Software", "Load this driver into the running kernel?", &p, "Load") {
                    elevate(&["insmod", &p], |ok, out| { if report("The driver could not be loaded.", ok, &out) { inform(None, "Update Driver Software", "Windows has successfully loaded the driver.", ""); } });
                }
            }
        }
        d.close();
    }); }
    d.show_all();
    d.run();
    d.close();
}

fn disable(s: &Rc<State>, d: &Dev) {
    if d.driver.is_empty() || d.sys.is_empty() { return; }
    if !confirm(Some(&s.m.win), &d.name, "Disabling this device will cause it to stop functioning.", "Do you really want to disable it?", "Yes") { return; }
    let id = Path::new(&d.sys).file_name().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
    let bus = if d.bus == "PCI" { "pci" } else { "usb" };
    let script = format!("echo {} > /sys/bus/{bus}/drivers/{}/unbind", q(&id), d.driver);
    let s2 = s.clone();
    elevate_sh(&script, move |ok, out| { if report("The device could not be disabled.", ok, &out) { load(&s2); } });
}

fn uninstall(s: &Rc<State>, d: &Dev) {
    if !confirm(Some(&s.m.win), "Confirm Device Uninstall", "Warning: You are about to uninstall this device from your system.", &format!("{}\n\nThe device is removed until the next restart or hardware scan.", d.name), "OK") { return; }
    let script = if d.bus == "PCI" { format!("echo 1 > {}/remove", q(&d.sys)) } else { format!("echo 0 > {}/authorized", q(&d.sys)) };
    let s2 = s.clone();
    elevate_sh(&script, move |ok, out| { if report("The device could not be uninstalled.", ok, &out) { load(&s2); } });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        assert_eq!(pci_name("NVIDIA Corporation", "TU116 [GeForce GTX 1660 SUPER]"), "NVIDIA GeForce GTX 1660 SUPER");
        assert_eq!(pci_name("Intel Corporation", "Ethernet Connection (2) I219-V"), "Intel Ethernet Connection (2) I219-V");
        assert_eq!(pci_class(0x030000, true), "Display adapters");
        assert_eq!(pci_class(0x010601, true), "IDE ATA/ATAPI controllers");
        assert_eq!(pci_class(0x0c0330, true), "Universal Serial Bus controllers");
        assert_eq!(pci_class(0x0c0500, false), "System devices");
        let mut e = vec![0u8; 128];
        e[54 + 3] = 0xfc;
        e[59..59 + 8].copy_from_slice(b"DELL U24");
        e[67] = 0x0a;
        assert_eq!(edid_name(&e).as_deref(), Some("DELL U24"));
    }
    #[test]
    fn collect_runs() {
        let d = collect(false);
        assert!(d.iter().any(|d| d.class == "Computer"));
        assert!(d.iter().any(|d| d.class == "Processors"));
    }
}
