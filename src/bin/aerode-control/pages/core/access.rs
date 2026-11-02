//! Ease of Access Center: Quick access to common tools (Magnifier, Narrator, On-Screen
//! Keyboard, High Contrast; "Always read this section aloud" / "Always scan this section") and
//! Explore all settings, each a Win7 settings page with OK / Cancel / Apply. What is set here
//! starts again at every log on (`aerode-control --apply-access` from the session):
//! Sticky Keys, Filter Keys (Bounce or Slow Keys), Toggle Keys and Mouse Keys are the X server's
//! AccessX controls (XKB, no xkbset needed); the mouse pointer scheme and size are the Xcursor
//! theme/size (xsettings + X resources, the colour shared with Mouse > Pointers); "Turn off all
//! unnecessary animations" is the animation Visual Effects of Performance Options; "Underline
//! keyboard shortcuts" is a GTK xsetting; High Contrast is GTK's HighContrast theme (session).
//! "Get recommendations" is Win7's questionnaire; "Change administrative settings" copies the
//! settings to new user accounts (/etc/skel, pkexec).
use super::*;
use std::collections::HashMap;

type Map = HashMap<String, String>;

fn get(m: &Map, k: &str) -> bool { m.get(k).map(|v| v == "1").unwrap_or(false) }
fn val<'a>(m: &'a Map, k: &str, d: &'a str) -> &'a str { m.get(k).map(|s| s.as_str()).unwrap_or(d) }

// ---------------------------------------------------------------------------- applying

/// The AccessX controls of the core keyboard.
pub fn apply_xkb(m: &Map) -> Result<(), String> {
    use x11rb::protocol::xkb::{self, AXOption, BoolCtrl, Control, ConnectionExt as _};
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let (c, _) = x11rb::connect(None).map_err(|x| e(&x))?;
    c.xkb_use_extension(1, 0).map_err(|x| e(&x))?.reply().map_err(|x| e(&x))?;
    let dev: xkb::DeviceSpec = xkb::ID::USE_CORE_KBD.into();
    let cur = c.xkb_get_controls(dev).map_err(|x| e(&x))?.reply().map_err(|x| e(&x))?;
    let mut en = BoolCtrl::from(0u32);
    if get(m, "stickykeys") { en |= BoolCtrl::STICKY_KEYS; }
    if get(m, "mousekeys") { en |= BoolCtrl::MOUSE_KEYS | BoolCtrl::MOUSE_KEYS_ACCEL; }
    if get(m, "filterkeys") { en |= if val(m, "filter_mode", "bounce") == "bounce" { BoolCtrl::BOUNCE_KEYS } else { BoolCtrl::SLOW_KEYS }; }
    // keyboard shortcuts (SHIFT five times, hold SHIFT eight seconds) are AccessXKeys
    if val(m, "sticky_shortcut", "1") == "1" || val(m, "filter_shortcut", "1") == "1" { en |= BoolCtrl::ACCESS_X_KEYS; }
    let feedback = get(m, "togglekeys") || val(m, "sticky_sound", "1") == "1" && get(m, "stickykeys");
    if feedback { en |= BoolCtrl::ACCESS_X_FEEDBACK_MASK; }
    // the X server's AccessX timeout (on by default, 2 minutes) turns Sticky/Filter/Mouse Keys off
    // again when the keyboard is idle; Win7 keeps them until they are turned off, so it goes
    let affect = BoolCtrl::STICKY_KEYS | BoolCtrl::MOUSE_KEYS | BoolCtrl::MOUSE_KEYS_ACCEL | BoolCtrl::BOUNCE_KEYS | BoolCtrl::SLOW_KEYS | BoolCtrl::ACCESS_X_KEYS
        | BoolCtrl::ACCESS_X_FEEDBACK_MASK | BoolCtrl::ACCESS_X_TIMEOUT_MASK;
    let mut opt = u16::from(cur.access_x_option);
    let set = |o: &mut u16, f: AXOption, on: bool| { if on { *o |= u16::from(f) } else { *o &= !u16::from(f) } };
    set(&mut opt, AXOption::INDICATOR_FB, get(m, "togglekeys"));
    set(&mut opt, AXOption::LATCH_TO_LOCK, val(m, "sticky_lock", "1") == "1");
    set(&mut opt, AXOption::TWO_KEYS, val(m, "sticky_two", "1") == "1");
    set(&mut opt, AXOption::STICKY_KEYS_FB, val(m, "sticky_sound", "1") == "1");
    set(&mut opt, AXOption::SK_ACCEPT_FB, true);
    let ms = |k: &str, d: u16| m.get(k).and_then(|v| v.parse::<u16>().ok()).unwrap_or(d);
    // which parameters change: the low bits of the change mask are the BoolCtrl bits
    let change = Control::CONTROLS_ENABLED | Control::from(u32::from(BoolCtrl::SLOW_KEYS | BoolCtrl::BOUNCE_KEYS | BoolCtrl::MOUSE_KEYS_ACCEL | BoolCtrl::ACCESS_X_FEEDBACK_MASK));
    c.xkb_set_controls(dev, 0u16.into(), 0u16.into(), 0u16.into(), 0u16.into(), 0u16.into(), 0u16.into(), 0u16.into(), 0u16.into(),
        cur.mouse_keys_dflt_btn, cur.groups_wrap, AXOption::from(opt), affect, en, change, cur.repeat_delay, cur.repeat_interval,
        ms("slow_delay", 1000), ms("bounce_delay", 500), cur.mouse_keys_delay.max(10), cur.mouse_keys_interval.max(10),
        ms("mouse_accel", 100), ms("mouse_speed", 30), cur.mouse_keys_curve, cur.access_x_timeout, cur.access_x_timeout_mask, cur.access_x_timeout_values,
        cur.access_x_timeout_options_mask, cur.access_x_timeout_options_values, &cur.per_key_repeat).map_err(|x| e(&x))?
        .check().map_err(|x| e(&x))?;
    Ok(())
}

