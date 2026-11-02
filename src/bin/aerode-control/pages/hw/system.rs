//! System ("View basic information about your computer") and the System Properties dialog:
//! Computer Name (hostnamectl through pkexec, Samba workgroup), Hardware (Device Manager, Device
//! Installation Settings), Advanced (Performance Options with the Win7 Visual Effects list,
//! User Profiles, Startup and Recovery, Environment Variables for ~/.config/environment.d),
//! System Protection (Snapper / Timeshift restore points) and Remote (xrdp, x11vnc).
use super::*;
use super::super::admin::{bg, confirm, elevate, elevate_sh, img, inform, q, report, see_also, shield, shield_button, side_shield, spawn_tool, columns, scrolled, sval};
use super::super::desktop::{check, group, radios, reload_shell, write_xsettings};

fn vbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, s) }
fn hbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, s) }

/// Open a Control Panel page from a dialog: in this window when there is one, else as its own process.
fn goto(cp: &Option<Rc<Cp>>, id: &str) {
    match cp { Some(cp) => cp.open(id), None => spawn_tool(id) }
}

// ------------------------------------------------------------------ facts

fn os_release() -> std::collections::HashMap<String, String> {
    let s = std::fs::read_to_string("/etc/os-release").or_else(|_| std::fs::read_to_string("/usr/lib/os-release")).unwrap_or_default();
    s.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.trim_matches('"').to_string())).collect()
}

fn cpu() -> String {
    let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = info.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))
        .or_else(|| info.lines().find_map(|l| l.strip_prefix("Model").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string())))
        .unwrap_or_else(|| "Unknown processor".into());
    let khz = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/base_frequency")
        .or_else(|_| std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq")).ok()
        .and_then(|s| s.trim().parse::<f64>().ok());
    // Win7: "Intel(R) Core(TM) i7 CPU 920 @ 2.67GHz   2.67 GHz"
    match khz { Some(k) => format!("{}   {:.2} GHz", model.split_whitespace().collect::<Vec<_>>().join(" "), k / 1e6), None => model }
}

/// (installed, usable) RAM in bytes: installed = the online memory blocks, usable = MemTotal.
fn memory() -> (u64, u64) {
    let usable = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next()?.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * 1024;
    let block = std::fs::read_to_string("/sys/devices/system/memory/block_size_bytes").ok().and_then(|s| u64::from_str_radix(s.trim(), 16).ok());
    let installed = block.and_then(|b| {
        let n = std::fs::read_dir("/sys/devices/system/memory").ok()?.flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("memory"))
            .filter(|e| std::fs::read_to_string(e.path().join("online")).map(|s| s.trim() == "1").unwrap_or(false)).count() as u64;
        (n > 0).then_some(n * b)
    }).unwrap_or(usable);
    (installed.max(usable), usable)
}

fn gb1(b: u64) -> String { format!("{:.2} GB", b as f64 / 1024f64.powi(3)) }

fn pen_and_touch() -> String {
    let devs = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
    let (mut touch, mut pen) = (false, false);
    for block in devs.split("\n\n") {
        let name = block.lines().find_map(|l| l.strip_prefix("N: Name=")).unwrap_or("").to_lowercase();
        if name.contains("touchscreen") || (name.contains("touch") && !name.contains("touchpad")) { touch = true; }
        if name.contains("pen") || name.contains("stylus") || name.contains("wacom") { pen = true; }
    }
    match (pen, touch) {
        (true, true) => "Pen and Touch Input Available".into(),
        (true, false) => "Pen Input Available".into(),
        (false, true) => "Touch Input Available".into(),
        _ => "No Pen or Touch Input is available for this Display".into(),
    }
}

/// "Computer description" = systemd's pretty host name (/etc/machine-info).
fn description() -> String {
    std::fs::read_to_string("/etc/machine-info").unwrap_or_default().lines()
        .find_map(|l| l.strip_prefix("PRETTY_HOSTNAME=")).map(|v| v.trim_matches('"').to_string()).unwrap_or_default()
}

/// The Windows (SMB) workgroup from Samba's configuration.
fn workgroup() -> (String, bool) {
    match std::fs::read_to_string("/etc/samba/smb.conf") {
        Ok(s) => (s.lines().find_map(|l| {
            let t = l.trim();
            let (k, v) = t.split_once('=')?;
            (k.trim().eq_ignore_ascii_case("workgroup")).then(|| v.trim().to_string())
        }).unwrap_or_else(|| "WORKGROUP".into()), true),
        Err(_) => ("WORKGROUP".into(), false),
    }
}

fn hostname() -> String { std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| glib::host_name().to_string()) }

/// Win7 product-ID look for the machine id: 00426-OEM-8992662-00006.
fn product_id() -> String {
    let id = std::fs::read_to_string("/etc/machine-id").unwrap_or_default();
    let d: String = id.trim().chars().map(|c| char::from_digit(c.to_digit(16).unwrap_or(0) % 10, 10).unwrap()).collect();
    if d.len() < 20 { return String::new(); }
    format!("{}-{}-{}-{}", &d[0..5], &d[5..8], &d[8..15], &d[15..20])
}

// ------------------------------------------------------------------ the page

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    side_shield(cp, "Device Manager", || spawn_tool("devmgr"));
    { let me = cp.clone(); side_shield(cp, "Remote settings", move || props(Some(&me), 4)); }
    { let me = cp.clone(); side_shield(cp, "System protection", move || props(Some(&me), 3)); }
    { let me = cp.clone(); side_shield(cp, "Advanced system settings", move || props(Some(&me), 2)); }
    see_also(cp, &[("Action Center", "actioncenter"), ("Windows Update", "update"), ("Performance Information and Tools", "perfinfo")]);

    let v = vbox(2);
    v.pack_start(&title("View basic information about your computer"), false, false, 0);
    let os = os_release();
    let get = |k: &str| os.get(k).cloned().unwrap_or_default();

    v.pack_start(&heading("Windows edition"), false, false, 0);
    let ed = hbox(12);
    let lines = vbox(3);
    lines.set_margin_start(16);
    let name = if get("PRETTY_NAME").is_empty() { "Linux".to_string() } else { get("PRETTY_NAME") };
    // rolling releases have no version; others carry it in PRETTY_NAME already
    let build = Some(get("VERSION_ID")).filter(|v| !v.is_empty() && !name.contains(v.as_str())).map(|s| format!(" {s}")).unwrap_or_default();
    for t in [format!("{name}{build}"), "Aerode – the Windows 7 desktop for Linux".to_string(),
        format!("Linux {}", std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim())] {
        lines.pack_start(&lbl(&t), false, false, 0);
    }
    ed.pack_start(&lines, true, true, 0);
    let logo = get("LOGO");
    if !logo.is_empty() && gtk::IconTheme::default().map(|t| t.has_icon(&logo)).unwrap_or(false) {
        let i = util::image(&logo, 64);
        i.set_margin_end(30);
        ed.pack_end(&i, false, false, 0);
    }
    v.pack_start(&ed, false, false, 0);

    v.pack_start(&heading("System"), false, false, 0);
    let (installed, usable) = memory();
    let g = rows_grid();
    let rating = hbox(6);
    match config::get().maintenance.wei.filter(|w| w.when > 0) {
        Some(w) => {
            let base = [w.cpu, w.mem, w.gfx, w.game, w.disk].into_iter().fold(f64::MAX, f64::min);
            rating.pack_start(&wei_badge(base), false, false, 0);
            let me = cp.clone();
            rating.pack_start(&link("Windows Experience Index", move || me.open("perfinfo")), false, false, 0);
        }
        None => { let me = cp.clone(); rating.pack_start(&link("System rating is not available", move || me.open("perfinfo")), false, false, 0); }
    }
    add_row(&g, 0, "Rating:", &rating);
    add_row(&g, 1, "Processor:", &value(&cpu()));
    add_row(&g, 2, "Installed memory (RAM):", &value(&if installed > usable + (64 << 20) { format!("{} ({} usable)", gb1(installed), gb1(usable)) } else { gb1(installed) }));
    add_row(&g, 3, "System type:", &value(&format!("{} Operating System", if cfg!(target_pointer_width = "64") { "64-bit" } else { "32-bit" })));
    add_row(&g, 4, "Pen and Touch:", &value(&pen_and_touch()));
    v.pack_start(&g, false, false, 0);

    v.pack_start(&heading("Computer name, domain, and workgroup settings"), false, false, 0);
    let row = hbox(10);
    let g = rows_grid();
    add_row(&g, 0, "Computer name:", &value(&hostname()));
    add_row(&g, 1, "Full computer name:", &value(&hostname()));
    add_row(&g, 2, "Computer description:", &value(&description()));
    add_row(&g, 3, "Workgroup:", &value(&workgroup().0));
    row.pack_start(&g, true, true, 0);
    let me = cp.clone();
    let change = super::super::admin::shield_link("Change settings", move || props(Some(&me), 0));
    change.set_valign(gtk::Align::Start);
    change.set_margin_end(30);
    row.pack_end(&change, false, false, 0);
    v.pack_start(&row, false, false, 0);

    v.pack_start(&heading("Windows activation"), false, false, 0);
    let g = rows_grid();
    let free = value(&format!("{} is free software: no activation is required.", get("NAME").trim().to_string().chars().take(40).collect::<String>().trim()));
    g.attach(&free, 0, 0, 2, 1);
    let pid = product_id();
    if !pid.is_empty() { add_row(&g, 1, "Product ID:", &value(&pid)); }
    v.pack_start(&g, false, false, 0);
    v.upcast()
}

