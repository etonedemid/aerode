//! Programs and Features (appwiz.cpl): "Uninstall or change a program" lists the explicitly
//! installed pacman packages (read from pacman's local database, the same data `pacman -Qei`
//! prints) and the Flatpak apps, with the program's own name and icon from its .desktop file.
//! Uninstall → `pkexec pacman -Rns` (what else goes with it is asked first with `-Rnsp`), Repair
//! → reinstall the same version. "View installed updates" reads pacman.log; uninstalling an
//! update goes back to the previous version from the package cache. "Turn Windows features on
//! or off" maps the Win7 features to the packages that provide them on Arch.
use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Default, Debug)]
pub struct Prog {
    pub name: String,
    /// package name or Flatpak application id
    pub pkg: String,
    pub publisher: String,
    pub installed: i64,
    pub size: u64,
    pub version: String,
    /// "pacman" | "flatpak-system" | "flatpak-user"
    pub kind: String,
    pub url: String,
    pub desc: String,
    pub icon: String,
    /// sync repository ("" = foreign, e.g. built from the AUR) and its version
    pub repo: String,
    pub sync_version: String,
}

const DB: &str = "/var/lib/pacman/local";

/// `%KEY%` sections of a pacman `desc` file.
fn parse_desc(t: &str) -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    let mut key = String::new();
    for l in t.lines() {
        if l.starts_with('%') && l.ends_with('%') && l.len() > 2 { key = l.trim_matches('%').to_string(); m.entry(key.clone()).or_default(); }
        else if !l.is_empty() && !key.is_empty() { m.entry(key.clone()).or_default().push(l.to_string()); }
    }
    m
}

/// Name and Icon of a .desktop file (the untranslated ones; NoDisplay entries are skipped).
fn desktop_entry(path: &std::path::Path) -> Option<(String, String)> {
    let t = std::fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let (mut name, mut icon, mut hidden) = (String::new(), String::new(), false);
    for l in t.lines() {
        let l = l.trim();
        if l.starts_with('[') { in_entry = l == "[Desktop Entry]"; continue; }
        if !in_entry { continue; }
        if let Some(v) = l.strip_prefix("Name=") { if name.is_empty() { name = v.to_string(); } }
        else if let Some(v) = l.strip_prefix("Icon=") { icon = v.to_string(); }
        else if l == "NoDisplay=true" || l == "Hidden=true" { hidden = true; }
    }
    (!name.is_empty() && !hidden).then_some((name, icon))
}

/// Who distributes a package, Win7's "Publisher".
fn publisher_of(repo: &str, packager: &str) -> String {
    match repo {
        "core" | "extra" | "multilib" | "core-testing" | "extra-testing" | "multilib-testing" | "community" => "Arch Linux".into(),
        "endeavouros" | "endeavouros-testing" => "EndeavourOS".into(),
        "chaotic-aur" => "Chaotic-AUR".into(),
        r if r.starts_with("cachyos") => "CachyOS".into(),
        "" => {
            let p = packager.split('<').next().unwrap_or("").trim();
            if p.is_empty() || p == "Unknown Packager" { "AUR".into() } else { p.to_string() }
        }
        r => r.to_string(),
    }
}

/// pkgname → (repo, version) of every sync package (`pacman -Sl`).
fn sync_map() -> HashMap<String, (String, String)> {
    let mut m = HashMap::new();
    for l in sh(&["pacman", "-Sl"]).lines() {
        let mut it = l.split_whitespace();
        if let (Some(repo), Some(name), Some(ver)) = (it.next(), it.next(), it.next()) {
            m.entry(name.to_string()).or_insert((repo.to_string(), ver.to_string()));
        }
    }
    m
}

/// Every installed local package name (dependencies too).
pub fn installed_names() -> HashSet<String> {
    let mut s = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(DB) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            // name-version-release
            let mut parts: Vec<&str> = n.rsplitn(3, '-').collect();
            if parts.len() == 3 { s.insert(parts.pop().unwrap().to_string()); }
        }
    } else {
        for l in sh(&["pacman", "-Qq"]).lines() { s.insert(l.trim().to_string()); }
    }
    s
}

