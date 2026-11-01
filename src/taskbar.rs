//! The Windows 7 "superbar": start orb, pinned/running task buttons, tray, clock.
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::x11::{Client, Wm};
use crate::{config, flyouts, glass, util};

pub struct Group {
    pub key: String,
    pub app: Option<gio::AppInfo>,
    pub pinned: bool,
    pub windows: Vec<Client>,
    pub button: gtk::Button,
}

pub struct Taskbar {
    pub win: gtk::Window,
    pub wm: Rc<Wm>,
    tasks: gtk::Box,
    groups: RefCell<Vec<Group>>,
    app_cache: RefCell<HashMap<String, Option<gio::AppInfo>>>,
    pub orb_cb: RefCell<Option<Rc<dyn Fn()>>>,
    pub tray: gtk::Box,
}

/// Key used to match an app to WM_CLASS.
pub fn app_key(app: &gio::AppInfo) -> String {
    if let Some(d) = app.downcast_ref::<gio::DesktopAppInfo>() {
        if let Some(c) = d.startup_wm_class() {
            return c.to_lowercase();
        }
    }
    let exe = app.executable();
    let exe = exe.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !exe.is_empty() && exe != "env" && exe != "flatpak" && exe != "sh" {
        return exe;
    }
    app.id().map(|i| i.trim_end_matches(".desktop").to_lowercase()).unwrap_or_default()
}

fn client_keys(c: &Client) -> Vec<String> {
    let mut v = vec![c.class.to_lowercase(), c.instance.to_lowercase()];
    if let Ok(exe) = std::fs::read_link(format!("/proc/{}/exe", c.pid)) {
        if let Some(n) = exe.file_name() {
            v.push(n.to_string_lossy().to_lowercase());
        }
    }
    v.retain(|s| !s.is_empty());
    v
}

impl Taskbar {
    pub fn new(app: &gtk::Application, wm: Rc<Wm>) -> Rc<Taskbar> {
        let geo = util::primary_geometry();
        let h = config::get().taskbar_height;
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.set_application(Some(app));
        win.set_title("Taskbar");
        win.set_type_hint(gdk::WindowTypeHint::Dock);
        win.set_decorated(false);
        win.set_skip_taskbar_hint(true);
        win.set_skip_pager_hint(true);
        win.stick();
        win.set_keep_above(true);
        win.set_app_paintable(true);
        win.set_size_request(geo.width(), h);
        win.move_(geo.x(), geo.y() + geo.height() - h);
        win.style_context().add_class("aero7");
        win.style_context().add_class("taskbar");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        win.add(&row);

        // start orb
        let orb_box = gtk::EventBox::new();
        orb_box.set_visible_window(false);
        let orb = gtk::DrawingArea::new();
        orb.set_size_request(54, h);
        orb_box.add(&orb);
        orb_box.set_tooltip_text(Some("Start"));
        let orb_state = Rc::new(std::cell::Cell::new(0));
        {
            let st = orb_state.clone();
            orb.connect_draw(move |w, cr| {
                let size = 46.0f64.min(w.allocated_height() as f64 + 8.0);
                cr.translate((w.allocated_width() as f64 - size) / 2.0, (w.allocated_height() as f64 - size) / 2.0);
                glass::paint_orb(cr, size, st.get());
                glib::Propagation::Proceed
            });
        }
        for (sig, val) in [(0, 1), (1, 0)] {
            let st = orb_state.clone();
            let o = orb.clone();
            let f = move || { st.set(val); o.queue_draw(); };
            if sig == 0 { orb_box.connect_enter_notify_event(move |_, _| { f(); glib::Propagation::Proceed }); }
            else { orb_box.connect_leave_notify_event(move |_, _| { f(); glib::Propagation::Proceed }); }
        }
        row.pack_start(&orb_box, false, false, 0);

        let tasks = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tasks.set_margin_start(2);
        row.pack_start(&tasks, true, true, 0);

        let tray = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.pack_end(&tray, false, false, 0);

        let tb = Rc::new(Taskbar {
            win: win.clone(),
            wm: wm.clone(),
            tasks,
            groups: RefCell::new(Vec::new()),
            app_cache: RefCell::new(HashMap::new()),
            orb_cb: RefCell::new(None),
            tray: tray.clone(),
        });

        {
            let tb2 = tb.clone();
            let st = orb_state.clone();
            let o = orb.clone();
            orb_box.connect_button_press_event(move |_, e| {
                if e.button() == 1 {
                    st.set(2);
                    o.queue_draw();
                    if let Some(cb) = tb2.orb_cb.borrow().clone() { cb(); }
                }
                glib::Propagation::Stop
            });
        }

        // tray: flyout buttons + clock + show desktop
        flyouts::build_tray(&tb);
        let sd = gtk::Button::new();
        sd.style_context().add_class("showdesk");
        sd.set_tooltip_text(Some("Show desktop"));
        {
            let wm = wm.clone();
            sd.connect_clicked(move |_| wm.set_showing_desktop(!wm.showing_desktop()));
        }
        tray.pack_end(&sd, false, false, 0);

        win.connect_draw(move |w, cr| {
            let g = util::primary_geometry();
            let (ww, hh) = (w.allocated_width() as f64, w.allocated_height() as f64);
            glass::paint_glass(cr, ww, hh, g.x() as f64, (g.y() + g.height()) as f64 - hh, 0.0, false);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            cr.rectangle(0.0, 0.0, ww, 1.0);
            cr.fill().ok();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
            cr.rectangle(0.0, 1.0, ww, 1.0);
            cr.fill().ok();
            glib::Propagation::Proceed
        });
        {
            let w = win.clone();
            glass::on_change(move || w.queue_draw());
        }
        {
            let wm = wm.clone();
            win.connect_realize(move |w| {
                let geo = util::primary_geometry();
                if let Some(x) = w.window().and_then(|g| g.downcast::<gdkx11::X11Window>().ok()) {
                    wm.set_strut(x.xid() as u32, (h) as u32, geo.x() as u32, (geo.x() + geo.width() - 1) as u32);
                }
            });
        }
        win.show_all();
        tb.refresh();
        {
            let tb2 = tb.clone();
            crate::x11::watch(wm, move || tb2.refresh());
        }
        tb
    }

