//! Control Panel pages: Startup, Services, Programs and Features, User Accounts, Network and Sharing
//! Center, Default Programs, Devices and Printers, Region and Language, Ease of Access.
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
    pub(crate) fn page_startup(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("Choose which programs start when you log on"), false, false, 0);
        let dir = glib::user_config_dir().join("autostart");
        let mut seen = std::collections::BTreeMap::new();
        for d in glib::system_config_dirs().iter().map(|d| d.join("autostart")).chain(std::iter::once(dir.clone())) {
            if let Ok(rd) = std::fs::read_dir(&d) { for e in rd.flatten() { seen.insert(e.file_name().to_string_lossy().into_owned(), e.path()); } }
        }
        for (name, path) in seen {
            let Some(app) = gio::DesktopAppInfo::from_filename(&path) else { continue };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let c = gtk::CheckButton::new();
            c.set_active(!app.is_hidden() && !(app.has_key("X-GNOME-Autostart-enabled") && !app.boolean("X-GNOME-Autostart-enabled")));
            let (dir, name2, p2) = (dir.clone(), name.clone(), path.clone());
            c.connect_toggled(move |c| {
                let _ = std::fs::create_dir_all(&dir);
                let target = dir.join(&name2);
                let mut content = std::fs::read_to_string(&p2).unwrap_or_default();
                content = content.lines().filter(|l| !l.starts_with("Hidden=")).collect::<Vec<_>>().join("\n");
                content.push_str(&format!("\nHidden={}\n", !c.is_active()));
                let _ = std::fs::write(target, content);
            });
            row.pack_start(&c, false, false, 0);
            row.pack_start(&util::app_image(app.upcast_ref(), 16), false, false, 0);
            let l = gtk::Label::new(Some(&format!("{}   —   {}", app.display_name(), app.commandline().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default())));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.pack_start(&l, true, true, 0);
            v.pack_start(&row, false, false, 0);
        }
        v.upcast()
    }

    pub(crate) fn page_services(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("Services"), false, false, 0);
        let store = gtk::ListStore::new(&[glib::Type::STRING; 4]);
        for l in out(&["systemctl", "list-units", "--type=service", "--all", "--no-legend", "--plain", "--no-pager"]).lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 { continue; }
            let it = store.append();
            let desc = f[4..].join(" ");
            store.set(&it, &[(0, &f[0].trim_end_matches(".service")), (1, &desc), (2, &if f[3] == "running" { "Started" } else { "" }), (3, &f[0])]);
        }
        let tv = gtk::TreeView::with_model(&store);
        for (i, t) in ["Name", "Description", "Status"].iter().enumerate() {
            let c = gtk::TreeViewColumn::new();
            c.set_title(t);
            let r = gtk::CellRendererText::new();
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", i as i32);
            c.set_resizable(true);
            c.set_sort_column_id(i as i32);
            tv.append_column(&c);
        }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_min_content_height(380);
        sw.add(&tv);
        v.pack_start(&sw, true, true, 0);
        let bb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        for (label, verb) in [("Start", "start"), ("Stop", "stop"), ("Restart", "restart")] {
            let b = gtk::Button::with_label(label);
            let (tv, me) = (tv.clone(), self.clone());
            b.connect_clicked(move |_| {
                if let Some((m, it)) = tv.selection().selected() {
                    let unit: String = m.value(&it, 3).get().unwrap_or_default();
                    if admin(&["systemctl", verb, &unit]) { me.open("services"); }
                }
            });
            bb.pack_start(&b, false, false, 0);
        }
        v.pack_start(&bb, false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- display

    pub(crate) fn page_programs(self: &Rc<Self>) -> gtk::Widget {
        self.side_link("View installed updates", "programs");
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.pack_start(&title("Uninstall or change a program"), false, false, 0);
        v.pack_start(&text("To uninstall a program, select it from the list and then click Uninstall."), false, false, 0);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.style_context().add_class("commandbar");
        let uninstall = gtk::Button::with_label("Uninstall");
        let search = gtk::SearchEntry::new();
        bar.pack_start(&uninstall, false, false, 0);
        bar.pack_end(&search, false, false, 0);
        v.pack_start(&bar, false, false, 0);
        let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::U64, glib::Type::STRING]);
        let filter = gtk::TreeModelFilter::new(&store, None);
        { let s = search.clone(); filter.set_visible_func(move |m, it| { let q = s.text().to_lowercase(); q.is_empty() || m.value(it, 0).get::<String>().unwrap_or_default().to_lowercase().contains(&q) || m.value(it, 1).get::<String>().unwrap_or_default().to_lowercase().contains(&q) }); }
        { let f = filter.clone(); search.connect_search_changed(move |_| f.refilter()); }
        let sorted = gtk::TreeModelSort::new(&filter);
        let tv = gtk::TreeView::with_model(&sorted);
        for (i, t, w) in [(0, "Name", 220), (1, "Publisher", 200), (2, "Installed On", 100), (3, "Size", 80), (5, "Version", 110)] {
            let c = gtk::TreeViewColumn::new();
            c.set_title(t);
            let r = gtk::CellRendererText::new();
            if i == 3 { r.set_xalign(1.0); }
            TreeViewColumnExt::pack_start(&c, &r, true);
            TreeViewColumnExt::add_attribute(&c, &r, "text", i);
            c.set_resizable(true);
            c.set_sort_column_id(if i == 3 { 4 } else { i });
            c.set_fixed_width(w);
            c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            tv.append_column(&c);
        }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_min_content_height(380);
        sw.add(&tv);
        v.pack_start(&sw, true, true, 0);
        let status = gtk::Label::new(Some("Loading installed programs..."));
        status.set_xalign(0.0);
        v.pack_start(&status, false, false, 0);
        // pacman -Qei in a thread (explicitly installed packages, like "programs")
        let (tx, rx) = std::sync::mpsc::channel::<Vec<[String; 6]>>();
        std::thread::spawn(move || {
            let txt = util::output(&["pacman", "-Qei"]).unwrap_or_default();
            let mut rows = vec![];
            for block in txt.split("\n\n") {
                let get = |k: &str| block.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim_start_matches([' ', ':']).trim().to_string())).unwrap_or_default();
                let name = get("Name");
                if name.is_empty() { continue; }
                let size = get("Installed Size");
                let bytes = { let mut it = size.split_whitespace(); let n: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0); (match it.next() { Some("KiB") => n * 1024.0, Some("MiB") => n * 1048576.0, Some("GiB") => n * 1073741824.0, _ => n }) as u64 };
                let date = get("Install Date");
                let short_date = chrono::DateTime::parse_from_str(&date, "%a %d %b %Y %I:%M:%S %p %Z").map(|d| d.format("%-m/%-d/%Y").to_string()).unwrap_or_else(|_| date.split_whitespace().take(4).collect::<Vec<_>>().join(" "));
                rows.push([name, get("Packager"), short_date, size, bytes.to_string(), get("Version")]);
            }
            let _ = tx.send(rows);
        });
        { let (store, status) = (store.clone(), status.clone()); glib::timeout_add_local(std::time::Duration::from_millis(100), move || match rx.try_recv() {
            Ok(rows) => {
                let total: u64 = rows.iter().map(|r| r[4].parse::<u64>().unwrap_or(0)).sum();
                for r in &rows { let it = store.append(); store.set(&it, &[(0, &r[0]), (1, &r[1]), (2, &r[2]), (3, &r[3]), (4, &r[4].parse::<u64>().unwrap_or(0)), (5, &r[5])]); }
                status.set_text(&format!("Currently installed programs   Total size: {}\n{} programs installed", util::human_size(total), rows.len()));
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(_) => { status.set_text("pacman was not found."); glib::ControlFlow::Break }
        }); }
        { let (tv, me) = (tv.clone(), self.clone()); uninstall.connect_clicked(move |_| {
            let Some((m, it)) = tv.selection().selected() else { return };
            let name: String = m.value(&it, 0).get().unwrap_or_default();
            let d = gtk::MessageDialog::new(Some(&me.win), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo, &format!("Are you sure you want to uninstall {name}?"));
            d.set_title("Programs and Features");
            let r = d.run();
            d.close();
            if r == gtk::ResponseType::Yes {
                if admin(&["pacman", "-Rns", "--noconfirm", &name]) { me.open("programs"); } else { util::error(&format!("{name} could not be uninstalled."), "Another package may depend on it."); }
            }
        }); }
        v.upcast()
    }

    // ----------------------------------------------------------------- users

    pub(crate) fn page_users(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Make changes to your user account"), false, false, 0);
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        let face = config::home().join(".face");
        let pic = gtk::Image::new();
        let set_pic = { let pic = pic.clone(); move |p: &std::path::Path| pic.set_from_pixbuf(gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, 64, 64, false).ok().as_ref()) };
        if face.exists() { set_pic(&face); } else if let Some(a) = config::data_file("avatar.png") { set_pic(&a); }
        h.pack_start(&pic, false, false, 0);
        let real = glib::real_name().to_string_lossy().into_owned();
        let user = glib::user_name().to_string_lossy().into_owned();
        let is_admin = out(&["id", "-nG"]).split_whitespace().any(|g| g == "wheel" || g == "sudo");
        let info = gtk::Label::new(None);
        info.set_markup(&format!("<b>{}</b>\n{}", glib::markup_escape_text(if real.is_empty() || real == "Unknown" { &user } else { &real }), if is_admin { "Administrator" } else { "Standard user" }));
        info.set_xalign(0.0);
        h.pack_start(&info, false, false, 0);
        v.pack_start(&h, false, false, 0);
        v.pack_start(&link("Change your password", || {
            if let Some(t) = util::terminal() { let _ = t; util::spawn(&["sh", "-c", "x-terminal-emulator -e passwd || xterm -e passwd || kgx -- passwd || gnome-terminal -- passwd || konsole -e passwd"]); }
        }), false, false, 0);
        {
            let w = self.win.clone();
            let sp = set_pic.clone();
            v.pack_start(&link("Change your picture", move || {
                let d = gtk::Dialog::with_buttons(Some("Change Your Picture"), Some(&w), gtk::DialogFlags::MODAL, &[("Change Picture", gtk::ResponseType::Ok), ("Browse for more pictures...", gtk::ResponseType::Other(1)), ("Cancel", gtk::ResponseType::Cancel)]);
                let flow = gtk::FlowBox::new();
                flow.set_max_children_per_line(6);
                let mut paths = vec![];
                if let Ok(rd) = std::fs::read_dir(config::data_dir().join("avatars")) { for e in rd.flatten() { paths.push(e.path()); } }
                paths.sort();
                for p in &paths { if let Ok(pb) = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, 64, 64, false) { flow.insert(&gtk::Image::from_pixbuf(Some(&pb)), -1); } }
                d.content_area().pack_start(&flow, true, true, 8);
                d.show_all();
                let r = d.run();
                let chosen = match r {
                    gtk::ResponseType::Ok => flow.selected_children().first().map(|c| paths[c.index() as usize].clone()),
                    gtk::ResponseType::Other(1) => {
                        let fc = gtk::FileChooserNative::new(Some("Browse"), Some(&d), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
                        let f = gtk::FileFilter::new(); f.add_pixbuf_formats(); fc.add_filter(f);
                        if fc.run() == gtk::ResponseType::Accept { fc.filename() } else { None }
                    }
                    _ => None,
                };
                d.close();
                if let Some(p) = chosen {
                    let face = config::home().join(".face");
                    if let Ok(pb) = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&p, 128, 128, false) { let _ = pb.savev(&face, "png", &[]); sp(&face); }
                }
            }), false, false, 0);
        }
        v.pack_start(&link("Change your account name", move || {
            if let Some(n) = aerode::desktop::prompt("Change Your Name", "New account name:", &glib::real_name().to_string_lossy()) { admin(&["chfn", "-f", &n, &glib::user_name().to_string_lossy()]); }
        }), false, false, 0);
        v.pack_start(&section("Other accounts"), false, false, 0);
        let users: Vec<String> = std::fs::read_to_string("/etc/passwd").unwrap_or_default().lines().filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() > 6 && f[2].parse::<u32>().map(|u| (1000..60000).contains(&u)).unwrap_or(false)).then(|| f[0].to_string())
        }).collect();
        for u in users { v.pack_start(&text(&format!("• {u}")), false, false, 0); }
        v.upcast()
    }

    // ----------------------------------------------------------------- network

    pub(crate) fn page_network(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("View your basic network information and set up connections"), false, false, 0);
        let st = aerode::sys::net_status();
        let map = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        for (icon, label) in [("computer", glib::host_name().to_string()), (aerode::sys::net_icon(&st), if st.kind == "none" { "Not connected".to_string() } else { st.name.clone() }), ("web-browser", "Internet".to_string())] {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
            b.pack_start(&util::image(icon, 48), false, false, 0);
            b.pack_start(&gtk::Label::new(Some(&label)), false, false, 0);
            map.pack_start(&b, false, false, 0);
            if label != "Internet" { map.pack_start(&gtk::Label::new(Some(if st.kind == "none" && icon != "computer" { "✕" } else { "———" })), false, false, 0); }
        }
        v.pack_start(&map, false, false, 6);
        v.pack_start(&section("View your active networks"), false, false, 0);
        for l in out(&["nmcli", "-t", "-f", "NAME,TYPE,DEVICE", "connection", "show", "--active"]).lines() {
            let f = aerode::sys::nm_split(l);
            if f.len() >= 3 { v.pack_start(&kv_grid(&[(&f[0], format!("Access type: Internet\nConnections: {} ({})", f[2], f[1]))]), false, false, 0); }
        }
        v.pack_start(&section("Change your networking settings"), false, false, 0);
        v.pack_start(&link("Set up a new connection or network", || { util::spawn(&["nm-connection-editor"]); }), false, false, 0);
        v.pack_start(&link("Change adapter settings", || { util::spawn(&["nm-connection-editor"]); }), false, false, 0);
        v.pack_start(&link("Troubleshoot problems", || { util::spawn(&["sh", "-c", "x-terminal-emulator -e sh -c 'nmcli general status; ip addr; ping -c 3 archlinux.org; read x' || xterm -e sh -c 'nmcli general status; ping -c 3 archlinux.org; read x'"]); }), false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- defaults

    pub(crate) fn page_defaults(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Set your default programs"), false, false, 0);
        let g = gtk::Grid::new();
        g.set_row_spacing(8);
        g.set_column_spacing(12);
        for (i, (label, mime)) in [("Web browser", "x-scheme-handler/http"), ("E-mail", "x-scheme-handler/mailto"), ("Music", "audio/mpeg"), ("Video", "video/mp4"),
            ("Photos", "image/jpeg"), ("Text documents", "text/plain"), ("PDF documents", "application/pdf"), ("Folders", "inode/directory")].iter().enumerate() {
            let l = gtk::Label::new(Some(label));
            l.set_xalign(0.0);
            g.attach(&l, 0, i as i32, 1, 1);
            let combo = gtk::AppChooserButton::new(mime);
            combo.set_show_default_item(true);
            let mime = mime.to_string();
            combo.connect_changed(move |c| {
                if let Some(a) = c.app_info() {
                    let _ = a.set_as_default_for_type(&mime);
                    if mime == "x-scheme-handler/http" { let _ = a.set_as_default_for_type("x-scheme-handler/https"); let _ = a.set_as_default_for_type("text/html"); }
                    if mime == "image/jpeg" { for t in ["image/png", "image/gif", "image/bmp", "image/webp"] { let _ = a.set_as_default_for_type(t); } }
                    if mime == "audio/mpeg" { for t in ["audio/flac", "audio/ogg", "audio/x-wav"] { let _ = a.set_as_default_for_type(t); } }
                    if mime == "video/mp4" { for t in ["video/x-matroska", "video/webm", "video/x-msvideo"] { let _ = a.set_as_default_for_type(t); } }
                }
            });
            g.attach(&combo, 1, i as i32, 1, 1);
        }
        v.pack_start(&g, false, false, 0);
        v.upcast()
    }

    // ----------------------------------------------------------------- devices

    pub(crate) fn page_devices(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.style_context().add_class("commandbar");
        let add_dev = gtk::Button::with_label("Add a device");
        add_dev.connect_clicked(|_| { if !util::spawn(&["blueman-manager"]) { util::spawn(&["sh", "-c", "x-terminal-emulator -e bluetoothctl || xterm -e bluetoothctl"]); } });
        let add_pr = gtk::Button::with_label("Add a printer");
        add_pr.connect_clicked(|_| { if !util::spawn(&["system-config-printer"]) { util::open_uri("http://localhost:631/admin"); } });
        bar.pack_start(&add_dev, false, false, 0);
        bar.pack_start(&add_pr, false, false, 0);
        v.pack_start(&bar, false, false, 0);
        let group = |t: &str, items: Vec<(String, &str)>| {
            v.pack_start(&section(&format!("{t} ({})", items.len())), false, false, 0);
            let f = gtk::FlowBox::new();
            f.set_selection_mode(gtk::SelectionMode::None);
            f.set_max_children_per_line(6);
            for (name, icon) in items {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
                b.pack_start(&util::image(icon, 48), false, false, 0);
                let l = gtk::Label::new(Some(&name));
                l.set_line_wrap(true);
                l.set_max_width_chars(14);
                l.set_justify(gtk::Justification::Center);
                b.pack_start(&l, false, false, 0);
                f.insert(&b, -1);
            }
            v.pack_start(&f, false, false, 0);
        };
        let mut devices = vec![(glib::host_name().to_string(), "computer")];
        for l in out(&["xrandr", "--query"]).lines().filter(|l| l.contains(" connected")) { devices.push((format!("Display {}", l.split_whitespace().next().unwrap_or("")), "preferences-desktop-display")); }
        let inputs = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
        for block in inputs.split("\n\n") {
            let name = block.lines().find_map(|l| l.strip_prefix("N: Name=")).map(|s| s.trim_matches('"').to_string()).unwrap_or_default();
            let h = block.lines().find_map(|l| l.strip_prefix("H: Handlers=")).unwrap_or("");
            if name.is_empty() || name.contains("Button") || name.contains("Video Bus") || name.contains("PC Speaker") { continue; }
            if h.contains("mouse") { devices.push((name, "input-mouse")); } else if h.contains("kbd") && h.contains("sysrq") { devices.push((name, "preferences-desktop-keyboard")); }
        }
        for l in out(&["bluetoothctl", "devices", "Paired"]).lines() { devices.push((l.splitn(3, ' ').nth(2).unwrap_or("Bluetooth device").to_string(), "bluetooth")); }
        group("Devices", devices);
        let printers: Vec<(String, &str)> = out(&["lpstat", "-p"]).lines().filter_map(|l| l.strip_prefix("printer ").map(|r| (r.split_whitespace().next().unwrap_or("").to_string(), "printer"))).collect();
        group("Printers and Faxes", printers);
        v.upcast()
    }

    // ----------------------------------------------------------------- region / access

    pub(crate) fn page_region(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Region and Language"), false, false, 0);
        let loc = out(&["localectl", "status"]);
        v.pack_start(&kv_grid(&[("Format:", loc.lines().find_map(|l| l.trim().strip_prefix("System Locale:")).unwrap_or("").trim().to_string()),
            ("Keyboard layout:", out(&["setxkbmap", "-query"]).lines().find_map(|l| l.strip_prefix("layout:")).unwrap_or("").trim().to_string())]), false, false, 0);
        v.pack_start(&section("Keyboards and Languages"), false, false, 0);
        v.pack_start(&input_languages(), false, false, 0);
        v.upcast()
    }

    pub(crate) fn page_access(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
        v.pack_start(&title("Make your computer easier to use"), false, false, 0);
        v.pack_start(&section("Quick access to common tools"), false, false, 0);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        for (label, cmd) in [("Start Magnifier", "kmag"), ("Start On-Screen Keyboard", "onboard"), ("Start Narrator", "orca")] {
            let b = gtk::Button::with_label(label);
            b.connect_clicked(move |_| { if !util::spawn(&[cmd]) { util::error(&format!("{label} is not installed."), &format!("Install the '{cmd}' package.")); } });
            row.pack_start(&b, false, false, 0);
        }
        v.pack_start(&row, false, false, 0);
        v.pack_start(&section("Make the computer easier to see"), false, false, 0);
        let scale = gtk::ComboBoxText::new();
        for (id, l) in [("1", "Smaller - 100% (default)"), ("1.25", "Medium - 125%"), ("1.5", "Larger - 150%")] { scale.append(Some(id), l); }
        scale.set_active_id(Some("1"));
        scale.connect_changed(|c| if let Some(id) = c.active_id() {
            let dpi = (96.0 * id.parse::<f64>().unwrap_or(1.0)) as u32;
            util::spawn(&["sh", "-c", &format!("echo 'Xft.dpi: {dpi}' | xrdb -merge")]);
        });
        v.pack_start(&scale, false, false, 0);
        v.pack_start(&text("Text size changes apply to programs started afterwards."), false, false, 0);
        v.upcast()
    }
}
