//! Run dialog and Personalization (wallpaper + window colour).
use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf::Pixbuf, glib};
use std::path::PathBuf;

use crate::{config, glass, util};

pub fn run_dialog() {
    let d = gtk::Dialog::new();
    d.set_title("Run");
    d.set_resizable(false);
    d.set_keep_above(true);
    d.set_icon_name(Some("system-run"));
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(12);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    top.pack_start(&util::image("system-run", 32), false, false, 0);
    let l = gtk::Label::new(Some("Type the name of a program, folder, document, or Internet\nresource, and Windows will open it for you."));
    l.set_xalign(0.0);
    top.pack_start(&l, false, false, 0);
    c.pack_start(&top, false, false, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.pack_start(&gtk::Label::new(Some("Open:")), false, false, 0);
    let combo = gtk::ComboBoxText::with_entry();
    let hist_file = config::conf_dir().join("run-history");
    let hist: Vec<String> = std::fs::read_to_string(&hist_file).unwrap_or_default().lines().map(String::from).collect();
    for h in &hist { combo.append_text(h); }
    let entry = combo.child().unwrap().downcast::<gtk::Entry>().unwrap();
    entry.set_activates_default(true);
    if let Some(h) = hist.first() { entry.set_text(h); entry.select_region(0, -1); }
    // command completion from $PATH
    let store = gtk::ListStore::new(&[glib::Type::STRING]);
    let mut seen = std::collections::BTreeSet::new();
    if let Some(p) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&p) {
            if let Ok(rd) = std::fs::read_dir(dir) { for e in rd.flatten() { seen.insert(e.file_name().to_string_lossy().into_owned()); } }
        }
    }
    for s in seen { store.set(&store.append(), &[(0, &s)]); }
    let comp = gtk::EntryCompletion::new();
    comp.set_model(Some(&store));
    comp.set_text_column(0);
    comp.set_inline_completion(true);
    comp.set_popup_completion(false);
    entry.set_completion(Some(&comp));
    row.pack_start(&combo, true, true, 0);
    c.pack_start(&row, false, false, 0);
    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let browse = d.add_button("Browse...", gtk::ResponseType::Other(1));
    let _ = browse;
    d.set_default_response(gtk::ResponseType::Ok);
    let g = util::primary_geometry();
    d.move_(g.x() + 12, g.y() + g.height() - config::get().taskbar_height - 200);
    d.connect_response(move |d, r| match r {
        gtk::ResponseType::Ok => {
            let t = entry.text().to_string();
            if util::run_command(&t) {
                let mut h: Vec<String> = hist.clone();
                h.retain(|x| x != &t);
                h.insert(0, t);
                h.truncate(26);
                let _ = std::fs::create_dir_all(config::conf_dir());
                let _ = std::fs::write(&hist_file, h.join("\n"));
                d.close();
            }
        }
        gtk::ResponseType::Other(1) => {
            let fc = gtk::FileChooserNative::new(Some("Browse"), Some(d), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
            if fc.run() == gtk::ResponseType::Accept {
                if let Some(p) = fc.filename() { entry.set_text(&glib::shell_quote(&p).to_string_lossy()); }
            }
        }
        _ => d.close(),
    });
    d.show_all();
    d.present();
}

const COLORS: &[(&str, [f64; 3])] = &[
    ("Sky", [0.10, 0.22, 0.36]), ("Twilight", [0.05, 0.13, 0.35]), ("Sea", [0.03, 0.33, 0.38]),
    ("Leaf", [0.13, 0.35, 0.10]), ("Lime", [0.40, 0.52, 0.05]), ("Sun", [0.62, 0.55, 0.05]),
    ("Pumpkin", [0.62, 0.32, 0.04]), ("Ruby", [0.50, 0.04, 0.06]), ("Fuchsia", [0.52, 0.05, 0.40]),
    ("Blush", [0.62, 0.35, 0.48]), ("Violet", [0.30, 0.10, 0.50]), ("Lavender", [0.48, 0.45, 0.62]),
    ("Taupe", [0.42, 0.37, 0.28]), ("Chocolate", [0.24, 0.12, 0.05]), ("Slate", [0.25, 0.27, 0.30]),
    ("Frost", [0.82, 0.86, 0.90]),
];

fn wallpaper_dirs() -> Vec<PathBuf> {
    vec![
        config::data_dir().join("wallpapers"),
        util::special_dir(glib::UserDirectory::Pictures, "Pictures"),
        PathBuf::from("/usr/share/backgrounds"),
        PathBuf::from("/usr/share/wallpapers"),
        PathBuf::from("/usr/share/endeavouros/backgrounds"),
    ]
}

fn collect_images(dir: &PathBuf, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 3 { collect_images(&p, out, depth + 1); continue; }
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ["jpg", "jpeg", "png", "webp"].contains(&ext.as_str()) && out.len() < 300 { out.push(p); }
    }
}

thread_local! { static PERS: std::cell::RefCell<Option<gtk::Window>> = std::cell::RefCell::new(None); }

