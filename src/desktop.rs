//! Desktop window: wallpaper, icons, context menu.
use gtk::gdk::prelude::GdkContextExt;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::{config, glass, util};

#[derive(Clone)]
enum Target {
    Uri(String),
    Path(PathBuf),
}

struct Item {
    label: String,
    icon: Option<gio::Icon>,
    icon_name: Option<String>,
    target: Target,
    special: Option<&'static str>,
}

pub struct Desktop {
    win: gtk::Window,
    fixed: gtk::Fixed,
    selected: RefCell<Vec<gtk::Button>>,
    _monitor: RefCell<Option<gio::FileMonitor>>,
    /// rubber-band selection: (start x, start y, current x, current y, ctrl held)
    band: RefCell<Option<(f64, f64, f64, f64, bool)>>,
    band_area: gtk::Box,
    band_base: RefCell<Vec<gtk::Button>>,
}

fn desktop_dir() -> PathBuf {
    util::special_dir(glib::UserDirectory::Desktop, "Desktop")
}

fn trash_full() -> bool {
    std::fs::read_dir(glib::user_data_dir().join("Trash/files")).map(|mut r| r.next().is_some()).unwrap_or(false)
}

impl Desktop {
    pub fn new(app: &gtk::Application) -> Rc<Desktop> {
        let g = util::primary_geometry();
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.set_application(Some(app));
        win.set_title("Desktop");
        win.set_type_hint(gdk::WindowTypeHint::Desktop);
        win.set_decorated(false);
        win.set_skip_taskbar_hint(true);
        win.set_skip_pager_hint(true);
        win.set_keep_below(true);
        win.stick();
        win.set_app_paintable(true);
        win.move_(g.x(), g.y());
        win.set_size_request(g.width(), g.height());
        win.style_context().add_class("aerode");
        win.style_context().add_class("desktop");
        let ev = gtk::EventBox::new();
        ev.set_visible_window(false);
        ev.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::BUTTON1_MOTION_MASK | gdk::EventMask::POINTER_MOTION_MASK);
        let fixed = gtk::Fixed::new();
        ev.add(&fixed);
        // the selection rectangle is drawn on an overlay that lets all input through
        let overlay = gtk::Overlay::new();
        overlay.add(&ev);
        // a window-less widget: a DrawingArea has its own input window, which pass-through does not
        // cover, and it swallowed every click on the desktop
        let band_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
        band_area.set_halign(gtk::Align::Fill);
        band_area.set_valign(gtk::Align::Fill);
        overlay.add_overlay(&band_area);
        overlay.set_overlay_pass_through(&band_area, true);
        win.add(&overlay);
        win.connect_draw(|_, cr| {
            let wp = glass::wallpaper();
            cr.set_source_pixbuf(&wp.sharp, 0.0, 0.0);
            cr.paint().ok();
            glib::Propagation::Proceed
        });
        let d = Rc::new(Desktop { win: win.clone(), fixed, selected: RefCell::new(vec![]), _monitor: RefCell::new(None),
            band: RefCell::new(None), band_area: band_area.clone(), band_base: RefCell::new(vec![]) });
        {
            let d2 = d.clone();
            ev.connect_button_press_event(move |_, e| {
                // clicking the desktop closes the Start menu and tray flyouts, like Windows
                crate::flyouts::close_open();
                crate::startmenu::hide_all();
                let ctrl = e.state().contains(gdk::ModifierType::CONTROL_MASK);
                if !ctrl { d2.clear_selection(); }
                match e.button() {
                    1 => {
                        let (x, y) = e.position();
                        *d2.band_base.borrow_mut() = d2.selected.borrow().clone();
                        *d2.band.borrow_mut() = Some((x, y, x, y, ctrl));
                        d2.win.present();
                    }
                    3 => d2.desktop_menu(e),
                    _ => {}
                }
                glib::Propagation::Stop
            });
        }
        {
            let d2 = d.clone();
            ev.connect_motion_notify_event(move |_, e| {
                let cur = *d2.band.borrow();
                if let Some((x0, y0, _, _, ctrl)) = cur {
                    let (x, y) = e.position();
                    *d2.band.borrow_mut() = Some((x0, y0, x, y, ctrl));
                    d2.update_band_selection();
                    d2.band_area.queue_draw();
                }
                glib::Propagation::Stop
            });
        }
        {
            let d2 = d.clone();
            ev.connect_button_release_event(move |_, _| {
                if d2.band.borrow_mut().take().is_some() { d2.band_area.queue_draw(); }
                glib::Propagation::Stop
            });
        }
        {
            let d2 = d.clone();
            band_area.connect_draw(move |_, cr| {
                if let Some((x0, y0, x1, y1, _)) = *d2.band.borrow() {
                    let (x, y, w, h) = (x0.min(x1).floor() + 0.5, y0.min(y1).floor() + 0.5, (x1 - x0).abs().floor(), (y1 - y0).abs().floor());
                    // Windows 7 selection rectangle: translucent highlight fill + solid highlight border
                    cr.rectangle(x, y, w, h);
                    cr.set_source_rgba(0.2, 0.6, 1.0, 0.25);
                    cr.fill_preserve().ok();
                    cr.set_source_rgba(0.2, 0.6, 1.0, 0.9);
                    cr.set_line_width(1.0);
                    cr.stroke().ok();
                }
                glib::Propagation::Proceed
            });
        }
        {
            let d2 = d.clone();
            win.connect_key_press_event(move |_, e| {
                match e.keyval() {
                    k if k == gdk::keys::constants::F5 => d2.rebuild(),
                    k if k == gdk::keys::constants::Delete => d2.delete_selected(),
                    _ => {}
                }
                glib::Propagation::Proceed
            });
        }
        { let w = win.clone(); glass::on_change(move || w.queue_draw()); }
        // watch ~/Desktop and the trash
        let dir = desktop_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(m) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            let d2 = Rc::downgrade(&d);
            let pending = Rc::new(std::cell::Cell::new(false));
            m.connect_changed(move |_, _, _, _| {
                if pending.replace(true) { return; }
                let (d2, p) = (d2.clone(), pending.clone());
                glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
                    p.set(false);
                    if let Some(d) = d2.upgrade() { d.rebuild(); }
                });
            });
            *d._monitor.borrow_mut() = Some(m);
        }
        d.rebuild();
        win.show_all();
        d
    }

    fn items(&self) -> Vec<Item> {
        let mut v = vec![];
        for s in config::get().desktop_icons {
            match s.as_str() {
                "computer" => v.push(Item { label: "Computer".into(), icon: None, icon_name: Some("computer".into()), target: Target::Uri("computer:///".into()), special: Some("computer") }),
                "home" => v.push(Item { label: glib::user_name().to_string_lossy().into_owned(), icon: None, icon_name: Some("user-home".into()), target: Target::Path(config::home()), special: None }),
                "trash" => v.push(Item { label: "Recycle Bin".into(), icon: None, icon_name: Some(if trash_full() { "user-trash-full" } else { "user-trash" }.into()), target: Target::Uri("trash:///".into()), special: Some("trash") }),
                "network" => v.push(Item { label: "Network".into(), icon: None, icon_name: Some("network-workgroup".into()), target: Target::Uri("network:///".into()), special: None }),
                "control" => v.push(Item { label: "Control Panel".into(), icon: None, icon_name: Some("preferences-system".into()), target: Target::Uri("aerode:control".into()), special: Some("control") }),
                _ => {}
            }
        }
        let mut files: Vec<_> = std::fs::read_dir(desktop_dir()).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        files.retain(|p: &PathBuf| !p.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(true));
        let cfg = config::get();
        let name_of = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let ext_of = |p: &PathBuf| p.extension().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let meta = |p: &PathBuf| std::fs::metadata(p).ok();
        files.sort_by(|a, b| {
            let o = match cfg.desktop_sort.as_str() {
                "size" => meta(a).map(|m| m.len()).cmp(&meta(b).map(|m| m.len())),
                "type" => ext_of(a).cmp(&ext_of(b)),
                "date" => meta(a).and_then(|m| m.modified().ok()).cmp(&meta(b).and_then(|m| m.modified().ok())),
                _ => std::cmp::Ordering::Equal,
            }.then_with(|| name_of(a).cmp(&name_of(b)));
            let o = if cfg.desktop_sort_desc { o.reverse() } else { o };
            b.is_dir().cmp(&a.is_dir()).then(o)
        });
        for p in files {
            if p.extension().map(|e| e == "desktop").unwrap_or(false) {
                if let Some(a) = gio::DesktopAppInfo::from_filename(&p) {
                    v.push(Item { label: a.display_name().to_string(), icon: a.icon(), icon_name: None, target: Target::Path(p), special: Some("launcher") });
                    continue;
                }
            }
            let gf = gio::File::for_path(&p);
            let info = gf.query_info("standard::icon,standard::display-name", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
            v.push(Item {
                label: info.as_ref().map(|i| i.display_name().to_string()).unwrap_or_else(|| p.file_name().unwrap().to_string_lossy().into_owned()),
                icon: info.and_then(|i| i.icon()),
                icon_name: None,
                target: Target::Path(p),
                special: None,
            });
        }
        v
    }

    pub fn rebuild(self: &Rc<Self>) {
        for c in self.fixed.children() { self.fixed.remove(&c); }
        self.selected.borrow_mut().clear();
        let size = config::get().icon_size;
        let g = util::primary_geometry();
        let (cw, ch) = (size + 30, size + 44);
        let rows = ((g.height() - config::get().taskbar_height - 8) / (ch + 4)).max(1);
        for (i, it) in self.items().into_iter().enumerate() {
            let (col, row) = (i as i32 / rows, i as i32 % rows);
            let w = self.icon_widget(it, size, cw);
            self.fixed.put(&w, 4 + col * (cw + 6), 4 + row * (ch + 4));
        }
        self.fixed.show_all();
    }

    fn icon_widget(self: &Rc<Self>, it: Item, size: i32, cw: i32) -> gtk::Button {
        let eb = gtk::Button::new();
        eb.set_relief(gtk::ReliefStyle::None);
        eb.set_focus_on_click(false);
        eb.style_context().add_class("desk-icon");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 2);
        v.set_size_request(cw, -1);
        let pb = match (&it.icon, &it.icon_name) {
            (Some(i), _) => util::gicon_pixbuf(Some(i), size),
            (None, Some(n)) => util::pixbuf_or(n, size, "text-x-generic"),
            _ => util::pixbuf("text-x-generic", size),
        };
        v.pack_start(&gtk::Image::from_pixbuf(pb.as_ref()), false, false, 0);
        let l = gtk::Label::new(Some(&it.label));
        l.set_line_wrap(true);
        l.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
        l.set_lines(2);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_justify(gtk::Justification::Center);
        l.set_max_width_chars(10);
        v.pack_start(&l, false, false, 0);
        eb.add(&v);
        let it = Rc::new(it);
        let d = Rc::downgrade(self);
        let it2 = it.clone();
        eb.connect_button_press_event(move |eb, e| {
            let Some(d) = d.upgrade() else { return glib::Propagation::Stop };
            if e.event_type() == gdk::EventType::DoubleButtonPress && e.button() == 1 {
                open_item(&it2);
                return glib::Propagation::Stop;
            }
            if !e.state().contains(gdk::ModifierType::CONTROL_MASK) { d.clear_selection(); }
            eb.style_context().add_class("selected");
            d.selected.borrow_mut().push(eb.clone());
            eb.queue_draw();
            d.win.present();
            if e.button() == 3 { d.item_menu(&it2, e); }
            glib::Propagation::Stop
        });
        unsafe { eb.set_data("path", match &it.target { Target::Path(p) => p.clone(), Target::Uri(u) => PathBuf::from(u) }); }
        eb
    }

    /// Select every icon the rubber band touches (plus the pre-existing selection with Ctrl).
    fn update_band_selection(&self) {
        let Some((x0, y0, x1, y1, ctrl)) = *self.band.borrow() else { return };
        let (rx0, ry0, rx1, ry1) = (x0.min(x1) as i32, y0.min(y1) as i32, x0.max(x1) as i32, y0.max(y1) as i32);
        let base = self.band_base.borrow().clone();
        let mut sel: Vec<gtk::Button> = if ctrl { base } else { vec![] };
        for child in self.fixed.children() {
            let Ok(b) = child.downcast::<gtk::Button>() else { continue };
            let a = b.allocation();
            let hit = a.x() < rx1 && a.x() + a.width() > rx0 && a.y() < ry1 && a.y() + a.height() > ry0;
            if hit && !sel.contains(&b) { sel.push(b); }
        }
        for w in self.selected.borrow().iter() { if !sel.contains(w) { w.style_context().remove_class("selected"); w.queue_draw(); } }
        for w in &sel { w.style_context().add_class("selected"); w.queue_draw(); }
        *self.selected.borrow_mut() = sel;
    }

    fn clear_selection(&self) {
        for w in self.selected.borrow_mut().drain(..) {
            w.style_context().remove_class("selected");
            w.queue_draw();
        }
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.selected.borrow().iter().filter_map(|w| unsafe { w.data::<PathBuf>("path").map(|p| p.as_ref().clone()) }).filter(|p| p.is_absolute()).collect()
    }

    fn delete_selected(self: &Rc<Self>) {
        for p in self.selected_paths() {
            let _ = gio::File::for_path(&p).trash(gio::Cancellable::NONE);
        }
        util::sound(crate::sounds::Event::EmptyRecycleBin);
        self.rebuild();
    }

    fn item_menu(self: &Rc<Self>, it: &Rc<Item>, e: &gdk::EventButton) {
        let m = gtk::Menu::new();
        let add = |m: &gtk::Menu, l: &str, f: Box<dyn Fn()>| { let i = gtk::MenuItem::with_label(l); i.connect_activate(move |_| f()); m.append(&i); };
        { let it = it.clone(); add(&m, "Open", Box::new(move || open_item(&it))); }
        match it.special {
            Some("trash") => {
                let d = Rc::downgrade(self);
                add(&m, "Empty Recycle Bin", Box::new(move || {
                    if confirm("Delete Multiple Items", "Are you sure you want to permanently delete all of the items in the Recycle Bin?") {
                        util::spawn(&["gio", "trash", "--empty"]);
                        util::sound(crate::sounds::Event::EmptyRecycleBin);
                        let d = d.clone();
                        glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || { if let Some(d) = d.upgrade() { d.rebuild(); } });
                    }
                }));
            }
            Some("computer") => {
                add(&m, "Manage", Box::new(|| { util::spawn(&["aerode-taskmgr"]); }));
                add(&m, "Properties", Box::new(|| { util::spawn(&["aerode-control", "system"]); }));
            }
            _ => {
                if let Target::Path(p) = &it.target {
                    if p.starts_with(desktop_dir()) {
                        m.append(&gtk::SeparatorMenuItem::new());
                        let d = Rc::downgrade(self);
                        add(&m, "Delete", Box::new(move || if let Some(d) = d.upgrade() { d.delete_selected() }));
                        let p2 = p.clone();
                        let d = Rc::downgrade(self);
                        add(&m, "Rename", Box::new(move || {
                            if let Some(n) = prompt("Rename", "New name:", &p2.file_name().unwrap().to_string_lossy()) {
                                let _ = std::fs::rename(&p2, p2.with_file_name(n));
                                if let Some(d) = d.upgrade() { d.rebuild(); }
                            }
                        }));
                        let p3 = p.clone();
                        add(&m, "Properties", Box::new(move || { util::spawn(&["aerode-explorer", "--properties", &p3.to_string_lossy()]); }));
                    }
                }
            }
        }
        m.show_all();
        m.popup_at_pointer(Some(e));
    }

    fn desktop_menu(self: &Rc<Self>, e: &gdk::EventButton) {
        let m = gtk::Menu::new();
        let view = gtk::Menu::new();
        for (l, s) in [("Large icons", 64), ("Medium icons", 48), ("Small icons", 32)] {
            let i = gtk::CheckMenuItem::with_label(l);
            i.set_draw_as_radio(true);
            i.set_active(config::get().icon_size == s);
            let d = Rc::downgrade(self);
            i.connect_activate(move |_| { config::update(|c| c.icon_size = s); if let Some(d) = d.upgrade() { d.rebuild(); } });
            view.append(&i);
        }
        view.append(&gtk::SeparatorMenuItem::new());
        let show = gtk::CheckMenuItem::with_label("Show desktop icons");
        show.set_active(true);
        let fx = self.fixed.clone();
        show.connect_toggled(move |c| fx.set_visible(c.is_active()));
        view.append(&show);
        let vi = gtk::MenuItem::with_label("View");
        vi.set_submenu(Some(&view));
        m.append(&vi);
        let refresh = gtk::MenuItem::with_label("Refresh");
        { let d = Rc::downgrade(self); refresh.connect_activate(move |_| if let Some(d) = d.upgrade() { d.rebuild() }); }
        let sort = gtk::Menu::new();
        let cur = config::get().desktop_sort;
        let mut group: Option<gtk::RadioMenuItem> = None;
        for (l, key) in [("Name", "name"), ("Size", "size"), ("Item type", "type"), ("Date modified", "date")] {
            let i = match &group { Some(g) => gtk::RadioMenuItem::with_label_from_widget(g, Some(l)), None => gtk::RadioMenuItem::with_label(l) };
            i.set_active(cur == key);
            group.get_or_insert_with(|| i.clone());
            let d = Rc::downgrade(self);
            // choosing the current key again flips the order, like Explorer
            i.connect_activate(move |_| {
                config::update(|c| { if c.desktop_sort == key { c.desktop_sort_desc = !c.desktop_sort_desc } else { c.desktop_sort = key.into(); c.desktop_sort_desc = false } });
                if let Some(d) = d.upgrade() { d.rebuild(); }
            });
            sort.append(&i);
        }
        let si = gtk::MenuItem::with_label("Sort by");
        si.set_submenu(Some(&sort));
        m.append(&si);
        m.append(&refresh);
        m.append(&gtk::SeparatorMenuItem::new());
        let clip = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        let has_files = clip.wait_is_target_available(&gdk::Atom::intern("x-special/gnome-copied-files")) || clip.wait_is_uris_available();
        let paste = gtk::MenuItem::with_label("Paste");
        paste.set_sensitive(has_files);
        {
            let d = Rc::downgrade(self);
            paste.connect_activate(move |_| {
                let d = d.clone();
                crate::explorer::ops::get_clipboard(move |files, cut| {
                    if files.is_empty() { return; }
                    let kind = if cut { crate::explorer::ops::Kind::Move } else { crate::explorer::ops::Kind::Copy };
                    let d = d.clone();
                    crate::explorer::ops::transfer(files, desktop_dir(), kind, move || if let Some(d) = d.upgrade() { d.rebuild() });
                });
            });
        }
        m.append(&paste);
        let paste_sc = gtk::MenuItem::with_label("Paste shortcut");
        paste_sc.set_sensitive(has_files);
        {
            let d = Rc::downgrade(self);
            paste_sc.connect_activate(move |_| {
                let d = d.clone();
                crate::explorer::ops::get_clipboard(move |files, _| {
                    for f in files {
                        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let mut dst = desktop_dir().join(format!("{name} - Shortcut"));
                        let mut n = 2;
                        while dst.exists() { dst = desktop_dir().join(format!("{name} - Shortcut ({n})")); n += 1; }
                        let _ = std::os::unix::fs::symlink(&f, &dst);
                    }
                    if let Some(d) = d.upgrade() { d.rebuild(); }
                });
            });
        }
        m.append(&paste_sc);
        m.append(&gtk::SeparatorMenuItem::new());
        let new = gtk::Menu::new();
        for (l, kind) in [("Folder", 0), ("Shortcut", 2), ("Text Document", 1)] {
            let i = gtk::MenuItem::with_label(l);
            let d = Rc::downgrade(self);
            i.connect_activate(move |_| {
                let dir = desktop_dir();
                let base = match kind { 0 => "New folder", 1 => "New Text Document.txt", _ => "" };
                if kind == 2 {
                    if let Some(cmd) = prompt("Create Shortcut", "Type the location of the item:", "") {
                        let name = cmd.split_whitespace().next().unwrap_or("Shortcut").rsplit('/').next().unwrap().to_string();
                        let f = dir.join(format!("{name}.desktop"));
                        let _ = std::fs::write(&f, format!("[Desktop Entry]\nType=Application\nName={name}\nExec={cmd}\nIcon=application-x-executable\n"));
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755));
                    }
                } else {
                    let mut p = dir.join(base);
                    let mut n = 2;
                    while p.exists() {
                        p = dir.join(if kind == 0 { format!("New folder ({n})") } else { format!("New Text Document ({n}).txt") });
                        n += 1;
                    }
                    let _ = if kind == 0 { std::fs::create_dir(&p) } else { std::fs::write(&p, "") };
                }
                if let Some(d) = d.upgrade() { d.rebuild(); }
            });
            new.append(&i);
        }
        let ni = gtk::MenuItem::with_label("New");
        ni.set_submenu(Some(&new));
        m.append(&ni);
        m.append(&gtk::SeparatorMenuItem::new());
        let res = gtk::MenuItem::with_label("Screen resolution");
        res.connect_activate(|_| { util::spawn(&["aerode-control", "display"]); });
        util::menu_icon(&res, "preferences-desktop-display");
        m.append(&res);
        let gad = gtk::MenuItem::with_label("Gadgets");
        gad.connect_activate(|_| crate::gadgets::show_gallery());
        util::menu_icon(&gad, util::first_icon(&["aerode-gadgets", "preferences-desktop-gadgets"]));
        m.append(&gad);
        let per = gtk::MenuItem::with_label("Personalize");
        per.connect_activate(|_| crate::dialogs::personalize());
        util::menu_icon(&per, "preferences-desktop-wallpaper");
        m.append(&per);
        m.show_all();
        m.popup_at_pointer(Some(e));
    }
}

