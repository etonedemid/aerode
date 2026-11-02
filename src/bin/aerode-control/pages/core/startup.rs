//! Startup Programs = System Configuration (msconfig): General, Services, Startup and Tools tabs.
//! Startup items are the XDG autostart entries the shell starts (turning one off writes a
//! `Hidden=true` copy to ~/.config/autostart, like msconfig keeps the date it was disabled);
//! the Services tab enables/disables systemd units at boot. Services = the services.msc console
//! (its own MMC window): Extended/Standard views, Start/Stop/Restart, Properties with the
//! startup type, log on account, recovery and dependencies. Every system change goes through pkexec.
use super::*;
use std::collections::HashMap;

// ------------------------------------------------------------------------------ data

/// Owning package of the files under the given prefixes (pacman's local file lists), worker thread.
fn owners(prefixes: &[&str]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let Ok(rd) = std::fs::read_dir("/var/lib/pacman/local") else { return m };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let parts: Vec<&str> = n.rsplitn(3, '-').collect();
        if parts.len() < 3 { continue; }
        let Ok(files) = std::fs::read_to_string(e.path().join("files")) else { continue };
        for f in files.lines().filter(|l| prefixes.iter().any(|p| l.starts_with(p))) { m.insert(format!("/{f}"), parts[2].to_string()); }
    }
    m
}

/// pkg → its publisher ("Arch Linux", "EndeavourOS", "AUR"…).
fn publishers() -> HashMap<String, String> {
    let mut repo = HashMap::new();
    for l in sh(&["pacman", "-Sl"]).lines() {
        let mut it = l.split_whitespace();
        if let (Some(r), Some(n)) = (it.next(), it.next()) { repo.entry(n.to_string()).or_insert(r.to_string()); }
    }
    repo.into_iter().map(|(n, r)| (n, match r.as_str() {
        "core" | "extra" | "multilib" | "core-testing" | "extra-testing" => "Arch Linux".to_string(),
        "endeavouros" => "EndeavourOS".to_string(),
        r => r.to_string(),
    })).collect()
}

fn manufacturer(path: &str, own: &HashMap<String, String>, publ: &HashMap<String, String>) -> String {
    match own.get(path) { Some(pkg) => publ.get(pkg).cloned().unwrap_or_else(|| "AUR".into()), None => "Unknown".into() }
}

/// msconfig's "Date Disabled" for services (kept by us; systemd does not record it).
fn dates_file() -> std::path::PathBuf { super::super::admin::cache_dir().join("msconfig-disabled") }
fn disabled_dates() -> HashMap<String, i64> {
    std::fs::read_to_string(dates_file()).unwrap_or_default().lines().filter_map(|l| { let (u, t) = l.split_once('\t')?; Some((u.to_string(), t.parse().ok()?)) }).collect()
}
fn save_dates(d: &HashMap<String, i64>) {
    let mut v: Vec<_> = d.iter().map(|(u, t)| format!("{u}\t{t}")).collect();
    v.sort();
    let _ = std::fs::write(dates_file(), v.join("\n") + "\n");
}

#[derive(Clone, Debug, Default)]
pub struct Item { pub file: String, pub name: String, pub command: String, pub location: String, pub path: String, pub on: bool, pub disabled_at: i64, pub maker: String }

/// The autostart entries that start in an Aerode session.
fn startup_items() -> Vec<Item> {
    let user = glib::user_config_dir().join("autostart");
    let mut seen: std::collections::BTreeMap<String, std::path::PathBuf> = std::collections::BTreeMap::new();
    for d in glib::system_config_dirs().iter().rev().map(|d| d.join("autostart")).chain(std::iter::once(user.clone())) {
        if let Ok(rd) = std::fs::read_dir(&d) { for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".desktop") { seen.insert(n, e.path()); }
        } }
    }
    let mut v = vec![];
    for (file, path) in seen {
        let Some(app) = gio::DesktopAppInfo::from_filename(&path) else { continue };
        let exe = app.executable().file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if matches!(exe.as_str(), "picom" | "compton" | "xcompmgr" | "compiz") || !app.shows_in(Some("Aerode")) { continue; }
        let on = !app.is_hidden() && !(app.has_key("X-GNOME-Autostart-enabled") && !app.boolean("X-GNOME-Autostart-enabled"));
        let disabled_at = app.string("X-Aerode-Date-Disabled").and_then(|s| s.parse().ok()).unwrap_or(0);
        // the system file it overrides, for its location and maker
        let system = glib::system_config_dirs().iter().map(|d| d.join("autostart").join(&file)).find(|p| p.exists());
        let shown = system.clone().unwrap_or_else(|| path.clone());
        v.push(Item {
            file, name: app.display_name().to_string(), command: app.commandline().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default(),
            location: shown.parent().map(|p| p.to_string_lossy().replace(&config::home().to_string_lossy().to_string(), "~")).unwrap_or_default(),
            path: shown.to_string_lossy().into_owned(), on, disabled_at,
            maker: String::new(),
        });
    }
    v
}

/// Turn an autostart entry on or off (a copy in ~/.config/autostart overrides the system one).
fn set_startup(item: &Item, on: bool) {
    let dir = glib::user_config_dir().join("autostart");
    let _ = std::fs::create_dir_all(&dir);
    let target = dir.join(&item.file);
    let src = if target.exists() { target.clone() } else { std::path::PathBuf::from(&item.path) };
    let content = std::fs::read_to_string(&src).unwrap_or_default();
    let mut out = vec![];
    let mut in_entry = false;
    for l in content.lines() {
        if l.starts_with('[') {
            if in_entry { push_flags(&mut out, on); }
            in_entry = l.trim() == "[Desktop Entry]";
        }
        if in_entry && (l.starts_with("Hidden=") || l.starts_with("X-GNOME-Autostart-enabled=") || l.starts_with("X-Aerode-Date-Disabled=")) { continue; }
        out.push(l.to_string());
    }
    if in_entry { push_flags(&mut out, on); }
    let _ = std::fs::write(target, out.join("\n") + "\n");
}
fn push_flags(out: &mut Vec<String>, on: bool) {
    while out.last().map(|l| l.trim().is_empty()).unwrap_or(false) { out.pop(); }
    out.push(format!("Hidden={}", !on));
    if !on { out.push(format!("X-Aerode-Date-Disabled={}", chrono::Local::now().timestamp())); }
    out.push(String::new());
}

