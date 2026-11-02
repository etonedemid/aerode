//! Default Programs: "Choose the programs that Windows uses by default" with Set your default
//! programs (every program that opens files: make it the default for all its types, or choose
//! them one by one), Associate a file type or protocol with a program (every extension the
//! shared MIME database knows plus the URL protocols, Change program… = the Open With dialog),
//! Change AutoPlay settings and Set program access and computer defaults (default web browser,
//! e-mail, media player, instant messaging; "Enable access to this program" hides or shows a
//! program in the Start menu). Defaults are written with GIO (~/.config/mimeapps.list).
use super::*;
use std::collections::BTreeMap;

impl Cp {
    pub(crate) fn page_defaults(self: &Rc<Self>) -> gtk::Widget {
        css();
        match sub(self, "defaults").as_deref() {
            Some("set") => return set_programs_page(self, None),
            Some("assoc") => return assoc_page(self),
            Some("access") => return access_page(self),
            Some(s) if s.starts_with("app/") => return set_programs_page(self, Some(s[4..].to_string())),
            _ => {}
        }
        let v = vbox(10);
        v.pack_start(&title("Choose the programs that Windows uses by default"), false, false, 0);
        // Win7: four command links (green arrow, dark blue text, a note under each)
        let nav = |_icon: &'static [&'static str], t: &str, d: &str, page: &'static str| { let c = self.clone(); command_link(t, d, move || go(&c, page)) };
        v.pack_start(&nav(&["preferences-desktop-default-applications", "preferences-system"], "Set your default programs", "Make a program the default for all file types and protocols it can open.", "defaults/set"), false, false, 0);
        v.pack_start(&nav(&["text-x-generic", "application-x-executable"], "Associate a file type or protocol with a program", "Make a file type or protocol (such as .mp3 or http://) always open in a specific program.", "defaults/assoc"), false, false, 0);
        v.pack_start(&nav(&["media-optical", "drive-optical"], "Change AutoPlay settings", "Play CDs or other media automatically.", "autoplay"), false, false, 0);
        v.pack_start(&nav(&["preferences-desktop-default-applications", "computer"], "Set program access and computer defaults", "Control access to certain programs and set defaults for this computer.", "defaults/access"), false, false, 0);
        v.upcast()
    }
}

/// Every program that opens some file type or protocol, by name.
fn programs() -> Vec<gio::AppInfo> {
    let mut v: Vec<gio::AppInfo> = gio::AppInfo::all().into_iter().filter(|a| a.should_show() && !a.supported_types().is_empty()).collect();
    v.sort_by_key(|a| a.display_name().to_lowercase());
    v.dedup_by(|a, b| a.id() == b.id());
    v
}

fn is_default(app: &gio::AppInfo, t: &str) -> bool {
    gio::AppInfo::default_for_type(t, false).and_then(|d| d.id()).map(|id| Some(id) == app.id()).unwrap_or(false)
}

/// mime type → its extensions, from the shared MIME database's globs. An extension belongs to
/// one type only, the one with the highest weight (globs2 lists them first), like Win7 where
/// every extension has exactly one file type.
fn globs() -> BTreeMap<String, Vec<String>> {
    let mut dirs: Vec<std::path::PathBuf> = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    globs_from(&dirs.iter().filter_map(|d| std::fs::read_to_string(d.join("mime/globs2")).ok()).collect::<Vec<_>>())
}

fn globs_from(files: &[String]) -> BTreeMap<String, Vec<String>> {
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for t in files {
        for l in t.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 3 { continue; }
            let Some(ext) = f[2].strip_prefix("*.") else { continue };
            if ext.contains(['*', '[', '?']) { continue; }
            let e = format!(".{}", ext.to_lowercase());
            if !seen.insert(e.clone()) { continue; }
            m.entry(f[1].to_string()).or_default().push(e);
        }
    }
    m
}

/// "HTTP" for x-scheme-handler/http, ".mp3" for audio/mpeg…
fn type_name(t: &str, ext: &BTreeMap<String, Vec<String>>) -> String {
    if let Some(s) = t.strip_prefix("x-scheme-handler/") { return s.to_uppercase(); }
    ext.get(t).and_then(|v| v.first().cloned()).unwrap_or_else(|| t.to_string())
}

