//! Notification daemon: `org.freedesktop.Notifications` shown as Windows 7
//! notification-area balloon tips.
//!
//! * The D-Bus side runs on its own thread (zbus blocking API, like aerode-polkit) and
//!   hands every request to the GTK main loop through an async channel; the GTK side
//!   sends the NotificationClosed / ActionInvoked signals back through another one.
//!   The GTK thread never waits on D-Bus.
//! * Win7 behaviour: one balloon at a time, the others queue (and wait while a
//!   full-screen program is in front); a balloon stays 9 s (fixed since Vista) and
//!   does not time out while the pointer is over it; clicking it is the "default"
//!   action; the stem points at a notification-area icon, which is shown temporarily
//!   for programs that have none there – as Win7 does.  Balloon, stem and close button
//!   come from the TOOLTIP class of the user's own aero.msstyles when it was extracted.
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use zbus::zvariant::OwnedValue;

use crate::{config, glass, msstyles, sounds, util};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const SPEC_VERSION: &str = "1.2";

/// What GetCapabilities reports – only what is implemented here.
pub const CAPABILITIES: &[&str] = &["actions", "body", "body-hyperlinks", "body-markup", "icon-static", "sound"];

/// "In Windows Vista and later, notifications are displayed for a fixed duration of 9 seconds."
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(9);
/// After the pointer leaves a balloon it stays at least this long.
const MIN_AFTER_HOVER: Duration = Duration::from_secs(3);
/// Win7 keeps "real-time" notifications queued for 60 s at most (our `transient` hint).
const QUEUE_TTL: Duration = Duration::from_secs(60);
/// Pause between two queued balloons (lets the compositor fade the old one out).
const GAP: Duration = Duration::from_millis(400);

// Balloon metrics at 96 dpi, measured on Windows 7 screenshots.
/// Stem height below the body (its 21 px msstyles frame overlaps the bottom border).
const STEM: i32 = 20;
/// Stem wall → right edge of the body.
const STEM_RIGHT: i32 = 16;
/// Soft drop shadow right and below (only drawn when composited).
const SHADOW: i32 = 5;
/// Text wraps at this width.
const TEXT_W: i32 = 304;

// ------------------------------------------------------------------ data

/// NotificationClosed reasons (spec values).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

/// The `image-data` / `icon_data` hint: raw pixels, signature (iiibiiay).
#[derive(Clone, Debug, PartialEq)]
pub struct RawImage {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub bits: i32,
    pub channels: i32,
    pub data: Vec<u8>,
}

impl RawImage {
    fn from_value(v: OwnedValue) -> Option<RawImage> {
        let (width, height, rowstride, has_alpha, bits, channels, data) =
            <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(v).ok()?;
        Some(RawImage { width, height, rowstride, has_alpha, bits, channels, data })
    }

    /// Validated copy as a pixbuf (8 bit RGB/RGBA only, like every other server).
    pub fn pixbuf(&self) -> Option<Pixbuf> {
        let (w, h, rs, ch) = (self.width, self.height, self.rowstride, self.channels);
        if self.bits != 8 || !(ch == 3 || ch == 4) || self.has_alpha != (ch == 4) || w <= 0 || h <= 0 || w > 4096 || h > 4096 || rs < w * ch {
            return None;
        }
        let need = rs as usize * (h as usize - 1) + (w * ch) as usize;
        if self.data.len() < need {
            return None;
        }
        let mut data = self.data[..need].to_vec();
        data.resize(rs as usize * h as usize, 0);
        Some(Pixbuf::from_bytes(&glib::Bytes::from_owned(data), Colorspace::Rgb, self.has_alpha, 8, w, h, rs))
    }
}

/// One Notify call, hints already decoded.
#[derive(Clone, Debug, Default)]
pub struct Note {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    /// (key, label) pairs
    pub actions: Vec<(String, String)>,
    /// 0 low, 1 normal, 2 critical
    pub urgency: u8,
    pub image: Option<RawImage>,
    pub image_path: Option<String>,
    pub icon_data: Option<RawImage>,
    pub desktop_entry: Option<String>,
    pub category: Option<String>,
    pub transient: bool,
    pub resident: bool,
    pub suppress_sound: bool,
    pub sound_file: Option<String>,
    pub sound_name: Option<String>,
    /// ms; -1 = server default, 0 = never
    pub expire_timeout: i32,
}

fn hint_str(h: &HashMap<String, OwnedValue>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| h.get(*k).and_then(|v| <&str>::try_from(v).ok()).filter(|s| !s.is_empty()))
        .map(str::to_string)
}

/// Integer/boolean hints arrive with whatever type the client felt like (byte, int32, boolean…).
fn hint_int(h: &HashMap<String, OwnedValue>, key: &str) -> Option<i64> {
    let v = h.get(key)?;
    u8::try_from(v).map(i64::from)
        .or_else(|_| bool::try_from(v).map(i64::from))
        .or_else(|_| i32::try_from(v).map(i64::from))
        .or_else(|_| u32::try_from(v).map(i64::from))
        .or_else(|_| i16::try_from(v).map(i64::from))
        .or_else(|_| u16::try_from(v).map(i64::from))
        .or_else(|_| i64::try_from(v))
        .or_else(|_| u64::try_from(v).map(|x| x as i64))
        .ok()
}

impl Note {
    #[allow(clippy::too_many_arguments)]
    pub fn from_dbus(id: u32, app_name: String, app_icon: String, summary: String, body: String,
                     actions: Vec<String>, mut hints: HashMap<String, OwnedValue>, expire_timeout: i32) -> Note {
        let image = ["image-data", "image_data"].iter().find_map(|k| hints.remove(*k).and_then(RawImage::from_value));
        let icon_data = hints.remove("icon_data").and_then(RawImage::from_value);
        let flag = |k: &str| hint_int(&hints, k).unwrap_or(0) != 0;
        Note {
            id,
            app_name,
            app_icon,
            summary,
            body,
            actions: actions.chunks_exact(2).map(|p| (p[0].clone(), p[1].clone())).collect(),
            urgency: hint_int(&hints, "urgency").map(|u| u.clamp(0, 2) as u8).unwrap_or(1),
            image,
            image_path: hint_str(&hints, &["image-path", "image_path"]),
            icon_data,
            desktop_entry: hint_str(&hints, &["desktop-entry"]),
            category: hint_str(&hints, &["category"]),
            transient: flag("transient"),
            resident: flag("resident"),
            suppress_sound: flag("suppress-sound"),
            sound_file: hint_str(&hints, &["sound-file"]),
            sound_name: hint_str(&hints, &["sound-name"]),
            expire_timeout,
        }
    }

    fn timeout(&self) -> Option<Duration> {
        if self.urgency >= 2 {
            return None; // critical notifications wait for the user
        }
        match self.expire_timeout {
            t if t < 0 => Some(DEFAULT_TIMEOUT),
            0 => None,
            t => Some(Duration::from_millis(t as u64)),
        }
    }

    fn has_default(&self) -> bool {
        self.actions.iter().any(|(k, _)| k == "default")
    }
}

/// D-Bus thread → GTK.
enum Msg {
    Notify(Note),
    Close(u32),
}

/// GTK → D-Bus thread (signals to emit).
enum Signal {
    Closed(u32, Reason),
    Action(u32, String),
}

// ------------------------------------------------------------------ markup

