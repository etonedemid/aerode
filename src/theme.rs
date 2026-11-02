//! CSS loading for the shell and aerode's own programs.
use gtk::prelude::*;
use std::path::PathBuf;

pub const SHELL_CSS: &str = include_str!("../data/shell.css");
pub const APP_CSS: &str = include_str!("../data/gtk.css");

/// Where the theme's image assets live: the installed GTK theme, else the source tree.
pub fn assets_dir() -> PathBuf {
    let installed = PathBuf::from("/usr/share/themes/Aerode/gtk-3.0/assets");
    if installed.is_dir() { installed } else { PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/data/gtk-assets")) }
}

pub fn load_css(css: &str) {
    let p = gtk::CssProvider::new();
    // CSS loaded from memory has no base for relative urls (and GTK 3 ignores data: urls)
    let css = css.replace("url(\"assets/", &format!("url(\"file://{}/", assets_dir().display()));
    if let Err(e) = p.load_from_data(css.as_bytes()) {
        eprintln!("aerode: css error: {e}");
    }
    if let Some(s) = gtk::gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(&s, &p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

/// Apply theme defaults for our own programs (fonts, icons, our widget CSS).
pub fn init_app() {
    if let Some(s) = gtk::Settings::default() {
        let icons = glib_icons();
        s.set_gtk_icon_theme_name(Some(&icons));
        s.set_gtk_font_name(Some("Segoe UI 9"));
        s.set_property("gtk-menu-images", true);
        s.set_property("gtk-button-images", true);
    }
    load_css(APP_CSS);
}

fn glib_icons() -> String {
    let local = gtk::glib::user_data_dir().join("icons/Aerode/index.theme");
    if local.exists() { "Aerode".into() } else { "Papirus".into() }
}
