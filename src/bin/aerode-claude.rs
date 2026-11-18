//! Claude - a Win7-style chat window over Claude Code. Each message runs
//! `claude -p MSG --output-format stream-json --include-partial-messages [--resume SESSION]`,
//! so it uses the Claude Code login (`claude auth login`, opened in aerode-cmd from File > Sign in).
//! Claude works in the folder chosen with File > Open folder (remembered; `aerode-claude DIR` opens one),
//! and does everything there without asking (`--dangerously-skip-permissions`): edits files, runs commands.
//! Should a tool still be refused, the reply says so.
//! The message goes in on stdin: as an argument, one starting with `-` would be read as an option.
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::io::BufRead;
use std::path::PathBuf;
use std::rc::Rc;

use aerode::{config, theme, util};

/// What the reader thread hands to the window.
enum Msg { Session(String), Delta(String), Text(String), Tool(String), Refused(String), Done(Option<String>) }

/// A tool call as one line: its name and what it works on (`Read: src/main.rs`, `Bash: cargo build`).
fn tool_label(name: Option<&str>, input: &serde_json::Value) -> String {
    let name = name.unwrap_or("tool");
    let what = ["file_path", "command", "pattern", "path", "url", "query", "description"].iter().find_map(|k| input[*k].as_str());
    match what.and_then(|w| w.lines().next()) {
        Some(w) if w.chars().count() > 80 => format!("{name}: {}...", w.chars().take(80).collect::<String>()),
        Some(w) => format!("{name}: {w}"),
        None => name.into(),
    }
}

/// Markdown in a reply as runs of text with their tag: ``` blocks, `code`, **bold** and # headings
/// lose their marks; everything else stays as written.
fn markdown(src: &str) -> Vec<(String, Option<&'static str>)> {
    let mut out = vec![];
    let mut fence = false;
    for line in src.split_inclusive('\n') {
        if line.trim_start().starts_with("```") { fence = !fence; continue; }
        if fence { out.push((line.into(), Some("codeblock"))); continue; }
        let h = line.trim_start_matches('#');
        if h.len() < line.len() && h.starts_with(' ') { out.push((h[1..].into(), Some("heading"))); continue; }
        let mut rest = line;
        loop {
            let next = [("`", "code"), ("**", "bold")].into_iter()
                .filter_map(|(m, tag)| { let a = rest.find(m)?; let n = rest[a + m.len()..].find(m)?; Some((a, n, m, tag)) })
                .min_by_key(|x| x.0);
            let Some((a, n, m, tag)) = next else {
                if !rest.is_empty() { out.push((rest.into(), None)); }
                break;
            };
            if a > 0 { out.push((rest[..a].into(), None)); }
            out.push((rest[a + m.len()..a + m.len() + n].into(), Some(tag)));
            rest = &rest[a + 2 * m.len() + n..];
        }
    }
    out
}

/// One stream-json line from `claude -p`, as the window's messages.
fn parse_line(line: &str, streamed: &mut bool) -> Vec<Msg> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return vec![] };
    let mut out = vec![];
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => {
            if let Some(s) = v["session_id"].as_str() { out.push(Msg::Session(s.into())); }
        }
        Some("stream_event") => {
            let e = &v["event"];
            if e["type"] == "content_block_delta" && e["delta"]["type"] == "text_delta" {
                if let Some(t) = e["delta"]["text"].as_str() { *streamed = true; out.push(Msg::Delta(t.into())); }
            }
            // a new message: its text streams again
            if e["type"] == "message_start" { *streamed = false; }
        }
        Some("assistant") => {
            for c in v["message"]["content"].as_array().into_iter().flatten() {
                match c["type"].as_str() {
                    Some("text") if !*streamed => out.push(Msg::Text(c["text"].as_str().unwrap_or("").into())),
                    Some("tool_use") => out.push(Msg::Tool(tool_label(c["name"].as_str(), &c["input"]))),
                    _ => {}
                }
            }
        }
        Some("result") => {
            for d in v["permission_denials"].as_array().into_iter().flatten() {
                out.push(Msg::Refused(tool_label(d["tool_name"].as_str(), &d["tool_input"])));
            }
            let err = v["is_error"].as_bool().unwrap_or(false);
            out.push(Msg::Done(err.then(|| v["result"].as_str().unwrap_or("Claude Code reported an error.").to_string())));
        }
        _ => {}
    }
    out
}

