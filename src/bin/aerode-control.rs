//! Control Panel – Windows 7 Control Panel remake backed by Linux tools.
//!   aerode-control [page]   pages: system display personalization sound datetime power mouse keyboard
//!                          programs users network defaults devices region access startup services
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use aerode::{config, sounds, theme, util};

const CSS: &str = r#"
.cp-main { background-color: #ffffff; }
.cp-title { font-size: 12pt; color: #1e3287; }
.cp-cat { color: #157b14; font-size: 11pt; background: none; border: none; box-shadow: none; padding: 0; }
.cp-cat:hover { color: #1fa41e; text-decoration: underline; }
button.cp-link { color: #0066cc; background: none; border: none; box-shadow: none; padding: 1px 0; min-height: 0; }
button.cp-link:hover { color: #3399ff; }
button.cp-link label { color: #0066cc; }
.cp-sidebar { background-image: linear-gradient(to bottom, #e6f0fb, #cfdff4 60%, #e7eff9); border-right: 1px solid #c4d4ea; padding: 14px 10px; }
.cp-sidebar label.side-h { color: #1e3287; }
.cp-header { padding: 4px 6px 5px 6px; }
.cp-addr { background-color: rgba(255,255,255,0.92); border: 1px solid rgba(0,0,0,0.55); border-radius: 2px; min-height: 22px; padding: 0 6px; }
.cp-addr label { color: #000; }
.cp-section { color: #1e3287; font-size: 10pt; }
"#;

struct Cp {
    win: gtk::ApplicationWindow,
    side: gtk::Box,
    body: gtk::Box,
    crumb: gtk::Label,
    history: RefCell<Vec<String>>,
}

fn link(text: &str, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.style_context().add_class("cp-link");
    b.set_halign(gtk::Align::Start);
    b.set_relief(gtk::ReliefStyle::None);
    if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_xalign(0.0); l.set_line_wrap(true); }
    b.connect_clicked(move |_| f());
    b
}

fn title(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.style_context().add_class("cp-title");
    l.set_margin_bottom(10);
    l
}

fn section(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.style_context().add_class("cp-section");
    l.set_margin_top(10);
    l
}

fn text(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(80);
    l
}

fn kv_grid(rows: &[(&str, String)]) -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(24);
    g.set_margin_start(16);
    for (i, (k, v)) in rows.iter().enumerate() {
        let a = gtk::Label::new(Some(k));
        a.set_xalign(0.0);
        a.style_context().add_class("dim-label");
        let b = gtk::Label::new(Some(v));
        b.set_xalign(0.0);
        b.set_selectable(true);
        b.set_line_wrap(true);
        g.attach(&a, 0, i as i32, 1, 1);
        g.attach(&b, 1, i as i32, 1, 1);
    }
    g
}

fn out(argv: &[&str]) -> String { util::output(argv).unwrap_or_default() }

/// Win7 "Text Services and Input Languages": default input language, installed keyboards
/// (Add / Remove / Move Up / Move Down), the hot key between input languages, Apply.
fn input_languages() -> gtk::Widget {
    use aerode::kbdlayout::{self, Layout};
    let (cur, opts) = kbdlayout::current();
    let list: Rc<RefCell<Vec<Layout>>> = Rc::new(RefCell::new(cur));
    let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
    v.set_margin_start(12);
    let name = |l: &Layout| {
        let (_, lang) = kbdlayout::language(l);
        format!("{lang} - {}", kbdlayout::describe(&l.layout, &l.variant))
    };
    // default input language = the first layout
    let defl = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    defl.pack_start(&gtk::Label::new(Some("Default input language:")), false, false, 0);
    let default_combo = gtk::ComboBoxText::new();
    defl.pack_start(&default_combo, true, true, 0);
    v.pack_start(&defl, false, false, 0);
    v.pack_start(&text("Installed services – the keyboards you can switch between:"), false, false, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lb = gtk::ListBox::new();
    lb.style_context().add_class("frame");
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_size_request(380, 130);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&lb);
    row.pack_start(&sw, true, true, 0);
    let btns = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (add, remove, up, down) = (gtk::Button::with_mnemonic("A_dd..."), gtk::Button::with_mnemonic("_Remove"), gtk::Button::with_mnemonic("Move _Up"), gtk::Button::with_mnemonic("Move Dow_n"));
    for b in [&add, &remove, &up, &down] { btns.pack_start(b, false, false, 0); }
    row.pack_start(&btns, false, false, 0);
    v.pack_start(&row, false, false, 0);
    let hk = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    hk.pack_start(&gtk::Label::new(Some("Between input languages:")), false, false, 0);
    let hot = gtk::ComboBoxText::new();
    let cur_hot = kbdlayout::hotkey(&opts);
    for (label, opt) in kbdlayout::HOTKEYS { hot.append(Some(opt), label); }
    if !hot.set_active_id(Some(&cur_hot)) { if cur_hot.is_empty() { hot.set_active_id(Some("")); } else { hot.append(Some(&cur_hot), &cur_hot); hot.set_active_id(Some(&cur_hot)); } }
    hk.pack_start(&hot, false, false, 0);
    v.pack_start(&hk, false, false, 0);
    let apply = gtk::Button::with_mnemonic("_Apply");
    apply.set_halign(gtk::Align::End);
    v.pack_start(&apply, false, false, 0);

    let refresh: Rc<dyn Fn()> = {
        let (list, lb, default_combo) = (list.clone(), lb.clone(), default_combo.clone());
        Rc::new(move || {
            for c in lb.children() { lb.remove(&c); }
            default_combo.remove_all();
            for l in list.borrow().iter() {
                let (abbr, _) = kbdlayout::language(l);
                let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let a = gtk::Label::new(Some(&abbr));
                a.style_context().add_class("dim-label");
                a.set_width_chars(3);
                r.pack_start(&a, false, false, 4);
                let t = gtk::Label::new(Some(&name(l)));
                t.set_xalign(0.0);
                r.pack_start(&t, true, true, 0);
                lb.add(&r);
                default_combo.append_text(&kbdlayout::language(l).1);
            }
            default_combo.set_active(Some(0));
            lb.show_all();
        })
    };
    refresh();
    let selected = { let lb = lb.clone(); move || lb.selected_row().map(|r| r.index() as usize) };
    { let (list, refresh, lb) = (list.clone(), refresh.clone(), lb.clone()); default_combo.connect_changed(move |c| {
        let Some(i) = c.active().map(|i| i as usize) else { return };
        if i == 0 { return; }
        { let mut l = list.borrow_mut(); if i < l.len() { let x = l.remove(i); l.insert(0, x); } }
        refresh();
        if let Some(r) = lb.row_at_index(0) { lb.select_row(Some(&r)); }
    }); }
    { let (list, refresh, sel) = (list.clone(), refresh.clone(), selected.clone()); remove.connect_clicked(move |_| {
        if let Some(i) = sel() { let mut l = list.borrow_mut(); if l.len() > 1 { l.remove(i); } }
        refresh();
    }); }
    for (b, dir) in [(&up, -1i32), (&down, 1)] {
        let (list, refresh, sel, lb) = (list.clone(), refresh.clone(), selected.clone(), lb.clone());
        b.connect_clicked(move |_| {
            let Some(i) = sel() else { return };
            let j = i as i32 + dir;
            { let mut l = list.borrow_mut(); if j < 0 || j as usize >= l.len() { return; } l.swap(i, j as usize); }
            refresh();
            if let Some(r) = lb.row_at_index(j) { lb.select_row(Some(&r)); }
        });
    }
    { let (list, refresh) = (list.clone(), refresh.clone()); add.connect_clicked(move |b| {
        if let Some(l) = add_input_language(b.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()).as_ref()) {
            if !list.borrow().contains(&l) { list.borrow_mut().push(l); }
            refresh();
        }
    }); }
    { let (list, hot) = (list.clone(), hot.clone()); apply.connect_clicked(move |_| {
        kbdlayout::apply(&list.borrow(), hot.active_id().as_deref().unwrap_or(""));
    }); }
    v.upcast()
}

/// "Add Input Language": every XKB layout with its variants, searchable.
fn add_input_language(parent: Option<&gtk::Window>) -> Option<aerode::kbdlayout::Layout> {
    let (layouts, variants) = aerode::kbdlayout::catalog();
    let d = gtk::Dialog::with_buttons(Some("Add Input Language"), parent, gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(420, 460);
    let c = d.content_area();
    c.set_spacing(6);
    c.set_border_width(10);
    c.pack_start(&text("Select the language and keyboard layout to add."), false, false, 0);
    let search = gtk::SearchEntry::new();
    c.pack_start(&search, false, false, 0);
    // columns: description, layout, variant
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let mut sorted = layouts.clone();
    sorted.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    for (code, desc) in &sorted {
        let it = store.insert_with_values(None, None, &[(0, desc), (1, code), (2, &String::new())]);
        for (_, var, vdesc) in variants.iter().filter(|(l, _, _)| l == code) {
            store.insert_with_values(Some(&it), None, &[(0, vdesc), (1, code), (2, var)]);
        }
    }
    let filter = gtk::TreeModelFilter::new(&store, None);
    {
        let search = search.clone();
        filter.set_visible_func(move |m, it| {
            let q = search.text().to_lowercase();
            if q.is_empty() { return true; }
            let hit = |it: &gtk::TreeIter| m.value(it, 0).get::<String>().unwrap_or_default().to_lowercase().contains(&q)
                || m.value(it, 1).get::<String>().unwrap_or_default().to_lowercase() == q;
            if hit(it) { return true; }
            // keep a layout row when one of its variants matches
            if let Some(ch) = m.iter_children(Some(it)) { loop { if hit(&ch) { return true; } if !m.iter_next(&ch) { break; } } }
            false
        });
    }
    { let f = filter.clone(); search.connect_search_changed(move |_| f.refilter()); }
    let tv = gtk::TreeView::with_model(&filter);
    tv.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let cell = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &cell, true);
    TreeViewColumnExt::add_attribute(&col, &cell, "text", 0);
    tv.append_column(&col);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&tv);
    c.pack_start(&sw, true, true, 0);
    { let d = d.clone(); tv.connect_row_activated(move |_, _, _| d.response(gtk::ResponseType::Ok)); }
    d.show_all();
    let r = d.run();
    let pick = tv.selection().selected().map(|(m, it)| aerode::kbdlayout::Layout {
        layout: m.value(&it, 1).get::<String>().unwrap_or_default(),
        variant: m.value(&it, 2).get::<String>().unwrap_or_default(),
    });
    d.close();
    if r == gtk::ResponseType::Ok { pick.filter(|l| !l.layout.is_empty()) } else { None }
}

/// Run a privileged command through polkit (shows our UAC prompt).
fn admin(argv: &[&str]) -> bool {
    let mut a = vec!["pkexec"];
    a.extend_from_slice(argv);
    std::process::Command::new(a[0]).args(&a[1..]).status().map(|s| s.success()).unwrap_or(false)
}

impl Cp {
    fn open(self: &Rc<Self>, page: &str) {
        for c in self.body.children() { self.body.remove(&c); }
        for c in self.side.children() { self.side.remove(&c); }
        self.history.borrow_mut().push(page.to_string());
        let (crumb, w): (&str, gtk::Widget) = match page {
            "system" => ("System and Security ▸ System", self.page_system()),
            "display" => ("Appearance and Personalization ▸ Display ▸ Screen Resolution", self.page_display()),
            "personalization" => { util::spawn(&["aerode", "personalize"]); ("Appearance and Personalization", self.page_category("appearance")) }
            "sound" => ("Hardware and Sound ▸ Sound", self.page_sound()),
            "datetime" => ("Clock, Language, and Region ▸ Date and Time", self.page_datetime()),
            "power" => ("Hardware and Sound ▸ Power Options", self.page_power()),
            "mouse" => ("Hardware and Sound ▸ Mouse", self.page_mouse()),
            "keyboard" => ("Hardware and Sound ▸ Keyboard", self.page_keyboard()),
            "programs" => ("Programs ▸ Programs and Features", self.page_programs()),
            "users" => ("User Accounts and Family Safety ▸ User Accounts", self.page_users()),
            "network" => ("Network and Internet ▸ Network and Sharing Center", self.page_network()),
            "defaults" => ("Programs ▸ Default Programs", self.page_defaults()),
            "devices" => ("Hardware and Sound ▸ Devices and Printers", self.page_devices()),
            "region" => ("Clock, Language, and Region ▸ Region and Language", self.page_region()),
            "access" => ("Ease of Access ▸ Ease of Access Center", self.page_access()),
            "startup" => ("System and Security ▸ Startup Programs", self.page_startup()),
            "services" => ("System and Security ▸ Administrative Tools ▸ Services", self.page_services()),
            c if c.starts_with("cat:") => (match &c[4..] { "system" => "System and Security", "network" => "Network and Internet", "hardware" => "Hardware and Sound", "programs" => "Programs", "users" => "User Accounts and Family Safety", "appearance" => "Appearance and Personalization", "clock" => "Clock, Language, and Region", _ => "Ease of Access" }, self.page_category(&c[4..])),
            _ => ("All Control Panel Items", self.page_home()),
        };
        self.crumb.set_text(&format!("Control Panel ▸ {crumb}"));
        self.win.set_title(crumb.rsplit(" ▸ ").next().unwrap_or("Control Panel"));
        if page.is_empty() || page == "home" { self.win.set_title("Control Panel"); self.crumb.set_text("Control Panel ▸"); }
        // the left task pane (Win7 shows "Control Panel Home" on every sub page)
        if !(page.is_empty() || page == "home") {
            let me = self.clone();
            self.side.pack_start(&link("Control Panel Home", move || me.open("home")), false, false, 0);
            self.side.show_all();
            self.side.set_visible(true);
        } else { self.side.set_visible(false); }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let pad = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pad.set_border_width(18);
        pad.pack_start(&w, true, true, 0);
        sw.add(&pad);
        self.body.pack_start(&sw, true, true, 0);
        self.body.show_all();
    }

    fn side_link(self: &Rc<Self>, t: &str, page: &'static str) {
        let me = self.clone();
        self.side.pack_start(&link(t, move || me.open(page)), false, false, 2);
    }

    // ----------------------------------------------------------------- home
    fn page_home(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.pack_start(&title("Adjust your computer's settings"), false, false, 0);
        top.pack_end(&gtk::Label::new(Some("View by:  Category")), false, false, 0);
        v.pack_start(&top, false, false, 0);
        let grid = gtk::Grid::new();
        grid.set_column_spacing(40);
        grid.set_row_spacing(18);
        type L = (&'static str, &'static str);
        let cats: [(&str, &str, &str, &[L]); 8] = [
            ("System and Security", "security-high", "system", &[("Review your computer's status", "system"), ("Startup programs", "startup"), ("Find and fix problems", "services")]),
            ("User Accounts and Family Safety", "user-home", "users", &[("Add or remove user accounts", "users"), ("Change your account picture", "users")]),
            ("Network and Internet", "network-workgroup", "network", &[("View network status and tasks", "network"), ("Choose homegroup and sharing options", "network")]),
            ("Appearance and Personalization", "preferences-desktop-wallpaper", "appearance", &[("Change the theme", "personalization"), ("Change desktop background", "personalization"), ("Adjust screen resolution", "display")]),
            ("Hardware and Sound", "printer", "hardware", &[("View devices and printers", "devices"), ("Add a device", "devices")]),
            ("Clock, Language, and Region", "preferences-system-time", "clock", &[("Change keyboards or other input methods", "region")]),
            ("Programs", "system-software-install", "programs", &[("Uninstall a program", "programs")]),
            ("Ease of Access", "preferences-desktop-accessibility", "access", &[("Let Windows suggest settings", "access"), ("Optimize visual display", "access")]),
        ];
        for (i, (name, icon, cat, links)) in cats.iter().enumerate() {
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            h.pack_start(&util::image(icon, 48), false, false, 0);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let t = gtk::Button::with_label(name);
            t.style_context().add_class("cp-cat");
            t.set_halign(gtk::Align::Start);
            t.set_relief(gtk::ReliefStyle::None);
            let (me, c) = (self.clone(), format!("cat:{cat}"));
            t.connect_clicked(move |_| me.open(&c));
            col.pack_start(&t, false, false, 0);
            for (l, p) in links.iter() { let (me, p) = (self.clone(), *p); col.pack_start(&link(l, move || me.open(p)), false, false, 0); }
            h.pack_start(&col, false, false, 0);
            grid.attach(&h, (i % 2) as i32, (i / 2) as i32, 1, 1);
        }
        v.pack_start(&grid, false, false, 0);
        v.upcast()
    }

    fn page_category(self: &Rc<Self>, cat: &str) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 10);
        type L = (&'static str, &'static str);
        let items: Vec<(&str, &str, &[L])> = match cat {
            "system" => vec![("System", "computer", &[("View amount of RAM and processor speed", "system"), ("See the name of this computer", "system")]),
                ("Power Options", "preferences-system-power", &[("Change battery settings", "power"), ("Change what the power buttons do", "power")]),
                ("Administrative Tools", "applications-system", &[("Startup programs", "startup"), ("View services", "services")])],
            "network" => vec![("Network and Sharing Center", "network-workgroup", &[("View network status and tasks", "network"), ("Connect to a network", "network")])],
            "hardware" => vec![("Devices and Printers", "printer", &[("Add a device", "devices"), ("Mouse", "mouse"), ("Keyboard", "keyboard")]),
                ("Sound", "audio-volume-high", &[("Adjust system volume", "sound"), ("Change system sounds", "sound"), ("Manage audio devices", "sound")]),
                ("Power Options", "preferences-system-power", &[("Change power-saving settings", "power")]),
                ("Display", "preferences-desktop-display", &[("Adjust screen resolution", "display")])],
            "programs" => vec![("Programs and Features", "system-software-install", &[("Uninstall a program", "programs")]),
                ("Default Programs", "preferences-desktop-default-applications", &[("Change default settings for media or devices", "defaults"), ("Set your default programs", "defaults")])],
            "users" => vec![("User Accounts", "user-home", &[("Change account picture", "users"), ("Change your password", "users")])],
            "appearance" => vec![("Personalization", "preferences-desktop-wallpaper", &[("Change the theme", "personalization"), ("Change desktop background", "personalization")]),
                ("Display", "preferences-desktop-display", &[("Adjust screen resolution", "display")])],
            "clock" => vec![("Date and Time", "preferences-system-time", &[("Set the time and date", "datetime"), ("Change the time zone", "datetime")]),
                ("Region and Language", "preferences-desktop-keyboard", &[("Change keyboards or other input methods", "region")])],
            _ => vec![("Ease of Access Center", "preferences-desktop-accessibility", &[("Optimize visual display", "access"), ("Change how your keyboard works", "access")])],
        };
        for (name, icon, links) in items {
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            h.pack_start(&util::image(icon, 48), false, false, 0);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let t = gtk::Label::new(Some(name));
            t.set_xalign(0.0);
            t.style_context().add_class("cp-cat");
            col.pack_start(&t, false, false, 0);
            for (l, p) in links.iter() { let (me, p) = (self.clone(), *p); col.pack_start(&link(l, move || me.open(p)), false, false, 0); }
            h.pack_start(&col, false, false, 0);
            v.pack_start(&h, false, false, 0);
        }
        v.upcast()
    }

    // ----------------------------------------------------------------- system
    fn page_system(self: &Rc<Self>) -> gtk::Widget {
        self.side_link("Device Manager", "devices");
        self.side_link("Startup programs", "startup");
        self.side_link("Services", "services");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("View basic information about your computer"), false, false, 0);
        let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let get = |k: &str| os.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
        v.pack_start(&section("Edition"), false, false, 0);
        v.pack_start(&kv_grid(&[(&get("PRETTY_NAME"), String::new()), ("Kernel", out(&["uname", "-r"]).trim().to_string()), ("Desktop", "Aerode".into())]), false, false, 0);
        let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))).unwrap_or_default();
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next()?.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0);
        v.pack_start(&section("System"), false, false, 0);
        v.pack_start(&kv_grid(&[
            ("Processor:", format!("{cpu}  ({} cores)", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))),
            ("Installed memory (RAM):", format!("{:.2} GB", mem as f64 / 1048576.0)),
            ("System type:", format!("{} Operating System", if cfg!(target_pointer_width = "64") { "64-bit" } else { "32-bit" })),
            ("Graphics:", out(&["sh", "-c", "lspci 2>/dev/null | grep -iE 'vga|3d' | cut -d: -f3 | head -1"]).trim().to_string()),
        ]), false, false, 0);
        v.pack_start(&section("Computer name, domain, and workgroup settings"), false, false, 0);
        let name = glib::host_name().to_string();
        v.pack_start(&kv_grid(&[("Computer name:", name.clone()), ("Uptime:", out(&["uptime", "-p"]).trim().to_string())]), false, false, 0);
        let me = self.clone();
        v.pack_start(&link("Change settings", move || {
            if let Some(n) = aerode::desktop::prompt("Computer Name", "Computer name:", &glib::host_name()) {
                if admin(&["hostnamectl", "set-hostname", &n]) { me.open("system"); }
            }
        }), false, false, 0);
        v.upcast()
    }

    fn page_startup(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("Choose which programs start when you log on"), false, false, 0);
        let dir = glib::user_config_dir().join("autostart");
        let mut seen = std::collections::BTreeMap::new();
        for d in glib::system_config_dirs().iter().map(|d| d.join("autostart")).chain(std::iter::once(dir.clone())) {
            if let Ok(rd) = std::fs::read_dir(&d) { for e in rd.flatten() { seen.insert(e.file_name().to_string_lossy().into_owned(), e.path()); } }
        }
        for (name, path) in seen {
            let Some(app) = gio::DesktopAppInfo::from_filename(&path) else { continue };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let c = gtk::CheckButton::new();
            c.set_active(!app.is_hidden() && !(app.has_key("X-GNOME-Autostart-enabled") && !app.boolean("X-GNOME-Autostart-enabled")));
            let (dir, name2, p2) = (dir.clone(), name.clone(), path.clone());
            c.connect_toggled(move |c| {
                let _ = std::fs::create_dir_all(&dir);
                let target = dir.join(&name2);
                let mut content = std::fs::read_to_string(&p2).unwrap_or_default();
                content = content.lines().filter(|l| !l.starts_with("Hidden=")).collect::<Vec<_>>().join("\n");
                content.push_str(&format!("\nHidden={}\n", !c.is_active()));
                let _ = std::fs::write(target, content);
            });
            row.pack_start(&c, false, false, 0);
            row.pack_start(&util::app_image(app.upcast_ref(), 16), false, false, 0);
            let l = gtk::Label::new(Some(&format!("{}   —   {}", app.display_name(), app.commandline().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default())));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.pack_start(&l, true, true, 0);
            v.pack_start(&row, false, false, 0);
        }
        v.upcast()
    }

    fn page_services(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("Services"), false, false, 0);
        let store = gtk::ListStore::new(&[glib::Type::STRING; 4]);
        for l in out(&["systemctl", "list-units", "--type=service", "--all", "--no-legend", "--plain", "--no-pager"]).lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 { continue; }
            let it = store.append();
            let desc = f[4..].join(" ");
            store.set(&it, &[(0, &f[0].trim_end_matches(".service")), (1, &desc), (2, &if f[3] == "running" { "Started" } else { "" }), (3, &f[0])]);
        }
        let tv = gtk::TreeView::with_model(&store);
        for (i, t) in ["Name", "Description", "Status"].iter().enumerate() {
            let c = gtk::TreeViewColumn::new();
            c.set_title(t);
            let r = gtk::CellRendererText::new();
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
            c.set_resizable(true);
            c.set_sort_column_id(i as i32);
            tv.append_column(&c);
        }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_min_content_height(380);
        sw.add(&tv);
        v.pack_start(&sw, true, true, 0);
        let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (label, verb) in [("Start", "start"), ("Stop", "stop"), ("Restart", "restart")] {
            let b = gtk::Button::with_label(label);
            let (tv, me) = (tv.clone(), self.clone());
            b.connect_clicked(move |_| {
                if let Some((m, it)) = tv.selection().selected() {
                    let unit: String = m.value(&it, 3).get().unwrap_or_default();
                    if admin(&["systemctl", verb, &unit]) { me.open("services"); }
                }
            });
            bb.pack_start(&b, false, false, 0);
        }
        v.pack_start(&bb, false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- display
    fn page_display(self: &Rc<Self>) -> gtk::Widget {
        self.side_link("Personalization", "personalization");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Change the appearance of your displays"), false, false, 0);
        // parse xrandr
        let xr = out(&["xrandr", "--query"]);
        // (name, primary, [(mode, [rates])], current mode, current rate, active)
        type Out = (String, bool, Vec<(String, Vec<String>)>, String, String, bool);
        let mut outputs: Vec<Out> = vec![];
        for l in xr.lines() {
            if !l.starts_with(' ') && l.contains(" connected") {
                let name = l.split_whitespace().next().unwrap_or("").to_string();
                let geom = l.split_whitespace().find(|w| w.contains('x') && w.contains('+'));
                let cur = geom.map(|w| w.split('+').next().unwrap().to_string()).unwrap_or_default();
                outputs.push((name, l.contains("primary"), vec![], cur, String::new(), geom.is_some()));
            } else if l.starts_with("   ") {
                if let Some(o) = outputs.last_mut() {
                    let mut it = l.split_whitespace();
                    let Some(mode) = it.next() else { continue };
                    let mut rates = vec![];
                    for r in it {
                        let clean = r.trim_end_matches(['*', '+', ' ']).to_string();
                        if clean.is_empty() { continue; }
                        if r.contains('*') { o.4 = clean.clone(); }
                        if !rates.contains(&clean) { rates.push(clean); }
                    }
                    // highest refresh first, like the Win7 list
                    rates.sort_by(|a, b| b.parse::<f64>().unwrap_or(0.0).partial_cmp(&a.parse::<f64>().unwrap_or(0.0)).unwrap());
                    match o.2.iter_mut().find(|(m, _)| m == mode) { Some(e) => { for r in rates { if !e.1.contains(&r) { e.1.push(r); } } } None => o.2.push((mode.to_string(), rates)) }
                }
            }
        }
        if outputs.is_empty() { v.pack_start(&text("No displays found (xrandr is not available)."), false, false, 0); return v.upcast(); }
        let g = gtk::Grid::new();
        g.set_row_spacing(8);
        g.set_column_spacing(12);
        let disp = gtk::ComboBoxText::new();
        for (i, o) in outputs.iter().enumerate() { disp.append(Some(&o.0), &format!("{}. {}{}", i + 1, o.0, if o.1 { " (primary)" } else { "" })); }
        disp.set_active(Some(0));
        let res = gtk::ComboBoxText::new();
        let rate = gtk::ComboBoxText::new();
        let orient = gtk::ComboBoxText::new();
        for (id, l) in [("normal", "Landscape"), ("left", "Portrait"), ("inverted", "Landscape (flipped)"), ("right", "Portrait (flipped)")] { orient.append(Some(id), l); }
        orient.set_active_id(Some("normal"));
        let multi = gtk::ComboBoxText::new();
        for (id, l) in [("extend", "Extend these displays"), ("dup", "Duplicate these displays"), ("only", "Show desktop only on this display")] { multi.append(Some(id), l); }
        multi.set_active_id(Some("extend"));
        let fill_rates = {
            let (rate, res, disp, outputs) = (rate.clone(), res.clone(), disp.clone(), outputs.clone());
            move || {
                rate.remove_all();
                let (Some(name), Some(mode)) = (disp.active_id(), res.active_id()) else { return };
                let Some(o) = outputs.iter().find(|o| o.0 == name.as_str()) else { return };
                let rates = o.2.iter().find(|(m, _)| *m == mode.as_str()).map(|(_, r)| r.clone()).unwrap_or_default();
                for r in &rates { rate.append(Some(r), &format!("{} Hertz", r.parse::<f64>().map(|v| v.round() as i64).unwrap_or(0))); }
                // keep the current rate if this is the current mode, otherwise pick the highest
                if mode.as_str() == o.3 && rates.contains(&o.4) { rate.set_active_id(Some(&o.4)); } else { rate.set_active(Some(0)); }
            }
        };
        let fill_res = {
            let (res, outputs) = (res.clone(), outputs.clone());
            move |name: &str| {
                res.remove_all();
                if let Some(o) = outputs.iter().find(|o| o.0 == name) {
                    for (i, (m, _)) in o.2.iter().enumerate() { res.append(Some(m), &format!("{m}{}", if i == 0 { " (Recommended)" } else { "" })); }
                    res.set_active_id(Some(&o.3));
                }
            }
        };
        fill_res(&outputs[0].0);
        fill_rates();
        { let (f, fr) = (fill_res.clone(), fill_rates.clone()); disp.connect_changed(move |d| if let Some(id) = d.active_id() { f(&id); fr(); }); }
        { let fr = fill_rates.clone(); res.connect_changed(move |_| fr()); }
        for (i, (l, w)) in [("Display:", disp.clone().upcast::<gtk::Widget>()), ("Resolution:", res.clone().upcast()), ("Screen refresh rate:", rate.clone().upcast()), ("Orientation:", orient.clone().upcast()), ("Multiple displays:", multi.clone().upcast())].into_iter().enumerate() {
            let lab = gtk::Label::new(Some(l));
            lab.set_xalign(1.0);
            g.attach(&lab, 0, i as i32, 1, 1);
            g.attach(&w, 1, i as i32, 1, 1);
        }
        v.pack_start(&g, false, false, 0);
        let me = self.clone();
        v.pack_start(&link("Make text and other items larger or smaller", move || me.open("access")), false, false, 0);
        // desktop composition (picom): Aero glass, shadows, vsync
        v.pack_start(&section("Desktop composition"), false, false, 0);
        let comp = gtk::CheckButton::with_label("Enable desktop composition (Aero glass, shadows, animations)");
        comp.set_active(!config::conf_dir().join("no-compositor").exists());
        comp.connect_toggled(|c| {
            let f = config::conf_dir().join("no-compositor");
            let _ = std::fs::create_dir_all(config::conf_dir());
            if c.is_active() { let _ = std::fs::remove_file(&f); } else { let _ = std::fs::write(&f, ""); let _ = std::process::Command::new("pkill").args(["-x", "picom"]).status(); }
            if c.is_active() { util::spawn(&["aerode", "restart"]); }
        });
        v.pack_start(&comp, false, false, 0);
        let vs = gtk::CheckButton::with_label("Sync to the display's refresh rate (VSync) – turn off if you are capped at 60 FPS");
        let user_conf = config::conf_dir().join("picom.conf");
        vs.set_active(!std::fs::read_to_string(&user_conf).map(|s| s.contains("vsync = false")).unwrap_or(false));
        vs.connect_toggled(move |c| {
            let base = std::fs::read_to_string("/usr/share/aerode/picom.conf").unwrap_or_default();
            let txt = base.replace("vsync = true;", if c.is_active() { "vsync = true;" } else { "vsync = false;" });
            let _ = std::fs::create_dir_all(config::conf_dir());
            let _ = std::fs::write(config::conf_dir().join("picom.conf"), txt);
            util::spawn(&["aerode", "restart"]);
        });
        v.pack_start(&vs, false, false, 0);
        let apply = gtk::Button::with_label("Apply");
        apply.set_halign(gtk::Align::End);
        let (w, outs) = (self.win.clone(), outputs.clone());
        apply.connect_clicked(move |_| {
            let (Some(o), Some(m)) = (disp.active_id(), res.active_id()) else { return };
            let this = outs.iter().find(|x| x.0 == o.as_str()).cloned();
            let (old, old_rate) = this.as_ref().map(|x| (x.3.clone(), x.4.clone())).unwrap_or_default();
            let rot = orient.active_id().map(|s| s.to_string()).unwrap_or_else(|| "normal".into());
            let r = rate.active_id().map(|s| s.to_string()).unwrap_or_default();
            let mut cmd = vec!["xrandr".to_string(), "--output".into(), o.to_string(), "--mode".into(), m.to_string()];
            // always pass the rate: without it xrandr may fall back to 60 Hz
            if !r.is_empty() { cmd.extend(["--rate".into(), r.clone()]); }
            cmd.extend(["--rotate".into(), rot]);
            let others: Vec<&Out> = outs.iter().filter(|x| x.0 != o.as_str()).collect();
            match multi.active_id().as_deref() {
                Some("dup") => for n in &others { cmd.extend(["--output".into(), n.0.clone(), "--same-as".into(), o.to_string()]); },
                Some("only") => for n in &others { cmd.extend(["--output".into(), n.0.clone(), "--off".into()]); },
                _ => if let Some(n) = others.first() {
                    // leave the other monitor's own mode/rate alone unless it was off
                    cmd.extend(["--output".into(), n.0.clone()]);
                    if !n.5 { cmd.push("--auto".into()); }
                    cmd.extend(["--right-of".into(), o.to_string()]);
                },
            }
            let refs: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
            util::output(&refs);
            // Win7: "Keep these display settings?" with a 15 s automatic revert
            let d = gtk::MessageDialog::new(Some(&w), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::None, "Do you want to keep these display settings?");
            d.set_title("Display Settings");
            d.add_button("Keep changes", gtk::ResponseType::Yes);
            d.add_button("Revert", gtk::ResponseType::No);
            let secs = Rc::new(std::cell::Cell::new(15));
            d.set_secondary_text(Some("Reverting to previous display settings in 15 seconds."));
            { let (d2, s) = (d.clone(), secs.clone()); glib::timeout_add_seconds_local(1, move || {
                let n = s.get() - 1; s.set(n);
                if n <= 0 { d2.response(gtk::ResponseType::No); return glib::ControlFlow::Break; }
                d2.set_secondary_text(Some(&format!("Reverting to previous display settings in {n} seconds.")));
                if d2.is_visible() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
            }); }
            let r = d.run();
            d.close();
            if r != gtk::ResponseType::Yes && !old.is_empty() {
                let mut back = vec!["xrandr", "--output", &o, "--mode", &old];
                if !old_rate.is_empty() { back.extend(["--rate", old_rate.as_str()]); }
                back.extend(["--rotate", "normal"]);
                util::output(&back);
            }
            else {
                // remember for next login
                let _ = std::fs::create_dir_all(config::conf_dir());
                let _ = std::fs::write(config::conf_dir().join("displays.sh"), format!("#!/bin/sh\n{}\n", cmd.join(" ")));
            }
        });
        v.pack_start(&apply, false, false, 8);
        v.upcast()
    }

    // ----------------------------------------------------------------- sound
    fn page_sound(self: &Rc<Self>) -> gtk::Widget {
        let nb = gtk::Notebook::new();
        // Playback / Recording devices
        for (label, kind) in [("Playback", "sinks"), ("Recording", "sources")] {
            let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
            v.set_border_width(10);
            v.pack_start(&text(&format!("Select a {} device below to modify its settings:", if kind == "sinks" { "playback" } else { "recording" })), false, false, 0);
            let list = gtk::ListBox::new();
            let def = out(&["pactl", if kind == "sinks" { "get-default-sink" } else { "get-default-source" }]).trim().to_string();
            let mut name = String::new();
            for l in out(&["pactl", "list", kind]).lines() {
                let l = l.trim();
                if let Some(n) = l.strip_prefix("Name: ") { name = n.to_string(); }
                if let Some(d) = l.strip_prefix("Description: ") {
                    if kind == "sources" && name.ends_with(".monitor") { continue; }
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                    row.set_border_width(6);
                    row.pack_start(&util::image(if kind == "sinks" { "audio-volume-high" } else { "audio-input-microphone" }, 32), false, false, 0);
                    let t = gtk::Label::new(None);
                    t.set_markup(&format!("{}\n<span foreground='#6d6d6d'>{}</span>", glib::markup_escape_text(d), if name == def { "Default Device" } else { "Ready" }));
                    t.set_xalign(0.0);
                    row.pack_start(&t, true, true, 0);
                    let b = gtk::Button::with_label("Set Default");
                    let (n, k) = (name.clone(), kind);
                    b.set_sensitive(name != def);
                    b.connect_clicked(move |b| { util::spawn(&["pactl", if k == "sinks" { "set-default-sink" } else { "set-default-source" }, &n]); b.set_sensitive(false); });
                    row.pack_end(&b, false, false, 0);
                    list.add(&row);
                }
            }
            v.pack_start(&list, true, true, 0);
            nb.append_page(&v, Some(&gtk::Label::new(Some(label))));
        }
        // Sounds tab: the Win7 scheme editor
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.set_border_width(10);
        v.pack_start(&text("A sound theme is a set of sounds applied to events in Windows and programs."), false, false, 0);
        let mute = gtk::CheckButton::with_label("Mute all system sounds (\"No Sounds\" scheme)");
        mute.set_active(config::get().sounds_muted);
        mute.connect_toggled(|c| { let m = c.is_active(); config::update(|cf| cf.sounds_muted = m); });
        v.pack_start(&mute, false, false, 0);
        let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
        let fill = {
            let store = store.clone();
            move || {
                store.clear();
                for (i, ev) in sounds::Event::ALL.iter().enumerate() {
                    let f = sounds::file_for(*ev).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).unwrap_or_else(|| "(None)".into());
                    let it = store.append();
                    store.set(&it, &[(0, &ev.label()), (1, &f), (2, &(i as u32))]);
                }
            }
        };
        fill();
        let tv = gtk::TreeView::with_model(&store);
        for (i, t) in ["Program Events", "Sound"].iter().enumerate() {
            let c = gtk::TreeViewColumn::new();
            c.set_title(t);
            let r = gtk::CellRendererText::new();
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
            c.set_resizable(true);
            tv.append_column(&c);
        }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_min_content_height(260);
        sw.add(&tv);
        v.pack_start(&sw, true, true, 0);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let combo = gtk::ComboBoxText::new();
        combo.append(Some(""), "(None)");
        if let Ok(rd) = std::fs::read_dir(config::data_dir().join("sounds")) {
            let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            for n in names { combo.append(Some(&n), &n); }
        }
        let test = gtk::Button::with_label("Test");
        let browse = gtk::Button::with_label("Browse...");
        row.pack_start(&gtk::Label::new(Some("Sounds:")), false, false, 0);
        row.pack_start(&combo, true, true, 0);
        row.pack_start(&test, false, false, 0);
        row.pack_start(&browse, false, false, 0);
        v.pack_start(&row, false, false, 0);
        let sel_ev = Rc::new(std::cell::Cell::new(None::<usize>));
        { let (combo, s) = (combo.clone(), sel_ev.clone()); tv.selection().connect_changed(move |sel| {
            if let Some((m, it)) = sel.selected() {
                let i: u32 = m.value(&it, 2).get().unwrap_or(0);
                s.set(Some(i as usize));
                let f: String = m.value(&it, 1).get().unwrap_or_default();
                combo.set_active_id(Some(if f == "(None)" { "" } else { &f }));
            }
        }); }
        { let (s, f) = (sel_ev.clone(), fill.clone()); combo.connect_changed(move |c| {
            let (Some(i), Some(id)) = (s.get(), c.active_id()) else { return };
            let ev = sounds::Event::ALL[i];
            let cur = sounds::file_for(ev).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).unwrap_or_default();
            if cur == id.as_str() { return; }
            config::update(|cf| { cf.sounds.insert(ev.key().to_string(), id.to_string()); });
            let _ = &f;
        }); }
        { let s = sel_ev.clone(); test.connect_clicked(move |_| if let Some(i) = s.get() { sounds::play(sounds::Event::ALL[i]); }); }
        { let (s, w, f) = (sel_ev.clone(), self.win.clone(), fill.clone()); browse.connect_clicked(move |_| {
            let Some(i) = s.get() else { return };
            let fc = gtk::FileChooserNative::new(Some("Browse for new sound"), Some(&w), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
            let ff = gtk::FileFilter::new(); ff.add_pattern("*.wav"); ff.add_pattern("*.ogg"); ff.add_pattern("*.oga"); ff.set_name(Some("Wave Files (*.wav)")); fc.add_filter(ff);
            if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() {
                let ev = sounds::Event::ALL[i];
                config::update(|cf| { cf.sounds.insert(ev.key().to_string(), p.to_string_lossy().into_owned()); });
                f();
            } }
        }); }
        let reset = gtk::Button::with_label("Restore Windows Default");
        { let f = fill.clone(); reset.connect_clicked(move |_| { config::update(|cf| cf.sounds.clear()); f(); }); }
        reset.set_halign(gtk::Align::Start);
        v.pack_start(&reset, false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Sounds"))));
        nb.upcast()
    }

    // ----------------------------------------------------------------- date/time
    fn page_datetime(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Date and Time"), false, false, 0);
        let now = gtk::Label::new(None);
        now.set_xalign(0.0);
        { let n = now.clone(); let tick = move || { n.set_text(&format!("Date: {}\nTime: {}", chrono::Local::now().format("%A, %B %-d, %Y"), chrono::Local::now().format("%-I:%M:%S %p"))); glib::ControlFlow::Continue }; tick(); glib::timeout_add_seconds_local(1, tick); }
        v.pack_start(&now, false, false, 0);
        let set = gtk::Button::with_label("Change date and time...");
        set.set_halign(gtk::Align::Start);
        let w = self.win.clone();
        set.connect_clicked(move |_| {
            let d = gtk::Dialog::with_buttons(Some("Date and Time Settings"), Some(&w), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
            let cal = gtk::Calendar::new();
            let t = gtk::Entry::new();
            t.set_text(&chrono::Local::now().format("%H:%M:%S").to_string());
            d.content_area().pack_start(&cal, false, false, 6);
            d.content_area().pack_start(&t, false, false, 6);
            d.show_all();
            if d.run() == gtk::ResponseType::Ok {
                let (y, m, day) = cal.date();
                let s = format!("{y:04}-{:02}-{day:02} {}", m + 1, t.text());
                admin(&["timedatectl", "set-ntp", "false"]);
                if !admin(&["timedatectl", "set-time", &s]) { util::error("Unable to set the time", "Make sure the time is in HH:MM:SS format."); }
            }
            d.close();
        });
        v.pack_start(&set, false, false, 0);
        v.pack_start(&section("Time zone"), false, false, 0);
        let cur = out(&["timedatectl", "show", "-p", "Timezone", "--value"]).trim().to_string();
        let tz = gtk::ComboBoxText::with_entry();
        for z in out(&["timedatectl", "list-timezones"]).lines() { tz.append(Some(z), z); }
        tz.set_active_id(Some(&cur));
        let ch = gtk::Button::with_label("Change time zone");
        { let tz = tz.clone(); ch.connect_clicked(move |_| if let Some(z) = tz.active_id() { admin(&["timedatectl", "set-timezone", &z]); }); }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.pack_start(&tz, true, true, 0);
        row.pack_start(&ch, false, false, 0);
        v.pack_start(&row, false, false, 0);
        v.pack_start(&section("Internet Time"), false, false, 0);
        let ntp = gtk::CheckButton::with_label("Synchronize with an Internet time server");
        ntp.set_active(out(&["timedatectl", "show", "-p", "NTP", "--value"]).trim() == "yes");
        ntp.connect_toggled(|c| { admin(&["timedatectl", "set-ntp", if c.is_active() { "true" } else { "false" }]); });
        v.pack_start(&ntp, false, false, 0);
        v.pack_start(&section("Clock"), false, false, 0);
        let h24 = gtk::CheckButton::with_label("Use 24-hour clock on the taskbar");
        h24.set_active(config::get().clock_24h);
        h24.connect_toggled(|c| { let a = c.is_active(); config::update(|cf| cf.clock_24h = a); util::spawn(&["aerode", "reload"]); });
        v.pack_start(&h24, false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- power
    fn page_power(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Select a power plan"), false, false, 0);
        v.pack_start(&text("Power plans can help you maximize your computer's performance or conserve energy."), false, false, 0);
        let cur = aerode::sys::power_profile();
        let mut grp: Option<gtk::RadioButton> = None;
        for (id, name, desc) in [("balanced", "Balanced (recommended)", "Automatically balances performance with energy consumption on capable hardware."),
            ("power-saver", "Power saver", "Saves energy by reducing your computer's performance where possible."),
            ("performance", "High performance", "Favors performance, but may use more energy.")] {
            let r = match &grp { None => gtk::RadioButton::with_label(name), Some(g) => gtk::RadioButton::with_label_from_widget(g, name) };
            if grp.is_none() { grp = Some(r.clone()); }
            r.set_active(cur.as_deref() == Some(id));
            r.set_sensitive(cur.is_some());
            r.connect_toggled(move |r| if r.is_active() { aerode::sys::set_power_profile(id) });
            v.pack_start(&r, false, false, 0);
            let d = text(desc);
            d.set_margin_start(24);
            d.style_context().add_class("dim-label");
            v.pack_start(&d, false, false, 0);
        }
        if cur.is_none() { v.pack_start(&text("Install power-profiles-daemon to switch power plans."), false, false, 0); }
        v.pack_start(&section("Turn off the display"), false, false, 0);
        let combo = gtk::ComboBoxText::new();
        for (secs, l) in [(60, "1 minute"), (300, "5 minutes"), (600, "10 minutes"), (900, "15 minutes"), (1800, "30 minutes"), (3600, "1 hour"), (0, "Never")] { combo.append(Some(&secs.to_string()), l); }
        combo.set_active_id(Some("600"));
        combo.connect_changed(|c| if let Some(s) = c.active_id() {
            let s = s.to_string();
            if s == "0" { util::spawn(&["xset", "-dpms"]); util::spawn(&["xset", "s", "off"]); } else { util::spawn(&["xset", "+dpms", "dpms", &s, &s, &s]); util::spawn(&["xset", "s", &s]); }
            let _ = std::fs::create_dir_all(config::conf_dir());
            let _ = std::fs::write(config::conf_dir().join("display-off"), &s);
        });
        v.pack_start(&combo, false, false, 0);
        if let Some(b) = aerode::sys::brightness() {
            v.pack_start(&section("Screen brightness"), false, false, 0);
            let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
            s.set_value(b as f64);
            s.connect_value_changed(|s| aerode::sys::set_brightness(s.value() as u32));
            v.pack_start(&s, false, false, 0);
        }
        v.upcast()
    }

    // ----------------------------------------------------------------- mouse / keyboard
    fn page_mouse(self: &Rc<Self>) -> gtk::Widget {
        let nb = gtk::Notebook::new();
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.set_border_width(10);
        let sw = gtk::CheckButton::with_label("Switch primary and secondary buttons");
        sw.connect_toggled(|c| { util::spawn(&["sh", "-c", &format!("for id in $(xinput list --id-only 2>/dev/null); do xinput set-prop $id 'libinput Left Handed Enabled' {} 2>/dev/null; done", c.is_active() as u8)]); });
        v.pack_start(&sw, false, false, 0);
        v.pack_start(&section("Double-click speed"), false, false, 0);
        let dc = gtk::Scale::with_range(gtk::Orientation::Horizontal, 200.0, 900.0, 50.0);
        dc.set_inverted(true);
        if let Some(s) = gtk::Settings::default() { dc.set_value(s.gtk_double_click_time() as f64); }
        dc.connect_value_changed(|s| if let Some(st) = gtk::Settings::default() { st.set_gtk_double_click_time(s.value() as i32); });
        v.pack_start(&dc, false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Buttons"))));
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.set_border_width(10);
        v.pack_start(&section("Select a pointer speed"), false, false, 0);
        let sp = gtk::Scale::with_range(gtk::Orientation::Horizontal, -1.0, 1.0, 0.1);
        sp.connect_value_changed(|s| { util::spawn(&["sh", "-c", &format!("for id in $(xinput list --id-only 2>/dev/null); do xinput set-prop $id 'libinput Accel Speed' {:.2} 2>/dev/null; done", s.value())]); });
        v.pack_start(&sp, false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Pointer Options"))));
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.set_border_width(10);
        v.pack_start(&text("Scheme"), false, false, 0);
        let schemes = gtk::ComboBoxText::new();
        for d in [glib::user_data_dir().join("icons"), std::path::PathBuf::from("/usr/share/icons")] {
            if let Ok(rd) = std::fs::read_dir(d) { for e in rd.flatten() { if e.path().join("cursors").is_dir() { let n = e.file_name().to_string_lossy().into_owned(); schemes.append(Some(&n), &if n == "Aerode-cursors" { "Windows Aero (system scheme)".into() } else { n.clone() }); } } }
        }
        schemes.set_active_id(Some(&std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "Aerode-cursors".into())));
        schemes.connect_changed(|c| if let Some(id) = c.active_id() {
            let d = glib::user_data_dir().join("icons/default");
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join("index.theme"), format!("[Icon Theme]\nInherits={id}\n"));
            if let Some(s) = gtk::Settings::default() { s.set_gtk_cursor_theme_name(Some(&id)); }
        });
        v.pack_start(&schemes, false, false, 0);
        v.pack_start(&text("The new pointer scheme fully applies the next time you log on."), false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Pointers"))));
        nb.upcast()
    }

    fn page_keyboard(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Keyboard Properties"), false, false, 0);
        v.pack_start(&section("Repeat delay"), false, false, 0);
        let delay = gtk::Scale::with_range(gtk::Orientation::Horizontal, 150.0, 1000.0, 50.0);
        delay.set_value(500.0);
        delay.set_inverted(true);
        v.pack_start(&delay, false, false, 0);
        v.pack_start(&section("Repeat rate"), false, false, 0);
        let rate = gtk::Scale::with_range(gtk::Orientation::Horizontal, 5.0, 50.0, 1.0);
        rate.set_value(30.0);
        v.pack_start(&rate, false, false, 0);
        let apply = { let (d, r) = (delay.clone(), rate.clone()); move || { util::spawn(&["xset", "r", "rate", &format!("{}", d.value() as u32), &format!("{}", r.value() as u32)]); } };
        { let a = apply.clone(); delay.connect_value_changed(move |_| a()); }
        rate.connect_value_changed(move |_| apply());
        let test = gtk::Entry::new();
        test.set_placeholder_text(Some("Click here and hold down a key to test repeat rate"));
        v.pack_start(&test, false, false, 0);
        let me = self.clone();
        v.pack_start(&link("Change keyboards or other input methods", move || me.open("region")), false, false, 6);
        v.upcast()
    }

    // ----------------------------------------------------------------- programs
    fn page_programs(self: &Rc<Self>) -> gtk::Widget {
        self.side_link("View installed updates", "programs");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("Uninstall or change a program"), false, false, 0);
        v.pack_start(&text("To uninstall a program, select it from the list and then click Uninstall."), false, false, 0);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.style_context().add_class("commandbar");
        let uninstall = gtk::Button::with_label("Uninstall");
        let search = gtk::SearchEntry::new();
        bar.pack_start(&uninstall, false, false, 0);
        bar.pack_end(&search, false, false, 0);
        v.pack_start(&bar, false, false, 0);
        let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U64, glib::Type::STRING]);
        let filter = gtk::TreeModelFilter::new(&store, None);
        { let s = search.clone(); filter.set_visible_func(move |m, it| { let q = s.text().to_lowercase(); q.is_empty() || m.value(it, 0).get::<String>().unwrap_or_default().to_lowercase().contains(&q) || m.value(it, 1).get::<String>().unwrap_or_default().to_lowercase().contains(&q) }); }
        { let f = filter.clone(); search.connect_search_changed(move |_| f.refilter()); }
        let sorted = gtk::TreeModelSort::new(&filter);
        let tv = gtk::TreeView::with_model(&sorted);
        for (i, t, w) in [(0, "Name", 220), (1, "Publisher", 200), (2, "Installed On", 100), (3, "Size", 80), (5, "Version", 110)] {
            let c = gtk::TreeViewColumn::new();
            c.set_title(t);
            let r = gtk::CellRendererText::new();
            if i == 3 { r.set_xalign(1.0); }
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", i);
            c.set_resizable(true);
            c.set_sort_column_id(if i == 3 { 4 } else { i });
            c.set_fixed_width(w);
            c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            tv.append_column(&c);
        }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_min_content_height(380);
        sw.add(&tv);
        v.pack_start(&sw, true, true, 0);
        let status = gtk::Label::new(Some("Loading installed programs..."));
        status.set_xalign(0.0);
        v.pack_start(&status, false, false, 0);
        // pacman -Qei in a thread (explicitly installed packages, like "programs")
        let (tx, rx) = std::sync::mpsc::channel::<Vec<[String; 6]>>();
        std::thread::spawn(move || {
            let txt = util::output(&["pacman", "-Qei"]).unwrap_or_default();
            let mut rows = vec![];
            for block in txt.split("\n\n") {
                let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim_start_matches([' ', ':']).trim().to_string())).unwrap_or_default();
                let name = get("Name");
                if name.is_empty() { continue; }
                let size = get("Installed Size");
                let bytes = { let mut it = size.split_whitespace(); let n: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0); (match it.next() { Some("KiB") => n * 1024.0, Some("MiB") => n * 1048576.0, Some("GiB") => n * 1073741824.0, _ => n }) as u64 };
                let date = get("Install Date");
                let short_date = chrono::DateTime::parse_from_str(&date, "%a %d %b %Y %I:%M:%S %p %Z").map(|d| d.format("%-m/%-d/%Y").to_string()).unwrap_or_else(|_| date.split_whitespace().take(4).collect::<Vec<_>>().join(" "));
                rows.push([name, get("Packager"), short_date, size, bytes.to_string(), get("Version")]);
            }
            let _ = tx.send(rows);
        });
        { let (store, status) = (store.clone(), status.clone()); glib::timeout_add_local(std::time::Duration::from_millis(100), move || match rx.try_recv() {
            Ok(rows) => {
                let total: u64 = rows.iter().map(|r| r[4].parse::<u64>().unwrap_or(0)).sum();
                for r in &rows { let it = store.append(); store.set(&it, &[(0, &r[0]), (1, &r[1]), (2, &r[2]), (3, &r[3]), (4, &r[4].parse::<u64>().unwrap_or(0)), (5, &r[5])]); }
                status.set_text(&format!("Currently installed programs   Total size: {}\n{} programs installed", util::human_size(total), rows.len()));
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => { status.set_text("pacman was not found."); glib::ControlFlow::Break }
        }); }
        { let (tv, me) = (tv.clone(), self.clone()); uninstall.connect_clicked(move |_| {
            let Some((m, it)) = tv.selection().selected() else { return };
            let name: String = m.value(&it, 0).get().unwrap_or_default();
            let d = gtk::MessageDialog::new(Some(&me.win), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo, &format!("Are you sure you want to uninstall {name}?"));
            d.set_title("Programs and Features");
            let r = d.run();
            d.close();
            if r == gtk::ResponseType::Yes {
                if admin(&["pacman", "-Rns", "--noconfirm", &name]) { me.open("programs"); } else { util::error(&format!("{name} could not be uninstalled."), "Another package may depend on it."); }
            }
        }); }
        v.upcast()
    }

    // ----------------------------------------------------------------- users
    fn page_users(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Make changes to your user account"), false, false, 0);
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        let face = config::home().join(".face");
        let pic = gtk::Image::new();
        let set_pic = { let pic = pic.clone(); move |p: &std::path::Path| pic.set_from_pixbuf(gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, 64, 64, false).ok().as_ref()) };
        if face.exists() { set_pic(&face); } else if let Some(a) = config::data_file("avatar.png") { set_pic(&a); }
        h.pack_start(&pic, false, false, 0);
        let real = glib::real_name().to_string_lossy().into_owned();
        let user = glib::user_name().to_string_lossy().into_owned();
        let is_admin = out(&["id", "-nG"]).split_whitespace().any(|g| g == "wheel" || g == "sudo");
        let info = gtk::Label::new(None);
        info.set_markup(&format!("<b>{}</b>\n{}", glib::markup_escape_text(if real.is_empty() || real == "Unknown" { &user } else { &real }), if is_admin { "Administrator" } else { "Standard user" }));
        info.set_xalign(0.0);
        h.pack_start(&info, false, false, 0);
        v.pack_start(&h, false, false, 0);
        v.pack_start(&link("Change your password", || {
            if let Some(t) = util::terminal() { let _ = t; util::spawn(&["sh", "-c", "x-terminal-emulator -e passwd || xterm -e passwd || kgx -- passwd || gnome-terminal -- passwd || konsole -e passwd"]); }
        }), false, false, 0);
        {
            let w = self.win.clone();
            let sp = set_pic.clone();
            v.pack_start(&link("Change your picture", move || {
                let d = gtk::Dialog::with_buttons(Some("Change Your Picture"), Some(&w), gtk::DialogFlags::MODAL, &[("Change Picture", gtk::ResponseType::Ok), ("Browse for more pictures...", gtk::ResponseType::Other(1)), ("Cancel", gtk::ResponseType::Cancel)]);
                let flow = gtk::FlowBox::new();
                flow.set_max_children_per_line(6);
                let mut paths = vec![];
                if let Ok(rd) = std::fs::read_dir(config::data_dir().join("avatars")) { for e in rd.flatten() { paths.push(e.path()); } }
                paths.sort();
                for p in &paths { if let Ok(pb) = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, 64, 64, false) { flow.insert(&gtk::Image::from_pixbuf(Some(&pb)), -1); } }
                d.content_area().pack_start(&flow, true, true, 8);
                d.show_all();
                let r = d.run();
                let chosen = match r {
                    gtk::ResponseType::Ok => flow.selected_children().first().map(|c| paths[c.index() as usize].clone()),
                    gtk::ResponseType::Other(1) => {
                        let fc = gtk::FileChooserNative::new(Some("Browse"), Some(&d), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
                        let f = gtk::FileFilter::new(); f.add_pixbuf_formats(); fc.add_filter(f);
                        if fc.run() == gtk::ResponseType::Accept { fc.filename() } else { None }
                    }
                    _ => None,
                };
                d.close();
                if let Some(p) = chosen {
                    let face = config::home().join(".face");
                    if let Ok(pb) = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&p, 128, 128, false) { let _ = pb.savev(&face, "png", &[]); sp(&face); }
                }
            }), false, false, 0);
        }
        v.pack_start(&link("Change your account name", move || {
            if let Some(n) = aerode::desktop::prompt("Change Your Name", "New account name:", &glib::real_name().to_string_lossy()) { admin(&["chfn", "-f", &n, &glib::user_name().to_string_lossy()]); }
        }), false, false, 0);
        v.pack_start(&section("Other accounts"), false, false, 0);
        let users: Vec<String> = std::fs::read_to_string("/etc/passwd").unwrap_or_default().lines().filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() > 6 && f[2].parse::<u32>().map(|u| (1000..60000).contains(&u)).unwrap_or(false)).then(|| f[0].to_string())
        }).collect();
        for u in users { v.pack_start(&text(&format!("• {u}")), false, false, 0); }
        v.upcast()
    }

    // ----------------------------------------------------------------- network
    fn page_network(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("View your basic network information and set up connections"), false, false, 0);
        let st = aerode::sys::net_status();
        let map = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        for (icon, label) in [("computer", glib::host_name().to_string()), (aerode::sys::net_icon(&st), if st.kind == "none" { "Not connected".to_string() } else { st.name.clone() }), ("web-browser", "Internet".to_string())] {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            b.pack_start(&util::image(icon, 48), false, false, 0);
            b.pack_start(&gtk::Label::new(Some(&label)), false, false, 0);
            map.pack_start(&b, false, false, 0);
            if label != "Internet" { map.pack_start(&gtk::Label::new(Some(if st.kind == "none" && icon != "computer" { "✕" } else { "———" })), false, false, 0); }
        }
        v.pack_start(&map, false, false, 6);
        v.pack_start(&section("View your active networks"), false, false, 0);
        for l in out(&["nmcli", "-t", "-f", "NAME,TYPE,DEVICE", "connection", "show", "--active"]).lines() {
            let f = aerode::sys::nm_split(l);
            if f.len() >= 3 { v.pack_start(&kv_grid(&[(&f[0], format!("Access type: Internet\nConnections: {} ({})", f[2], f[1]))]), false, false, 0); }
        }
        v.pack_start(&section("Change your networking settings"), false, false, 0);
        v.pack_start(&link("Set up a new connection or network", || { util::spawn(&["nm-connection-editor"]); }), false, false, 0);
        v.pack_start(&link("Change adapter settings", || { util::spawn(&["nm-connection-editor"]); }), false, false, 0);
        v.pack_start(&link("Troubleshoot problems", || { util::spawn(&["sh", "-c", "x-terminal-emulator -e sh -c 'nmcli general status; ip addr; ping -c 3 archlinux.org; read x' || xterm -e sh -c 'nmcli general status; ping -c 3 archlinux.org; read x'"]); }), false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- defaults
    fn page_defaults(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Set your default programs"), false, false, 0);
        let g = gtk::Grid::new();
        g.set_row_spacing(8);
        g.set_column_spacing(12);
        for (i, (label, mime)) in [("Web browser", "x-scheme-handler/http"), ("E-mail", "x-scheme-handler/mailto"), ("Music", "audio/mpeg"), ("Video", "video/mp4"),
            ("Photos", "image/jpeg"), ("Text documents", "text/plain"), ("PDF documents", "application/pdf"), ("Folders", "inode/directory")].iter().enumerate() {
            let l = gtk::Label::new(Some(label));
            l.set_xalign(0.0);
            g.attach(&l, 0, i as i32, 1, 1);
            let combo = gtk::AppChooserButton::new(mime);
            combo.set_show_default_item(true);
            let mime = mime.to_string();
            combo.connect_changed(move |c| {
                if let Some(a) = c.app_info() {
                    let _ = a.set_as_default_for_type(&mime);
                    if mime == "x-scheme-handler/http" { let _ = a.set_as_default_for_type("x-scheme-handler/https"); let _ = a.set_as_default_for_type("text/html"); }
                    if mime == "image/jpeg" { for t in ["image/png", "image/gif", "image/bmp", "image/webp"] { let _ = a.set_as_default_for_type(t); } }
                    if mime == "audio/mpeg" { for t in ["audio/flac", "audio/ogg", "audio/x-wav"] { let _ = a.set_as_default_for_type(t); } }
                    if mime == "video/mp4" { for t in ["video/x-matroska", "video/webm", "video/x-msvideo"] { let _ = a.set_as_default_for_type(t); } }
                }
            });
            g.attach(&combo, 1, i as i32, 1, 1);
        }
        v.pack_start(&g, false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- devices
    fn page_devices(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.style_context().add_class("commandbar");
        let add_dev = gtk::Button::with_label("Add a device");
        add_dev.connect_clicked(|_| { if !util::spawn(&["blueman-manager"]) { util::spawn(&["sh", "-c", "x-terminal-emulator -e bluetoothctl || xterm -e bluetoothctl"]); } });
        let add_pr = gtk::Button::with_label("Add a printer");
        add_pr.connect_clicked(|_| { if !util::spawn(&["system-config-printer"]) { util::open_uri("http://localhost:631/admin"); } });
        bar.pack_start(&add_dev, false, false, 0);
        bar.pack_start(&add_pr, false, false, 0);
        v.pack_start(&bar, false, false, 0);
        let group = |t: &str, items: Vec<(String, &str)>| {
            v.pack_start(&section(&format!("{t} ({})", items.len())), false, false, 0);
            let f = gtk::FlowBox::new();
            f.set_selection_mode(gtk::SelectionMode::None);
            f.set_max_children_per_line(6);
            for (name, icon) in items {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
                b.pack_start(&util::image(icon, 48), false, false, 0);
                let l = gtk::Label::new(Some(&name));
                l.set_line_wrap(true);
                l.set_max_width_chars(14);
                l.set_justify(gtk::Justification::Center);
                b.pack_start(&l, false, false, 0);
                f.insert(&b, -1);
            }
            v.pack_start(&f, false, false, 0);
        };
        let mut devices = vec![(glib::host_name().to_string(), "computer")];
        for l in out(&["xrandr", "--query"]).lines().filter(|l| l.contains(" connected")) { devices.push((format!("Display {}", l.split_whitespace().next().unwrap_or("")), "preferences-desktop-display")); }
        let inputs = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
        for block in inputs.split("\n\n") {
            let name = block.lines().find_map(|l| l.strip_prefix("N: Name=")).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
            let h = block.lines().find_map(|l| l.strip_prefix("H: Handlers=")).unwrap_or("");
            if name.is_empty() || name.contains("Button") || name.contains("Video Bus") || name.contains("PC Speaker") { continue; }
            if h.contains("mouse") { devices.push((name, "input-mouse")); } else if h.contains("kbd") && h.contains("sysrq") { devices.push((name, "preferences-desktop-keyboard")); }
        }
        for l in out(&["bluetoothctl", "devices", "Paired"]).lines() { devices.push((l.splitn(3, ' ').nth(2).unwrap_or("Bluetooth device").to_string(), "bluetooth")); }
        group("Devices", devices);
        let printers: Vec<(String, &str)> = out(&["lpstat", "-p"]).lines().filter_map(|l| l.strip_prefix("printer ").map(|r| (r.split_whitespace().next().unwrap_or("").to_string(), "printer"))).collect();
        group("Printers and Faxes", printers);
        v.upcast()
    }

    // ----------------------------------------------------------------- region / access
    fn page_region(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Region and Language"), false, false, 0);
        let loc = out(&["localectl", "status"]);
        v.pack_start(&kv_grid(&[("Format:", loc.lines().find_map(|l| l.trim().strip_prefix("System Locale:")).unwrap_or("").trim().to_string()),
            ("Keyboard layout:", out(&["setxkbmap", "-query"]).lines().find_map(|l| l.strip_prefix("layout:")).unwrap_or("").trim().to_string())]), false, false, 0);
        v.pack_start(&section("Keyboards and Languages"), false, false, 0);
        v.pack_start(&input_languages(), false, false, 0);
        v.upcast()
    }

    fn page_access(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Make your computer easier to use"), false, false, 0);
        v.pack_start(&section("Quick access to common tools"), false, false, 0);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        for (label, cmd) in [("Start Magnifier", "kmag"), ("Start On-Screen Keyboard", "onboard"), ("Start Narrator", "orca")] {
            let b = gtk::Button::with_label(label);
            b.connect_clicked(move |_| { if !util::spawn(&[cmd]) { util::error(&format!("{label} is not installed."), &format!("Install the '{cmd}' package.")); } });
            row.pack_start(&b, false, false, 0);
        }
        v.pack_start(&row, false, false, 0);
        v.pack_start(&section("Make the computer easier to see"), false, false, 0);
        let scale = gtk::ComboBoxText::new();
        for (id, l) in [("1", "Smaller - 100% (default)"), ("1.25", "Medium - 125%"), ("1.5", "Larger - 150%")] { scale.append(Some(id), l); }
        scale.set_active_id(Some("1"));
        scale.connect_changed(|c| if let Some(id) = c.active_id() {
            let dpi = (96.0 * id.parse::<f64>().unwrap_or(1.0)) as u32;
            util::spawn(&["sh", "-c", &format!("echo 'Xft.dpi: {dpi}' | xrdb -merge")]);
        });
        v.pack_start(&scale, false, false, 0);
        v.pack_start(&text("Text size changes apply to programs started afterwards."), false, false, 0);
        v.upcast()
    }
}

fn build(app: &gtk::Application, page: String) {
    theme::init_app();
    theme::load_css(aerode::explorer::CSS);
    theme::load_css(CSS);
    let win = gtk::ApplicationWindow::new(app);
    win.set_default_size(880, 620);
    win.set_icon_name(Some(util::first_icon(&["preferences-system"])));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    // glass header: back/forward + breadcrumb + search
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.style_context().add_class("cp-header");
    header.style_context().add_class("exp-header");
    let back = gtk::Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
    back.style_context().add_class("nav");
    let fwd = gtk::Button::from_icon_name(Some("go-next-symbolic"), gtk::IconSize::Button);
    fwd.style_context().add_class("nav");
    fwd.set_sensitive(false);
    header.pack_start(&back, false, false, 0);
    header.pack_start(&fwd, false, false, 0);
    let addr = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    addr.style_context().add_class("cp-addr");
    addr.pack_start(&util::image("preferences-system", 16), false, false, 0);
    let crumb = gtk::Label::new(Some("Control Panel ▸"));
    crumb.set_xalign(0.0);
    addr.pack_start(&crumb, true, true, 0);
    header.pack_start(&addr, true, true, 0);
    let search = gtk::Entry::new();
    search.set_placeholder_text(Some("Search Control Panel"));
    search.style_context().add_class("exp-search");
    search.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("system-search-symbolic"));
    header.pack_end(&search, false, false, 0);
    header.connect_draw(|h, cr| {
        use gtk::gdk::prelude::GdkContextExt;
        let (x, y) = h.window().map(|gw| { let (_, x, y) = gw.origin(); (x, y) }).unwrap_or((0, 0));
        cr.set_source_pixbuf(&aerode::glass::wallpaper().blurred, -x as f64, -y as f64);
        cr.paint().ok();
        let c = aerode::wm::colorization(&config::get().tint);
        cr.set_source_rgba(c[0], c[1], c[2], 0.55);
        cr.paint().ok();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.18);
        cr.paint().ok();
        glib::Propagation::Proceed
    });
    { let h = header.clone(); win.connect_configure_event(move |_, _| { h.queue_draw(); false }); }
    root.pack_start(&header, false, false, 0);
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    main.style_context().add_class("cp-main");
    let side = gtk::Box::new(gtk::Orientation::Vertical, 4);
    side.style_context().add_class("cp-sidebar");
    side.set_size_request(200, -1);
    main.pack_start(&side, false, false, 0);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.pack_start(&body, true, true, 0);
    root.pack_start(&main, true, true, 0);
    win.add(&root);
    let cp = Rc::new(Cp { win: win.clone(), side, body, crumb, history: RefCell::new(vec![]) });
    { let c = cp.clone(); back.connect_clicked(move |_| {
        let prev = { let mut h = c.history.borrow_mut(); h.pop(); h.pop() };
        c.open(&prev.unwrap_or_default());
    }); }
    {
        // search: jump to the first page whose name matches
        let c = cp.clone();
        search.connect_activate(move |s| {
            let q = s.text().to_lowercase();
            let pages = [("system", "system computer name ram processor"), ("display", "display resolution screen monitor"), ("personalization", "personalization wallpaper background theme color"),
                ("sound", "sound audio volume speakers microphone"), ("datetime", "date time clock zone"), ("power", "power battery sleep plan"), ("mouse", "mouse pointer cursor"),
                ("keyboard", "keyboard repeat"), ("programs", "programs uninstall software features"), ("users", "user account password picture"), ("network", "network internet wifi"),
                ("defaults", "default programs browser"), ("devices", "devices printers bluetooth"), ("region", "region language keyboard layout"), ("access", "ease access magnifier"),
                ("startup", "startup autostart"), ("services", "services")];
            if let Some((p, _)) = pages.iter().find(|(_, k)| k.split(' ').any(|w| w.starts_with(&q) || q.contains(w))) { c.open(p); }
        });
    }
    root.show_all();
    cp.open(&page);
    win.show();
    let _ = gdk::Display::default();
}

fn main() -> glib::ExitCode {
    let page = std::env::args().nth(1).unwrap_or_default();
    let app = gtk::Application::new(Some("org.aerode.ControlPanel"), gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(move |app| build(app, page.clone()));
    app.run_with_args::<&str>(&[])
}
