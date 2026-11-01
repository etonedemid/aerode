//! File operations with Windows 7 style progress / conflict / delete dialogs.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::{sounds, util};

#[derive(Clone, Copy, PartialEq)]
pub enum Kind { Copy, Move }

/// The shared clipboard for cut/copy, compatible with GNOME/KDE file managers.
pub fn set_clipboard(files: &[PathBuf], cut: bool) {
    let cb = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
    let uris: Vec<String> = files.iter().map(|p| gio::File::for_path(p).uri().to_string()).collect();
    let gnome = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
    let uri_list = uris.join("\r\n") + "\r\n";
    let targets = [
        gtk::TargetEntry::new("x-special/gnome-copied-files", gtk::TargetFlags::empty(), 0),
        gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 1),
        gtk::TargetEntry::new("UTF8_STRING", gtk::TargetFlags::empty(), 2),
    ];
    let plain = files.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");
    cb.set_with_data(&targets, move |_, sel, info| {
        match info {
            0 => sel.set(&gtk::gdk::Atom::intern("x-special/gnome-copied-files"), 8, gnome.as_bytes()),
            1 => sel.set(&gtk::gdk::Atom::intern("text/uri-list"), 8, uri_list.as_bytes()),
            _ => { sel.set_text(&plain); }
        }
    });
}

/// Read the clipboard: (files, cut?)
pub fn get_clipboard(cb: impl FnOnce(Vec<PathBuf>, bool) + 'static) {
    let clip = gtk::Clipboard::get(&gtk::gdk::SELECTION_CLIPBOARD);
    let cb = Rc::new(RefCell::new(Some(cb)));
    let cb2 = cb.clone();
    clip.request_contents(&gtk::gdk::Atom::intern("x-special/gnome-copied-files"), move |c, data| {
        let text = String::from_utf8_lossy(&data.data()).to_string();
        if !text.trim().is_empty() {
            let mut lines = text.lines();
            let cut = lines.next() == Some("cut");
            let files = lines.filter_map(|u| gio::File::for_uri(u.trim()).path()).collect();
            if let Some(f) = cb.borrow_mut().take() { f(files, cut); }
            return;
        }
        let cb3 = cb2.clone();
        c.request_uris(move |_, uris| {
            let files = uris.iter().filter_map(|u| gio::File::for_uri(u).path()).collect();
            if let Some(f) = cb3.borrow_mut().take() { f(files, false); }
        });
    });
}