/// "Microsoft Word Document", "URL:HTTP Protocol"; Win7's "DOCM File" when the MIME database
/// has no name for the type.
fn type_desc(t: &str, name: &str) -> String {
    if let Some(s) = t.strip_prefix("x-scheme-handler/") { return format!("URL:{} Protocol", s.to_uppercase()); }
    let d = gio::content_type_get_description(t).to_string();
    if d.is_empty() || d == t || d.contains('/') { format!("{} File", name.trim_start_matches('.').to_uppercase()) } else { d }
}

fn type_icon(t: &str) -> String {
    if t.starts_with("x-scheme-handler/") { return "text-html".into(); }
    gio::content_type_get_generic_icon_name(t).map(|s| s.to_string()).unwrap_or_else(|| "text-x-generic".into())
}

fn default_name(t: &str) -> String { gio::AppInfo::default_for_type(t, false).map(|a| a.display_name().to_string()).unwrap_or_else(|| "Unknown application".into()) }

// ------------------------------------------------------------------ Set Default Programs

fn set_programs_page(cp: &Rc<Cp>, app_id: Option<String>) -> gtk::Widget {
    if let Some(id) = app_id { return associations_for(cp, &id); }
    sub_crumb(cp, "Set Default Programs");
    let v = vbox(8);
    v.pack_start(&title("Set your default programs"), false, false, 0);
    v.pack_start(&text("To set a program as the default for all file types and protocols it can open, click the program and then click Set as default."), false, false, 0);
    let row = hbox(14);
    let lv = vbox(2);
    lv.pack_start(&label("Programs", ""), false, false, 0);
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
    let apps = programs();
    for a in &apps { store.insert_with_values(None, &[(0, &util::gicon_pixbuf(a.icon().as_ref(), 16)), (1, &a.display_name().to_string()), (2, &a.id().map(|s| s.to_string()).unwrap_or_default())]); }
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    icon_column(&tv, "", 0, 1, 0);
    let sw = scrolled(&tv);
    sw.set_size_request(250, 330);
    lv.pack_start(&sw, true, true, 0);
    row.pack_start(&lv, false, false, 0);
    let right = vbox(8);
    row.pack_start(&right, true, true, 0);
    v.pack_start(&row, true, true, 0);
    let ok = gtk::Button::with_label("OK");
    { let c = cp.clone(); ok.connect_clicked(move |_| back(&c, "defaults")); }
    v.pack_start(&button_row(&[&ok]), false, false, 0);
    let show: Rc<dyn Fn(Option<gio::AppInfo>)> = {
        let (right, cp) = (right.clone(), cp.clone());
        let me: Rc<RefCell<Option<Rc<dyn Fn(Option<gio::AppInfo>)>>>> = Rc::new(RefCell::new(None));
        let me2 = me.clone();
        let f: Rc<dyn Fn(Option<gio::AppInfo>)> = Rc::new(move |a: Option<gio::AppInfo>| {
            for c in right.children() { right.remove(&c); }
            let Some(a) = a else { right.pack_start(&label("Select a program from the list.", "cp-hint"), false, false, 20); right.show_all(); return };
            let h = hbox(10);
            h.pack_start(&util::app_image(&a, 32), false, false, 0);
            let n = vbox(0);
            n.pack_start(&bold(&a.display_name()), false, false, 0);
            if let Some(dai) = a.downcast_ref::<gio::DesktopAppInfo>() { if let Some(gn) = dai.generic_name() { n.pack_start(&label(&gn, "dim"), false, false, 0); } }
            h.pack_start(&n, false, false, 0);
            right.pack_start(&h, false, false, 0);
            if let Some(d) = a.description() { let l = label(&d, ""); l.set_max_width_chars(50); right.pack_start(&l, false, false, 0); }
            let types: Vec<String> = a.supported_types().iter().map(|s| s.to_string()).collect();
            let have = types.iter().filter(|t| is_default(&a, t)).count();
            let n = types.len();
            right.pack_start(&label(&if have == n { "This program has all its defaults".to_string() } else { format!("This program has {have} out of {n} defaults") }, ""), false, false, 6);
            let (a2, me3) = (a.clone(), me2.clone());
            right.pack_start(&command_link("Set this program as default", "Use the selected program to open all file types and protocols it can open by default.", move || {
                for t in a2.supported_types() { let _ = a2.set_as_default_for_type(&t); }
                if let Some(f) = me3.borrow().clone() { f(Some(a2.clone())); }
            }), false, false, 4);
            let (c, id) = (cp.clone(), a.id().map(|s| s.to_string()).unwrap_or_default());
            right.pack_start(&command_link("Choose defaults for this program", "Choose which file types and protocols the selected program opens by default.", move || go(&c, &format!("defaults/app/{id}"))), false, false, 0);
            right.show_all();
        });
        *me.borrow_mut() = Some(f.clone());
        f
    };
    show(None);
    { let show = show.clone(); tv.selection().connect_changed(move |s| { let a = s.selected().and_then(|(m, it)| util::app_by_id(&sval(&m, &it, 2))); show(a); }); }
    v.upcast()
}

