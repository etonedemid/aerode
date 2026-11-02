//! Devices and Printers: Win7's tiles in groups (Devices, Printers and Faxes, Unspecified) for
//! this computer (DMI), the monitors (xrandr + EDID names), keyboards and mice
//! (/proc/bus/input/devices), audio outputs (PulseAudio/PipeWire), webcams and other USB devices,
//! USB drives (lsblk), paired Bluetooth devices (bluetoothctl) and printers (CUPS: lpstat, the
//! default one with the green check). Each kind has Win7's context menu, a details pane and a
//! Properties sheet; Add a device (Bluetooth scan and pair), Add a printer, See what's printing.
use super::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    /// computer | monitor | keyboard | mouse | audio | camera | storage | bluetooth | printer | usb
    pub kind: String,
    pub name: String,
    pub maker: String,
    pub model: String,
    pub icon: Vec<&'static str>,
    /// printer name, Bluetooth address, block device, sink name…
    pub id: String,
    pub default: bool,
    pub status: String,
    pub connected: bool,
    pub location: String,
    pub mount: String,
}

impl Device {
    fn group(&self) -> usize { match self.kind.as_str() { "printer" => 1, "usb" => 2, _ => 0 } }
    fn category(&self) -> &'static str {
        match self.kind.as_str() {
            "computer" => "Computer", "monitor" => "Display", "keyboard" => "Keyboard", "mouse" => "Mouse", "audio" => "Audio device",
            "camera" => "Imaging device", "storage" => "Storage device", "bluetooth" => "Bluetooth device", "printer" => "Printer", _ => "Unspecified",
        }
    }
}

fn read(p: &str) -> String { std::fs::read_to_string(p).unwrap_or_default().trim().to_string() }

/// The 3-letter PNP vendor id in an EDID → a manufacturer name for the common ones.
fn pnp_vendor(code: &str) -> String {
    match code {
        "SAM" => "Samsung", "DEL" => "Dell", "GSM" => "LG Electronics", "ACR" => "Acer", "AUS" => "ASUS", "BNQ" => "BenQ", "HWP" => "HP", "LEN" => "Lenovo",
        "PHL" => "Philips", "VSC" => "ViewSonic", "AOC" => "AOC", "NEC" => "NEC", "SNY" => "Sony", "APP" => "Apple", "IVM" => "Iiyama", "MSI" | "MST" => "MSI",
        "GBT" => "Gigabyte", "BOE" => "BOE", "AUO" => "AU Optronics", "CMN" => "Chimei Innolux", "SHP" => "Sharp", "LGD" => "LG Display", "EIZ" => "EIZO", "HPN" => "HP",
        c => return c.to_string(),
    }.to_string()
}

/// (manufacturer, model name) from an EDID blob.
pub fn edid_names(e: &[u8]) -> (String, String) {
    if e.len() < 128 { return (String::new(), String::new()); }
    let v = ((e[8] as u16) << 8) | e[9] as u16;
    let code: String = [(v >> 10) & 31, (v >> 5) & 31, v & 31].iter().map(|c| (b'A' + *c as u8 - 1) as char).collect();
    let mut name = String::new();
    for d in 0..4 {
        let o = 54 + d * 18;
        if e[o] == 0 && e[o + 1] == 0 && e[o + 3] == 0xFC {
            name = e[o + 5..o + 18].iter().take_while(|b| **b != 0x0a).map(|b| *b as char).collect::<String>().trim().to_string();
        }
    }
    (pnp_vendor(&code), name)
}

fn monitors() -> Vec<Device> {
    let mut v = vec![];
    let out = sh(&["xrandr", "--verbose"]);
    let (mut cur, mut edid, mut in_edid) = (None::<String>, String::new(), false);
    let flush = |cur: &mut Option<String>, edid: &mut String, v: &mut Vec<Device>| {
        if let Some(o) = cur.take() {
            let bytes: Vec<u8> = (0..edid.len() / 2).filter_map(|i| u8::from_str_radix(&edid[i * 2..i * 2 + 2], 16).ok()).collect();
            let (maker, model) = edid_names(&bytes);
            v.push(Device { kind: "monitor".into(), name: if model.is_empty() { "Generic PnP Monitor".into() } else { model.clone() }, maker,
                model: if model.is_empty() { "Generic PnP Monitor".into() } else { model }, icon: vec!["video-display", "aerode-dev-monitor", "preferences-desktop-display"],
                id: o.clone(), location: format!("Connected to {o}"), status: "This device is working properly.".into(), connected: true, ..Default::default() });
        }
        edid.clear();
    };
    for l in out.lines() {
        if !l.starts_with(' ') && !l.starts_with('\t') {
            in_edid = false;
            flush(&mut cur, &mut edid, &mut v);
            if l.contains(" connected") { cur = l.split_whitespace().next().map(|s| s.to_string()); }
        } else if l.trim() == "EDID:" { in_edid = true; }
        else if in_edid && l.trim().chars().all(|c| c.is_ascii_hexdigit()) && !l.trim().is_empty() { edid.push_str(l.trim()); }
        else { in_edid = false; }
    }
    flush(&mut cur, &mut edid, &mut v);
    v
}

fn inputs() -> Vec<Device> {
    let mut v: Vec<Device> = vec![];
    for block in std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default().split("\n\n") {
        let get = |p: &str| block.lines().find_map(|l| l.strip_prefix(p)).unwrap_or("").trim().to_string();
        let name = get("N: Name=").trim_matches('"').to_string();
        let h = get("H: Handlers=");
        let sysfs = get("S: Sysfs=");
        if name.is_empty() || ["Button", "Video Bus", "PC Speaker", "Lid Switch", "HDA ", "HDMI", "Headphone", "Mic", "xtest", "XTEST", "WMI", "Consumer Control", "System Control", "Wireless hotkeys", "ACPI"].iter().any(|s| name.contains(s)) { continue; }
        let usb = sysfs.contains("/usb");
        let bt = get("P: Phys=").contains(':') && get("B: EV=").is_empty() == false && sysfs.contains("bluetooth");
        let (kind, icon): (&str, Vec<&'static str>) = if h.contains("mouse") {
            ("mouse", if name.to_lowercase().contains("touchpad") { vec!["input-touchpad", "input-mouse"] } else { vec!["input-mouse", "aerode-dev-mouse"] })
        } else if h.contains("kbd") && (h.contains("sysrq") || h.contains("leds")) { ("keyboard", vec!["input-keyboard", "preferences-desktop-keyboard"]) } else { continue };
        // one tile per physical device (a mouse often brings a second "Keyboard" node)
        let base = name.trim_end_matches(" Keyboard").trim_end_matches(" Mouse").trim_end_matches(" Consumer Control").to_string();
        if v.iter().any(|d| d.name.trim_end_matches(" Keyboard").trim_end_matches(" Mouse") == base && (d.kind == kind || usb)) { continue; }
        let words: Vec<&str> = name.split_whitespace().collect();
        v.push(Device { kind: kind.into(), name: name.clone(), maker: if usb || bt { words.first().map(|s| s.to_string()).unwrap_or_default() } else { "(Standard system devices)".into() },
            model: name.clone(), icon, id: sysfs, location: if bt { "Bluetooth".into() } else if usb { "USB".into() } else { "Built-in (PS/2 or I2C)".into() },
            status: "This device is working properly.".into(), connected: true, ..Default::default() });
    }
    v
}

fn audio() -> Vec<Device> {
    aerode::sys::sinks().into_iter().filter(|(n, _, _)| !n.contains(".monitor")).map(|(n, d, def)| {
        let l = d.to_lowercase();
        let icon = if l.contains("headphone") || l.contains("headset") { vec!["audio-headphones", "audio-headset", "audio-speakers"] } else if l.contains("hdmi") || l.contains("displayport") { vec!["video-display", "audio-speakers"] } else { vec!["audio-speakers", "audio-card"] };
        Device { kind: "audio".into(), name: d.clone(), model: d, maker: String::new(), icon, id: n, default: def, status: "This device is working properly.".into(), connected: true, ..Default::default() }
    }).collect()
}

fn printers() -> Vec<Device> {
    let def = sh(&["lpstat", "-d"]).split(':').nth(1).map(|s| s.trim().to_string()).unwrap_or_default();
    let jobs = sh(&["lpstat", "-o"]);
    let mut v = vec![];
    for l in sh(&["lpstat", "-p"]).lines() {
        let Some(rest) = l.strip_prefix("printer ") else { continue };
        let name = rest.split_whitespace().next().unwrap_or("").to_string();
        let n = jobs.lines().filter(|j| j.starts_with(&format!("{name}-"))).count();
        let state = if rest.contains("disabled") { "Offline" } else if rest.contains("printing") { "Printing" } else { "Ready" };
        let opts = sh(&["lpoptions", "-p", &name]);
        let opt = |k: &str| opts.split(&format!(" {k}=")).nth(1).map(|s| if let Some(q) = s.strip_prefix('\'') { q.split('\'').next().unwrap_or("").to_string() } else { s.split(' ').next().unwrap_or("").to_string() }).unwrap_or_default();
        let model = opt("printer-make-and-model").split(',').next().unwrap_or("").trim().to_string();
        // the printer's description is the name Win7 would show ("HP LaserJet P1102"), else the queue name
        let shown = { let i = opt("printer-info"); if i.is_empty() { name.replace('_', " ") } else { i } };
        let fax = name.to_lowercase().contains("fax");
        v.push(Device { kind: "printer".into(), name: shown, maker: model.split_whitespace().next().unwrap_or("").to_string(), model, id: name.clone(), default: name == def,
            icon: if fax { vec!["printer-network", "printer"] } else { vec!["printer", "preferences-devices-printer"] }, status: format!("{n} document(s) in queue"), location: state.into(), connected: state != "Offline", ..Default::default() });
    }
    v
}

fn bluetooth() -> Vec<Device> {
    if !util::which("bluetoothctl") { return vec![]; }
    let mut v = vec![];
    let mut list = sh(&["bluetoothctl", "devices", "Paired"]);
    if list.trim().is_empty() { list = sh(&["bluetoothctl", "paired-devices"]); }
    for l in list.lines() {
        let mut it = l.splitn(3, ' ');
        let (Some("Device"), Some(mac), Some(name)) = (it.next(), it.next(), it.next()) else { continue };
        let info = sh(&["bluetoothctl", "info", mac]);
        let get = |k: &str| info.lines().find_map(|l| l.trim().strip_prefix(k)).unwrap_or("").trim().to_string();
        let icon: Vec<&'static str> = match get("Icon:").as_str() {
            "audio-headset" | "audio-headphones" => vec!["audio-headset", "audio-headphones", "bluetooth"], "audio-card" => vec!["audio-speakers", "bluetooth"],
            "input-mouse" => vec!["input-mouse", "bluetooth"], "input-keyboard" => vec!["input-keyboard", "bluetooth"], "phone" => vec!["phone", "bluetooth"],
            "input-gaming" => vec!["input-gaming", "bluetooth"], _ => vec!["bluetooth"],
        };
        let connected = get("Connected:") == "yes";
        v.push(Device { kind: "bluetooth".into(), name: name.to_string(), model: name.to_string(), maker: String::new(), icon, id: mac.to_string(), connected,
            status: if connected { "Connected".into() } else { "Not connected".into() }, location: "Bluetooth".into(), ..Default::default() });
    }
    v
}

