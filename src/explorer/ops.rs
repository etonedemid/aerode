//! File operations like Windows 7 Explorer: copy/move/delete run on a worker thread and report to
//! the Win7 progress dialog (with More details, Cancel), conflicts ("Copy and Replace / Don't copy /
//! Copy, but keep both files", folder merge), "Interrupted Action" errors, the Recycle Bin, Undo.
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::trash;
use crate::{sounds, util};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { Copy, Move }

// ------------------------------------------------------------------ clipboard

/// Put files on the clipboard for cut/copy, in the formats GNOME, KDE and other apps read.
pub fn set_clipboard(files: &[PathBuf], cut: bool) {
    let cb = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    let uris: Vec<String> = files.iter().map(|p| gio::File::for_path(p).uri().to_string()).collect();
    let gnome = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
    let uri_list = uris.join("\r\n") + "\r\n";
    let targets = [
        gtk::TargetEntry::new("x-special/gnome-copied-files", gtk::TargetFlags::empty(), 0),
        gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 1),
        gtk::TargetEntry::new("application/x-kde-cutselection", gtk::TargetFlags::empty(), 3),
        gtk::TargetEntry::new("UTF8_STRING", gtk::TargetFlags::empty(), 2),
        gtk::TargetEntry::new("text/plain;charset=utf-8", gtk::TargetFlags::empty(), 2),
    ];
    let plain = files.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    cb.set_with_data(&targets, move |_, sel, info| {
        match info {
            0 => sel.set(&gdk::Atom::intern("x-special/gnome-copied-files"), 8, gnome.as_bytes()),
            1 => sel.set(&gdk::Atom::intern("text/uri-list"), 8, uri_list.as_bytes()),
            3 => sel.set(&gdk::Atom::intern("application/x-kde-cutselection"), 8, if cut { b"1" } else { b"0" }),
            _ => { sel.set_text(&plain); }
        }
    });
    CUT.with(|c| *c.borrow_mut() = if cut { files.to_vec() } else { vec![] });
}

thread_local! {
    /// files last cut in this process (Explorer draws them faded until they are pasted)
    static CUT: RefCell<Vec<PathBuf>> = const { RefCell::new(vec![]) };
}

pub fn clear_cut() { CUT.with(|c| c.borrow_mut().clear()); }

pub fn is_cut(p: &Path) -> bool {
    CUT.with(|c| c.borrow().iter().any(|x| x == p))
}

fn uris_to_paths(text: &str) -> Vec<PathBuf> {
    text.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).filter_map(|u| {
        if u.starts_with('/') { Some(PathBuf::from(u)) } else { gio::File::for_uri(u).path() }
    }).collect()
}

/// Read the clipboard: (files, cut?). Understands GNOME, KDE and plain uri-list owners.
pub fn get_clipboard(cb: impl FnOnce(Vec<PathBuf>, bool) + 'static) {
    let clip = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    let cb = Rc::new(RefCell::new(Some(cb)));
    clip.request_contents(&gdk::Atom::intern("x-special/gnome-copied-files"), move |c, data| {
        let text = String::from_utf8_lossy(&data.data()).to_string();
        if !text.trim().is_empty() {
            let mut lines = text.lines();
            let cut = lines.next().map(|l| l.trim()) == Some("cut");
            let files = uris_to_paths(&lines.collect::<Vec<_>>().join("\n"));
            if let Some(f) = cb.borrow_mut().take() { f(files, cut); }
            return;
        }
        let cb2 = cb.clone();
        c.request_contents(&gdk::Atom::intern("text/uri-list"), move |c, data| {
            let files = uris_to_paths(&String::from_utf8_lossy(&data.data()));
            let cb3 = cb2.clone();
            if files.is_empty() { if let Some(f) = cb3.borrow_mut().take() { f(vec![], false); } return; }
            // Dolphin marks a cut with application/x-kde-cutselection = "1"
            c.request_contents(&gdk::Atom::intern("application/x-kde-cutselection"), move |_, d| {
                let cut = d.data().first() == Some(&b'1');
                if let Some(f) = cb3.borrow_mut().take() { f(files, cut); }
            });
        });
    });
}

thread_local! {
    /// None until the clipboard's targets were read once
    static HAS_FILES: Cell<Option<bool>> = const { Cell::new(None) };
}

fn refresh_has_files(clip: &gtk::Clipboard) {
    clip.request_contents(&gdk::Atom::intern("TARGETS"), |_, data| {
        let yes = data.targets().unwrap_or_default().iter().any(|a| { let n = a.name(); n == "x-special/gnome-copied-files" || n == "text/uri-list" });
        HAS_FILES.with(|h| h.set(Some(yes)));
    });
}

/// Whether the clipboard holds files (for greying out Paste). It does not wait for the clipboard
/// owner each time (a hung program would freeze the menu): the answer is kept up to date.
pub fn clipboard_has_files() -> bool {
    let clip = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    if let Some(v) = HAS_FILES.with(|h| h.get()) { return v; }
    HAS_FILES.with(|h| h.set(Some(false)));
    clip.connect_local("owner-change", false, |args| {
        if let Some(c) = args.first().and_then(|v| v.get::<gtk::Clipboard>().ok()) { refresh_has_files(&c); }
        None
    });
    refresh_has_files(&clip);
    // the very first time the answer is needed now
    clip.wait_is_target_available(&gdk::Atom::intern("x-special/gnome-copied-files")) || clip.wait_is_target_available(&gdk::Atom::intern("text/uri-list"))
}

// ------------------------------------------------------------------ names

/// `split_ext` on the raw bytes of a name (names that are not UTF-8 keep their bytes).
fn split_ext_bytes(n: &[u8]) -> (&[u8], &[u8]) {
    match n.iter().rposition(|&c| c == b'.') { Some(i) if i > 0 && i + 1 < n.len() => (&n[..i], &n[i..]), _ => (n, &[]) }
}

fn os_name(parts: &[&[u8]]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(parts.concat())
}

/// Win7 name for a copy pasted into the same folder: "file - Copy.txt", "file - Copy (2).txt".
pub fn copy_name(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let p = dir.join(name);
    if p.symlink_metadata().is_err() { return p; }
    let (stem, ext) = if p.is_dir() { (name.as_bytes(), &[][..]) } else { split_ext_bytes(name.as_bytes()) };
    for i in 1.. {
        let n = if i == 1 { b" - Copy".to_vec() } else { format!(" - Copy ({i})").into_bytes() };
        let p = dir.join(os_name(&[stem, &n, ext]));
        if p.symlink_metadata().is_err() { return p; }
    }
    unreachable!()
}

/// Win7 name for "keep both files" / renaming onto an existing name: "file (2).txt".
pub fn numbered_name(target: &Path) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    let dir = target.parent().unwrap_or(Path::new("/"));
    let name = target.file_name().map(|n| n.as_bytes()).unwrap_or_default();
    let (stem, ext) = if target.is_dir() { (name, &[][..]) } else { split_ext_bytes(name) };
    // "file (2).txt" becomes "file (3).txt", not "file (2) (2).txt"
    let base = match stem.windows(2).rposition(|w| w == b" (") {
        Some(i) if stem.ends_with(b")") && stem.len() > i + 3 && stem[i + 2..stem.len() - 1].iter().all(|c| c.is_ascii_digit()) => &stem[..i],
        _ => stem,
    };
    for i in 2.. {
        let p = dir.join(os_name(&[base, format!(" ({i})").as_bytes(), ext]));
        if p.symlink_metadata().is_err() { return p; }
    }
    unreachable!()
}

/// A free "New folder" / "New folder (2)" style name in `dir`.
pub fn new_name(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let p = dir.join(format!("{base}{ext}"));
    if p.symlink_metadata().is_err() { return p; }
    for i in 2.. {
        let p = dir.join(format!("{base} ({i}){ext}"));
        if p.symlink_metadata().is_err() { return p; }
    }
    unreachable!()
}

/// "name - Shortcut" for a symlink to `target` placed in `dir` (extension kept last so it opens).
pub fn shortcut_name(dir: &Path, target: &Path) -> PathBuf {
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Local Disk".into());
    let (stem, ext) = if target.is_dir() { (name.as_str(), "") } else { trash::split_ext(&name) };
    new_name(dir, &format!("{stem} - Shortcut"), ext)
}

pub fn display_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

// ------------------------------------------------------------------ undo

pub enum UndoOp {
    Rename { from: PathBuf, to: PathBuf },
    /// (original, now)
    Move(Vec<(PathBuf, PathBuf)>),
    /// copies that were created
    Copy(Vec<PathBuf>),
    Trash(Vec<trash::Trashed>),
    New(PathBuf),
}

impl UndoOp {
    fn label(&self) -> &'static str {
        match self { UndoOp::Rename { .. } => "Undo Rename", UndoOp::Move(_) => "Undo Move", UndoOp::Copy(_) => "Undo Copy", UndoOp::Trash(_) => "Undo Delete", UndoOp::New(_) => "Undo New" }
    }
}

thread_local! {
    static UNDO: RefCell<Vec<UndoOp>> = const { RefCell::new(vec![]) };
}

