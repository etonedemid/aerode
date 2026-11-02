//! com.canonical.dbusmenu client: the context menus that notification-area items export,
//! shown as ordinary (Win7-styled) GTK menus.
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::collections::HashMap;
use zbus::zvariant::{OwnedValue, Value};

use super::sni::{conn, debug, str_of, unvariant};

const IFACE: &str = "com.canonical.dbusmenu";

struct Node {
    id: i32,
    props: HashMap<String, OwnedValue>,
    children: Vec<Node>,
}

impl Node {
    fn s(&self, k: &str) -> Option<String> { self.props.get(k).and_then(|v| str_of(v)) }
    fn b(&self, k: &str, def: bool) -> bool {
        match self.props.get(k).map(|v| unvariant(v)) { Some(Value::Bool(b)) => *b, _ => def }
    }
    fn i(&self, k: &str) -> Option<i32> {
        match self.props.get(k).map(|v| unvariant(v)) { Some(Value::I32(i)) => Some(*i), _ => None }
    }
    fn bytes(&self, k: &str) -> Option<Vec<u8>> {
        match self.props.get(k).map(|v| unvariant(v)) {
            Some(Value::Array(a)) => Some(a.inner().iter().filter_map(|b| if let Value::U8(b) = b { Some(*b) } else { None }).collect()),
            _ => None,
        }
    }
}

fn node(v: &Value) -> Option<Node> {
    let Value::Structure(s) = unvariant(v) else { return None };
    let f = s.fields();
    let Value::I32(id) = unvariant(f.first()?) else { return None };
    let mut props = HashMap::new();
    if let Some(Value::Dict(d)) = f.get(1).map(unvariant) {
        for (k, v) in d.iter() {
            if let (Value::Str(k), Ok(v)) = (unvariant(k), unvariant(v).try_to_owned()) { props.insert(k.to_string(), v); }
        }
    }
    let children = match f.get(2).map(unvariant) {
        Some(Value::Array(a)) => a.inner().iter().filter_map(node).collect(),
        _ => vec![],
    };
    Some(Node { id: *id, props, children })
}

async fn layout(conn: &zbus::Connection, dest: &str, path: &str, parent: i32) -> Option<Node> {
    let m = conn.call_method(Some(dest), path, Some(IFACE), "GetLayout", &(parent, -1i32, Vec::<String>::new())).await
        .map_err(|e| debug(&format!("GetLayout: {e}"))).ok()?;
    let (_rev, (id, props, children)): (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>)) =
        m.body().deserialize().map_err(|e| debug(&format!("GetLayout reply: {e}"))).ok()?;
    Some(Node { id, props, children: children.iter().filter_map(|c| node(c)).collect() })
}

async fn about_to_show(conn: &zbus::Connection, dest: &str, path: &str, id: i32) -> bool {
    match conn.call_method(Some(dest), path, Some(IFACE), "AboutToShow", &(id,)).await {
        Ok(m) => m.body().deserialize::<bool>().unwrap_or(false),
        Err(_) => false,
    }
}

fn event(dest: &str, path: &str, id: i32, what: &'static str) {
    let Some(conn) = conn() else { return };
    let (dest, path) = (dest.to_string(), path.to_string());
    let time = gtk::current_event_time();
    glib::spawn_future_local(async move {
        let _ = conn.call_method(Some(dest.as_str()), path.as_str(), Some(IFACE), "Event", &(id, what, Value::from(0i32), time)).await;
    });
}

/// Fetch the menu at `path` on `dest` and pop it up at screen position (x, y), where the
/// icon was clicked (above it when there is no room below, as on the taskbar).
pub fn show(dest: String, path: String, x: i32, y: i32) {
    let Some(conn) = conn() else { return };
    glib::spawn_future_local(async move {
        about_to_show(&conn, &dest, &path, 0).await;
        let Some(root) = layout(&conn, &dest, &path, 0).await else { debug(&format!("no menu layout from {dest}{path}")); return };
        if !root.children.iter().any(|c| c.b("visible", true)) { return; }
        let m = gtk::Menu::new();
        fill(&m, &dest, &path, &root.children);
        event(&dest, &path, 0, "opened");
        { let (d, p) = (dest.clone(), path.clone()); m.connect_deactivate(move |_| event(&d, &p, 0, "closed")); }
        m.show_all();
        let Some(rootwin) = gdk::Screen::default().and_then(|s| s.root_window()) else { return };
        m.popup_at_rect(&rootwin, &gdk::Rectangle::new(x, y, 1, 1), gdk::Gravity::NorthWest, gdk::Gravity::NorthWest, None);
    });
}