fn usb_storage() -> Vec<Device> {
    let out = sh(&["lsblk", "-P", "-o", "NAME,TRAN,VENDOR,MODEL,HOTPLUG,SIZE,TYPE,MOUNTPOINT"]);
    let mut v: Vec<Device> = vec![];
    let field = |l: &str, k: &str| l.split(&format!("{k}=\"")).nth(1).and_then(|r| r.split('"').next()).unwrap_or("").trim().to_string();
    for l in out.lines() {
        let (name, ty) = (field(l, "NAME"), field(l, "TYPE"));
        if ty == "disk" && (field(l, "TRAN") == "usb" || (field(l, "HOTPLUG") == "1" && !name.starts_with("sr"))) {
            let model = format!("{} {}", field(l, "VENDOR"), field(l, "MODEL")).trim().to_string();
            v.push(Device { kind: "storage".into(), name: if model.is_empty() { "USB Drive".into() } else { model.clone() }, maker: field(l, "VENDOR"), model, id: format!("/dev/{name}"),
                icon: vec!["drive-removable-media-usb", "drive-removable-media", "media-flash"], location: format!("{} ({})", field(l, "SIZE"), if field(l, "TRAN") == "usb" { "USB" } else { "removable" }),
                status: "This device is working properly.".into(), connected: true, ..Default::default() });
        } else if ty == "part" {
            let mp = field(l, "MOUNTPOINT");
            if let Some(d) = v.iter_mut().rev().find(|d| name.starts_with(d.id.trim_start_matches("/dev/"))) { if d.mount.is_empty() && !mp.is_empty() { d.mount = mp; } }
        }
    }
    v
}

/// USB devices the other groups don't cover (webcams go to Devices, the rest to Unspecified).
fn usb_other() -> Vec<Device> {
    let mut v = vec![];
    let Ok(rd) = std::fs::read_dir("/sys/bus/usb/devices") else { return v };
    for e in rd.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        if n.contains(':') || n.starts_with("usb") { continue; }
        let product = read(&format!("{}/product", p.display()));
        if product.is_empty() { continue; }
        // interface classes: 0e video, 03 HID, 08 storage, 07 printer, 09 hub, e0 wireless (Bluetooth), 01 audio
        let classes: Vec<String> = std::fs::read_dir(&p).map(|rd| rd.flatten().filter(|i| i.file_name().to_string_lossy().contains(':')).map(|i| read(&format!("{}/bInterfaceClass", i.path().display()))).collect()).unwrap_or_default();
        if classes.iter().any(|c| matches!(c.as_str(), "03" | "08" | "07" | "09" | "e0" | "01")) || read(&format!("{}/bDeviceClass", p.display())) == "09" { continue; }
        let camera = classes.iter().any(|c| c == "0e");
        v.push(Device { kind: if camera { "camera".into() } else { "usb".into() }, name: product.clone(), model: product, maker: read(&format!("{}/manufacturer", p.display())),
            icon: if camera { vec!["camera-web", "camera-video"] } else { vec!["aerode-dev-usb", "drive-removable-media-usb", "application-x-executable"] },
            id: n, location: "USB".into(), status: "This device is working properly.".into(), connected: true, ..Default::default() });
    }
    v
}

fn computer() -> Device {
    let vendor = read("/sys/class/dmi/id/sys_vendor");
    let product = read("/sys/class/dmi/id/product_name");
    let chassis: u32 = read("/sys/class/dmi/id/chassis_type").parse().unwrap_or(3);
    let laptop = matches!(chassis, 8 | 9 | 10 | 14 | 30 | 31 | 32);
    let clean = |s: &str| if s.is_empty() || s.contains("To Be Filled") || s.contains("System Product") || s.contains("Default string") { String::new() } else { s.to_string() };
    Device { kind: "computer".into(), name: glib::host_name().to_string().to_uppercase(), maker: clean(&vendor), model: clean(&product),
        icon: if laptop { vec!["computer-laptop", "computer"] } else { vec!["computer", "user-desktop-computer"] }, status: "This device is working properly.".into(), connected: true, ..Default::default() }
}

/// Everything shown (worker thread).
pub fn all() -> Vec<Device> {
    let mut v = vec![computer()];
    v.extend(monitors());
    v.extend(inputs());
    v.extend(audio());
    v.extend(usb_storage());
    v.extend(bluetooth());
    v.extend(usb_other());
    v.extend(printers());
    v
}

// ------------------------------------------------------------------------------- page

struct Ui { cp: Rc<Cp>, groups: gtk::Box, details: gtk::Box, dynamic: gtk::Box, devs: RefCell<Vec<Device>>, tiles: RefCell<Vec<gtk::Button>>, sel: Cell<Option<usize>> }

impl Cp {
    pub(crate) fn page_devices(self: &Rc<Self>) -> gtk::Widget {
        css();
        let v = vbox(0);
        let frame = vbox(0);
        frame.style_context().add_class("dp-view");
        let bar = hbox(2);
        bar.style_context().add_class("cp-cmdbar");
        let add_dev = cmd_button("Add a device");
        let add_pr = cmd_button("Add a printer");
        bar.pack_start(&add_dev, false, false, 0);
        bar.pack_start(&add_pr, false, false, 0);
        let dynamic = hbox(2);
        bar.pack_start(&dynamic, false, false, 0);
        let help = cmd_button("");
        help.set_image(Some(&img(&["help-browser", "help-contents"], 16)));
        help.set_tooltip_text(Some("Get help"));
        help.connect_clicked(|_| { util::open_uri("https://wiki.archlinux.org/title/CUPS"); });
        bar.pack_end(&help, false, false, 0);
        frame.pack_start(&clip(&bar), false, false, 0);
        let groups = vbox(4);
        groups.set_border_width(10);
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        sw.add(&groups);
        sw.set_min_content_height(360);
        frame.pack_start(&sw, true, true, 0);
        let details = hbox(12);
        details.style_context().add_class("cp-details");
        frame.pack_start(&details, false, false, 0);
        v.pack_start(&frame, true, true, 0);
        v.set_vexpand(true);
        // a shell folder in Win7: no task pane, the command bar runs the full width under the address bar
        { let (me, v) = (self.clone(), v.clone()); glib::idle_add_local_once(move || {
            me.side.set_visible(false);
            if let Some(pad) = v.parent().and_then(|p| p.downcast::<gtk::Container>().ok()) { pad.set_border_width(0); }
        }); }
        let ui = Rc::new(Ui { cp: self.clone(), groups, details, dynamic, devs: RefCell::new(vec![]), tiles: RefCell::new(vec![]), sel: Cell::new(None) });
        { let u = ui.clone(); add_dev.connect_clicked(move |_| add_device(&u)); }
        { let u = ui.clone(); add_pr.connect_clicked(move |_| add_printer(&u)); }
        reload(&ui);
        v.upcast()
    }
}

fn reload(ui: &Rc<Ui>) {
    for c in ui.groups.children() { ui.groups.remove(&c); }
    ui.groups.pack_start(&label("Loading devices...", "cp-hint"), false, false, 0);
    ui.groups.show_all();
    let u = ui.clone();
    bg(all, move |devs| fill(&u, devs));
}

fn fill(ui: &Rc<Ui>, devs: Vec<Device>) {
    for c in ui.groups.children() { ui.groups.remove(&c); }
    ui.tiles.borrow_mut().clear();
    for (g, gname) in ["Devices", "Printers and Faxes", "Unspecified"].iter().enumerate() {
        let items: Vec<(usize, &Device)> = devs.iter().enumerate().filter(|(_, d)| d.group() == g).collect();
        if items.is_empty() && g == 2 { continue; }
        let head = hbox(6);
        let arrow = label("▾", "dp-head");
        head.pack_start(&arrow, false, false, 0);
        head.pack_start(&label(&format!("{gname} ({})", items.len()), "dp-head"), false, false, 0);
        let r = hbox(0);
        r.style_context().add_class("dp-rule");
        r.set_valign(gtk::Align::Center);
        head.pack_start(&r, true, true, 0);
        let eb = gtk::EventBox::new();
        eb.add(&head);
        ui.groups.pack_start(&eb, false, false, 4);
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(10);
        flow.set_min_children_per_line(3);
        flow.set_homogeneous(true);
        flow.set_column_spacing(4);
        flow.set_row_spacing(4);
        flow.set_valign(gtk::Align::Start);
        for (i, d) in items { flow.add(&tile(ui, i, d)); }
        { let (f, a) = (flow.clone(), arrow.clone()); eb.connect_button_press_event(move |_, _| { let s = !f.is_visible(); f.set_visible(s); a.set_text(if s { "▾" } else { "▸" }); glib::Propagation::Stop }); }
        ui.groups.pack_start(&flow, false, false, 0);
    }
    *ui.devs.borrow_mut() = devs;
    ui.groups.show_all();
    select(ui, None);
}

/// Win7's "default" badge on the default printer: a glossy green disc with a white check, at the
/// icon's bottom left.
fn default_badge(size: i32) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::new();
    da.set_size_request(size, size);
    da.set_halign(gtk::Align::Start);
    da.set_valign(gtk::Align::End);
    da.connect_draw(move |_, cr| {
        let s = size as f64;
        let (cx, r) = (s / 2.0, s / 2.0 - 1.0);
        cr.arc(cx, cx, r, 0.0, std::f64::consts::TAU);
        let g = gtk::cairo::RadialGradient::new(cx * 0.8, cx * 0.6, 0.0, cx, cx, r);
        g.add_color_stop_rgb(0.0, 0.55, 0.88, 0.40);
        g.add_color_stop_rgb(0.7, 0.16, 0.62, 0.14);
        g.add_color_stop_rgb(1.0, 0.07, 0.42, 0.07);
        let _ = cr.set_source(&g);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        cr.set_line_width(s * 0.14);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        cr.move_to(s * 0.28, s * 0.52);
        cr.line_to(s * 0.44, s * 0.68);
        cr.line_to(s * 0.73, s * 0.34);
        let _ = cr.stroke();
        glib::Propagation::Stop
    });
    da
}

/// Win7 opens Devices and Printers in Large icons view.
const TILE: i32 = 64;

