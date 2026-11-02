//! Control Panel pages: System (with System Properties), Display, Sound, Date and Time, Power
//! Options, Mouse, Keyboard – one file each under hw/. Settings that X forgets at logout are saved
//! where the session re-applies them: ~/.config/aerode/input.sh (mouse and keyboard), displays.sh
//! (screen layout), the xsettings/Xresources written by `desktop::write_xsettings` (text size,
//! double-click and cursor blink time) and config.json (power plans, read by the shell's watcher).
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use aerode::{config, util};
#[allow(unused_imports)]
use super::super::*;

mod datetime;
mod display;
mod keyboard;
mod mouse;
mod power;
mod sound;
mod system;

impl Cp {
    pub(crate) fn page_system(self: &Rc<Self>) -> gtk::Widget { super::admin::css(); system::page(self) }
    pub(crate) fn page_display(self: &Rc<Self>) -> gtk::Widget { display::page(self) }
    pub(crate) fn page_sound(self: &Rc<Self>) -> gtk::Widget { sound::page(self) }
    pub(crate) fn page_datetime(self: &Rc<Self>) -> gtk::Widget { datetime::page(self) }
    pub(crate) fn page_power(self: &Rc<Self>) -> gtk::Widget { power::page(self) }
    pub(crate) fn page_mouse(self: &Rc<Self>) -> gtk::Widget { mouse::page(self) }
    pub(crate) fn page_keyboard(self: &Rc<Self>) -> gtk::Widget { keyboard::page(self) }
}

/// Pages reached from inside an applet ("Change plan settings", "Screen Resolution", a tab of a
/// property sheet …): they have their own id so Back and `aerode-control <id>` work.
/// Returns (breadcrumb after "Control Panel ▸", page).
pub fn subpage(cp: &Rc<Cp>, id: &str) -> Option<(String, gtk::Widget)> {
    let hs = "Hardware and Sound";
    let (crumb, w): (String, gtk::Widget) = match id {
        "resolution" => ("Appearance and Personalization ▸ Display ▸ Screen Resolution".to_string(), display::page_resolution(cp)),
        "sound-recording" => (format!("{hs} ▸ Sound"), sound::page_tab(cp, 1)),
        "sound-sounds" => (format!("{hs} ▸ Sound"), sound::page_tab(cp, 2)),
        "sound-comms" => (format!("{hs} ▸ Sound"), sound::page_tab(cp, 3)),
        "datetime-clocks" => ("Clock, Language, and Region ▸ Date and Time".into(), datetime::page_tab(cp, 1)),
        "datetime-internet" => ("Clock, Language, and Region ▸ Date and Time".into(), datetime::page_tab(cp, 2)),
        "mouse-pointers" => (format!("{hs} ▸ Mouse"), mouse::page_tab(cp, 1)),
        "mouse-options" => (format!("{hs} ▸ Mouse"), mouse::page_tab(cp, 2)),
        "mouse-wheel" => (format!("{hs} ▸ Mouse"), mouse::page_tab(cp, 3)),
        "mouse-hardware" => (format!("{hs} ▸ Mouse"), mouse::page_tab(cp, 4)),
        "keyboard-hardware" => (format!("{hs} ▸ Keyboard"), keyboard::page_tab(cp, 1)),
        // System Properties tabs and Performance Options: the System page with the dialog over it
        "sysprops" | "sysprops-hardware" | "sysprops-advanced" | "sysprops-protection" | "sysprops-remote" | "visualeffects" => {
            super::admin::css();
            let w = system::page(cp);
            let (me, id) = (cp.clone(), id.to_string());
            glib::idle_add_local_once(move || match id.as_str() {
                "visualeffects" => { system::performance(Some(&cpwin(&me))).present(); }
                _ => system::props(Some(&me), ["sysprops", "sysprops-hardware", "sysprops-advanced", "sysprops-protection", "sysprops-remote"].iter().position(|x| *x == id).unwrap_or(0) as u32),
            });
            ("System and Security ▸ System".into(), w)
        }
        id if id.starts_with("power-") => (format!("System and Security ▸ Power Options ▸ {}", power::crumb(id)), power::subpage(cp, id)?),
        _ => return None,
    };
    Some((crumb, w))
}

/// `aerode-control sysprops…` / `visualeffects` started on their own: only the dialog.
pub fn standalone(app: &gtk::Application, id: &str) -> bool { system::standalone(app, id) }

/// Performance Options (Visual Effects) over Control Panel, for "Adjust visual effects" links.
pub(crate) fn visual_effects(cp: &Rc<Cp>) { system::performance(Some(&cpwin(cp))).present(); }

