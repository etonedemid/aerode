//! Icons, application lookup, launching, misc helpers.
use gtk::gdk_pixbuf::{InterpType, Pixbuf};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config;

pub fn pixbuf(name: &str, size: i32) -> Option<Pixbuf> {
    if name.starts_with('/') {
        return Pixbuf::from_file_at_scale(name, size, size, true).ok();
    }
    let theme = gtk::IconTheme::default()?;
    theme
        .load_icon(name, size, gtk::IconLookupFlags::FORCE_SIZE)
        .ok()
        .flatten()
}

/// Show a 16px icon in a menu item's gutter (on the check-background box position), like Win7.
pub fn menu_icon(item: &gtk::MenuItem, name: &str) {
    let Some(pb) = pixbuf(name, 16) else { return };
    item.connect_local("draw", true, move |args| {
        let w = args[0].get::<gtk::Widget>().ok()?;
        let cr = args[1].get::<gtk::cairo::Context>().ok()?;
        let y = (w.allocated_height() - 16) as f64 / 2.0;
        cr.set_source_pixbuf(&pb, 3.0, y.floor());
        if w.is_sensitive() { cr.paint().ok(); } else { cr.paint_with_alpha(0.45).ok(); }
        Some(false.to_value())
    });
}

pub fn pixbuf_or(name: &str, size: i32, fallback: &str) -> Option<Pixbuf> {
    pixbuf(name, size).or_else(|| pixbuf(fallback, size))
}

pub fn gicon_pixbuf(icon: Option<&gio::Icon>, size: i32) -> Option<Pixbuf> {
    let theme = gtk::IconTheme::default()?;
    icon.and_then(|i| theme.lookup_by_gicon(i, size, gtk::IconLookupFlags::FORCE_SIZE))
        .and_then(|info| info.load_icon().ok())
        .map(|p| scale(&p, size))
        .or_else(|| pixbuf("application-x-executable", size))
}

pub fn scale(p: &Pixbuf, size: i32) -> Pixbuf {
    if p.width() == size && p.height() == size {
        return p.clone();
    }
    p.scale_simple(size, size, InterpType::Bilinear).unwrap_or_else(|| p.clone())
}

pub fn image(name: &str, size: i32) -> gtk::Image {
    match pixbuf_or(name, size, "application-x-executable") {
        Some(p) => gtk::Image::from_pixbuf(Some(&p)),
        None => gtk::Image::new(),
    }
}

pub fn app_image(app: &gio::AppInfo, size: i32) -> gtk::Image {
    match gicon_pixbuf(app.icon().as_ref(), size) {
        Some(p) => gtk::Image::from_pixbuf(Some(&p)),
        None => gtk::Image::new(),
    }
}

/// Returns the first icon name the theme knows, else the last one.
pub fn first_icon<'a>(names: &[&'a str]) -> &'a str {
    if let Some(t) = gtk::IconTheme::default() {
        for n in names {
            if t.has_icon(n) {
                return n;
            }
        }
    }
    names[names.len() - 1]
}

// ------------------------------------------------------------- apps
pub fn all_apps() -> Vec<gio::AppInfo> {
    let mut v: Vec<_> = gio::AppInfo::all().into_iter().filter(|a| a.should_show()).collect();
    v.sort_by_key(|a| a.display_name().to_lowercase());
    v
}

pub fn app_by_id(id: &str) -> Option<gio::AppInfo> {
    gio::DesktopAppInfo::new(id).map(|d| d.upcast())
}

pub fn first_app(ids: &[&str]) -> Option<gio::AppInfo> {
    ids.iter().find_map(|i| app_by_id(i))
}