/// The explicitly installed packages (worker thread).
fn pacman_programs(sync: &HashMap<String, (String, String)>) -> Vec<Prog> {
    let mut v = vec![];
    let apps_dir = std::path::Path::new("/usr/share/applications");
    if let Ok(rd) = std::fs::read_dir(DB) {
        for e in rd.flatten() {
            let Ok(t) = std::fs::read_to_string(e.path().join("desc")) else { continue };
            let d = parse_desc(&t);
            let get = |k: &str| d.get(k).and_then(|v| v.first()).cloned().unwrap_or_default();
            if get("REASON") == "1" { continue; }
            let name = get("NAME");
            if name.is_empty() { continue; }
            let mut p = Prog { pkg: name.clone(), name: name.clone(), version: get("VERSION"), desc: get("DESC"), url: get("URL"),
                installed: get("INSTALLDATE").parse().unwrap_or(0), size: get("SIZE").parse().unwrap_or(0), kind: "pacman".into(), ..Default::default() };
            let (repo, sv) = sync.get(&name).cloned().unwrap_or_default();
            p.publisher = publisher_of(&repo, &get("PACKAGER"));
            p.repo = repo;
            p.sync_version = sv;
            // the program's own name and icon (its first visible launcher)
            if let Ok(files) = std::fs::read_to_string(e.path().join("files")) {
                for f in files.lines().filter(|l| l.starts_with("usr/share/applications/") && l.ends_with(".desktop")) {
                    if let Some((n, i)) = desktop_entry(&std::path::Path::new("/").join(f)) { p.name = n; p.icon = i; break; }
                }
            }
            v.push(p);
        }
        let _ = apps_dir;
        return v;
    }
    // no readable database: what pacman prints (English keys)
    let (_, txt) = run(&["env", "LC_ALL=C", "pacman", "-Qei"]);
    for block in txt.split("\n\n") {
        let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k).filter(|r| r.trim_start().starts_with(':')).map(|r| r.trim_start()[1..].trim().to_string())).unwrap_or_default();
        let name = get("Name");
        if name.is_empty() { continue; }
        let size = get("Installed Size");
        let bytes = { let mut it = size.split_whitespace(); let n: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0); (match it.next() { Some("KiB") => n * 1024.0, Some("MiB") => n * 1048576.0, Some("GiB") => n * 1073741824.0, _ => n }) as u64 };
        // "Mon 02 Sep 2024 10:11:12 AM CEST": chrono cannot read zone names, the date is local anyway
        let date = get("Install Date");
        let no_zone = date.rsplit_once(' ').map(|x| x.0).unwrap_or(&date).to_string();
        let installed = chrono::NaiveDateTime::parse_from_str(&no_zone, "%a %d %b %Y %I:%M:%S %p").ok()
            .and_then(|d| d.and_local_timezone(chrono::Local).single()).map(|d| d.timestamp()).unwrap_or(0);
        let (repo, sv) = sync.get(&name).cloned().unwrap_or_default();
        v.push(Prog { name: name.clone(), pkg: name, publisher: publisher_of(&repo, &get("Packager")), installed, size: bytes, version: get("Version"),
            kind: "pacman".into(), url: get("URL"), desc: get("Description"), repo, sync_version: sv, ..Default::default() });
    }
    v
}

/// "1.2 GB" / "345.6 MB" / "12 kB" (Flatpak prints SI units) → bytes.
fn parse_si(s: &str) -> u64 {
    let s = s.replace('\u{a0}', " ");
    let mut it = s.split_whitespace();
    let n: f64 = it.next().and_then(|x| x.replace(',', ".").parse().ok()).unwrap_or(0.0);
    let m = match it.next().unwrap_or("").to_lowercase().as_str() { "kb" => 1e3, "mb" => 1e6, "gb" => 1e9, "tb" => 1e12, "kib" => 1024.0, "mib" => 1048576.0, "gib" => 1073741824.0, _ => 1.0 };
    (n * m) as u64
}

/// The Flatpak apps (worker thread).
fn flatpak_programs() -> Vec<Prog> {
    if !util::which("flatpak") { return vec![]; }
    let (ok, txt) = run(&["flatpak", "list", "--app", "--columns=name,application,version,size,origin,installation"]);
    if !ok { return vec![]; }
    let home = config::home();
    let mut v = vec![];
    for l in txt.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 6 { continue; }
        let user = f[5].trim() == "user";
        let base = if user { home.join(".local/share/flatpak") } else { std::path::PathBuf::from("/var/lib/flatpak") };
        let installed = std::fs::symlink_metadata(base.join("app").join(f[1]).join("current")).ok()
            .and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let icon = desktop_entry(&base.join("exports/share/applications").join(format!("{}.desktop", f[1]))).map(|e| e.1).unwrap_or_else(|| f[1].to_string());
        let origin = f[4].trim();
        v.push(Prog { name: f[0].to_string(), pkg: f[1].to_string(), publisher: if origin == "flathub" { "Flathub".into() } else { origin.to_string() },
            installed, size: parse_si(f[3]), version: f[2].to_string(), kind: if user { "flatpak-user".into() } else { "flatpak-system".into() },
            icon, desc: format!("Flatpak application {}", f[1]), ..Default::default() });
    }
    v
}

