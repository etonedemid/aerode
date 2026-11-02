//! `aerode` – the shell process (desktop, taskbar, start menu) and helper commands.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use aerode::{desktop, dialogs, glass, startmenu, sys, taskbar, theme, util, x11};

const USAGE: &str = "usage: aerode [shell|start|run|personalize|reload|lock|quit|restart]
       aerode sysmenu WINDOW X Y | taskbar-launch N
       aerode query-volume | query-network";

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
        Err(e) => { eprintln!("aerode: cannot connect to X: {e}"); std::process::exit(1) }
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
    aerode::gadgets::restore();
    autostart();
    util::sound(aerode::sounds::Event::Logon);
    aerode::sounds::start_watchers();
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
            // the session starts the compositor with aerode's config; picom's own autostart entry would race it
            let exe = a.executable().file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            if matches!(exe.as_str(), "picom" | "compton" | "xcompmgr" | "compiz") { continue; }
            if a.is_hidden() || !a.shows_in(Some("Aerode")) { continue; }
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
            aerode::config::reload();
            glass::reload();
            theme::write_openbox_theme();
            util::spawn(&["openbox", "--reconfigure"]);
            SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { s.taskbar.refresh() });
        }
        "quit" => app.quit(),
        "sysmenu" => {
            build(app);
            let n = |i: usize| args.get(i).and_then(|s| s.parse::<i64>().ok());
            if let (Some(w), Some(x), Some(y)) = (n(2), n(3), n(4)) {
                SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { aerode::sysmenu::show(s.taskbar.wm.clone(), w as u32, x as i32, y as i32) });
            }
        }
        "taskbar-launch" => {
            build(app);
            if let Some(i) = args.get(2).and_then(|s| s.parse::<usize>().ok()) {
                SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { s.taskbar.launch_index(i) });
            }
        }
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
            aerode::lock::lock();
            return glib::ExitCode::SUCCESS;
        }
        Some("restart") => { restart_session(); return glib::ExitCode::SUCCESS; }
        Some("-h") | Some("--help") => { println!("{USAGE}"); return glib::ExitCode::SUCCESS; }
        _ => {}
    }
    let app = gtk::Application::new(Some("org.aerode.Shell"), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    app.connect_command_line(|app, cl| {
        let a: Vec<String> = cl.arguments().iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let r = command(app, &a);
        if SHELL.with(|s| s.borrow().is_some()) { std::mem::forget(app.hold()); }
        r.into()
    });
    app.connect_activate(|app| build(app));
    app.run_with_args(&args)
}

/// `aerode restart`: restart the window manager in place, the shell and the compositor,
/// without logging out (open windows are kept).
fn restart_session() {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask};
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aerode".into());
    let detached = |argv: &[&str]| {
        use std::os::unix::process::CommandExt;
        let _ = std::process::Command::new(argv[0]).args(&argv[1..]).stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0).spawn();
    };
    // 1. window manager
    let mut wm_ok = false;
    if let Ok((conn, screen)) = x11rb::connect(None) {
        let root = conn.setup().roots[screen].root;
        let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).ok().and_then(|c| c.reply().ok()).map(|r| r.atom).unwrap_or(0);
        let check = atom("_NET_SUPPORTING_WM_CHECK");
        let restart = atom("_AERODE_RESTART");
        let win = conn.get_property(false, root, check, AtomEnum::WINDOW, 0, 1).ok().and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0);
        let name = conn.get_property(false, win, atom("_NET_WM_NAME"), atom("UTF8_STRING"), 0, 64).ok().and_then(|c| c.reply().ok()).map(|r| r.value).unwrap_or_default();
        if win != 0 && name == b"Aerode" {
            let ev = ClientMessageEvent::new(32, win, restart, [0u32; 5]);
            wm_ok = conn.send_event(false, win, EventMask::NO_EVENT, ev).is_ok() && conn.flush().is_ok();
        }
    }
    if wm_ok { println!("aerode: window manager restarting"); } else { println!("aerode: window manager not running, starting it"); detached(&["aerode-wm"]); }
    // 2. shell (taskbar, desktop, start menu)
    let _ = std::process::Command::new(&exe).arg("quit").status();
    std::thread::sleep(std::time::Duration::from_millis(600));
    for pid in util::output(&["pgrep", "-x", "aerode"]).unwrap_or_default().lines() {
        if pid.trim() != std::process::id().to_string() { let _ = std::process::Command::new("kill").arg(pid.trim()).status(); }
    }
    detached(&[&exe, "shell"]);
    println!("aerode: shell restarted");
    // 3. compositor
    if util::which("picom") {
        let _ = std::process::Command::new("pkill").args(["-x", "picom"]).status();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let user = aerode::config::conf_dir().join("picom.conf");
        let user = user.to_string_lossy().into_owned();
        let conf = [user.as_str(), "/usr/share/aerode/picom.conf"].into_iter().find(|p| std::path::Path::new(p).exists());
        if !aerode::config::conf_dir().join("no-compositor").exists() {
            match conf { Some(c) => detached(&["picom", "--config", c]), None => detached(&["picom"]) }
            println!("aerode: compositor restarted");
        }
    }
}
