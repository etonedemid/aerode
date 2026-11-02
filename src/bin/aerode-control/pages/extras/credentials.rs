//! Credential Manager: the Windows Vault = the Secret Service keyring (gnome-keyring, KeePassXC,
//! KWallet's Secret Service, over D-Bus), plus KWallet's own entries when KWallet is used and the
//! personal certificates of the NSS database (Chromium, Thunderbird…). Windows credentials are
//! network passwords (gvfs: smb, ftp, sftp, WebDAV); everything else is a generic credential.
use gtk::prelude::*;
use gtk::glib;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use aerode::{config, util};
use crate::{link, text, title, Cp};
use super::common::*;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const SS: &str = "org.freedesktop.secrets";
const APPID: &str = "Aerode Credential Manager";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind { Windows, Cert, Generic }

#[derive(Clone, Debug)]
enum Source { Secret(String), KWallet { folder: String, key: String }, Nss(String) }

#[derive(Clone, Debug)]
struct Cred { kind: Kind, source: Source, name: String, address: String, user: String, modified: i64, keyring: String, app: String }

#[derive(Clone, Debug, Default)]
struct Vault {
    creds: Vec<Cred>,
    /// locked collections: (object path, label)
    locked: Vec<(String, String)>,
    /// why the Secret Service could not be read
    ss_error: Option<String>,
    /// KWallet is used but its wallet is closed (opening it asks for its password)
    kwallet_closed: bool,
}

fn take<T: TryFrom<OwnedValue>>(m: &mut HashMap<String, OwnedValue>, k: &str) -> Option<T> { m.remove(k).and_then(|v| T::try_from(v).ok()) }

fn get_all(conn: &Connection, dest: &str, path: &str, iface: &str) -> zbus::Result<HashMap<String, OwnedValue>> {
    Proxy::new(conn, dest.to_string(), path.to_string(), "org.freedesktop.DBus.Properties")?.call("GetAll", &(iface,))
}

fn classify(attrs: &HashMap<String, String>, label: &str) -> Kind {
    let schema = attrs.get("xdg:schema").map(|s| s.as_str()).unwrap_or("");
    if schema == "org.gnome.keyring.NetworkPassword" || (attrs.contains_key("server") && attrs.contains_key("protocol")) { return Kind::Windows; }
    if attrs.get("setting-key").map(|k| k.ends_with("private-key-password")).unwrap_or(false) || label.to_lowercase().contains("certificate") { return Kind::Cert; }
    Kind::Generic
}

