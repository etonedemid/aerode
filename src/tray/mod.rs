//! The Windows 7 notification area for programs: the icons applications put next to the clock
//! (StatusNotifierItem over D-Bus, legacy XEmbed tray icons, and the shell's own extras such as
//! external-monitor brightness), the "Show hidden icons" arrow with its overflow flyout, and the
//! Notification Area Icons settings behind "Customize...".
//!
//! Like Win7, a program's icon starts out in the overflow ("Only show notifications"); the user
//! promotes it with Customize.... An icon that asks for attention comes out onto the taskbar until
//! it calms down, unless the user picked "Hide icon and notifications".
mod dbusmenu;
mod sni;
mod xembed;

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::{config, flyouts, util};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Passive,
    Active,
    Attention,
}

#[derive(Clone)]
pub enum Backend {
    Sni(Rc<sni::Item>),
    XEmbed(u32),
    Builtin(Rc<dyn Fn(&gtk::Widget)>),
}

pub struct Item {
    pub id: String,
    /// per-program key for the user's behaviour setting
    pub key: String,
    pub title: String,
    pub tooltip: String,
    pub icon: Option<Pixbuf>,
    pub status: Status,
    /// false while the program has hidden its icon
    pub shown: bool,
    pub backend: Backend,
}

/// The behaviours of Win7's Notification Area Icons page (config value, label).
pub const BEHAVIORS: &[(&str, &str)] = &[
    ("show", "Show icon and notifications"),
    ("hide", "Hide icon and notifications"),
    ("notify", "Only show notifications"),
];

/// A program's notification behaviour ("notify" = Win7's default for new icons); "hide" also
/// means its notifications should not be shown.
pub fn behavior(key: &str) -> String {
    config::get().tray_icons.get(key).cloned().unwrap_or_else(|| "notify".into())
}

fn on_taskbar(cfg: &config::Config, it: &Item) -> bool {
    if cfg.tray_show_all { return true; }
    match cfg.tray_icons.get(&it.key).map(String::as_str).unwrap_or("notify") {
        "show" => true,
        "hide" => false,
        // hidden icons are promoted while they have something to say
        _ => it.status == Status::Attention,
    }
}

type Buttons = Vec<(String, gtk::Button, gtk::Image)>;

struct Area {
    chevron: gtk::Button,
    visible: gtk::Box,
    items: RefCell<Vec<Item>>,
    buttons: RefCell<Buttons>,
    /// the open overflow flyout, its grid and buttons
    overflow: RefCell<Option<(gtk::Window, gtk::Grid, Buttons)>>,
}