    fn app_for(&self, c: &Client) -> Option<gio::AppInfo> {
        let keys = client_keys(c);
        let k0 = keys.first().cloned().unwrap_or_default();
        if let Some(v) = self.app_cache.borrow().get(&k0) {
            return v.clone();
        }
        let apps = util::all_apps();
        let found = keys.iter().find_map(|k| {
            apps.iter().find(|a| &app_key(a) == k).cloned().or_else(|| {
                apps.iter().find(|a| a.id().map(|i| i.trim_end_matches(".desktop").to_lowercase().ends_with(k.as_str())).unwrap_or(false)).cloned()
            })
        });
        self.app_cache.borrow_mut().insert(k0, found.clone());
        found
    }

    pub fn refresh(self: &Rc<Self>) {
        let clients = self.wm.clients();
        let active = self.wm.active();
        let pinned: Vec<gio::AppInfo> = util::pinned().iter().filter_map(|id| util::app_by_id(id)).collect();

        // compute desired groups: (key, app, pinned, windows)
        let mut want: Vec<(String, Option<gio::AppInfo>, bool, Vec<Client>)> =
            pinned.iter().map(|a| (app_key(a), Some(a.clone()), true, vec![])).collect();
        for c in clients {
            let keys = client_keys(&c);
            if let Some(g) = want.iter_mut().find(|g| keys.contains(&g.0)) {
                g.3.push(c);
                continue;
            }
            let app = self.app_for(&c);
            let key = app.as_ref().map(app_key).unwrap_or_else(|| keys.first().cloned().unwrap_or_else(|| c.id.to_string()));
            if let Some(g) = want.iter_mut().find(|g| g.0 == key) {
                g.3.push(c);
            } else {
                want.push((key, app, false, vec![c]));
            }
        }
        // keep previous order for running unpinned groups (stability)
        let old_keys: Vec<String> = self.groups.borrow().iter().map(|g| g.key.clone()).collect();
        let new_keys: Vec<String> = want.iter().map(|g| g.0.clone()).collect();
        if old_keys != new_keys {
            for c in self.tasks.children() {
                self.tasks.remove(&c);
            }
            let mut groups = Vec::new();
            for (key, app, pinned, windows) in want {
                let button = gtk::Button::new();
                button.style_context().add_class("task");
                self.tasks.pack_start(&button, false, false, 0);
                self.wire(&button, key.clone());
                groups.push(Group { key, app, pinned, windows, button });
            }
            *self.groups.borrow_mut() = groups;
        } else {
            let mut gs = self.groups.borrow_mut();
            for (g, w) in gs.iter_mut().zip(want) {
                g.windows = w.3;
                g.app = w.1;
                g.pinned = w.2;
            }
        }
        for g in self.groups.borrow().iter() {
            let sc = g.button.style_context();
            let running = !g.windows.is_empty();
            let is_active = g.windows.iter().any(|w| w.id == active && !w.minimized);
            for (cls, on) in [("running", running), ("active", is_active), ("urgent", g.windows.iter().any(|w| w.urgent))] {
                if on { sc.add_class(cls) } else { sc.remove_class(cls) }
            }
            let tip = if g.windows.is_empty() {
                g.app.as_ref().map(|a| a.display_name().to_string()).unwrap_or_default()
            } else {
                g.windows.iter().map(|w| w.title.clone()).collect::<Vec<_>>().join("\n")
            };
            g.button.set_tooltip_text(Some(&tip));
            if g.button.child().is_none() {
                let pb = g.app.as_ref().and_then(|a| util::gicon_pixbuf(a.icon().as_ref(), 32))
                    .or_else(|| g.windows.first().and_then(|w| self.wm.icon(w.id, 32)).map(|p| util::scale(&p, 32)))
                    .or_else(|| util::pixbuf("application-x-executable", 32));
                let ov = gtk::Overlay::new();
                ov.add(&gtk::Image::from_pixbuf(pb.as_ref()));
                g.button.add(&ov);
                g.button.show_all();
            }
            // Win7 shows a stacked edge when several windows are grouped
            if let Some(ov) = g.button.child().and_then(|c| c.downcast::<gtk::Overlay>().ok()) {
                let kids = ov.children();
                let has_badge = kids.len() > 1;
                if g.windows.len() > 1 && !has_badge {
                    let l = gtk::Label::new(Some(&g.windows.len().to_string()));
                    l.set_halign(gtk::Align::End);
                    l.set_valign(gtk::Align::End);
                    ov.add_overlay(&l);
                    l.show();
                } else if let Some(l) = kids.get(1).and_then(|w| w.clone().downcast::<gtk::Label>().ok()) {
                    if g.windows.len() > 1 { l.set_text(&g.windows.len().to_string()); } else { ov.remove(&l); }
                }
            }
        }
    }

