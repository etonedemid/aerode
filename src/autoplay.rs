//! AutoPlay (Windows 7): what happens when a disc, USB drive, memory card or phone shows up.
//! The Control Panel's AutoPlay page edits the default per type of media; the shell watches the
//! volume monitor, mounts new removable media like Windows does, and then either does what was
//! chosen or shows the AutoPlay window.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{config, util};

pub struct Media {
    /// key in config.autoplay_actions
    pub key: &'static str,
    /// the Windows 7 name in the Control Panel list
    pub name: &'static str,
    /// "Always do this for …:" in the AutoPlay window
    pub plural: &'static str,
    pub icon: &'static str,
    /// x-content types that identify it; the programs registered for them are offered
    pub content: &'static [&'static str],
    /// what those programs do ("Import pictures and videos", "Burn files to disc"…)
    pub content_verb: &'static str,
    /// a file type whose default program is offered too ("" = none) and what it does
    pub player: &'static str,
    pub player_verb: &'static str,
    /// listed under "Devices" instead of "Media"
    pub device: bool,
}

const fn m(key: &'static str, name: &'static str, plural: &'static str, icon: &'static str, content: &'static [&'static str], content_verb: &'static str, player: &'static str, player_verb: &'static str) -> Media {
    Media { key, name, plural, icon, content, content_verb, player, player_verb, device: false }
}

/// Every media type in the order of the Windows 7 AutoPlay page.
pub const MEDIA: &[Media] = &[
    m("audiocd", "Audio CD", "audio CDs", "media-optical-cd", &["x-content/audio-cdda"], "Play audio CD", "audio/mpeg", "Play audio CD"),
    m("enhancedaudiocd", "Enhanced audio CD", "enhanced audio CDs", "media-optical-cd", &["x-content/audio-cdda"], "Play audio CD", "audio/mpeg", "Play audio CD"),
    m("dvdmovie", "DVD movie", "DVD movies", "media-optical-dvd", &["x-content/video-dvd"], "Play DVD movie", "video/mp4", "Play DVD movie"),
    m("enhanceddvdmovie", "Enhanced DVD movie", "enhanced DVD movies", "media-optical-dvd", &["x-content/video-dvd"], "Play DVD movie", "video/mp4", "Play DVD movie"),
    m("software", "Software and games", "software and games", "applications-games", &["x-content/unix-software", "x-content/win32-software", "x-content/software"], "Install or run program", "", ""),
    m("pictures", "Pictures", "pictures", "image-x-generic", &["x-content/image-dcf", "x-content/image-picturecd"], "Import pictures and videos", "image/jpeg", "View pictures"),
    m("videofiles", "Video files", "video files", "video-x-generic", &[], "", "video/mp4", "Play"),
    m("audiofiles", "Audio files", "audio files", "audio-x-generic", &["x-content/audio-player"], "Play", "audio/mpeg", "Play"),
    m("blankcd", "Blank CD", "blank CDs", "media-optical", &["x-content/blank-cd"], "Burn files to disc", "", ""),
    m("mixed", "Mixed content", "mixed content", "drive-removable-media", &[], "", "video/mp4", "Play"),
    m("bluray", "Blu-ray Disc movie", "Blu-ray Disc movies", "media-optical", &["x-content/video-bluray"], "Play Blu-ray Disc movie", "video/mp4", "Play Blu-ray Disc movie"),
    m("dvdaudio", "DVD-Audio", "DVD-Audio discs", "media-optical-dvd", &["x-content/audio-dvd"], "Play DVD-Audio", "audio/mpeg", "Play DVD-Audio"),
    m("vcd", "Video CD", "video CDs", "media-optical-cd", &["x-content/video-vcd"], "Play video CD", "video/mp4", "Play video CD"),
    m("svcd", "Super Video CD", "super video CDs", "media-optical-cd", &["x-content/video-svcd"], "Play super video CD", "video/mp4", "Play super video CD"),
    m("blankdvd", "Blank DVD", "blank DVDs", "media-optical-dvd", &["x-content/blank-dvd"], "Burn files to disc", "", ""),
    m("blankbd", "Blank Blu-ray Disc", "blank Blu-ray Discs", "media-optical", &["x-content/blank-bd"], "Burn files to disc", "", ""),
    m("hddvd", "HD DVD movie", "HD DVD movies", "media-optical-dvd", &["x-content/video-hddvd"], "Play HD DVD movie", "video/mp4", "Play HD DVD movie"),
    m("blankhddvd", "Blank HD DVD", "blank HD DVDs", "media-optical-dvd", &["x-content/blank-hddvd"], "Burn files to disc", "", ""),
    Media { key: "devices", name: "Cameras and phones", plural: "this device", icon: "camera-photo", content: &["x-content/image-dcf"], content_verb: "Import pictures and videos", player: "", player_verb: "", device: true },
];

