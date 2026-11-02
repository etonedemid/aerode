//! Network and Sharing Center on NetworkManager (nmcli; /sys and getifaddrs without it): the basic
//! network map (this computer — network — Internet), the active networks with their location
//! (Home/Work/Public = the connection's firewalld zone), Change your networking settings,
//! Manage wireless networks (NM Wi-Fi profiles in autoconnect-priority order), Network
//! Connections (Change adapter settings: tiles, enable/disable, Status with speed, IPv4/IPv6 and
//! bytes, Details, Windows Network Diagnostics) and Advanced sharing settings (Avahi, Samba,
//! CUPS printer sharing, the Public folder usershare).
use super::*;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Dev {
    pub iface: String,
    /// ethernet | wifi | other NM types
    pub kind: String,
    /// connected | disconnected | unavailable | unmanaged | connecting…
    pub state: String,
    pub connection: String,
    pub uuid: String,
    pub vendor: String,
    pub product: String,
    pub driver: String,
    pub mac: String,
    pub autoconnect: bool,
    pub ip4: Vec<String>,
    pub gw4: String,
    pub dns4: Vec<String>,
    pub domain: String,
    pub ip6: Vec<String>,
    pub gw6: String,
    pub dns6: Vec<String>,
    pub dhcp: HashMap<String, String>,
    pub ssid: String,
    pub signal: u32,
    pub rate: String,
    pub zone: String,
}

