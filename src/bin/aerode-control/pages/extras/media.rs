//! Aerode Media: paired Media Center Extenders (Xbox 360), the library folders, the theme and
//! the server. Everything is in ~/.config/aerode/media (config.json is the one aerode-media
//! reads; <device id>.cer per paired extender), the server is `aerode-media run`.
use gtk::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;

use crate::{section, title, Cp};
use super::common::*;

const PRESETS: [&str; 5] = ["aero", "midnight", "emerald", "sunset", "graphite"];
const LIBS: [(&str, &str); 3] = [("pictures", "Pictures"), ("videos", "Videos"), ("music", "Music")];

fn dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("aerode/media")
}

fn autostart() -> PathBuf {
    dir().parent().unwrap().parent().unwrap().join("autostart/aerode-media.desktop")
}

fn load() -> serde_json::Value {
    let mut v: serde_json::Value = std::fs::read(dir().join("config.json")).ok()
        .and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| serde_json::json!({}));
    let home = std::env::var("HOME").unwrap_or_default();
    for (key, folder) in LIBS {
        if v.get(key).is_none() { v[key] = serde_json::json!([format!("{home}/{folder}")]); }
    }
    if v.get("theme").is_none() { v["theme"] = "aero".into(); }
    v
}

/// Theme names: the built-in ones and any themes/<name>.json.
fn themes() -> Vec<String> {
    let mut v: Vec<String> = PRESETS.iter().map(|s| s.to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(dir().join("themes")) {
        let mut extra: Vec<String> = rd.flatten()
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .filter_map(|j| j["name"].as_str().map(str::to_string))
            .collect();
        extra.sort();
        v.extend(extra);
    }
    v
}

/// Device ids of the paired extenders (their certificates).
fn extenders() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir()).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".cer")).map(str::to_string))
        .filter(|n| n != "host")
        .collect();
    v.sort();
    v
}

fn running() -> bool { exec(&sv(&["pgrep", "-x", "aerode-media"]), None).ok }

/// A folder list with Add/Remove, like Windows' "Include folder in library" dialog.
fn folders(paths: &[String]) -> (gtk::Box, gtk::ListStore) {
    let (tv, store) = table(1, &["Folder"]);
    tv.set_headers_visible(false);
    for p in paths { store.insert_with_values(None, &[(0, p)]); }
    let h = hbox(6);
    h.pack_start(&scrolled(&tv, 64), true, true, 0);
    let b = vbox(6);
    let add = gtk::Button::with_label("Add...");
    let rm = gtk::Button::with_label("Remove");
    for x in [&add, &rm] { x.set_size_request(75, -1); b.pack_start(x, false, false, 0); }
    h.pack_start(&b, false, false, 0);
    { let store = store.clone(); add.connect_clicked(move |btn| {
        let parent = btn.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok());
        let d = gtk::FileChooserDialog::with_buttons(Some("Include Folder"), parent.as_ref(), gtk::FileChooserAction::SelectFolder,
            &[("Cancel", gtk::ResponseType::Cancel), ("Include folder", gtk::ResponseType::Accept)]);
        if d.run() == gtk::ResponseType::Accept {
            if let Some(p) = d.filename() { store.insert_with_values(None, &[(0, &p.to_string_lossy().to_string())]); }
        }
        d.close();
    }); }
    { let (store, tv) = (store.clone(), tv.clone()); rm.connect_clicked(move |_| {
        if let Some((_, it)) = tv.selection().selected() { store.remove(&it); }
    }); }
    (h, store)
}

fn rows(store: &gtk::ListStore) -> Vec<String> {
    let mut v = Vec::new();
    store.foreach(|m, _, it| { if let Ok(s) = m.value(it, 0).get::<String>() { v.push(s); } false });
    v
}

