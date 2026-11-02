//! The Windows 7 start menu.
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{config, glass, sys, util};

thread_local! {
    static HIDE_ALL: RefCell<Option<Rc<dyn Fn()>>> = RefCell::new(None);
}

/// Hide the Start menu if it is open (used when clicking the desktop).
pub fn hide_all() { if let Some(f) = HIDE_ALL.with(|h| h.borrow().clone()) { f(); } }

/// Customize Start Menu items: (key, Win7 label, "link" = link / menu / don't display or "check", default)
pub const START_ITEMS: &[(&str, &str, &str, &str)] = &[
    ("computer", "Computer", "link", "link"),
    ("control", "Control Panel", "link", "link"),
    ("defaults", "Default Programs", "check", "on"),
    ("devices", "Devices and Printers", "check", "on"),
    ("documents", "Documents", "link", "link"),
    ("downloads", "Downloads", "link", "hide"),
    ("dragdrop", "Enable context menus and dragging and dropping", "check", "on"),
    ("games", "Games", "link", "link"),
    ("help", "Help", "check", "on"),
    ("highlight", "Highlight newly installed programs", "check", "on"),
    ("music", "Music", "link", "link"),
    ("network", "Network", "check", "off"),
    ("hoveropen", "Open submenus when I pause on them with the mouse pointer", "check", "on"),
    ("user", "Personal folder", "link", "link"),
    ("pictures", "Pictures", "link", "link"),
    ("recent", "Recent Items", "check", "off"),
    ("run", "Run command", "check", "on"),
    ("searchcp", "Search programs and Control Panel", "check", "on"),
    ("sortname", "Sort All Programs menu by name", "check", "on"),
    ("largeicons", "Use large icons", "check", "on"),
    ("videos", "Videos", "link", "hide"),
];

/// The Customize Start Menu value of `key` ("link" | "menu" | "hide" | "on" | "off").
pub fn start_item(key: &str) -> String {
    config::get().taskbar.start_items.get(key).cloned()
        .unwrap_or_else(|| START_ITEMS.iter().find(|i| i.0 == key).map(|i| i.3.to_string()).unwrap_or_else(|| "on".into()))
}

/// Taskbar and Start Menu Properties > Power button action: (id, label), in the Win7 list order
pub const POWER_ACTIONS: &[(&str, &str)] = &[("switchuser", "Switch user"), ("logoff", "Log off"), ("lock", "Lock"), ("restart", "Restart"),
    ("sleep", "Sleep"), ("hibernate", "Hibernate"), ("shutdown", "Shut down")];

fn power_action(id: &str) -> fn() {
    match id {
        "switchuser" => || { util::spawn(&["dm-tool", "switch-to-greeter"]); },
        "logoff" => sys::log_off, "lock" => sys::lock, "restart" => sys::restart,
        "sleep" => sys::sleep, "hibernate" => sys::hibernate, _ => sys::shutdown,
    }
}

const W: i32 = 400;
const H: i32 = 535;
const LEFT_W: i32 = 252;

pub struct StartMenu {
    win: gtk::Window,
    list: gtk::Box,
    search: gtk::Entry,
    avatar: gtk::Image,
    mode: Cell<u8>, // 0 frequent, 1 all programs, 2 search
    all_btn_label: gtk::Label,
    expanded: RefCell<Vec<String>>,
    pub on_hide: RefCell<Option<Box<dyn Fn()>>>,
    hidden_at: Cell<Option<std::time::Instant>>,
    /// the menu got keyboard focus since it was last shown (see the window-state handler)
    armed: Cell<bool>,
    /// right-column links by Customize Start Menu key, shown or hidden on every open
    links: RefCell<Vec<(&'static str, gtk::Widget)>>,
    power_btn: RefCell<Option<gtk::Button>>,
}

const CATS: &[(&str, &str, &str)] = &[
    ("Accessories", "Utility", "applications-accessories"),
    ("Development", "Development", "applications-development"),
    ("Education", "Education", "applications-science"),
    ("Games", "Game", "applications-games"),
    ("Graphics", "Graphics", "applications-graphics"),
    ("Internet", "Network", "applications-internet"),
    ("Multimedia", "AudioVideo", "applications-multimedia"),
    ("Office", "Office", "applications-office"),
    ("System Tools", "System", "applications-system"),
    ("Control Panel", "Settings", "preferences-system"),
];

fn avatar_pixbuf(size: i32) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let face = config::home().join(".face");
    let p = if face.exists() { Some(face) } else { config::data_file("avatar.png") };
    p.and_then(|p| gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, size, size, false).ok())
        .or_else(|| util::pixbuf("avatar-default", size))
}

