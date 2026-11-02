//! StatusNotifierItem ("AppIndicator") support: Discord, Steam, Telegram, nm-applet, Qt and
//! Electron apps put their notification-area icons on D-Bus this way.
//!
//! The shell runs `org.kde.StatusNotifierWatcher` when nobody else does, registers itself as a
//! host, reads the items' properties and forwards clicks, scrolling and menus to them.
//! All D-Bus work is zbus futures spawned on the GTK main context: the socket I/O happens on
//! zbus' own executor thread, the watcher's method calls arrive there and are handed over through
//! an async-channel, so the UI thread never blocks on the bus.
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use zbus::export::futures_util::StreamExt;
use zbus::object_server::SignalContext;
use zbus::zvariant::{OwnedValue, Value};

use super::{Backend, Status};
use crate::util;

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const ITEM_FDO: &str = "org.freedesktop.StatusNotifierItem";
const DBUS: &str = "org.freedesktop.DBus";

thread_local! {
    static CONN: RefCell<Option<zbus::Connection>> = const { RefCell::new(None) };
    static ITEMS: RefCell<HashMap<String, Rc<Item>>> = RefCell::new(HashMap::new());
    /// the watcher's item list when we are the watcher
    static OWN_LIST: RefCell<Option<Arc<Mutex<Vec<String>>>>> = const { RefCell::new(None) };
}

/// The session bus connection, once it is up.
pub fn conn() -> Option<zbus::Connection> {
    CONN.with(|c| c.borrow().clone())
}

/// One registered item.
pub struct Item {
    /// "service/path", the form the watcher lists it in
    id: String,
    bus: String,
    path: String,
    /// unique bus name of the process behind it (signals come from this name)
    owner: RefCell<String>,
    /// interface name the item answers to (a few use the freedesktop name)
    iface: Cell<&'static str>,
    menu: RefCell<Option<String>>,
    is_menu: Cell<bool>,
    exe: RefCell<String>,
    refreshing: Cell<bool>,
    again: Cell<bool>,
    known: Cell<bool>,
}