fn escape(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// Length of a valid XML entity at the start of `s` ("&amp;", "&#123;", "&#x1F;"…), if any.
fn entity_len(s: &str) -> Option<usize> {
    let end = s.find(';')?;
    let name = &s[1..end];
    let ok = matches!(name, "amp" | "lt" | "gt" | "quot" | "apos")
        || name.strip_prefix("#x").or_else(|| name.strip_prefix("#X"))
            .map(|h| !h.is_empty() && h.len() <= 6 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .or_else(|| name.strip_prefix('#').map(|d| !d.is_empty() && d.len() <= 7 && d.chars().all(|c| c.is_ascii_digit())))
            .unwrap_or(false);
    ok.then_some(end + 1)
}

/// Decode the entities a sanitiser would keep (for attribute values and plain text).
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match entity_len(rest) {
            Some(n) => {
                let name = &rest[1..n - 1];
                let c = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")).and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                        .and_then(char::from_u32),
                };
                if let Some(c) = c { out.push(c); }
                rest = &rest[n..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name).map(|i| i + from) {
        from = i + name.len();
        let before_ok = i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace();
        let after = lower[from..].trim_start();
        if !before_ok || !after.starts_with('=') { continue; }
        let vstart = tag.len() - after.len() + 1;
        let v = tag[vstart..].trim_start();
        let val = match v.chars().next() {
            Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
            _ => v.split(|c: char| c.is_ascii_whitespace() || c == '/').next().unwrap_or(""),
        };
        return Some(unescape(val));
    }
    None
}

/// The body-markup subset of the spec (`b i u a img`) turned into well-formed GtkLabel
/// markup: unknown tags are dropped (their text kept), `<img>` becomes its alt text,
/// stray `<` and `&` are escaped, nesting errors are repaired.
pub fn sanitize_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut open: Vec<&'static str> = vec![];
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        match c {
            '<' => {
                // a tag starts with a name (or '/'); "1 < 2" from a plain-text client is text
                let tagish = rest[1..].chars().next().map(|c| c.is_ascii_alphabetic() || c == '/').unwrap_or(false);
                let Some(end) = rest.find('>').filter(|_| tagish) else {
                    out.push_str("&lt;");
                    rest = &rest[1..];
                    continue;
                };
                let inner = rest[1..end].trim();
                rest = &rest[end + 1..];
                let (closing, body) = match inner.strip_prefix('/') { Some(b) => (true, b.trim_start()), None => (false, inner) };
                let name: String = body.chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
                let tag: Option<&'static str> = match name.as_str() { "b" => Some("b"), "i" => Some("i"), "u" => Some("u"), "a" => Some("a"), _ => None };
                if closing {
                    if let Some(t) = tag {
                        if let Some(pos) = open.iter().rposition(|o| *o == t) {
                            while open.len() > pos {
                                out.push_str(&format!("</{}>", open.pop().unwrap()));
                            }
                        }
                    }
                    continue;
                }
                match (tag, name.as_str()) {
                    (Some("a"), _) => {
                        // links can't nest; a link without a target is plain text
                        match attr(body, "href").filter(|h| !h.is_empty()) {
                            Some(href) if !open.contains(&"a") => {
                                out.push_str(&format!("<a href=\"{}\">", escape(&href)));
                                open.push("a");
                            }
                            _ => {}
                        }
                    }
                    (Some(t), _) => {
                        if !body.trim_end().ends_with('/') {
                            out.push_str(&format!("<{t}>"));
                            open.push(t);
                        }
                    }
                    (None, "img") => {
                        if let Some(alt) = attr(body, "alt") { out.push_str(&escape(&alt)); }
                    }
                    (None, "br") => out.push('\n'),
                    _ => {}
                }
            }
            '&' => match entity_len(rest) {
                Some(n) => {
                    out.push_str(&rest[..n]);
                    rest = &rest[n..];
                }
                None => {
                    out.push_str("&amp;");
                    rest = &rest[1..];
                }
            },
            '>' => {
                out.push_str("&gt;");
                rest = &rest[1..];
            }
            _ => {
                // GMarkup rejects control characters
                if !c.is_control() || c == '\n' || c == '\t' { out.push(c); }
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    while let Some(t) = open.pop() {
        out.push_str(&format!("</{t}>"));
    }
    out
}

/// Win7 draws balloon text with GDI, one line every tmHeight pixels (16 px for Segoe UI
/// 9 pt, 21 px for 12 pt); Pango spaces lines a little wider.  Tighten `markup` to `px`
/// per line with a line-height factor of the label's own font (Pango ≥ 1.50).
fn gdi_lines(label: &gtk::Label, markup: &str, px: f64) -> String {
    if pango::version_check(1, 50, 0).is_some() { return markup.to_string(); }
    let m = label.pango_context().metrics(None, None);
    let h = (m.ascent() + m.descent()) as f64 / pango::SCALE as f64;
    if h <= px || h > px * 1.5 { return markup.to_string(); }
    format!("<span line_height=\"{:.3}\">{markup}</span>", px / h)
}

/// Plain-text version of a body (all tags dropped, entities decoded).
pub fn strip_markup(s: &str) -> String {
    let m = sanitize_markup(s);
    let mut out = String::new();
    let mut in_tag = false;
    for c in m.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out)
}

// ------------------------------------------------------------------ D-Bus side

struct Server {
    tx: async_channel::Sender<Msg>,
    next: Arc<AtomicU32>,
}

impl Server {
    fn alloc(&self) -> u32 {
        loop {
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            if id != 0 { return id; }
        }
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[zbus(out_args("capabilities"))]
    fn get_capabilities(&self) -> Vec<String> {
        CAPABILITIES.iter().map(|s| s.to_string()).collect()
    }

    #[allow(clippy::too_many_arguments)]
    #[zbus(out_args("id"))]
    async fn notify(&self, app_name: String, replaces_id: u32, app_icon: String, summary: String, body: String,
                    actions: Vec<String>, hints: HashMap<String, OwnedValue>, expire_timeout: i32) -> u32 {
        let id = if replaces_id != 0 { replaces_id } else { self.alloc() };
        let note = Note::from_dbus(id, app_name, app_icon, summary, body, actions, hints, expire_timeout);
        let _ = self.tx.send(Msg::Notify(note)).await;
        id
    }

    async fn close_notification(&self, id: u32) {
        let _ = self.tx.send(Msg::Close(id)).await;
    }

    #[zbus(out_args("name", "vendor", "version", "spec_version"))]
    fn get_server_information(&self) -> (String, String, String, String) {
        ("aerode".into(), "aerode".into(), env!("CARGO_PKG_VERSION").into(), SPEC_VERSION.into())
    }

    #[zbus(signal)]
    async fn notification_closed(ctxt: &zbus::SignalContext<'_>, id: u32, reason: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(ctxt: &zbus::SignalContext<'_>, id: u32, action_key: &str) -> zbus::Result<()>;
}

/// Who currently owns the name, for the log ("dunst (pid 1234)").
fn owner_desc(proxy: &zbus::blocking::fdo::DBusProxy) -> String {
    let Ok(name) = zbus::names::BusName::try_from(NAME) else { return "another program".into() };
    let Ok(owner) = proxy.get_name_owner(name) else { return "another program".into() };
    let pid = proxy.get_connection_unix_process_id(owner.clone().into()).ok();
    let comm = pid.and_then(|p| std::fs::read_to_string(format!("/proc/{p}/comm")).ok()).map(|s| s.trim().to_string());
    match (comm, pid) {
        (Some(c), Some(p)) => format!("{c} (pid {p})"),
        _ => owner.to_string(),
    }
}

/// Serve on the session bus until the GTK side goes away.  Err = connection problem.
fn serve(tx: &async_channel::Sender<Msg>, signals: &async_channel::Receiver<Signal>, next: &Arc<AtomicU32>) -> zbus::Result<()> {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    let conn = zbus::blocking::connection::Builder::session()?
        .serve_at(PATH, Server { tx: tx.clone(), next: next.clone() })?
        .build()?;
    // without DoNotQueue the bus keeps us queued while another daemon holds the name and
    // hands it over (NameAcquired) as soon as that one releases it or exits
    let flags = RequestNameFlags::ReplaceExisting | RequestNameFlags::AllowReplacement;
    let reply = conn.request_name_with_flags(NAME, flags)?;
    let proxy = zbus::blocking::fdo::DBusProxy::new(&conn)?;
    match reply {
        RequestNameReply::InQueue | RequestNameReply::Exists => {
            eprintln!("aerode: {NAME} is held by {}; waiting for it to be released", owner_desc(&proxy));
        }
        _ => eprintln!("aerode: notification daemon active ({NAME})"),
    }
    // later ownership changes are only logged
    let (lost, acquired) = (proxy.receive_name_lost()?, proxy.receive_name_acquired()?);
    let ours = |n: &str| n == NAME;
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            for s in lost {
                if s.args().map(|a| ours(a.name.as_str())).unwrap_or(false) {
                    eprintln!("aerode: {} took over {NAME}; waiting for it to be released", owner_desc(&proxy));
                }
            }
        });
    }
    std::thread::spawn(move || {
        for s in acquired {
            if s.args().map(|a| ours(a.name.as_str())).unwrap_or(false) {
                eprintln!("aerode: notification daemon active ({NAME})");
            }
        }
    });
    while let Ok(s) = signals.recv_blocking() {
        let r = match s {
            Signal::Closed(id, reason) => conn.emit_signal(None::<zbus::names::BusName>, PATH, NAME, "NotificationClosed", &(id, reason as u32)),
            Signal::Action(id, key) => conn.emit_signal(None::<zbus::names::BusName>, PATH, NAME, "ActionInvoked", &(id, key.as_str())),
        };
        if let Err(e) = r {
            eprintln!("aerode: notification signal: {e}");
            if matches!(e, zbus::Error::InputOutput(_)) { return Err(e); }
        }
    }
    Ok(())
}

