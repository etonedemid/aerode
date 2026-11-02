//! Screen Saver Settings: the monitor picture with a live preview, Screen saver list, Settings...,
//! Preview, Wait N minutes, On resume, display logon screen, Change power settings.
//!
//! Backed by the X screen saver timer (`xset s`, also written to ~/.config/aerode/screensaver-wait
//! for the next logon) and xss-lock, which the session starts with `aerode screensaver`.
use super::*;
use aerode::screensaver::{Saver, SAVERS};
use gtk::cairo;
use std::cell::Cell;

/// Win7's picture of a monitor (black glossy panel on a silver stand), the screen area filled by `screen`.
fn draw_monitor(cr: &cairo::Context, w: f64, h: f64, screen: &dyn Fn(&cairo::Context, f64, f64)) {
    let (mw, mh) = (w * 0.86, h * 0.72);
    let (mx, my) = ((w - mw) / 2.0, 2.0);
    // stand: neck and an elliptic foot
    let neck = cairo::LinearGradient::new(w / 2.0 - 14.0, 0.0, w / 2.0 + 14.0, 0.0);
    neck.add_color_stop_rgb(0.0, 0.55, 0.57, 0.6);
    neck.add_color_stop_rgb(0.5, 0.86, 0.88, 0.9);
    neck.add_color_stop_rgb(1.0, 0.5, 0.52, 0.55);
    cr.rectangle(w / 2.0 - 13.0, my + mh - 4.0, 26.0, h * 0.14);
    cr.set_source(&neck).ok();
    cr.fill().ok();
    cr.save().ok();
    cr.translate(w / 2.0, my + mh + h * 0.17);
    cr.scale(w * 0.3, h * 0.045);
    cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
    cr.restore().ok();
    let foot = cairo::LinearGradient::new(0.0, my + mh + h * 0.13, 0.0, my + mh + h * 0.21);
    foot.add_color_stop_rgb(0.0, 0.92, 0.93, 0.95);
    foot.add_color_stop_rgb(1.0, 0.52, 0.54, 0.58);
    cr.set_source(&foot).ok();
    cr.fill().ok();
    // bezel
    aerode::glass::rounded(cr, mx, my, mw, mh, 6.0);
    let bz = cairo::LinearGradient::new(0.0, my, 0.0, my + mh);
    bz.add_color_stop_rgb(0.0, 0.25, 0.26, 0.28);
    bz.add_color_stop_rgb(0.5, 0.08, 0.08, 0.09);
    bz.add_color_stop_rgb(1.0, 0.16, 0.17, 0.19);
    cr.set_source(&bz).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    // screen
    let (sx, sy, sw, sh) = (mx + 9.0, my + 9.0, mw - 18.0, mh - 22.0);
    cr.save().ok();
    cr.rectangle(sx, sy, sw, sh);
    cr.clip();
    cr.translate(sx, sy);
    screen(cr, sw, sh);
    cr.restore().ok();
    // the glossy reflection across the upper bezel
    aerode::glass::rounded(cr, mx + 1.0, my + 1.0, mw - 2.0, mh * 0.45, 5.0);
    let gl = cairo::LinearGradient::new(0.0, my, 0.0, my + mh * 0.45);
    gl.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.22);
    gl.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
    cr.set_source(&gl).ok();
    cr.fill().ok();
    // power light
    cr.arc(w / 2.0, my + mh - 6.0, 1.6, 0.0, std::f64::consts::TAU);
    cr.set_source_rgb(0.35, 0.75, 1.0);
    cr.fill().ok();
}