pub fn push_undo(op: UndoOp) {
    UNDO.with(|u| { let mut u = u.borrow_mut(); u.push(op); if u.len() > 50 { u.remove(0); } });
}

/// "Undo Rename" etc. for menus, None when there is nothing to undo.
pub fn undo_label() -> Option<&'static str> {
    UNDO.with(|u| u.borrow().last().map(|o| o.label()))
}

/// Ctrl+Z: revert the last rename/move/copy/delete/new.
pub fn undo(parent: Option<&gtk::Window>, on_done: impl Fn() + 'static) {
    let Some(op) = UNDO.with(|u| u.borrow_mut().pop()) else { return };
    let on_done = Rc::new(on_done);
    match op {
        UndoOp::Rename { from, to } => {
            if from.symlink_metadata().is_ok() { util::error("Can't undo the rename", &format!("There is already a file with the name \"{}\".", display_name(&from))); return; }
            if let Err(e) = std::fs::rename(&to, &from) { util::error("Can't undo the rename", &e.to_string()); }
            on_done();
        }
        UndoOp::Move(pairs) => {
            let back: Vec<(PathBuf, PathBuf)> = pairs.into_iter().filter(|(_, now)| now.symlink_metadata().is_ok()).map(|(o, n)| (n, o)).collect();
            let d = on_done.clone();
            run_transfer(back, Kind::Move, false, parent, false, move |_| d());
        }
        // undoing a copy or a new item deletes it: with the usual confirmation, to the Recycle Bin
        UndoOp::Copy(created) => {
            let d = on_done.clone();
            let created: Vec<PathBuf> = created.into_iter().filter(|p| p.symlink_metadata().is_ok()).collect();
            delete(created, false, parent, true, move || d());
        }
        UndoOp::New(p) => {
            let d = on_done.clone();
            delete(vec![p], false, parent, true, move || d());
        }
        UndoOp::Trash(items) => {
            let d = on_done.clone();
            restore(items, parent, move || d());
        }
    }
}

// ------------------------------------------------------------------ worker protocol

enum Question {
    File { src: PathBuf, dst: PathBuf, kind: Kind, more: usize },
    Folder { src: PathBuf, dst: PathBuf },
    Error { verb: &'static str, path: PathBuf, msg: String, retry: bool },
    NoTrash { path: PathBuf },
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Answer { Replace, Skip, KeepBoth, Cancel, Retry, Yes, No }

#[derive(Clone, Default)]
struct Progress {
    total_items: u64,
    total_bytes: u64,
    done_items: u64,
    done_bytes: u64,
    by_bytes: bool,
    calculating: bool,
    name: String,
    from: PathBuf,
    to: Option<PathBuf>,
}

enum ToGui {
    Progress(Progress),
    Ask(Question, std::sync::mpsc::Sender<(Answer, bool)>),
    Done(JobResult),
}

#[derive(Default)]
pub struct JobResult {
    /// top-level (source, final target) of a copy or move
    pub pairs: Vec<(PathBuf, PathBuf)>,
    pub trashed: Vec<trash::Trashed>,
    pub deleted: Vec<PathBuf>,
    pub cancelled: bool,
}

struct Abort;

struct Worker {
    tx: async_channel::Sender<ToGui>,
    cancel: Arc<AtomicBool>,
    kind: Kind,
    file_policy: Option<Answer>,
    folder_policy: Option<Answer>,
    error_policy: Option<Answer>,
    trash_policy: Option<Answer>,
    conflicts_left: usize,
    prog: Progress,
    last: Instant,
}

impl Worker {
    fn cancelled(&self) -> bool { self.cancel.load(Ordering::Relaxed) }

    fn ask(&self, q: Question) -> (Answer, bool) {
        let (rtx, rrx) = std::sync::mpsc::channel();
        if self.tx.send_blocking(ToGui::Ask(q, rtx)).is_err() { return (Answer::Cancel, true); }
        rrx.recv().unwrap_or((Answer::Cancel, true))
    }

    fn send(&mut self, force: bool) {
        if force || self.last.elapsed() >= Duration::from_millis(120) {
            self.last = Instant::now();
            let _ = self.tx.send_blocking(ToGui::Progress(self.prog.clone()));
        }
    }

    /// Ask what to do about an error: Ok(true) = try again, Ok(false) = skip.
    fn on_error(&mut self, verb: &'static str, path: &Path, e: &std::io::Error) -> Result<bool, Abort> {
        if self.cancelled() { return Err(Abort); }
        if let Some(a) = self.error_policy { return if a == Answer::Skip { Ok(false) } else { Err(Abort) }; }
        let (a, all) = self.ask(Question::Error { verb, path: path.to_path_buf(), msg: e.to_string(), retry: true });
        if all && a == Answer::Skip { self.error_policy = Some(a); }
        match a { Answer::Retry => Ok(true), Answer::Skip => Ok(false), _ => Err(Abort) }
    }

    fn add_done(&mut self, items: u64, bytes: u64) {
        self.prog.done_items += items;
        self.prog.done_bytes += bytes;
        self.send(false);
    }

    // ---------------------------------------------------------- copy / move

    fn discover(&mut self, src: &Path, mirror: Option<&Path>) {
        if self.cancelled() { return; }
        let Ok(m) = std::fs::symlink_metadata(src) else { return };
        self.prog.total_items += 1;
        if let Some(t) = mirror {
            if let Ok(tm) = std::fs::symlink_metadata(t) { if !(m.is_dir() && tm.is_dir()) { self.conflicts_left += 1; } }
        }
        if m.is_dir() {
            if let Ok(rd) = std::fs::read_dir(src) {
                for e in rd.flatten() {
                    let child_mirror = mirror.map(|t| t.join(e.file_name()));
                    self.discover(&e.path(), child_mirror.as_deref());
                }
            }
        } else {
            self.prog.total_bytes += m.len();
        }
        self.send(false);
    }

    fn transfer(&mut self, pairs: Vec<(PathBuf, PathBuf)>, rename_same_dir: bool) -> JobResult {
        let mut res = JobResult::default();
        // a copy, or a move to another drive, has to read everything: count it first ("Calculating...")
        let same_dev = |s: &Path, d: &Path| -> bool {
            let a = std::fs::symlink_metadata(s).map(|m| m.dev()).ok();
            let b = d.parent().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.dev());
            a.is_some() && a == b
        };
        let walk = self.kind == Kind::Copy || pairs.iter().any(|(s, d)| !same_dev(s, d));
        self.prog.calculating = true;
        self.prog.by_bytes = walk;
        if let Some((s, d)) = pairs.first() { self.prog.from = s.parent().map(|p| p.to_path_buf()).unwrap_or_default(); self.prog.to = d.parent().map(|p| p.to_path_buf()); }
        self.send(true);
        for (s, d) in &pairs {
            if walk {
                let mirror = if rename_same_dir && s.parent() == d.parent() { None } else { Some(d.as_path()) };
                self.discover(s, mirror);
            } else {
                self.prog.total_items += 1;
                if let (Ok(sm), Ok(tm)) = (std::fs::symlink_metadata(s), std::fs::symlink_metadata(d)) { if !(sm.is_dir() && tm.is_dir()) && s != d { self.conflicts_left += 1; } }
            }
        }
        self.prog.calculating = false;
        self.send(true);
        for (src, target) in pairs {
            if self.cancelled() { res.cancelled = true; break; }
            let target = if rename_same_dir && src.parent() == target.parent() {
                if self.kind == Kind::Move { continue; } // moving into the same folder does nothing
                copy_name(target.parent().unwrap_or(Path::new("/")), src.file_name().unwrap_or_default())
            } else { target };
            match self.item(&src, target) {
                Ok(Some(t)) => res.pairs.push((src, t)),
                Ok(None) => {}
                Err(Abort) => { res.cancelled = true; break; }
            }
        }
        self.send(true);
        res
    }

    fn subtree(&self, p: &Path) -> (u64, u64) {
        let mut acc = (0, 0);
        fn walk(p: &Path, acc: &mut (u64, u64)) {
            let Ok(m) = std::fs::symlink_metadata(p) else { return };
            acc.0 += 1;
            if m.is_dir() { if let Ok(rd) = std::fs::read_dir(p) { for e in rd.flatten() { walk(&e.path(), acc); } } } else { acc.1 += m.len(); }
        }
        if self.prog.by_bytes { walk(p, &mut acc); } else { acc = (1, 0); }
        acc
    }

    /// Copy or move one item to `target` (its final full path). Ok(Some(final path)) when done.
    fn item(&mut self, src: &Path, target: PathBuf) -> Result<Option<PathBuf>, Abort> {
        if self.cancelled() { return Err(Abort); }
        let mut target = target;
        let meta = loop {
            match std::fs::symlink_metadata(src) {
                Ok(m) => break m,
                Err(e) => { if !self.on_error(if self.kind == Kind::Copy { "copy" } else { "move" }, src, &e)? { return Ok(None); } }
            }
        };
        self.prog.name = display_name(src);
        self.prog.from = src.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.prog.to = target.parent().map(|p| p.to_path_buf());
        if meta.is_dir() && is_inside(&target, src) {
            // "The destination folder is a subfolder of the source folder."
            let (a, _) = self.ask(Question::Error { verb: "subfolder", path: src.to_path_buf(), msg: String::new(), retry: false });
            if a == Answer::Skip { let (i, b) = self.subtree(src); self.add_done(i, b); return Ok(None); }
            return Err(Abort);
        }
        if src == target { let (i, b) = self.subtree(src); self.add_done(i, b); return Ok(None); }
        let mut merge = false;
        // Replace: the old file goes only once the new one is complete (rename over it)
        let mut replace = false;
        if let Ok(tm) = std::fs::symlink_metadata(&target) {
            if meta.is_dir() && tm.is_dir() {
                let (a, all) = match self.folder_policy { Some(a) => (a, true), None => self.ask(Question::Folder { src: src.to_path_buf(), dst: target.clone() }) };
                if all && a != Answer::Cancel { self.folder_policy = Some(a); }
                match a {
                    Answer::Yes => merge = true,
                    Answer::No => { let (i, b) = self.subtree(src); self.add_done(i, b); return Ok(None); }
                    _ => return Err(Abort),
                }
            } else {
                let more = self.conflicts_left.saturating_sub(1);
                let (a, all) = match self.file_policy { Some(a) => (a, true), None => self.ask(Question::File { src: src.to_path_buf(), dst: target.clone(), kind: self.kind, more }) };
                self.conflicts_left = self.conflicts_left.saturating_sub(1);
                if all && a != Answer::Cancel { self.file_policy = Some(a); }
                match a {
                    Answer::Replace if !tm.is_dir() && !meta.is_dir() => replace = true,
                    Answer::Replace | Answer::KeepBoth => target = numbered_name(&target),
                    Answer::Skip => { let (i, b) = self.subtree(src); self.add_done(i, b); return Ok(None); }
                    _ => return Err(Abort),
                }
            }
        }
        // a move within one drive is a rename
        if self.kind == Kind::Move && !merge {
            loop {
                match std::fs::rename(src, &target) {
                    Ok(()) => {
                        let (i, b) = if self.prog.by_bytes { self.subtree(&target) } else { (1, 0) };
                        self.add_done(i, b);
                        return Ok(Some(target));
                    }
                    Err(e) if e.raw_os_error() == Some(libc::EXDEV) => break,
                    Err(e) => { if !self.on_error("move", src, &e)? { return Ok(None); } }
                }
            }
        }
        if meta.is_dir() {
            if !merge {
                loop {
                    match std::fs::create_dir(&target) {
                        Ok(()) => break,
                        Err(e) => { if !self.on_error("copy", &target, &e)? { return Ok(None); } }
                    }
                }
            }
            let kids: Vec<std::ffi::OsString> = std::fs::read_dir(src).map(|rd| rd.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
            for k in kids { self.item(&src.join(&k), target.join(&k))?; }
            if !merge { let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(meta.mode() & 0o7777)); set_times(&target, &meta); }
            if self.kind == Kind::Move { let _ = std::fs::remove_dir(src); }
            self.add_done(1, 0);
        } else if meta.file_type().is_symlink() {
            loop {
                let tmp = if replace { temp_name(&target) } else { target.clone() };
                let r = std::fs::read_link(src).and_then(|l| std::os::unix::fs::symlink(l, &tmp)).and_then(|_| if replace { std::fs::rename(&tmp, &target) } else { Ok(()) });
                match r { Ok(()) => break, Err(e) => { if replace { let _ = std::fs::remove_file(&tmp); } if !self.on_error("copy", src, &e)? { return Ok(None); } } }
            }
            if self.kind == Kind::Move { let _ = std::fs::remove_file(src); }
            self.add_done(1, meta.len());
        } else if meta.is_file() {
            let base = self.prog.done_bytes;
            loop {
                // a replacement is written next to the old file, then renamed over it
                let out = if replace { temp_name(&target) } else { target.clone() };
                match self.copy_file(src, &out, &meta).and_then(|_| if replace { std::fs::rename(&out, &target) } else { Ok(()) }) {
                    Ok(()) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted && self.cancelled() => { let _ = std::fs::remove_file(&out); return Err(Abort); }
                    Err(e) => {
                        // only what this copy created is removed, never a file that was there
                        if e.kind() != std::io::ErrorKind::AlreadyExists { let _ = std::fs::remove_file(&out); }
                        self.prog.done_bytes = base;
                        if !self.on_error("copy", src, &e)? { self.add_done(1, meta.len()); return Ok(None); }
                    }
                }
            }
            if self.kind == Kind::Move {
                if let Err(e) = std::fs::remove_file(src) { let _ = self.on_error("move", src, &e)?; }
            }
            self.prog.done_bytes = base + meta.len();
            self.add_done(1, 0);
        } else {
            let e = std::io::Error::new(std::io::ErrorKind::Unsupported, "This kind of file (device, pipe or socket) can't be copied.");
            let _ = self.on_error("copy", src, &e)?;
            return Ok(None);
        }
        Ok(Some(target))
    }

    fn copy_file(&mut self, src: &Path, dst: &Path, meta: &std::fs::Metadata) -> std::io::Result<()> {
        use std::io::{Read, Write};
        let mut i = std::fs::File::open(src)?;
        let mut o = std::fs::OpenOptions::new().write(true).create_new(true).mode(meta.mode() & 0o7777 | 0o200).open(dst)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            if self.cancelled() { return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")); }
            let n = i.read(&mut buf)?;
            if n == 0 { break; }
            o.write_all(&buf[..n])?;
            self.prog.done_bytes += n as u64;
            self.send(false);
        }
        o.flush()?;
        drop(o);
        let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(meta.mode() & 0o7777));
        set_times(dst, meta);
        Ok(())
    }

