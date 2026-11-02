//! aerode-polkit – polkit authentication agent styled like the Windows 7
//! User Account Control credential prompt (dimmed "secure desktop").
//!
//! Registration and the agent interface are plain D-Bus (zbus).  The actual
//! password check is done by polkit's own setuid helper
//! (polkit-agent-helper-1), exactly like polkit-gnome/kde do internally.
use gtk::gdk::prelude::GdkContextExt;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use zbus::zvariant::{OwnedValue, Value};

use aerode::{config, sounds, theme, util};

const OBJ: &str = "/org/aerode/PolicyKit1/AuthenticationAgent";

struct Request {
    action_id: String,
    message: String,
    icon: String,
    details: HashMap<String, String>,
    cookie: String,
    users: Vec<String>,
    reply: async_channel::Sender<bool>,
}

struct Agent {
    tx: async_channel::Sender<Request>,
    cancel: async_channel::Sender<String>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> zbus::fdo::Result<()> {
        let mut users = vec![];
        for (kind, props) in identities {
            if kind == "unix-user" {
                if let Some(uid) = props.get("uid").and_then(|v| u32::try_from(v).ok()) {
                    if let Some(n) = user_name(uid) { users.push(n); }
                }
            } else if kind == "unix-group" {
                if let Some(gid) = props.get("gid").and_then(|v| u32::try_from(v).ok()) { users.extend(group_members(gid)); }
            }
        }
        // prefer the current user if they are allowed to authenticate
        let me = glib::user_name().to_string_lossy().into_owned();
        users.dedup();
        if let Some(i) = users.iter().position(|u| *u == me) { let u = users.remove(i); users.insert(0, u); }
        if users.is_empty() { users.push("root".into()); }
        let (rtx, rrx) = async_channel::bounded(1);
        self.tx.send(Request { action_id, message, icon: icon_name, details, cookie, users, reply: rtx }).await
            .map_err(|_| zbus::fdo::Error::Failed("ui gone".into()))?;
        match rrx.recv().await {
            Ok(true) => Ok(()),
            _ => Err(zbus::fdo::Error::Failed("org.freedesktop.PolicyKit1.Error.Cancelled".into())),
        }
    }

    async fn cancel_authentication(&self, cookie: String) {
        let _ = self.cancel.send(cookie).await;
    }
}

fn user_name(uid: u32) -> Option<String> {
    std::fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2] == uid.to_string()).then(|| f[0].to_string())
    }).or_else(|| util::output(&["id", "-nu", &uid.to_string()]).map(|s| s.trim().to_string()))
}

fn group_members(gid: u32) -> Vec<String> {
    util::output(&["getent", "group", &gid.to_string()])
        .and_then(|l| l.trim().rsplit(':').next().map(|m| m.split(',').filter(|s| !s.is_empty()).map(String::from).collect()))
        .unwrap_or_default()
}

fn helper_path() -> Option<&'static str> {
    ["/usr/lib/polkit-1/polkit-agent-helper-1", "/usr/libexec/polkit-agent-helper-1", "/usr/lib/policykit-1/polkit-agent-helper-1"]
        .into_iter().find(|p| std::path::Path::new(p).exists())
}

/// Run polkit's setuid helper. Returns Ok(true) on SUCCESS, Ok(false) on failure,
/// with any PAM error/info text collected for display.
fn authenticate(user: &str, cookie: &str, password: &str) -> (bool, String) {
    let Some(helper) = helper_path() else { return (false, "polkit-agent-helper-1 not found".into()) };
    let Ok(mut child) = Command::new(helper).arg(user).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
        return (false, "could not start the polkit helper".into());
    };
    let mut stdin = child.stdin.take().unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    // modern helpers read the cookie from stdin
    let _ = writeln!(stdin, "{cookie}");
    let mut msg = String::new();
    let mut ok = false;
    for line in out.lines().map_while(Result::ok) {
        if line.starts_with("PAM_PROMPT_ECHO_OFF") || line.starts_with("PAM_PROMPT_ECHO_ON") {
            let _ = writeln!(stdin, "{password}");
        } else if let Some(t) = line.strip_prefix("PAM_ERROR_MSG ") {
            msg = t.to_string();
        } else if let Some(t) = line.strip_prefix("PAM_TEXT_INFO ") {
            msg = t.to_string();
        } else if line == "SUCCESS" {
            ok = true;
            break;
        } else if line == "FAILURE" {
            break;
        }
    }
    drop(stdin);
    let _ = child.wait();
    (ok, msg)
}