struct App {
    log: gtk::TextBuffer,
    view: gtk::TextView,
    input: gtk::TextView,
    send: gtk::Button,
    stop: gtk::Button,
    status: gtk::Label,
    signin: gtk::InfoBar,
    win: gtk::ApplicationWindow,
    /// the assistant: its idle and work loops; `anim` is the timer playing one
    pet: gtk::Image,
    idle: Vec<gtk::gdk_pixbuf::Pixbuf>,
    work: Vec<gtk::gdk_pixbuf::Pixbuf>,
    anim: RefCell<Option<glib::SourceId>>,
    folder: RefCell<PathBuf>,
    session: RefCell<Option<String>>,
    child: Cell<Option<u32>>,
    /// where the reply's text since the last tool line starts (char offset); formatted when it ends
    seg: Cell<i32>,
    /// counts conversations: a reply still arriving from the last one is dropped
    turn: Cell<u32>,
}

impl App {
    fn append(&self, text: &str, tag: Option<&str>) {
        let start = self.log.char_count();
        self.log.insert(&mut self.log.end_iter(), text);
        if let Some(t) = tag { self.log.apply_tag_by_name(t, &self.log.iter_at_offset(start), &self.log.end_iter()); }
        if let Some(mark) = self.log.create_mark(None, &self.log.end_iter(), false) {
            self.view.scroll_mark_onscreen(&mark);
            self.log.delete_mark(&mark);
        }
    }

    /// The reply's text since `seg` again, with its markdown applied.
    fn format_segment(&self) {
        let (mut a, mut b) = (self.log.iter_at_offset(self.seg.get()), self.log.end_iter());
        let text = self.log.text(&a, &b, false).map(|t| t.to_string()).unwrap_or_default();
        self.log.delete(&mut a, &mut b);
        for (t, tag) in markdown(&text) { self.append(&t, tag); }
        self.seg.set(self.log.char_count());
    }

    fn busy(self: &Rc<Self>, on: bool) {
        self.animate(on);
        self.send.set_sensitive(!on);
        self.stop.set_sensitive(on);
        self.status.set_text(&if on { "Claude is writing...".to_string() } else { format!("Ready - {}", self.folder.borrow().display()) });
    }

    /// Loops the work frames while Claude writes, the idle frames otherwise.
    fn animate(self: &Rc<Self>, working: bool) {
        if let Some(t) = self.anim.borrow_mut().take() { t.remove(); }
        let (n, ms) = if working { (self.work.len(), 110) } else { (self.idle.len(), 180) };
        if n == 0 { return; }
        let me = self.clone();
        let i = Cell::new(0usize);
        let step = move || {
            let frames = if working { &me.work } else { &me.idle };
            let mut f = i.get() % frames.len();
            // idle3 blinks: only every fourth breath, else the open-eyed pose before it
            if !working && f == 3 && (i.get() / frames.len()) % 4 != 3 { f = 2; }
            me.pet.set_from_pixbuf(Some(&frames[f]));
            i.set(i.get() + 1);
            glib::ControlFlow::Continue
        };
        step();
        *self.anim.borrow_mut() = Some(glib::timeout_add_local(std::time::Duration::from_millis(ms), step));
    }