pub fn media(key: &str) -> Option<&'static Media> { MEDIA.iter().find(|m| m.key == key) }

fn is_blank(m: &Media) -> bool { m.key.starts_with("blank") }

/// Discs (as opposed to drives and devices): no "Use this drive for backup".
fn is_disc(m: &Media) -> bool {
    matches!(m.key, "audiocd" | "enhancedaudiocd" | "dvdmovie" | "enhanceddvdmovie" | "bluray" | "dvdaudio" | "vcd" | "svcd" | "hddvd") || is_blank(m)
}

/// One thing AutoPlay can do with a medium.
#[derive(Clone)]
pub struct Choice {
    /// stored in the config: "app:<desktop id>" | "open" | "run" | "none" | "ask"
    pub id: String,
    /// "Play audio CD"
    pub main: String,
    /// "using VLC media player" ("" for Take no action / Ask me every time)
    pub note: String,
    pub app: Option<gio::AppInfo>,
    pub icon: &'static str,
}

impl Choice {
    /// The one-line Control Panel wording: "Play audio CD using VLC media player".
    pub fn label(&self) -> String { if self.note.is_empty() { self.main.clone() } else { format!("{} {}", self.main, self.note) } }
}

/// What can be done with this kind of media: the registered programs first, then opening the
/// folder; the Control Panel also gets "Take no action" and "Ask me every time".
pub fn choices(m: &Media, for_settings: bool) -> Vec<Choice> {
    let mut v: Vec<Choice> = vec![];
    let mut add = |app: gio::AppInfo, verb: &str| {
        let Some(id) = app.id() else { return };
        let id = format!("app:{id}");
        if v.iter().any(|c| c.id == id) || verb.is_empty() { return; }
        v.push(Choice { id, main: verb.to_string(), note: format!("using {}", app.display_name()), app: Some(app), icon: "" });
    };
    for ct in m.content {
        for app in gio::AppInfo::all_for_type(ct) { add(app, m.content_verb); }
    }
    if !m.player.is_empty() {
        if let Some(app) = gio::AppInfo::default_for_type(m.player, false) { add(app, m.player_verb); }
    }
    if m.key == "software" {
        v.insert(0, Choice { id: "run".into(), main: "Install or run program from your media".into(), note: String::new(), app: None, icon: "system-run" });
    }
    if !is_blank(m) {
        let main = if m.device { "Open device to view files" } else { "Open folder to view files" };
        v.push(Choice { id: "open".into(), main: main.into(), note: "using Windows Explorer".into(), app: None, icon: "folder-open" });
    }
    if for_settings {
        v.push(Choice { id: "none".into(), main: "Take no action".into(), note: String::new(), app: None, icon: "process-stop" });
        v.push(Choice { id: "ask".into(), main: "Ask me every time".into(), note: String::new(), app: None, icon: "dialog-question" });
    }
    v
}

// ------------------------------------------------------------------ the shell side

/// A medium that just showed up.
#[derive(Clone)]
pub struct Target {
    pub name: String,
    pub icon: Option<gio::Icon>,
    pub uri: String,
    pub path: Option<PathBuf>,
    pub mount: Option<gio::Mount>,
}

thread_local! {
    static MONITOR: RefCell<Option<gio::VolumeMonitor>> = const { RefCell::new(None) };
    /// volumes that appeared recently: only media inserted now get AutoPlay, not a drive the
    /// user opened in Explorer later
    static RECENT: RefCell<Vec<(String, Instant)>> = const { RefCell::new(Vec::new()) };
    static OPEN: RefCell<Vec<gtk::Window>> = const { RefCell::new(Vec::new()) };
}

fn volume_key(v: &gio::Volume) -> String {
    v.identifier("unix-device").or_else(|| v.identifier("uuid")).map(|s| s.to_string()).unwrap_or_else(|| v.name().to_string())
}

