//! Mouse Properties: Buttons (switch primary and secondary, double-click speed with the folder
//! test, ClickLock), Pointers (cursor schemes with the Win7 role list, previews decoded from the
//! Xcursor files), Pointer Options (libinput speed and acceleration profile, hide while typing),
//! Wheel (lines per notch for KDE/Qt programs) and Hardware. Saved in config and re-applied at
//! logon by ~/.config/aerode/input.sh (xinput) and the xsettings (double-click time).
use super::*;
use super::super::admin::{columns, img, scrolled, spawn_tool};
use super::super::desktop::{check, group, radios, write_xsettings, Sheet};
use gtk::gdk_pixbuf::Pixbuf;
use std::cell::Cell;

fn vbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, s) }
fn hbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, s) }

// ------------------------------------------------------------------ input devices

/// (name, kind, phys, vendor id) of the mice or keyboards in /proc/bus/input/devices.
pub(super) fn input_devices(keyboards: bool) -> Vec<(String, String, String, String)> {
    let s = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
    s.split("\n\n").filter_map(|b| {
        let get = |p: &str| b.lines().find_map(|l| l.strip_prefix(p)).unwrap_or("").to_string();
        let name = get("N: Name=").trim_matches('"').to_string();
        let handlers = get("H: Handlers=");
        let ev = get("B: EV=");
        let vendor = get("I: ").split_whitespace().find_map(|f| f.strip_prefix("Vendor=")).unwrap_or("").to_string();
        let ok = if keyboards { handlers.contains("kbd") && ev == "120013" } else { handlers.contains("mouse") };
        if name.is_empty() || !ok { return None; }
        let lname = name.to_lowercase();
        let kind = if keyboards { "Keyboards".to_string() } else if lname.contains("touchpad") || lname.contains("trackpad") { "Mice and other pointing devices (touchpad)".to_string() } else { "Mice and other pointing devices".to_string() };
        Some((name, kind, get("P: Phys="), vendor))
    }).collect()
}