/// The pointer scheme in use: Mouse > Pointers' choice (~/.config/aerode/cursor-theme, which the
/// session exports as XCURSOR_THEME).
fn current_theme() -> String {
    std::fs::read_to_string(config::conf_dir().join("cursor-theme")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
        .or_else(|| std::env::var("XCURSOR_THEME").ok().filter(|t| !t.is_empty()))
        .unwrap_or_else(|| pointer_theme("white"))
}

/// Ease of Access' pointer colour is the Mouse pointer scheme (Win7 shares the setting): saved
/// where Mouse > Pointers keeps it, for this session and the next log ons.
fn set_pointer_scheme(scheme: &str) {
    let theme = pointer_theme(scheme);
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(config::conf_dir().join("cursor-theme"), &theme);
    config::update(|c| c.hw.mouse.cursor_theme = theme.clone());
    if let Some(s) = gtk::Settings::default() { s.set_gtk_cursor_theme_name(Some(&theme)); }
}

/// The Visual Effects that "Turn off all unnecessary animations" turns off (Win7 shares them
/// with System Properties > Performance Options).
const ANIMATIONS: &[&str] = &["animate_controls", "animate_minmax", "animate_taskbar", "menu_fade", "tooltip_fade", "menu_item_fade", "combo_slide", "list_smooth"];

/// Are all the animation effects off?
fn animations_off() -> bool { let hw = config::get().hw; ANIMATIONS.iter().all(|k| !hw.effect(k)) }

/// "Turn off all unnecessary animations": the Performance Options effects, then the xsettings
/// that come from them (Gtk/EnableAnimations) for this session and the next log ons.
fn set_animations(off: bool) {
    config::update(|c| {
        c.hw.effects_off.retain(|k| !ANIMATIONS.contains(&k.as_str()));
        if off { c.hw.effects_off.extend(ANIMATIONS.iter().map(|s| s.to_string())); }
        c.hw.visual_effects = "custom".into();
    });
    super::super::desktop::write_xsettings();
    if let Some(s) = gtk::Settings::default() { s.set_gtk_enable_animations(!off); }
    util::spawn(&["aerode", "reload"]);
}

/// Live xsettings and X resources (pointer size, mnemonics); the pointer size is also exported
/// by the session at log on (~/.config/aerode/cursor-size → XCURSOR_SIZE).
pub fn apply_xsettings(m: &Map) {
    let size = match val(m, "cursor_size", "regular") { "large" => 48, "xl" => 64, _ => 32 };
    let theme = current_theme();
    let keys = vec![
        ("Gtk/CursorThemeName".to_string(), format!("\"{theme}\"")),
        ("Gtk/CursorThemeSize".to_string(), size.to_string()),
        ("Gtk/AutoMnemonics".to_string(), if get(m, "underline") { "0".into() } else { "1".into() }),
    ];
    let dir = config::conf_dir();
    let live = dir.join("xsettingsd.conf");
    if let Ok(cur) = std::fs::read_to_string(&live) {
        let mut out: Vec<String> = cur.lines().filter(|l| !keys.iter().any(|(k, _)| l.split_whitespace().next() == Some(k))).map(String::from).collect();
        out.extend(keys.iter().map(|(k, v)| format!("{k} {v}")));
        let _ = std::fs::write(&live, out.join("\n") + "\n");
        hup_xsettingsd();
    }
    // Xcursor for X programs and the window manager's own cursors
    let res = dir.join("Xresources-access");
    let _ = std::fs::write(&res, format!("Xcursor.theme: {theme}\nXcursor.size: {size}\n"));
    let _ = run(&["xrdb", "-merge", &res.to_string_lossy()]);
    let cs = dir.join("cursor-size");
    if size == 32 { let _ = std::fs::remove_file(&cs); } else { let _ = std::fs::write(&cs, format!("{size}\n")); }
    util::spawn(&["env", &format!("XCURSOR_THEME={theme}"), &format!("XCURSOR_SIZE={size}"), "xsetroot", "-cursor_name", "left_ptr"]);
    if let Some(s) = gtk::Settings::default() { s.set_gtk_cursor_theme_size(size); }
    // High Contrast: GTK's theme, chosen by the session at log on (GTK_THEME)
    let hc = dir.join("high-contrast");
    if get(m, "highcontrast") { let _ = std::fs::write(&hc, if val(m, "hc_theme", "black") == "white" { "HighContrast\n" } else { "HighContrastInverse\n" }); } else { let _ = std::fs::remove_file(&hc); }
}

/// SIGHUP the xsettingsd serving our display (it re-reads its config).
fn hup_xsettingsd() {
    let disp = std::env::var("DISPLAY").unwrap_or_default();
    let Ok(rd) = std::fs::read_dir("/proc") else { return };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        if std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| c.trim() != "xsettingsd").unwrap_or(true) { continue; }
        let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        if env.split(|b| *b == 0).any(|kv| kv == format!("DISPLAY={disp}").as_bytes()) { unsafe { libc::kill(pid, libc::SIGHUP); } }
    }
}

const MAGNIFIERS: &[&str] = &["magnus", "kmag", "xzoom", "xmag"];
const NARRATORS: &[&str] = &["orca"];
const KEYBOARDS: &[&str] = &["onboard", "florence", "xvkbd", "matchbox-keyboard"];

fn start_tool(cands: &[&str], what: &str, pkg: &str) -> bool {
    if cands.iter().any(|c| util::spawn(&[c])) { return true; }
    util::error(&format!("{what} is not installed."), &format!("Install {pkg}."));
    false
}

/// `aerode-control --apply-access`: what was chosen in Ease of Access, at log on.
pub fn apply_at_logon() -> glib::ExitCode {
    let m = config::get().ease_of_access;
    if m.is_empty() { return glib::ExitCode::SUCCESS; }
    // xsettingsd was started just before us: let it install its SIGHUP handler first
    std::thread::sleep(std::time::Duration::from_secs(2));
    if let Err(e) = apply_xkb(&m) { eprintln!("aerode-control: AccessX: {e}"); }
    if gtk::init().is_ok() { apply_xsettings(&m); }
    for (k, c) in [("magnifier", MAGNIFIERS), ("narrator", NARRATORS), ("osk", KEYBOARDS)] {
        if get(&m, k) { let _ = c.iter().any(|c| util::spawn(&[c])); }
    }
    glib::ExitCode::SUCCESS
}

/// "See also: Learn about additional assistive technologies online" (wrapped to the task pane's width).
fn learn_more(cp: &Rc<Cp>) {
    let v = vbox(2);
    let h = label("See also", "dim");
    v.pack_start(&h, false, false, 2);
    let l = link("Learn about additional assistive technologies online", || { util::open_uri("https://wiki.archlinux.org/title/Accessibility"); });
    if let Some(lab) = l.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { lab.set_max_width_chars(24); }
    v.pack_start(&l, false, false, 0);
    cp.side.pack_end(&v, false, false, 8);
}