fn rows_grid() -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(7);
    g.set_column_spacing(16);
    g.set_margin_start(16);
    g
}

fn value(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_selectable(true);
    l.set_line_wrap(true);
    l.set_max_width_chars(60);
    l
}

fn add_row(g: &gtk::Grid, r: i32, k: &str, w: &impl IsA<gtk::Widget>) {
    let a = gtk::Label::new(Some(k));
    a.set_xalign(0.0);
    a.set_valign(gtk::Align::Start);
    a.style_context().add_class("hw-dim");
    a.set_size_request(170, -1);
    g.attach(&a, 0, r, 1, 1);
    g.attach(w, 1, r, 1, 1);
}

/// The blue Windows Experience Index badge with the base score.
fn wei_badge(score: f64) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_size_request(30, 26);
    d.connect_draw(move |w, cr| {
        let (wd, ht) = (w.allocated_width() as f64, w.allocated_height() as f64);
        let g = gtk::cairo::LinearGradient::new(0.0, 0.0, 0.0, ht);
        g.add_color_stop_rgb(0.0, 0.36, 0.62, 0.90);
        g.add_color_stop_rgb(1.0, 0.10, 0.33, 0.66);
        let r = 3.0;
        cr.new_sub_path();
        cr.arc(wd - r - 0.5, r + 0.5, r, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(wd - r - 0.5, ht - r - 0.5, r, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(r + 0.5, ht - r - 0.5, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(r + 0.5, r + 0.5, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.close_path();
        cr.set_source(&g).ok();
        cr.fill_preserve().ok();
        cr.set_source_rgb(0.08, 0.25, 0.52);
        cr.set_line_width(1.0);
        cr.stroke().ok();
        let t = format!("{:.1}", score);
        let layout = w.create_pango_layout(Some(&t));
        let mut f = gtk::pango::FontDescription::from_string("Segoe UI Bold 9");
        f.set_weight(gtk::pango::Weight::Bold);
        layout.set_font_description(Some(&f));
        let (lw, lh) = layout.pixel_size();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.move_to((wd - lw as f64) / 2.0, (ht - lh as f64) / 2.0);
        pangocairo::functions::show_layout(cr, &layout);
        glib::Propagation::Stop
    });
    d
}

// ------------------------------------------------------------------ System Properties

/// `aerode-control sysprops…` / `visualeffects`: the dialog on its own, like SystemPropertiesAdvanced.exe.
pub fn standalone(app: &gtk::Application, id: &str) -> bool {
    let d = match id {
        "sysprops" => props_dialog(None, 0),
        "sysprops-hardware" => props_dialog(None, 1),
        "sysprops-advanced" => props_dialog(None, 2),
        "sysprops-protection" => props_dialog(None, 3),
        "sysprops-remote" => props_dialog(None, 4),
        "visualeffects" => performance(None),
        _ => return false,
    };
    super::super::admin::css();
    d.set_application(Some(app));
    d.present();
    true
}

/// Show System Properties at a tab, over Control Panel.
pub fn props(cp: Option<&Rc<Cp>>, tab: u32) {
    let d = props_dialog(cp.cloned(), tab);
    d.present();
}

fn props_dialog(cp: Option<Rc<Cp>>, tab: u32) -> gtk::Dialog {
    let parent = cp.as_ref().map(|c| cpwin(c));
    let s = DlgSheet::new(parent.as_ref(), "System Properties", true);
    let parent_of = { let d = s.d.clone(); move || -> gtk::Window { d.clone().upcast() } };

    // ---------------------------------------------------------------- Computer Name
    let t = vbox(10);
    let top = hbox(12);
    top.pack_start(&img(&["computer"], 48), false, false, 0);
    top.pack_start(&wrap("Windows uses the following information to identify your computer on the network.", 42), false, false, 0);
    t.pack_start(&top, false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    let desc = gtk::Entry::new();
    let old_desc = description();
    desc.set_text(&old_desc);
    desc.set_hexpand(true);
    let dl = lbl("Computer _description:");
    dl.set_mnemonic_widget(Some(&desc));
    g.attach(&dl, 0, 0, 1, 1);
    g.attach(&desc, 1, 0, 1, 1);
    let ex = wrap("For example: \"Kitchen Computer\" or \"Mary's Computer\".", 34);
    g.attach(&ex, 1, 1, 1, 1);
    let full = lbl(&hostname());
    let (wg, _) = workgroup();
    let wgl = lbl(&wg);
    g.attach(&lbl("Full computer name:"), 0, 2, 1, 1);
    g.attach(&full, 1, 2, 1, 1);
    g.attach(&lbl("Workgroup:"), 0, 3, 1, 1);
    g.attach(&wgl, 1, 3, 1, 1);
    t.pack_start(&g, false, false, 6);
    let r1 = hbox(10);
    r1.pack_start(&wrap("To use a wizard to join a domain or workgroup, click Network ID.", 40), true, true, 0);
    let netid = gtk::Button::with_mnemonic("Net_work ID...");
    netid.set_valign(gtk::Align::Center);
    netid.set_sensitive(false);
    netid.set_tooltip_text(Some("Joining a domain is not supported. Use Change... to set the workgroup."));
    r1.pack_end(&netid, false, false, 0);
    t.pack_start(&r1, false, false, 0);
    let r2 = hbox(10);
    r2.pack_start(&wrap("To rename this computer or change its domain or workgroup, click Change.", 40), true, true, 0);
    let ch = shield_button("_Change...");
    ch.set_valign(gtk::Align::Center);
    r2.pack_end(&ch, false, false, 0);
    t.pack_start(&r2, false, false, 0);
    { let (full, wgl, p) = (full.clone(), wgl.clone(), parent_of.clone()); ch.connect_clicked(move |_| change_name(&p(), &full, &wgl)); }
    s.watch_entry(&desc);
    s.tab("Computer Name", &t);

    // ---------------------------------------------------------------- Hardware
    let t = vbox(10);
    let dm = hbox(12);
    dm.pack_start(&img(&["computer", "preferences-system"], 32), false, false, 0);
    let col = vbox(8);
    col.pack_start(&wrap("The Device Manager lists all the hardware devices installed on your computer. Use the Device Manager to change the properties of any device.", 44), false, false, 0);
    let dmb = shield_button("_Device Manager");
    dmb.set_halign(gtk::Align::End);
    dmb.connect_clicked(|_| spawn_tool("devmgr"));
    col.pack_start(&dmb, false, false, 0);
    dm.pack_start(&col, true, true, 0);
    t.pack_start(&group("Device Manager", &dm), false, false, 0);
    let di = hbox(12);
    di.pack_start(&img(&["system-software-update", "system-software-install"], 32), false, false, 0);
    let col = vbox(8);
    col.pack_start(&wrap("Choose whether Windows downloads driver software and realistic icons for your devices.", 44), false, false, 0);
    let dib = gtk::Button::with_mnemonic("Device Installation _Settings");
    dib.set_halign(gtk::Align::End);
    { let p = parent_of.clone(); dib.connect_clicked(move |_| device_install_settings(&p())); }
    col.pack_start(&dib, false, false, 0);
    di.pack_start(&col, true, true, 0);
    t.pack_start(&group("Device Installation Settings", &di), false, false, 0);
    s.tab("Hardware", &t);

    // ---------------------------------------------------------------- Advanced
    let t = vbox(10);
    t.pack_start(&wrap("You must be logged on as an Administrator to make most of these changes.", 50), false, false, 0);
    let mk = |title: &str, body: &str, btn: &str| -> (gtk::Frame, gtk::Button) {
        let b = vbox(8);
        b.pack_start(&wrap(body, 50), false, false, 0);
        let bt = gtk::Button::with_mnemonic(btn);
        bt.set_halign(gtk::Align::End);
        bt.set_size_request(85, -1);
        b.pack_start(&bt, false, false, 0);
        (group(title, &b), bt)
    };
    let (f1, perf_b) = mk("Performance", "Visual effects, processor scheduling, memory usage, and virtual memory", "_Settings...");
    let (f2, prof_b) = mk("User Profiles", "Desktop settings related to your logon", "S_ettings...");
    let (f3, start_b) = mk("Startup and Recovery", "System startup, system failure, and debugging information", "Se_ttings...");
    for f in [&f1, &f2, &f3] { t.pack_start(f, false, false, 0); }
    let env_b = gtk::Button::with_mnemonic("E_nvironment Variables...");
    env_b.set_halign(gtk::Align::End);
    t.pack_start(&env_b, false, false, 0);
    { let p = parent_of.clone(); perf_b.connect_clicked(move |_| { performance(Some(&p())).present(); }); }
    { let (p, cp) = (parent_of.clone(), cp.clone()); prof_b.connect_clicked(move |_| user_profiles(&p(), &cp)); }
    { let p = parent_of.clone(); start_b.connect_clicked(move |_| startup_recovery(&p())); }
    { let p = parent_of.clone(); env_b.connect_clicked(move |_| env_vars(&p())); }
    s.tab("Advanced", &t);

    // ---------------------------------------------------------------- System Protection
    let t = vbox(10);
    let top = hbox(12);
    top.pack_start(&img(&["drive-harddisk-system", "drive-harddisk"], 48), false, false, 0);
    let tc = vbox(4);
    tc.pack_start(&wrap("Use system protection to undo unwanted system changes and restore previous versions of files.", 42), false, false, 0);
    tc.pack_start(&link("What is system protection?", || { util::open_uri("https://wiki.archlinux.org/title/System_backup#Snapshots"); }), false, false, 0);
    top.pack_start(&tc, true, true, 0);
    t.pack_start(&top, false, false, 0);
    let sr = hbox(10);
    sr.pack_start(&wrap("You can undo system changes by reverting your computer to a previous restore point.", 34), true, true, 0);
    let srb = gtk::Button::with_mnemonic("_System Restore...");
    srb.set_valign(gtk::Align::Center);
    { let (cp, d) = (cp.clone(), s.d.clone()); srb.connect_clicked(move |_| { d.close(); goto(&cp, "recovery"); }); }
    sr.pack_end(&srb, false, false, 0);
    t.pack_start(&group("System Restore", &sr), false, false, 0);
    let ps = vbox(8);
    let store = gtk::ListStore::new(&[gtk::gdk_pixbuf::Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    super::super::admin::icon_column(&tv, "Available Drives", 0, 1, 220);
    columns(&tv, &[("Protection", 2, 0)]);
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 92);
    ps.pack_start(&sw, false, false, 0);
    let tool = snapshot_tool();
    fill_protection(&store, tool);
    let cfg = hbox(10);
    cfg.pack_start(&wrap("Configure restore settings, manage disk space, and delete restore points.", 34), true, true, 0);
    let cfgb = gtk::Button::with_mnemonic("C_onfigure...");
    cfgb.set_valign(gtk::Align::Center);
    { let (cp, d) = (cp.clone(), s.d.clone()); cfgb.connect_clicked(move |_| {
        // Win7 opens the drive's restore settings: the snapshot tool's own window, else Backup and Restore
        if !(util::spawn(&["timeshift-launcher"]) || util::spawn(&["snapper-gui"]) || util::spawn(&["btrfs-assistant-launcher"])) { d.close(); goto(&cp, "backup"); }
    }); }
    cfg.pack_end(&cfgb, false, false, 0);
    ps.pack_start(&cfg, false, false, 0);
    let cr = hbox(10);
    cr.pack_start(&wrap("Create a restore point right now for the drives that have system protection turned on.", 34), true, true, 0);
    let crb = gtk::Button::with_mnemonic("C_reate...");
    crb.set_valign(gtk::Align::Center);
    crb.set_sensitive(tool.is_some());
    if tool.is_none() { crb.set_tooltip_text(Some("Install Timeshift or Snapper to turn on system protection.")); }
    { let p = parent_of.clone(); crb.connect_clicked(move |_| create_restore_point(&p(), tool)); }
    cr.pack_end(&crb, false, false, 0);
    ps.pack_start(&cr, false, false, 0);
    t.pack_start(&group("Protection Settings", &ps), false, false, 0);
    s.tab("System Protection", &t);

    // ---------------------------------------------------------------- Remote
    let t = vbox(10);
    let ra = vbox(6);
    let has_vnc = util::which("x11vnc");
    let ra_on = check("_Allow Remote Assistance connections to this computer", config::get().hw.remote_assistance && has_vnc);
    ra_on.set_sensitive(has_vnc);
    if !has_vnc { ra_on.set_tooltip_text(Some("Install x11vnc to allow Remote Assistance connections.")); }
    ra.pack_start(&ra_on, false, false, 0);
    let rah = hbox(10);
    rah.pack_start(&link("What happens when I enable Remote Assistance?", || {}), false, false, 0);
    let rab = gtk::Button::with_mnemonic("Ad_vanced...");
    rab.set_sensitive(has_vnc);
    { let p = parent_of.clone(); rab.connect_clicked(move |_| inform(Some(&p()), "Remote Assistance Settings",
        "Remote Assistance shares this desktop over VNC on this computer only (localhost).",
        "The helper connects through an SSH tunnel (ssh -L 5900:localhost:5900 you@this-computer) with the password you set when you turned Remote Assistance on.")); }
    rah.pack_end(&rab, false, false, 0);
    ra.pack_start(&rah, false, false, 0);
    t.pack_start(&group("Remote Assistance", &ra), false, false, 0);
    let rd = vbox(6);
    rd.pack_start(&wrap("Click an option, and then specify who can connect, if needed.", 50), false, false, 0);
    let rr = radios(&["_Don't allow connections to this computer", "Allow connections from computers running any version of Remote _Desktop (less secure)",
        "Allow connections only from computers running Remote Desktop with _Network Level Authentication (more secure)"], 0);
    for r in &rr {
        if let Some(l) = r.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_line_wrap(true); l.set_max_width_chars(46); }
        rd.pack_start(r, false, false, 0);
    }
    let has_xrdp = util::which("xrdp");
    for r in &rr { r.set_sensitive(false); }
    let rdh = hbox(10);
    rdh.pack_start(&link("Help me choose", || {}), false, false, 0);
    let users = gtk::Button::with_mnemonic("_Select Users...");
    users.set_sensitive(false);
    { let p = parent_of.clone(); users.connect_clicked(move |_| inform(Some(&p()), "Remote Desktop Users",
        &format!("{} already has access.", glib::user_name().to_string_lossy()),
        "Every user account on this computer can sign in through Remote Desktop (xrdp).")); }
    rdh.pack_end(&users, false, false, 0);
    rd.pack_start(&rdh, false, false, 0);
    if !has_xrdp { rd.pack_start(&wrap("Install xrdp to allow Remote Desktop connections.", 50), false, false, 0); }
    t.pack_start(&group("Remote Desktop", &rd), false, false, 0);
    // the current xrdp state, asked off the GTK thread
    let rd_was: Rc<std::cell::Cell<u8>> = Rc::new(std::cell::Cell::new(0));
    if has_xrdp {
        let (rr, users, rd_was, d) = (rr.clone(), users.clone(), rd_was.clone(), s.dirty());
        util::output_async(vec!["systemctl".into(), "is-enabled".into(), "xrdp.service".into()], move |o| {
            let on = o.map(|s| s.trim() == "enabled").unwrap_or(false);
            let i = if on { 2 } else { 0 };
            rd_was.set(i);
            rr[i as usize].set_active(true);
            for r in &rr { r.set_sensitive(true); }
            users.set_sensitive(true);
            for r in &rr { let d = d.clone(); r.connect_toggled(move |_| d()); }
        });
    }
    s.watch_check(&ra_on);
    s.tab("Remote", &t);

    // ---------------------------------------------------------------- apply
    {
        let p = parent_of.clone();
        s.on_apply(move || {
            let new_desc = desc.text().to_string();
            if new_desc != description() && new_desc != old_desc {
                elevate(&["hostnamectl", "set-hostname", "--pretty", &new_desc], |ok, out| { report("The computer description could not be changed.", ok, &out); });
            }
            // Remote Assistance: x11vnc for localhost only (helpers come through an SSH tunnel)
            let ra = ra_on.is_active();
            if ra != config::get().hw.remote_assistance {
                if ra && !remote_assistance_on(&p()) { return false; }
                if !ra { remote_assistance_off(); }
                config::update(|c| c.hw.remote_assistance = ra);
            }
            if has_xrdp {
                let want = rr.iter().position(|r| r.is_active()).unwrap_or(0) as u8;
                if (want == 0) != (rd_was.get() == 0) {
                    let verb = if want == 0 { "disable" } else { "enable" };
                    let rd_was = rd_was.clone();
                    elevate(&["systemctl", verb, "--now", "xrdp.service"], move |ok, out| { if report("Remote Desktop settings could not be changed.", ok, &out) { rd_was.set(want); } });
                }
            }
            true
        });
    }
    s.show(tab);
    s.d.clone()
}

/// Win7 "Computer Name/Domain Changes".
fn change_name(parent: &gtk::Window, full: &gtk::Label, wg_label: &gtk::Label) {
    let d = ok_cancel(Some(parent), "Computer Name/Domain Changes");
    let c = d.content_area();
    c.pack_start(&wrap("You can change the name and the membership of this computer. Changes might affect access to network resources.", 46), false, false, 0);
    let name = gtk::Entry::new();
    let old = hostname();
    name.set_text(&old);
    name.set_max_length(63);
    let nl = lbl("_Computer name:");
    nl.set_mnemonic_widget(Some(&name));
    c.pack_start(&nl, false, false, 0);
    c.pack_start(&name, false, false, 0);
    c.pack_start(&lbl("Full computer name:"), false, false, 0);
    let fl = lbl(&old);
    c.pack_start(&fl, false, false, 0);
    { let fl = fl.clone(); name.connect_changed(move |e| fl.set_text(&e.text())); }
    let more = gtk::Button::with_mnemonic("_More...");
    more.set_halign(gtk::Align::End);
    { let (name, d2) = (name.clone(), d.clone()); more.connect_clicked(move |_| {
        let nb: String = name.text().to_uppercase().chars().take(15).collect();
        inform(Some(d2.upcast_ref()), "DNS Suffix and NetBIOS Computer Name", &format!("NetBIOS computer name: {nb}"),
            "This name is used for interoperability with older computers and services (Samba).");
    }); }
    c.pack_start(&more, false, false, 0);
    let m = vbox(4);
    let rs = radios(&["_Domain:", "_Workgroup:"], 1);
    rs[0].set_sensitive(false);
    let dom = gtk::Entry::new();
    dom.set_sensitive(false);
    dom.set_margin_start(20);
    let (wg, has_samba) = workgroup();
    let wge = gtk::Entry::new();
    wge.set_text(&wg);
    wge.set_margin_start(20);
    wge.set_max_length(15);
    m.pack_start(&rs[0], false, false, 0);
    m.pack_start(&dom, false, false, 0);
    m.pack_start(&rs[1], false, false, 0);
    m.pack_start(&wge, false, false, 0);
    c.pack_start(&group("Member of", &m), false, false, 0);
    d.show_all();
    loop {
        if d.run() != gtk::ResponseType::Ok { break; }
        let n = name.text().trim().to_string();
        let w = wge.text().trim().to_uppercase();
        let bad = n.is_empty() || n.starts_with('-') || n.ends_with('-') || !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if bad {
            util::error(&format!("The new computer name \"{n}\" contains characters that are not allowed."),
                "Characters that are not allowed include ` ~ ! @ # $ % ^ & * ( ) = + _ [ ] { } \\ | ; : . ' \" , < > / and ?");
            continue;
        }
        let mut script = vec![];
        if n != old { script.push(format!("hostnamectl set-hostname {}", q(&n))); }
        if w != wg && !w.is_empty() {
            if has_samba { script.push(format!("sed -i 's/^\\([[:space:]]*[wW][oO][rR][kK][gG][rR][oO][uU][pP][[:space:]]*=\\).*/\\1 {}/' /etc/samba/smb.conf", w.replace(['\'', '/', '\\', '&'], ""))); }
            else { inform(Some(d.upcast_ref()), "Computer Name/Domain Changes", "The workgroup is used for Windows file sharing, which is not set up on this computer.", "Install Samba to share files with Windows computers."); }
        }
        if script.is_empty() { break; }
        let (full, wg_label, parent) = (full.clone(), wg_label.clone(), parent.clone());
        elevate_sh(&script.join(" && "), move |ok, out| {
            if report("The computer name could not be changed.", ok, &out) {
                full.set_text(&n);
                if has_samba && !w.is_empty() { wg_label.set_text(&w); }
                inform(Some(&parent), "Computer Name/Domain Changes", "You must restart your computer to apply these changes",
                    "Before restarting, save any open files and close all programs.");
            }
        });
        break;
    }
    d.close();
}

/// "Device Installation Settings": whether drivers and device icons come from the Internet.
fn device_install_settings(parent: &gtk::Window) {
    let d = gtk::Dialog::new();
    d.set_title("Device Installation Settings");
    d.set_transient_for(Some(parent));
    d.set_modal(true);
    d.set_resizable(false);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(8);
    let h = gtk::Label::new(None);
    h.set_markup("<span size='large' foreground='#1e3287'>Do you want Windows to download driver software and realistic icons for your devices?</span>");
    h.set_line_wrap(true);
    h.set_max_width_chars(56);
    h.set_xalign(0.0);
    c.pack_start(&h, false, false, 0);
    let cur = config::get().hw.driver_downloads;
    let top = radios(&["_Yes, do this automatically (recommended)", "_No, let me choose what to do"], if cur == "auto" { 0 } else { 1 });
    c.pack_start(&top[0], false, false, 0);
    c.pack_start(&top[1], false, false, 0);
    let sub = radios(&["_Always install the best driver software from Windows Update.", "_Install driver software from Windows Update if it is not found on my computer.", "Ne_ver install driver software from Windows Update."],
        match cur.as_str() { "never" => 2, "ask" => 1, _ => 0 });
    let subbox = vbox(4);
    subbox.set_margin_start(24);
    for r in &sub { subbox.pack_start(r, false, false, 0); }
    let icons = check("_Replace generic device icons with enhanced icons", true);
    subbox.pack_start(&icons, false, false, 4);
    subbox.set_sensitive(cur != "auto");
    { let sb = subbox.clone(); top[1].connect_toggled(move |r| sb.set_sensitive(r.is_active())); }
    c.pack_start(&subbox, false, false, 0);
    c.pack_start(&link("Why should I have Windows do this automatically?", || {}), false, false, 4);
    d.add_button("Save Changes", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        let v = if top[0].is_active() { "auto" } else if sub[2].is_active() { "never" } else if sub[1].is_active() { "ask" } else { "auto" };
        config::update(|c| c.hw.driver_downloads = v.to_string());
    }
    d.close();
}

// ------------------------------------------------------------------ Performance Options

/// Win7 Performance Options: Visual Effects, Advanced (processor scheduling, virtual memory),
/// Data Execution Prevention.
pub fn performance(parent: Option<&gtk::Window>) -> gtk::Dialog {
    let s = DlgSheet::new(parent, "Performance Options", true);
    let c = config::get();
    let live = live_effects();
    // Visual Effects
    let t = vbox(6);
    t.pack_start(&wrap("Select the settings you want to use for the appearance and performance of Windows on this computer.", 52), false, false, 0);
    let mode = c.hw.visual_effects.clone();
    let rs = radios(&["_Let Windows choose what's best for my computer", "Adjust for _best appearance", "Adjust for best _performance", "_Custom:"],
        match mode.as_str() { "appearance" => 1, "performance" => 2, "custom" => 3, _ => 0 });
    for r in &rs { t.pack_start(r, false, false, 0); }
    let list = vbox(0);
    list.set_border_width(3);
    let checks: Vec<(&'static str, gtk::CheckButton)> = aerode::config::VISUAL_EFFECTS.iter().map(|(k, l)| (*k, check(l, live.iter().find(|x| x.0 == *k).map(|x| x.1).unwrap_or(true)))).collect();
    for (_, cb) in &checks { list.pack_start(cb, false, false, 0); }
    let white = gtk::EventBox::new();
    white.style_context().add_class("view");
    white.add(&list);
    let sw = scrolled(&white);
    sw.set_size_request(360, 250);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    t.pack_start(&sw, true, true, 0);
    let setting = Rc::new(std::cell::Cell::new(false));
    // a preset ticks every box (best appearance) or none (best performance)
    for (i, r) in rs.iter().enumerate().take(3) {
        let (checks, setting) = (checks.clone(), setting.clone());
        r.connect_toggled(move |r| {
            if !r.is_active() { return; }
            setting.set(true);
            for (_, cb) in &checks { cb.set_active(i != 2); }
            setting.set(false);
        });
    }
    for (_, cb) in &checks {
        let (custom, setting, d) = (rs[3].clone(), setting.clone(), s.dirty());
        cb.connect_toggled(move |_| { d(); if !setting.get() { custom.set_active(true); } });
    }
    for r in &rs { s.watch_check(r); }
    s.tab("Visual Effects", &t);

    // Advanced
    let t = vbox(10);
    let ps = vbox(6);
    ps.pack_start(&wrap("Choose how to allocate processor resources.", 50), false, false, 0);
    ps.pack_start(&lbl("Adjust for best performance of:"), false, false, 0);
    let autogroup = std::fs::read_to_string("/proc/sys/kernel/sched_autogroup_enabled").map(|s| s.trim() == "1").unwrap_or(true);
    let sch = radios(&["P_rograms", "Bac_kground services"], if autogroup { 0 } else { 1 });
    let shh = hbox(30);
    for r in &sch { shh.pack_start(r, false, false, 0); s.watch_check(r); }
    ps.pack_start(&shh, false, false, 0);
    t.pack_start(&group("Processor scheduling", &ps), false, false, 0);
    let vm = vbox(6);
    vm.pack_start(&wrap("A paging file is an area on the hard disk that Windows uses as if it were RAM.", 50), false, false, 0);
    let vmh = hbox(10);
    let total = lbl("Total paging file size for all drives:");
    vmh.pack_start(&total, true, true, 0);
    let vmb = gtk::Button::with_mnemonic("_Change...");
    vmh.pack_end(&vmb, false, false, 0);
    vm.pack_start(&vmh, false, false, 0);
    {
        let total = total.clone();
        util::output_async(vec!["swapon".into(), "--show=SIZE".into(), "--bytes".into(), "--noheadings".into()], move |o| {
            let sum: u64 = o.unwrap_or_default().lines().filter_map(|l| l.trim().parse::<u64>().ok()).sum();
            total.set_text(&format!("Total paging file size for all drives:   {} MB", sum / (1 << 20)));
        });
    }
    { let d = s.d.clone(); vmb.connect_clicked(move |_| virtual_memory(d.upcast_ref())); }
    t.pack_start(&group("Virtual memory", &vm), false, false, 0);
    s.tab("Advanced", &t);

    // Data Execution Prevention
    let t = vbox(10);
    let top = hbox(12);
    top.pack_start(&img(&["security-high"], 32), false, false, 0);
    top.pack_start(&wrap("Data Execution Prevention (DEP) helps protect against damage from viruses and other security threats. How does it work?", 44), false, false, 0);
    t.pack_start(&top, false, false, 0);
    let dep = radios(&["_Turn on DEP for essential Windows programs and services only", "T_urn on DEP for all programs and services except those I select:"], 1);
    for r in &dep { r.set_sensitive(false); t.pack_start(r, false, false, 0); }
    let ex = gtk::TreeView::new();
    let exs = scrolled(&ex);
    exs.set_size_request(-1, 110);
    exs.set_sensitive(false);
    t.pack_start(&exs, false, false, 0);
    let nx = std::fs::read_to_string("/proc/cpuinfo").map(|s| s.lines().any(|l| l.starts_with("flags") && l.split_whitespace().any(|f| f == "nx"))).unwrap_or(true);
    t.pack_start(&wrap(if nx { "Your computer's processor supports hardware-based DEP. Linux enforces it for every program (NX)." } else { "Your computer's processor does not support hardware-based DEP." }, 52), false, false, 0);
    s.tab("Data Execution Prevention", &t);

    let parent_w: gtk::Window = s.d.clone().upcast();
    s.on_apply(move || {
        let mode = ["auto", "appearance", "performance", "custom"][rs.iter().position(|r| r.is_active()).unwrap_or(0)];
        let on: Vec<(&str, bool)> = checks.iter().map(|(k, c)| (*k, c.is_active())).collect();
        apply_effects(mode, &on, &live);
        let want_auto = sch[0].is_active();
        if want_auto != autogroup {
            let v = want_auto as u8;
            let script = format!("sysctl -w kernel.sched_autogroup_enabled={v} && mkdir -p /etc/sysctl.d && echo 'kernel.sched_autogroup_enabled = {v}' > /etc/sysctl.d/90-aerode-scheduling.conf");
            if confirm(Some(&parent_w), "Performance Options", "Change how processor time is shared?",
                if want_auto { "Programs you use get priority over background services (the kernel groups processes by session)." } else { "Background services get the same share of processor time as the programs you use." }, "Change") {
                elevate_sh(&script, |ok, out| { report("Processor scheduling could not be changed.", ok, &out); });
                config::update(|c| c.hw.scheduling = if want_auto { "programs".into() } else { "services".into() });
            }
        }
        true
    });
    s.show(0);
    s.d.clone()
}

/// Effects that live in other settings (composition, glass, Aero Peek, font smoothing, shadows):
/// their current state; the rest come from config.
fn live_effects() -> Vec<(&'static str, bool)> {
    let c = config::get();
    aerode::config::VISUAL_EFFECTS.iter().map(|(k, _)| (*k, match *k {
        "composition" => !config::conf_dir().join("no-compositor").exists(),
        "glass" => c.colorization.transparency,
        "aero_peek" => c.taskbar.aero_peek,
        "font_smoothing" => c.fonts.antialias,
        "window_shadow" => !picom_conf().contains("shadow = false;"),
        "pointer_shadow" => c.hw.mouse.cursor_shadow && c.hw.effect("pointer_shadow"),
        k => c.hw.effect(k),
    })).collect()
}

fn picom_conf() -> String {
    std::fs::read_to_string(config::conf_dir().join("picom.conf")).or_else(|_| std::fs::read_to_string("/usr/share/aerode/picom.conf")).unwrap_or_default()
}

fn apply_effects(mode: &str, on: &[(&str, bool)], was: &[(&'static str, bool)]) {
    let get = |k: &str| on.iter().find(|x| x.0 == k).map(|x| x.1).unwrap_or(true);
    let changed = |k: &str| get(k) != was.iter().find(|x| x.0 == k).map(|x| x.1).unwrap_or(true);
    config::update(|c| {
        c.hw.visual_effects = mode.to_string();
        c.hw.effects_off = on.iter().filter(|x| !x.1).map(|x| x.0.to_string()).collect();
        c.colorization.transparency = get("glass");
        c.taskbar.aero_peek = get("aero_peek");
        c.fonts.antialias = get("font_smoothing");
        c.hw.mouse.cursor_shadow = get("pointer_shadow");
    });
    let mut restart = false;
    if changed("composition") {
        let f = config::conf_dir().join("no-compositor");
        let _ = std::fs::create_dir_all(config::conf_dir());
        if get("composition") { let _ = std::fs::remove_file(&f); } else { let _ = std::fs::write(&f, ""); }
        restart = true;
    }
    if changed("window_shadow") {
        let base = std::fs::read_to_string("/usr/share/aerode/picom.conf").ok().or_else(|| std::fs::read_to_string(config::conf_dir().join("picom.conf")).ok());
        if let Some(base) = base {
            let vsync_off = picom_conf().contains("vsync = false;");
            let mut txt = base.replacen("shadow = true;", if get("window_shadow") { "shadow = true;" } else { "shadow = false;" }, 1);
            if vsync_off { txt = txt.replace("vsync = true;", "vsync = false;"); }
            let _ = std::fs::write(config::conf_dir().join("picom.conf"), txt);
            restart = true;
        }
    }
    if changed("font_smoothing") || ["animate_controls", "menu_fade", "tooltip_fade", "combo_slide", "list_smooth"].iter().any(|k| changed(k)) { write_xsettings(); }
    if restart { util::spawn(&["aerode", "restart"]); } else { reload_shell(); }
}

/// Win7 "Virtual Memory" for Linux swap: read-only (changing swap is left to the system tools).
fn virtual_memory(parent: &gtk::Window) {
    let d = ok_cancel(Some(parent), "Virtual Memory");
    let c = d.content_area();
    let auto = check("_Automatically manage paging file size for all drives", true);
    auto.set_sensitive(false);
    c.pack_start(&auto, false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Drive [Volume Label]", 0, 200), ("Paging File Size (MB)", 1, 0)]);
    let sw = scrolled(&tv);
    sw.set_size_request(340, 110);
    let inner = vbox(6);
    inner.pack_start(&sw, false, false, 0);
    let summary = lbl("");
    inner.pack_start(&summary, false, false, 0);
    c.pack_start(&group("Paging file size for each drive", &inner), false, false, 0);
    let swp = lbl("");
    c.pack_start(&group("Total paging file size for all drives", &swp), false, false, 0);
    {
        let (store, swp) = (store.clone(), swp.clone());
        util::output_async(vec!["swapon".into(), "--show=NAME,TYPE,SIZE".into(), "--bytes".into(), "--noheadings".into(), "--raw".into()], move |o| {
            let mut sum = 0u64;
            for l in o.unwrap_or_default().lines() {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() < 3 { continue; }
                let size = f[2].parse::<u64>().unwrap_or(0);
                sum += size;
                let name = if f[0].contains("zram") { format!("{} [compressed RAM]", f[0]) } else { format!("{} [{}]", f[0], f[1]) };
                store.insert_with_values(None, &[(0, &name), (1, &format!("{}", size >> 20))]);
            }
            let swappiness = std::fs::read_to_string("/proc/sys/vm/swappiness").unwrap_or_default();
            swp.set_text(&format!("Currently allocated:   {} MB\nSwappiness:   {}", sum >> 20, swappiness.trim()));
        });
    }
    summary.set_text("Swap is set up by the system (fstab, zram-generator).");
    d.show_all();
    d.run();
    d.close();
}

// ------------------------------------------------------------------ User Profiles

fn user_profiles(parent: &gtk::Window, cp: &Option<Rc<Cp>>) {
    let d = ok_cancel(Some(parent), "User Profiles");
    let c = d.content_area();
    let top = hbox(12);
    top.pack_start(&img(&["system-users", "user-home"], 32), false, false, 0);
    top.pack_start(&wrap("User profiles contain settings for your desktop and other information related to your user account. A different profile can be created on each computer you use, or you can select a roaming profile that is the same on every computer you use.", 52), false, false, 0);
    c.pack_start(&top, false, false, 0);
    c.pack_start(&lbl("Profiles stored on this computer:"), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Name", 0, 150), ("Size", 1, 70), ("Type", 2, 50), ("Status", 3, 50), ("Modified", 4, 0)]);
    let sw = scrolled(&tv);
    sw.set_size_request(420, 140);
    c.pack_start(&sw, false, false, 0);
    let bb = hbox(6);
    bb.set_halign(gtk::Align::End);
    for l in ["Change Type", "Delete", "Copy To"] { let b = gtk::Button::with_label(l); b.set_sensitive(false); bb.pack_start(&b, false, false, 0); }
    c.pack_start(&bb, false, false, 0);
    let me = cp.clone();
    let d2 = d.clone();
    c.pack_start(&link("To create new user accounts, open User Accounts in Control Panel.", move || { d2.close(); goto(&me, "users"); }), false, false, 0);
    // home folders of the regular accounts (uid ≥ 1000); the sizes take a while (du)
    let pw = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let me_name = glib::user_name().to_string_lossy().into_owned();
    let homes: Vec<(String, String)> = pw.lines().filter_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        let uid: u32 = f.get(2)?.parse().ok()?;
        (uid >= 1000 && uid < 60000 && !f.get(6)?.ends_with("nologin")).then(|| (f[0].to_string(), f[5].to_string()))
    }).collect();
    for (user, home) in homes {
        let modified = std::fs::metadata(&home).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| super::super::admin::day(d.as_secs() as i64)).unwrap_or_default();
        let it = store.insert_with_values(None, &[(0, &format!("{}\\{user}", hostname().to_uppercase())), (1, &"…"), (2, &"Local"), (3, &"Local"), (4, &modified)]);
        if user == me_name {
            let (store, home) = (store.clone(), home.clone());
            bg(move || super::super::admin::sh(&["du", "-sb", "--one-file-system", &home]), move |o| {
                let b: u64 = o.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0);
                if store.iter_is_valid(&it) { store.set(&it, &[(1, &super::super::admin::gb(b))]); }
            });
        } else {
            store.set(&it, &[(1, &"")]);
        }
    }
    d.show_all();
    d.run();
    d.close();
}

// ------------------------------------------------------------------ Startup and Recovery

/// The boot loader in use: ("systemd-boot" | "grub" | "", its timeout in seconds).
fn boot_loader() -> (&'static str, Option<u32>) {
    if let Ok(s) = std::fs::read_to_string("/etc/default/grub") {
        let t = s.lines().find_map(|l| l.trim().strip_prefix("GRUB_TIMEOUT=")).and_then(|v| v.trim_matches(['"', '\'']).parse().ok());
        if std::path::Path::new("/boot/grub/grub.cfg").exists() || !std::path::Path::new("/boot/loader").exists() { return ("grub", t); }
    }
    for p in ["/boot/loader/loader.conf", "/efi/loader/loader.conf", "/boot/efi/loader/loader.conf"] {
        if let Ok(s) = std::fs::read_to_string(p) {
            return ("systemd-boot", s.lines().find_map(|l| l.trim().strip_prefix("timeout")).and_then(|v| v.trim().parse().ok()));
        }
        if std::path::Path::new(p).parent().map(|d| d.exists()).unwrap_or(false) { return ("systemd-boot", None); }
    }
    ("", None)
}

fn startup_recovery(parent: &gtk::Window) {
    let d = ok_cancel(Some(parent), "Startup and Recovery");
    let c = d.content_area();
    let os = os_release();
    let (loader, timeout) = boot_loader();
    let st = vbox(6);
    st.pack_start(&lbl("Default operating system:"), false, false, 0);
    let def = gtk::ComboBoxText::new();
    def.append_text(os.get("PRETTY_NAME").map(|s| s.as_str()).unwrap_or("Linux"));
    def.set_active(Some(0));
    def.set_sensitive(false);
    st.pack_start(&def, false, false, 0);
    let row = hbox(6);
    let show_list = check("_Time to display list of operating systems:", timeout.map(|t| t > 0).unwrap_or(true));
    let secs = gtk::SpinButton::with_range(0.0, 999.0, 1.0);
    secs.set_value(timeout.unwrap_or(5) as f64);
    row.pack_start(&show_list, false, false, 0);
    row.pack_start(&secs, false, false, 0);
    row.pack_start(&lbl("seconds"), false, false, 0);
    st.pack_start(&row, false, false, 0);
    let sens = loader != "";
    show_list.set_sensitive(sens);
    secs.set_sensitive(sens && show_list.is_active());
    { let s2 = secs.clone(); show_list.connect_toggled(move |c| s2.set_sensitive(c.is_active())); }
    let ed = hbox(6);
    ed.pack_start(&wrap(&format!("To edit the boot options file manually, click Edit. ({})", if loader.is_empty() { "boot loader not found" } else { loader }), 40), true, true, 0);
    let edit = gtk::Button::with_mnemonic("_Edit");
    edit.set_sensitive(loader == "grub");
    edit.connect_clicked(|_| { if !util::spawn(&["aerode-notepad", "/etc/default/grub"]) { util::open_path(std::path::Path::new("/etc/default/grub")); } });
    ed.pack_end(&edit, false, false, 0);
    st.pack_start(&ed, false, false, 0);
    c.pack_start(&group("System startup", &st), false, false, 0);
    let sf = vbox(6);
    let panic = std::fs::read_to_string("/proc/sys/kernel/panic").ok().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    let log = check("_Write an event to the system log", true);
    log.set_sensitive(false);
    let reboot = check("Automatically _restart", panic != 0);
    reboot.set_sensitive(false);
    reboot.set_tooltip_text(Some("kernel.panic: restart this many seconds after a kernel panic (0 = never)"));
    sf.pack_start(&log, false, false, 0);
    sf.pack_start(&reboot, false, false, 0);
    sf.pack_start(&lbl("Write debugging information"), false, false, 0);
    let coredump = gtk::ComboBoxText::new();
    coredump.append_text(if util::which("coredumpctl") { "Program core dumps (systemd-coredump)" } else { "(none)" });
    coredump.set_active(Some(0));
    coredump.set_sensitive(false);
    sf.pack_start(&coredump, false, false, 0);
    sf.pack_start(&lbl("Dump file:"), false, false, 0);
    let df = gtk::Entry::new();
    df.set_text("/var/lib/systemd/coredump");
    df.set_sensitive(false);
    sf.pack_start(&df, false, false, 0);
    c.pack_start(&group("System failure", &sf), false, false, 0);
    d.show_all();
    if d.run() == gtk::ResponseType::Ok && sens {
        let want = if show_list.is_active() { secs.value() as u32 } else { 0 };
        if Some(want) != timeout {
            let script = match loader {
                "grub" => format!("sed -i 's/^GRUB_TIMEOUT=.*/GRUB_TIMEOUT={want}/' /etc/default/grub && grub-mkconfig -o /boot/grub/grub.cfg"),
                _ => format!("bootctl set-timeout {want}"),
            };
            if confirm(Some(d.upcast_ref()), "Startup and Recovery", "Change how long the list of operating systems is shown?", &format!("The boot loader ({loader}) will wait {want} seconds."), "Change") {
                elevate_sh(&script, |ok, out| { report("The startup settings could not be changed.", ok, &out); });
            }
        }
    }
    d.close();
}

// ------------------------------------------------------------------ Environment Variables

const USER_ENV: &str = "90-aerode.conf";

fn env_dir() -> std::path::PathBuf { glib::user_config_dir().join("environment.d") }

/// KEY=VALUE lines of an environment.d style file.
fn parse_env(s: &str) -> Vec<(String, String)> {
    s.lines().filter_map(|l| {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') { return None; }
        let (k, v) = t.split_once('=')?;
        let k = k.trim().trim_start_matches("export ").trim();
        (!k.is_empty()).then(|| (k.to_string(), v.trim().trim_matches('"').to_string()))
    }).collect()
}

/// The user's variables: (name, value, file it comes from).
fn user_env() -> Vec<(String, String, String)> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(env_dir()).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "conf").unwrap_or(false)).collect()).unwrap_or_default();
    files.sort();
    let mut out: Vec<(String, String, String)> = vec![];
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        for (k, v) in parse_env(&std::fs::read_to_string(&f).unwrap_or_default()) {
            // a later file overrides an earlier one
            out.retain(|x| x.0 != k);
            out.push((k, v, name.clone()));
        }
    }
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

fn env_dialog(parent: &gtk::Window, title: &str, name: &str, value: &str) -> Option<(String, String)> {
    let d = ok_cancel(Some(parent), title);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(10);
    let n = gtk::Entry::new();
    n.set_text(name);
    n.set_width_chars(36);
    let v = gtk::Entry::new();
    v.set_text(value);
    v.set_activates_default(true);
    let nl = lbl("Variable _name:");
    nl.set_mnemonic_widget(Some(&n));
    let vl = lbl("Variable _value:");
    vl.set_mnemonic_widget(Some(&v));
    g.attach(&nl, 0, 0, 1, 1);
    g.attach(&n, 1, 0, 1, 1);
    g.attach(&vl, 0, 1, 1, 1);
    g.attach(&v, 1, 1, 1, 1);
    d.content_area().pack_start(&g, false, false, 0);
    d.show_all();
    let r = d.run();
    let out = (n.text().trim().to_string(), v.text().to_string());
    d.close();
    let ok = !out.0.is_empty() && out.0.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !out.0.chars().next().unwrap().is_ascii_digit();
    if r == gtk::ResponseType::Ok && !ok { util::error("The variable name is not valid.", "Use letters, digits and underscores, not starting with a digit."); return None; }
    (r == gtk::ResponseType::Ok).then_some(out)
}

fn env_vars(parent: &gtk::Window) {
    let d = ok_cancel(Some(parent), "Environment Variables");
    let c = d.content_area();
    let user = glib::user_name().to_string_lossy().into_owned();
    // working copies; OK writes them
    let uvars: Rc<RefCell<Vec<(String, String, String)>>> = Rc::new(RefCell::new(user_env()));
    let svars: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(parse_env(&std::fs::read_to_string("/etc/environment").unwrap_or_default())));
    let (svars_was, uvars_was) = (svars.borrow().clone(), uvars.borrow().clone());
    let mk = |caption: &str| -> (gtk::Box, gtk::ListStore, gtk::TreeView, [gtk::Button; 3]) {
        let v = vbox(6);
        let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
        let tv = gtk::TreeView::with_model(&store);
        columns(&tv, &[("Variable", 0, 140), ("Value", 1, 0)]);
        let sw = scrolled(&tv);
        sw.set_size_request(400, 130);
        v.pack_start(&sw, false, false, 0);
        let bb = hbox(6);
        bb.set_halign(gtk::Align::End);
        let bs = [gtk::Button::with_mnemonic("_New..."), gtk::Button::with_mnemonic("_Edit..."), gtk::Button::with_mnemonic("_Delete")];
        for b in &bs { b.set_size_request(75, -1); bb.pack_start(b, false, false, 0); }
        v.pack_start(&bb, false, false, 0);
        let _ = caption;
        (v, store, tv, bs)
    };
    let (ub, ustore, utv, ubtn) = mk("user");
    let (sb, sstore, stv, sbtn) = mk("system");
    c.pack_start(&group(&format!("User variables for {user}"), &ub), false, false, 0);
    c.pack_start(&group("System variables", &sb), false, false, 0);
    let fill_u = { let (s, u) = (ustore.clone(), uvars.clone()); Rc::new(move || { s.clear(); for (k, v, _) in u.borrow().iter() { s.insert_with_values(None, &[(0, k), (1, v)]); } }) };
    let fill_s = { let (s, u) = (sstore.clone(), svars.clone()); Rc::new(move || { s.clear(); for (k, v) in u.borrow().iter() { s.insert_with_values(None, &[(0, k), (1, v)]); } }) };
    fill_u();
    fill_s();
    let sel = |tv: &gtk::TreeView| tv.selection().selected().map(|(m, it)| sval(&m, &it, 0));
    let dw: gtk::Window = d.clone().upcast();
    // user: New / Edit / Delete
    { let (u, f, w) = (uvars.clone(), fill_u.clone(), dw.clone()); ubtn[0].connect_clicked(move |_| {
        if let Some((k, v)) = env_dialog(&w, "New User Variable", "", "") { u.borrow_mut().retain(|x| x.0 != k); u.borrow_mut().push((k, v, USER_ENV.into())); f(); }
    }); }
    let edit_u = { let (u, f, w, tv) = (uvars.clone(), fill_u.clone(), dw.clone(), utv.clone()); Rc::new(move || {
        let Some(k) = sel(&tv) else { return };
        let cur = u.borrow().iter().find(|x| x.0 == k).cloned();
        if let Some((_, v, file)) = cur {
            if let Some((nk, nv)) = env_dialog(&w, "Edit User Variable", &k, &v) {
                let mut l = u.borrow_mut();
                l.retain(|x| x.0 != k && x.0 != nk);
                l.push((nk, nv, if file.is_empty() { USER_ENV.into() } else { file }));
            }
            f();
        }
    }) };
    { let e = edit_u.clone(); ubtn[1].connect_clicked(move |_| e()); }
    { let e = edit_u.clone(); utv.connect_row_activated(move |_, _, _| e()); }
    { let (u, f, tv) = (uvars.clone(), fill_u.clone(), utv.clone()); ubtn[2].connect_clicked(move |_| { if let Some(k) = sel(&tv) { u.borrow_mut().retain(|x| x.0 != k); f(); } }); }
    // system: same on /etc/environment (written through pkexec on OK)
    { let (u, f, w) = (svars.clone(), fill_s.clone(), dw.clone()); sbtn[0].connect_clicked(move |_| {
        if let Some((k, v)) = env_dialog(&w, "New System Variable", "", "") { u.borrow_mut().retain(|x| x.0 != k); u.borrow_mut().push((k, v)); f(); }
    }); }
    let edit_s = { let (u, f, w, tv) = (svars.clone(), fill_s.clone(), dw.clone(), stv.clone()); Rc::new(move || {
        let Some(k) = sel(&tv) else { return };
        let cur = u.borrow().iter().find(|x| x.0 == k).cloned();
        if let Some((_, v)) = cur {
            if let Some((nk, nv)) = env_dialog(&w, "Edit System Variable", &k, &v) {
                let mut l = u.borrow_mut();
                let pos = l.iter().position(|x| x.0 == k).unwrap_or(l.len());
                l.retain(|x| x.0 != k && x.0 != nk);
                let n = l.len();
                l.insert(pos.min(n), (nk, nv));
            }
            f();
        }
    }) };
    { let e = edit_s.clone(); sbtn[1].connect_clicked(move |_| e()); }
    { let e = edit_s.clone(); stv.connect_row_activated(move |_, _, _| e()); }
    { let (u, f, tv) = (svars.clone(), fill_s.clone(), stv.clone()); sbtn[2].connect_clicked(move |_| { if let Some(k) = sel(&tv) { u.borrow_mut().retain(|x| x.0 != k); f(); } }); }
    for b in &sbtn { shield(b); }
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        // user files: rewrite each file whose variables changed, keeping comments
        let now = uvars.borrow().clone();
        if now != uvars_was {
            let _ = std::fs::create_dir_all(env_dir());
            let mut files: Vec<String> = now.iter().map(|x| x.2.clone()).chain(uvars_was.iter().map(|x| x.2.clone())).collect();
            files.sort();
            files.dedup();
            for f in files {
                let path = env_dir().join(&f);
                let old = std::fs::read_to_string(&path).unwrap_or_default();
                let mine: Vec<&(String, String, String)> = now.iter().filter(|x| x.2 == f).collect();
                let mut out: Vec<String> = old.lines().filter(|l| { let t = l.trim(); t.is_empty() || t.starts_with('#') }).map(String::from).collect();
                if f == USER_ENV && out.is_empty() { out.push("# written by aerode (System Properties > Environment Variables)".into()); }
                for (k, v, _) in mine { out.push(format!("{k}={v}")); }
                let _ = std::fs::write(&path, out.join("\n") + "\n");
            }
        }
        let snow = svars.borrow().clone();
        if snow != svars_was {
            let body: String = snow.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
            let script = format!("printf '%s' {} > /etc/environment", q(&format!("#\n# This file is parsed by pam_env module\n#\n# Syntax: simple \"KEY=VAL\" pairs on separate lines\n#\n{body}")));
            elevate_sh(&script, |ok, out| { report("The system variables could not be saved.", ok, &out); });
        }
    }
    d.close();
}