pub fn load() -> Vec<Prog> {
    let sync = sync_map();
    let mut v = pacman_programs(&sync);
    v.extend(flatpak_programs());
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

fn icon_for(p: &Prog, size: i32) -> Option<Pixbuf> {
    if p.icon.starts_with('/') { if let Ok(pb) = Pixbuf::from_file_at_scale(&p.icon, size, size, true) { return Some(pb); } }
    if !p.icon.is_empty() {
        if let Some(t) = gtk::IconTheme::default() { if t.has_icon(&p.icon) { return util::pixbuf(&p.icon, size); } }
    }
    pix(&["application-x-executable", "package-x-generic"], size)
}

// ------------------------------------------------------------------------------- the page

const C_ICON: u32 = 0; const C_NAME: u32 = 1; const C_PUB: u32 = 2; const C_DATE: u32 = 3; const C_SIZE: u32 = 4;
const C_BYTES: u32 = 5; const C_VER: u32 = 6; const C_IDX: u32 = 7; const C_WHEN: u32 = 8;

struct Ui {
    store: gtk::ListStore,
    tv: gtk::TreeView,
    progs: RefCell<Vec<Prog>>,
    details: gtk::Box,
    bar_dynamic: gtk::Box,
    busy: Cell<bool>,
}

impl Cp {
    pub(crate) fn page_programs(self: &Rc<Self>) -> gtk::Widget {
        css();
        match sub(self, "programs").as_deref() {
            Some("updates") => return updates_page(self),
            Some("features") => { let me = self.clone(); glib::idle_add_local_once(move || features_dialog(&me)); }
            _ => {}
        }
        self.side_link("View installed updates", "programs/updates");
        { let me = self.clone(); side_shield(self, "Turn Windows features on or off", move || features_dialog(&me)); }
        if let Some(store) = software_center() { side_action(self, "Install a program from the network", move || { util::spawn(&[store]); }); }
        let v = vbox(6);
        v.pack_start(&title("Uninstall or change a program"), false, false, 0);
        v.pack_start(&text("To uninstall a program, select it from the list and then click Uninstall, Change, or Repair."), false, false, 0);
        let frame = vbox(0);
        frame.style_context().add_class("cp-list");
        frame.set_margin_top(6);
        // command bar: Organize ▾, then what the selected program offers
        let bar = hbox(2);
        bar.style_context().add_class("cp-cmdbar");
        let organize = gtk::Menu::new();
        bar.pack_start(&menu_button("Organize", &organize), false, false, 0);
        let bar_dynamic = hbox(2);
        bar.pack_start(&bar_dynamic, false, false, 0);
        frame.pack_start(&clip(&bar), false, false, 0);
        let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U64, glib::Type::STRING, glib::Type::U32, glib::Type::I64]);
        let tv = gtk::TreeView::with_model(&store);
        tv.set_enable_search(true);
        tv.set_search_column(C_NAME as i32);
        icon_column(&tv, "Name", C_ICON as i32, C_NAME as i32, 230);
        columns(&tv, &[("Publisher", C_PUB as i32, 140)]);
        for (t, col, sort, w, right) in [("Installed On", C_DATE, C_WHEN, 82, false), ("Size", C_SIZE, C_BYTES, 66, true), ("Version", C_VER, C_VER, 110, false)] {
            let c = gtk::TreeViewColumn::new();
            c.set_title(t);
            let r = gtk::CellRendererText::new();
            if right { r.set_xalign(1.0); }
            r.set_property("ellipsize", gtk::pango::EllipsizeMode::End);
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", col as i32);
            c.set_resizable(true);
            c.set_sort_column_id(sort as i32);
            c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            c.set_fixed_width(w);
            tv.append_column(&c);
        }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.add(&tv);
        sw.set_min_content_height(300);
        frame.pack_start(&sw, true, true, 0);
        let details = hbox(12);
        details.style_context().add_class("cp-details");
        frame.pack_start(&details, false, false, 0);
        v.pack_start(&frame, true, true, 0);
        let ui = Rc::new(Ui { store, tv: tv.clone(), progs: RefCell::new(vec![]), details: details.clone(), bar_dynamic, busy: Cell::new(false) });
        {
            let pane = gtk::CheckMenuItem::with_mnemonic("_Details pane");
            pane.set_active(true);
            let d = details.clone();
            pane.connect_toggled(move |m| d.set_visible(m.is_active()));
            let layout = gtk::MenuItem::with_mnemonic("_Layout");
            let lm = gtk::Menu::new();
            lm.append(&pane);
            layout.set_submenu(Some(&lm));
            organize.append(&layout);
            organize.append(&gtk::SeparatorMenuItem::new());
            let u = ui.clone();
            menu_item(&organize, "_Refresh", move || reload(&u));
        }
        { let u = ui.clone(); tv.selection().connect_changed(move |_| selected_changed(&u)); }
        { let u = ui.clone(); tv.connect_row_activated(move |_, _, _| uninstall(&u)); }
        {
            let u = ui.clone();
            tv.connect_button_press_event(move |tv, e| {
                if e.button() != 3 { return glib::Propagation::Proceed; }
                let (x, y) = e.position();
                if let Some((Some(path), _, _, _)) = tv.path_at_pos(x as i32, y as i32) { tv.selection().select_path(&path); } else { return glib::Propagation::Proceed; }
                let m = gtk::Menu::new();
                { let u = u.clone(); menu_item(&m, "_Uninstall", move || uninstall(&u)); }
                if selected(&u).map(|p| can_repair(&p)).unwrap_or(false) { let u = u.clone(); menu_item(&m, "R_epair", move || repair(&u)); }
                m.show_all();
                m.popup_at_pointer(Some(e));
                glib::Propagation::Stop
            });
        }
        reload(&ui);
        v.upcast()
    }
}

/// A store front to install programs with, when one is installed.
fn software_center() -> Option<&'static str> {
    ["pamac-manager", "octopi", "gnome-software", "plasma-discover", "bauh"].into_iter().find(|c| util::which(c))
}

fn reload(ui: &Rc<Ui>) {
    ui.store.clear();
    set_details_summary(ui, None);
    let u = ui.clone();
    bg(load, move |progs| {
        let total: u64 = progs.iter().map(|p| p.size).sum();
        for (i, p) in progs.iter().enumerate() {
            u.store.insert_with_values(None, &[(C_ICON, &icon_for(p, 16)), (C_NAME, &p.name), (C_PUB, &p.publisher),
                (C_DATE, &if p.installed > 0 { day(p.installed) } else { String::new() }), (C_SIZE, &if p.size > 0 { util::human_size(p.size) } else { String::new() }),
                (C_BYTES, &p.size), (C_VER, &p.version), (C_IDX, &(i as u32)), (C_WHEN, &p.installed)]);
        }
        let n = progs.len();
        *u.progs.borrow_mut() = progs;
        set_details_summary(&u, Some((n, total)));
    });
}

fn day(t: i64) -> String { super::super::admin::day(t) }