fn removable(v: &gio::Volume) -> bool {
    v.drive().map(|d| d.is_removable() || d.is_media_removable() || d.can_eject()).unwrap_or(false) || v.can_eject()
}

/// Start watching for new media (called once from the shell).
pub fn watch() {
    if MONITOR.with(|m| m.borrow().is_some()) { return; }
    let vm = gio::VolumeMonitor::get();
    vm.connect_volume_added(|_, v| {
        if !removable(v) { return; }
        RECENT.with(|r| {
            let mut r = r.borrow_mut();
            r.retain(|(_, t)| t.elapsed() < Duration::from_secs(120));
            r.push((volume_key(v), Instant::now()));
        });
        // Windows mounts what you plug in; udiskie (started by the session when installed) does
        // the same, so leave it to that
        if v.get_mount().is_none() && v.can_mount() && v.should_automount() && !util::which("udiskie") {
            let op = gtk::MountOperation::new(None::<&gtk::Window>);
            v.mount(gio::MountMountFlags::NONE, Some(&op), gio::Cancellable::NONE, |r| {
                if let Err(e) = r { if !e.matches(gio::IOErrorEnum::FailedHandled) { eprintln!("aerode: could not mount: {e}"); } }
            });
        }
    });
    vm.connect_mount_added(|_, mnt| {
        if mnt.is_shadowed() { return; }
        let scheme = mnt.root().uri_scheme().map(|s| s.to_string()).unwrap_or_default();
        let device = matches!(scheme.as_str(), "mtp" | "gphoto2" | "afc");
        let fresh = match mnt.volume() {
            Some(v) => {
                let k = volume_key(&v);
                let hit = RECENT.with(|r| r.borrow().iter().any(|(key, t)| *key == k && t.elapsed() < Duration::from_secs(60)));
                RECENT.with(|r| r.borrow_mut().retain(|(key, _)| *key != k));
                hit
            }
            // no volume: gvfs mounts of phones/cameras and audio CDs
            None => device || scheme == "cdda",
        };
        if !fresh { return; }
        let t = Target { name: mnt.name().to_string(), icon: Some(mnt.icon()), uri: mnt.root().uri().to_string(), path: mnt.root().path(), mount: Some(mnt.clone()) };
        mnt.guess_content_type(false, gio::Cancellable::NONE, move |r| {
            let types: Vec<String> = r.map(|v| v.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
            autoplay(t, types, device);
        });
    });
    MONITOR.with(|m| *m.borrow_mut() = Some(vm));
}

/// `aerode autoplay DIR`: AutoPlay for a folder as if it were a medium that was just inserted
/// (for testing, and for scripts/udev rules that mount things themselves).
pub fn for_path(p: &Path) {
    let f = gio::File::for_path(p);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
    let t = Target { name, icon: Some(gio::ThemedIcon::new("drive-removable-media").upcast()), uri: f.uri().to_string(), path: Some(p.to_path_buf()), mount: None };
    // let gio sniff it like a mount (DCIM, VIDEO_TS, autorun…)
    let types: Vec<String> = gio::content_type_guess_for_tree(&f).iter().map(|s| s.to_string()).collect();
    autoplay(t, types, false);
}

fn autoplay(t: Target, types: Vec<String>, device: bool) {
    config::reload();
    let cfg = config::get();
    if !cfg.autoplay { return; }
    let path = t.path.clone();
    let has = move |ct: &str| types.iter().any(|x| x == ct);
    // the disc formats gio recognises outright
    let direct = [
        ("x-content/blank-cd", "blankcd"), ("x-content/blank-dvd", "blankdvd"), ("x-content/blank-bd", "blankbd"), ("x-content/blank-hddvd", "blankhddvd"),
        ("x-content/video-bluray", "bluray"), ("x-content/video-hddvd", "hddvd"), ("x-content/video-vcd", "vcd"), ("x-content/video-svcd", "svcd"),
        ("x-content/audio-dvd", "dvdaudio"),
    ];
    let mut key: Option<&'static str> = direct.iter().find(|(ct, _)| has(ct)).map(|(_, k)| *k);
    if key.is_none() && has("x-content/audio-cdda") {
        // an audio CD with a data session too is an "enhanced" one
        key = Some(if path.as_deref().map(|p| dir_has_files(p)).unwrap_or(false) { "enhancedaudiocd" } else { "audiocd" });
    }
    if key.is_none() && has("x-content/video-dvd") {
        key = Some(if path.as_deref().map(|p| extra_on_dvd(p)).unwrap_or(false) { "enhanceddvdmovie" } else { "dvdmovie" });
    }
    if key.is_none() && (device || t.uri.starts_with("mtp:") || t.uri.starts_with("gphoto2:")) { key = Some("devices"); }
    if key.is_none() && (has("x-content/unix-software") || has("x-content/win32-software") || has("x-content/software") || path.as_deref().and_then(autorun).is_some()) {
        key = Some("software");
    }
    if key.is_none() && (has("x-content/image-dcf") || has("x-content/image-picturecd")) { key = Some("pictures"); }
    if key.is_none() && has("x-content/audio-player") { key = Some("audiofiles"); }
    match (key, path) {
        (Some(k), _) => decide(t, media(k)),
        // a plain drive: look at what is on it (in a thread – it can be a big, slow stick)
        (None, Some(p)) => {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || { let _ = tx.send_blocking(scan(&p)); });
            glib::MainContext::default().spawn_local(async move {
                let k = rx.recv().await.ok().flatten();
                decide(t, k.and_then(media));
            });
        }
        (None, None) => decide(t, None),
    }
}