// ------------------------------------------------------------------ the watcher
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
    tx: async_channel::Sender<String>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
    ) {
        let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
        // libappindicator registers its object path, everybody else a bus name
        let id = if service.starts_with('/') { format!("{sender}{service}") } else { format!("{service}/StatusNotifierItem") };
        {
            let mut l = self.items.lock().unwrap();
            if l.contains(&id) { return; }
            l.push(id.clone());
        }
        let _ = Self::status_notifier_item_registered(&ctxt, &id).await;
        let _ = self.tx.send(id).await;
    }

    async fn register_status_notifier_host(&self, _service: &str, #[zbus(signal_context)] ctxt: SignalContext<'_>) {
        let _ = Self::status_notifier_host_registered(&ctxt).await;
    }

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.items.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(ctxt: &SignalContext<'_>, service: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn status_notifier_item_unregistered(ctxt: &SignalContext<'_>, service: &str) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn status_notifier_host_registered(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn status_notifier_host_unregistered(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

/// Start the watcher/host. Items appear in the notification area as they register.
pub fn start() {
    glib::spawn_future_local(async {
        if let Err(e) = run().await {
            eprintln!("aerode: notification area: StatusNotifierItem support unavailable: {e}");
        }
    });
}

async fn signals(conn: &zbus::Connection, sender: Option<&str>, iface: &str, member: Option<&str>) -> zbus::Result<zbus::MessageStream> {
    let mut b = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).interface(iface.to_string())?;
    if let Some(s) = sender { b = b.sender(s.to_string())?; }
    if let Some(m) = member { b = b.member(m.to_string())?; }
    zbus::MessageStream::for_match_rule(b.build(), conn, Some(512)).await
}

/// Own org.kde.StatusNotifierWatcher if nobody does.
async fn become_watcher(conn: &zbus::Connection, tx: &async_channel::Sender<String>) -> bool {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    let list: Arc<Mutex<Vec<String>>> = Default::default();
    if !conn.object_server().at(WATCHER_PATH, Watcher { items: list.clone(), tx: tx.clone() }).await.unwrap_or(false) {
        return false;
    }
    match conn.request_name_with_flags(WATCHER, RequestNameFlags::DoNotQueue.into()).await {
        Ok(RequestNameReply::PrimaryOwner) | Ok(RequestNameReply::AlreadyOwner) => {
            OWN_LIST.with(|l| *l.borrow_mut() = Some(list));
            true
        }
        _ => {
            let _ = conn.object_server().remove::<Watcher, _>(WATCHER_PATH).await;
            false
        }
    }
}

async fn run() -> zbus::Result<()> {
    let conn = zbus::Connection::session().await?;
    CONN.with(|c| *c.borrow_mut() = Some(conn.clone()));
    let me = conn.unique_name().map(|n| n.to_string()).unwrap_or_default();
    // subscribe before anything can happen
    let mut owners = signals(&conn, Some(DBUS), DBUS, Some("NameOwnerChanged")).await?;
    let item_sigs = signals(&conn, None, ITEM, None).await?;
    let fdo_sigs = signals(&conn, None, ITEM_FDO, None).await?;
    let mut watcher_sigs = signals(&conn, None, WATCHER, None).await?;
    let (tx, rx) = async_channel::unbounded::<String>();
    let own = Rc::new(Cell::new(become_watcher(&conn, &tx).await));
    let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
    let _ = conn.request_name(host.as_str()).await;
    if !own.get() {
        adopt_external(&conn, &host).await;
    }
    // registrations made with our watcher
    glib::spawn_future_local(async move {
        while let Ok(id) = rx.recv().await { add(&id); }
    });
    // New* signals of the items
    for mut s in [item_sigs, fdo_sigs] {
        glib::spawn_future_local(async move {
            while let Some(Ok(m)) = s.next().await {
                let h = m.header();
                let (Some(sender), Some(path)) = (h.sender(), h.path()) else { continue };
                let (sender, path) = (sender.to_string(), path.to_string());
                let found: Vec<Rc<Item>> = ITEMS.with(|i| i.borrow().values().filter(|it| *it.owner.borrow() == sender && it.path == path).cloned().collect());
                for it in found { refresh(it); }
            }
        });
    }
    // somebody else's watcher, when we are only a host
    glib::spawn_future_local({
        let own = own.clone();
        async move {
            while let Some(Ok(m)) = watcher_sigs.next().await {
                let h = m.header();
                if own.get() || h.sender().map(|s| s.as_str() == me).unwrap_or(true) { continue; }
                let member = h.member().map(|m| m.to_string()).unwrap_or_default();
                let Ok(id) = m.body().deserialize::<String>() else { continue };
                match member.as_str() {
                    "StatusNotifierItemRegistered" => add(&id),
                    "StatusNotifierItemUnregistered" => remove(&id),
                    _ => {}
                }
            }
        }
    });
    // items disappear with their process; the watcher may change hands
    while let Some(Ok(m)) = owners.next().await {
        let Ok((name, _old, new)) = m.body().deserialize::<(String, String, String)>() else { continue };
        let hit: Vec<Rc<Item>> = ITEMS.with(|i| i.borrow().values().filter(|it| it.bus == name || *it.owner.borrow() == name).cloned().collect());
        for it in hit {
            if it.bus == name && !new.is_empty() {
                // the item's name moved to another connection of the program
                debug(&format!("{name} moved to {new}"));
                *it.owner.borrow_mut() = new.clone();
                refresh(it);
            } else if it.bus == name || it.bus.starts_with(':') {
                debug(&format!("{name} left the bus"));
                remove(&it.id);
            } else if new.is_empty() {
                // the connection that owned the item's name is gone: is the name still there?
                let conn = conn.clone();
                glib::spawn_future_local(async move {
                    match bus_call(&conn, "GetNameOwner", &(it.bus.as_str(),)).await.and_then(|m| m.body().deserialize::<String>()) {
                        Ok(o) if !o.is_empty() => { *it.owner.borrow_mut() = o; refresh(it); }
                        _ => { debug(&format!("{} left the bus", it.bus)); remove(&it.id); }
                    }
                });
            }
        }
        if name == WATCHER && new.is_empty() && !own.get() {
            // the other watcher went away: take over (apps re-register when it reappears)
            own.set(become_watcher(&conn, &tx).await);
            if !own.get() { adopt_external(&conn, &host).await; }
        }
    }
    Ok(())
}

/// Register as host with another watcher and pick up what it already lists.
async fn adopt_external(conn: &zbus::Connection, host: &str) {
    let _ = conn.call_method(Some(WATCHER), WATCHER_PATH, Some(WATCHER), "RegisterStatusNotifierHost", &(host,)).await;
    let r = conn.call_method(Some(WATCHER), WATCHER_PATH, Some("org.freedesktop.DBus.Properties"), "Get", &(WATCHER, "RegisteredStatusNotifierItems")).await;
    if let Ok(m) = r {
        if let Ok(v) = m.body().deserialize::<OwnedValue>() {
            if let Value::Array(a) = unvariant(&v) {
                for e in a.inner() {
                    if let Value::Str(s) = unvariant(e) { add(s.as_str()); }
                }
            }
        }
    }
}

fn split_id(id: &str) -> (String, String) {
    match id.find('/') {
        Some(i) => (id[..i].to_string(), id[i..].to_string()),
        None => (id.to_string(), "/StatusNotifierItem".to_string()),
    }
}

async fn bus_call<B>(conn: &zbus::Connection, method: &str, body: &B) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(DBUS), "/org/freedesktop/DBus", Some(DBUS), method, body).await
}