/// The details pane with nothing selected: "Currently installed programs  Total size: …".
fn set_details_summary(ui: &Rc<Ui>, n: Option<(usize, u64)>) {
    for c in ui.details.children() { ui.details.remove(&c); }
    for c in ui.bar_dynamic.children() { ui.bar_dynamic.remove(&c); }
    ui.details.pack_start(&img(&["system-software-install", "package-x-generic"], 48), false, false, 0);
    let v = vbox(2);
    v.set_valign(gtk::Align::Center);
    match n {
        None => { v.pack_start(&label("Loading installed programs...", ""), false, false, 0); }
        Some((n, total)) => {
            let h = hbox(16);
            h.pack_start(&label("Currently installed programs", "n"), false, false, 0);
            let t = hbox(4);
            t.pack_start(&label("Total size:", "k"), false, false, 0);
            t.pack_start(&label(&util::human_size(total), ""), false, false, 0);
            h.pack_start(&t, false, false, 0);
            v.pack_start(&h, false, false, 0);
            v.pack_start(&label(&format!("{n} programs installed"), "k"), false, false, 0);
        }
    }
    ui.details.pack_start(&v, true, true, 0);
    ui.details.show_all();
}

fn selected(ui: &Rc<Ui>) -> Option<Prog> {
    let (m, it) = ui.tv.selection().selected()?;
    let i = m.value(&it, C_IDX as i32).get::<u32>().ok()? as usize;
    ui.progs.borrow().get(i).cloned()
}

fn can_repair(p: &Prog) -> bool { p.kind == "pacman" && !p.repo.is_empty() && p.sync_version == p.version }

fn selected_changed(ui: &Rc<Ui>) {
    let Some(p) = selected(ui) else {
        let n = ui.progs.borrow().len();
        let total = ui.progs.borrow().iter().map(|p| p.size).sum();
        set_details_summary(ui, Some((n, total)));
        return;
    };
    for c in ui.bar_dynamic.children() { ui.bar_dynamic.remove(&c); }
    let un = cmd_button("Uninstall");
    { let u = ui.clone(); un.connect_clicked(move |_| uninstall(&u)); }
    ui.bar_dynamic.pack_start(&un, false, false, 0);
    if can_repair(&p) { let r = cmd_button("Repair"); let u = ui.clone(); r.connect_clicked(move |_| repair(&u)); ui.bar_dynamic.pack_start(&r, false, false, 0); }
    ui.bar_dynamic.show_all();
    // details pane: icon, publisher and the facts Win7 shows
    for c in ui.details.children() { ui.details.remove(&c); }
    let i = gtk::Image::from_pixbuf(icon_for(&p, 48).as_ref());
    i.set_valign(gtk::Align::Start);
    ui.details.pack_start(&i, false, false, 0);
    let g = gtk::Grid::new();
    g.set_column_spacing(8);
    g.set_row_spacing(2);
    let n = label(&p.publisher, "n");
    g.attach(&n, 0, 0, 1, 1);
    let row = |col: i32, r: i32, k: &str, v: &str| {
        if v.is_empty() { return; }
        let kl = label(k, "k");
        kl.set_xalign(1.0);
        g.attach(&kl, col, r, 1, 1);
        let vl = label(v, "");
        vl.set_selectable(true);
        vl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        vl.set_line_wrap(false);
        vl.set_max_width_chars(40);
        g.attach(&vl, col + 1, r, 1, 1);
    };
    row(1, 0, "Product version:", &p.version);
    row(1, 1, "Size:", &if p.size > 0 { util::human_size(p.size) } else { String::new() });
    row(1, 2, "Install date:", &if p.installed > 0 { day(p.installed) } else { String::new() });
    row(3, 0, "Help link:", &p.url);
    row(3, 1, "Package:", &if p.kind == "pacman" { format!("{}{}", p.pkg, if p.repo.is_empty() { " (foreign)".to_string() } else { format!(" ({})", p.repo) }) } else { p.pkg.clone() });
    row(3, 2, "Comments:", &p.desc);
    ui.details.pack_start(&g, true, true, 0);
    ui.details.show_all();
}

fn uninstall(ui: &Rc<Ui>) {
    let Some(p) = selected(ui) else { return };
    if ui.busy.get() { return; }
    let parent = win_of(&ui.tv);
    if p.kind.starts_with("flatpak") {
        if !ask(parent.as_ref(), &format!("Are you sure you want to uninstall {}?", p.name), "") { return; }
        let scope = if p.kind == "flatpak-user" { "--user" } else { "--system" };
        ui.busy.set(true);
        let (u, id, name) = (ui.clone(), p.pkg.clone(), p.name.clone());
        bg(move || run(&["flatpak", "uninstall", "--noninteractive", "-y", scope, &id]), move |(ok, out)| {
            u.busy.set(false);
            if !ok { util::error(&format!("{name} could not be uninstalled."), out.trim()); }
            reload(&u);
        });
        return;
    }
    // what goes with it (dependencies nothing else needs), or why it can't go
    let (u, pkg) = (ui.clone(), p.pkg.clone());
    bg(move || run(&["env", "LC_ALL=C", "pacman", "-Rnsp", "--print-format", "%n", &pkg]), move |(ok, out)| {
        let parent = win_of(&u.tv);
        if !ok {
            let why: Vec<&str> = out.lines().filter(|l| l.contains("breaks dependency") || l.starts_with("error")).collect();
            util::error(&format!("{} cannot be uninstalled.", p.name), &if why.is_empty() { out.trim().to_string() } else { why.join("\n") });
            return;
        }
        let also: Vec<&str> = out.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && *l != p.pkg && !l.starts_with("checking")).collect();
        let secondary = if also.is_empty() { String::new() } else {
            format!("These packages were installed only for {} and will be removed too:\n{}", p.name, also.join(", "))
        };
        if !ask(parent.as_ref(), &format!("Are you sure you want to uninstall {}?", p.name), &secondary) { return; }
        u.busy.set(true);
        set_busy_text(&u, &format!("Please wait while {} is uninstalled...", p.name));
        let (u2, name) = (u.clone(), p.name.clone());
        elevate(&["pacman", "-Rns", "--noconfirm", &p.pkg], move |ok, out| {
            u2.busy.set(false);
            if !ok && !out.contains("Not authorized") && !out.contains("dismissed") { util::error(&format!("{name} could not be uninstalled."), out.trim()); }
            reload(&u2);
        });
    });
}

