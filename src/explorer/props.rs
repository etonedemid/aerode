//! Windows 7 style file/folder Properties dialog.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

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
    l.set_max_width_chars(36);
    l.upcast()
}

pub fn show(path: &Path, parent: Option<&gtk::Window>) {
    let path = path.to_path_buf();
    let Ok(meta) = std::fs::symlink_metadata(&path) else { util::error("Cannot read properties", &path.display().to_string()); return };
    let gf = gio::File::for_path(&path);
    let info = gf.query_info("standard::*", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let d = gtk::Dialog::new();
    d.set_title(&format!("{name} Properties"));
    if let Some(p) = parent { d.set_transient_for(Some(p)); }
    d.set_default_size(380, -1);
    let nb = gtk::Notebook::new();
    nb.set_border_width(6);
    d.content_area().pack_start(&nb, true, true, 0);

    // ---- General
    let g = gtk::Grid::new();
    g.set_border_width(12);
    g.set_row_spacing(7);
    g.set_column_spacing(18);
    let icon = info.as_ref().and_then(|i| i.icon());
    g.attach(&gtk::Image::from_pixbuf(util::gicon_pixbuf(icon.as_ref(), 32).as_ref()), 0, 0, 1, 1);
    let name_e = gtk::Entry::new();
    name_e.set_text(&name);
    name_e.set_hexpand(true);
    g.attach(&name_e, 1, 0, 1, 1);
    g.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, 1, 2, 1);
    let ctype = info.as_ref().and_then(|i| i.content_type()).map(|c| c.to_string()).unwrap_or_default();
    let desc = gio::content_type_get_description(&ctype).to_string();
    let mut r = 2;
    if meta.is_dir() {
        row(&g, r, "Type:", &val("File folder")); r += 1;
    } else {
        let ext = path.extension().map(|e| format!(" (.{})", e.to_string_lossy())).unwrap_or_default();
        row(&g, r, "Type of file:", &val(&format!("{desc}{ext}"))); r += 1;
        let app = gio::AppInfo::default_for_type(&ctype, false);
        let ob = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if let Some(a) = &app { ob.pack_start(&util::app_image(a, 16), false, false, 0); }
        ob.pack_start(&gtk::Label::new(Some(&app.as_ref().map(|a| a.display_name().to_string()).unwrap_or_else(|| "Unknown application".into()))), false, false, 0);
        let change = gtk::Button::with_label("Change...");
        { let (ct, p) = (ctype.clone(), path.clone()); change.connect_clicked(move |b| {
            let w = b.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok());
            let dlg = gtk::AppChooserDialog::new(w.as_ref(), gtk::DialogFlags::MODAL, &gio::File::for_path(&p));
            if dlg.run() == gtk::ResponseType::Ok { if let Some(a) = dlg.app_info() { let _ = a.set_as_default_for_type(&ct); } }
            dlg.close();
        }); }
        ob.pack_start(&change, false, false, 12);
        row(&g, r, "Opens with:", ob.upcast_ref()); r += 1;
    }
    g.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, r, 2, 1); r += 1;
    row(&g, r, "Location:", &val(&path.parent().map(|p| p.display().to_string()).unwrap_or_default())); r += 1;
    let size_l = gtk::Label::new(Some(if meta.is_dir() { "Calculating..." } else { "" }));
    size_l.set_xalign(0.0);
    let disk_l = gtk::Label::new(None);
    disk_l.set_xalign(0.0);
    row(&g, r, "Size:", size_l.upcast_ref()); r += 1;
    row(&g, r, "Size on disk:", disk_l.upcast_ref()); r += 1;
    let contains = gtk::Label::new(None);
    contains.set_xalign(0.0);
    if meta.is_dir() { row(&g, r, "Contains:", contains.upcast_ref()); r += 1; }
    if meta.is_dir() {
        // count in a thread, like Win7 updates live
        let (tx, rx) = std::sync::mpsc::channel();
        let p2 = path.clone();
        std::thread::spawn(move || {
            fn walk(p: &Path, acc: &mut (u64, u64, u64, u64)) {
                if let Ok(rd) = std::fs::read_dir(p) {
                    for e in rd.flatten() {
                        if let Ok(m) = e.metadata() {
                            if m.is_dir() { acc.3 += 1; walk(&e.path(), acc); } else { acc.0 += m.len(); acc.1 += m.blocks() * 512; acc.2 += 1; }
                        }
                    }
                }
            }
            let mut acc = (0, 0, 0, 0);
            walk(&p2, &mut acc);
            let _ = tx.send(acc);
        });
        let (s, dl, c) = (size_l.clone(), disk_l.clone(), contains.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || match rx.try_recv() {
            Ok((b, disk, files, dirs)) => { s.set_text(&bytes_text(b)); dl.set_text(&bytes_text(disk)); c.set_text(&format!("{files} Files, {dirs} Folders")); glib::ControlFlow::Break }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => glib::ControlFlow::Break,
        });
    } else {
        size_l.set_text(&bytes_text(meta.len()));
        disk_l.set_text(&bytes_text(meta.blocks() * 512));
    }
    g.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, r, 2, 1); r += 1;
    row(&g, r, "Created:", &val(&fmt_time(meta.created().ok()))); r += 1;
    row(&g, r, "Modified:", &val(&fmt_time(meta.modified().ok()))); r += 1;
    row(&g, r, "Accessed:", &val(&fmt_time(meta.accessed().ok()))); r += 1;
    g.attach(&gtk::Separator::new(gtk::Orientation::Horizontal), 0, r, 2, 1); r += 1;
    let attrs = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let ro = gtk::CheckButton::with_label("Read-only");
    ro.set_active(meta.permissions().mode() & 0o200 == 0);
    let hidden = gtk::CheckButton::with_label("Hidden");
    hidden.set_active(name.starts_with('.'));
    attrs.pack_start(&ro, false, false, 0);
    attrs.pack_start(&hidden, false, false, 0);
    row(&g, r, "Attributes:", attrs.upcast_ref());
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));

    // ---- Security (Unix permissions presented like the Win7 tab)
    let sec = gtk::Grid::new();
    sec.set_border_width(12);
    sec.set_row_spacing(6);
    sec.set_column_spacing(12);
    let owner = util::output(&["stat", "-c", "%U (%G)", &path.to_string_lossy()]).unwrap_or_default();
    let ol = gtk::Label::new(Some(&format!("Owner: {}", owner.trim())));
    ol.set_xalign(0.0);
    sec.attach(&ol, 0, 0, 4, 1);
    let hdr = ["Permissions for", "Read", "Write", "Execute"];
    for (i, h) in hdr.iter().enumerate() { let l = gtk::Label::new(Some(h)); l.set_xalign(0.0); sec.attach(&l, i as i32, 1, 1, 1); }
    let mode = meta.permissions().mode();
    let mut checks = vec![];
    for (ri, (who, shift)) in [("Owner", 6), ("Group", 3), ("Everyone", 0)].iter().enumerate() {
        let l = gtk::Label::new(Some(who));
        l.set_xalign(0.0);
        sec.attach(&l, 0, ri as i32 + 2, 1, 1);
        for (ci, bit) in [4u32, 2, 1].iter().enumerate() {
            let c = gtk::CheckButton::new();
            c.set_active(mode & (bit << shift) != 0);
            sec.attach(&c, ci as i32 + 1, ri as i32 + 2, 1, 1);
            checks.push((c, bit << shift));
        }
    }
    nb.append_page(&sec, Some(&gtk::Label::new(Some("Security"))));

    // ---- Details (images: dimensions)
    if ctype.starts_with("image/") {
        let dg = gtk::Grid::new();
        dg.set_border_width(12);
        dg.set_row_spacing(6);
        dg.set_column_spacing(18);
        if let Some((_, w, h)) = gtk::gdk_pixbuf::Pixbuf::file_info(&path) {
            row(&dg, 0, "Dimensions", &val(&format!("{w} x {h}")));
            row(&dg, 1, "Width", &val(&format!("{w} pixels")));
            row(&dg, 2, "Height", &val(&format!("{h} pixels")));
        }
        nb.append_page(&dg, Some(&gtk::Label::new(Some("Details"))));
    }

    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let apply = d.add_button("Apply", gtk::ResponseType::Apply);
    apply.set_sensitive(true);
    let path2 = path.clone();
    d.connect_response(move |d, resp| {
        if resp == gtk::ResponseType::Ok || resp == gtk::ResponseType::Apply {
            let mut cur = path2.clone();
            // permissions
            let mut m = std::fs::metadata(&cur).map(|m| m.permissions().mode()).unwrap_or(0) & !0o777;
            for (c, bit) in &checks { if c.is_active() { m |= bit; } }
            if ro.is_active() { m &= !0o222; }
            let _ = std::fs::set_permissions(&cur, std::fs::Permissions::from_mode(m));
            // rename / hide
            let mut new = name_e.text().to_string();
            if hidden.is_active() && !new.starts_with('.') { new = format!(".{new}"); }
            if !hidden.is_active() { new = new.trim_start_matches('.').to_string(); if new.is_empty() { new = name.clone(); } }
            if new != name {
                let target: PathBuf = cur.with_file_name(&new);
                if std::fs::rename(&cur, &target).is_ok() { cur = target; }
            }
            let _ = cur;
        }
        if resp != gtk::ResponseType::Apply { d.close(); }
    });
    d.show_all();
}
