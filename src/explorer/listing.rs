//! Folder listing off the GUI thread: directories, the Recycle Bin, search and Recent Places are
//! read by worker threads and handed to the window in batches; thumbnails are made by a worker too
//! (and kept in the freedesktop thumbnail cache, like Win7 keeps thumbcache.db).
use gtk::prelude::*;
use gtk::{gio, glib};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::trash;

/// One file as the views show it.
#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub broken: bool,
    pub size: u64,
    pub mtime: i64,
    pub ctime: i64,
    pub ctype: String,
    pub desc: String,
    pub hidden: bool,
    /// Icon= of a .desktop file
    pub icon_name: Option<String>,
    /// Name= of a .desktop file
    pub app_name: Option<String>,
    /// Recycle Bin: original location and deletion time
    pub trash: Option<(PathBuf, i64)>,
    /// the texts the views show, made by `render` (on the worker thread for listings)
    pub text: RowText,
}

/// Everything a row shows besides its icon, prepared off the GUI thread.
#[derive(Clone, Debug, Default)]
pub struct RowText {
    /// shown name (Folder Options can hide known extensions)
    pub name: String,
    pub date: String,
    /// Details "Size": "1 KB"
    pub size: String,
    pub uri: String,
    /// Tiles markup: name, type, size
    pub tile: String,
    /// Content view markup: date and size
    pub content: String,
    /// Content view markup: name and type
    pub content_name: String,
    /// name order key (see `natural_key`): lowercase shown name, then the real name
    pub key: Vec<u8>,
    /// lowercase type, for sorting by Type
    pub type_key: String,
    /// "Original Location" (Recycle Bin) or "Folder" (search results)
    pub orig: String,
    pub del: String,
}

/// How listings are made: Folder Options that change what rows show.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanOpts {
    pub hide_ext: bool,
    pub show_hidden: bool,
}

thread_local! {
    static KNOWN: std::cell::RefCell<HashMap<String, bool>> = std::cell::RefCell::new(HashMap::new());
}

pub fn fmt_date(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map(|d| d.with_timezone(&chrono::Local).format("%-m/%-d/%Y %-I:%M %p").to_string()).unwrap_or_default()
}

/// Details view sizes: "1 KB" rounded up, thousands separated.
pub fn kb(n: u64) -> String {
    let k = n.div_ceil(1024);
    let s = k.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); }
    format!("{o} KB")
}

/// Folder Options > Hide extensions for known file types: known = some program opens the type
/// the extension stands for (judged by the name, like Windows).
pub fn shown_name(e: &Entry, hide_ext: bool) -> String {
    if let Some(a) = &e.app_name { return a.clone(); }
    if e.trash.is_some() || e.is_dir || !hide_ext { return e.name.clone(); }
    let Some(dot) = e.name.rfind('.').filter(|i| *i > 0 && *i + 1 < e.name.len()) else { return e.name.clone() };
    let (ct, uncertain) = gio::content_type_guess(Some(&e.name), &[]);
    if uncertain || ct == "application/octet-stream" { return e.name.clone(); }
    let ct = ct.to_string();
    let known = KNOWN.with(|k| *k.borrow_mut().entry(ct.clone()).or_insert_with(|| !gio::AppInfo::all_for_type(&ct).is_empty()));
    if known { e.name[..dot].to_string() } else { e.name.clone() }
}

/// A byte string whose plain order is Explorer's "logical" order ("file2" < "file10"): digit
/// runs become '0', their length (without leading zeros) as two bytes, then the digits. The
/// '0' keeps a number's place relative to other characters, the length orders the values.
pub fn natural_key(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len() + 8);
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() { i += 1; }
            let run = &b[start..i];
            let sig = { let z = run.iter().take_while(|c| **c == b'0').count(); if z == run.len() { &run[run.len() - 1..] } else { &run[z..] } };
            let n = sig.len().min(0xffff) as u16;
            out.push(b'0');
            out.extend_from_slice(&n.to_be_bytes());
            out.extend_from_slice(sig);
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