fn repair(ui: &Rc<Ui>) {
    let Some(p) = selected(ui) else { return };
    if !confirm(win_of(&ui.tv).as_ref(), "Programs and Features", &format!("Do you want to repair {}?", p.name),
        &format!("Its files will be installed again from the {} package ({}).", p.pkg, p.version), "_Repair") { return; }
    set_busy_text(ui, &format!("Please wait while {} is repaired...", p.name));
    let (u, name) = (ui.clone(), p.name.clone());
    elevate(&["pacman", "-S", "--noconfirm", &p.pkg], move |ok, out| {
        report(&format!("{name} could not be repaired."), ok, &out);
        reload(&u);
    });
}

fn set_busy_text(ui: &Rc<Ui>, t: &str) {
    for c in ui.details.children() { ui.details.remove(&c); }
    ui.details.pack_start(&img(&["system-software-install"], 48), false, false, 0);
    let s = gtk::Spinner::new();
    s.start();
    ui.details.pack_start(&s, false, false, 0);
    ui.details.pack_start(&label(t, ""), false, false, 0);
    ui.details.show_all();
}

/// Win7's "Are you sure you want to uninstall …?" (Yes / No).
fn ask(parent: Option<&gtk::Window>, primary: &str, secondary: &str) -> bool {
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Question, gtk::ButtonsType::None, primary);
    // Win7 order: Yes, then No
    d.add_button("_Yes", gtk::ResponseType::Yes);
    d.add_button("_No", gtk::ResponseType::No);
    d.set_default_response(gtk::ResponseType::No);
    d.set_title("Programs and Features");
    if !secondary.is_empty() { d.set_secondary_text(Some(secondary)); }
    let r = d.run();
    d.close();
    r == gtk::ResponseType::Yes
}

// ------------------------------------------------------------------------ installed updates

#[derive(Clone, Debug, PartialEq)]
pub struct Upd { pub pkg: String, pub old: String, pub new: String, pub when: i64 }

/// "[2024-05-01T10:22:33+0200] [ALPM] upgraded linux (6.8.1-1 -> 6.8.2-1)", newest first.
pub fn parse_log(t: &str) -> Vec<Upd> {
    let mut v = vec![];
    for l in t.lines() {
        let Some(rest) = l.split_once("] [ALPM] upgraded ").map(|x| x.1) else { continue };
        let ts = l.trim_start_matches('[').split(']').next().unwrap_or("");
        let when = chrono::DateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%z").map(|d| d.timestamp())
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M").map(|d| d.and_utc().timestamp())).unwrap_or(0);
        let Some((pkg, vers)) = rest.split_once(" (") else { continue };
        let Some((old, new)) = vers.trim_end_matches(')').split_once(" -> ") else { continue };
        v.push(Upd { pkg: pkg.to_string(), old: old.to_string(), new: new.to_string(), when });
    }
    v.reverse();
    v
}

/// The cached package file of `pkg` at `ver`, if pacman kept it.
fn cached(pkg: &str, ver: &str) -> Option<std::path::PathBuf> {
    let prefix = format!("{pkg}-{ver}-");
    std::fs::read_dir("/var/cache/pacman/pkg").ok()?.flatten().map(|e| e.path()).find(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        n.starts_with(&prefix) && n.contains(".pkg.tar") && !n.ends_with(".sig") && !n[prefix.len()..].contains('-')
    })
}

