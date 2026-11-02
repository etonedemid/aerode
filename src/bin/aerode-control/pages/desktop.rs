//! Appearance and desktop applets: Desktop Gadgets, Screen Saver Settings, Desktop Icon Settings,
//! Taskbar and Start Menu Properties, Folder Options, Fonts, Getting Started.
//! The Win7 property sheets (tabs + OK / Cancel / Apply) are built with [`Sheet`].
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use aerode::{config, util};
#[allow(unused_imports)]
use super::super::*;

mod folders;
mod fonts;
mod screensaver;
mod taskbar;

pub use folders::page_folderoptions;
pub use fonts::page_fonts;
pub(crate) use fonts::cleartype_tuner;
pub use screensaver::page_screensaver;
pub use taskbar::page_taskbar;

// ------------------------------------------------------------------ property sheet

/// A Windows 7 property sheet shown in the Control Panel page: tabs, then OK / Cancel / Apply.
/// Apply is greyed out until something changes; OK applies and closes, Cancel just closes
/// ("closes" = back to the previous Control Panel page, or quits when the sheet was opened directly).
pub(crate) struct Sheet {
    pub nb: gtk::Notebook,
    root: gtk::Box,
    apply_btn: gtk::Button,
    on_apply: RefCell<Option<Rc<dyn Fn() -> bool>>>,
}

impl Sheet {
    pub fn new(cp: &Rc<Cp>) -> Rc<Sheet> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_halign(gtk::Align::Start);
        let nb = gtk::Notebook::new();
        nb.set_size_request(410, -1);
        root.pack_start(&nb, false, false, 0);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_halign(gtk::Align::End);
        let ok = gtk::Button::with_label("OK");
        let cancel = gtk::Button::with_label("Cancel");
        let apply_btn = gtk::Button::with_mnemonic("_Apply");
        for b in [&ok, &cancel, &apply_btn] { b.set_size_request(75, -1); row.pack_start(b, false, false, 0); }
        apply_btn.set_sensitive(false);
        root.pack_start(&row, false, false, 0);
        let s = Rc::new(Sheet { nb, root, apply_btn: apply_btn.clone(), on_apply: RefCell::new(None) });
        // the buttons own the sheet (it lives as long as the page shows it)
        { let s2 = s.clone(); apply_btn.connect_clicked(move |_| { s2.apply(); }); }
        { let (s2, cp) = (s.clone(), cp.clone()); ok.connect_clicked(move |_| {
            if !s2.apply_btn.is_sensitive() || s2.apply() { leave(&cp); }
        }); }
        { let cp = cp.clone(); cancel.connect_clicked(move |_| leave(&cp)); }
        s
    }

    pub fn tab(&self, label: &str, page: &impl IsA<gtk::Widget>) {
        page.as_ref().set_margin_start(10);
        page.as_ref().set_margin_end(10);
        page.as_ref().set_margin_top(10);
        page.as_ref().set_margin_bottom(10);
        self.nb.append_page(page, Some(&gtk::Label::new(Some(label))));
    }

    /// The function Apply/OK runs; returning false keeps the sheet open (e.g. a cancelled prompt).
    pub fn on_apply(&self, f: impl Fn() -> bool + 'static) { *self.on_apply.borrow_mut() = Some(Rc::new(f)); }

    fn apply(&self) -> bool {
        let f = self.on_apply.borrow().clone();
        let ok = f.map(|f| f()).unwrap_or(true);
        if ok { self.apply_btn.set_sensitive(false); }
        ok
    }

    /// Something changed: Apply becomes available.
    pub fn dirty(self: &Rc<Self>) -> Rc<dyn Fn()> {
        let b = self.apply_btn.clone();
        Rc::new(move || b.set_sensitive(true))
    }

    pub fn watch_check(self: &Rc<Self>, c: &gtk::CheckButton) { let d = self.dirty(); c.connect_toggled(move |_| d()); }
    pub fn watch_combo(self: &Rc<Self>, c: &gtk::ComboBoxText) { let d = self.dirty(); c.connect_changed(move |_| d()); }
    pub fn watch_spin(self: &Rc<Self>, c: &gtk::SpinButton) { let d = self.dirty(); c.connect_value_changed(move |_| d()); }

    pub fn widget(&self) -> gtk::Widget { self.root.clone().upcast() }
}