/// A large icon tile with the name under it; the default printer gets the green check.
fn tile(ui: &Rc<Ui>, i: usize, d: &Device) -> gtk::Button {
    let b = gtk::Button::new();
    b.style_context().add_class("dp-tile");
    b.set_relief(gtk::ReliefStyle::None);
    let v = vbox(2);
    let ov = gtk::Overlay::new();
    let icon = img(&d.icon, TILE);
    if d.kind == "bluetooth" && !d.connected { icon.set_opacity(0.55); }
    ov.add(&icon);
    if d.default && d.kind == "printer" { ov.add_overlay(&default_badge(18)); }
    v.pack_start(&ov, false, false, 0);
    let l = gtk::Label::new(Some(&d.name));
    l.set_line_wrap(true);
    l.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_lines(2);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(14);
    l.set_width_chars(14);
    l.set_justify(gtk::Justification::Center);
    v.pack_start(&l, false, false, 0);
    b.add(&v);
    b.set_tooltip_text(Some(&format!("{}\n{}", d.name, d.category())));
    { let u = ui.clone(); b.connect_clicked(move |_| select(&u, Some(i))); }
    {
        let u = ui.clone();
        b.connect_button_press_event(move |_, e| {
            if e.event_type() == gdk::EventType::DoubleButtonPress && e.button() == 1 { default_action(&u, i); return glib::Propagation::Stop; }
            if e.button() == 3 { select(&u, Some(i)); context_menu(&u, i, e); return glib::Propagation::Stop; }
            glib::Propagation::Proceed
        });
    }
    ui.tiles.borrow_mut().push(b.clone());
    b
}

fn tile_index(ui: &Rc<Ui>, i: usize) -> Option<usize> {
    // tiles are created in group order; map device index → tile position
    let devs = ui.devs.borrow();
    let mut order: Vec<usize> = (0..devs.len()).collect();
    order.sort_by_key(|k| devs[*k].group());
    order.iter().position(|k| *k == i)
}

fn select(ui: &Rc<Ui>, i: Option<usize>) {
    ui.sel.set(i);
    let pos = i.and_then(|i| tile_index(ui, i));
    for (k, t) in ui.tiles.borrow().iter().enumerate() { if Some(k) == pos { t.style_context().add_class("sel") } else { t.style_context().remove_class("sel") } }
    for c in ui.dynamic.children() { ui.dynamic.remove(&c); }
    for c in ui.details.children() { ui.details.remove(&c); }
    let devs = ui.devs.borrow();
    let Some(d) = i.and_then(|i| devs.get(i)).cloned() else {
        ui.details.pack_start(&img(&["printer", "preferences-devices-printer"], 48), false, false, 0);
        let l = label(&format!("{} items", devs.len()), "n");
        l.set_valign(gtk::Align::Center);
        ui.details.pack_start(&l, false, false, 0);
        ui.details.show_all();
        return;
    };
    drop(devs);
    let i = i.unwrap();
    // the command bar offers what Win7 offers for that kind of device
    let add = |t: &str, f: Rc<dyn Fn()>| { let b = cmd_button(t); b.connect_clicked(move |_| f()); ui.dynamic.pack_start(&b, false, false, 0); };
    let acts: Vec<(String, Rc<dyn Fn()>)> = actions(ui, i).into_iter().map(|(t, f)| (t.replace('_', ""), f)).filter(|(t, _)| !t.is_empty() && t != "Properties" && t != "Troubleshoot").collect();
    let pick = |t: &str| acts.iter().find(|(x, _)| x == t).cloned();
    if d.kind == "printer" {
        // Win7: See what's printing | Print server properties | Remove device
        if let Some((t, f)) = pick("See what's printing") { add(&t, f); }
        add("Print server properties", Rc::new(|| { if !util::spawn(&["system-config-printer", "--show-server-settings"]) { util::open_uri("http://localhost:631/admin"); } }));
    } else {
        for (t, f) in acts.iter().filter(|(t, _)| t != "Remove device").take(2).cloned() { add(&t, f); }
    }
    if let Some((t, f)) = pick("Remove device") { add(&t, f); }
    ui.dynamic.show_all();
    let icon = img(&d.icon, 48);
    icon.set_valign(gtk::Align::Start);
    ui.details.pack_start(&icon, false, false, 0);
    let g = gtk::Grid::new();
    g.set_column_spacing(8);
    g.set_row_spacing(2);
    g.attach(&label(&d.name, "n"), 0, 0, 4, 1);
    // Win7 fills the pane in two columns of "Key: value" so it stays low
    let mut n = 0;
    let mut row = |k: &str, v: &str| {
        if v.is_empty() { return; }
        let (c, r) = ((n % 2) * 2, 1 + n / 2);
        let kl = label(k, "k");
        kl.set_xalign(1.0);
        if c > 0 { kl.set_margin_start(24); }
        g.attach(&kl, c, r, 1, 1);
        let vl = label(v, "");
        vl.set_line_wrap(false);
        vl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        vl.set_max_width_chars(36);
        g.attach(&vl, c + 1, r, 1, 1);
        n += 1;
    };
    // Win7: State "Default" (or Offline) for a printer, its queue under Status
    if d.kind == "printer" { row("State:", if d.default { "Default" } else if d.location == "Offline" { "Offline" } else { "" }); row("Status:", &d.status); } else if d.kind == "bluetooth" { row("Status:", &d.status); }
    row("Manufacturer:", &d.maker);
    row("Model:", &d.model);
    row("Category:", d.category());
    if d.default && d.kind == "audio" { row("Status:", "Default device"); }
    ui.details.pack_start(&g, true, true, 0);
    ui.details.show_all();
}

/// Win7's context menu entries for a device ("" = separator).
fn actions(ui: &Rc<Ui>, i: usize) -> Vec<(String, Rc<dyn Fn()>)> {
    let d = ui.devs.borrow()[i].clone();
    let cp = ui.cp.clone();
    let nav = |page: &'static str| -> Rc<dyn Fn()> { let c = cp.clone(); Rc::new(move || go(&c, page)) };
    let sep = || (String::new(), Rc::new(|| {}) as Rc<dyn Fn()>);
    let props: Rc<dyn Fn()> = { let (u, d) = (ui.clone(), d.clone()); Rc::new(move || properties(&u, &d)) };
    let trouble = nav("troubleshooting");
    let mut v: Vec<(String, Rc<dyn Fn()>)> = vec![];
    match d.kind.as_str() {
        "computer" => {
            v.push(("_Browse files".into(), Rc::new(|| util::explore("computer:///"))));
            v.push(sep());
            v.push(("_System properties".into(), nav("system")));
            v.push(("_Network settings".into(), nav("network")));
            v.push(("_Sound settings".into(), nav("sound")));
            v.push(("_Mouse settings".into(), nav("mouse")));
            v.push(("_Keyboard settings".into(), nav("keyboard")));
            v.push(("Region and _language".into(), nav("region")));
            v.push(("_Power options".into(), nav("power")));
            v.push(("_Windows Update".into(), nav("update")));
            v.push(("_Device installation settings".into(), nav("sysprops-hardware")));
        }
        "monitor" => v.push(("_Display settings".into(), nav("display"))),
        "keyboard" => { v.push(("_Keyboard settings".into(), nav("keyboard"))); v.push(("Region and _language".into(), nav("region"))); }
        "mouse" => v.push(("_Mouse settings".into(), nav("mouse"))),
        "audio" => {
            v.push(("_Sound settings".into(), nav("sound")));
            if !d.default { let (id, u) = (d.id.clone(), ui.clone()); v.push(("Set as _Default Device".into(), Rc::new(move || { aerode::sys::set_default_sink(&id); reload(&u); }))); }
        }
        "camera" => v.push(("_Take pictures".into(), Rc::new(|| { for c in ["cheese", "guvcview", "kamoso", "snapshot"] { if util::spawn(&[c]) { return; } } util::error("No camera program is installed.", "Install Cheese or guvcview."); }))),
        "storage" => {
            let m = d.mount.clone();
            v.push(("_Browse files".into(), Rc::new(move || { if m.is_empty() { util::explore("computer:///") } else { util::open_path(std::path::Path::new(&m)); } })));
            let (dev, u) = (d.id.clone(), ui.clone());
            v.push(("_Eject".into(), Rc::new(move || eject(&u, &dev))));
        }
        "bluetooth" => {
            let (mac, u, c) = (d.id.clone(), ui.clone(), d.connected);
            v.push((if c { "_Disconnect".into() } else { "_Connect".into() }, Rc::new(move || {
                let (mac, u) = (mac.clone(), u.clone());
                bg(move || run(&["bluetoothctl", if c { "disconnect" } else { "connect" }, &mac]), move |(ok, out)| { if !ok { util::error("The device could not be connected.", out.trim()); } reload(&u); });
            })));
            v.push(("_Sound settings".into(), nav("sound")));
        }
        "printer" => {
            let (n, u) = (d.id.clone(), ui.clone());
            v.push(("_See what's printing".into(), { let (n, s, u) = (n.clone(), d.name.clone(), u.clone()); Rc::new(move || queue(&u, &n, &s)) }));
            if !d.default { let (n, u) = (n.clone(), u.clone()); v.push(("Set as _default printer".into(), Rc::new(move || { let u = u.clone(); let n = n.clone(); bg(move || run(&["lpoptions", "-d", &n]), move |(ok, out)| { report("The default printer could not be set.", ok, &out); reload(&u); }); }))); }
            { let (n, s, u) = (n.clone(), d.name.clone(), ui.clone()); v.push(("Printing _preferences".into(), Rc::new(move || preferences(win_of(&u.groups).as_ref(), &n, &s)))); }
            { let n = n.clone(); v.push(("P_rinter properties".into(), Rc::new(move || { if !util::spawn(&["system-config-printer", "--show-printer", &n]) { printer_web(&n, ""); } }))); }
            let _ = &u;
        }
        _ => {}
    }
    v.push(sep());
    v.push(("_Troubleshoot".into(), trouble));
    if matches!(d.kind.as_str(), "printer" | "bluetooth") {
        v.push(sep());
        let (d2, u) = (d.clone(), ui.clone());
        v.push(("Remove de_vice".into(), Rc::new(move || remove_device(&u, &d2))));
    }
    v.push(sep());
    v.push(("P_roperties".into(), props));
    v
}

fn context_menu(ui: &Rc<Ui>, i: usize, e: &gdk::EventButton) {
    let m = gtk::Menu::new();
    let acts = actions(ui, i);
    let mut first = true;
    for (t, f) in acts {
        if t.is_empty() { m.append(&gtk::SeparatorMenuItem::new()); continue; }
        let mi = menu_item(&m, &t, move || f());
        // the default action is bold (Win7)
        if first { if let Some(l) = mi.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&l.text()))); l.set_use_underline(true); } first = false; }
    }
    m.show_all();
    m.popup_at_pointer(Some(e));
}

