//! Windows Firewall on ufw or firewalld: the two network locations with the green/red shield
//! headers, Turn Windows Firewall on or off, Allow a program or feature (ufw rules / firewalld
//! services and ports per zone: home = private, public = public), Restore defaults, Advanced
//! settings (firewall-config / gufw). Every change goes through pkexec.
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use super::super::super::{link, text, title, Cp};
use super::*;

#[derive(Clone, Default, Debug)]
pub struct Fw {
    /// "ufw" | "firewalld" | "" (none); `vendor` names another active firewall (nftables…)
    pub backend: &'static str,
    pub vendor: String,
    pub on: bool,
    pub default_zone: String,
    /// active firewalld zones with their interfaces
    pub zones: Vec<(String, Vec<String>)>,
    pub incoming: String,
}

fn active(unit: &str) -> bool { sh(&["systemctl", "is-active", unit]).trim() == "active" }

/// What protects this computer (worker thread; no root needed).
pub fn state() -> Fw {
    let mut f = Fw::default();
    let fwd = util::which("firewall-cmd");
    let ufw = util::which("ufw");
    let fwd_on = fwd && sh(&["firewall-cmd", "--state"]).trim() == "running";
    if fwd_on || (fwd && !ufw) {
        f.backend = "firewalld";
        f.on = fwd_on;
        if fwd_on {
            f.default_zone = sh(&["firewall-cmd", "--get-default-zone"]).trim().to_string();
            let mut cur: Option<(String, Vec<String>)> = None;
            for l in sh(&["firewall-cmd", "--get-active-zones"]).lines() {
                if !l.starts_with(' ') && !l.trim().is_empty() { if let Some(c) = cur.take() { f.zones.push(c); } cur = Some((l.split_whitespace().next().unwrap_or("").to_string(), vec![])); }
                else if let (Some(c), Some(i)) = (cur.as_mut(), l.trim().strip_prefix("interfaces:")) { c.1 = i.split_whitespace().map(|s| s.to_string()).collect(); }
            }
            if let Some(c) = cur.take() { f.zones.push(c); }
        }
    } else if ufw {
        f.backend = "ufw";
        let conf = std::fs::read_to_string("/etc/ufw/ufw.conf").unwrap_or_default();
        let enabled = conf.lines().any(|l| l.trim().eq_ignore_ascii_case("ENABLED=yes"));
        f.on = enabled && (active("ufw") || sh(&["systemctl", "is-enabled", "ufw"]).trim() == "enabled");
        f.incoming = std::fs::read_to_string("/etc/default/ufw").unwrap_or_default().lines().find_map(|l| l.strip_prefix("DEFAULT_INPUT_POLICY=")).unwrap_or("\"DROP\"").trim_matches('"').to_string();
    }
    if !f.on {
        for (unit, name) in [("nftables", "nftables"), ("iptables", "iptables"), ("opensnitchd", "OpenSnitch")] { if active(unit) { f.vendor = name.into(); break; } }
    }
    f
}

/// Zone → Win7 network location.
pub fn is_private(zone: &str) -> bool { matches!(zone, "home" | "work" | "internal" | "trusted") }

/// Friendly names for common services / ports.
pub fn friendly(service: &str) -> String {
    let s = service.trim();
    let named = match s {
        "ssh" | "22/tcp" | "OpenSSH" | "22" => "Secure Shell (SSH)",
        "samba" | "445/tcp" | "Samba" | "CIFS" => "File and Printer Sharing (Samba)",
        "samba-client" => "File and Printer Sharing (client)",
        "ipp" | "631/tcp" | "CUPS" => "Network printing (IPP)",
        "ipp-client" => "Network printing (client)",
        "mdns" | "5353/udp" | "mDNS" => "Network Discovery (mDNS)",
        "dhcpv6-client" => "Core Networking (DHCPv6)",
        "kdeconnect" | "1714:1764/tcp" | "1714:1764/udp" | "1714-1764/tcp" | "1714-1764/udp" | "KDE Connect" => "KDE Connect",
        "http" | "80/tcp" | "WWW" => "World Wide Web Services (HTTP)",
        "https" | "443/tcp" | "WWW Secure" => "Secure World Wide Web Services (HTTPS)",
        "cockpit" | "9090/tcp" => "Remote Administration (Cockpit)",
        "vnc-server" | "5900/tcp" => "Remote Desktop (VNC)",
        "rdp" | "3389/tcp" => "Remote Desktop",
        "syncthing" | "22000/tcp" => "Syncthing",
        "steam-streaming" => "Steam In-Home Streaming",
        "upnp-client" => "Network Discovery (UPnP)",
        "llmnr" => "Link-Local Multicast Name Resolution",
        "wireguard" => "WireGuard VPN",
        "transmission-client" => "Transmission BitTorrent",
        _ => "",
    };
    if !named.is_empty() { return named.into(); }
    if let Some((p, proto)) = s.split_once('/') { if p.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '-') { return format!("Port {} ({})", p.replace(':', "-"), proto.to_uppercase()); } }
    s.to_string()
}