fn unique_name(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() { return p; }
    let (stem, ext) = match name.rsplit_once('.') { Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")), _ => (name.to_string(), String::new()) };
    // Win7: "file - Copy.txt", "file - Copy (2).txt"
    let mut i = 1;
    loop {
        let cand = if i == 1 { format!("{stem} - Copy{ext}") } else { format!("{stem} - Copy ({i}){ext}") };
        let p = dir.join(&cand);
        if !p.exists() { return p; }
        i += 1;
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Conflict { Replace, Skip, KeepBoth, Ask }

/// Win7 "Copy File" conflict dialog. Returns (choice, apply to all).
fn ask_conflict(src: &Path, dst: &Path, kind: Kind) -> (Conflict, bool) {
    let d = gtk::Dialog::new();
    d.set_title(if kind == Kind::Copy { "Copy File" } else { "Move File" });
    d.set_modal(true);
    d.set_keep_above(true);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(10);
    let h = gtk::Label::new(None);
    h.set_markup(&format!("<span size='large' foreground='#003399'>There is already a file with the same name in this location.</span>\nClick the file you want to keep"));
    h.set_xalign(0.0);
    c.pack_start(&h, false, false, 0);
    let choice = Rc::new(Cell::new(Conflict::Skip));
    let info = |p: &Path| {
        let m = std::fs::metadata(p).ok();
        format!("{}\nSize: {}\nDate modified: {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            m.as_ref().map(|m| util::human_size(m.len())).unwrap_or_default(),
            m.and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%-m/%-d/%Y %-I:%M %p").to_string()).unwrap_or_default())
    };
    let opts = [
        (Conflict::Replace, if kind == Kind::Copy { "Copy and Replace" } else { "Move and Replace" }, format!("Replace the file in the destination folder with the file you are {}:\n{}", if kind == Kind::Copy { "copying" } else { "moving" }, info(src))),
        (Conflict::Skip, if kind == Kind::Copy { "Don't copy" } else { "Don't move" }, format!("No files will be changed. Leave this file in the destination folder:\n{}", info(dst))),
        (Conflict::KeepBoth, if kind == Kind::Copy { "Copy, but keep both files" } else { "Move, but keep both files" }, "The file you are copying will be renamed".to_string()),
    ];
    for (k, title, body) in opts {
        let b = gtk::Button::new();
        b.set_relief(gtk::ReliefStyle::None);
        let v = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let t = gtk::Label::new(None);
        t.set_markup(&format!("<span foreground='#151c55' size='large'>➜ {title}</span>"));
        t.set_xalign(0.0);
        let s = gtk::Label::new(Some(&body));
        s.set_xalign(0.0);
        s.set_margin_start(20);
        v.pack_start(&t, false, false, 0);
        v.pack_start(&s, false, false, 0);
        b.add(&v);
        let (ch, d2) = (choice.clone(), d.clone());
        b.connect_clicked(move |_| { ch.set(k); d2.response(gtk::ResponseType::Ok); });
        c.pack_start(&b, false, false, 0);
    }
    let all = gtk::CheckButton::with_label("Do this for all conflicts");
    c.pack_start(&all, false, false, 0);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.show_all();
    sounds::play(sounds::Event::Exclamation);
    let r = d.run();
    let res = if r == gtk::ResponseType::Ok { (choice.get(), all.is_active()) } else { (Conflict::Skip, true) };
    d.close();
    res
}

fn dir_size(p: &Path) -> (u64, u64) {
    let mut bytes = 0;
    let mut count = 0;
    if let Ok(m) = std::fs::symlink_metadata(p) {
        if m.is_dir() {
            if let Ok(rd) = std::fs::read_dir(p) { for e in rd.flatten() { let (b, c) = dir_size(&e.path()); bytes += b; count += c; } }
        } else { bytes += m.len(); count += 1; }
    }
    (bytes, count)
}

/// Copy or move `files` into `dest` with the Win7 progress dialog.
pub fn transfer(files: Vec<PathBuf>, dest: PathBuf, kind: Kind, on_done: impl Fn() + 'static) {
    let files: Vec<PathBuf> = files.into_iter().filter(|f| {
        // moving a folder into itself is refused like Windows
        !(dest.starts_with(f)) && !(kind == Kind::Move && f.parent() == Some(dest.as_path()))
    }).collect();
    if files.is_empty() { return; }
    let (total_bytes, total_items) = files.iter().map(|f| dir_size(f)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    // ---- dialog
    let d = gtk::Window::new(gtk::WindowType::Toplevel);
    d.set_title(&format!("{} {} items", if kind == Kind::Copy { "Copying" } else { "Moving" }, total_items));
    d.set_default_size(440, -1);
    d.set_keep_above(true);
    let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
    v.set_border_width(14);
    let head = gtk::Label::new(None);
    head.set_xalign(0.0);
    let src_dir = files[0].parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dst_dir = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dest.display().to_string());
    head.set_markup(&format!("{} {} items from <span foreground='#0066cc'>{}</span> to <span foreground='#0066cc'>{}</span>",
        if kind == Kind::Copy { "Copying" } else { "Moving" }, total_items, glib::markup_escape_text(&src_dir), glib::markup_escape_text(&dst_dir)));
    let remaining = gtk::Label::new(Some("Calculating..."));
    remaining.set_xalign(0.0);
    remaining.set_markup("<span size='large' foreground='#003399'>Calculating...</span>");
    let bar = gtk::ProgressBar::new();
    let details = gtk::Label::new(None);
    details.set_xalign(0.0);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_halign(gtk::Align::End);
    v.pack_start(&head, false, false, 0);
    v.pack_start(&remaining, false, false, 0);
    v.pack_start(&bar, false, false, 0);
    v.pack_start(&details, false, false, 0);
    v.pack_start(&cancel, false, false, 0);
    d.add(&v);
    let canc = gio::Cancellable::new();
    { let c = canc.clone(); cancel.connect_clicked(move |_| c.cancel()); }
    // show only if it takes a while (like Windows)
    let shown = Rc::new(Cell::new(false));
    { let (d, s) = (d.clone(), shown.clone()); glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || { if !s.get() { s.set(true); d.show_all(); } }); }

    let start = std::time::Instant::now();
    let done_bytes = Rc::new(Cell::new(0u64));
    let mut policy = Conflict::Ask;
    let mut queue: Vec<(PathBuf, PathBuf)> = vec![];
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let mut target = dest.join(&name);
        if target.exists() || f.parent() == Some(dest.as_path()) {
            if f.parent() == Some(dest.as_path()) { target = unique_name(&dest, &name); }
            else {
                let ch = if policy != Conflict::Ask { policy } else { let (c, all) = ask_conflict(f, &target, kind); if all { policy = c; } c };
                match ch {
                    Conflict::Skip => continue,
                    Conflict::KeepBoth => target = unique_name(&dest, &name),
                    _ => {}
                }
            }
        }
        queue.push((f.clone(), target));
    }
    // ---- work: walk sequentially on the main loop (gio async ops)
    let queue = Rc::new(RefCell::new(queue));
    let on_done = Rc::new(on_done);
    fn step(queue: Rc<RefCell<Vec<(PathBuf, PathBuf)>>>, kind: Kind, canc: gio::Cancellable, done: Rc<Cell<u64>>, total: u64,
            start: std::time::Instant, bar: gtk::ProgressBar, remaining: gtk::Label, details: gtk::Label, dlg: gtk::Window,
            shown: Rc<Cell<bool>>, on_done: Rc<dyn Fn()>) {
        let next = { let mut q = queue.borrow_mut(); if q.is_empty() { None } else { Some(q.remove(0)) } };
        let Some((src, dst)) = next else {
            shown.set(true);
            dlg.close();
            on_done();
            return;
        };
        if canc.is_cancelled() { shown.set(true); dlg.close(); on_done(); return; }
        details.set_text(&format!("Name: {}\nItems remaining: {}", src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), queue.borrow().len() + 1));
        let meta = std::fs::symlink_metadata(&src);
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        // fast path: rename within the same filesystem
        if kind == Kind::Move && std::fs::rename(&src, &dst).is_ok() {
            done.set(done.get() + dir_size(&dst).0);
            glib::idle_add_local_once(move || step(queue, kind, canc, done, total, start, bar, remaining, details, dlg, shown, on_done));
            return;
        }
        if is_dir {
            // expand directory into its children (copy dir by creating it, then queue the contents)
            let _ = std::fs::create_dir_all(&dst);
            if let Ok(rd) = std::fs::read_dir(&src) {
                let mut q = queue.borrow_mut();
                let mut kids: Vec<(PathBuf, PathBuf)> = rd.flatten().map(|e| (e.path(), dst.join(e.file_name()))).collect();
                kids.append(&mut q);
                *q = kids;
                if kind == Kind::Move { q.push((src.clone(), PathBuf::from("\u{0}rmdir"))); }
            }
            glib::idle_add_local_once(move || step(queue, kind, canc, done, total, start, bar, remaining, details, dlg, shown, on_done));
            return;
        }
        if dst.as_os_str() == "\u{0}rmdir" {
            let _ = std::fs::remove_dir(&src);
            glib::idle_add_local_once(move || step(queue, kind, canc, done, total, start, bar, remaining, details, dlg, shown, on_done));
            return;
        }
        let base = done.get();
        let (sf, df) = (gio::File::for_path(&src), gio::File::for_path(&dst));
        let flags = gio::FileCopyFlags::OVERWRITE | gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        let (bar2, rem2, done2) = (bar.clone(), remaining.clone(), done.clone());
        let progress = Box::new(move |cur: i64, _tot: i64| {
            let d = base + cur.max(0) as u64;
            done2.set(d);
            let frac = if total > 0 { d as f64 / total as f64 } else { 1.0 };
            bar2.set_fraction(frac.min(1.0));
            let el = start.elapsed().as_secs_f64();
            if el > 1.0 && frac > 0.0 {
                let left = (el / frac - el).max(0.0) as u64;
                let t = if left > 60 { format!("About {} minutes", left / 60 + 1) } else { format!("About {} seconds", left.max(1)) };
                rem2.set_markup(&format!("<span size='large' foreground='#003399'>{t} remaining</span>"));
            }
        });
        let canc2 = canc.clone();
        let cb = move |r: Result<(), glib::Error>| {
            if let Err(e) = &r {
                if !e.matches(gio::IOErrorEnum::Cancelled) { util::error(&format!("Could not {} '{}'", if kind == Kind::Copy { "copy" } else { "move" }, src.display()), e.message()); }
            } else if kind == Kind::Move { let _ = std::fs::remove_file(&src); }
            done.set(base + std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0));
            step(queue, kind, canc, done, total, start, bar, remaining, details, dlg, shown, on_done);
        };
        sf.copy_async(&df, flags, glib::Priority::DEFAULT, Some(&canc2), Some(progress), cb);
    }
    step(queue, kind, canc, done_bytes, total_bytes, start, bar, remaining, details, d, shown, Rc::new(move || on_done()));
}

