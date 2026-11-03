//! Helpers shared by the applets in this group: background work without freezing the window,
//! privileged commands (pkexec → aerode's UAC prompt), Windows 7 command links, wizards, group
//! boxes, and applets that Windows opens as their own window.
use gtk::prelude::*;
use gtk::glib;
use std::rc::Rc;

use aerode::util;
use crate::Cp;

const CSS: &str = r#"
.xt-instr { color: #003399; font-size: 12pt; }
.xt-white { background-color: #ffffff; }
.xt-cmdarea { background-color: #f0f0f0; border-top: 1px solid #dfdfdf; padding: 10px 11px; }
button.xt-cmdlink { background: none; border: 1px solid transparent; box-shadow: none; border-radius: 3px; padding: 6px 8px; }
button.xt-cmdlink:hover { background-image: linear-gradient(to bottom, #fcfdff, #e3f0fc); border-color: #c6dcf3; }
button.xt-cmdlink label.xt-cl-main { color: #151c55; font-size: 11pt; }
button.xt-cmdlink label.xt-cl-note { color: #151c55; }
button.xt-cmdlink:disabled label { color: #8a8a8a; }
.xt-band { background-image: linear-gradient(to bottom, #fbfcfe, #eaf0f8); border: 1px solid #dde6f2; border-radius: 2px; padding: 4px 8px; }
.xt-band label.xt-band-title { color: #1e3287; font-size: 10pt; }
.xt-grey { color: #6d6d6d; }
.xt-strong { font-weight: bold; }
.xt-name { font-size: 11pt; }
.xt-card { background-color: #ffffff; border: 1px solid #d5dfe5; border-radius: 3px; }
button.xt-cred-head { background: none; border: 1px solid transparent; box-shadow: none; border-radius: 2px; padding: 3px 4px; }
button.xt-cred-head:hover { background-image: linear-gradient(to bottom, #f7fbff, #e5f1fc); border-color: #cfe3f8; }
"#;

pub fn css() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if !DONE.with(|d| d.replace(true)) { aerode::theme::load_css(CSS); }
}

// ------------------------------------------------------------------ background work

/// Run `work` in a thread and hand its result to `done` on the GTK main loop.
pub fn background<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || { let _ = tx.send_blocking(work()); });
    glib::MainContext::default().spawn_local(async move { if let Ok(v) = rx.recv().await { done(v); } });
}

/// What a finished command printed.
#[derive(Clone, Debug, Default)]
pub struct Out { pub ok: bool, pub code: i32, pub stdout: String, pub stderr: String }

impl Out {
    /// pkexec: the user dismissed the UAC prompt or typed a wrong password
    pub fn cancelled(&self) -> bool { self.code == 126 || self.code == 127 && self.stderr.contains("Not authorized") }
    pub fn text(&self) -> String { format!("{}\n{}", self.stdout.trim(), self.stderr.trim()).trim().to_string() }
}

pub fn sv(a: &[&str]) -> Vec<String> { a.iter().map(|s| s.to_string()).collect() }

/// Run a command to completion (worker threads only). A missing program is a failure with code 127.
pub fn exec(argv: &[String], stdin: Option<&str>) -> Out {
    use std::process::{Command, Stdio};
    if argv.is_empty() || !util::which(&argv[0]) {
        return Out { code: 127, stderr: format!("{}: command not found", argv.first().map(|s| s.as_str()).unwrap_or("")), ..Default::default() };
    }
    let mut c = Command::new(&argv[0]);
    c.args(&argv[1..]).env("LC_ALL", "C").stdout(Stdio::piped()).stderr(Stdio::piped())
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    match c.spawn() {
        Ok(mut ch) => {
            if let (Some(s), Some(mut i)) = (stdin, ch.stdin.take()) { use std::io::Write; let _ = i.write_all(s.as_bytes()); }
            match ch.wait_with_output() {
                Ok(o) => Out { ok: o.status.success(), code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into_owned(), stderr: String::from_utf8_lossy(&o.stderr).into_owned() },
                Err(e) => Out { code: -1, stderr: e.to_string(), ..Default::default() },
            }
        }
        Err(e) => Out { code: 127, stderr: e.to_string(), ..Default::default() },
    }
}

pub fn exec_async(argv: Vec<String>, stdin: Option<String>, cb: impl FnOnce(Out) + 'static) {
    background(move || exec(&argv, stdin.as_deref()), cb);
}

/// `AERODE_DRY_RUN=1`: commands that change the system (repairs, pkexec, deleting things) are only
/// printed and reported as done – for trying the dialogs on a machine that must stay untouched.
pub fn dry_run() -> bool { std::env::var_os("AERODE_DRY_RUN").is_some() }

/// A command that changes something (worker threads only).
pub fn change_blocking(argv: &[String], stdin: Option<&str>) -> Out {
    if dry_run() {
        eprintln!("[aerode dry run] {}", argv.join(" "));
        std::thread::sleep(std::time::Duration::from_millis(300));
        return Out { ok: true, ..Default::default() };
    }
    exec(argv, stdin)
}

pub fn change(argv: Vec<String>, stdin: Option<String>, cb: impl FnOnce(Out) + 'static) {
    background(move || change_blocking(&argv, stdin.as_deref()), cb);
}

/// A privileged change through polkit (aerode's UAC prompt), without freezing the window.
pub fn admin_change(argv: &[&str], cb: impl FnOnce(Out) + 'static) {
    let mut a = vec!["pkexec".to_string()];
    a.extend(argv.iter().map(|s| s.to_string()));
    change(a, None, cb);
}

/// A privileged read (listing snapshots…): runs for real even in a dry run.
pub fn admin_read(argv: &[&str], cb: impl FnOnce(Out) + 'static) {
    let mut a = vec!["pkexec".to_string()];
    a.extend(argv.iter().map(|s| s.to_string()));
    exec_async(a, None, cb);
}

// ------------------------------------------------------------------ widgets

/// The Windows 7 "main instruction" text of wizards and dialogs.
pub fn instr(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.style_context().add_class("xt-instr");
    l
}

pub fn label(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(70);
    l
}

pub fn grey(t: &str) -> gtk::Label {
    let l = label(t);
    l.style_context().add_class("xt-grey");
    l
}

pub fn esc(s: &str) -> String { glib::markup_escape_text(s).to_string() }

/// The green arrow in front of a command link.
pub fn arrow() -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_size_request(16, 16);
    d.set_valign(gtk::Align::Start);
    d.set_margin_top(3);
    d.connect_draw(|_, cr| {
        let g = gtk::cairo::LinearGradient::new(0.0, 2.0, 0.0, 14.0);
        g.add_color_stop_rgb(0.0, 0.42, 0.80, 0.30);
        g.add_color_stop_rgb(1.0, 0.10, 0.52, 0.10);
        cr.move_to(2.0, 6.0); cr.line_to(8.0, 6.0); cr.line_to(8.0, 2.5); cr.line_to(14.5, 8.0);
        cr.line_to(8.0, 13.5); cr.line_to(8.0, 10.0); cr.line_to(2.0, 10.0); cr.close_path();
        let _ = cr.set_source(&g);
        let _ = cr.fill_preserve();
        cr.set_source_rgb(0.06, 0.38, 0.06);
        cr.set_line_width(0.8);
        let _ = cr.stroke();
        glib::Propagation::Stop
    });
    d
}

/// A Windows 7 command link: green arrow (or an icon), a line of text and an optional note.
pub fn cmd_link(icon: Option<&str>, main: &str, note: &str, f: impl Fn() + 'static) -> gtk::Button {
    css();
    let b = gtk::Button::new();
    b.style_context().add_class("xt-cmdlink");
    b.set_relief(gtk::ReliefStyle::None);
    b.set_halign(gtk::Align::Fill);
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    match icon {
        Some(i) => { let im = util::image(i, 32); im.set_valign(gtk::Align::Start); h.pack_start(&im, false, false, 0); }
        None => h.pack_start(&arrow(), false, false, 0),
    }
    let v = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let a = gtk::Label::new(Some(main));
    a.set_xalign(0.0);
    a.set_line_wrap(true);
    a.style_context().add_class("xt-cl-main");
    v.pack_start(&a, false, false, 0);
    if !note.is_empty() {
        let n = gtk::Label::new(Some(note));
        n.set_xalign(0.0);
        n.set_line_wrap(true);
        n.set_max_width_chars(64);
        n.style_context().add_class("xt-cl-note");
        v.pack_start(&n, false, false, 0);
    }
    h.pack_start(&v, true, true, 0);
    b.add(&h);
    b.connect_clicked(move |_| f());
    b
}

pub fn shield() -> &'static str { util::first_icon(&["aerode-uac-shield", "security-high", "dialog-password"]) }

/// A push button with the UAC shield (the action asks for an administrator password).
pub fn shield_button(text: &str) -> gtk::Button {
    let b = gtk::Button::with_mnemonic(text);
    b.set_image(Some(&util::image(shield(), 16)));
    b.set_always_show_image(true);
    b
}

/// A property-sheet group box ("Home page", "Proxy server"…).
pub fn group(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    let f = gtk::Frame::new(Some(title));
    let pad = gtk::Box::new(gtk::Orientation::Vertical, 6);
    pad.set_border_width(8);
    pad.pack_start(child, true, true, 0);
    f.add(&pad);
    f
}

/// `widget.tap(|w| w.set_…())` for one-off tweaks inside an expression.
pub trait Tap: Sized { fn tap(self, f: impl FnOnce(&Self)) -> Self { f(&self); self } }
impl<T> Tap for T {}

pub fn hbox(sp: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, sp) }
pub fn vbox(sp: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, sp) }

/// Buttons right-aligned in a row (OK / Cancel / Apply, Save changes / Cancel…).
pub fn button_row(bs: &[&gtk::Button]) -> gtk::Box {
    let h = hbox(6);
    for b in bs.iter().rev() { b.set_size_request(75, -1); h.pack_end(*b, false, false, 0); }
    h
}

/// A section header with a link on its right (Credential Manager style).
pub fn band(title: &str, right: Option<&gtk::Widget>) -> gtk::Box {
    css();
    let h = hbox(8);
    h.style_context().add_class("xt-band");
    let l = gtk::Label::new(Some(title));
    l.style_context().add_class("xt-band-title");
    l.set_xalign(0.0);
    h.pack_start(&l, true, true, 0);
    if let Some(r) = right { h.pack_end(r, false, false, 0); }
    h
}

pub fn scrolled(child: &impl IsA<gtk::Widget>, h: i32) -> gtk::ScrolledWindow {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    sw.set_min_content_height(h);
    sw.add(child);
    sw
}

/// A TreeView over string columns; `titles` names the visible ones (the rest stay hidden).
pub fn table(cols: usize, titles: &[&str]) -> (gtk::TreeView, gtk::ListStore) {
    let store = gtk::ListStore::new(&vec![glib::Type::STRING; cols]);
    let tv = gtk::TreeView::with_model(&store);
    for (i, t) in titles.iter().enumerate() {
        let c = gtk::TreeViewColumn::new();
        c.set_title(t);
        let r = gtk::CellRendererText::new();
        r.set_property("ellipsize", gtk::pango::EllipsizeMode::End);
        TreeViewColumnExt::pack_start(&c, &r, true);
        TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
        c.set_resizable(true);
        c.set_expand(i == 0);
        tv.append_column(&c);
    }
    (tv, store)
}

/// Let column `i` take the spare width instead of the first one.
pub fn expand_column(tv: &gtk::TreeView, i: usize) {
    for (n, c) in tv.columns().iter().enumerate() { c.set_expand(n == i); }
}

/// "Pacific Standard Time"-like name of the time zone: the tz database name and its offset.
pub fn time_zone() -> String {
    let now = chrono::Local::now();
    let name = std::fs::read_link("/etc/localtime").ok()
        .and_then(|p| p.to_string_lossy().split("zoneinfo/").nth(1).map(|s| s.replace('_', " ")))
        .unwrap_or_else(|| now.format("%Z").to_string());
    format!("{name} (UTC{})", now.format("%:z"))
}

pub fn selected(tv: &gtk::TreeView, col: i32) -> Option<String> {
    tv.selection().selected().and_then(|(m, it)| m.value(&it, col).get::<String>().ok())
}

/// Modal message box (its sound comes from `util::message_dialog_sounds`, like Windows).
pub fn message(parent: Option<&gtk::Window>, kind: gtk::MessageType, title: &str, primary: &str, secondary: &str) {
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL, kind, gtk::ButtonsType::Ok, primary);
    d.set_title(title);
    if !secondary.is_empty() { d.set_secondary_text(Some(secondary)); }
    d.run();
    d.close();
}

/// Yes/No question; `no` is the default button, so Enter never confirms something destructive.
pub fn ask(parent: Option<&gtk::Window>, kind: gtk::MessageType, title: &str, primary: &str, secondary: &str, yes: &str, no: &str) -> bool {
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL, kind, gtk::ButtonsType::None, primary);
    d.set_title(title);
    if !secondary.is_empty() { d.set_secondary_text(Some(secondary)); }
    d.add_button(yes, gtk::ResponseType::Yes);
    d.add_button(no, gtk::ResponseType::No);
    d.set_default_response(gtk::ResponseType::No);
    let r = d.run();
    d.close();
    r == gtk::ResponseType::Yes
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mark { Ok, Bad, Warn, Info }

/// The small status icons of troubleshooters (green check, red cross, yellow warning, blue i).
pub fn mark(m: Mark) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_size_request(16, 16);
    d.set_valign(gtk::Align::Center);
    d.connect_draw(move |_, cr| {
        use std::f64::consts::PI;
        match m {
            Mark::Ok | Mark::Bad | Mark::Info => {
                let (a, b) = match m { Mark::Ok => ((0.45, 0.80, 0.35), (0.12, 0.52, 0.10)), Mark::Bad => ((0.95, 0.42, 0.36), (0.72, 0.08, 0.06)), _ => ((0.45, 0.65, 0.95), (0.10, 0.32, 0.75)) };
                let g = gtk::cairo::RadialGradient::new(6.0, 5.0, 1.0, 8.0, 8.0, 8.0);
                g.add_color_stop_rgb(0.0, a.0, a.1, a.2);
                g.add_color_stop_rgb(1.0, b.0, b.1, b.2);
                cr.arc(8.0, 8.0, 7.2, 0.0, 2.0 * PI);
                let _ = cr.set_source(&g);
                let _ = cr.fill();
                cr.set_source_rgb(1.0, 1.0, 1.0);
                cr.set_line_width(2.0);
                match m {
                    Mark::Ok => { cr.move_to(4.5, 8.2); cr.line_to(7.0, 10.8); cr.line_to(11.5, 5.2); let _ = cr.stroke(); }
                    Mark::Bad => { cr.move_to(5.3, 5.3); cr.line_to(10.7, 10.7); cr.move_to(10.7, 5.3); cr.line_to(5.3, 10.7); let _ = cr.stroke(); }
                    _ => { cr.rectangle(7.0, 7.0, 2.0, 5.0); cr.rectangle(7.0, 3.6, 2.0, 2.0); let _ = cr.fill(); }
                }
            }
            Mark::Warn => {
                cr.move_to(8.0, 1.0); cr.line_to(15.2, 14.5); cr.line_to(0.8, 14.5); cr.close_path();
                let g = gtk::cairo::LinearGradient::new(0.0, 1.0, 0.0, 14.5);
                g.add_color_stop_rgb(0.0, 1.0, 0.88, 0.35);
                g.add_color_stop_rgb(1.0, 0.93, 0.66, 0.05);
                let _ = cr.set_source(&g);
                let _ = cr.fill_preserve();
                cr.set_source_rgb(0.62, 0.44, 0.02);
                cr.set_line_width(0.8);
                let _ = cr.stroke();
                cr.set_source_rgb(0.1, 0.1, 0.1);
                cr.rectangle(7.1, 5.0, 1.8, 5.2); cr.rectangle(7.1, 11.3, 1.8, 1.8);
                let _ = cr.fill();
            }
        }
        glib::Propagation::Stop
    });
    d
}

/// Windows 7 short date and time: 10/14/2023 10:00:01 AM.
pub fn win_time(t: chrono::DateTime<chrono::Local>) -> String { t.format("%-m/%-d/%Y %-I:%M:%S %p").to_string() }
pub fn win_date(t: chrono::DateTime<chrono::Local>) -> String { t.format("%-m/%-d/%Y").to_string() }

pub fn unix_time(secs: i64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(secs, 0).single()
}

// ------------------------------------------------------------------ wizards

/// A wizard window: glass strip with the round back button (Aero wizard) or a classic one with
/// "< Back"; a white page; the grey command area with the buttons at the bottom.
pub struct Wizard {
    pub win: gtk::Window,
    pub back: gtk::Button,
    pub body: gtk::Box,
    pub foot: gtk::Box,
    pub buttons: gtk::Box,
}

impl Wizard {
    pub fn page(&self, w: &impl IsA<gtk::Widget>) {
        for c in self.body.children() { self.body.remove(&c); }
        self.body.pack_start(w, true, true, 0);
        self.body.show_all();
    }
    pub fn set_buttons(&self, bs: &[&gtk::Button]) {
        for c in self.buttons.children() { self.buttons.remove(&c); }
        for b in bs { b.set_size_request(75, -1); self.buttons.pack_start(*b, false, false, 0); }
        self.buttons.show_all();
    }
    pub fn set_foot(&self, w: Option<&gtk::Widget>) {
        for c in self.foot.children() { self.foot.remove(&c); }
        if let Some(w) = w { self.foot.pack_start(w, false, false, 0); }
        self.foot.show_all();
    }
}

pub fn glass_strip() -> gtk::Box {
    let header = hbox(6);
    header.style_context().add_class("exp-header");
    header.style_context().add_class("cp-header");
    // the dialog frame's glass continued into the window (same as the Control Panel header)
    header.connect_draw(|h, cr| {
        aerode::glass::paint_widget_glass(h, cr);
        glib::Propagation::Proceed
    });
    header
}

pub fn wizard(title: &str, icon: &str, parent: Option<&gtk::Window>, size: (i32, i32), aero: bool) -> Rc<Wizard> {
    css();
    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title(title);
    win.set_icon_name(Some(util::first_icon(&[icon, "preferences-system"])));
    win.set_default_size(size.0, size.1);
    win.set_resizable(false);
    win.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    if let Some(p) = parent { win.set_transient_for(Some(p)); win.set_position(gtk::WindowPosition::CenterOnParent); } else { win.set_position(gtk::WindowPosition::Center); }
    let root = vbox(0);
    let back = if aero {
        let b = gtk::Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
        b.style_context().add_class("nav");
        aerode::shellstyle::nav_button(&b, 1);
        let strip = glass_strip();
        strip.pack_start(&b, false, false, 0);
        // the strip is the frame's glass continued: translucent under a compositor, redrawn when
        // the window moves or gains/loses focus (like the Control Panel header)
        aerode::glass::enable_rgba(&win);
        { let s = strip.clone(); win.connect_configure_event(move |_, _| { s.queue_draw(); false }); }
        {
            let s = strip.clone();
            win.connect_window_state_event(move |_, e| {
                if e.changed_mask().intersects(gtk::gdk::WindowState::FOCUSED | gtk::gdk::WindowState::MAXIMIZED) { s.queue_draw(); }
                glib::Propagation::Proceed
            });
        }
        root.pack_start(&strip, false, false, 0);
        b
    } else { gtk::Button::with_mnemonic("< _Back") };
    let body = vbox(10);
    body.style_context().add_class("xt-white");
    body.set_border_width(0);
    let pad = vbox(0);
    pad.style_context().add_class("xt-white");
    pad.set_margin_start(0);
    let inner = vbox(0);
    inner.set_margin_start(if aero { 34 } else { 20 });
    inner.set_margin_end(24);
    inner.set_margin_top(14);
    inner.set_margin_bottom(10);
    inner.pack_start(&body, true, true, 0);
    pad.pack_start(&inner, true, true, 0);
    root.pack_start(&pad, true, true, 0);
    let cmd = hbox(6);
    cmd.style_context().add_class("xt-cmdarea");
    let foot = hbox(6);
    cmd.pack_start(&foot, true, true, 0);
    let buttons = hbox(6);
    cmd.pack_end(&buttons, false, false, 0);
    root.pack_start(&cmd, false, false, 0);
    win.add(&root);
    Rc::new(Wizard { win, back, body, foot, buttons })
}

// ------------------------------------------------------------------ navigation

/// Windows opens some applets as their own window (Internet Properties, Color Management,
/// Mobility Center) over the Control Panel, which stays where it was. `aerode-control <id>` (like
/// inetcpl.cpl) shows only that window.
pub fn standalone(cp: &Rc<Cp>, make: impl FnOnce(Option<&gtk::Window>) -> gtk::Window) -> gtk::Widget {
    let direct = cp.history.borrow().len() <= 1;
    let parent: gtk::Window = cp.win.clone().upcast();
    let w = make(if direct { None } else { Some(&parent) });
    if let Some(app) = cp.win.application() { w.set_application(Some(&app)); }
    if direct {
        // nothing else was asked for: hide the Control Panel and quit with the window
        cp.win.set_opacity(0.0);
        let cw = cp.win.clone();
        glib::idle_add_local_once(move || { cw.hide(); cw.set_opacity(1.0); });
        let cw = cp.win.clone();
        w.connect_destroy(move |_| if !cw.is_visible() { cw.close(); });
    } else {
        // stay on the page the applet was opened from
        let me = cp.clone();
        glib::idle_add_local_once(move || {
            let prev = { let mut h = me.history.borrow_mut(); h.pop(); h.pop() };
            me.open(&prev.unwrap_or_default());
        });
    }
    w.show_all();
    w.present();
    gtk::Box::new(gtk::Orientation::Vertical, 0).upcast()
}

/// Task-pane links of a page: they go below "Control Panel Home", which the Control Panel adds
/// after the page is built.
pub fn side_later(cp: &Rc<Cp>, f: impl FnOnce(&Rc<Cp>) + 'static) {
    let me = cp.clone();
    glib::idle_add_local_once(move || { f(&me); me.side.show_all(); });
}

/// A sub page of an applet ("Advanced recovery methods"…): replaces the content, extends the
/// address bar, and Back returns to the applet's main page.
pub fn subpage(cp: &Rc<Cp>, holder: &gtk::Box, crumb: &str, w: &impl IsA<gtk::Widget>) {
    let cur = cp.history.borrow().last().cloned().unwrap_or_default();
    cp.history.borrow_mut().push(cur);
    let base = cp.crumb.text().to_string();
    cp.crumb.set_text(&format!("{base} ▸ {crumb}"));
    for c in holder.children() { holder.remove(&c); }
    holder.pack_start(w, true, true, 0);
    holder.show_all();
}

/// Replace a placeholder's content (for pages filled in when background work is done).
pub fn fill(holder: &gtk::Box, w: &impl IsA<gtk::Widget>) {
    for c in holder.children() { holder.remove(&c); }
    holder.pack_start(w, true, true, 0);
    holder.show_all();
}

/// "Please wait…" placeholder shown while a page loads.
pub fn loading(text: &str) -> gtk::Box {
    let h = hbox(8);
    let sp = gtk::Spinner::new();
    sp.start();
    h.pack_start(&sp, false, false, 0);
    h.pack_start(&grey(text), false, false, 0);
    h
}