fn speak(t: &str) { for c in [&["spd-say", "--wait"][..], &["espeak-ng"], &["espeak"]] { let mut a = c.to_vec(); a.push(t); if util::spawn(&a) { return; } } }

// ------------------------------------------------------------------- pointer previews

/// The arrow (`left_ptr`) of an installed Xcursor theme at the nominal size nearest `size`.
fn cursor_image(theme: &str, size: u32) -> Option<Pixbuf> {
    let mut dirs = vec![glib::user_data_dir().join("icons"), glib::home_dir().join(".icons")];
    dirs.extend(glib::system_data_dirs().into_iter().map(|d| d.join("icons")));
    let data = dirs.iter().find_map(|d| std::fs::read(d.join(theme).join("cursors/left_ptr")).ok())?;
    let (w, h, rgba) = xcursor_image(&data, size)?;
    Some(Pixbuf::from_bytes(&glib::Bytes::from_owned(rgba), gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, w as i32, h as i32, w as i32 * 4))
}

/// Decode one image of an Xcursor file: (width, height, straight RGBA) cropped to its visible
/// part.
pub fn xcursor_image(d: &[u8], size: u32) -> Option<(usize, usize, Vec<u8>)> {
    let u = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?)) };
    if d.get(0..4)? != b"Xcur" { return None; }
    let (hsize, ntoc) = (u(4)? as usize, u(12)? as usize);
    let mut best: Option<(u32, usize)> = None;
    for i in 0..ntoc.min(256) {
        let o = hsize + i * 12;
        if u(o)? != 0xfffd_0002 { continue; }
        let (sub, pos) = (u(o + 4)?, u(o + 8)? as usize);
        if best.map(|(s, _)| sub.abs_diff(size) < s.abs_diff(size)).unwrap_or(true) { best = Some((sub, pos)); }
    }
    let (_, p) = best?;
    let (w, h) = (u(p + 16)? as usize, u(p + 20)? as usize);
    if w == 0 || h == 0 || w > 512 || h > 512 { return None; }
    let px = d.get(p + 36..p + 36 + w * h * 4)?;
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h { for x in 0..w { if px[(y * w + x) * 4 + 3] > 0 { x0 = x0.min(x); y0 = y0.min(y); x1 = x1.max(x + 1); y1 = y1.max(y + 1); } } }
    if x1 <= x0 || y1 <= y0 { return None; }
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        for x in x0..x1 {
            // premultiplied BGRA (a little-endian ARGB word) → straight RGBA
            let c = &px[(y * w + x) * 4..(y * w + x) * 4 + 4];
            let a = c[3] as u32;
            let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a / 2) / a).min(255) as u8 };
            out.extend_from_slice(&[un(c[2]), un(c[1]), un(c[0]), c[3]]);
        }
    }
    Some((x1 - x0, y1 - y0, out))
}

/// Win7's pointer schemes in "Make the mouse easier to use": (size key, colour key, name).
const POINTERS: &[(&str, &str, &str)] = &[
    ("regular", "white", "Regular White"), ("regular", "black", "Regular Black"), ("regular", "inverting", "Regular Inverting"),
    ("large", "white", "Large White"), ("large", "black", "Large Black"), ("large", "inverting", "Large Inverting"),
    ("xl", "white", "Extra Large White"), ("xl", "black", "Extra Large Black"), ("xl", "inverting", "Extra Large Inverting"),
];

/// The cursor theme behind a colour scheme: Windows Aero (the pointers extracted from the ISO)
/// for white, Adwaita for black.
fn pointer_theme(scheme: &str) -> String {
    if scheme == "black" { return "Adwaita".into(); }
    let have = |t: &str| glib::user_data_dir().join("icons").join(t).join("cursors").is_dir() || glib::system_data_dirs().iter().any(|d| d.join("icons").join(t).join("cursors").is_dir());
    if have("Aerode-cursors") { return "Aerode-cursors".into(); }
    std::env::var("XCURSOR_THEME").ok().filter(|t| !t.is_empty() && t != "Adwaita").unwrap_or_else(|| "default".into())
}

// ---------------------------------------------------------------------------- the page