/// Capture the current screen, darkened – the UAC "secure desktop" look.
fn dimmed_backdrop() -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let root = gdk::Screen::default()?.root_window()?;
    let (w, h) = (root.width(), root.height());
    let surf = gtk::cairo::ImageSurface::create(gtk::cairo::Format::Rgb24, w, h).ok()?;
    {
        let cr = gtk::cairo::Context::new(&surf).ok()?;
        cr.set_source_window(&root, 0.0, 0.0);
        cr.paint().ok()?;
    }
    gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h)
}

fn program_name(r: &Request) -> String {
    r.details.get("polkit.message").cloned().filter(|s| !s.is_empty())
        .or_else(|| r.details.get("program").map(|p| p.rsplit('/').next().unwrap_or(p).to_string()))
        .unwrap_or_else(|| r.message.clone())
}

fn show(req: Request, cancel_rx: async_channel::Receiver<String>) {
    let geo = util::primary_geometry();
    let backdrop = dimmed_backdrop();
    let win = gtk::Window::new(gtk::WindowType::Popup);
    win.set_app_paintable(true);
    win.move_(0, 0);
    if let Some(root) = gdk::Screen::default().and_then(|s| s.root_window()) { win.set_size_request(root.width(), root.height()); }
    win.connect_draw(move |_, cr| {
        if let Some(b) = &backdrop { cr.set_source_pixbuf(b, 0.0, 0.0); cr.paint().ok(); }
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        cr.paint().ok();
        glib::Propagation::Proceed
    });
    let fixed = gtk::Fixed::new();
    win.add(&fixed);

    // ---- the dialog
    let dlg = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dlg.style_context().add_class("uac");
    dlg.set_size_request(456, -1);
    // caption
    let cap = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    cap.style_context().add_class("uac-caption");
    cap.pack_start(&util::image(util::first_icon(&["aerode-uac-shield", "security-high", "dialog-password"]), 16), false, false, 0);
    cap.pack_start(&gtk::Label::new(Some("User Account Control")), false, false, 0);
    dlg.pack_start(&cap, false, false, 0);
    // banner
    let banner = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    banner.style_context().add_class("uac-banner");
    banner.pack_start(&util::image(util::first_icon(&["aerode-uac-shield", "security-high", "dialog-password"]), 32), false, false, 0);
    let q = gtk::Label::new(Some("Do you want to allow the following program to make changes to this computer?"));
    q.set_line_wrap(true);
    q.set_xalign(0.0);
    q.set_max_width_chars(46);
    q.style_context().add_class("uac-question");
    banner.pack_start(&q, true, true, 0);
    dlg.pack_start(&banner, false, false, 0);
    // body
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.style_context().add_class("uac-body");
    let prog = gtk::Grid::new();
    prog.set_column_spacing(10);
    prog.set_row_spacing(2);
    prog.attach(&util::image(if req.icon.is_empty() { "application-x-executable" } else { &req.icon }, 32), 0, 0, 1, 3);
    let rows = [
        ("Program name:", program_name(&req)),
        ("Verified publisher:", req.details.get("polkit.caller-pid").map(|_| "Linux system".to_string()).unwrap_or_else(|| "Linux system".into())),
        ("File origin:", "Hard drive on this computer".to_string()),
    ];
    for (i, (k, v)) in rows.iter().enumerate() {
        let kl = gtk::Label::new(Some(k));
        kl.set_xalign(0.0);
        kl.style_context().add_class("uac-key");
        let vl = gtk::Label::new(Some(v));
        vl.set_xalign(0.0);
        vl.set_line_wrap(true);
        vl.set_max_width_chars(34);
        prog.attach(&kl, 1, i as i32, 1, 1);
        prog.attach(&vl, 2, i as i32, 1, 1);
    }
    body.pack_start(&prog, false, false, 0);
    let msg = gtk::Label::new(Some("To continue, type an administrator password, and then click Yes."));
    msg.set_xalign(0.0);
    body.pack_start(&msg, false, false, 4);
    // credential tile
    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    tile.style_context().add_class("uac-tile");
    let face = config::home().join(".face");
    let av = if face.exists() { Some(face) } else { config::data_file("avatar.png") };
    let avimg = av.and_then(|p| gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, 48, 48, false).ok())
        .map(|p| gtk::Image::from_pixbuf(Some(&p))).unwrap_or_else(|| util::image("avatar-default", 48));
    tile.pack_start(&avimg, false, false, 0);
    let tv = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let user_combo = gtk::ComboBoxText::new();
    for u in &req.users { user_combo.append_text(u); }
    user_combo.set_active(Some(0));
    if req.users.len() == 1 {
        let l = gtk::Label::new(Some(&req.users[0]));
        l.set_xalign(0.0);
        l.style_context().add_class("uac-user");
        tv.pack_start(&l, false, false, 0);
    } else {
        tv.pack_start(&user_combo, false, false, 0);
    }
    let pw = gtk::Entry::new();
    pw.set_visibility(false);
    pw.set_placeholder_text(Some("Password"));
    pw.set_width_chars(28);
    tv.pack_start(&pw, false, false, 0);
    let err = gtk::Label::new(None);
    err.style_context().add_class("uac-error");
    err.set_xalign(0.0);
    tv.pack_start(&err, false, false, 0);
    tile.pack_start(&tv, true, true, 0);
    body.pack_start(&tile, false, false, 0);
    // details
    let exp = gtk::Expander::new(Some("Show details"));
    let mut dtxt = format!("Action: {}\n{}", req.action_id, req.message);
    for (k, v) in &req.details { dtxt += &format!("\n{k}: {v}"); }
    let dl = gtk::Label::new(Some(&dtxt));
    dl.set_xalign(0.0);
    dl.set_selectable(true);
    dl.set_line_wrap(true);
    exp.add(&dl);
    body.pack_start(&exp, false, false, 0);
    dlg.pack_start(&body, false, false, 0);
    // footer
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.style_context().add_class("uac-foot");
    let yes = gtk::Button::with_label("Yes");
    let no = gtk::Button::with_label("No");
    yes.set_size_request(73, -1);
    no.set_size_request(73, -1);
    let link = gtk::Label::new(Some("Change when these notifications appear"));
    link.style_context().add_class("cp-link");
    foot.pack_start(&link, false, false, 0);
    foot.pack_end(&no, false, false, 0);
    foot.pack_end(&yes, false, false, 0);
    dlg.pack_start(&foot, false, false, 0);

    dlg.show_all();
    let (_, nat) = dlg.preferred_size();
    fixed.put(&dlg, geo.x() + (geo.width() - 456) / 2, geo.y() + (geo.height() - nat.height) / 2);
    win.show_all();
    // grab input like the secure desktop does
    if let Some(seat) = gdk::Display::default().and_then(|d| d.default_seat()) {
        if let Some(gw) = win.window() {
            let _ = seat.grab(&gw, gdk::SeatCapabilities::ALL, true, None, None, None);
        }
    }
    pw.grab_focus();
    sounds::play(sounds::Event::UserAccountControl);

    let reply = req.reply.clone();
    let done = std::rc::Rc::new(std::cell::Cell::new(false));
    let finish = {
        let (win, done, reply) = (win.clone(), done.clone(), reply.clone());
        move |ok: bool| {
            if done.replace(true) { return; }
            if let Some(seat) = gdk::Display::default().and_then(|d| d.default_seat()) { seat.ungrab(); }
            win.close();
            let _ = reply.send_blocking(ok);
        }
    };
    let finish = std::rc::Rc::new(finish);
    {
        let f = finish.clone();
        no.connect_clicked(move |_| f(false));
    }
    {
        let (f, pwc, err, cookie, users, combo) = (finish.clone(), pw.clone(), err.clone(), req.cookie.clone(), req.users.clone(), user_combo.clone());
        let yes2 = yes.clone();
        let attempt = std::rc::Rc::new(move || {
            let user = if users.len() == 1 { users[0].clone() } else { combo.active_text().map(|s| s.to_string()).unwrap_or_default() };
            let pass = pwc.text().to_string();
            yes2.set_sensitive(false);
            pwc.set_sensitive(false);
            let (tx, rx) = std::sync::mpsc::channel();
            let cookie = cookie.clone();
            std::thread::spawn(move || { let _ = tx.send(authenticate(&user, &cookie, &pass)); });
            let (f, pw, err, yes2) = (f.clone(), pwc.clone(), err.clone(), yes2.clone());
            glib::timeout_add_local(std::time::Duration::from_millis(50), move || match rx.try_recv() {
                Ok((true, _)) => { f(true); glib::ControlFlow::Break }
                Ok((false, m)) => {
                    err.set_text(if m.is_empty() { "The user name or password is incorrect." } else { &m });
                    pw.set_text("");
                    pw.set_sensitive(true);
                    yes2.set_sensitive(true);
                    pw.grab_focus();
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => glib::ControlFlow::Break,
            });
        });
        let a = attempt.clone();
        yes.connect_clicked(move |_| a());
        pw.connect_activate(move |_| attempt());
    }
    {
        let f = finish.clone();
        win.connect_key_press_event(move |_, e| {
            if e.keyval() == gdk::keys::constants::Escape { f(false); }
            glib::Propagation::Proceed
        });
    }
    // polkit may cancel (e.g. the caller went away)
    let cookie = req.cookie.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        if done.get() { return glib::ControlFlow::Break; }
        while let Ok(c) = cancel_rx.try_recv() { if c == cookie { finish(false); return glib::ControlFlow::Break; } }
        glib::ControlFlow::Continue
    });
}