fn dir_has_files(p: &Path) -> bool { std::fs::read_dir(p).map(|mut r| r.next().is_some()).unwrap_or(false) }

fn extra_on_dvd(p: &Path) -> bool {
    std::fs::read_dir(p).map(|r| r.flatten().any(|e| {
        let n = e.file_name().to_string_lossy().to_uppercase();
        !matches!(n.as_str(), "VIDEO_TS" | "AUDIO_TS" | "JACKET_P")
    })).unwrap_or(false)
}

/// An autorun program on the medium (freedesktop autostart spec, or a Windows autorun.inf).
fn autorun(root: &Path) -> Option<PathBuf> {
    [".autorun", "autorun", "autorun.sh", "autorun.inf", "AUTORUN.INF", "Autorun.inf"].iter().map(|n| root.join(n)).find(|p| p.is_file())
}

/// Classify a drive by the files on it: pictures / video / audio, or mixed content.
fn scan(root: &Path) -> Option<&'static str> {
    const IMG: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "raf"];
    const VID: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "webm", "m4v", "mpg", "mpeg", "3gp", "flv", "mts", "m2ts", "vob", "ogv"];
    const AUD: &[&str] = &["mp3", "flac", "ogg", "oga", "opus", "wav", "m4a", "aac", "wma", "ape", "alac", "aiff", "mka"];
    let (mut i, mut v, mut a, mut seen) = (0u32, 0u32, 0u32, 0u32);
    let start = Instant::now();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 4000 || start.elapsed() > Duration::from_secs(4) { stack.clear(); break; }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') || name == "system volume information" || name == "$recycle.bin" { continue; }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() { if depth < 4 { stack.push((e.path(), depth + 1)); } continue; }
            let ext = name.rsplit_once('.').map(|(_, x)| x).unwrap_or("");
            if IMG.contains(&ext) { i += 1; } else if VID.contains(&ext) { v += 1; } else if AUD.contains(&ext) { a += 1; }
        }
    }
    match ((i > 0) as u8 + (v > 0) as u8 + (a > 0) as u8, i > 0, v > 0) {
        (0, _, _) => None,
        (1, true, _) => Some("pictures"),
        (1, _, true) => Some("videofiles"),
        (1, _, _) => Some("audiofiles"),
        _ => Some("mixed"),
    }
}

fn decide(t: Target, m: Option<&'static Media>) {
    let action = m.and_then(|m| config::get().autoplay_actions.get(m.key).cloned()).unwrap_or_else(|| "ask".into());
    match action.as_str() {
        "none" => {}
        "ask" => show(t, m),
        a => {
            // a default whose program was uninstalled since: ask instead
            if a.starts_with("app:") && util::app_by_id(&a[4..]).is_none() { show(t, m); } else { act(a, &t, None); }
        }
    }
}

/// Do what was chosen.
fn act(action: &str, t: &Target, parent: Option<&gtk::Window>) {
    match action {
        "open" => match &t.path { Some(p) => util::explore(&p.to_string_lossy()), None => util::explore(&t.uri) },
        "run" => run_autorun(t, parent),
        "backup" => { util::spawn(&["aerode-control", "backup"]); }
        a if a.starts_with("app:") => {
            let Some(app) = util::app_by_id(&a[4..]) else { return };
            let ctx = gtk::gdk::Display::default().and_then(|d| d.app_launch_context());
            if let Err(e) = app.launch_uris(&[t.uri.as_str()], ctx.as_ref()) {
                util::error(&format!("Windows cannot open '{}'.", app.display_name()), &e.to_string());
            }
        }
        _ => {}
    }
}