fn default_action(ui: &Rc<Ui>, i: usize) {
    if let Some((_, f)) = actions(ui, i).into_iter().find(|(t, _)| !t.is_empty()) { f(); }
}

fn printer_web(name: &str, page: &str) {
    if !util::spawn(&["system-config-printer", "--show-printer", name]) { util::open_uri(&format!("http://localhost:631/printers/{name}{page}")); }
}

/// One print job: (job id "HP-12", document, status, owner, size, submitted).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Job { pub id: String, pub doc: String, pub status: String, pub owner: String, pub size: String, pub submitted: String }

/// The queue from `lpq` (rank, owner, job, files, size) and `lpstat -o` (submit time).
pub fn jobs(printer: &str, lpq: &str, lpstat: &str) -> Vec<Job> {
    let mut v = vec![];
    for l in lpq.lines() {
        // "active  demid   12      report.pdf                      24576 bytes"
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 5 || f[2].parse::<u32>().is_err() || f[f.len() - 1] != "bytes" { continue; }
        let id = format!("{printer}-{}", f[2]);
        let doc = f[3..f.len() - 2].join(" ");
        let submitted = lpstat.lines().find(|s| s.split_whitespace().next() == Some(id.as_str())).map(|s| s.split_whitespace().skip(3).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let status = match f[0] { "active" => "Printing", r if r.contains("held") => "Paused", _ => "" };
        v.push(Job { id, doc: if doc.is_empty() { "(untitled)".into() } else { doc }, status: status.into(), owner: f[1].into(), size: util::human_size(f[f.len() - 2].parse().unwrap_or(0)), submitted });
    }
    v
}

/// Win7's print queue window ("HP LaserJet P1102 - Paused"): Printer / Document / View menus,
/// the documents, a status bar; refreshed every two seconds.
fn queue(ui: &Rc<Ui>, printer: &str, shown: &str) {
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title(shown);
    w.set_icon_name(Some("printer"));
    w.set_default_size(640, 300);
    let v = vbox(0);
    let mb = gtk::MenuBar::new();
    v.pack_start(&mb, false, false, 0);
    // job id, document, status, owner, size, submitted
    let store = gtk::ListStore::new(&[glib::Type::STRING; 6]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Document Name", 1, 210), ("Status", 2, 80), ("Owner", 3, 80), ("Size", 4, 80), ("Submitted", 5, 0)]);
    let sw = scrolled(&tv);
    v.pack_start(&sw, true, true, 0);
    let sbar = hbox(0);
    sbar.style_context().add_class("cp-details");
    let status = label("", "");
    sbar.pack_start(&status, false, false, 0);
    v.pack_start(&sbar, false, false, 0);
    w.add(&v);
    let (p, s) = (printer.to_string(), shown.to_string());
    let paused = Rc::new(Cell::new(false));
    let load: Rc<dyn Fn()> = { let (store, status, p, s, w, paused, tv) = (store.clone(), status.clone(), p.clone(), s.clone(), w.clone(), paused.clone(), tv.clone()); Rc::new(move || {
        let (store, status, p2, s, w, paused, tv) = (store.clone(), status.clone(), p.clone(), s.clone(), w.clone(), paused.clone(), tv.clone());
        let p3 = p2.clone();
        bg(move || (sh(&["lpq", "-P", &p3]), sh(&["lpstat", "-o", &p3]), sh(&["lpstat", "-p", &p3])), move |(q, o, st)| {
            // the selected document stays selected across refreshes
            let sel = tv.selection().selected().map(|(m, it)| sval(&m, &it, 0));
            store.clear();
            let js = jobs(&p2, &q, &o);
            for j in &js {
                let it = store.insert_with_values(None, &[(0, &j.id), (1, &j.doc), (2, &j.status), (3, &j.owner), (4, &j.size), (5, &j.submitted)]);
                if sel.as_deref() == Some(j.id.as_str()) { tv.selection().select_iter(&it); }
            }
            status.set_text(&format!("{} document(s) in queue", js.len()));
            paused.set(st.contains("disabled"));
            w.set_title(&if paused.get() { format!("{s} - Paused") } else { s.clone() });
        });
    }) };
    load();
    { let (l, ww) = (load.clone(), w.downgrade()); glib::timeout_add_seconds_local(2, move || { if ww.upgrade().is_none() { return glib::ControlFlow::Break; } l(); glib::ControlFlow::Continue }); }
    let menu = |title: &str| { let m = gtk::Menu::new(); let mi = gtk::MenuItem::with_mnemonic(title); mi.set_submenu(Some(&m)); mb.append(&mi); m };
    let selected = { let tv = tv.clone(); move || tv.selection().selected().map(|(m, it)| sval(&m, &it, 0)) };
    let job_cmd = |m: &gtk::Menu, t: &str, args: &'static [&'static str], what: &'static str| {
        let (sel, l) = (selected.clone(), load.clone());
        menu_item(m, t, move || {
            let Some(id) = sel() else { return };
            let l = l.clone();
            let mut a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            for x in a.iter_mut() { if x == "{}" { *x = id.clone(); } }
            bg(move || { let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect(); run(&r) }, move |(ok, out)| { report(what, ok, &out); l(); });
        });
    };
    let pm = menu("_Printer");
    { let (u, p) = (ui.clone(), p.clone()); menu_item(&pm, "Set As _Default Printer", move || { let (u, p) = (u.clone(), p.clone()); bg(move || run(&["lpoptions", "-d", &p]), move |(ok, out)| { report("The default printer could not be set.", ok, &out); reload(&u); }); }); }
    { let (p, s, w) = (p.clone(), s.clone(), w.clone()); menu_item(&pm, "Printing Pr_eferences...", move || preferences(Some(&w), &p, &s)); }
    pm.append(&gtk::SeparatorMenuItem::new());
    { let (p, l, paused) = (p.clone(), load.clone(), paused.clone()); menu_item(&pm, "_Pause Printing", move || {
        let l = l.clone();
        elevate(&[if paused.get() { "cupsenable" } else { "cupsdisable" }, &p], move |ok, out| { report("The printer could not be paused.", ok, &out); l(); });
    }); }
    { let (p, l, w) = (p.clone(), load.clone(), w.clone()); menu_item(&pm, "_Cancel All Documents", move || {
        if !confirm(Some(&w), "Printers", "Are you sure you want to cancel all documents?", "The documents will be deleted from the print queue.", "_Yes") { return; }
        let (p, l) = (p.clone(), l.clone());
        bg(move || run(&["cancel", "-a", &p]), move |(ok, out)| { report("The documents could not be canceled.", ok, &out); l(); });
    }); }
    pm.append(&gtk::SeparatorMenuItem::new());
    { let p = p.clone(); menu_item(&pm, "P_roperties", move || { if !util::spawn(&["system-config-printer", "--show-printer", &p]) { printer_web(&p, ""); } }); }
    pm.append(&gtk::SeparatorMenuItem::new());
    { let w = w.clone(); menu_item(&pm, "_Close", move || w.close()); }
    let dm = menu("_Document");
    job_cmd(&dm, "_Pause", &["lp", "-i", "{}", "-H", "hold"], "The document could not be paused.");
    job_cmd(&dm, "_Resume", &["lp", "-i", "{}", "-H", "resume"], "The document could not be resumed.");
    job_cmd(&dm, "Res_tart", &["lp", "-i", "{}", "-H", "restart"], "The document could not be restarted.");
    job_cmd(&dm, "_Cancel", &["cancel", "{}"], "The document could not be canceled.");
    let vm = menu("_View");
    let sb = gtk::CheckMenuItem::with_mnemonic("_Status Bar");
    sb.set_active(true);
    { let s = sbar.clone(); sb.connect_toggled(move |c| s.set_visible(c.is_active())); }
    vm.append(&sb);
    // Delete cancels the selected document; the context menu is the Document menu
    { let (sel, l) = (selected.clone(), load.clone()); tv.connect_key_press_event(move |_, e| {
        if e.keyval() != gdk::keys::constants::Delete { return glib::Propagation::Proceed; }
        if let Some(id) = sel() { let l = l.clone(); bg(move || run(&["cancel", &id]), move |(ok, out)| { report("The document could not be canceled.", ok, &out); l(); }); }
        glib::Propagation::Stop
    }); }
    { let dm = dm.clone(); tv.connect_button_press_event(move |tv, e| {
        if e.button() != 3 { return glib::Propagation::Proceed; }
        let (x, y) = e.position();
        if let Some((Some(p), _, _, _)) = tv.path_at_pos(x as i32, y as i32) { tv.selection().select_path(&p); dm.show_all(); dm.popup_at_pointer(Some(e)); }
        glib::Propagation::Stop
    }); }
    w.show_all();
}

/// An option of `lpoptions -l`: key, label, choices, the current one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PpdOption { pub key: String, pub label: String, pub choices: Vec<String>, pub current: String }

/// "PageSize/Media Size: *Letter Legal A4" → PpdOption.
pub fn ppd_options(out: &str) -> Vec<PpdOption> {
    out.lines().filter_map(|l| {
        let (head, vals) = l.split_once(':')?;
        let (key, label) = head.split_once('/').unwrap_or((head, head));
        let mut o = PpdOption { key: key.trim().into(), label: label.trim().into(), ..Default::default() };
        for c in vals.split_whitespace() {
            let c2 = c.trim_start_matches('*');
            if c.starts_with('*') { o.current = c2.to_string(); }
            o.choices.push(c2.to_string());
        }
        (!o.key.is_empty() && !o.choices.is_empty()).then_some(o)
    }).collect()
}