/// One row of the allowed list: key (ufw rule spec / firewalld service or port), private, public.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule { pub key: String, pub private: bool, pub public: bool }

/// `ufw status` (needs root) → allowed rules (v6 duplicates merged).
pub fn parse_ufw(out: &str) -> Vec<Rule> {
    let mut m: BTreeMap<String, Rule> = BTreeMap::new();
    for l in out.lines() {
        // "[ 1] 22/tcp   ALLOW IN   Anywhere" (status numbered) or the plain form
        let l = l.trim_start();
        let l = if l.starts_with('[') { l.split_once(']').map(|x| x.1).unwrap_or(l) } else { l };
        let Some(pos) = l.find("ALLOW") else { continue };
        let to = l[..pos].trim().trim_end_matches("(v6)").trim().to_string();
        if to.is_empty() || to.starts_with("To") { continue; }
        m.entry(to.clone()).or_insert(Rule { key: to, private: true, public: true });
    }
    m.into_values().collect()
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    let holder = vbox(0);
    holder.style_context().add_class("adm");
    let ui = Rc::new(Ui { cp: cp.clone(), holder: holder.clone(), fw: RefCell::new(None) });
    { let u = ui.clone(); side_action(cp, "Allow a program or feature through Windows Firewall", move || allowed(&u)); }
    { let u = ui.clone(); side_shield(cp, "Change notification settings", move || customize(&u)); }
    { let u = ui.clone(); side_shield(cp, "Turn Windows Firewall on or off", move || customize(&u)); }
    { let u = ui.clone(); side_shield(cp, "Restore defaults", move || restore(&u)); }
    side_shield(cp, "Advanced settings", || { for t in [&["firewall-config"][..], &["gufw"], &["plasma-firewall"]] { if util::spawn(t) { return; } } util::error("No advanced firewall tool is installed.", "Install firewall-config (for firewalld) or gufw (for ufw)."); });
    { let cp2 = cp.clone(); side_action(cp, "Troubleshoot my network", move || cp2.open("troubleshooting")); }
    see_also(cp, &[("Action Center", "actioncenter"), ("Network and Sharing Center", "network")]);
    main_view(&ui);
    holder.upcast()
}

struct Ui { cp: Rc<Cp>, holder: gtk::Box, fw: RefCell<Option<(Fw, Vec<String>)>> }

fn refresh(ui: &Rc<Ui>, then: impl FnOnce(&Rc<Ui>) + 'static) {
    let u = ui.clone();
    bg(|| {
        let f = state();
        let nets: Vec<String> = sh(&["nmcli", "-t", "-f", "NAME,DEVICE", "connection", "show", "--active"]).lines().filter_map(|l| { let p = aerode::sys::nm_split(l); (p.len() >= 2 && p[1] != "lo").then(|| format!("{}\u{1}{}", p[0], p[1])) }).collect();
        (f, nets)
    }, move |r| { *u.fw.borrow_mut() = Some(r); then(&u); });
}