/// "Set associations for a program": the program's types with check boxes.
fn associations_for(cp: &Rc<Cp>, id: &str) -> gtk::Widget {
    sub_crumb(cp, "Set Program Associations");
    let v = vbox(8);
    let Some(app) = util::app_by_id(id) else { v.pack_start(&title("That program is not installed anymore."), false, false, 0); return v.upcast() };
    v.pack_start(&title("Set associations for a program"), false, false, 0);
    v.pack_start(&text("Select the extensions you want this program to open by default, and then click Save."), false, false, 0);
    let h = hbox(10);
    h.pack_start(&util::app_image(&app, 32), false, false, 0);
    h.pack_start(&bold(&app.display_name()), false, false, 0);
    v.pack_start(&h, false, false, 0);
    let all = check("Select _All", false);
    v.pack_start(&all, false, false, 0);
    // check, name, description, current default, mime, was default, icon name
    let store = gtk::ListStore::new(&[glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL, glib::Type::STRING]);
    let ext = globs();
    // Win7 lists extensions and protocols: types without an extension (inode/directory…) are left out
    let mut types: Vec<String> = app.supported_types().iter().map(|s| s.to_string()).filter(|t| t.starts_with("x-scheme-handler/") || ext.contains_key(t)).collect();
    types.sort_by_key(|t| (t.starts_with("x-scheme-handler/"), type_name(t, &ext)));
    for t in &types {
        let d = is_default(&app, t);
        let name = type_name(t, &ext);
        store.insert_with_values(None, &[(0, &d), (1, &name), (2, &type_desc(t, &name)), (3, &default_name(t)), (4, t), (5, &d), (6, &type_icon(t))]);
    }
    let tv = gtk::TreeView::with_model(&store);
    let col = gtk::TreeViewColumn::new();
    col.set_title("Name");
    let tog = gtk::CellRendererToggle::new();
    TreeViewColumnExt::pack_start(&col, &tog, false);
    TreeViewColumnExt::add_attribute(&col, &tog, "active", 0);
    let pr = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&col, &pr, false);
    TreeViewColumnExt::add_attribute(&col, &pr, "icon-name", 6);
    let r = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &r, true);
    TreeViewColumnExt::add_attribute(&col, &r, "text", 1);
    col.set_fixed_width(150);
    col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    col.set_sort_column_id(1);
    tv.append_column(&col);
    columns(&tv, &[("Description", 2, 260), ("Current Default", 3, 0)]);
    { let s = store.clone(); tog.connect_toggled(move |_, p| { if let Some(it) = s.iter(&p) { let on = !s.value(&it, 0).get::<bool>().unwrap_or(false); s.set(&it, &[(0, &on)]); } }); }
    { let s = store.clone(); all.connect_toggled(move |c| { let on = c.is_active(); s.foreach(|m, _, it| { if let Some(ls) = m.downcast_ref::<gtk::ListStore>() { ls.set(it, &[(0, &on)]); } false }); }); }
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 300);
    v.pack_start(&sw, true, true, 0);
    let save = gtk::Button::with_label("Save");
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "defaults/set")); }
    let (c, s) = (cp.clone(), store.clone());
    save.connect_clicked(move |_| {
        s.foreach(|m, _, it| {
            let (on, was, t) = (m.value(it, 0).get::<bool>().unwrap_or(false), m.value(it, 5).get::<bool>().unwrap_or(false), sval(m, it, 4));
            if on && !was { let _ = app.set_as_default_for_type(&t); }
            // un-ticking: GIO has no "unset"; the next program that handles the type takes over
            if !on && was { let _ = gio::AppInfo::reset_type_associations(&t); }
            false
        });
        back(&c, "defaults/set");
    });
    v.pack_start(&button_row(&[&save, &cancel]), false, false, 0);
    v.upcast()
}