/// "<printer> Printing Preferences": Layout and Paper/Quality like Win7, Advanced... with every
/// option of the driver; saved per user (`lpoptions -p`, ~/.cups/lpoptions).
fn preferences(parent: Option<&gtk::Window>, printer: &str, shown: &str) {
    let d = dialog(parent, &format!("{shown} Printing Preferences"), &[]);
    d.set_default_size(420, 420);
    let opts = ppd_options(&sh(&["lpoptions", "-p", printer, "-l"]));
    let cur = sh(&["lpoptions", "-p", printer]);
    let changed: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(vec![]));
    let set: Rc<dyn Fn(&str, &str)> = { let (c, d) = (changed.clone(), d.clone()); Rc::new(move |k: &str, v: &str| { d.set_response_sensitive(gtk::ResponseType::Apply, true); let mut c = c.borrow_mut(); c.retain(|(x, _)| x != k); c.push((k.to_string(), v.to_string())); }) };
    let nb = gtk::Notebook::new();
    let grid = || { let g = gtk::Grid::new(); g.set_row_spacing(10); g.set_column_spacing(12); g.set_border_width(12); g };
    let row = |g: &gtk::Grid, r: i32, t: &str, c: &gtk::ComboBoxText| { g.attach(&label(t, ""), 0, r, 1, 1); c.set_hexpand(true); g.attach(c, 1, r, 1, 1); };
    // a fixed list (IPP attributes every CUPS queue has)
    let fixed = |key: &'static str, list: &[(&str, &str)], dflt: &str| {
        let v = lp_opt(&cur, key);
        let c = combo(list, if v.is_empty() { dflt } else { &v });
        let s = set.clone();
        c.connect_changed(move |c| if let Some(id) = c.active_id() { s(key, &id); });
        c
    };
    // a driver option, when the driver has it
    let driver = |keys: &[&str]| -> Option<gtk::ComboBoxText> {
        let o = opts.iter().find(|o| keys.contains(&o.key.as_str()))?.clone();
        let list: Vec<(&str, &str)> = o.choices.iter().map(|c| (c.as_str(), c.as_str())).collect();
        let c = combo(&list, &o.current);
        let (s, k) = (set.clone(), o.key.clone());
        c.connect_changed(move |c| if let Some(id) = c.active_id() { s(&k, &id); });
        Some(c)
    };
    let lg = grid();
    row(&lg, 0, "Orientation:", &fixed("orientation-requested", &[("3", "Portrait"), ("4", "Landscape"), ("5", "Rotated Landscape"), ("6", "Rotated Portrait")], "3"));
    let duplex = driver(&["Duplex", "sides"]).unwrap_or_else(|| fixed("sides", &[("one-sided", "None"), ("two-sided-long-edge", "Flip on Long Edge"), ("two-sided-short-edge", "Flip on Short Edge")], "one-sided"));
    row(&lg, 1, "Print on Both Sides:", &duplex);
    row(&lg, 2, "Page Order:", &fixed("outputorder", &[("normal", "Front to Back"), ("reverse", "Back to Front")], "normal"));
    row(&lg, 3, "Pages per Sheet:", &fixed("number-up", &[("1", "1"), ("2", "2"), ("4", "4"), ("6", "6"), ("9", "9"), ("16", "16")], "1"));
    nb.append_page(&lg, Some(&gtk::Label::new(Some("Layout"))));
    let pg = grid();
    let mut r = 0;
    for (t, keys) in [("Paper Source:", &["InputSlot", "media-source"][..]), ("Media:", &["MediaType", "media-type"]), ("Paper Size:", &["PageSize", "media"]), ("Color:", &["ColorModel", "print-color-mode", "ColorMode"]), ("Print Quality:", &["print-quality", "cupsPrintQuality", "Resolution", "OutputMode"])] {
        if let Some(c) = driver(keys) { row(&pg, r, t, &c); r += 1; }
    }
    if r == 0 { pg.attach(&label("The printer driver has no paper or quality settings.", "cp-hint"), 0, 0, 2, 1); }
    nb.append_page(&pg, Some(&gtk::Label::new(Some("Paper/Quality"))));
    d.content_area().pack_start(&nb, true, true, 0);
    let adv = gtk::Button::with_mnemonic("Ad_vanced...");
    adv.set_halign(gtk::Align::End);
    {
        let (o, s, d2, shown) = (opts.clone(), set.clone(), d.clone(), shown.to_string());
        adv.connect_clicked(move |_| {
            let a = dialog(Some(d2.upcast_ref()), &format!("{shown} Advanced Options"), &[("OK", gtk::ResponseType::Ok)]);
            a.set_default_size(440, 420);
            let g = grid();
            for (i, op) in o.iter().enumerate() {
                let list: Vec<(&str, &str)> = op.choices.iter().map(|c| (c.as_str(), c.as_str())).collect();
                let c = combo(&list, &op.current);
                let (s, k) = (s.clone(), op.key.clone());
                c.connect_changed(move |c| if let Some(id) = c.active_id() { s(&k, &id); });
                row(&g, i as i32, &format!("{}:", op.label), &c);
            }
            if o.is_empty() { g.attach(&label("The printer driver has no options.", "cp-hint"), 0, 0, 2, 1); }
            a.content_area().pack_start(&scrolled(&g), true, true, 0);
            a.connect_response(|a, _| a.close());
            a.show_all();
        });
    }
    d.content_area().pack_start(&adv, false, false, 0);
    for (t, rsp) in [("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel), ("_Apply", gtk::ResponseType::Apply)] { d.add_button(t, rsp); }
    d.set_response_sensitive(gtk::ResponseType::Apply, false);
    let p = printer.to_string();
    d.connect_response(move |d, rsp| {
        if matches!(rsp, gtk::ResponseType::Ok | gtk::ResponseType::Apply) {
            let c: Vec<(String, String)> = changed.borrow_mut().drain(..).collect();
            if !c.is_empty() {
                let mut a = vec!["lpoptions".to_string(), "-p".into(), p.clone()];
                for (k, v) in c { a.push("-o".into()); a.push(format!("{k}={v}")); }
                bg(move || { let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect(); run(&r) }, |(ok, out)| { report("The printing preferences could not be saved.", ok, &out); });
            }
            if rsp == gtk::ResponseType::Apply { d.set_response_sensitive(gtk::ResponseType::Apply, false); return; }
        }
        d.close();
    });
    d.show_all();
}

fn remove_device(ui: &Rc<Ui>, d: &Device) {
    if !confirm(win_of(&ui.groups).as_ref(), "Remove Device", "Are you sure you want to remove this device?", &d.name, "_Yes") { return; }
    let u = ui.clone();
    if d.kind == "printer" {
        elevate(&["lpadmin", "-x", &d.id], move |ok, out| { report("The printer could not be removed.", ok, &out); reload(&u); });
    } else {
        let mac = d.id.clone();
        bg(move || run(&["bluetoothctl", "remove", &mac]), move |(ok, out)| { report("The device could not be removed.", ok, &out); reload(&u); });
    }
}

fn eject(ui: &Rc<Ui>, dev: &str) {
    let (u, dev) = (ui.clone(), dev.to_string());
    bg(move || {
        // unmount every partition, then power the drive off (Safely Remove Hardware)
        for l in sh(&["lsblk", "-ln", "-o", "PATH,MOUNTPOINT", &dev]).lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() == 2 { let r = run(&["udisksctl", "unmount", "-b", f[0]]); if !r.0 { return r; } }
        }
        run(&["udisksctl", "power-off", "-b", &dev])
    }, move |(ok, out)| {
        if ok { inform(win_of(&u.groups).as_ref(), "Safe To Remove Hardware", "The device can now be safely removed from the computer.", ""); } else { util::error("Problem Ejecting USB Mass Storage Device", out.trim()); }
        reload(&u);
    });
}

/// Device properties: General (Device Information) and Hardware (functions, status).
fn properties(ui: &Rc<Ui>, d: &Device) {
    let dl = dialog(win_of(&ui.groups).as_ref(), &format!("{} Properties", d.name), &[]);
    dl.set_default_size(400, 420);
    let nb = gtk::Notebook::new();
    let g = vbox(10);
    g.set_border_width(10);
    let h = hbox(10);
    h.pack_start(&img(&d.icon, 48), false, false, 0);
    h.pack_start(&label(&d.name, "n"), false, false, 0);
    g.pack_start(&h, false, false, 0);
    g.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
    g.pack_start(&bold("Device Information"), false, false, 0);
    let rows: Vec<(&str, String)> = vec![("Manufacturer:", if d.maker.is_empty() { "Unavailable".into() } else { d.maker.clone() }), ("Model:", d.model.clone()),
        ("Model number:", if d.kind == "printer" || d.kind == "bluetooth" { d.id.clone() } else { "Unavailable".into() }), ("Categories:", d.category().into()), ("Description:", if d.location.is_empty() { "Unavailable".into() } else { d.location.clone() })];
    g.pack_start(&kv_grid(&rows), false, false, 0);
    g.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
    g.pack_start(&bold("Device Tasks"), false, false, 0);
    g.pack_start(&label("To view tasks for this device, right-click the icon for the device in Devices and Printers.", ""), false, false, 0);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    let hw = vbox(8);
    hw.set_border_width(10);
    hw.pack_start(&label(&d.name, ""), false, false, 0);
    hw.pack_start(&label("Device Functions:", ""), false, false, 0);
    let st = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    st.insert_with_values(None, &[(0, &d.name), (1, &d.category())]);
    let tv = gtk::TreeView::with_model(&st);
    columns(&tv, &[("Name", 0, 220), ("Type", 1, 0)]);
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 90);
    hw.pack_start(&sw, false, false, 0);
    let fs = vbox(4);
    fs.pack_start(&kv_grid(&[("Manufacturer:", if d.maker.is_empty() { "Unknown".into() } else { d.maker.clone() }), ("Location:", if d.location.is_empty() { "on this computer".into() } else { d.location.clone() }),
        ("Device status:", if d.connected { "This device is working properly.".into() } else { d.status.clone() })]), false, false, 0);
    let pb = gtk::Button::with_mnemonic("P_roperties");
    pb.set_halign(gtk::Align::End);
    pb.connect_clicked(|_| spawn_tool("devmgr"));
    fs.pack_start(&pb, false, false, 0);
    hw.pack_start(&group("Device Function Summary", &fs), false, false, 0);
    nb.append_page(&hw, Some(&gtk::Label::new(Some("Hardware"))));
    dl.content_area().pack_start(&nb, true, true, 0);
    for (t, r) in [("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel), ("_Apply", gtk::ResponseType::Apply)] { let b = dl.add_button(t, r); if r == gtk::ResponseType::Apply { b.set_sensitive(false); } }
    dl.connect_response(|d, _| d.close());
    dl.show_all();
}

// ------------------------------------------------------------------------- add a device

/// "Add a device": scan for Bluetooth devices, then pair, trust and connect the chosen one.
fn add_device(ui: &Rc<Ui>) {
    if !util::which("bluetoothctl") {
        inform(win_of(&ui.groups).as_ref(), "Add a device", "Windows can't find a Bluetooth adapter.", "Install bluez and bluez-utils (Turn Windows features on or off > Bluetooth support) and start the bluetooth service.");
        return;
    }
    let d = dialog(win_of(&ui.groups).as_ref(), "Add a device", &[]);
    d.set_default_size(520, 380);
    let c = d.content_area();
    c.pack_start(&label("Select a device to add to this computer", "wu-title"), false, false, 0);
    c.pack_start(&label("Windows will continue to look for new devices and display them here.", ""), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    columns(&tv, &[("Name", 0, 0), ("Type", 2, 140)]);
    c.pack_start(&scrolled(&tv), true, true, 0);
    let state = hbox(6);
    let sp = gtk::Spinner::new();
    sp.start();
    state.pack_start(&sp, false, false, 0);
    let st = label("Searching for devices...", "cp-hint");
    state.pack_start(&st, false, false, 0);
    c.pack_start(&state, false, false, 0);
    c.pack_start(&link("What if Windows doesn't find my device?", || { util::open_uri("https://wiki.archlinux.org/title/Bluetooth#Troubleshooting"); }), false, false, 0);
    let next = d.add_button("_Next", gtk::ResponseType::Ok);
    next.set_sensitive(false);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    { let n = next.clone(); tv.selection().connect_changed(move |s| n.set_sensitive(s.selected().is_some())); }
    let open = Rc::new(Cell::new(true));
    // scan in rounds while the dialog is open
    let scan: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let f: Rc<dyn Fn()> = { let (store, open, scan, sp, st) = (store.clone(), open.clone(), scan.clone(), sp.clone(), st.clone()); Rc::new(move || {
        let (store, open, scan, sp, st) = (store.clone(), open.clone(), scan.clone(), sp.clone(), st.clone());
        bg(|| {
            let _ = run(&["bluetoothctl", "--timeout", "8", "scan", "on"]);
            let paired = sh(&["bluetoothctl", "devices", "Paired"]);
            sh(&["bluetoothctl", "devices"]).lines().filter(|l| !paired.contains(l.trim())).filter_map(|l| { let mut it = l.splitn(3, ' '); it.next(); Some((it.next()?.to_string(), it.next()?.to_string())) }).collect::<Vec<_>>()
        }, move |found: Vec<(String, String)>| {
            if !open.get() { return; }
            for (mac, name) in found {
                let mut have = false;
                store.foreach(|m, _, it| { if sval(m, it, 1) == mac { have = true; } have });
                if !have { store.insert_with_values(None, &[(0, &name), (1, &mac), (2, &"Bluetooth")]); }
            }
            if store.iter_n_children(None) == 0 { st.set_text("Windows is still looking for devices. Make sure the device is turned on and discoverable."); }
            let _ = &sp;
            if let Some(f) = scan.borrow().clone() { f(); }
        });
    }) };
    *scan.borrow_mut() = Some(f.clone());
    f();
    let u = ui.clone();
    let tv2 = tv.clone();
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok {
            let Some((m, it)) = tv2.selection().selected() else { return };
            let (name, mac) = (sval(&m, &it, 0), sval(&m, &it, 1));
            st.set_text(&format!("Adding {name}..."));
            let (d2, u2) = (d.clone(), u.clone());
            d.set_response_sensitive(gtk::ResponseType::Ok, false);
            bg(move || {
                let p = run(&["bluetoothctl", "--timeout", "20", "pair", &mac]);
                if !p.0 && !p.1.contains("AlreadyExists") { return p; }
                let _ = run(&["bluetoothctl", "trust", &mac]);
                run(&["bluetoothctl", "connect", &mac])
            }, move |(ok, out)| {
                if ok { inform(Some(d2.upcast_ref()), "Add a device", &format!("This device has been successfully added to this computer: {name}"), ""); d2.close(); reload(&u2); }
                else { util::error(&format!("Adding {name} failed."), out.trim()); d2.set_response_sensitive(gtk::ResponseType::Ok, true); }
            });
            return;
        }
        open.set(false);
        util::spawn(&["bluetoothctl", "scan", "off"]);
        d.close();
    });
    d.show_all();
}