    // ---------------------------------------------------------- delete

    fn remove_tree(&mut self, p: &Path) -> Result<bool, Abort> {
        if self.cancelled() { return Err(Abort); }
        let Ok(m) = std::fs::symlink_metadata(p) else { return Ok(true) };
        self.prog.name = display_name(p);
        if m.is_dir() {
            let kids: Vec<PathBuf> = std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            let mut ok = true;
            for k in kids { ok &= self.remove_tree(&k)?; }
            if !ok { return Ok(false); }
            loop {
                match std::fs::remove_dir(p) { Ok(()) => break, Err(e) => { if !self.on_error("delete", p, &e)? { return Ok(false); } } }
            }
        } else {
            loop {
                match std::fs::remove_file(p) { Ok(()) => break, Err(e) => { if !self.on_error("delete", p, &e)? { return Ok(false); } } }
            }
        }
        self.add_done(1, 0);
        Ok(true)
    }

    fn delete(&mut self, files: Vec<PathBuf>, permanent: bool) -> JobResult {
        let mut res = JobResult::default();
        self.prog.by_bytes = false;
        if let Some(f) = files.first() { self.prog.from = f.parent().map(|p| p.to_path_buf()).unwrap_or_default(); }
        if permanent {
            self.prog.calculating = true;
            self.send(true);
            for f in &files { let (i, _) = { let mut acc = (0, 0); count(f, &mut acc, &self.cancel); acc }; self.prog.total_items += i; }
            self.prog.calculating = false;
        } else {
            self.prog.total_items = files.len() as u64;
        }
        self.send(true);
        for f in files {
            if self.cancelled() { res.cancelled = true; break; }
            // an item of the Recycle Bin itself: delete it and its .trashinfo
            let info = if trash::in_trash(&f) { trash::lookup(&f).map(|t| t.info) } else { None };
            if permanent || info.is_some() {
                match self.remove_tree(&f) {
                    Ok(true) => { if let Some(i) = info { let _ = std::fs::remove_file(i); } res.deleted.push(f); }
                    Ok(false) => {}
                    Err(Abort) => { res.cancelled = true; break; }
                }
                continue;
            }
            self.prog.name = display_name(&f);
            loop {
                match trash::trash(&f) {
                    Ok(t) => { res.trashed.push(t); self.add_done(1, 0); break; }
                    Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                        let (a, all) = match self.trash_policy { Some(a) => (a, true), None => self.ask(Question::NoTrash { path: f.clone() }) };
                        if all && a != Answer::Cancel { self.trash_policy = Some(a); }
                        match a {
                            Answer::Yes => { match self.remove_tree(&f) { Ok(true) => res.deleted.push(f.clone()), Ok(false) => {}, Err(Abort) => { res.cancelled = true; return res; } } }
                            Answer::No => self.add_done(1, 0),
                            _ => { res.cancelled = true; return res; }
                        }
                        break;
                    }
                    Err(e) => match self.on_error("delete", &f, &e) { Ok(true) => continue, Ok(false) => { self.add_done(1, 0); break; } Err(Abort) => { res.cancelled = true; return res; } },
                }
            }
        }
        res
    }
}

