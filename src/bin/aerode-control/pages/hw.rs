//! Control Panel pages: System, Display, Sound, Date and Time, Power Options, Mouse, Keyboard.
use gtk::prelude::*;
#[allow(unused_imports)]
use gtk::{gdk, gio, glib};
#[allow(unused_imports)]
use std::cell::RefCell;
use std::rc::Rc;
#[allow(unused_imports)]
use aerode::{config, sounds, theme, util};
#[allow(unused_imports)]
use super::super::*;

impl Cp {
    pub(crate) fn page_system(self: &Rc<Self>) -> gtk::Widget {
        self.side_link("Device Manager", "devices");
        self.side_link("Startup programs", "startup");
        self.side_link("Services", "services");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("View basic information about your computer"), false, false, 0);
        let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let get = |k: &str| os.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
        v.pack_start(&section("Edition"), false, false, 0);
        v.pack_start(&kv_grid(&[(&get("PRETTY_NAME"), String::new()), ("Kernel", out(&["uname", "-r"]).trim().to_string()), ("Desktop", "Aerode".into())]), false, false, 0);
        let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))).unwrap_or_default();
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next()?.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0);
        v.pack_start(&section("System"), false, false, 0);
        v.pack_start(&kv_grid(&[
            ("Processor:", format!("{cpu}  ({} cores)", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))),
            ("Installed memory (RAM):", format!("{:.2} GB", mem as f64 / 1048576.0)),
            ("System type:", format!("{} Operating System", if cfg!(target_pointer_width = "64") { "64-bit" } else { "32-bit" })),
            ("Graphics:", out(&["sh", "-c", "lspci 2>/dev/null | grep -iE 'vga|3d' | cut -d: -f3 | head -1"]).trim().to_string()),
        ]), false, false, 0);
        v.pack_start(&section("Computer name, domain, and workgroup settings"), false, false, 0);
        let name = glib::host_name().to_string();
        v.pack_start(&kv_grid(&[("Computer name:", name.clone()), ("Uptime:", out(&["uptime", "-p"]).trim().to_string())]), false, false, 0);
        let me = self.clone();
        v.pack_start(&link("Change settings", move || {
            if let Some(n) = aerode::desktop::prompt("Computer Name", "Computer name:", &glib::host_name()) {
                if admin(&["hostnamectl", "set-hostname", &n]) { me.open("system"); }
            }
        }), false, false, 0);
        v.upcast()
    }

    pub(crate) fn page_display(self: &Rc<Self>) -> gtk::Widget {
        self.side_link("Personalization", "personalization");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Change the appearance of your displays"), false, false, 0);
        // parse xrandr
        let xr = out(&["xrandr", "--query"]);
        // (name, primary, [(mode, [rates])], current mode, current rate, active)
        type Out = (String, bool, Vec<(String, Vec<String>)>, String, String, bool);
        let mut outputs: Vec<Out> = vec![];
        for l in xr.lines() {
            if !l.starts_with(' ') && l.contains(" connected") {
                let name = l.split_whitespace().next().unwrap_or("").to_string();
                let geom = l.split_whitespace().find(|w| w.contains('x') && w.contains('+'));
                let cur = geom.map(|w| w.split('+').next().unwrap().to_string()).unwrap_or_default();
                outputs.push((name, l.contains("primary"), vec![], cur, String::new(), geom.is_some()));
            } else if l.starts_with("   ") {
                if let Some(o) = outputs.last_mut() {
                    let mut it = l.split_whitespace();
                    let Some(mode) = it.next() else { continue };
                    let mut rates = vec![];
                    for r in it {
                        let clean = r.trim_end_matches(['*', '+', ' ']).to_string();
                        if clean.is_empty() { continue; }
                        if r.contains('*') { o.4 = clean.clone(); }
                        if !rates.contains(&clean) { rates.push(clean); }
                    }
                    // highest refresh first, like the Win7 list
                    rates.sort_by(|a, b| b.parse::<f64>().unwrap_or(0.0).partial_cmp(&a.parse::<f64>().unwrap_or(0.0)).unwrap());
                    match o.2.iter_mut().find(|(m, _)| m == mode) { Some(e) => { for r in rates { if !e.1.contains(&r) { e.1.push(r); } } } None => o.2.push((mode.to_string(), rates)) }
                }
            }
        }
        if outputs.is_empty() { v.pack_start(&text("No displays found (xrandr is not available)."), false, false, 0); return v.upcast(); }
        let g = gtk::Grid::new();
        g.set_row_spacing(8);
        g.set_column_spacing(12);
        let disp = gtk::ComboBoxText::new();
        for (i, o) in outputs.iter().enumerate() { disp.append(Some(&o.0), &format!("{}. {}{}", i + 1, o.0, if o.1 { " (primary)" } else { "" })); }
        disp.set_active(Some(0));
        let res = gtk::ComboBoxText::new();
        let rate = gtk::ComboBoxText::new();
        let orient = gtk::ComboBoxText::new();
        for (id, l) in [("normal", "Landscape"), ("left", "Portrait"), ("inverted", "Landscape (flipped)"), ("right", "Portrait (flipped)")] { orient.append(Some(id), l); }
        orient.set_active_id(Some("normal"));
        let multi = gtk::ComboBoxText::new();
        for (id, l) in [("extend", "Extend these displays"), ("dup", "Duplicate these displays"), ("only", "Show desktop only on this display")] { multi.append(Some(id), l); }
        multi.set_active_id(Some("extend"));
        let fill_rates = {
            let (rate, res, disp, outputs) = (rate.clone(), res.clone(), disp.clone(), outputs.clone());
            move || {
                rate.remove_all();
                let (Some(name), Some(mode)) = (disp.active_id(), res.active_id()) else { return };
                let Some(o) = outputs.iter().find(|o| o.0 == name.as_str()) else { return };
                let rates = o.2.iter().find(|(m, _)| *m == mode.as_str()).map(|(_, r)| r.clone()).unwrap_or_default();
                for r in &rates { rate.append(Some(r), &format!("{} Hertz", r.parse::<f64>().map(|v| v.round() as i64).unwrap_or(0))); }
                // keep the current rate if this is the current mode, otherwise pick the highest
                if mode.as_str() == o.3 && rates.contains(&o.4) { rate.set_active_id(Some(&o.4)); } else { rate.set_active(Some(0)); }
            }
        };
        let fill_res = {
            let (res, outputs) = (res.clone(), outputs.clone());
            move |name: &str| {
                res.remove_all();
                if let Some(o) = outputs.iter().find(|o| o.0 == name) {
                    for (i, (m, _)) in o.2.iter().enumerate() { res.append(Some(m), &format!("{m}{}", if i == 0 { " (Recommended)" } else { "" })); }
                    res.set_active_id(Some(&o.3));
                }
            }
        };
        fill_res(&outputs[0].0);
        fill_rates();
        { let (f, fr) = (fill_res.clone(), fill_rates.clone()); disp.connect_changed(move |d| if let Some(id) = d.active_id() { f(&id); fr(); }); }
        { let fr = fill_rates.clone(); res.connect_changed(move |_| fr()); }
        for (i, (l, w)) in [("Display:", disp.clone().upcast::<gtk::Widget>()), ("Resolution:", res.clone().upcast()), ("Screen refresh rate:", rate.clone().upcast()), ("Orientation:", orient.clone().upcast()), ("Multiple displays:", multi.clone().upcast())].into_iter().enumerate() {
            let lab = gtk::Label::new(Some(l));
            lab.set_xalign(1.0);
            g.attach(&lab, 0, i as i32, 1, 1);
            g.attach(&w, 1, i as i32, 1, 1);
        }
        v.pack_start(&g, false, false, 0);
        let me = self.clone();
        v.pack_start(&link("Make text and other items larger or smaller", move || me.open("access")), false, false, 0);
        // desktop composition (picom): Aero glass, shadows, vsync
        v.pack_start(&section("Desktop composition"), false, false, 0);
        let comp = gtk::CheckButton::with_label("Enable desktop composition (Aero glass, shadows, animations)");
        comp.set_active(!config::conf_dir().join("no-compositor").exists());
        comp.connect_toggled(|c| {
            let f = config::conf_dir().join("no-compositor");
            let _ = std::fs::create_dir_all(config::conf_dir());
            if c.is_active() { let _ = std::fs::remove_file(&f); } else { let _ = std::fs::write(&f, ""); let _ = std::process::Command::new("pkill").args(["-x", "picom"]).status(); }
            if c.is_active() { util::spawn(&["aerode", "restart"]); }
        });
        v.pack_start(&comp, false, false, 0);
        let vs = gtk::CheckButton::with_label("Sync to the display's refresh rate (VSync) – turn off if you are capped at 60 FPS");
        let user_conf = config::conf_dir().join("picom.conf");
        vs.set_active(!std::fs::read_to_string(&user_conf).map(|s| s.contains("vsync = false")).unwrap_or(false));
        vs.connect_toggled(move |c| {
            let base = std::fs::read_to_string("/usr/share/aerode/picom.conf").unwrap_or_default();
            let txt = base.replace("vsync = true;", if c.is_active() { "vsync = true;" } else { "vsync = false;" });
            let _ = std::fs::create_dir_all(config::conf_dir());
            let _ = std::fs::write(config::conf_dir().join("picom.conf"), txt);
            util::spawn(&["aerode", "restart"]);
        });
        v.pack_start(&vs, false, false, 0);
        let apply = gtk::Button::with_label("Apply");
        apply.set_halign(gtk::Align::End);
        let (w, outs) = (self.win.clone(), outputs.clone());
        apply.connect_clicked(move |_| {
            let (Some(o), Some(m)) = (disp.active_id(), res.active_id()) else { return };
            let this = outs.iter().find(|x| x.0 == o.as_str()).cloned();
            let (old, old_rate) = this.as_ref().map(|x| (x.3.clone(), x.4.clone())).unwrap_or_default();
            let rot = orient.active_id().map(|s| s.to_string()).unwrap_or_else(|| "normal".into());
            let r = rate.active_id().map(|s| s.to_string()).unwrap_or_default();
            let mut cmd = vec!["xrandr".to_string(), "--output".into(), o.to_string(), "--mode".into(), m.to_string()];
            // always pass the rate: without it xrandr may fall back to 60 Hz
            if !r.is_empty() { cmd.extend(["--rate".into(), r.clone()]); }
            cmd.extend(["--rotate".into(), rot]);
            let others: Vec<&Out> = outs.iter().filter(|x| x.0 != o.as_str()).collect();
            match multi.active_id().as_deref() {
                Some("dup") => for n in &others { cmd.extend(["--output".into(), n.0.clone(), "--same-as".into(), o.to_string()]); },
                Some("only") => for n in &others { cmd.extend(["--output".into(), n.0.clone(), "--off".into()]); },
                _ => if let Some(n) = others.first() {
                    // leave the other monitor's own mode/rate alone unless it was off
                    cmd.extend(["--output".into(), n.0.clone()]);
                    if !n.5 { cmd.push("--auto".into()); }
                    cmd.extend(["--right-of".into(), o.to_string()]);
                },
            }
            let refs: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
            util::output(&refs);
            // Win7: "Keep these display settings?" with a 15 s automatic revert
            let d = gtk::MessageDialog::new(Some(&w), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::None, "Do you want to keep these display settings?");
            d.set_title("Display Settings");
            d.add_button("Keep changes", gtk::ResponseType::Yes);
            d.add_button("Revert", gtk::ResponseType::No);
            let secs = Rc::new(std::cell::Cell::new(15));
            d.set_secondary_text(Some("Reverting to previous display settings in 15 seconds."));
            { let (d2, s) = (d.clone(), secs.clone()); glib::timeout_add_seconds_local(1, move || {
                let n = s.get() - 1; s.set(n);
                if n <= 0 { d2.response(gtk::ResponseType::No); return glib::ControlFlow::Break; }
                d2.set_secondary_text(Some(&format!("Reverting to previous display settings in {n} seconds.")));
                if d2.is_visible() { glib::ControlFlow::Continue } else { glib::ControlFlow::Break }
            }); }
            let r = d.run();
            d.close();
            if r != gtk::ResponseType::Yes && !old.is_empty() {
                let mut back = vec!["xrandr", "--output", &o, "--mode", &old];
                if !old_rate.is_empty() { back.extend(["--rate", old_rate.as_str()]); }
                back.extend(["--rotate", "normal"]);
                util::output(&back);
            }
            else {
                // remember for next login
                let _ = std::fs::create_dir_all(config::conf_dir());
                let _ = std::fs::write(config::conf_dir().join("displays.sh"), format!("#!/bin/sh\n{}\n", cmd.join(" ")));
            }
        });
        v.pack_start(&apply, false, false, 8);
        v.upcast()
    }

    // ----------------------------------------------------------------- sound

    pub(crate) fn page_sound(self: &Rc<Self>) -> gtk::Widget {
        let nb = gtk::Notebook::new();
        // Playback / Recording devices
        for (label, kind) in [("Playback", "sinks"), ("Recording", "sources")] {
            let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
            v.set_border_width(10);
            v.pack_start(&text(&format!("Select a {} device below to modify its settings:", if kind == "sinks" { "playback" } else { "recording" })), false, false, 0);
            let list = gtk::ListBox::new();
            let def = out(&["pactl", if kind == "sinks" { "get-default-sink" } else { "get-default-source" }]).trim().to_string();
            let mut name = String::new();
            for l in out(&["pactl", "list", kind]).lines() {
                let l = l.trim();
                if let Some(n) = l.strip_prefix("Name: ") { name = n.to_string(); }
                if let Some(d) = l.strip_prefix("Description: ") {
                    if kind == "sources" && name.ends_with(".monitor") { continue; }
                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
                    row.set_border_width(6);
                    row.pack_start(&util::image(if kind == "sinks" { "audio-volume-high" } else { "audio-input-microphone" }, 32), false, false, 0);
                    let t = gtk::Label::new(None);
                    t.set_markup(&format!("{}\n<span foreground='#6d6d6d'>{}</span>", glib::markup_escape_text(d), if name == def { "Default Device" } else { "Ready" }));
                    t.set_xalign(0.0);
                    row.pack_start(&t, true, true, 0);
                    let b = gtk::Button::with_label("Set Default");
                    let (n, k) = (name.clone(), kind);
                    b.set_sensitive(name != def);
                    b.connect_clicked(move |b| { util::spawn(&["pactl", if k == "sinks" { "set-default-sink" } else { "set-default-source" }, &n]); b.set_sensitive(false); });
                    row.pack_end(&b, false, false, 0);
                    list.add(&row);
                }
            }
            v.pack_start(&list, true, true, 0);
            nb.append_page(&v, Some(&gtk::Label::new(Some(label))));
        }
        // Sounds tab: the Win7 scheme editor
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.set_border_width(10);
        v.pack_start(&text("A sound theme is a set of sounds applied to events in Windows and programs."), false, false, 0);
        let mute = gtk::CheckButton::with_label("Mute all system sounds (\"No Sounds\" scheme)");
        mute.set_active(config::get().sounds_muted);
        mute.connect_toggled(|c| { let m = c.is_active(); config::update(|cf| cf.sounds_muted = m); });
        v.pack_start(&mute, false, false, 0);
        let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
        let fill = {
            let store = store.clone();
            move || {
                store.clear();
                for (i, ev) in sounds::Event::ALL.iter().enumerate() {
                    let f = sounds::file_for(*ev).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).unwrap_or_else(|| "(None)".into());
                    let it = store.append();
                    store.set(&it, &[(0, &ev.label()), (1, &f), (2, &(i as u32))]);
                }
            }
        };
        fill();
        let tv = gtk::TreeView::with_model(&store);
        for (i, t) in ["Program Events", "Sound"].iter().enumerate() {
            let c = gtk::TreeViewColumn::new();
            c.set_title(t);
            let r = gtk::CellRendererText::new();
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
            c.set_resizable(true);
            tv.append_column(&c);
        }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_min_content_height(260);
        sw.add(&tv);
        v.pack_start(&sw, true, true, 0);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let combo = gtk::ComboBoxText::new();
        combo.append(Some(""), "(None)");
        if let Ok(rd) = std::fs::read_dir(config::data_dir().join("sounds")) {
            let mut names: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            for n in names { combo.append(Some(&n), &n); }
        }
        let test = gtk::Button::with_label("Test");
        let browse = gtk::Button::with_label("Browse...");
        row.pack_start(&gtk::Label::new(Some("Sounds:")), false, false, 0);
        row.pack_start(&combo, true, true, 0);
        row.pack_start(&test, false, false, 0);
        row.pack_start(&browse, false, false, 0);
        v.pack_start(&row, false, false, 0);
        let sel_ev = Rc::new(std::cell::Cell::new(None::<usize>));
        { let (combo, s) = (combo.clone(), sel_ev.clone()); tv.selection().connect_changed(move |sel| {
            if let Some((m, it)) = sel.selected() {
                let i: u32 = m.value(&it, 2).get().unwrap_or(0);
                s.set(Some(i as usize));
                let f: String = m.value(&it, 1).get().unwrap_or_default();
                combo.set_active_id(Some(if f == "(None)" { "" } else { &f }));
            }
        }); }
        { let (s, f) = (sel_ev.clone(), fill.clone()); combo.connect_changed(move |c| {
            let (Some(i), Some(id)) = (s.get(), c.active_id()) else { return };
            let ev = sounds::Event::ALL[i];
            let cur = sounds::file_for(ev).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).unwrap_or_default();
            if cur == id.as_str() { return; }
            config::update(|cf| { cf.sounds.insert(ev.key().to_string(), id.to_string()); });
            let _ = &f;
        }); }
        { let s = sel_ev.clone(); test.connect_clicked(move |_| if let Some(i) = s.get() { sounds::play(sounds::Event::ALL[i]); }); }
        { let (s, w, f) = (sel_ev.clone(), self.win.clone(), fill.clone()); browse.connect_clicked(move |_| {
            let Some(i) = s.get() else { return };
            let fc = gtk::FileChooserNative::new(Some("Browse for new sound"), Some(&w), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
            let ff = gtk::FileFilter::new(); ff.add_pattern("*.wav"); ff.add_pattern("*.ogg"); ff.add_pattern("*.oga"); ff.set_name(Some("Wave Files (*.wav)")); fc.add_filter(ff);
            if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() {
                let ev = sounds::Event::ALL[i];
                config::update(|cf| { cf.sounds.insert(ev.key().to_string(), p.to_string_lossy().into_owned()); });
                f();
            } }
        }); }
        let reset = gtk::Button::with_label("Restore Windows Default");
        { let f = fill.clone(); reset.connect_clicked(move |_| { config::update(|cf| cf.sounds.clear()); f(); }); }
        reset.set_halign(gtk::Align::Start);
        v.pack_start(&reset, false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Sounds"))));
        nb.upcast()
    }

    // ----------------------------------------------------------------- date/time

    pub(crate) fn page_datetime(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Date and Time"), false, false, 0);
        let now = gtk::Label::new(None);
        now.set_xalign(0.0);
        { let n = now.clone(); let tick = move || { n.set_text(&format!("Date: {}\nTime: {}", chrono::Local::now().format("%A, %B %-d, %Y"), chrono::Local::now().format("%-I:%M:%S %p"))); glib::ControlFlow::Continue }; tick(); glib::timeout_add_seconds_local(1, tick); }
        v.pack_start(&now, false, false, 0);
        let set = gtk::Button::with_label("Change date and time...");
        set.set_halign(gtk::Align::Start);
        let w = self.win.clone();
        set.connect_clicked(move |_| {
            let d = gtk::Dialog::with_buttons(Some("Date and Time Settings"), Some(&w), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
            let cal = gtk::Calendar::new();
            let t = gtk::Entry::new();
            t.set_text(&chrono::Local::now().format("%H:%M:%S").to_string());
            d.content_area().pack_start(&cal, false, false, 6);
            d.content_area().pack_start(&t, false, false, 6);
            d.show_all();
            if d.run() == gtk::ResponseType::Ok {
                let (y, m, day) = cal.date();
                let s = format!("{y:04}-{:02}-{day:02} {}", m + 1, t.text());
                admin(&["timedatectl", "set-ntp", "false"]);
                if !admin(&["timedatectl", "set-time", &s]) { util::error("Unable to set the time", "Make sure the time is in HH:MM:SS format."); }
            }
            d.close();
        });
        v.pack_start(&set, false, false, 0);
        v.pack_start(&section("Time zone"), false, false, 0);
        let cur = out(&["timedatectl", "show", "-p", "Timezone", "--value"]).trim().to_string();
        let tz = gtk::ComboBoxText::with_entry();
        for z in out(&["timedatectl", "list-timezones"]).lines() { tz.append(Some(z), z); }
        tz.set_active_id(Some(&cur));
        let ch = gtk::Button::with_label("Change time zone");
        { let tz = tz.clone(); ch.connect_clicked(move |_| if let Some(z) = tz.active_id() { admin(&["timedatectl", "set-timezone", &z]); }); }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.pack_start(&tz, true, true, 0);
        row.pack_start(&ch, false, false, 0);
        v.pack_start(&row, false, false, 0);
        v.pack_start(&section("Internet Time"), false, false, 0);
        let ntp = gtk::CheckButton::with_label("Synchronize with an Internet time server");
        ntp.set_active(out(&["timedatectl", "show", "-p", "NTP", "--value"]).trim() == "yes");
        ntp.connect_toggled(|c| { admin(&["timedatectl", "set-ntp", if c.is_active() { "true" } else { "false" }]); });
        v.pack_start(&ntp, false, false, 0);
        v.pack_start(&section("Clock"), false, false, 0);
        let h24 = gtk::CheckButton::with_label("Use 24-hour clock on the taskbar");
        h24.set_active(config::get().clock_24h);
        h24.connect_toggled(|c| { let a = c.is_active(); config::update(|cf| cf.clock_24h = a); util::spawn(&["aerode", "reload"]); });
        v.pack_start(&h24, false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- power

    pub(crate) fn page_power(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Select a power plan"), false, false, 0);
        v.pack_start(&text("Power plans can help you maximize your computer's performance or conserve energy."), false, false, 0);
        let cur = aerode::sys::power_profile();
        let mut grp: Option<gtk::RadioButton> = None;
        for (id, name, desc) in [("balanced", "Balanced (recommended)", "Automatically balances performance with energy consumption on capable hardware."),
            ("power-saver", "Power saver", "Saves energy by reducing your computer's performance where possible."),
            ("performance", "High performance", "Favors performance, but may use more energy.")] {
            let r = match &grp { None => gtk::RadioButton::with_label(name), Some(g) => gtk::RadioButton::with_label_from_widget(g, name) };
            if grp.is_none() { grp = Some(r.clone()); }
            r.set_active(cur.as_deref() == Some(id));
            r.set_sensitive(cur.is_some());
            r.connect_toggled(move |r| if r.is_active() { aerode::sys::set_power_profile(id) });
            v.pack_start(&r, false, false, 0);
            let d = text(desc);
            d.set_margin_start(24);
            d.style_context().add_class("dim-label");
            v.pack_start(&d, false, false, 0);
        }
        if cur.is_none() { v.pack_start(&text("Install power-profiles-daemon to switch power plans."), false, false, 0); }
        v.pack_start(&section("Turn off the display"), false, false, 0);
        let combo = gtk::ComboBoxText::new();
        for (secs, l) in [(60, "1 minute"), (300, "5 minutes"), (600, "10 minutes"), (900, "15 minutes"), (1800, "30 minutes"), (3600, "1 hour"), (0, "Never")] { combo.append(Some(&secs.to_string()), l); }
        combo.set_active_id(Some("600"));
        combo.connect_changed(|c| if let Some(s) = c.active_id() {
            let s = s.to_string();
            if s == "0" { util::spawn(&["xset", "-dpms"]); util::spawn(&["xset", "s", "off"]); } else { util::spawn(&["xset", "+dpms", "dpms", &s, &s, &s]); util::spawn(&["xset", "s", &s]); }
            let _ = std::fs::create_dir_all(config::conf_dir());
            let _ = std::fs::write(config::conf_dir().join("display-off"), &s);
        });
        v.pack_start(&combo, false, false, 0);
        if let Some(b) = aerode::sys::brightness() {
            v.pack_start(&section("Screen brightness"), false, false, 0);
            let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
            s.set_value(b as f64);
            s.connect_value_changed(|s| aerode::sys::set_brightness(s.value() as u32));
            v.pack_start(&s, false, false, 0);
        }
        v.upcast()
    }

    // ----------------------------------------------------------------- mouse / keyboard

    pub(crate) fn page_mouse(self: &Rc<Self>) -> gtk::Widget {
        let nb = gtk::Notebook::new();
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.set_border_width(10);
        let sw = gtk::CheckButton::with_label("Switch primary and secondary buttons");
        sw.connect_toggled(|c| { util::spawn(&["sh", "-c", &format!("for id in $(xinput list --id-only 2>/dev/null); do xinput set-prop $id 'libinput Left Handed Enabled' {} 2>/dev/null; done", c.is_active() as u8)]); });
        v.pack_start(&sw, false, false, 0);
        v.pack_start(&section("Double-click speed"), false, false, 0);
        let dc = gtk::Scale::with_range(gtk::Orientation::Horizontal, 200.0, 900.0, 50.0);
        dc.set_inverted(true);
        if let Some(s) = gtk::Settings::default() { dc.set_value(s.gtk_double_click_time() as f64); }
        dc.connect_value_changed(|s| if let Some(st) = gtk::Settings::default() { st.set_gtk_double_click_time(s.value() as i32); });
        v.pack_start(&dc, false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Buttons"))));
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.set_border_width(10);
        v.pack_start(&section("Select a pointer speed"), false, false, 0);
        let sp = gtk::Scale::with_range(gtk::Orientation::Horizontal, -1.0, 1.0, 0.1);
        sp.connect_value_changed(|s| { util::spawn(&["sh", "-c", &format!("for id in $(xinput list --id-only 2>/dev/null); do xinput set-prop $id 'libinput Accel Speed' {:.2} 2>/dev/null; done", s.value())]); });
        v.pack_start(&sp, false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Pointer Options"))));
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.set_border_width(10);
        v.pack_start(&text("Scheme"), false, false, 0);
        let schemes = gtk::ComboBoxText::new();
        for d in [glib::user_data_dir().join("icons"), std::path::PathBuf::from("/usr/share/icons")] {
            if let Ok(rd) = std::fs::read_dir(d) { for e in rd.flatten() { if e.path().join("cursors").is_dir() { let n = e.file_name().to_string_lossy().into_owned(); schemes.append(Some(&n), &if n == "Aerode-cursors" { "Windows Aero (system scheme)".into() } else { n.clone() }); } } }
        }
        schemes.set_active_id(Some(&std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "Aerode-cursors".into())));
        schemes.connect_changed(|c| if let Some(id) = c.active_id() {
            let d = glib::user_data_dir().join("icons/default");
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join("index.theme"), format!("[Icon Theme]\nInherits={id}\n"));
            if let Some(s) = gtk::Settings::default() { s.set_gtk_cursor_theme_name(Some(&id)); }
        });
        v.pack_start(&schemes, false, false, 0);
        v.pack_start(&text("The new pointer scheme fully applies the next time you log on."), false, false, 0);
        nb.append_page(&v, Some(&gtk::Label::new(Some("Pointers"))));
        nb.upcast()
    }

    pub(crate) fn page_keyboard(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Keyboard Properties"), false, false, 0);
        v.pack_start(&section("Repeat delay"), false, false, 0);
        let delay = gtk::Scale::with_range(gtk::Orientation::Horizontal, 150.0, 1000.0, 50.0);
        delay.set_value(500.0);
        delay.set_inverted(true);
        v.pack_start(&delay, false, false, 0);
        v.pack_start(&section("Repeat rate"), false, false, 0);
        let rate = gtk::Scale::with_range(gtk::Orientation::Horizontal, 5.0, 50.0, 1.0);
        rate.set_value(30.0);
        v.pack_start(&rate, false, false, 0);
        let apply = { let (d, r) = (delay.clone(), rate.clone()); move || { util::spawn(&["xset", "r", "rate", &format!("{}", d.value() as u32), &format!("{}", r.value() as u32)]); } };
        { let a = apply.clone(); delay.connect_value_changed(move |_| a()); }
        rate.connect_value_changed(move |_| apply());
        let test = gtk::Entry::new();
        test.set_placeholder_text(Some("Click here and hold down a key to test repeat rate"));
        v.pack_start(&test, false, false, 0);
        let me = self.clone();
        v.pack_start(&link("Change keyboards or other input methods", move || me.open("region")), false, false, 6);
        v.upcast()
    }

    // ----------------------------------------------------------------- programs
}
