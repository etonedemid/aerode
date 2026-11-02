//! `aerode` – the shell process (desktop, taskbar, start menu) and helper commands.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use aerode::{desktop, dialogs, glass, startmenu, sys, taskbar, theme, util, x11};

const USAGE: &str = "usage: aerode [shell|start|run|personalize|window-color|gadgets|reload|lock|quit|restart]
       aerode sysmenu WINDOW X Y | taskbar-launch N | screensaver [--preview NAME] | autoplay DIR
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
    // the saved window colour goes live for every glass painter (the WM, Explorer, …); this also
    // drops an unsaved preview left behind by a shell that died with Personalize open
    glass::publish(Some(&aerode::config::get().colorization.dwm()));
    // keyboard layouts set up in Plasma would otherwise be lost under aerode
    aerode::kbdlayout::import_plasma();
    aerode::kbdlayout::ensure();
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
    // notification daemon (balloon tips above the notification area); before autostart so
    // that programs started there find it instead of D-Bus-activating another daemon
    aerode::notify::start(&tb.tray, tb.wm.clone());
    autostart();
    util::sound(aerode::sounds::Event::Logon);
    aerode::sounds::start_watchers();
    // AutoPlay for discs, drives and devices plugged in from now on
    aerode::autoplay::watch();
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

/// Is another process's aerode taskbar on this display? (Then this one was meant to reach it over
/// D-Bus and must not build a second desktop and taskbar.)
fn other_shell_running() -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let Ok((conn, screen)) = x11rb::connect(None) else { return false };
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).ok().and_then(|c| c.reply().ok()).map(|r| r.atom).unwrap_or(0);
    let (list, pid_a) = (atom("_NET_CLIENT_LIST"), atom("_NET_WM_PID"));
    let wins: Vec<u32> = conn.get_property(false, root, list, AtomEnum::WINDOW, 0, 4096).ok().and_then(|c| c.reply().ok())
        .and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default();
    let me = std::process::id();
    wins.into_iter().any(|w| {
        let name = conn.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 64).ok().and_then(|c| c.reply().ok()).map(|r| r.value).unwrap_or_default();
        let class = conn.get_property(false, w, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64).ok().and_then(|c| c.reply().ok()).map(|r| r.value).unwrap_or_default();
        let pid = conn.get_property(false, w, pid_a, AtomEnum::CARDINAL, 0, 1).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0);
        name == b"Taskbar" && class.starts_with(b"aerode\0") && pid != me && pid != 0
    })
}