fn main_view(ui: &Rc<Ui>) {
    for c in ui.holder.children() { ui.holder.remove(&c); }
    let v = vbox(8);
    v.pack_start(&title("Help protect your computer with Windows Firewall"), false, false, 0);
    v.pack_start(&text("Windows Firewall can help prevent hackers or malicious software from gaining access to your computer through the Internet or a network."), false, false, 0);
    v.pack_start(&link("How does a firewall help protect my computer?", || { util::open_uri("https://wiki.archlinux.org/title/Firewall"); }), false, false, 0);
    v.pack_start(&link("What are network locations?", || { util::open_uri("https://firewalld.org/documentation/zone/predefined-zones.html"); }), false, false, 0);
    let body = vbox(10);
    body.set_margin_top(8);
    v.pack_start(&body, false, false, 0);
    ui.holder.pack_start(&v, true, true, 0);
    ui.holder.show_all();
    let Some((f, nets)) = ui.fw.borrow().clone() else {
        body.pack_start(&label("Checking the firewall…", "dim"), false, false, 0);
        body.show_all();
        refresh(ui, |u| main_view(u));
        return;
    };
    // the red "Update your Firewall settings" box
    if !f.on && f.vendor.is_empty() {
        let h = hbox(0);
        h.style_context().add_class("wu-panel");
        let band = vbox(0);
        band.style_context().add_class("ac-band-red");
        h.pack_start(&band, false, false, 0);
        let inner = hbox(12);
        inner.set_border_width(10);
        inner.pack_start(&img(&["aerode-shield-error", "dialog-error"], 32), false, false, 0);
        let col = vbox(3);
        col.pack_start(&bold("Update your Firewall settings"), false, false, 0);
        col.pack_start(&label(if f.backend.is_empty() { "No firewall program is installed on this computer, so nothing blocks incoming connections." } else { "Windows Firewall is not using the recommended settings to protect your computer." }, ""), false, false, 0);
        col.pack_start(&link("What are the recommended settings?", || { util::open_uri("https://wiki.archlinux.org/title/Uncomplicated_Firewall#Basic_configuration"); }), false, false, 0);
        inner.pack_start(&col, true, true, 0);
        let b = shield_button(if f.backend.is_empty() { "_Install a firewall" } else { "_Use recommended settings" });
        b.set_valign(gtk::Align::Center);
        { let u = ui.clone(); let be = f.backend; b.connect_clicked(move |_| recommended(&u, be)); }
        inner.pack_start(&b, false, false, 0);
        h.pack_start(&inner, true, true, 0);
        body.pack_start(&h, false, false, 0);
    }
    if !f.vendor.is_empty() {
        let h = hbox(10);
        h.pack_start(&img(&["dialog-information"], 32), false, false, 0);
        h.pack_start(&label(&format!("These settings are being managed by vendor application {}", f.vendor), ""), false, false, 0);
        body.pack_start(&h, false, false, 0);
    }
    // which location each active connection is in
    let (mut private_nets, mut public_nets) = (vec![], vec![]);
    for n in &nets {
        let (name, dev) = n.split_once('\u{1}').unwrap_or((n.as_str(), ""));
        let zone = f.zones.iter().find(|z| z.1.iter().any(|i| i == dev)).map(|z| z.0.clone()).unwrap_or_else(|| f.default_zone.clone());
        if f.backend == "firewalld" && is_private(&zone) { private_nets.push(name.to_string()) } else { public_nets.push(name.to_string()) }
    }
    for (private, nets) in [(true, private_nets), (false, public_nets)] {
        body.pack_start(&location(&f, private, &nets), false, false, 0);
    }
    body.show_all();
}

