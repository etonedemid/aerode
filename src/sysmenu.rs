//! The window ("system") menu: right-click on a title bar, or Alt+Space.
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::rc::Rc;

use crate::x11::Wm;

#[derive(Clone, Copy)]
enum Glyph { Restore, Minimize, Maximize, Close }

/// Marlett-style 9px glyph in the menu gutter (Win7 draws these with the Marlett font).
fn glyph(item: &gtk::MenuItem, g: Glyph) {
    item.connect_local("draw", true, move |args| {
        let w = args[0].get::<gtk::Widget>().ok()?;
        let cr = args[1].get::<gtk::cairo::Context>().ok()?;
        let (x, y) = (7.0, ((w.allocated_height() as f64 - 9.0) / 2.0).floor());
        let c = if w.is_sensitive() { 0.0 } else { 0.43 };
        cr.set_source_rgb(c, c, c);
        let frame = |cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64| {
            cr.rectangle(x, y, w, 2.0);
            cr.rectangle(x, y + 2.0, 1.0, h - 2.0);
            cr.rectangle(x + w - 1.0, y + 2.0, 1.0, h - 2.0);
            cr.rectangle(x, y + h - 1.0, w, 1.0);
        };
        match g {
            Glyph::Minimize => cr.rectangle(x + 1.0, y + 7.0, 6.0, 2.0),
            Glyph::Maximize => frame(&cr, x, y, 9.0, 9.0),
            Glyph::Restore => {
                // back window: only the parts the front one does not cover
                cr.rectangle(x + 3.0, y, 6.0, 2.0);
                cr.rectangle(x + 8.0, y + 2.0, 1.0, 4.0);
                cr.rectangle(x + 6.0, y + 5.0, 2.0, 1.0);
                frame(&cr, x, y + 3.0, 6.0, 6.0);
            }
            Glyph::Close => {
                cr.set_line_width(1.6);
                cr.move_to(x + 1.0, y + 1.0); cr.line_to(x + 8.0, y + 8.0);
                cr.move_to(x + 8.0, y + 1.0); cr.line_to(x + 1.0, y + 8.0);
                cr.stroke().ok();
            }
        }
        cr.fill().ok();
        Some(false.to_value())
    });
}

/// Pop up the system menu of client window `w` at root coordinates (x, y).
pub fn show(wm: Rc<Wm>, w: u32, x: i32, y: i32) {
    let (maximized, minimized, resizable, can_min) = wm.window_state(w);
    let m = gtk::Menu::new();
    let add = |label: &str, sensitive: bool, g: Option<Glyph>, f: Box<dyn Fn()>| -> gtk::MenuItem {
        let it = gtk::MenuItem::with_mnemonic(label);
        it.set_sensitive(sensitive);
        if let Some(g) = g { glyph(&it, g); }
        it.connect_activate(move |_| f());
        m.append(&it);
        it
    };
    { let wm = wm.clone(); add("_Restore", maximized || minimized, Some(Glyph::Restore), Box::new(move || if maximized { wm.set_maximized(w, false) } else { wm.activate(w) })); }
    { let wm = wm.clone(); add("_Move", !maximized && !minimized, None, Box::new(move || wm.keyboard_moveresize(w, false))); }
    { let wm = wm.clone(); add("_Size", resizable && !maximized && !minimized, None, Box::new(move || wm.keyboard_moveresize(w, true))); }
    { let wm = wm.clone(); add("Mi_nimize", can_min && !minimized, Some(Glyph::Minimize), Box::new(move || wm.minimize(w))); }
    { let wm = wm.clone(); add("Ma_ximize", resizable && !maximized, Some(Glyph::Maximize), Box::new(move || { wm.activate(w); wm.set_maximized(w, true) })); }
    m.append(&gtk::SeparatorMenuItem::new());
    let close = { let wm = wm.clone(); add("_Close", true, Some(Glyph::Close), Box::new(move || wm.close(w))) };
    if let Some(l) = close.child().and_then(|c| c.downcast::<gtk::AccelLabel>().ok()) {
        l.set_markup_with_mnemonic("<b>_Close</b>");
        l.set_accel(*gdk::keys::constants::F4, gdk::ModifierType::MOD1_MASK);
    }
    m.show_all();
    popup(m, x, y, 0);
}

/// GTK gives up on a popup whose grab fails; the WM may still hold the button/key that opened it.
fn popup(m: gtk::Menu, x: i32, y: i32, tries: u32) {
    let Some(root) = gdk::Screen::default().and_then(|s| s.root_window()) else { return };
    // anchored to the root window GTK would shrink the menu to a few rows; flip/slide instead
    m.set_anchor_hints(gdk::AnchorHints::FLIP | gdk::AnchorHints::SLIDE);
    m.popup_at_rect(&root, &gdk::Rectangle::new(x, y, 1, 1), gdk::Gravity::NorthWest, gdk::Gravity::NorthWest, None);
    if !m.is_visible() && tries < 15 {
        glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || popup(m, x, y, tries + 1));
    }
}