impl Cp {
    pub(crate) fn page_access(self: &Rc<Self>) -> gtk::Widget {
        css();
        if let Some(s) = sub(self, "access") {
            if s == "questions" { return questionnaire(self); }
            if s == "admin" { return admin_page(self); }
            if s == "recommended" { return recommended(self); }
            if let Some(w) = settings_page(self, &s) { return w; }
        }
        { let c = self.clone(); side_shield(self, "Change administrative settings", move || go(&c, "access/admin")); }
        learn_more(self);
        let v = vbox(8);
        v.pack_start(&title("Make your computer easier to use"), false, false, 0);
        let qa = vbox(6);
        qa.style_context().add_class("eoa-box");
        qa.pack_start(&label("Quick access to common tools", "eoa-h"), false, false, 0);
        qa.pack_start(&label("Use the tools in this section to help you get started.", ""), false, false, 0);
        qa.pack_start(&label("Windows can read and scan this list automatically.  Press the SPACEBAR to select the highlighted tool.", ""), false, false, 0);
        let m = config::get().ease_of_access;
        let read = check("Always _read this section aloud", get(&m, "read_aloud"));
        let scan = check("Always _scan this section", get(&m, "scan"));
        let checks = hbox(20);
        checks.pack_start(&read, false, false, 0);
        checks.pack_start(&scan, false, false, 0);
        qa.pack_start(&checks, false, false, 0);
        let grid = gtk::Grid::new();
        grid.set_column_spacing(40);
        grid.set_row_spacing(4);
        grid.set_margin_start(10);
        let tool = |icon: &'static [&'static str], t: &str, f: Box<dyn Fn()>| {
            let b = gtk::Button::new();
            b.style_context().add_class("eoa-tool");
            b.set_relief(gtk::ReliefStyle::None);
            let h = hbox(8);
            h.pack_start(&img(icon, 32), false, false, 0);
            h.pack_start(&gtk::Label::new(Some(t)), false, false, 0);
            b.add(&h);
            b.connect_clicked(move |_| f());
            b
        };
        let tools = [
            tool(&["system-search", "zoom-in"], "Start Magnifier", Box::new(|| { start_tool(MAGNIFIERS, "Magnifier", "magnus, kmag or xzoom"); })),
            tool(&["audio-speakers", "audio-volume-high"], "Start Narrator", Box::new(|| { start_tool(NARRATORS, "Narrator", "orca"); })),
            tool(&["input-keyboard", "preferences-desktop-keyboard"], "Start On-Screen Keyboard", Box::new(|| { start_tool(KEYBOARDS, "On-Screen Keyboard", "onboard"); })),
            { let c = self.clone(); tool(&["preferences-desktop-theme", "preferences-desktop-display"], "Set up High Contrast", Box::new(move || go(&c, "access/see"))) },
        ];
        for (i, b) in tools.iter().enumerate() { grid.attach(b, (i % 2) as i32, (i / 2) as i32, 1, 1); }
        qa.pack_start(&grid, false, false, 0);
        v.pack_start(&qa, false, false, 0);
        let ns = hbox(4);
        ns.set_margin_start(2);
        ns.pack_start(&label("Not sure where to start?", ""), false, false, 0);
        { let c = self.clone(); ns.pack_start(&link("Get recommendations to make your computer easier to use", move || go(&c, "access/questions")), false, false, 0); }
        v.pack_start(&ns, false, false, 4);
        // read aloud / scan, like Win7 does when the page opens
        let texts = ["Start Magnifier", "Start Narrator", "Start On-Screen Keyboard", "Set up High Contrast"];
        if get(&m, "read_aloud") { speak(&format!("Quick access to common tools. {}", texts.join(". "))); }
        if get(&m, "scan") {
            let tl: Vec<gtk::Button> = tools.to_vec();
            let i = Rc::new(Cell::new(0usize));
            let v2 = v.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(1500), move || {
                if v2.parent().is_none() && v2.toplevel().map(|t| !t.is_toplevel()).unwrap_or(true) { return glib::ControlFlow::Break; }
                let k = i.get() % tl.len();
                tl[k].grab_focus();
                i.set(k + 1);
                glib::ControlFlow::Continue
            });
        }
        for (c, k) in [(&read, "read_aloud"), (&scan, "scan")] {
            c.connect_toggled(move |c| { let on = c.is_active(); config::update(|cf| { cf.ease_of_access.insert(k.into(), if on { "1".into() } else { "0".into() }); }); });
        }
        v.pack_start(&label("Explore all settings", "eoa-h"), false, false, 6);
        v.pack_start(&label("When you select these settings, they will automatically start each time you log on.", ""), false, false, 0);
        let list = vbox(10);
        list.set_margin_start(8);
        for (icon, t, d, page) in [
            (&["video-display", "preferences-desktop-display"][..], "Use the computer without a display", "Optimize for blindness", "access/nodisplay"),
            (&["preferences-desktop-display", "video-display"][..], "Make the computer easier to see", "Optimize visual display", "access/see"),
            (&["input-keyboard", "input-mouse"][..], "Use the computer without a mouse or keyboard", "Set up alternative input devices", "access/noinput"),
            (&["input-mouse"][..], "Make the mouse easier to use", "Adjust settings for the mouse or other pointing devices", "access/mouse"),
            (&["input-keyboard", "preferences-desktop-keyboard"][..], "Make the keyboard easier to use", "Adjust settings for the keyboard", "access/keyboard"),
            (&["audio-speakers", "audio-volume-high"][..], "Use text or visual alternatives for sounds", "Set up alternatives for sounds", "access/sound"),
            (&["preferences-desktop-accessibility"][..], "Make it easier to focus on tasks", "Adjust settings for reading and typing", "access/focus"),
        ] {
            let c = self.clone();
            list.pack_start(&task(icon, t, d, move || go(&c, page)), false, false, 0);
        }
        v.pack_start(&list, false, false, 0);
        v.upcast()
    }
}

// ------------------------------------------------------------------------ settings pages