/// Process name behind a bus name (for the per-app setting key).
async fn exe_of(conn: &zbus::Connection, name: &str) -> String {
    let Ok(m) = bus_call(conn, "GetConnectionUnixProcessID", &(name,)).await else { return String::new() };
    let Ok(pid) = m.body().deserialize::<u32>() else { return String::new() };
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty())
        .or_else(|| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok().map(|s| s.trim().to_string()))
        .unwrap_or_default()
}

fn add(id: &str) {
    if ITEMS.with(|i| i.borrow().contains_key(id)) { return; }
    debug(&format!("item registered: {id}"));
    let Some(conn) = conn() else { return };
    let (bus, path) = split_id(id);
    let it = Rc::new(Item {
        id: id.to_string(), bus: bus.clone(), path, owner: RefCell::new(String::new()), iface: Cell::new(ITEM),
        menu: RefCell::new(None), is_menu: Cell::new(false), exe: RefCell::new(String::new()),
        refreshing: Cell::new(false), again: Cell::new(false), known: Cell::new(false),
    });
    ITEMS.with(|i| i.borrow_mut().insert(id.to_string(), it.clone()));
    glib::spawn_future_local(async move {
        let owner = if bus.starts_with(':') {
            bus.clone()
        } else {
            match bus_call(&conn, "GetNameOwner", &(bus.as_str(),)).await.and_then(|m| m.body().deserialize::<String>()) {
                Ok(o) => o,
                Err(e) => { debug(&format!("GetNameOwner({bus}): {e}")); remove(&it.id); return; }
            }
        };
        *it.owner.borrow_mut() = owner.clone();
        *it.exe.borrow_mut() = exe_of(&conn, &owner).await;
        refresh(it);
    });
}

fn remove(id: &str) {
    let Some(it) = ITEMS.with(|i| i.borrow_mut().remove(id)) else { return };
    debug(&format!("item gone: {id}"));
    super::remove(&format!("sni:{}", it.id));
    let own = OWN_LIST.with(|l| l.borrow().clone());
    if let (Some(list), Some(conn)) = (own, conn()) {
        let was = { let mut l = list.lock().unwrap(); let n = l.len(); l.retain(|x| x != id); n != l.len() };
        if was {
            let id = id.to_string();
            glib::spawn_future_local(async move {
                if let Ok(ctxt) = SignalContext::new(&conn, WATCHER_PATH) {
                    let _ = Watcher::status_notifier_item_unregistered(&ctxt, &id).await;
                }
            });
        }
    }
}

/// Re-read the item's properties (coalescing bursts of New* signals).
fn refresh(it: Rc<Item>) {
    if it.refreshing.get() { it.again.set(true); return; }
    it.refreshing.set(true);
    glib::spawn_future_local(async move {
        loop {
            it.again.set(false);
            load(&it).await;
            if !it.again.get() { break; }
        }
        it.refreshing.set(false);
    });
}