fn updates_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Installed Updates");
    cp.side_link("Uninstall a program", "programs");
    { let me = cp.clone(); side_shield(cp, "Turn Windows features on or off", move || features_dialog(&me)); }
    let v = vbox(6);
    v.pack_start(&title("Uninstall an update"), false, false, 0);
    v.pack_start(&text("To uninstall an update, select it from the list and then click Uninstall or Change."), false, false, 0);
    let frame = vbox(0);
    frame.style_context().add_class("cp-list");
    frame.set_margin_top(6);
    let bar = hbox(2);
    bar.style_context().add_class("cp-cmdbar");
    let organize = gtk::Menu::new();
    bar.pack_start(&menu_button("Organize", &organize), false, false, 0);
    let un = cmd_button("Uninstall");
    un.set_no_show_all(true);
    bar.pack_start(&un, false, false, 0);
    frame.pack_start(&clip(&bar), false, false, 0);
    // groups ("Arch Linux (123)") with the updates under them; columns name, program, version,
    // publisher, installed on, sort key, package, old version, cache file
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::I64, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Name", 0, 280), ("Program", 1, 140), ("Version", 2, 130), ("Publisher", 3, 110)]);
    {
        let c = gtk::TreeViewColumn::new();
        c.set_title("Installed On");
        let r = gtk::CellRendererText::new();
        TreeViewColumnExt::pack_start(&c, &r, true);
        TreeViewColumnExt::add_attribute(&c, &r, "text", 4);
        c.set_sort_column_id(5);
        tv.append_column(&c);
    }
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.add(&tv);
    sw.set_min_content_height(340);
    frame.pack_start(&sw, true, true, 0);
    let status = label("Loading installed updates...", "cp-hint");
    status.set_margin_start(8);
    status.set_margin_top(4);
    status.set_margin_bottom(4);
    frame.pack_start(&status, false, false, 0);
    v.pack_start(&frame, true, true, 0);
    let reload: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let r: Rc<dyn Fn()> = {
        let (store, tv, status) = (store.clone(), tv.clone(), status.clone());
        Rc::new(move || {
            store.clear();
            let (store, tv, status) = (store.clone(), tv.clone(), status.clone());
            bg(|| {
                let log = std::fs::read_to_string("/var/log/pacman.log").unwrap_or_default();
                let mut ups = parse_log(&log);
                ups.truncate(3000);
                let sync = sync_map();
                let cur = installed_versions();
                let rows: Vec<(Upd, String, Option<String>)> = ups.into_iter().map(|u| {
                    let repo = sync.get(&u.pkg).map(|r| r.0.clone()).unwrap_or_default();
                    // only the latest update of a package that is still at that version can be undone
                    let file = (cur.get(&u.pkg) == Some(&u.new)).then(|| cached(&u.pkg, &u.old)).flatten().map(|p| p.to_string_lossy().into_owned());
                    (u, publisher_of(&repo, ""), file)
                }).collect();
                rows
            }, move |rows| {
                let mut groups: BTreeMap<String, Vec<(Upd, Option<String>)>> = BTreeMap::new();
                let mut seen = HashSet::new();
                for (u, publ, file) in rows {
                    let file = if seen.insert(u.pkg.clone()) { file } else { None };
                    groups.entry(publ).or_default().push((u, file));
                }
                let n: usize = groups.values().map(|g| g.len()).sum();
                for (publ, ups) in &groups {
                    let parent = store.insert_with_values(None, None, &[(0, &format!("{publ} ({})", ups.len())), (5, &i64::MAX)]);
                    for (u, file) in ups {
                        store.insert_with_values(Some(&parent), None, &[(0, &format!("Update for {} ({} → {})", u.pkg, u.old, u.new)), (1, &u.pkg), (2, &u.new), (3, publ),
                            (4, &day(u.when)), (5, &u.when), (6, &u.pkg), (7, &u.old), (8, &file.clone().unwrap_or_default())]);
                    }
                }
                tv.expand_all();
                status.set_text(&if n == 0 { "No updates are recorded in /var/log/pacman.log.".to_string() } else { format!("{n} updates installed") });
            });
        })
    };
    *reload.borrow_mut() = Some(r.clone());
    { let r = r.clone(); menu_item(&organize, "_Refresh", move || r()); }
    {
        let un2 = un.clone();
        tv.selection().connect_changed(move |s| {
            let file = s.selected().map(|(m, it)| sval(&m, &it, 8)).unwrap_or_default();
            un2.set_visible(s.selected().map(|(m, it)| m.iter_parent(&it).is_some()).unwrap_or(false));
            un2.set_sensitive(!file.is_empty());
            un2.set_tooltip_text(Some(if file.is_empty() { "This update cannot be removed: the previous version is not in the package cache, or a newer update replaced it." } else { "Go back to the version this update replaced" }));
        });
    }
    {
        let (tv, r) = (tv.clone(), r.clone());
        un.connect_clicked(move |b| {
            let Some((m, it)) = tv.selection().selected() else { return };
            let (pkg, old, file) = (sval(&m, &it, 6), sval(&m, &it, 7), sval(&m, &it, 8));
            if file.is_empty() { return; }
            if !confirm(win_of(b).as_ref(), "Uninstall an update", "Are you sure you want to uninstall this update?",
                &format!("{pkg} goes back to version {old} (from the package cache)."), "_Yes") { return; }
            let r = r.clone();
            elevate(&["pacman", "-U", "--noconfirm", &file], move |ok, out| { report(&format!("The update for {pkg} could not be uninstalled."), ok, &out); r(); });
        });
    }
    r();
    v.upcast()
}

/// pkg → installed version (local database).
fn installed_versions() -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(rd) = std::fs::read_dir(DB) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            let parts: Vec<&str> = n.rsplitn(3, '-').collect();
            if parts.len() == 3 { m.insert(parts[2].to_string(), format!("{}-{}", parts[1], parts[0])); }
        }
    }
    m
}

// --------------------------------------------------------------------- Windows Features

