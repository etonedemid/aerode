//! Windows 7 style Properties dialog: General (sizes and contents counted in the background,
//! attributes), Shortcut (symlinks), Security (Unix permissions), Details (images); several
//! selected items get the combined "N Files, M Folders" sheet.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{listing, ops, trash};
use crate::util;

fn fmt_time(t: Option<std::time::SystemTime>) -> String {
    t.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%A, %B %-d, %Y, %-I:%M:%S %p").to_string()).unwrap_or_else(|| "—".into())
}

fn bytes_text(n: u64) -> String {
    let mut s = n.to_string();
    let mut o = String::new();
    while s.len() > 3 { o = format!(",{}{o}", &s[s.len() - 3..]); s.truncate(s.len() - 3); }
    format!("{} ({s}{o} bytes)", util::human_size(n))
}

fn row(g: &gtk::Grid, r: i32, k: &str, v: &gtk::Widget) {
    let l = gtk::Label::new(Some(k));
    l.set_xalign(0.0);
    l.set_valign(gtk::Align::Start);
    g.attach(&l, 0, r, 1, 1);
    v.set_halign(gtk::Align::Start);
    g.attach(v, 1, r, 1, 1);
}

fn val(s: &str) -> gtk::Widget {
    let l = gtk::Label::new(Some(s));
    l.set_xalign(0.0);
    l.set_selectable(true);
    l.set_line_wrap(true);
    l.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_max_width_chars(38);
    l.upcast()
}

fn sep(g: &gtk::Grid, r: &mut i32) {
    g.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, *r, 2, 1);
    *r += 1;
}

/// (bytes, bytes on disk, files, folders)
type Count = (u64, u64, u64, u64);

/// Count the items in the background; the labels update live like Win7 and the thread stops
/// when the dialog closes.
fn count_async(paths: Vec<PathBuf>, size_l: gtk::Label, disk_l: gtk::Label, contains: Option<gtk::Label>, dialog: &gtk::Dialog) {
    let stop = Arc::new(AtomicBool::new(false));
    { let s = stop.clone(); dialog.connect_destroy(move |_| s.store(true, Ordering::Relaxed)); }
    let (tx, rx) = async_channel::unbounded::<(Count, bool)>();
    std::thread::spawn(move || {
        fn walk(p: &Path, top: bool, acc: &mut Count, stop: &AtomicBool, tx: &async_channel::Sender<(Count, bool)>, last: &mut std::time::Instant) {
            if stop.load(Ordering::Relaxed) { return; }
            let Ok(m) = std::fs::symlink_metadata(p) else { return };
            if m.is_dir() {
                if !top { acc.3 += 1; }
                if let Ok(rd) = std::fs::read_dir(p) { for e in rd.flatten() { walk(&e.path(), false, acc, stop, tx, last); } }
            } else {
                acc.0 += m.len();
                acc.1 += m.blocks() * 512;
                acc.2 += 1;
            }
            if last.elapsed() > std::time::Duration::from_millis(150) { *last = std::time::Instant::now(); let _ = tx.send_blocking((*acc, false)); }
        }
        let mut acc = (0, 0, 0, 0);
        let mut last = std::time::Instant::now();
        let multi = paths.len() > 1;
        for p in &paths {
            // with several items the selected folders count too
            if multi && std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false) { acc.3 += 1; }
            walk(p, true, &mut acc, &stop, &tx, &mut last);
        }
        let _ = tx.send_blocking((acc, true));
    });
    glib::MainContext::default().spawn_local(async move {
        while let Ok(((b, disk, files, dirs), done)) = rx.recv().await {
            size_l.set_text(&bytes_text(b));
            disk_l.set_text(&bytes_text(disk));
            if let Some(c) = &contains { c.set_text(&format!("{} Files, {} Folders", group(files), group(dirs))); }
            if done { break; }
        }
    });
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); }
    o
}

fn dialog(title: &str, parent: Option<&gtk::Window>) -> (gtk::Dialog, gtk::Notebook) {
    let d = gtk::Dialog::new();
    d.set_title(title);
    if let Some(p) = parent { d.set_transient_for(Some(p)); }
    d.set_default_size(390, -1);
    d.set_resizable(false);
    d.set_icon_name(Some("document-properties"));
    let nb = gtk::Notebook::new();
    nb.set_border_width(6);
    d.content_area().pack_start(&nb, true, true, 0);
    (d, nb)
}