fn open_item(it: &Item) {
    match (&it.target, it.special) {
        (Target::Path(p), Some("launcher")) => {
            if let Some(a) = gio::DesktopAppInfo::from_filename(p) { util::launch(a.upcast_ref()); }
        }
        (_, Some("control")) => { util::spawn(&["aerode-control"]); }
        (Target::Path(p), _) if p.is_dir() => util::explore(&p.to_string_lossy()),
        (Target::Path(p), _) => { util::open_path(p); }
        (Target::Uri(u), _) => util::explore(u),
    }
}

pub fn confirm(title: &str, text: &str) -> bool {
    let d = gtk::MessageDialog::new(None::<&gtk::Window>, gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::YesNo, text);
    util::sound(crate::sounds::Event::Exclamation);
    d.set_title(title);
    d.set_keep_above(true);
    let r = d.run();
    d.close();
    r == gtk::ResponseType::Yes
}

pub fn prompt(title: &str, label: &str, initial: &str) -> Option<String> {
    let d = gtk::Dialog::with_buttons(Some(title), None::<&gtk::Window>, gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(6);
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    let e = gtk::Entry::new();
    e.set_text(initial);
    e.set_activates_default(true);
    c.pack_start(&l, false, false, 0);
    c.pack_start(&e, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.set_keep_above(true);
    d.show_all();
    let r = d.run();
    let t = e.text().to_string();
    d.close();
    (r == gtk::ResponseType::Ok && !t.trim().is_empty()).then_some(t)
}
