//! Color Management: the Windows 7 "Color Management" dialog on top of colord (the system color
//! management service, over D-Bus): devices and their profiles, the default profile, adding
//! profiles (colormgr import-profile), all installed profiles, and display calibration.
use gtk::prelude::*;
use gtk::glib;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use aerode::util;
use crate::Cp;
use super::common::*;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const CD: &str = "org.freedesktop.ColorManager";

#[derive(Clone, Debug, Default)]
struct Profile { path: String, id: String, title: String, filename: String, kind: String, colorspace: String, created: i64 }

#[derive(Clone, Debug, Default)]
struct Device { path: String, id: String, kind: String, vendor: String, model: String, output: String, profiles: Vec<String> }

impl Device {
    fn label(&self, i: usize) -> String {
        let kind = match self.kind.as_str() { "display" => "Display", "printer" => "Printer", "scanner" => "Scanner", "camera" => "Camera", "webcam" => "Webcam", k => k };
        let name = format!("{} {}", self.vendor, self.model).trim().to_string();
        let name = if name.is_empty() { self.id.clone() } else { name };
        if self.output.is_empty() { format!("{}. {kind}: {name}", i + 1) } else { format!("{}. {kind}: {name} - {}", i + 1, self.output) }
    }
}

fn take<T: TryFrom<OwnedValue>>(m: &mut HashMap<String, OwnedValue>, k: &str) -> Option<T> { m.remove(k).and_then(|v| T::try_from(v).ok()) }

fn get_all(conn: &zbus::blocking::Connection, path: &str, iface: &str) -> zbus::Result<HashMap<String, OwnedValue>> {
    let p = zbus::blocking::Proxy::new(conn, CD, path.to_string(), "org.freedesktop.DBus.Properties")?;
    p.call("GetAll", &(iface,))
}

/// Everything colord knows (None: colord is not available).
fn load() -> Option<(Vec<Device>, Vec<Profile>)> {
    let conn = zbus::blocking::Connection::system().ok()?;
    let m = zbus::blocking::Proxy::new(&conn, CD, "/org/freedesktop/ColorManager", CD).ok()?;
    let devs: Vec<OwnedObjectPath> = m.call("GetDevices", &()).ok()?;
    let profs: Vec<OwnedObjectPath> = m.call("GetProfiles", &()).unwrap_or_default();
    let mut devices = vec![];
    for d in devs {
        let Ok(mut p) = get_all(&conn, d.as_str(), "org.freedesktop.ColorManager.Device") else { continue };
        let meta: HashMap<String, String> = take(&mut p, "Metadata").unwrap_or_default();
        let profiles: Vec<OwnedObjectPath> = take(&mut p, "Profiles").unwrap_or_default();
        devices.push(Device {
            path: d.to_string(), id: take(&mut p, "DeviceId").unwrap_or_default(), kind: take(&mut p, "Kind").unwrap_or_default(),
            vendor: take(&mut p, "Vendor").unwrap_or_default(), model: take(&mut p, "Model").unwrap_or_default(),
            output: meta.get("XRANDR_name").cloned().unwrap_or_default(), profiles: profiles.iter().map(|x| x.to_string()).collect(),
        });
    }
    let mut profiles = vec![];
    for pr in profs {
        let Ok(mut p) = get_all(&conn, pr.as_str(), "org.freedesktop.ColorManager.Profile") else { continue };
        profiles.push(Profile {
            path: pr.to_string(), id: take(&mut p, "ProfileId").unwrap_or_default(), title: take(&mut p, "Title").unwrap_or_default(),
            filename: take(&mut p, "Filename").unwrap_or_default(), kind: take(&mut p, "Kind").unwrap_or_default(),
            colorspace: take(&mut p, "Colorspace").unwrap_or_default(), created: take::<i64>(&mut p, "Created").unwrap_or(0),
        });
    }
    devices.sort_by(|a, b| (a.kind != "display").cmp(&(b.kind != "display")).then(a.id.cmp(&b.id)));
    Some((devices, profiles))
}

/// A colord Device method (AddProfile, RemoveProfile, MakeProfileDefault).
fn device_call(dev: String, method: &'static str, profile: String, done: impl FnOnce(Result<(), String>) + 'static) {
    background(move || -> Result<(), String> {
        let conn = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
        let p = zbus::blocking::Proxy::new(&conn, CD, dev, "org.freedesktop.ColorManager.Device").map_err(|e| e.to_string())?;
        let path = zbus::zvariant::ObjectPath::try_from(profile.as_str()).map_err(|e| e.to_string())?;
        let r = if method == "AddProfile" { p.call_method(method, &("hard", path)) } else { p.call_method(method, &(path,)) };
        r.map(|_| ()).map_err(|e| e.to_string())
    }, done);
}