/// Is `target` `dir` itself or inside it (also through symbolic links)?
fn is_inside(target: &Path, dir: &Path) -> bool {
    if target.starts_with(dir) { return true; }
    let Ok(d) = std::fs::canonicalize(dir) else { return false };
    // the target does not exist yet: resolve the deepest folder of it that does
    let mut t = target.to_path_buf();
    let mut rest = vec![];
    while std::fs::symlink_metadata(&t).is_err() {
        match (t.file_name().map(|n| n.to_os_string()), t.parent()) { (Some(n), Some(p)) => { rest.push(n); t = p.to_path_buf(); } _ => return false }
    }
    let Ok(mut t) = std::fs::canonicalize(&t) else { return false };
    for n in rest.into_iter().rev() { t.push(n); }
    t.starts_with(&d)
}

/// A hidden temporary name next to `p` for writing a replacement.
fn temp_name(p: &Path) -> PathBuf {
    let mut n = std::ffi::OsString::from(".");
    n.push(p.file_name().unwrap_or_default());
    n.push(format!(".aerode-{}", std::process::id()));
    let t = p.with_file_name(&n);
    if t.symlink_metadata().is_err() { return t; }
    for i in 2.. { let mut m = n.clone(); m.push(format!("-{i}")); let t = p.with_file_name(m); if t.symlink_metadata().is_err() { return t; } }
    unreachable!()
}

fn count(p: &Path, acc: &mut (u64, u64), cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) { return; }
    let Ok(m) = std::fs::symlink_metadata(p) else { return };
    acc.0 += 1;
    if m.is_dir() { if let Ok(rd) = std::fs::read_dir(p) { for e in rd.flatten() { count(&e.path(), acc, cancel); } } } else { acc.1 += m.len(); }
}

fn set_times(p: &Path, meta: &std::fs::Metadata) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return };
    let ts = [
        libc::timespec { tv_sec: meta.atime() as libc::time_t, tv_nsec: meta.atime_nsec() as _ },
        libc::timespec { tv_sec: meta.mtime() as libc::time_t, tv_nsec: meta.mtime_nsec() as _ },
    ];
    unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), ts.as_ptr(), libc::AT_SYMLINK_NOFOLLOW); }
}

// ------------------------------------------------------------------ GUI side

#[derive(Clone, Copy, PartialEq)]
enum JobKind { Copy, Move, Delete, Recycle }

fn run_job(kind: JobKind, parent: Option<&gtk::Window>, work: impl FnOnce(&mut Worker) -> JobResult + Send + 'static, on_done: impl FnOnce(JobResult) + 'static) {
    let (tx, rx) = async_channel::unbounded::<ToGui>();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let (tx, cancel) = (tx.clone(), cancel.clone());
        let wk = match kind { JobKind::Move => Kind::Move, _ => Kind::Copy };
        std::thread::spawn(move || {
            let mut w = Worker { tx: tx.clone(), cancel, kind: wk, file_policy: None, folder_policy: None, error_policy: None, trash_policy: None,
                conflicts_left: 0, prog: Progress::default(), last: Instant::now() };
            let r = work(&mut w);
            let _ = tx.send_blocking(ToGui::Done(r));
        });
    }
    drop(tx);
    let dlg = ProgressDialog::new(kind, parent, cancel.clone());
    let parent = parent.cloned();
    let started = Instant::now();
    let mut on_done = Some(on_done);
    glib::MainContext::default().spawn_local(async move {
        // Explorer shows the progress window only for operations that take a moment
        let (d2, fired) = (dlg.clone(), Rc::new(Cell::new(false)));
        let f2 = fired.clone();
        let mut show_timer = Some(glib::timeout_add_local_once(Duration::from_millis(700), move || { f2.set(true); d2.show(); }));
        let mut stop_timer = move || if let Some(t) = show_timer.take() { if !fired.get() { t.remove(); } };
        while let Ok(m) = rx.recv().await {
            match m {
                ToGui::Progress(p) => dlg.update(&p, started),
                ToGui::Ask(q, reply) => {
                    stop_timer();
                    if started.elapsed() > Duration::from_millis(300) { dlg.show(); }
                    let win = if dlg.shown.get() { Some(dlg.win.clone().upcast::<gtk::Window>()) } else { parent.clone() };
                    ask_dialog(q, win.as_ref(), move |a| { let _ = reply.send(a); });
                }
                ToGui::Done(r) => {
                    stop_timer();
                    dlg.win.hide();
                    unsafe { dlg.win.destroy(); }
                    if let Some(f) = on_done.take() { f(r); }
                    break;
                }
            }
        }
    });
}

/// The Win7 copy/move/delete progress window.
struct ProgressDialog {
    win: gtk::Window,
    kind: JobKind,
    shown: Cell<bool>,
    head: gtk::Label,
    fromto: gtk::Label,
    remaining: gtk::Label,
    bar: gtk::ProgressBar,
    d_name: gtk::Label,
    d_from: gtk::Label,
    d_to: gtk::Label,
    d_time: gtk::Label,
    d_items: gtk::Label,
    d_speed: gtk::Label,
    cancel: Arc<AtomicBool>,
    speed: Cell<f64>,
    last_sample: Cell<(f64, u64)>,
}

fn fmt_remaining(secs: f64) -> String {
    let s = secs.max(1.0).round() as u64;
    if s < 60 { return format!("About {s} second{}", if s == 1 { "" } else { "s" }); }
    let m = (s + 30) / 60;
    if m < 60 { return format!("About {m} minute{}", if m == 1 { "" } else { "s" }); }
    let (h, mm) = (m / 60, m % 60);
    if mm == 0 { format!("About {h} hour{}", if h == 1 { "" } else { "s" }) } else { format!("About {h} hour{} and {mm} minute{}", if h == 1 { "" } else { "s" }, if mm == 1 { "" } else { "s" }) }
}

fn folder_link(p: &Path) -> String {
    let name = display_name(p);
    format!("<span foreground='#0066cc'>{}</span> ({})", glib::markup_escape_text(&name), glib::markup_escape_text(&p.display().to_string()))
}

fn items_text(n: u64) -> String { format!("{n} item{}", if n == 1 { "" } else { "s" }) }

impl ProgressDialog {
    fn new(kind: JobKind, parent: Option<&gtk::Window>, cancel: Arc<AtomicBool>) -> Rc<ProgressDialog> {
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.set_title("Calculating...");
        win.set_default_size(460, -1);
        win.set_resizable(false);
        win.set_icon_name(Some(match kind { JobKind::Delete | JobKind::Recycle => "user-trash-full", _ => "edit-copy" }));
        if let Some(p) = parent { win.set_transient_for(Some(p)); }
        win.set_type_hint(gdk::WindowTypeHint::Dialog);
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.set_border_width(12);
        let lab = |markup: bool| { let l = gtk::Label::new(None); l.set_xalign(0.0); l.set_line_wrap(true); l.set_max_width_chars(60); if markup { l.set_use_markup(true); } l };
        let head = lab(false);
        let fromto = lab(true);
        let remaining = lab(true);
        remaining.set_markup("<span size='large' foreground='#003399'>Calculating...</span>");
        let bar = gtk::ProgressBar::new();
        bar.set_size_request(-1, 16);
        bar.style_context().add_class("win7-copy-progress");
        v.pack_start(&head, false, false, 0);
        v.pack_start(&fromto, false, false, 0);
        v.pack_start(&remaining, false, false, 4);
        v.pack_start(&bar, false, false, 2);
        // More details
        let grid = gtk::Grid::new();
        grid.set_row_spacing(3);
        grid.set_column_spacing(10);
        grid.set_margin_top(6);
        let mut vals = vec![];
        for (i, k) in ["Name:", "From:", "To:", "Time remaining:", "Items remaining:", "Speed:"].iter().enumerate() {
            let kl = gtk::Label::new(Some(k));
            kl.set_xalign(1.0);
            kl.style_context().add_class("dim-label");
            let vl = gtk::Label::new(None);
            vl.set_xalign(0.0);
            vl.set_use_markup(true);
            vl.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            vl.set_max_width_chars(48);
            grid.attach(&kl, 0, i as i32, 1, 1);
            grid.attach(&vl, 1, i as i32, 1, 1);
            vals.push(vl);
        }
        let revealer = gtk::Revealer::new();
        revealer.add(&grid);
        v.pack_start(&revealer, false, false, 0);
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bottom.set_margin_top(8);
        let more = gtk::Button::new();
        more.set_relief(gtk::ReliefStyle::None);
        let mb = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let arrow = gtk::Image::from_icon_name(Some("pan-down-symbolic"), gtk::IconSize::Menu);
        let ml = gtk::Label::new(Some("More details"));
        mb.pack_start(&arrow, false, false, 0);
        mb.pack_start(&ml, false, false, 0);
        more.add(&mb);
        {
            let (r, a, l) = (revealer.clone(), arrow.clone(), ml.clone());
            more.connect_clicked(move |_| {
                let open = !r.reveals_child();
                r.set_reveal_child(open);
                a.set_from_icon_name(Some(if open { "pan-up-symbolic" } else { "pan-down-symbolic" }), gtk::IconSize::Menu);
                l.set_text(if open { "Fewer details" } else { "More details" });
            });
        }
        let cancel_b = gtk::Button::with_label("Cancel");
        cancel_b.set_size_request(80, -1);
        bottom.pack_start(&more, false, false, 0);
        bottom.pack_end(&cancel_b, false, false, 0);
        v.pack_start(&bottom, false, false, 0);
        win.add(&v);
        let mut vals = vals.into_iter();
        let d = Rc::new(ProgressDialog {
            win: win.clone(), kind, shown: Cell::new(false), head, fromto, remaining, bar,
            d_name: vals.next().unwrap(), d_from: vals.next().unwrap(), d_to: vals.next().unwrap(), d_time: vals.next().unwrap(),
            d_items: vals.next().unwrap(), d_speed: vals.next().unwrap(),
            cancel, speed: Cell::new(0.0), last_sample: Cell::new((0.0, 0)),
        });
        {
            let d2 = Rc::downgrade(&d);
            let cancel_now = move || if let Some(d) = d2.upgrade() {
                d.cancel.store(true, Ordering::Relaxed);
                d.win.set_title("Cancelling...");
                d.remaining.set_markup("<span size='large' foreground='#003399'>Cancelling...</span>");
            };
            let c2 = cancel_now.clone();
            cancel_b.connect_clicked(move |_| c2());
            win.connect_delete_event(move |_, _| { cancel_now(); glib::Propagation::Stop });
        }
        d
    }