pub fn page_screensaver(cp: &Rc<Cp>) -> gtk::Widget {
    cp.side_link("Personalization", "personalization");
    cp.side_link("Power Options", "power");
    let o = config::get().screensaver;
    let sheet = Sheet::new(cp);
    let page = vbox(10);
    // ---- the monitor with a live preview
    let geo = util::primary_geometry();
    let cur: Rc<RefCell<Option<Saver>>> = Rc::new(RefCell::new(None));
    let start = {
        let cur = cur.clone();
        move |kind: &str| {
            *cur.borrow_mut() = if kind.is_empty() { None } else {
                Some(Saver::new(kind, geo.width() as f64, geo.height() as f64, Some(aerode::glass::wallpaper().sharp.clone())))
            };
        }
    };
    start(&o.name);
    let area = gtk::DrawingArea::new();
    area.set_size_request(230, 190);
    area.set_halign(gtk::Align::Center);
    {
        let cur = cur.clone();
        area.connect_draw(move |a, cr| {
            let (w, h) = (a.allocated_width() as f64, a.allocated_height() as f64);
            draw_monitor(cr, w, h, &|cr, sw, sh| match cur.borrow().as_ref() {
                Some(s) => s.draw(cr, sw, sh),
                None => {
                    // (None): the desktop
                    let wp = aerode::glass::wallpaper();
                    cr.scale(sw / wp.sharp.width() as f64, sh / wp.sharp.height() as f64);
                    gtk::gdk::prelude::GdkContextExt::set_source_pixbuf(cr, &wp.sharp, 0.0, 0.0);
                    cr.paint().ok();
                }
            });
            glib::Propagation::Stop
        });
    }
    {
        let cur = cur.clone();
        let last = Cell::new(None::<i64>);
        area.add_tick_callback(move |a, clock| {
            let t = clock.frame_time();
            if let (Some(l), Some(s)) = (last.get(), cur.borrow_mut().as_mut()) { s.step((t - l) as f64 / 1e6); }
            last.set(Some(t));
            if cur.borrow().is_some() { a.queue_draw(); }
            glib::ControlFlow::Continue
        });
    }
    page.pack_start(&area, false, false, 0);
    // ---- Screen saver
    let g = vbox(8);
    let row = hbox(6);
    let list = combo(SAVERS, &o.name);
    if o.name.is_empty() { list.set_active(Some(0)); }
    list.set_hexpand(true);
    let settings = gtk::Button::with_mnemonic("Se_ttings...");
    let preview = gtk::Button::with_mnemonic("Pre_view");
    row.pack_start(&list, true, true, 0);
    row.pack_start(&settings, false, false, 0);
    row.pack_start(&preview, false, false, 0);
    g.pack_start(&row, false, false, 0);
    let row2 = hbox(6);
    let wl = gtk::Label::with_mnemonic("_Wait:");
    let wait = gtk::SpinButton::with_range(1.0, 9999.0, 1.0);
    wait.set_value(o.wait.max(1) as f64);
    wait.set_width_chars(4);
    wl.set_mnemonic_widget(Some(&wait));
    row2.pack_start(&wl, false, false, 0);
    row2.pack_start(&wait, false, false, 0);
    row2.pack_start(&gtk::Label::new(Some("minutes")), false, false, 0);
    let lock = check("On _resume, display logon screen", o.lock);
    row2.pack_start(&lock, false, false, 12);
    g.pack_start(&row2, false, false, 0);
    page.pack_start(&group("Screen saver", &g), false, false, 0);
    sheet.watch_combo(&list);
    sheet.watch_spin(&wait);
    sheet.watch_check(&lock);
    let sync = { let (s, p) = (settings.clone(), preview.clone()); move |id: &str| { s.set_sensitive(!id.is_empty()); p.set_sensitive(!id.is_empty()); } };
    sync(&o.name);
    { let (start, sync) = (start.clone(), sync.clone()); list.connect_changed(move |c| { let id = c.active_id().map(|s| s.to_string()).unwrap_or_default(); start(&id); sync(&id); }); }
    // ---- Power management
    let pm = hbox(10);
    let pimg = util::image("preferences-system-power", 32);
    pimg.set_valign(gtk::Align::Start);
    pm.pack_start(&pimg, false, false, 0);
    let pcol = vbox(4);
    let pt = text("Conserve energy or maximize performance by adjusting display brightness and other power settings.");
    pt.set_max_width_chars(52);
    pcol.pack_start(&pt, false, false, 0);
    { let cp = cp.clone(); pcol.pack_start(&link("Change power settings", move || cp.open("power")), false, false, 0); }
    pm.pack_start(&pcol, true, true, 0);
    page.pack_start(&group("Power management", &pm), false, false, 0);
    // Photos has settings of its own; the others say so, like Win7
    let photos: Rc<RefCell<(String, String, bool)>> = Rc::new(RefCell::new((o.photos_dir.clone(), o.photos_speed.clone(), o.photos_shuffle)));
    {
        let (list, cp, photos, d, start) = (list.clone(), cp.clone(), photos.clone(), sheet.dirty(), start.clone());
        settings.connect_clicked(move |_| {
            let id = list.active_id().map(|s| s.to_string()).unwrap_or_default();
            if id == "photos" {
                if photos_settings(&parent_window(&cp), &photos) {
                    // the preview reads the folder from config: show the new choice after Apply
                    d();
                    start(&id);
                }
            } else {
                let m = gtk::MessageDialog::new(Some(&parent_window(&cp)), gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::Ok,
                    "This screen saver has no options that you can set.");
                m.set_title("Screen Saver");
                m.run();
                m.close();
            }
        });
    }
    {
        let list = list.clone();
        preview.connect_clicked(move |_| {
            if let Some(id) = list.active_id().filter(|s| !s.is_empty()) { util::spawn(&["aerode", "screensaver", "--preview", &id]); }
        });
    }
    sheet.tab("Screen Saver", &page);
    let win = parent_window(cp);
    sheet.on_apply(move || {
        let name = list.active_id().map(|s| s.to_string()).unwrap_or_default();
        let (wait_m, lock_on) = (wait.value() as u32, lock.is_active());
        let ph = photos.borrow().clone();
        config::update(|c| {
            let s = &mut c.screensaver;
            s.name = name.clone();
            s.wait = wait_m;
            s.lock = lock_on;
            (s.photos_dir, s.photos_speed, s.photos_shuffle) = ph;
        });
        // the X screen saver timer fires xss-lock, which runs `aerode screensaver`
        let secs = if name.is_empty() && !lock_on { 0 } else { wait_m * 60 };
        let _ = std::fs::write(config::conf_dir().join("screensaver-wait"), secs.to_string());
        if secs == 0 { util::spawn(&["xset", "s", "off"]); } else { util::spawn(&["xset", "s", &secs.to_string()]); }
        if !util::which("xss-lock") {
            let m = gtk::MessageDialog::new(Some(&win), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::Ok,
                "The screen saver needs xss-lock, which is not installed.");
            m.set_secondary_text(Some("Install it with: sudo pacman -S xss-lock"));
            m.set_title("Screen Saver Settings");
            m.run();
            m.close();
        }
        true
    });
    sheet.widget()
}