#[derive(Clone, Debug, Default)]
pub struct Svc {
    pub unit: String, pub desc: String, pub active: String, pub sub: String, pub file_state: String, pub user: String,
    pub path: String, pub exec: String, pub requires: String, pub wants: String, pub required_by: String, pub wanted_by: String,
    pub restart: String, pub restart_sec: String, pub can_start: bool, pub can_stop: bool,
}

impl Svc {
    pub fn name(&self) -> &str { self.unit.trim_end_matches(".service") }
    pub fn status(&self) -> &'static str {
        match self.active.as_str() { "active" | "reloading" => "Started", "activating" => "Starting", "deactivating" => "Stopping", _ => "" }
    }
    pub fn startup(&self) -> &'static str {
        match self.file_state.as_str() {
            "enabled" | "enabled-runtime" | "alias" => "Automatic",
            "masked" | "masked-runtime" => "Disabled",
            "" | "transient" => "Manual",
            _ => "Manual",
        }
    }
    pub fn logon(&self) -> String { if self.user.is_empty() || self.user == "root" { "Local System".into() } else { self.user.clone() } }
}

const SHOW_PROPS: &str = "Id,Description,ActiveState,SubState,UnitFileState,User,FragmentPath,ExecStart,Requires,Wants,RequiredBy,WantedBy,Restart,RestartUSec,CanStart,CanStop";

/// Every installed system service (loaded ones and unit files), worker thread.
pub fn services() -> Vec<Svc> {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for l in sh(&["systemctl", "list-units", "--type=service", "--all", "--no-legend", "--plain", "--no-pager"]).lines() {
        if let Some(u) = l.split_whitespace().next() { if u.ends_with(".service") { names.insert(u.to_string()); } }
    }
    let mut states: Vec<(String, String)> = vec![];
    for l in sh(&["systemctl", "list-unit-files", "--type=service", "--no-legend", "--no-pager"]).lines() {
        let mut it = l.split_whitespace();
        if let (Some(u), Some(st)) = (it.next(), it.next()) { if u.ends_with(".service") && !u.ends_with("@.service") { names.insert(u.to_string()); states.push((u.to_string(), st.to_string())); } }
    }
    if names.is_empty() { return vec![]; }
    let mut args: Vec<&str> = vec!["systemctl", "show", "--no-pager", "-p", SHOW_PROPS, "--"];
    args.extend(names.iter().map(|s| s.as_str()));
    let v = parse_show(&sh(&args));
    if !v.is_empty() { return v; }
    // no service manager to ask (a chroot, a container): what the unit files say
    from_unit_files(&states)
}

/// Services described by their unit files alone (state unknown).
fn from_unit_files(states: &[(String, String)]) -> Vec<Svc> {
    let dirs = ["/etc/systemd/system", "/usr/lib/systemd/system", "/lib/systemd/system"];
    let mut v: Vec<Svc> = states.iter().map(|(u, st)| {
        let path = dirs.iter().map(|d| format!("{d}/{u}")).find(|p| std::path::Path::new(p).exists()).unwrap_or_default();
        let t = std::fs::read_to_string(&path).unwrap_or_default();
        let key = |k: &str| t.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}="))).unwrap_or("").trim().to_string();
        Svc { unit: u.clone(), desc: key("Description"), file_state: st.clone(), user: key("User"), path, exec: key("ExecStart").trim_start_matches(['-', '@', '+', '!']).to_string(),
            requires: key("Requires"), wants: key("Wants"), wanted_by: key("WantedBy"), restart: key("Restart"), can_start: st != "masked", ..Default::default() }
    }).collect();
    v.sort_by(|a, b| a.unit.to_lowercase().cmp(&b.unit.to_lowercase()));
    v
}

pub fn parse_show(t: &str) -> Vec<Svc> {
    let mut v = vec![];
    for block in t.split("\n\n") {
        let mut s = Svc::default();
        for l in block.lines() {
            let Some((k, val)) = l.split_once('=') else { continue };
            let val = val.to_string();
            match k {
                "Id" => s.unit = val, "Description" => s.desc = val, "ActiveState" => s.active = val, "SubState" => s.sub = val,
                "UnitFileState" => s.file_state = val, "User" => s.user = val, "FragmentPath" => s.path = val,
                "ExecStart" => s.exec = val.split("argv[]=").nth(1).map(|a| a.split(" ;").next().unwrap_or("").trim().to_string()).unwrap_or_default(),
                "Requires" => s.requires = val, "Wants" => s.wants = val, "RequiredBy" => s.required_by = val, "WantedBy" => s.wanted_by = val,
                "Restart" => s.restart = val, "RestartUSec" => s.restart_sec = val,
                "CanStart" => s.can_start = val == "yes", "CanStop" => s.can_stop = val == "yes",
                _ => {}
            }
        }
        if s.unit.ends_with(".service") && s.file_state != "bad" { v.push(s); }
    }
    v.sort_by(|a, b| a.unit.to_lowercase().cmp(&b.unit.to_lowercase()));
    v
}

/// Services msconfig hides with "Hide all Microsoft services": the ones the base system brings.
fn is_system_service(s: &Svc, maker: &str) -> bool {
    let u = s.unit.as_str();
    u.starts_with("systemd-") || u.starts_with("dbus") || u.starts_with("getty") || u.starts_with("user@") || u.starts_with("modprobe@")
        || u.starts_with("serial-getty") || u.starts_with("emergency") || u.starts_with("rescue") || u.starts_with("initrd")
        || (maker == "Arch Linux" && (s.path.starts_with("/usr/lib/systemd/system/systemd") || s.path.contains("/dbus")))
}

