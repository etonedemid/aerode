//! The speaker icon in the notification area (SndVolSSO.dll), its flyout (sndvol.exe.mui DIALOG
//! 201) and its context menu (SndVolSSO MENU 130).
//!
//! Nothing here talks to the sound server on the GTK thread: the icon, the tooltip and an open
//! flyout show the last state read by the audio worker (`sys::query_async`, refreshed on
//! `pactl subscribe` events), and every change is queued with `sys::send`, which applies them in
//! order and keeps only the newest slider position.
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use super::{close_open, control, link, popup, tray_button};
use crate::mixer::{self, Slider};
use crate::sys::{self, AudioCmd, AudioInfo, AudioState};
use crate::taskbar::Taskbar;
use crate::util;

/// One device column of the flyout.
struct Col {
    root: gtk::Box,
    /// None = follows the default device
    fixed: Option<String>,
    sink: RefCell<String>,
    title: RefCell<String>,
    slider: Rc<Slider>,
    dev: gtk::Button,
    dev_img: gtk::Image,
    mute: gtk::Button,
    mute_img: gtk::Image,
    muted: Cell<bool>,
    last_user: Cell<Option<Instant>>,
}

impl Col {
    /// The user is moving it (or just did): don't move it from outside meanwhile.
    fn busy(&self) -> bool {
        self.slider.dragging() || self.last_user.get().map(|t| t.elapsed().as_millis() < 600).unwrap_or(false)
    }
    fn show_mute(&self, m: bool) {
        self.muted.set(m);
        if let Some(p) = mixer::mute_glyph(m, 16) { self.mute_img.set_from_pixbuf(Some(&p)); }
        // sndvol.exe.mui 501 / 508
        self.mute.set_tooltip_text(Some(&format!("{} {}", if m { "Unmute" } else { "Mute" }, self.title.borrow())));
    }
    fn sink_arg(&self) -> Option<String> {
        let s = self.sink.borrow();
        (!s.is_empty()).then(|| s.clone())
    }
}

struct Vol {
    button: gtk::Button,
    img: gtk::Image,
    info: RefCell<Option<AudioInfo>>,
    /// the tooltip text; applied only while no flyout is open over the icon
    tip: RefCell<String>,
    fly: RefCell<Vec<Rc<Col>>>,
    fly_open: Cell<bool>,
}

impl Vol {
    fn refresh(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        sys::query_async(false, move |i| { if let (Some(v), Some(i)) = (w.upgrade(), i) { v.apply(i); } });
    }

    fn set_tip(&self, t: String) {
        // the icon's tooltip must not pop up over its own open flyout (Win7 suppresses it)
        if self.fly_open.get() { self.button.set_has_tooltip(false); } else { self.button.set_tooltip_text(Some(&t)); }
        *self.tip.borrow_mut() = t;
    }

    fn apply(&self, i: AudioInfo) {
        if let Some(p) = util::pixbuf(i.icon(), 16) { self.img.set_from_pixbuf(Some(&p)); }
        self.set_tip(i.tooltip());
        for c in self.fly.borrow().iter() { update_col(c, &i); }
        *self.info.borrow_mut() = Some(i);
    }
}

fn update_col(c: &Col, i: &AudioInfo) {
    let sink = match &c.fixed { None => i.default_sink(), Some(n) => i.sinks.iter().find(|s| &s.name == n) };
    let usable = sink.is_some() && (c.fixed.is_some() || i.state == AudioState::Ok);
    c.slider.set_enabled(usable);
    c.mute.set_sensitive(usable);
    match sink.filter(|_| usable) {
        Some(s) => {
            *c.sink.borrow_mut() = s.name.clone();
            *c.title.borrow_mut() = s.title.clone();
            if !c.busy() { c.slider.set_value(s.level().min(100)); }
            c.show_mute(s.muted);
            if let Some(p) = util::pixbuf(util::first_icon(s.icons), 32) { c.dev_img.set_from_pixbuf(Some(&p)); }
            c.dev.set_tooltip_text(Some(&s.title));
        }
        None => {
            c.slider.set_value(0);
            if let Some(p) = util::pixbuf(sys::volume_icon(None), 32) { c.dev_img.set_from_pixbuf(Some(&p)); }
            c.dev.set_tooltip_text(Some(if i.state == AudioState::Ok { AudioState::NotInstalled.text() } else { i.state.text() }));
            c.show_mute(false);
        }
    }
}

