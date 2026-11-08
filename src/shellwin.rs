//! Shell surfaces (taskbar, Start, flyouts, balloons...) are layer-shell surfaces: Wayland has no
//! absolute window positions, so `place` pins them to the monitor's top-left with margins, and we
//! remember where they are because a surface can't ask.
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

pub use gtk_layer_shell::Layer as ShellLayer;

/// Make `w` a shell surface. Call before it is realized. `ns` starting with "aerode-glass" gets a
/// blurred backdrop from the compositor (`-r<radius>`, `-t<top inset>`).
pub fn init(w: &impl IsA<gtk::Window>, layer: Layer, ns: &str, keyboard: KeyboardMode) {
    w.init_layer_shell();
    w.set_layer(layer);
    w.set_namespace(ns);
    w.set_keyboard_mode(keyboard);
    w.set_anchor(Edge::Top, true);
    w.set_anchor(Edge::Left, true);
}

/// Put a shell surface at screen coordinates.
pub fn place(w: &impl IsA<gtk::Window>, x: i32, y: i32) {
    let win = w.as_ref();
    if let Some(m) = gtk::gdk::Display::default().and_then(|d| d.monitor_at_point(x, y)) {
        let g = m.geometry();
        LayerShell::set_monitor(win, &m);
        win.set_layer_shell_margin(Edge::Left, x - g.x());
        win.set_layer_shell_margin(Edge::Top, y - g.y());
    }
    // SAFETY: only ever read back as (i32, i32) under this key
    unsafe { win.set_data("aerode-pos", (x, y)); }
}

/// Dock along the bottom of the monitor at (x, y) and reserve `h` pixels there (the taskbar).
pub fn dock_bottom(w: &impl IsA<gtk::Window>, x: i32, y: i32, h: i32) {
    let win = w.as_ref();
    if let Some(m) = gtk::gdk::Display::default().and_then(|d| d.monitor_at_point(x, y)) { LayerShell::set_monitor(win, &m); }
    win.set_anchor(Edge::Top, false);
    win.set_anchor(Edge::Bottom, true);
    win.set_anchor(Edge::Left, true);
    win.set_anchor(Edge::Right, true);
    win.set_exclusive_zone(h);
    // SAFETY: only ever read back as (i32, i32) under this key
    unsafe { win.set_data("aerode-pos", (x, y)); }
}

/// Where `place` put it; (0, 0) for a window the compositor placed.
pub fn pos(w: &impl IsA<gtk::Window>) -> (i32, i32) {
    // SAFETY: only ever stored as (i32, i32) under this key
    unsafe { w.as_ref().data::<(i32, i32)>("aerode-pos").map(|p| *p.as_ref()).unwrap_or((0, 0)) }
}

/// Screen position of `gw`'s widget window: GDK's origin is relative to the surface on Wayland.
pub fn gdk_origin(widget: &impl IsA<gtk::Widget>) -> (i32, i32) {
    let w = widget.as_ref();
    let (ox, oy) = w.window().map(|gw| { let (_, x, y) = gw.origin(); (x, y) }).unwrap_or((0, 0));
    let (px, py) = w.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()).map(|t| pos(&t)).unwrap_or((0, 0));
    (ox + px, oy + py)
}

/// Screen position of a widget's top-left corner.
pub fn widget_pos(widget: &impl IsA<gtk::Widget>) -> (i32, i32) {
    let w = widget.as_ref();
    let Some(top) = w.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()) else { return (0, 0) };
    let (x, y) = w.translate_coordinates(&top, 0, 0).unwrap_or((0, 0));
    let (px, py) = pos(&top);
    (x + px, y + py)
}

/// The virtual screen: GDK's root window has no size on Wayland.
pub fn screen_size() -> (i32, i32) {
    let Some(d) = gtk::gdk::Display::default() else { return (1, 1) };
    (0..d.n_monitors()).filter_map(|i| d.monitor(i)).map(|m| m.geometry()).fold((1, 1), |a, g| (a.0.max(g.x() + g.width()), a.1.max(g.y() + g.height())))
}

/// The pointer on the screen.
pub fn pointer() -> (i32, i32) {
    crate::wmipc::call(&crate::wmipc::Req::Pointer).map(|v| (v["x"].as_i64().unwrap_or(0) as i32, v["y"].as_i64().unwrap_or(0) as i32)).unwrap_or((0, 0))
}

/// A window with no client area that gives menus somewhere to hang from at a screen position
/// (the window menu of a title bar, a tray icon's menu): destroyed when the menu closes.
pub fn popup_menu_at(m: &gtk::Menu, x: i32, y: i32) {
    let host = gtk::Window::new(gtk::WindowType::Toplevel);
    init(&host, Layer::Overlay, "aerode-menu-host", KeyboardMode::OnDemand);
    host.set_app_paintable(true);
    if let Some(v) = gtk::prelude::WidgetExt::screen(&host).and_then(|s| s.rgba_visual()) { host.set_visual(Some(&v)); }
    host.set_size_request(1, 1);
    place(&host, x, y);
    host.show();
    let h = host.clone();
    m.connect_deactivate(move |_| {
        let h = h.clone();
        // the item's activate runs after deactivate
        gtk::glib::idle_add_local_once(move || unsafe { h.destroy(); });
    });
    m.set_attach_widget(Some(&host));
    m.set_anchor_hints(gtk::gdk::AnchorHints::FLIP | gtk::gdk::AnchorHints::SLIDE);
    let (m2, h2) = (m.clone(), host.clone());
    // GTK needs the surface configured before a popup can hang from it
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
        m2.popup_at_widget(&h2, gtk::gdk::Gravity::NorthWest, gtk::gdk::Gravity::NorthWest, None);
    });
}