/// (name, description, packages); a name ending in '/' starts a group, "" closes it.
const FEATURES: &[(&str, &str, &str)] = &[
    ("Bluetooth support", "Connect Bluetooth devices: mice, keyboards, headsets and phones.", "bluez bluez-utils"),
    ("File and Printer Sharing", "Share folders and printers with Windows computers (Samba).", "samba"),
    ("Firewall", "The Uncomplicated Firewall that Windows Firewall in Control Panel manages.", "ufw"),
    ("Flatpak", "Install sandboxed programs from Flathub.", "flatpak"),
    ("Games/", "Games that come with Windows, the Linux versions.", ""),
    ("Chess Titans", "Chess (GNOME Chess).", "gnome-chess"),
    ("Mahjong Titans", "Mahjongg solitaire.", "gnome-mahjongg"),
    ("Minesweeper", "Clear the mines.", "gnome-mines"),
    ("Solitaire, Spider Solitaire and FreeCell", "Card games (AisleRiot).", "aisleriot"),
    ("Sudoku", "Number puzzles.", "gnome-sudoku"),
    ("", "", ""),
    ("Internet Information Services/", "Web servers.", ""),
    ("Apache HTTP Server", "The Apache web server.", "apache"),
    ("nginx", "A small, fast web server and reverse proxy.", "nginx"),
    ("", "", ""),
    ("Media Features/", "Programs that play music, video and DVDs.", ""),
    ("Windows Media Center", "A media center for the TV (Kodi).", "kodi"),
    ("Windows Media Player", "Plays music and video (VLC).", "vlc"),
    ("", "", ""),
    ("Microsoft .NET Framework", "The .NET runtime and Mono for .NET programs.", "dotnet-runtime mono"),
    ("Print and Document Services/", "Printing and scanning.", ""),
    ("Internet Printing Client", "Print to local and network printers (CUPS).", "cups cups-pdf"),
    ("Printer configuration", "Add and set up printers.", "system-config-printer"),
    ("Scan Management", "Scanner drivers (SANE).", "sane"),
    ("Windows Fax and Scan", "Scan documents and pictures.", "simple-scan"),
    ("", "", ""),
    ("Remote Desktop/", "Connect to this computer from another one, and to other computers.", ""),
    ("Remote Desktop Connection", "Connect to other computers (RDP, VNC).", "remmina freerdp"),
    ("Secure Shell server", "Log on to this computer over the network (OpenSSH).", "openssh"),
    ("", "", ""),
    ("Remote Differential Compression", "Copies only the differences between files (rsync).", "rsync"),
    ("Simple Network Management Protocol (SNMP)", "SNMP agent and tools.", "net-snmp"),
    ("Subsystem for Windows-based Applications", "Run Windows programs (Wine).", "wine"),
    ("Tablet PC Components/", "Pen and touch.", ""),
    ("Tablet PC Input Panel", "An on-screen keyboard for pen and touch (Onboard).", "onboard"),
    ("Windows Journal", "Take handwritten notes (Xournal++).", "xournalpp"),
    ("", "", ""),
    ("Telnet Client", "Connect to remote computers with the Telnet protocol.", "inetutils"),
    ("TFTP Client", "Transfer files with the Trivial File Transfer Protocol.", "tftp-hpa"),
    ("Virtualization/", "Run other operating systems in a virtual machine.", ""),
    ("QEMU/KVM with Virtual Machine Manager", "", "qemu-desktop virt-manager"),
    ("VirtualBox", "", "virtualbox"),
    ("", "", ""),
    ("Windows Search", "Find files by name instantly (plocate).", "plocate"),
    ("XPS Viewer", "View PDF and XPS documents (Document Viewer).", "evince"),
];

const F_ON: u32 = 0; const F_MIX: u32 = 1; const F_NAME: u32 = 2; const F_DESC: u32 = 3; const F_PKGS: u32 = 4; const F_WAS: u32 = 5; const F_ICON: u32 = 6;

