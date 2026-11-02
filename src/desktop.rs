//! Desktop window: wallpaper, icons, context menu.
use gtk::gdk::prelude::GdkContextExt;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    /// icon key → its top-left on the desktop, as laid out now
    placed: RefCell<HashMap<String, (i32, i32)>>,
    /// grid step (width, height) of the current icon size
    step: Cell<(i32, i32)>,
    /// a drag from the desktop: the keys of the dragged icons, the one under the pointer and
    /// where inside it the pointer was
    dragging: RefCell<Vec<String>>,
    drag_anchor: RefCell<Option<(String, f64, f64)>>,
    /// the press on an already selected icon: becomes "select only this one" if no drag follows
    pending_single: RefCell<Option<gtk::Button>>,
    /// where a drop would go: a folder, the Recycle Bin ("trash:") or None = onto the desktop
    drop_target: RefCell<Option<(Option<PathBuf>, gdk::DragAction)>>,
    drop_hl: RefCell<Option<gtk::Button>>,
}

/// Desktop icon spacing, like Win7's icon grid.
const GRID_X0: i32 = 4;
const GRID_Y0: i32 = 4;

fn item_key(t: &Target) -> String {
    match t { Target::Path(p) => p.to_string_lossy().into_owned(), Target::Uri(u) => u.clone() }
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
        // the wallpaper is converted to a cairo surface once (per wallpaper), not on every repaint –
        // set_source_pixbuf on a full-screen pixbuf for each frame made dragging the selection box stutter
        let cache: RefCell<Option<(glib::WeakRef<gtk::gdk_pixbuf::Pixbuf>, gtk::cairo::Surface)>> = RefCell::new(None);
        win.connect_draw(move |w, cr| {
            let wp = glass::wallpaper();
            let mut c = cache.borrow_mut();
            let stale = c.as_ref().map(|(p, _)| p.upgrade().as_ref() != Some(&wp.sharp)).unwrap_or(true);
            if stale {
                let surf = w.window().and_then(|gw| gw.create_similar_surface(gtk::cairo::Content::Color, wp.sharp.width(), wp.sharp.height()));
                if let Some(s) = surf {
                    if let Ok(c2) = gtk::cairo::Context::new(&s) { c2.set_source_pixbuf(&wp.sharp, 0.0, 0.0); c2.paint().ok(); }
                    let weak = glib::WeakRef::new(); weak.set(Some(&wp.sharp));
                    *c = Some((weak, s));
                }
            }
            match c.as_ref() {
                Some((_, s)) => { cr.set_source_surface(s, 0.0, 0.0).ok(); }
                None => cr.set_source_pixbuf(&wp.sharp, 0.0, 0.0),
            }
            cr.paint().ok();
            glib::Propagation::Proceed
        });
        let d = Rc::new(Desktop { win: win.clone(), fixed, selected: RefCell::new(vec![]), _monitor: RefCell::new(None),
            band: RefCell::new(None), band_area: band_area.clone(), band_base: RefCell::new(vec![]),
            placed: RefCell::new(HashMap::new()), step: Cell::new((1, 1)), dragging: RefCell::new(vec![]),
            drag_anchor: RefCell::new(None), pending_single: RefCell::new(None), drop_target: RefCell::new(None),
            drop_hl: RefCell::new(None) });
        // on the overlay: GTK's drop-target lookup stops at the (input pass-through) selection
        // rectangle layer on top, which would hide every drop on the desktop
        d.wire_drop(overlay.upcast_ref());
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
                if let Some((x0, y0, ox, oy, ctrl)) = cur {
                    let (x, y) = e.position();
                    *d2.band.borrow_mut() = Some((x0, y0, x, y, ctrl));
                    d2.update_band_selection();
                    // repaint only what the rectangle covered before and covers now
                    let (l, t) = (x0.min(ox).min(x).floor() as i32 - 2, y0.min(oy).min(y).floor() as i32 - 2);
                    let (r, b) = (x0.max(ox).max(x).ceil() as i32 + 2, y0.max(oy).max(y).ceil() as i32 + 2);
                    d2.band_area.queue_draw_area(l, t, r - l, b - t);
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
                    // Shift+Delete deletes permanently, like Explorer
                    k if k == gdk::keys::constants::Delete || k == gdk::keys::constants::KP_Delete => d2.delete_selected(e.state().contains(gdk::ModifierType::SHIFT_MASK)),
                    k if (k == gdk::keys::constants::z || k == gdk::keys::constants::Z) && e.state().contains(gdk::ModifierType::CONTROL_MASK) => {
                        let d3 = Rc::downgrade(&d2);
                        crate::explorer::ops::undo(None, move || if let Some(d) = d3.upgrade() { d.rebuild() });
                    }
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
        let cfg = config::get();
        // Desktop Icon Settings > Change Icon
        let art = |key: &str, def: &str| Some(cfg.desktop_icon_art.get(key).cloned().unwrap_or_else(|| def.to_string()));
        for s in &cfg.desktop_icons {
            match s.as_str() {
                "computer" => v.push(Item { label: "Computer".into(), icon: None, icon_name: art("computer", "computer"), target: Target::Uri("computer:///".into()), special: Some("computer") }),
                "home" => v.push(Item { label: glib::user_name().to_string_lossy().into_owned(), icon: None, icon_name: art("home", "user-home"), target: Target::Path(config::home()), special: None }),
                "trash" => v.push(Item { label: "Recycle Bin".into(), icon: None, icon_name: if trash_full() { art("trash-full", "user-trash-full") } else { art("trash", "user-trash") }, target: Target::Uri("trash:///".into()), special: Some("trash") }),
                "network" => v.push(Item { label: "Network".into(), icon: None, icon_name: art("network", "network-workgroup"), target: Target::Uri("network:///".into()), special: None }),
                "control" => v.push(Item { label: "Control Panel".into(), icon: None, icon_name: art("control", "preferences-system"), target: Target::Uri("aerode:control".into()), special: Some("control") }),
                _ => {}
            }
        }
        let mut files: Vec<_> = std::fs::read_dir(desktop_dir()).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        // Folder Options > Show hidden files, folders, and drives
        files.retain(|p: &PathBuf| cfg.folder.show_hidden || !p.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(true));
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
            let info = gf.query_info("standard::icon,standard::display-name,standard::type", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
            v.push(Item {
                label: info.as_ref().map(|i| util::shown_name(i, cfg.folder.hide_ext)).unwrap_or_else(|| p.file_name().unwrap().to_string_lossy().into_owned()),
                icon: info.and_then(|i| i.icon()),
                icon_name: None,
                target: Target::Path(p),
                special: None,
            });
        }
        v
    }

    pub fn rebuild(self: &Rc<Self>) {
        // keep the selection across a rebuild (icons moved, a file appeared)
        let was_selected: Vec<String> = self.selected.borrow().iter().filter_map(|b| unsafe { b.data::<String>("key").map(|k| k.as_ref().clone()) }).collect();
        for c in self.fixed.children() { self.fixed.remove(&c); }
        self.selected.borrow_mut().clear();
        let cfg = config::get();
        let size = cfg.icon_size;
        let g = util::primary_geometry();
        let (cw, ch) = (size + 30, size + 44);
        let (sx, sy) = (cw + 6, ch + 4);
        self.step.set((sx, sy));
        let rows = ((g.height() - cfg.taskbar_height - 8) / sy).max(1);
        let cols = ((g.width() - 8) / sx).max(1);
        let items = self.items();
        // icons the user placed keep their spot (snapped to the grid when Align icons to grid is
        // on); the rest fill the first free cells top to bottom, left to right, like Win7
        let mut placed: HashMap<String, (i32, i32)> = HashMap::new();
        let mut taken = std::collections::HashSet::new();
        let cell_of = |x: i32, y: i32| (((x - GRID_X0) as f64 / sx as f64).round().max(0.0) as i32, ((y - GRID_Y0) as f64 / sy as f64).round().max(0.0) as i32);
        if !cfg.desktop_auto_arrange {
            for it in &items {
                let k = item_key(&it.target);
                if let Some(&(x, y)) = cfg.desktop_positions.get(&k) {
                    let (c, r) = cell_of(x, y);
                    if c >= cols || r >= rows || taken.contains(&(c, r)) { continue; }
                    taken.insert((c, r));
                    let pos = if cfg.desktop_align_grid { (GRID_X0 + c * sx, GRID_Y0 + r * sy) } else { (x, y) };
                    placed.insert(k, pos);
                }
            }
        }
        let mut next = 0;
        for it in &items {
            let k = item_key(&it.target);
            if placed.contains_key(&k) { continue; }
            while taken.contains(&(next / rows, next % rows)) { next += 1; }
            let (c, r) = (next / rows, next % rows);
            taken.insert((c, r));
            placed.insert(k, (GRID_X0 + c * sx, GRID_Y0 + r * sy));
        }
        for it in items {
            let k = item_key(&it.target);
            let (x, y) = placed[&k];
            let sel = was_selected.contains(&k);
            let w = self.icon_widget(it, size, cw);
            if sel { w.style_context().add_class("selected"); self.selected.borrow_mut().push(w.clone()); }
            self.fixed.put(&w, x, y);
        }
        *self.placed.borrow_mut() = placed;
        self.fixed.show_all();
    }

    /// The icon button under a point of the desktop.
    fn icon_at(&self, x: f64, y: f64) -> Option<gtk::Button> {
        self.fixed.children().into_iter().filter_map(|c| c.downcast::<gtk::Button>().ok()).find(|b| {
            let a = b.allocation();
            x >= a.x() as f64 && y >= a.y() as f64 && x < (a.x() + a.width()) as f64 && y < (a.y() + a.height()) as f64
        })
    }

    fn key_of(b: &gtk::Button) -> Option<String> { unsafe { b.data::<String>("key").map(|k| k.as_ref().clone()) } }

    /// Desktop icons are drop targets too: folders (move/copy into them), the Recycle Bin (delete),
    /// the desktop itself (move the icons there, or bring files in from Explorer).
    fn wire_drop(self: &Rc<Self>, ev: &gtk::Widget) {
        let targets = [gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0)];
        ev.drag_dest_set(gtk::DestDefaults::empty(), &targets, gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK);
        let d = Rc::downgrade(self);
        ev.connect_drag_motion(move |w, ctx, x, y, time| {
            let Some(d) = d.upgrade() else { return false };
            let dragging = d.dragging.borrow().clone();
            let sources: Vec<PathBuf> = dragging.iter().filter(|k| k.starts_with('/')).map(PathBuf::from).collect();
            let over = d.icon_at(x as f64, y as f64).filter(|b| Self::key_of(b).map(|k| !dragging.contains(&k)).unwrap_or(false));
            // what is under the pointer: a folder or the Recycle Bin takes the drop
            let into = over.as_ref().and_then(Self::key_of).and_then(|k| {
                if k == "trash:///" { Some(PathBuf::from("trash:")) } else if Path::new(&k).is_dir() { Some(PathBuf::from(k)) } else { None }
            });
            let hl = if into.is_some() { over.clone() } else { None };
            if d.drop_hl.borrow().as_ref() != hl.as_ref() {
                if let Some(o) = d.drop_hl.borrow().as_ref() { o.style_context().remove_class("drop"); }
                if let Some(n) = &hl { n.style_context().add_class("drop"); }
                *d.drop_hl.borrow_mut() = hl;
            }
            let act = match &into {
                Some(p) if p == Path::new("trash:") => gdk::DragAction::MOVE,
                Some(p) if sources.iter().any(|s| p.starts_with(s)) => gdk::DragAction::empty(),
                Some(p) => crate::explorer::drop_action(w.upcast_ref(), ctx, p, &sources),
                // icons of this desktop just move; files from elsewhere come to ~/Desktop
                None if !dragging.is_empty() => gdk::DragAction::MOVE,
                None => crate::explorer::drop_action(w.upcast_ref(), ctx, &desktop_dir(), &sources),
            };
            *d.drop_target.borrow_mut() = Some((into, act));
            ctx.drag_status(act, time);
            true
        });
        let d = Rc::downgrade(self);
        ev.connect_drag_leave(move |_, _, _| {
            let Some(d) = d.upgrade() else { return };
            if let Some(o) = d.drop_hl.borrow_mut().take() { o.style_context().remove_class("drop"); }
        });
        ev.connect_drag_drop(move |w, ctx, _, _, time| {
            w.drag_get_data(ctx, &gdk::Atom::intern("text/uri-list"), time);
            true
        });
        let d = Rc::downgrade(self);
        ev.connect_drag_data_received(move |_, ctx, x, y, data, _, time| {
            let Some(d) = d.upgrade() else { return };
            if let Some(o) = d.drop_hl.borrow_mut().take() { o.style_context().remove_class("drop"); }
            let target = d.drop_target.borrow_mut().take();
            let uris: Vec<String> = data.uris().iter().map(|u| u.to_string()).collect();
            let files: Vec<PathBuf> = uris.iter().filter_map(|u| gio::File::for_uri(u).path()).collect();
            let internal = !d.dragging.borrow().is_empty();
            let ok = match target {
                Some((_, act)) if act.is_empty() => false,
                Some((Some(p), _)) if p == Path::new("trash:") => {
                    let dd = Rc::downgrade(&d);
                    if !files.is_empty() { crate::explorer::ops::delete(files, false, None, true, move || if let Some(d) = dd.upgrade() { d.rebuild() }); }
                    true
                }
                Some((Some(dir), act)) => {
                    let dd = Rc::downgrade(&d);
                    if act.contains(gdk::DragAction::LINK) { crate::explorer::ops::create_shortcuts(&files, &dir); }
                    else if !files.is_empty() {
                        let kind = if act.contains(gdk::DragAction::MOVE) { crate::explorer::ops::Kind::Move } else { crate::explorer::ops::Kind::Copy };
                        crate::explorer::ops::transfer(files, dir, kind, None, move |_| if let Some(d) = dd.upgrade() { d.rebuild() });
                    }
                    true
                }
                Some((None, _)) if internal => { d.move_dragged(x as f64, y as f64); true }
                Some((None, act)) => {
                    // files from Explorer or another program: into ~/Desktop, placed where they were dropped
                    let dd = Rc::downgrade(&d);
                    let (px, py) = (x, y);
                    let done = move |made: Vec<PathBuf>| if let Some(d) = dd.upgrade() { d.place_new(&made, px, py); };
                    if act.contains(gdk::DragAction::LINK) { let made = crate::explorer::ops::create_shortcuts(&files, &desktop_dir()); done(made); }
                    else if !files.is_empty() {
                        let kind = if act.contains(gdk::DragAction::MOVE) { crate::explorer::ops::Kind::Move } else { crate::explorer::ops::Kind::Copy };
                        crate::explorer::ops::transfer(files, desktop_dir(), kind, None, done);
                    }
                    true
                }
                None => false,
            };
            ctx.drag_finish(ok, false, time);
        });
    }

    /// Icons dragged within the desktop: the one under the pointer goes where it was dropped, the
    /// others keep their place relative to it; occupied cells push an icon to the next free one.
    fn move_dragged(self: &Rc<Self>, x: f64, y: f64) {
        let keys = self.dragging.borrow().clone();
        let Some((anchor, ox, oy)) = self.drag_anchor.borrow().clone() else { return };
        let placed = self.placed.borrow().clone();
        let Some(&(ax, ay)) = placed.get(&anchor) else { return };
        let (nx, ny) = ((x - ox).round() as i32, (y - oy).round() as i32);
        let (dx, dy) = (nx - ax, ny - ay);
        let g = util::primary_geometry();
        let (sx, sy) = self.step.get();
        let maxx = (g.width() - sx).max(0);
        let maxy = (g.height() - config::get().taskbar_height - sy).max(0);
        let align = config::get().desktop_align_grid;
        let snap = |x: i32, y: i32| -> (i32, i32) {
            let (x, y) = (x.clamp(0, maxx), y.clamp(0, maxy));
            if !align { return (x, y); }
            (GRID_X0 + ((x - GRID_X0) as f64 / sx as f64).round() as i32 * sx, GRID_Y0 + ((y - GRID_Y0) as f64 / sy as f64).round() as i32 * sy)
        };
        let mut pos: HashMap<String, (i32, i32)> = placed.iter().filter(|(k, _)| !keys.contains(k)).map(|(k, v)| (k.clone(), *v)).collect();
        let occupied = |pos: &HashMap<String, (i32, i32)>, p: (i32, i32)| pos.values().any(|q| (q.0 - p.0).abs() < sx / 2 && (q.1 - p.1).abs() < sy / 2);
        for k in &keys {
            let Some(&(kx, ky)) = placed.get(k) else { continue };
            let mut p = snap(kx + dx, ky + dy);
            // next free cell downwards, then in the next column
            let mut guard = 0;
            while occupied(&pos, p) && guard < 500 {
                p.1 += sy;
                if p.1 > maxy { p.1 = GRID_Y0; p.0 += sx; if p.0 > maxx { p.0 = GRID_X0; } }
                guard += 1;
            }
            pos.insert(k.clone(), p);
        }
        config::update(|c| { c.desktop_auto_arrange = false; c.desktop_positions = pos.clone(); });
        self.rebuild();
    }

    /// Files that just arrived on the desktop by a drop: put them where they were dropped.
    fn place_new(self: &Rc<Self>, made: &[PathBuf], x: i32, y: i32) {
        if config::get().desktop_auto_arrange || made.is_empty() { self.rebuild(); return; }
        let (sx, sy) = self.step.get();
        let mut pos = self.placed.borrow().clone();
        let (px, mut py) = (x - sx / 2, y - sy / 3);
        for m in made {
            let k = m.to_string_lossy().into_owned();
            pos.insert(k, (px.max(0), py.max(0)));
            py += sy;
        }
        config::update(|c| c.desktop_positions = pos.clone());
        self.rebuild();
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
            (None, Some(n)) if n.starts_with('/') => gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(n, size, size, true).ok().or_else(|| util::pixbuf("text-x-generic", size)),
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
        unsafe { eb.set_data("key", item_key(&it.target)); }
        // drag the icon (and the rest of the selection): around the desktop, into folders or the
        // Recycle Bin, into Explorer windows and other programs (text/uri-list). Set up before the
        // press handler so GTK's drag detection sees the press first.
        eb.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &[gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0)],
            gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK);
        if let Some(p) = &pb { eb.drag_source_set_icon_pixbuf(p); }
        {
            let d = Rc::downgrade(self);
            eb.connect_drag_begin(move |eb, _| {
                let Some(d) = d.upgrade() else { return };
                d.pending_single.borrow_mut().take();
                if !d.selected.borrow().contains(eb) { d.clear_selection(); eb.style_context().add_class("selected"); d.selected.borrow_mut().push(eb.clone()); }
                *d.dragging.borrow_mut() = d.selected.borrow().iter().filter_map(Self::key_of).collect();
            });
        }
        {
            let d = Rc::downgrade(self);
            eb.connect_drag_data_get(move |_, _, data, _, _| {
                let Some(d) = d.upgrade() else { return };
                let keys = d.dragging.borrow().clone();
                let uris: Vec<String> = keys.iter().map(|k| if k.starts_with('/') { gio::File::for_path(k).uri().to_string() } else { k.clone() }).collect();
                let refs: Vec<&str> = uris.iter().map(|s| s.as_str()).collect();
                data.set_uris(&refs);
            });
        }
        {
            let d = Rc::downgrade(self);
            eb.connect_drag_end(move |_, _| {
                let Some(d) = d.upgrade() else { return };
                d.dragging.borrow_mut().clear();
                // a move into another folder/program removed files: the monitor rebuilds
            });
        }
        // never let GTK's default handler fail drags of special items (Computer, Network…)
        eb.connect_drag_failed(|_, _, _| glib::Propagation::Stop);
        {
            let d = Rc::downgrade(self);
            eb.connect_button_release_event(move |eb, e| {
                let Some(d) = d.upgrade() else { return glib::Propagation::Proceed };
                // a click (no drag) on one icon of a selection selects just that one
                if e.button() == 1 && d.pending_single.borrow().as_ref() == Some(eb) {
                    d.pending_single.borrow_mut().take();
                    d.clear_selection();
                    eb.style_context().add_class("selected");
                    d.selected.borrow_mut().push(eb.clone());
                }
                glib::Propagation::Proceed
            });
        }
        let d = Rc::downgrade(self);
        let it2 = it.clone();
        eb.connect_button_press_event(move |eb, e| {
            let Some(d) = d.upgrade() else { return glib::Propagation::Stop };
            // Folder Options > Click items as follows (single-click opens, like a link)
            let single = config::get().folder.single_click && e.event_type() == gdk::EventType::ButtonPress
                && !e.state().intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
            if (single || e.event_type() == gdk::EventType::DoubleButtonPress) && e.button() == 1 {
                open_item(&it2);
                return glib::Propagation::Stop;
            }
            let ctrl = e.state().contains(gdk::ModifierType::CONTROL_MASK);
            let already = d.selected.borrow().contains(eb);
            if e.button() == 1 {
                // where inside the icon the drag grabbed it (for dropping it in place)
                let (px, py) = e.position();
                if let Some(k) = Self::key_of(eb) { *d.drag_anchor.borrow_mut() = Some((k, px, py)); }
            }
            if ctrl && already && e.button() == 1 {
                eb.style_context().remove_class("selected");
                d.selected.borrow_mut().retain(|b| b != eb);
            } else if already && !ctrl {
                // keep the selection so it can be dragged; a plain click narrows it on release
                if e.button() == 1 { *d.pending_single.borrow_mut() = Some(eb.clone()); }
            } else {
                if !ctrl { d.clear_selection(); }
                eb.style_context().add_class("selected");
                d.selected.borrow_mut().push(eb.clone());
            }
            eb.queue_draw();
            d.win.present();
            if e.button() == 3 { d.item_menu(&it2, e); }
            // GTK's drag detection (connected first) has already seen the press
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
        let old = self.selected.borrow().clone();
        if old == sel { return; }
        for w in old.iter() { if !sel.contains(w) { w.style_context().remove_class("selected"); } }
        for w in &sel { if !old.contains(w) { w.style_context().add_class("selected"); } }
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

    /// Explorer's delete: the Win7 confirmation, the Recycle Bin (Undo with Ctrl+Z), Shift+Delete.
    fn delete_selected(self: &Rc<Self>, permanent: bool) {
        let paths = self.selected_paths();
        if paths.is_empty() { return; }
        let d = Rc::downgrade(self);
        crate::explorer::ops::delete(paths, permanent, None, true, move || if let Some(d) = d.upgrade() { d.rebuild() });
    }

    fn item_menu(self: &Rc<Self>, it: &Rc<Item>, e: &gdk::EventButton) {
        let m = gtk::Menu::new();
        let add = |m: &gtk::Menu, l: &str, f: Box<dyn Fn()>| { let i = gtk::MenuItem::with_label(l); i.connect_activate(move |_| f()); m.append(&i); };
        { let it = it.clone(); add(&m, "Open", Box::new(move || open_item(&it))); }
        match it.special {
            Some("trash") => {
                let d = Rc::downgrade(self);
                add(&m, "Empty Recycle Bin", Box::new(move || {
                    let d = d.clone();
                    crate::explorer::ops::empty_trash(None, move || if let Some(d) = d.upgrade() { d.rebuild() });
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
                        add(&m, "Delete", Box::new(move || if let Some(d) = d.upgrade() { d.delete_selected(false) }));
                        let p2 = p.clone();
                        let d = Rc::downgrade(self);
                        add(&m, "Rename", Box::new(move || {
                            if let Some(n) = prompt("Rename", "New name:", &p2.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) {
                                // Explorer's checks: never over an existing file, extension warning, Undo
                                if crate::explorer::ops::rename(&p2, &n, None).is_some() { if let Some(d) = d.upgrade() { d.rebuild(); } }
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
        let auto = gtk::CheckMenuItem::with_label("Auto arrange icons");
        auto.set_active(config::get().desktop_auto_arrange);
        { let d = Rc::downgrade(self); auto.connect_toggled(move |c| { let on = c.is_active(); config::update(|cf| cf.desktop_auto_arrange = on); if let Some(d) = d.upgrade() { d.rebuild(); } }); }
        view.append(&auto);
        let align = gtk::CheckMenuItem::with_label("Align icons to grid");
        align.set_active(config::get().desktop_align_grid);
        { let d = Rc::downgrade(self); align.connect_toggled(move |c| { let on = c.is_active(); config::update(|cf| cf.desktop_align_grid = on); if let Some(d) = d.upgrade() { d.rebuild(); } }); }
        view.append(&align);
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
                // sorting rearranges every icon (Win7 forgets where they were dragged)
                config::update(|c| { if c.desktop_sort == key { c.desktop_sort_desc = !c.desktop_sort_desc } else { c.desktop_sort = key.into(); c.desktop_sort_desc = false } c.desktop_positions.clear(); });
                if let Some(d) = d.upgrade() { d.rebuild(); }
            });
            sort.append(&i);
        }
        let si = gtk::MenuItem::with_label("Sort by");
        si.set_submenu(Some(&sort));
        m.append(&si);
        m.append(&refresh);
        m.append(&gtk::SeparatorMenuItem::new());
        let has_files = crate::explorer::ops::clipboard_has_files();
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
                    crate::explorer::ops::transfer(files, desktop_dir(), kind, None, move |_| if let Some(d) = d.upgrade() { d.rebuild() });
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
        res.connect_activate(|_| { util::spawn(&["aerode-control", "resolution"]); });
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