fn grid() -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_border_width(12);
    g.set_row_spacing(7);
    g.set_column_spacing(18);
    g
}

/// Properties of one or more items.
pub fn show(paths: &[PathBuf], parent: Option<&gtk::Window>) {
    let paths: Vec<PathBuf> = paths.iter().filter(|p| p.symlink_metadata().is_ok()).cloned().collect();
    match paths.len() {
        0 => util::error("Properties", "The item can't be found. It may have been moved or deleted."),
        1 => show_one(&paths[0], parent),
        _ => show_many(paths, parent),
    }
}

fn show_many(paths: Vec<PathBuf>, parent: Option<&gtk::Window>) {
    let first = &paths[0];
    let (files, dirs): (Vec<&PathBuf>, Vec<&PathBuf>) = paths.iter().partition(|p| !p.is_dir());
    let (d, nb) = dialog(&format!("{}, ... Properties", ops::display_name(first)), parent);
    let g = grid();
    let mut r = 0;
    g.attach(&gtk::Image::from_pixbuf(util::pixbuf_or("document-multiple", 32, "text-x-generic").as_ref()), 0, r, 1, 1);
    let what = match (files.len(), dirs.len()) { (f, 0) => format!("{f} Files"), (0, d) => format!("{d} Folders"), (f, d) => format!("{f} Files, {d} Folders") };
    g.attach(&val(&what), 1, r, 1, 1);
    r += 1;
    sep(&g, &mut r);
    let types: std::collections::HashSet<String> = paths.iter().filter_map(|p| listing::stat_entry(p, None)).map(|e| e.desc).collect();
    let ty = if types.len() == 1 { format!("All of type {}", types.iter().next().cloned().unwrap_or_default()) } else { "Multiple Types".into() };
    row(&g, r, "Type:", &val(&ty)); r += 1;
    let parents: std::collections::HashSet<Option<&Path>> = paths.iter().map(|p| p.parent()).collect();
    let loc = if parents.len() == 1 { format!("All in {}", first.parent().map(|p| p.display().to_string()).unwrap_or_default()) } else { "Multiple Folders".into() };
    row(&g, r, "Location:", &val(&loc)); r += 1;
    let size_l = gtk::Label::new(Some("Calculating..."));
    let disk_l = gtk::Label::new(Some("Calculating..."));
    row(&g, r, "Size:", size_l.upcast_ref()); r += 1;
    row(&g, r, "Size on disk:", disk_l.upcast_ref()); r += 1;
    sep(&g, &mut r);
    let attrs = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let ro = gtk::CheckButton::with_label("Read-only");
    let ro_states: Vec<bool> = paths.iter().map(|p| std::fs::metadata(p).map(|m| m.permissions().mode() & 0o200 == 0).unwrap_or(false)).collect();
    ro.set_active(ro_states.iter().all(|x| *x));
    ro.set_inconsistent(ro_states.iter().any(|x| *x) && !ro_states.iter().all(|x| *x));
    attrs.pack_start(&ro, false, false, 0);
    row(&g, r, "Attributes:", attrs.upcast_ref());
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    count_async(paths.clone(), size_l, disk_l, None, &d);
    let touched = Rc::new(RefCell::new(false));
    { let t = touched.clone(); ro.connect_toggled(move |c| { c.set_inconsistent(false); *t.borrow_mut() = true; }); }
    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.add_button("Apply", gtk::ResponseType::Apply);
    d.connect_response(move |d, resp| {
        if (resp == gtk::ResponseType::Ok || resp == gtk::ResponseType::Apply) && *touched.borrow() {
            for p in &paths { set_readonly(p, ro.is_active()); }
            *touched.borrow_mut() = false;
        }
        if resp != gtk::ResponseType::Apply { d.close(); }
    });
    d.show_all();
}

fn set_readonly(p: &Path, on: bool) {
    let Ok(m) = std::fs::metadata(p) else { return };
    let mode = m.permissions().mode();
    // read-only removes write for everyone; clearing it gives the owner write again
    let new = if on { mode & !0o222 } else { mode | 0o200 };
    if new != mode {
        if let Err(e) = std::fs::set_permissions(p, std::fs::Permissions::from_mode(new)) {
            util::error("Error Applying Attributes", &format!("An error occurred applying attributes to the file:\n{}\n\n{e}", p.display()));
        }
    }
}

