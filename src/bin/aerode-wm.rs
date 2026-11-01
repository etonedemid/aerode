//! aerode-wm – Windows 7 style window manager.
fn main() {
    gtk::init().ok(); // only for the icon theme / pango font setup
    match aerode::wm::Wm::new() {
        Ok(mut wm) => { if let Err(e) = wm.run() { eprintln!("aerode-wm: {e}"); std::process::exit(1); } }
        Err(e) => { eprintln!("aerode-wm: {e:#}"); std::process::exit(1); }
    }
}