pub fn page_media(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    let cfg = load();
    let v = vbox(6);
    v.pack_start(&title("Set up Aerode Media"), false, false, 0);
    if !aerode::util::which("aerode-media") {
        v.pack_start(&instr("Aerode Media is not installed. It is optional: run ./install.sh --media in the Aerode folder, or get it from github.com/etonedemid/aerode-media."), false, false, 0);
        return v.upcast();
    }
    v.pack_start(&instr("Aerode Media serves a Media Center to Xbox 360 consoles on your network. Open Windows Media Center on the Xbox to connect."), false, false, 0);

    // ------------------------------------------------ extenders
    v.pack_start(&section("Media Center Extenders"), false, false, 4);
    let (tv, ext) = table(3, &["Extender", "Address", "Status"]);
    for id in extenders() {
        ext.insert_with_values(None, &[(0, &format!("Xbox 360 ({id})")), (1, &String::new()), (2, &"Paired".to_string())]);
    }
    let ebar = hbox(6);
    ebar.pack_start(&scrolled(&tv, 72), true, true, 0);
    let eb = vbox(6);
    let add = gtk::Button::with_label("Add an Extender...");
    let rm = gtk::Button::with_label("Remove");
    let search = gtk::Button::with_label("Search");
    for x in [&add, &rm, &search] { eb.pack_start(x, false, false, 0); }
    ebar.pack_start(&eb, false, false, 0);
    v.pack_start(&ebar, false, false, 0);
    {
        // Find which paired extenders are on the network (SSDP, a few seconds).
        let ext = ext.clone();
        let find = move |b: &gtk::Button| {
            b.set_sensitive(false);
            let (ext, b) = (ext.clone(), b.clone());
            exec_async(sv(&["aerode-media", "discover"]), None, move |o| {
                b.set_sensitive(true);
                ext.foreach(|m, _, it| {
                    let name: String = m.value(it, 0).get().unwrap_or_default();
                    let id = name.trim_start_matches("Xbox 360 (").trim_end_matches(')').to_lowercase();
                    let hit = o.stdout.lines().find(|l| l.to_lowercase().contains(&id));
                    let s = m.clone().downcast::<gtk::ListStore>().unwrap();
                    match hit {
                        Some(l) => {
                            let mut f = l.split("  ");
                            let ip = f.next().unwrap_or("").to_string();
                            let nm = f.next().unwrap_or("Xbox 360").to_string();
                            s.set(it, &[(0, &format!("{nm} ({id})")), (1, &ip), (2, &"Online".to_string())]);
                        }
                        None => s.set(it, &[(1, &String::new()), (2, &"Offline".to_string())]),
                    }
                    false
                });
            });
        };
        find(&search);
        search.connect_clicked(find);
    }
    { let (ext, tv) = (ext.clone(), tv.clone()); rm.connect_clicked(move |b| {
        let Some((_, it)) = tv.selection().selected() else { return };
        let name: String = ext.value(&it, 0).get().unwrap_or_default();
        let id = name.rsplit('(').next().unwrap_or("").trim_end_matches(')').to_string();
        let parent = b.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok());
        if ask(parent.as_ref(), gtk::MessageType::Question, "Aerode Media", "Remove this extender?",
            "It will have to be set up again with its setup key to connect.", "Remove", "Cancel") {
            let _ = std::fs::remove_file(dir().join(format!("{id}.cer")));
            ext.remove(&it);
        }
    }); }
    { let ext = ext.clone(); add.connect_clicked(move |b| {
        let parent = b.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok());
        let d = gtk::Dialog::with_buttons(Some("Add an Extender"), parent.as_ref(), gtk::DialogFlags::MODAL,
            &[("Next", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
        let c = d.content_area();
        c.set_spacing(8);
        c.set_border_width(12);
        c.pack_start(&instr("Enter the setup key shown on your Xbox"), false, false, 0);
        c.pack_start(&label("On the Xbox 360, open Windows Media Center and follow the steps until it shows an 8-digit setup key."), false, false, 0);
        let e = gtk::Entry::new();
        e.set_max_length(9);
        e.set_placeholder_text(Some("1234-5678"));
        e.set_activates_default(true);
        d.set_default_response(gtk::ResponseType::Ok);
        c.pack_start(&e, false, false, 0);
        d.show_all();
        let r = d.run();
        let key: String = e.text().chars().filter(|c| c.is_ascii_digit()).collect();
        d.close();
        if r != gtk::ResponseType::Ok { return; }
        if key.len() != 8 {
            message(parent.as_ref(), gtk::MessageType::Error, "Add an Extender", "The setup key has 8 digits.", "");
            return;
        }
        b.set_sensitive(false);
        let (b, ext) = (b.clone(), ext.clone());
        exec_async(vec!["aerode-media".into(), "pair".into(), key], None, move |o| {
            b.set_sensitive(true);
            let parent = b.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok());
            if o.ok {
                ext.clear();
                for id in extenders() { ext.insert_with_values(None, &[(0, &format!("Xbox 360 ({id})")), (1, &String::new()), (2, &"Paired".to_string())]); }
                message(parent.as_ref(), gtk::MessageType::Info, "Add an Extender", "The extender was set up.",
                    "Start Aerode Media below, then continue on the Xbox.");
            } else {
                message(parent.as_ref(), gtk::MessageType::Error, "Add an Extender", "The extender could not be set up.", &o.text());
            }
        });
    }); }

    // ------------------------------------------------ libraries
    v.pack_start(&section("Libraries"), false, false, 4);
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    g.set_margin_start(16);
    let mut libs = Vec::new();
    for (i, (key, name)) in LIBS.iter().enumerate() {
        let l = gtk::Label::new(Some(name));
        l.set_xalign(0.0);
        l.set_valign(gtk::Align::Start);
        l.set_width_chars(10);
        g.attach(&l, 0, i as i32, 1, 1);
        let paths: Vec<String> = cfg[*key].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
        let (w, store) = folders(&paths);
        w.set_hexpand(true);
        g.attach(&w, 1, i as i32, 1, 1);
        libs.push((*key, store));
    }
    v.pack_start(&g, false, false, 0);

    // ------------------------------------------------ appearance and server
    v.pack_start(&section("Appearance"), false, false, 4);
    let th = gtk::ComboBoxText::new();
    for t in themes() { th.append(Some(&t), &t); }
    if !th.set_active_id(cfg["theme"].as_str()) { th.set_active(Some(0)); }
    let row = hbox(10);
    row.set_margin_start(16);
    row.pack_start(&label("Theme:"), false, false, 0);
    row.pack_start(&th, false, false, 0);
    v.pack_start(&row, false, false, 0);

    v.pack_start(&section("Server"), false, false, 4);
    let srv = hbox(10);
    srv.set_margin_start(16);
    let state = label("");
    let toggle = gtk::Button::new();
    let show = { let (state, toggle) = (state.clone(), toggle.clone()); move |on: bool| {
        state.set_text(if on { "Aerode Media is running." } else { "Aerode Media is not running." });
        toggle.set_label(if on { "Stop" } else { "Start" });
    } };
    show(running());
    toggle.connect_clicked({ let show = show.clone(); move |_| {
        if running() {
            // SIGINT: it says goodbye to the Xbox instead of leaving it on a frozen frame
            let _ = exec(&sv(&["pkill", "-INT", "-x", "aerode-media"]), None);
            std::thread::sleep(std::time::Duration::from_millis(800));
        } else {
            let log = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".cache/aerode/aerode-media.log");
            let _ = std::fs::create_dir_all(log.parent().unwrap());
            let out = std::fs::File::create(&log).ok();
            let mut c = std::process::Command::new("setsid");
            c.args(["-f", "aerode-media", "run"]).stdin(std::process::Stdio::null());
            if let Some(f) = out { c.stderr(f.try_clone().unwrap()).stdout(f); }
            let _ = c.status();
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        show(running());
    } });
    srv.pack_start(&state, false, false, 0);
    srv.pack_start(&toggle, false, false, 0);
    v.pack_start(&srv, false, false, 0);
    let auto = gtk::CheckButton::with_mnemonic("Start Aerode Media when I _log on");
    auto.set_margin_start(16);
    auto.set_active(autostart().exists());
    v.pack_start(&auto, false, false, 0);

    // ------------------------------------------------ save / cancel
    let bar = hbox(6);
    bar.set_margin_top(16);
    let save = gtk::Button::with_mnemonic("_Save changes");
    let cancel = gtk::Button::with_label("Cancel");
    let back = { let me = cp.clone(); move || {
        let prev = { let mut h = me.history.borrow_mut(); h.pop(); h.pop() };
        me.open(&prev.unwrap_or_else(|| "cat:hardware".into()));
    } };
    { let back = back.clone(); save.connect_clicked(move |b| {
        let mut cfg = load();
        for (key, store) in &libs { cfg[*key] = serde_json::json!(rows(store)); }
        if let Some(t) = th.active_id() { cfg["theme"] = t.as_str().into(); }
        let _ = std::fs::create_dir_all(dir());
        let r = std::fs::write(dir().join("config.json"), serde_json::to_vec_pretty(&cfg).unwrap()).and_then(|_| {
            let a = autostart();
            if auto.is_active() {
                std::fs::create_dir_all(a.parent().unwrap())?;
                std::fs::write(&a, "[Desktop Entry]\nType=Application\nName=Aerode Media\nExec=aerode-media run\nNoDisplay=true\n")
            } else if a.exists() {
                std::fs::remove_file(&a)
            } else {
                Ok(())
            }
        });
        if let Err(e) = r {
            let parent = b.toplevel().and_then(|w| w.downcast::<gtk::Window>().ok());
            message(parent.as_ref(), gtk::MessageType::Error, "Aerode Media", "The settings could not be saved.", &e.to_string());
            return;
        }
        back();
    }); }
    cancel.connect_clicked(move |_| back());
    bar.pack_end(&button_row(&[&save, &cancel]), false, false, 0);
    v.pack_start(&bar, false, false, 0);
    v.upcast()
}