/// "Install or run program from your media": always ask first (the freedesktop autostart spec
/// requires it, and Windows shows a UAC/security prompt here too).
fn run_autorun(t: &Target, parent: Option<&gtk::Window>) {
    let Some(root) = t.path.clone() else { return };
    let Some(file) = autorun(&root) else { util::explore(&root.to_string_lossy()); return };
    let is_inf = file.extension().map(|e| e.eq_ignore_ascii_case("inf")).unwrap_or(false);
    // autorun.inf: open=setup.exe → run it with Wine
    let argv: Vec<String> = if is_inf {
        let inf = std::fs::read_to_string(&file).unwrap_or_default();
        let open = inf.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            matches!(k.trim().to_lowercase().as_str(), "open" | "shellexecute").then(|| v.trim().to_string())
        });
        match open {
            Some(o) if util::which("wine") => {
                let exe = o.split_whitespace().next().unwrap_or("").replace('\\', "/");
                vec!["wine".into(), root.join(exe).to_string_lossy().into_owned()]
            }
            Some(_) => { util::error("This program needs Wine to run.", "Install the wine package to run Windows programs from discs and drives."); return; }
            None => { util::explore(&root.to_string_lossy()); return; }
        }
    } else {
        vec!["sh".into(), file.to_string_lossy().into_owned()]
    };
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::None,
        &format!("Do you want to allow the following program from \"{}\" to run?", t.name));
    d.set_title("AutoPlay");
    d.set_secondary_text(Some(&format!("Program name: {}\n\nOnly run programs from media you trust. Programs from an unknown source can harm your computer.", file.display())));
    d.add_button("_Run", gtk::ResponseType::Yes);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.set_default_response(gtk::ResponseType::Cancel);
    d.set_keep_above(true);
    util::sound(crate::sounds::Event::Exclamation);
    let r = d.run();
    d.close();
    if r != gtk::ResponseType::Yes { return; }
    let _ = std::process::Command::new(&argv[0]).args(&argv[1..]).current_dir(&root)
        .stdin(std::process::Stdio::null()).spawn().map(|mut c| { std::thread::spawn(move || { let _ = c.wait(); }); });
}