pub(super) fn volume(tb: &Rc<Taskbar>) {
    let (b, img) = tray_button(tb, sys::volume_icon(None), "");
    b.set_has_tooltip(false);
    let v = Rc::new(Vol { button: b.clone(), img, info: RefCell::new(None), tip: RefCell::new(String::new()), fly: RefCell::new(vec![]), fly_open: Cell::new(false) });
    v.refresh();
    // follow the sound server's change events; poll only as a safety net (or without pactl)
    let w = Rc::downgrade(&v);
    let events = util::watch_lines(&["pactl", "subscribe"], |l| l.contains(" on sink ") || l.contains(" on server") || l.contains(" on card "),
        std::time::Duration::from_millis(40), move || { if let Some(v) = w.upgrade() { v.refresh(); } });
    { let w = Rc::downgrade(&v); glib::timeout_add_seconds_local(if events { 60 } else { 3 }, move || { if let Some(v) = w.upgrade() { v.refresh(); } glib::ControlFlow::Continue }); }
    // mouse wheel over the speaker (an aerode extra; Win7 has none): like the volume keys, 2 % a
    // notch from the device's real level, unmuting (touchpads: accumulated smooth deltas)
    {
        let (v, acc) = (v.clone(), Cell::new(0.0f64));
        b.add_events(gdk::EventMask::SCROLL_MASK | gdk::EventMask::SMOOTH_SCROLL_MASK);
        b.connect_scroll_event(move |_, e| {
            let steps = match e.direction() {
                gdk::ScrollDirection::Up => 1,
                gdk::ScrollDirection::Down => -1,
                gdk::ScrollDirection::Smooth => { let a = acc.get() - e.delta().1; let n = a.trunc() as i32; acc.set(a - n as f64); n }
                _ => 0,
            };
            if steps == 0 { return glib::Propagation::Stop; }
            let Some(mut i) = v.info.borrow().clone() else { return glib::Propagation::Stop };
            if i.state != AudioState::Ok { return glib::Propagation::Stop; }
            sys::send(AudioCmd::Step(steps));
            // show the intended state at once; the change event confirms it
            let def = i.default.clone();
            if let Some(s) = i.sinks.iter_mut().find(|s| s.name == def) {
                let to = sys::stepped(s.level(), steps);
                s.channels = sys::scale_channels(&s.channels, (to as u64 * 65536 + 50) / 100, None);
                s.muted = false;
            }
            v.apply(i);
            glib::Propagation::Stop
        });
    }
    // right click: SndVolSSO's context menu
    {
        let v = v.clone();
        b.connect_button_press_event(move |b, e| {
            if e.button() != 3 || e.event_type() != gdk::EventType::ButtonPress { return glib::Propagation::Proceed; }
            close_open();
            context_menu(&v, b, e);
            glib::Propagation::Stop
        });
    }
    let vc = v.clone();
    b.connect_clicked(move |b| {
        if close_open() { return; }
        open_flyout(&vc, b);
    });
}

fn context_menu(v: &Rc<Vol>, b: &gtk::Button, e: &gdk::EventButton) {
    let m = gtk::Menu::new();
    let add = |label: &str, f: Box<dyn Fn()>| {
        let i = gtk::MenuItem::with_mnemonic(label);
        i.connect_activate(move |_| f());
        m.append(&i);
    };
    let usable = v.info.borrow().as_ref().map(|i| i.state == AudioState::Ok).unwrap_or(false);
    // without a usable device the first item becomes "Troubleshoot sound problems" (string 2011)
    if usable || v.info.borrow().is_none() { add("Open Volume _Mixer", Box::new(mixer::open)); }
    else { add("_Troubleshoot sound problems", Box::new(|| control("troubleshooting"))); }
    m.append(&gtk::SeparatorMenuItem::new());
    add("_Playback devices", Box::new(|| control("sound")));
    add("_Recording devices", Box::new(|| control("sound-recording")));
    add("_Sounds", Box::new(|| control("sound-sounds")));
    m.append(&gtk::SeparatorMenuItem::new());
    let v2 = v.clone();
    add("_Volume control options", Box::new(move || {
        let w = Rc::downgrade(&v2);
        // the device list wants a fresh read; off the GTK thread like every other one
        sys::query_async(false, move |i| {
            let i = i.or_else(|| w.upgrade().and_then(|v| v.info.borrow().clone())).unwrap_or_default();
            mixer::options_dialog(&i);
        });
    }));
    m.show_all();
    m.set_attach_widget(Some(b));
    // no tooltip over the open menu
    b.set_has_tooltip(false);
    let bb = b.clone();
    m.connect_hide(move |_| bb.set_has_tooltip(true));
    m.popup_at_pointer(Some(e));
}