fn show_one(path: &Path, parent: Option<&gtk::Window>) {
    let path = path.to_path_buf();
    let Ok(meta) = std::fs::symlink_metadata(&path) else { util::error("Cannot read properties", &path.display().to_string()); return };
    let is_link = meta.file_type().is_symlink();
    let tmeta = std::fs::metadata(&path).unwrap_or_else(|_| meta.clone());
    let is_dir = tmeta.is_dir();
    let trashed = if trash::in_trash(&path) { trash::lookup(&path) } else { None };
    let name = match &trashed { Some(t) => ops::display_name(&t.orig), None => ops::display_name(&path) };
    let entry = listing::stat_entry(&path, None);
    let (d, nb) = dialog(&format!("{name} Properties"), parent);

    // ---- General
    let g = grid();
    let icon = gio::File::for_path(&path).query_info("standard::icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok().and_then(|i| i.icon());
    g.attach(&gtk::Image::from_pixbuf(util::gicon_pixbuf(icon.as_ref(), 32).as_ref()), 0, 0, 1, 1);
    let name_e = gtk::Entry::new();
    name_e.set_text(&name);
    name_e.set_hexpand(true);
    name_e.set_sensitive(trashed.is_none());
    g.attach(&name_e, 1, 0, 1, 1);
    let mut r = 1;
    sep(&g, &mut r);
    let ctype = entry.as_ref().map(|e| e.ctype.clone()).unwrap_or_default();
    let desc = entry.as_ref().map(|e| e.desc.clone()).unwrap_or_default();
    if is_link && !is_dir {
        row(&g, r, "Type of file:", &val("Shortcut")); r += 1;
    } else if is_dir {
        row(&g, r, "Type:", &val("File folder")); r += 1;
    } else {
        let ext = path.extension().map(|e| format!(" (.{})", e.to_string_lossy())).unwrap_or_default();
        row(&g, r, "Type of file:", &val(&format!("{desc}{ext}"))); r += 1;
        let app = gio::AppInfo::default_for_type(&ctype, false);
        let ob = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let app_img = gtk::Image::new();
        let app_l = gtk::Label::new(None);
        let set_app = { let (i, l) = (app_img.clone(), app_l.clone()); move |a: Option<&gio::AppInfo>| {
            match a { Some(a) => { i.set_from_pixbuf(util::gicon_pixbuf(a.icon().as_ref(), 16).as_ref()); l.set_text(&a.display_name()); } None => { i.clear(); l.set_text("Unknown application"); } }
        } };
        set_app(app.as_ref());
        ob.pack_start(&app_img, false, false, 0);
        ob.pack_start(&app_l, false, false, 0);
        let change = gtk::Button::with_label("Change...");
        { let (ct, p) = (ctype.clone(), path.clone()); change.connect_clicked(move |b| {
            let w = b.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok());
            let dlg = gtk::AppChooserDialog::new(w.as_ref(), gtk::DialogFlags::MODAL, &gio::File::for_path(&p));
            dlg.set_title("Open with");
            if dlg.run() == gtk::ResponseType::Ok { if let Some(a) = dlg.app_info() { if a.set_as_default_for_type(&ct).is_ok() { set_app(Some(&a)); } } }
            dlg.close();
        }); }
        ob.pack_start(&change, false, false, 12);
        row(&g, r, "Opens with:", ob.upcast_ref()); r += 1;
    }
    sep(&g, &mut r);
    if let Some(t) = &trashed {
        row(&g, r, "Origin:", &val(&t.orig.parent().map(|p| p.display().to_string()).unwrap_or_default())); r += 1;
        row(&g, r, "Deleted:", &val(&chrono::DateTime::from_timestamp(t.deleted, 0).map(|d| d.with_timezone(&chrono::Local).format("%-m/%-d/%Y %-I:%M %p").to_string()).unwrap_or_default())); r += 1;
    } else {
        row(&g, r, "Location:", &val(&path.parent().map(|p| p.display().to_string()).unwrap_or_default())); r += 1;
    }
    let size_l = gtk::Label::new(Some(if is_dir { "Calculating..." } else { "" }));
    size_l.set_xalign(0.0);
    let disk_l = gtk::Label::new(None);
    disk_l.set_xalign(0.0);
    row(&g, r, "Size:", size_l.upcast_ref()); r += 1;
    row(&g, r, "Size on disk:", disk_l.upcast_ref()); r += 1;
    let contains = gtk::Label::new(Some("Calculating..."));
    contains.set_xalign(0.0);
    if is_dir && !is_link { row(&g, r, "Contains:", contains.upcast_ref()); r += 1; }
    if is_dir && !is_link {
        count_async(vec![path.clone()], size_l.clone(), disk_l.clone(), Some(contains.clone()), &d);
    } else {
        size_l.set_text(&bytes_text(meta.len()));
        disk_l.set_text(&bytes_text(meta.blocks() * 512));
    }
    sep(&g, &mut r);
    row(&g, r, "Created:", &val(&fmt_time(meta.created().ok()))); r += 1;
    row(&g, r, "Modified:", &val(&fmt_time(meta.modified().ok()))); r += 1;
    row(&g, r, "Accessed:", &val(&fmt_time(meta.accessed().ok()))); r += 1;
    sep(&g, &mut r);
    let attrs = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let ro = gtk::CheckButton::with_label("Read-only");
    ro.set_active(tmeta.permissions().mode() & 0o200 == 0);
    let hidden = gtk::CheckButton::with_label("Hidden");
    hidden.set_active(name.starts_with('.'));
    hidden.set_sensitive(trashed.is_none());
    attrs.pack_start(&ro, false, false, 0);
    attrs.pack_start(&hidden, false, false, 0);
    row(&g, r, "Attributes:", attrs.upcast_ref());
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));

    // ---- Shortcut (symbolic links)
    if is_link {
        let sg = grid();
        let target = std::fs::read_link(&path).unwrap_or_default();
        let abs = if target.is_absolute() { target.clone() } else { path.parent().map(|p| p.join(&target)).unwrap_or(target.clone()) };
        sg.attach(&gtk::Image::from_pixbuf(util::gicon_pixbuf(icon.as_ref(), 32).as_ref()), 0, 0, 1, 1);
        sg.attach(&val(&name), 1, 0, 1, 1);
        let mut r = 1;
        sep(&sg, &mut r);
        row(&sg, r, "Target type:", &val(if abs.is_dir() { "File folder" } else if abs.exists() { "File" } else { "(missing)" })); r += 1;
        row(&sg, r, "Target location:", &val(&abs.parent().map(|p| ops::display_name(p)).unwrap_or_default())); r += 1;
        let te = gtk::Entry::new();
        te.set_text(&target.to_string_lossy());
        te.set_editable(false);
        te.set_width_chars(34);
        row(&sg, r, "Target:", te.upcast_ref()); r += 1;
        sep(&sg, &mut r);
        let open_loc = gtk::Button::with_label("Open File Location");
        { let a = abs.clone(); open_loc.connect_clicked(move |_| { if let Some(p) = a.parent() { util::explore(&p.to_string_lossy()); } }); }
        open_loc.set_sensitive(abs.exists());
        sg.attach(&open_loc, 0, r, 2, 1);
        nb.append_page(&sg, Some(&gtk::Label::new(Some("Shortcut"))));
    }

    // ---- Security (Unix permissions presented like the Win7 tab)
    let sec = gtk::Grid::new();
    sec.set_border_width(12);
    sec.set_row_spacing(6);
    sec.set_column_spacing(12);
    let (owner, groupn) = (user_name(tmeta.uid()), group_name(tmeta.gid()));
    let ol = gtk::Label::new(Some(&format!("Object name:  {}\n\nOwner: {owner}    Group: {groupn}", path.display())));
    ol.set_xalign(0.0);
    ol.set_line_wrap(true);
    ol.set_max_width_chars(48);
    sec.attach(&ol, 0, 0, 4, 1);
    let hdr = ["Permissions for", "Read", "Write", if is_dir { "List/Enter" } else { "Execute" }];
    for (i, h) in hdr.iter().enumerate() { let l = gtk::Label::new(Some(h)); l.set_xalign(0.0); sec.attach(&l, i as i32, 1, 1, 1); }
    let mode = tmeta.permissions().mode();
    let mut checks = vec![];
    let mine = tmeta.uid() == unsafe { libc::getuid() } || unsafe { libc::getuid() } == 0;
    for (ri, (who, shift)) in [(owner.as_str(), 6), (groupn.as_str(), 3), ("Everyone", 0)].iter().enumerate() {
        let l = gtk::Label::new(Some(who));
        l.set_xalign(0.0);
        sec.attach(&l, 0, ri as i32 + 2, 1, 1);
        for (ci, bit) in [4u32, 2, 1].iter().enumerate() {
            let c = gtk::CheckButton::new();
            c.set_active(mode & (bit << shift) != 0);
            c.set_sensitive(mine);
            sec.attach(&c, ci as i32 + 1, ri as i32 + 2, 1, 1);
            checks.push((c, bit << shift));
        }
    }
    if !mine {
        let l = gtk::Label::new(Some("To change permissions, you must be the owner of this item."));
        l.set_xalign(0.0);
        sec.attach(&l, 0, 6, 4, 1);
    }
    nb.append_page(&sec, Some(&gtk::Label::new(Some("Security"))));

    // ---- Details (images)
    if ctype.starts_with("image/") {
        let dg = grid();
        if let Some((fmt, w, h)) = gtk::gdk_pixbuf::Pixbuf::file_info(&path) {
            let mut r = 0;
            let hl = gtk::Label::new(None);
            hl.set_markup("<b>Image</b>");
            hl.set_xalign(0.0);
            dg.attach(&hl, 0, r, 2, 1); r += 1;
            row(&dg, r, "Dimensions", &val(&format!("{w} x {h}"))); r += 1;
            row(&dg, r, "Width", &val(&format!("{w} pixels"))); r += 1;
            row(&dg, r, "Height", &val(&format!("{h} pixels"))); r += 1;
            if let Some(n) = fmt.name() { row(&dg, r, "Format", &val(&n.to_uppercase())); r += 1; }
            let fl = gtk::Label::new(None);
            fl.set_markup("<b>File</b>");
            fl.set_xalign(0.0);
            dg.attach(&fl, 0, r, 2, 1); r += 1;
            row(&dg, r, "Name", &val(&name)); r += 1;
            row(&dg, r, "Size", &val(&util::human_size(meta.len())));
        }
        nb.append_page(&dg, Some(&gtk::Label::new(Some("Details"))));
    }

    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let apply = d.add_button("Apply", gtk::ResponseType::Apply);
    apply.set_sensitive(false);
    // Apply lights up after a change, like Windows
    {
        let a = apply.clone();
        name_e.connect_changed(move |_| a.set_sensitive(true));
        let a = apply.clone();
        ro.connect_toggled(move |_| a.set_sensitive(true));
        let a = apply.clone();
        hidden.connect_toggled(move |_| a.set_sensitive(true));
        for (c, _) in &checks { let a = apply.clone(); c.connect_toggled(move |_| a.set_sensitive(true)); }
    }
    let cur = Rc::new(RefCell::new(path.clone()));
    let orig_mode = mode & 0o7777;
    let ro_orig = ro.is_active();
    d.connect_response(move |d, resp| {
        if (resp == gtk::ResponseType::Ok || resp == gtk::ResponseType::Apply) && apply.is_sensitive() {
            let p = cur.borrow().clone();
            // permissions: only when something changed
            let mut m = orig_mode & !0o777;
            for (c, bit) in &checks { if c.is_active() { m |= bit; } }
            if ro.is_active() != ro_orig { if ro.is_active() { m &= !0o222; } else { m |= 0o200; } }
            if m != orig_mode && !is_link {
                if let Err(e) = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(m)) {
                    util::error("Error Applying Security", &format!("An error occurred while applying security information to:\n{}\n\n{e}", p.display()));
                }
            }
            // rename / hide
            let typed = name_e.text().to_string();
            let mut new = typed.clone();
            if hidden.is_active() && !new.starts_with('.') { new = format!(".{new}"); }
            if !hidden.is_active() && new.starts_with('.') { let t = new.trim_start_matches('.').to_string(); if !t.is_empty() { new = t; } }
            if new != ops::display_name(&p) {
                if let Some(np) = ops::rename(&p, &new, d.transient_for().as_ref()) {
                    *cur.borrow_mut() = np.clone();
                    name_e.set_text(&ops::display_name(&np));
                    d.set_title(&format!("{} Properties", ops::display_name(&np)));
                }
            }
            apply.set_sensitive(false);
        }
        if resp != gtk::ResponseType::Apply { d.close(); }
    });
    d.show_all();
}

fn user_name(uid: u32) -> String {
    std::fs::read_to_string("/etc/passwd").ok().and_then(|s| s.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2].parse::<u32>().ok() == Some(uid)).then(|| f[0].to_string())
    })).unwrap_or_else(|| uid.to_string())
}

fn group_name(gid: u32) -> String {
    std::fs::read_to_string("/etc/group").ok().and_then(|s| s.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() > 2 && f[2].parse::<u32>().ok() == Some(gid)).then(|| f[0].to_string())
    })).unwrap_or_else(|| gid.to_string())
}