    fn show(&self) {
        if self.shown.replace(true) { return; }
        self.win.show_all();
        self.win.present();
    }

    fn update(&self, p: &Progress, started: Instant) {
        if self.cancel.load(Ordering::Relaxed) { return; }
        let verb = match self.kind { JobKind::Copy => "Copying", JobKind::Move => "Moving", JobKind::Delete => "Deleting", JobKind::Recycle => "Moving" };
        let size = if p.by_bytes && p.total_bytes > 0 { format!(" ({})", util::human_size(p.total_bytes)) } else { String::new() };
        if p.calculating {
            self.win.set_title("Calculating...");
            self.head.set_text(&format!("Discovered {}{}", items_text(p.total_items), if p.by_bytes { format!(" ({})", util::human_size(p.total_bytes)) } else { String::new() }));
            return;
        }
        let tail = if self.kind == JobKind::Recycle { " to the Recycle Bin" } else { "" };
        self.head.set_text(&format!("{verb} {}{size}{tail}", items_text(p.total_items)));
        let ft = match (&p.to, self.kind) {
            (Some(t), JobKind::Copy | JobKind::Move) => format!("from {} to {}", folder_link(&p.from), folder_link(t)),
            _ => format!("from {}", folder_link(&p.from)),
        };
        self.fromto.set_markup(&ft);
        let frac = if p.by_bytes && p.total_bytes > 0 { p.done_bytes as f64 / p.total_bytes as f64 } else if p.total_items > 0 { p.done_items as f64 / p.total_items as f64 } else { 0.0 };
        let frac = frac.clamp(0.0, 1.0);
        self.bar.set_fraction(frac);
        self.win.set_title(&format!("{}% complete", (frac * 100.0).floor() as u32));
        // speed: smoothed over the last samples
        let el = started.elapsed().as_secs_f64();
        let (t0, b0) = self.last_sample.get();
        if el - t0 >= 0.5 {
            let inst = (p.done_bytes.saturating_sub(b0)) as f64 / (el - t0);
            let s = if self.speed.get() == 0.0 { inst } else { self.speed.get() * 0.7 + inst * 0.3 };
            self.speed.set(s);
            self.last_sample.set((el, p.done_bytes));
        }
        let left = if el > 1.0 && frac > 0.01 { Some(el / frac - el) } else { None };
        match left {
            Some(l) => self.remaining.set_markup(&format!("<span size='large' foreground='#003399'>{} remaining</span>", fmt_remaining(l))),
            None => self.remaining.set_markup("<span size='large' foreground='#003399'>Calculating...</span>"),
        }
        self.d_name.set_text(&p.name);
        self.d_from.set_markup(&folder_link(&p.from));
        self.d_to.set_markup(&p.to.as_ref().map(|t| folder_link(t)).unwrap_or_default());
        self.d_time.set_text(&left.map(fmt_remaining).unwrap_or_else(|| "Calculating...".into()));
        let items_left = p.total_items.saturating_sub(p.done_items);
        self.d_items.set_text(&if p.by_bytes { format!("{items_left} ({})", util::human_size(p.total_bytes.saturating_sub(p.done_bytes))) } else { items_left.to_string() });
        let sp = self.speed.get();
        self.d_speed.set_text(&if !p.by_bytes { String::new() } else if sp >= 1024.0 * 1024.0 { format!("{:.1} MB/second", sp / 1048576.0) } else { format!("{:.0} KB/second", sp / 1024.0) });
    }
}

// ------------------------------------------------------------------ Win7 dialogs

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(&format!("<span size='large' foreground='#003399'>{}</span>", glib::markup_escape_text(text)));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(56);
    l
}

fn fmt_mtime(m: &std::fs::Metadata) -> String {
    m.modified().ok().map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%-m/%-d/%Y %-I:%M %p").to_string()).unwrap_or_default()
}

pub fn file_icon(p: &Path, size: i32) -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let info = gio::File::for_path(p).query_info("standard::icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
    util::gicon_pixbuf(info.and_then(|i| i.icon()).as_ref(), size)
}

/// Icon + name + folder + size + date, as in the conflict and delete dialogs.
fn file_card(p: &Path, show_folder: bool) -> gtk::Box {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let img = gtk::Image::from_pixbuf(file_icon(p, 48).as_ref());
    img.set_valign(gtk::Align::Start);
    h.pack_start(&img, false, false, 0);
    let v = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let m = std::fs::symlink_metadata(p).ok();
    let mut lines = vec![display_name(p)];
    if show_folder { if let Some(par) = p.parent() { lines.push(format!("{} ({})", display_name(par), par.display())); } }
    if let Some(m) = &m {
        if !m.is_dir() { lines.push(format!("Size: {}", util::human_size(m.len()))); }
        lines.push(format!("Date modified: {}", fmt_mtime(m)));
    }
    for (i, t) in lines.iter().enumerate() {
        let l = gtk::Label::new(Some(t));
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        l.set_max_width_chars(50);
        if i > 0 { l.style_context().add_class("dim-label"); }
        v.pack_start(&l, false, false, 0);
    }
    h.pack_start(&v, true, true, 0);
    h
}

fn dialog(title: &str, parent: Option<&gtk::Window>) -> (gtk::Dialog, gtk::Box) {
    let d = gtk::Dialog::new();
    d.set_title(title);
    d.set_modal(true);
    d.set_resizable(false);
    if let Some(p) = parent { d.set_transient_for(Some(p)); }
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(8);
    (d, c)
}

/// A Win7 "command link": green arrow, title, explanation and optional file card.
fn command_link(title: &str, body: &str, card: Option<gtk::Box>) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_relief(gtk::ReliefStyle::None);
    b.style_context().add_class("command-link");
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let arrow = gtk::Label::new(None);
    arrow.set_markup("<span size='x-large' foreground='#2f9e2f'>➜</span>");
    arrow.set_valign(gtk::Align::Start);
    h.pack_start(&arrow, false, false, 0);
    let v = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let t = gtk::Label::new(None);
    t.set_markup(&format!("<span size='large' foreground='#151c55'>{}</span>", glib::markup_escape_text(title)));
    t.set_xalign(0.0);
    v.pack_start(&t, false, false, 0);
    if !body.is_empty() {
        let s = gtk::Label::new(Some(body));
        s.set_xalign(0.0);
        s.set_line_wrap(true);
        s.set_max_width_chars(56);
        v.pack_start(&s, false, false, 0);
    }
    if let Some(c) = card { c.set_margin_top(4); v.pack_start(&c, false, false, 0); }
    h.pack_start(&v, true, true, 0);
    b.add(&h);
    b
}

const R_REPLACE: u16 = 1;
const R_SKIP: u16 = 2;
const R_BOTH: u16 = 3;