/// Close a property sheet: back to where the user came from, or quit when started on this page.
pub(crate) fn leave(cp: &Rc<Cp>) {
    if cp.history.borrow().len() <= 1 { cp.win.close(); return; }
    let prev = { let mut h = cp.history.borrow_mut(); h.pop(); h.pop() };
    cp.open(&prev.unwrap_or_default());
}

/// A Win7 group box.
pub(crate) fn group(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    let f = gtk::Frame::new(Some(label));
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.set_border_width(8);
    b.pack_start(child, true, true, 0);
    f.add(&b);
    f
}

pub(crate) fn vbox(spacing: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, spacing) }
pub(crate) fn hbox(spacing: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, spacing) }

pub(crate) fn check(label: &str, on: bool) -> gtk::CheckButton {
    let c = gtk::CheckButton::with_mnemonic(label);
    c.set_active(on);
    c
}

pub(crate) fn combo(items: &[(&str, &str)], active: &str) -> gtk::ComboBoxText {
    let c = gtk::ComboBoxText::new();
    for (id, l) in items { c.append(Some(id), l); }
    if !c.set_active_id(Some(active)) { c.set_active(Some(0)); }
    c
}

/// Radio buttons in one group; returns them in order.
pub(crate) fn radios(labels: &[&str], active: usize) -> Vec<gtk::RadioButton> {
    let mut v: Vec<gtk::RadioButton> = vec![];
    for (i, l) in labels.iter().enumerate() {
        let r = match v.first() { None => gtk::RadioButton::with_mnemonic(l), Some(g) => gtk::RadioButton::with_mnemonic_from_widget(g, l) };
        r.set_active(i == active);
        v.push(r);
    }
    v
}

pub(crate) fn parent_window(cp: &Rc<Cp>) -> gtk::Window { cp.win.clone().upcast() }

/// Ask the running shell to re-read config.json (desktop icons, taskbar, Start menu).
pub(crate) fn reload_shell() { util::spawn(&["aerode", "reload"]); }