// ------------------------------------------------------------------- System Configuration

impl Cp {
    pub(crate) fn page_startup(self: &Rc<Self>) -> gtk::Widget {
        css();
        self.side_link("Services", "services");
        { let me = self.clone(); side_action(self, "Open the Startup folder", move || {
            let d = glib::user_config_dir().join("autostart");
            let _ = std::fs::create_dir_all(&d);
            if !util::spawn(&["aerode-explorer", &d.to_string_lossy()]) { util::open_path(&d); }
            let _ = &me;
        }); }
        see_also(self, &[("Administrative Tools", "admintools"), ("Task Manager", "!taskmgr")]);
        let sheet = Sheet::new(self);
        sheet.nb.set_size_request(640, 430);
        let pending_start: Rc<RefCell<HashMap<String, (Item, bool)>>> = Rc::new(RefCell::new(HashMap::new()));
        let pending_svc: Rc<RefCell<HashMap<String, bool>>> = Rc::new(RefCell::new(HashMap::new()));
        // while the lists load, setting the General radios to what is found must not change anything
        let loading = Rc::new(Cell::new(true));

        // ---------------------------------------------------------------- General
        let g = vbox(8);
        let sel = vbox(4);
        let r = radios(&["_Normal startup", "_Diagnostic startup", "_Selective startup"], 0);
        let descs = ["Load all device drivers and services", "Load basic devices and services only", ""];
        for (i, b) in r.iter().enumerate() {
            sel.pack_start(b, false, false, 0);
            if !descs[i].is_empty() { let l = label(descs[i], ""); l.set_margin_start(24); sel.pack_start(&l, false, false, 0); }
        }
        r[1].set_sensitive(false);
        r[1].set_tooltip_text(Some("Diagnostic startup is not available: Linux has no minimal set of services that is safe to start alone."));
        let load_sys = check("Load s_ystem services", true);
        let load_start = check("Load s_tartup items", true);
        let orig = check("_Use original boot configuration", true);
        load_sys.set_sensitive(false);
        orig.set_sensitive(false);
        for c in [&load_sys, &load_start, &orig] { c.set_margin_start(24); sel.pack_start(c, false, false, 0); }
        g.pack_start(&group("Startup selection", &sel), false, false, 0);
        sheet.tab("General", &g);

        // ---------------------------------------------------------------- Services
        let s = vbox(6);
        // check, service, manufacturer, status, date disabled, unit, system?
        let sstore = gtk::ListStore::new(&[glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL]);
        let sfilter = gtk::TreeModelFilter::new(&sstore, None);
        let hide_sys = check("_Hide all system services", false);
        { let h = hide_sys.clone(); sfilter.set_visible_func(move |m, it| !(h.is_active() && m.value(it, 6).get::<bool>().unwrap_or(false))); }
        { let f = sfilter.clone(); hide_sys.connect_toggled(move |_| f.refilter()); }
        let stv = gtk::TreeView::with_model(&sfilter);
        let stog = toggle_col(&stv, "Service", 0, 1, 220);
        columns(&stv, &[("Manufacturer", 2, 130), ("Status", 3, 80), ("Date Disabled", 4, 0)]);
        let ssw = scrolled(&stv);
        ssw.set_size_request(-1, 300);
        s.pack_start(&ssw, true, true, 0);
        let note = label("Note that some secure system services may not be disabled.", "cp-hint");
        s.pack_start(&note, false, false, 0);
        let srow = hbox(8);
        srow.pack_start(&hide_sys, true, true, 0);
        let (en_all, dis_all) = (gtk::Button::with_mnemonic("_Enable all"), gtk::Button::with_mnemonic("_Disable all"));
        srow.pack_start(&en_all, false, false, 0);
        srow.pack_start(&dis_all, false, false, 0);
        s.pack_start(&srow, false, false, 0);
        sheet.tab("Services", &s);
        {
            let (sstore, pending, d, filter) = (sstore.clone(), pending_svc.clone(), sheet.dirty(), sfilter.clone());
            stog.connect_toggled(move |_, path| {
                let Some(fit) = filter.iter(&path) else { return };
                let it = filter.convert_iter_to_child_iter(&fit);
                let on = !sstore.value(&it, 0).get::<bool>().unwrap_or(false);
                sstore.set(&it, &[(0, &on)]);
                pending.borrow_mut().insert(sval(&sstore, &it, 5), on);
                d();
            });
        }
        for (b, on) in [(&en_all, true), (&dis_all, false)] {
            let (filter, sstore, pending, d) = (sfilter.clone(), sstore.clone(), pending_svc.clone(), sheet.dirty());
            b.connect_clicked(move |_| {
                filter.foreach(|_, _, fit| {
                    let it = filter.convert_iter_to_child_iter(fit);
                    if sstore.value(&it, 0).get::<bool>().unwrap_or(false) != on {
                        sstore.set(&it, &[(0, &on)]);
                        pending.borrow_mut().insert(sval(&sstore, &it, 5), on);
                    }
                    false
                });
                d();
            });
        }

        // ---------------------------------------------------------------- Startup
        let st = vbox(6);
        // check, item, manufacturer, command, location, date disabled, index, icon
        let tstore = gtk::ListStore::new(&[glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
        let ttv = gtk::TreeView::with_model(&tstore);
        let ttog = toggle_col(&ttv, "Startup Item", 0, 1, 170);
        columns(&ttv, &[("Manufacturer", 2, 100), ("Command", 3, 150), ("Location", 4, 110), ("Date Disabled", 5, 0)]);
        let tsw = scrolled(&ttv);
        tsw.set_size_request(-1, 330);
        st.pack_start(&tsw, true, true, 0);
        let trow = hbox(8);
        trow.set_halign(gtk::Align::End);
        let (ten, tdis) = (gtk::Button::with_mnemonic("_Enable all"), gtk::Button::with_mnemonic("_Disable all"));
        trow.pack_start(&ten, false, false, 0);
        trow.pack_start(&tdis, false, false, 0);
        st.pack_start(&trow, false, false, 0);
        sheet.tab("Startup", &st);
        let items: Rc<RefCell<Vec<Item>>> = Rc::new(RefCell::new(vec![]));
        {
            let (tstore, items, pending, d, ls) = (tstore.clone(), items.clone(), pending_start.clone(), sheet.dirty(), load_start.clone());
            ttog.connect_toggled(move |_, path| {
                let Some(it) = tstore.iter(&path) else { return };
                let on = !tstore.value(&it, 0).get::<bool>().unwrap_or(false);
                tstore.set(&it, &[(0, &on)]);
                let i = tstore.value(&it, 6).get::<u32>().unwrap_or(0) as usize;
                if let Some(item) = items.borrow().get(i) { pending.borrow_mut().insert(item.file.clone(), (item.clone(), on)); }
                ls.set_active(!all_off(&tstore));
                d();
            });
        }
        for (b, on) in [(&ten, true), (&tdis, false)] {
            let (tstore, items, pending, d) = (tstore.clone(), items.clone(), pending_start.clone(), sheet.dirty());
            b.connect_clicked(move |_| {
                tstore.foreach(|_, _, it| {
                    tstore.set(it, &[(0, &on)]);
                    let i = tstore.value(it, 6).get::<u32>().unwrap_or(0) as usize;
                    if let Some(item) = items.borrow().get(i) { pending.borrow_mut().insert(item.file.clone(), (item.clone(), on)); }
                    false
                });
                d();
            });
        }
        // General's radios drive the startup items like msconfig does
        {
            let (tstore, items, pending, d, r0, ls) = (tstore.clone(), items.clone(), pending_start.clone(), sheet.dirty(), r[0].clone(), load_start.clone());
            let set_all = move |on: bool| {
                tstore.foreach(|_, _, it| {
                    tstore.set(it, &[(0, &on)]);
                    let i = tstore.value(it, 6).get::<u32>().unwrap_or(0) as usize;
                    if let Some(item) = items.borrow().get(i) { pending.borrow_mut().insert(item.file.clone(), (item.clone(), on)); }
                    false
                });
                d();
            };
            let set_all = Rc::new(set_all);
            { let (sa, ls2, l) = (set_all.clone(), ls.clone(), loading.clone()); r0.connect_toggled(move |b| if b.is_active() && !l.get() { ls2.set_active(true); sa(true); }); }
            { let (sa, r2, l) = (set_all.clone(), r[2].clone(), loading.clone()); ls.connect_toggled(move |c| { if l.get() { return; } if !c.is_active() { r2.set_active(true); } sa(c.is_active()); }); }
        }

        // ---------------------------------------------------------------- Tools
        let tools = vbox(6);
        let tl = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
        for (n, d, c) in TOOLS { tl.insert_with_values(None, &[(0, n), (1, d), (2, c)]); }
        let tlv = gtk::TreeView::with_model(&tl);
        columns(&tlv, &[("Tool Name", 0, 200), ("Description", 1, 0)]);
        let tlsw = scrolled(&tlv);
        tlsw.set_size_request(-1, 300);
        tools.pack_start(&tlsw, true, true, 0);
        tools.pack_start(&label("Selected command:", ""), false, false, 0);
        let cmd = gtk::Entry::new();
        cmd.set_editable(false);
        tools.pack_start(&cmd, false, false, 0);
        let launch = gtk::Button::with_mnemonic("_Launch");
        launch.set_halign(gtk::Align::End);
        launch.set_size_request(80, -1);
        tools.pack_start(&launch, false, false, 0);
        { let c = cmd.clone(); tlv.selection().connect_changed(move |s| { if let Some((m, it)) = s.selected() { c.set_text(&sval(&m, &it, 2)); } }); }
        { let c = cmd.clone(); launch.connect_clicked(move |_| { let t = c.text().to_string(); if !t.is_empty() { util::run_command(&t); } }); }
        { let c = cmd.clone(); tlv.connect_row_activated(move |_, _, _| { let t = c.text().to_string(); if !t.is_empty() { util::run_command(&t); } }); }
        sheet.tab("Tools", &tools);

        // ---------------------------------------------------------------- load
        {
            let (sstore, tstore, items, r0, r2, ls, loading) = (sstore.clone(), tstore.clone(), items.clone(), r[0].clone(), r[2].clone(), load_start.clone(), loading.clone());
            let raw = startup_items();
            bg(move || {
                let own = owners(&["usr/lib/systemd/system/", "etc/xdg/autostart/", "etc/systemd/system/"]);
                let publ = publishers();
                let svcs = services();
                (own, publ, svcs)
            }, move |(own, publ, svcs)| {
                let dates = disabled_dates();
                let mut any_off = false;
                for s in svcs.iter().filter(|s| !matches!(s.file_state.as_str(), "static" | "indirect" | "generated" | "transient" | "" | "alias" | "masked")) {
                    let maker = manufacturer(&s.path, &own, &publ);
                    let on = s.file_state.starts_with("enabled");
                    let date = if on { String::new() } else { dates.get(&s.unit).map(|t| super::super::admin::when(*t)).unwrap_or_default() };
                    // msconfig lists the display names ("Bluetooth service", not bluetooth.service)
                    let shown = if s.desc.is_empty() { s.name().to_string() } else { s.desc.clone() };
                    sstore.insert_with_values(None, &[(0, &on), (1, &shown), (2, &maker), (3, &if s.active == "active" { "Running" } else { "Stopped" }), (4, &date), (5, &s.unit), (6, &is_system_service(s, &maker))]);
                }
                let mut list = raw;
                for (i, it) in list.iter_mut().enumerate() {
                    it.maker = manufacturer(&it.path, &own, &publ);
                    if !it.on { any_off = true; }
                    tstore.insert_with_values(None, &[(0, &it.on), (1, &it.name), (2, &it.maker), (3, &it.command), (4, &it.location),
                        (5, &if it.on || it.disabled_at == 0 { String::new() } else { super::super::admin::when(it.disabled_at) }), (6, &(i as u32))]);
                }
                *items.borrow_mut() = list;
                // like msconfig: anything turned off → Selective startup (showing the state changes nothing)
                if any_off { r2.set_active(true); ls.set_active(!all_off(&tstore)); } else { r0.set_active(true); }
                loading.set(false);
            });
        }

        // ---------------------------------------------------------------- apply
        {
            let (ps, pv, me) = (pending_start.clone(), pending_svc.clone(), self.clone());
            sheet.on_apply(move || {
                for (_, (item, on)) in ps.borrow_mut().drain() { if item.on != on || !on { set_startup(&item, on); } }
                let changes: Vec<(String, bool)> = pv.borrow_mut().drain().collect();
                if changes.is_empty() { return true; }
                let script = changes.iter().map(|(u, on)| format!("systemctl {} {}", if *on { "enable" } else { "disable" }, q(u))).collect::<Vec<_>>().join("; ");
                let me = me.clone();
                elevate_sh(&script, move |ok, out| {
                    if !report("System Configuration could not change the services.", ok, &out) { return; }
                    let mut d = disabled_dates();
                    for (u, on) in &changes { if *on { d.remove(u); } else { d.insert(u.clone(), chrono::Local::now().timestamp()); } }
                    save_dates(&d);
                    restart_prompt(&me);
                });
                true
            });
        }
        sheet.widget()
    }

    pub(crate) fn page_services(self: &Rc<Self>) -> gtk::Widget {
        // services.msc is a console of its own (like Event Viewer): start it, stay where we were
        spawn_tool("services");
        let me = self.clone();
        glib::idle_add_local_once(move || {
            let prev = { let mut h = me.history.borrow_mut(); h.pop(); h.pop() };
            me.open(&prev.unwrap_or_default());
        });
        gtk::Box::new(gtk::Orientation::Vertical, 0).upcast()
    }
}

fn all_off(store: &gtk::ListStore) -> bool {
    let mut any = false;
    store.foreach(|m, _, it| { if m.value(it, 0).get::<bool>().unwrap_or(false) { any = true; } any });
    !any
}

/// A first column with a check box (model `check`) and text (`text`).
fn toggle_col(tv: &gtk::TreeView, title: &str, check: i32, text: i32, width: i32) -> gtk::CellRendererToggle {
    let col = gtk::TreeViewColumn::new();
    col.set_title(title);
    let t = gtk::CellRendererToggle::new();
    TreeViewColumnExt::pack_start(&col, &t, false);
    TreeViewColumnExt::add_attribute(&col, &t, "active", check);
    let r = gtk::CellRendererText::new();
    r.set_property("ellipsize", gtk::pango::EllipsizeMode::End);
    TreeViewColumnExt::pack_start(&col, &r, true);
    TreeViewColumnExt::add_attribute(&col, &r, "text", text);
    col.set_resizable(true);
    col.set_sort_column_id(text);
    col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    col.set_fixed_width(width);
    tv.append_column(&col);
    t
}

/// "You may need to restart your computer to apply these changes."
fn restart_prompt(cp: &Rc<Cp>) {
    let d = gtk::MessageDialog::new(Some(&cp.win), gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::None,
        "You may need to restart your computer to apply these changes.");
    d.set_title("System Configuration");
    d.set_secondary_text(Some("Before restarting, save any open files and close all programs."));
    d.add_button("_Restart", gtk::ResponseType::Accept);
    d.add_button("E_xit without restart", gtk::ResponseType::Close);
    d.set_default_response(gtk::ResponseType::Close);
    let r = d.run();
    d.close();
    if r == gtk::ResponseType::Accept { aerode::sys::restart(); }
}

/// msconfig's Tools tab: (name, description, command).
const TOOLS: &[(&str, &str, &str)] = &[
    ("About Windows", "Display Windows version information.", "aerode-control system"),
    ("Change UAC Settings", "Change User Account Control settings.", "aerode-control users/uac"),
    ("Action Center", "Open the Action Center.", "aerode-control actioncenter"),
    ("Windows Troubleshooting", "Troubleshoot problems with your computer.", "aerode-control troubleshooting"),
    ("Computer Management", "View and configure system settings and components.", "aerode-control compmgmt"),
    ("System Information", "View advanced information about hardware and software settings.", "aerode-control sysinfo"),
    ("Event Viewer", "View monitoring and troubleshooting messages.", "aerode-control eventvwr"),
    ("Programs", "Launch, add or remove programs and Windows components.", "aerode-control programs"),
    ("System Properties", "View basic information about your computer system settings.", "aerode-control system"),
    ("Internet Options", "View Internet Properties.", "aerode-control internet"),
    ("Internet Protocol Configuration", "View and configure network address settings.", "aerode-cmd -e sh -c 'ip addr; echo; read -n 1 -s -r -p \"Press any key to continue . . .\"'"),
    ("Task Manager", "View details about programs and processes running on your computer.", "aerode-taskmgr"),
    ("Command Prompt", "Open a command prompt window.", "aerode-cmd"),
    ("Services", "Start, stop and configure services.", "aerode-control services"),
    ("Disk Management", "Create and format hard disk partitions.", "aerode-control diskmgmt"),
    ("Device Manager", "View and update your hardware's settings and driver software.", "aerode-control devmgr"),
];

// --------------------------------------------------------------------------- services.msc

pub fn services_window(app: &gtk::Application) -> gtk::Window {
    use super::super::admin::mmc::{act, act_off, head, Mmc, I_PROPS, I_REFRESH, I_RUN, I_STOP};
    css();
    let m = Mmc::new(app, "Services", &["aerode-services", "applications-system"], true, true);
    m.add(None, "local", "Services (Local)", &["aerode-services", "applications-system"]);
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
    let svcs: Rc<RefCell<Vec<Svc>>> = Rc::new(RefCell::new(vec![]));
    let tv = gtk::TreeView::with_model(&store);
    icon_column(&tv, "Name", 0, 1, 200);
    columns(&tv, &[("Description", 2, 260), ("Status", 3, 70), ("Startup Type", 4, 95), ("Log On As", 5, 0)]);
    // Extended view: the selected service's description and links on the left
    let ext = vbox(8);
    ext.set_size_request(200, -1);
    ext.set_border_width(10);
    let ext_name = bold("");
    ext_name.set_line_wrap(true);
    let ext_links = vbox(2);
    let ext_desc = label("Select an item to view its description.", "");
    ext_desc.set_max_width_chars(28);
    ext.pack_start(&ext_name, false, false, 0);
    ext.pack_start(&ext_links, false, false, 0);
    ext.pack_start(&ext_desc, false, false, 0);
    let (h, extra) = head("Services (Local)", "Loading…");
    let body = hbox(0);
    body.pack_start(&ext, false, false, 0);
    body.pack_start(&scrolled(&tv), true, true, 0);
    let tabs = gtk::Notebook::new();
    tabs.set_tab_pos(gtk::PositionType::Bottom);
    tabs.set_show_border(false);
    let center = vbox(0);
    center.pack_start(&h, false, false, 0);
    center.pack_start(&body, true, true, 0);
    let page_ext = vbox(0);
    let page_std = vbox(0);
    tabs.append_page(&page_ext, Some(&gtk::Label::new(Some("Extended"))));
    tabs.append_page(&page_std, Some(&gtk::Label::new(Some("Standard"))));
    let outer = vbox(0);
    outer.pack_start(&center, true, true, 0);
    outer.pack_start(&tabs, false, false, 0);
    { let e = ext.clone(); tabs.connect_switch_page(move |_, _, n| e.set_visible(n == 0)); }
    let selected = { let (tv, svcs) = (tv.clone(), svcs.clone()); move || -> Option<Svc> {
        let (mo, it) = tv.selection().selected()?;
        let i = mo.value(&it, 6).get::<u32>().ok()? as usize;
        svcs.borrow().get(i).cloned()
    } };
    let selected: Rc<dyn Fn() -> Option<Svc>> = Rc::new(selected);
    let reload: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let do_verb: Rc<dyn Fn(&str)> = {
        let (sel, reload, win) = (selected.clone(), reload.clone(), m.win.clone());
        Rc::new(move |verb: &str| {
            let Some(s) = sel() else { return };
            let reload = reload.clone();
            let what = match verb { "start" => "start", "stop" => "stop", _ => "restart" };
            let _ = &win;
            let name = s.name().to_string();
            elevate(&["systemctl", verb, &s.unit], move |ok, out| {
                report(&format!("Windows could not {what} the {name} service on Local Computer."), ok, &out);
                if let Some(r) = reload.borrow().clone() { r(); }
            });
        })
    };
    let set_actions: Rc<dyn Fn()> = {
        let (m2, sel, do_verb, ext_name, ext_links, ext_desc) = (m.clone(), selected.clone(), do_verb.clone(), ext_name.clone(), ext_links.clone(), ext_desc.clone());
        let reload = reload.clone();
        Rc::new(move || {
            let s = sel();
            let mut acts = vec![];
            for c in ext_links.children() { ext_links.remove(&c); }
            match &s {
                Some(s) => {
                    let started = s.active == "active";
                    let dv = do_verb.clone();
                    acts.push(if !started && s.can_start { act("Start", I_RUN, move || dv("start")) } else { act_off("Start") });
                    let dv = do_verb.clone();
                    acts.push(if started && s.can_stop { act("Stop", I_STOP, move || dv("stop")) } else { act_off("Stop") });
                    acts.push(act_off("Pause"));
                    let dv = do_verb.clone();
                    acts.push(if started { act("Restart", I_REFRESH, move || dv("restart")) } else { act_off("Restart") });
                    let (s2, w, r) = (s.clone(), m2.win.clone(), reload.clone());
                    acts.push(act("Properties", I_PROPS, move || properties(&w, &s2, r.clone())));
                    ext_name.set_text(s.name());
                    let links: Vec<(&str, &str)> = if started { vec![("Stop", "stop"), ("Restart", "restart")] } else if s.can_start { vec![("Start", "start")] } else { vec![] };
                    for (t, verb) in links {
                        let dv = do_verb.clone();
                        let row = hbox(0);
                        row.pack_start(&link(t, move || dv(verb)), false, false, 0);
                        row.pack_start(&label(" the service", ""), false, false, 0);
                        ext_links.pack_start(&row, false, false, 0);
                    }
                    ext_desc.set_text(&format!("Description:\n{}", s.desc));
                }
                None => { ext_name.set_text(""); ext_desc.set_text("Select an item to view its description."); }
            }
            ext_links.show_all();
            let r = reload.clone();
            let mut groups = vec![("Services (Local)".to_string(), vec![act("Refresh", I_REFRESH, move || { if let Some(f) = r.borrow().clone() { f(); } })])];
            if let Some(s) = &s { groups.push((s.name().to_string(), acts)); }
            m2.set_actions(groups);
        })
    };
    {
        let (store, svcs, extra, sa, tv2) = (store.clone(), svcs.clone(), extra.clone(), set_actions.clone(), tv.clone());
        *reload.borrow_mut() = Some(Rc::new(move || {
            let keep = tv2.selection().selected().map(|(mo, it)| sval(&mo, &it, 1));
            let (store, svcs, extra, sa, tv3) = (store.clone(), svcs.clone(), extra.clone(), sa.clone(), tv2.clone());
            bg(services, move |list| {
                store.clear();
                let pb = pix(&["aerode-services", "applications-system"], 16);
                for (i, s) in list.iter().enumerate() {
                    let it = store.insert_with_values(None, &[(0, &pb), (1, &s.name()), (2, &s.desc), (3, &s.status()), (4, &s.startup()), (5, &s.logon()), (6, &(i as u32))]);
                    if keep.as_deref() == Some(s.name()) { tv3.selection().select_iter(&it); }
                }
                extra.set_text(&format!("{} services", list.len()));
                *svcs.borrow_mut() = list;
                sa();
            });
        }));
    }
    { let sa = set_actions.clone(); tv.selection().connect_changed(move |_| sa()); }
    {
        let (sel, w, r) = (selected.clone(), m.win.clone(), reload.clone());
        tv.connect_row_activated(move |_, _, _| { if let Some(s) = sel() { properties(&w, &s, r.clone()); } });
    }
    {
        let m2 = m.clone();
        tv.connect_button_press_event(move |tv, e| {
            if e.button() != 3 { return glib::Propagation::Proceed; }
            let (x, y) = e.position();
            if let Some((Some(p), _, _, _)) = tv.path_at_pos(x as i32, y as i32) { tv.selection().select_path(&p); }
            m2.popup_actions(e);
            glib::Propagation::Stop
        });
    }
    let r0 = reload.borrow().clone();
    let tb_start = m.tool(I_RUN, "Start Service", { let dv = do_verb.clone(); move || dv("start") });
    let tb_stop = m.tool(I_STOP, "Stop Service", { let dv = do_verb.clone(); move || dv("stop") });
    let tb_restart = m.tool(I_REFRESH, "Restart Service", { let dv = do_verb.clone(); move || dv("restart") });
    let _ = (&tb_start, &tb_stop, &tb_restart);
    {
        let (o, r) = (outer.clone(), r0.clone());
        m.handle("local", move |m, _| { m.set_center(&o); if let Some(r) = &r { r(); } });
    }
    m.keep(Rc::new(svcs.clone()));
    m.show("local");
    m.win.clone()
}

/// "<service> Properties (Local Computer)": General, Log On, Recovery, Dependencies.
fn properties(parent: &gtk::Window, s: &Svc, reload: Rc<RefCell<Option<Rc<dyn Fn()>>>>) {
    let d = dialog(Some(parent), &format!("{} Properties (Local Computer)", s.name()), &[]);
    d.set_default_size(420, 470);
    let nb = gtk::Notebook::new();
    // ---------------- General
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(10);
    g.set_border_width(10);
    let mut r = 0;
    let mut row = |k: &str, w: &gtk::Widget| { let l = label(k, ""); l.set_valign(gtk::Align::Start); g.attach(&l, 0, r, 1, 1); w.set_hexpand(true); g.attach(w, 1, r, 1, 1); r += 1; };
    row("Service name:", label(&s.unit, "").upcast_ref());
    let disp = gtk::Entry::new(); disp.set_text(s.name()); disp.set_editable(false);
    row("Display name:", disp.upcast_ref());
    let desc = super::super::admin::text_box(&s.desc);
    desc.set_size_request(-1, 50);
    row("Description:", scrolled(&desc).upcast_ref());
    let path = gtk::Entry::new(); path.set_text(if s.exec.is_empty() { &s.path } else { &s.exec }); path.set_editable(false);
    let pl = vbox(3);
    pl.pack_start(&label("Path to executable:", ""), false, false, 0);
    pl.pack_start(&path, false, false, 0);
    g.attach(&pl, 0, r, 2, 1); r += 1;
    let startup = combo(&[("Automatic", "Automatic"), ("Manual", "Manual"), ("Disabled", "Disabled")], s.startup());
    let fixed = matches!(s.file_state.as_str(), "static" | "indirect" | "generated" | "transient" | "alias" | "");
    if fixed { startup.set_sensitive(false); startup.set_tooltip_text(Some("This service is started by other services or on demand; its startup type cannot be changed.")); }
    let mut row2 = |k: &str, w: &gtk::Widget| { let l = label(k, ""); g.attach(&l, 0, r, 1, 1); g.attach(w, 1, r, 1, 1); r += 1; };
    row2("Startup type:", startup.upcast_ref());
    g.attach(&link("Help me configure service startup options.", || { util::open_uri("https://wiki.archlinux.org/title/Systemd#Using_units"); }), 0, r, 2, 1); r += 1;
    g.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, r, 2, 1); r += 1;
    let started = s.active == "active";
    g.attach(&label("Service status:", ""), 0, r, 1, 1);
    g.attach(&label(if started { "Started" } else { "Stopped" }, ""), 1, r, 1, 1); r += 1;
    let bb = hbox(6);
    let (b_start, b_stop, b_pause, b_resume) = (gtk::Button::with_mnemonic("_Start"), gtk::Button::with_mnemonic("S_top"), gtk::Button::with_mnemonic("_Pause"), gtk::Button::with_mnemonic("_Resume"));
    b_start.set_sensitive(!started && s.can_start);
    b_stop.set_sensitive(started && s.can_stop);
    b_pause.set_sensitive(false);
    b_resume.set_sensitive(false);
    for b in [&b_start, &b_stop, &b_pause, &b_resume] { b.set_size_request(75, -1); bb.pack_start(b, false, false, 0); }
    g.attach(&bb, 0, r, 2, 1); r += 1;
    let sp = label("You can specify the start parameters that apply when you start the service from here.", "");
    sp.set_max_width_chars(50);
    g.attach(&sp, 0, r, 2, 1); r += 1;
    let params = gtk::Entry::new(); params.set_sensitive(false);
    g.attach(&label("Start parameters:", ""), 0, r, 1, 1);
    g.attach(&params, 1, r, 1, 1);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    // ---------------- Log On
    let lo = vbox(8);
    lo.set_border_width(10);
    lo.pack_start(&label("Log on as:", ""), false, false, 0);
    let local = s.logon() == "Local System";
    let rr = radios(&["_Local System account", "_This account:"], if local { 0 } else { 1 });
    for b in &rr { b.set_sensitive(false); }
    lo.pack_start(&rr[0], false, false, 0);
    let ia = check("Allo_w service to interact with desktop", false);
    ia.set_sensitive(false);
    ia.set_margin_start(24);
    lo.pack_start(&ia, false, false, 0);
    let ta = hbox(8);
    ta.pack_start(&rr[1], false, false, 0);
    let acct = gtk::Entry::new(); acct.set_text(if local { "" } else { &s.user }); acct.set_sensitive(false);
    ta.pack_start(&acct, true, true, 0);
    lo.pack_start(&ta, false, false, 0);
    nb.append_page(&lo, Some(&gtk::Label::new(Some("Log On"))));
    // ---------------- Recovery
    let rc = gtk::Grid::new();
    rc.set_border_width(10);
    rc.set_row_spacing(8);
    rc.set_column_spacing(10);
    let rcl = label("Select the computer's response if this service fails.", "");
    rc.attach(&rcl, 0, 0, 2, 1);
    let action = match s.restart.as_str() { "no" | "" => "Take No Action", _ => "Restart the Service" };
    for (i, k) in ["First failure:", "Second failure:", "Subsequent failures:"].iter().enumerate() {
        rc.attach(&label(k, ""), 0, i as i32 + 1, 1, 1);
        let c = combo(&[("a", action)], "a"); c.set_sensitive(false);
        rc.attach(&c, 1, i as i32 + 1, 1, 1);
    }
    rc.attach(&label("Restart service after:", ""), 0, 4, 1, 1);
    rc.attach(&label(&s.restart_sec.replace("ms", " ms").replace("min", " minutes"), ""), 1, 4, 1, 1);
    rc.attach(&label(&format!("Restart= {}", if s.restart.is_empty() { "no" } else { &s.restart }), "cp-hint"), 0, 5, 2, 1);
    nb.append_page(&rc, Some(&gtk::Label::new(Some("Recovery"))));
    // ---------------- Dependencies
    let dp = vbox(6);
    dp.set_border_width(10);
    let dl = label(&format!("Some services depend on other services, system drivers or load order groups. If a system component is stopped, or is not running properly, dependent services can be affected.\n\n{}", s.name()), "");
    dl.set_max_width_chars(55);
    dp.pack_start(&dl, false, false, 0);
    let mk = |t: &str, list: &str| {
        let b = vbox(3);
        b.pack_start(&label(t, ""), false, false, 0);
        let st = gtk::ListStore::new(&[glib::Type::STRING]);
        let mut any = false;
        for u in list.split_whitespace().filter(|u| u.ends_with(".service") || u.ends_with(".target") || u.ends_with(".socket")) { st.insert_with_values(None, &[(0, &u)]); any = true; }
        if !any { st.insert_with_values(None, &[(0, &"<No Dependencies>")]); }
        let tv = gtk::TreeView::with_model(&st);
        tv.set_headers_visible(false);
        columns(&tv, &[("", 0, 0)]);
        let sw = scrolled(&tv);
        sw.set_size_request(-1, 110);
        b.pack_start(&sw, true, true, 0);
        b
    };
    dp.pack_start(&mk("This service depends on the following system components:", &format!("{} {}", s.requires, s.wants)), true, true, 0);
    dp.pack_start(&mk("The following system components depend on this service:", &format!("{} {}", s.required_by, s.wanted_by)), true, true, 0);
    nb.append_page(&dp, Some(&gtk::Label::new(Some("Dependencies"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let apply = d.add_button("_Apply", gtk::ResponseType::Apply);
    apply.set_sensitive(false);
    { let a = apply.clone(); startup.connect_changed(move |_| a.set_sensitive(true)); }
    for (b, verb) in [(&b_start, "start"), (&b_stop, "stop")] {
        let (unit, d2, rl) = (s.unit.clone(), d.clone(), reload.clone());
        b.connect_clicked(move |_| {
            let (d3, rl) = (d2.clone(), rl.clone());
            let what = if verb == "start" { "start" } else { "stop" };
            elevate(&["systemctl", verb, &unit], move |ok, out| {
                report(&format!("Windows could not {what} the service."), ok, &out);
                if let Some(r) = rl.borrow().clone() { r(); }
                d3.close();
            });
        });
    }
    let unit = s.unit.clone();
    let orig = s.startup().to_string();
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok || r == gtk::ResponseType::Apply {
            let want = startup.active_id().map(|s| s.to_string()).unwrap_or_default();
            if want != orig && !want.is_empty() {
                let u = q(&unit);
                let script = match want.as_str() {
                    "Automatic" => format!("systemctl unmask {u}; systemctl enable {u}"),
                    "Manual" => format!("systemctl unmask {u}; systemctl disable {u}"),
                    _ => format!("systemctl disable {u}; systemctl mask {u}"),
                };
                let rl = reload.clone();
                elevate_sh(&script, move |ok, out| { report("The startup type could not be changed.", ok, &out); if let Some(r) = rl.borrow().clone() { r(); } });
            }
            if r == gtk::ResponseType::Apply { d.set_response_sensitive(gtk::ResponseType::Apply, false); return; }
        }
        d.close();
    });
    d.show_all();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn show() {
        let t = "Id=sshd.service\nDescription=OpenSSH Daemon\nActiveState=inactive\nSubState=dead\nUnitFileState=disabled\nUser=\nExecStart={ path=/usr/bin/sshd ; argv[]=/usr/bin/sshd -D ; ignore_errors=no }\nCanStart=yes\n\nId=foo.socket\n";
        let v = parse_show(t);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].exec, "/usr/bin/sshd -D");
        assert_eq!(v[0].startup(), "Manual");
        assert_eq!(v[0].logon(), "Local System");
        assert!(v[0].can_start);
    }
}
