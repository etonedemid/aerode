//! Lock screen: renders a Win7-style "Locked" image and hands off to i3lock
//! (a proven, secure locker) – we never implement password handling ourselves.
use gtk::cairo;
use gtk::gdk::prelude::GdkContextExt;
use gtk::glib;

use crate::{config, glass, util};

pub fn lock() {
    let g = util::primary_geometry();
    let (w, h) = (g.width(), g.height());
    let bg = config::data_file("wallpapers/../lockscreen.jpg")
        .and_then(|p| gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, w, h, false).ok())
        .unwrap_or_else(|| glass::wallpaper().sharp.clone());
    let surf = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h).unwrap();
    {
        let cr = cairo::Context::new(&surf).unwrap();
        cr.set_source_pixbuf(&bg, 0.0, 0.0);
        cr.paint().ok();
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0 - 60.0);
        // avatar frame
        let face = config::home().join(".face");
        let av = if face.exists() { Some(face) } else { config::data_file("avatar.png") };
        if let Some(p) = av.and_then(|p| gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(p, 126, 126, false).ok()) {
            glass::rounded(&cr, cx - 72.0, cy - 72.0, 144.0, 144.0, 8.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
            cr.fill_preserve().ok();
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.5);
            cr.stroke().ok();
            cr.set_source_pixbuf(&p, cx - 63.0, cy - 63.0);
            cr.paint().ok();
        }
        let user = glib::real_name().to_string_lossy().into_owned();
        let user = if user.is_empty() || user == "Unknown" { glib::user_name().to_string_lossy().into_owned() } else { user };
        for (text, size, dy) in [(user.as_str(), 26.0, 110.0), ("Locked", 15.0, 145.0)] {
            cr.select_font_face("Segoe UI", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            cr.set_font_size(size);
            let ext = cr.text_extents(text).unwrap();
            cr.move_to(cx - ext.width() / 2.0 + 1.0, cy + dy + 1.0);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
            cr.show_text(text).ok();
            cr.move_to(cx - ext.width() / 2.0, cy + dy);
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.show_text(text).ok();
        }
    }
    let cache = glib::user_cache_dir().join("aerode");
    let _ = std::fs::create_dir_all(&cache);
    let png = cache.join("lock.png");
    if let Ok(mut f) = std::fs::File::create(&png) { let _ = surf.write_to_png(&mut f); }
    let p = png.to_string_lossy().into_owned();
    let ok = std::process::Command::new("i3lock")
        .args(["-n", "-e", "-f", "-i", &p, "--color=1d4f7a"])
        .status()
        .or_else(|_| std::process::Command::new("xsecurelock").status())
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = std::process::Command::new("loginctl").arg("lock-session").status();
    }
}
