//! The Windows 7 "superbar": start orb, pinned/running task buttons, tray, clock.
use gtk::prelude::*;
use gtk::{cairo, gdk, gdk_pixbuf::Pixbuf, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::x11::{Client, Wm};
use crate::{config, flyouts, glass, msstyles, shellstyle, util};

pub struct Group {
    pub key: String,
    pub app: Option<gio::AppInfo>,
    pub pinned: bool,
    pub windows: Vec<Client>,
    pub button: gtk::Button,
    vis: Rc<BtnVis>,
}

/// What a task button's msstyles painter needs (kept current by `refresh`), plus its hot-track state.
#[derive(Default)]
struct BtnVis {
    /// number of windows in the group (0 = pinned, not running)
    count: Cell<usize>,
    active: Cell<bool>,
    urgent: Cell<bool>,
    /// blink phase of a flashing button (false = shown as normal for this half-period)
    flash_off: Cell<bool>,
    flash: RefCell<Option<glib::SourceId>>,
    hover: Cell<bool>,
    pressed: Cell<bool>,
    /// pointer x inside the button: the colour hot-track light follows it
    x: Cell<f64>,
    /// hot state opacity (0..1), faded in and out
    fade: Cell<f64>,
    anim: RefCell<Option<glib::SourceId>>,
    /// the icon's dominant colour (Win7 "color hot-track")
    color: Cell<[f64; 3]>,
}

pub struct Taskbar {
    pub win: gtk::Window,
    pub wm: Rc<Wm>,
    tasks: gtk::Box,
    groups: RefCell<Vec<Group>>,
    app_cache: RefCell<HashMap<String, Option<gio::AppInfo>>>,
    pub orb_cb: RefCell<Option<Rc<dyn Fn()>>>,
    pub tray: gtk::Box,
    orb: Orb,
}

/// The start orb: 0 normal, 1 hot, 2 pressed (also while the Start menu is open, like Win7).
struct Orb {
    area: gtk::DrawingArea,
    events: gtk::EventBox,
    state: Rc<Cell<i32>>,
    hover: Rc<Cell<bool>>,
    menu_open: Rc<Cell<bool>>,
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
        crate::glass::enable_rgba(&win);
        win.set_size_request(geo.width(), h);
        win.move_(geo.x(), geo.y() + geo.height() - h);
        win.style_context().add_class("aerode");
        win.style_context().add_class("taskbar");
        if config::get().taskbar.small_icons { win.style_context().add_class("small"); }

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        // right-clicking the bar itself (not a button on it) opens the taskbar's menu
        let row_ev = gtk::EventBox::new();
        row_ev.set_visible_window(false);
        row_ev.add(&row);
        win.add(&row_ev);

        // start orb
        let orb_box = gtk::EventBox::new();
        orb_box.set_visible_window(false);
        // the DrawingArea has its own window: keep the event box on top or it never sees clicks
        orb_box.set_above_child(true);
        orb_box.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
        let orb = gtk::DrawingArea::new();
        orb.set_size_request(54, h);
        orb_box.add(&orb);
        orb_box.set_tooltip_text(Some("Start"));
        let orb_state = Rc::new(Cell::new(0));
        let orb_hover = Rc::new(Cell::new(false));
        let menu_open = Rc::new(Cell::new(false));
        {
            let st = orb_state.clone();
            orb.connect_draw(move |w, cr| {
                // Win7 lets the orb overhang the 40 px bar; with small icons it fits the 30 px bar
                // explorer.exe's 54×54 orb at its own size, centred on the bar like Win7 (the glow
                // above the bar is cut off: Win7 shows it in a separate window over the desktop)
                let hh = w.allocated_height() as f64;
                let size = if hh < 36.0 { hh + 2.0 } else { 54.0 };
                cr.translate((w.allocated_width() as f64 - size) / 2.0, (w.allocated_height() as f64 - size) / 2.0);
                glass::paint_orb(cr, size, st.get());
                glib::Propagation::Proceed
            });
        }
        for inside in [true, false] {
            let (st, hv, open) = (orb_state.clone(), orb_hover.clone(), menu_open.clone());
            let o = orb.clone();
            // the orb stays lit while the menu is open
            let f = move || { hv.set(inside); if !open.get() { st.set(inside as i32); o.queue_draw(); } };
            if inside { orb_box.connect_enter_notify_event(move |_, _| { f(); glib::Propagation::Proceed }); }
            else { orb_box.connect_leave_notify_event(move |_, _| { f(); glib::Propagation::Proceed }); }
        }
        row.pack_start(&orb_box, false, false, 0);

        // the task band starts 4 px right of the orb's 54 px: first button frame at x = 59 (Win7)
        let tasks = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        // native Win7: the first task button's outline is at x = 57 (orb 54 + 2 + the 1 px button margin)
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
            orb: Orb { area: orb.clone(), events: orb_box.clone(), state: orb_state.clone(), hover: orb_hover, menu_open },
        });

        {
            let tbw = Rc::downgrade(&tb);
            row_ev.connect_button_press_event(move |_, e| {
                if e.button() != 3 { return glib::Propagation::Proceed; }
                // presses on task buttons and notification icons are theirs
                let on_button = { let mut ev: gtk::gdk::Event = (**e).clone(); gtk::event_widget(&mut ev) }.map(|w| w.is::<gtk::Button>() || w.ancestor(gtk::Button::static_type()).is_some()).unwrap_or(false);
                if on_button { return glib::Propagation::Proceed; }
                let Some(tb) = tbw.upgrade() else { return glib::Propagation::Proceed };
                crate::flyouts::close_open();
                crate::thumbs::hide();
                let m = crate::flyouts::build_menu(tb.menu_items());
                m.popup_at_pointer(Some(e));
                glib::Propagation::Stop
            });
        }
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

        // tray, packed from the right: show desktop (at the very edge), clock, system icons,
        // notification area, language bar
        let sd = gtk::Button::new();
        sd.style_context().add_class("showdesk");
        sd.set_tooltip_text(Some("Show desktop"));
        {
            // Aero Peek: resting on the button for half a second shows the desktop through the
            // windows (Taskbar Properties > Use Aero Peek to preview the desktop)
            let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
            let stop = { let p = pending.clone(); move || { if let Some(s) = p.borrow_mut().take() { s.remove(); } crate::thumbs::peek_desktop(false); } };
            {
                let p = pending.clone();
                sd.connect_enter_notify_event(move |_, _| {
                    if config::get().taskbar.aero_peek {
                        let p2 = p.clone();
                        *p.borrow_mut() = Some(glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
                            p2.borrow_mut().take();
                            crate::thumbs::peek_desktop(true);
                        }));
                    }
                    glib::Propagation::Proceed
                });
            }
            { let stop = stop.clone(); sd.connect_leave_notify_event(move |_, _| { stop(); glib::Propagation::Proceed }); }
            let wm = wm.clone();
            sd.connect_clicked(move |_| { stop(); wm.set_showing_desktop(!wm.showing_desktop()) });
        }
        if shellstyle::available() {
            // ShowDesktop::Button: the glass strip at the bar's right end (1 normal, 2 hot, 3 pressed)
            sd.style_context().add_class("w7");
            sd.connect_draw(|b, cr| {
                let st = b.state_flags();
                let s = if st.contains(gtk::StateFlags::ACTIVE) { 3 } else if st.contains(gtk::StateFlags::PRELIGHT) { 2 } else { 1 };
                shellstyle::paint(cr, "ShowDesktop::Button", 0, s, 0.0, 0.0, b.allocated_width() as f64, b.allocated_height() as f64);
                glib::Propagation::Proceed
            });
            // hover and press look the same to CSS here, so GTK would not repaint on its own
            sd.connect_state_flags_changed(|b, _| b.queue_draw());
        }
        tray.pack_end(&sd, false, false, 0);
        flyouts::build_tray(&tb);

        if shellstyle::available() { win.style_context().add_class("w7"); }
        let band = tb.tasks.clone();
        win.connect_draw(move |w, cr| {
            let g = util::primary_geometry();
            let (ww, hh) = (w.allocated_width() as f64, w.allocated_height() as f64);
            let (sx, sy) = (g.x() as f64, (g.y() + g.height()) as f64 - hh);
            if shellstyle::available() {
                let a = band.allocation();
                paint_bar(cr, ww, hh, sx, sy, a.x() as f64, a.width() as f64);
                return glib::Propagation::Proceed;
            }
            glass::paint_glass(cr, ww, hh, sx, sy, 0.0, false);
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
                let vis = Rc::new(BtnVis::default());
                if shellstyle::available() { themed_task_button(&button, &vis); }
                self.wire(&button, key.clone());
                groups.push(Group { key, app, pinned, windows, button, vis });
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
            // where each window minimises to (_NET_WM_ICON_GEOMETRY: the WM's minimise animation
            // flies into the button, like Win7); grouped windows share their group's button
            for w in &g.windows { self.wm.set_icon_geometry_of(w.id, &g.button); }
            let sc = g.button.style_context();
            let running = !g.windows.is_empty();
            let is_active = g.windows.iter().any(|w| w.id == active && !w.minimized);
            let urgent = g.windows.iter().any(|w| w.urgent);
            for (cls, on) in [("running", running), ("active", is_active), ("urgent", urgent)] {
                if on { sc.add_class(cls) } else { sc.remove_class(cls) }
            }
            let v = &g.vis;
            if (v.count.get(), v.active.get(), v.urgent.get()) != (g.windows.len(), is_active, urgent) {
                let starts = urgent && !v.urgent.get();
                v.count.set(g.windows.len());
                v.active.set(is_active);
                v.urgent.set(urgent);
                if starts { start_flash(&g.button, v); }
                g.button.queue_draw();
            }
            let tip = if g.windows.is_empty() {
                g.app.as_ref().map(|a| a.display_name().to_string()).unwrap_or_default()
            } else {
                g.windows.iter().map(|w| w.title.clone()).collect::<Vec<_>>().join("\n")
            };
            // running buttons show thumbnails instead (Win7); pinned ones keep the name tooltip
            g.button.set_tooltip_text(if running { None } else { Some(&tip) });
            if g.button.child().is_none() {
                // Taskbar Properties > Use small icons
                let isz = if config::get().taskbar.small_icons { 16 } else { 32 };
                let pb = g.app.as_ref().and_then(|a| util::gicon_pixbuf(a.icon().as_ref(), isz))
                    .or_else(|| g.windows.first().and_then(|w| self.wm.icon(w.id, isz as u32)).map(|p| util::scale(&p, isz)))
                    .or_else(|| util::pixbuf("application-x-executable", isz));
                g.vis.color.set(pb.as_ref().and_then(dominant_color).unwrap_or_else(default_hot_color));
                let ov = gtk::Overlay::new();
                ov.add(&gtk::Image::from_pixbuf(pb.as_ref()));
                g.button.add(&ov);
                g.button.show_all();
            }
            if shellstyle::available() {
                // a stacked button (several windows) is narrower: keep the icon centred on its front
                if let Some(c) = g.button.child() {
                    let e = stack_inset(g.windows.len()) as i32;
                    if c.margin_end() != e { c.set_margin_end(e); }
                }
                continue;
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
        // a new button is laid out after its first refresh: publish its rectangle once it has one
        // (and whenever it moves along the taskbar)
        {
            let (tb, k) = (Rc::downgrade(self), key.clone());
            button.connect_size_allocate(move |b, _| {
                let Some(tb) = tb.upgrade() else { return };
                let Ok(groups) = tb.groups.try_borrow() else { return };
                if let Some(g) = groups.iter().find(|g| g.key == k) {
                    for w in &g.windows { tb.wm.set_icon_geometry_of(w.id, b); }
                }
            });
        }
        let tb = Rc::downgrade(self);
        let k = key.clone();
        button.connect_button_release_event(move |b, e| {
            let Some(tb) = tb.upgrade() else { return glib::Propagation::Proceed };
            let groups = tb.groups.borrow();
            let Some(g) = groups.iter().find(|g| g.key == k) else { return glib::Propagation::Proceed };
            match e.button() {
                1 => {
                    if g.windows.is_empty() {
                        crate::thumbs::hide();
                        if let Some(a) = &g.app { util::launch(a); }
                    } else if g.windows.len() == 1 {
                        crate::thumbs::hide();
                        let w = &g.windows[0];
                        if w.id == tb.wm.active() && !w.minimized { tb.wm.minimize(w.id) } else { tb.wm.activate(w.id) }
                    } else if crate::thumbs::visible_for(b.upcast_ref()) {
                        crate::thumbs::hide();
                    } else {
                        // several windows: their thumbnails, like Win7
                        crate::thumbs::show_now(&tb.wm, b.upcast_ref(), tb.thumb_wins(&g.windows));
                    }
                }
                2 => { if let Some(a) = &g.app { util::launch(a); } }
                3 => {
                    crate::thumbs::hide();
                    let m = tb.jump_menu(g);
                    m.popup_at_widget(b, gdk::Gravity::North, gdk::Gravity::South, Some(e));
                }
                _ => {}
            }
            glib::Propagation::Stop
        });
        // live thumbnails when the pointer rests on a running button
        let tb = Rc::downgrade(self);
        button.connect_enter_notify_event(move |b, _| {
            let Some(tb) = tb.upgrade() else { return glib::Propagation::Proceed };
            let groups = tb.groups.borrow();
            match groups.iter().find(|g| g.key == key) {
                Some(g) if !g.windows.is_empty() => crate::thumbs::hover(&tb.wm, b.upcast_ref(), tb.thumb_wins(&g.windows)),
                _ => crate::thumbs::leave(),
            }
            glib::Propagation::Proceed
        });
        button.connect_leave_notify_event(|_, _| { crate::thumbs::leave(); glib::Propagation::Proceed });
    }

    /// The taskbar's right-click menu (Win7): window arrangement, Show the desktop, Task Manager,
    /// Lock the taskbar, Properties. A leading ✓ marks a checked item.
    pub fn menu_items(self: &Rc<Self>) -> Vec<(String, Option<Rc<dyn Fn()>>)> {
        let wm = self.wm.clone();
        let arrange = |how: u8| -> Option<Rc<dyn Fn()>> {
            let wm = wm.clone();
            Some(Rc::new(move || arrange_windows(&wm, how)))
        };
        let wm2 = self.wm.clone();
        let locked = config::get().taskbar.locked;
        vec![
            ("Ca_scade windows".into(), arrange(0)),
            ("Show windows stac_ked".into(), arrange(1)),
            ("Show windows side by s_ide".into(), arrange(2)),
            ("Show the _desktop".into(), Some(Rc::new(move || wm2.set_showing_desktop(!wm2.showing_desktop())))),
            (String::new(), None),
            ("Start Tas_k Manager".into(), Some(Rc::new(|| { util::spawn(&["aerode-taskmgr"]); }))),
            (String::new(), None),
            (format!("{}Lock the tas_kbar", if locked { "\u{2713}" } else { "" }), Some(Rc::new(move || {
                config::update(|c| c.taskbar.locked = !locked);
            }))),
            ("P_roperties".into(), Some(Rc::new(|| { util::spawn(&["aerode-control", "taskbar"]); }))),
        ]
    }

    fn thumb_wins(&self, wins: &[Client]) -> Vec<crate::thumbs::Win> {
        wins.iter().map(|c| crate::thumbs::Win { id: c.id, title: c.title.clone(), icon: self.wm.icon(c.id, 16), minimized: c.minimized }).collect()
    }

    /// Win+1..9: like clicking the Nth taskbar button; with several windows, cycle through them.
    pub fn launch_index(self: &Rc<Self>, n: usize) {
        let groups = self.groups.borrow();
        let Some(g) = n.checked_sub(1).and_then(|i| groups.get(i)) else { return };
        let active = self.wm.active();
        match g.windows.len() {
            0 => { if let Some(a) = &g.app { util::launch(a); } }
            1 => { let w = &g.windows[0]; if w.id == active && !w.minimized { self.wm.minimize(w.id) } else { self.wm.activate(w.id) } }
            k => {
                let next = g.windows.iter().position(|w| w.id == active).map(|i| (i + 1) % k).unwrap_or(0);
                self.wm.activate(g.windows[next].id);
            }
        }
    }

    /// Win7's taskbar Jump List: the program's "Recent" category (the files it opened, as in the
    /// Start menu), then the tasks area – the program itself, Pin/Unpin this program from taskbar,
    /// Close window – on its tinted band. Colours measured on a native Win7 Jump List
    /// (learn.microsoft.com shell/images/taskbar/jumplist.png).
    fn jump_menu(self: &Rc<Self>, g: &Group) -> gtk::Menu {
        let m = gtk::Menu::new();
        m.style_context().add_class("jumplist");
        // no check-mark column: Win7's icons sit near the left edge
        m.set_reserve_toggle_size(false);
        let row = |icon: Option<gtk::Image>, label: &str, task: bool| -> gtk::MenuItem {
            let it = gtk::MenuItem::new();
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            match icon { Some(i) => h.pack_start(&i, false, false, 0), None => { let sp = gtk::Box::new(gtk::Orientation::Horizontal, 0); sp.set_size_request(16, 16); h.pack_start(&sp, false, false, 0); } }
            let l = gtk::Label::new(Some(label));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            l.set_max_width_chars(34);
            h.pack_start(&l, true, true, 0);
            it.add(&h);
            it.style_context().add_class(if task { "jl-task" } else { "jl-item" });
            it
        };
        let gicon_img = |i: &gio::Icon| gtk::Image::from_gicon(i, gtk::IconSize::Menu);
        let theme_img = |names: &[&str]| -> Option<gtk::Image> {
            let n = util::first_icon(names);
            (!n.is_empty()).then(|| gtk::Image::from_icon_name(Some(n), gtk::IconSize::Menu))
        };
        // ---- the Recent category
        if let Some(a) = &g.app {
            let (cat, items) = crate::startmenu::jump_items(a, &crate::startmenu::recent_index());
            if !items.is_empty() {
                let head = gtk::MenuItem::new();
                let hb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let hl = gtk::Label::new(Some(cat));
                hl.style_context().add_class("jl-heading");
                hb.pack_start(&hl, false, false, 0);
                let rule = gtk::Separator::new(gtk::Orientation::Horizontal);
                rule.set_valign(gtk::Align::Center);
                rule.style_context().add_class("jl-rule");
                hb.pack_start(&rule, true, true, 0);
                head.add(&hb);
                head.style_context().add_class("jl-head");
                head.set_sensitive(false);
                m.append(&head);
                for j in items {
                    let it = row(j.icon.as_ref().map(gicon_img), &j.label, false);
                    if let Some(t) = &j.tip { it.set_tooltip_text(Some(t)); }
                    let act = j.act.clone();
                    it.connect_activate(move |_| act());
                    m.append(&it);
                }
            }
        }
        // ---- the tasks band
        let sep = gtk::SeparatorMenuItem::new();
        sep.style_context().add_class("jl-band");
        if !m.children().is_empty() { m.append(&sep); }
        if let Some(a) = &g.app {
            let it = row(a.icon().as_ref().map(gicon_img), &a.display_name(), true);
            let a2 = a.clone();
            it.connect_activate(move |_| util::launch(&a2));
            m.append(&it);
            if let Some(id) = a.id() {
                let id = id.to_string();
                let label = if g.pinned { "Unpin this program from taskbar" } else { "Pin this program to taskbar" };
                let pin = row(Some(jl_icon(JlIcon::Pin)).or_else(|| theme_img(&["pin", "window-pin", "emblem-pin"])), label, true);
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
            let it = row(Some(jl_icon(JlIcon::Close)), if g.windows.len() > 1 { "Close all windows" } else { "Close window" }, true);
            let wm = self.wm.clone();
            let ids: Vec<u32> = g.windows.iter().map(|w| w.id).collect();
            it.connect_activate(move |_| ids.iter().for_each(|i| wm.close(*i)));
            m.append(&it);
        }
        m.show_all();
        m
    }

    /// Apply a changed bar size (Taskbar Properties > Use small icons) without restarting the shell.
    pub fn resize(self: &Rc<Self>) {
        let cfg = config::get();
        let h = cfg.taskbar_height;
        let sc = self.win.style_context();
        if sc.has_class("small") == cfg.taskbar.small_icons && self.win.allocated_height() == h { return; }
        if cfg.taskbar.small_icons { sc.add_class("small") } else { sc.remove_class("small") }
        let geo = util::primary_geometry();
        // the orb's drawing area sets the bar's minimum height
        if let Some(orb) = self.win.child().and_then(|r| r.downcast::<gtk::Box>().ok()).and_then(|r| r.children().into_iter().next())
            .and_then(|e| e.downcast::<gtk::EventBox>().ok()).and_then(|e| e.child()) { orb.set_size_request(54, h); }
        self.win.set_size_request(geo.width(), h);
        self.win.resize(geo.width(), h);
        self.win.move_(geo.x(), geo.y() + geo.height() - h);
        if let Some(x) = self.win.window().and_then(|g| g.downcast::<gdkx11::X11Window>().ok()) {
            self.wm.set_strut(x.xid() as u32, h as u32, geo.x() as u32, (geo.x() + geo.width() - 1) as u32);
        }
        // refresh() rebuilds the button contents at the new icon size
        for g in self.groups.borrow().iter() { if let Some(c) = g.button.child() { g.button.remove(&c); } }
    }

    /// The Start menu opened or closed: the orb shows its pressed state while the menu is open and
    /// its "Start" tooltip stays away (Win7 shows no tooltip over its own open menu).
    pub fn set_start_open(&self, open: bool) {
        let o = &self.orb;
        o.menu_open.set(open);
        o.state.set(if open { 2 } else { o.hover.get() as i32 });
        o.area.queue_draw();
        if open { o.events.set_has_tooltip(false); } else { o.events.set_tooltip_text(Some("Start")); }
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

// ------------------------------------------------------------------ msstyles painting

/// The bar behind the buttons: glass, then aero.msstyles' TaskBar2Composited::TaskBar (bottom
/// dock: the dark/light top edge and a 23 % darkening) over the bar, except over the task band,
/// which gets TaskBand2Composited::TaskBand2 instead: the same edge, with the darkening fading out
/// over 39 px at both ends of the band (clear glass in the middle, as in a Win7 capture).
fn paint_bar(cr: &cairo::Context, w: f64, h: f64, sx: f64, sy: f64, band_x: f64, band_w: f64) {
    cr.save().ok();
    cr.set_operator(cairo::Operator::Source);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    cr.paint().ok();
    cr.restore().ok();
    glass::paint_glass_fill(cr, w, h, sx, sy, true, false);
    cr.save().ok();
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, w, h);
    cr.rectangle(band_x, 2.0, band_w, h - 2.0);
    cr.clip();
    shellstyle::paint(cr, "TaskBar2Composited::TaskBar", 1, 0, 0.0, 0.0, w, h);
    cr.restore().ok();
    cr.save().ok();
    cr.rectangle(band_x, 2.0, band_w, h - 2.0);
    cr.clip();
    shellstyle::paint(cr, "TaskBand2Composited::TaskBand2", 1, 0, band_x, 0.0, band_w, h);
    cr.restore().ok();
}

/// How much narrower the front button of a group is: TaskBand2 parts 13/14 (8 px) show one
/// stacked edge behind it for 2 windows and two for 3 or more.
fn stack_inset(windows: usize) -> f64 {
    match windows { 0 | 1 => 0.0, 2 => 5.0, _ => 6.0 }
}

/// TaskBand2's HOTTRACKING colour: the glow of icons without a colour of their own.
fn default_hot_color() -> [f64; 3] {
    shellstyle::color("TaskBand2", 5, 0, msstyles::HOTTRACKING).unwrap_or([51.0 / 255.0, 194.0 / 255.0, 1.0])
}

/// Win7's colour hot-track colour: the predominant colour of the icon, where black, white and
/// greys don't count as colours. Hue histogram (15° bins, weighted by saturation × value × alpha)
/// over the opaque pixels; the winning bin and its neighbours are averaged and brought to full
/// brightness (the glow is a light, not a paint). None for icons without colour.
fn dominant_color(pb: &Pixbuf) -> Option<[f64; 3]> {
    let (w, h, n, rs) = (pb.width() as usize, pb.height() as usize, pb.n_channels() as usize, pb.rowstride() as usize);
    if n < 3 { return None; }
    let px = pb.read_pixel_bytes();
    let mut bins = [(0.0f64, [0.0f64; 3]); 24];
    let mut opaque = 0.0;
    for y in 0..h {
        for x in 0..w {
            let o = y * rs + x * n;
            if o + n > px.len() { continue; }
            let a = if n == 4 { px[o + 3] as f64 / 255.0 } else { 1.0 };
            if a < 0.5 { continue; }
            opaque += 1.0;
            let (r, g, b) = (px[o] as f64 / 255.0, px[o + 1] as f64 / 255.0, px[o + 2] as f64 / 255.0);
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            let s = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
            if s < 0.25 || mx < 0.2 { continue; }
            let d = mx - mn;
            let sector = if mx == r { ((g - b) / d).rem_euclid(6.0) } else if mx == g { (b - r) / d + 2.0 } else { (r - g) / d + 4.0 };
            let wgt = s * mx * a;
            let k = ((sector * 60.0 / 15.0) as usize).min(23);
            bins[k].0 += wgt;
            for (c, v) in bins[k].1.iter_mut().zip([r, g, b]) { *c += v * wgt; }
        }
    }
    let around = |k: usize| [(k + 23) % 24, k, (k + 1) % 24];
    let score = |k: usize| around(k).iter().map(|i| bins[*i].0).sum::<f64>();
    let best = (0..24).max_by(|a, b| score(*a).total_cmp(&score(*b)))?;
    let (mut wsum, mut c) = (0.0, [0.0; 3]);
    for i in around(best) {
        wsum += bins[i].0;
        for j in 0..3 { c[j] += bins[i].1[j]; }
    }
    // a few coloured pixels in a grey icon don't make it coloured
    if wsum <= 0.0 || wsum < opaque * 0.03 { return None; }
    let c = c.map(|v| v / wsum);
    let m = c[0].max(c[1]).max(c[2]);
    (m > 0.0).then(|| c.map(|v| v / m))
}

/// Paint a task button from TaskBand2 part 5 (60×40 frames: 1 normal, 2 hot, 3 pressed, 4 active,
/// 5 active hot, 6 active pressed, 7 flashing) with the colour hot-track glow on hover.
fn themed_task_button(b: &gtk::Button, v: &Rc<BtnVis>) {
    b.style_context().add_class("w7");
    v.color.set(default_hot_color());
    b.add_events(gdk::EventMask::POINTER_MOTION_MASK | gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
    {
        let v = v.clone();
        b.connect_enter_notify_event(move |b, e| {
            v.hover.set(true);
            v.x.set(e.position().0);
            animate_hot(b, &v);
            glib::Propagation::Proceed
        });
    }
    {
        let v = v.clone();
        b.connect_leave_notify_event(move |b, e| {
            // crossings into the button's own children are not a leave
            if e.detail() == gdk::NotifyType::Inferior { return glib::Propagation::Proceed; }
            v.hover.set(false);
            v.pressed.set(false);
            animate_hot(b, &v);
            glib::Propagation::Proceed
        });
    }
    {
        let v = v.clone();
        b.connect_motion_notify_event(move |b, e| {
            v.x.set(e.position().0);
            if v.fade.get() > 0.0 { b.queue_draw(); }
            glib::Propagation::Proceed
        });
    }
    {
        let v = v.clone();
        b.connect_button_press_event(move |b, e| {
            if e.button() == 1 { v.pressed.set(true); b.queue_draw(); }
            glib::Propagation::Proceed
        });
    }
    {
        let v = v.clone();
        b.connect_button_release_event(move |b, _| {
            v.pressed.set(false);
            b.queue_draw();
            glib::Propagation::Proceed
        });
    }
    let v = v.clone();
    b.connect_draw(move |b, cr| {
        paint_task_button(cr, &v, b.allocated_width() as f64, b.allocated_height() as f64);
        glib::Propagation::Proceed
    });
}

/// A window asks for attention: like FlashWindowEx on Win7 the button blinks orange (7 times, at the
/// caret blink rate) and then stays orange until the window is activated.
fn start_flash(b: &gtk::Button, v: &Rc<BtnVis>) {
    if let Some(id) = v.flash.borrow_mut().take() { id.remove(); }
    let (b2, v2) = (b.downgrade(), Rc::downgrade(v));
    let mut tick = 0;
    let id = glib::timeout_add_local(std::time::Duration::from_millis(530), move || {
        let (Some(b), Some(v)) = (b2.upgrade(), v2.upgrade()) else { return glib::ControlFlow::Break };
        tick += 1;
        // stops early once the window got activated
        let phase = if v.urgent.get() { flash_off(tick) } else { None };
        v.flash_off.set(phase.unwrap_or(false));
        b.queue_draw();
        if phase.is_none() { v.flash.borrow_mut().take(); return glib::ControlFlow::Break; }
        glib::ControlFlow::Continue
    });
    *v.flash.borrow_mut() = Some(id);
}

/// Blink phase after `tick` half-periods: Some(true) = shown as normal, Some(false) = orange,
/// None = done (the button stays orange). Seven orange flashes, like FlashWindowEx's default.
fn flash_off(tick: u32) -> Option<bool> { (tick < 13).then_some(tick % 2 == 1) }

/// Fade the hot state in (0.1 s) or out (0.3 s), the way Win7's taskbar buttons light up and dim.
fn animate_hot(b: &gtk::Button, v: &Rc<BtnVis>) {
    b.queue_draw();
    if v.anim.borrow().is_some() { return; }
    let (b2, v2) = (b.downgrade(), Rc::downgrade(v));
    let mut last = std::time::Instant::now();
    let id = glib::timeout_add_local(std::time::Duration::from_millis(15), move || {
        let (Some(b), Some(v)) = (b2.upgrade(), v2.upgrade()) else { return glib::ControlFlow::Break };
        let dt = last.elapsed().as_secs_f64();
        last = std::time::Instant::now();
        let (target, dur) = if v.hover.get() { (1.0, 0.1) } else { (0.0, 0.3) };
        let f = v.fade.get();
        let nf = if f < target { (f + dt / dur).min(target) } else { (f - dt / dur).max(target) };
        v.fade.set(nf);
        b.queue_draw();
        if nf == target {
            v.anim.borrow_mut().take();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
    *v.anim.borrow_mut() = Some(id);
}

fn paint_task_button(cr: &cairo::Context, v: &BtnVis, w: f64, h: f64) {
    let count = v.count.get();
    let running = count > 0;
    let active = v.active.get() && running;
    let fw = w - stack_inset(count);
    if count >= 2 {
        // the stacked edges (TaskBand2 13 inactive / 14 active; state 2 = one edge, 3 = two)
        shellstyle::paint(cr, "TaskBand2", if active { 14 } else { 13 }, if count == 2 { 2 } else { 3 }, w - 8.0, 0.0, 8.0, h);
    }
    let flashing = v.urgent.get() && !active && !v.flash_off.get();
    let base = if flashing { Some(7) } else if active { Some(4) } else if running { Some(1) } else { None };
    let pressed = v.pressed.get() && v.hover.get();
    let hot = if pressed { 1.0 } else { v.fade.get() };
    if let Some(s) = base {
        if hot < 1.0 || flashing {
            cr.push_group();
            shellstyle::paint(cr, "TaskBand2", 5, s, 0.0, 0.0, fw, h);
            cr.pop_group_to_source().ok();
            cr.paint_with_alpha(if flashing { 1.0 } else { 1.0 - hot }).ok();
        }
    }
    if hot > 0.0 {
        let st = match (active, pressed) { (true, true) => 6, (true, false) => 5, (false, true) => 3, (false, false) => 2 };
        cr.push_group();
        if !flashing { shellstyle::paint(cr, "TaskBand2", 5, st, 0.0, 0.0, fw, h); }
        paint_hot_glow(cr, v.color.get(), v.x.get(), fw, h, pressed);
        cr.pop_group_to_source().ok();
        cr.paint_with_alpha(hot).ok();
    }
}

/// The colour hot-track light: the button fills with the icon's colour from below, brightest in
/// an elliptical light at the bottom edge centred on the pointer, with a near-white core; the
/// inner edge picks up the colour around the pointer too.
fn paint_hot_glow(cr: &cairo::Context, c: [f64; 3], mx: f64, w: f64, h: f64, pressed: bool) {
    let mx = mx.clamp(2.0, (w - 2.0).max(2.0));
    let k = if pressed { 0.75 } else { 1.0 };
    cr.save().ok();
    glass::rounded(cr, 1.0, 1.0, w - 2.0, h - 2.0, 2.0);
    cr.clip();
    let lg = cairo::LinearGradient::new(0.0, 0.0, 0.0, h);
    lg.add_color_stop_rgba(0.0, c[0], c[1], c[2], 0.0);
    lg.add_color_stop_rgba(0.45, c[0], c[1], c[2], 0.08 * k);
    lg.add_color_stop_rgba(1.0, c[0], c[1], c[2], 0.2 * k);
    cr.set_source(&lg).ok();
    cr.paint().ok();
    let blob = |rx: f64, ry: f64, col: [f64; 3], stops: &[(f64, f64)]| {
        cr.save().ok();
        cr.translate(mx, h);
        cr.scale(rx, ry);
        let rg = cairo::RadialGradient::new(0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        for (o, a) in stops { rg.add_color_stop_rgba(*o, col[0], col[1], col[2], a * k); }
        cr.set_source(&rg).ok();
        cr.paint().ok();
        cr.restore().ok();
    };
    blob(w * 0.62, h * 0.9, c, &[(0.0, 0.95), (0.4, 0.5), (1.0, 0.0)]);
    let core = c.map(|v| v + (1.0 - v) * 0.7);
    blob(w * 0.28, h * 0.38, core, &[(0.0, 0.85), (1.0, 0.0)]);
    cr.restore().ok();
    // the edge lights up around the pointer
    let eg = cairo::LinearGradient::new(mx - w, 0.0, mx + w, 0.0);
    eg.add_color_stop_rgba(0.0, c[0], c[1], c[2], 0.1 * k);
    eg.add_color_stop_rgba(0.5, c[0], c[1], c[2], 0.65 * k);
    eg.add_color_stop_rgba(1.0, c[0], c[1], c[2], 0.1 * k);
    cr.set_source(&eg).ok();
    cr.set_line_width(1.0);
    glass::rounded(cr, 1.5, 1.5, w - 3.0, h - 3.0, 2.0);
    cr.stroke().ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixbuf(px: Vec<u8>) -> Pixbuf {
        Pixbuf::from_bytes(&glib::Bytes::from_owned(px), gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 16, 16, 64)
    }
    fn solid(rgba: [u8; 4], n: usize) -> Vec<u8> { (0..n).flat_map(|_| rgba).collect() }

    #[test]
    fn dominant_color_ignores_greys_and_picks_the_main_hue() {
        // 16×16: 3/4 orange, 1/4 grey, a white row
        let mut px = solid([255, 128, 0, 255], 192);
        px.extend(solid([128, 128, 128, 255], 48));
        px.extend(solid([255, 255, 255, 255], 16));
        let c = dominant_color(&pixbuf(px)).unwrap();
        assert!((c[0] - 1.0).abs() < 1e-9 && (c[1] - 128.0 / 255.0).abs() < 0.01 && c[2] < 0.01, "{c:?}");
        // greys and transparent colour only: no colour of its own
        let mut px = solid([90, 90, 90, 255], 128);
        px.extend(solid([255, 0, 0, 0], 128));
        assert_eq!(dominant_color(&pixbuf(px)), None);
    }

    #[test]
    fn flashing_blinks_seven_times_then_stays_orange() {
        // tick 0 (the request) is orange; count the orange half-periods until it settles
        let orange = 1 + (1..100).take_while(|t| flash_off(*t).is_some()).filter(|t| flash_off(*t) == Some(false)).count();
        assert_eq!(orange, 7);
        assert_eq!(flash_off(1), Some(true));
        assert_eq!(flash_off(13), None);
    }

    #[test]
    fn stacked_buttons_narrow_by_the_edge_strip() {
        assert_eq!(stack_inset(1), 0.0);
        assert_eq!(stack_inset(2), 5.0);
        assert_eq!(stack_inset(7), 6.0);
    }
}

/// Cascade (0), stack (1) or tile side by side (2) the open windows of the current desktop on the
/// primary monitor's work area, like Win7's taskbar menu.
fn arrange_windows(wm: &Rc<Wm>, how: u8) {
    let desk = wm.current_desktop();
    let wins: Vec<Client> = wm.clients().into_iter().filter(|c| !c.minimized && (c.desktop == desk || c.desktop == 0xffffffff)).collect();
    if wins.is_empty() { return; }
    let g = util::primary_geometry();
    let (x0, y0, w, h) = (g.x(), g.y(), g.width(), g.height() - config::get().taskbar_height);
    let n = wins.len() as i32;
    for (i, c) in wins.iter().enumerate() {
        let i = i as i32;
        match how {
            0 => {
                // Win7 cascades from the top left, 26 px a step, each window about 2/3 of the area
                let step = 26;
                let k = i % (((h / 3) / step).max(1));
                wm.place_outer(c.id, x0 + k * step, y0 + k * step, w * 2 / 3, h * 2 / 3);
            }
            1 => { let hh = h / n; wm.place_outer(c.id, x0, y0 + i * hh, w, hh); }
            _ => { let ww = w / n; wm.place_outer(c.id, x0 + i * ww, y0, ww, h); }
        }
    }
}

#[derive(Clone, Copy)]
enum JlIcon { Pin, Close }

/// The Jump List's own 16 px task icons, as in Win7: Close window is the theme's small red
/// caption close button (aero.msstyles Window part 19) with a white, dark-outlined X; the pin is
/// Win7's blue pushpin, drawn (no Windows resource carries it as an image).
fn jl_icon(which: JlIcon) -> gtk::Image {
    let surf = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 16, 16).expect("surface");
    {
        let cr = gtk::cairo::Context::new(&surf).expect("cairo");
        match which {
            JlIcon::Close => {
                if !shellstyle::paint(&cr, "Window", 19, 1, 1.0, 1.0, 14.0, 14.0) {
                    crate::glass::rounded(&cr, 1.5, 1.5, 13.0, 13.0, 2.0);
                    let g = gtk::cairo::LinearGradient::new(0.0, 1.0, 0.0, 15.0);
                    g.add_color_stop_rgb(0.0, 0.93, 0.55, 0.47);
                    g.add_color_stop_rgb(0.5, 0.80, 0.25, 0.16);
                    g.add_color_stop_rgb(1.0, 0.72, 0.18, 0.10);
                    cr.set_source(&g).ok();
                    cr.fill_preserve().ok();
                    cr.set_source_rgb(0.45, 0.10, 0.06);
                    cr.set_line_width(1.0);
                    cr.stroke().ok();
                }
                // the X: a dark outline under a white stroke
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                for (w, c) in [(3.2, (0.25, 0.22, 0.22)), (1.6, (1.0, 1.0, 1.0))] {
                    cr.set_line_width(w);
                    cr.set_source_rgb(c.0, c.1, c.2);
                    cr.move_to(5.0, 5.0); cr.line_to(11.0, 11.0);
                    cr.move_to(11.0, 5.0); cr.line_to(5.0, 11.0);
                    cr.stroke().ok();
                }
            }
            JlIcon::Pin => {
                // Win7's pushpin: a grey needle pointing down-left, a blue collar disc and a
                // round glossy blue head up-right
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                cr.set_line_width(1.8);
                cr.set_source_rgb(0.42, 0.44, 0.48);
                cr.move_to(1.5, 14.5); cr.line_to(6.5, 9.5);
                cr.stroke().ok();
                cr.save().ok();
                cr.translate(7.6, 8.4);
                cr.rotate(-std::f64::consts::FRAC_PI_4);
                cr.scale(1.0, 0.5);
                cr.arc(0.0, 0.0, 5.6, 0.0, std::f64::consts::TAU);
                cr.restore().ok();
                cr.set_source_rgb(0.12, 0.34, 0.70);
                cr.fill().ok();
                let g = gtk::cairo::RadialGradient::new(9.6, 4.0, 0.5, 10.6, 5.4, 4.8);
                g.add_color_stop_rgb(0.0, 0.80, 0.92, 1.0);
                g.add_color_stop_rgb(0.45, 0.30, 0.60, 0.95);
                g.add_color_stop_rgb(1.0, 0.08, 0.30, 0.68);
                cr.arc(10.6, 5.4, 4.4, 0.0, std::f64::consts::TAU);
                cr.set_source(&g).ok();
                cr.fill().ok();
            }
        }
    }
    gtk::Image::from_surface(Some(&surf))
}