/// Win7 delete confirmation, then move to the Recycle Bin (or delete permanently).
pub fn delete(files: &[PathBuf], permanent: bool, parent: Option<&gtk::Window>) -> bool {
    if files.is_empty() { return false; }
    let what = if files.len() == 1 {
        let p = &files[0];
        let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if p.is_dir() { format!("this folder") } else { format!("this file") }.replace("this", &format!("'{n}' (this"))
    } else { format!("these {} items", files.len()) };
    let text = if permanent { format!("Are you sure you want to permanently delete {what}?") } else { format!("Are you sure you want to move {what} to the Recycle Bin?") };
    let text = text.replace("(this file", "").replace("(this folder", "");
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::YesNo, &text);
    d.set_title(if files.len() == 1 { if files[0].is_dir() { "Delete Folder" } else { "Delete File" } } else { "Delete Multiple Items" });
    d.set_default_response(gtk::ResponseType::Yes);
    sounds::play(sounds::Event::Exclamation);
    let r = d.run();
    d.close();
    if r != gtk::ResponseType::Yes { return false; }
    for f in files {
        let res = if permanent {
            if f.is_dir() && !f.is_symlink() { std::fs::remove_dir_all(f) } else { std::fs::remove_file(f) }.map_err(|e| e.to_string())
        } else {
            gio::File::for_path(f).trash(gio::Cancellable::NONE).map_err(|e| e.to_string())
        };
        if let Err(e) = res { util::error(&format!("Could not delete '{}'", f.display()), &e); }
    }
    if !permanent { sounds::play(sounds::Event::EmptyRecycleBin); }
    true
}