fn kind_group(k: &str) -> &'static str {
    match k {
        "display-device" => "Display profiles",
        "output-device" => "Printer profiles",
        "input-device" => "Scanner and camera profiles",
        "colorspace-conversion" => "Color space profiles",
        "abstract" => "Abstract profiles",
        "named-color" => "Named color profiles",
        _ => "Other profiles",
    }
}

fn file_name(p: &str) -> String { Path::new(p).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() }

fn title_of(p: &Profile) -> String { if p.title.is_empty() { file_name(&p.filename) } else { p.title.clone() } }

pub fn page_colormgmt(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    standalone(cp, |parent| {
        let w = gtk::Window::new(gtk::WindowType::Toplevel);
        w.set_title("Color Management");
        w.set_icon_name(Some(util::first_icon(&["preferences-color", "preferences-desktop-display"])));
        w.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
        w.set_default_size(580, 540);
        if let Some(p) = parent { w.set_transient_for(Some(p)); w.set_position(gtk::WindowPosition::CenterOnParent); } else { w.set_position(gtk::WindowPosition::Center); }
        let root = vbox(8);
        root.set_border_width(8);
        let body = vbox(0);
        body.pack_start(&loading("Reading color profiles…"), false, false, 12);
        root.pack_start(&body, true, true, 0);
        let close = gtk::Button::with_mnemonic("_Close");
        { let w = w.clone(); close.connect_clicked(move |_| w.close()); }
        root.pack_start(&button_row(&[&close]), false, false, 0);
        w.add(&root);
        reload(&w, &body, 0);
        w
    })
}

fn reload(w: &gtk::Window, body: &gtk::Box, tab: u32) {
    let (w, body) = (w.clone(), body.clone());
    background(load, move |r| {
        let page = match r {
            None => unavailable(),
            Some((devs, profs)) => {
                let nb = gtk::Notebook::new();
                nb.append_page(&devices_tab(&w, &body, &devs, &profs), Some(&gtk::Label::new(Some("Devices"))));
                nb.append_page(&profiles_tab(&w, &body, &profs), Some(&gtk::Label::new(Some("All Profiles"))));
                nb.append_page(&advanced_tab(), Some(&gtk::Label::new(Some("Advanced"))));
                nb.show_all();
                nb.set_current_page(Some(tab));
                nb.upcast()
            }
        };
        fill(&body, &page);
    });
}

fn unavailable() -> gtk::Widget {
    let v = vbox(10);
    v.set_border_width(16);
    let h = hbox(12);
    h.pack_start(&util::image("dialog-information", 32), false, false, 0);
    let l = label("Color Management is not available because the color management service (colord) is not installed or not running.");
    l.style_context().add_class("xt-name");
    h.pack_start(&l, true, true, 0);
    v.pack_start(&h, false, false, 0);
    v.pack_start(&label("Windows uses this service to keep track of the color profiles of your displays, printers, scanners and cameras, so that colors look the same on all of them."), false, false, 0);
    v.pack_start(&label("Install the colord package, and xiccd to load display profiles when you log on, then open Color Management again:"), false, false, 0);
    v.pack_start(&crate::kv_grid(&[("Packages:", "colord, xiccd".into()), ("Command:", "sudo pacman -S colord xiccd".into())]), false, false, 0);
    v.upcast()
}

