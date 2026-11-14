//! aerode-wm - the Windows 7 style Wayland compositor. Everything after `--` runs inside it.
fn main() {
    aerode::util::log_panics("aerode-wm");
    if let Err(e) = aerode::wm::run() { eprintln!("aerode-wm: {e:#}"); std::process::exit(1); }
}