impl Entry {
    /// Would a view show `o` exactly like `self`?
    pub fn same_look(&self, o: &Entry) -> bool {
        self.mtime == o.mtime && self.ctime == o.ctime && self.size == o.size && self.is_dir == o.is_dir && self.is_link == o.is_link && self.broken == o.broken
            && self.hidden == o.hidden && self.ctype == o.ctype && self.text.name == o.text.name && self.trash == o.trash && self.icon_name == o.icon_name
    }

    /// Fill in `text` (any thread).
    pub fn render(&mut self, hide_ext: bool) {
        // control characters (a newline in a name) would break the rows
        let name: String = shown_name(self, hide_ext).chars().map(|c| if c.is_control() { '?' } else { c }).collect();
        let esc = |s: &str| glib::markup_escape_text(s).to_string();
        let gray = |s: &str| format!("<span foreground='#6d6d6d'>{}</span>", esc(s));
        let date = fmt_date(self.mtime);
        let human = crate::util::human_size(self.size);
        let tile = format!("{}\n{}{}", esc(&name), gray(&self.desc), if self.is_dir { String::new() } else { format!("\n{}", gray(&human)) });
        let content = if self.is_dir { format!("{} {}", gray("Date modified:"), esc(&date)) } else {
            format!("{} {}\n{} {}", gray("Date modified:"), esc(&date), gray("Size:"), esc(&human))
        };
        let (orig, del) = match &self.trash {
            Some((o, d)) => (o.display().to_string(), fmt_date(*d)),
            None => (self.path.parent().map(|p| p.display().to_string()).unwrap_or_default(), String::new()),
        };
        self.text = RowText {
            key: { let mut k = natural_key(&name.to_lowercase()); k.push(1); k.extend_from_slice(self.name.as_bytes()); k },
            type_key: self.desc.to_lowercase(),
            size: if self.is_dir { String::new() } else { kb(self.size) },
            uri: gio::File::for_path(&self.path).uri().to_string(),
            content_name: format!("{}\n{}", esc(&name), gray(&self.desc)),
            name, date, tile, content, orig, del,
        };
    }
}

thread_local! {
    static DESC: std::cell::RefCell<HashMap<String, String>> = std::cell::RefCell::new(HashMap::new());
}

/// Windows-style type names ("Text Document", "File folder", "ZIP File"…).
pub fn describe(ct: &str, name: &str, is_dir: bool) -> String {
    if is_dir { return "File folder".into(); }
    let ext = trash::split_ext(name).1.trim_start_matches('.').to_uppercase();
    let win = match ct {
        "text/plain" | "text/x-log" => Some("Text Document"),
        "application/zip" => Some("Compressed (zipped) Folder"),
        "application/x-desktop" => Some("Shortcut"),
        "inode/symlink" => Some("Shortcut"),
        "application/x-executable" | "application/x-pie-executable" | "application/x-sharedlib" if ext.is_empty() => Some("Application"),
        "application/x-shellscript" => Some("Shell Script"),
        "application/vnd.appimage" => Some("Application"),
        "image/jpeg" => Some("JPEG image"),
        "image/png" => Some("PNG image"),
        "image/bmp" => Some("Bitmap image"),
        "image/gif" => Some("GIF image"),
        _ => None,
    };
    if let Some(w) = win { return w.into(); }
    if ct == "application/octet-stream" || ct.is_empty() || ct == "application/x-zerosize" {
        return if ext.is_empty() { "File".into() } else { format!("{ext} File") };
    }
    DESC.with(|d| d.borrow_mut().entry(ct.to_string()).or_insert_with(|| {
        let s = gio::content_type_get_description(ct).to_string();
        // GIO says "plain text document": Windows capitalises type names
        let mut c = s.chars();
        match c.next() { Some(f) => f.to_uppercase().collect::<String>() + c.as_str(), None => s }
    }).clone())
}

fn desktop_keys(p: &Path) -> (Option<String>, Option<String>) {
    let Ok(t) = std::fs::read_to_string(p) else { return (None, None) };
    let mut icon = None;
    let mut name = None;
    let mut in_main = false;
    for l in t.lines() {
        let l = l.trim();
        if l.starts_with('[') { in_main = l == "[Desktop Entry]"; continue; }
        if !in_main { continue; }
        if let Some(v) = l.strip_prefix("Icon=") { icon = Some(v.to_string()); }
        if let Some(v) = l.strip_prefix("Name=") { name = Some(v.to_string()); }
    }
    (icon, name)
}