fn devices_tab(w: &gtk::Window, body: &gtk::Box, devs: &[Device], profs: &[Profile]) -> gtk::Widget {
    let v = vbox(8);
    v.set_border_width(10);
    let row = hbox(8);
    row.pack_start(&gtk::Label::new(Some("Device:")), false, false, 0);
    let combo = gtk::ComboBoxText::new();
    for (i, d) in devs.iter().enumerate() { combo.append(Some(&d.path), &d.label(i)); }
    row.pack_start(&combo, true, true, 0);
    let ident = gtk::Button::with_mnemonic("_Identify monitors");
    ident.connect_clicked(|_| identify());
    row.pack_end(&ident, false, false, 0);
    v.pack_start(&row, false, false, 0);
    if devs.is_empty() { v.pack_start(&grey("No devices are registered with color management. Displays appear here when xiccd or another color daemon is running."), false, false, 0); }
    v.pack_start(&label("Profiles associated with this device:"), false, false, 4);
    let (tv, store) = table(3, &["Name", "File name"]);
    if let Some(c) = tv.column(1) { c.set_min_width(170); }
    v.pack_start(&scrolled(&tv, 200), true, true, 0);
    let bb = hbox(6);
    let add = gtk::Button::with_mnemonic("_Add...");
    let remove = gtk::Button::with_mnemonic("_Remove");
    let def = gtk::Button::with_mnemonic("_Set as Default Profile");
    bb.pack_start(&add, false, false, 0);
    bb.pack_start(&remove, false, false, 0);
    bb.pack_end(&def, false, false, 0);
    v.pack_start(&bb, false, false, 0);
    let fill_list = {
        let (store, devs, profs) = (store.clone(), devs.to_vec(), profs.to_vec());
        move |path: &str| {
            store.clear();
            let Some(d) = devs.iter().find(|d| d.path == path) else { return };
            for (i, pp) in d.profiles.iter().enumerate() {
                let Some(p) = profs.iter().find(|p| p.path == *pp) else { continue };
                let name = if i == 0 { format!("{} (default)", title_of(p)) } else { title_of(p) };
                store.insert_with_values(None, &[(0, &name), (1, &file_name(&p.filename)), (2, &p.path)]);
            }
        }
    };
    { let f = fill_list.clone(); combo.connect_changed(move |c| if let Some(id) = c.active_id() { f(&id); }); }
    combo.set_active(Some(0));
    {
        let (w, body, combo, profs) = (w.clone(), body.clone(), combo.clone(), profs.to_vec());
        add.connect_clicked(move |_| {
            let Some(dev) = combo.active_id().map(|s| s.to_string()) else { return };
            let (w, body) = (w.clone(), body.clone());
            let attach = move |p: String, w: gtk::Window, body: gtk::Box| device_call(dev, "AddProfile", p, move |r| { report(&w, r); reload(&w, &body, 0); });
            match choose_profile(&w, &profs) {
                Some(Pick::Installed(p)) => attach(p, w, body),
                Some(Pick::File(f)) => background(move || import(&f), move |(o, path)| match (o.ok, path) {
                    (true, Some(p)) => attach(p, w, body),
                    (true, None) => reload(&w, &body, 0),
                    _ => message(Some(&w), gtk::MessageType::Error, "Color Management", "The profile could not be installed.", &o.text()),
                }),
                None => {}
            }
        });
    }
    for (b, method) in [(&remove, "RemoveProfile"), (&def, "MakeProfileDefault")] {
        let (w, body, combo, tv) = (w.clone(), body.clone(), combo.clone(), tv.clone());
        b.connect_clicked(move |_| {
            let (Some(dev), Some(p)) = (combo.active_id().map(|s| s.to_string()), selected(&tv, 2)) else { return };
            let (w, body) = (w.clone(), body.clone());
            device_call(dev, method, p, move |r| { report(&w, r); reload(&w, &body, 0); });
        });
    }
    v.pack_start(&grey("The default profile is the one programs and the display use. Changes apply to everyone who uses this computer."), false, false, 0);
    v.upcast()
}

fn report(w: &gtk::Window, r: Result<(), String>) {
    if let Err(e) = r { message(Some(w), gtk::MessageType::Error, "Color Management", "The change could not be made.", &e); }
}

enum Pick { Installed(String), File(std::path::PathBuf) }