fn secret_items(v: &mut Vault) -> zbus::Result<()> {
    let conn = Connection::session()?;
    let mut svc = get_all(&conn, SS, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service")?;
    let colls: Vec<OwnedObjectPath> = take(&mut svc, "Collections").unwrap_or_default();
    for c in colls {
        let Ok(mut cp) = get_all(&conn, SS, c.as_str(), "org.freedesktop.Secret.Collection") else { continue };
        let label: String = take(&mut cp, "Label").unwrap_or_default();
        let label = if label.is_empty() { c.as_str().rsplit('/').next().unwrap_or("").to_string() } else { label };
        if take::<bool>(&mut cp, "Locked").unwrap_or(false) { v.locked.push((c.to_string(), label.clone())); }
        let items: Vec<OwnedObjectPath> = take(&mut cp, "Items").unwrap_or_default();
        for it in items {
            let Ok(mut ip) = get_all(&conn, SS, it.as_str(), "org.freedesktop.Secret.Item") else { continue };
            let name: String = take(&mut ip, "Label").unwrap_or_default();
            let attrs: HashMap<String, String> = take(&mut ip, "Attributes").unwrap_or_default();
            let modified = take::<u64>(&mut ip, "Modified").or_else(|| take::<u64>(&mut ip, "Created")).unwrap_or(0) as i64;
            let kind = classify(&attrs, &name);
            let a = |k: &str| attrs.get(k).cloned().unwrap_or_default();
            let user = ["user", "username", "account", "login", "email", "UserName"].iter().map(|k| a(k)).find(|s| !s.is_empty()).unwrap_or_default();
            let address = if kind == Kind::Windows {
                let server = a("server");
                let proto = a("protocol");
                let share = { let d = a("domain"); if d.is_empty() { String::new() } else { format!(" ({d})") } };
                if proto.is_empty() { format!("{server}{share}") } else { format!("{proto}://{server}{share}") }
            } else { ["service", "target", "url", "origin_url", "server", "host"].iter().map(|k| a(k)).find(|s| !s.is_empty()).unwrap_or_else(|| name.clone()) };
            let app = ["application", "xdg:schema"].iter().map(|k| a(k)).find(|s| !s.is_empty()).unwrap_or_default();
            let shown = if name.is_empty() { address.clone() } else { name };
            v.creds.push(Cred { kind, source: Source::Secret(it.to_string()), name: shown, address, user, modified, keyring: label.clone(), app });
        }
    }
    Ok(())
}

/// The kwalletd answering on the session bus: (bus name, object path).
fn kwallet_service(conn: &Connection) -> Option<(&'static str, &'static str)> {
    let dbus = Proxy::new(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus").ok()?;
    let mut names: Vec<String> = dbus.call("ListNames", &()).unwrap_or_default();
    names.extend(dbus.call::<_, _, Vec<String>>("ListActivatableNames", &()).unwrap_or_default());
    [("org.kde.kwalletd6", "/modules/kwalletd6"), ("org.kde.kwalletd5", "/modules/kwalletd5")].into_iter().find(|(n, _)| names.iter().any(|x| x == n))
}

fn kwallet_used() -> bool { config::home().join(".local/share/kwalletd/kdewallet.kwl").exists() }

/// Open the wallet (asks for its password when it isn't open yet unless `only_if_open`).
fn kwallet_open(conn: &Connection, only_if_open: bool) -> Option<(Proxy<'static>, i32)> {
    let (name, path) = kwallet_service(conn)?;
    let p = Proxy::new(conn, name, path, "org.kde.KWallet").ok()?;
    let wallet: String = p.call("networkWallet", &()).unwrap_or_else(|_| "kdewallet".into());
    if only_if_open && !p.call::<_, _, bool>("isOpen", &(wallet.as_str(),)).unwrap_or(false) { return None; }
    let h: i32 = p.call("open", &(wallet.as_str(), 0i64, APPID)).ok()?;
    (h >= 0).then_some((p, h))
}

fn kwallet_items(v: &mut Vault, ask_password: bool) {
    if !kwallet_used() { return; }
    let Ok(conn) = Connection::session() else { return };
    if kwallet_service(&conn).is_none() { return; }
    let Some((p, h)) = kwallet_open(&conn, !ask_password) else { v.kwallet_closed = true; return };
    let folders: Vec<String> = p.call("folderList", &(h, APPID)).unwrap_or_default();
    for f in folders {
        let entries: Vec<String> = p.call("entryList", &(h, f.as_str(), APPID)).unwrap_or_default();
        for e in entries {
            let kind = if f == "Network Management" && e.contains("802-1x") { Kind::Cert } else { Kind::Generic };
            v.creds.push(Cred { kind, source: Source::KWallet { folder: f.clone(), key: e.clone() }, name: e.clone(), address: e.clone(), user: String::new(), modified: 0, keyring: format!("KWallet ▸ {f}"), app: f.clone() });
        }
    }
}

fn nssdb() -> String { format!("sql:{}", config::home().join(".pki/nssdb").display()) }

fn nss_certs(v: &mut Vault) {
    if !util::which("certutil") || !config::home().join(".pki/nssdb").is_dir() { return; }
    let o = exec(&sv(&["certutil", "-L", "-d", &nssdb()]), None);
    for l in o.stdout.lines().skip(3) {
        let l = l.trim_end();
        let Some((nick, trust)) = l.rsplit_once(char::is_whitespace) else { continue };
        if !trust.contains('u') { continue; }
        let nick = nick.trim().to_string();
        v.creds.push(Cred { kind: Kind::Cert, source: Source::Nss(nick.clone()), name: nick.clone(), address: nick, user: String::new(), modified: 0, keyring: "Certificate database (~/.pki/nssdb)".into(), app: "NSS".into() });
    }
}

fn load(ask_kwallet: bool) -> Vault {
    let mut v = Vault::default();
    if let Err(e) = secret_items(&mut v) {
        let e = e.to_string();
        v.ss_error = Some(if e.contains("ServiceUnknown") || e.contains("was not provided") { "no keyring program (Secret Service) is running".into() } else { e });
    }
    kwallet_items(&mut v, ask_kwallet);
    nss_certs(&mut v);
    v.creds.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

/// Answer a Secret Service prompt (unlock, delete…) and wait until it is done.
fn prompt(conn: &Connection, path: &str) -> zbus::Result<()> {
    if path == "/" { return Ok(()); }
    let p = Proxy::new(conn, SS, path.to_string(), "org.freedesktop.Secret.Prompt")?;
    let mut sig = p.receive_signal("Completed")?;
    p.call_method("Prompt", &("",))?;
    let _ = sig.next();
    Ok(())
}

fn unlock(paths: Vec<String>) -> Result<(), String> {
    let conn = Connection::session().map_err(|e| e.to_string())?;
    let svc = Proxy::new(&conn, SS, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").map_err(|e| e.to_string())?;
    let objs: Vec<ObjectPath> = paths.iter().filter_map(|p| ObjectPath::try_from(p.as_str()).ok()).collect();
    let (_, pr): (Vec<OwnedObjectPath>, OwnedObjectPath) = svc.call("Unlock", &(objs,)).map_err(|e| e.to_string())?;
    prompt(&conn, pr.as_str()).map_err(|e| e.to_string())
}

fn remove(c: &Cred) -> Result<(), String> {
    if dry_run() { eprintln!("[aerode dry run] remove credential {:?}", c.source); return Ok(()); }
    match &c.source {
        Source::Secret(path) => {
            let conn = Connection::session().map_err(|e| e.to_string())?;
            let it = Proxy::new(&conn, SS, path.clone(), "org.freedesktop.Secret.Item").map_err(|e| e.to_string())?;
            let pr: OwnedObjectPath = it.call("Delete", &()).map_err(|e| e.to_string())?;
            prompt(&conn, pr.as_str()).map_err(|e| e.to_string())
        }
        Source::KWallet { folder, key } => {
            let conn = Connection::session().map_err(|e| e.to_string())?;
            let (p, h) = kwallet_open(&conn, false).ok_or("KWallet is not available")?;
            let r: i32 = p.call("removeEntry", &(h, folder.as_str(), key.as_str(), APPID)).map_err(|e| e.to_string())?;
            if r == 0 { Ok(()) } else { Err(format!("KWallet error {r}")) }
        }
        Source::Nss(nick) => { let o = exec(&sv(&["certutil", "-D", "-n", nick, "-d", &nssdb()]), None); if o.ok { Ok(()) } else { Err(o.text()) } }
    }
}

/// Store a password in the default keyring (a new item, or `item` to change its secret).
fn store(label: String, attrs: HashMap<String, String>, password: String, item: Option<String>) -> Result<(), String> {
    if dry_run() { eprintln!("[aerode dry run] store credential {label} {attrs:?}"); return Ok(()); }
    let e = |e: zbus::Error| e.to_string();
    let conn = Connection::session().map_err(e)?;
    let svc = Proxy::new(&conn, SS, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").map_err(e)?;
    let (_, session): (OwnedValue, OwnedObjectPath) = svc.call("OpenSession", &("plain", Value::from(""))).map_err(e)?;
    let secret = (session.as_ref(), Vec::<u8>::new(), password.into_bytes(), "text/plain");
    if let Some(item) = item {
        let it = Proxy::new(&conn, SS, item, "org.freedesktop.Secret.Item").map_err(e)?;
        it.call_method("SetSecret", &(secret,)).map_err(e)?;
        return Ok(());
    }
    let mut coll: OwnedObjectPath = svc.call("ReadAlias", &("default",)).map_err(e)?;
    if coll.as_str() == "/" {
        let mut sp = get_all(&conn, SS, "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").map_err(e)?;
        let cs: Vec<OwnedObjectPath> = take(&mut sp, "Collections").unwrap_or_default();
        coll = cs.into_iter().find(|c| !c.as_str().ends_with("/session")).ok_or("There is no keyring to save the credential in.")?;
    }
    let mut cp = get_all(&conn, SS, coll.as_str(), "org.freedesktop.Secret.Collection").map_err(e)?;
    if take::<bool>(&mut cp, "Locked").unwrap_or(false) { unlock(vec![coll.to_string()])?; }
    let c = Proxy::new(&conn, SS, coll.to_string(), "org.freedesktop.Secret.Collection").map_err(e)?;
    let mut props: HashMap<&str, Value> = HashMap::new();
    props.insert("org.freedesktop.Secret.Item.Label", Value::from(label.as_str()));
    props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attrs));
    let (_, pr): (OwnedObjectPath, OwnedObjectPath) = c.call("CreateItem", &(props, secret, true)).map_err(e)?;
    prompt(&conn, pr.as_str()).map_err(e)
}

// ------------------------------------------------------------------ the page

pub fn page_credentials(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    side_later(cp, |cp| cp.side_link("User Accounts", "users"));
    let holder = vbox(0);
    holder.pack_start(&loading("Reading the vault…"), false, false, 0);
    refresh(cp, &holder, false);
    holder.upcast()
}

fn refresh(cp: &Rc<Cp>, holder: &gtk::Box, ask_kwallet: bool) {
    let (me, h) = (cp.clone(), holder.clone());
    background(move || load(ask_kwallet), move |v| { let p = page(&me, &h, &v); fill(&h, &p); });
}

fn page(cp: &Rc<Cp>, holder: &gtk::Box, v: &Vault) -> gtk::Widget {
    let b = vbox(8);
    b.pack_start(&title("Store credentials for automatic logon"), false, false, 0);
    b.pack_start(&text("Use Credential Manager to store credentials, such as user names and passwords, that you use to log on to websites or other computers on a network."), false, false, 0);
    let vault = hbox(10);
    vault.set_margin_top(6);
    vault.pack_start(&util::image("dialog-password", 32), false, false, 0);
    let vt = vbox(0);
    let n = gtk::Label::new(None);
    n.set_markup("<b>Windows Vault</b>");
    n.set_xalign(0.0);
    vt.pack_start(&n, false, false, 0);
    vt.pack_start(&grey("Default vault location"), false, false, 0);
    vault.pack_start(&vt, true, true, 0);
    { let me = cp.clone(); vault.pack_end(&link("Restore vault", move || restore_vault(&me)), false, false, 0); }
    { let me = cp.clone(); vault.pack_end(&link("Back up vault", move || backup_vault(&me)), false, false, 8); }
    b.pack_start(&vault, false, false, 0);
    if let Some(e) = &v.ss_error {
        let h = hbox(8);
        h.pack_start(&mark(Mark::Warn), false, false, 0);
        h.pack_start(&label(&format!("The vault (Secret Service keyring) is not available: {e}. Install gnome-keyring (or another Secret Service provider such as KeePassXC) and log on again.")), true, true, 0);
        b.pack_start(&h, false, false, 4);
    }
    for (path, l) in &v.locked {
        let h = hbox(8);
        h.pack_start(&mark(Mark::Info), false, false, 0);
        h.pack_start(&label(&format!("The \"{l}\" keyring is locked, so its credentials can't be shown.")), true, true, 0);
        let (me, hd, p) = (cp.clone(), holder.clone(), path.clone());
        h.pack_end(&link("Unlock", move || {
            let (me, hd) = (me.clone(), hd.clone());
            let p = p.clone();
            background(move || unlock(vec![p]), move |r| { if let Err(e) = r { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "Credential Manager", "The keyring could not be unlocked.", &e); } refresh(&me, &hd, false); });
        }), false, false, 0);
        b.pack_start(&h, false, false, 0);
    }
    for (kind, head, add, empty) in [
        (Kind::Windows, "Windows Credentials", "Add a Windows credential", "No Windows credentials."),
        (Kind::Cert, "Certificate-Based Credentials", "Add a certificate-based credential", "No certificates."),
        (Kind::Generic, "Generic Credentials", "Add a generic credential", "No generic credentials."),
    ] {
        let (me, hd) = (cp.clone(), holder.clone());
        let addl = link(add, move || match kind { Kind::Cert => add_certificate(&me, &hd), k => add_credential(&me, &hd, k) });
        let sec = band(head, Some(&addl.upcast()));
        sec.set_margin_top(10);
        b.pack_start(&sec, false, false, 0);
        let items: Vec<&Cred> = v.creds.iter().filter(|c| c.kind == kind).collect();
        let list = vbox(0);
        list.set_margin_start(10);
        if items.is_empty() { list.pack_start(&grey(empty).tap(|l| l.set_margin_top(4)), false, false, 2); }
        for c in items { list.pack_start(&item_row(cp, holder, c), false, false, 0); }
        b.pack_start(&list, false, false, 0);
    }
    if v.kwallet_closed {
        let (me, hd) = (cp.clone(), holder.clone());
        let l = link("Show the credentials stored in KWallet (asks for the wallet password)", move || { fill(&hd, &loading("Opening KWallet…")); refresh(&me, &hd, true); });
        l.set_margin_top(8);
        b.pack_start(&l, false, false, 0);
    }
    b.upcast()
}

/// "Today" or the short date, as the vault list shows it.
fn modified_text(secs: i64) -> Option<String> {
    let t = unix_time(secs)?;
    Some(if t.date_naive() == chrono::Local::now().date_naive() { "Today".into() } else { win_date(t) })
}

/// The round chevron on the right of a vault entry (down = collapsed, up = expanded).
fn chevron(open: Rc<std::cell::Cell<bool>>) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_size_request(16, 16);
    d.set_valign(gtk::Align::Center);
    d.connect_draw(move |_, cr| {
        cr.arc(8.0, 8.0, 7.0, 0.0, 2.0 * std::f64::consts::PI);
        cr.set_source_rgb(0.97, 0.98, 1.0);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(0.62, 0.70, 0.80);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        cr.set_source_rgb(0.25, 0.35, 0.55);
        cr.set_line_width(1.6);
        let (a, b) = if open.get() { (9.5, 6.5) } else { (6.5, 9.5) };
        cr.move_to(4.8, a); cr.line_to(8.0, b); cr.line_to(11.2, a);
        let _ = cr.stroke();
        glib::Propagation::Stop
    });
    d
}

fn item_row(cp: &Rc<Cp>, holder: &gtk::Box, c: &Cred) -> gtk::Widget {
    // Win7: the entry's name, "Modified: date" and a round chevron on the right; a click opens it
    let headb = gtk::Button::new();
    headb.set_relief(gtk::ReliefStyle::None);
    headb.style_context().add_class("xt-cred-head");
    let head = hbox(8);
    let n = label(&c.name);
    n.set_ellipsize(gtk::pango::EllipsizeMode::End);
    n.set_max_width_chars(50);
    head.pack_start(&n, true, true, 0);
    let open = Rc::new(std::cell::Cell::new(false));
    let chev = chevron(open.clone());
    head.pack_end(&chev, false, false, 0);
    if c.modified > 0 { if let Some(t) = modified_text(c.modified) { head.pack_end(&grey(&format!("Modified: {t}")), false, false, 0); } }
    else if matches!(c.source, Source::KWallet { .. }) { head.pack_end(&grey("KWallet"), false, false, 0); }
    headb.add(&head);
    let e = gtk::Revealer::new();
    e.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    e.set_transition_duration(150);
    { let (e, open, chev) = (e.clone(), open.clone(), chev.clone()); headb.connect_clicked(move |_| { open.set(!open.get()); e.set_reveal_child(open.get()); chev.queue_draw(); }); }
    let body = vbox(6);
    body.set_margin_start(18);
    body.set_margin_top(4);
    body.set_margin_bottom(6);
    let mut rows: Vec<(&str, String)> = vec![];
    match c.kind {
        Kind::Cert => { rows.push(("Certificate:", c.address.clone())); }
        _ => { rows.push(("Internet or network address:", c.address.clone())); rows.push(("User name:", if c.user.is_empty() { "—".into() } else { c.user.clone() })); rows.push(("Password:", "••••••••".into())); }
    }
    rows.push(("Persistence:", c.keyring.clone()));
    if !c.app.is_empty() { rows.push(("Saved by:", c.app.clone())); }
    body.pack_start(&crate::kv_grid(&rows), false, false, 0);
    let acts = hbox(16);
    acts.set_margin_start(16);
    if let (Source::Secret(path), true) = (&c.source, c.kind != Kind::Cert) {
        let (me, hd, path, c2) = (cp.clone(), holder.clone(), path.clone(), c.clone());
        acts.pack_start(&link("Edit", move || edit_credential(&me, &hd, &c2, &path)), false, false, 0);
    }
    {
        let (me, hd, c2) = (cp.clone(), holder.clone(), c.clone());
        acts.pack_start(&link("Remove from vault", move || {
            let w: gtk::Window = me.win.clone().upcast();
            let (t, q) = match c2.kind {
                Kind::Windows => ("Delete Windows Credential", "Are you sure you want to permanently delete this Windows credential?"),
                Kind::Cert => ("Delete Certificate-Based Credential", "Are you sure you want to permanently delete this certificate?"),
                Kind::Generic => ("Delete Generic Credential", "Are you sure you want to permanently delete this generic credential?"),
            };
            if !ask(Some(&w), gtk::MessageType::Warning, t, q, &format!("{}\n\nPrograms that use it will ask for the password again.", c2.name), "_Yes", "_No") { return; }
            let (me, hd, c3) = (me.clone(), hd.clone(), c2.clone());
            background(move || remove(&c3), move |r| {
                if let Err(e) = r { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "Credential Manager", "The credential could not be removed.", &e); }
                refresh(&me, &hd, false);
            });
        }), false, false, 0);
    }
    body.pack_start(&acts, false, false, 0);
    e.add(&body);
    let frame = vbox(0);
    frame.pack_start(&headb, false, false, 0);
    frame.pack_start(&e, false, false, 0);
    frame.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
    frame.upcast()
}