// ------------------------------------------------------------------------ Set Associations

fn assoc_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Set Associations");
    let v = vbox(8);
    v.pack_start(&title("Associate a file type or protocol with a specific program"), false, false, 0);
    v.pack_start(&text("Click on an extension to view the program that currently opens it by default. To change the default program, click Change program."), false, false, 0);
    let head = hbox(10);
    let cur_icon = gtk::Image::new();
    head.pack_start(&cur_icon, false, false, 0);
    let cur_text = vbox(0);
    head.pack_start(&cur_text, true, true, 0);
    let change = gtk::Button::with_mnemonic("C_hange program...");
    change.set_sensitive(false);
    change.set_valign(gtk::Align::Center);
    head.pack_end(&change, false, false, 0);
    v.pack_start(&head, false, false, 4);
    // icon name, name, description, current default, mime
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    let col = gtk::TreeViewColumn::new();
    col.set_title("Name");
    let pr = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&col, &pr, false);
    TreeViewColumnExt::add_attribute(&col, &pr, "icon-name", 0);
    let r = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &r, true);
    TreeViewColumnExt::add_attribute(&col, &r, "text", 1);
    col.set_fixed_width(130);
    col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
    col.set_sort_column_id(1);
    tv.append_column(&col);
    columns(&tv, &[("Description", 2, 280), ("Current Default", 3, 0)]);
    tv.set_fixed_height_mode(true);
    tv.set_enable_search(true);
    tv.set_search_column(1);
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 330);
    v.pack_start(&sw, true, true, 0);
    let status = label("Loading file types...", "cp-hint");
    v.pack_start(&status, false, false, 0);
    let close = gtk::Button::with_label("Close");
    { let c = cp.clone(); close.connect_clicked(move |_| back(&c, "defaults")); }
    v.pack_start(&button_row(&[&close]), false, false, 0);
    {
        let (store, status) = (store.clone(), status.clone());
        bg(|| {
            let ext = globs();
            let mut rows: Vec<(String, String, String, String, String)> = vec![];
            for (mime, exts) in &ext {
                let icon = type_icon(mime);
                let def = default_name(mime);
                for e in exts { rows.push((icon.clone(), e.clone(), type_desc(mime, e), def.clone(), mime.clone())); }
            }
            rows.sort_by(|a, b| a.1.cmp(&b.1));
            for p in ["ftp", "http", "https", "irc", "magnet", "mailto", "sms", "ssh", "steam", "tel", "webcal", "xmpp", "zoommtg"] {
                let t = format!("x-scheme-handler/{p}");
                rows.push((type_icon(&t), p.to_uppercase(), type_desc(&t, p), default_name(&t), t));
            }
            rows
        }, move |rows| {
            for r in rows { store.insert_with_values(None, &[(0, &r.0), (1, &r.1), (2, &r.2), (3, &r.3), (4, &r.4)]); }
            // Win7 has no count line: the label only says "Loading…" until the list is there
            status.hide();
        });
    }
    let update: Rc<dyn Fn()> = {
        let (tv, cur_icon, cur_text, change) = (tv.clone(), cur_icon.clone(), cur_text.clone(), change.clone());
        Rc::new(move || {
            for c in cur_text.children() { cur_text.remove(&c); }
            let Some((m, it)) = tv.selection().selected() else { change.set_sensitive(false); return };
            let mime = sval(&m, &it, 4);
            cur_icon.set_from_pixbuf(util::pixbuf(&sval(&m, &it, 0), 32).as_ref());
            cur_text.pack_start(&bold(&sval(&m, &it, 1)), false, false, 0);
            cur_text.pack_start(&label(&sval(&m, &it, 2), ""), false, false, 0);
            cur_text.pack_start(&label(&format!("Current Default: {}", default_name(&mime)), "dim"), false, false, 0);
            cur_text.show_all();
            change.set_sensitive(true);
        })
    };
    { let u = update.clone(); tv.selection().connect_changed(move |_| u()); }
    let do_change: Rc<dyn Fn()> = {
        let (tv, store, update) = (tv.clone(), store.clone(), update.clone());
        Rc::new(move || {
            let Some((m, it)) = tv.selection().selected() else { return };
            let (name, mime) = (sval(&m, &it, 1), sval(&m, &it, 4));
            if open_with(win_of(&tv).as_ref(), &name, &mime) {
                let def = default_name(&mime);
                // every extension of that type shows the new default
                store.foreach(|mm, _, i| { if sval(mm, i, 4) == mime { if let Some(ls) = mm.downcast_ref::<gtk::ListStore>() { ls.set(i, &[(3, &def)]); } } false });
                update();
            }
        })
    };
    { let f = do_change.clone(); change.connect_clicked(move |_| f()); }
    { let f = do_change.clone(); tv.connect_row_activated(move |_, _, _| f()); }
    v.upcast()
}