/// Write the settings Control Panel owns in the session's xsettingsd config (font smoothing,
/// ClearType, recent files), tell this display's xsettingsd to re-read it, and mirror the font
/// settings into the X resources that Qt, Firefox and other Xft programs read.
pub(crate) fn write_xsettings() {
    let c = config::get();
    let f = &c.fonts;
    let hw = &c.hw;
    let mut lines = vec![
        format!("Xft/Antialias {}", f.antialias as u8),
        format!("Xft/Hinting {}", f.hinting as u8),
        format!("Xft/HintStyle \"{}\"", f.hintstyle),
        format!("Xft/RGBA \"{}\"", f.rgba),
        format!("Gtk/RecentFilesEnabled {}", c.taskbar.track_items as u8),
        // Display (text size), Mouse (double-click speed), Keyboard (cursor blink rate),
        // Performance Options (animations)
        format!("Xft/DPI {}", hw.dpi.max(48) * 1024),
        format!("Net/DoubleClickTime {}", hw.mouse.double_click.clamp(100, 2000)),
        format!("Net/CursorBlink {}", (hw.keyboard.blink > 0) as u8),
        format!("Net/CursorBlinkTime {}", if hw.keyboard.blink > 0 { hw.keyboard.blink * 2 } else { 1200 }),
        format!("Gtk/EnableAnimations {}", ["animate_controls", "menu_fade", "combo_slide"].iter().any(|k| hw.effect(k)) as u8),
    ];
    // the session exports XCURSOR_THEME from cursor-theme, so this one only goes to the live config
    let live_only = (!hw.mouse.cursor_theme.is_empty()).then(|| format!("Gtk/CursorThemeName \"{}\"", hw.mouse.cursor_theme));
    let dir = config::conf_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("xsettings-cp.conf"), lines.join("\n") + "\n");
    lines.extend(live_only);
    // the live config the session generated: replace our keys, keep everything else
    let live = dir.join("xsettingsd.conf");
    if let Ok(cur) = std::fs::read_to_string(&live) {
        let keys: Vec<&str> = lines.iter().filter_map(|l| l.split_whitespace().next()).collect();
        let mut out: Vec<String> = cur.lines().filter(|l| !keys.iter().any(|k| l.split_whitespace().next() == Some(k))).map(String::from).collect();
        out.extend(lines.iter().cloned());
        let _ = std::fs::write(&live, out.join("\n") + "\n");
    }
    hup_xsettingsd();
    let mut xres = format!("Xft.antialias: {}\nXft.hinting: {}\nXft.hintstyle: {}\nXft.rgba: {}\nXft.lcdfilter: lcddefault\nXft.dpi: {}\n",
        f.antialias as u8, f.hinting as u8, f.hintstyle, f.rgba, hw.dpi.max(48));
    if !hw.mouse.cursor_theme.is_empty() { xres += &format!("Xcursor.theme: {}\n", hw.mouse.cursor_theme); }
    let _ = std::fs::write(dir.join("Xresources"), &xres);
    util::spawn(&["xrdb", "-merge", &dir.join("Xresources").to_string_lossy()]);
    // this process follows straight away
    if let Some(s) = gtk::Settings::default() {
        s.set_gtk_xft_antialias(f.antialias as i32);
        s.set_gtk_xft_hinting(f.hinting as i32);
        s.set_gtk_xft_hintstyle(Some(&f.hintstyle));
        s.set_gtk_xft_rgba(Some(&f.rgba));
        s.set_gtk_double_click_time(hw.mouse.double_click.clamp(100, 2000) as i32);
        s.set_gtk_cursor_blink(hw.keyboard.blink > 0);
        if hw.keyboard.blink > 0 { s.set_gtk_cursor_blink_time(hw.keyboard.blink as i32 * 2); }
    }
    // GNOME programs keep their own switch for recent files
    if util::which("gsettings") {
        util::spawn(&["gsettings", "set", "org.gnome.desktop.privacy", "remember-recent-files", if c.taskbar.track_items { "true" } else { "false" }]);
    }
}

/// SIGHUP the xsettingsd serving our display (only ours: other sessions keep theirs).
fn hup_xsettingsd() {
    let disp = std::env::var("DISPLAY").unwrap_or_default();
    let Ok(rd) = std::fs::read_dir("/proc") else { return };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        if std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| c.trim() != "xsettingsd").unwrap_or(true) { continue; }
        let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        let same = env.split(|b| *b == 0).any(|kv| kv == format!("DISPLAY={disp}").as_bytes());
        if same { unsafe { libc::kill(pid, libc::SIGHUP); } }
    }
}

// ------------------------------------------------------------------ Desktop Gadgets

pub fn page_gadgets(cp: &Rc<Cp>) -> gtk::Widget {
    // Win7 opens the gadget gallery straight away
    util::spawn(&["aerode", "gadgets"]);
    cp.side_link("Personalization", "personalization");
    let v = vbox(6);
    v.pack_start(&title("Desktop Gadgets"), false, false, 0);
    let h = hbox(12);
    h.pack_start(&util::image(util::first_icon(&["aerode-gadgets", "preferences-desktop-gadgets"]), 48), false, false, 0);
    let col = vbox(2);
    col.pack_start(&link("Add gadgets to the desktop", || { util::spawn(&["aerode", "gadgets"]); }), false, false, 0);
    col.pack_start(&link("Uninstall a gadget", || { util::spawn(&["aerode", "gadgets"]); }), false, false, 0);
    col.pack_start(&link("Restore desktop gadgets installed with Windows", || { util::spawn(&["aerode", "gadgets"]); }), false, false, 0);
    h.pack_start(&col, false, false, 0);
    v.pack_start(&h, false, false, 0);
    v.pack_start(&text("Gadgets are mini-programs on the desktop, such as a clock, a calendar and a CPU meter. Right-click the desktop and click Gadgets to open the gallery at any time."), false, false, 8);
    v.upcast()
}

