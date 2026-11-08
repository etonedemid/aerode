//! Input languages the Windows 7 way: the language indicator ("EN") docked left of the notification
//! area, a menu to pick the language, one input language per application (Win7 keeps the language per
//! program, not globally), the switching hot key, and the saved layout list
//! (~/.config/aerode/keyboard.json, read by the compositor).
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use crate::config;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Layout {
    pub layout: String,
    pub variant: String,
}

/// Win7's language bar abbreviation and language name for common XKB layouts.
const LANGS: &[(&str, &str, &str)] = &[
    ("us", "EN", "English (United States)"), ("gb", "EN", "English (United Kingdom)"), ("au", "EN", "English (Australia)"),
    ("ca", "FR", "French (Canada)"), ("ie", "EN", "English (Ireland)"), ("ru", "RU", "Russian (Russia)"),
    ("ua", "UK", "Ukrainian (Ukraine)"), ("by", "BE", "Belarusian (Belarus)"), ("de", "DE", "German (Germany)"),
    ("at", "DE", "German (Austria)"), ("ch", "DE", "German (Switzerland)"), ("fr", "FR", "French (France)"),
    ("be", "NL", "Dutch (Belgium)"), ("nl", "NL", "Dutch (Netherlands)"), ("es", "ES", "Spanish (Spain)"),
    ("latam", "ES", "Spanish (Mexico)"), ("it", "IT", "Italian (Italy)"), ("pt", "PT", "Portuguese (Portugal)"),
    ("br", "PT", "Portuguese (Brazil)"), ("pl", "PL", "Polish (Poland)"), ("cz", "CS", "Czech (Czech Republic)"),
    ("sk", "SK", "Slovak (Slovakia)"), ("hu", "HU", "Hungarian (Hungary)"), ("ro", "RO", "Romanian (Romania)"),
    ("bg", "BG", "Bulgarian (Bulgaria)"), ("rs", "SR", "Serbian (Cyrillic, Serbia)"), ("hr", "HR", "Croatian (Croatia)"),
    ("si", "SL", "Slovenian (Slovenia)"), ("se", "SV", "Swedish (Sweden)"), ("no", "NB", "Norwegian, Bokmål (Norway)"),
    ("dk", "DA", "Danish (Denmark)"), ("fi", "FI", "Finnish (Finland)"), ("ee", "ET", "Estonian (Estonia)"),
    ("lv", "LV", "Latvian (Latvia)"), ("lt", "LT", "Lithuanian (Lithuania)"), ("gr", "EL", "Greek (Greece)"),
    ("tr", "TR", "Turkish (Turkey)"), ("il", "HE", "Hebrew (Israel)"), ("ara", "AR", "Arabic (Saudi Arabia)"),
    ("ir", "FA", "Persian"), ("in", "HI", "Hindi (India)"), ("th", "TH", "Thai (Thailand)"), ("vn", "VI", "Vietnamese (Vietnam)"),
    ("cn", "ZH", "Chinese (Simplified, PRC)"), ("tw", "ZH", "Chinese (Traditional, Taiwan)"), ("jp", "JA", "Japanese (Japan)"),
    ("kr", "KO", "Korean (Korea)"), ("kz", "KK", "Kazakh (Kazakhstan)"), ("ge", "KA", "Georgian (Georgia)"),
    ("am", "HY", "Armenian (Armenia)"), ("az", "AZ", "Azeri (Latin, Azerbaijan)"), ("is", "IS", "Icelandic (Iceland)"),
    ("mk", "MK", "Macedonian (FYROM)"), ("al", "SQ", "Albanian (Albania)"), ("uz", "UZ", "Uzbek (Latin, Uzbekistan)"),
];

/// (two-letter indicator, language name) for a layout.
pub fn language(l: &Layout) -> (String, String) {
    match LANGS.iter().find(|(k, _, _)| *k == l.layout) {
        Some((_, abbr, name)) => (abbr.to_string(), name.to_string()),
        None => (l.layout.chars().take(2).collect::<String>().to_uppercase(), describe(&l.layout, "")),
    }
}

fn rules_file() -> Option<String> {
    ["/usr/share/X11/xkb/rules/evdev.lst", "/usr/share/X11/xkb/rules/base.lst"].iter().find_map(|p| std::fs::read_to_string(p).ok())
}