thread_local! {
    static AREA: RefCell<Option<Rc<Area>>> = const { RefCell::new(None) };
    static FRAMES: RefCell<HashMap<(String, i32, i32), Option<Pixbuf>>> = RefCell::new(HashMap::new());
    static CUSTOMIZE: RefCell<Option<gtk::Window>> = const { RefCell::new(None) };
    /// id of the icon being dragged
    static DRAGGING: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn area() -> Option<Rc<Area>> {
    AREA.with(|a| a.borrow().clone())
}

/// Build the notification area (left of the system icons) and start listening for icons.
pub fn attach(tb: &Rc<crate::taskbar::Taskbar>) {
    let visible = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let chevron = gtk::Button::new();
    chevron.style_context().add_class("chevron");
    chevron.set_tooltip_text(Some("Show hidden icons"));
    chevron.set_size_request(21, -1);
    chevron.set_no_show_all(true);
    chevron.connect_draw(|w, cr| {
        let open = area().map(|a| a.overflow.borrow().is_some()).unwrap_or(false);
        paint_chevron(w, cr, open);
        glib::Propagation::Stop
    });
    chevron.connect_clicked(|_| toggle_overflow());
    tb.tray.pack_end(&visible, false, false, 0);
    tb.tray.pack_end(&chevron, false, false, 0);
    // drag and drop: onto the notification area = show, onto the arrow = back into the overflow
    drop_target(&tb.tray, "show");
    drop_target(&chevron, "notify");
    chevron.connect_drag_motion(|_, _, _, _, _| {
        // hovering a dragged taskbar icon over the arrow opens the overflow to drop it in
        let from_taskbar = DRAGGING.with(|d| d.borrow().clone())
            .and_then(|id| area().map(|a| a.buttons.borrow().iter().any(|(i, _, _)| *i == id))).unwrap_or(false);
        if from_taskbar && area().map(|a| a.overflow.borrow().is_none()).unwrap_or(false) { toggle_overflow(); }
        false
    });
    let a = Rc::new(Area { chevron, visible, items: RefCell::new(vec![]), buttons: RefCell::new(vec![]), overflow: RefCell::new(None) });
    AREA.with(|x| *x.borrow_mut() = Some(a));
    sni::start();
    xembed::start();
}

/// Add one of the shell's own icons (e.g. monitor brightness); `activate` gets the widget to
/// anchor its flyout to (the icon, or the arrow when it was clicked in the overflow).
pub fn add_builtin(key: &str, title: &str, icon: &str, tooltip: &str, activate: impl Fn(&gtk::Widget) + 'static) {
    upsert(Item {
        id: format!("builtin:{key}"),
        key: key.into(),
        title: title.into(),
        tooltip: tooltip.into(),
        icon: util::pixbuf(icon, 16),
        status: Status::Active,
        shown: true,
        backend: Backend::Builtin(Rc::new(activate)),
    });
}

/// Insert or replace an item.
pub(crate) fn upsert(item: Item) {
    let Some(a) = area() else { return };
    let cfg = config::get();
    let moved = {
        let mut items = a.items.borrow_mut();
        match items.iter_mut().find(|i| i.id == item.id) {
            Some(old) => {
                let moved = old.shown != item.shown || on_taskbar(&cfg, old) != on_taskbar(&cfg, &item);
                *old = item;
                moved
            }
            None => { items.push(item); true }
        }
    };
    if moved { a.sync(); } else { a.refresh_buttons(); }
}

/// Change an item in place (icon/tooltip updates).
pub(crate) fn update(id: &str, f: impl FnOnce(&mut Item)) {
    let Some(a) = area() else { return };
    let cfg = config::get();
    let moved = {
        let mut items = a.items.borrow_mut();
        let Some(it) = items.iter_mut().find(|i| i.id == id) else { return };
        let before = (it.shown, on_taskbar(&cfg, it));
        f(it);
        before != (it.shown, on_taskbar(&cfg, it))
    };
    if moved { a.sync(); } else { a.refresh_buttons(); }
}

pub(crate) fn remove(id: &str) {
    let Some(a) = area() else { return };
    let had = {
        let mut items = a.items.borrow_mut();
        let n = items.len();
        items.retain(|i| i.id != id);
        n != items.len()
    };
    if had { a.sync(); }
}

/// Re-apply the behaviour settings (after Customize... or `aerode reload`).
pub fn reload() {
    if let Some(a) = area() { a.sync(); }
}

fn set_button(b: &gtk::Button, img: &gtk::Image, it: &Item) {
    img.set_from_pixbuf(it.icon.as_ref());
    let tip = it.tooltip.trim();
    if b.tooltip_text().as_deref() != Some(tip) { b.set_tooltip_text(Some(tip)); }
}

impl Area {
    /// Put every icon where its behaviour says: on the taskbar or in the overflow.
    fn sync(self: &Rc<Self>) {
        let cfg = config::get();
        let hidden;
        {
            let items = self.items.borrow();
            let want: Vec<&Item> = items.iter().filter(|i| i.shown && on_taskbar(&cfg, i)).collect();
            hidden = items.iter().filter(|i| i.shown && !on_taskbar(&cfg, i)).count();
            let mut btns = self.buttons.borrow_mut();
            btns.retain(|(id, b, _)| {
                let keep = want.iter().any(|i| &i.id == id);
                if !keep { self.visible.remove(b); }
                keep
            });
            for (pos, it) in want.iter().enumerate() {
                if !btns.iter().any(|(id, _, _)| *id == it.id) {
                    let (b, img) = item_button(&it.id, false);
                    self.visible.pack_start(&b, false, false, 0);
                    b.show_all();
                    btns.push((it.id.clone(), b, img));
                }
                if let Some((_, b, img)) = btns.iter().find(|(id, _, _)| *id == it.id) {
                    set_button(b, img, it);
                    self.visible.reorder_child(b, pos as i32);
                }
            }
        }
        // Win7 shows the arrow only when something is hidden
        self.chevron.set_visible(hidden > 0);
        let open = self.overflow.borrow().as_ref().map(|(w, g, _)| (w.clone(), g.clone()));
        if let Some((w, grid)) = open {
            if hidden == 0 {
                flyouts::close_open();
            } else {
                let btns = self.fill_grid(&grid);
                if let Some(o) = self.overflow.borrow_mut().as_mut() { o.2 = btns; }
                reposition(&w, self.chevron.upcast_ref());
            }
        }
    }

    /// Icons/tooltips changed but nothing moved.
    fn refresh_buttons(&self) {
        let items = self.items.borrow();
        let ov = self.overflow.borrow();
        let all = self.buttons.borrow();
        for (id, b, img) in all.iter().chain(ov.iter().flat_map(|o| o.2.iter())) {
            if let Some(it) = items.iter().find(|i| &i.id == id) { set_button(b, img, it); }
        }
    }

    fn fill_grid(&self, grid: &gtk::Grid) -> Buttons {
        for c in grid.children() { grid.remove(&c); }
        let cfg = config::get();
        let items = self.items.borrow();
        let hidden: Vec<&Item> = items.iter().filter(|i| i.shown && !on_taskbar(&cfg, i)).collect();
        // Win7 keeps the overflow roughly square
        let cols = ((hidden.len() as f64).sqrt().ceil() as i32).max(1);
        let mut out = vec![];
        for (n, it) in hidden.iter().enumerate() {
            let (b, img) = item_button(&it.id, true);
            set_button(&b, &img, it);
            grid.attach(&b, n as i32 % cols, n as i32 / cols, 1, 1);
            out.push((it.id.clone(), b, img));
        }
        grid.show_all();
        out
    }
}

/// Keep an open flyout sitting above its anchor after its content changed size.
fn reposition(w: &gtk::Window, anchor: &gtk::Widget) {
    let Some(child) = w.child() else { return };
    let (_, nat) = child.preferred_size();
    let geo = util::primary_geometry();
    let (ax, _) = anchor.window().map(|gw| { let (_, x, y) = gw.origin(); (x, y) }).unwrap_or((0, 0));
    let alloc = anchor.allocation();
    let x = (ax + alloc.x() + alloc.width() / 2 - nat.width / 2).clamp(geo.x() + 4, geo.x() + geo.width() - nat.width - 4);
    let y = geo.y() + geo.height() - config::get().taskbar_height - nat.height - 4;
    w.resize(nat.width.max(1), nat.height.max(1));
    w.move_(x, y);
}

/// A notification-area icon: on the taskbar (`overflow` false) or in the overflow grid.
fn item_button(id: &str, overflow: bool) -> (gtk::Button, gtk::Image) {
    let b = gtk::Button::new();
    let img = gtk::Image::new();
    b.add(&img);
    if overflow {
        b.style_context().add_class("overflow-icon");
        b.set_size_request(32, 32);
    } else {
        themed_tray_button(&b);
    }
    b.add_events(gdk::EventMask::SCROLL_MASK | gdk::EventMask::SMOOTH_SCROLL_MASK);
    {
        let id = id.to_string();
        b.connect_button_release_event(move |b, e| {
            // like a click: only when released over the icon
            let (x, y) = e.position();
            let al = b.allocation();
            if x >= 0.0 && y >= 0.0 && x < al.width() as f64 && y < al.height() as f64 {
                click(&id, e, b.upcast_ref(), overflow);
            }
            glib::Propagation::Proceed
        });
    }
    {
        let id = id.to_string();
        let acc = Cell::new((0.0f64, 0.0f64));
        b.connect_scroll_event(move |_, e| {
            let (dx, dy) = match e.direction() {
                gdk::ScrollDirection::Up => (0, 1),
                gdk::ScrollDirection::Down => (0, -1),
                gdk::ScrollDirection::Left => (-1, 0),
                gdk::ScrollDirection::Right => (1, 0),
                gdk::ScrollDirection::Smooth => {
                    let (ax, ay) = acc.get();
                    let (ax, ay) = (ax + e.delta().0, ay - e.delta().1);
                    let (nx, ny) = (ax.trunc() as i32, ay.trunc() as i32);
                    acc.set((ax - nx as f64, ay - ny as f64));
                    (nx, ny)
                }
                _ => (0, 0),
            };
            if dy != 0 { scroll(&id, dy, false); }
            if dx != 0 { scroll(&id, dx, true); }
            glib::Propagation::Stop
        });
    }
    // Win7: drag an icon out of the overflow onto the taskbar to show it there, or onto the
    // arrow / into the overflow to hide it again
    b.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &dnd_targets(), gdk::DragAction::MOVE);
    {
        let id = id.to_string();
        b.connect_drag_begin(move |_, ctx| {
            DRAGGING.with(|d| *d.borrow_mut() = Some(id.clone()));
            flyouts::keep_open(true);
            let icon = area().and_then(|a| a.items.borrow().iter().find(|i| i.id == id).and_then(|i| i.icon.clone()));
            if let Some(pb) = icon { ctx.drag_set_icon_pixbuf(&pb, 8, 8); }
        });
    }
    {
        let id = id.to_string();
        b.connect_drag_data_get(move |_, _, sel, _, _| sel.set(&sel.target(), 8, id.as_bytes()));
    }
    b.connect_drag_end(|_, _| {
        DRAGGING.with(|d| *d.borrow_mut() = None);
        flyouts::keep_open(false);
        if area().map(|a| a.overflow.borrow().is_some()).unwrap_or(false) { flyouts::close_open(); }
    });
    if !overflow {
        // dragged off the taskbar and let go anywhere else: hidden, like Win7
        let id = id.to_string();
        b.connect_drag_failed(move |_, _, _| {
            set_behavior(&id, "notify");
            glib::Propagation::Stop
        });
    }
    (b, img)
}

