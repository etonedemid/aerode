//! aerode-wm – Windows 7 style window manager.
/// Panics go to stderr and to ~/.cache/aerode/aerode-wm.log (with a backtrace) so they can be
/// reported; the event loop catches them and keeps the session alive.
fn log_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let dir = std::env::var_os("XDG_CACHE_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
            .map(|d| d.join("aerode"));
        if let Some(dir) = dir {
            let _ = std::fs::create_dir_all(&dir);
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("aerode-wm.log")) {
                use std::io::Write;
                let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let _ = writeln!(f, "--- {when} aerode-wm {}: {info}\n{}", env!("CARGO_PKG_VERSION"), std::backtrace::Backtrace::force_capture());
            }
        }
    }));
}

fn main() {
    log_panics();
    gtk::init().ok(); // only for the icon theme / pango font setup
    match aerode::wm::Wm::new() {
        Ok(mut wm) => { if let Err(e) = wm.run() { eprintln!("aerode-wm: {e}"); std::process::exit(1); } }
        Err(e) => { eprintln!("aerode-wm: {e:#}"); std::process::exit(1); }
    }
}