/// Win7's "Open with" dialog; true when a new default was chosen.
fn open_with(parent: Option<&gtk::Window>, name: &str, mime: &str) -> bool {
    let d = dialog(parent, "Open with", &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(440, 480);
    let c = d.content_area();
    c.pack_start(&label("Choose the program you want to use to open this file:", ""), false, false, 0);
    let h = hbox(8);
    h.pack_start(&util::image(&gio::content_type_get_generic_icon_name(mime).map(|s| s.to_string()).unwrap_or_else(|| "text-x-generic".into()), 32), false, false, 0);
    h.pack_start(&label(&format!("File:  {name}"), ""), false, false, 0);
    c.pack_start(&h, false, false, 0);
    let w = gtk::AppChooserWidget::new(mime);
    w.set_show_default(true);
    w.set_show_recommended(true);
    w.set_show_fallback(true);
    w.set_show_other(true);
    w.set_default_text("No programs are registered for this type.");
    c.pack_start(&w, true, true, 0);
    let always = check("Al_ways use the selected program to open this kind of file", true);
    always.set_sensitive(false);
    c.pack_start(&always, false, false, 0);
    let row = hbox(8);
    let browse = gtk::Button::with_mnemonic("B_rowse...");
    row.pack_end(&browse, false, false, 0);
    c.pack_start(&row, false, false, 0);
    c.pack_start(&label("If the program you want is not in the list or on your computer, you can look for the appropriate program on the Web.", "cp-hint"), false, false, 0);
    let chosen: Rc<RefCell<Option<gio::AppInfo>>> = Rc::new(RefCell::new(None));
    { let (d2, ch) = (d.clone(), chosen.clone()); browse.connect_clicked(move |_| {
        let fc = gtk::FileChooserNative::new(Some("Open with..."), Some(&d2), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
        let _ = fc.set_current_folder("/usr/bin");
        if fc.run() == gtk::ResponseType::Accept {
            if let Some(p) = fc.filename() {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Ok(a) = gio::AppInfo::create_from_commandline(&format!("{} %f", glib::shell_quote(&p).to_string_lossy()), Some(&name), gio::AppInfoCreateFlags::NONE) {
                    *ch.borrow_mut() = Some(a);
                    d2.response(gtk::ResponseType::Ok);
                }
            }
        }
    }); }
    { let d2 = d.clone(); w.connect_application_activated(move |_, _| d2.response(gtk::ResponseType::Ok)); }
    d.show_all();
    let r = d.run();
    let app = chosen.borrow().clone().or_else(|| w.app_info());
    d.close();
    if r != gtk::ResponseType::Ok { return false; }
    let Some(a) = app else { return false };
    if let Err(e) = a.set_as_default_for_type(mime) { util::error("The default program could not be changed.", &e.to_string()); return false; }
    true
}

// ------------------------------------------------- Set Program Access and Computer Defaults

const ACCESS: &[(&str, &str, &[&str])] = &[
    ("web browser", "Web browser", &["x-scheme-handler/http", "x-scheme-handler/https", "text/html", "application/xhtml+xml"]),
    ("e-mail program", "E-mail", &["x-scheme-handler/mailto", "message/rfc822"]),
    ("media player", "Media player", &["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav", "video/mp4", "video/x-matroska", "video/webm", "video/x-msvideo"]),
    ("instant messaging program", "Instant messaging", &["x-scheme-handler/xmpp", "x-scheme-handler/irc", "x-scheme-handler/tg", "x-scheme-handler/discord"]),
    ("virtual machine for Java", "Java", &["application/x-java-archive", "application/x-java-jnlp-file"]),
];

/// The programs that handle a category's first type (they appear in its list).
fn candidates(types: &[&str]) -> Vec<gio::AppInfo> {
    let mut v: Vec<gio::AppInfo> = vec![];
    for t in types {
        for a in gio::AppInfo::all_for_type(t) { if !v.iter().any(|x| x.id() == a.id()) && a.should_show() { v.push(a); } }
        if !v.is_empty() && *t == types[0] { break; }
    }
    v.sort_by_key(|a| a.display_name().to_lowercase());
    v
}

/// A program is "accessible" unless we hid it from the Start menu (a NoDisplay override).
fn hidden_override(id: &str) -> std::path::PathBuf { glib::user_data_dir().join("applications").join(id) }
fn is_hidden_by_us(id: &str) -> bool { std::fs::read_to_string(hidden_override(id)).map(|t| t.contains("X-Aerode-Access=disabled")).unwrap_or(false) }

fn set_access(app: &gio::AppInfo, on: bool) {
    let Some(id) = app.id() else { return };
    let path = hidden_override(&id);
    if on { if is_hidden_by_us(&id) { let _ = std::fs::remove_file(&path); } return; }
    let Some(src) = app.downcast_ref::<gio::DesktopAppInfo>().and_then(|d| d.filename()) else { return };
    let Ok(t) = std::fs::read_to_string(&src) else { return };
    let mut out = vec![];
    let mut in_entry = false;
    for l in t.lines() {
        if l.starts_with('[') { if in_entry { out.push("NoDisplay=true".to_string()); out.push("X-Aerode-Access=disabled".to_string()); } in_entry = l.trim() == "[Desktop Entry]"; }
        if in_entry && l.starts_with("NoDisplay=") { continue; }
        out.push(l.to_string());
    }
    if in_entry { out.push("NoDisplay=true".into()); out.push("X-Aerode-Access=disabled".into()); }
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new("/")));
    let _ = std::fs::write(&path, out.join("\n") + "\n");
}

