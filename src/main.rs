//! `aero7` – the shell process (desktop, taskbar, start menu) and helper commands.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use aero7::{desktop, dialogs, glass, startmenu, sys, taskbar, theme, util, x11};

const USAGE: &str = "usage: aero7 [shell|start|run|personalize|reload|lock|quit]
       aero7 query-volume | query-network";

struct Shell {
    _desktop: Rc<desktop::Desktop>,
    taskbar: Rc<taskbar::Taskbar>,
    start: Rc<startmenu::StartMenu>,
}

thread_local! { static SHELL: RefCell<Option<Shell>> = RefCell::new(None); }

fn build(app: &gtk::Application) {
    if SHELL.with(|s| s.borrow().is_some()) { return; }
    theme::init_app();
    theme::load_css(theme::SHELL_CSS);
    let wm = match x11::Wm::connect() {
        Ok(w) => Rc::new(w),
        Err(e) => { eprintln!("aero7: cannot connect to X: {e}"); std::process::exit(1) }
    };
    theme::write_openbox_theme();
    let d = desktop::Desktop::new(app);
    let tb = taskbar::Taskbar::new(app, wm);
    let sm = startmenu::StartMenu::new(app);
    { let s = sm.clone(); *tb.orb_cb.borrow_mut() = Some(Rc::new(move || s.toggle())); }
    {
        let w = tb.win.clone();
        *sm.on_hide.borrow_mut() = Some(Box::new(move || w.queue_draw()));
    }
    // follow monitor changes
    if let Some(screen) = gtk::gdk::Screen::default() {
        screen.connect_size_changed(|_| glass::reload());
    }
    aero7::gadgets::restore();
    autostart();
    util::play_sound("Windows Logon Sound.wav");
    SHELL.with(|s| *s.borrow_mut() = Some(Shell { _desktop: d, taskbar: tb, start: sm }));
}

/// XDG autostart (~/.config/autostart, /etc/xdg/autostart), respecting OnlyShowIn/NotShowIn/Hidden.
fn autostart() {
    let mut seen = std::collections::HashSet::new();
    let mut dirs = vec![glib::user_config_dir().join("autostart")];
    for d in glib::system_config_dirs() { dirs.push(d.join("autostart")); }
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".desktop") || !seen.insert(name) { continue; }
            let Some(a) = gio::DesktopAppInfo::from_filename(e.path()) else { continue };
            if a.is_hidden() || !a.shows_in(Some("Aero7")) { continue; }
            if a.boolean("X-GNOME-Autostart-enabled") == false && a.has_key("X-GNOME-Autostart-enabled") { continue; }
            let _ = a.launch(&[], None::<&gio::AppLaunchContext>);
        }
    }
}

fn command(app: &gtk::Application, args: &[String]) -> i32 {
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("shell");
    match cmd {
        "shell" => build(app),
        "start" => { build(app); SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { s.start.toggle() }); }
        "run" => { build(app); dialogs::run_dialog(); }
        "personalize" => { build(app); dialogs::personalize(); }
        "reload" => {
            aero7::config::reload();
            glass::reload();
            theme::write_openbox_theme();
            util::spawn(&["openbox", "--reconfigure"]);
            SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { s.taskbar.refresh() });
        }
        "quit" => app.quit(),
        _ => { eprintln!("{USAGE}"); return 2; }
    }
    0
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // helper commands that must not go through the single-instance shell
    match args.get(1).map(|s| s.as_str()) {
        Some("query-volume") => {
            if let Some((p, m)) = sys::volume() { println!("{p} {}", m as u8); }
            return glib::ExitCode::SUCCESS;
        }
        Some("query-network") => {
            let n = sys::net_status();
            println!("{}\t{}\t{}", n.kind, n.signal, n.name);
            return glib::ExitCode::SUCCESS;
        }
        Some("lock") => {
            gtk::init().expect("gtk");
            aero7::lock::lock();
            return glib::ExitCode::SUCCESS;
        }
        Some("-h") | Some("--help") => { println!("{USAGE}"); return glib::ExitCode::SUCCESS; }
        _ => {}
    }
    let app = gtk::Application::new(Some("org.aero7.Shell"), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    app.connect_command_line(|app, cl| {
        let a: Vec<String> = cl.arguments().iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let r = command(app, &a);
        if SHELL.with(|s| s.borrow().is_some()) { std::mem::forget(app.hold()); }
        r.into()
    });
    app.connect_activate(|app| build(app));
    app.run_with_args(&args)
}