// ------------------------------------------------------------------------ add a printer

use super::super::extras::common::{self as xc, Wizard};

/// A printer the wizard can add: what the list shows and the CUPS device URI / driver.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewPrinter { pub name: String, pub address: String, pub uri: String, pub driver: String, pub model: String }

/// Network printers CUPS has found (DNS-SD, driverless): `lpstat -e` with their `lpoptions`.
fn network_printers() -> Vec<NewPrinter> {
    let installed: Vec<String> = sh(&["lpstat", "-p"]).lines().filter_map(|l| l.strip_prefix("printer ")?.split_whitespace().next().map(String::from)).collect();
    sh(&["lpstat", "-e"]).lines().map(str::trim).filter(|n| !n.is_empty() && !installed.iter().any(|i| i == n)).map(|n| {
        let o = sh(&["lpoptions", "-p", n]);
        let (uri, info, model) = (lp_opt(&o, "device-uri"), lp_opt(&o, "printer-info"), lp_opt(&o, "printer-make-and-model"));
        NewPrinter { name: if info.is_empty() { n.replace('_', " ") } else { info }, address: uri_host(&uri), uri, driver: "everywhere".into(), model }
    }).collect()
}

/// A value of `lpoptions -p` output (`key=value` or `key='quoted value'`).
pub fn lp_opt(opts: &str, k: &str) -> String {
    let Some(s) = format!(" {opts}").split(&format!(" {k}=")).nth(1).map(String::from) else { return String::new() };
    if let Some(q) = s.strip_prefix('\'') { q.split('\'').next().unwrap_or("").to_string() } else { s.split(' ').next().unwrap_or("").to_string() }
}

/// "ipp://HP1234.local:631/ipp/print" → "HP1234.local".
pub fn uri_host(uri: &str) -> String {
    let rest = uri.split_once("://").map(|x| x.1).unwrap_or(uri);
    let host = rest.split('/').next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let h = host.split(':').next().unwrap_or(host).replace("%20", " ");
    // a DNS-SD service name: "Brother HL-L2350DW series._ipp._tcp.local" → its instance name
    match h.find("._") { Some(i) if uri.starts_with("dnssd:") => h[..i].to_string(), _ => h }
}

/// A CUPS queue name from a Windows printer name (no spaces, slashes or #).
pub fn queue_name(name: &str) -> String {
    let q: String = name.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
    let q = q.trim_matches('_').to_string();
    if q.is_empty() { "Printer".into() } else { q.chars().take(127).collect() }
}

/// `lpinfo -m`: (manufacturer, model, driver name), the manufacturer being the model's first word.
pub fn drivers(out: &str) -> Vec<(String, String, String)> {
    let mut v: Vec<(String, String, String)> = out.lines().filter_map(|l| {
        let (ppd, model) = l.trim().split_once(' ')?;
        let model = model.trim();
        let maker = if ppd == "everywhere" { "Generic".to_string() } else { model.split_whitespace().next().unwrap_or("Generic").to_string() };
        Some((maker, model.to_string(), ppd.to_string()))
    }).collect();
    v.sort_by(|a, b| (a.0.to_lowercase(), a.1.to_lowercase()).cmp(&(b.0.to_lowercase(), b.1.to_lowercase())));
    v.dedup_by(|a, b| a.1 == b.1);
    v
}

/// Win7's Add Printer wizard on CUPS: a network printer found on the network (driverless IPP
/// Everywhere) or by address, or a local one (a port and a driver from CUPS' list); then its
/// name, sharing, and "You've successfully added …" with Set as the default printer and Print a
/// test page. Adding the queue asks for the administrator password (pkexec lpadmin).
fn add_printer(ui: &Rc<Ui>) {
    if !util::which("lpstat") || !util::which("lpadmin") {
        util::error("The print spooler is not installed.", "Turn on Print and Document Services in Turn Windows features on or off (CUPS), then start the cups service.");
        return;
    }
    let wz = xc::wizard("Add Printer", "printer", win_of(&ui.groups).as_ref(), (600, 450), true);
    wz.back.connect_clicked(|_| { if let Some(f) = BACK.with(|b| b.borrow().clone()) { f(); } });
    printer_type_page(ui, &wz);
    wz.win.show_all();
}

fn cancel_button(wz: &Rc<Wizard>) -> gtk::Button {
    let b = gtk::Button::with_label("Cancel");
    let w = wz.win.clone();
    b.connect_clicked(move |_| w.close());
    b
}

thread_local!(static BACK: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) });

/// What the wizard's back button does on the current page (None: it is greyed).
fn set_back(wz: &Rc<Wizard>, f: Option<Rc<dyn Fn()>>) {
    wz.back.set_sensitive(f.is_some());
    BACK.with(|b| *b.borrow_mut() = f);
}

fn printer_type_page(ui: &Rc<Ui>, wz: &Rc<Wizard>) {
    set_back(wz, None);
    let v = vbox(10);
    v.pack_start(&xc::instr("What type of printer do you want to install?"), false, false, 0);
    let (u, w) = (ui.clone(), wz.clone());
    v.pack_start(&command_link("Add a local printer", "Use this option only if you don't have a USB printer. (Windows automatically installs USB printers when you plug them in.)", move || port_page(&u, &w)), false, false, 4);
    let (u, w) = (ui.clone(), wz.clone());
    v.pack_start(&command_link("Add a network, wireless or Bluetooth printer", "Make sure that your computer is connected to the network, or that your Bluetooth or wireless printer is turned on.", move || search_page(&u, &w)), false, false, 0);
    wz.page(&v);
    let next = gtk::Button::with_mnemonic("_Next");
    next.set_sensitive(false);
    wz.set_buttons(&[&next, &cancel_button(wz)]);
}