/// The Hardware tab of Mouse and Keyboard Properties.
pub(super) fn hardware_tab(keyboards: bool) -> gtk::Box {
    let t = vbox(8);
    t.pack_start(&lbl("D_evices:"), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let devs = input_devices(keyboards);
    for (n, k, p, v) in &devs { store.insert_with_values(None, &[(0, n), (1, k), (2, p), (3, v)]); }
    let tv = gtk::TreeView::with_model(&store);
    columns(&tv, &[("Name", 0, 220), ("Type", 1, 0)]);
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 120);
    t.pack_start(&sw, false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(4);
    g.set_column_spacing(12);
    let vals: Vec<gtk::Label> = ["Manufacturer:", "Location:", "Device status:"].iter().enumerate().map(|(i, k)| { g.attach(&lbl(k), 0, i as i32, 1, 1); let v = lbl(""); g.attach(&v, 1, i as i32, 1, 1); v }).collect();
    let props = gtk::Button::with_mnemonic("P_roperties");
    props.set_halign(gtk::Align::End);
    props.connect_clicked(|_| spawn_tool("devmgr"));
    let dp = vbox(8);
    dp.pack_start(&g, false, false, 0);
    dp.pack_start(&props, false, false, 0);
    t.pack_start(&group("Device Properties", &dp), false, false, 0);
    { let vals = vals.clone(); tv.selection().connect_changed(move |s| {
        let Some((m, it)) = s.selected() else { return };
        let phys = super::super::admin::sval(&m, &it, 2);
        let vendor = super::super::admin::sval(&m, &it, 3);
        let name = super::super::admin::sval(&m, &it, 0);
        let maker = match vendor.as_str() { "046d" => "Logitech".to_string(), "045e" => "Microsoft".into(), "1532" => "Razer".into(), "1038" => "SteelSeries".into(), "093a" => "Pixart".into(), "04f3" => "Elan".into(), "06cb" => "Synaptics".into(), "0001" => "(Standard system devices)".into(), _ => name.split_whitespace().next().unwrap_or("").to_string() };
        vals[0].set_text(&maker);
        vals[1].set_text(&if phys.contains("usb") { "on USB Input Device".to_string() } else if phys.starts_with("isa") || phys.contains("serio") { "plugged into keyboard/mouse port".into() } else if phys.is_empty() { "Unknown".into() } else { phys.clone() });
        vals[2].set_text("This device is working properly.");
    }); }
    if let Some(it) = store.iter_first() { tv.selection().select_iter(&it); }
    t
}

// ------------------------------------------------------------------ Xcursor previews

/// Decode the image of an Xcursor file closest to `size` (premultiplied ARGB → Pixbuf).
fn xcursor(path: &std::path::Path, size: u32) -> Option<Pixbuf> {
    let b = std::fs::read(path).ok()?;
    if b.get(0..4)? != b"Xcur" { return None; }
    let u = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    let ntoc = u(12)? as usize;
    let mut best: Option<(u32, usize)> = None;
    for k in 0..ntoc {
        let e = 16 + k * 12;
        if u(e)? != 0xfffd0002 { continue; }
        let (sub, pos) = (u(e + 4)?, u(e + 8)? as usize);
        let d = sub.abs_diff(size);
        if best.map(|b| d < b.0.abs_diff(size) || (d == b.0.abs_diff(size) && sub > b.0)).unwrap_or(true) { best = Some((sub, pos)); }
    }
    let (_, pos) = best?;
    let (w, h) = (u(pos + 16)? as usize, u(pos + 20)? as usize);
    if w == 0 || h == 0 || w > 512 || h > 512 { return None; }
    let mut px = Vec::with_capacity(w * h * 4);
    for i in 0..w * h {
        let v = u(pos + 36 + i * 4)?;
        let (a, r, g, bl) = ((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8);
        let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
        px.extend_from_slice(&[un(r), un(g), un(bl), a]);
    }
    let pb = Pixbuf::from_bytes(&glib::Bytes::from_owned(px), gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, w as i32, h as i32, (w * 4) as i32);
    Some(if w as u32 > size || h as u32 > size { pb.scale_simple(size as i32, size as i32, gtk::gdk_pixbuf::InterpType::Bilinear).unwrap_or(pb) } else { pb })
}

/// Cursor themes: (id, Windows-style name, its directory).
fn cursor_themes() -> Vec<(String, String, std::path::PathBuf)> {
    let mut dirs = vec![glib::user_data_dir().join("icons"), glib::home_dir().join(".icons")];
    for d in glib::system_data_dirs() { dirs.push(d.join("icons")); }
    dirs.push("/usr/share/icons".into());
    let mut out: Vec<(String, String, std::path::PathBuf)> = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let id = e.file_name().to_string_lossy().into_owned();
            if !p.join("cursors").is_dir() || id == "default" || out.iter().any(|o| o.0 == id) { continue; }
            let name = std::fs::read_to_string(p.join("index.theme")).ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("Name=").map(|v| v.trim().to_string()))).unwrap_or_else(|| id.clone());
            let name = match id.as_str() { "Aerode-cursors" => "Windows Aero (system scheme)".to_string(), "Aero7-cursors" => "Windows Aero (large) (system scheme)".to_string(), _ => format!("{name} (system scheme)") };
            out.push((id, name, p));
        }
    }
    out.sort_by(|a, b| (!a.0.starts_with("Aerode"), a.1.to_lowercase()).cmp(&(!b.0.starts_with("Aerode"), b.1.to_lowercase())));
    out
}