    fn wire(self: &Rc<Self>, button: &gtk::Button, key: String) {
        let tb = Rc::downgrade(self);
        let k = key.clone();
        button.connect_button_release_event(move |b, e| {
            let Some(tb) = tb.upgrade() else { return glib::Propagation::Proceed };
            let groups = tb.groups.borrow();
            let Some(g) = groups.iter().find(|g| g.key == k) else { return glib::Propagation::Proceed };
            match e.button() {
                1 => {
                    if g.windows.is_empty() {
                        if let Some(a) = &g.app { util::launch(a); }
                    } else if g.windows.len() == 1 {
                        let w = &g.windows[0];
                        if w.id == tb.wm.active() && !w.minimized { tb.wm.minimize(w.id) } else { tb.wm.activate(w.id) }
                    } else {
                        let m = tb.window_menu(&g.windows);
                        m.popup_at_widget(b, gdk::Gravity::North, gdk::Gravity::South, Some(e));
                    }
                }
                2 => { if let Some(a) = &g.app { util::launch(a); } }
                3 => {
                    let m = tb.jump_menu(g);
                    m.popup_at_widget(b, gdk::Gravity::North, gdk::Gravity::South, Some(e));
                }
                _ => {}
            }
            glib::Propagation::Stop
        });
    }

    fn window_menu(self: &Rc<Self>, wins: &[Client]) -> gtk::Menu {
        let m = gtk::Menu::new();
        for w in wins {
            let it = gtk::MenuItem::with_label(&w.title);
            let (wm, id) = (self.wm.clone(), w.id);
            it.connect_activate(move |_| wm.activate(id));
            m.append(&it);
        }
        m.show_all();
        m
    }

    fn jump_menu(self: &Rc<Self>, g: &Group) -> gtk::Menu {
        let m = gtk::Menu::new();
        if let Some(a) = &g.app {
            let it = gtk::MenuItem::with_label(&a.display_name());
            let a2 = a.clone();
            it.connect_activate(move |_| util::launch(&a2));
            m.append(&it);
            if let Some(id) = a.id() {
                let id = id.to_string();
                let pin = gtk::MenuItem::with_label(if g.pinned { "Unpin this program from taskbar" } else { "Pin this program to taskbar" });
                let tb = Rc::downgrade(self);
                let was = g.pinned;
                pin.connect_activate(move |_| {
                    let mut p = util::pinned();
                    if was { p.retain(|x| x != &id) } else { p.push(id.clone()) }
                    config::update(|c| c.pinned = Some(p));
                    if let Some(tb) = tb.upgrade() { tb.refresh(); }
                });
                m.append(&pin);
            }
        }
        if !g.windows.is_empty() {
            m.append(&gtk::SeparatorMenuItem::new());
            let it = gtk::MenuItem::with_label(if g.windows.len() > 1 { "Close all windows" } else { "Close window" });
            let wm = self.wm.clone();
            let ids: Vec<u32> = g.windows.iter().map(|w| w.id).collect();
            it.connect_activate(move |_| ids.iter().for_each(|i| wm.close(*i)));
            m.append(&it);
        }
        m.show_all();
        m
    }

    pub fn pin(self: &Rc<Self>, id: &str) {
        let mut p = util::pinned();
        if !p.iter().any(|x| x == id) {
            p.push(id.to_string());
            config::update(|c| c.pinned = Some(p));
            self.refresh();
        }
    }
}
