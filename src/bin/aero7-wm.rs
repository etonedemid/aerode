//! aero7-wm – Windows 7 style window manager.
fn main() {
    gtk::init().ok(); // only for the icon theme / pango font setup
    match aero7::wm::Wm::new() {
        Ok(mut wm) => { if let Err(e) = wm.run() { eprintln!("aero7-wm: {e}"); std::process::exit(1); } }
        Err(e) => { eprintln!("aero7-wm: {e:#}"); std::process::exit(1); }
    }
}