fn ask_dialog(q: Question, parent: Option<&gtk::Window>, reply: impl FnOnce((Answer, bool)) + 'static) {
    let reply = Rc::new(RefCell::new(Some(reply)));
    let finish = { let r = reply.clone(); move |a: (Answer, bool)| { if let Some(f) = r.borrow_mut().take() { f(a); } } };
    sounds::play(sounds::Event::Exclamation);
    match q {
        Question::File { src, dst, kind, more } => {
            let copy = kind == Kind::Copy;
            let (d, c) = dialog(if copy { "Copy File" } else { "Move File" }, parent);
            c.pack_start(&heading("There is already a file with the same name in this location."), false, false, 0);
            let sub = gtk::Label::new(Some("Click the file you want to keep"));
            sub.set_xalign(0.0);
            c.pack_start(&sub, false, false, 0);
            let renamed = numbered_name(&dst);
            let opts = [
                (R_REPLACE, if copy { "Copy and Replace" } else { "Move and Replace" }, format!("Replace the file in the destination folder with the file you are {}:", if copy { "copying" } else { "moving" }), Some(file_card(&src, true))),
                (R_SKIP, if copy { "Don't copy" } else { "Don't move" }, "No files will be changed. Leave this file in the destination folder:".to_string(), Some(file_card(&dst, true))),
                (R_BOTH, if copy { "Copy, but keep both files" } else { "Move, but keep both files" },
                    format!("The file you are {} will be renamed \"{}\"", if copy { "copying" } else { "moving" }, display_name(&renamed)), None),
            ];
            let all = gtk::CheckButton::with_label(&format!("Do this for the next {more} conflict{}", if more == 1 { "" } else { "s" }));
            for (code, title, body, card) in opts {
                let b = command_link(title, &body, card);
                let d2 = d.clone();
                b.connect_clicked(move |_| d2.response(gtk::ResponseType::Other(code)));
                c.pack_start(&b, false, false, 0);
            }
            d.add_button("Cancel", gtk::ResponseType::Cancel);
            if more > 0 { c.pack_start(&all, false, false, 4); }
            d.connect_response(move |d, r| {
                let a = match r { gtk::ResponseType::Other(R_REPLACE) => Answer::Replace, gtk::ResponseType::Other(R_SKIP) => Answer::Skip, gtk::ResponseType::Other(R_BOTH) => Answer::KeepBoth, _ => Answer::Cancel };
                finish((a, all.is_active()));
                d.close();
            });
            d.show_all();
        }
        Question::Folder { src, dst } => {
            let (d, c) = dialog("Confirm Folder Replace", parent);
            c.pack_start(&heading(&format!("This destination already contains a folder named '{}'.", display_name(&dst))), false, false, 0);
            let t = gtk::Label::new(Some("If any files have the same names, you will be asked if you want to replace those files.\n\nDo you still want to merge this folder"));
            t.set_xalign(0.0);
            t.set_line_wrap(true);
            c.pack_start(&t, false, false, 0);
            c.pack_start(&file_card(&src, true), false, false, 0);
            let w = gtk::Label::new(Some("with this one?"));
            w.set_xalign(0.0);
            c.pack_start(&w, false, false, 0);
            c.pack_start(&file_card(&dst, true), false, false, 0);
            let all = gtk::CheckButton::with_label("Do this for all current items");
            c.pack_start(&all, false, false, 6);
            d.add_button("Yes", gtk::ResponseType::Yes);
            d.add_button("No", gtk::ResponseType::No);
            d.add_button("Cancel", gtk::ResponseType::Cancel);
            d.set_default_response(gtk::ResponseType::Yes);
            d.connect_response(move |d, r| {
                let a = match r { gtk::ResponseType::Yes => Answer::Yes, gtk::ResponseType::No => Answer::No, _ => Answer::Cancel };
                finish((a, all.is_active()));
                d.close();
            });
            d.show_all();
        }
        Question::Error { verb, path, msg, retry } => {
            let (d, c) = dialog("Interrupted Action", parent);
            let denied = msg.contains("Permission denied") || msg.contains("os error 13") || msg.contains("os error 1)");
            let head = match verb {
                "subfolder" => "The destination folder is a subfolder of the source folder.".to_string(),
                _ if denied => "You need permission to perform this action".to_string(),
                v => format!("An unexpected error is keeping you from {} the {}. If you continue to receive this error, you can use the error code to search for help with this problem.",
                    match v { "copy" => "copying", "move" => "moving", "delete" => "deleting", "replace" => "replacing", _ => "using" }, if path.is_dir() { "folder" } else { "file" }),
            };
            if denied { d.set_title(if verb == "delete" { "File Access Denied" } else { "Destination Folder Access Denied" }); }
            c.pack_start(&heading(&head), false, false, 0);
            if !msg.is_empty() && !denied {
                let e = gtk::Label::new(Some(&format!("Error: {msg}")));
                e.set_xalign(0.0);
                e.set_line_wrap(true);
                c.pack_start(&e, false, false, 0);
            }
            c.pack_start(&file_card(&path, true), false, false, 4);
            let all = gtk::CheckButton::with_label("Do this for all current items");
            if retry { c.pack_start(&all, false, false, 4); }
            if retry { d.add_button("Try Again", gtk::ResponseType::Other(9)); }
            d.add_button("Skip", gtk::ResponseType::Other(R_SKIP));
            d.add_button("Cancel", gtk::ResponseType::Cancel);
            d.connect_response(move |d, r| {
                let a = match r { gtk::ResponseType::Other(9) => Answer::Retry, gtk::ResponseType::Other(R_SKIP) => Answer::Skip, _ => Answer::Cancel };
                finish((a, all.is_active()));
                d.close();
            });
            d.show_all();
        }
        Question::NoTrash { path } => {
            let (d, c) = dialog(if path.is_dir() { "Delete Folder" } else { "Delete File" }, parent);
            c.pack_start(&heading(&format!("This {} can't be moved to the Recycle Bin.", if path.is_dir() { "folder" } else { "file" })), false, false, 0);
            let t = gtk::Label::new(Some(&format!("Do you want to permanently delete it?")));
            t.set_xalign(0.0);
            c.pack_start(&t, false, false, 0);
            c.pack_start(&file_card(&path, false), false, false, 4);
            let all = gtk::CheckButton::with_label("Do this for all current items");
            c.pack_start(&all, false, false, 4);
            d.add_button("Yes", gtk::ResponseType::Yes);
            d.add_button("No", gtk::ResponseType::No);
            d.add_button("Cancel", gtk::ResponseType::Cancel);
            d.connect_response(move |d, r| {
                let a = match r { gtk::ResponseType::Yes => Answer::Yes, gtk::ResponseType::No => Answer::No, _ => Answer::Cancel };
                finish((a, all.is_active()));
                d.close();
            });
            d.show_all();
        }
    }
}

/// Can `p` go to the Recycle Bin (same drive as the home trash, or a drive we can make a trash on)?
fn can_trash(p: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(p) else { return false };
    if std::fs::metadata(gtk::glib::user_data_dir()).map(|d| d.dev() == m.dev()).unwrap_or(false) { return true; }
    // other drives: writable top directory or an existing trash
    let mut top = p.parent().map(|x| x.to_path_buf());
    while let Some(t) = top.clone() {
        match t.parent() { Some(par) if std::fs::metadata(par).map(|pm| pm.dev() == m.dev()).unwrap_or(false) => top = Some(par.to_path_buf()), _ => break }
    }
    let Some(top) = top else { return false };
    let uid = unsafe { libc::getuid() };
    top.join(format!(".Trash-{uid}")).is_dir() || top.join(".Trash").join(uid.to_string()).is_dir() || writable(&top)
}

fn writable(p: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(p.as_os_str())) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

/// The Win7 delete confirmation. Returns true when the user said Yes.
pub fn confirm_delete(files: &[PathBuf], permanent: bool, parent: Option<&gtk::Window>) -> bool {
    let one = files.len() == 1;
    let is_dir = one && files[0].is_dir() && !files[0].is_symlink();
    let title = if !one { "Delete Multiple Items" } else if is_dir { "Delete Folder" } else { "Delete File" };
    let what = if !one { format!("these {} items", files.len()) } else if is_dir { "this folder".into() } else { "this file".into() };
    let text = if permanent { format!("Are you sure you want to permanently delete {what}?") } else { format!("Are you sure you want to move {what} to the Recycle Bin?") };
    let (d, c) = dialog(title, parent);
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let icon = if permanent { "dialog-warning" } else { "user-trash-full" };
    let img = gtk::Image::from_pixbuf(util::pixbuf_or(icon, 32, "dialog-warning").as_ref());
    img.set_valign(gtk::Align::Start);
    h.pack_start(&img, false, false, 0);
    let v = gtk::Box::new(gtk::Orientation::Vertical, 10);
    let l = gtk::Label::new(Some(&text));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(52);
    v.pack_start(&l, false, false, 0);
    if one {
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let p = &files[0];
        let img = gtk::Image::from_pixbuf(file_icon(p, 48).as_ref());
        img.set_valign(gtk::Align::Start);
        card.pack_start(&img, false, false, 0);
        let lv = gtk::Box::new(gtk::Orientation::Vertical, 1);
        let m = std::fs::symlink_metadata(p).ok();
        let ty = super::listing::stat_entry(p, None).map(|e| e.desc).unwrap_or_default();
        let mut lines = vec![display_name(p)];
        if !is_dir { lines.push(format!("Type: {ty}")); }
        if let Some(m) = &m {
            if !is_dir { lines.push(format!("Size: {}", util::human_size(m.len()))); }
            if is_dir { if let Ok(t) = m.created().or_else(|_| m.modified()) { lines.push(format!("Date created: {}", chrono::DateTime::<chrono::Local>::from(t).format("%-m/%-d/%Y %-I:%M %p"))); } }
            else { lines.push(format!("Date modified: {}", fmt_mtime(m))); }
        }
        for (i, t) in lines.iter().enumerate() {
            let l = gtk::Label::new(Some(t));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            l.set_max_width_chars(46);
            if i > 0 { l.style_context().add_class("dim-label"); }
            lv.pack_start(&l, false, false, 0);
        }
        card.pack_start(&lv, true, true, 0);
        v.pack_start(&card, false, false, 0);
    }
    h.pack_start(&v, true, true, 0);
    c.pack_start(&h, false, false, 0);
    d.add_button("Yes", gtk::ResponseType::Yes);
    d.add_button("No", gtk::ResponseType::No);
    d.set_default_response(gtk::ResponseType::Yes);
    d.show_all();
    sounds::play(sounds::Event::Exclamation);
    let r = d.run();
    d.close();
    r == gtk::ResponseType::Yes
}