pub fn categories(app: &gio::AppInfo) -> String {
    app.downcast_ref::<gio::DesktopAppInfo>()
        .and_then(|d| d.categories())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

pub fn terminal() -> Option<gio::AppInfo> {
    first_app(&[
        "org.gnome.Terminal.desktop", "org.gnome.Console.desktop", "org.kde.konsole.desktop",
        "xfce4-terminal.desktop", "org.xfce.terminal.desktop", "lxterminal.desktop",
        "qterminal.desktop", "kitty.desktop", "Alacritty.desktop", "xterm.desktop",
    ])
}

pub fn default_for(mime: &str) -> Option<gio::AppInfo> {
    gio::AppInfo::default_for_type(mime, false)
}

pub fn pinned() -> Vec<String> {
    if let Some(p) = config::get().pinned {
        return p;
    }
    let mut ids: Vec<String> = Vec::new();
    for a in [
        app_by_id("aerode-explorer.desktop").or_else(|| default_for("inode/directory")),
        default_for("x-scheme-handler/http"),
        default_for("video/mp4"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(id) = a.id() {
            if !ids.contains(&id.to_string()) {
                ids.push(id.to_string());
            }
        }
    }
    config::update(|c| c.pinned = Some(ids.clone()));
    ids
}

fn launch_ctx() -> Option<gdk::AppLaunchContext> {
    gdk::Display::default().and_then(|d| d.app_launch_context())
}

pub fn launch(app: &gio::AppInfo) {
    // Start menu privacy: "Store and display recently opened programs"
    if let Some(id) = app.id().filter(|_| config::get().taskbar.track_programs) {
        config::update(|c| *c.usage.entry(id.to_string()).or_insert(0) += 1);
    }
    if let Err(e) = app.launch(&[], launch_ctx().as_ref()) {
        error(&format!("Windows cannot open '{}'.", app.display_name()), &e.to_string());
    }
}

pub fn open_uri(uri: &str) -> bool {
    match gio::AppInfo::launch_default_for_uri(uri, launch_ctx().as_ref()) {
        Ok(_) => true,
        Err(_) => spawn(&["xdg-open", uri]),
    }
}

pub fn open_path(p: &Path) -> bool {
    open_uri(&gio::File::for_path(p).uri())
}

/// Open a folder, preferring our own Explorer.
pub fn explore(target: &str) {
    if which("aerode-explorer") {
        spawn(&["aerode-explorer", target]);
    } else if target.contains("://") {
        open_uri(target);
    } else {
        open_path(Path::new(target));
    }
}

pub fn which(cmd: &str) -> bool {
    if cmd.contains('/') {
        return Path::new(cmd).exists();
    }
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

/// Fire-and-forget a process (reaped in a helper thread).
pub fn spawn(argv: &[&str]) -> bool {
    if argv.is_empty() || !which(argv[0]) {
        return false;
    }
    match Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut c) => {
            std::thread::spawn(move || {
                let _ = c.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Run and capture stdout (blocking; only for quick commands).
pub fn output(argv: &[&str]) -> Option<String> {
    if !which(argv[0]) {
        return None;
    }
    Command::new(argv[0])
        .args(&argv[1..])
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Run a command asynchronously and hand stdout to `cb` on the main loop.
pub fn output_async(argv: Vec<String>, cb: impl FnOnce(Option<String>) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        let _ = tx.send_blocking(output(&refs));
    });
    // woken by the channel: no polling while the command runs
    glib::MainContext::default().spawn_local(async move {
        if let Ok(v) = rx.recv().await { cb(v); }
    });
}

/// Run `on_change` (on the main loop, coalesced over `debounce`) whenever a line printed by the
/// long-running monitor command `argv` (e.g. `pactl subscribe`, `nmcli monitor`) passes `wanted`.
/// The command is restarted when it exits (audio/network daemons restart); returns false when it
/// cannot be started at all, so the caller can fall back to polling.
pub fn watch_lines(argv: &[&str], wanted: fn(&str) -> bool, debounce: std::time::Duration, on_change: impl Fn() + 'static) -> bool {
    use std::io::BufRead;
    if !which(argv[0]) { return false; }
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let (tx, rx) = async_channel::bounded::<()>(1);
    std::thread::spawn(move || loop {
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).env("LC_ALL", "C").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        // the monitor dies with us: `pactl subscribe` / `nmcli monitor` only notice a dead reader
        // when their next write fails, which can be never on a quiet system. (The death signal
        // follows the spawning thread, which is this loop and lives as long as the watch.)
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM); Ok(()) });
        }
        let child = cmd.spawn();
        let Ok(mut child) = child else { return };
        if let Some(out) = child.stdout.take() {
            for line in std::io::BufReader::new(out).lines() {
                let Ok(line) = line else { break };
                // a full channel already means "changed": drop the extra notifications
                if wanted(&line) && tx.try_send(()).is_err() && tx.is_closed() { let _ = child.kill(); return; }
            }
        }
        let _ = child.wait();
        if tx.is_closed() { return; }
        std::thread::sleep(std::time::Duration::from_secs(3));
        let _ = tx.try_send(()); // the daemon restarted: state may have changed meanwhile
    });
    glib::MainContext::default().spawn_local(async move {
        while rx.recv().await.is_ok() {
            glib::timeout_future(debounce).await;
            while rx.try_recv().is_ok() {}
            on_change();
        }
    });
    true
}

