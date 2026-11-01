//! Windows Explorer remake.
//!   aero7-explorer [path | computer:/// | trash:/// | network:/// | library:Documents]
//!   aero7-explorer --properties <path>
use gtk::prelude::*;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "--properties").unwrap_or(false) {
        gtk::init().expect("gtk");
        aero7::theme::init_app();
        if let Some(p) = args.get(1) {
            aero7::explorer::props::show(std::path::Path::new(p), None);
            // quit when the dialog closes
            glib::timeout_add_local(std::time::Duration::from_millis(300), || {
                if gtk::Window::list_toplevels().iter().filter(|w| w.is_visible()).count() == 0 { gtk::main_quit(); return glib::ControlFlow::Break; }
                glib::ControlFlow::Continue
            });
            gtk::main();
        }
        return glib::ExitCode::SUCCESS;
    }
    let app = gtk::Application::new(Some("org.aero7.Explorer"), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    app.connect_command_line(|app, cl| {
        aero7::theme::init_app();
        let a: Vec<String> = cl.arguments().iter().skip(1).map(|s| s.to_string_lossy().into_owned()).collect();
        let target = a.first().cloned().unwrap_or_else(|| "library:Documents".into());
        let loc = aero7::explorer::Loc::parse(&target);
        let ex = aero7::explorer::Explorer::new(app, loc);
        std::mem::forget(ex);
        0.into()
    });
    app.run()
}