/// Win7 "Photos Screen Saver Settings": folder, slide show speed, shuffle. True on Save.
fn photos_settings(parent: &gtk::Window, photos: &Rc<RefCell<(String, String, bool)>>) -> bool {
    let d = gtk::Dialog::with_buttons(Some("Photos Screen Saver Settings"), Some(parent), gtk::DialogFlags::MODAL,
        &[("Save", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_spacing(10);
    c.set_border_width(12);
    let (dir, speed, shuffle) = photos.borrow().clone();
    let pictures = util::special_dir(glib::UserDirectory::Pictures, "Pictures");
    let g = gtk::Grid::new();
    g.set_row_spacing(10);
    g.set_column_spacing(10);
    let l1 = gtk::Label::new(Some("Use pictures from:"));
    l1.set_xalign(0.0);
    g.attach(&l1, 0, 0, 1, 1);
    let folder = gtk::Entry::new();
    folder.set_text(&if dir.is_empty() { pictures.to_string_lossy().into_owned() } else { dir.clone() });
    folder.set_hexpand(true);
    g.attach(&folder, 1, 0, 1, 1);
    let browse = gtk::Button::with_mnemonic("_Browse...");
    g.attach(&browse, 2, 0, 1, 1);
    let l2 = gtk::Label::new(Some("Slide show speed:"));
    l2.set_xalign(0.0);
    g.attach(&l2, 0, 1, 1, 1);
    let sp = combo(&[("slow", "Slow"), ("medium", "Medium"), ("fast", "Fast")], &speed);
    g.attach(&sp, 1, 1, 1, 1);
    let sh = check("_Shuffle pictures", shuffle);
    g.attach(&sh, 0, 2, 2, 1);
    c.pack_start(&g, false, false, 0);
    {
        let (d2, folder) = (d.clone(), folder.clone());
        browse.connect_clicked(move |_| {
            let fc = gtk::FileChooserNative::new(Some("Browse For Folder"), Some(&d2), gtk::FileChooserAction::SelectFolder, Some("OK"), Some("Cancel"));
            let _ = fc.set_current_folder(folder.text().as_str());
            if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() { folder.set_text(&p.to_string_lossy()); } }
        });
    }
    d.show_all();
    let ok = d.run() == gtk::ResponseType::Ok;
    if ok {
        let f = folder.text().to_string();
        let f = if std::path::Path::new(&f) == pictures { String::new() } else { f };
        *photos.borrow_mut() = (f, sp.active_id().map(|s| s.to_string()).unwrap_or_else(|| "medium".into()), sh.is_active());
    }
    d.close();
    ok
}