/// One line of a settings page.
enum It {
    /// blue heading
    H(&'static str),
    Check(&'static str, &'static str),
    /// a check box for something this computer cannot do (shown greyed, like Win7 without the hardware)
    Off(&'static str, &'static str),
    Radio(&'static str, &'static [(&'static str, &'static str)]),
    Combo(&'static str, &'static str, &'static [(&'static str, &'static str)]),
    Link(&'static str, &'static str),
    Act(&'static str, fn()),
    Text(&'static str),
    /// the 3 × 3 grid of pointer schemes with their pictures (cursor_size + cursor_scheme)
    Pointers,
}
use It::*;

const SECS: &[(&str, &str)] = &[("500", "0.5 seconds"), ("1000", "1.0 seconds"), ("1500", "1.5 seconds"), ("2000", "2.0 seconds")];

fn page_items(id: &str) -> Option<(&'static str, &'static str, Vec<It>)> {
    let no_wm = "The window manager does not offer this yet.";
    Some(match id {
        "see" => ("Make the computer easier to see", "Make the computer easier to see", vec![
            Text("When you select these settings, they will automatically start each time you log on."),
            H("High Contrast"),
            Check("highcontrast", "Turn on High Contrast"),
            Radio("hc_theme", &[("black", "High Contrast Black"), ("white", "High Contrast White")]),
            Text("Programs started after you log on again use High Contrast."),
            Off("Turn on or off High Contrast when left ALT + left SHIFT + PRINT SCREEN is pressed", no_wm),
            H("Hear text and descriptions read aloud"),
            Check("narrator", "Turn on Narrator"),
            Off("Turn on Audio Description", "Audio Description is not available."),
            H("Make things on the screen larger"),
            Link("Change the size of text and icons", "display"),
            Check("magnifier", "Turn on Magnifier"),
            H("Make things on the screen easier to see"),
            Link("Adjust the color and transparency of the window borders", "window-color"),
            Link("Fine tune display effects", "visualeffects"),
            Off("Make the focus rectangle thicker", no_wm),
            Check("noanim", "Turn off all unnecessary animations (when possible)"),
            Off("Remove background images (where available)", "The desktop background is set in Personalization."),
        ]),
        "nodisplay" => ("Use the computer without a display", "Use the computer without a display", vec![
            Text("When you select these settings, they will automatically start each time you log on."),
            H("Hear text read aloud"),
            Check("narrator", "Turn on Narrator"),
            Act("Set up Text to Speech", || { if !util::spawn(&["spd-conf"]) { util::open_uri("https://wiki.archlinux.org/title/Speech_dispatcher"); } }),
            Off("Turn on Audio Description", "Audio Description is not available."),
            H("Adjust time limits and flashing visuals"),
            Check("noanim", "Turn off all unnecessary animations (when possible)"),
        ]),
        "noinput" => ("Use the computer without a mouse or keyboard", "Use the computer without a mouse or keyboard", vec![
            Text("When you select these settings, they will automatically start each time you log on."),
            H("Type using a pointing device"),
            Check("osk", "Use On-Screen Keyboard"),
            H("Use Speech Recognition"),
            Off("Use Speech Recognition", "Speech Recognition is not available on this computer."),
        ]),
        "mouse" => ("Make the mouse easier to use", "Make the mouse easier to use", vec![
            Text("When you select these settings, they will automatically start each time you log on."),
            H("Mouse pointers"),
            Text("Change the color and size of mouse pointers."),
            Pointers,
            H("Control the mouse with the keyboard"),
            Check("mousekeys", "Turn on Mouse Keys"),
            Text("Use the numeric keypad to move the mouse around the screen."),
            Link("Set up Mouse Keys", "access/mousekeys"),
            H("Make it easier to manage windows"),
            Off("Activate a window by hovering over it with the mouse", no_wm),
            Off("Prevent windows from being automatically arranged when moved to the edge of the screen", no_wm),
            H("See also"),
            Link("Mouse settings", "mouse"),
        ]),
        "keyboard" => ("Make the keyboard easier to use", "Make the keyboard easier to use", vec![
            Text("When you select these settings, they will automatically start each time you log on."),
            H("Control the mouse with the keyboard"),
            Check("mousekeys", "Turn on Mouse Keys"),
            Link("Set up Mouse Keys", "access/mousekeys"),
            H("Make it easier to type"),
            Check("stickykeys", "Turn on Sticky Keys"),
            Link("Set up Sticky Keys", "access/stickykeys"),
            Check("togglekeys", "Turn on Toggle Keys"),
            Text("Hear a tone when you press CAPS LOCK, NUM LOCK, or SCROLL LOCK."),
            Check("filterkeys", "Turn on Filter Keys"),
            Text("Ignore or slow down brief or repeated keystrokes and adjust keyboard repeat rates."),
            Link("Set up Filter Keys", "access/filterkeys"),
            H("Make it easier to use keyboard shortcuts"),
            Check("underline", "Underline keyboard shortcuts and access keys"),
            H("See also"),
            Link("Add a Dvorak keyboard and change other keyboard input settings", "region"),
        ]),
        "sound" => ("Use text or visual alternatives for sounds", "Use text or visual alternatives for sounds", vec![
            Text("When you select these settings, they will automatically start each time you log on."),
            H("Use visual cues instead of sounds"),
            Off("Turn on visual notifications for sounds (Sound Sentry)", no_wm),
            H("Turn on text captions for spoken dialog"),
            Off("Turn on text captions for spoken dialog (when available)", "Programs on Linux show captions themselves."),
            H("See also"),
            Link("Audio Devices and Sound Themes", "sound"),
        ]),
        "focus" => ("Make it easier to focus on tasks", "Make it easier to focus on tasks", vec![
            Text("When you select these settings, they will automatically start each time you log on."),
            H("Reduce distractions"),
            Check("narrator", "Turn on Narrator"),
            Check("noanim", "Turn off all unnecessary animations (when possible)"),
            Off("Remove background images (where available)", "The desktop background is set in Personalization."),
            H("Make it easier to type"),
            Check("stickykeys", "Turn on Sticky Keys"),
            Link("Set up Sticky Keys", "access/stickykeys"),
            Check("togglekeys", "Turn on Toggle Keys"),
            Check("filterkeys", "Turn on Filter Keys"),
            Link("Set up Filter Keys", "access/filterkeys"),
        ]),
        "mousekeys" => ("Set up Mouse Keys", "Set up Mouse Keys", vec![
            Check("mousekeys", "Turn on Mouse Keys"),
            H("Keyboard shortcut"),
            Text("Press SHIFT + NUM LOCK to turn Mouse Keys on or off when the keyboard layout allows it (Keyboard: keypad pointer keys option)."),
            H("Pointer speed"),
            Combo("mouse_speed", "Top speed:", &[("10", "Low"), ("30", "Medium"), ("60", "High"), ("120", "Very high")]),
            Combo("mouse_accel", "Acceleration:", &[("300", "Slow"), ("100", "Medium"), ("40", "Fast")]),
            H("Other settings"),
            Off("Use Mouse Keys when NUM LOCK is on", "The X server uses the keypad for Mouse Keys while they are on."),
            Off("Display the Mouse Keys icon on the taskbar", "The notification area has no Mouse Keys icon yet."),
        ]),
        "stickykeys" => ("Set up Sticky Keys", "Set up Sticky Keys", vec![
            Check("stickykeys", "Turn on Sticky Keys"),
            H("Keyboard shortcut"),
            Check("sticky_shortcut", "Turn on Sticky Keys when SHIFT is pressed five times"),
            H("Options"),
            Check("sticky_lock", "Lock modifier keys when pressed twice in a row"),
            Check("sticky_two", "Turn off Sticky Keys when two keys are pressed at once"),
            H("Feedback"),
            Check("sticky_sound", "Play a sound when modifier keys are pressed"),
        ]),
        "filterkeys" => ("Set up Filter Keys", "Set up Filter Keys", vec![
            Check("filterkeys", "Turn on Filter Keys"),
            H("Keyboard shortcut"),
            Check("filter_shortcut", "Turn on Filter Keys when right SHIFT is pressed for 8 seconds"),
            H("Filter options"),
            Radio("filter_mode", &[("bounce", "Turn on Bounce Keys"), ("slow", "Turn on Repeat Keys and Slow Keys")]),
            Combo("bounce_delay", "How long should the computer wait before accepting keystrokes? (Bounce Keys)", SECS),
            Combo("slow_delay", "How long should the user hold down the key? (Slow Keys)", SECS),
        ]),
        _ => return None,
    })
}

/// Defaults where Win7's are "on".
fn default_of(k: &str) -> &'static str {
    match k {
        "sticky_shortcut" | "sticky_lock" | "sticky_two" | "sticky_sound" | "filter_shortcut" => "1",
        "hc_theme" => "black", "cursor_scheme" => "white", "cursor_size" => "regular", "filter_mode" => "bounce",
        "mouse_speed" => "30", "mouse_accel" => "100", "bounce_delay" => "500", "slow_delay" => "1000",
        _ => "0",
    }
}

fn settings_page(cp: &Rc<Cp>, id: &str) -> Option<gtk::Widget> {
    let (crumb, heading, items) = page_items(id)?;
    Some(render(cp, crumb, heading, items, &Map::new()))
}

/// A settings page: `items`, starting from the saved settings with `preset` over them (the
/// questionnaire's recommendations), OK / Cancel / Apply.
fn render(cp: &Rc<Cp>, crumb: &str, heading: &str, items: Vec<It>, preset: &Map) -> gtk::Widget {
    sub_crumb(cp, crumb);
    learn_more(cp);
    let mut saved = config::get().ease_of_access;
    // the animations check box shows the Performance Options state (they are one setting)
    saved.insert("noanim".into(), if animations_off() { "1".into() } else { "0".into() });
    let mut start = saved.clone();
    start.extend(preset.iter().map(|(k, v)| (k.clone(), v.clone())));
    let state: Rc<RefCell<Map>> = Rc::new(RefCell::new(start));
    let v = vbox(6);
    v.pack_start(&title(heading), false, false, 0);
    let apply_b = gtk::Button::with_mnemonic("_Apply");
    apply_b.set_sensitive(!preset.is_empty());
    let dirty = { let a = apply_b.clone(); move || a.set_sensitive(true) };
    let body = vbox(4);
    body.set_margin_start(10);
    for it in items {
        match it {
            H(t) => { let l = label(t, "eoa-h"); l.set_margin_top(10); body.pack_start(&l, false, false, 0); body.pack_start(&{ let r = hbox(0); r.style_context().add_class("ac-rule"); r }, false, false, 2); }
            Text(t) => {
                let l = label(t, "cp-task-desc");
                l.set_max_width_chars(80);
                // a page's introduction is flush left, notes under a setting are indented
                if !["When you select", "Based on your answers", "No settings are"].iter().any(|p| t.starts_with(p)) { l.set_margin_start(24); }
                body.pack_start(&l, false, false, 0);
            }
            Check(k, t) => {
                let on = state.borrow().get(k).map(|s| s == "1").unwrap_or(default_of(k) == "1");
                let c = check(t, on);
                let (s, d) = (state.clone(), dirty.clone());
                c.connect_toggled(move |c| { s.borrow_mut().insert(k.into(), if c.is_active() { "1".into() } else { "0".into() }); d(); });
                body.pack_start(&c, false, false, 0);
            }
            Off(t, why) => { let c = check(t, false); c.set_sensitive(false); c.set_tooltip_text(Some(why)); body.pack_start(&c, false, false, 0); }
            Radio(k, opts) => {
                let cur = state.borrow().get(k).cloned().unwrap_or_else(|| default_of(k).into());
                let labels: Vec<&str> = opts.iter().map(|o| o.1).collect();
                let rs = radios(&labels, opts.iter().position(|o| o.0 == cur).unwrap_or(0));
                for (r, (val, _)) in rs.iter().zip(opts.iter()) {
                    r.set_margin_start(20);
                    let (s, d) = (state.clone(), dirty.clone());
                    r.connect_toggled(move |r| if r.is_active() { s.borrow_mut().insert(k.into(), val.to_string()); d(); });
                    body.pack_start(r, false, false, 0);
                }
            }
            Combo(k, t, opts) => {
                let h = hbox(8);
                h.set_margin_start(20);
                let l = label(t, ""); l.set_line_wrap(false);
                h.pack_start(&l, false, false, 0);
                let cur = state.borrow().get(k).cloned().unwrap_or_else(|| default_of(k).into());
                let c = combo(opts, &cur);
                let (s, d) = (state.clone(), dirty.clone());
                c.connect_changed(move |c| { if let Some(id) = c.active_id() { s.borrow_mut().insert(k.into(), id.to_string()); d(); } });
                h.pack_start(&c, false, false, 0);
                body.pack_start(&h, false, false, 0);
            }
            Link(t, page) => {
                let c = cp.clone();
                // a few links open a shell dialog or the Performance Options sheet, not a page
                let l = link(t, move || match page {
                    "window-color" => { util::spawn(&["aerode", "window-color"]); }
                    "visualeffects" => super::super::hw::visual_effects(&c),
                    p => go(&c, p),
                });
                l.set_margin_start(20);
                body.pack_start(&l, false, false, 0);
            }
            Act(t, f) => { let l = link(t, f); l.set_margin_start(20); body.pack_start(&l, false, false, 0); }
            Pointers => {
                let g = gtk::Grid::new();
                g.set_column_spacing(26);
                g.set_row_spacing(6);
                g.set_margin_start(20);
                // the colour follows the pointer scheme in use when Ease of Access never set it
                let cur = (state.borrow().get("cursor_size").cloned().unwrap_or_else(|| "regular".into()),
                    state.borrow().get("cursor_scheme").cloned().unwrap_or_else(|| if current_theme() == "Adwaita" { "black".into() } else { "white".into() }));
                let mut first: Option<gtk::RadioButton> = None;
                for (i, (size, scheme, name)) in POINTERS.iter().enumerate() {
                    let r = match &first { Some(f) => gtk::RadioButton::with_label_from_widget(f, name), None => gtk::RadioButton::with_label(name) };
                    if first.is_none() { first = Some(r.clone()); }
                    let cell = hbox(8);
                    cell.pack_start(&r, false, false, 0);
                    let px = match *size { "large" => 26, "xl" => 33, _ => 20 };
                    // the theme's own arrow; Win7's inverting pointers have no X equivalent
                    let pic = if *scheme == "inverting" { None } else { cursor_image(&pointer_theme(scheme), if *size == "regular" { 32 } else { 48 }) };
                    let im = gtk::Image::new();
                    if let Some(p) = pic.and_then(|p| { let h = px; let w = (p.width() * h / p.height().max(1)).max(1); p.scale_simple(w, h, gtk::gdk_pixbuf::InterpType::Bilinear) }) { im.set_from_pixbuf(Some(&p)); }
                    im.set_size_request(24, 34);
                    cell.pack_start(&im, false, false, 0);
                    if *scheme == "inverting" { r.set_sensitive(false); r.set_tooltip_text(Some("The X server has no inverting pointers.")); }
                    if (cur.0.as_str(), cur.1.as_str()) == (*size, *scheme) { r.set_active(true); }
                    let (s, d) = (state.clone(), dirty.clone());
                    r.connect_toggled(move |r| if r.is_active() {
                        s.borrow_mut().insert("cursor_size".into(), size.to_string());
                        s.borrow_mut().insert("cursor_scheme".into(), scheme.to_string());
                        d();
                    });
                    g.attach(&cell, (i % 3) as i32, (i / 3) as i32, 1, 1);
                }
                body.pack_start(&g, false, false, 4);
            }
        }
    }
    v.pack_start(&body, false, false, 0);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    let apply: Rc<dyn Fn()> = {
        let (s, ab) = (state.clone(), apply_b.clone());
        let before = Rc::new(RefCell::new(saved));
        Rc::new(move || {
            let m = s.borrow().clone();
            let old = before.borrow().clone();
            config::update(|c| c.ease_of_access = m.clone());
            if let Err(e) = apply_xkb(&m) { util::error("The keyboard settings could not be changed.", &e); }
            if let Some(s) = m.get("cursor_scheme").filter(|s| Some(*s) != old.get("cursor_scheme")) { set_pointer_scheme(s); }
            if get(&m, "noanim") != get(&old, "noanim") { set_animations(get(&m, "noanim")); }
            apply_xsettings(&m);
            // tools turned on now start now (Win7 starts them on OK)
            for (k, c, what, pkg) in [("magnifier", MAGNIFIERS, "Magnifier", "magnus, kmag or xzoom"), ("narrator", NARRATORS, "Narrator", "orca"), ("osk", KEYBOARDS, "On-Screen Keyboard", "onboard")] {
                if get(&m, k) && !get(&old, k) { start_tool(c, what, pkg); }
            }
            *before.borrow_mut() = m;
            ab.set_sensitive(false);
        })
    };
    { let a = apply.clone(); apply_b.connect_clicked(move |_| a()); }
    { let (a, c, ab) = (apply.clone(), cp.clone(), apply_b.clone()); ok.connect_clicked(move |_| { if ab.is_sensitive() { a(); } back(&c, "access"); }); }
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "access")); }
    v.pack_start(&button_row(&[&ok, &cancel, &apply_b]), false, false, 0);
    v.upcast()
}