/// "Add a Windows credential" / "Add a generic credential".
fn add_credential(cp: &Rc<Cp>, holder: &gtk::Box, kind: Kind) {
    let w: gtk::Window = cp.win.clone().upcast();
    let t = if kind == Kind::Windows { "Add a Windows Credential" } else { "Add a Generic Credential" };
    let d = gtk::Dialog::with_buttons(Some(t), Some(&w), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(460, -1);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(8);
    c.pack_start(&instr("Type the address of the website or network location and your credentials"), false, false, 0);
    c.pack_start(&label("Make sure that the user name and password that you type can be used to access the location."), false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(8);
    g.set_column_spacing(10);
    let (addr, user, pw) = (gtk::Entry::new(), gtk::Entry::new(), gtk::Entry::new());
    pw.set_visibility(false);
    pw.set_activates_default(true);
    let first = if kind == Kind::Windows { "Internet or network address\n(e.g. myserver, server.company.com):" } else { "Internet or network address:" };
    for (i, (l, e)) in [(first, &addr), ("User name:", &user), ("Password:", &pw)].into_iter().enumerate() {
        let lab = gtk::Label::new(Some(l));
        lab.set_xalign(0.0);
        g.attach(&lab, 0, i as i32, 1, 1);
        e.set_hexpand(true);
        g.attach(e, 1, i as i32, 1, 1);
    }
    c.pack_start(&g, false, false, 6);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    let r = d.run();
    let (a, u, p) = (addr.text().trim().to_string(), user.text().trim().to_string(), pw.text().to_string());
    d.close();
    if r != gtk::ResponseType::Ok || a.is_empty() { return; }
    let mut attrs: HashMap<String, String> = HashMap::new();
    let label = if kind == Kind::Windows {
        // the way gvfs stores network passwords, so Explorer's smb:// finds it
        let (proto, server) = match a.split_once("://") { Some((p, s)) => (p.to_string(), s.trim_end_matches('/').to_string()), None => ("smb".into(), a.trim_start_matches("\\\\").replace('\\', "/").trim_end_matches('/').to_string()) };
        let server = server.split('/').next().unwrap_or(&server).to_string();
        let (domain, user) = match u.split_once('\\') { Some((d, u)) => (d.to_string(), u.to_string()), None => (String::new(), u.clone()) };
        attrs.insert("xdg:schema".into(), "org.gnome.keyring.NetworkPassword".into());
        attrs.insert("server".into(), server.to_lowercase());
        attrs.insert("protocol".into(), proto.clone());
        attrs.insert("user".into(), user.clone());
        if !domain.is_empty() { attrs.insert("domain".into(), domain); }
        format!("{proto}://{user}@{server}/")
    } else {
        attrs.insert("xdg:schema".into(), "org.freedesktop.Secret.Generic".into());
        attrs.insert("service".into(), a.clone());
        attrs.insert("username".into(), u.clone());
        attrs.insert("application".into(), "Credential Manager".into());
        a.clone()
    };
    let (me, hd) = (cp.clone(), holder.clone());
    background(move || store(label, attrs, p, None), move |r| {
        if let Err(e) = r { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "Credential Manager", "The credential could not be saved.", &e); }
        refresh(&me, &hd, false);
    });
}

fn edit_credential(cp: &Rc<Cp>, holder: &gtk::Box, c: &Cred, path: &str) {
    let w: gtk::Window = cp.win.clone().upcast();
    let t = if c.kind == Kind::Windows { "Edit Windows Credential" } else { "Edit Generic Credential" };
    let d = gtk::Dialog::with_buttons(Some(t), Some(&w), gtk::DialogFlags::MODAL, &[("_Save", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let b = d.content_area();
    b.set_border_width(14);
    b.set_spacing(8);
    b.pack_start(&instr("Edit the password for this credential"), false, false, 0);
    b.pack_start(&crate::kv_grid(&[("Internet or network address:", c.address.clone()), ("User name:", c.user.clone())]), false, false, 0);
    let h = hbox(8);
    h.pack_start(&gtk::Label::new(Some("Password:")), false, false, 0);
    let pw = gtk::Entry::new();
    pw.set_visibility(false);
    pw.set_activates_default(true);
    h.pack_start(&pw, true, true, 0);
    b.pack_start(&h, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    let r = d.run();
    let p = pw.text().to_string();
    d.close();
    if r != gtk::ResponseType::Ok { return; }
    let (me, hd, path) = (cp.clone(), holder.clone(), path.to_string());
    background(move || store(String::new(), HashMap::new(), p, Some(path)), move |r| {
        if let Err(e) = r { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "Credential Manager", "The credential could not be changed.", &e); }
        refresh(&me, &hd, false);
    });
}

/// "Add a certificate-based credential": import a personal certificate (PKCS #12) into the NSS
/// database that Chromium-based browsers, Thunderbird and others use.
fn add_certificate(cp: &Rc<Cp>, holder: &gtk::Box) {
    let w: gtk::Window = cp.win.clone().upcast();
    if !util::which("pk12util") {
        message(Some(&w), gtk::MessageType::Info, "Credential Manager", "Certificates can't be added yet.", "Install the nss package (it provides pk12util and certutil), then try again.");
        return;
    }
    let fc = gtk::FileChooserNative::new(Some("Select a certificate"), Some(&w), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
    let f = gtk::FileFilter::new();
    f.set_name(Some("Personal Information Exchange (*.pfx; *.p12)"));
    for p in ["*.pfx", "*.p12", "*.PFX", "*.P12"] { f.add_pattern(p); }
    fc.add_filter(f);
    if fc.run() != gtk::ResponseType::Accept { return; }
    let Some(file) = fc.filename() else { return };
    let Some(pass) = password_prompt(&w, "Type the password that protects this certificate's private key:") else { return };
    let db = config::home().join(".pki/nssdb");
    let (me, hd) = (cp.clone(), holder.clone());
    background(move || {
        if !db.is_dir() {
            let _ = std::fs::create_dir_all(&db);
            exec(&sv(&["certutil", "-N", "-d", &nssdb(), "--empty-password"]), None);
        }
        change_blocking(&sv(&["pk12util", "-i", &file.to_string_lossy(), "-d", &nssdb(), "-W", &pass]), None)
    }, move |o| {
        if !o.ok { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "Credential Manager", "The certificate could not be added.", &o.text()); }
        refresh(&me, &hd, false);
    });
}

fn password_prompt(parent: &gtk::Window, text: &str) -> Option<String> {
    let d = gtk::Dialog::with_buttons(Some("Credential Manager"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    c.pack_start(&label(text), false, false, 0);
    let e = gtk::Entry::new();
    e.set_visibility(false);
    e.set_activates_default(true);
    c.pack_start(&e, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    let r = d.run();
    let t = e.text().to_string();
    d.close();
    (r == gtk::ResponseType::Ok).then_some(t)
}

/// The folders that hold the vault: gnome-keyring's keyrings and KWallet's wallets.
fn vault_dirs() -> Vec<&'static str> {
    ["keyrings", "kwalletd"].into_iter().filter(|d| glib::user_data_dir().join(d).is_dir()).collect()
}

fn backup_vault(cp: &Rc<Cp>) {
    let w: gtk::Window = cp.win.clone().upcast();
    let dirs = vault_dirs();
    if dirs.is_empty() { message(Some(&w), gtk::MessageType::Info, "Stored User Names and Passwords", "There is nothing to back up.", "No keyrings or wallets were found in your home folder."); return; }
    let fc = gtk::FileChooserNative::new(Some("Back up Stored User Names and Passwords"), Some(&w), gtk::FileChooserAction::Save, Some("Save"), Some("Cancel"));
    fc.set_current_name(&format!("Credentials {}.tar.gz", chrono::Local::now().format("%Y-%m-%d")));
    fc.set_do_overwrite_confirmation(true);
    if fc.run() != gtk::ResponseType::Accept { return; }
    let Some(file) = fc.filename() else { return };
    let mut argv = sv(&["tar", "-czf", &file.to_string_lossy(), "-C", &glib::user_data_dir().to_string_lossy()]);
    argv.extend(dirs.iter().map(|d| d.to_string()));
    let w2 = w.clone();
    exec_async(argv, None, move |o| {
        if o.ok { message(Some(&w2), gtk::MessageType::Info, "Stored User Names and Passwords", "The backup was created successfully.", "Your keyrings stay encrypted with their passwords inside the backup. Keep it on a removable drive."); }
        else { message(Some(&w2), gtk::MessageType::Error, "Stored User Names and Passwords", "The backup could not be created.", &o.text()); }
    });
}

fn restore_vault(cp: &Rc<Cp>) {
    let w: gtk::Window = cp.win.clone().upcast();
    let fc = gtk::FileChooserNative::new(Some("Restore Stored User Names and Passwords"), Some(&w), gtk::FileChooserAction::Open, Some("Restore"), Some("Cancel"));
    let f = gtk::FileFilter::new();
    f.set_name(Some("Credential backups (*.tar.gz)"));
    f.add_pattern("*.tar.gz");
    fc.add_filter(f);
    if fc.run() != gtk::ResponseType::Accept { return; }
    let Some(file) = fc.filename() else { return };
    if !ask(Some(&w), gtk::MessageType::Warning, "Stored User Names and Passwords", "Restoring replaces all the credentials in your vault with the ones in the backup. Do you want to continue?",
        "Your current keyrings are kept in a folder named \"…before restore\" next to them. Log off and log on again after restoring.", "_Restore", "Cancel") { return; }
    let data: PathBuf = glib::user_data_dir();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let w2 = w.clone();
    background(move || {
        if dry_run() { eprintln!("[aerode dry run] restore vault from {}", file.display()); return Out { ok: true, ..Default::default() }; }
        let list = exec(&sv(&["tar", "-tzf", &file.to_string_lossy()]), None);
        if !list.ok || !list.stdout.lines().all(|l| l.starts_with("keyrings") || l.starts_with("kwalletd")) {
            return Out { stderr: "This file is not a credential backup.".into(), ..Default::default() };
        }
        for d in ["keyrings", "kwalletd"] {
            let p = data.join(d);
            if p.exists() && list.stdout.lines().any(|l| l.starts_with(d)) { let _ = std::fs::rename(&p, data.join(format!("{d} before restore {stamp}"))); }
        }
        exec(&sv(&["tar", "-xzf", &file.to_string_lossy(), "-C", &data.to_string_lossy()]), None)
    }, move |o| {
        if o.ok { message(Some(&w2), gtk::MessageType::Info, "Stored User Names and Passwords", "Your credentials were restored.", "Log off and log on again to use them."); }
        else { message(Some(&w2), gtk::MessageType::Error, "Stored User Names and Passwords", "The credentials could not be restored.", &o.text()); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kinds() {
        let mut a = HashMap::new();
        a.insert("xdg:schema".to_string(), "org.gnome.keyring.NetworkPassword".to_string());
        assert_eq!(classify(&a, "x"), Kind::Windows);
        let mut b = HashMap::new();
        b.insert("setting-key".to_string(), "private-key-password".to_string());
        assert_eq!(classify(&b, "x"), Kind::Cert);
        assert_eq!(classify(&HashMap::new(), "Chrome Safe Storage"), Kind::Generic);
    }
}
