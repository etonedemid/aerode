//! Render two Win7 frames to a PNG (active with the close button half-way into its hover glow,
//! inactive): `cargo run --example frame -- [out.png]`.
use aerode::wm::decor::*;
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "frame.png".into());
    let d = Decor::load();
    println!("themed: {}", d.themed());
    let (w, h) = (520, 300);
    let s = cairo::ImageSurface::create(cairo::Format::ARgb32, 1100, 340).unwrap();
    let cr = cairo::Context::new(&s).unwrap();
    // fake desktop background
    cr.set_source_rgb(0.12, 0.42, 0.75);
    cr.paint().unwrap();
    // the frame's glass: a plain Win7 sky tint here (aerode-wm fills it with the DWM formula)
    let glass = |cr: &cairo::Context| { cr.set_source_rgba(0.45, 0.72, 0.99, 0.42); cr.paint().ok(); 0.0 };
    for (i, (active, st, glow)) in [(true, BState::Hot, 0.5), (false, BState::Normal, 0.0)].iter().enumerate() {
        cr.save().unwrap();
        cr.translate(20.0 + i as f64 * 550.0, 20.0);
        let btns = [(Btn::Min, BState::Normal), (Btn::Max, BState::Normal), (Btn::Close, *st)];
        d.render(&cr, &Params { w, h, title: "Untitled - Notepad", icon: None, active: *active, maximized: false, small: false,
            buttons: &btns, glow: &[(Btn::Close, *glow)], composited: true, glass: &glass, screen_pos: (0, 0), screen_size: (1100, 340) });
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(BORDER as f64, TOP as f64, (w - 2 * BORDER) as f64, (h - TOP - BORDER) as f64);
        cr.fill().unwrap();
        cr.restore().unwrap();
    }
    drop(cr);
    s.write_to_png(&mut std::fs::File::create(&out).unwrap()).unwrap();
    println!("wrote {out}");
}