fn fill(m: &gtk::Menu, dest: &str, path: &str, nodes: &[Node]) {
    for c in m.children() { m.remove(&c); }
    for n in nodes {
        if !n.b("visible", true) { continue; }
        if n.s("type").as_deref() == Some("separator") {
            m.append(&gtk::SeparatorMenuItem::new());
            continue;
        }
        let label = n.s("label").unwrap_or_default();
        let toggle = n.s("toggle-type").unwrap_or_default();
        let item: gtk::MenuItem = if toggle == "checkmark" || toggle == "radio" {
            let c = gtk::CheckMenuItem::with_mnemonic(&label);
            c.set_draw_as_radio(toggle == "radio");
            match n.i("toggle-state").unwrap_or(0) {
                0 => c.set_active(false),
                1 => c.set_active(true),
                _ => c.set_inconsistent(true),
            }
            c.upcast()
        } else {
            gtk::MenuItem::with_mnemonic(&label)
        };
        item.set_sensitive(n.b("enabled", true));
        if toggle.is_empty() {
            let icon = n.s("icon-name").filter(|s| !s.is_empty()).and_then(|name| crate::util::pixbuf(&name, 16))
                .or_else(|| n.bytes("icon-data").and_then(|d| png(&d)));
            if let Some(pb) = icon { gutter_icon(&item, pb); }
        }
        if let Some(acc) = shortcut(n) {
            if let Some(l) = item.child().and_then(|c| c.downcast::<gtk::AccelLabel>().ok()) { l.set_accel(acc.0, acc.1); }
        }
        if n.s("children-display").as_deref() == Some("submenu") || !n.children.is_empty() {
            let sub = gtk::Menu::new();
            fill(&sub, dest, path, &n.children);
            let (d, p, id) = (dest.to_string(), path.to_string(), n.id);
            // apps may fill submenus lazily: ask, and reload this branch when told to
            sub.connect_show(move |sub| {
                event(&d, &p, id, "opened");
                let (d, p, sub) = (d.clone(), p.clone(), sub.clone());
                glib::spawn_future_local(async move {
                    let Some(conn) = conn() else { return };
                    if about_to_show(&conn, &d, &p, id).await {
                        if let Some(n) = layout(&conn, &d, &p, id).await { fill(&sub, &d, &p, &n.children); sub.show_all(); }
                    }
                });
            });
            { let (d, p) = (dest.to_string(), path.to_string()); sub.connect_hide(move |_| event(&d, &p, id, "closed")); }
            item.set_submenu(Some(&sub));
        } else {
            let (d, p, id) = (dest.to_string(), path.to_string(), n.id);
            item.connect_activate(move |_| event(&d, &p, id, "clicked"));
        }
        m.append(&item);
    }
    m.show_all();
}

fn png(data: &[u8]) -> Option<Pixbuf> {
    let s = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(data));
    Pixbuf::from_stream_at_scale(&s, 16, 16, true, None::<&gio::Cancellable>).ok()
}

/// 16px icon in the menu's gutter, where Win7 menus draw it.
fn gutter_icon(item: &gtk::MenuItem, pb: Pixbuf) {
    item.connect_local("draw", true, move |args| {
        let w = args[0].get::<gtk::Widget>().ok()?;
        let cr = args[1].get::<gtk::cairo::Context>().ok()?;
        let y = (w.allocated_height() - pb.height()) as f64 / 2.0;
        cr.set_source_pixbuf(&pb, 3.0, y.floor());
        if w.is_sensitive() { cr.paint().ok(); } else { cr.paint_with_alpha(0.45).ok(); }
        Some(false.to_value())
    });
}

/// `shortcut` (`aas`, e.g. [["Control", "q"]]) → accelerator for the label.
fn shortcut(n: &Node) -> Option<(u32, gdk::ModifierType)> {
    let Value::Array(a) = unvariant(n.props.get("shortcut")?) else { return None };
    let Value::Array(keys) = unvariant(a.inner().first()?) else { return None };
    let mut mods = gdk::ModifierType::empty();
    let mut key = 0u32;
    for k in keys.inner() {
        let Some(k) = str_of(k) else { continue };
        match k.as_str() {
            "Control" => mods |= gdk::ModifierType::CONTROL_MASK,
            "Alt" => mods |= gdk::ModifierType::MOD1_MASK,
            "Shift" => mods |= gdk::ModifierType::SHIFT_MASK,
            "Super" => mods |= gdk::ModifierType::SUPER_MASK,
            other => key = *gdk::keys::Key::from_name(other),
        }
    }
    (key != 0).then_some((key, mods))
}