/// One network location: coloured header with shield, Connected, chevron; details when open.
fn location(f: &Fw, private: bool, nets: &[String]) -> gtk::Widget {
    let v = vbox(0);
    let head = hbox(8);
    let connected = !nets.is_empty();
    head.style_context().add_class(if !f.on { "fw-head-red" } else if connected { "fw-head-green" } else { "fw-head-grey" });
    head.pack_start(&img(if f.on { &["aerode-shield-ok", "security-high"] } else { &["aerode-shield-error", "dialog-error"] }, 24), false, false, 0);
    let t = label(if private { "Home or work (private) networks" } else { "Public networks" }, "fw-head-title");
    head.pack_start(&t, true, true, 0);
    head.pack_start(&gtk::Label::new(Some(if connected { "Connected" } else { "Not Connected" })), false, false, 0);
    let chev = chevron(connected);
    head.pack_start(&chev, false, false, 0);
    v.pack_start(&head, false, false, 0);
    let body = vbox(6);
    body.style_context().add_class("fw-body");
    body.pack_start(&label(if private { "Networks at home or work where you know and trust the people and devices on the network" } else { "Networks in public places such as airports or coffee shops" }, ""), false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(20);
    g.set_margin_top(6);
    let state = if f.on { "On".to_string() } else if !f.vendor.is_empty() { format!("Managed by {}", f.vendor) } else { "Off".into() };
    let incoming = if !f.on { "Allow all connections".to_string() } else if f.incoming.eq_ignore_ascii_case("ACCEPT") { "Allow all connections (incoming policy is ACCEPT)".into() } else { "Block all connections to programs that are not on the list of allowed programs".into() };
    let netbox = vbox(2);
    if nets.is_empty() { netbox.pack_start(&label("None", ""), false, false, 0); }
    for n in nets { let h = hbox(4); h.pack_start(&img(&["network-wired", "network-workgroup"], 16), false, false, 0); h.pack_start(&label(n, ""), false, false, 0); netbox.pack_start(&h, false, false, 0); }
    let rows: Vec<(String, gtk::Widget)> = vec![
        ("Windows Firewall state:".into(), label(&state, "").upcast()),
        ("Incoming connections:".into(), { let l = label(&incoming, ""); l.set_max_width_chars(48); l.upcast() }),
        (format!("Active {} networks:", if private { "home or work (private)" } else { "public" }), netbox.upcast()),
        ("Notification state:".into(), { let l = label("Do not notify me when Windows Firewall blocks a new program", ""); l.set_max_width_chars(48); l.upcast() }),
    ];
    for (i, (k, w)) in rows.iter().enumerate() {
        let l = label(k, ""); l.set_yalign(0.0); l.set_max_width_chars(28);
        g.attach(&l, 0, i as i32, 1, 1);
        g.attach(w, 1, i as i32, 1, 1);
    }
    body.pack_start(&g, false, false, 0);
    v.pack_start(&body, false, false, 0);
    body.set_no_show_all(!connected);
    if connected { body.show_all(); }
    { let (b, c) = (body.clone(), chev.clone()); chev.connect_clicked(move |_| { let open = !b.is_visible(); if open { b.set_no_show_all(false); b.show_all(); } else { b.hide(); } set_chevron(&c, open); }); }
    v.upcast()
}

/// "Use recommended settings" / install ufw.
fn recommended(ui: &Rc<Ui>, backend: &str) {
    let script = match backend {
        "firewalld" => "systemctl enable --now firewalld".to_string(),
        "ufw" => "ufw default deny incoming && ufw default allow outgoing && systemctl enable --now ufw && ufw --force enable".into(),
        _ => {
            if !confirm(win_of(&ui.holder).as_ref(), "Windows Firewall", "Install and turn on a firewall?", "The ufw firewall will be installed from the Arch repositories and set to block incoming connections.", "Install") { return; }
            "pacman -S --needed --noconfirm ufw && ufw default deny incoming && ufw default allow outgoing && systemctl enable --now ufw && ufw --force enable".into()
        }
    };
    let u = ui.clone();
    elevate_sh(&script, move |ok, out| { report("Windows Firewall could not change the settings.", ok, &out); *u.fw.borrow_mut() = None; main_view(&u); });
}

/// "Customize settings for each type of network".
fn customize(ui: &Rc<Ui>) {
    let Some((f, _)) = ui.fw.borrow().clone() else { return };
    let v = vbox(10);
    v.pack_start(&title("Customize settings for each type of network"), false, false, 0);
    v.pack_start(&text("You can modify the firewall settings for each type of network location that you use."), false, false, 0);
    v.pack_start(&link("What are network locations?", || { util::open_uri("https://firewalld.org/documentation/zone/predefined-zones.html"); }), false, false, 0);
    if f.backend == "ufw" { v.pack_start(&label("ufw has one set of rules for every network, so both locations are turned on or off together.", "dim"), false, false, 0); }
    let mut radios: Vec<(gtk::RadioButton, gtk::RadioButton)> = vec![];
    for name in ["Home or work (private) network location settings", "Public network location settings"] {
        v.pack_start(&super::super::super::section(name), false, false, 0);
        let g = gtk::Grid::new();
        g.set_row_spacing(4);
        g.set_column_spacing(8);
        g.set_margin_start(12);
        let on = gtk::RadioButton::with_label("Turn on Windows Firewall");
        let off = gtk::RadioButton::with_label_from_widget(&on, "Turn off Windows Firewall (not recommended)");
        if f.on { on.set_active(true) } else { off.set_active(true) }
        g.attach(&img(&["aerode-shield-ok", "security-high"], 24), 0, 0, 1, 1);
        g.attach(&on, 1, 0, 1, 1);
        let block = gtk::CheckButton::with_label("Block all incoming connections, including those in the list of allowed programs");
        block.set_sensitive(false);
        block.set_tooltip_text(Some("Not available: allowed programs are always reachable while the firewall is on."));
        block.set_margin_start(24);
        g.attach(&block, 1, 1, 1, 1);
        let notify = gtk::CheckButton::with_label("Notify me when Windows Firewall blocks a new program");
        notify.set_sensitive(false);
        notify.set_margin_start(24);
        g.attach(&notify, 1, 2, 1, 1);
        g.attach(&img(&["aerode-shield-error", "dialog-error"], 24), 0, 3, 1, 1);
        g.attach(&off, 1, 3, 1, 1);
        v.pack_start(&g, false, false, 0);
        radios.push((on, off));
    }
    // the firewall is global: keep the two locations in step
    let syncing = Rc::new(std::cell::Cell::new(false));
    for i in 0..2 {
        let (me, other) = (radios[i].clone(), radios[1 - i].clone());
        let s = syncing.clone();
        me.0.connect_toggled(move |b| { if s.get() { return; } s.set(true); if b.is_active() { other.0.set_active(true) } else { other.1.set_active(true) } s.set(false); });
    }
    let bb = hbox(8);
    let ok = shield_button("OK");
    let cancel = gtk::Button::with_label("Cancel");
    ok.set_size_request(80, -1);
    cancel.set_size_request(80, -1);
    bb.pack_end(&cancel, false, false, 0);
    bb.pack_end(&ok, false, false, 0);
    v.pack_start(&bb, false, false, 8);
    { let u = ui.clone(); cancel.connect_clicked(move |_| main_view(&u)); }
    { let (u, on) = (ui.clone(), radios[0].0.clone()); let was = f.on; let be = f.backend; ok.connect_clicked(move |_| {
        let want = on.is_active();
        if want == was { main_view(&u); return; }
        let script = match (be, want) {
            ("ufw", true) => "systemctl enable --now ufw && ufw --force enable",
            ("ufw", false) => "ufw disable",
            ("firewalld", true) => "systemctl enable --now firewalld",
            ("firewalld", false) => "systemctl disable --now firewalld",
            _ => { recommended(&u, ""); return; }
        };
        let u2 = u.clone();
        elevate_sh(script, move |ok, out| { report("Windows Firewall could not change the settings.", ok, &out); *u2.fw.borrow_mut() = None; main_view(&u2); });
    }); }
    subpage(&ui.cp, &ui.holder, "firewall", "Customize Settings", &v);
}

fn restore(ui: &Rc<Ui>) {
    let Some((f, _)) = ui.fw.borrow().clone() else { return };
    let v = vbox(10);
    v.pack_start(&title("Restore default settings"), false, false, 0);
    v.pack_start(&text("Restoring defaults will remove all Windows Firewall settings that you have configured for all network locations. This might cause some programs to stop working."), false, false, 0);
    let b = shield_button("_Restore defaults");
    b.set_halign(gtk::Align::Start);
    b.set_sensitive(!f.backend.is_empty());
    let u = ui.clone();
    b.connect_clicked(move |b| {
        let d = gtk::MessageDialog::new(win_of(b).as_ref(), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo, "Are you sure you want to restore the defaults?");
        d.set_title("Restore Defaults Confirmation");
        let r = d.run();
        d.close();
        if r != gtk::ResponseType::Yes { return; }
        let script = if f.backend == "ufw" { "ufw --force reset && ufw default deny incoming && ufw default allow outgoing && systemctl enable --now ufw && ufw --force enable" }
            else { "rm -f /etc/firewalld/zones/*.xml /etc/firewalld/zones/*.xml.old && systemctl enable --now firewalld && firewall-cmd --complete-reload" };
        let u2 = u.clone();
        elevate_sh(script, move |ok, out| { report("The defaults could not be restored.", ok, &out); *u2.fw.borrow_mut() = None; main_view(&u2); });
    });
    v.pack_start(&b, false, false, 0);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_halign(gtk::Align::End);
    { let u = ui.clone(); cancel.connect_clicked(move |_| main_view(&u)); }
    v.pack_start(&cancel, false, false, 0);
    subpage(&ui.cp, &ui.holder, "firewall", "Restore Defaults", &v);
}

/// "Allow programs to communicate through Windows Firewall".
fn allowed(ui: &Rc<Ui>) {
    let Some((f, _)) = ui.fw.borrow().clone() else { return };
    let v = vbox(8);
    v.pack_start(&title("Allow programs to communicate through Windows Firewall"), false, false, 0);
    let top = hbox(10);
    top.pack_start(&text("To add, change, or remove allowed programs and ports, click Change settings."), true, true, 0);
    let change = shield_button("Change setti_ngs");
    top.pack_end(&change, false, false, 0);
    v.pack_start(&top, false, false, 0);
    v.pack_start(&link("What are the risks of allowing a program to communicate?", || { util::open_uri("https://wiki.archlinux.org/title/Firewall"); }), false, false, 0);
    let frame = gtk::Frame::new(Some("Allowed programs and features:"));
    let inner = vbox(6);
    inner.set_border_width(8);
    // key, name, private, public, original private, original public
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL, glib::Type::BOOL, glib::Type::BOOL, glib::Type::BOOL]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Name", 1, 0)]);
    let toggles: Vec<gtk::CellRendererToggle> = [("Home/Work (Private)", 2), ("Public", 3)].iter().map(|(t, c)| {
        let r = gtk::CellRendererToggle::new();
        let col = gtk::TreeViewColumn::new();
        col.set_title(t);
        TreeViewColumnExt::pack_start(&col, &r, false);
        TreeViewColumnExt::add_attribute(&col, &r, "active", *c);
        col.set_fixed_width(130);
        col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
        tv.append_column(&col);
        r
    }).collect();
    let sw = scrolled(&tv);
    sw.set_min_content_height(260);
    inner.pack_start(&sw, true, true, 0);
    let bb = hbox(6);
    let details = gtk::Button::with_mnemonic("Detai_ls...");
    let remove = gtk::Button::with_mnemonic("_Remove");
    bb.pack_end(&remove, false, false, 0);
    bb.pack_end(&details, false, false, 0);
    inner.pack_start(&bb, false, false, 0);
    frame.add(&inner);
    v.pack_start(&frame, true, true, 0);
    let another = gtk::Button::with_mnemonic("Allo_w another program...");
    another.set_halign(gtk::Align::End);
    v.pack_start(&another, false, false, 0);
    let status = label("", "dim");
    v.pack_start(&status, false, false, 0);
    let ok_bar = hbox(8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    ok.set_size_request(80, -1);
    cancel.set_size_request(80, -1);
    ok_bar.pack_end(&cancel, false, false, 0);
    ok_bar.pack_end(&ok, false, false, 0);
    v.pack_start(&ok_bar, false, false, 4);
    let editable = Rc::new(std::cell::Cell::new(false));
    let set_editable = { let (tv, details, remove, another, editable, change) = (tv.clone(), details.clone(), remove.clone(), another.clone(), editable.clone(), change.clone()); move |on: bool| {
        editable.set(on); tv.set_sensitive(on); details.set_sensitive(on); remove.set_sensitive(on); another.set_sensitive(on); change.set_sensitive(!on);
    } };
    set_editable(false);
    let fill = { let store = store.clone(); move |rules: Vec<Rule>| {
        store.clear();
        for r in rules { store.insert_with_values(None, &[(0, &r.key), (1, &friendly(&r.key)), (2, &r.private), (3, &r.public), (4, &r.private), (5, &r.public)]); }
    } };
    let backend = f.backend;
    for (i, t) in toggles.iter().enumerate() {
        let store = store.clone();
        t.connect_toggled(move |_, p| {
            let Some(it) = store.iter(&p) else { return };
            let col = 2 + i as u32;
            let v: bool = store.value(&it, col as i32).get().unwrap_or(false);
            store.set_value(&it, col, &(!v).to_value());
            // ufw has one rule set for both locations
            if backend == "ufw" { store.set_value(&it, if i == 0 { 3 } else { 2 }, &(!v).to_value()); }
        });
    }
    // firewalld can be read without root: show the list (greyed) right away
    if backend == "firewalld" && f.on {
        let fill2 = fill.clone();
        status.set_text("Loading…");
        let st = status.clone();
        bg(|| firewalld_rules(), move |r| { fill2(r); st.set_text(""); });
    } else if backend.is_empty() {
        change.set_sensitive(false);
        status.set_text("No firewall is installed.");
    } else if !f.on {
        status.set_text("Windows Firewall is turned off. Turn it on to use the list of allowed programs.");
    }
    { let (fill, st, set_editable) = (fill.clone(), status.clone(), set_editable.clone()); change.connect_clicked(move |_| {
        if backend == "ufw" {
            st.set_text("Reading the firewall rules…");
            let (fill, st, se) = (fill.clone(), st.clone(), set_editable.clone());
            elevate(&["ufw", "status"], move |ok, out| { if ok { fill(parse_ufw(&out)); se(true); st.set_text(""); } else { st.set_text(""); report("The firewall rules could not be read.", ok, &out); } });
        } else { set_editable(true); }
    }); }
    { let (tv, store) = (tv.clone(), store.clone()); remove.connect_clicked(move |_| { if let Some((_, it)) = tv.selection().selected() { store.set_value(&it, 2, &false.to_value()); store.set_value(&it, 3, &false.to_value()); } }); }
    { let tv = tv.clone(); details.connect_clicked(move |b| {
        let Some((m, it)) = tv.selection().selected() else { return };
        let key = sval(&m, &it, 0);
        inform(win_of(b).as_ref(), "Edit a Program", &friendly(&key), &format!("Firewall rule: {key}\n{}", if key.contains('/') { "Allows incoming connections to this port." } else { "Allows the ports this service uses." }));
    }); }
    { let (store, b2) = (store.clone(), backend); another.connect_clicked(move |b| { if let Some(k) = add_program(win_of(b).as_ref(), b2) { store.insert_with_values(None, &[(0, &k), (1, &friendly(&k)), (2, &true), (3, &(b2 == "ufw")), (4, &false), (5, &false)]); } }); }
    { let u = ui.clone(); cancel.connect_clicked(move |_| main_view(&u)); }
    { let (u, store) = (ui.clone(), store.clone()); ok.connect_clicked(move |_| {
        if !editable.get() { main_view(&u); return; }
        let mut cmds: Vec<String> = vec![];
        store.foreach(|m, _, it| {
            let key = sval(m, it, 0);
            let (p, pu, op, opu): (bool, bool, bool, bool) = (m.value(it, 2).get().unwrap_or(false), m.value(it, 3).get().unwrap_or(false), m.value(it, 4).get().unwrap_or(false), m.value(it, 5).get().unwrap_or(false));
            if backend == "ufw" {
                if (p || pu) && !(op || opu) { cmds.push(format!("ufw allow {} comment {}", q(&key), q(&friendly(&key)))); }
                if !(p || pu) && (op || opu) { cmds.push(format!("ufw delete allow {}", q(&key))); }
            } else {
                let kind = if key.contains('/') { "port" } else { "service" };
                for (zone, now, was) in [("home", p, op), ("public", pu, opu)] {
                    if now && !was { cmds.push(format!("firewall-cmd --permanent --zone={zone} --add-{kind}={}", q(&key))); }
                    if !now && was { cmds.push(format!("firewall-cmd --permanent --zone={zone} --remove-{kind}={}", q(&key))); }
                }
            }
            false
        });
        if cmds.is_empty() { main_view(&u); return; }
        if backend == "firewalld" { cmds.push("firewall-cmd --reload".into()); }
        let u2 = u.clone();
        elevate_sh(&cmds.join(" && "), move |ok, out| { report("Windows Firewall could not change the allowed programs.", ok, &out); *u2.fw.borrow_mut() = None; main_view(&u2); });
    }); }
    subpage(&ui.cp, &ui.holder, "firewall", "Allowed Programs", &v);
}

/// firewalld services and ports of the home (private) and public zones, plus common ones.
fn firewalld_rules() -> Vec<Rule> {
    let list = |zone: &str| -> Vec<String> {
        let mut v: Vec<String> = sh(&["firewall-cmd", &format!("--zone={zone}"), "--list-services"]).split_whitespace().map(|s| s.to_string()).collect();
        v.extend(sh(&["firewall-cmd", &format!("--zone={zone}"), "--list-ports"]).split_whitespace().map(|s| s.to_string()));
        v
    };
    let (home, public) = (list("home"), list("public"));
    let mut keys: Vec<String> = home.iter().chain(public.iter()).cloned().collect();
    let known = sh(&["firewall-cmd", "--get-services"]);
    for c in ["ssh", "samba", "ipp", "mdns", "kdeconnect", "http", "https", "syncthing", "vnc-server", "rdp", "steam-streaming"] { if known.split_whitespace().any(|k| k == c) { keys.push(c.into()); } }
    keys.sort_by_key(|k| friendly(k).to_lowercase());
    keys.dedup();
    keys.into_iter().map(|k| Rule { private: home.contains(&k), public: public.contains(&k), key: k }).collect()
}

/// "Add a Program": a known service (ufw app profile / firewalld service) or a port.
fn add_program(parent: Option<&gtk::Window>, backend: &str) -> Option<String> {
    let d = gtk::Dialog::with_buttons(Some("Add a Program"), parent, gtk::DialogFlags::MODAL, &[("_Add", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(420, 440);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(6);
    let prog = gtk::RadioButton::with_label("Select the program you want to add:");
    c.pack_start(&prog, false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let known: Vec<String> = if backend == "ufw" { sh(&["ufw", "app", "list"]).lines().skip(1).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect() }
        else { sh(&["firewall-cmd", "--get-services"]).split_whitespace().map(|s| s.to_string()).collect() };
    let mut items: Vec<(String, String)> = known.into_iter().map(|k| (friendly(&k), k)).collect();
    if items.is_empty() { for k in ["22/tcp", "80/tcp", "443/tcp", "445/tcp", "631/tcp", "5353/udp", "1714:1764/tcp", "3389/tcp", "5900/tcp", "22000/tcp"] { items.push((friendly(k), k.to_string())); } }
    items.sort();
    for (n, k) in &items { store.insert_with_values(None, &[(0, n), (1, k)]); }
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    columns(&tv, &[("Programs", 0, 0)]);
    let sw = scrolled(&tv);
    sw.set_min_content_height(220);
    c.pack_start(&sw, true, true, 0);
    let port = gtk::RadioButton::with_label_from_widget(&prog, "Allow a port:");
    c.pack_start(&port, false, false, 0);
    let g = gtk::Grid::new();
    g.set_column_spacing(8);
    g.set_row_spacing(6);
    g.set_margin_start(20);
    g.attach(&gtk::Label::new(Some("Port number:")), 0, 0, 1, 1);
    let num = gtk::Entry::new();
    num.set_placeholder_text(Some("8080 or 6000:6010"));
    g.attach(&num, 1, 0, 1, 1);
    let tcp = gtk::RadioButton::with_label("TCP");
    let udp = gtk::RadioButton::with_label_from_widget(&tcp, "UDP");
    let pr = hbox(10);
    pr.pack_start(&tcp, false, false, 0);
    pr.pack_start(&udp, false, false, 0);
    g.attach(&gtk::Label::new(Some("Protocol:")), 0, 1, 1, 1);
    g.attach(&pr, 1, 1, 1, 1);
    c.pack_start(&g, false, false, 0);
    { let num = num.clone(); let port = port.clone(); num.connect_changed(move |_| port.set_active(true)); }
    d.show_all();
    let r = d.run();
    let out = if r != gtk::ResponseType::Ok { None } else if port.is_active() {
        let n = num.text().trim().to_string();
        (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == ':' || c == '-')).then(|| format!("{}/{}", n.replace('-', ":"), if udp.is_active() { "udp" } else { "tcp" }))
    } else { tv.selection().selected().map(|(m, it)| sval(&m, &it, 1)) };
    d.close();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ufw_status() {
        let out = "Status: active\n\nTo                         Action      From\n--                         ------      ----\n[ 1] 22/tcp                     ALLOW IN    Anywhere\n[ 2] KDE Connect                ALLOW IN    Anywhere\n[ 3] 22/tcp (v6)                ALLOW IN    Anywhere (v6)\n";
        let r = parse_ufw(out);
        assert_eq!(r.len(), 2);
        assert!(r.iter().any(|x| x.key == "22/tcp"));
        assert_eq!(friendly("22/tcp"), "Secure Shell (SSH)");
        assert_eq!(friendly("8080/tcp"), "Port 8080 (TCP)");
    }
}
