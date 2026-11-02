//! AutoPlay: "Choose what happens when you insert each type of media or device". The choices are
//! saved in config.json; the shell (src/autoplay.rs) acts on them when media is inserted.
use gtk::prelude::*;
use gtk::glib;
use std::rc::Rc;

use aerode::autoplay::{choices, MEDIA};
use aerode::{config, util};
use crate::{section, title, Cp};
use super::common::*;

/// A combo with the 16 px icons of the choices, like the Win7 one.
fn combo(m: &'static aerode::autoplay::Media, current: &str) -> gtk::ComboBox {
    let store = gtk::ListStore::new(&[gtk::gdk_pixbuf::Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
    for c in choices(m, true) {
        let pb = match &c.app { Some(a) => util::gicon_pixbuf(a.icon().as_ref(), 16), None => util::pixbuf_or(util::first_icon(&[c.icon, "folder"]), 16, "folder") };
        store.insert_with_values(None, &[(0, &pb), (1, &c.label()), (2, &c.id)]);
    }
    let cb = gtk::ComboBox::with_model(&store);
    let pr = gtk::CellRendererPixbuf::new();
    CellLayoutExt::pack_start(&cb, &pr, false);
    CellLayoutExt::add_attribute(&cb, &pr, "pixbuf", 0);
    let tr = gtk::CellRendererText::new();
    tr.set_property("ellipsize", gtk::pango::EllipsizeMode::End);
    CellLayoutExt::pack_start(&cb, &tr, true);
    CellLayoutExt::add_attribute(&cb, &tr, "text", 1);
    cb.set_id_column(2);
    if !cb.set_active_id(Some(current)) { cb.set_active_id(Some("ask")); }
    cb.set_size_request(340, -1);
    cb
}

pub fn page_autoplay(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    config::reload();
    let cfg = config::get();
    let v = vbox(6);
    v.pack_start(&title("Choose what happens when you insert each type of media or device"), false, false, 0);
    let use_ap = gtk::CheckButton::with_mnemonic("_Use AutoPlay for all media and devices");
    use_ap.set_active(cfg.autoplay);
    v.pack_start(&use_ap, false, false, 0);
    let mut combos: Vec<(&'static str, gtk::ComboBox)> = vec![];
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
    for (head, devices) in [("Media", false), ("Devices", true)] {
        rows.pack_start(&section(head), false, false, 4);
        let g = gtk::Grid::new();
        g.set_row_spacing(6);
        g.set_column_spacing(10);
        g.set_margin_start(16);
        for (i, m) in MEDIA.iter().filter(|m| m.device == devices).enumerate() {
            g.attach(&util::image(util::first_icon(&[m.icon, "media-optical", "drive-removable-media"]), 32), 0, i as i32, 1, 1);
            let l = gtk::Label::new(Some(m.name));
            l.set_xalign(0.0);
            l.set_width_chars(22);
            g.attach(&l, 1, i as i32, 1, 1);
            let cur = cfg.autoplay_actions.get(m.key).map(|s| s.as_str()).unwrap_or("ask");
            let cb = combo(m, cur);
            cb.set_valign(gtk::Align::Center);
            g.attach(&cb, 2, i as i32, 1, 1);
            combos.push((m.key, cb));
        }
        rows.pack_start(&g, false, false, 0);
    }
    { let rows = rows.clone(); use_ap.connect_toggled(move |c| rows.set_sensitive(c.is_active())); }
    rows.set_sensitive(cfg.autoplay);
    v.pack_start(&rows, false, false, 0);
    let bar = hbox(6);
    bar.set_margin_top(16);
    let reset = gtk::Button::with_mnemonic("_Reset all defaults");
    { let (combos, use_ap) = (combos.clone(), use_ap.clone()); reset.connect_clicked(move |_| {
        use_ap.set_active(true);
        for (_, cb) in &combos { cb.set_active_id(Some("ask")); }
    }); }
    bar.pack_start(&reset, false, false, 0);
    let save = gtk::Button::with_mnemonic("_Save");
    let cancel = gtk::Button::with_label("Cancel");
    let back = { let me = cp.clone(); move || {
        let prev = { let mut h = me.history.borrow_mut(); h.pop(); h.pop() };
        me.open(&prev.unwrap_or_else(|| "cat:hardware".into()));
    } };
    { let (combos, use_ap, back) = (combos.clone(), use_ap.clone(), back.clone()); save.connect_clicked(move |_| {
        config::reload();
        config::update(|c| {
            c.autoplay = use_ap.is_active();
            for (k, cb) in &combos {
                match cb.active_id() {
                    Some(id) if id != "ask" => { c.autoplay_actions.insert(k.to_string(), id.to_string()); }
                    _ => { c.autoplay_actions.remove(*k); }
                }
            }
        });
        back();
    }); }
    cancel.connect_clicked(move |_| back());
    bar.pack_end(&button_row(&[&save, &cancel]), false, false, 0);
    v.pack_start(&bar, false, false, 0);
    v.upcast()
}