/// "Associate Color Profile": pick an installed profile, or browse for a file to install.
fn choose_profile(parent: &gtk::Window, profs: &[Profile]) -> Option<Pick> {
    let d = gtk::Dialog::with_buttons(Some("Associate Color Profile"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(480, 380);
    d.add_button("_Browse...", gtk::ResponseType::Other(1));
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(6);
    c.pack_start(&label("Select a color profile to associate with this device:"), false, false, 0);
    let (tv, store) = table(3, &["Name", "File name"]);
    if let Some(c) = tv.column(1) { c.set_min_width(170); }
    for p in profs { store.insert_with_values(None, &[(0, &title_of(p)), (1, &file_name(&p.filename)), (2, &p.path)]); }
    c.pack_start(&scrolled(&tv, 260), true, true, 0);
    d.show_all();
    let out = loop {
        match d.run() {
            gtk::ResponseType::Other(1) => if let Some(f) = pick_icc(&d.clone().upcast()) { break Some(Pick::File(f)); },
            gtk::ResponseType::Ok => break selected(&tv, 2).map(Pick::Installed),
            _ => break None,
        }
    };
    d.close();
    out
}

fn pick_icc(parent: &gtk::Window) -> Option<std::path::PathBuf> {
    let fc = gtk::FileChooserNative::new(Some("Install Profile"), Some(parent), gtk::FileChooserAction::Open, Some("Add"), Some("Cancel"));
    let f = gtk::FileFilter::new();
    f.set_name(Some("ICC Profiles (*.icm; *.icc)"));
    for p in ["*.icc", "*.icm", "*.ICC", "*.ICM"] { f.add_pattern(p); }
    fc.add_filter(f);
    if fc.run() == gtk::ResponseType::Accept { fc.filename() } else { None }
}

/// Install an ICC file for this user (colormgr copies it to ~/.local/share/icc, where colord
/// finds it); returns the new profile's object path when it can be found.
fn import(f: &Path) -> (Out, Option<String>) {
    let o = if util::which("colormgr") { exec(&sv(&["colormgr", "import-profile", &f.to_string_lossy()]), None) } else {
        let dir = gtk::glib::user_data_dir().join("icc");
        let _ = std::fs::create_dir_all(&dir);
        match std::fs::copy(f, dir.join(f.file_name().unwrap_or_default())) { Ok(_) => Out { ok: true, ..Default::default() }, Err(e) => Out { stderr: e.to_string(), ..Default::default() } }
    };
    std::thread::sleep(std::time::Duration::from_millis(400));
    let name = file_name(&f.to_string_lossy());
    let path = load().and_then(|(_, ps)| ps.into_iter().find(|p| file_name(&p.filename) == name).map(|p| p.path));
    (o, path)
}

fn profiles_tab(w: &gtk::Window, body: &gtk::Box, profs: &[Profile]) -> gtk::Widget {
    let v = vbox(8);
    v.set_border_width(10);
    // name, file, object path, filename (full)
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let mut groups: Vec<(&str, Vec<&Profile>)> = vec![];
    for p in profs {
        let g = kind_group(&p.kind);
        match groups.iter_mut().find(|(n, _)| *n == g) { Some(e) => e.1.push(p), None => groups.push((g, vec![p])) }
    }
    for (g, ps) in &groups {
        let it = store.insert_with_values(None, None, &[(0, &format!("{g} ({})", ps.len())), (1, &""), (2, &""), (3, &"")]);
        for p in ps { store.insert_with_values(Some(&it), None, &[(0, &title_of(p)), (1, &file_name(&p.filename)), (2, &p.path), (3, &p.filename)]); }
    }
    let tv = gtk::TreeView::with_model(&store);
    for (i, t) in ["Name", "File name"].iter().enumerate() {
        let c = gtk::TreeViewColumn::new();
        c.set_title(t);
        let r = gtk::CellRendererText::new();
        r.set_property("ellipsize", gtk::pango::EllipsizeMode::End);
        TreeViewColumnExt::pack_start(&c, &r, true);
        TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
        c.set_resizable(true);
        c.set_expand(i == 0);
        if i == 1 { c.set_min_width(170); }
        tv.append_column(&c);
    }
    tv.expand_all();
    v.pack_start(&scrolled(&tv, 250), true, true, 0);
    let info = vbox(0);
    v.pack_start(&info, false, false, 0);
    {
        let (info, profs) = (info.clone(), profs.to_vec());
        tv.selection().connect_changed(move |s| {
            let Some((m, it)) = s.selected() else { return };
            let path: String = m.value(&it, 2).get().unwrap_or_default();
            let Some(p) = profs.iter().find(|p| p.path == path) else { fill(&info, &vbox(0)); return };
            let created = unix_time(p.created).map(win_date).unwrap_or_default();
            fill(&info, &crate::kv_grid(&[("Color space:", p.colorspace.to_uppercase()), ("Created:", created), ("File:", p.filename.clone()), ("Profile ID:", p.id.clone())]));
        });
    }
    let bb = hbox(6);
    let add = gtk::Button::with_mnemonic("_Add...");
    let remove = gtk::Button::with_mnemonic("_Remove");
    bb.pack_start(&add, false, false, 0);
    bb.pack_start(&remove, false, false, 0);
    v.pack_start(&bb, false, false, 0);
    {
        let (w, body) = (w.clone(), body.clone());
        add.connect_clicked(move |_| {
            let Some(f) = pick_icc(&w) else { return };
            let (w, body) = (w.clone(), body.clone());
            background(move || import(&f).0, move |o| {
                if !o.ok { message(Some(&w), gtk::MessageType::Error, "Color Management", "The profile could not be installed.", &o.text()); }
                reload(&w, &body, 1);
            });
        });
    }
    {
        let (w, body, tv) = (w.clone(), body.clone(), tv.clone());
        remove.connect_clicked(move |_| {
            let Some(file) = selected(&tv, 3).filter(|f| !f.is_empty()) else { return };
            let user_dir = gtk::glib::user_data_dir().join("icc");
            if !Path::new(&file).starts_with(&user_dir) {
                message(Some(&w), gtk::MessageType::Info, "Color Management", "This profile is part of the system.", "Only profiles you installed yourself can be removed here.");
                return;
            }
            if !ask(Some(&w), gtk::MessageType::Warning, "Color Management", &format!("Are you sure you want to remove \"{}\"?", file_name(&file)), "Devices that use this profile go back to their default.", "_Yes", "_No") { return; }
            let _ = std::fs::remove_file(&file);
            let (w, body) = (w.clone(), body.clone());
            glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || reload(&w, &body, 1));
        });
    }
    v.upcast()
}

fn xiccd_autostart() -> std::path::PathBuf { gtk::glib::user_config_dir().join("autostart/xiccd.desktop") }

fn advanced_tab() -> gtk::Widget {
    let v = vbox(10);
    v.set_border_width(10);
    let dc = vbox(6);
    dc.pack_start(&label("Calibrate your display so that colors and brightness look right, and save the result as its color profile."), false, false, 0);
    let tool = ["displaycal", "DisplayCAL", "gcm-calibrate"].into_iter().find(|t| util::which(t));
    let cal = gtk::Button::with_mnemonic("_Calibrate display");
    cal.set_halign(gtk::Align::End);
    cal.set_sensitive(tool.is_some());
    if let Some(t) = tool { cal.connect_clicked(move |_| { util::spawn(&[t]); }); }
    dc.pack_start(&cal, false, false, 0);
    if tool.is_none() { dc.pack_start(&grey("Install DisplayCAL (displaycal) to calibrate your display with a colorimeter."), false, false, 0); }
    let use_cal = gtk::CheckButton::with_mnemonic("_Use Windows display calibration");
    let have = util::which("xiccd");
    use_cal.set_sensitive(have);
    use_cal.set_active(have && xiccd_autostart().exists());
    use_cal.connect_toggled(|c| {
        let f = xiccd_autostart();
        if c.is_active() {
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            let _ = std::fs::write(&f, "[Desktop Entry]\nType=Application\nName=Display color profiles (xiccd)\nExec=xiccd\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n");
            if !util::output(&["pgrep", "-x", "xiccd"]).is_some() { util::spawn(&["xiccd"]); }
        } else { let _ = std::fs::remove_file(&f); }
    });
    dc.pack_start(&use_cal, false, false, 0);
    dc.pack_start(&grey(if have { "Loads the default profile of each display (its calibration curves) when you log on." } else { "Install xiccd to load display profiles and calibration when you log on." }), false, false, 0);
    v.pack_start(&group("Display Calibration", &dc), false, false, 0);
    v.upcast()
}

/// "Identify monitors": a big number on every screen for a few seconds.
fn identify() {
    let Some(disp) = gtk::gdk::Display::default() else { return };
    for i in 0..disp.n_monitors() {
        let Some(m) = disp.monitor(i) else { continue };
        let g = m.geometry();
        let w = gtk::Window::new(gtk::WindowType::Popup);
        w.set_default_size(160, 160);
        let l = gtk::Label::new(None);
        l.set_markup(&format!("<span font='96' weight='bold' foreground='#ffffff'>{}</span>", i + 1));
        let bx = vbox(0);
        bx.pack_start(&l, true, true, 0);
        w.add(&bx);
        w.connect_draw(|_, cr| { cr.set_source_rgb(0.11, 0.24, 0.5); let _ = cr.paint(); glib::Propagation::Proceed });
        w.move_(g.x() + 40, g.y() + g.height() - 220);
        w.show_all();
        glib::timeout_add_seconds_local_once(3, move || w.close());
    }
}