fn access_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Set Program Access and Computer Defaults");
    let v = vbox(8);
    v.pack_start(&title("Set Program Access and Computer Defaults"), false, false, 0);
    v.pack_start(&text("A program configuration specifies default programs for certain activities, such as Web browsing or sending e-mail, and which programs are accessible from the Start menu, desktop, and other locations."), false, false, 0);
    v.pack_start(&label("Choose a configuration:", ""), false, false, 4);
    // Win7: one row per configuration (radio + the double chevron that opens it); Custom is the
    // last one and the one shown open
    let cfg = radios(&["_Non-Microsoft", "_Custom"], 1);
    let list = vbox(0);
    list.style_context().add_class("pa-list");
    let section = |r: &gtk::RadioButton, body: &gtk::Box, open: bool| {
        let row = hbox(0);
        row.style_context().add_class("pa-row");
        row.pack_start(r, true, true, 0);
        let chev = gtk::Button::with_label(if open { "︽" } else { "︾" });
        chev.style_context().add_class("pa-chev");
        chev.set_relief(gtk::ReliefStyle::None);
        chev.set_tooltip_text(Some(if open { "Collapse" } else { "Expand" }));
        row.pack_end(&chev, false, false, 0);
        list.pack_start(&row, false, false, 0);
        body.style_context().add_class("pa-body");
        // a closed section stays closed when the page is shown
        body.set_no_show_all(!open);
        list.pack_start(body, false, false, 0);
        let b = body.clone();
        chev.connect_clicked(move |c| {
            let open = !b.is_visible();
            b.set_no_show_all(!open);
            if open { b.show_all(); } else { b.hide(); }
            c.set_label(if open { "︽" } else { "︾" });
            c.set_tooltip_text(Some(if open { "Collapse" } else { "Expand" }));
        });
    };
    let nm = vbox(4);
    for (what, _, _) in ACCESS {
        nm.pack_start(&label(&format!("Choose a default {what}:"), ""), false, false, 0);
        let l = label(&format!("Use my current {what}"), "cp-hint");
        l.set_margin_start(16);
        nm.pack_start(&l, false, false, 0);
    }
    let note = label("Programs from Microsoft are not installed on this computer: your current programs stay the defaults.", "cp-hint");
    note.set_margin_top(4);
    nm.pack_start(&note, false, false, 0);
    let body = vbox(8);
    v.pack_start(&list, false, false, 0);
    { let b = body.clone(); cfg[1].connect_toggled(move |r| b.set_sensitive(r.is_active())); }
    // per category: "Use my current X" + one radio per program, each with "Enable access to this program"
    let picks: Rc<RefCell<Vec<(Vec<String>, Vec<(gtk::RadioButton, Option<gio::AppInfo>)>)>>> = Rc::new(RefCell::new(vec![]));
    let accesses: Rc<RefCell<Vec<(gio::AppInfo, gtk::CheckButton)>>> = Rc::new(RefCell::new(vec![]));
    for (what, _, types) in ACCESS {
        body.pack_start(&label(&format!("Choose a default {what}:"), ""), false, false, 0);
        let apps = candidates(types);
        let cur = types.first().and_then(|t| gio::AppInfo::default_for_type(t, false)).and_then(|a| a.id());
        let g = gtk::Grid::new();
        g.set_column_spacing(30);
        g.set_row_spacing(2);
        g.set_margin_start(16);
        let first = gtk::RadioButton::with_label(&format!("Use my current {what}"));
        first.set_active(true);
        g.attach(&first, 0, 0, 1, 1);
        let mut rs = vec![(first.clone(), None)];
        for (i, a) in apps.iter().enumerate() {
            let r = gtk::RadioButton::from_widget(&first);
            let h = hbox(6);
            h.pack_start(&util::app_image(a, 16), false, false, 0);
            h.pack_start(&label(&a.display_name(), if a.id() == cur { "ok-text" } else { "" }), false, false, 0);
            r.add(&h);
            g.attach(&r, 0, i as i32 + 1, 1, 1);
            let acc = check("Enable access to this program", !a.id().map(|id| is_hidden_by_us(&id)).unwrap_or(false));
            g.attach(&acc, 1, i as i32 + 1, 1, 1);
            accesses.borrow_mut().push((a.clone(), acc));
            rs.push((r, Some(a.clone())));
        }
        if apps.is_empty() { g.attach(&label("No programs of this kind are installed.", "cp-hint"), 0, 1, 2, 1); }
        body.pack_start(&g, false, false, 0);
        picks.borrow_mut().push((types.iter().map(|s| s.to_string()).collect(), rs));
    }
    section(&cfg[0], &nm, false);
    section(&cfg[1], &body, true);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    let help = gtk::Button::with_mnemonic("_Help");
    help.connect_clicked(|_| { util::open_uri("https://wiki.archlinux.org/title/Default_applications"); });
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "defaults")); }
    let (c, custom) = (cp.clone(), cfg[1].clone());
    ok.connect_clicked(move |_| {
        if custom.is_active() {
            for (types, rs) in picks.borrow().iter() {
                if let Some(a) = rs.iter().find(|(r, _)| r.is_active()).and_then(|(_, a)| a.clone()) {
                    for t in types { if a.supported_types().iter().any(|s| s == t) || t == &types[0] { let _ = a.set_as_default_for_type(t); } }
                }
            }
            for (a, c) in accesses.borrow().iter() { set_access(a, c.is_active()); }
            util::spawn(&["aerode", "reload"]);
        }
        back(&c, "defaults");
    });
    v.pack_start(&button_row(&[&ok, &cancel, &help]), false, false, 0);
    v.upcast()
}

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("audio/mpeg".to_string(), vec![".mp3".to_string()]);
        assert_eq!(super::type_name("audio/mpeg", &m), ".mp3");
        assert_eq!(super::type_name("x-scheme-handler/http", &m), "HTTP");
        assert_eq!(super::type_desc("x-scheme-handler/http", "HTTP"), "URL:HTTP Protocol");
    }
    #[test]
    fn one_type_per_extension() {
        let g = super::globs_from(&["# comment\n80:application/msword:*.doc\n50:text/plain:*.doc\n50:text/plain:*.txt\n50:text/x-foo:*.[ch]\n".to_string()]);
        assert_eq!(g.get("application/msword"), Some(&vec![".doc".to_string()]));
        assert_eq!(g.get("text/plain"), Some(&vec![".txt".to_string()]));
        assert!(g.get("text/x-foo").is_none());
    }
}