/// The XKB catalogue: layouts (code, description) and variants (layout, variant, description).
pub fn catalog() -> (Vec<(String, String)>, Vec<(String, String, String)>) {
    let (mut layouts, mut variants) = (vec![], vec![]);
    let Some(txt) = rules_file() else { return (layouts, variants) };
    let mut section = "";
    for line in txt.lines() {
        if let Some(s) = line.strip_prefix("! ") { section = if s.starts_with("layout") { "layout" } else if s.starts_with("variant") { "variant" } else { "" }; continue; }
        let t = line.trim();
        let Some((code, rest)) = t.split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match section {
            "layout" => layouts.push((code.to_string(), rest.to_string())),
            "variant" => if let Some((lay, desc)) = rest.split_once(": ") { variants.push((lay.to_string(), code.to_string(), desc.to_string())) },
            _ => {}
        }
    }
    (layouts, variants)
}

/// Human description of a layout/variant from the XKB catalogue.
pub fn describe(layout: &str, variant: &str) -> String {
    let (ls, vs) = catalog();
    if !variant.is_empty() {
        if let Some((_, _, d)) = vs.iter().find(|(l, v, _)| l == layout && v == variant) { return d.clone(); }
    }
    ls.iter().find(|(c, _)| c == layout).map(|(_, d)| d.clone()).unwrap_or_else(|| layout.to_string())
}

/// Hot keys for switching input languages (Win7 "Advanced Key Settings" plus common extras), as XKB
/// options. Alt+Space is left out: it opens the window menu.
pub const HOTKEYS: &[(&str, &str)] = &[
    ("Alt+Shift (Windows default)", "grp:alt_shift_toggle"),
    ("Left Alt+Left Shift", "grp:lalt_lshift_toggle"),
    ("Ctrl+Shift", "grp:ctrl_shift_toggle"),
    ("Left Ctrl+Left Shift", "grp:lctrl_lshift_toggle"),
    ("Windows logo key+Space", "grp:win_space_toggle"),
    ("Caps Lock", "grp:caps_toggle"),
    ("Shift+Caps Lock", "grp:shift_caps_toggle"),
    ("Right Alt", "grp:toggle"),
    ("Right Ctrl", "grp:rctrl_toggle"),
    ("Menu key", "grp:menu_toggle"),
    ("Scroll Lock", "grp:sclk_toggle"),
    ("(None)", ""),
];

pub const DEFAULT_SWITCH: &str = "grp:alt_shift_toggle";

/// The configured switching option ("" = none).
pub fn switch_option() -> String {
    config::get().kbd_switch.unwrap_or_else(|| DEFAULT_SWITCH.to_string())
}

fn code(l: &Layout) -> String { if l.variant.is_empty() { l.layout.clone() } else { format!("{}({})", l.layout, l.variant) } }

fn parse_code(c: &str) -> Layout {
    match c.split_once('(') {
        Some((l, v)) => Layout { layout: l.to_string(), variant: v.trim_end_matches(')').to_string() },
        None => Layout { layout: c.to_string(), variant: String::new() },
    }
}

/// Make the X server use the configured layouts and switching key: at login, and again whenever a
/// keyboard appears (X gives a hot-plugged keyboard the system defaults, losing both).
pub fn ensure() {
    let (cur, cur_opts) = current();
    let cfg = config::get();
    let want: Vec<Layout> = if cfg.kbd_layouts.is_empty() { cur.clone() } else { cfg.kbd_layouts.iter().map(|c| parse_code(c)).collect() };
    let mut opts: Vec<String> = cur_opts.iter().filter(|o| !o.starts_with("grp:")).cloned().collect();
    let sw = switch_option();
    if !sw.is_empty() { opts.push(sw); }
    let same_opts = { let mut a = opts.clone(); let mut b = cur_opts.clone(); a.sort(); b.sort(); a == b };
    if want != cur || !same_opts { apply_with_options(&want, &opts); }
}

fn split_layouts(layouts: &str, variants: &str) -> Vec<Layout> {
    let vs: Vec<&str> = variants.split(',').collect();
    layouts.split(',').filter(|l| !l.trim().is_empty()).enumerate()
        .map(|(i, l)| Layout { layout: l.trim().to_string(), variant: vs.get(i).map(|v| v.trim().to_string()).unwrap_or_default() }).collect()
}

/// Layouts and XKB options in use now.
pub fn current() -> (Vec<Layout>, Vec<String>) {
    let v = crate::wmipc::call(&crate::wmipc::Req::KeyboardState).unwrap_or_default();
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut layouts = split_layouts(&get("layout"), &get("variant"));
    if layouts.is_empty() { layouts.push(Layout { layout: "us".into(), variant: String::new() }); }
    (layouts, get("options").split(',').filter(|o| !o.is_empty()).map(String::from).collect())
}

/// The language-switching option in use ("" = none).
pub fn hotkey(options: &[String]) -> String {
    options.iter().find(|o| o.starts_with("grp:")).cloned().unwrap_or_default()
}