/// stat + type detection for one path (any thread).
pub fn stat_entry(path: &Path, hidden_list: Option<&[String]>) -> Option<Entry> {
    let lm = std::fs::symlink_metadata(path).ok()?;
    let is_link = lm.file_type().is_symlink();
    let (m, broken) = if is_link { match std::fs::metadata(path) { Ok(m) => (m, false), Err(_) => (lm.clone(), true) } } else { (lm.clone(), false) };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let is_dir = m.is_dir();
    let ctype = if is_dir { "inode/directory".to_string() } else if broken { "inode/symlink".to_string() } else if !m.is_file() {
        match m.mode() & libc::S_IFMT { libc::S_IFIFO => "inode/fifo", libc::S_IFSOCK => "inode/socket", libc::S_IFCHR => "inode/chardevice", libc::S_IFBLK => "inode/blockdevice", _ => "application/octet-stream" }.to_string()
    } else {
        let (ct, uncertain) = gio::content_type_guess(Some(&name), &[]);
        if uncertain && m.len() > 0 && m.len() < 1 << 40 {
            // no telling extension: look at the start of the file like GIO does
            let mut buf = vec![0u8; 4096];
            let n = std::fs::File::open(path).and_then(|mut f| { use std::io::Read; f.read(&mut buf) }).unwrap_or(0);
            if n > 0 { gio::content_type_guess(Some(&name), &buf[..n]).0.to_string() } else { ct.to_string() }
        } else if m.len() == 0 && uncertain { "application/x-zerosize".to_string() } else { ct.to_string() }
    };
    let hidden = name.starts_with('.') || hidden_list.map(|h| h.iter().any(|x| x == &name)).unwrap_or(false);
    let (icon_name, app_name) = if ctype == "application/x-desktop" && m.len() < 256 * 1024 { desktop_keys(path) } else { (None, None) };
    // a symbolic link to a file is what Windows calls a shortcut
    let desc = if is_link && !is_dir { "Shortcut".to_string() } else { describe(&ctype, &name, is_dir) };
    Some(Entry { path: path.to_path_buf(), name, is_dir, is_link, broken, size: if is_dir { 0 } else { m.len() }, mtime: m.mtime(), ctime: lm.ctime(),
        ctype, desc, hidden, icon_name, app_name, trash: None, text: RowText::default() })
}

/// Names listed in a folder's `.hidden` file (the GTK/Nautilus convention).
pub fn hidden_list(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join(".hidden")).map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default()
}

pub enum ScanMsg {
    Batch(Vec<Entry>),
    Error(std::io::Error),
    Done,
}

/// Sends entries in batches: the first quickly (so small folders appear at once), then bigger ones.
struct Batcher { tx: async_channel::Sender<ScanMsg>, buf: Vec<Entry>, last: Instant, first: bool }

impl Batcher {
    fn new(tx: async_channel::Sender<ScanMsg>) -> Self { Batcher { tx, buf: vec![], last: Instant::now(), first: true } }
    fn push(&mut self, e: Entry) -> bool {
        self.buf.push(e);
        let wait = if self.first { Duration::from_millis(120) } else { Duration::from_millis(400) };
        if self.buf.len() >= 4000 || self.last.elapsed() >= wait { return self.flush(); }
        true
    }
    fn flush(&mut self) -> bool {
        self.first = false;
        self.last = Instant::now();
        if self.buf.is_empty() { return true; }
        self.tx.send_blocking(ScanMsg::Batch(std::mem::take(&mut self.buf))).is_ok()
    }
}

impl ScanOpts {
    /// stat + render, None for missing files and hidden ones that are not shown
    fn entry(&self, p: &Path, hidden_list: Option<&[String]>) -> Option<Entry> {
        let mut e = stat_entry(p, hidden_list)?;
        if e.hidden && !self.show_hidden { return None; }
        e.render(self.hide_ext);
        Some(e)
    }
}