    fn set_folder(&self, dir: PathBuf) {
        self.new_conversation();
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string());
        self.win.set_title(&format!("{name} - Claude"));
        self.status.set_text(&format!("Ready - {}", dir.display()));
        let _ = std::fs::write(config::conf_dir().join("claude-folder"), dir.to_string_lossy().as_bytes());
        *self.folder.borrow_mut() = dir;
    }

    fn choose_folder(&self) {
        let fc = gtk::FileChooserDialog::with_buttons(Some("Open Folder"), Some(&self.win), gtk::FileChooserAction::SelectFolder,
            &[("_Select Folder", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
        fc.set_default_size(780, 520);
        let _ = fc.set_current_folder(&*self.folder.borrow());
        if fc.run() == gtk::ResponseType::Accept {
            if let Some(dir) = fc.filename() { self.set_folder(dir); }
        }
        fc.close();
    }

    fn check_login(self: &Rc<Self>) {
        let me = self.clone();
        util::output_async(vec!["claude".into(), "auth".into(), "status".into()], move |out| {
            let logged = out.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()).and_then(|v| v["loggedIn"].as_bool());
            if logged == Some(true) { me.signin.hide(); } else { me.signin.show_all(); }
            if logged.is_none() { me.status.set_text("Claude Code (claude) is not installed."); }
        });
    }

    /// `claude auth login` wants a terminal (it may ask for a code), so it runs in a console window.
    fn sign_in(self: &Rc<Self>) {
        let (tx, rx) = async_channel::bounded::<()>(1);
        std::thread::spawn(move || {
            let _ = std::process::Command::new("aerode-cmd").args(["-e", "claude", "auth", "login", "--claudeai"]).status();
            let _ = tx.send_blocking(());
        });
        let me = self.clone();
        glib::MainContext::default().spawn_local(async move {
            if rx.recv().await.is_ok() { me.check_login(); }
        });
    }

    fn send_message(self: &Rc<Self>) {
        let buf = self.input.buffer().unwrap();
        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).map(|t| t.trim().to_string()).unwrap_or_default();
        if text.is_empty() || self.child.get().is_some() { return; }
        buf.set_text("");
        if self.log.char_count() > 0 { self.append("\n", None); }
        self.append("You:\n", Some("you"));
        self.append(&format!("{text}\n\n"), None);
        self.append("Claude:\n", Some("claude"));
        self.seg.set(self.log.char_count());

        let mut cmd = std::process::Command::new("claude");
        cmd.args(["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--dangerously-skip-permissions"]);
        if let Some(s) = self.session.borrow().as_ref() { cmd.args(["--resume", s]); }
        cmd.current_dir(&*self.folder.borrow()).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => { self.append(&format!("Could not start claude: {e}\n"), Some("error")); return; }
        };
        self.child.set(Some(child.id()));
        self.busy(true);
        let out = child.stdout.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, rx) = async_channel::unbounded::<Msg>();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            // claude reads all of stdin before it answers, so this cannot block on its output
            let _ = stdin.write_all(text.as_bytes());
            drop(stdin);
            let errs = std::thread::spawn(move || { let mut s = String::new(); let _ = stderr.read_to_string(&mut s); s });
            let mut streamed = false;
            let mut done = false;
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                for m in parse_line(&line, &mut streamed) {
                    done |= matches!(m, Msg::Done(_));
                    let _ = tx.send_blocking(m);
                }
            }
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            let errs = errs.join().unwrap_or_default();
            // killed by Stop it says nothing; a failed start says why on stderr
            let why = errs.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("Stopped.").to_string();
            if !done { let _ = tx.send_blocking(Msg::Done((!ok).then_some(why))); }
        });
        let me = self.clone();
        let turn = self.turn.get();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(m) = rx.recv().await {
                let stale = me.turn.get() != turn;
                match m {
                    Msg::Done(_) if stale => { me.child.set(None); me.busy(false); }
                    _ if stale => {}
                    Msg::Session(s) => *me.session.borrow_mut() = Some(s),
                    Msg::Delta(t) | Msg::Text(t) => me.append(&t, None),
                    Msg::Tool(what) => {
                        me.format_segment();
                        me.append(&format!("\n[{what}]\n"), Some("tool"));
                        me.seg.set(me.log.char_count());
                        me.status.set_text(&format!("Claude is working - {what}"));
                    }
                    Msg::Refused(what) => {
                        me.format_segment();
                        me.append(&format!("\n[Refused: {what}]\n"), Some("error"));
                        me.seg.set(me.log.char_count());
                    }
                    Msg::Done(err) => {
                        me.format_segment();
                        if let Some(e) = err {
                            me.append(&format!("{e}\n"), Some("error"));
                            if e.contains("Not logged in") { me.signin.show_all(); }
                        }
                        me.append("\n", None);
                        me.child.set(None);
                        me.busy(false);
                        me.input.grab_focus();
                    }
                }
            }
        });
    }

    fn stop(&self) {
        if let Some(pid) = self.child.get() { unsafe { libc::kill(pid as i32, libc::SIGTERM); } }
    }

    fn new_conversation(&self) {
        self.stop();
        self.turn.set(self.turn.get() + 1);
        *self.session.borrow_mut() = None;
        self.log.set_text("");
    }
}