const DND_TARGET: &str = "AERODE_NOTIFICATION_ICON";

fn dnd_targets() -> [gtk::TargetEntry; 1] {
    [gtk::TargetEntry::new(DND_TARGET, gtk::TargetFlags::SAME_APP, 0)]
}

/// Dropping an icon on `w` gives its program this behaviour.
fn drop_target(w: &impl IsA<gtk::Widget>, behavior: &'static str) {
    w.drag_dest_set(gtk::DestDefaults::MOTION | gtk::DestDefaults::DROP, &dnd_targets(), gdk::DragAction::MOVE);
    w.connect_drag_data_received(move |_, _, _, _, sel, _, _| set_behavior(&String::from_utf8_lossy(&sel.data()), behavior));
}

/// Store a behaviour for the program behind icon `id` and re-arrange.
fn set_behavior(id: &str, behavior: &str) {
    let Some(key) = area().and_then(|a| a.items.borrow().iter().find(|i| i.id == id).map(|i| i.key.clone())) else { return };
    config::update(|c| {
        // "notify" is the default and not stored
        if behavior == "notify" { c.tray_icons.remove(&key); } else { c.tray_icons.insert(key, behavior.to_string()); }
    });
    reload();
}

fn backend_of(id: &str) -> Option<Backend> {
    area()?.items.borrow().iter().find(|i| i.id == id).map(|i| i.backend.clone())
}