fn features_dialog(cp: &Rc<Cp>) {
    css();
    let d = dialog(Some(&cp.win.clone().upcast()), "Windows Features", &[]);
    d.set_default_size(420, 470);
    let c = d.content_area();
    let h = hbox(8);
    h.pack_start(&label("Turn Windows features on or off", "wu-title"), true, true, 0);
    c.pack_start(&h, false, false, 0);
    c.pack_start(&text("To turn a feature on, select its check box. To turn a feature off, clear its check box. A filled box means that only part of the feature is turned on."), false, false, 0);
    let store = gtk::TreeStore::new(&[glib::Type::BOOL, glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL, Pixbuf::static_type()]);
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    tv.set_tooltip_column(F_DESC as i32);
    let col = gtk::TreeViewColumn::new();
    let tog = gtk::CellRendererToggle::new();
    TreeViewColumnExt::pack_start(&col, &tog, false);
    TreeViewColumnExt::add_attribute(&col, &tog, "active", F_ON as i32);
    TreeViewColumnExt::add_attribute(&col, &tog, "inconsistent", F_MIX as i32);
    let pr = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&col, &pr, false);
    TreeViewColumnExt::add_attribute(&col, &pr, "pixbuf", F_ICON as i32);
    let r = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &r, true);
    TreeViewColumnExt::add_attribute(&col, &r, "text", F_NAME as i32);
    tv.append_column(&col);
    let sw = scrolled(&tv);
    c.pack_start(&sw, true, true, 0);
    let status = vbox(4);
    c.pack_start(&status, false, false, 0);
    let ok = d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    ok.set_sensitive(false);
    let loading = label("Please wait...", "cp-hint");
    status.pack_start(&loading, false, false, 0);
    // fill once we know what is installed
    {
        let (store, tv, ok, loading) = (store.clone(), tv.clone(), ok.clone(), loading.clone());
        bg(installed_names, move |have| {
            let (folder, item) = (pix(&["folder"], 16), pix(&["application-x-executable", "system-run"], 16));
            let mut parent: Option<gtk::TreeIter> = None;
            for (name, desc, pkgs) in FEATURES {
                if name.is_empty() { if let Some(p) = parent.take() { group_state(&store, &p); } continue; }
                if let Some(g) = name.strip_suffix('/') {
                    parent = Some(store.insert_with_values(None, None, &[(F_ON, &false), (F_MIX, &false), (F_NAME, &g), (F_DESC, desc), (F_PKGS, &""), (F_WAS, &false), (F_ICON, &folder)]));
                    continue;
                }
                let on = pkgs.split_whitespace().all(|p| have.contains(p));
                store.insert_with_values(parent.as_ref(), None, &[(F_ON, &on), (F_MIX, &false), (F_NAME, name), (F_DESC, desc), (F_PKGS, pkgs), (F_WAS, &on), (F_ICON, &item)]);
            }
            loading.set_text("");
            ok.set_sensitive(true);
            let _ = tv;
        });
    }
    {
        let store = store.clone();
        tog.connect_toggled(move |_, path| {
            let Some(it) = store.iter(&path) else { return };
            let on = !store.value(&it, F_ON as i32).get::<bool>().unwrap_or(false) || store.value(&it, F_MIX as i32).get::<bool>().unwrap_or(false);
            store.set(&it, &[(F_ON, &on), (F_MIX, &false)]);
            if let Some(ch) = store.iter_children(Some(&it)) { loop { store.set(&ch, &[(F_ON, &on), (F_MIX, &false)]); if !store.iter_next(&ch) { break; } } }
            if let Some(p) = store.iter_parent(&it) { group_state(&store, &p); }
        });
    }
    d.show_all();
    let store2 = store.clone();
    let (d2, status2) = (d.clone(), status.clone());
    d.connect_response(move |d, r| {
        if r != gtk::ResponseType::Ok { d.close(); return; }
        let (mut add, mut del) = (vec![], vec![]);
        store2.foreach(|m, _, it| {
            let pkgs = sval(m, it, F_PKGS as i32);
            if pkgs.is_empty() { return false; }
            let (on, was) = (m.value(it, F_ON as i32).get::<bool>().unwrap_or(false), m.value(it, F_WAS as i32).get::<bool>().unwrap_or(false));
            if on && !was { add.extend(pkgs.split_whitespace().map(String::from)); }
            if !on && was { del.extend(pkgs.split_whitespace().map(String::from)); }
            false
        });
        if add.is_empty() && del.is_empty() { d.close(); return; }
        let mut script = String::new();
        if !add.is_empty() { script.push_str(&format!("pacman -S --needed --noconfirm {} || exit 1; ", add.iter().map(|p| q(p)).collect::<Vec<_>>().join(" "))); }
        if !del.is_empty() { script.push_str(&format!("pacman -Rns --noconfirm {} || exit 1; ", del.iter().map(|p| q(p)).collect::<Vec<_>>().join(" "))); }
        // "Please wait while Windows makes changes to features. This might take several minutes."
        for c in status2.children() { status2.remove(&c); }
        status2.pack_start(&label("Please wait while Windows makes changes to features. This might take several minutes.", ""), false, false, 0);
        let pb = gtk::ProgressBar::new();
        status2.pack_start(&pb, false, false, 0);
        status2.show_all();
        let pulse = glib::timeout_add_local(std::time::Duration::from_millis(120), { let pb = pb.clone(); move || { pb.pulse(); glib::ControlFlow::Continue } });
        d.set_response_sensitive(gtk::ResponseType::Ok, false);
        d.set_response_sensitive(gtk::ResponseType::Cancel, false);
        let d3 = d2.clone();
        let pulse = RefCell::new(Some(pulse));
        elevate_sh(&script, move |ok, out| {
            if let Some(p) = pulse.borrow_mut().take() { p.remove(); }
            if ok { inform(Some(d3.upcast_ref()), "Windows Features", "Windows completed the requested changes.", ""); }
            else { report("Windows couldn't complete the requested changes.", ok, &out); }
            d3.close();
        });
    });
}

/// A group's box: on when every feature in it is, filled when some are.
fn group_state(store: &gtk::TreeStore, parent: &gtk::TreeIter) {
    let (mut on, mut all) = (0, 0);
    if let Some(ch) = store.iter_children(Some(parent)) { loop {
        all += 1;
        if store.value(&ch, F_ON as i32).get::<bool>().unwrap_or(false) { on += 1; }
        if !store.iter_next(&ch) { break; }
    } }
    store.set(parent, &[(F_ON, &(all > 0 && on == all)), (F_MIX, &(on > 0 && on < all))]);
    // the original state of a group follows its children too
    store.set(parent, &[(F_WAS, &(all > 0 && on == all))]);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log() {
        let t = "[2024-05-01T10:22:33+0200] [ALPM] upgraded linux (6.8.1.arch1-1 -> 6.8.2.arch1-1)\n[2024-05-01T10:22:34+0200] [ALPM] installed foo (1-1)\n[2024-05-02T08:00:00+0200] [ALPM] upgraded bash (5.2.026-2 -> 5.2.026-5)\n";
        let u = parse_log(t);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].pkg, "bash");
        assert_eq!(u[1], Upd { pkg: "linux".into(), old: "6.8.1.arch1-1".into(), new: "6.8.2.arch1-1".into(), when: 1714551753 });
    }
    #[test]
    fn desc() {
        let d = parse_desc("%NAME%\nfirefox\n\n%VERSION%\n120.0-1\n\n%DEPENDS%\ngtk3\nlibxt\n\n");
        assert_eq!(d["NAME"], vec!["firefox"]);
        assert_eq!(d["DEPENDS"].len(), 2);
    }
    #[test]
    fn si() {
        assert_eq!(parse_si("1.5 GB"), 1_500_000_000);
        assert_eq!(parse_si("12 kB"), 12_000);
        assert_eq!(publisher_of("extra", "x"), "Arch Linux");
        assert_eq!(publisher_of("", "Unknown Packager"), "AUR");
    }
}