/// List a folder.
pub fn scan_dir(dir: PathBuf, opts: ScanOpts, cancel: Arc<AtomicBool>) -> async_channel::Receiver<ScanMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let rd = match std::fs::read_dir(&dir) { Ok(r) => r, Err(e) => { let _ = tx.send_blocking(ScanMsg::Error(e)); return; } };
        let hidden = hidden_list(&dir);
        let mut b = Batcher::new(tx.clone());
        for e in rd.flatten() {
            if cancel.load(Ordering::Relaxed) { return; }
            if let Some(en) = opts.entry(&e.path(), Some(&hidden)) { if !b.push(en) { return; } }
        }
        b.flush();
        let _ = tx.send_blocking(ScanMsg::Done);
    });
    rx
}

/// Live updates: read the changed files of folder `dir` again. Yields (path, entry) – None
/// when the file is gone (or hidden and not shown).
pub fn stat_paths(dir: PathBuf, paths: Vec<PathBuf>, opts: ScanOpts) -> async_channel::Receiver<Vec<(PathBuf, Option<Entry>)>> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let hidden = hidden_list(&dir);
        let v = paths.into_iter().map(|p| { let e = opts.entry(&p, Some(&hidden)); (p, e) }).collect();
        let _ = tx.send_blocking(v);
    });
    rx
}

/// List the Recycle Bin (all trash directories).
pub fn scan_trash(opts: ScanOpts, cancel: Arc<AtomicBool>) -> async_channel::Receiver<ScanMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let mut b = Batcher::new(tx.clone());
        for t in trash::list() {
            if cancel.load(Ordering::Relaxed) { return; }
            if let Some(mut en) = stat_entry(&t.file, None) {
                // shown under its original name
                en.name = t.orig.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(en.name);
                en.hidden = false;
                en.desc = describe(&en.ctype, &en.name, en.is_dir);
                en.trash = Some((t.orig.parent().map(|p| p.to_path_buf()).unwrap_or_default(), t.deleted));
                en.render(opts.hide_ext);
                if !b.push(en) { return; }
            }
        }
        b.flush();
        let _ = tx.send_blocking(ScanMsg::Done);
    });
    rx
}

/// Recent Places: folders of recently used files and recently visited folders.
pub fn scan_paths(paths: Vec<PathBuf>, opts: ScanOpts, cancel: Arc<AtomicBool>) -> async_channel::Receiver<ScanMsg> {
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let mut b = Batcher::new(tx.clone());
        for p in paths {
            if cancel.load(Ordering::Relaxed) { return; }
            let o = ScanOpts { show_hidden: true, ..opts };
            if let Some(en) = o.entry(&p, None) { if !b.push(en) { return; } }
        }
        b.flush();
        let _ = tx.send_blocking(ScanMsg::Done);
    });
    rx
}

/// Windows search box: names containing the words (or whole words without partial matches), in
/// this folder and subfolders, streamed as they are found.
pub fn search(base: PathBuf, query: String, partial: bool, subfolders: bool, opts: ScanOpts, cancel: Arc<AtomicBool>) -> async_channel::Receiver<ScanMsg> {
    let hidden = opts.show_hidden;
    let (tx, rx) = async_channel::unbounded();
    std::thread::spawn(move || {
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(|w| w.trim_matches('"').to_string()).filter(|w| !w.is_empty()).collect();
        let mut b = Batcher::new(tx.clone());
        let mut stack = vec![base.clone()];
        let mut found = 0;
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                if cancel.load(Ordering::Relaxed) { return; }
                let name = e.file_name().to_string_lossy().to_lowercase();
                if name.starts_with('.') && !hidden { continue; }
                let hit = !words.is_empty() && words.iter().all(|w| {
                    if partial { name.contains(w.as_str()) } else { name.split(|c: char| !c.is_alphanumeric()).any(|x| x == w) || &name == w }
                });
                if hit {
                    if let Some(en) = opts.entry(&e.path(), None) { found += 1; if !b.push(en) { return; } }
                    if found >= 20000 { b.flush(); let _ = tx.send_blocking(ScanMsg::Done); return; }
                }
                // not into the kernel's virtual folders (huge and endless), unless searching there
                if subfolders && e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let p = e.path();
                    if !["/proc", "/sys", "/dev"].iter().any(|v| p == Path::new(v) && !base.starts_with(v)) { stack.push(p); }
                }
            }
            // keep the first results coming even in huge trees
            if !b.flush_if_due() { return; }
        }
        b.flush();
        let _ = tx.send_blocking(ScanMsg::Done);
    });
    rx
}