// ------------------------------------------------------------------ System Protection

/// The snapshot tool that provides restore points.
fn snapshot_tool() -> Option<&'static str> {
    if util::which("snapper") && std::path::Path::new("/etc/snapper/configs").read_dir().map(|mut r| r.next().is_some()).unwrap_or(false) { return Some("snapper"); }
    if util::which("timeshift") { return Some("timeshift"); }
    None
}

fn fill_protection(store: &gtk::ListStore, tool: Option<&'static str>) {
    let store = store.clone();
    util::output_async(vec!["findmnt".into(), "-rno".into(), "TARGET,FSTYPE".into(), "-t".into(), "ext4,btrfs,xfs,f2fs,ext3,jfs".into()], move |o| {
        // snapper protects the subvolumes it has a config for; Timeshift the root file system
        let snapper_vols: Vec<String> = std::fs::read_dir("/etc/snapper/configs").map(|rd| rd.flatten().filter_map(|e| {
            let s = std::fs::read_to_string(e.path()).ok()?;
            s.lines().find_map(|l| l.strip_prefix("SUBVOLUME=")).map(|v| v.trim_matches('"').to_string())
        }).collect()).unwrap_or_default();
        let ts = std::path::Path::new("/etc/timeshift/timeshift.json").exists() || std::path::Path::new("/etc/timeshift.json").exists();
        let pb = util::pixbuf(util::first_icon(&["drive-harddisk-system", "drive-harddisk"]), 16);
        let mut seen = std::collections::HashSet::new();
        for l in o.unwrap_or_default().lines() {
            let mut f = l.split_whitespace();
            let (Some(target), Some(_fs)) = (f.next(), f.next()) else { continue };
            let target = target.replace("\\x20", " ");
            if target.starts_with("/boot") || target.starts_with("/efi") || target.starts_with("/var/lib/docker") || target.contains("/.snapshots") || !seen.insert(target.clone()) { continue; }
            let on = match tool { Some("snapper") => snapper_vols.contains(&target), Some("timeshift") => ts && target == "/", _ => false };
            let name = if target == "/" { "/ (System)".to_string() } else { target.clone() };
            store.insert_with_values(None, &[(0, &pb), (1, &name), (2, &if on { "On" } else { "Off" })]);
        }
    });
}