/// Win+R semantics: path → open, URL → browser, else command line.
pub fn run_command(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }
    let expanded = shellexpand(cmd);
    let p = Path::new(&expanded);
    if p.is_dir() || (p.exists() && !is_exec(p)) {
        return open_path(p);
    }
    if cmd.contains("://") {
        return open_uri(cmd);
    }
    if let Some(app) = app_by_id(&format!("{cmd}.desktop")) {
        launch(&app);
        return true;
    }
    match glib::shell_parse_argv(expanded.as_str()) {
        Ok(argv) => {
            let v: Vec<String> = argv.iter().map(|s| s.to_string_lossy().into_owned()).collect();
            let r: Vec<&str> = v.iter().map(|s| s.as_str()).collect();
            if spawn(&r) {
                return true;
            }
        }
        Err(_) => {}
    }
    error(
        &format!("Windows cannot find '{cmd}'."),
        "Make sure you typed the name correctly, and then try again.",
    );
    false
}

fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

pub fn shellexpand(s: &str) -> String {
    let mut out = if let Some(rest) = s.strip_prefix('~') {
        format!("{}{}", config::home().display(), rest)
    } else {
        s.to_string()
    };
    for (k, v) in std::env::vars() {
        out = out.replace(&format!("${k}"), &v).replace(&format!("%{k}%"), &v);
    }
    out
}

pub fn error(title: &str, text: &str) {
    message_box(None, gtk::MessageType::Error, title, text);
}

/// The name Explorer and the desktop show for a file: Folder Options > "Hide extensions for known
/// file types" drops the extension when some program is registered for the type the extension
/// stands for (judged by the name alone, like Windows: "notes.zzqq" keeps its extension).
pub fn shown_name(info: &gio::FileInfo, hide_ext: bool) -> String {
    let name = info.display_name().to_string();
    if !hide_ext || info.file_type() == gio::FileType::Directory { return name; }
    let Some(dot) = name.rfind('.').filter(|i| *i > 0 && *i + 1 < name.len()) else { return name };
    let (ct, uncertain) = gio::content_type_guess(Some(&name), &[]);
    if uncertain || ct.is_empty() || ct == "application/octet-stream" || ct == "application/x-desktop" { return name; }
    if !gio::AppInfo::all_for_type(&ct).is_empty() { name[..dot].to_string() } else { name }
}

pub fn special_dir(d: glib::UserDirectory, fallback: &str) -> std::path::PathBuf {
    glib::user_special_dir(d).unwrap_or_else(|| config::home().join(fallback))
}

/// Play an event from the Windows 7 sound scheme.
pub fn sound(ev: crate::sounds::Event) {
    crate::sounds::play(ev);
}

/// The sound MessageBox plays for its icon (MessageBeep): Error → Critical Stop, Warning →
/// Exclamation, Information → Asterisk, Question → Question (silent in Windows Default), no icon →
/// Default Beep. Only for real message boxes: task dialogs (save prompts, "keep these display
/// settings?") and progress windows make no sound.
pub fn message_sound(kind: gtk::MessageType) {
    use crate::sounds::Event;
    sound(match kind {
        gtk::MessageType::Error => Event::CriticalStop,
        gtk::MessageType::Warning => Event::Exclamation,
        gtk::MessageType::Info => Event::Asterisk,
        gtk::MessageType::Question => Event::Question,
        _ => Event::DefaultBeep,
    });
}

/// A Win7 message box: icon, text, OK, and the icon's sound. Returns at once (non-modal).
pub fn message_box(parent: Option<&gtk::Window>, kind: gtk::MessageType, title: &str, text: &str) {
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::empty(), kind, gtk::ButtonsType::Ok, title);
    if !text.is_empty() { d.set_secondary_text(Some(text)); }
    if parent.is_none() { d.set_keep_above(true); }
    d.connect_response(|d, _| d.close());
    d.show_all();
    if !DIALOG_SOUNDS.load(std::sync::atomic::Ordering::Relaxed) { message_sound(kind); }
}