// ------------------------------------------------------------------ Desktop Icon Settings

/// (key in config.desktop_icons, checkbox label)
const DESK_ICONS: &[(&str, &str)] = &[("computer", "_Computer"), ("home", "_User's Files"), ("network", "_Network"), ("trash", "_Recycle Bin"), ("control", "C_ontrol Panel")];

/// The icons the list shows: (art key, label, default icon)
fn desk_art() -> Vec<(&'static str, String, &'static str)> {
    vec![("computer", "Computer".into(), "computer"), ("home", glib::user_name().to_string_lossy().into_owned(), "user-home"),
        ("network", "Network".into(), "network-workgroup"), ("trash-full", "Recycle Bin (full)".into(), "user-trash-full"),
        ("trash", "Recycle Bin (empty)".into(), "user-trash")]
}

fn art_pixbuf(icon: &str, size: i32) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    if icon.starts_with('/') { gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(icon, size, size, true).ok() } else { util::pixbuf_or(icon, size, "text-x-generic") }
}

pub fn page_desktopicons(cp: &Rc<Cp>) -> gtk::Widget {
    cp.side_link("Personalization", "personalization");
    let cfg = config::get();
    let sheet = Sheet::new(cp);
    let page = vbox(10);
    // Win7 lays the five boxes out in two columns
    let g = gtk::Grid::new();
    g.set_column_spacing(40);
    g.set_row_spacing(4);
    let mut checks = vec![];
    for (i, (key, label)) in DESK_ICONS.iter().enumerate() {
        let c = check(label, cfg.desktop_icons.iter().any(|k| k == key));
        sheet.watch_check(&c);
        let (col, row) = if i < 3 { (0, i as i32) } else { (1, i as i32 - 3) };
        g.attach(&c, col, row, 1, 1);
        checks.push((*key, c));
    }
    page.pack_start(&group("Desktop icons", &g), false, false, 0);
    // icon list (Change Icon... / Restore Default)
    let art: Rc<RefCell<std::collections::HashMap<String, String>>> = Rc::new(RefCell::new(cfg.desktop_icon_art.clone()));
    let store = gtk::ListStore::new(&[gtk::gdk_pixbuf::Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
    let fill = {
        let (store, art) = (store.clone(), art.clone());
        Rc::new(move || {
            store.clear();
            for (key, label, def) in desk_art() {
                let icon = art.borrow().get(key).cloned().unwrap_or_else(|| def.to_string());
                store.insert_with_values(None, &[(0, &art_pixbuf(&icon, 48)), (1, &label), (2, &key)]);
            }
        })
    };
    fill();
    let iv = gtk::IconView::with_model(&store);
    iv.set_pixbuf_column(0);
    iv.set_text_column(1);
    iv.set_item_width(60);
    iv.set_columns(5);
    iv.set_column_spacing(2);
    iv.set_item_padding(3);
    IconViewExt::set_margin(&iv, 4);
    iv.set_selection_mode(gtk::SelectionMode::Single);
    iv.select_path(&gtk::TreePath::from_indicesv(&[0]));
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    sw.set_size_request(-1, 112);
    sw.add(&iv);
    page.pack_start(&sw, false, false, 0);
    let row = hbox(6);
    row.set_halign(gtk::Align::End);
    let change = gtk::Button::with_mnemonic("C_hange Icon...");
    let restore = gtk::Button::with_mnemonic("Re_store Default");
    row.pack_start(&change, false, false, 0);
    row.pack_start(&restore, false, false, 0);
    page.pack_start(&row, false, false, 0);
    // aerode themes don't carry desktop icons: shown as in Win7, but fixed
    let themes = check("A_llow themes to change desktop icons", true);
    themes.set_sensitive(false);
    page.pack_start(&themes, false, false, 0);
    let selected = { let iv = iv.clone(); let store = store.clone(); move || {
        iv.selected_items().first().and_then(|p| store.iter(p)).map(|it| store.value(&it, 2).get::<String>().unwrap_or_default())
    } };
    {
        let (art, fill, sel, d, cp) = (art.clone(), fill.clone(), selected.clone(), sheet.dirty(), cp.clone());
        change.connect_clicked(move |_| {
            let Some(key) = sel() else { return };
            let def = desk_art().into_iter().find(|a| a.0 == key).map(|a| a.2).unwrap_or("text-x-generic");
            let cur = art.borrow().get(&key).cloned().unwrap_or_else(|| def.to_string());
            if let Some(icon) = change_icon(&parent_window(&cp), &cur) {
                art.borrow_mut().insert(key, icon);
                fill();
                d();
            }
        });
    }
    { let (art, fill, sel, d) = (art.clone(), fill.clone(), selected.clone(), sheet.dirty()); restore.connect_clicked(move |_| {
        if let Some(key) = sel() { if art.borrow_mut().remove(&key).is_some() { fill(); d(); } }
    }); }
    { let (cp, sel) = (cp.clone(), selected.clone()); iv.connect_item_activated(move |_, _| if sel().is_some() {
        // double-click = Change Icon..., like Win7
        let _ = &cp; change.clicked();
    }); }
    sheet.tab("Desktop Icons", &page);
    {
        let art = art.clone();
        sheet.on_apply(move || {
            let on: Vec<String> = checks.iter().filter(|(_, c)| c.is_active()).map(|(k, _)| k.to_string()).collect();
            let a = art.borrow().clone();
            config::update(|c| { c.desktop_icons = on; c.desktop_icon_art = a; });
            reload_shell();
            true
        });
    }
    sheet.widget()
}

/// Win7 "Change Icon": every place/device icon of the icon theme, or a picture file.
fn change_icon(parent: &gtk::Window, current: &str) -> Option<String> {
    let d = gtk::Dialog::with_buttons(Some("Change Icon"), Some(parent), gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(400, 360);
    let c = d.content_area();
    c.set_spacing(6);
    c.set_border_width(10);
    c.pack_start(&text("Look for icons in this file:"), false, false, 0);
    let row = hbox(6);
    let theme_name = gtk::Settings::default().and_then(|s| s.gtk_icon_theme_name()).map(|s| s.to_string()).unwrap_or_else(|| "hicolor".into());
    let entry = gtk::Entry::new();
    entry.set_text(if current.starts_with('/') { current } else { &theme_name });
    row.pack_start(&entry, true, true, 0);
    let browse = gtk::Button::with_mnemonic("_Browse...");
    row.pack_start(&browse, false, false, 0);
    c.pack_start(&row, false, false, 0);
    c.pack_start(&text("Select an icon from the list below:"), false, false, 0);
    let store = gtk::ListStore::new(&[gtk::gdk_pixbuf::Pixbuf::static_type(), glib::Type::STRING]);
    let theme = gtk::IconTheme::default();
    let mut names: Vec<String> = vec![];
    if let Some(t) = &theme {
        // folders and drives first, like imageres.dll; programs only when the theme has few of those
        for ctx in ["Places", "Devices", "Legacy", "Applications"] {
            if ctx == "Applications" && names.len() >= 40 { break; }
            let mut v: Vec<String> = t.list_icons(Some(ctx)).iter().map(|n| n.to_string()).filter(|n| !n.ends_with("-symbolic") && !names.contains(n)).collect();
            v.sort();
            names.extend(v);
        }
    }
    names.truncate(600);
    let iv = gtk::IconView::with_model(&store);
    iv.set_pixbuf_column(0);
    iv.set_item_width(40);
    iv.set_selection_mode(gtk::SelectionMode::Single);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&iv);
    c.pack_start(&sw, true, true, 0);
    // render in small batches so the dialog opens at once
    {
        let (store, iv, cur) = (store.clone(), iv.clone(), current.to_string());
        let queue = RefCell::new(names.into_iter());
        glib::idle_add_local(move || {
            for _ in 0..40 {
                let Some(n) = queue.borrow_mut().next() else { return glib::ControlFlow::Break };
                if let Some(pb) = util::pixbuf(&n, 32) {
                    let it = store.insert_with_values(None, &[(0, &pb), (1, &n)]);
                    if n == cur { if let Some(p) = store.path(&it).into() { iv.select_path(&p); iv.scroll_to_path(&p, false, 0.0, 0.0); } }
                }
            }
            glib::ControlFlow::Continue
        });
    }
    let picked: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let (d2, picked, entry) = (d.clone(), picked.clone(), entry.clone());
        browse.connect_clicked(move |_| {
            let fc = gtk::FileChooserNative::new(Some("Change Icon"), Some(&d2), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
            let ff = gtk::FileFilter::new();
            ff.set_name(Some("Icon files"));
            for p in ["*.png", "*.svg", "*.ico", "*.xpm", "*.jpg"] { ff.add_pattern(p); }
            fc.add_filter(ff);
            if fc.run() == gtk::ResponseType::Accept {
                if let Some(p) = fc.filename() { entry.set_text(&p.to_string_lossy()); *picked.borrow_mut() = Some(p.to_string_lossy().into_owned()); d2.response(gtk::ResponseType::Ok); }
            }
        });
    }
    { let d2 = d.clone(); iv.connect_item_activated(move |_, _| d2.response(gtk::ResponseType::Ok)); }
    d.show_all();
    let r = d.run();
    let from_list = iv.selected_items().first().and_then(|p| store.iter(p)).map(|it| store.value(&it, 1).get::<String>().unwrap_or_default());
    d.close();
    if r != gtk::ResponseType::Ok { return None; }
    picked.borrow().clone().or(from_list)
}

// ------------------------------------------------------------------ Getting Started

pub fn page_gettingstarted(cp: &Rc<Cp>) -> gtk::Widget {
    cp.side_link("Personalization", "personalization");
    cp.side_link("Display", "display");
    cp.side_link("User Accounts", "users");
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let osv = |k: &str| os.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
    let (os_name, os_url) = (osv("NAME"), osv("HOME_URL"));
    let os_name = if os_name.is_empty() { "Linux".to_string() } else { os_name };
    type Act = Rc<dyn Fn()>;
    let page = |p: &'static str| -> Act { let cp = cp.clone(); Rc::new(move || cp.open(p)) };
    // (title, icon, description, action link text, action)
    let tasks: Vec<(String, &str, String, &str, Act)> = vec![
        (format!("Go online to find out what's new in {os_name}"), "web-browser",
            format!("Visit the {os_name} website for news, documentation and the community forum."), "Go online to learn more",
            { let u = if os_url.is_empty() { "https://wiki.archlinux.org/".to_string() } else { os_url.clone() }; Rc::new(move || { util::open_uri(&u); }) }),
        ("Personalize Windows".into(), "preferences-desktop-wallpaper",
            "Choose a theme, desktop background, window color, sounds and screen saver to give your computer a personal touch.".into(), "Personalize Windows",
            Rc::new(|| { util::spawn(&["aerode", "personalize"]); })),
        ("Transfer files and settings from another computer".into(), "folder-remote",
            "Copy your documents, pictures, music and settings from an old computer or an external drive into your user folders.".into(), "Open your user folder",
            Rc::new(|| util::explore(&config::home().to_string_lossy()))),
        ("Use a homegroup to share with other computers in your home".into(), "folder-publicshare",
            "Share files and printers with the other computers on your home network.".into(), "Choose homegroup and sharing options", page("homegroup")),
        ("Change keyboards or other input methods".into(), "preferences-desktop-keyboard",
            "Add the keyboard layouts you type with and choose the hot key that switches between them.".into(), "Change keyboards", page("region")),
        ("Get more programs".into(), "system-software-install",
            "Browse the programs installed on this computer, remove the ones you don't use and add new ones.".into(), "Open Programs and Features", page("programs")),
        ("Back up your files".into(), "drive-harddisk",
            "Make copies of your important files so that you can restore them if the originals are lost or damaged.".into(), "Back up your files", page("backup")),
        ("Add new users to your computer".into(), "system-users",
            "If several people share this computer, give each of them an account with their own files and settings.".into(), "Add new users", page("users")),
        ("Change the size of the text on your screen".into(), "preferences-desktop-display",
            "Make text and other items larger or smaller, and set the screen resolution.".into(), "Adjust display", page("display")),
    ];
    let v = vbox(0);
    // top: the selected task's description (Win7's blue-gradient banner)
    let banner = hbox(16);
    banner.style_context().add_class("gs-banner");
    banner.set_border_width(14);
    let big = gtk::Image::new();
    banner.pack_start(&big, false, false, 0);
    let col = vbox(6);
    let head = gtk::Label::new(None);
    head.set_xalign(0.0);
    head.style_context().add_class("cp-title");
    let desc = text("");
    let act_box = hbox(0);
    col.pack_start(&head, false, false, 0);
    col.pack_start(&desc, false, false, 0);
    col.pack_start(&act_box, false, false, 0);
    banner.pack_start(&col, true, true, 0);
    let top = gtk::Label::new(None);
    top.set_markup("<span size='x-large' foreground='#1e3287'>Get started with Windows</span>");
    top.set_xalign(0.0);
    top.set_margin_bottom(10);
    v.pack_start(&top, false, false, 0);
    v.pack_start(&banner, false, false, 0);
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::Single);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(3);
    flow.set_max_children_per_line(3);
    flow.set_row_spacing(4);
    flow.set_column_spacing(4);
    flow.set_margin_top(12);
    flow.set_activate_on_single_click(false);
    let tasks = Rc::new(tasks);
    for (t, icon, _, _, _) in tasks.iter() {
        let h = hbox(8);
        h.set_border_width(6);
        h.pack_start(&util::image(icon, 32), false, false, 0);
        let l = gtk::Label::new(Some(t));
        l.set_xalign(0.0);
        l.set_line_wrap(true);
        l.set_max_width_chars(22);
        h.pack_start(&l, true, true, 0);
        flow.add(&h);
    }
    let show = {
        let (tasks, big, head, desc, act_box) = (tasks.clone(), big.clone(), head.clone(), desc.clone(), act_box.clone());
        move |i: usize| {
            let Some((t, icon, d, a, f)) = tasks.get(i) else { return };
            if let Some(pb) = util::pixbuf(icon, 64) { big.set_from_pixbuf(Some(&pb)); }
            head.set_text(t);
            desc.set_text(d);
            for c in act_box.children() { act_box.remove(&c); }
            let f = f.clone();
            act_box.pack_start(&link(a, move || f()), false, false, 0);
            act_box.show_all();
        }
    };
    show(1);
    { let show = show.clone(); flow.connect_selected_children_changed(move |f| if let Some(c) = f.selected_children().first() { show(c.index() as usize); }); }
    { let tasks = tasks.clone(); flow.connect_child_activated(move |_, c| if let Some(t) = tasks.get(c.index() as usize) { (t.4)(); }); }
    if let Some(c) = flow.child_at_index(1) { flow.select_child(&c); }
    v.pack_start(&flow, false, false, 0);
    aerode::theme::load_css(".gs-banner { background-image: linear-gradient(to bottom, #f4f9fe, #dcebfa); border: 1px solid #b9d1ea; border-radius: 4px; }\n\
        flowboxchild { border: 1px solid transparent; border-radius: 3px; }\n\
        flowboxchild:hover { background-image: linear-gradient(to bottom, #f5fbfe, #e3f1fc); border-color: #b8d6fb; }\n\
        flowboxchild:selected { background-image: linear-gradient(to bottom, #e6f2fd, #c7e1fb); border-color: #7da2ce; color: #000; }");
    v.upcast()
}