impl Dev {
    pub fn up(&self) -> bool { self.state.starts_with("connected") }
    pub fn wifi(&self) -> bool { self.kind == "wifi" }
    /// Win7's adapter icon state line ("Network cable unplugged", "Disabled", the network name…).
    pub fn status_line(&self) -> String {
        if self.up() { return if self.wifi() && !self.ssid.is_empty() { self.ssid.clone() } else { self.connection.clone() }; }
        match self.state.as_str() {
            "unavailable" if self.kind == "ethernet" => "Network cable unplugged".into(),
            "unavailable" => "Not connected".into(),
            "unmanaged" => "Unmanaged".into(),
            "disconnected" if !self.autoconnect => "Disabled".into(),
            s if s.starts_with("connecting") => "Identifying...".into(),
            _ => "Not connected".into(),
        }
    }
    pub fn description(&self) -> String {
        let p = format!("{} {}", self.vendor.split(' ').next().unwrap_or(""), self.product).trim().to_string();
        if p.is_empty() { self.driver.clone() } else { p }
    }
    pub fn location(&self) -> &'static str {
        match self.zone.as_str() { "home" | "internal" | "trusted" => "Home network", "work" => "Work network", _ => "Public network" }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Net { pub devs: Vec<Dev>, pub connectivity: String, pub nm: bool }

impl Net {
    pub fn internet(&self) -> bool { matches!(self.connectivity.as_str(), "full" | "unknown") && self.devs.iter().any(|d| d.up() && !d.gw4.is_empty() || d.up() && !d.gw6.is_empty()) }
    pub fn active(&self) -> Vec<&Dev> { self.devs.iter().filter(|d| d.up()).collect() }
}

/// Addresses of every interface: iface → (IPv4 "a.b.c.d/nn", IPv6 "x::y/nn").
fn ifaddrs() -> HashMap<String, (Vec<String>, Vec<String>)> {
    let mut m: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    unsafe {
        let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifa) != 0 { return m; }
        let mut p = ifa;
        while !p.is_null() {
            let a = &*p;
            p = a.ifa_next;
            if a.ifa_addr.is_null() { continue; }
            let name = std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy().into_owned();
            let fam = (*a.ifa_addr).sa_family as i32;
            if fam == libc::AF_INET {
                let s = &*(a.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(s.sin_addr.s_addr));
                let bits = if a.ifa_netmask.is_null() { 32 } else { u32::from_be((*(a.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr).count_ones() };
                m.entry(name).or_default().0.push(format!("{ip}/{bits}"));
            } else if fam == libc::AF_INET6 {
                let s = &*(a.ifa_addr as *const libc::sockaddr_in6);
                let ip = std::net::Ipv6Addr::from(s.sin6_addr.s6_addr);
                let bits: u32 = if a.ifa_netmask.is_null() { 128 } else { (*(a.ifa_netmask as *const libc::sockaddr_in6)).sin6_addr.s6_addr.iter().map(|b| b.count_ones()).sum() };
                m.entry(name).or_default().1.push(format!("{ip}/{bits}"));
            }
        }
        libc::freeifaddrs(ifa);
    }
    m
}

fn sysfs(iface: &str, f: &str) -> String { std::fs::read_to_string(format!("/sys/class/net/{iface}/{f}")).unwrap_or_default().trim().to_string() }

/// Everything the Network and Sharing Center shows (worker thread).
pub fn gather() -> Net {
    let mut net = Net::default();
    if util::which("nmcli") {
        net.nm = true;
        net.connectivity = sh(&["nmcli", "-t", "-f", "CONNECTIVITY", "general"]).trim().to_string();
        let wifi = sh(&["nmcli", "-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY,RATE", "device", "wifi", "list"]);
        for l in sh(&["nmcli", "-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).lines() {
            let f = aerode::sys::nm_split(l);
            if f.len() < 4 || matches!(f[1].as_str(), "loopback" | "bridge" | "tun" | "veth" | "dummy" | "wifi-p2p") { continue; }
            let mut d = Dev { iface: f[0].clone(), kind: f[1].clone(), state: f[2].clone(), connection: f[3].clone(), autoconnect: true, ..Default::default() };
            for l in sh(&["nmcli", "-t", "-f", "all", "device", "show", &d.iface]).lines() {
                let Some((k, v)) = l.split_once(':') else { continue };
                let v = v.replace("\\:", ":");
                match k {
                    "GENERAL.VENDOR" => d.vendor = v, "GENERAL.PRODUCT" => d.product = v, "GENERAL.DRIVER" => d.driver = v,
                    "GENERAL.HWADDR" => d.mac = v, "GENERAL.CON-UUID" => d.uuid = v, "GENERAL.AUTOCONNECT" => d.autoconnect = v == "yes",
                    "IP4.GATEWAY" => d.gw4 = v, "IP6.GATEWAY" => d.gw6 = v,
                    k if k.starts_with("IP4.ADDRESS") => d.ip4.push(v), k if k.starts_with("IP6.ADDRESS") => d.ip6.push(v),
                    k if k.starts_with("IP4.DNS") => d.dns4.push(v), k if k.starts_with("IP6.DNS") => d.dns6.push(v),
                    k if k.starts_with("IP4.DOMAIN") => if d.domain.is_empty() { d.domain = v },
                    k if k.starts_with("DHCP4.OPTION") => if let Some((a, b)) = v.split_once(" = ") { d.dhcp.insert(a.trim().into(), b.trim().into()); },
                    _ => {}
                }
            }
            if d.wifi() && d.up() {
                if let Some(w) = wifi.lines().map(aerode::sys::nm_split).find(|w| w.first().map(|s| s == "*").unwrap_or(false)) {
                    d.ssid = w.get(1).cloned().unwrap_or_default();
                    d.signal = w.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    d.rate = w.get(4).cloned().unwrap_or_default();
                }
            }
            if !d.uuid.is_empty() { d.zone = sh(&["nmcli", "-t", "-g", "connection.zone", "connection", "show", &d.uuid]).trim().to_string(); }
            net.devs.push(d);
        }
        return net;
    }
    // no NetworkManager: the kernel's view
    let addrs = ifaddrs();
    let route = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    let dns: Vec<String> = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default().lines().filter_map(|l| l.strip_prefix("nameserver ").map(|s| s.trim().to_string())).collect();
    if let Ok(rd) = std::fs::read_dir("/sys/class/net") {
        let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        for n in names {
            if n == "lo" || ["veth", "docker", "br-", "virbr", "tun", "tap"].iter().any(|p| n.starts_with(p)) { continue; }
            let wifi = std::path::Path::new(&format!("/sys/class/net/{n}/wireless")).exists();
            let oper = sysfs(&n, "operstate");
            let up = oper == "up" || (oper == "unknown" && sysfs(&n, "carrier") == "1");
            let gw = route.lines().skip(1).map(|l| l.split_whitespace().collect::<Vec<_>>()).find(|f| f.len() > 2 && f[0] == n && f[1] == "00000000")
                .and_then(|f| u32::from_str_radix(f[2], 16).ok()).map(|g| std::net::Ipv4Addr::from(u32::from_be(g)).to_string()).unwrap_or_default();
            let (ip4, ip6) = addrs.get(&n).cloned().unwrap_or_default();
            net.devs.push(Dev { iface: n.clone(), kind: if wifi { "wifi".into() } else { "ethernet".into() },
                state: if up { "connected".into() } else if sysfs(&n, "carrier") == "0" { "unavailable".into() } else { "disconnected".into() },
                connection: n.clone(), mac: sysfs(&n, "address").to_uppercase(), autoconnect: true, ip4, ip6, gw4: gw, dns4: dns.clone(), driver: std::fs::read_link(format!("/sys/class/net/{n}/device/driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default(), ..Default::default() });
        }
    }
    net.connectivity = if net.devs.iter().any(|d| d.up() && !d.gw4.is_empty()) { "full".into() } else { "none".into() };
    net
}

/// Win7 names its adapters by type: Local Area Connection, Wireless Network Connection 2…
fn adapter_names(devs: &[Dev]) -> Vec<String> {
    let (mut e, mut w, mut o) = (0, 0, 0);
    devs.iter().map(|d| {
        let (base, n) = match d.kind.as_str() { "ethernet" => ("Local Area Connection", { e += 1; e }), "wifi" => ("Wireless Network Connection", { w += 1; w }), _ => ("Network Connection", { o += 1; o }) };
        if n == 1 { base.to_string() } else { format!("{base} {n}") }
    }).collect()
}

fn dev_icon(d: &Dev) -> &'static [&'static str] {
    match (d.kind.as_str(), d.up()) {
        ("wifi", true) => match d.signal { 0..=24 => &["network-wireless-signal-weak"], 25..=49 => &["network-wireless-signal-ok"], 50..=74 => &["network-wireless-signal-good"], _ => &["network-wireless-signal-excellent", "network-wireless"] },
        ("wifi", false) => &["network-wireless-disconnected", "network-wireless-signal-none", "network-wireless"],
        (_, true) => &["network-wired", "network-transmit-receive"],
        _ => &["network-wired-disconnected", "network-offline", "network-wired"],
    }
}

// ------------------------------------------------------------------------------- page

impl Cp {
    pub(crate) fn page_network(self: &Rc<Self>) -> gtk::Widget {
        css();
        match sub(self, "network").as_deref() {
            Some("wireless") => return wireless_page(self),
            Some("adapters") => return adapters_page(self),
            Some("sharing") => return sharing_page(self),
            _ => {}
        }
        self.side_link("Manage wireless networks", "network/wireless");
        self.side_link("Change adapter settings", "network/adapters");
        self.side_link("Change advanced sharing settings", "network/sharing");
        see_also(self, &[("HomeGroup", "homegroup"), ("Internet Options", "internet"), ("Windows Firewall", "firewall")]);
        let v = vbox(8);
        let top = hbox(0);
        top.pack_start(&title("View your basic network information and set up connections"), true, true, 0);
        v.pack_start(&top, false, false, 0);
        let map = vbox(0);
        v.pack_start(&map, false, false, 0);
        let active = vbox(8);
        let ah = hbox(0);
        let sec = section("View your active networks");
        ah.pack_start(&sec, true, true, 0);
        { let me = self.clone(); let l = link("Connect or disconnect", move || connect_dialog(&me)); l.set_valign(gtk::Align::End); ah.pack_end(&l, false, false, 0); }
        v.pack_start(&ah, false, false, 0);
        v.pack_start(&rule(), false, false, 0);
        v.pack_start(&active, false, false, 0);
        v.pack_start(&section("Change your networking settings"), false, false, 0);
        v.pack_start(&rule(), false, false, 0);
        let tasks = vbox(10);
        tasks.set_margin_start(8);
        { let me = self.clone(); tasks.pack_start(&task(&["network-workgroup", "preferences-system-network"], "Set up a new connection or network", "Set up a wireless, broadband, dial-up, ad hoc, or VPN connection; or set up a router or access point.", move || new_connection(&me)), false, false, 0); }
        { let me = self.clone(); tasks.pack_start(&task(&["network-wireless", "network-workgroup"], "Connect to a network", "Connect or reconnect to a wireless, wired, dial-up, or VPN network connection.", move || connect_dialog(&me)), false, false, 0); }
        { let me = self.clone(); tasks.pack_start(&task(&["folder-publicshare", "network-workgroup"], "Choose homegroup and sharing options", "Access files and printers located on other network computers, or change sharing settings.", move || go(&me, "homegroup")), false, false, 0); }
        { let me = self.clone(); tasks.pack_start(&task(&["help-browser", "dialog-question"], "Troubleshoot problems", "Diagnose and repair network problems, or get troubleshooting information.", move || diagnose(&me.win.clone().upcast(), None)), false, false, 0); }
        v.pack_start(&tasks, false, false, 0);
        map.pack_start(&label("Checking the network...", "cp-hint"), false, false, 8);
        let me = self.clone();
        bg(gather, move |net| fill_main(&me, &net, &map, &active));
        v.upcast()
    }
}

fn rule() -> gtk::Box { let r = hbox(0); r.style_context().add_class("ac-rule"); r }

fn fill_main(cp: &Rc<Cp>, net: &Net, map: &gtk::Box, active: &gtk::Box) {
    for c in map.children() { map.remove(&c); }
    // computer —— network —— Internet
    let row = hbox(0);
    row.set_margin_top(6);
    row.set_margin_bottom(4);
    let node = |icons: &[&str], name: &str, sub: &str| {
        let b = vbox(2);
        b.style_context().add_class("nsc-node");
        b.set_size_request(150, -1);
        b.pack_start(&img(icons, 48), false, false, 0);
        let l = label(name, "");
        l.set_justify(gtk::Justification::Center);
        l.set_xalign(0.5);
        l.set_max_width_chars(20);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        b.pack_start(&l, false, false, 0);
        if !sub.is_empty() { let s = label(sub, "dim"); s.set_xalign(0.5); b.pack_start(&s, false, false, 0); }
        b
    };
    let line = |ok: bool| {
        let o = vbox(0);
        o.set_valign(gtk::Align::Start);
        o.set_margin_top(22);
        let h = hbox(0);
        let l = hbox(0);
        l.style_context().add_class(if ok { "nsc-line" } else { "nsc-line-bad" });
        l.set_size_request(70, 2);
        l.set_valign(gtk::Align::Center);
        h.pack_start(&l, true, true, 0);
        if !ok { let x = label("✕", "bad-text"); x.set_margin_start(2); x.set_margin_end(2); h.set_center_widget(Some(&x)); }
        o.pack_start(&h, false, false, 0);
        o
    };
    let host = glib::host_name().to_string();
    row.pack_start(&node(&["computer", "user-desktop-computer"], &host, "(This computer)"), false, false, 0);
    let act = net.active();
    match act.first() {
        Some(d) => {
            let name = if act.len() > 1 { "Multiple networks".to_string() } else if !d.ssid.is_empty() { d.ssid.clone() } else { d.connection.clone() };
            row.pack_start(&line(true), false, false, 0);
            row.pack_start(&node(&["network-workgroup", "network-wired"], &name, ""), false, false, 0);
            row.pack_start(&line(net.internet()), false, false, 0);
            row.pack_start(&node(&["web-browser", "applications-internet", "network-workgroup"], "Internet", ""), false, false, 0);
        }
        None => {
            row.pack_start(&line(false), false, false, 0);
            row.pack_start(&node(&["network-offline", "network-error"], "Not connected", ""), false, false, 0);
        }
    }
    let fm = { let me = cp.clone(); link("See full map", move || full_map(&me)) };
    fm.set_valign(gtk::Align::Start);
    if let Some(l) = fm.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_line_wrap(false); }
    row.pack_end(&fm, false, false, 0);
    map.pack_start(&row, false, false, 0);
    map.show_all();
    // active networks
    for c in active.children() { active.remove(&c); }
    if act.is_empty() {
        active.pack_start(&label("You are currently not connected to any networks.", ""), false, false, 0);
    }
    let names = adapter_names(&net.devs);
    for d in act {
        let h = hbox(14);
        h.set_margin_start(8);
        h.pack_start(&img(&["network-workgroup"], 48), false, false, 0);
        let left = vbox(1);
        left.set_size_request(230, -1);
        left.pack_start(&bold(if !d.ssid.is_empty() { &d.ssid } else { &d.connection }), false, false, 0);
        let (me, dd) = (cp.clone(), d.clone());
        left.pack_start(&link(d.location(), move || set_location(&me, &dd)), false, false, 0);
        h.pack_start(&left, false, false, 0);
        let g = gtk::Grid::new();
        g.set_column_spacing(10);
        g.set_row_spacing(3);
        let k = |t: &str| { let l = label(t, "nsc-k"); l.set_line_wrap(false); l };
        g.attach(&k("Access type:"), 0, 0, 1, 1);
        g.attach(&label(if net.internet() && !d.gw4.is_empty() { "Internet" } else if !d.ip4.is_empty() { "No Internet access" } else { "No network access" }, ""), 1, 0, 1, 1);
        g.attach(&k("Connections:"), 0, 1, 1, 1);
        let idx = net.devs.iter().position(|x| x.iface == d.iface).unwrap_or(0);
        let cname = format!("{}{}", names.get(idx).cloned().unwrap_or_default(), if d.wifi() { format!(" ({})", d.ssid) } else { String::new() });
        let ch = hbox(4);
        ch.pack_start(&img(dev_icon(d), 16), false, false, 0);
        let (me, dd, nm) = (cp.clone(), d.clone(), names.get(idx).cloned().unwrap_or_default());
        ch.pack_start(&link(&cname, move || status_dialog(&me, &dd, &nm)), false, false, 0);
        g.attach(&ch, 1, 1, 1, 1);
        h.pack_start(&g, false, false, 0);
        active.pack_start(&h, false, false, 0);
    }
    active.show_all();
}

/// "Set Network Location": Home / Work / Public = the connection's firewalld zone.
fn set_location(cp: &Rc<Cp>, d: &Dev) {
    let dl = dialog(Some(&cp.win.clone().upcast()), "Set Network Location", &[("Cancel", gtk::ResponseType::Cancel)]);
    let c = dl.content_area();
    c.pack_start(&label("Select a location for the network", "wu-title"), false, false, 0);
    c.pack_start(&label(&format!("This computer is connected to a network. Windows will automatically apply the correct network settings based on the network's location: \"{}\".", if d.ssid.is_empty() { &d.connection } else { &d.ssid }), ""), false, false, 0);
    let opts = [("home", "Home network", "If all the computers on this network are at your home, and you recognize them, this is a trusted home network. Don't choose this for public places such as coffee shops or airports.", &["user-home", "network-workgroup"][..]),
        ("work", "Work network", "If all the computers on this network are at your workplace, and you recognize them, this is a trusted work network. Don't choose this for public places such as coffee shops or airports.", &["network-workgroup"][..]),
        ("public", "Public network", "If you don't recognize all the computers on the network (for example, you're in a coffee shop or airport, or you have mobile broadband), this is a public network and is not trusted.", &["network-workgroup"][..])];
    for (zone, name, desc, icon) in opts {
        let t = task(icon, name, desc, { let (dl, uuid, me) = (dl.clone(), d.uuid.clone(), cp.clone()); move || {
            dl.close();
            let me = me.clone();
            bg({ let uuid = uuid.clone(); move || run(&["nmcli", "connection", "modify", &uuid, "connection.zone", zone]) }, move |(ok, out)| {
                if !ok { report("The network location could not be changed.", ok, &out); }
                go(&me, "network");
            });
        } });
        c.pack_start(&t, false, false, 4);
    }
    c.pack_start(&label("Treat all future networks that I connect to as public, and don't ask me again.", "cp-hint"), false, false, 0);
    dl.show_all();
    dl.connect_response(|d, _| d.close());
}

/// "Network Map": this computer, the gateway and the other computers this one has talked to.
fn full_map(cp: &Rc<Cp>) {
    let dl = dialog(Some(&cp.win.clone().upcast()), "Network Map", &[("Close", gtk::ResponseType::Close)]);
    dl.set_default_size(520, 360);
    let c = dl.content_area();
    c.pack_start(&label("Network Map", "wu-title"), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Device", 0, 180), ("IP address", 1, 130), ("Physical address", 2, 140), ("Connection", 3, 0)]);
    c.pack_start(&scrolled(&tv), true, true, 0);
    bg(|| {
        let net = gather();
        let gws: Vec<String> = net.devs.iter().map(|d| d.gw4.clone()).filter(|g| !g.is_empty()).collect();
        let mut rows = vec![(format!("{} (This computer)", glib::host_name()), net.devs.iter().flat_map(|d| d.ip4.first().cloned()).next().unwrap_or_default().split('/').next().unwrap_or("").to_string(), String::new(), String::new())];
        for l in std::fs::read_to_string("/proc/net/arp").unwrap_or_default().lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 6 || f[3] == "00:00:00:00:00:00" { continue; }
            let name = if gws.iter().any(|g| g == f[0]) { "Gateway".to_string() } else {
                // reverse lookup is quick on a LAN (mDNS/router DNS); keep the address when it fails
                (f[0].parse::<std::net::IpAddr>().ok()).and_then(|ip| dns_name(ip)).unwrap_or_else(|| "Computer".into())
            };
            rows.push((name, f[0].to_string(), f[3].to_uppercase(), f[5].to_string()));
        }
        rows
    }, move |rows| { for r in rows { store.insert_with_values(None, &[(0, &r.0), (1, &r.1), (2, &r.2), (3, &r.3)]); } });
    dl.show_all();
    dl.connect_response(|d, _| d.close());
}

fn dns_name(ip: std::net::IpAddr) -> Option<String> {
    let (ok, out) = run(&["getent", "hosts", &ip.to_string()]);
    if !ok { return None; }
    out.split_whitespace().nth(1).map(|s| s.to_string())
}

// ------------------------------------------------------- set up a connection or network

use super::super::extras::common::{self as xc, Wizard};

thread_local!(static WIZ_BACK: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) });

fn wiz_back(wz: &Rc<Wizard>, f: Option<Rc<dyn Fn()>>) {
    wz.back.set_sensitive(f.is_some());
    WIZ_BACK.with(|b| *b.borrow_mut() = f);
}

fn wiz_cancel(wz: &Rc<Wizard>, label: &str) -> gtk::Button {
    let b = gtk::Button::with_mnemonic(label);
    let w = wz.win.clone();
    b.connect_clicked(move |_| w.close());
    b
}

/// The NetworkManager VPN plugins installed: (service type, Win7-style name).
fn vpn_plugins() -> Vec<(String, String)> {
    let mut v = vec![];
    for d in ["/usr/lib/NetworkManager/VPN", "/etc/NetworkManager/VPN"] {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
            let get = |k: &str| t.lines().find_map(|l| l.strip_prefix(k)).map(|s| s.trim().to_string());
            if let (Some(svc), Some(name)) = (get("service="), get("name=")) { if !v.iter().any(|(s, _): &(String, String)| *s == svc) { v.push((svc, name.to_uppercase())); } }
        }
    }
    v
}

/// "Set Up a Connection or Network": Win7's wizard on NetworkManager (nmcli): broadband (PPPoE),
/// router set up, a hidden or new wireless network, an ad hoc network (a hotspot), VPN (a plug-in
/// or an imported OpenVPN / WireGuard file) and mobile broadband dial-up.
fn new_connection(cp: &Rc<Cp>) {
    if !util::which("nmcli") {
        for t in [&["nm-connection-editor"][..], &["plasma-open-settings", "kcm_networkmanagement"]] { if util::spawn(t) { return; } }
        util::error("NetworkManager is not installed.", "Install networkmanager to set up connections from here.");
        return;
    }
    let wz = xc::wizard("Set Up a Connection or Network", "network-workgroup", Some(&cp.win.clone().upcast()), (600, 450), true);
    wz.back.connect_clicked(|_| { if let Some(f) = WIZ_BACK.with(|b| b.borrow().clone()) { f(); } });
    choose_option(cp, &wz);
    wz.win.show_all();
}

fn choose_option(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    wiz_back(wz, None);
    wz.set_foot(None);
    let v = vbox(6);
    v.pack_start(&xc::instr("Choose a connection option"), false, false, 0);
    let list = vbox(0);
    list.style_context().add_class("cp-list");
    let options: [(&[&str], &str, &str, Rc<dyn Fn()>); 6] = [
        (&["network-wired", "network-workgroup"], "Connect to the Internet", "Set up a wireless, broadband, or dial-up connection to the Internet.", { let (c, w) = (cp.clone(), wz.clone()); Rc::new(move || internet_page(&c, &w)) }),
        (&["network-wireless", "network-workgroup"], "Set up a new network", "Configure a new router or access point.", { let (c, w) = (cp.clone(), wz.clone()); Rc::new(move || router_page(&c, &w)) }),
        (&["network-wireless-signal-excellent", "network-wireless"], "Manually connect to a wireless network", "Connect to a hidden network or create a new wireless profile.", { let (c, w) = (cp.clone(), wz.clone()); Rc::new(move || { w.win.close(); add_wifi(&c, Rc::new(|| {})); }) }),
        (&["network-wireless-hotspot", "network-wireless"], "Set up a wireless ad hoc (computer-to-computer) network", "Set up a temporary network for sharing files or an Internet connection.", { let (c, w) = (cp.clone(), wz.clone()); Rc::new(move || adhoc_page(&c, &w)) }),
        (&["network-vpn", "network-workgroup"], "Connect to a workplace", "Set up a dial-up or VPN connection to your workplace.", { let (c, w) = (cp.clone(), wz.clone()); Rc::new(move || workplace_page(&c, &w)) }),
        (&["modem", "network-cellular", "phone"], "Set up a dial-up connection", "Connect to the Internet using a dial-up connection.", { let (c, w) = (cp.clone(), wz.clone()); Rc::new(move || dialup_page(&c, &w)) }),
    ];
    let chosen: Rc<Cell<usize>> = Rc::new(Cell::new(0));
    let lb = gtk::ListBox::new();
    lb.set_activate_on_single_click(false);
    for (icon, t, d, _) in options.iter() {
        let h = hbox(10);
        h.set_border_width(5);
        h.pack_start(&img(icon, 32), false, false, 0);
        let tv = vbox(1);
        tv.pack_start(&label(t, ""), false, false, 0);
        tv.pack_start(&label(d, "cp-task-desc"), false, false, 0);
        h.pack_start(&tv, true, true, 0);
        lb.add(&h);
    }
    list.pack_start(&lb, false, false, 0);
    v.pack_start(&list, false, false, 0);
    wz.page(&v);
    let next = gtk::Button::with_mnemonic("_Next");
    wz.set_buttons(&[&next, &wiz_cancel(wz, "Cancel")]);
    if let Some(r) = lb.row_at_index(0) { lb.select_row(Some(&r)); }
    { let c = chosen.clone(); lb.connect_row_selected(move |_, r| if let Some(r) = r { c.set(r.index().max(0) as usize); }); }
    let acts: Vec<Rc<dyn Fn()>> = options.iter().map(|o| o.3.clone()).collect();
    { let a = acts.clone(); lb.connect_row_activated(move |_, r| { if let Some(f) = a.get(r.index().max(0) as usize) { f(); } }); }
    next.connect_clicked(move |_| { if let Some(f) = acts.get(chosen.get()) { f(); } });
}

/// "Connect to the Internet": already online → Browse now / set up anyway; else broadband (PPPoE).
fn internet_page(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    { let (c, w) = (cp.clone(), wz.clone()); wiz_back(wz, Some(Rc::new(move || choose_option(&c, &w)))); }
    let v = vbox(8);
    let online = sh(&["nmcli", "-t", "-f", "CONNECTIVITY", "general"]).trim() == "full";
    if online {
        v.pack_start(&xc::instr("You are already connected to the Internet"), false, false, 0);
        // the default web browser on its home page
        let w = wz.clone();
        v.pack_start(&command_link("Browse the Internet now", "", move || {
            if let Some(a) = gio::AppInfo::default_for_uri_scheme("http") { let _ = a.launch(&[], None::<&gio::AppLaunchContext>); }
            w.win.close();
        }), false, false, 6);
        let (c, w) = (cp.clone(), wz.clone());
        v.pack_start(&command_link("Set up a new connection anyway", "", move || broadband_page(&c, &w)), false, false, 0);
        wz.page(&v);
        wz.set_buttons(&[&wiz_cancel(wz, "Cancel")]);
    } else { broadband_page(cp, wz); }
}

/// "Type the information from your Internet service provider (ISP)" (PPPoE on the wired adapter).
fn broadband_page(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    { let (c, w) = (cp.clone(), wz.clone()); wiz_back(wz, Some(Rc::new(move || internet_page(&c, &w)))); }
    let v = vbox(10);
    v.pack_start(&xc::instr("Type the information from your Internet service provider (ISP)"), false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    let (user, pass, name) = (gtk::Entry::new(), gtk::Entry::new(), gtk::Entry::new());
    user.set_placeholder_text(Some("[Name your ISP gave you]"));
    pass.set_placeholder_text(Some("[Password your ISP gave you]"));
    pass.set_visibility(false);
    name.set_text("Broadband Connection");
    for (i, (l, e)) in [("User name:", &user), ("Password:", &pass), ("Connection name:", &name)].into_iter().enumerate() {
        g.attach(&label(l, ""), 0, [0, 1, 4][i], 1, 1);
        e.set_hexpand(true);
        g.attach(e, 1, [0, 1, 4][i], 1, 1);
    }
    let show = check("_Show characters", false);
    let remember = check("_Remember this password", true);
    { let p = pass.clone(); show.connect_toggled(move |s| p.set_visibility(s.is_active())); }
    g.attach(&show, 1, 2, 1, 1);
    g.attach(&remember, 1, 3, 1, 1);
    v.pack_start(&g, false, false, 0);
    let ifname = sh(&["nmcli", "-t", "-f", "DEVICE,TYPE", "device"]).lines().map(aerode::sys::nm_split).find(|f| f.len() > 1 && f[1] == "ethernet").map(|f| f[0].clone()).unwrap_or_default();
    if ifname.is_empty() { v.pack_start(&label("This computer has no wired network adapter for a broadband (PPPoE) connection.", "cp-hint"), false, false, 0); }
    wz.page(&v);
    let connect = gtk::Button::with_mnemonic("_Connect");
    connect.set_sensitive(!ifname.is_empty());
    wz.set_buttons(&[&connect, &wiz_cancel(wz, "Cancel")]);
    let w = wz.clone();
    connect.connect_clicked(move |_| {
        let (u, p, n) = (user.text().to_string(), pass.text().to_string(), name.text().trim().to_string());
        if u.is_empty() || n.is_empty() { return; }
        let mut a: Vec<String> = ["nmcli", "connection", "add", "type", "pppoe", "con-name", &n, "ifname", &ifname, "pppoe.username", &u].iter().map(|s| s.to_string()).collect();
        if remember.is_active() { a.extend(["pppoe.password".to_string(), p]); } else { a.extend(["pppoe.password-flags".to_string(), "2".to_string()]); }
        finish(&w, a, &format!("The {n} is ready to use"));
    });
}

/// Run an nmcli change and end on Win7's "… is ready to use" page (or the error).
fn finish(wz: &Rc<Wizard>, argv: Vec<String>, ready: &str) {
    wiz_back(wz, None);
    wz.page(&xc::loading("Connecting..."));
    wz.set_buttons(&[]);
    let (w, ready) = (wz.clone(), ready.to_string());
    bg(move || { let r: Vec<&str> = argv.iter().map(|s| s.as_str()).collect(); run(&r) }, move |(ok, out)| {
        let v = vbox(10);
        if ok {
            v.pack_start(&xc::instr(&ready), false, false, 0);
            v.pack_start(&label("You can find it in Network Connections and in the network list of the notification area.", ""), false, false, 0);
        } else {
            v.pack_start(&xc::instr("The connection could not be set up"), false, false, 0);
            let l = label(out.trim(), "cp-hint");
            l.set_line_wrap(true);
            l.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
            l.set_max_width_chars(70);
            v.pack_start(&l, false, false, 0);
        }
        w.page(&v);
        w.set_buttons(&[&wiz_cancel(&w, "_Close")]);
    });
}

/// "Set up a new network": the router's own set-up page (Windows Connect Now has no Linux side).
fn router_page(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    { let (c, w) = (cp.clone(), wz.clone()); wiz_back(wz, Some(Rc::new(move || choose_option(&c, &w)))); }
    let v = vbox(10);
    v.pack_start(&xc::instr("Configure a new router or access point"), false, false, 0);
    let gw = sh(&["ip", "-4", "route", "show", "default"]).split_whitespace().skip_while(|w| *w != "via").nth(1).unwrap_or("").to_string();
    v.pack_start(&label("Routers and access points are set up from their own web page. Connect the router to this computer with a cable or join its network, then open its settings page.", ""), false, false, 0);
    if gw.is_empty() {
        v.pack_start(&label("Windows can't find a router on this network.", "cp-hint"), false, false, 0);
    } else {
        let u = format!("http://{gw}/");
        v.pack_start(&command_link(&format!("Open the router's settings page ({gw})"), "Your browser opens the page; the password is often printed on the router.", move || { util::open_uri(&u); }), false, false, 6);
    }
    wz.page(&v);
    wz.set_buttons(&[&wiz_cancel(wz, "Cancel")]);
}

/// "Give your network a name and choose security options" (a NetworkManager hotspot).
fn adhoc_page(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    { let (c, w) = (cp.clone(), wz.clone()); wiz_back(wz, Some(Rc::new(move || choose_option(&c, &w)))); }
    let v = vbox(10);
    v.pack_start(&xc::instr("Give your network a name and choose security options"), false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    let name = gtk::Entry::new();
    let sec = combo(&[("wpa", "WPA2-Personal"), ("none", "No authentication (Open)")], "wpa");
    let key = gtk::Entry::new();
    key.set_visibility(false);
    let hide = check("_Hide characters", true);
    { let k = key.clone(); hide.connect_toggled(move |h| k.set_visibility(!h.is_active())); }
    for (i, (l, w)) in [("Network name:", name.upcast_ref::<gtk::Widget>()), ("Security type:", sec.upcast_ref()), ("Security key:", key.upcast_ref())].into_iter().enumerate() {
        g.attach(&label(l, ""), 0, i as i32, 1, 1);
        w.set_hexpand(true);
        g.attach(w, 1, i as i32, 1, 1);
    }
    g.attach(&hide, 2, 2, 1, 1);
    v.pack_start(&g, false, false, 0);
    let save = check("_Save this network", false);
    v.pack_start(&save, false, false, 0);
    let err = label("", "cp-hint");
    v.pack_start(&err, false, false, 0);
    let wifi = sh(&["nmcli", "-t", "-f", "DEVICE,TYPE", "device"]).lines().map(aerode::sys::nm_split).find(|f| f.len() > 1 && f[1] == "wifi").map(|f| f[0].clone()).unwrap_or_default();
    if wifi.is_empty() { err.set_text("This computer has no wireless network adapter."); }
    wz.page(&v);
    let next = gtk::Button::with_mnemonic("_Next");
    next.set_sensitive(!wifi.is_empty());
    wz.set_buttons(&[&next, &wiz_cancel(wz, "Cancel")]);
    let w = wz.clone();
    next.connect_clicked(move |_| {
        let (n, k, open) = (name.text().trim().to_string(), key.text().to_string(), sec.active_id().as_deref() == Some("none"));
        if n.is_empty() { name.grab_focus(); return; }
        if !open && k.len() < 8 { err.set_text("The security key must be at least 8 characters (WPA2-Personal)."); return; }
        let mut a: Vec<String> = ["nmcli", "device", "wifi", "hotspot", "ifname", &wifi, "con-name", &n, "ssid", &n].iter().map(|s| s.to_string()).collect();
        if !open { a.extend(["password".to_string(), k]); }
        let mut cmd = a.iter().map(|s| q(s)).collect::<Vec<_>>().join(" ");
        // not saved: a temporary network that does not start again at the next log on (Win7)
        if !save.is_active() { cmd.push_str(&format!(" && nmcli connection modify {} connection.autoconnect no", q(&n))); }
        finish(&w, vec!["sh".into(), "-c".into(), cmd], &format!("The {n} network is ready to use"));
    });
}

/// "Connect to a workplace": VPN through a NetworkManager plug-in or an imported file.
fn workplace_page(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    { let (c, w) = (cp.clone(), wz.clone()); wiz_back(wz, Some(Rc::new(move || choose_option(&c, &w)))); }
    let v = vbox(8);
    v.pack_start(&xc::instr("How do you want to connect?"), false, false, 0);
    let (c, w) = (cp.clone(), wz.clone());
    v.pack_start(&command_link("Use my Internet connection (VPN)", "Connect using a virtual private network (VPN) connection through the Internet.", move || vpn_page(&c, &w)), false, false, 6);
    let dial = command_link("Dial directly", "Connect directly to a phone number without going through the Internet.", || {});
    dial.set_sensitive(false);
    v.pack_start(&dial, false, false, 0);
    wz.page(&v);
    wz.set_buttons(&[&wiz_cancel(wz, "Cancel")]);
}

fn vpn_page(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    { let (c, w) = (cp.clone(), wz.clone()); wiz_back(wz, Some(Rc::new(move || workplace_page(&c, &w)))); }
    let v = vbox(10);
    v.pack_start(&xc::instr("Type the Internet address to connect to"), false, false, 0);
    v.pack_start(&label("Your network administrator can give you this address.", ""), false, false, 0);
    let plugins = vpn_plugins();
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    let addr = gtk::Entry::new();
    addr.set_placeholder_text(Some("Example: Contoso.com or 157.54.0.1 or 3ffe:1234::1111"));
    let name = gtk::Entry::new();
    name.set_text("VPN Connection");
    let kinds: Vec<(&str, &str)> = plugins.iter().map(|(s, n)| (s.as_str(), n.as_str())).collect();
    let kind = combo(&kinds, kinds.first().map(|k| k.0).unwrap_or(""));
    for (i, (l, w)) in [("Internet address:", addr.upcast_ref::<gtk::Widget>()), ("Destination name:", name.upcast_ref()), ("VPN type:", kind.upcast_ref())].into_iter().enumerate() {
        g.attach(&label(l, ""), 0, i as i32, 1, 1);
        w.set_hexpand(true);
        g.attach(w, 1, i as i32, 1, 1);
    }
    v.pack_start(&g, false, false, 0);
    if plugins.is_empty() {
        g.set_sensitive(false);
        v.pack_start(&label("No VPN plug-ins for NetworkManager are installed (networkmanager-openvpn, -openconnect, -l2tp…). A WireGuard or OpenVPN configuration file can be imported.", "cp-hint"), false, false, 0);
    }
    let import = gtk::Button::with_mnemonic("_Import a configuration file...");
    import.set_halign(gtk::Align::Start);
    v.pack_start(&import, false, false, 0);
    wz.page(&v);
    let create = gtk::Button::with_mnemonic("C_reate");
    create.set_sensitive(!plugins.is_empty());
    wz.set_buttons(&[&create, &wiz_cancel(wz, "Cancel")]);
    {
        let w = wz.clone();
        import.connect_clicked(move |_| {
            let fc = gtk::FileChooserNative::new(Some("Import VPN configuration"), Some(&w.win), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
            let f = gtk::FileFilter::new();
            f.set_name(Some("VPN configuration (*.ovpn, *.conf)"));
            f.add_pattern("*.ovpn");
            f.add_pattern("*.conf");
            fc.add_filter(f);
            if fc.run() != gtk::ResponseType::Accept { return; }
            let Some(p) = fc.filename() else { return };
            let ps = p.to_string_lossy().into_owned();
            let ty = if ps.ends_with(".ovpn") { "openvpn" } else { "wireguard" };
            let n = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            finish(&w, ["nmcli", "connection", "import", "type", ty, "file", &ps].iter().map(|s| s.to_string()).collect(), &format!("The {n} connection is ready to use"));
        });
    }
    let w = wz.clone();
    create.connect_clicked(move |_| {
        let (a, n) = (addr.text().trim().to_string(), name.text().trim().to_string());
        let Some(svc) = kind.active_id().map(|s| s.to_string()) else { return };
        if a.is_empty() { addr.grab_focus(); return; }
        if n.is_empty() { name.grab_focus(); return; }
        finish(&w, ["nmcli", "connection", "add", "type", "vpn", "con-name", &n, "vpn-type", &svc, "vpn.data", &format!("gateway={a}, remote={a}")].iter().map(|s| s.to_string()).collect(),
            &format!("The {n} connection is ready to use"));
    });
}

/// "Set up a dial-up connection": mobile broadband modems (ModemManager), by APN.
fn dialup_page(cp: &Rc<Cp>, wz: &Rc<Wizard>) {
    { let (c, w) = (cp.clone(), wz.clone()); wiz_back(wz, Some(Rc::new(move || choose_option(&c, &w)))); }
    let v = vbox(10);
    let modem = sh(&["nmcli", "-t", "-f", "DEVICE,TYPE", "device"]).lines().map(aerode::sys::nm_split).find(|f| f.len() > 1 && f[1] == "gsm").map(|f| f[0].clone());
    let Some(modem) = modem else {
        v.pack_start(&xc::instr("Windows can't detect a dial-up modem"), false, false, 0);
        v.pack_start(&label("Plug in a mobile broadband modem or phone (ModemManager must be running), then try again.", ""), false, false, 0);
        wz.page(&v);
        wz.set_buttons(&[&wiz_cancel(wz, "Cancel")]);
        return;
    };
    v.pack_start(&xc::instr("Type the information from your Internet service provider (ISP)"), false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    let (apn, user, pass, name) = (gtk::Entry::new(), gtk::Entry::new(), gtk::Entry::new(), gtk::Entry::new());
    apn.set_placeholder_text(Some("[Access point name your ISP gave you]"));
    pass.set_visibility(false);
    name.set_text("Dial-up Connection");
    for (i, (l, e)) in [("Access point (APN):", &apn), ("User name:", &user), ("Password:", &pass), ("Connection name:", &name)].into_iter().enumerate() {
        g.attach(&label(l, ""), 0, i as i32, 1, 1);
        e.set_hexpand(true);
        g.attach(e, 1, i as i32, 1, 1);
    }
    v.pack_start(&g, false, false, 0);
    wz.page(&v);
    let connect = gtk::Button::with_mnemonic("_Connect");
    wz.set_buttons(&[&connect, &wiz_cancel(wz, "Cancel")]);
    let w = wz.clone();
    connect.connect_clicked(move |_| {
        let n = name.text().trim().to_string();
        if apn.text().is_empty() || n.is_empty() { return; }
        let mut a: Vec<String> = ["nmcli", "connection", "add", "type", "gsm", "ifname", &modem, "con-name", &n, "gsm.apn", apn.text().as_str()].iter().map(|s| s.to_string()).collect();
        if !user.text().is_empty() { a.extend(["gsm.username".to_string(), user.text().to_string(), "gsm.password".to_string(), pass.text().to_string()]); }
        finish(&w, a, &format!("The {n} is ready to use"));
    });
}

// ------------------------------------------------------------------ connect to a network

/// "Connect to a network": the networks in range, Connect / Disconnect.
fn connect_dialog(cp: &Rc<Cp>) {
    let dl = dialog(Some(&cp.win.clone().upcast()), "Connect to a Network", &[]);
    dl.set_default_size(340, 420);
    let c = dl.content_area();
    let status = label("Currently connected to:", "");
    c.pack_start(&status, false, false, 0);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    c.pack_start(&scrolled(&list), true, true, 0);
    let row = hbox(6);
    row.set_halign(gtk::Align::End);
    let btn = gtk::Button::with_mnemonic("_Connect");
    btn.set_sensitive(false);
    let close = gtk::Button::with_label("Close");
    row.pack_start(&btn, false, false, 0);
    row.pack_start(&close, false, false, 0);
    c.pack_start(&row, false, false, 0);
    { let d = dl.clone(); close.connect_clicked(move |_| d.close()); }
    let nets: Rc<RefCell<Vec<(bool, String, u32, bool)>>> = Rc::new(RefCell::new(vec![]));
    {
        let (list, status, nets) = (list.clone(), status.clone(), nets.clone());
        bg(|| (aerode::sys::wifi_list(), aerode::sys::known_connections(), aerode::sys::net_status()), move |(wl, known, st)| {
            status.set_text(&if st.kind == "none" { "Not connected".to_string() } else { format!("Currently connected to:  {}", st.name) });
            if wl.is_empty() { list.add(&label("No wireless networks are in range.", "cp-hint")); }
            for w in &wl {
                let h = hbox(8);
                h.set_border_width(4);
                let v = vbox(0);
                v.pack_start(&label(&w.ssid, ""), false, false, 0);
                let sub = if w.active { "Connected" } else if known.contains(&w.ssid) { "Saved network" } else if w.secure { "Secured" } else { "Unsecured network" };
                v.pack_start(&label(sub, "dim"), false, false, 0);
                h.pack_start(&v, true, true, 0);
                let icon = match w.signal { 0..=24 => "network-wireless-signal-weak", 25..=49 => "network-wireless-signal-ok", 50..=74 => "network-wireless-signal-good", _ => "network-wireless-signal-excellent" };
                h.pack_end(&img(&[icon], 16), false, false, 0);
                list.add(&h);
                nets.borrow_mut().push((w.active, w.ssid.clone(), w.signal, w.secure && !known.contains(&w.ssid)));
            }
            list.show_all();
        });
    }
    { let (b, nets) = (btn.clone(), nets.clone()); list.connect_row_selected(move |_, r| {
        let n = r.and_then(|r| nets.borrow().get(r.index() as usize).cloned());
        b.set_sensitive(n.is_some());
        if let Some(n) = n { b.set_label(if n.0 { "_Disconnect" } else { "_Connect" }); b.set_use_underline(true); }
    }); }
    let (list2, nets2, d2) = (list.clone(), nets.clone(), dl.clone());
    btn.connect_clicked(move |b| {
        let Some(r) = list2.selected_row() else { return };
        let Some((active, ssid, _, needs_key)) = nets2.borrow().get(r.index() as usize).cloned() else { return };
        let argv: Vec<String> = if active {
            let dev = sh(&["nmcli", "-t", "-f", "DEVICE,TYPE,STATE", "device"]).lines().map(aerode::sys::nm_split).find(|f| f.len() > 2 && f[1] == "wifi" && f[2] == "connected").map(|f| f[0].clone()).unwrap_or_default();
            vec!["nmcli".into(), "device".into(), "disconnect".into(), dev]
        } else if needs_key {
            let Some(key) = security_key(Some(d2.upcast_ref()), &ssid) else { return };
            vec!["nmcli".into(), "device".into(), "wifi".into(), "connect".into(), ssid.clone(), "password".into(), key]
        } else { vec!["nmcli".into(), "device".into(), "wifi".into(), "connect".into(), ssid.clone()] };
        b.set_sensitive(false);
        let (d3, b2) = (d2.clone(), b.clone());
        bg(move || { let a: Vec<&str> = argv.iter().map(|s| s.as_str()).collect(); run(&a) }, move |(ok, out)| {
            b2.set_sensitive(true);
            if ok { d3.close(); } else { util::error(&format!("Windows was unable to connect to {ssid}"), out.trim()); }
        });
    });
    dl.show_all();
}

/// "Type the network security key".
fn security_key(parent: Option<&gtk::Window>, ssid: &str) -> Option<String> {
    let d = dialog(parent, "Connect to a Network", &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.pack_start(&label("Type the network security key", "wu-title"), false, false, 0);
    let h = hbox(8);
    h.pack_start(&label("Security key:", ""), false, false, 0);
    let e = gtk::Entry::new();
    e.set_visibility(false);
    e.set_activates_default(true);
    e.set_width_chars(28);
    h.pack_start(&e, true, true, 0);
    c.pack_start(&h, false, false, 0);
    let hide = check("_Hide characters", true);
    { let e = e.clone(); hide.connect_toggled(move |c| e.set_visibility(!c.is_active())); }
    c.pack_start(&hide, false, false, 0);
    c.pack_start(&label(&format!("Network: {ssid}"), "cp-hint"), false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    let r = d.run();
    let k = e.text().to_string();
    d.close();
    (r == gtk::ResponseType::Ok && !k.is_empty()).then_some(k)
}

// ------------------------------------------------------------- manage wireless networks

#[derive(Clone, Debug, Default)]
struct Profile { name: String, uuid: String, auto: bool, prio: i32, security: String }

fn security_name(key_mgmt: &str) -> &'static str {
    match key_mgmt.trim() { "wpa-psk" => "WPA2-Personal", "sae" => "WPA3-Personal", "wpa-eap" | "wpa-eap-suite-b-192" => "WPA2-Enterprise", "owe" => "Enhanced Open", "none" | "ieee8021x" => "WEP", _ => "No authentication (Open)" }
}

fn wifi_profiles() -> Vec<Profile> {
    let mut v: Vec<Profile> = sh(&["nmcli", "-t", "-f", "NAME,UUID,TYPE,AUTOCONNECT,AUTOCONNECT-PRIORITY", "connection", "show"]).lines().map(aerode::sys::nm_split)
        .filter(|f| f.len() >= 5 && f[2] == "802-11-wireless")
        .map(|f| Profile { name: f[0].clone(), uuid: f[1].clone(), auto: f[3] == "yes", prio: f[4].parse().unwrap_or(0), security: String::new() }).collect();
    for p in v.iter_mut() { p.security = security_name(&sh(&["nmcli", "-t", "-g", "802-11-wireless-security.key-mgmt", "connection", "show", &p.uuid])).into(); }
    v.sort_by(|a, b| b.prio.cmp(&a.prio).then(a.name.cmp(&b.name)));
    v
}

fn wireless_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Manage Wireless Networks");
    cp.side_link("Network and Sharing Center", "network");
    { let c = cp.clone(); side_action(cp, "Change adapter settings", move || go(&c, "network/adapters")); }
    let v = vbox(6);
    v.pack_start(&title("Manage wireless networks that use (Wireless Network Connection)"), false, false, 0);
    v.pack_start(&text("Windows tries to connect to these networks in the order listed below."), false, false, 0);
    let frame = vbox(0);
    frame.style_context().add_class("cp-list");
    frame.set_margin_top(6);
    let bar = hbox(2);
    bar.style_context().add_class("cp-cmdbar");
    let (add, remove, up, down, props, types_, nsc) = (cmd_button("Add"), cmd_button("Remove"), cmd_button("Move up"), cmd_button("Move down"), cmd_button("Adapter properties"), cmd_button("Profile types"), cmd_button("Network and Sharing Center"));
    for b in [&add, &remove, &up, &down, &props, &types_, &nsc] { bar.pack_start(b, false, false, 0); }
    frame.pack_start(&clip(&bar), false, false, 0);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.add(&list);
    sw.set_min_content_height(300);
    frame.pack_start(&sw, true, true, 0);
    let foot = label("", "cp-hint");
    foot.set_margin_start(8);
    foot.set_margin_top(4);
    foot.set_margin_bottom(4);
    frame.pack_start(&foot, false, false, 0);
    v.pack_start(&frame, true, true, 0);
    let profs: Rc<RefCell<Vec<Profile>>> = Rc::new(RefCell::new(vec![]));
    let fill: Rc<dyn Fn()> = {
        let (list, profs, foot) = (list.clone(), profs.clone(), foot.clone());
        Rc::new(move || {
            let (list, profs, foot) = (list.clone(), profs.clone(), foot.clone());
            bg(wifi_profiles, move |v| {
                for c in list.children() { list.remove(&c); }
                for p in &v {
                    let h = hbox(10);
                    h.set_border_width(6);
                    h.pack_start(&img(&["network-wireless-signal-excellent", "network-wireless"], 32), false, false, 0);
                    let col = vbox(1);
                    col.pack_start(&bold(&p.name), false, false, 0);
                    col.pack_start(&label(&format!("Security: {}        Type: Any supported", p.security), "dim"), false, false, 0);
                    h.pack_start(&col, true, true, 0);
                    h.pack_end(&label(if p.auto { "Automatically connect" } else { "Manually connect" }, "dim"), false, false, 0);
                    list.add(&h);
                }
                foot.set_text(&format!("{} networks", v.len()));
                list.show_all();
                *profs.borrow_mut() = v;
            });
        })
    };
    fill();
    let selected = { let (list, profs) = (list.clone(), profs.clone()); move || list.selected_row().and_then(|r| profs.borrow().get(r.index() as usize).cloned().map(|p| (r.index() as usize, p))) };
    { let c = cp.clone(); nsc.connect_clicked(move |_| go(&c, "network")); }
    { let c = cp.clone(); props.connect_clicked(move |_| go(&c, "network/adapters")); }
    types_.connect_clicked(|b| inform(win_of(b).as_ref(), "Wireless Network Profile Type", "Every wireless network profile on this computer can be used by all users.",
        "NetworkManager keeps Wi-Fi profiles for all users unless a profile is restricted to one user in its properties."));
    { let (sel, fill) = (selected.clone(), fill.clone()); remove.connect_clicked(move |b| {
        let Some((_, p)) = sel() else { return };
        if !confirm(win_of(b).as_ref(), "Manage Wireless Networks", &format!("Are you sure you want to remove {}?", p.name), "You will not be able to connect to this network automatically anymore.", "_Remove") { return; }
        let fill = fill.clone();
        bg(move || run(&["nmcli", "connection", "delete", &p.uuid]), move |(ok, out)| { report("The network could not be removed.", ok, &out); fill(); });
    }); }
    for (b, dir) in [(&up, -1i32), (&down, 1)] {
        let (sel, fill, profs) = (selected.clone(), fill.clone(), profs.clone());
        b.connect_clicked(move |_| {
            let Some((i, _)) = sel() else { return };
            let j = i as i32 + dir;
            let mut v = profs.borrow().clone();
            if j < 0 || j as usize >= v.len() { return; }
            v.swap(i, j as usize);
            // the list order becomes the priority order (highest first)
            let n = v.len() as i32;
            let cmds: Vec<(String, i32)> = v.iter().enumerate().map(|(k, p)| (p.uuid.clone(), (n - k as i32) * 10)).collect();
            let fill = fill.clone();
            bg(move || { let mut ok = (true, String::new()); for (u, pr) in &cmds { let r = run(&["nmcli", "connection", "modify", u, "connection.autoconnect-priority", &pr.to_string()]); if !r.0 { ok = r; break; } } ok },
                move |(ok, out)| { report("The order could not be changed.", ok, &out); fill(); });
        });
    }
    { let (sel, c) = (selected.clone(), cp.clone()); list.connect_row_activated(move |_, _| { if let Some((_, p)) = sel() { wifi_properties(&c, &p); } }); }
    { let (c, fill) = (cp.clone(), fill.clone()); add.connect_clicked(move |_| add_wifi(&c, fill.clone())); }
    v.upcast()
}

/// "<network> Wireless Network Properties": Connection and Security tabs.
fn wifi_properties(cp: &Rc<Cp>, p: &Profile) {
    let d = dialog(Some(&cp.win.clone().upcast()), &format!("{} Wireless Network Properties", p.name), &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let nb = gtk::Notebook::new();
    let g = gtk::Grid::new();
    g.set_border_width(10);
    g.set_row_spacing(6);
    g.set_column_spacing(12);
    let vals: Vec<String> = sh(&["nmcli", "-t", "-g", "802-11-wireless.ssid,802-11-wireless.hidden,connection.autoconnect,connection.zone", "connection", "show", &p.uuid]).lines().map(|s| s.to_string()).collect();
    let get = |i: usize| vals.get(i).cloned().unwrap_or_default();
    for (i, (k, val)) in [("Name:", p.name.clone()), ("SSID:", get(0)), ("Network type:", "Access point".into()), ("Network availability:", "All users".into())].into_iter().enumerate() {
        g.attach(&label(k, ""), 0, i as i32, 1, 1);
        let l = label(&val, ""); l.set_selectable(true);
        g.attach(&l, 1, i as i32, 1, 1);
    }
    let auto = check("Connect _automatically when this network is in range", get(2) != "no");
    let pref = check("Connect to a more _preferred network if available", true);
    pref.set_sensitive(false);
    let hidden = check("Connect even if the network is not _broadcasting its name (SSID)", get(1) == "yes");
    for (i, c) in [&auto, &pref, &hidden].iter().enumerate() { g.attach(*c, 0, 5 + i as i32, 2, 1); }
    nb.append_page(&g, Some(&gtk::Label::new(Some("Connection"))));
    let s = gtk::Grid::new();
    s.set_border_width(10);
    s.set_row_spacing(6);
    s.set_column_spacing(12);
    s.attach(&label("Security type:", ""), 0, 0, 1, 1);
    s.attach(&label(&p.security, ""), 1, 0, 1, 1);
    s.attach(&label("Encryption type:", ""), 0, 1, 1, 1);
    s.attach(&label(if p.security.starts_with("WPA") { "AES" } else { "None" }, ""), 1, 1, 1, 1);
    s.attach(&label("Network security key", ""), 0, 2, 1, 1);
    let key = gtk::Entry::new();
    key.set_visibility(false);
    key.set_placeholder_text(Some("••••••••"));
    s.attach(&key, 1, 2, 1, 1);
    let show = check("_Show characters", false);
    s.attach(&show, 1, 3, 1, 1);
    { let (k, uuid) = (key.clone(), p.uuid.clone()); show.connect_toggled(move |c| {
        k.set_visibility(c.is_active());
        if c.is_active() && k.text().is_empty() {
            // NetworkManager asks the polkit agent before it hands out a saved key
            let k = k.clone();
            util::output_async(vec!["nmcli".into(), "-s".into(), "-g".into(), "802-11-wireless-security.psk".into(), "connection".into(), "show".into(), uuid.clone()], move |o| { if let Some(o) = o { k.set_text(o.trim()); } });
        }
    }); }
    nb.append_page(&s, Some(&gtk::Label::new(Some("Security"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.show_all();
    let uuid = p.uuid.clone();
    let (a0, h0) = (auto.is_active(), hidden.is_active());
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok {
            let mut args: Vec<String> = vec!["nmcli".into(), "connection".into(), "modify".into(), uuid.clone()];
            if auto.is_active() != a0 { args.extend(["connection.autoconnect".into(), if auto.is_active() { "yes".into() } else { "no".into() }]); }
            if hidden.is_active() != h0 { args.extend(["802-11-wireless.hidden".into(), if hidden.is_active() { "yes".into() } else { "no".into() }]); }
            let k = key.text().to_string();
            if !k.is_empty() && show.is_active() { args.extend(["802-11-wireless-security.psk".into(), k]); }
            if args.len() > 4 { bg(move || { let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect(); run(&a) }, |(ok, out)| { report("The network properties could not be saved.", ok, &out); }); }
        }
        d.close();
    });
}

/// "Manually connect to a wireless network".
fn add_wifi(cp: &Rc<Cp>, done: Rc<dyn Fn()>) {
    let d = dialog(Some(&cp.win.clone().upcast()), "Manually connect to a wireless network", &[("Next", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.pack_start(&label("Enter information for the wireless network you want to add", "wu-title"), false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(12);
    let name = gtk::Entry::new();
    let sec = combo(&[("none", "No authentication (Open)"), ("wpa-psk", "WPA2-Personal"), ("sae", "WPA3-Personal")], "wpa-psk");
    let enc = combo(&[("aes", "AES")], "aes");
    let key = gtk::Entry::new();
    key.set_visibility(false);
    let hide = check("_Hide characters", true);
    { let k = key.clone(); hide.connect_toggled(move |h| k.set_visibility(!h.is_active())); }
    for (i, (l, w)) in [("Network name:", name.upcast_ref::<gtk::Widget>()), ("Security type:", sec.upcast_ref()), ("Encryption type:", enc.upcast_ref()), ("Security Key:", key.upcast_ref())].into_iter().enumerate() {
        g.attach(&label(l, ""), 0, i as i32, 1, 1);
        w.set_hexpand(true);
        g.attach(w, 1, i as i32, 1, 1);
    }
    g.attach(&hide, 2, 3, 1, 1);
    c.pack_start(&g, false, false, 0);
    let auto = check("S_tart this connection automatically", true);
    let hidden = check("Connect even if the network is not _broadcasting", false);
    c.pack_start(&auto, false, false, 0);
    c.pack_start(&hidden, false, false, 0);
    c.pack_start(&label("Warning: If you select this option, your computer's privacy might be at risk.", "cp-hint"), false, false, 0);
    d.show_all();
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok {
            let ssid = name.text().to_string();
            if ssid.is_empty() { return; }
            let mut a: Vec<String> = ["nmcli", "connection", "add", "type", "wifi", "con-name", &ssid, "ssid", &ssid, "connection.autoconnect", if auto.is_active() { "yes" } else { "no" },
                "802-11-wireless.hidden", if hidden.is_active() { "yes" } else { "no" }].iter().map(|s| s.to_string()).collect();
            let km = sec.active_id().map(|s| s.to_string()).unwrap_or_default();
            if km != "none" { a.extend(["wifi-sec.key-mgmt".into(), km, "wifi-sec.psk".into(), key.text().to_string()]); }
            let done = done.clone();
            bg(move || { let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect(); run(&r) }, move |(ok, out)| { report("The network could not be added.", ok, &out); done(); });
        }
        d.close();
    });
}

// ------------------------------------------------------------------- network connections

fn adapters_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Network Connections");
    cp.side_link("Network and Sharing Center", "network");
    let v = vbox(0);
    let frame = vbox(0);
    frame.style_context().add_class("cp-list");
    let bar = hbox(2);
    bar.style_context().add_class("cp-cmdbar");
    let organize = gtk::Menu::new();
    bar.pack_start(&menu_button("Organize", &organize), false, false, 0);
    let dynamic = hbox(2);
    bar.pack_start(&dynamic, false, false, 0);
    frame.pack_start(&clip(&bar), false, false, 0);
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_max_children_per_line(3);
    flow.set_min_children_per_line(2);
    flow.set_column_spacing(10);
    flow.set_row_spacing(6);
    flow.set_border_width(10);
    flow.set_valign(gtk::Align::Start);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.add(&flow);
    sw.set_min_content_height(380);
    frame.pack_start(&sw, true, true, 0);
    v.pack_start(&frame, true, true, 0);
    let state: Rc<RefCell<(Vec<Dev>, Vec<String>, Option<usize>)>> = Rc::new(RefCell::new((vec![], vec![], None)));
    let reload: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let tiles: Rc<RefCell<Vec<gtk::Button>>> = Rc::new(RefCell::new(vec![]));
    let select: Rc<dyn Fn(Option<usize>)> = {
        let (state, dynamic, tiles, cp, reload) = (state.clone(), dynamic.clone(), tiles.clone(), cp.clone(), reload.clone());
        Rc::new(move |i: Option<usize>| {
            state.borrow_mut().2 = i;
            for (k, t) in tiles.borrow().iter().enumerate() { if Some(k) == i { t.style_context().add_class("sel") } else { t.style_context().remove_class("sel") } }
            for c in dynamic.children() { dynamic.remove(&c); }
            let Some(i) = i else { return };
            let (d, name) = { let s = state.borrow(); (s.0[i].clone(), s.1[i].clone()) };
            let disabled = d.state == "disconnected" && !d.autoconnect;
            let en = cmd_button(if disabled { "Enable this network device" } else { "Disable this network device" });
            { let (d, r) = (d.clone(), reload.clone()); en.connect_clicked(move |_| set_enabled(&d, disabled, r.clone())); }
            dynamic.pack_start(&en, false, false, 0);
            if d.wifi() { let cb = cmd_button(if d.up() { "Disconnect from network" } else { "Connect To" }); let c = cp.clone(); cb.connect_clicked(move |_| connect_dialog(&c)); dynamic.pack_start(&cb, false, false, 0); }
            let dg = cmd_button("Diagnose this connection");
            { let (w, d) = (cp.win.clone(), d.clone()); dg.connect_clicked(move |_| diagnose(w.upcast_ref(), Some(&d))); }
            dynamic.pack_start(&dg, false, false, 0);
            let st = cmd_button("View status of this connection");
            { let (c, d, n) = (cp.clone(), d.clone(), name.clone()); st.connect_clicked(move |_| status_dialog(&c, &d, &n)); }
            st.set_sensitive(d.up());
            dynamic.pack_start(&st, false, false, 0);
            let ch = cmd_button("Change settings of this connection");
            { let d = d.clone(); ch.connect_clicked(move |_| edit_connection(&d)); }
            dynamic.pack_start(&ch, false, false, 0);
            dynamic.show_all();
        })
    };
    let fill: Rc<dyn Fn()> = {
        let (flow, state, tiles, select, cp) = (flow.clone(), state.clone(), tiles.clone(), select.clone(), cp.clone());
        let reload2 = reload.clone();
        Rc::new(move || {
            let (flow, state, tiles, select, cp, reload) = (flow.clone(), state.clone(), tiles.clone(), select.clone(), cp.clone(), reload2.clone());
            bg(gather, move |net| {
                for c in flow.children() { flow.remove(&c); }
                tiles.borrow_mut().clear();
                let names = adapter_names(&net.devs);
                for (i, d) in net.devs.iter().enumerate() {
                    let b = gtk::Button::new();
                    b.style_context().add_class("dp-tile");
                    b.set_relief(gtk::ReliefStyle::None);
                    let h = hbox(8);
                    h.pack_start(&img(dev_icon(d), 48), false, false, 0);
                    let col = vbox(0);
                    col.set_valign(gtk::Align::Center);
                    col.pack_start(&label(&names[i], ""), false, false, 0);
                    col.pack_start(&label(&d.status_line(), "dim"), false, false, 0);
                    let desc = label(&d.description(), "dim");
                    desc.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    desc.set_line_wrap(false);
                    desc.set_max_width_chars(30);
                    col.pack_start(&desc, false, false, 0);
                    h.pack_start(&col, true, true, 0);
                    b.add(&h);
                    b.set_size_request(270, -1);
                    { let s = select.clone(); b.connect_clicked(move |_| s(Some(i))); }
                    {
                        let (d, n, cp, s, r) = (d.clone(), names[i].clone(), cp.clone(), select.clone(), reload.clone());
                        b.connect_button_press_event(move |_, e| {
                            if e.event_type() == gdk::EventType::DoubleButtonPress && e.button() == 1 { if d.up() { status_dialog(&cp, &d, &n); } else { set_enabled(&d, true, r.clone()); } return glib::Propagation::Stop; }
                            if e.button() != 3 { return glib::Propagation::Proceed; }
                            s(Some(i));
                            let m = gtk::Menu::new();
                            let disabled = d.state == "disconnected" && !d.autoconnect;
                            { let (d, r) = (d.clone(), r.clone()); menu_item(&m, if disabled { "_Enable" } else { "Disa_ble" }, move || set_enabled(&d, disabled, r.clone())); }
                            if d.wifi() { let c = cp.clone(); menu_item(&m, if d.up() { "Disc_onnect" } else { "C_onnect / Disconnect" }, move || connect_dialog(&c)); }
                            { let up = d.up(); let (c, d, n) = (cp.clone(), d.clone(), n.clone()); menu_item(&m, "Stat_us", move || status_dialog(&c, &d, &n)).set_sensitive(up); }
                            { let (w, d) = (cp.win.clone(), d.clone()); menu_item(&m, "Dia_gnose", move || diagnose(w.upcast_ref(), Some(&d))); }
                            m.append(&gtk::SeparatorMenuItem::new());
                            { let d = d.clone(); menu_item(&m, "P_roperties", move || edit_connection(&d)); }
                            m.show_all();
                            m.popup_at_pointer(Some(e));
                            glib::Propagation::Stop
                        });
                    }
                    flow.add(&b);
                    tiles.borrow_mut().push(b);
                }
                if net.devs.is_empty() { flow.add(&label("No network adapters were found.", "cp-hint")); }
                flow.show_all();
                *state.borrow_mut() = (net.devs, names, None);
                select(None);
            });
        })
    };
    *reload.borrow_mut() = Some(fill.clone());
    { let f = fill.clone(); menu_item(&organize, "_Refresh", move || f()); }
    fill();
    v.upcast()
}

/// Disable = disconnect and don't connect automatically; Enable = the opposite.
fn set_enabled(d: &Dev, enable: bool, reload: Rc<RefCell<Option<Rc<dyn Fn()>>>>) {
    let dev = d.iface.clone();
    bg(move || {
        let r1 = run(&["nmcli", "device", "set", &dev, "autoconnect", if enable { "yes" } else { "no" }]);
        let r2 = run(&["nmcli", "device", if enable { "connect" } else { "disconnect" }, &dev]);
        if !r1.0 { r1 } else { r2 }
    }, move |(ok, out)| {
        report(if enable { "The network device could not be enabled." } else { "The network device could not be disabled." }, ok, &out);
        if let Some(f) = reload.borrow().clone() { f(); }
    });
}

fn edit_connection(d: &Dev) {
    if util::which("nm-connection-editor") {
        if d.uuid.is_empty() { util::spawn(&["nm-connection-editor"]); } else { util::spawn(&["nm-connection-editor", "--edit", &d.uuid]); }
    } else if !terminal("nmtui edit") {}
}

/// Bytes sent/received of an interface.
fn bytes(iface: &str) -> (u64, u64) { (sysfs(iface, "statistics/tx_bytes").parse().unwrap_or(0), sysfs(iface, "statistics/rx_bytes").parse().unwrap_or(0)) }

/// Link speed in Win7's words ("100.0 Mbps", "1.0 Gbps").
fn speed(d: &Dev) -> String {
    if d.wifi() {
        let r: f64 = d.rate.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        return if r <= 0.0 { String::new() } else if r >= 1000.0 { format!("{:.1} Gbps", r / 1000.0) } else { format!("{r:.1} Mbps") };
    }
    match sysfs(&d.iface, "speed").parse::<i64>() { Ok(s) if s >= 1000 => format!("{:.1} Gbps", s as f64 / 1000.0), Ok(s) if s > 0 => format!("{:.1} Mbps", s as f64), _ => String::new() }
}

/// "<Connection> Status": connectivity, media state, SSID, speed, signal, Sent/Received bytes.
fn status_dialog(cp: &Rc<Cp>, d: &Dev, name: &str) {
    let dl = dialog(Some(&cp.win.clone().upcast()), &format!("{name} Status"), &[]);
    dl.set_default_size(360, -1);
    let nb = gtk::Notebook::new();
    let general = vbox(10);
    general.set_border_width(10);
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(20);
    let net_ok = d.up() && !d.gw4.is_empty();
    let v6 = d.ip6.iter().any(|a| !a.starts_with("fe80")) && !d.gw6.is_empty();
    let mut rows: Vec<(&str, String)> = vec![
        ("IPv4 Connectivity:", if net_ok { "Internet".into() } else if !d.ip4.is_empty() { "No Internet access".into() } else { "No network access".into() }),
        ("IPv6 Connectivity:", if v6 { "Internet".into() } else if !d.ip6.is_empty() { "No Internet access".into() } else { "No network access".into() }),
        ("Media State:", if d.up() { "Enabled".into() } else if d.state == "unavailable" { "Media disconnected".into() } else { "Disabled".into() }),
    ];
    if d.wifi() { rows.push(("SSID:", d.ssid.clone())); }
    if let Some(t) = lease_obtained(d) { rows.push(("Duration:", { let s = (chrono::Local::now().timestamp() - t).max(0); format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) })); }
    rows.push(("Speed:", speed(d)));
    if d.wifi() { rows.push(("Signal Quality:", format!("{}  ({}%)", "▮".repeat(((d.signal + 19) / 20) as usize) + &"▯".repeat(5 - ((d.signal + 19) / 20).min(5) as usize), d.signal))); }
    for (i, (k, v)) in rows.iter().enumerate() {
        g.attach(&label(k, ""), 0, i as i32, 1, 1);
        g.attach(&label(v, ""), 1, i as i32, 1, 1);
    }
    let cbox = vbox(8);
    cbox.pack_start(&g, false, false, 0);
    let brow = hbox(6);
    let details = gtk::Button::with_mnemonic("D_etails...");
    { let d = d.clone(); details.connect_clicked(move |b| details_dialog(win_of(b).as_ref(), &d)); }
    brow.pack_start(&details, false, false, 0);
    if d.wifi() { let wp = gtk::Button::with_mnemonic("_Wireless Properties"); let (c, d) = (cp.clone(), d.clone()); wp.connect_clicked(move |_| {
        let p = Profile { name: d.connection.clone(), uuid: d.uuid.clone(), auto: true, prio: 0, security: String::new() };
        wifi_properties(&c, &p);
    }); brow.pack_start(&wp, false, false, 0); }
    cbox.pack_start(&brow, false, false, 0);
    general.pack_start(&group("Connection", &cbox), false, false, 0);
    // Activity: Sent —(computer)— Received, bytes, updated every second
    let act = vbox(6);
    let ar = hbox(0);
    let sent_l = label("Sent", ""); sent_l.set_xalign(0.0);
    ar.pack_start(&sent_l, true, true, 0);
    ar.set_center_widget(Some(&img(&["network-transmit-receive", "network-wired", "computer"], 32)));
    let recv_l = label("Received", ""); recv_l.set_xalign(1.0);
    ar.pack_end(&recv_l, true, true, 0);
    act.pack_start(&ar, false, false, 0);
    let br = hbox(0);
    br.pack_start(&label("Bytes:", ""), false, false, 0);
    let (tx, rx) = bytes(&d.iface);
    let txl = label(&thousands(tx), ""); txl.set_xalign(1.0);
    let rxl = label(&thousands(rx), ""); rxl.set_xalign(1.0);
    br.pack_start(&txl, true, true, 0);
    br.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 12);
    br.pack_start(&rxl, true, true, 0);
    act.pack_start(&br, false, false, 0);
    general.pack_start(&group("Activity", &act), false, false, 0);
    let iface = d.iface.clone();
    let timer = glib::timeout_add_local(std::time::Duration::from_secs(1), move || { let (t, r) = bytes(&iface); txl.set_text(&thousands(t)); rxl.set_text(&thousands(r)); glib::ControlFlow::Continue });
    let timer = RefCell::new(Some(timer));
    let actions = hbox(6);
    let props = shield_button("_Properties");
    { let d = d.clone(); props.connect_clicked(move |_| edit_connection(&d)); }
    let disable = shield_button("_Disable");
    { let (d, dl2) = (d.clone(), dl.clone()); disable.connect_clicked(move |_| { set_enabled(&d, false, Rc::new(RefCell::new(None))); dl2.close(); }); }
    let diag = gtk::Button::with_mnemonic("Dia_gnose");
    { let d = d.clone(); diag.connect_clicked(move |b| { if let Some(w) = win_of(b) { diagnose(&w, Some(&d)); } }); }
    for b in [&props, &disable, &diag] { actions.pack_start(b, false, false, 0); }
    general.pack_start(&actions, false, false, 0);
    nb.append_page(&general, Some(&gtk::Label::new(Some("General"))));
    dl.content_area().pack_start(&nb, true, true, 0);
    dl.add_button("_Close", gtk::ResponseType::Close);
    dl.connect_response(move |d, _| { if let Some(t) = timer.borrow_mut().take() { t.remove(); } d.close(); });
    dl.show_all();
}

/// When the DHCP lease was obtained (expiry − lease time), unix seconds.
fn lease_obtained(d: &Dev) -> Option<i64> {
    let exp: i64 = d.dhcp.get("expiry")?.parse().ok()?;
    let lt: i64 = d.dhcp.get("dhcp_lease_time")?.parse().ok()?;
    Some(exp - lt)
}

fn mask(bits: u32) -> String { let m: u32 = if bits == 0 { 0 } else { u32::MAX << (32 - bits.min(32)) }; std::net::Ipv4Addr::from(m).to_string() }

/// "Network Connection Details".
fn details_dialog(parent: Option<&gtk::Window>, d: &Dev) {
    let dl = dialog(parent, "Network Connection Details", &[("_Close", gtk::ResponseType::Close)]);
    dl.set_default_size(380, 380);
    let c = dl.content_area();
    c.pack_start(&label("Network Connection Details:", ""), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let (ip, bits) = d.ip4.first().and_then(|a| a.split_once('/')).map(|(a, b)| (a.to_string(), b.parse().unwrap_or(24))).unwrap_or_default();
    let dhcp = !d.dhcp.is_empty();
    let obtained = lease_obtained(d).map(super::super::admin::when);
    let expires = d.dhcp.get("expiry").and_then(|e| e.parse().ok()).map(super::super::admin::when);
    let mut rows: Vec<(String, String)> = vec![
        ("Connection-specific DN...".into(), d.domain.clone()),
        ("Description".into(), d.description()),
        ("Physical Address".into(), d.mac.replace(':', "-")),
        ("DHCP Enabled".into(), if dhcp { "Yes".into() } else { "No".into() }),
        ("IPv4 Address".into(), ip.clone()),
        ("IPv4 Subnet Mask".into(), if ip.is_empty() { String::new() } else { mask(bits) }),
    ];
    if dhcp {
        rows.push(("Lease Obtained".into(), obtained.unwrap_or_default()));
        rows.push(("Lease Expires".into(), expires.unwrap_or_default()));
    }
    rows.push(("IPv4 Default Gateway".into(), d.gw4.clone()));
    if dhcp { rows.push(("IPv4 DHCP Server".into(), d.dhcp.get("dhcp_server_identifier").cloned().unwrap_or_default())); }
    for (i, s) in d.dns4.iter().enumerate() { rows.push((if i == 0 { "IPv4 DNS Server".into() } else { String::new() }, s.clone())); }
    rows.push(("IPv4 WINS Server".into(), String::new()));
    rows.push(("NetBIOS over Tcpip En...".into(), if util::which("nmbd") { "Yes".into() } else { "No".into() }));
    for a in &d.ip6 {
        let addr = a.split('/').next().unwrap_or("").to_string();
        rows.push((if addr.starts_with("fe80") { "Link-local IPv6 Address".into() } else { "IPv6 Address".into() }, addr));
    }
    rows.push(("IPv6 Default Gateway".into(), d.gw6.clone()));
    for (i, s) in d.dns6.iter().enumerate() { rows.push((if i == 0 { "IPv6 DNS Server".into() } else { String::new() }, s.clone())); }
    for (k, v) in &rows { store.insert_with_values(None, &[(0, k), (1, v)]); }
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Property", 0, 160), ("Value", 1, 0)]);
    c.pack_start(&scrolled(&tv), true, true, 0);
    let text: String = rows.iter().map(|(k, v)| format!("{k}\t{v}")).collect::<Vec<_>>().join("\n");
    let copy = gtk::Button::with_mnemonic("_Copy");
    copy.set_halign(gtk::Align::Start);
    copy.connect_clicked(move |_| super::super::admin::copy(&text));
    c.pack_start(&copy, false, false, 0);
    dl.connect_response(|d, _| d.close());
    dl.show_all();
}

// ------------------------------------------------------------- Windows Network Diagnostics

/// The checks Windows Network Diagnostics runs, as (problem found?, text). Worker thread.
fn checks(dev: Option<Dev>) -> Vec<(bool, String)> {
    use std::net::ToSocketAddrs;
    let net = gather();
    let mut out = vec![];
    let devs: Vec<Dev> = match &dev { Some(d) => net.devs.iter().filter(|x| x.iface == d.iface).cloned().collect(), None => net.devs.clone() };
    let up: Vec<&Dev> = devs.iter().filter(|d| d.up()).collect();
    if up.is_empty() {
        for d in &devs {
            out.push((true, match d.state.as_str() {
                "unavailable" if d.kind == "ethernet" => format!("{}: a network cable is not properly plugged in or may be broken.", d.iface),
                "disconnected" if !d.autoconnect => format!("{}: the network adapter is disabled. Enable it in Network Connections.", d.iface),
                _ => format!("{}: not connected to a network.", d.iface),
            }));
        }
        if devs.is_empty() { out.push((true, "Windows couldn't find a network adapter.".into())); }
        return out;
    }
    for d in &up {
        if d.ip4.is_empty() && d.ip6.iter().all(|a| a.starts_with("fe80")) { out.push((true, format!("{} doesn't have a valid IP configuration.", d.iface))); continue; }
        if d.gw4.is_empty() && d.gw6.is_empty() { out.push((true, format!("{}: no default gateway is configured.", d.iface))); continue; }
        if !d.gw4.is_empty() && util::which("ping") {
            let ok = run(&["ping", "-c", "1", "-W", "2", &d.gw4]).0;
            out.push((!ok, if ok { format!("The default gateway {} is available.", d.gw4) } else { format!("The default gateway {} is not available.", d.gw4) }));
        }
    }
    let dns = ("archlinux.org", 443).to_socket_addrs().map(|mut a| a.next()).ok().flatten();
    match dns {
        None => out.push((true, "The DNS server isn't responding.".into())),
        Some(addr) => {
            out.push((false, "The DNS server is responding.".into()));
            let ok = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(4)).is_ok();
            out.push((!ok, if ok { "This computer can reach the Internet.".into() } else { "Windows can't communicate with archlinux.org (the remote host isn't responding).".into() }));
        }
    }
    out
}

/// "Windows Network Diagnostics": detecting problems, then what was found.
fn diagnose(parent: &gtk::Window, dev: Option<&Dev>) {
    let dl = dialog(Some(parent), "Windows Network Diagnostics", &[]);
    dl.set_default_size(460, 300);
    let c = dl.content_area();
    let head = label("Detecting problems", "wu-title");
    c.pack_start(&head, false, false, 0);
    let body = vbox(6);
    let pb = gtk::ProgressBar::new();
    body.pack_start(&pb, false, false, 4);
    body.pack_start(&label("Checking the network adapter, the IP configuration, the default gateway, DNS and the Internet connection...", ""), false, false, 0);
    c.pack_start(&body, true, true, 0);
    let close = dl.add_button("Cancel", gtk::ResponseType::Close).downcast::<gtk::Button>().ok();
    let pulse = glib::timeout_add_local(std::time::Duration::from_millis(120), { let pb = pb.clone(); move || { pb.pulse(); glib::ControlFlow::Continue } });
    let pulse = Rc::new(RefCell::new(Some(pulse)));
    { let p = pulse.clone(); dl.connect_response(move |d, _| { if let Some(t) = p.borrow_mut().take() { t.remove(); } d.close(); }); }
    let dev = dev.cloned();
    bg(move || checks(dev), move |res| {
        if let Some(t) = pulse.borrow_mut().take() { t.remove(); }
        for ch in body.children() { body.remove(&ch); }
        let problems: Vec<&(bool, String)> = res.iter().filter(|r| r.0).collect();
        head.set_text("Troubleshooting has completed");
        if problems.is_empty() {
            body.pack_start(&label("Windows did not find any problems with this connection.", ""), false, false, 0);
        } else {
            body.pack_start(&label("The troubleshooter made some changes to your system. Try attempting the task you were trying to do before." , "cp-hint"), false, false, 0);
            body.pack_start(&bold("Problems found"), false, false, 4);
        }
        for (bad, t) in &res {
            let h = hbox(6);
            h.pack_start(&img(if *bad { &["dialog-error"][..] } else { &["aerode-shield-ok", "emblem-ok", "dialog-information"][..] }, 16), false, false, 0);
            h.pack_start(&label(t, ""), false, false, 0);
            body.pack_start(&h, false, false, 0);
        }
        if let Some(b) = &close { b.set_label("Close"); }
        body.show_all();
    });
    dl.show_all();
}

// ------------------------------------------------------------ advanced sharing settings

#[derive(Clone, Debug, Default, PartialEq)]
struct Sharing { discovery: bool, has_avahi: bool, files: bool, has_samba: bool, printers: bool, has_cups: bool, public: bool, password: bool, media: bool, has_media: bool }

fn unit_on(u: &str) -> bool { sh(&["systemctl", "is-active", u]).trim() == "active" }

fn sharing_state() -> Sharing {
    let smb = std::fs::read_to_string("/etc/samba/smb.conf").unwrap_or_default();
    let guest = smb.lines().map(|l| l.trim().to_lowercase()).any(|l| l.starts_with("map to guest") && l.contains("bad user"));
    Sharing {
        discovery: unit_on("avahi-daemon.service"), has_avahi: util::which("avahi-daemon"),
        files: unit_on("smb.service"), has_samba: util::which("smbd"),
        printers: sh(&["cupsctl"]).lines().any(|l| l.trim() == "_share_printers=1"), has_cups: util::which("cupsctl"),
        public: sh(&["net", "usershare", "list"]).lines().any(|l| l.trim() == "Public"),
        password: !guest,
        media: unit_on("minidlna.service") || sh(&["systemctl", "--user", "is-active", "rygel.service"]).trim() == "active",
        has_media: util::which("minidlnad") || util::which("rygel"),
    }
}

fn sharing_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Advanced sharing settings");
    let v = vbox(8);
    v.pack_start(&title("Change sharing options for different network profiles"), false, false, 0);
    v.pack_start(&text("Windows creates a separate network profile for each network you use. You can choose specific options for each profile."), false, false, 0);
    let body = vbox(8);
    v.pack_start(&body, false, false, 0);
    body.pack_start(&label("Please wait...", "cp-hint"), false, false, 0);
    let c = cp.clone();
    bg(|| (sharing_state(), gather()), move |(st, net)| {
        for ch in body.children() { body.remove(&ch); }
        let current_home = net.active().first().map(|d| d.location() != "Public network").unwrap_or(false);
        // both profiles show the same switches: the services behind them serve every network
        let widgets: Rc<RefCell<Vec<(&'static str, gtk::RadioButton)>>> = Rc::new(RefCell::new(vec![]));
        for (home, name) in [(true, "Home or Work"), (false, "Public")] {
            let cur = home == current_home;
            let head = hbox(8);
            head.style_context().add_class(if cur { "fw-head-green" } else { "fw-head-grey" });
            head.pack_start(&bold(&if cur { format!("{name} (current profile)") } else { name.to_string() }), true, true, 0);
            let chev = chevron(cur);
            head.pack_end(&chev, false, false, 0);
            body.pack_start(&head, false, false, 0);
            let inner = vbox(10);
            inner.style_context().add_class("fw-body");
            let opt = |inner: &gtk::Box, key: &'static str, h: &str, desc: &str, on: &str, off: &str, val: bool, avail: bool, note: &str| {
                inner.pack_start(&bold(h), false, false, 0);
                let d = label(desc, ""); d.set_max_width_chars(80); inner.pack_start(&d, false, false, 0);
                let r = radios(&[on, off], if val { 0 } else { 1 });
                for b in &r { b.set_margin_start(20); b.set_sensitive(avail); inner.pack_start(b, false, false, 0); }
                if !avail && !note.is_empty() { let n = label(note, "cp-hint"); n.set_margin_start(20); inner.pack_start(&n, false, false, 0); }
                widgets.borrow_mut().push((key, r[0].clone()));
            };
            opt(&inner, "discovery", "Network discovery", "When network discovery is on, this computer can see other network computers and devices and is visible to other network computers.",
                "Turn on network discovery", "Turn off network discovery", st.discovery, st.has_avahi, "Network discovery needs Avahi (the avahi package).");
            opt(&inner, "files", "File and printer sharing", "When file and printer sharing is on, files and printers that you have shared from this computer can be accessed by people on the network.",
                "Turn on file and printer sharing", "Turn off file and printer sharing", st.files || st.printers, st.has_samba || st.has_cups, "Turn on File and Printer Sharing in Windows Features (Samba) first.");
            opt(&inner, "public", "Public folder sharing", "When Public folder sharing is on, people on the network, including homegroup members, can access files in the Public folders.",
                "Turn on sharing so anyone with network access can read and write files in the Public folders", "Turn off Public folder sharing (people logged on to this computer can still access these folders)", st.public, st.has_samba, "");
            opt(&inner, "media", "Media streaming", "When media streaming is on, people and devices on the network can access pictures, music, and videos on this computer. This computer can also find media on the network.",
                "Turn on media streaming", "Turn off media streaming", st.media, st.has_media, "Media streaming needs Rygel or MiniDLNA.");
            inner.pack_start(&bold("File sharing connections"), false, false, 0);
            let fr = radios(&["Use 128-bit encryption to help protect file sharing connections (recommended)", "Enable file sharing for devices that use 40- or 56-bit encryption"], 0);
            for b in &fr { b.set_margin_start(20); b.set_sensitive(false); inner.pack_start(b, false, false, 0); }
            opt(&inner, "password", "Password protected sharing", "When password protected sharing is on, only people who have a user account and password on this computer can access shared files, printers attached to this computer, and the Public folders. To give other people access, you must turn off password protected sharing.",
                "Turn on password protected sharing", "Turn off password protected sharing", st.password, st.has_samba && std::path::Path::new("/etc/samba/smb.conf").exists(), "");
            inner.set_no_show_all(!cur);
            if cur { inner.show_all(); }
            { let (inner, chev2) = (inner.clone(), chev.clone()); chev.connect_clicked(move |_| { let open = !inner.is_visible(); if open { inner.set_no_show_all(false); inner.show_all(); } else { inner.hide(); } set_chevron(&chev2, open); }); }
            body.pack_start(&inner, false, false, 0);
        }
        let save = shield_button("Save changes");
        let cancel = gtk::Button::with_label("Cancel");
        { let c = c.clone(); cancel.connect_clicked(move |_| back(&c, "network")); }
        let (c2, st0) = (c.clone(), st.clone());
        save.connect_clicked(move |_| {
            // the current profile's choice wins when both were changed
            let mut want = st0.clone();
            let ws = widgets.borrow();
            let n = ws.len() / 2;
            let cur_off = if current_home { 0 } else { n };
            for (k, r) in ws[cur_off..cur_off + n].iter().chain(ws[n - cur_off..n - cur_off + n].iter()) {
                let on = r.is_active();
                let base = match *k { "discovery" => st0.discovery, "files" => st0.files || st0.printers, "public" => st0.public, "media" => st0.media, _ => st0.password };
                if on == base { continue; }
                match *k { "discovery" => want.discovery = on, "files" => { want.files = on; want.printers = on; } "public" => want.public = on, "media" => want.media = on, _ => want.password = on }
            }
            apply_sharing(&c2, &st0, &want);
        });
        body.pack_start(&button_row(&[&save, &cancel]), false, false, 0);
        body.show_all();
    });
    v.upcast()
}

fn apply_sharing(cp: &Rc<Cp>, was: &Sharing, want: &Sharing) {
    let mut root: Vec<String> = vec![];
    if want.discovery != was.discovery { root.push(if want.discovery { "systemctl enable --now avahi-daemon.service".into() } else { "systemctl disable --now avahi-daemon.service avahi-daemon.socket".into() }); }
    if want.files != was.files && was.has_samba { root.push(if want.files { "systemctl enable --now smb.service nmb.service".into() } else { "systemctl disable --now smb.service nmb.service".into() }); }
    if want.printers != was.printers && was.has_cups { root.push(format!("cupsctl {}", if want.printers { "--share-printers" } else { "--no-share-printers" })); }
    if want.media != was.media && util::which("minidlnad") { root.push(if want.media { "systemctl enable --now minidlna.service".into() } else { "systemctl disable --now minidlna.service".into() }); }
    if want.password != was.password {
        let val = if want.password { "Never" } else { "Bad User" };
        root.push(format!("f=/etc/samba/smb.conf; if grep -qi '^[[:space:]]*map to guest' $f; then sed -i 's/^[[:space:]]*map to guest.*/   map to guest = {val}/I' $f; else sed -i '/^\\[global\\]/a\\   map to guest = {val}' $f; fi; systemctl try-reload-or-restart smb.service"));
    }
    // user-level: the Public folder usershare and Rygel
    let public = want.public != was.public;
    let rygel = want.media != was.media && !util::which("minidlnad") && util::which("rygel");
    let (c, want2) = (cp.clone(), want.clone());
    let user_part = move || {
        if public {
            let dir = util::special_dir(glib::UserDirectory::PublicShare, "Public");
            let _ = std::fs::create_dir_all(&dir);
            let r = if want2.public { run(&["net", "usershare", "add", "Public", &dir.to_string_lossy(), "Public folder", "Everyone:F", "guest_ok=y"]) } else { run(&["net", "usershare", "delete", "Public"]) };
            if !r.0 { util::error("Public folder sharing could not be changed.", r.1.trim()); }
        }
        if rygel { let _ = run(&["systemctl", "--user", if want2.media { "enable" } else { "disable" }, "--now", "rygel.service"]); }
        back(&c, "network");
    };
    if root.is_empty() { user_part(); return; }
    elevate_sh(&root.join("; "), move |ok, out| { if report("The sharing settings could not be saved.", ok, &out) { user_part(); } });
}

use super::super::admin::{chevron, set_chevron};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        let d = |k: &str| Dev { kind: k.into(), ..Default::default() };
        assert_eq!(adapter_names(&[d("ethernet"), d("wifi"), d("ethernet")]), vec!["Local Area Connection", "Wireless Network Connection", "Local Area Connection 2"]);
        assert_eq!(mask(24), "255.255.255.0");
        assert_eq!(mask(0), "0.0.0.0");
        assert_eq!(security_name("wpa-psk"), "WPA2-Personal");
    }
}