/// The Win7 pointer roles and the X cursor names that stand for them.
const ROLES: &[(&str, &[&str])] = &[
    ("Normal Select", &["left_ptr", "default", "arrow"]), ("Help Select", &["help", "question_arrow", "whats_this"]),
    ("Working In Background", &["left_ptr_watch", "progress"]), ("Busy", &["watch", "wait"]),
    ("Precision Select", &["crosshair", "cross"]), ("Text Select", &["xterm", "text", "ibeam"]), ("Handwriting", &["pencil"]),
    ("Unavailable", &["not-allowed", "crossed_circle", "circle"]), ("Vertical Resize", &["sb_v_double_arrow", "size_ver", "ns-resize"]),
    ("Horizontal Resize", &["sb_h_double_arrow", "size_hor", "ew-resize"]), ("Diagonal Resize 1", &["size_fdiag", "bd_double_arrow", "nwse-resize"]),
    ("Diagonal Resize 2", &["size_bdiag", "fd_double_arrow", "nesw-resize"]), ("Move", &["fleur", "move", "size_all"]),
    ("Alternate Select", &["center_ptr", "up_arrow"]), ("Link Select", &["hand2", "pointer", "hand1"]),
];

fn role_pixbuf(dir: &std::path::Path, names: &[&str], size: u32) -> Option<Pixbuf> {
    names.iter().find_map(|n| xcursor(&dir.join("cursors").join(n), size))
}

fn current_theme() -> String {
    let c = config::get().hw.mouse.cursor_theme;
    if !c.is_empty() { return c; }
    std::env::var("XCURSOR_THEME").ok().filter(|s| !s.is_empty())
        .or_else(|| gtk::Settings::default().and_then(|s| s.gtk_cursor_theme_name()).map(|s| s.to_string()))
        .unwrap_or_else(|| "Aerode-cursors".into())
}

// ------------------------------------------------------------------ the sheet

pub fn page(cp: &Rc<Cp>) -> gtk::Widget { page_tab(cp, 0) }