/// One of the assistant's loops (`idle` or `work`, frames 0..5), rendered by data/assistant/crab.py; empty when missing.
fn assistant_frames(set: &str, size: i32) -> Vec<gtk::gdk_pixbuf::Pixbuf> {
    let installed = PathBuf::from("/usr/share/aerode/assistant/crab");
    let dir = if installed.is_dir() { installed } else { PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/data/assistant/crab")) };
    (0..6).filter_map(|i| gtk::gdk_pixbuf::Pixbuf::from_file_at_size(dir.join(format!("{set}{i}.png")), size, size).ok()).collect()
}

fn menu_item(menu: &gtk::Menu, label: &str, f: impl Fn() + 'static) {
    let it = gtk::MenuItem::with_mnemonic(label);
    it.connect_activate(move |_| f());
    menu.append(&it);
}

fn build(app: &gtk::Application, dir: Option<PathBuf>) {
    theme::init_app();
    let win = gtk::ApplicationWindow::new(app);
    win.set_title("Claude");
    win.set_default_size(640, 720);
    win.set_icon_name(Some(util::first_icon(&["internet-chat", "im-message-new", "user-available"])));

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menubar = gtk::MenuBar::new();
    vbox.pack_start(&menubar, false, false, 0);

    let signin = gtk::InfoBar::new();
    signin.set_message_type(gtk::MessageType::Info);
    signin.content_area().add(&gtk::Label::new(Some("Sign in to Claude to start a conversation.")));
    signin.add_button("_Sign in...", gtk::ResponseType::Accept);
    signin.set_no_show_all(true);
    vbox.pack_start(&signin, false, false, 0);

    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_left_margin(10);
    view.set_right_margin(10);
    view.set_top_margin(8);
    let log = view.buffer().unwrap();
    let tags = log.tag_table().unwrap();
    tags.add(&gtk::TextTag::builder().name("you").weight(700).foreground("#1e395b").build());
    tags.add(&gtk::TextTag::builder().name("claude").weight(700).foreground("#3399ff").build());
    tags.add(&gtk::TextTag::builder().name("tool").foreground("#6d6d6d").style(gtk::pango::Style::Italic).build());
    tags.add(&gtk::TextTag::builder().name("error").foreground("#c42b1c").build());
    tags.add(&gtk::TextTag::builder().name("bold").weight(700).build());
    tags.add(&gtk::TextTag::builder().name("heading").weight(700).scale(1.15).foreground("#1e395b").build());
    tags.add(&gtk::TextTag::builder().name("code").family("Consolas, monospace").background("#eef0f3").build());
    tags.add(&gtk::TextTag::builder().name("codeblock").family("Consolas, monospace").paragraph_background("#f4f5f7").left_margin(22).build());
    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_shadow_type(gtk::ShadowType::In);
    scroll.add(&view);

    // the conversation over the compose box, as a Win7 IM window: a splitter between them
    let paned = gtk::Paned::new(gtk::Orientation::Vertical);
    paned.pack1(&scroll, true, false);
    let compose = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    compose.set_border_width(6);
    let (idle, work) = (assistant_frames("idle", 88), assistant_frames("work", 88));
    let pet = gtk::Image::new();
    compose.pack_start(&pet, false, false, 0);
    let input = gtk::TextView::new();
    input.set_wrap_mode(gtk::WrapMode::WordChar);
    input.set_left_margin(4);
    input.set_top_margin(3);
    let iscroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    iscroll.set_shadow_type(gtk::ShadowType::In);
    iscroll.set_size_request(-1, 70);
    iscroll.add(&input);
    compose.pack_start(&iscroll, true, true, 0);
    let buttons = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let send = gtk::Button::with_mnemonic("_Send");
    let stop = gtk::Button::with_mnemonic("S_top");
    send.set_size_request(75, 23);
    stop.set_size_request(75, 23);
    stop.set_sensitive(false);
    buttons.pack_start(&send, false, false, 0);
    buttons.pack_start(&stop, false, false, 0);
    compose.pack_start(&buttons, false, false, 0);
    paned.pack2(&compose, false, false);
    paned.set_position(560);
    vbox.pack_start(&paned, true, true, 0);

    let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    statusbar.style_context().add_class("statusbar");
    let status = gtk::Label::new(Some("Ready"));
    status.set_xalign(0.0);
    status.set_margin_start(4);
    statusbar.pack_start(&status, true, true, 0);
    vbox.pack_start(&statusbar, false, false, 2);
    win.add(&vbox);

    let saved = std::fs::read_to_string(config::conf_dir().join("claude-folder")).ok().map(PathBuf::from).filter(|p| p.is_dir());
    let folder = dir.or(saved).unwrap_or_else(glib::home_dir);
    let me = Rc::new(App { win: win.clone(), pet, idle, work, anim: RefCell::new(None), folder: RefCell::new(folder.clone()), log, view, input: input.clone(), send: send.clone(), stop: stop.clone(), status, signin: signin.clone(),
        session: RefCell::new(None), child: Cell::new(None), seg: Cell::new(0), turn: Cell::new(0) });

    let file = gtk::Menu::new();
    { let a = me.clone(); menu_item(&file, "_New conversation", move || a.new_conversation()); }
    { let a = me.clone(); menu_item(&file, "_Open folder...", move || a.choose_folder()); }
    file.append(&gtk::SeparatorMenuItem::new());
    { let a = me.clone(); menu_item(&file, "Sign _in...", move || a.sign_in()); }
    { let a = me.clone(); menu_item(&file, "Sign _out", move || {
        let _ = std::process::Command::new("claude").args(["auth", "logout"]).status();
        a.check_login();
    }); }
    file.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); menu_item(&file, "E_xit", move || w.close()); }
    let top = gtk::MenuItem::with_mnemonic("_File");
    top.set_submenu(Some(&file));
    menubar.append(&top);
    let help = gtk::Menu::new();
    { let w = win.clone(); menu_item(&help, "_About Claude", move || {
        let d = gtk::AboutDialog::new();
        d.set_transient_for(Some(&w));
        d.set_program_name("Claude");
        d.set_comments(Some("A chat window over Claude Code, in the Windows 7 style of aerode."));
        d.set_logo_icon_name(Some(util::first_icon(&["internet-chat", "im-message-new"])));
        d.run();
        d.close();
    }); }
    let top = gtk::MenuItem::with_mnemonic("_Help");
    top.set_submenu(Some(&help));
    menubar.append(&top);

    { let a = me.clone(); signin.connect_response(move |_, _| a.sign_in()); }
    { let a = me.clone(); send.connect_clicked(move |_| a.send_message()); }
    { let a = me.clone(); stop.connect_clicked(move |_| a.stop()); }
    // Enter sends, Shift+Enter is a new line
    { let a = me.clone(); input.connect_key_press_event(move |_, ev| {
        let enter = matches!(ev.keyval(), gtk::gdk::keys::constants::Return | gtk::gdk::keys::constants::KP_Enter);
        if enter && !ev.state().contains(gtk::gdk::ModifierType::SHIFT_MASK) { a.send_message(); return glib::Propagation::Stop; }
        glib::Propagation::Proceed
    }); }
    { let a = me.clone(); win.connect_destroy(move |_| a.stop()); }

    win.show_all();
    me.set_folder(folder);
    input.grab_focus();
    me.check_login();
    me.animate(false);
}