impl Batcher {
    fn flush_if_due(&mut self) -> bool {
        let wait = if self.first { Duration::from_millis(120) } else { Duration::from_millis(400) };
        if !self.buf.is_empty() && self.last.elapsed() >= wait { return self.flush(); }
        true
    }
}

// ------------------------------------------------------------------ thumbnails

#[derive(Clone, Copy, PartialEq)]
pub enum ThumbKind { Image, Video }

pub fn thumb_kind(ct: &str) -> Option<ThumbKind> {
    if ct.starts_with("image/") && ct != "image/x-icns" { return Some(ThumbKind::Image); }
    if ct.starts_with("video/") { return Some(ThumbKind::Video); }
    None
}

pub struct ThumbReq { pub generation: u32, pub path: PathBuf, pub mtime: i64, pub size: i32, pub kind: ThumbKind }

pub struct ThumbResult { pub generation: u32, pub path: PathBuf, pub mtime: i64, pub size: i32, pub kind: ThumbKind, pub data: glib::Bytes, pub w: i32, pub h: i32, pub alpha: bool, pub stride: i32 }

/// A worker thread that makes thumbnails; requests of an older generation are dropped.
pub struct Thumbnailer {
    tx: std::sync::mpsc::Sender<ThumbReq>,
    pub current: Arc<AtomicU32>,
}

fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("thumbnails")
}

fn make_thumb(r: &ThumbReq) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    use gtk::gdk_pixbuf::Pixbuf;
    let uri = gio::File::for_path(&r.path).uri().to_string();
    let md5 = glib::compute_checksum_for_data(glib::ChecksumType::Md5, uri.as_bytes())?;
    let (flavor, px) = if r.size > 128 { ("large", 256) } else { ("normal", 128) };
    let cached = cache_dir().join(flavor).join(format!("{md5}.png"));
    if let Ok(pb) = Pixbuf::from_file(&cached) {
        if pb.option("tEXt::Thumb::MTime").map(|m| m.parse::<i64>().ok() == Some(r.mtime)).unwrap_or(false) { return Some(pb); }
    }
    let pb = match r.kind {
        ThumbKind::Image => {
            // don't decode giant files just for an icon
            let len = std::fs::metadata(&r.path).map(|m| m.len()).unwrap_or(0);
            if len > 200 * 1024 * 1024 { return None; }
            let (_, w, h) = Pixbuf::file_info(&r.path)?;
            if w <= 0 || h <= 0 { return None; }
            // small pictures are not enlarged
            if w <= px && h <= px { Pixbuf::from_file(&r.path).ok()?.apply_embedded_orientation() } else { Pixbuf::from_file_at_scale(&r.path, px, px, true).ok()?.apply_embedded_orientation() }?
        }
        ThumbKind::Video => {
            let tmp = std::env::temp_dir().join(format!("aerode-thumb-{}-{md5}.png", std::process::id()));
            let ok = if crate::util::which("ffmpegthumbnailer") {
                std::process::Command::new("ffmpegthumbnailer").arg("-i").arg(&r.path).arg("-o").arg(&tmp).arg("-s").arg(px.to_string()).arg("-c").arg("png")
                    .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
            } else if crate::util::which("ffmpeg") {
                std::process::Command::new("ffmpeg").args(["-v", "quiet", "-y", "-i"]).arg(&r.path)
                    .args(["-vf", &format!("thumbnail,scale='min({px},iw)':-1"), "-frames:v", "1"]).arg(&tmp)
                    .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
            } else { false };
            let pb = if ok { Pixbuf::from_file_at_scale(&tmp, px, px, true).ok() } else { None };
            let _ = std::fs::remove_file(&tmp);
            pb?
        }
    };
    // store it for next time (and for other file managers)
    let dir = cache_dir().join(flavor);
    if std::fs::create_dir_all(&dir).is_ok() {
        let tmp = dir.join(format!(".{md5}.{}.png", std::process::id()));
        let mt = r.mtime.to_string();
        if pb.savev(&tmp, "png", &[("tEXt::Thumb::URI", uri.as_str()), ("tEXt::Thumb::MTime", mt.as_str()), ("tEXt::Software", "aerode")]).is_ok() {
            let _ = std::fs::rename(&tmp, &cached);
        } else { let _ = std::fs::remove_file(&tmp); }
    }
    Some(pb)
}