/// "Searching for available printers…": what CUPS finds on the network.
fn search_page(ui: &Rc<Ui>, wz: &Rc<Wizard>) {
    { let (u, w) = (ui.clone(), wz.clone()); set_back(wz, Some(Rc::new(move || printer_type_page(&u, &w)))); }
    let v = vbox(8);
    let head = xc::instr("Searching for available printers...");
    v.pack_start(&head, false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
    let tv = gtk::TreeView::with_model(&store);
    let col = gtk::TreeViewColumn::new();
    col.set_title("Printer Name");
    let pr = gtk::CellRendererPixbuf::new();
    pr.set_property("icon-name", "printer");
    TreeViewColumnExt::pack_start(&col, &pr, false);
    let r = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &r, true);
    TreeViewColumnExt::add_attribute(&col, &r, "text", 0);
    col.set_fixed_width(260);
    col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    tv.append_column(&col);
    columns(&tv, &[("Address", 1, 0)]);
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 200);
    v.pack_start(&sw, true, true, 0);
    let bar = gtk::ProgressBar::new();
    v.pack_start(&bar, false, false, 0);
    let stop = gtk::Button::with_mnemonic("_Stop");
    stop.set_halign(gtk::Align::End);
    v.pack_start(&stop, false, false, 0);
    let found: Rc<RefCell<Vec<NewPrinter>>> = Rc::new(RefCell::new(vec![]));
    let next = gtk::Button::with_mnemonic("_Next");
    next.set_sensitive(false);
    { let n = next.clone(); tv.selection().connect_changed(move |s| n.set_sensitive(s.selected().is_some())); }
    let (u, w) = (ui.clone(), wz.clone());
    let foot = link("The printer that I want isn't listed", move || by_address_page(&u, &w));
    wz.set_foot(Some(foot.upcast_ref()));
    wz.page(&v);
    wz.set_buttons(&[&next, &cancel_button(wz)]);
    let pulse = { let b = bar.clone(); glib::timeout_add_local(std::time::Duration::from_millis(120), move || { b.pulse(); glib::ControlFlow::Continue }) };
    let pulse = Rc::new(RefCell::new(Some(pulse)));
    let done: Rc<dyn Fn(&str)> = { let (p, bar, stop, head) = (pulse.clone(), bar.clone(), stop.clone(), head.clone()); Rc::new(move |t: &str| {
        if let Some(id) = p.borrow_mut().take() { id.remove(); }
        bar.hide();
        stop.hide();
        head.set_text(t);
    }) };
    { let d = done.clone(); stop.connect_clicked(move |_| d("Select a printer")); }
    {
        let (store, found, d, p) = (store.clone(), found.clone(), done.clone(), pulse.clone());
        bg(|| { std::thread::sleep(std::time::Duration::from_millis(600)); network_printers() }, move |list| {
            if p.borrow().is_none() && list.is_empty() { return; }
            for n in &list { store.insert_with_values(None, &[(0, &n.name), (1, &n.address), (2, &(found.borrow().len() as u32))]); found.borrow_mut().push(n.clone()); }
            d(if list.is_empty() { "No printers were found" } else { "Select a printer" });
        });
    }
    let (u, w, tv2) = (ui.clone(), wz.clone(), tv.clone());
    next.connect_clicked(move |_| {
        let Some((m, it)) = tv2.selection().selected() else { return };
        let i = m.value(&it, 2).get::<u32>().unwrap_or(0) as usize;
        if let Some(p) = found.borrow().get(i).cloned() { name_page(&u, &w, p, false); }
    });
}

/// "Find a printer by name or TCP/IP address".
fn by_address_page(ui: &Rc<Ui>, wz: &Rc<Wizard>) {
    { let (u, w) = (ui.clone(), wz.clone()); set_back(wz, Some(Rc::new(move || search_page(&u, &w)))); }
    wz.set_foot(None);
    let v = vbox(8);
    v.pack_start(&xc::instr("Find a printer by name or TCP/IP address"), false, false, 0);
    let rs = radios(&["_Browse for a printer", "_Select a shared printer by name", "_Add a printer using a TCP/IP address or hostname"], 2);
    let shared = gtk::Entry::new();
    shared.set_placeholder_text(Some("ipp://server:631/printers/name"));
    shared.set_margin_start(24);
    let host = gtk::Entry::new();
    host.set_placeholder_text(Some("192.168.1.20 or printer.local"));
    let kind = combo(&[("ipp", "IPP Device"), ("socket", "TCP/IP Device (port 9100)"), ("lpd", "LPD/LPR Device")], "ipp");
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    g.set_margin_start(24);
    g.attach(&label("Device type:", ""), 0, 0, 1, 1);
    g.attach(&kind, 1, 0, 1, 1);
    g.attach(&label("Hostname or IP address:", ""), 0, 1, 1, 1);
    g.attach(&host, 1, 1, 1, 1);
    host.set_hexpand(true);
    v.pack_start(&rs[0], false, false, 0);
    v.pack_start(&rs[1], false, false, 0);
    v.pack_start(&shared, false, false, 0);
    v.pack_start(&label("Example: \\\\computername\\printername or ipp://computername/printers/printername", "cp-hint"), false, false, 0);
    v.pack_start(&rs[2], false, false, 0);
    v.pack_start(&g, false, false, 0);
    let sync = { let (s, g, r1, r2) = (shared.clone(), g.clone(), rs[1].clone(), rs[2].clone()); move || { s.set_sensitive(r1.is_active()); g.set_sensitive(r2.is_active()); } };
    sync();
    for r in &rs { let f = sync.clone(); r.connect_toggled(move |_| f()); }
    wz.page(&v);
    let next = gtk::Button::with_mnemonic("_Next");
    wz.set_buttons(&[&next, &cancel_button(wz)]);
    let (u, w, rs2) = (ui.clone(), wz.clone(), rs.clone());
    next.connect_clicked(move |_| {
        if rs2[0].is_active() { search_page(&u, &w); return; }
        let p = if rs2[1].is_active() {
            let t = shared.text().trim().replace('\\', "/");
            if t.is_empty() { return; }
            // \\server\printer → the server's CUPS queue (or a Windows share through Samba)
            let uri = if t.contains("://") { t.clone() } else {
                let parts: Vec<&str> = t.trim_start_matches('/').splitn(2, '/').collect();
                if parts.len() < 2 { return; }
                format!("smb://{}/{}", parts[0], parts[1])
            };
            NewPrinter { name: uri.rsplit('/').next().unwrap_or("Printer").replace('_', " "), address: uri_host(&uri), driver: "everywhere".into(), uri, model: String::new() }
        } else {
            let h = host.text().trim().to_string();
            if h.is_empty() { return; }
            let uri = match kind.active_id().as_deref() { Some("socket") => format!("socket://{h}:9100"), Some("lpd") => format!("lpd://{h}/queue"), _ => format!("ipp://{h}/ipp/print") };
            let driverless = uri.starts_with("ipp");
            NewPrinter { name: format!("Printer on {h}"), address: h, uri, driver: if driverless { "everywhere".into() } else { String::new() }, model: String::new() }
        };
        if p.driver.is_empty() { driver_page(&u, &w, p); } else { name_page(&u, &w, p, false); }
    });
}

/// "Choose a printer port" (local printers): the parallel ports, or a new TCP/IP port.
fn port_page(ui: &Rc<Ui>, wz: &Rc<Wizard>) {
    { let (u, w) = (ui.clone(), wz.clone()); set_back(wz, Some(Rc::new(move || printer_type_page(&u, &w)))); }
    wz.set_foot(None);
    let v = vbox(8);
    v.pack_start(&xc::instr("Choose a printer port"), false, false, 0);
    v.pack_start(&label("A printer port is a type of connection that allows your computer to exchange information with a printer.", ""), false, false, 0);
    let rs = radios(&["_Use an existing port:", "_Create a new port:"], 0);
    let mut ports: Vec<(String, String)> = (0..4).filter(|i| std::path::Path::new(&format!("/dev/lp{i}")).exists()).map(|i| (format!("parallel:/dev/lp{i}"), format!("LPT{}: (Printer Port)", i + 1))).collect();
    for e in std::fs::read_dir("/dev/usb").into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with("lp") { ports.push((format!("file:/dev/usb/{n}"), format!("USB{:03} (Virtual printer port for USB)", n[2..].parse::<u32>().unwrap_or(0) + 1))); }
    }
    ports.push(("file:/dev/null".into(), "FILE: (Print to File)".into()));
    let port_list: Vec<(&str, &str)> = ports.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let existing = combo(&port_list, &ports[0].0);
    let kind = combo(&[("socket", "Standard TCP/IP Port"), ("ipp", "Internet Printing Protocol (IPP)"), ("lpd", "LPR Port")], "socket");
    let host = gtk::Entry::new();
    host.set_placeholder_text(Some("Hostname or IP address"));
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    g.attach(&rs[0], 0, 0, 1, 1);
    g.attach(&existing, 1, 0, 1, 1);
    g.attach(&rs[1], 0, 1, 1, 1);
    let nk = hbox(6);
    nk.pack_start(&label("Type of port:", ""), false, false, 0);
    nk.pack_start(&kind, true, true, 0);
    g.attach(&nk, 1, 1, 1, 1);
    g.attach(&host, 1, 2, 1, 1);
    existing.set_hexpand(true);
    v.pack_start(&g, false, false, 8);
    let sync = { let (e, k, h, r) = (existing.clone(), kind.clone(), host.clone(), rs[0].clone()); move || { let a = r.is_active(); e.set_sensitive(a); k.set_sensitive(!a); h.set_sensitive(!a); } };
    sync();
    for r in &rs { let f = sync.clone(); r.connect_toggled(move |_| f()); }
    wz.page(&v);
    let next = gtk::Button::with_mnemonic("_Next");
    wz.set_buttons(&[&next, &cancel_button(wz)]);
    let (u, w, r0) = (ui.clone(), wz.clone(), rs[0].clone());
    next.connect_clicked(move |_| {
        let uri = if r0.is_active() { existing.active_id().map(|s| s.to_string()).unwrap_or_default() } else {
            let h = host.text().trim().to_string();
            if h.is_empty() { host.grab_focus(); return; }
            match kind.active_id().as_deref() { Some("ipp") => format!("ipp://{h}/ipp/print"), Some("lpd") => format!("lpd://{h}/queue"), _ => format!("socket://{h}:9100") }
        };
        driver_page(&u, &w, NewPrinter { name: String::new(), address: uri_host(&uri), uri, driver: String::new(), model: String::new() });
    });
}