fn main() -> glib::ExitCode {
    util::log_panics("claude");
    let app = gtk::Application::new(Some("org.aerode.Claude"), gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(|app| { util::message_dialog_sounds(); build(app, None) });
    app.connect_open(|app, files, _| { util::message_dialog_sounds(); for f in files { build(app, f.path().filter(|p| p.is_dir())); } });
    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_json() {
        let mut s = false;
        assert!(matches!(&parse_line(r#"{"type":"system","subtype":"init","session_id":"abc"}"#, &mut s)[..], [Msg::Session(id)] if id == "abc"));
        assert!(matches!(&parse_line(r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}}"#, &mut s)[..], [Msg::Delta(t)] if t == "Hi"));
        // already streamed: the full message is not shown twice
        assert!(parse_line(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}"#, &mut s).is_empty());
        assert!(matches!(&parse_line(r#"{"type":"result","is_error":true,"result":"Not logged in"}"#, &mut s)[..], [Msg::Done(Some(e))] if e == "Not logged in"));
        assert!(matches!(&parse_line(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo build\nls"}}]}}"#, &mut s)[..], [Msg::Tool(t)] if t == "Bash: cargo build"));
        assert!(matches!(&parse_line(r#"{"type":"result","is_error":false,"permission_denials":[{"tool_name":"Bash","tool_input":{"command":"rm x"}}]}"#, &mut s)[..], [Msg::Refused(t), Msg::Done(None)] if t == "Bash: rm x"));
    }

    #[test]
    fn markdown_runs() {
        let r = markdown("# Title\nUse `cargo` and **care**.\n```rust\nfn x() {}\n```\na ` lone tick\n");
        let want: Vec<(String, Option<&str>)> = vec![("Title\n".into(), Some("heading")), ("Use ".into(), None), ("cargo".into(), Some("code")),
            (" and ".into(), None), ("care".into(), Some("bold")), (".\n".into(), None), ("fn x() {}\n".into(), Some("codeblock")),
            ("a ` lone tick\n".into(), None)];
        assert_eq!(r, want);
    }
}
