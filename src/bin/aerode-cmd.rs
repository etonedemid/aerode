//! aerode-cmd – Command Prompt.
//!   aerode-cmd                    console window running the configured shell (cmd by default)
//!   aerode-cmd -e PROGRAM [ARGS]  console window running PROGRAM (e.g. fish)
//!   aerode-cmd --shell [/c|/k CMD] the cmd.exe interpreter itself (runs inside the console)
use gtk::glib;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aerode-cmd".into());
    if args.get(1).map(|a| a == "--shell").unwrap_or(false) {
        // the console window's settings (history size, insert mode, popup colours) by its title
        let cfg = aerode::console::host::load_for(&std::env::var("AERODE_CONSOLE_KEY").unwrap_or_default());
        let mut sh = aerode::cmdshell::Shell::new(&cfg);
        let rest = &args[2..];
        let flag = rest.first().map(|a| a.to_lowercase());
        let cmdline = rest.get(1..).map(|r| r.join(" ")).unwrap_or_default();
        match flag.as_deref() {
            Some("/c") => { sh.run_line(&cmdline); std::process::exit(sh.exit.unwrap_or(sh.errorlevel)); }
            Some("/k") => { sh.run_line(&cmdline); }
            _ => {
                // the start-up banner, then one empty line before the prompt, like cmd.exe
                print!("{}\r\n\r\n", aerode::cmdshell::banner().trim_end().replace('\n', "\r\n"));
            }
        }
        std::process::exit(sh.repl());
    }
    gtk::init().expect("gtk");
    aerode::theme::init_app();
    let cfg = aerode::console::host::load_config();
    let (argv, title) = if args.get(1).map(|a| a == "-e").unwrap_or(false) && args.len() > 2 {
        (args[2..].to_vec(), args[2].clone())
    } else if args.get(1).map(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k")).unwrap_or(false) {
        let mut v = vec![exe.clone(), "--shell".into()];
        v.extend(args[1..].iter().cloned());
        (v, "C:\\Windows\\system32\\cmd.exe".into())
    } else if cfg.shell == "cmd" || cfg.shell.is_empty() {
        (vec![exe.clone(), "--shell".into()], "C:\\Windows\\system32\\cmd.exe".into())
    } else {
        let v = glib::shell_parse_argv(cfg.shell.as_str()).map(|v| v.iter().map(|s| s.to_string_lossy().into_owned()).collect()).unwrap_or_else(|_| vec![cfg.shell.clone()]);
        let t = v[0].clone();
        (v, t)
    };
    let con = aerode::console::host::Console::new(None, argv, &title);
    con.win.connect_destroy(|_| gtk::main_quit());
    gtk::main();
    glib::ExitCode::SUCCESS
}