static DIALOG_SOUNDS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Every GtkMessageDialog of this process plays Win7's sound for its icon when it appears, like
/// MessageBox does (error → Critical Stop, warning → Exclamation, info → Asterisk, question →
/// Question). Dialogs without an icon (MessageType::Other, Win7's silent task dialogs) stay
/// quiet. One emission hook on "map", so no dialog can forget it.
pub fn message_dialog_sounds() {
    use gtk::glib::gobject_ffi;
    use gtk::glib::translate::IntoGlib;
    if DIALOG_SOUNDS.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    unsafe extern "C" fn hook(_hint: *mut gobject_ffi::GSignalInvocationHint, n: u32, params: *const gobject_ffi::GValue, _data: gtk::glib::ffi::gpointer) -> gtk::glib::ffi::gboolean {
        if n >= 1 {
            let v = unsafe { &*(params as *const gtk::glib::Value) };
            if let Ok(d) = v.get::<gtk::glib::Object>().map(|o| o.downcast::<gtk::MessageDialog>()) {
                if let Ok(d) = d {
                    let kind = d.message_type();
                    if kind != gtk::MessageType::Other { message_sound(kind); }
                }
            }
        }
        gtk::glib::ffi::GTRUE
    }
    unsafe {
        let id = gobject_ffi::g_signal_lookup(c"map".as_ptr(), gtk::Widget::static_type().into_glib());
        if id != 0 { gobject_ffi::g_signal_add_emission_hook(id, 0, Some(hook), std::ptr::null_mut(), None); }
    }
}

pub fn primary_geometry() -> gdk::Rectangle {
    let d = gdk::Display::default().expect("no display");
    let m = d.primary_monitor().or_else(|| d.monitor(0)).expect("no monitor");
    m.geometry()
}

pub fn human_size(n: u64) -> String {
    let u = ["bytes", "KB", "MB", "GB", "TB"];
    let mut f = n as f64;
    let mut i = 0;
    while f >= 1024.0 && i < u.len() - 1 {
        f /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} bytes") } else if f < 10.0 { format!("{f:.2} {}", u[i]) } else { format!("{f:.0} {}", u[i]) }
}

/// Panics go to stderr and, with a backtrace, to ~/.cache/aerode/<name>.log so they can be
/// reported (the session restarts a crashed shell or window manager).
pub fn log_panics(name: &'static str) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let dir = std::env::var_os("XDG_CACHE_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
            .map(|d| d.join("aerode"));
        if let Some(dir) = dir {
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.log"))) {
                use std::io::Write;
                let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let _ = writeln!(f, "--- {when} {name} {}: {info}\n{}", env!("CARGO_PKG_VERSION"), std::backtrace::Backtrace::force_capture());
            }
        }
    }));
}

/// This user's processes named `name` (exactly, as /proc/<pid>/comm shows it) that belong to this
/// X display – $DISPLAY in their environment, ":0" and ":0.0" being the same – and, with `arg1`,
/// whose first argument is that ("shell" for the shell, not `aerode mixer` or `aerode lock`).
/// What `aerode restart` and Log off may signal; other sessions of the same user are left alone.
pub fn procs_on_display(name: &str, arg1: Option<&str>) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt;
    let norm = |d: &str| match d.rsplit_once(':') { Some((h, n)) => format!("{h}:{}", n.split('.').next().unwrap_or(n)), None => d.to_string() };
    let ours = norm(&std::env::var("DISPLAY").unwrap_or_default());
    let uid = unsafe { libc::getuid() };
    let me = std::process::id();
    let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
    rd.flatten().filter_map(|e| {
        let pid: u32 = e.file_name().to_str()?.parse().ok()?;
        if pid == me || e.metadata().ok()?.uid() != uid { return None; }
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        if comm.trim_end() != name { return None; }
        if let Some(a) = arg1 {
            let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
            if cmd.split(|b| *b == 0).nth(1) != Some(a.as_bytes()) { return None; }
        }
        let env = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        let on_ours = env.split(|b| *b == 0).any(|kv| kv.strip_prefix(b"DISPLAY=").map(|d| norm(&String::from_utf8_lossy(d)) == ours).unwrap_or(false));
        on_ours.then_some(pid)
    }).collect()
}

/// Send `sig` to every process `procs_on_display` finds; true when there was at least one.
pub fn signal_on_display(name: &str, arg1: Option<&str>, sig: i32) -> bool {
    let pids = procs_on_display(name, arg1);
    for p in &pids { unsafe { libc::kill(*p as i32, sig); } }
    !pids.is_empty()
}

#[cfg(test)]
mod procs_tests {
    #[test]
    fn finds_nothing_for_an_unknown_name() {
        assert!(super::procs_on_display("no-such-program-xyz", None).is_empty());
    }
}