const CSS: &str = r#"
.uac { background-color: #ffffff; border: 1px solid #3c6d9e; box-shadow: 0 0 0 6px rgba(160,200,240,0.55); }
.uac-caption { background-image: linear-gradient(to bottom, #c9dcf2, #a7c3e6); padding: 5px 8px; border-bottom: 1px solid #8eaacf; }
.uac-caption label { color: #000; }
.uac-banner { background-image: linear-gradient(to bottom, #f6d36b, #eab73b); padding: 10px 12px; border-bottom: 1px solid #c79c2c; }
.uac-question { font-size: 12pt; color: #000; }
.uac-body { padding: 12px 16px; background-color: #fff; }
.uac-key { color: #000; }
.uac-tile { background-image: linear-gradient(to bottom, #f5f9fe, #e3eefb); border: 1px solid #a8c3e3; border-radius: 3px; padding: 8px; }
.uac-user { font-weight: bold; }
.uac-error { color: #c00; }
.uac-foot { background-color: #f0f0f0; border-top: 1px solid #dfdfdf; padding: 10px 12px; }
"#;

fn main() {
    gtk::init().expect("gtk");
    theme::init_app();
    theme::load_css(CSS);
    let (tx, rx) = async_channel::unbounded::<Request>();
    let (ctx, crx) = async_channel::unbounded::<String>();
    if std::env::args().any(|a| a == "--demo") {
        // preview the prompt without polkit (no authentication is performed)
        let (rtx, _rrx) = async_channel::bounded(1);
        let mut details = HashMap::new();
        details.insert("program".to_string(), "/usr/bin/pacman".to_string());
        show(Request { action_id: "org.archlinux.pkexec.pacman".into(), message: "Authentication is required to run pacman".into(),
            icon: "system-software-install".into(), details, cookie: String::new(),
            users: vec![glib::user_name().to_string_lossy().into_owned()], reply: rtx }, crx);
        gtk::main();
        return;
    }
    std::thread::spawn(move || {
        let run = || -> zbus::Result<()> {
            let conn = zbus::blocking::Connection::system()?;
            conn.object_server().at(OBJ, Agent { tx, cancel: ctx })?;
            let session = std::env::var("XDG_SESSION_ID").ok().or_else(|| {
                util::output(&["loginctl", "show-session", "auto", "-p", "Id", "--value"]).map(|s| s.trim().to_string())
            }).unwrap_or_default();
            let mut subj: HashMap<&str, Value> = HashMap::new();
            subj.insert("session-id", Value::from(session.as_str()));
            let proxy = zbus::blocking::Proxy::new(&conn, "org.freedesktop.PolicyKit1", "/org/freedesktop/PolicyKit1/Authority", "org.freedesktop.PolicyKit1.Authority")?;
            proxy.call_method("RegisterAuthenticationAgent", &(("unix-session", subj), "en_US.UTF-8", OBJ))?;
            eprintln!("aerode-polkit: registered for session {session}");
            loop { std::thread::park(); }
        };
        if let Err(e) = run() { eprintln!("aerode-polkit: {e}"); std::process::exit(1); }
    });
    // UI loop: one prompt at a time
    let busy = std::rc::Rc::new(std::cell::Cell::new(false));
    let queue: std::rc::Rc<std::cell::RefCell<Vec<Request>>> = Default::default();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        while let Ok(r) = rx.try_recv() { queue.borrow_mut().push(r); }
        if !busy.get() && !queue.borrow().is_empty() {
            let r = queue.borrow_mut().remove(0);
            busy.set(true);
            let b = busy.clone();
            let (otx, orx) = async_channel::bounded::<bool>(1);
            let orig = r.reply.clone();
            let req = Request { reply: otx, ..r };
            show(req, crx.clone());
            glib::timeout_add_local(std::time::Duration::from_millis(100), move || match orx.try_recv() {
                Ok(v) => { let _ = orig.send_blocking(v); b.set(false); glib::ControlFlow::Break }
                Err(async_channel::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(_) => { b.set(false); glib::ControlFlow::Break }
            });
        }
        glib::ControlFlow::Continue
    });
    gtk::main();
}