impl Thumbnailer {
    pub fn new(on_result: impl Fn(ThumbResult) + 'static) -> Thumbnailer {
        let (tx, rx) = std::sync::mpsc::channel::<ThumbReq>();
        let (rtx, rrx) = async_channel::unbounded::<ThumbResult>();
        let current = Arc::new(AtomicU32::new(0));
        let cur = current.clone();
        std::thread::spawn(move || {
            while let Ok(r) = rx.recv() {
                if r.generation != cur.load(Ordering::Relaxed) { continue; }
                let Some(pb) = make_thumb(&r) else { continue };
                // scale to the size asked for (the cache holds 128 or 256 px)
                let s = r.size;
                let (w, h) = (pb.width(), pb.height());
                let pb = if w > s || h > s {
                    let f = (s as f64 / w as f64).min(s as f64 / h as f64);
                    pb.scale_simple(((w as f64 * f).round() as i32).max(1), ((h as f64 * f).round() as i32).max(1), gtk::gdk_pixbuf::InterpType::Bilinear).unwrap_or(pb)
                } else { pb };
                let res = ThumbResult { generation: r.generation, path: r.path, mtime: r.mtime, size: r.size, kind: r.kind, data: pb.read_pixel_bytes(), w: pb.width(), h: pb.height(), alpha: pb.has_alpha(), stride: pb.rowstride() };
                if rtx.send_blocking(res).is_err() { return; }
            }
        });
        glib::MainContext::default().spawn_local(async move {
            while let Ok(r) = rrx.recv().await { on_result(r); }
        });
        Thumbnailer { tx, current }
    }

    pub fn request(&self, r: ThumbReq) { let _ = self.tx.send(r); }

    pub fn tx_clone(&self) -> std::sync::mpsc::Sender<ThumbReq> { self.tx.clone() }

    /// Forget queued requests (a new folder or view).
    pub fn new_generation(&self) -> u32 { self.current.fetch_add(1, Ordering::Relaxed) + 1 }
}

pub fn result_pixbuf(r: &ThumbResult) -> gtk::gdk_pixbuf::Pixbuf {
    gtk::gdk_pixbuf::Pixbuf::from_bytes(&r.data, gtk::gdk_pixbuf::Colorspace::Rgb, r.alpha, 8, r.w, r.h, r.stride)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names() {
        assert_eq!(describe("inode/directory", "x", true), "File folder");
        assert_eq!(describe("application/octet-stream", "data.bin", false), "BIN File");
        assert_eq!(describe("application/octet-stream", "data", false), "File");
        assert_eq!(describe("text/plain", "a.txt", false), "Text Document");
    }

    #[test]
    fn natural_keys() {
        let mut v = vec!["file10.txt", "file2.txt", "file1.txt", "a", "file02.txt", "file", "file 3", "b1", "a10b", "a9b", "ä"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, vec!["a", "a9b", "a10b", "b1", "file", "file 3", "file1.txt", "file2.txt", "file02.txt", "file10.txt", "ä"]);
        assert!(natural_key("x99999999999999999999") > natural_key("x100"));
    }

    #[test]
    fn stat_odd_names() {
        use std::os::unix::ffi::OsStrExt;
        let d = std::env::temp_dir().join(format!("aerode-listing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let bad = d.join(std::ffi::OsStr::from_bytes(b"bad\xffname.txt"));
        std::fs::write(&bad, "x").unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", d.join("broken")).unwrap();
        let e = stat_entry(&bad, None).unwrap();
        assert_eq!(e.path, bad);
        assert!(!e.is_dir && e.size == 1);
        let b = stat_entry(&d.join("broken"), None).unwrap();
        assert!(b.is_link && b.broken);
        let _ = std::fs::remove_dir_all(&d);
    }
}