async fn get_all(conn: &zbus::Connection, it: &Item, iface: &str) -> Option<HashMap<String, OwnedValue>> {
    let m = conn.call_method(Some(it.bus.as_str()), it.path.as_str(), Some("org.freedesktop.DBus.Properties"), "GetAll", &(iface,)).await.ok()?;
    m.body().deserialize::<HashMap<String, OwnedValue>>().ok().filter(|p| !p.is_empty())
}

async fn load(it: &Rc<Item>) {
    let Some(conn) = conn() else { return };
    let mut props = get_all(&conn, it, it.iface.get()).await;
    if props.is_none() && !it.known.get() {
        props = get_all(&conn, it, ITEM_FDO).await;
        if props.is_some() { it.iface.set(ITEM_FDO); }
    }
    let Some(p) = props else {
        if !it.known.get() { debug(&format!("no properties from {}", it.id)); remove(&it.id); }
        return;
    };
    if ITEMS.with(|i| !i.borrow().contains_key(&it.id)) { return; }
    it.known.set(true);
    let s = |k: &str| p.get(k).and_then(|v| str_of(v)).unwrap_or_default();
    let status = match s("Status").as_str() {
        "Passive" => Status::Passive,
        "NeedsAttention" => Status::Attention,
        _ => Status::Active,
    };
    *it.menu.borrow_mut() = p.get("Menu").and_then(|v| str_of(v)).filter(|m| !m.is_empty() && m != "/");
    it.is_menu.set(matches!(p.get("ItemIsMenu").map(|v| unvariant(v)), Some(Value::Bool(true))));
    let theme_path = s("IconThemePath");
    let pick = |name: &str, pix: &str, size: i32| -> Option<Pixbuf> {
        named_icon(&s(name), &theme_path, size).or_else(|| p.get(pix).and_then(|v| pixmap_icon(v, size)))
    };
    let mut icon = None;
    if status == Status::Attention { icon = pick("AttentionIconName", "AttentionIconPixmap", 16); }
    let mut icon = icon.or_else(|| pick("IconName", "IconPixmap", 16));
    // status overlay in the bottom-right corner (Win7 puts state badges there too)
    if let (Some(base), Some(ov)) = (icon.as_ref(), named_icon(&s("OverlayIconName"), &theme_path, 10).or_else(|| p.get("OverlayIconPixmap").and_then(|v| pixmap_icon(v, 10)))) {
        let b = base.copy().unwrap_or_else(|| base.clone());
        ov.composite(&b, 6, 6, 10, 10, 6.0, 6.0, 1.0, 1.0, InterpType::Bilinear, 255);
        icon = Some(b);
    }
    let (tip_title, tip_text) = p.get("ToolTip").map(|v| tooltip_of(v)).unwrap_or_default();
    let title = [s("Title"), tip_title.clone(), s("Id"), it.exe.borrow().clone()].into_iter().find(|x| !x.trim().is_empty()).unwrap_or_default();
    let mut tooltip = if !tip_title.trim().is_empty() { tip_title } else { title.clone() };
    if !tip_text.trim().is_empty() { tooltip = format!("{tooltip}\n{tip_text}"); }
    let exe = it.exe.borrow().to_lowercase();
    let id_prop = s("Id").to_lowercase();
    let key = if id_prop.is_empty() { exe.clone() } else if exe.is_empty() || exe == id_prop { id_prop } else { format!("{exe}/{id_prop}") };
    super::upsert(super::Item {
        id: format!("sni:{}", it.id),
        key: if key.is_empty() { it.bus.clone() } else { key },
        title,
        tooltip,
        icon: icon.or_else(|| util::pixbuf("application-x-executable", 16)),
        status,
        shown: true,
        backend: Backend::Sni(it.clone()),
    });
}

// ------------------------------------------------------------------ value helpers
pub(super) fn unvariant<'a>(v: &'a Value<'a>) -> &'a Value<'a> {
    match v {
        Value::Value(b) => unvariant(b),
        _ => v,
    }
}

