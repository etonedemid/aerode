use aerode::wm::decor::*;
fn main() {
    let d = Decor::load();
    println!("themed: {}", d.themed());
    let (w, h) = (520, 300);
    let s = cairo::ImageSurface::create(cairo::Format::ARgb32, 1100, 340).unwrap();
    let cr = cairo::Context::new(&s).unwrap();
    // fake desktop background
    let bg = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(aerode::config::data_dir().join("wallpaper.jpg"), 1100, 620, false).unwrap();
    gtk::gdk::prelude::GdkContextExt::set_source_pixbuf(&cr, &bg, 0.0, -150.0);
    cr.paint().unwrap();
    for (i, (active, st)) in [(true, BState::Hot), (false, BState::Normal)].iter().enumerate() {
        cr.save().unwrap();
        cr.translate(20.0 + i as f64 * 550.0, 20.0);
        let btns = [(Btn::Min, BState::Normal), (Btn::Max, BState::Normal), (Btn::Close, *st)];
        // the WM's glass: Win7's default Sky colourisation, painted as for a compositor
        let g = aerode::wm::glass::Glass::new(aerode::dwmcolor::Colorization::default(), d.theme);
        let glass = |cr: &cairo::Context| g.fill(cr, 0.0, 0.0, *active, false, true);
        d.render(&cr, &Params { w, h, title: "Untitled - Notepad", icon: None, active: *active, maximized: false, small: false, buttons: &btns,
            composited: true, glass: &glass, screen_pos: (0, 0), screen_size: (1100, 620) });
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(BORDER as f64, TOP as f64, (w - 2 * BORDER) as f64, (h - TOP - BORDER) as f64);
        cr.fill().unwrap();
        cr.restore().unwrap();
    }
    drop(cr);
    s.write_to_png(&mut std::fs::File::create("/tmp/claude-0/frame.png").unwrap()).unwrap();
}
