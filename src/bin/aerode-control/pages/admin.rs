//! System and Security / administrative applets.
//!
//! Control Panel pages: Action Center, Windows Update, Windows Firewall, Backup and Restore,
//! BitLocker Drive Encryption, Administrative Tools, Performance Information and Tools.
//! Like on Windows 7 the MMC consoles (Event Viewer, Task Scheduler, Disk Management, Device
//! Manager, Computer Management) and System Information (msinfo32) open in their own window:
//! `aerode-control eventvwr` shows only that window, and clicking one inside Control Panel starts
//! it as its own process (mmc.exe on Windows) while Control Panel stays where it was.
use gtk::prelude::*;
use gtk::{gdk_pixbuf::Pixbuf, glib};
use std::rc::Rc;

use super::super::Cp;
use aerode::util;

pub(crate) mod actioncenter;
mod backup;
mod bitlocker;
mod cleanup;
mod devmgr;
mod diskmgmt;
mod eventvwr;
mod firewall;
mod mmc;
mod perfinfo;
mod sysinfo;
mod taskschd;
mod tools;
mod update;

pub fn page_actioncenter(cp: &Rc<Cp>) -> gtk::Widget { css(); actioncenter::page(cp) }
pub fn page_firewall(cp: &Rc<Cp>) -> gtk::Widget { css(); firewall::page(cp) }
pub fn page_update(cp: &Rc<Cp>) -> gtk::Widget { css(); update::page(cp) }
pub fn page_backup(cp: &Rc<Cp>) -> gtk::Widget { css(); backup::page(cp) }
pub fn page_bitlocker(cp: &Rc<Cp>) -> gtk::Widget { css(); bitlocker::page(cp) }
pub fn page_admintools(cp: &Rc<Cp>) -> gtk::Widget { css(); tools::page(cp) }
pub fn page_perfinfo(cp: &Rc<Cp>) -> gtk::Widget { css(); perfinfo::page(cp) }
pub fn page_eventvwr(cp: &Rc<Cp>) -> gtk::Widget { tool_window(cp, "eventvwr") }
pub fn page_taskschd(cp: &Rc<Cp>) -> gtk::Widget { tool_window(cp, "taskschd") }
pub fn page_diskmgmt(cp: &Rc<Cp>) -> gtk::Widget { tool_window(cp, "diskmgmt") }
pub fn page_devmgr(cp: &Rc<Cp>) -> gtk::Widget { tool_window(cp, "devmgr") }
pub fn page_sysinfo(cp: &Rc<Cp>) -> gtk::Widget { tool_window(cp, "sysinfo") }

/// `aerode-control <tool>` for the tools that have their own window: build only that window.
pub fn standalone(app: &gtk::Application, page: &str) -> bool {
    let build: fn(&gtk::Application) -> gtk::Window = match page {
        "eventvwr" => eventvwr::window,
        "taskschd" => taskschd::window,
        "diskmgmt" => diskmgmt::window,
        "devmgr" => devmgr::window,
        "compmgmt" => mmc::compmgmt,
        "sysinfo" => sysinfo::window,
        "cleanmgr" => cleanup::window,
        "mdsched" => tools::mdsched_window,
        _ => return false,
    };
    css();
    build(app).present();
    true
}

/// Opened from inside Control Panel: start the tool as its own process and stay on the page we
/// came from (the Back button keeps working as before).
fn tool_window(cp: &Rc<Cp>, id: &'static str) -> gtk::Widget {
    spawn_tool(id);
    let me = cp.clone();
    glib::idle_add_local_once(move || {
        let prev = { let mut h = me.history.borrow_mut(); h.pop(); h.pop() };
        me.open(&prev.unwrap_or_default());
    });
    gtk::Box::new(gtk::Orientation::Vertical, 0).upcast()
}

/// Start `aerode-control <id>` (this very binary when possible).
pub(crate) fn spawn_tool(id: &str) {
    let exe = std::env::current_exe().ok().filter(|p| p.exists()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "aerode-control".into());
    util::spawn(&[&exe, id]);
}

// ------------------------------------------------------------------------------------- styling