impl StartMenu {
    pub fn new(app: &gtk::Application) -> Rc<StartMenu> {
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.set_application(Some(app));
        win.set_title("Start menu");
        win.set_decorated(false);
        win.set_type_hint(gdk::WindowTypeHint::Utility);
        win.set_skip_taskbar_hint(true);
        win.set_skip_pager_hint(true);
        win.set_keep_above(true);
        win.set_app_paintable(true);
        crate::glass::enable_rgba(&win);
        win.set_size_request(W, H);
        win.style_context().add_class("aerode");
        win.style_context().add_class("startmenu");

        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.set_margin_start(7);
        root.set_margin_top(7);
        root.set_margin_bottom(8);
        win.add(&root);

        // ---- left white pane + search
        let leftcol = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.style_context().add_class("sm-left");
        left.set_size_request(LEFT_W, -1);
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.set_margin_top(6);
        sw.add(&list);
        left.pack_start(&sw, true, true, 0);
        left.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
        let all = gtk::Button::new();
        all.style_context().add_class("sm-item");
        all.style_context().add_class("sm-allprogs");
        let all_lbl = gtk::Label::new(Some("All Programs"));
        let ab = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        ab.set_margin_start(6);
        let arrow = gtk::Label::new(Some("▶"));
        ab.pack_start(&arrow, false, false, 0);
        ab.pack_start(&all_lbl, false, false, 0);
        all.add(&ab);
        left.pack_start(&all, false, false, 4);
        leftcol.pack_start(&left, true, true, 0);
        let search = gtk::Entry::new();
        search.set_placeholder_text(Some("Search programs and files"));
        search.style_context().add_class("sm-search");
        search.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("system-search-symbolic"));
        search.set_margin_top(9);
        search.set_margin_bottom(2);
        search.set_margin_start(10);
        search.set_margin_end(10);
        leftcol.pack_start(&search, false, false, 0);
        root.pack_start(&leftcol, false, false, 0);

        // ---- right glass column
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.style_context().add_class("sm-right");
        right.set_hexpand(true);
        let avatar = gtk::Image::new();
        if let Some(p) = avatar_pixbuf(48) { avatar.set_from_pixbuf(Some(&p)); }
        let av_frame = gtk::Frame::new(None);
        av_frame.add(&avatar);
        av_frame.set_halign(gtk::Align::Center);
        av_frame.set_margin_top(4);
        av_frame.set_margin_bottom(10);
        right.pack_start(&av_frame, false, false, 0);

        let sm = Rc::new(StartMenu {
            win: win.clone(), list, search: search.clone(), avatar: avatar.clone(), mode: Cell::new(0),
            all_btn_label: all_lbl, expanded: RefCell::new(vec![]), on_hide: RefCell::new(None), hidden_at: Cell::new(None),
            armed: Cell::new(false),
            links: RefCell::new(vec![]), power_btn: RefCell::new(None),
        });