const CSS: &str = r#"
.hw-rule { background-color: #d5dfef; min-height: 1px; }
.hw-head { color: #1e3287; }
.hw-white { background-color: #ffffff; }
.hw-list { background-color: #ffffff; border: 1px solid #828790; }
.hw-row { padding: 4px; border: 1px solid transparent; border-radius: 2px; }
.hw-row-sel { background-image: linear-gradient(to bottom, #f1f7fe, #cfe5fc); border: 1px solid #84acdd; }
.hw-dim { color: #6d6d6d; }
.hw-sub { color: #1e3287; font-size: 11pt; }
.hw-badge { color: #ffffff; font-weight: bold; }
"#;

pub(crate) fn css() {
    thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if DONE.with(|d| d.replace(true)) { return; }
    aerode::theme::load_css(CSS);
}

/// Win7 section heading on a Control Panel page: blue text and a thin rule running to the right.
pub(crate) fn heading(t: &str) -> gtk::Box {
    css();
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    h.set_margin_top(12);
    h.set_margin_bottom(4);
    let l = gtk::Label::new(Some(t));
    l.style_context().add_class("hw-head");
    h.pack_start(&l, false, false, 0);
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    r.style_context().add_class("hw-rule");
    r.set_valign(gtk::Align::Center);
    h.pack_start(&r, true, true, 0);
    h
}

/// A label for a dialog row.
pub(crate) fn lbl(t: &str) -> gtk::Label {
    let l = gtk::Label::with_mnemonic(t);
    l.set_xalign(0.0);
    l
}

/// Wrapped text limited to a width (for property-sheet tabs).
pub(crate) fn wrap(t: &str, chars: i32) -> gtk::Label {
    let l = text(t);
    l.set_max_width_chars(chars);
    l
}

/// A Windows 7 property sheet in its own dialog window (System Properties, Performance Options,
/// Advanced display settings …): tabs, then OK / Cancel / Apply. Apply is greyed until something
/// changes; OK applies then closes; `on_apply` returning false keeps the dialog open.
pub(crate) struct DlgSheet {
    pub d: gtk::Dialog,
    pub nb: gtk::Notebook,
    apply_btn: Option<gtk::Widget>,
    on_apply: RefCell<Option<Rc<dyn Fn() -> bool>>>,
}

impl DlgSheet {
    pub fn new(parent: Option<&gtk::Window>, title: &str, with_apply: bool) -> Rc<DlgSheet> {
        let d = gtk::Dialog::new();
        d.set_title(title);
        if let Some(p) = parent { d.set_transient_for(Some(p)); }
        d.set_modal(true);
        d.set_resizable(false);
        d.set_destroy_with_parent(true);
        d.set_icon_name(Some(util::first_icon(&["preferences-system"])));
        let nb = gtk::Notebook::new();
        let c = d.content_area();
        c.set_border_width(7);
        c.pack_start(&nb, true, true, 0);
        for (l, r) in [("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)] { d.add_button(l, r).set_size_request(75, -1); }
        let apply_btn = with_apply.then(|| { let b = d.add_button("_Apply", gtk::ResponseType::Apply); b.set_size_request(75, -1); b.set_sensitive(false); b });
        let s = Rc::new(DlgSheet { d: d.clone(), nb, apply_btn, on_apply: RefCell::new(None) });
        {
            let s2 = s.clone();
            d.connect_response(move |d, r| match r {
                gtk::ResponseType::Apply => { s2.apply(); }
                gtk::ResponseType::Ok => { if !s2.is_dirty() || s2.apply() { d.close(); } }
                // the window is going away already
                gtk::ResponseType::DeleteEvent => {}
                _ => d.close(),
            });
        }
        s
    }

    pub fn tab(&self, label: &str, page: &impl IsA<gtk::Widget>) {
        let w = page.as_ref();
        w.set_margin_start(10);
        w.set_margin_end(10);
        w.set_margin_top(10);
        w.set_margin_bottom(10);
        self.nb.append_page(page, Some(&gtk::Label::with_mnemonic(label)));
    }

    pub fn on_apply(&self, f: impl Fn() -> bool + 'static) { *self.on_apply.borrow_mut() = Some(Rc::new(f)); }

    fn is_dirty(&self) -> bool { self.apply_btn.as_ref().map(|b| b.is_sensitive()).unwrap_or(true) }

    fn apply(&self) -> bool {
        let f = self.on_apply.borrow().clone();
        let ok = f.map(|f| f()).unwrap_or(true);
        if ok { if let Some(b) = &self.apply_btn { b.set_sensitive(false); } }
        ok
    }

    /// Something changed: Apply becomes available.
    pub fn dirty(&self) -> Rc<dyn Fn()> {
        let b = self.apply_btn.clone();
        Rc::new(move || if let Some(b) = &b { b.set_sensitive(true) })
    }
    pub fn watch_check(&self, c: &impl IsA<gtk::ToggleButton>) { let d = self.dirty(); c.connect_toggled(move |_| d()); }
    pub fn watch_combo(&self, c: &impl IsA<gtk::ComboBox>) { let d = self.dirty(); c.connect_changed(move |_| d()); }
    pub fn watch_entry(&self, e: &gtk::Entry) { let d = self.dirty(); e.connect_changed(move |_| d()); }

    pub fn show(&self, page: u32) {
        self.d.show_all();
        self.nb.set_current_page(Some(page));
        self.d.present();
    }
}

/// A plain modal dialog with OK / Cancel; `content` is filled by the caller. Returns the dialog.
pub(crate) fn ok_cancel(parent: Option<&gtk::Window>, title: &str) -> gtk::Dialog {
    let d = gtk::Dialog::new();
    d.set_title(title);
    if let Some(p) = parent { d.set_transient_for(Some(p)); }
    d.set_modal(true);
    d.set_resizable(false);
    d.set_destroy_with_parent(true);
    d.content_area().set_border_width(10);
    d.content_area().set_spacing(8);
    for (l, r) in [("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)] { d.add_button(l, r).set_size_request(75, -1); }
    d.set_default_response(gtk::ResponseType::Ok);
    d
}

/// A horizontal slider between two captions ("Slow ─────●── Fast"), Win7 style (no value shown).
pub(crate) fn slider(min: f64, max: f64, step: f64, value: f64, left: &str, right: &str) -> (gtk::Box, gtk::Scale) {
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    s.set_draw_value(false);
    // snap to the steps like Windows' trackbars
    if step >= 1.0 { s.set_round_digits(0); }
    s.set_value(value);
    s.set_hexpand(true);
    s.set_size_request(160, -1);
    h.pack_start(&lbl(left), false, false, 0);
    h.pack_start(&s, true, true, 0);
    h.pack_start(&lbl(right), false, false, 0);
    (h, s)
}

/// The session's mouse and keyboard settings that X forgets at logout, as a script the session
/// runs (data/aerode-session); also run now.
pub(crate) fn write_input_script() {
    let c = config::get().hw;
    let (m, k) = (&c.mouse, &c.keyboard);
    let mut s = String::from("#!/bin/sh\n# written by aerode (Control Panel > Mouse, Keyboard)\n");
    // 0 = never set in Keyboard Properties: leave the X server's repeat alone
    if k.delay > 0 && k.rate > 0 { s += &format!("xset r rate {} {}\n", k.delay, k.rate); }
    // every pointer libinput drives (mice and touchpads); properties a device lacks are skipped
    s += "for id in $(xinput list --id-only 2>/dev/null); do\n";
    s += &format!("  xinput set-prop $id 'libinput Left Handed Enabled' {} 2>/dev/null\n", m.left_handed as u8);
    s += &format!("  xinput set-prop $id 'libinput Accel Speed' {:.2} 2>/dev/null\n", m.speed.clamp(-1.0, 1.0));
    // Enhance pointer precision = the adaptive profile, else flat (libinput ≥ 1.23 has a third, custom one)
    let (a, f) = if m.precision { (1, 0) } else { (0, 1) };
    s += &format!("  xinput set-prop $id 'libinput Accel Profile Enabled' {a} {f} 0 2>/dev/null || xinput set-prop $id 'libinput Accel Profile Enabled' {a} {f} 2>/dev/null\n");
    s += &format!("  xinput set-prop $id 'libinput Tapping Drag Lock Enabled' {} 2>/dev/null\n", m.click_lock as u8);
    s += "done\n";
    if m.hide_typing {
        s += "pgrep -u \"$(id -u)\" -x unclutter >/dev/null || { command -v unclutter >/dev/null && unclutter --timeout 0 --jitter 2 --ignore-scrolling --keystroke --fork 2>/dev/null; } || { command -v xbanish >/dev/null && ! pgrep -u \"$(id -u)\" -x xbanish >/dev/null && (xbanish &); }\n";
    }
    let dir = config::conf_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("input.sh");
    let _ = std::fs::write(&path, s);
    util::spawn(&["sh", &path.to_string_lossy()]);
}

/// Window of the Control Panel (parent for dialogs).
pub(crate) fn cpwin(cp: &Rc<Cp>) -> gtk::Window { cp.win.clone().upcast() }

/// "10 minutes", "1 hour", "Never" for a timer in minutes.
pub(crate) fn minutes_label(m: u32) -> String {
    aerode::power::TIMEOUTS.iter().find(|t| t.0 == m).map(|t| t.1.to_string()).unwrap_or_else(|| format!("{m} minutes"))
}
