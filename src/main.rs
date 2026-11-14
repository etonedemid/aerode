//! `aerode` – the shell process (desktop, taskbar, start menu) and helper commands.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use aerode::{desktop, dialogs, glass, startmenu, sys, taskbar, theme, util, wmclient};

const USAGE: &str = "usage: aerode [shell|start|run|personalize|window-color|gadgets|reload|lock|quit|restart]
       aerode sysmenu WINDOW X Y | taskbar-launch N | screensaver [--preview NAME] | autoplay DIR
       aerode query-volume | query-network
       aerode mixer | volume-step up|down [N] | audio-mute | mic-mute";

struct Shell {
    _desktop: Rc<desktop::Desktop>,
    taskbar: Rc<taskbar::Taskbar>,
    start: Rc<startmenu::StartMenu>,
}

thread_local! { static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) }; }

fn build(app: &gtk::Application) {
    if SHELL.with(|s| s.borrow().is_some()) { return; }
    theme::init_app();
    // the shell speaks Win7's English: without this the date flyout's calendar alone follows LC_TIME
    // ("noviembre", "dom lun mar" under an English "Saturday, November 14")
    unsafe { libc::setlocale(libc::LC_TIME, c"C".as_ptr()); }
    theme::load_css(theme::SHELL_CSS);
    let wm = match wmclient::Wm::connect() {
        Ok(w) => Rc::new(w),
        Err(e) => { eprintln!("aerode: {e}"); std::process::exit(1) }
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
    { let t = tb.clone(); startmenu::start_overrides_menus(sm.clone(), move |x, y| t.on_orb(x, y)); }
    {
        let t = tb.clone();
        *sm.on_hide.borrow_mut() = Some(Box::new(move || { t.set_start_open(false); t.win.queue_draw(); }));
        let t = tb.clone();
        *sm.on_show.borrow_mut() = Some(Box::new(move || t.set_start_open(true)));
    }
    // follow monitor changes
    if let Some(screen) = gtk::gdk::Screen::default() {
        screen.connect_size_changed(|_| glass::reload());
    }
    aerode::gadgets::restore();
    // notification daemon (balloon tips above the notification area); before autostart so
    // that programs started there find it instead of D-Bus-activating another daemon
    aerode::notify::start(&tb.tray, tb.wm.clone());
    // Startup programs and the logon (or, first after boot, startup) sound once per logon session:
    // not again when the shell restarts (`aerode restart`, a crash), like Explorer
    if aerode::sounds::first_shell_of_session() {
        autostart();
        aerode::sounds::logon();
    }
    aerode::sounds::start_watchers();
    // AutoPlay for discs, drives and devices plugged in from now on
    aerode::autoplay::watch();
    // Power Options: display-off and sleep timers of the active plan, on battery / plugged in
    aerode::power::start();
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
            // leftovers from an X11 setup: a second compositor makes no sense inside aerode-wm
            let exe = a.executable().file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            if matches!(exe.as_str(), "picom" | "compton" | "xcompmgr" | "compiz") { continue; }
            if a.is_hidden() || !a.shows_in(Some("Aerode")) { continue; }
            if !a.boolean("X-GNOME-Autostart-enabled") && a.has_key("X-GNOME-Autostart-enabled") { continue; }
            let _ = a.launch(&[], None::<&gio::AppLaunchContext>);
        }
    }
}

/// Is another process's aerode shell in this session? (Then this one was meant to reach it over
/// D-Bus and must not build a second desktop and taskbar.)
fn other_shell_running() -> bool {
    !util::procs_on_display("aerode", Some("shell")).is_empty()
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
            aerode::wmipc::send(aerode::wmipc::Req::ReloadConfig);
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
            // "<level> <muted>\t<tooltip>" (the level is the loudest channel)
            if let Some(i) = sys::audio_info(false) {
                match i.level() { Some((p, m)) => println!("{p} {}\t{}", m as u8, i.tooltip()), None => println!("- -\t{}", i.tooltip()) }
            }
            return glib::ExitCode::SUCCESS;
        }
        // the volume keys (bound by aerode-wm): Win7's 2 % steps, unmuting, keeping the balance
        Some("volume-step") => {
            let n: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
            match args.get(2).map(|s| s.as_str()) {
                Some("up") => sys::apply_step(n),
                Some("down") => sys::apply_step(-n),
                _ => { eprintln!("{USAGE}"); return glib::ExitCode::FAILURE; }
            }
            return glib::ExitCode::SUCCESS;
        }
        Some("audio-mute") => { sys::apply_mute(None, None); return glib::ExitCode::SUCCESS; }
        Some("mic-mute") => { sys::apply_mic_mute_toggle(); return glib::ExitCode::SUCCESS; }
        // Win7's Volume Mixer (sndvol.exe): its own process
        Some("mixer") => { aerode::mixer::run(); return glib::ExitCode::SUCCESS; }
        Some("query-network") => {
            let n = sys::net_status();
            println!("{}\t{}\t{}", n.kind, n.signal, n.name);
            return glib::ExitCode::SUCCESS;
        }
        Some("lock") => {
            gtk::init().expect("gtk");
            aerode::lock::lock(!args.iter().any(|a| a == "--sleep"));
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
        r
    });
    app.connect_activate(build);
    app.run_with_args(&args)
}

/// Is this display's aerode-wm running from a binary that has since been replaced on disk
/// (/proc/<pid>/exe reads "… (deleted)" after ./install.sh)?
fn wm_binary_replaced() -> bool {
    util::procs_on_display("aerode-wm", None).into_iter().any(|pid| {
        std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().ends_with(" (deleted)")).unwrap_or(false)
    })
}

/// `aerode restart`: restart the shell (taskbar, desktop, Start) without logging out. The compositor
/// can't be replaced in place on Wayland: its clients would lose their connection, so a new
/// aerode-wm takes a log off.
fn restart_session() {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aerode".into());
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new(&exe).arg("quit").status();
    std::thread::sleep(std::time::Duration::from_millis(600));
    // a shell that did not quit: only this session's `aerode shell`
    util::signal_on_display("aerode", Some("shell"), libc::SIGTERM);
    let _ = std::process::Command::new(&exe).arg("shell").stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0).spawn();
    println!("aerode: shell restarted");
    if wm_binary_replaced() { println!("aerode: aerode-wm was updated: log off and back on to run the new one"); }
}