const CSS: &str = r#"
/* Action Center messages: coloured band on the left, white body, light border */
.ac-msg { border: 1px solid #d9dfe7; background-color: #ffffff; }
.ac-band-red { background-image: linear-gradient(to bottom, #d6262b, #9e0f13); min-width: 14px; }
.ac-band-yellow { background-image: linear-gradient(to bottom, #ffd84a, #f2b10b); min-width: 14px; }
.ac-band-green { background-image: linear-gradient(to bottom, #6cc24a, #2f8f1f); min-width: 14px; }
.ac-band-blue { background-image: linear-gradient(to bottom, #7fb4e8, #2f6fb6); min-width: 14px; }
.ac-msg-title { font-weight: bold; }
.ac-sec { color: #1e3287; font-size: 11pt; }
.ac-rule { background-color: #d5dfef; min-height: 1px; }
button.ac-chev { min-width: 18px; min-height: 18px; padding: 0; border-radius: 50%; border: 1px solid #a7b5c7;
  background-image: linear-gradient(to bottom, #ffffff, #e3e9f1); box-shadow: none; }
button.ac-chev:hover { border-color: #3c7fb1; background-image: linear-gradient(to bottom, #f2f9ff, #bfe2f8); }
.ac-status-k { color: #000000; font-weight: bold; }
.ac-status-v { color: #555555; }
.dim { color: #6d6d6d; }
.ok-text { color: #157b14; }
.bad-text { color: #c4161c; }
/* Windows Update status panel */
.wu-panel { border: 1px solid #c8cdd3; background-color: #ffffff; }
.wu-band-green { background-image: linear-gradient(to bottom, #8fd36d, #3c9a21); min-width: 12px; }
.wu-band-yellow { background-image: linear-gradient(to bottom, #ffe26b, #f1b40e); min-width: 12px; }
.wu-band-red { background-image: linear-gradient(to bottom, #f06a6a, #b81414); min-width: 12px; }
.wu-band-grey { background-image: linear-gradient(to bottom, #d9dee5, #9aa4b1); min-width: 12px; }
.wu-title { color: #1e3287; font-size: 12pt; }
/* Windows Firewall network location headers */
.fw-head-green { background-image: linear-gradient(to right, #c7ebba, #e9f7e4 45%, #ffffff); border: 1px solid #a7d29a; padding: 3px 6px; }
.fw-head-red { background-image: linear-gradient(to right, #f4b9b9, #fbe2e2 45%, #ffffff); border: 1px solid #df9a9a; padding: 3px 6px; }
.fw-head-grey { background-image: linear-gradient(to right, #e3e8ee, #f3f5f8 45%, #ffffff); border: 1px solid #c9d0d9; padding: 3px 6px; }
.fw-head-title { font-weight: bold; }
.fw-body { border: 1px solid #d9dfe7; border-top: none; padding: 8px 12px; }
/* Windows Experience Index */
.wei-table { border: 1px solid #c8d3e2; }
.wei-head { background-image: linear-gradient(to bottom, #fbfcfe, #e8eef6); border-bottom: 1px solid #c8d3e2; padding: 3px 8px; }
.wei-row { border-bottom: 1px solid #e6ebf2; padding: 5px 8px; }
.wei-score { font-weight: bold; }
.wei-base { border: 1px solid #8eaad3; border-radius: 3px; background-image: linear-gradient(to bottom, #ffffff, #dde8f6); padding: 10px 18px; }
.wei-base-num { font-size: 30pt; color: #1e3287; }
/* MMC consoles */
.mmc-actions { background-color: #ffffff; border-left: 1px solid #a3acb8; }
.mmc-actions-title { background-image: linear-gradient(to bottom, #fdfdfe, #e4e9f0); border-bottom: 1px solid #bcc5d1; padding: 3px 6px; font-weight: bold; }
.mmc-act-head { background-image: linear-gradient(to bottom, #f7f9fc, #dde4ee); border-top: 1px solid #ffffff; border-bottom: 1px solid #c4ccd7; padding: 2px 6px; font-weight: bold; }
.mmc-act button { padding: 1px 4px; border: 1px solid transparent; border-radius: 2px; background: none; box-shadow: none; min-height: 0; }
.mmc-act button:hover { background-image: linear-gradient(to bottom, #f5fafe, #dcebfb); border-color: #b5d3f3; }
.mmc-act button label { color: #000000; }
.mmc-act button:disabled label { color: #8d8d8d; }
.mmc-head { background-image: linear-gradient(to bottom, #fbfcfe, #e2e9f3); border-bottom: 1px solid #b6c3d6; padding: 3px 8px; }
.mmc-head .t { font-weight: bold; }
.mmc-tree, .mmc-tree treeview.view { background-color: #ffffff; }
.mmc-status { border-top: 1px solid #c6ccd4; padding: 1px 6px; background-color: #f0f0f0; }
.mmc-desc { border: 1px solid #c6cfdb; background-image: linear-gradient(to bottom, #ffffff, #f2f6fb); padding: 8px; }
.mmc-group { background-image: linear-gradient(to bottom, #f7f9fc, #e3e9f2); border: 1px solid #c4ccd7; padding: 2px 6px; font-weight: bold; }
.legend-box { border: 1px solid #808080; min-width: 12px; min-height: 10px; }
.si-find { border-top: 1px solid #c6ccd4; padding: 4px 6px; background-color: #f0f0f0; }
/* Explorer-style selection in lists (black text on light blue), as in Win7 consoles */
treeview.view:selected, treeview.view:selected:focus { color: #000000; background-image: linear-gradient(to bottom, #e5f1fb, #cce4f7); border: 1px solid #99c9ef; }
"#;

pub(crate) fn css() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) { return; }
    aerode::theme::load_css(CSS);
}

// ------------------------------------------------------------------------------------- helpers

/// Run `work` on a worker thread and hand its result to `done` on the GTK main loop.
pub(crate) fn bg<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || { let _ = tx.send_blocking(work()); });
    glib::MainContext::default().spawn_local(async move { if let Ok(v) = rx.recv().await { done(v) } });
}

/// Like `bg`, for work that reports progress: `work` gets a sender, `each` sees every message on
/// the main loop; the stream ends when `work` returns.
pub(crate) fn stream<M: Send + 'static>(work: impl FnOnce(async_channel::Sender<M>) + Send + 'static, mut each: impl FnMut(M) + 'static) {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || work(tx));
    glib::MainContext::default().spawn_local(async move { while let Ok(m) = rx.recv().await { each(m) } });
}

/// Run a command and return (success, stdout + stderr). Never call on the GTK thread.
pub(crate) fn run(argv: &[&str]) -> (bool, String) {
    if argv.is_empty() || !util::which(argv[0]) { return (false, format!("{} was not found.", argv.first().unwrap_or(&""))); }
    match std::process::Command::new(argv[0]).args(&argv[1..]).stdin(std::process::Stdio::null()).output() {
        Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(e) => (false, e.to_string()),
    }
}

/// Stdout of a quick command run from a worker thread ("" on failure).
pub(crate) fn sh(argv: &[&str]) -> String { util::output(argv).unwrap_or_default() }

/// Run a privileged command through pkexec (our UAC prompt) without blocking the UI; `done` gets
/// (success, output). The polkit agent shows its own dialog, so nothing waits on the GTK thread.
pub(crate) fn elevate(argv: &[&str], done: impl FnOnce(bool, String) + 'static) {
    let mut v: Vec<String> = vec!["pkexec".into()];
    v.extend(argv.iter().map(|s| s.to_string()));
    bg(move || { let r: Vec<&str> = v.iter().map(|s| s.as_str()).collect(); run(&r) }, move |(ok, out)| done(ok, out));
}

/// A privileged shell snippet (several commands under one UAC prompt).
pub(crate) fn elevate_sh(script: &str, done: impl FnOnce(bool, String) + 'static) {
    elevate(&["sh", "-c", script], done);
}

/// Show what went wrong after a privileged command (126/127 = cancelled at the UAC prompt: quiet).
pub(crate) fn report(title: &str, ok: bool, out: &str) -> bool {
    if !ok && !out.contains("Not authorized") && !out.contains("dismissed") && !out.trim().is_empty() {
        util::error(title, out.trim());
    } else if !ok && out.trim().is_empty() {
        util::error(title, "The operation did not complete.");
    }
    ok
}

/// Single-quote for sh -c.
pub(crate) fn q(s: &str) -> String { format!("'{}'", s.replace('\'', "'\\''")) }

/// Win7 confirmation: a question with an action button and Cancel. Modal, but the main loop keeps
/// running while it waits.
pub(crate) fn confirm(parent: Option<&gtk::Window>, title: &str, primary: &str, secondary: &str, ok: &str) -> bool {
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Warning, gtk::ButtonsType::None, primary);
    d.set_title(title);
    if !secondary.is_empty() { d.set_secondary_text(Some(secondary)); }
    let b = d.add_button(ok, gtk::ResponseType::Ok);
    if let Ok(b) = b.downcast::<gtk::Button>() { shield(&b); }
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.set_default_response(gtk::ResponseType::Cancel);
    let r = d.run();
    d.close();
    r == gtk::ResponseType::Ok
}

/// Information message (OK only).
pub(crate) fn inform(parent: Option<&gtk::Window>, title: &str, primary: &str, secondary: &str) {
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, gtk::MessageType::Info, gtk::ButtonsType::Ok, primary);
    d.set_title(title);
    if !secondary.is_empty() { d.set_secondary_text(Some(secondary)); }
    d.run();
    d.close();
}

/// Put the UAC shield on a button (Win7 marks every action that needs administrator rights).
pub(crate) fn shield(b: &gtk::Button) {
    b.set_image(Some(&img(&["security-medium"], 16)));
    b.set_always_show_image(true);
}

pub(crate) fn shield_button(label: &str) -> gtk::Button {
    let b = gtk::Button::with_mnemonic(label);
    shield(&b);
    b
}

/// The first icon of `names` the theme has (the Win7 one extracted by aerode-extract first,
/// freedesktop fallbacks after it).
pub(crate) fn img(names: &[&str], size: i32) -> gtk::Image { util::image(util::first_icon(names), size) }
pub(crate) fn pix(names: &[&str], size: i32) -> Option<Pixbuf> { util::pixbuf(util::first_icon(names), size) }

/// The window a widget lives in.
pub(crate) fn win_of(w: &impl IsA<gtk::Widget>) -> Option<gtk::Window> { w.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()) }

/// A Win7 control panel link with a shield (admin rights needed).
pub(crate) fn shield_link(text: &str, f: impl Fn() + 'static) -> gtk::Box {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    h.pack_start(&img(&["security-medium"], 16), false, false, 0);
    h.pack_start(&super::super::link(text, f), false, false, 0);
    h
}

/// Task-pane link that runs a closure (cp.side_link only navigates).
pub(crate) fn side_action(cp: &Rc<Cp>, text: &str, f: impl Fn() + 'static) {
    cp.side.pack_start(&super::super::link(text, f), false, false, 2);
}
pub(crate) fn side_shield(cp: &Rc<Cp>, text: &str, f: impl Fn() + 'static) {
    cp.side.pack_start(&shield_link(text, f), false, false, 2);
}

/// "See also" at the bottom of the task pane.
pub(crate) fn see_also(cp: &Rc<Cp>, links: &[(&str, &'static str)]) {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let h = gtk::Label::new(Some("See also"));
    h.set_xalign(0.0);
    h.style_context().add_class("dim");
    v.pack_start(&h, false, false, 2);
    for (t, page) in links {
        let (me, page) = (cp.clone(), *page);
        if page.starts_with('!') { let id = &page[1..]; v.pack_start(&super::super::link(t, move || spawn_tool(id)), false, false, 0); }
        else { v.pack_start(&super::super::link(t, move || me.open(page)), false, false, 0); }
    }
    cp.side.pack_end(&v, false, false, 8);
}

/// "See also" with arbitrary actions.
pub(crate) fn see_also_fn(cp: &Rc<Cp>, links: Vec<(&str, Box<dyn Fn()>)>) {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let h = gtk::Label::new(Some("See also"));
    h.set_xalign(0.0);
    h.style_context().add_class("dim");
    v.pack_start(&h, false, false, 2);
    for (t, f) in links { v.pack_start(&super::super::link(t, f), false, false, 0); }
    cp.side.pack_end(&v, false, false, 8);
}

/// Bold label.
pub(crate) fn bold(t: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(t)));
    l.set_xalign(0.0);
    l
}

/// Label with a css class.
pub(crate) fn label(t: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    if !class.is_empty() { l.style_context().add_class(class); }
    l
}

/// A list view with text columns (title, model column, width; width 0 = expand). Returns the
/// view; the caller owns the model.
pub(crate) fn columns(tv: &gtk::TreeView, cols: &[(&str, i32, i32)]) {
    for &(t, c, w) in cols {
        let col = gtk::TreeViewColumn::new();
        col.set_title(t);
        let r = gtk::CellRendererText::new();
        r.set_property("ellipsize", gtk::pango::EllipsizeMode::End);
        TreeViewColumnExt::pack_start(&col, &r, true);
        TreeViewColumnExt::add_attribute(&col, &r, "text", c);
        col.set_resizable(true);
        col.set_sort_column_id(c);
        if w > 0 { col.set_sizing(gtk::TreeViewColumnSizing::Fixed); col.set_fixed_width(w); } else { col.set_expand(true); }
        tv.append_column(&col);
    }
}

/// A first column with a 16 px icon (model column `icon`, a Pixbuf) followed by text `text`.
pub(crate) fn icon_column(tv: &gtk::TreeView, title: &str, icon: i32, text: i32, width: i32) -> gtk::TreeViewColumn {
    let col = gtk::TreeViewColumn::new();
    col.set_title(title);
    let p = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&col, &p, false);
    TreeViewColumnExt::add_attribute(&col, &p, "pixbuf", icon);
    let r = gtk::CellRendererText::new();
    r.set_property("ellipsize", gtk::pango::EllipsizeMode::End);
    TreeViewColumnExt::pack_start(&col, &r, true);
    TreeViewColumnExt::add_attribute(&col, &r, "text", text);
    col.set_resizable(true);
    col.set_sort_column_id(text);
    if width > 0 { col.set_sizing(gtk::TreeViewColumnSizing::Fixed); col.set_fixed_width(width); } else { col.set_expand(true); }
    tv.append_column(&col);
    col
}

pub(crate) fn scrolled(w: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(w);
    sw
}

/// The text of a string model column.
pub(crate) fn sval(m: &impl IsA<gtk::TreeModel>, it: &gtk::TreeIter, col: i32) -> String {
    m.value(it, col).get::<String>().unwrap_or_default()
}

/// Win7 (en-US) short date and time: 11/2/2026 3:04:05 PM.
pub(crate) fn when(secs: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(secs, 0) { chrono::LocalResult::Single(t) => t.format("%-m/%-d/%Y %-I:%M:%S %p").to_string(), _ => String::new() }
}
/// 11/2/2026
pub(crate) fn day(secs: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(secs, 0) { chrono::LocalResult::Single(t) => t.format("%-m/%-d/%Y").to_string(), _ => String::new() }
}
/// "Today at 3:04 PM" / "Yesterday at …" / "11/2/2026 at 3:04 PM" (Windows Update, Backup).
pub(crate) fn when_words(secs: i64) -> String {
    use chrono::TimeZone;
    let chrono::LocalResult::Single(t) = chrono::Local.timestamp_opt(secs, 0) else { return "Never".into() };
    let today = chrono::Local::now().date_naive();
    let d = t.date_naive();
    let date = if d == today { "Today".to_string() } else if Some(d) == today.pred_opt() { "Yesterday".to_string() } else { t.format("%-m/%-d/%Y").to_string() };
    format!("{date} at {}", t.format("%-I:%M %p"))
}
pub(crate) fn now() -> i64 { chrono::Local::now().timestamp() }

/// Windows-style sizes: 465.76 GB, 100 MB.
pub(crate) fn gb(n: u64) -> String {
    let f = n as f64;
    if f >= 1024f64.powi(4) { format!("{:.2} TB", f / 1024f64.powi(4)) }
    else if f >= 1024f64.powi(3) { format!("{:.2} GB", f / 1024f64.powi(3)) }
    else if f >= 1024f64.powi(2) { format!("{:.0} MB", f / 1024f64.powi(2)) }
    else if f >= 1024.0 { format!("{:.0} KB", f / 1024.0) }
    else { format!("{n} bytes") }
}

/// Free and total bytes of the file system holding `path`.
pub(crate) fn fs_space(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 { return None; }
    Some((s.f_bavail as u64 * s.f_frsize as u64, s.f_blocks as u64 * s.f_frsize as u64))
}

/// Open a terminal running `cmd` (used where a tool needs to talk to the user, e.g. yay).
pub(crate) fn terminal(cmd: &str) -> bool {
    let hold = format!("{cmd}; echo; read -n 1 -s -r -p 'Press any key to close this window . . .'");
    for t in [vec!["aerode-cmd", "-e", "bash", "-c", &hold], vec!["x-terminal-emulator", "-e", "bash", "-c", &hold], vec!["konsole", "-e", "bash", "-c", &hold],
        vec!["gnome-terminal", "--", "bash", "-c", &hold], vec!["xfce4-terminal", "-x", "bash", "-c", &hold], vec!["alacritty", "-e", "bash", "-c", &hold],
        vec!["kitty", "bash", "-c", &hold], vec!["xterm", "-e", "bash", "-c", &hold]] {
        if util::spawn(&t) { return true; }
    }
    util::error("No terminal program was found.", "Install a terminal emulator (for example xterm).");
    false
}

/// A horizontal row: widgets packed left to right.
pub(crate) fn hbox(spacing: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, spacing) }
pub(crate) fn vbox(spacing: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, spacing) }

/// Round expand/collapse button with a chevron (Action Center, Windows Firewall).
pub(crate) fn chevron(open: bool) -> gtk::Button {
    let b = gtk::Button::new();
    b.style_context().add_class("ac-chev");
    b.set_relief(gtk::ReliefStyle::None);
    b.set_valign(gtk::Align::Center);
    let l = gtk::Label::new(Some(if open { "⌃" } else { "⌄" }));
    b.add(&l);
    b
}
pub(crate) fn set_chevron(b: &gtk::Button, open: bool) {
    if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_text(if open { "⌃" } else { "⌄" }); }
}

/// Write `text` to a file the user picks (Save As).
pub(crate) fn save_text(parent: Option<&gtk::Window>, title: &str, name: &str, text: &str) {
    let fc = gtk::FileChooserNative::new(Some(title), parent, gtk::FileChooserAction::Save, Some("_Save"), Some("_Cancel"));
    fc.set_current_name(name);
    fc.set_do_overwrite_confirmation(true);
    if fc.run() == gtk::ResponseType::Accept {
        if let Some(p) = fc.filename() {
            if let Err(e) = std::fs::write(&p, text) { util::error("The file could not be saved.", &e.to_string()); }
        }
    }
}

/// A read-only multi-line text box.
pub(crate) fn text_box(t: &str) -> gtk::TextView {
    let tv = gtk::TextView::new();
    tv.set_editable(false);
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    tv.set_left_margin(4);
    tv.set_right_margin(4);
    tv.buffer().map(|b| b.set_text(t));
    tv
}

/// Copy text to the clipboard.
pub(crate) fn copy(t: &str) {
    gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD).set_text(t);
}

/// Seconds since the epoch of a file's modification time.
pub(crate) fn mtime(p: &std::path::Path) -> Option<i64> {
    p.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

/// Cache files shared between applets (Windows Update result → Action Center …).
pub(crate) fn cache_dir() -> std::path::PathBuf {
    let d = glib::user_cache_dir().join("aerode");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Show a sub page of an applet in place (Windows Update ▸ View update history …): the
/// breadcrumb gets the extra level and Back returns to the applet's main page.
pub(crate) fn subpage(cp: &Rc<Cp>, holder: &gtk::Box, page_id: &str, crumb: &str, w: &impl IsA<gtk::Widget>) {
    for c in holder.children() { holder.remove(&c); }
    holder.pack_start(w, true, true, 0);
    holder.show_all();
    let cur = cp.crumb.text().to_string();
    let base = cur.split(" ▸ ").take(3).collect::<Vec<_>>().join(" ▸ ");
    cp.crumb.set_text(&format!("{base} ▸ {crumb}"));
    cp.history.borrow_mut().push(page_id.to_string());
}