        let user = glib::real_name().to_string_lossy().into_owned();
        let user = if user.is_empty() || user == "Unknown" { glib::user_name().to_string_lossy().into_owned() } else { user };
        let home = config::home();
        type Act = Rc<dyn Fn()>;
        let ex = |p: std::path::PathBuf| -> Act { Rc::new(move || util::explore(&p.to_string_lossy())) };
        let ctrl = |page: &'static str| -> Act { Rc::new(move || { util::spawn(&["aerode-control", page]); }) };
        let recent: Act = Rc::new(|| {
            // Recent Items: the files GTK programs recorded, newest first
            let Some(rm) = gtk::RecentManager::default() else { return };
            let mut items = rm.items();
            items.sort_by_key(|i| std::cmp::Reverse(i.modified()));
            let m = gtk::Menu::new();
            for i in items.iter().take(config::get().taskbar.jump_items.max(1) as usize) {
                let Some(uri) = i.uri() else { continue };
                let it = gtk::MenuItem::with_label(&i.display_name().map(|s| s.to_string()).unwrap_or_else(|| uri.to_string()));
                let u = uri.to_string();
                it.connect_activate(move |_| { util::open_uri(&u); });
                m.append(&it);
            }
            if m.children().is_empty() { let e = gtk::MenuItem::with_label("(Empty)"); e.set_sensitive(false); m.append(&e); }
            m.show_all();
            m.popup_at_pointer(None);
        });
        let links: Vec<Option<(&'static str, String, &str, Act)>> = vec![
            Some(("user", user.clone(), "user-home", ex(home.clone()))),
            Some(("documents", "Documents".into(), "folder-documents", ex(util::special_dir(glib::UserDirectory::Documents, "Documents")))),
            Some(("pictures", "Pictures".into(), "folder-pictures", ex(util::special_dir(glib::UserDirectory::Pictures, "Pictures")))),
            Some(("music", "Music".into(), "folder-music", ex(util::special_dir(glib::UserDirectory::Music, "Music")))),
            Some(("videos", "Videos".into(), "folder-videos", ex(util::special_dir(glib::UserDirectory::Videos, "Videos")))),
            Some(("downloads", "Downloads".into(), "folder-download", ex(util::special_dir(glib::UserDirectory::Downloads, "Downloads")))),
            None,
            Some(("games", "Games".into(), util::first_icon(&["folder-games", "applications-games"]), Rc::new(|| util::explore("aerode:games")))),
            Some(("recent", "Recent Items".into(), util::first_icon(&["document-open-recent", "folder-recent"]), recent)),
            Some(("computer", "Computer".into(), "computer", Rc::new(|| util::explore("computer:///")))),
            Some(("network", "Network".into(), "network-workgroup", Rc::new(|| util::explore("network:///")))),
            None,
            Some(("control", "Control Panel".into(), "preferences-system", ctrl(""))),
            Some(("devices", "Devices and Printers".into(), "printer", ctrl("devices"))),
            Some(("defaults", "Default Programs".into(), "preferences-desktop-default-applications", ctrl("defaults"))),
            Some(("help", "Help and Support".into(), "help-browser", Rc::new(|| { util::open_uri("https://wiki.archlinux.org/"); }))),
            Some(("run", "Run...".into(), "system-run", Rc::new(|| crate::dialogs::run_dialog()))),
        ];
        for l in links {
            match l {
                None => right.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0),
                Some((key, label, icon, act)) => {
                    let b = gtk::Button::with_label(&label);
                    b.style_context().add_class("sm-link");
                    if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_xalign(0.0); }
                    let s = sm.clone();
                    b.connect_clicked(move |_| { s.hide(); act(); });
                    // Win7 swaps the user picture for the hovered item's icon
                    let (av, icon) = (avatar.clone(), icon.to_string());
                    b.connect_enter_notify_event(move |_, _| { if let Some(p) = util::pixbuf(&icon, 48) { av.set_from_pixbuf(Some(&p)); } glib::Propagation::Proceed });
                    let av = avatar.clone();
                    b.connect_leave_notify_event(move |_, _| { if let Some(p) = avatar_pixbuf(48) { av.set_from_pixbuf(Some(&p)); } glib::Propagation::Proceed });
                    right.pack_start(&b, false, false, 1);
                    sm.links.borrow_mut().push((key, b.upcast()));
                }
            }
        }
        // shut down button + options
        let pw = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pw.set_halign(gtk::Align::Start);
        pw.set_margin_start(16);
        pw.set_valign(gtk::Align::End);
        let shut = gtk::Button::with_label("Shut down");
        shut.style_context().add_class("sm-power");
        let arrow = gtk::Button::with_label("▸");
        arrow.style_context().add_class("sm-power");
        arrow.style_context().add_class("arrow");
        // Taskbar and Start Menu Properties > Power button action
        { let s = sm.clone(); shut.connect_clicked(move |_| { s.hide(); power_action(&config::get().taskbar.power_button)(); }); }
        *sm.power_btn.borrow_mut() = Some(shut.clone());
        {
            let s = sm.clone();
            arrow.connect_clicked(move |a| {
                let m = gtk::Menu::new();
                // the menu offers every action except the button's own
                let cur = config::get().taskbar.power_button;
                let mut items: Vec<(&str, &str)> = vec![("switchuser", "Switch user"), ("logoff", "Log off"), ("lock", "Lock"), ("", ""),
                    ("restart", "Restart"), ("sleep", "Sleep"), ("hibernate", "Hibernate"), ("shutdown", "Shut down")];
                items.retain(|i| i.0 != cur.as_str() && !(i.0 == "shutdown" && cur.is_empty()));
                for (id, l) in items {
                    if l.is_empty() { m.append(&gtk::SeparatorMenuItem::new()); continue; }
                    let f = power_action(id);
                    let it = gtk::MenuItem::with_label(l);
                    let s = s.clone();
                    it.connect_activate(move |_| { s.hide(); f(); });
                    m.append(&it);
                }
                m.show_all();
                m.popup_at_widget(a, gdk::Gravity::NorthEast, gdk::Gravity::NorthWest, None);
            });
        }
        pw.pack_start(&shut, false, false, 0);
        pw.pack_start(&arrow, false, false, 0);
        right.pack_end(&pw, false, false, 6);
        root.pack_start(&right, true, true, 0);

        // ---- behaviour
        { let s = sm.clone(); all.connect_clicked(move |_| { s.mode.set(if s.mode.get() == 1 { 0 } else { 1 }); s.populate(); }); }
        {
            let s = sm.clone();
            search.connect_changed(move |e| { s.mode.set(if e.text().is_empty() { 0 } else { 2 }); s.populate(); });
        }
        {
            let s = sm.clone();
            search.connect_activate(move |e| {
                let q = e.text().to_string();
                if let Some(first) = s.list.children().into_iter().find_map(|c| c.downcast::<gtk::Button>().ok()) {
                    first.clicked();
                } else if !q.is_empty() {
                    s.hide();
                    util::run_command(&q);
                }
            });
        }
        {
            let s = sm.clone();
            win.connect_key_press_event(move |_, e| {
                let k = e.keyval();
                if k == gdk::keys::constants::Escape || k == gdk::keys::constants::Super_L {
                    s.hide();
                    return glib::Propagation::Stop;
                }
                if !s.search.has_focus() && k.to_unicode().map(|c| !c.is_control()).unwrap_or(false) {
                    s.search.grab_focus_without_selecting();
                }
                glib::Propagation::Proceed
            });
        }
        // Close when the WM makes another window active (_NET_WM_STATE_FOCUSED, which GDK reports as the
        // FOCUSED window state), not on every focus-out: the WM's passive grab on the Windows key and the
        // menu's own popups (power options, item context menus) cause NotifyGrab focus-outs. Hiding on those
        // closed the menu on the key *press*, so the window behind it was activated while the key was still
        // held (its WM caption at the press, its own content only at the release) and a long press reopened
        // the menu; it also closed the menu as soon as the power-options arrow was clicked. Armed by the
        // first focus-in after show(), so a stale FOCUSED flag from the previous opening cannot close it.
        { let s = sm.clone(); win.connect_focus_in_event(move |_, _| { s.armed.set(true); glib::Propagation::Proceed }); }
        {
            let s = sm.clone();
            win.connect_window_state_event(move |_, e| {
                let lost = e.changed_mask().contains(gdk::WindowState::FOCUSED) && !e.new_window_state().contains(gdk::WindowState::FOCUSED);
                if lost && s.armed.get() { s.hide(); }
                glib::Propagation::Proceed
            });
        }
        win.connect_draw(move |w, cr| {
            let (ww, hh) = (w.allocated_width() as f64, w.allocated_height() as f64);
            let (x, y) = w.position();
            glass::paint_glass(cr, ww, hh, x as f64, y as f64, 6.0, true);
            glib::Propagation::Proceed
        });
        root.show_all();
        { let s2 = Rc::downgrade(&sm); HIDE_ALL.with(|h| *h.borrow_mut() = Some(Rc::new(move || if let Some(s) = s2.upgrade() { s.hide() }))); }
        sm
    }

    pub fn visible(&self) -> bool {
        self.win.is_visible()
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.visible() { self.hide(); return; }
        // a Windows-key press that reaches the menu itself (no WM key grab) hides it in the key handler;
        // the `aerode start` sent for that same press must not then reopen it
        if self.hidden_at.get().map(|t| t.elapsed().as_millis() < 400).unwrap_or(false) { return; }
        self.show()
    }

    pub fn show(self: &Rc<Self>) {
        crate::flyouts::close_open();
        self.mode.set(0);
        self.search.set_text("");
        self.expanded.borrow_mut().clear();
        self.apply_options();
        self.populate();
        let g = util::primary_geometry();
        self.win.move_(g.x(), g.y() + g.height() - config::get().taskbar_height - H);
        self.armed.set(false);
        self.win.show();
        self.win.present_with_time(gtk::current_event_time());
        self.search.grab_focus();
    }

    /// Customize Start Menu items and the power button label (set in Control Panel).
    fn apply_options(&self) {
        let tb = config::get().taskbar;
        for (key, w) in self.links.borrow().iter() {
            let v = start_item(key);
            w.set_visible(v != "hide" && v != "off" && !(*key == "recent" && !tb.track_items));
        }
        if let Some(b) = self.power_btn.borrow().as_ref() {
            b.set_label(POWER_ACTIONS.iter().find(|a| a.0 == tb.power_button).map(|a| a.1).unwrap_or("Shut down"));
        }
    }

    pub fn hide(&self) {
        if self.win.is_visible() {
            self.hidden_at.set(Some(std::time::Instant::now()));
            self.armed.set(false);
            self.win.hide();
            if let Some(f) = self.on_hide.borrow().as_ref() { f(); }
        }
    }

    fn item(self: &Rc<Self>, app: &gio::AppInfo, size: i32, indent: i32) -> gtk::Button {
        let b = gtk::Button::new();
        b.style_context().add_class("sm-item");
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        h.set_margin_start(indent);
        h.pack_start(&util::app_image(app, size), false, false, 0);
        let l = gtk::Label::new(Some(&app.display_name()));
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        h.pack_start(&l, true, true, 0);
        b.add(&h);
        if let Some(d) = app.description() { b.set_tooltip_text(Some(&d)); }
        let (s, a) = (self.clone(), app.clone());
        b.connect_clicked(move |_| { s.hide(); util::launch(&a); });
        let a = app.clone();
        b.connect_button_press_event(move |b, e| {
            if e.button() == 3 {
                let m = gtk::Menu::new();
                let open = gtk::MenuItem::with_label("Open");
                let a2 = a.clone();
                open.connect_activate(move |_| util::launch(&a2));
                m.append(&open);
                if let Some(id) = a.id() {
                    let pin = gtk::MenuItem::with_label("Pin to Taskbar");
                    let id = id.to_string();
                    pin.connect_activate(move |_| {
                        let mut p = util::pinned();
                        if !p.contains(&id) { p.push(id.clone()); config::update(|c| c.pinned = Some(p)); }
                    });
                    m.append(&pin);
                }
                m.show_all();
                m.popup_at_pointer(Some(e));
                let _ = b;
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        b
    }

    fn populate(self: &Rc<Self>) {
        for c in self.list.children() { self.list.remove(&c); }
        let apps = util::all_apps();
        match self.mode.get() {
            0 => {
                self.all_btn_label.set_text("All Programs");
                // Privacy: "Store and display recently opened programs" off = an empty list, like Win7
                let tb = config::get().taskbar;
                let n = if tb.track_programs { tb.recent_programs as usize } else { 0 };
                let isz = if start_item("largeicons") == "off" { 16 } else { 32 };
                let usage = config::get().usage;
                let mut freq: Vec<_> = apps.iter().filter(|a| a.id().map(|i| usage.contains_key(i.as_str())).unwrap_or(false)).cloned().collect();
                freq.sort_by_key(|a| std::cmp::Reverse(usage.get(a.id().unwrap().as_str()).copied().unwrap_or(0)));
                // seed with sensible defaults on first run
                let seeds = ["aerode-explorer.desktop", "aerode-notepad.desktop", "aerode-calc.desktop", "aerode-mspaint.desktop", "aerode-taskmgr.desktop"];
                for s in seeds {
                    if freq.len() >= n { break; }
                    if let Some(a) = util::app_by_id(s) { if !freq.iter().any(|f| f.id() == a.id()) { freq.push(a); } }
                }
                if let Some(t) = util::terminal() { if freq.len() < n && !freq.iter().any(|f| f.id() == t.id()) { freq.push(t); } }
                for a in freq.iter().take(n) {
                    self.list.pack_start(&self.item(a, isz, 0), false, false, 1);
                }
            }
            1 => {
                self.all_btn_label.set_text("Back");
                let mut used = std::collections::HashSet::new();
                for (name, cat, icon) in CATS {
                    let members: Vec<_> = apps.iter().filter(|a| util::categories(a).split(';').any(|c| c == *cat)).cloned().collect();
                    if members.is_empty() { continue; }
                    let open = self.expanded.borrow().iter().any(|e| e == name);
                    let b = gtk::Button::new();
                    b.style_context().add_class("sm-item");
                    let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    h.pack_start(&util::image(util::first_icon(&["folder"]), 16), false, false, 0);
                    let _ = icon;
                    h.pack_start(&gtk::Label::new(Some(name)), false, false, 0);
                    b.add(&h);
                    let (s, n) = (self.clone(), name.to_string());
                    b.connect_clicked(move |_| {
                        { let mut e = s.expanded.borrow_mut(); if let Some(i) = e.iter().position(|x| x == &n) { e.remove(i); } else { e.push(n.clone()); } }
                        s.populate();
                    });
                    self.list.pack_start(&b, false, false, 0);
                    for a in &members {
                        used.insert(a.id().map(|s| s.to_string()).unwrap_or_default());
                        if open { self.list.pack_start(&self.item(a, 16, 18), false, false, 0); }
                    }
                }
                for a in apps.iter().filter(|a| !used.contains(&a.id().map(|s| s.to_string()).unwrap_or_default())) {
                    self.list.pack_start(&self.item(a, 16, 0), false, false, 0);
                }
            }
            _ => {
                self.all_btn_label.set_text("See more results");
                let q = self.search.text().to_lowercase();
                let hits: Vec<_> = apps.iter().filter(|a| {
                    a.display_name().to_lowercase().contains(&q)
                        || a.description().map(|d| d.to_lowercase().contains(&q)).unwrap_or(false)
                        || a.downcast_ref::<gio::DesktopAppInfo>().and_then(|d| d.string("Keywords")).map(|k| k.to_lowercase().contains(&q)).unwrap_or(false)
                        || a.executable().to_string_lossy().to_lowercase().contains(&q)
                }).cloned().collect();
                let head = gtk::Label::new(Some(&format!("Programs ({})", hits.len())));
                head.style_context().add_class("sm-heading");
                head.set_xalign(0.0);
                self.list.pack_start(&head, false, false, 0);
                for a in hits.iter().take(8) { self.list.pack_start(&self.item(a, 16, 6), false, false, 0); }
                // quick file search in home (names only, shallow)
                let files = file_search(&q, 6);
                if !files.is_empty() {
                    let head = gtk::Label::new(Some(&format!("Files ({})", files.len())));
                    head.style_context().add_class("sm-heading");
                    head.set_xalign(0.0);
                    self.list.pack_start(&head, false, false, 0);
                    for f in files {
                        let b = gtk::Button::new();
                        b.style_context().add_class("sm-item");
                        let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        h.set_margin_start(6);
                        let gf = gio::File::for_path(&f);
                        let icon = gf.query_info("standard::icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok().and_then(|i| i.icon());
                        h.pack_start(&gtk::Image::from_pixbuf(util::gicon_pixbuf(icon.as_ref(), 16).as_ref()), false, false, 0);
                        let l = gtk::Label::new(f.file_name().map(|s| s.to_string_lossy().into_owned()).as_deref());
                        l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                        h.pack_start(&l, false, false, 0);
                        b.add(&h);
                        b.set_tooltip_text(Some(&f.to_string_lossy()));
                        let s = self.clone();
                        b.connect_clicked(move |_| { s.hide(); util::open_path(&f); });
                        self.list.pack_start(&b, false, false, 0);
                    }
                }
            }
        }
        self.list.show_all();
    }
}

fn file_search(q: &str, limit: usize) -> Vec<std::path::PathBuf> {
    if q.len() < 2 { return vec![]; }
    let mut out = vec![];
    let mut stack = vec![(config::home(), 0)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            visited += 1;
            if visited > 20000 || out.len() >= limit { return out; }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') { continue; }
            let p = e.path();
            if name.contains(q) { out.push(p.clone()); }
            if depth < 4 && e.file_type().map(|t| t.is_dir()).unwrap_or(false) { stack.push((p, depth + 1)); }
        }
    }
    out
}