fn dbus_thread(tx: async_channel::Sender<Msg>, signals: async_channel::Receiver<Signal>) {
    let next = Arc::new(AtomicU32::new(1));
    let mut logged = false;
    loop {
        match serve(&tx, &signals, &next) {
            Ok(()) => return,
            Err(e) => {
                if !logged { eprintln!("aerode: notification daemon: {e} (retrying)"); }
                logged = true;
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    }
}

// ------------------------------------------------------------------ icons and sound

fn load_icon(spec: &str, size: i32) -> Option<Pixbuf> {
    let spec = spec.trim();
    if spec.is_empty() { return None; }
    let path = if spec.starts_with("file://") { gio::File::for_uri(spec).path() } else if spec.starts_with('/') { Some(spec.into()) } else { None };
    match path {
        Some(p) => Pixbuf::from_file_at_scale(p, size, size, true).ok(),
        None => util::pixbuf(spec, size),
    }
}

/// Shrink to fit `size` (aspect kept); small images stay as they are.
fn fit(p: &Pixbuf, size: i32) -> Pixbuf {
    let (w, h) = (p.width(), p.height());
    if w <= size && h <= size { return p.clone(); }
    let s = size as f64 / w.max(h) as f64;
    p.scale_simple(((w as f64 * s).round() as i32).max(1), ((h as f64 * s).round() as i32).max(1), InterpType::Hyper).unwrap_or_else(|| p.clone())
}

/// The balloon's picture: (pixbuf, large).  Spec priority: image-data, image-path,
/// app_icon, icon_data.  Without one, the small 16 px info (or warning) icon of NIIF_INFO.
fn balloon_icon(n: &Note) -> Option<(Pixbuf, bool)> {
    n.image.as_ref().and_then(RawImage::pixbuf).map(|p| fit(&p, 32))
        .or_else(|| n.image_path.as_deref().and_then(|s| load_icon(s, 32)))
        .or_else(|| load_icon(&n.app_icon, 32))
        .or_else(|| n.icon_data.as_ref().and_then(RawImage::pixbuf).map(|p| fit(&p, 32)))
        .map(|p| (p, true))
        .or_else(|| util::pixbuf(if n.urgency >= 2 { "dialog-warning" } else { "dialog-information" }, 16).map(|p| (p, false)))
}

fn app_info(n: &Note) -> Option<gio::AppInfo> {
    if let Some(e) = &n.desktop_entry {
        let id = if e.ends_with(".desktop") { e.clone() } else { format!("{e}.desktop") };
        if let Some(a) = util::app_by_id(&id) { return Some(a); }
    }
    let name = n.app_name.trim().to_lowercase();
    if name.is_empty() || name == "notify-send" { return None; }
    util::all_apps().into_iter().find(|a| {
        a.display_name().to_lowercase() == name
            || a.id().map(|i| i.trim_end_matches(".desktop").to_lowercase().rsplit('.').next() == Some(name.as_str())).unwrap_or(false)
            || a.executable().file_name().map(|f| f.to_string_lossy().to_lowercase() == name).unwrap_or(false)
    })
}

/// 16 px icon for the temporary notification-area icon.
fn tray_icon(n: &Note, app: Option<&gio::AppInfo>) -> Option<Pixbuf> {
    app.and_then(|a| a.icon()).and_then(|i| {
        gtk::IconTheme::default()?.lookup_by_gicon(&i, 16, gtk::IconLookupFlags::FORCE_SIZE)?.load_icon().ok()
    }).map(|p| util::scale(&p, 16))
        .or_else(|| load_icon(&n.app_icon, 16))
        .or_else(|| n.image_path.as_deref().and_then(|s| load_icon(s, 16)))
        .or_else(|| n.image.as_ref().or(n.icon_data.as_ref()).and_then(RawImage::pixbuf).map(|p| fit(&p, 16)))
        .or_else(|| util::pixbuf(if n.urgency >= 2 { "dialog-warning" } else { "dialog-information" }, 16))
}

/// freedesktop sound-theme names → the Win7 scheme event that fits.
fn sound_event(name: &str) -> Option<sounds::Event> {
    use sounds::Event::*;
    Some(match name {
        "message-new-email" | "message-new-instant" | "message" => NewMail,
        "device-added" => DeviceConnect,
        "device-removed" => DeviceDisconnect,
        "battery-low" => LowBattery,
        "battery-caution" | "power-unplug-battery-low" => CriticalBattery,
        "dialog-error" => CriticalStop,
        "dialog-warning" => Exclamation,
        "dialog-information" => Asterisk,
        "dialog-question" => Question,
        "bell" | "bell-window-system" => DefaultBeep,
        "trash-empty" => EmptyRecycleBin,
        _ => return None,
    })
}

/// Win7 plays no balloon sound within 5 s of the last one (explorer.exe tray balloon code).
const SOUND_GAP: Duration = Duration::from_secs(5);

/// The balloon's sound: "Windows Balloon.wav" (System Notification) unless the program asked for
/// another one (sound-file / sound-name) or none (suppress-sound, Win7's NIIF_NOSOUND). Like
/// Explorer's own sounds it doesn't cut off a sound already playing in the shell.
fn play_sound(n: &Note, last: &Cell<Option<Instant>>) {
    if n.suppress_sound { return; }
    if last.get().map(|t| t.elapsed() < SOUND_GAP).unwrap_or(false) { return; }
    last.set(Some(Instant::now()));
    if let Some(f) = &n.sound_file {
        let p = if f.starts_with("file://") { gio::File::for_uri(f).path() } else { Some(std::path::PathBuf::from(f)) };
        if let Some(p) = p.filter(|p| p.is_file()) {
            if !sounds::muted() { sounds::play_file_with(&p, sounds::Policy::Yield); }
            return;
        }
    }
    sounds::play_with(n.sound_name.as_deref().and_then(sound_event).unwrap_or(sounds::Event::SystemNotification), sounds::Policy::Yield);
}

// ------------------------------------------------------------------ balloon chrome

/// TOOLTIP parts of aero.msstyles: TTP_BALLOON (3), TTP_CLOSE (5), TTP_BALLOONSTEM (6).
struct Assets {
    body: cairo::ImageSurface,
    margins: [i32; 4], // l r t b
    stem_right: cairo::ImageSurface,
    stem_left: cairo::ImageSurface,
    close: Vec<cairo::ImageSurface>,
}

fn load_assets() -> Option<Assets> {
    let t = glass::theme()?;
    let s = |part: i32, state: i32| t.frame("Tooltip", part, state).map(|i| crate::wm::decor::rgba_surface(&i));
    Some(Assets {
        body: s(3, 0)?,
        margins: t.margins("Tooltip", 3, 0, msstyles::SIZINGMARGINS).unwrap_or([4; 4]),
        // BALLOONSTEM states: 4 = pointing down, right wall; 6 = pointing down, left wall
        stem_right: s(6, 4)?,
        stem_left: s(6, 6)?,
        close: (1..=3).filter_map(|st| s(5, st)).collect(),
    })
}

/// Nine-slice `src` (sizing margins l r t b, stretched) into the rectangle.
fn nine(cr: &cairo::Context, src: &cairo::ImageSurface, m: [i32; 4], x: f64, y: f64, w: f64, h: f64) {
    let (sw, sh) = (src.width() as f64, src.height() as f64);
    let [ml, mr, mt, mb] = m.map(|v| v as f64);
    let cols = [(0.0, ml, x, ml), (ml, sw - ml - mr, x + ml, w - ml - mr), (sw - mr, mr, x + w - mr, mr)];
    let rows = [(0.0, mt, y, mt), (mt, sh - mt - mb, y + mt, h - mt - mb), (sh - mb, mb, y + h - mb, mb)];
    for (sx, cw, dx, dw) in cols {
        for (sy, ch, dy, dh) in rows {
            if cw <= 0.0 || ch <= 0.0 || dw <= 0.0 || dh <= 0.0 { continue; }
            // copy the cell so stretching can't bleed neighbouring pixels in
            let Ok(cell) = cairo::ImageSurface::create(cairo::Format::ARgb32, cw as i32, ch as i32) else { continue };
            if let Ok(c) = cairo::Context::new(&cell) {
                c.set_source_surface(src, -sx, -sy).ok();
                c.set_operator(cairo::Operator::Source);
                c.paint().ok();
            }
            let pat = cairo::SurfacePattern::create(&cell);
            pat.set_extend(cairo::Extend::Pad);
            pat.set_filter(cairo::Filter::Bilinear);
            cr.save().ok();
            cr.translate(dx, dy);
            cr.scale(dw / cw, dh / ch);
            cr.set_source(&pat).ok();
            cr.rectangle(0.0, 0.0, cw, ch);
            cr.fill().ok();
            cr.restore().ok();
        }
    }
}

/// Balloon body (bw × bh at 0,0) with its stem below; `wall` = x of the stem's straight side.
fn paint_balloon(cr: &cairo::Context, a: Option<&Assets>, bw: f64, bh: f64, wall: f64, left: bool) {
    if let Some(a) = a {
        nine(cr, &a.body, a.margins, 0.0, 0.0, bw, bh);
        let (stem, x) = if left { (&a.stem_left, wall) } else { (&a.stem_right, wall - (a.stem_right.width() - 1) as f64) };
        cr.set_source_surface(stem, x, bh - 1.0).ok();
        cr.paint().ok();
        return;
    }
    // no msstyles: the same shape and colours drawn by hand
    let r = 3.0;
    let (l, t, rr, b) = (0.5, 0.5, bw - 0.5, bh - 0.5);
    let pi = std::f64::consts::PI;
    cr.new_path();
    cr.arc(l + r, t + r, r, pi, 1.5 * pi);
    cr.arc(rr - r, t + r, r, 1.5 * pi, 0.0);
    cr.arc(rr - r, b - r, r, 0.0, 0.5 * pi);
    if left {
        cr.line_to(wall + STEM as f64, b);
        cr.line_to(wall + 0.5, b + STEM as f64);
        cr.line_to(wall + 0.5, b);
    } else {
        cr.line_to(wall + 0.5, b);
        cr.line_to(wall + 0.5, b + STEM as f64);
        cr.line_to(wall - STEM as f64, b);
    }
    cr.arc(l + r, b - r, r, 0.5 * pi, pi);
    cr.close_path();
    let g = cairo::LinearGradient::new(0.0, 0.0, 0.0, bh);
    g.add_color_stop_rgb(0.0, 1.0, 1.0, 1.0);
    g.add_color_stop_rgb(1.0, 0.894, 0.898, 0.941);
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgb(0.463, 0.463, 0.463);
    cr.set_line_width(1.0);
    cr.stroke().ok();
}

fn box_blur(a: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let mut buf = vec![0f32; len];
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        for (i, b) in buf.iter_mut().enumerate() {
            let lo = i.saturating_sub(r);
            let hi = (i + r).min(len - 1);
            *b = (lo..=hi).map(|k| a[idx(line, k)]).sum::<f32>() * norm;
        }
        for (i, b) in buf.iter().enumerate() {
            a[idx(line, i)] = *b;
        }
    }
}

/// Soft shadow of `fg`'s opaque area, offset right/down (the Win7 CS_DROPSHADOW look).
fn shadow_of(fg: &cairo::ImageSurface, dx: usize, dy: usize, strength: f32) -> Option<cairo::ImageSurface> {
    let (w, h) = (fg.width() as usize, fg.height() as usize);
    let stride = fg.stride() as usize;
    let mut a = vec![0f32; w * h];
    fg.with_data(|d| {
        for y in dy..h {
            for x in dx..w {
                a[y * w + x] = d[(y - dy) * stride + (x - dx) * 4 + 3] as f32 / 255.0;
            }
        }
    }).ok()?;
    for _ in 0..2 {
        box_blur(&mut a, w, h, 1, true);
        box_blur(&mut a, w, h, 1, false);
    }
    let mut out = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32).ok()?;
    let os = out.stride() as usize;
    {
        let mut d = out.data().ok()?;
        for y in 0..h {
            for x in 0..w {
                d[y * os + x * 4 + 3] = (a[y * w + x] * strength * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Some(out)
}

/// Region of pixels with alpha above `min`.
fn alpha_region(s: &cairo::ImageSurface, min: u8) -> cairo::Region {
    let region = cairo::Region::create();
    let (w, h, stride) = (s.width() as usize, s.height() as usize, s.stride() as usize);
    let _ = s.with_data(|d| {
        for y in 0..h {
            let mut x = 0;
            while x < w {
                if d[y * stride + x * 4 + 3] > min {
                    let x0 = x;
                    while x < w && d[y * stride + x * 4 + 3] > min { x += 1; }
                    let _ = region.union_rectangle(&cairo::RectangleInt::new(x0 as i32, y as i32, (x - x0) as i32, 1));
                } else {
                    x += 1;
                }
            }
        }
    });
    region
}

// ------------------------------------------------------------------ GTK side

struct Shown {
    note: Note,
    temp: Option<gtk::Widget>,
    timer: Option<glib::SourceId>,
    deadline: Option<Instant>,
    /// time left while the pointer rests on the balloon
    paused: Option<Duration>,
}

struct Rendered {
    size: (i32, i32),
    anchor: (i32, i32),
    composited: bool,
    surf: cairo::ImageSurface,
    /// where the balloon takes the pointer (not the shadow or the empty corner by the stem)
    input: cairo::Region,
}

struct Daemon {
    tray: gtk::Box,
    wm: Rc<crate::x11::Wm>,
    win: gtk::Window,
    body: gtk::Box,
    close_state: Rc<Cell<usize>>,
    assets: Option<Assets>,
    bg: RefCell<Option<Rendered>>,
    /// (stem tip x, taskbar top y) on screen
    anchor: Cell<(i32, i32)>,
    queue: RefCell<VecDeque<(Note, Instant)>>,
    cur: RefCell<Option<Shown>>,
    /// a balloon is being prepared or we are between two balloons
    busy: Cell<bool>,
    pressed: Cell<bool>,
    out: async_channel::Sender<Signal>,
    fullscreen_atom: Cell<u32>,
    /// when the last balloon sound played
    last_sound: Cell<Option<Instant>>,
}

/// Start serving org.freedesktop.Notifications; balloons point into `tray`.
pub fn start(tray: &gtk::Box, wm: Rc<crate::x11::Wm>) {
    let (tx, rx) = async_channel::unbounded::<Msg>();
    let (stx, srx) = async_channel::unbounded::<Signal>();
    if let Err(e) = std::thread::Builder::new().name("aerode-notify".into()).spawn(move || dbus_thread(tx, srx)) {
        eprintln!("aerode: notification daemon: {e}");
        return;
    }
    let d = Daemon::new(tray.clone(), wm, stx);
    glib::MainContext::default().spawn_local(async move {
        while let Ok(m) = rx.recv().await {
            match m {
                Msg::Notify(n) => d.notify(n),
                Msg::Close(id) => d.close_by_call(id),
            }
        }
    });
}

impl Daemon {
    fn new(tray: gtk::Box, wm: Rc<crate::x11::Wm>, out: async_channel::Sender<Signal>) -> Rc<Daemon> {
        let win = gtk::Window::new(gtk::WindowType::Popup);
        win.set_title("Notification");
        win.set_type_hint(gdk::WindowTypeHint::Notification);
        win.set_app_paintable(true);
        win.set_resizable(false);
        glass::enable_rgba(&win);
        // the balloon fades in and out itself (crate::anim), not the compositor
        crate::anim::self_animated(&win);
        win.style_context().add_class("aerode");
        win.style_context().add_class("balloon");
        win.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
        {
            // the balloon draws its own (Win7-sized) shadow: say so like GTK's client-side
            // decorations do, which compositors honour (picom: shadow-exclude _GTK_FRAME_EXTENTS)
            let wm = wm.clone();
            win.connect_realize(move |w| {
                use x11rb::connection::Connection;
                use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
                use x11rb::wrapper::ConnectionExt as _;
                let Some(xid) = w.window().and_then(|g| g.downcast::<gdkx11::X11Window>().ok()).map(|x| x.xid() as u32) else { return };
                let Some(atom) = wm.conn.intern_atom(false, b"_GTK_FRAME_EXTENTS").ok().and_then(|c| c.reply().ok()).map(|r| r.atom) else { return };
                let _ = wm.conn.change_property32(PropMode::REPLACE, xid, atom, AtomEnum::CARDINAL, &[0, SHADOW as u32, 0, SHADOW as u32]);
                let _ = wm.conn.flush();
            });
        }
        // [icon | title, body, links | close] inside the body, the stem and shadow below/right
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_margin_start(11);
        row.set_margin_top(8);
        row.set_margin_end(8 + SHADOW);
        row.set_margin_bottom(12 + STEM + SHADOW);
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.pack_start(&body, true, true, 0);
        let close = gtk::EventBox::new();
        close.set_visible_window(false);
        close.set_size_request(17, 16);
        close.set_valign(gtk::Align::Start);
        close.set_margin_start(10);
        close.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
        row.pack_end(&close, false, false, 0);
        win.add(&row);
        row.show_all();

        let d = Rc::new(Daemon {
            tray, wm, win: win.clone(), body, close_state: Rc::new(Cell::new(0)), assets: load_assets(),
            bg: RefCell::new(None), anchor: Cell::new((0, 0)), queue: RefCell::new(VecDeque::new()),
            cur: RefCell::new(None), busy: Cell::new(false), pressed: Cell::new(false), out, fullscreen_atom: Cell::new(0),
            last_sound: Cell::new(None),
        });
        let weak = Rc::downgrade(&d);

        // close button (TTP_CLOSE: normal, hot, pressed)
        {
            let w = weak.clone();
            close.connect_draw(move |_, cr| {
                let Some(d) = w.upgrade() else { return glib::Propagation::Proceed };
                let st = d.close_state.get();
                match d.assets.as_ref().and_then(|a| a.close.get(st).or(a.close.first())) {
                    Some(s) => {
                        cr.set_source_surface(s, 0.0, 0.0).ok();
                        cr.paint().ok();
                    }
                    None => {
                        let c = if st == 0 { 0.59 } else { 0.75 };
                        cr.set_source_rgb(c, if st == 0 { c } else { 0.2 }, if st == 0 { c } else { 0.2 });
                        cr.set_line_width(2.0);
                        cr.move_to(4.5, 4.5);
                        cr.line_to(12.0, 11.5);
                        cr.move_to(12.0, 4.5);
                        cr.line_to(4.5, 11.5);
                        cr.stroke().ok();
                    }
                }
                glib::Propagation::Stop
            });
        }
        for (ev, st) in [(0, 1usize), (1, 0)] {
            let (s, c) = (d.close_state.clone(), close.clone());
            let f = move || { s.set(st); c.queue_draw(); };
            if ev == 0 { close.connect_enter_notify_event(move |_, _| { f(); glib::Propagation::Proceed }); }
            else { close.connect_leave_notify_event(move |_, _| { f(); glib::Propagation::Proceed }); }
        }
        {
            let s = d.close_state.clone();
            close.connect_button_press_event(move |c, e| {
                if e.button() == 1 { s.set(2); c.queue_draw(); }
                glib::Propagation::Stop
            });
        }
        {
            let w = weak.clone();
            close.connect_button_release_event(move |c, e| {
                let Some(d) = w.upgrade() else { return glib::Propagation::Stop };
                let a = c.allocation();
                let (x, y) = e.position();
                let inside = x >= 0.0 && y >= 0.0 && x < a.width() as f64 && y < a.height() as f64;
                d.close_state.set(if inside { 1 } else { 0 });
                c.queue_draw();
                if e.button() == 1 && inside { d.finish(Reason::Dismissed); }
                glib::Propagation::Stop
            });
        }

        // the balloon itself
        {
            let w = weak.clone();
            win.connect_draw(move |_, cr| {
                cr.save().ok();
                cr.set_operator(cairo::Operator::Source);
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
                cr.paint().ok();
                cr.restore().ok();
                if let Some(d) = w.upgrade() {
                    if let Some(r) = d.bg.borrow().as_ref() {
                        cr.set_source_surface(&r.surf, 0.0, 0.0).ok();
                        cr.paint().ok();
                    }
                }
                glib::Propagation::Proceed
            });
        }
        {
            let w = weak.clone();
            win.connect_size_allocate(move |_, a| {
                if let Some(d) = w.upgrade() { d.render(a.width(), a.height()); }
            });
        }
        {
            let w = weak.clone();
            win.connect_enter_notify_event(move |_, _| {
                if let Some(d) = w.upgrade() { d.pause(); }
                glib::Propagation::Proceed
            });
        }
        {
            let w = weak.clone();
            win.connect_leave_notify_event(move |_, e| {
                if e.detail() != gdk::NotifyType::Inferior {
                    if let Some(d) = w.upgrade() { d.resume(); }
                }
                glib::Propagation::Proceed
            });
        }
        {
            let w = weak.clone();
            win.connect_button_press_event(move |_, _| {
                if let Some(d) = w.upgrade() { d.pressed.set(true); }
                glib::Propagation::Stop
            });
        }
        {
            let w = weak.clone();
            win.connect_button_release_event(move |_, e| {
                let Some(d) = w.upgrade() else { return glib::Propagation::Stop };
                if d.pressed.replace(false) { d.clicked(e.button()); }
                glib::Propagation::Stop
            });
        }
        d
    }

    // ---------------------------------------------------------- queue

    fn notify(self: &Rc<Self>, n: Note) {
        let id = n.id;
        let is_current = self.cur.borrow().as_ref().map(|s| s.note.id == id).unwrap_or(false);
        if is_current {
            // replaces_id of the balloon on screen: update it in place (no new sound)
            self.fill(&n);
            if let Some(s) = self.cur.borrow_mut().as_mut() {
                if let Some(t) = s.temp.as_ref().and_then(|t| t.clone().downcast::<gtk::Button>().ok()) {
                    if let Some(img) = t.child().and_then(|c| c.downcast::<gtk::Image>().ok()) {
                        img.set_from_pixbuf(tray_icon(&n, app_info(&n).as_ref()).as_ref());
                    }
                }
                s.note = n;
            }
            self.relayout();
            self.arm_timer();
            return;
        }
        {
            let mut q = self.queue.borrow_mut();
            if let Some(slot) = q.iter_mut().find(|(q, _)| q.id == id) {
                slot.0 = n;
                return;
            }
            q.push_back((n, Instant::now()));
        }
        // a balloon that waits for the user (critical, or no timeout) gives way once others queue
        // up behind it: since Vista every balloon times out, so one can't hold back all the rest
        let waits = self.cur.borrow().as_ref().map(|s| s.note.timeout().is_none() && s.timer.is_none() && s.paused.is_none()).unwrap_or(false);
        if waits && !self.busy.get() { self.arm_timer(); }
        self.next();
    }

    fn close_by_call(self: &Rc<Self>, id: u32) {
        if self.cur.borrow().as_ref().map(|s| s.note.id == id).unwrap_or(false) {
            self.finish(Reason::Closed);
            return;
        }
        let removed = {
            let mut q = self.queue.borrow_mut();
            let before = q.len();
            q.retain(|(n, _)| n.id != id);
            before != q.len()
        };
        if removed { self.emit(Signal::Closed(id, Reason::Closed)); }
    }

    fn emit(&self, s: Signal) {
        let _ = self.out.try_send(s);
    }

    /// Win7 holds notifications back while a full-screen program is in front.
    fn fullscreen_in_front(&self) -> bool {
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
        let c = &self.wm.conn;
        if self.fullscreen_atom.get() == 0 {
            let a = c.intern_atom(false, b"_NET_WM_STATE_FULLSCREEN").ok().and_then(|r| r.reply().ok()).map(|r| r.atom).unwrap_or(0);
            self.fullscreen_atom.set(a);
        }
        let active = self.wm.active();
        if active == 0 { return false; }
        c.get_property(false, active, self.wm.atoms._NET_WM_STATE, AtomEnum::ATOM, 0, 64).ok()
            .and_then(|r| r.reply().ok())
            .and_then(|r| r.value32().map(|mut v| v.any(|a| a == self.fullscreen_atom.get())))
            .unwrap_or(false)
    }

    /// Show the next queued notification if nothing is on screen.
    fn next(self: &Rc<Self>) {
        if self.busy.get() || self.cur.borrow().is_some() { return; }
        if self.queue.borrow().is_empty() { return; }
        if self.fullscreen_in_front() {
            self.busy.set(true);
            let d = self.clone();
            glib::timeout_add_local_once(Duration::from_secs(2), move || { d.busy.set(false); d.next(); });
            return;
        }
        let n = loop {
            let Some((n, t)) = self.queue.borrow_mut().pop_front() else { return };
            if n.transient && t.elapsed() > QUEUE_TTL {
                self.emit(Signal::Closed(n.id, Reason::Expired));
                continue;
            }
            break n;
        };
        self.show(n);
    }

    // ---------------------------------------------------------- showing

    fn show(self: &Rc<Self>, n: Note) {
        self.busy.set(true);
        let app = app_info(&n);
        // point at the program's own notification-area icon, or show one temporarily
        let existing = self.tray.children().into_iter().find(|c| {
            let sc = c.style_context();
            !n.app_name.trim().is_empty() && sc.has_class("tray") && !sc.has_class("lang") && !sc.has_class("notify-temp") && c.is_visible()
                && c.tooltip_text().map(|t| t.lines().next().unwrap_or("").trim().eq_ignore_ascii_case(n.app_name.trim())).unwrap_or(false)
        });
        let (target, temp) = match existing {
            Some(w) => (w, None),
            None => {
                let b = gtk::Button::new();
                b.style_context().add_class("tray");
                b.style_context().add_class("notify-temp");
                b.add(&gtk::Image::from_pixbuf(tray_icon(&n, app.as_ref()).as_ref()));
                let tip = app.as_ref().map(|a| a.display_name().to_string()).filter(|s| !s.is_empty())
                    .unwrap_or_else(|| if n.app_name.is_empty() { n.summary.clone() } else { n.app_name.clone() });
                b.set_tooltip_text(Some(&tip));
                self.tray.pack_end(&b, false, false, 0);
                // Win7 promotes the icon at the left end of the notification area, next to the
                // language bar (children() lists pack_end children reversed: use the box position)
                if let Some(lang) = self.tray.children().into_iter().find(|c| c.style_context().has_class("lang")) {
                    self.tray.reorder_child(&b, self.tray.child_position(&lang));
                }
                let w = Rc::downgrade(self);
                b.connect_clicked(move |_| { if let Some(d) = w.upgrade() { d.clicked(1); } });
                b.show_all();
                (b.clone().upcast::<gtk::Widget>(), Some(b.upcast::<gtk::Widget>()))
            }
        };
        self.fill(&n);
        *self.cur.borrow_mut() = Some(Shown { note: n, temp, timer: None, deadline: None, paused: None });
        // wait until the taskbar has laid the icon out, then pop up
        let d = self.clone();
        let tries = Cell::new(0);
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let ready = target.is_mapped() && target.allocation().width() > 1;
            tries.set(tries.get() + 1);
            if !ready && tries.get() < 15 { return glib::ControlFlow::Continue; }
            if d.cur.borrow().is_none() {
                // closed (CloseNotification) before it popped up
                d.busy.set(false);
                d.next();
                return glib::ControlFlow::Break;
            }
            d.anchor.set(d.anchor_of(&target));
            d.relayout();
            // balloons are tooltips: "Fade or slide ToolTips into view"
            crate::anim::show_faded(&d.win, crate::anim::BALLOON_IN, crate::anim::effect("tooltip_fade"));
            d.busy.set(false);
            if let Some(s) = d.cur.borrow().as_ref() { play_sound(&s.note, &d.last_sound); }
            d.arm_timer();
            glib::ControlFlow::Break
        });
    }

    /// (stem tip x, taskbar top y) for a widget in the taskbar.
    fn anchor_of(&self, w: &gtk::Widget) -> (i32, i32) {
        let found = (|| {
            let top = w.toplevel()?;
            let (_, ox, oy) = top.window()?.origin();
            let (x, _) = w.translate_coordinates(&top, 0, 0)?;
            let a = w.allocation();
            (a.width() > 1).then_some((ox + x + a.width() / 2, oy))
        })();
        found.unwrap_or_else(|| {
            let g = util::primary_geometry();
            (g.x() + g.width() - 150, g.y() + g.height() - config::get().taskbar_height)
        })
    }

    /// Put the note's icon, title, body and action links into the balloon.
    fn fill(self: &Rc<Self>, n: &Note) {
        for c in self.body.children() { self.body.remove(&c); }
        if let Some((pb, large)) = balloon_icon(n) {
            let img = gtk::Image::from_pixbuf(Some(&pb));
            img.set_valign(gtk::Align::Start);
            img.set_halign(gtk::Align::Center);
            if large {
                img.set_size_request(32, 32);
                img.set_margin_top(1);
                img.set_margin_end(7);
            } else {
                img.set_margin_top(6);
                img.set_margin_end(6);
            }
            self.body.pack_start(&img, false, false, 0);
        }
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        col.set_valign(gtk::Align::Start);
        col.set_margin_top(2);
        self.body.pack_start(&col, true, true, 0);
        let mut labels = vec![];
        if !n.summary.trim().is_empty() {
            // the summary is plain text
            let t = gtk::Label::new(None);
            t.style_context().add_class("balloon-title");
            col.pack_start(&t, false, false, 0);
            t.set_markup(&gdi_lines(&t, &escape(n.summary.trim()), 21.0));
            labels.push(t);
        }
        if !n.body.trim().is_empty() {
            let b = gtk::Label::new(None);
            b.style_context().add_class("balloon-body");
            col.pack_start(&b, false, false, 0);
            b.set_markup(&gdi_lines(&b, &sanitize_markup(n.body.trim()), 16.0));
            if b.text().is_empty() && !strip_markup(&n.body).trim().is_empty() {
                b.set_text(strip_markup(&n.body).trim()); // markup GTK did not accept
            }
            b.set_lines(10);
            b.set_ellipsize(pango::EllipsizeMode::End);
            b.connect_activate_link(|_, uri| {
                util::open_uri(uri);
                glib::Propagation::Stop
            });
            labels.push(b);
        }
        let links: Vec<(usize, &(String, String))> = n.actions.iter().enumerate().filter(|(_, (k, l))| k != "default" && !l.is_empty()).collect();
        if !links.is_empty() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            row.set_margin_top(4);
            for (i, (_, label)) in links {
                let l = gtk::Label::new(None);
                l.set_markup(&format!("<a href=\"action:{i}\">{}</a>", escape(label)));
                l.style_context().add_class("balloon-action");
                let (w, id, key) = (Rc::downgrade(self), n.id, n.actions[i].0.clone());
                l.connect_activate_link(move |_, _| {
                    if let Some(d) = w.upgrade() { d.invoke(id, &key); }
                    glib::Propagation::Stop
                });
                row.pack_start(&l, false, false, 0);
                labels.push(l);
            }
            col.pack_start(&row, false, false, 0);
        }
        self.body.show_all();
        for l in &labels {
            l.set_xalign(0.0);
            l.set_line_wrap(true);
            l.set_line_wrap_mode(pango::WrapMode::WordChar);
            // wrap at TEXT_W pixels: GtkLabel measures max-width-chars in the larger of the
            // font's average character and digit widths
            let m = l.pango_context().metrics(None, None);
            let cw = m.approximate_char_width().max(m.approximate_digit_width()) / pango::SCALE;
            l.set_max_width_chars((TEXT_W / cw.max(1)).max(8));
        }
    }

    /// Size the window to its content and move it into place.
    fn relayout(&self) {
        self.win.set_size_request(-1, -1);
        let (_, nat) = self.win.preferred_size();
        self.win.set_size_request(nat.width, nat.height);
        self.win.resize(nat.width, nat.height);
        self.render(nat.width, nat.height);
    }

    /// Paint the balloon background for a window of ww × wh, set its shape and position.
    fn render(&self, ww: i32, wh: i32) {
        let comp = glass::composited();
        let anchor = self.anchor.get();
        if let Some(r) = self.bg.borrow().as_ref() {
            if r.size == (ww, wh) && r.anchor == anchor && r.composited == comp { return; }
        }
        let (bw, bh) = (ww - SHADOW, wh - SHADOW - STEM);
        if bw < 40 || bh < 20 { return; }
        // the stem points at the icon; the body rests on the taskbar's top edge
        let (tx, top) = anchor;
        let wa = gdk::Display::default().and_then(|d| d.primary_monitor().or_else(|| d.monitor(0))).map(|m| m.workarea())
            .unwrap_or_else(util::primary_geometry);
        let x = (tx - (bw - STEM_RIGHT)).min(wa.x() + wa.width() - bw - 2).max(wa.x() + 2);
        let y = (top - bh).max(wa.y());
        let wall = (tx - x).clamp(4, bw - 5);
        let left = wall < STEM + 4;
        let Ok(fg) = cairo::ImageSurface::create(cairo::Format::ARgb32, ww, wh) else { return };
        if let Ok(cr) = cairo::Context::new(&fg) {
            paint_balloon(&cr, self.assets.as_ref(), bw as f64, bh as f64, wall as f64, left);
        }
        // clicks only on the balloon; the shadow (composited only) is part of the visible shape
        let input = alpha_region(&fg, 127);
        self.win.input_shape_combine_region(Some(&input));
        let surf = if comp {
            match shadow_of(&fg, 2, 2, 0.55) {
                Some(sh) => {
                    if let Ok(cr) = cairo::Context::new(&sh) {
                        cr.set_source_surface(&fg, 0.0, 0.0).ok();
                        cr.paint().ok();
                    }
                    sh
                }
                None => fg,
            }
        } else {
            fg
        };
        // without a compositor alpha is ignored: cut the window to the opaque balloon
        self.win.shape_combine_region(Some(&alpha_region(&surf, if comp { 0 } else { 127 })));
        self.win.move_(x, y);
        *self.bg.borrow_mut() = Some(Rendered { size: (ww, wh), anchor, composited: comp, surf, input });
        self.win.queue_draw();
    }

    // ---------------------------------------------------------- timing

    fn arm_timer(self: &Rc<Self>) {
        let mut cur = self.cur.borrow_mut();
        let Some(s) = cur.as_mut() else { return };
        if let Some(t) = s.timer.take() { t.remove(); }
        s.paused = None;
        s.deadline = None;
        let Some(dur) = s.note.timeout().or_else(|| (!self.queue.borrow().is_empty()).then_some(DEFAULT_TIMEOUT)) else { return };
        if self.hovered() {
            s.paused = Some(dur);
            return;
        }
        s.deadline = Some(Instant::now() + dur);
        s.timer = Some(self.expire_after(s.note.id, dur));
    }

    fn expire_after(self: &Rc<Self>, id: u32, dur: Duration) -> glib::SourceId {
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(dur, move || {
            let Some(d) = w.upgrade() else { return };
            let mine = d.cur.borrow_mut().as_mut().filter(|s| s.note.id == id).map(|s| s.timer = None).is_some();
            if mine { d.finish(Reason::Expired); }
        })
    }

    fn hovered(&self) -> bool {
        let Some(gw) = self.win.window().filter(|_| self.win.is_visible()) else { return false };
        let Some(p) = gdk::Display::default().and_then(|d| d.default_seat()).and_then(|s| s.pointer()) else { return false };
        let (_, x, y, _) = gw.device_position(&p);
        self.bg.borrow().as_ref().map(|r| r.input.contains_point(x, y)).unwrap_or(false)
    }

    fn pause(self: &Rc<Self>) {
        let mut cur = self.cur.borrow_mut();
        let Some(s) = cur.as_mut() else { return };
        if let (Some(t), Some(dl)) = (s.timer.take(), s.deadline.take()) {
            t.remove();
            s.paused = Some(dl.saturating_duration_since(Instant::now()));
        }
    }

    fn resume(self: &Rc<Self>) {
        let mut cur = self.cur.borrow_mut();
        let Some(s) = cur.as_mut() else { return };
        if let Some(left) = s.paused.take() {
            let dur = left.max(MIN_AFTER_HOVER);
            s.deadline = Some(Instant::now() + dur);
            s.timer = Some(self.expire_after(s.note.id, dur));
        }
    }

    // ---------------------------------------------------------- interaction

    fn clicked(self: &Rc<Self>, button: u32) {
        let Some((id, default)) = self.cur.borrow().as_ref().map(|s| (s.note.id, s.note.has_default())) else { return };
        if button == 1 && default {
            self.invoke(id, "default");
        } else {
            self.finish(Reason::Dismissed);
        }
    }

    fn invoke(self: &Rc<Self>, id: u32, key: &str) {
        let resident = match self.cur.borrow().as_ref() { Some(s) if s.note.id == id => s.note.resident, _ => return };
        self.emit(Signal::Action(id, key.to_string()));
        if !resident { self.finish(Reason::Dismissed); }
    }

    /// Take the balloon down and report why; then the next one.
    fn finish(self: &Rc<Self>, reason: Reason) {
        let Some(s) = self.cur.borrow_mut().take() else { return };
        if let Some(t) = s.timer { t.remove(); }
        crate::anim::fade(&self.win, 0.0, crate::anim::BALLOON_OUT, crate::anim::effect("tooltip_fade"), |w| w.hide());
        self.pressed.set(false);
        self.close_state.set(0);
        if let Some(t) = s.temp { self.tray.remove(&t); }
        self.emit(Signal::Closed(s.note.id, reason));
        if self.busy.get() { return; } // still waiting to pop up: show() bails out
        self.busy.set(true);
        let d = self.clone();
        glib::timeout_add_local_once(GAP, move || { d.busy.set(false); d.next(); });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{StructureBuilder, Value};

    fn ov(v: Value<'_>) -> OwnedValue { v.try_to_owned().unwrap() }

    #[test]
    fn markup_subset() {
        assert_eq!(sanitize_markup("<b>bold</b> & <i>it</i>"), "<b>bold</b> &amp; <i>it</i>");
        assert_eq!(sanitize_markup("1 < 2 > 0"), "1 &lt; 2 &gt; 0");
        assert_eq!(sanitize_markup("&amp; &lt; &#169; &nbsp;"), "&amp; &lt; &#169; &amp;nbsp;");
        assert_eq!(sanitize_markup("<font color='red'>x</font>"), "x");
        assert_eq!(sanitize_markup("<img src=\"a.png\" alt=\"Pic & co\"/> y"), "Pic &amp; co y");
        assert_eq!(sanitize_markup("<a href=\"http://x/?a=1&amp;b=2\">l</a>"), "<a href=\"http://x/?a=1&amp;b=2\">l</a>");
        assert_eq!(sanitize_markup("<a href='q\"'>l</a>"), "<a href=\"q&quot;\">l</a>");
        assert_eq!(sanitize_markup("<a>no target</a>"), "no target");
        // misnested and unclosed tags are repaired
        assert_eq!(sanitize_markup("<b><i>x</b>y</i>"), "<b><i>x</i></b>y");
        assert_eq!(sanitize_markup("<u>open"), "<u>open</u>");
        assert_eq!(sanitize_markup("line<br/>two"), "line\ntwo");
        assert_eq!(sanitize_markup("<B>caps</B>"), "<b>caps</b>");
    }

    #[test]
    fn markup_to_text() {
        assert_eq!(strip_markup("<b>a</b> &amp; <a href='x'>b</a> &#x263A;"), "a & b \u{263A}");
    }

    #[test]
    fn hints_decoded() {
        let mut h = HashMap::new();
        h.insert("urgency".to_string(), ov(Value::U8(2)));
        h.insert("transient".to_string(), ov(Value::Bool(true)));
        h.insert("resident".to_string(), ov(Value::I32(1)));
        h.insert("suppress-sound".to_string(), ov(Value::Bool(false)));
        h.insert("image-path".to_string(), ov(Value::from("/tmp/x.png")));
        h.insert("sound-name".to_string(), ov(Value::from("message-new-email")));
        h.insert("desktop-entry".to_string(), ov(Value::from("org.example.App")));
        let img = StructureBuilder::new()
            .add_field(2i32).add_field(1i32).add_field(8i32).add_field(true).add_field(8i32).add_field(4i32)
            .add_field(vec![1u8, 2, 3, 4, 5, 6, 7, 8])
            .build();
        h.insert("image-data".to_string(), ov(Value::from(img)));
        let n = Note::from_dbus(7, "app".into(), "".into(), "S".into(), "B".into(),
            vec!["default".into(), "Open".into(), "later".into(), "Later".into(), "odd".into()], h, -1);
        assert_eq!(n.id, 7);
        assert_eq!(n.urgency, 2);
        assert!(n.transient && n.resident && !n.suppress_sound);
        assert_eq!(n.image_path.as_deref(), Some("/tmp/x.png"));
        assert_eq!(n.sound_name.as_deref(), Some("message-new-email"));
        assert_eq!(n.desktop_entry.as_deref(), Some("org.example.App"));
        assert_eq!(n.actions, vec![("default".to_string(), "Open".to_string()), ("later".to_string(), "Later".to_string())]);
        assert!(n.has_default());
        let img = n.image.as_ref().unwrap();
        assert_eq!((img.width, img.height, img.rowstride, img.channels), (2, 1, 8, 4));
        assert_eq!(img.data.len(), 8);
        // critical never expires on its own
        assert_eq!(n.timeout(), None);
    }

    #[test]
    fn timeouts() {
        let n = |t: i32, u: u8| Note { expire_timeout: t, urgency: u, ..Default::default() };
        assert_eq!(n(-1, 1).timeout(), Some(DEFAULT_TIMEOUT));
        assert_eq!(n(0, 1).timeout(), None);
        assert_eq!(n(2500, 0).timeout(), Some(Duration::from_millis(2500)));
        assert_eq!(n(2500, 2).timeout(), None);
    }

    #[test]
    fn raw_image_validation() {
        let ok = RawImage { width: 2, height: 2, rowstride: 8, has_alpha: true, bits: 8, channels: 4, data: vec![0; 16] };
        let short = RawImage { data: vec![0; 10], ..ok.clone() };
        let bad_stride = RawImage { rowstride: 4, ..ok.clone() };
        let bad_bits = RawImage { bits: 16, ..ok.clone() };
        // pixbuf() needs no display; only the validation is tested here
        assert!(short.pixbuf().is_none());
        assert!(bad_stride.pixbuf().is_none());
        assert!(bad_bits.pixbuf().is_none());
        assert!(ok.pixbuf().is_some());
    }
}