/// Win7 "Create a restore point": a description, then the snapshot through pkexec.
fn create_restore_point(parent: &gtk::Window, tool: Option<&'static str>) {
    let Some(tool) = tool else { return };
    let d = gtk::Dialog::new();
    d.set_title("System Protection");
    d.set_transient_for(Some(parent));
    d.set_modal(true);
    d.set_resizable(false);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(8);
    let h = gtk::Label::new(None);
    h.set_markup("<span size='large' foreground='#1e3287'>Create a restore point</span>");
    h.set_xalign(0.0);
    c.pack_start(&h, false, false, 0);
    c.pack_start(&wrap("Type a description to help you identify the restore point. The current date and time are added automatically.", 48), false, false, 0);
    let e = gtk::Entry::new();
    e.set_activates_default(true);
    c.pack_start(&e, false, false, 0);
    let create = d.add_button("C_reate", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.set_default_response(gtk::ResponseType::Ok);
    create.set_sensitive(false);
    { let b = create.clone(); e.connect_changed(move |e| b.set_sensitive(!e.text().trim().is_empty())); }
    d.show_all();
    let r = d.run();
    let desc = e.text().trim().to_string();
    d.close();
    if r != gtk::ResponseType::Ok || desc.is_empty() { return; }
    let argv: Vec<&str> = if tool == "snapper" { vec!["snapper", "-c", "root", "create", "-d", &desc] } else { vec!["timeshift", "--create", "--comments", &desc] };
    let busy = gtk::MessageDialog::new(Some(parent), gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::None, "Creating a restore point...");
    busy.set_title("System Protection");
    busy.show_all();
    let p = parent.clone();
    elevate(&argv, move |ok, out| {
        busy.close();
        if report("The restore point could not be created.", ok, &out) {
            inform(Some(&p), "System Protection", "The restore point was created successfully.", "");
        }
    });
}

// ------------------------------------------------------------------ Remote Assistance

fn ra_autostart() -> std::path::PathBuf { glib::user_config_dir().join("autostart").join("aerode-remote-assistance.desktop") }

/// Turn Remote Assistance on: a VNC password (asked once), x11vnc for localhost now and at logon.
fn remote_assistance_on(parent: &gtk::Window) -> bool {
    let pass = glib::home_dir().join(".vnc").join("passwd");
    if !pass.exists() {
        let d = ok_cancel(Some(parent), "Remote Assistance");
        let c = d.content_area();
        c.pack_start(&wrap("Choose the password your helper types to connect to this computer.", 44), false, false, 0);
        let e = gtk::Entry::new();
        e.set_visibility(false);
        e.set_activates_default(true);
        c.pack_start(&e, false, false, 0);
        d.show_all();
        let r = d.run();
        let pw = e.text().to_string();
        d.close();
        if r != gtk::ResponseType::Ok || pw.is_empty() { return false; }
        let _ = std::fs::create_dir_all(pass.parent().unwrap());
        let ok = std::process::Command::new("x11vnc").args(["-storepasswd", &pw, &pass.to_string_lossy()]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        if !ok { util::error("The password could not be saved.", "x11vnc -storepasswd failed."); return false; }
    }
    let exec = format!("x11vnc -localhost -forever -shared -quiet -rfbauth {}", pass.to_string_lossy());
    let _ = std::fs::create_dir_all(ra_autostart().parent().unwrap());
    let _ = std::fs::write(ra_autostart(), format!("[Desktop Entry]\nType=Application\nName=Remote Assistance\nExec={exec}\nOnlyShowIn=Aerode;\nNoDisplay=true\n"));
    let argv: Vec<&str> = exec.split_whitespace().collect();
    util::spawn(&argv);
    true
}

fn remote_assistance_off() {
    let _ = std::fs::remove_file(ra_autostart());
    // stop only our own x11vnc (started with -localhost from this account)
    let me = unsafe { libc::getuid() };
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let owner = std::fs::metadata(format!("/proc/{pid}")).map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(0);
            if owner == me && cmd.starts_with(b"x11vnc\0") && cmd.windows(10).any(|w| w == b"-localhost") { unsafe { libc::kill(pid, libc::SIGTERM); } }
        }
    }
}