fn command(app: &gtk::Application, args: &[String]) -> i32 {
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("shell");
    // we are the primary instance but a shell is already running: D-Bus did not connect us to it
    // (no session bus, or a different one) – never stack a second desktop and taskbar on top
    if cmd != "quit" && SHELL.with(|s| s.borrow().is_none()) && other_shell_running() {
        eprintln!("aerode: a shell is already running on this display but could not be reached over the session bus");
        return 1;
    }
    match cmd {
        "shell" => build(app),
        "start" => { build(app); SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { s.start.toggle() }); }
        "run" => { build(app); dialogs::run_dialog(); }
        "personalize" => { build(app); dialogs::personalize(); }
        "window-color" => { build(app); dialogs::window_color(); }
        "reload" => {
            aerode::config::reload();
            glass::reload();
            glass::publish(Some(&aerode::config::get().colorization.dwm()));
            // settings changed in Control Panel (desktop icons, folder options, taskbar size...)
            SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { s._desktop.rebuild(); s.taskbar.resize(); s.taskbar.refresh() });
            aerode::tray::reload();
        }
        "gadgets" => { build(app); aerode::gadgets::show_gallery(); }
        "quit" => app.quit(),
        "sysmenu" => {
            build(app);
            let n = |i: usize| args.get(i).and_then(|s| s.parse::<i64>().ok());
            if let (Some(w), Some(x), Some(y)) = (n(2), n(3), n(4)) {
                SHELL.with(|s| if let Some(s) = s.borrow().as_ref() { aerode::sysmenu::show(s.taskbar.wm.clone(), w as u32, x as i32, y as i32) });
            }
        }
        "autoplay" => {
            build(app);
            match args.get(2) {
                Some(p) if std::path::Path::new(p).is_dir() => aerode::autoplay::for_path(std::path::Path::new(p)),
                _ => { eprintln!("aerode autoplay: DIR must be a folder"); return 2; }
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
    aerode::util::log_panics("aerode");
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
        Some("screensaver") => {
            gtk::init().expect("gtk");
            aerode::screensaver::command(&args);
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

/// Is this display's aerode-wm running from a binary that has since been replaced on disk
/// (/proc/<pid>/exe reads "… (deleted)" after ./install.sh)?
fn wm_binary_replaced() -> bool {
    // ":0.0" and ":0" are the same display
    let norm = |d: &str| match d.rsplit_once(':') { Some((h, n)) => format!("{h}:{}", n.split('.').next().unwrap_or(n)), None => d.to_string() };
    let ours = norm(std::env::var("DISPLAY").unwrap_or_default().as_str());
    util::output(&["pgrep", "-x", "aerode-wm"]).unwrap_or_default().lines().any(|pid| {
        let pid = pid.trim();
        let replaced = std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().ends_with(" (deleted)")).unwrap_or(false);
        let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        replaced && env.split(|b| *b == 0).any(|kv| kv.strip_prefix(b"DISPLAY=").map(|d| norm(&*String::from_utf8_lossy(d)) == ours).unwrap_or(false))
    })
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
    let (mut wm_ok, mut wm_stale) = (false, false);
    if let Ok((conn, screen)) = x11rb::connect(None) {
        let root = conn.setup().roots[screen].root;
        let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).ok().and_then(|c| c.reply().ok()).map(|r| r.atom).unwrap_or(0);
        let check = atom("_NET_SUPPORTING_WM_CHECK");
        let restart = atom("_AERODE_RESTART");
        let win = conn.get_property(false, root, check, AtomEnum::WINDOW, 0, 1).ok().and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0);
        let name = conn.get_property(false, win, atom("_NET_WM_NAME"), atom("UTF8_STRING"), 0, 64).ok().and_then(|c| c.reply().ok()).map(|r| r.value).unwrap_or_default();
        // a WM built before it published its pid here cannot re-exec a binary that ./install.sh replaced: it
        // would tear down every frame, fail to exec "aerode-wm (deleted)" and keep running the OLD code
        let reexec_ok = conn.get_property(false, win, atom("_NET_WM_PID"), AtomEnum::CARDINAL, 0, 1).ok()
            .and_then(|c| c.reply().ok()).map(|r| r.value_len > 0).unwrap_or(false);
        if win != 0 && name == b"Aerode" && !reexec_ok && wm_binary_replaced() {
            wm_stale = true;
        } else if win != 0 && name == b"Aerode" {
            let ev = ClientMessageEvent::new(32, win, restart, [0u32; 5]);
            // round trip before the connection is dropped, or the server may never deliver it
            wm_ok = conn.send_event(false, win, EventMask::NO_EVENT, ev).is_ok() && conn.get_input_focus().map(|c| c.reply().is_ok()).unwrap_or(false);
        }
    }
    if wm_stale {
        println!("aerode: the running window manager is an older build that cannot load the new one in place;");
        println!("        log out and back in once (the window manager was left alone)");
    } else if wm_ok { println!("aerode: window manager restarting"); } else { println!("aerode: window manager not running, starting it"); detached(&["aerode-wm"]); }
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
        // a copy saved by an older Control Panel lacks newer rules: rebuild it (keeping VSync off)
        if let Ok(old) = std::fs::read_to_string(&user) {
            if !old.contains("# aerode-picom 2") {
                match std::fs::read_to_string("/usr/share/aerode/picom.conf") {
                    Ok(base) if old.contains("vsync = false") => { let _ = std::fs::write(&user, base.replace("vsync = true;", "vsync = false;")); }
                    Ok(_) => { let _ = std::fs::remove_file(&user); }
                    Err(_) => {}
                }
            }
        }
        let user = user.to_string_lossy().into_owned();
        let conf = [user.as_str(), "/usr/share/aerode/picom.conf"].into_iter().find(|p| std::path::Path::new(p).exists());
        if !aerode::config::conf_dir().join("no-compositor").exists() {
            match conf { Some(c) => detached(&["picom", "--config", c]), None => detached(&["picom"]) }
            println!("aerode: compositor restarted");
        }
    }
}