// ------------------------------------------------------------- get recommendations

/// Win7's questionnaire: (page title, statements as (answer key, text)).
const QUESTIONS: &[(&str, &[(&str, &str)])] = &[
    ("Eyesight (1 of 5)", &[
        ("q_tv", "Images and text on TV are difficult to see (even when I'm wearing glasses or contact lenses)"),
        ("q_light", "Lighting conditions make it difficult to see images on my monitor"),
        ("q_blind", "I am blind"),
        ("q_vision", "I have another type of vision impairment (even if glasses correct it)"),
    ]),
    ("Dexterity (2 of 5)", &[
        ("q_pens", "Pens and pencils are difficult to use"),
        ("q_nokbd", "I do not use a keyboard"),
        ("q_nomouse", "I do not use a mouse"),
        ("q_dexterity", "I have a condition that makes moving my hands or arms difficult"),
    ]),
    ("Hearing (3 of 5)", &[
        ("q_conv", "Conversations are difficult to hear (even with a hearing aid)"),
        ("q_deaf", "I am deaf"),
        ("q_hearing", "I have another type of hearing impairment"),
    ]),
    ("Speech (4 of 5)", &[
        ("q_speech", "I have a speech impairment"),
    ]),
    ("Reasoning (5 of 5)", &[
        ("q_learning", "I have a learning disability, such as dyslexia"),
        ("q_concentrate", "I find it difficult to concentrate"),
    ]),
];