pub fn empty_trash(parent: Option<&gtk::Window>) {
    let d = gtk::MessageDialog::new(parent, gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::YesNo,
        "Are you sure you want to permanently delete all of the items in the Recycle Bin?");
    d.set_title("Delete Multiple Items");
    sounds::play(sounds::Event::Exclamation);
    let r = d.run();
    d.close();
    if r == gtk::ResponseType::Yes {
        let t = glib::user_data_dir().join("Trash");
        for sub in ["files", "info"] {
            if let Ok(rd) = std::fs::read_dir(t.join(sub)) {
                for e in rd.flatten() { let p = e.path(); let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) }; }
            }
        }
        sounds::play(sounds::Event::EmptyRecycleBin);
    }
}

/// Restore an item from the Recycle Bin to its original location.
pub fn restore(trash_name: &str) -> bool {
    let t = glib::user_data_dir().join("Trash");
    let info = std::fs::read_to_string(t.join("info").join(format!("{trash_name}.trashinfo"))).unwrap_or_default();
    let Some(orig) = info.lines().find_map(|l| l.strip_prefix("Path=")) else { return false };
    let orig = PathBuf::from(percent_decode(orig));
    if let Some(p) = orig.parent() { let _ = std::fs::create_dir_all(p); }
    let ok = std::fs::rename(t.join("files").join(trash_name), &orig).is_ok();
    if ok { let _ = std::fs::remove_file(t.join("info").join(format!("{trash_name}.trashinfo"))); }
    ok
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; continue; }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