const CSS: &str = r#"
.ap-body { background-color: #ffffff; }
.ap-name { font-size: 11pt; }
.ap-section { color: #1e3287; }
.ap-rule { background-color: #d5dfe5; min-height: 1px; }
button.ap-choice { background: none; border: 1px solid transparent; border-radius: 3px; box-shadow: none; padding: 3px 6px; }
button.ap-choice:hover { background-image: linear-gradient(to bottom, #f5faff, #dcecfd); border-color: #b5d3f6; }
button.ap-choice label.ap-main { color: #000000; }
button.ap-choice label.ap-note { color: #6d6d6d; }
button.ap-link { background: none; border: none; box-shadow: none; padding: 0; }
button.ap-link label { color: #0066cc; }
button.ap-link:hover label { color: #3399ff; text-decoration: underline; }
"#;

fn section(text: &str) -> gtk::Widget {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let l = gtk::Label::new(Some(text));
    l.style_context().add_class("ap-section");
    h.pack_start(&l, false, false, 0);
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.style_context().add_class("ap-rule");
    rule.set_valign(gtk::Align::Center);
    h.pack_start(&rule, true, true, 0);
    h.set_margin_top(8);
    h.upcast()
}

fn choice_button(c: &Choice) -> gtk::Button {
    let b = gtk::Button::new();
    b.style_context().add_class("ap-choice");
    b.set_relief(gtk::ReliefStyle::None);
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let img = match &c.app { Some(a) => util::app_image(a, 32), None => util::image(util::first_icon(&[c.icon, "folder"]), 32) };
    h.pack_start(&img, false, false, 0);
    let v = gtk::Box::new(gtk::Orientation::Vertical, 0);
    v.set_valign(gtk::Align::Center);
    let a = gtk::Label::new(Some(&c.main));
    a.set_xalign(0.0);
    a.style_context().add_class("ap-main");
    v.pack_start(&a, false, false, 0);
    if !c.note.is_empty() {
        let n = gtk::Label::new(Some(&c.note));
        n.set_xalign(0.0);
        n.style_context().add_class("ap-note");
        v.pack_start(&n, false, false, 0);
    }
    h.pack_start(&v, true, true, 0);
    b.add(&h);
    b
}

/// The Windows 7 AutoPlay window.
pub fn show(t: Target, m: Option<&'static Media>) {
    thread_local!(static CSS_DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if !CSS_DONE.with(|c| c.replace(true)) { crate::theme::load_css(CSS); }
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title("AutoPlay");
    w.set_icon_name(Some(util::first_icon(&["media-optical", "drive-removable-media"])));
    w.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    w.set_resizable(false);
    w.set_keep_above(true);
    w.set_position(gtk::WindowPosition::Center);
    w.set_default_size(372, -1);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.style_context().add_class("ap-body");
    body.set_border_width(12);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = t.icon.as_ref().and_then(|i| util::gicon_pixbuf(Some(i), 48)).map(|p| gtk::Image::from_pixbuf(Some(&p))).unwrap_or_else(|| util::image("drive-removable-media", 48));
    head.pack_start(&icon, false, false, 0);
    let name = gtk::Label::new(Some(&t.name));
    name.style_context().add_class("ap-name");
    name.set_xalign(0.0);
    name.set_line_wrap(true);
    head.pack_start(&name, true, true, 0);
    body.pack_start(&head, false, false, 0);
    let always = gtk::CheckButton::with_label(&format!("Always do this for {}:", m.map(|m| m.plural).unwrap_or("")));
    if m.is_some() { body.pack_start(&always, false, false, 6); }
    let all = m.map(|m| choices(m, false)).unwrap_or_default();
    let (general, specific): (Vec<Choice>, Vec<Choice>) = all.into_iter().partition(|c| c.id == "open");
    let add = |c: &Choice, body: &gtk::Box| {
        let b = choice_button(c);
        let (w, t, id, always, key) = (w.clone(), t.clone(), c.id.clone(), always.clone(), m.map(|m| m.key));
        b.connect_clicked(move |_| {
            if let (true, Some(k)) = (always.is_active(), key) {
                config::reload();
                config::update(|cf| { cf.autoplay_actions.insert(k.to_string(), id.clone()); });
            }
            w.hide();
            act(&id, &t, None);
            w.close();
        });
        body.pack_start(&b, false, false, 0);
    };
    if let (Some(m), false) = (m, specific.is_empty()) {
        body.pack_start(&section(&format!("{} options", m.name)), false, false, 0);
        for c in &specific { add(c, &body); }
    }
    let mut general = if general.is_empty() && m.map(|m| !is_blank(m)).unwrap_or(true) {
        vec![Choice { id: "open".into(), main: "Open folder to view files".into(), note: "using Windows Explorer".into(), app: None, icon: "folder-open" }]
    } else { general };
    // a USB drive or memory card can take the backup (Backup and Restore)
    if t.path.is_some() && m.map(|m| !is_disc(m) && !m.device).unwrap_or(true) {
        general.push(Choice { id: "backup".into(), main: "Use this drive for backup".into(), note: "using Windows Backup".into(), app: None, icon: "drive-harddisk" });
    }
    if !general.is_empty() {
        body.pack_start(&section("General options"), false, false, 0);
        for c in &general { add(c, &body); }
    }
    let more = gtk::Button::with_label("View more AutoPlay options in Control Panel");
    more.style_context().add_class("ap-link");
    more.set_relief(gtk::ReliefStyle::None);
    more.set_halign(gtk::Align::Start);
    more.set_margin_top(10);
    { let w = w.clone(); more.connect_clicked(move |_| { util::spawn(&["aerode-control", "autoplay"]); w.close(); }); }
    body.pack_start(&more, false, false, 0);
    w.add(&body);
    // the medium was taken out again: nothing left to do
    if let Some(mnt) = &t.mount {
        let w2 = w.downgrade();
        mnt.connect_unmounted(move |_| if let Some(w) = w2.upgrade() { w.close(); });
    }
    OPEN.with(|o| o.borrow_mut().push(w.clone()));
    w.connect_destroy(|w| OPEN.with(|o| o.borrow_mut().retain(|x| x != w)));
    w.show_all();
    w.present();
}