// ------------------------------------------------------------------ public operations

/// Copy or move `files` into folder `dest` (Win7 rules: copying into the same folder makes
/// "X - Copy"; moving into the same folder does nothing). `on_done` gets the new paths.
pub fn transfer(files: Vec<PathBuf>, dest: PathBuf, kind: Kind, parent: Option<&gtk::Window>, on_done: impl Fn(Vec<PathBuf>) + 'static) {
    let pairs: Vec<(PathBuf, PathBuf)> = files.into_iter().filter_map(|f| {
        let name = f.file_name()?.to_os_string();
        Some((f, dest.join(name)))
    }).collect();
    if pairs.is_empty() { return; }
    run_transfer(pairs, kind, true, parent, true, move |r| on_done(r.pairs.into_iter().map(|(_, t)| t).collect()));
}

fn run_transfer(pairs: Vec<(PathBuf, PathBuf)>, kind: Kind, rename_same_dir: bool, parent: Option<&gtk::Window>, record: bool, on_done: impl FnOnce(JobResult) + 'static) {
    let jk = if kind == Kind::Copy { JobKind::Copy } else { JobKind::Move };
    run_job(jk, parent, move |w| w.transfer(pairs, rename_same_dir), move |r| {
        if record && !r.pairs.is_empty() {
            push_undo(if kind == Kind::Copy { UndoOp::Copy(r.pairs.iter().map(|(_, t)| t.clone()).collect()) } else { UndoOp::Move(r.pairs.clone()) });
        }
        on_done(r);
    });
}

/// Delete with the Win7 confirmation (when `confirm`): to the Recycle Bin, or permanently
/// (Shift+Delete, items already in the Recycle Bin, drives without a trash).
pub fn delete(files: Vec<PathBuf>, permanent: bool, parent: Option<&gtk::Window>, confirm: bool, on_done: impl Fn() + 'static) {
    let files: Vec<PathBuf> = files.into_iter().filter(|f| f.file_name().is_some()).collect();
    if files.is_empty() { return; }
    let in_bin = files.iter().all(|f| trash::in_trash(f));
    let permanent = permanent || in_bin || files.iter().any(|f| !can_trash(f));
    if confirm && !confirm_delete(&files, permanent, parent) { return; }
    let jk = if permanent { JobKind::Delete } else { JobKind::Recycle };
    run_job(jk, parent, move |w| w.delete(files, permanent), move |r| {
        if !r.trashed.is_empty() {
            sounds::play(sounds::Event::EmptyRecycleBin);
            push_undo(UndoOp::Trash(r.trashed));
        }
        on_done();
    });
}

/// Put Recycle Bin items back where they came from.
pub fn restore(items: Vec<trash::Trashed>, parent: Option<&gtk::Window>, on_done: impl Fn() + 'static) {
    let pairs: Vec<(PathBuf, PathBuf)> = items.iter().map(|t| (t.file.clone(), t.orig.clone())).collect();
    for t in &items { if let Some(p) = t.orig.parent() { let _ = std::fs::create_dir_all(p); } }
    run_transfer(pairs, Kind::Move, false, parent, false, move |r| {
        for (src, _) in &r.pairs {
            if let Some(t) = items.iter().find(|t| &t.file == src) { let _ = std::fs::remove_file(&t.info); }
        }
        on_done();
    });
}

/// "Empty the Recycle Bin" with the Win7 confirmation.
pub fn empty_trash(parent: Option<&gtk::Window>, on_done: impl Fn() + 'static) {
    let n = trash::count();
    if n == 0 { return; }
    let text = if n == 1 { "Are you sure you want to permanently delete this item?".to_string() } else { format!("Are you sure you want to permanently delete these {n} items?") };
    let (d, c) = dialog(if n == 1 { "Delete File" } else { "Delete Multiple Items" }, parent);
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    h.pack_start(&gtk::Image::from_pixbuf(util::pixbuf_or("user-trash-full", 32, "dialog-warning").as_ref()), false, false, 0);
    let l = gtk::Label::new(Some(&text));
    l.set_xalign(0.0);
    h.pack_start(&l, true, true, 0);
    c.pack_start(&h, false, false, 0);
    d.add_button("Yes", gtk::ResponseType::Yes);
    d.add_button("No", gtk::ResponseType::No);
    d.set_default_response(gtk::ResponseType::Yes);
    d.show_all();
    sounds::play(sounds::Event::Exclamation);
    let r = d.run();
    d.close();
    if r != gtk::ResponseType::Yes { return; }
    // everything in files/ (also orphans without info) plus the info files
    let mut files: Vec<PathBuf> = vec![];
    for dir in trash::trash_dirs() {
        if let Ok(rd) = std::fs::read_dir(dir.join("files")) { files.extend(rd.flatten().map(|e| e.path())); }
    }
    run_job(JobKind::Delete, parent, move |w| {
        let r = w.delete(files, true);
        if !r.cancelled {
            for dir in trash::trash_dirs() {
                if let Ok(rd) = std::fs::read_dir(dir.join("info")) { for e in rd.flatten() { let _ = std::fs::remove_file(e.path()); } }
                let _ = std::fs::remove_file(dir.join("directorysizes"));
            }
        }
        r
    }, move |_| { sounds::play(sounds::Event::EmptyRecycleBin); on_done(); });
}

/// Rename with Win7's checks; records Undo. Returns the new path.
pub fn rename(p: &Path, new_name: &str, parent: Option<&gtk::Window>) -> Option<PathBuf> {
    // Windows trims spaces around the name (and trailing dots)
    let new_name = new_name.trim().trim_end_matches('.').trim_end();
    let old = display_name(p);
    if new_name.is_empty() { util::error("Rename", "You must type a file name."); return None; }
    if new_name == old { return None; }
    if new_name.contains('/') || new_name == "." || new_name == ".." {
        util::error("Rename", "A file name can't contain the character /");
        return None;
    }
    let mut target = p.with_file_name(new_name);
    // a different extension can make a file unusable
    if !p.is_dir() {
        let (_, oe) = trash::split_ext(&old);
        let (_, ne) = trash::split_ext(new_name);
        if !oe.eq_ignore_ascii_case(ne) && !oe.is_empty() {
            let m = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::YesNo,
                "If you change a file name extension, the file might become unusable.\n\nAre you sure you want to change it?");
            m.set_title("Rename");
            sounds::play(sounds::Event::Exclamation);
            let r = m.run();
            m.close();
            if r != gtk::ResponseType::Yes { return None; }
        }
    }
    // the name is taken (on a case-insensitive drive "a"->"A" finds the file itself: no conflict)
    let same_file = |a: &Path, b: &Path| match (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) { (Ok(x), Ok(y)) => x.ino() == y.ino() && x.dev() == y.dev(), _ => false };
    if target.symlink_metadata().is_ok() && !same_file(&target, p) {
        let alt = numbered_name(&target);
        let kind = if p.is_dir() { "folder" } else { "file" };
        let m = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo,
            &format!("There is already a {kind} with the same name in this location."));
        m.set_title(if p.is_dir() { "Rename Folder" } else { "Rename File" });
        m.set_secondary_text(Some(&format!("Do you want to rename \"{old}\" to \"{}\"?", display_name(&alt))));
        sounds::play(sounds::Event::Exclamation);
        let r = m.run();
        m.close();
        if r != gtk::ResponseType::Yes { return None; }
        target = alt;
    }
    match std::fs::rename(p, &target) {
        Ok(()) => { push_undo(UndoOp::Rename { from: p.to_path_buf(), to: target.clone() }); Some(target) }
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::PermissionDenied { "You need permission to perform this action".to_string() } else { e.to_string() };
            util::error(if p.is_dir() { "Rename Folder" } else { "Rename File" }, &msg);
            None
        }
    }
}

/// New folder (Ctrl+Shift+N) / New > Text Document: creates it and records Undo.
pub fn create_new(dir: &Path, folder: bool) -> Option<PathBuf> {
    let p = if folder { new_name(dir, "New folder", "") } else { new_name(dir, "New Text Document", ".txt") };
    let r = if folder { std::fs::create_dir(&p) } else { std::fs::OpenOptions::new().write(true).create_new(true).open(&p).map(|_| ()) };
    match r {
        Ok(()) => { push_undo(UndoOp::New(p.clone())); Some(p) }
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::PermissionDenied { "You need permission to perform this action".to_string() } else { e.to_string() };
            util::error(if folder { "Unable to create the folder" } else { "Unable to create the file" }, &msg);
            None
        }
    }
}

/// Shortcuts are symbolic links named "X - Shortcut".
pub fn create_shortcuts(targets: &[PathBuf], dir: &Path) -> Vec<PathBuf> {
    let mut made = vec![];
    for t in targets {
        let link = shortcut_name(dir, t);
        match std::os::unix::fs::symlink(t, &link) {
            Ok(()) => made.push(link),
            Err(e) => { util::error("Unable to create the shortcut", &e.to_string()); break; }
        }
    }
    made
}

