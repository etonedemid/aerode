//! Print where GTK/GDK believes its toplevel is (from ConfigureNotify) next to where the X server
//! says it is – they differ when the window manager forgets ICCCM's synthetic ConfigureNotify.
//!   cargo run --example gtkpos
use gtk::prelude::*;

fn main() {
    gtk::init().expect("gtk");
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title("gtkpos");
    w.set_default_size(300, 200);
    w.show_all();
    let w2 = w.clone();
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
        let gw = w2.window().unwrap();
        let (cx, cy) = gw.position();
        let (_, ox, oy) = gw.origin();
        println!("gdk cached position: {cx},{cy}   server origin: {ox},{oy}   {}", if (cx, cy) == (ox, oy) { "OK" } else { "MISMATCH" });
        gtk::main_quit();
    });
    gtk::main();
}