fn open_flyout(v: &Rc<Vol>, b: &gtk::Button) {
    // the columns: the default device, then (Volume Control Options) every other device playing
    // sound and the ticked ones
    let info = v.info.borrow().clone();
    let prefs = sys::volume_prefs();
    let mut sinks: Vec<Option<String>> = vec![None];
    if let Some(i) = &info {
        let def = i.default.clone();
        for s in &i.sinks {
            if s.name == def { continue; }
            if (prefs.all_playing && s.running) || prefs.devices.contains(&s.name) { sinks.push(Some(s.name.clone())); }
        }
    }
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.style_context().add_class("vol-flyout");
    let mut cols = vec![];
    for fixed in sinks {
        let c = column(fixed);
        body.pack_start(&c.root, false, false, 0);
        cols.push(c);
    }
    for c in &cols { wire(v, c); }
    match &info { Some(i) => for c in &cols { update_col(c, i); }, None => for c in &cols { c.slider.set_enabled(false); c.mute.set_sensitive(false); } }
    // "Mixer", centred under the columns
    let mix = link("Mixer", mixer::open);
    mix.set_halign(gtk::Align::Center);
    mix.style_context().add_class("vol-mixer");
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    foot.style_context().add_class("vol-foot");
    foot.set_center_widget(Some(&mix));
    *v.fly.borrow_mut() = cols.clone();
    let w = popup(b.upcast_ref(), body.upcast_ref(), Some(foot.upcast_ref()));
    v.fly_open.set(true);
    {
        let v = v.clone();
        w.connect_destroy(move |_| {
            v.fly.borrow_mut().clear();
            v.fly_open.set(false);
            // the current text, not the one from when the flyout opened
            let t = v.tip.borrow().clone();
            v.button.set_tooltip_text(Some(&t));
        });
    }
    if let Some(c) = cols.first() { c.slider.area.grab_focus(); }
    v.refresh();
}

/// A device column: the device icon as a flat button (tooltip: its name; click: its Properties),
/// the trackbar, the mute button (SndVol ids 300, 304, 305).
fn column(fixed: Option<String>) -> Rc<Col> {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_size_request(74, -1);
    let dev_img = util::image(util::first_icon(&["audio-speakers", "audio-volume-high"]), 32);
    let dev = mixer::flat_button(&dev_img);
    root.pack_start(&dev, false, false, 0);
    let slider = Slider::new(115);
    slider.area.set_margin_top(13);
    slider.area.set_margin_bottom(14);
    root.pack_start(&slider.area, false, false, 0);
    let mute_img = gtk::Image::new();
    let mute = mixer::flat_button(&mute_img);
    mute.set_margin_bottom(13);
    root.pack_start(&mute, false, false, 0);
    let c = Rc::new(Col { root, fixed, sink: RefCell::new(String::new()), title: RefCell::new("Speakers".into()), slider, dev, dev_img, mute, mute_img,
        muted: Cell::new(false), last_user: Cell::new(None) });
    c.show_mute(false);
    c
}

fn wire(v: &Rc<Vol>, c: &Rc<Col>) {
    {
        let c2 = Rc::downgrade(c);
        c.dev.connect_clicked(move |_| {
            let Some(c) = c2.upgrade() else { return };
            let s = c.sink.borrow().clone();
            close_open();
            mixer::device_properties(&s);
        });
    }
    // Win7: pressing, wheeling or key-pressing the slider of a muted device unmutes it first
    {
        let c2 = Rc::downgrade(c);
        c.slider.connect_touched(move || {
            let Some(c) = c2.upgrade() else { return };
            if c.muted.get() {
                c.show_mute(false);
                sys::send(AudioCmd::Mute { sink: c.sink_arg(), mute: Some(false) });
            }
        });
    }
    {
        let c2 = Rc::downgrade(c);
        c.slider.connect_changed(move |pct| {
            let Some(c) = c2.upgrade() else { return };
            c.last_user.set(Some(Instant::now()));
            sys::send(AudioCmd::Level { sink: c.sink_arg(), pct });
        });
    }
    // TB_ENDTRACK: stop any sound, then the Default Beep – after the last level was set
    c.slider.connect_ended(|| {
        if let Some(f) = crate::sounds::file_for(crate::sounds::Event::DefaultBeep) { sys::send(AudioCmd::Play(f)); }
    });
    {
        let (c2, v2) = (Rc::downgrade(c), Rc::downgrade(v));
        c.mute.connect_clicked(move |_| {
            let (Some(c), Some(v)) = (c2.upgrade(), v2.upgrade()) else { return };
            // SetMute(!cached) and repaint from the intended state; the change event confirms it
            let to = !c.muted.get();
            c.show_mute(to);
            sys::send(AudioCmd::Mute { sink: c.sink_arg(), mute: Some(to) });
            let i = v.info.borrow().clone();
            if let Some(mut i) = i {
                let name = c.sink.borrow().clone();
                if let Some(s) = i.sinks.iter_mut().find(|s| s.name == name) { s.muted = to; }
                v.apply(i);
            }
        });
    }
}
