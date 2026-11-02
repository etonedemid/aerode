//! Windows Explorer remake.
//!   aerode-explorer [path | computer:/// | trash:/// | network:/// | libraries: | library:Documents]
//!   aerode-explorer --select <path>       open the folder of <path> with it selected
//!   aerode-explorer --properties <path>…
use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    aerode::util::log_panics("aerode-explorer");
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "--properties").unwrap_or(false) {
        gtk::init().expect("gtk");
        aerode::theme::init_app();
        aerode::theme::load_css(aerode::explorer::CSS);
        let cwd = std::env::current_dir().unwrap_or_default();
        let paths: Vec<std::path::PathBuf> = args[1..].iter().map(|p| cwd.join(p)).collect();
        if !paths.is_empty() {
            aerode::explorer::props::show(&paths, None);
            // quit when the dialog closes
            glib::timeout_add_local(std::time::Duration::from_millis(300), || {
                if gtk::Window::list_toplevels().iter().filter(|w| w.is_visible()).count() == 0 { gtk::main_quit(); return glib::ControlFlow::Break; }
                glib::ControlFlow::Continue
            });
            gtk::main();
        }
        return glib::ExitCode::SUCCESS;
    }
    // debugging aid: AERODE_STALL_PROBE=1 reports main loop stalls (GUI thread blocked) on stderr
    if std::env::var_os("AERODE_STALL_PROBE").is_some() {
        let last = std::rc::Rc::new(std::cell::Cell::new(std::time::Instant::now()));
        let t0 = std::time::Instant::now();
        glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
            let d = last.get().elapsed();
            if d > std::time::Duration::from_millis(120) { eprintln!("STALL {:>6} ms at t={:.2}s", d.as_millis(), t0.elapsed().as_secs_f64()); }
            last.set(std::time::Instant::now());
            glib::ControlFlow::Continue
        });
    }
    // one process for all Explorer windows (shared undo, clipboard state, caches)
    let app = gtk::Application::new(Some("org.aerode.Explorer"), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    app.connect_command_line(|app, cl| {
        aerode::theme::init_app();
        let a: Vec<String> = cl.arguments().iter().skip(1).map(|s| s.to_string_lossy().into_owned()).collect();
        let (select, target) = match a.first().map(|s| s.as_str()) {
            Some("--select") | Some("/select") => (a.get(1).cloned(), a.get(1).cloned()),
            _ => (None, a.first().cloned()),
        };
        let target = target.unwrap_or_else(|| "libraries:".into());
        // relative paths are relative to where the command was run, not to the running instance
        let resolve = |t: &str| -> String {
            if t.contains("://") || t.contains(':') && !t.starts_with('/') || t.starts_with('/') || t.starts_with('~') { return t.to_string(); }
            cl.create_file_for_arg(t).path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| t.to_string())
        };
        let mut target = resolve(&target);
        let sel = select.map(|s| std::path::PathBuf::from(resolve(&s)));
        if let Some(s) = &sel { if let Some(p) = s.parent() { target = p.to_string_lossy().into_owned(); } }
        let loc = aerode::explorer::Loc::parse(&target);
        let ex = aerode::explorer::Explorer::new(app, loc);
        if let Some(s) = sel { ex.select_when_shown(s); }
        0.into()
    });
    app.run()
}