pub fn personalize() {
    if let Some(w) = PERS.with(|p| p.borrow().clone()) { w.present(); return; }
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title("Personalization");
    w.set_icon_name(Some("preferences-desktop-wallpaper"));
    w.set_default_size(760, 560);
    let nb = gtk::Notebook::new();
    w.add(&nb);

    // ---- desktop background
    let bg = gtk::Box::new(gtk::Orientation::Vertical, 8);
    bg.set_border_width(12);
    let h = gtk::Label::new(None);
    h.set_markup("<span size='large' foreground='#1e3287'>Choose your desktop background</span>\nClick a picture to make it your desktop background.");
    h.set_xalign(0.0);
    bg.pack_start(&h, false, false, 0);
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::Single);
    flow.set_max_children_per_line(6);
    flow.set_homogeneous(true);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.add(&flow);
    bg.pack_start(&sw, true, true, 0);
    let mut imgs = vec![];
    for d in wallpaper_dirs() { collect_images(&d, &mut imgs, 0); }
    let paths = std::rc::Rc::new(imgs);
    // load thumbnails lazily so the window opens instantly
    {
        let (flow, paths) = (flow.clone(), paths.clone());
        let idx = std::cell::Cell::new(0usize);
        glib::idle_add_local(move || {
            let i = idx.get();
            if i >= paths.len() { return glib::ControlFlow::Break; }
            idx.set(i + 1);
            if let Ok(p) = Pixbuf::from_file_at_scale(&paths[i], 128, 80, true) {
                let img = gtk::Image::from_pixbuf(Some(&p));
                img.set_tooltip_text(paths[i].file_name().and_then(|n| n.to_str()));
                flow.insert(&img, -1);
                img.show();
            }
            glib::ControlFlow::Continue
        });
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.pack_start(&gtk::Label::new(Some("Picture position:")), false, false, 0);
    let pos = gtk::ComboBoxText::new();
    for (id, l) in [("fill", "Fill"), ("fit", "Fit"), ("stretch", "Stretch"), ("center", "Center")] { pos.append(Some(id), l); }
    pos.set_active_id(Some(&config::get().wallpaper_mode));
    row.pack_start(&pos, false, false, 0);
    let browse = gtk::Button::with_label("Browse...");
    row.pack_end(&browse, false, false, 0);
    bg.pack_start(&row, false, false, 0);
    {
        let paths = paths.clone();
        flow.connect_child_activated(move |_, c| {
            let img = c.child().and_then(|w| w.downcast::<gtk::Image>().ok());
            let name = img.and_then(|i| i.tooltip_text()).map(|s| s.to_string());
            if let Some(p) = paths.iter().find(|p| p.file_name().and_then(|n| n.to_str()) == name.as_deref()) {
                let s = p.to_string_lossy().into_owned();
                config::update(|c| c.wallpaper = Some(s));
                glass::reload();
            }
        });
    }
    pos.connect_changed(|c| { if let Some(id) = c.active_id() { config::update(|cf| cf.wallpaper_mode = id.to_string()); glass::reload(); } });
    {
        let w2 = w.clone();
        browse.connect_clicked(move |_| {
            let fc = gtk::FileChooserNative::new(Some("Browse"), Some(&w2), gtk::FileChooserAction::Open, Some("Open"), Some("Cancel"));
            let f = gtk::FileFilter::new();
            f.add_pixbuf_formats();
            fc.add_filter(f);
            if fc.run() == gtk::ResponseType::Accept {
                if let Some(p) = fc.filename() { config::update(|c| c.wallpaper = Some(p.to_string_lossy().into_owned())); glass::reload(); }
            }
        });
    }
    nb.append_page(&bg, Some(&gtk::Label::new(Some("Desktop Background"))));

    // ---- window colour
    let col = gtk::Box::new(gtk::Orientation::Vertical, 10);
    col.set_border_width(12);
    let h = gtk::Label::new(None);
    h.set_markup("<span size='large' foreground='#1e3287'>Change the color of your window borders, Start menu, and taskbar</span>");
    h.set_xalign(0.0);
    col.pack_start(&h, false, false, 0);
    let grid = gtk::FlowBox::new();
    grid.set_max_children_per_line(8);
    grid.set_selection_mode(gtk::SelectionMode::None);
    let intensity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.15, 0.9, 0.01);
    intensity.set_value(config::get().tint[3]);
    for (name, rgb) in COLORS {
        let b = gtk::Button::new();
        let da = gtk::DrawingArea::new();
        da.set_size_request(48, 48);
        let rgb = *rgb;
        da.connect_draw(move |_, cr| {
            glass::rounded(cr, 1.0, 1.0, 46.0, 46.0, 5.0);
            cr.set_source_rgb(rgb[0], rgb[1], rgb[2]);
            cr.fill_preserve().ok();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
            cr.stroke().ok();
            glib::Propagation::Proceed
        });
        b.add(&da);
        b.set_tooltip_text(Some(name));
        let inten = intensity.clone();
        b.connect_clicked(move |_| {
            let a = inten.value();
            config::update(|c| c.tint = [rgb[0], rgb[1], rgb[2], a]);
            apply_tint();
        });
        grid.insert(&b, -1);
    }
    col.pack_start(&grid, false, false, 0);
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    r.pack_start(&gtk::Label::new(Some("Color intensity:")), false, false, 0);
    r.pack_start(&intensity, true, true, 0);
    intensity.connect_value_changed(|s| { let v = s.value(); config::update(|c| c.tint[3] = v); apply_tint(); });
    col.pack_start(&r, false, false, 0);
    let sounds = gtk::Label::new(Some("Tip: sounds, screen saver and mouse pointers are in Control Panel."));
    sounds.set_xalign(0.0);
    col.pack_end(&sounds, false, false, 0);
    nb.append_page(&col, Some(&gtk::Label::new(Some("Window Color"))));

    w.connect_destroy(|_| PERS.with(|p| *p.borrow_mut() = None));
    w.show_all();
    PERS.with(|p| *p.borrow_mut() = Some(w));
}

/// Push the tint to every glass surface and the window-frame theme.
fn apply_tint() {
    glass::reload();
    crate::theme::write_openbox_theme();
    util::spawn(&["openbox", "--reconfigure"]);
    let _ = gdk::Display::default();
}