pub fn page_tab(cp: &Rc<Cp>, tab: u32) -> gtk::Widget {
    css();
    let sheet = Sheet::new(cp);
    let m = config::get().hw.mouse;

    // ---------------------------------------------------------------- Buttons
    let t = vbox(10);
    let bc = hbox(12);
    let bcl = vbox(6);
    let swap = check("_Switch primary and secondary buttons", m.left_handed);
    bcl.pack_start(&swap, false, false, 0);
    bcl.pack_start(&wrap("Select this check box to make the button on the right the one you use for primary functions such as selecting and dragging.", 40), false, false, 0);
    bc.pack_start(&bcl, true, true, 0);
    bc.pack_start(&img(&["input-mouse"], 48), false, false, 0);
    t.pack_start(&group("Button configuration", &bc), false, false, 0);
    sheet.watch_check(&swap);
    let dc = hbox(12);
    let dcl = vbox(6);
    dcl.pack_start(&wrap("Double-click the folder to test your setting. If the folder does not open or close, try using a slower setting.", 40), false, false, 0);
    // Win7: 900 ms (Slow) … 200 ms (Fast)
    let (srow, speed) = slider(200.0, 900.0, 10.0, 1100.0 - m.double_click.clamp(200, 900) as f64, "Slow", "Fast");
    let sp = hbox(8);
    sp.pack_start(&lbl("Spee_d:"), false, false, 0);
    sp.pack_start(&srow, true, true, 0);
    dcl.pack_start(&sp, false, false, 0);
    dc.pack_start(&dcl, true, true, 0);
    // the test folder opens and closes on a double-click at the chosen speed
    let folder = gtk::EventBox::new();
    let fimg = img(&["folder"], 48);
    folder.add(&fimg);
    folder.set_valign(gtk::Align::Center);
    {
        let (fimg, speed) = (fimg.clone(), speed.clone());
        let last = Cell::new(None::<u32>);
        let open = Cell::new(false);
        folder.connect_button_press_event(move |_, e| {
            if e.event_type() != gtk::gdk::EventType::ButtonPress { return glib::Propagation::Stop; }
            let limit = (1100.0 - speed.value()) as u32;
            let now = e.time();
            if last.get().map(|l| now.wrapping_sub(l) <= limit).unwrap_or(false) {
                open.set(!open.get());
                if let Some(p) = util::pixbuf(util::first_icon(if open.get() { &["folder-open", "folder-visiting", "folder"] } else { &["folder"] }), 48) { fimg.set_from_pixbuf(Some(&p)); }
                last.set(None);
            } else { last.set(Some(now)); }
            glib::Propagation::Stop
        });
    }
    dc.pack_start(&folder, false, false, 0);
    t.pack_start(&group("Double-click speed", &dc), false, false, 0);
    sheet.dirty();
    { let d = sheet.dirty(); speed.connect_value_changed(move |_| d()); }
    let cl = vbox(6);
    let clr = hbox(10);
    let click_lock = check("_Turn on ClickLock", m.click_lock);
    clr.pack_start(&click_lock, true, true, 0);
    let cls = gtk::Button::with_mnemonic("S_ettings...");
    cls.set_sensitive(m.click_lock);
    clr.pack_end(&cls, false, false, 0);
    cl.pack_start(&clr, false, false, 0);
    cl.pack_start(&wrap("Enables you to highlight or drag without holding down the mouse button. To set, briefly press the mouse button. To release, click the mouse button again.", 52), false, false, 0);
    t.pack_start(&group("ClickLock", &cl), false, false, 0);
    { let b = cls.clone(); click_lock.connect_toggled(move |c| b.set_sensitive(c.is_active())); }
    sheet.watch_check(&click_lock);
    let lock_ms = Rc::new(Cell::new(m.click_lock_ms));
    { let (lm, d, w) = (lock_ms.clone(), sheet.dirty(), cpwin(cp)); cls.connect_clicked(move |_| {
        let dlg = ok_cancel(Some(&w), "Settings for ClickLock");
        let c = dlg.content_area();
        let top = hbox(10);
        top.pack_start(&img(&["input-mouse"], 32), false, false, 0);
        top.pack_start(&wrap("Adjust how long you need to hold down a mouse or trackball button before your click is \"locked\"", 40), false, false, 0);
        c.pack_start(&top, false, false, 0);
        let (row, s) = slider(200.0, 2200.0, 100.0, lm.get() as f64, "Short", "Long");
        c.pack_start(&row, false, false, 0);
        dlg.show_all();
        if dlg.run() == gtk::ResponseType::Ok { lm.set(s.value() as u32); d(); }
        dlg.close();
    }); }
    sheet.tab("Buttons", &t);

    // ---------------------------------------------------------------- Pointers
    let t = vbox(8);
    let themes = cursor_themes();
    let cur_theme = current_theme();
    let sch = hbox(12);
    let schl = vbox(6);
    let scheme = gtk::ComboBoxText::new();
    scheme.append(Some(""), "(None)");
    for (id, name, _) in &themes { scheme.append(Some(id), name); }
    if !scheme.set_active_id(Some(&cur_theme)) { scheme.set_active(Some(0)); }
    schl.pack_start(&scheme, false, false, 0);
    let sbb = hbox(6);
    sbb.set_halign(gtk::Align::End);
    for l in ["Sa_ve As...", "_Delete"] { let b = gtk::Button::with_mnemonic(l); b.set_sensitive(false); sbb.pack_start(&b, false, false, 0); }
    schl.pack_start(&sbb, false, false, 0);
    sch.pack_start(&schl, true, true, 0);
    let big = gtk::Image::new();
    big.set_size_request(64, 64);
    let bigf = gtk::Frame::new(None);
    bigf.add(&big);
    sch.pack_start(&bigf, false, false, 0);
    t.pack_start(&group("Scheme", &sch), false, false, 0);
    t.pack_start(&lbl("_Customize:"), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, Pixbuf::static_type()]);
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let tr = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &tr, true);
    TreeViewColumnExt::add_attribute(&col, &tr, "text", 0);
    col.set_expand(true);
    tv.append_column(&col);
    let col2 = gtk::TreeViewColumn::new();
    let pr = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&col2, &pr, false);
    TreeViewColumnExt::add_attribute(&col2, &pr, "pixbuf", 1);
    tv.append_column(&col2);
    let sw = scrolled(&tv);
    sw.set_size_request(-1, 200);
    t.pack_start(&sw, true, true, 0);
    let fill = { let (store, big, themes) = (store.clone(), big.clone(), themes.clone()); Rc::new(move |id: &str| {
        store.clear();
        let dir = themes.iter().find(|t| t.0 == id).map(|t| t.2.clone());
        for (role, names) in ROLES {
            let pb = dir.as_ref().and_then(|d| role_pixbuf(d, names, 32));
            store.insert_with_values(None, &[(0, role), (1, &pb)]);
        }
        big.set_from_pixbuf(dir.as_ref().and_then(|d| role_pixbuf(d, ROLES[0].1, 48)).as_ref());
    }) };
    fill(&cur_theme);
    { let (f, d) = (fill.clone(), sheet.dirty()); scheme.connect_changed(move |c| { if let Some(id) = c.active_id() { f(&id); } d(); }); }
    let pr2 = hbox(10);
    let shadow = check("_Enable pointer shadow", m.cursor_shadow);
    pr2.pack_start(&shadow, true, true, 0);
    let ud = gtk::Button::with_mnemonic("Use De_fault");
    { let s = scheme.clone(); ud.connect_clicked(move |_| { s.set_active_id(Some("Aerode-cursors")); }); }
    let br = gtk::Button::with_mnemonic("_Browse...");
    br.set_sensitive(false);
    pr2.pack_end(&br, false, false, 0);
    pr2.pack_end(&ud, false, false, 0);
    t.pack_start(&pr2, false, false, 0);
    let allow = check("Allow _themes to change mouse pointers", true);
    allow.set_sensitive(false);
    t.pack_start(&allow, false, false, 0);
    sheet.watch_check(&shadow);
    sheet.tab("Pointers", &t);

    // ---------------------------------------------------------------- Pointer Options
    let t = vbox(10);
    let mo = hbox(12);
    mo.pack_start(&img(&["input-mouse"], 32), false, false, 0);
    let mol = vbox(6);
    mol.pack_start(&lbl("Select a pointer _speed:"), false, false, 0);
    // Win7's 20 positions (10 = default) on libinput's -1 … 1
    let (prow, pspeed) = slider(0.0, 20.0, 1.0, (m.speed * 10.0 + 10.0).round(), "Slow", "Fast");
    mol.pack_start(&prow, false, false, 0);
    let precision = check("Enhance pointer _precision", m.precision);
    mol.pack_start(&precision, false, false, 0);
    mo.pack_start(&mol, true, true, 0);
    t.pack_start(&group("Motion", &mo), false, false, 0);
    { let d = sheet.dirty(); pspeed.connect_value_changed(move |_| d()); }
    sheet.watch_check(&precision);
    let snap = hbox(12);
    snap.pack_start(&img(&["input-mouse"], 32), false, false, 0);
    let snapc = check("A_utomatically move pointer to the default button in a dialog box", false);
    snapc.set_sensitive(false);
    snap.pack_start(&snapc, true, true, 0);
    t.pack_start(&group("Snap To", &snap), false, false, 0);
    let vis = vbox(8);
    let tr_row = hbox(12);
    tr_row.pack_start(&img(&["input-mouse"], 32), false, false, 0);
    let trails = vbox(4);
    let trc = check("_Display pointer trails", false);
    trc.set_sensitive(false);
    trails.pack_start(&trc, false, false, 0);
    let (trs, _) = slider(0.0, 10.0, 1.0, 5.0, "Short", "Long");
    trs.set_sensitive(false);
    trails.pack_start(&trs, false, false, 0);
    tr_row.pack_start(&trails, true, true, 0);
    vis.pack_start(&tr_row, false, false, 0);
    let ht = hbox(12);
    ht.pack_start(&img(&["input-keyboard"], 32), false, false, 0);
    let can_hide = util::which("unclutter") || util::which("xbanish");
    let hide = check("_Hide pointer while typing", m.hide_typing && can_hide);
    hide.set_sensitive(can_hide);
    if !can_hide { hide.set_tooltip_text(Some("Install unclutter (unclutter-xfixes) or xbanish to hide the pointer while typing.")); }
    ht.pack_start(&hide, true, true, 0);
    vis.pack_start(&ht, false, false, 0);
    sheet.watch_check(&hide);
    let ctrl = hbox(12);
    ctrl.pack_start(&img(&["input-mouse"], 32), false, false, 0);
    let ctrlc = check("_Show location of pointer when I press the CTRL key", false);
    ctrlc.set_sensitive(false);
    ctrl.pack_start(&ctrlc, true, true, 0);
    vis.pack_start(&ctrl, false, false, 0);
    t.pack_start(&group("Visibility", &vis), false, false, 0);
    sheet.tab("Pointer Options", &t);

    // ---------------------------------------------------------------- Wheel
    let t = vbox(10);
    let vs = hbox(12);
    vs.pack_start(&img(&["input-mouse"], 32), false, false, 0);
    let vsl = vbox(6);
    vsl.pack_start(&lbl("Roll the wheel one notch to scroll:"), false, false, 0);
    let wr = radios(&["_The following number of lines at a time:", "_One screen at a time"], if m.wheel_page { 1 } else { 0 });
    let lines = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
    lines.set_value(m.wheel_lines.max(1) as f64);
    lines.set_halign(gtk::Align::Start);
    lines.set_margin_start(26);
    vsl.pack_start(&wr[0], false, false, 0);
    vsl.pack_start(&lines, false, false, 0);
    vsl.pack_start(&wr[1], false, false, 0);
    vs.pack_start(&vsl, true, true, 0);
    t.pack_start(&group("Vertical Scrolling", &vs), false, false, 0);
    { let l = lines.clone(); wr[0].connect_toggled(move |r| l.set_sensitive(r.is_active())); }
    lines.set_sensitive(!m.wheel_page);
    let hs = hbox(12);
    hs.pack_start(&img(&["input-mouse"], 32), false, false, 0);
    let hsl = vbox(6);
    hsl.pack_start(&wrap("Tilt the wheel to scroll the following number of characters at a time:", 40), false, false, 0);
    let chars = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
    chars.set_value(m.wheel_chars.max(1) as f64);
    chars.set_halign(gtk::Align::Start);
    hsl.pack_start(&chars, false, false, 0);
    hs.pack_start(&hsl, true, true, 0);
    t.pack_start(&group("Horizontal Scrolling", &hs), false, false, 0);
    for w in [&lines, &chars] { let d = sheet.dirty(); w.connect_value_changed(move |_| d()); }
    sheet.watch_check(wr[0].upcast_ref());
    sheet.tab("Wheel", &t);

    // ---------------------------------------------------------------- Hardware
    sheet.tab("Hardware", &hardware_tab(false));

    sheet.on_apply(move || {
        let theme = scheme.active_id().map(|s| s.to_string()).unwrap_or_default();
        let old = config::get().hw.mouse;
        let n = aerode::config::MouseOptions {
            left_handed: swap.is_active(), double_click: (1100.0 - speed.value()).round() as u32, click_lock: click_lock.is_active(), click_lock_ms: lock_ms.get(),
            speed: ((pspeed.value() - 10.0) / 10.0).clamp(-1.0, 1.0), precision: precision.is_active(), hide_typing: hide.is_active(),
            wheel_lines: lines.value() as u32, wheel_page: wr[1].is_active(), wheel_chars: chars.value() as u32, cursor_theme: theme.clone(), cursor_shadow: shadow.is_active(),
        };
        let shadow_on = n.cursor_shadow;
        config::update(|c| {
            c.hw.mouse = n.clone();
            // the Visual Effects list has the same switch
            c.hw.effects_off.retain(|k| k != "pointer_shadow");
            if !shadow_on { c.hw.effects_off.push("pointer_shadow".into()); }
        });
        write_input_script();
        write_xsettings();
        if theme != old.cursor_theme && !theme.is_empty() { apply_cursor_theme(&theme); }
        if n.wheel_lines != old.wheel_lines || n.wheel_page != old.wheel_page { write_wheel(&n); }
        true
    });
    let w = sheet.widget();
    { let nb = sheet.nb.clone(); glib::idle_add_local_once(move || nb.set_current_page(Some(tab))); }
    w
}