fn click(id: &str, e: &gdk::EventButton, w: &gtk::Widget, overflow: bool) {
    let (Some(a), Some(backend)) = (area(), backend_of(id)) else { return };
    let (rx, ry) = e.root();
    let (x, y) = (rx as i32, ry as i32);
    // clicking an icon in the overflow closes it, like Win7
    let anchor: gtk::Widget = if overflow { flyouts::close_open(); a.chevron.clone().upcast() } else { w.clone() };
    match (backend, e.button()) {
        (Backend::Sni(it), 1) => sni::activate(&it, x, y),
        (Backend::Sni(it), 2) => sni::secondary(&it, x, y),
        (Backend::Sni(it), 3) => sni::context_menu(&it, x, y),
        (Backend::XEmbed(win), b @ 1..=3) => xembed::click(win, b as u8, x, y),
        (Backend::Builtin(f), 1) => f(&anchor),
        _ => {}
    }
}

fn scroll(id: &str, delta: i32, horizontal: bool) {
    match backend_of(id) {
        Some(Backend::Sni(it)) => sni::scroll(&it, delta, horizontal),
        Some(Backend::XEmbed(win)) => {
            // X buttons 4/5 (wheel up/down), 6/7 (left/right)
            let b = match (horizontal, delta > 0) { (false, true) => 4, (false, false) => 5, (true, false) => 6, (true, true) => 7 };
            let (x, y) = gdk::Display::default().and_then(|d| d.default_seat()).and_then(|s| s.pointer())
                .map(|p| { let (_, x, y) = p.position(); (x, y) }).unwrap_or((0, 0));
            for _ in 0..delta.unsigned_abs().min(10) { xembed::click(win, b, x, y); }
        }
        _ => {}
    }
}