/// Apply a layout list with these XKB options now; the compositor saves it for the next sessions
/// (~/.config/aerode/keyboard.json).
pub fn apply_with_options(layouts: &[Layout], options: &[String]) {
    if layouts.is_empty() { return; }
    let layout = layouts.iter().map(|x| x.layout.as_str()).collect::<Vec<_>>().join(",");
    let variant = layouts.iter().map(|x| x.variant.as_str()).collect::<Vec<_>>().join(",");
    crate::wmipc::send(crate::wmipc::Req::SetKeyboard { layout, variant, options: options.join(",") });
}

/// Apply layouts with a switching hot key ("" = none), keeping the user's other XKB options
/// (compose, caps…), and remember both in the config.
pub fn apply(layouts: &[Layout], hot: &str) {
    let (_, opts) = current();
    let mut opts: Vec<String> = opts.into_iter().filter(|o| !o.starts_with("grp:")).collect();
    if !hot.is_empty() { opts.push(hot.to_string()); }
    let codes: Vec<String> = layouts.iter().map(code).collect();
    config::update(|c| { c.kbd_layouts = codes.clone(); c.kbd_switch = Some(hot.to_string()); });
    apply_with_options(layouts, &opts);
}

/// First session: adopt the keyboard layouts set up in Plasma (kxkbrc) – under aerode nothing else applies them.
pub fn import_plasma() {
    if config::conf_dir().join("keyboard.json").exists() { return; }
    let Ok(txt) = std::fs::read_to_string(gtk::glib::user_config_dir().join("kxkbrc")) else { return };
    let mut kv = HashMap::new();
    let mut in_layout = false;
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with('[') { in_layout = t == "[Layout]"; continue; }
        if let (true, Some((k, v))) = (in_layout, t.split_once('=')) { kv.insert(k.trim().to_string(), v.trim().to_string()); }
    }
    if kv.get("Use").map(|v| v != "true").unwrap_or(true) { return; }
    let layouts = split_layouts(kv.get("LayoutList").map(String::as_str).unwrap_or(""), kv.get("VariantList").map(String::as_str).unwrap_or(""));
    if layouts.is_empty() { return; }
    let mut opts: Vec<String> = kv.get("Options").map(|o| o.split(',').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default();
    if !opts.iter().any(|o| o.starts_with("grp:")) { opts.push(DEFAULT_SWITCH.into()); }
    let sw = opts.iter().find(|o| o.starts_with("grp:")).cloned().unwrap_or_default();
    let codes: Vec<String> = layouts.iter().map(code).collect();
    config::update(|c| { c.kbd_layouts = codes.clone(); c.kbd_switch = Some(sw.clone()); });
    apply_with_options(&layouts, &opts);
}

// ------------------------------------------------------------------ taskbar indicator
struct Indicator {
    layouts: RefCell<Vec<Layout>>,
    group: Cell<u8>,
    /// the active window and the key its language is remembered under (its process, like Win7's per-program language)
    active: Cell<(u32, u32)>,
    per_app: RefCell<HashMap<u32, u8>>,
    ensure_pending: Rc<Cell<bool>>,
    button: gtk::Button,
    label: gtk::Label,
}

impl Indicator {
    fn lock(&self, g: u8) { crate::wmipc::send(crate::wmipc::Req::SetGroup { group: g as u32 }); }

    fn reload(&self) {
        *self.layouts.borrow_mut() = current().0;
        self.read_group();
        self.update();
    }

    fn read_group(&self) {
        let g = crate::wmipc::call(&crate::wmipc::Req::KeyboardState).and_then(|v| v.get("group")?.as_u64()).unwrap_or(0) as u8;
        self.group.set(g);
    }

    fn update(&self) {
        let layouts = self.layouts.borrow();
        // Win7 shows the docked language bar only when more than one input language is installed
        self.button.set_visible(layouts.len() > 1);
        if let Some(l) = layouts.get(self.group.get() as usize).or(layouts.first()) {
            let (abbr, name) = language(l);
            self.label.set_text(&abbr);
            self.button.set_tooltip_text(Some(&name));
        }
    }

    fn active_changed(&self) {
        let st = crate::wmipc::state();
        let w = st.active;
        let (old, old_key) = self.active.get();
        if w == old { return; }
        if old != 0 { self.per_app.borrow_mut().insert(old_key, self.group.get()); }
        if w == 0 { self.active.set((0, 0)); return; }
        let key = st.windows.iter().find(|x| x.id == w).map(|x| x.pid).filter(|p| *p != 0).unwrap_or(w);
        self.active.set((w, key));
        // a program seen for the first time starts in the default (first) input language
        let want = self.per_app.borrow().get(&key).copied().unwrap_or(0);
        if want != self.group.get() && (want as usize) < self.layouts.borrow().len() { self.lock(want); }
    }