/// A new pointer scheme: for this session (root window, GTK) and the next logons
/// (~/.config/aerode/cursor-theme, read by the session script; the default icon theme for others).
fn apply_cursor_theme(id: &str) {
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(config::conf_dir().join("cursor-theme"), id);
    let d = glib::user_data_dir().join("icons/default");
    let _ = std::fs::create_dir_all(&d);
    let _ = std::fs::write(d.join("index.theme"), format!("[Icon Theme]\nInherits={id}\n"));
    if let Some(s) = gtk::Settings::default() { s.set_gtk_cursor_theme_name(Some(id)); }
    util::spawn(&["env", &format!("XCURSOR_THEME={id}"), "xsetroot", "-cursor_name", "left_ptr"]);
}

/// Lines per wheel notch for KDE and Qt programs (kdeglobals [KDE] WheelScrollLines).
fn write_wheel(m: &aerode::config::MouseOptions) {
    let path = glib::user_config_dir().join("kdeglobals");
    let s = std::fs::read_to_string(&path).unwrap_or_default();
    let v = if m.wheel_page { 20 } else { m.wheel_lines.max(1) };
    let mut out: Vec<String> = vec![];
    let (mut in_kde, mut done, mut seen) = (false, false, false);
    for l in s.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            if in_kde && !done { out.push(format!("WheelScrollLines={v}")); done = true; }
            in_kde = t == "[KDE]";
            seen |= in_kde;
        } else if in_kde && t.starts_with("WheelScrollLines=") { out.push(format!("WheelScrollLines={v}")); done = true; continue; }
        out.push(l.to_string());
    }
    if in_kde && !done { out.push(format!("WheelScrollLines={v}")); done = true; }
    if !seen && !done { if !out.is_empty() { out.push(String::new()); } out.push("[KDE]".into()); out.push(format!("WheelScrollLines={v}")); }
    let _ = std::fs::write(&path, out.join("\n") + "\n");
}

#[cfg(test)]
mod tests {
    #[test]
    fn xcursor_decodes() {
        // a 2x1 cursor: one opaque red pixel, one transparent
        let mut b = Vec::new();
        b.extend_from_slice(b"Xcur");
        for v in [16u32, 0x10000, 1, 0xfffd0002, 24, 28] { b.extend_from_slice(&v.to_le_bytes()); }
        for v in [36u32, 0xfffd0002, 24, 1, 2, 1, 0, 0, 0, 0xffff0000, 0] { b.extend_from_slice(&v.to_le_bytes()); }
        let dir = std::env::temp_dir().join(format!("aerode-xcur-test-{}", std::process::id()));
        std::fs::write(&dir, &b).unwrap();
        let p = super::xcursor(&dir, 32).unwrap();
        let _ = std::fs::remove_file(&dir);
        assert_eq!((p.width(), p.height()), (2, 1));
        let px = p.read_pixel_bytes();
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        assert_eq!(px[7], 0);
    }
}