// ------------------------------------------------------------------ overflow flyout
fn toggle_overflow() {
    if flyouts::close_open() { return; }
    let Some(a) = area() else { return };
    let grid = gtk::Grid::new();
    grid.set_halign(gtk::Align::Center);
    grid.style_context().add_class("overflow-grid");
    let btns = a.fill_grid(&grid);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.pack_start(&grid, false, false, 0);
    drop_target(&body, "notify");
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let link = gtk::Button::with_label("Customize...");
    link.style_context().add_class("link");
    link.set_halign(gtk::Align::Center);
    link.connect_clicked(|_| { flyouts::close_open(); customize(); });
    foot.set_center_widget(Some(&link));
    let w = flyouts::popup(a.chevron.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
    w.style_context().add_class("overflow");
    *a.overflow.borrow_mut() = Some((w.clone(), grid, btns));
    // Win7 drops the arrow's tooltip while the overflow is open
    a.chevron.set_has_tooltip(false);
    a.chevron.queue_draw();
    let a2 = a.clone();
    w.connect_destroy(move |_| {
        *a2.overflow.borrow_mut() = None;
        a2.chevron.set_has_tooltip(true);
        a2.chevron.queue_draw();
    });
}

// ------------------------------------------------------------------ msstyles painting
/// One state of an aero.msstyles part as a pixbuf (cached).
fn theme_frame(class: &str, part: i32, state: i32) -> Option<Pixbuf> {
    FRAMES.with(|f| {
        f.borrow_mut().entry((class.to_string(), part, state)).or_insert_with(|| {
            let img = crate::glass::theme()?.frame(class, part, state)?;
            let (w, h) = (img.width() as i32, img.height() as i32);
            Some(Pixbuf::from_bytes(&glib::Bytes::from_owned(img.into_raw()), Colorspace::Rgb, true, 8, w, h, w * 4))
        }).clone()
    })
}

/// Draw `pb` stretched to w×h keeping the sizing margins (left, right, top, bottom) unscaled.
fn nine_slice(cr: &cairo::Context, pb: &Pixbuf, w: f64, h: f64, m: [i32; 4]) {
    let (iw, ih) = (pb.width() as f64, pb.height() as f64);
    let (l, r, t, b) = (m[0] as f64, m[1] as f64, m[2] as f64, m[3] as f64);
    let (l, r) = if l + r > w { (w / 2.0, w / 2.0) } else { (l, r) };
    let (t, b) = if t + b > h { (h / 2.0, h / 2.0) } else { (t, b) };
    let xs = [(0.0, m[0] as f64, 0.0, l), (m[0] as f64, iw - m[1] as f64, l, w - r), (iw - m[1] as f64, iw, w - r, w)];
    let ys = [(0.0, m[2] as f64, 0.0, t), (m[2] as f64, ih - m[3] as f64, t, h - b), (ih - m[3] as f64, ih, h - b, h)];
    for (sx0, sx1, dx0, dx1) in xs {
        for (sy0, sy1, dy0, dy1) in ys {
            if sx1 <= sx0 || sy1 <= sy0 || dx1 <= dx0 || dy1 <= dy0 { continue; }
            cr.save().ok();
            cr.rectangle(dx0, dy0, dx1 - dx0, dy1 - dy0);
            cr.clip();
            cr.translate(dx0, dy0);
            cr.scale((dx1 - dx0) / (sx1 - sx0), (dy1 - dy0) / (sy1 - sy0));
            cr.set_source_pixbuf(pb, -sx0, -sy0);
            cr.source().set_extend(cairo::Extend::Pad);
            cr.paint().ok();
            cr.restore().ok();
        }
    }
}

fn hover_state(w: &gtk::Widget) -> i32 {
    let st = w.state_flags();
    if st.contains(gtk::StateFlags::ACTIVE) { 3 } else if st.contains(gtk::StateFlags::PRELIGHT) { 2 } else { 1 }
}

/// Give a taskbar notification-area button Win7's hover/pressed look (TrayNotify::Toolbar).
pub fn themed_tray_button(b: &gtk::Button) {
    b.style_context().add_class("tray");
    if theme_frame("TrayNotify::Toolbar", 1, 2).is_none() { return; }
    b.style_context().add_class("w7tray");
    b.connect_draw(|w, cr| {
        let s = hover_state(w.upcast_ref());
        if s > 1 {
            if let Some(pb) = theme_frame("TrayNotify::Toolbar", 1, s) {
                let m = crate::glass::theme().and_then(|t| t.margins("TrayNotify::Toolbar", 1, 0, 3601)).unwrap_or([2, 2, 14, 14]);
                nine_slice(cr, &pb, w.allocated_width() as f64, w.allocated_height() as f64, m);
            }
        }
        glib::Propagation::Proceed
    });
}

/// The "Show hidden icons" arrow: TrayNotifyHoriz::Button (up arrow), TrayNotifyHorizOpen::Button
/// (down arrow) while the overflow is open.
fn paint_chevron(w: &gtk::Button, cr: &cairo::Context, open: bool) {
    let s = hover_state(w.upcast_ref());
    let cls = if open { "TrayNotifyHorizOpen::Button" } else { "TrayNotifyHoriz::Button" };
    let (ww, wh) = (w.allocated_width() as f64, w.allocated_height() as f64);
    if let Some(pb) = theme_frame(cls, 0, s) {
        cr.set_source_pixbuf(&pb, ((ww - pb.width() as f64) / 2.0).floor(), ((wh - pb.height() as f64) / 2.0).floor());
        cr.paint().ok();
        return;
    }
    // no msstyles: a plain white triangle
    let (cx, cy) = ((ww / 2.0).floor(), (wh / 2.0).floor());
    if s > 1 {
        crate::glass::rounded(cr, 1.5, cy - 9.5, ww - 3.0, 19.0, 2.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, if s == 3 { 0.15 } else { 0.25 });
        cr.fill_preserve().ok();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
        cr.set_line_width(1.0);
        cr.stroke().ok();
    }
    let d = if open { 1.0 } else { -1.0 };
    cr.move_to(cx - 4.5, cy - d * 2.5);
    cr.line_to(cx + 4.5, cy - d * 2.5);
    cr.line_to(cx, cy + d * 2.5);
    cr.close_path();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
    cr.fill().ok();
}

// ------------------------------------------------------------------ Customize...
/// Win7's "Notification Area Icons" page: a behaviour per icon, and "Always show all icons".
pub fn customize() {
    if let Some(w) = CUSTOMIZE.with(|c| c.borrow().clone()) { w.present(); return; }
    let Some(a) = area() else { return };
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title("Notification Area Icons");
    w.set_icon_name(Some("preferences-system"));
    w.set_default_size(660, 500);
    w.set_position(gtk::WindowPosition::Center);
    w.style_context().add_class("notify-icons");
    let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
    v.set_border_width(18);
    let h = gtk::Label::new(None);
    h.set_markup("<span size='large' foreground='#1e3287'>Select which icons and notifications appear on the taskbar</span>");
    h.set_xalign(0.0);
    v.pack_start(&h, false, false, 0);
    let d = gtk::Label::new(Some("If you choose to hide icons and notifications, you won't be notified about changes or updates. To view hidden icons at any time, click the arrow next to the notification area on the taskbar."));
    d.set_line_wrap(true);
    d.set_xalign(0.0);
    d.set_max_width_chars(90);
    v.pack_start(&d, false, false, 4);

    let grid = gtk::Grid::new();
    grid.style_context().add_class("notify-list");
    grid.set_column_spacing(0);
    for (col, t) in ["Icons", "Behaviors"].iter().enumerate() {
        let l = gtk::Label::new(Some(t));
        l.set_xalign(0.0);
        l.style_context().add_class("notify-head");
        l.set_hexpand(col == 0);
        grid.attach(&l, col as i32, 0, 1, 1);
    }
    let cfg = config::get();
    let mut combos: Vec<(String, gtk::ComboBoxText)> = vec![];
    {
        let items = a.items.borrow();
        let mut row = 1;
        for it in items.iter() {
            if combos.iter().any(|(k, _)| *k == it.key) { continue; }
            // Win7 lists the 16 px tray icon beside the name, the tooltip greyed below it
            let left = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            left.style_context().add_class("notify-row");
            let img = gtk::Image::from_pixbuf(it.icon.as_ref());
            img.set_valign(gtk::Align::Start);
            img.set_size_request(16, 16);
            left.pack_start(&img, false, false, 0);
            let names = gtk::Box::new(gtk::Orientation::Vertical, 0);
            names.set_valign(gtk::Align::Center);
            let n = gtk::Label::new(Some(&it.title));
            n.set_xalign(0.0);
            n.set_ellipsize(gtk::pango::EllipsizeMode::End);
            names.pack_start(&n, false, false, 0);
            let desc = it.tooltip.lines().filter(|l| l.trim() != it.title.trim()).collect::<Vec<_>>().join(" ");
            if !desc.trim().is_empty() {
                let dl = gtk::Label::new(Some(desc.trim()));
                dl.set_xalign(0.0);
                dl.set_ellipsize(gtk::pango::EllipsizeMode::End);
                dl.set_max_width_chars(48);
                dl.style_context().add_class("dim");
                names.pack_start(&dl, false, false, 0);
            }
            left.pack_start(&names, true, true, 0);
            grid.attach(&left, 0, row, 1, 1);
            let c = gtk::ComboBoxText::new();
            for (id, l) in BEHAVIORS { c.append(Some(id), l); }
            c.set_active_id(Some(cfg.tray_icons.get(&it.key).map(String::as_str).unwrap_or("notify")));
            c.set_valign(gtk::Align::Center);
            c.set_margin_end(8);
            grid.attach(&c, 1, row, 1, 1);
            combos.push((it.key.clone(), c));
            row += 1;
        }
        if row == 1 {
            let l = gtk::Label::new(Some("No programs have put icons in the notification area."));
            l.style_context().add_class("dim");
            l.set_margin_top(12);
            grid.attach(&l, 0, 1, 2, 1);
        }
    }
    let frame = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    frame.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    frame.set_shadow_type(gtk::ShadowType::In);
    frame.add(&grid);
    v.pack_start(&frame, true, true, 0);

    let restore = gtk::Button::with_label("Restore default icon behaviors");
    restore.style_context().add_class("link");
    restore.set_halign(gtk::Align::Start);
    v.pack_start(&restore, false, false, 0);
    let all = gtk::CheckButton::with_label("Always show all icons and notifications on the taskbar");
    all.set_active(cfg.tray_show_all);
    v.pack_start(&all, false, false, 4);
    let combos = Rc::new(combos);
    {
        let combos = combos.clone();
        restore.connect_clicked(move |_| for (_, c) in combos.iter() { c.set_active_id(Some("notify")); });
    }
    {
        let (grid, restore) = (grid.clone(), restore.clone());
        let f = move |on: bool| { grid.set_sensitive(!on); restore.set_sensitive(!on); };
        f(cfg.tray_show_all);
        all.connect_toggled(move |c| f(c.is_active()));
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    ok.set_size_request(75, -1);
    cancel.set_size_request(75, -1);
    buttons.pack_end(&cancel, false, false, 0);
    buttons.pack_end(&ok, false, false, 0);
    v.pack_start(&buttons, false, false, 6);
    {
        let (w2, all) = (w.clone(), all.clone());
        ok.connect_clicked(move |_| {
            let show_all = all.is_active();
            let chosen: Vec<(String, String)> = combos.iter().filter_map(|(k, c)| c.active_id().map(|id| (k.clone(), id.to_string()))).collect();
            config::update(|c| {
                for (k, b) in chosen {
                    // "notify" is the default: keep the file small
                    if b == "notify" { c.tray_icons.remove(&k); } else { c.tray_icons.insert(k, b); }
                }
                c.tray_show_all = show_all;
            });
            reload();
            w2.close();
        });
    }
    { let w2 = w.clone(); cancel.connect_clicked(move |_| w2.close()); }
    w.connect_destroy(|_| CUSTOMIZE.with(|c| *c.borrow_mut() = None));
    w.add(&v);
    w.show_all();
    ok.grab_focus();
    CUSTOMIZE.with(|c| *c.borrow_mut() = Some(w));
}