/// The settings Win7 recommends for the answers (key → "1" or a value).
pub fn recommend(answers: &[&str]) -> Map {
    let mut m = Map::new();
    let has = |k: &str| answers.contains(&k);
    let mut on = |k: &str| { m.insert(k.to_string(), "1".to_string()); };
    if has("q_tv") || has("q_vision") { on("magnifier"); on("noanim"); }
    if has("q_light") { on("highcontrast"); }
    if has("q_blind") { on("narrator"); on("noanim"); }
    if has("q_nokbd") { on("osk"); }
    if has("q_nomouse") { on("mousekeys"); }
    if has("q_pens") || has("q_dexterity") { on("stickykeys"); on("filterkeys"); }
    if has("q_learning") || has("q_concentrate") { on("noanim"); on("narrator"); }
    if has("q_tv") || has("q_vision") || has("q_light") { m.insert("cursor_size".into(), "large".into()); }
    m
}

thread_local!(static ANSWERS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) });

fn questionnaire(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Get recommendations to make your computer easier to use");
    learn_more(cp);
    let v = vbox(6);
    v.pack_start(&title("Get recommendations to make your computer easier to use"), false, false, 0);
    v.pack_start(&text("Answer the following questions to get recommendations for settings that can make your computer easier to see, hear, and use. For each question, select all the statements that apply to you."), false, false, 0);
    v.pack_start(&label("Your answers are used to determine recommended settings only. If another program or Web site wants to use this information to better suit your needs, you will be explicitly asked for permission by that program.", "cp-task-desc"), false, false, 4);
    let stack = gtk::Stack::new();
    let mut boxes: Vec<(&'static str, gtk::CheckButton)> = vec![];
    for (i, (head, qs)) in QUESTIONS.iter().enumerate() {
        let p = vbox(4);
        let h = label(head, "eoa-h");
        h.set_margin_top(10);
        p.pack_start(&h, false, false, 0);
        p.pack_start(&{ let r = hbox(0); r.style_context().add_class("ac-rule"); r }, false, false, 2);
        p.pack_start(&label("Select all statements that apply to you:", ""), false, false, 2);
        for (k, t) in qs.iter() {
            let c = check(t, ANSWERS.with(|a| a.borrow().contains(k)));
            c.set_margin_start(14);
            p.pack_start(&c, false, false, 0);
            boxes.push((k, c));
        }
        stack.add_named(&p, &i.to_string());
    }
    v.pack_start(&stack, false, false, 0);
    let back_b = gtk::Button::with_mnemonic("_Back");
    let next = gtk::Button::with_mnemonic("_Next");
    let cancel = gtk::Button::with_label("Cancel");
    back_b.set_sensitive(false);
    let page = Rc::new(Cell::new(0usize));
    let show: Rc<dyn Fn()> = { let (stack, page, back_b, next) = (stack.clone(), page.clone(), back_b.clone(), next.clone()); Rc::new(move || {
        stack.set_visible_child_name(&page.get().to_string());
        back_b.set_sensitive(page.get() > 0);
        next.set_label(if page.get() + 1 == QUESTIONS.len() { "_Done" } else { "_Next" });
        next.set_use_underline(true);
    }) };
    { let (p, s) = (page.clone(), show.clone()); back_b.connect_clicked(move |_| { if p.get() > 0 { p.set(p.get() - 1); s(); } }); }
    {
        let (p, s, c) = (page.clone(), show.clone(), cp.clone());
        next.connect_clicked(move |_| {
            if p.get() + 1 < QUESTIONS.len() { p.set(p.get() + 1); s(); return; }
            ANSWERS.with(|a| *a.borrow_mut() = boxes.iter().filter(|(_, b)| b.is_active()).map(|(k, _)| *k).collect());
            go(&c, "access/recommended");
        });
    }
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "access")); }
    v.pack_start(&button_row(&[&back_b, &next, &cancel]), false, false, 0);
    show();
    v.upcast()
}