pub(super) fn str_of(v: &Value) -> Option<String> {
    match unvariant(v) {
        Value::Str(s) => Some(s.to_string()),
        Value::ObjectPath(p) => Some(p.to_string()),
        _ => None,
    }
}

fn bytes_of(v: &Value) -> Option<Vec<u8>> {
    match unvariant(v) {
        Value::Array(a) => Some(a.inner().iter().filter_map(|b| if let Value::U8(b) = b { Some(*b) } else { None }).collect()),
        _ => None,
    }
}

/// `a(iiay)`: ARGB32 images in network byte order.
fn pixmaps(v: &Value) -> Vec<(i32, i32, Vec<u8>)> {
    let Value::Array(a) = unvariant(v) else { return vec![] };
    a.inner().iter().filter_map(|e| {
        let Value::Structure(s) = unvariant(e) else { return None };
        let f = s.fields();
        match (f.first().map(unvariant), f.get(1).map(unvariant), f.get(2)) {
            (Some(Value::I32(w)), Some(Value::I32(h)), Some(d)) => Some((*w, *h, bytes_of(d)?)),
            _ => None,
        }
    }).filter(|(w, h, d)| *w > 0 && *h > 0 && d.len() >= (*w * *h * 4) as usize).collect()
}

/// The pixmap closest to `size` (the smallest one at least that big), as a pixbuf scaled to it.
pub(super) fn pixmap_icon(v: &Value, size: i32) -> Option<Pixbuf> {
    let mut list = pixmaps(v);
    if list.is_empty() { return None; }
    list.sort_by_key(|(w, _, _)| *w);
    let (w, h, d) = list.iter().find(|(w, _, _)| *w >= size).unwrap_or_else(|| list.last().unwrap()).clone();
    Some(util::scale(&argb_to_pixbuf(w, h, &d), size))
}

pub(super) fn argb_to_pixbuf(w: i32, h: i32, argb: &[u8]) -> Pixbuf {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for px in argb.chunks_exact(4).take((w * h) as usize) {
        rgba.extend_from_slice(&[px[1], px[2], px[3], px[0]]);
    }
    Pixbuf::from_bytes(&glib::Bytes::from_owned(rgba), Colorspace::Rgb, true, 8, w, h, w * 4)
}

thread_local! {
    static THEMES: RefCell<HashMap<String, gtk::IconTheme>> = RefCell::new(HashMap::new());
}

/// An icon by name: absolute path, the item's own IconThemePath, or the icon theme.
pub(super) fn named_icon(name: &str, theme_path: &str, size: i32) -> Option<Pixbuf> {
    if name.is_empty() { return None; }
    if name.starts_with('/') {
        return Pixbuf::from_file_at_scale(name, size, size, true).ok();
    }
    if !theme_path.is_empty() {
        for ext in ["png", "svg", "xpm"] {
            let f = std::path::Path::new(theme_path).join(format!("{name}.{ext}"));
            if f.exists() {
                if let Ok(p) = Pixbuf::from_file_at_scale(&f, size, size, true) { return Some(p); }
            }
        }
        let theme = THEMES.with(|t| t.borrow_mut().entry(theme_path.to_string()).or_insert_with(|| {
            let th = gtk::IconTheme::new();
            if let Some(s) = gdk::Screen::default() { th.set_screen(&s); }
            th.prepend_search_path(theme_path);
            th
        }).clone());
        if let Ok(Some(p)) = theme.load_icon(name, size, gtk::IconLookupFlags::FORCE_SIZE) { return Some(p); }
    }
    util::pixbuf(name, size)
}

/// `(sa(iiay)ss)` → (title, plain-text description).
fn tooltip_of(v: &Value) -> (String, String) {
    let Value::Structure(s) = unvariant(v) else { return Default::default() };
    let f = s.fields();
    let t = f.get(2).and_then(str_of).unwrap_or_default();
    let d = f.get(3).and_then(str_of).unwrap_or_default();
    (t, strip_markup(&d))
}

/// Tooltips may carry a subset of HTML; Win7 tooltips are plain text.
pub(super) fn strip_markup(s: &str) -> String {
    let s = s.replace("<br/>", "\n").replace("<br>", "\n").replace("<br />", "\n");
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ").replace("&amp;", "&").trim().to_string()
}