    /// The compositor reports a change of group or of the layout list.
    fn keyboard_event(&self) {
        let (was, had) = (self.group.get(), self.layouts.borrow().clone());
        self.reload();
        let g = self.group.get();
        if g != was {
            let (w, key) = self.active.get();
            if w != 0 { self.per_app.borrow_mut().insert(key, g); }
        }
        if *self.layouts.borrow() != had && !self.ensure_pending.replace(true) {
            let p = self.ensure_pending.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || { p.set(false); ensure(); });
        }
    }

    fn pick(self: &Rc<Self>, i: usize) {
        self.lock(i as u8);
        let (w, key) = self.active.get();
        if w != 0 { self.per_app.borrow_mut().insert(key, i as u8); }
    }

    /// Left click: the list of input languages, the current one marked (Win7 draws the
    /// abbreviation in the gutter and highlights it for the active language).
    fn menu(self: &Rc<Self>, e: &gdk::EventButton) {
        let m = gtk::Menu::new();
        let cur = self.group.get() as usize;
        for (i, l) in self.layouts.borrow().iter().enumerate() {
            let (abbr, name) = language(l);
            let it = gtk::MenuItem::with_label(&name);
            let on = i == cur;
            it.connect_local("draw", true, move |args| {
                let w = args[0].get::<gtk::Widget>().ok()?;
                let cr = args[1].get::<gtk::cairo::Context>().ok()?;
                let y = ((w.allocated_height() - 22) / 2) as f64;
                if on {
                    // POPUPCHECKBACKGROUND colours
                    crate::glass::rounded(&cr, 0.5, y + 0.5, 21.0, 21.0, 3.0);
                    cr.set_source_rgba(230.0 / 255.0, 241.0 / 255.0, 1.0, 0.6);
                    cr.fill_preserve().ok();
                    cr.set_source_rgba(152.0 / 255.0, 196.0 / 255.0, 250.0 / 255.0, 0.9);
                    cr.set_line_width(1.0);
                    cr.stroke().ok();
                }
                let layout = pangocairo::functions::create_layout(&cr);
                layout.set_text(&abbr);
                let mut fd = gtk::pango::FontDescription::from_string("Segoe UI 7");
                fd.set_weight(gtk::pango::Weight::Bold);
                layout.set_font_description(Some(&fd));
                let (tw, th) = layout.pixel_size();
                cr.set_source_rgb(0.12, 0.2, 0.45);
                cr.move_to(11.0 - tw as f64 / 2.0, y + 11.0 - th as f64 / 2.0);
                pangocairo::functions::show_layout(&cr, &layout);
                Some(false.to_value())
            });
            let me = self.clone();
            it.connect_activate(move |_| me.pick(i));
            m.append(&it);
        }
        m.append(&gtk::SeparatorMenuItem::new());
        let s = gtk::MenuItem::with_label("Settings...");
        s.connect_activate(|_| { crate::util::spawn(&["aerode-control", "region"]); });
        m.append(&s);
        m.show_all();
        // Win7 drops the tooltip when the menu opens (GTK would leave it over the menu)
        self.button.set_has_tooltip(false);
        { let me = self.clone(); m.connect_deactivate(move |_| { me.button.set_has_tooltip(true); me.update(); }); }
        m.popup_at_widget(&self.button, gdk::Gravity::North, gdk::Gravity::South, Some(e));
    }
}

/// Dock the input-language indicator into the taskbar's notification area.
pub fn attach(tray: &gtk::Box) {
    let label = gtk::Label::new(Some("EN"));
    let button = gtk::Button::new();
    button.style_context().add_class("tray");
    button.style_context().add_class("lang");
    button.add(&label);
    button.set_no_show_all(true);
    label.show();
    tray.pack_end(&button, false, false, 0);
    let ind = Rc::new(Indicator {
        layouts: RefCell::new(vec![]), group: Cell::new(0), active: Cell::new((0, 0)), per_app: RefCell::new(HashMap::new()),
        ensure_pending: Rc::new(Cell::new(false)), button: button.clone(), label,
    });
    ind.reload();
    ind.active_changed();
    {
        let me = ind.clone();
        button.connect_button_press_event(move |_, e| {
            if e.button() == 3 {
                let m = gtk::Menu::new();
                let s = gtk::MenuItem::with_label("Settings...");
                s.connect_activate(|_| { crate::util::spawn(&["aerode-control", "region"]); });
                m.append(&s);
                m.show_all();
                m.popup_at_pointer(Some(e));
            } else if e.button() == 1 {
                me.menu(e);
            }
            glib::Propagation::Stop
        });
    }
    let me = ind.clone();
    crate::wmclient::on_event(move |ev| match ev {
        "keyboard" => me.keyboard_event(),
        "state" => me.active_changed(),
        _ => {}
    });
}