/// "Install the printer driver": Manufacturer and Printers lists from CUPS' drivers.
fn driver_page(ui: &Rc<Ui>, wz: &Rc<Wizard>, p: NewPrinter) {
    { let (u, w) = (ui.clone(), wz.clone()); set_back(wz, Some(Rc::new(move || port_page(&u, &w)))); }
    wz.set_foot(None);
    let v = vbox(8);
    v.pack_start(&xc::instr("Install the printer driver"), false, false, 0);
    let h = hbox(10);
    h.pack_start(&img(&["printer"], 32), false, false, 0);
    h.pack_start(&label("Choose your printer from the list. Click Windows Update to see more models.\n\nTo install the driver from an installation CD, click Have Disk.", ""), true, true, 0);
    v.pack_start(&h, false, false, 0);
    let makers = gtk::ListStore::new(&[glib::Type::STRING]);
    let models = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let (mt, pt) = (gtk::TreeView::with_model(&makers), gtk::TreeView::with_model(&models));
    columns(&mt, &[("Manufacturer", 0, 0)]);
    columns(&pt, &[("Printers", 0, 0)]);
    let row = hbox(8);
    let (ms, ps) = (scrolled(&mt), scrolled(&pt));
    ms.set_size_request(180, 170);
    ps.set_size_request(-1, 170);
    row.pack_start(&ms, false, false, 0);
    row.pack_start(&ps, true, true, 0);
    v.pack_start(&row, true, true, 0);
    let status = label("Loading the list of printers...", "cp-hint");
    v.pack_start(&status, false, false, 0);
    let bh = hbox(8);
    bh.set_halign(gtk::Align::End);
    let update = gtk::Button::with_mnemonic("_Windows Update");
    update.set_sensitive(false);
    update.set_tooltip_text(Some("Printer drivers come with your distribution's packages (cups-filters, gutenprint, hplip…)."));
    let disk = gtk::Button::with_mnemonic("_Have Disk...");
    bh.pack_start(&update, false, false, 0);
    bh.pack_start(&disk, false, false, 0);
    v.pack_start(&bh, false, false, 0);
    wz.page(&v);
    let next = gtk::Button::with_mnemonic("_Next");
    next.set_sensitive(false);
    wz.set_buttons(&[&next, &cancel_button(wz)]);
    let all: Rc<RefCell<Vec<(String, String, String)>>> = Rc::new(RefCell::new(vec![]));
    {
        let (makers, all, status) = (makers.clone(), all.clone(), status.clone());
        bg(|| drivers(&sh(&["lpinfo", "-m"])), move |d| {
            let mut seen: Vec<String> = vec![];
            for (m, _, _) in &d { if !seen.contains(m) { seen.push(m.clone()); makers.insert_with_values(None, &[(0, m)]); } }
            status.set_text(if d.is_empty() { "No printer drivers were found. Install cups-filters (driverless printing), gutenprint or hplip." } else { "" });
            *all.borrow_mut() = d;
        });
    }
    { let (models, all) = (models.clone(), all.clone()); mt.selection().connect_changed(move |s| {
        models.clear();
        let Some((m, it)) = s.selected() else { return };
        let maker = sval(&m, &it, 0);
        for (mk, model, ppd) in all.borrow().iter() { if *mk == maker { models.insert_with_values(None, &[(0, model), (1, ppd)]); } }
    }); }
    { let n = next.clone(); pt.selection().connect_changed(move |s| n.set_sensitive(s.selected().is_some())); }
    let picked: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
    {
        let (w, picked, n) = (wz.clone(), picked.clone(), next.clone());
        disk.connect_clicked(move |_| {
            let fc = gtk::FileChooserNative::new(Some("Install From Disk"), Some(&w.win), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
            let f = gtk::FileFilter::new();
            f.set_name(Some("Printer driver (*.ppd, *.ppd.gz)"));
            f.add_pattern("*.ppd");
            f.add_pattern("*.PPD");
            f.add_pattern("*.ppd.gz");
            fc.add_filter(f);
            if fc.run() == gtk::ResponseType::Accept {
                if let Some(path) = fc.filename() {
                    let model = path.file_stem().map(|s| s.to_string_lossy().trim_end_matches(".ppd").to_string()).unwrap_or_default();
                    *picked.borrow_mut() = Some((model, format!("ppdfile:{}", path.display())));
                    n.set_sensitive(true);
                    n.clicked();
                }
            }
        });
    }
    let (u, w) = (ui.clone(), wz.clone());
    next.connect_clicked(move |_| {
        let (model, ppd) = match picked.borrow_mut().take() {
            Some(x) => x,
            None => { let Some((m, it)) = pt.selection().selected() else { return }; (sval(&m, &it, 0), sval(&m, &it, 1)) }
        };
        let mut p = p.clone();
        p.name = model.split(',').next().unwrap_or(&model).trim().to_string();
        p.driver = ppd;
        p.model = model;
        name_page(&u, &w, p, true);
    });
}

/// "Type a printer name", then sharing (local printers) and the install.
fn name_page(ui: &Rc<Ui>, wz: &Rc<Wizard>, p: NewPrinter, local: bool) {
    { let (u, w) = (ui.clone(), wz.clone()); set_back(wz, Some(Rc::new(move || if local { port_page(&u, &w) } else { search_page(&u, &w) }))); }
    wz.set_foot(None);
    let v = vbox(10);
    v.pack_start(&xc::instr("Type a printer name"), false, false, 0);
    let h = hbox(10);
    h.pack_start(&label("Printer name:", ""), false, false, 0);
    let e = gtk::Entry::new();
    e.set_text(&p.name);
    e.set_hexpand(true);
    h.pack_start(&e, true, true, 0);
    v.pack_start(&h, false, false, 0);
    let drv = if p.driver == "everywhere" { "the driverless (IPP Everywhere)".to_string() } else { format!("the {}", if p.model.is_empty() { &p.driver } else { &p.model }) };
    v.pack_start(&label(&format!("This printer will be installed with {drv} driver."), ""), false, false, 0);
    let share = check("_Share this printer so that others on your network can find and use it", false);
    v.pack_start(&share, false, false, 8);
    wz.page(&v);
    let next = xc::shield_button("_Next");
    wz.set_buttons(&[&next, &cancel_button(wz)]);
    let (u, w) = (ui.clone(), wz.clone());
    next.connect_clicked(move |b| {
        let shown = e.text().trim().to_string();
        if shown.is_empty() { e.grab_focus(); return; }
        let q = queue_name(&shown);
        let mut argv = vec!["lpadmin".to_string(), "-p".into(), q.clone(), "-E".into(), "-v".into(), p.uri.clone(), "-D".into(), shown.clone()];
        if let Some(f) = p.driver.strip_prefix("ppdfile:") { argv.push("-P".into()); argv.push(f.to_string()); } else { argv.push("-m".into()); argv.push(p.driver.clone()); }
        if share.is_active() { argv.push("-o".into()); argv.push("printer-is-shared=true".into()); }
        b.set_sensitive(false);
        w.page(&xc::loading("Installing printer..."));
        let (u2, w2, b2) = (u.clone(), w.clone(), b.clone());
        let a: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        elevate(&a, move |ok, out| {
            b2.set_sensitive(true);
            if !ok {
                report("Windows could not add the printer.", ok, &out);
                w2.win.close();
                return;
            }
            reload(&u2);
            done_page(&u2, &w2, &q, &shown);
        });
    });
}

/// "You've successfully added …".
fn done_page(ui: &Rc<Ui>, wz: &Rc<Wizard>, queue: &str, shown: &str) {
    set_back(wz, None);
    let v = vbox(10);
    v.pack_start(&xc::instr(&format!("You've successfully added {shown}")), false, false, 0);
    let def = check("Set as the _default printer", false);
    v.pack_start(&def, false, false, 6);
    let test = gtk::Button::with_mnemonic("_Print a test page");
    test.set_halign(gtk::Align::Start);
    v.pack_start(&label("To check if your printer is working properly, or to see troubleshooting information for the printer, print a test page.", ""), false, false, 6);
    v.pack_start(&test, false, false, 0);
    { let q = queue.to_string(); test.connect_clicked(move |_| {
        let page = ["/usr/share/cups/data/testprint", "/usr/share/cups/data/testprint.ps"].into_iter().find(|p| std::path::Path::new(p).exists()).unwrap_or("/usr/share/cups/data/testprint");
        let q = q.clone();
        bg(move || run(&["lp", "-d", &q, page]), |(ok, out)| { report("The test page could not be printed.", ok, &out); });
    }); }
    wz.page(&v);
    let finish = gtk::Button::with_mnemonic("_Finish");
    wz.set_buttons(&[&finish, &cancel_button(wz)]);
    let (u, w, q) = (ui.clone(), wz.clone(), queue.to_string());
    finish.connect_clicked(move |_| {
        if def.is_active() { let (u, q) = (u.clone(), q.clone()); bg(move || run(&["lpoptions", "-d", &q]), move |(ok, out)| { report("The default printer could not be set.", ok, &out); reload(&u); }); }
        w.win.close();
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn edid() {
        let mut e = vec![0u8; 128];
        // "DEL" = 00100 00101 01100
        let v: u16 = (4 << 10) | (5 << 5) | 12;
        e[8] = (v >> 8) as u8; e[9] = v as u8;
        let o = 54;
        e[o + 3] = 0xFC;
        e[o + 5..o + 5 + 8].copy_from_slice(b"DELL U27");
        e[o + 13] = 0x0a;
        let (m, n) = super::edid_names(&e);
        assert_eq!(m, "Dell");
        assert_eq!(n, "DELL U27");
    }
    #[test]
    fn printers() {
        assert_eq!(super::uri_host("ipp://HP1234.local:631/ipp/print"), "HP1234.local");
        assert_eq!(super::uri_host("socket://192.168.1.20:9100"), "192.168.1.20");
        assert_eq!(super::uri_host("dnssd://HP%20LaserJet._ipp._tcp.local/"), "HP LaserJet");
        assert_eq!(super::queue_name("HP LaserJet P1102 (Office)"), "HP_LaserJet_P1102__Office");
        assert_eq!(super::queue_name("  "), "Printer");
        let o = "copies=1 device-uri=usb://HP/LaserJet printer-make-and-model='HP LaserJet Professional P1102, hpcups 3.24' printer-state=3";
        assert_eq!(super::lp_opt(o, "device-uri"), "usb://HP/LaserJet");
        assert_eq!(super::lp_opt(o, "printer-make-and-model"), "HP LaserJet Professional P1102, hpcups 3.24");
        assert_eq!(super::lp_opt(o, "copies"), "1");
        assert_eq!(super::lp_opt(o, "nothing"), "");
        let j = super::jobs("HP", "HP is ready and printing\nRank    Owner   Job     File(s)                         Total Size\nactive  demid   12      my report.pdf                   24576 bytes\n1st     root    13      (stdin)                         100 bytes\n",
            "HP-12 demid 24576 Mon 02 Nov 2026 10:12:00 AM CET\nHP-13 root 100 Mon 02 Nov 2026 10:13:00 AM CET\n");
        assert_eq!(j.len(), 2);
        assert_eq!((j[0].id.as_str(), j[0].doc.as_str(), j[0].status.as_str(), j[0].owner.as_str()), ("HP-12", "my report.pdf", "Printing", "demid"));
        assert_eq!(j[0].submitted, "Mon 02 Nov 2026 10:12:00 AM CET");
        assert_eq!(j[1].status, "");
        let o = super::ppd_options("PageSize/Media Size: *Letter Legal A4\nDuplex/2-Sided Printing: *None DuplexNoTumble DuplexTumble\n");
        assert_eq!(o[0], super::PpdOption { key: "PageSize".into(), label: "Media Size".into(), choices: vec!["Letter".into(), "Legal".into(), "A4".into()], current: "Letter".into() });
        assert_eq!(o[1].current, "None");
        let d = super::drivers("everywhere IPP Everywhere\ndrv:///hpcups.drv/hp-laserjet_p1102.ppd HP LaserJet Professional P1102, hpcups 3.24\ngutenprint.5.3://epson-sx100/expert Epson Stylus SX100 - CUPS+Gutenprint v5.3.4\n");
        assert_eq!(d[0], ("Epson".to_string(), "Epson Stylus SX100 - CUPS+Gutenprint v5.3.4".to_string(), "gutenprint.5.3://epson-sx100/expert".to_string()));
        assert_eq!(d[1].0, "Generic");
        assert_eq!(d[2].0, "HP");
    }
}