// ------------------------------------------------------------------ actions
fn call<B>(it: &Rc<Item>, method: &'static str, body: B, fallback_menu: Option<(i32, i32)>)
where
    B: serde::Serialize + zbus::zvariant::DynamicType + 'static,
{
    let Some(conn) = conn() else { return };
    let it = it.clone();
    glib::spawn_future_local(async move {
        let r = conn.call_method(Some(it.bus.as_str()), it.path.as_str(), Some(it.iface.get()), method, &body).await;
        if let Err(e) = r {
            debug(&format!("{method} on {}: {e}", it.id));
            // items without an Activate method expect their menu on a left click
            if let (Some((x, y)), true) = (fallback_menu, it.menu.borrow().is_some()) { show_menu(&it, x, y); }
        }
    });
}

fn show_menu(it: &Rc<Item>, x: i32, y: i32) {
    if let Some(menu) = it.menu.borrow().clone() {
        super::dbusmenu::show(it.bus.clone(), menu, x, y);
    }
}

pub(super) fn debug(msg: &str) {
    if std::env::var_os("AERODE_TRAY_DEBUG").is_some() { eprintln!("aerode tray: {msg}"); }
}

/// Left click.
pub fn activate(it: &Rc<Item>, x: i32, y: i32) {
    if it.is_menu.get() {
        if it.menu.borrow().is_some() { show_menu(it, x, y); } else { call(it, "ContextMenu", (x, y), None); }
        return;
    }
    call(it, "Activate", (x, y), Some((x, y)));
}

/// Middle click.
pub fn secondary(it: &Rc<Item>, x: i32, y: i32) {
    call(it, "SecondaryActivate", (x, y), None);
}

/// Right click: the exported dbusmenu, or let the item show its own menu.
pub fn context_menu(it: &Rc<Item>, x: i32, y: i32) {
    if it.menu.borrow().is_some() { show_menu(it, x, y); } else { call(it, "ContextMenu", (x, y), None); }
}

/// Mouse wheel: `delta` in notches (positive = up/right).
pub fn scroll(it: &Rc<Item>, delta: i32, horizontal: bool) {
    call(it, "Scroll", (delta * 120, if horizontal { "horizontal" } else { "vertical" }), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids() {
        assert_eq!(split_id("org.kde.StatusNotifierItem-12-1/StatusNotifierItem"), ("org.kde.StatusNotifierItem-12-1".into(), "/StatusNotifierItem".into()));
        assert_eq!(split_id(":1.42/org/ayatana/NotificationItem/steam"), (":1.42".into(), "/org/ayatana/NotificationItem/steam".into()));
        assert_eq!(split_id(":1.7"), (":1.7".into(), "/StatusNotifierItem".into()));
    }

    #[test]
    fn tooltip_markup() {
        assert_eq!(strip_markup("Status: <b>Online</b><br/>3 new &amp; 1 &lt;old&gt;"), "Status: Online\n3 new & 1 <old>");
        assert_eq!(strip_markup("plain"), "plain");
    }

    #[test]
    fn pixmaps_pick_and_convert() {
        // a(iiay): a 1×1 and a 2×2 image, ARGB in network byte order
        let px = |argb: [u8; 4], n: usize| -> Vec<u8> { argb.iter().copied().cycle().take(n * 4).collect() };
        let entry = |w: i32, h: i32, d: Vec<u8>| Value::from(zbus::zvariant::Structure::from((w, h, d)));
        let first = entry(1, 1, vec![]);
        let mut arr = zbus::zvariant::Array::new(first.value_signature());
        arr.append(entry(1, 1, px([255, 10, 20, 30], 1))).unwrap();
        arr.append(entry(2, 2, px([128, 200, 100, 50], 4))).unwrap();
        let v = Value::from(arr);
        assert_eq!(pixmaps(&v).len(), 2);
        // the smallest image at least as big as asked for, converted to RGBA
        let pb = pixmap_icon(&v, 2).unwrap();
        assert_eq!((pb.width(), pb.height()), (2, 2));
        assert_eq!(&pb.read_pixel_bytes()[..4], &[200, 100, 50, 128]);
    }
}