/// "Recommended settings": every setting the answers point to, already ticked.
fn recommended(cp: &Rc<Cp>) -> gtk::Widget {
    let answers = ANSWERS.with(|a| a.borrow().clone());
    let rec = recommend(&answers);
    let mut items = vec![Text("Based on your answers, the following settings are recommended. Select the settings you want to use, and then click OK or Apply.")];
    if rec.is_empty() { items.push(Text("No settings are recommended for your answers. You can still choose settings from the Ease of Access Center.")); }
    items.extend([
        H("Use the computer without a display"),
        Check("narrator", "Turn on Narrator"),
        H("Make the computer easier to see"),
        Check("highcontrast", "Turn on High Contrast"),
        Check("magnifier", "Turn on Magnifier"),
        Check("noanim", "Turn off all unnecessary animations (when possible)"),
        H("Make the mouse easier to use"),
        Pointers,
        Check("mousekeys", "Turn on Mouse Keys"),
        H("Make the keyboard easier to use"),
        Check("osk", "Use On-Screen Keyboard"),
        Check("stickykeys", "Turn on Sticky Keys"),
        Check("filterkeys", "Turn on Filter Keys"),
    ]);
    if answers.iter().any(|a| matches!(*a, "q_conv" | "q_deaf" | "q_hearing")) {
        items.push(H("Use text or visual alternatives for sounds"));
        items.push(Off("Turn on visual notifications for sounds (Sound Sentry)", "The window manager does not offer this yet."));
    }
    render(cp, "Recommended settings", "Recommended settings", items, &rec)
}

// ------------------------------------------------------------- administrative settings

/// "Change administrative settings": copy the Ease of Access settings to new user accounts.
fn admin_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Change administrative settings");
    learn_more(cp);
    let v = vbox(6);
    v.pack_start(&title("Change administrative settings"), false, false, 0);
    v.pack_start(&text("You can apply the Ease of Access settings you chose to the logon desktop and to new user accounts. These settings affect all users of this computer."), false, false, 0);
    let h1 = label("Logon desktop settings", "eoa-h");
    h1.set_margin_top(10);
    v.pack_start(&h1, false, false, 0);
    let logon = check("Apply all settings to the logon desktop", false);
    logon.set_sensitive(false);
    logon.set_tooltip_text(Some("The logon screen belongs to the display manager (SDDM, LightDM…), which has its own accessibility settings."));
    logon.set_margin_start(10);
    v.pack_start(&logon, false, false, 0);
    let h2 = label("New user settings", "eoa-h");
    h2.set_margin_top(10);
    v.pack_start(&h2, false, false, 0);
    let newu = check("Apply all settings to defaults for new users", std::path::Path::new("/etc/skel/.config/aerode/config.json").exists());
    newu.set_margin_start(10);
    v.pack_start(&newu, false, false, 0);
    let ok = gtk::Button::with_label("OK");
    super::super::admin::shield(&ok);
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "access")); }
    let (c, n) = (cp.clone(), newu.clone());
    ok.connect_clicked(move |_| {
        if !n.is_active() { back(&c, "access"); return; }
        // new accounts start with only the Ease of Access part of this user's settings
        let json = serde_json::json!({ "ease_of_access": config::get().ease_of_access }).to_string();
        let c2 = c.clone();
        elevate_sh(&format!("mkdir -p /etc/skel/.config/aerode && printf '%s\\n' {} > /etc/skel/.config/aerode/config.json", q(&json)), move |ok, out| {
            if report("The settings could not be applied to new users.", ok, &out) { back(&c2, "access"); }
        });
    });
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

#[cfg(test)]
mod tests {
    #[test]
    fn recommendations() {
        let m = super::recommend(&["q_blind", "q_nomouse"]);
        assert_eq!(m.get("narrator").map(|s| s.as_str()), Some("1"));
        assert_eq!(m.get("mousekeys").map(|s| s.as_str()), Some("1"));
        assert!(m.get("magnifier").is_none());
        assert!(super::recommend(&[]).is_empty());
    }
    #[test]
    fn xcursor() {
        // one 2x2 image of nominal size 32: one opaque white pixel at (1,1)
        let le = |v: u32| v.to_le_bytes();
        let mut d = b"Xcur".to_vec();
        for v in [16, 0x1_0000, 1] { d.extend(le(v)); }
        for v in [0xfffd_0002, 32, 28] { d.extend(le(v)); }
        for v in [36, 0xfffd_0002, 32, 1, 2, 2, 0, 0, 0] { d.extend(le(v)); }
        for v in [0u32, 0, 0, 0xffff_ffff] { d.extend(le(v)); }
        let (w, h, px) = super::xcursor_image(&d, 32).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(px, vec![255, 255, 255, 255]);
        assert!(super::xcursor_image(b"nope", 32).is_none());
    }
}