/// Send to > Compressed (zipped) folder: "<first name>.zip" next to the files, made in the background.
pub fn zip(files: &[PathBuf], on_done: impl FnOnce(Option<PathBuf>) + 'static) {
    let Some(first) = files.first() else { return };
    let Some(dir) = first.parent().map(|p| p.to_path_buf()) else { return };
    let stem = if first.is_dir() { display_name(first) } else { trash::split_ext(&display_name(first)).0.to_string() };
    let target = new_name(&dir, &stem, ".zip");
    let names: Vec<std::ffi::OsString> = files.iter().filter_map(|f| f.file_name().map(|n| n.to_os_string())).collect();
    let tool = if util::which("zip") { Some("zip") } else if util::which("7z") { Some("7z") } else if util::which("bsdtar") { Some("bsdtar") } else { None };
    let Some(tool) = tool else {
        util::error("Compressed (zipped) Folders", "No program to create .zip files was found. Install the zip package (sudo pacman -S zip).");
        return;
    };
    let (tx, rx) = async_channel::bounded::<bool>(1);
    let t2 = target.clone();
    std::thread::spawn(move || {
        let mut c = std::process::Command::new(tool);
        match tool {
            "zip" => { c.arg("-r").arg("-q").arg("-y").arg(&t2).arg("--").args(&names); }
            "7z" => { c.arg("a").arg("-tzip").arg(&t2).arg("--").args(&names); }
            _ => { c.arg("-a").arg("-cf").arg(&t2).args(&names); }
        }
        let ok = c.current_dir(&dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().map(|s| s.success()).unwrap_or(false);
        let _ = tx.send_blocking(ok);
    });
    glib::MainContext::default().spawn_local(async move {
        let ok = rx.recv().await.unwrap_or(false);
        if ok { push_undo(UndoOp::New(target.clone())); on_done(Some(target)); }
        else { let _ = std::fs::remove_file(&target); util::error("Compressed (zipped) Folders", "The compressed (zipped) folder could not be created."); on_done(None); }
    });
}

/// Extract All... for .zip files into "<name>" next to it.
pub fn extract(zip: &Path, on_done: impl FnOnce(Option<PathBuf>) + 'static) {
    let Some(dir) = zip.parent().map(|p| p.to_path_buf()) else { return };
    let target = new_name(&dir, trash::split_ext(&display_name(zip)).0, "");
    let tool = if util::which("unzip") { "unzip" } else if util::which("7z") { "7z" } else if util::which("bsdtar") { "bsdtar" } else {
        util::error("Compressed (zipped) Folders", "No program to extract .zip files was found. Install the unzip package.");
        return;
    };
    let (z, t2) = (zip.to_path_buf(), target.clone());
    let (tx, rx) = async_channel::bounded::<bool>(1);
    std::thread::spawn(move || {
        let _ = std::fs::create_dir(&t2);
        let mut c = std::process::Command::new(tool);
        match tool {
            "unzip" => { c.arg("-q").arg(&z).arg("-d").arg(&t2); }
            "7z" => { c.arg("x").arg(format!("-o{}", t2.display())).arg(&z); }
            _ => { c.arg("-xf").arg(&z).arg("-C").arg(&t2); }
        }
        let ok = c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        let _ = tx.send_blocking(ok);
    });
    glib::MainContext::default().spawn_local(async move {
        let ok = rx.recv().await.unwrap_or(false);
        if ok { push_undo(UndoOp::New(target.clone())); on_done(Some(target)); } else { util::error("Compressed (zipped) Folders", "The compressed (zipped) folder is invalid."); on_done(None); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aerode-ops-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn win7_names() {
        let d = tmp("names");
        std::fs::write(d.join("a.txt"), "").unwrap();
        let os = std::ffi::OsStr::new;
        assert_eq!(copy_name(&d, os("a.txt")), d.join("a - Copy.txt"));
        std::fs::write(d.join("a - Copy.txt"), "").unwrap();
        assert_eq!(copy_name(&d, os("a.txt")), d.join("a - Copy (2).txt"));
        // names that are not UTF-8 keep their bytes
        {
            use std::os::unix::ffi::OsStrExt;
            let bad = std::ffi::OsStr::from_bytes(b"b\xffd.txt");
            std::fs::write(d.join(bad), "").unwrap();
            assert_eq!(copy_name(&d, bad), d.join(std::ffi::OsStr::from_bytes(b"b\xffd - Copy.txt")));
            assert_eq!(numbered_name(&d.join(bad)), d.join(std::ffi::OsStr::from_bytes(b"b\xffd (2).txt")));
        }
        assert_eq!(numbered_name(&d.join("a.txt")), d.join("a (2).txt"));
        std::fs::write(d.join("a (2).txt"), "").unwrap();
        assert_eq!(numbered_name(&d.join("a (2).txt")), d.join("a (3).txt"));
        assert_eq!(new_name(&d, "New folder", ""), d.join("New folder"));
        std::fs::create_dir(d.join("New folder")).unwrap();
        assert_eq!(new_name(&d, "New folder", ""), d.join("New folder (2)"));
        assert_eq!(shortcut_name(&d, &d.join("a.txt")), d.join("a - Shortcut.txt"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn worker(kind: Kind) -> (Worker, async_channel::Receiver<ToGui>) {
        let (tx, rx) = async_channel::unbounded();
        (Worker { tx, cancel: Arc::new(AtomicBool::new(false)), kind, file_policy: None, folder_policy: None, error_policy: None, trash_policy: None,
            conflicts_left: 0, prog: Progress::default(), last: Instant::now() }, rx)
    }

    #[test]
    fn copy_tree_and_merge() {
        let d = tmp("copy");
        let src = d.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/f.txt"), "hello").unwrap();
        std::os::unix::fs::symlink("sub/f.txt", src.join("link")).unwrap();
        let dst = d.join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        let (mut w, _rx) = worker(Kind::Copy);
        let r = w.transfer(vec![(src.clone(), dst.join("src"))], true);
        assert!(!r.cancelled);
        assert_eq!(std::fs::read_to_string(dst.join("src/sub/f.txt")).unwrap(), "hello");
        assert!(dst.join("src/link").symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(w.prog.done_bytes, 5 + std::fs::symlink_metadata(src.join("link")).unwrap().len());
        // copying into the same folder makes "src - Copy"
        let (mut w, _rx) = worker(Kind::Copy);
        let r = w.transfer(vec![(src.clone(), d.join("src"))], true);
        assert_eq!(r.pairs[0].1, d.join("src - Copy"));
        // merge with a policy: folder Yes, files Skip -> existing file stays
        std::fs::write(dst.join("src/sub/f.txt"), "keep").unwrap();
        let (mut w, _rx) = worker(Kind::Copy);
        w.folder_policy = Some(Answer::Yes);
        w.file_policy = Some(Answer::Skip);
        w.transfer(vec![(src.clone(), dst.join("src"))], true);
        assert_eq!(std::fs::read_to_string(dst.join("src/sub/f.txt")).unwrap(), "keep");
        // keep both
        let (mut w, _rx) = worker(Kind::Copy);
        w.folder_policy = Some(Answer::Yes);
        w.file_policy = Some(Answer::KeepBoth);
        w.transfer(vec![(src.clone(), dst.join("src"))], true);
        assert_eq!(std::fs::read_to_string(dst.join("src/sub/f (2).txt")).unwrap(), "hello");
        // move: rename within the drive
        let (mut w, _rx) = worker(Kind::Move);
        let r = w.transfer(vec![(dst.join("src"), d.join("moved"))], true);
        assert_eq!(r.pairs.len(), 1);
        assert!(d.join("moved/sub/f.txt").exists() && !dst.join("src").exists());
        // into itself is refused (the worker asks; a closed channel answers Cancel)
        let (mut w, rx) = worker(Kind::Copy);
        drop(rx);
        let r = w.transfer(vec![(d.join("moved"), d.join("moved/sub/moved"))], true);
        assert!(r.cancelled && !d.join("moved/sub/moved").exists());
        // …also when the destination reaches the source through a symbolic link
        std::os::unix::fs::symlink(d.join("moved/sub"), d.join("via")).unwrap();
        assert!(is_inside(&d.join("via/x/moved"), &d.join("moved")));
        assert!(!is_inside(&d.join("dst/moved"), &d.join("moved")));
        let (mut w, rx) = worker(Kind::Copy);
        drop(rx);
        let r = w.transfer(vec![(d.join("moved"), d.join("via/moved"))], true);
        assert!(r.cancelled && !d.join("moved/sub/moved").exists());
        // Copy and Replace: the new content, no temporary file left behind
        std::fs::write(d.join("new.txt"), "new").unwrap();
        std::fs::write(dst.join("new.txt"), "old").unwrap();
        let (mut w, _rx) = worker(Kind::Copy);
        w.file_policy = Some(Answer::Replace);
        w.transfer(vec![(d.join("new.txt"), dst.join("new.txt"))], true);
        assert_eq!(std::fs::read_to_string(dst.join("new.txt")).unwrap(), "new");
        assert!(std::fs::read_dir(&dst).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains(".aerode-")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
