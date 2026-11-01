//! The Windows Command Processor (cmd.exe) for Aerode.
//! Paths are shown Windows-style: C:\ = /, C:\Users\<name> = /home/<name>.
pub mod line;

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ------------------------------------------------------------------ paths
pub fn to_win(p: &Path) -> String {
    let s = p.to_string_lossy();
    let s = if let Some(r) = s.strip_prefix("/home") { format!("\\Users{r}") } else { s.to_string() };
    let s = s.replace('/', "\\");
    let s = if s.is_empty() { "\\".to_string() } else { s };
    format!("C:{s}")
}

/// Resolve a Windows-ish path (absolute, relative, C:\..., forward slashes) to a Linux path,
/// matching existing components case-insensitively like Windows does.
pub fn to_linux(cwd: &Path, w: &str) -> PathBuf {
    let w = w.trim().trim_matches('"');
    if w.is_empty() { return cwd.to_path_buf(); }
    let (abs, rest) = if w.len() >= 2 && w.as_bytes()[1] == b':' && w.as_bytes()[0].is_ascii_alphabetic() {
        let r = &w[2..];
        if r.starts_with('\\') || r.starts_with('/') { (Some(PathBuf::from("/")), r.to_string()) } else { (Some(cwd.to_path_buf()), r.to_string()) }
    } else if w.starts_with('\\') || w.starts_with('/') { (Some(PathBuf::from("/")), w.to_string()) }
    else if let Some(r) = w.strip_prefix('~') { (Some(dirs_home()), r.to_string()) }
    else { (None, w.to_string()) };
    let mut p = abs.unwrap_or_else(|| cwd.to_path_buf());
    let mut first = true;
    for comp in rest.split(['\\', '/']) {
        match comp {
            "" | "." => continue,
            ".." => { p.pop(); }
            c => {
                // C:\Users ↔ /home
                if first && p == Path::new("/") && c.eq_ignore_ascii_case("users") { p = PathBuf::from("/home"); first = false; continue; }
                let exact = p.join(c);
                if exact.exists() { p = exact; } else {
                    let hit = std::fs::read_dir(&p).ok().and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(c)));
                    p = hit.map(|e| e.path()).unwrap_or(exact);
                }
            }
        }
        first = false;
    }
    p
}

fn dirs_home() -> PathBuf { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/")) }

fn wildcard_match(pat: &str, name: &str) -> bool {
    fn m(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => m(&p[1..], n) || (!n.is_empty() && m(p, &n[1..])),
            (Some('?'), Some(_)) => m(&p[1..], &n[1..]),
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b) && m(&p[1..], &n[1..]),
            // "*.*" also matches names without a dot, like Windows
            (Some('.'), None) => p[1..].iter().all(|c| *c == '*'),
            _ => false,
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    m(&p, &n)
}

/// Expand a path that may contain wildcards in its last component.
fn glob(cwd: &Path, w: &str) -> Vec<PathBuf> {
    let w = w.trim_matches('"');
    if !w.contains('*') && !w.contains('?') { return vec![to_linux(cwd, w)]; }
    let (dir, pat) = match w.rfind(['\\', '/']) { Some(i) => (to_linux(cwd, &w[..i.max(1)]), &w[i + 1..]), None => (cwd.to_path_buf(), w) };
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.flatten().filter(|e| wildcard_match(pat, &e.file_name().to_string_lossy())).map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); }
    o
}

fn win_date(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t).format("%m/%d/%Y  %I:%M %p").to_string()
}

// ------------------------------------------------------------------ parsing
#[derive(Debug, Clone, PartialEq)]
enum Join { Seq, And, Or }

#[derive(Debug, Clone, Default)]
struct Simple { words: Vec<String>, raw: String, out: Option<(String, bool)>, err: Option<(String, bool)>, err_to_out: bool, input: Option<String> }

/// Split a line into pipelines joined by & && ||, honouring quotes and ^.
fn split_ops(line: &str) -> Vec<(Join, Vec<String>)> {
    let mut res: Vec<(Join, Vec<String>)> = vec![];
    let mut cur_pipe: Vec<String> = vec![];
    let mut cur = String::new();
    let mut join = Join::Seq;
    let mut q = false;
    let mut depth = 0i32;
    let cs: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '^' && !q && i + 1 < cs.len() { cur.push(c); cur.push(cs[i + 1]); i += 2; continue; }
        if c == '"' { q = !q; }
        if !q && c == '(' { depth += 1; }
        if !q && c == ')' { depth -= 1; }
        if !q && depth <= 0 {
            if c == '|' && cs.get(i + 1) == Some(&'|') { cur_pipe.push(std::mem::take(&mut cur)); res.push((join, std::mem::take(&mut cur_pipe))); join = Join::Or; i += 2; continue; }
            if c == '&' && cs.get(i + 1) == Some(&'&') { cur_pipe.push(std::mem::take(&mut cur)); res.push((join, std::mem::take(&mut cur_pipe))); join = Join::And; i += 2; continue; }
            if c == '&' && !(i > 0 && (cs[i - 1] == '>' || cs[i - 1] == '<')) { cur_pipe.push(std::mem::take(&mut cur)); res.push((join, std::mem::take(&mut cur_pipe))); join = Join::Seq; i += 1; continue; }
            if c == '|' { cur_pipe.push(std::mem::take(&mut cur)); i += 1; continue; }
        }
        cur.push(c);
        i += 1;
    }
    cur_pipe.push(cur);
    res.push((join, cur_pipe));
    res.into_iter().filter(|(_, p)| p.iter().any(|s| !s.trim().is_empty())).collect()
}

/// Split one command into words + redirections.
fn parse_simple(s: &str) -> Simple {
    let mut r = Simple { raw: s.trim().to_string(), ..Default::default() };
    let cs: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut word = String::new();
    let mut q = false;
    let mut pending: Option<(u8, bool)> = None; // (fd: 1/2/0, append)
    let flush = |word: &mut String, r: &mut Simple, pending: &mut Option<(u8, bool)>| {
        if word.is_empty() { return; }
        let w = std::mem::take(word);
        match pending.take() {
            Some((1, a)) => r.out = Some((w, a)),
            Some((2, a)) => r.err = Some((w, a)),
            Some((0, _)) => r.input = Some(w),
            _ => r.words.push(w),
        }
    };
    while i < cs.len() {
        let c = cs[i];
        if c == '^' && !q && i + 1 < cs.len() { word.push(cs[i + 1]); i += 2; continue; }
        if c == '"' { q = !q; word.push(c); i += 1; continue; }
        if !q && (c == ' ' || c == '\t' || c == ',' || c == ';' || c == '=') && pending.is_none() && !(c == '=' && !word.is_empty()) {
            flush(&mut word, &mut r, &mut pending);
            i += 1;
            continue;
        }
        if !q && c == ' ' { flush(&mut word, &mut r, &mut pending); i += 1; continue; }
        if !q && (c == '>' || c == '<') {
            let fd = if c == '<' { 0 } else if word == "2" { word.clear(); 2 } else if word == "1" { word.clear(); 1 } else { 1 };
            flush(&mut word, &mut r, &mut pending);
            let append = c == '>' && cs.get(i + 1) == Some(&'>');
            i += if append { 2 } else { 1 };
            if fd == 2 && cs.get(i) == Some(&'&') && cs.get(i + 1) == Some(&'1') { r.err_to_out = true; i += 2; continue; }
            pending = Some((fd, append));
            while cs.get(i) == Some(&' ') { i += 1; }
            continue;
        }
        word.push(c);
        i += 1;
    }
    flush(&mut word, &mut r, &mut pending);
    r
}

fn unquote(s: &str) -> String { s.replace('"', "") }

// ------------------------------------------------------------------ shell
pub struct Shell {
    pub cwd: PathBuf,
    pub vars: BTreeMap<String, String>,
    pub errorlevel: i32,
    pub echo: bool,
    pub prompt: String,
    dirstack: Vec<PathBuf>,
    locals: Vec<(BTreeMap<String, String>, PathBuf)>,
    pub editor: line::Editor,
    tty: Option<line::Tty>,
    pub exit: Option<i32>,
    batch: Option<Batch>,
}

struct Batch { lines: Vec<String>, pc: usize, args: Vec<String>, path: PathBuf, exit_b: bool }

type Out<'a> = &'a mut dyn Write;

macro_rules! w { ($o:expr, $($a:tt)*) => { let _ = write!($o, $($a)*); } }
macro_rules! wl { ($o:expr) => { let _ = write!($o, "\r\n"); }; ($o:expr, $($a:tt)*) => { let _ = write!($o, $($a)*); let _ = write!($o, "\r\n"); } }

const BUILTINS: &[(&str, &str)] = &[
    ("ATTRIB", "Shows or changes the read-only and hidden attributes of files."),
    ("CALL", "Runs one batch file from another."),
    ("CD", "Shows or changes the current directory."),
    ("CHDIR", "Same as CD."),
    ("CLS", "Clears the screen."),
    ("COLOR", "Sets the console text and background colours (COLOR bf)."),
    ("COPY", "Copies one or more files."),
    ("DATE", "Shows the date."),
    ("DEL", "Deletes one or more files."),
    ("DIR", "Lists the files and subfolders in a folder."),
    ("ECHO", "Prints a message, or turns command echoing on or off."),
    ("ERASE", "Same as DEL."),
    ("EXIT", "Closes the command processor or ends a batch file."),
    ("FIND", "Searches files or input for a text string."),
    ("FOR", "Runs a command for each item in a set."),
    ("GOTO", "Jumps to a labelled line in a batch file."),
    ("HELP", "Lists the available commands."),
    ("IF", "Runs a command only when a condition holds."),
    ("IPCONFIG", "Shows the network configuration."),
    ("MD", "Creates a folder."),
    ("MKDIR", "Same as MD."),
    ("MORE", "Shows output one screen at a time."),
    ("MOVE", "Moves or renames files and folders."),
    ("PATH", "Shows or sets the search path for programs."),
    ("PAUSE", "Waits for a key press."),
    ("PING", "Tests whether another computer can be reached."),
    ("POPD", "Returns to the folder saved by PUSHD."),
    ("PROMPT", "Changes the command prompt."),
    ("PUSHD", "Saves the current folder, then changes to another."),
    ("RD", "Removes a folder."),
    ("REM", "Marks a comment in a batch file."),
    ("REN", "Renames files."),
    ("RENAME", "Same as REN."),
    ("RMDIR", "Same as RD."),
    ("SET", "Shows, sets or removes environment variables."),
    ("SETLOCAL", "Starts keeping environment changes local to a batch file."),
    ("ENDLOCAL", "Ends local environment changes."),
    ("SHIFT", "Shifts batch file parameters."),
    ("SORT", "Sorts input lines."),
    ("START", "Opens a program, document or folder in a new window."),
    ("TASKKILL", "Ends processes by PID or image name."),
    ("TASKLIST", "Lists running processes."),
    ("TIME", "Shows the time."),
    ("TIMEOUT", "Waits for a number of seconds."),
    ("TITLE", "Sets the window title."),
    ("TREE", "Draws the folder structure of a drive or path."),
    ("TYPE", "Shows the contents of text files."),
    ("VER", "Shows the version."),
    ("VOL", "Shows the volume label and serial number."),
    ("WHERE", "Finds where a program is located."),
    ("WHOAMI", "Shows the current user."),
];

impl Shell {
    pub fn new(cfg: &crate::console::host::ConsoleConfig) -> Shell {
        let mut vars = BTreeMap::new();
        for (k, v) in std::env::vars() { vars.insert(k.to_uppercase(), v); }
        let home = dirs_home();
        let user = vars.get("USER").cloned().unwrap_or_else(|| "User".into());
        vars.insert("USERNAME".into(), user.clone());
        vars.insert("USERPROFILE".into(), to_win(&home));
        vars.insert("HOMEDRIVE".into(), "C:".into());
        vars.insert("HOMEPATH".into(), to_win(&home)[2..].to_string());
        vars.insert("SYSTEMROOT".into(), "C:\\Windows".into());
        vars.insert("WINDIR".into(), "C:\\Windows".into());
        vars.insert("COMSPEC".into(), "C:\\Windows\\system32\\cmd.exe".into());
        vars.insert("OS".into(), "Windows_NT".into());
        vars.insert("COMPUTERNAME".into(), gtk::glib::host_name().to_uppercase());
        vars.insert("PATHEXT".into(), ".COM;.EXE;.BAT;.CMD".into());
        vars.insert("PROMPT".into(), "$P$G".into());
        Shell {
            cwd: home, vars, errorlevel: 0, echo: true, prompt: "$P$G".into(), dirstack: vec![], locals: vec![],
            editor: line::Editor { history: vec![], max_history: cfg.history_size as usize, discard_dups: cfg.discard_duplicates, insert: cfg.insert_mode, popup_fg: cfg.popup_fg, popup_bg: cfg.popup_bg },
            tty: None, exit: None, batch: None,
        }
    }

    fn var(&self, k: &str) -> Option<String> {
        let ku = k.to_uppercase();
        match ku.as_str() {
            "CD" => Some(to_win(&self.cwd)),
            "DATE" => Some(chrono::Local::now().format("%a %m/%d/%Y").to_string()),
            "TIME" => Some(chrono::Local::now().format("%k:%M:%S.%2f").to_string()),
            "RANDOM" => Some(((std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0) / 7) % 32768).to_string()),
            "ERRORLEVEL" => Some(self.errorlevel.to_string()),
            "CMDCMDLINE" => Some("C:\\Windows\\system32\\cmd.exe".into()),
            _ => self.vars.get(&ku).cloned(),
        }
    }

    /// %VAR% expansion (+ %0-%9 and %~dp0 in batch files, %VAR:a=b% substitution).
    fn expand(&self, s: &str) -> String {
        let mut out = String::new();
        let cs: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            if cs[i] == '%' {
                if let Some(b) = &self.batch {
                    if cs.get(i + 1) == Some(&'%') { out.push('%'); i += 2; continue; }
                    if let Some(d) = cs.get(i + 1).and_then(|c| c.to_digit(10)) {
                        out.push_str(&if d == 0 { to_win(&b.path) } else { b.args.get(d as usize - 1).cloned().unwrap_or_default() });
                        i += 2;
                        continue;
                    }
                    if cs.get(i + 1) == Some(&'*') { out.push_str(&b.args.join(" ")); i += 2; continue; }
                    if cs.get(i + 1) == Some(&'~') {
                        // %~0..9 with modifiers d p n x f
                        let mut j = i + 2;
                        let mut mods = String::new();
                        while j < cs.len() && "dpnxf".contains(cs[j]) { mods.push(cs[j]); j += 1; }
                        if let Some(d) = cs.get(j).and_then(|c| c.to_digit(10)) {
                            let arg = if d == 0 { b.path.clone() } else { to_linux(&self.cwd, b.args.get(d as usize - 1).map(|s| s.as_str()).unwrap_or("")) };
                            let w = to_win(&arg);
                            let r = if mods.is_empty() { unquote(&if d == 0 { w.clone() } else { b.args.get(d as usize - 1).cloned().unwrap_or_default() }) } else {
                                let mut r = String::new();
                                if mods.contains('f') { r = w.clone(); }
                                if mods.contains('d') { r.push_str("C:"); }
                                if mods.contains('p') { r.push_str(&to_win(arg.parent().unwrap_or(Path::new("/")))[2..]); if !r.ends_with('\\') { r.push('\\'); } }
                                if mods.contains('n') { r.push_str(&arg.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()); }
                                if mods.contains('x') { r.push_str(&arg.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default()); }
                                r
                            };
                            out.push_str(&r);
                            i = j + 1;
                            continue;
                        }
                    }
                }
                if let Some(end) = cs[i + 1..].iter().position(|c| *c == '%') {
                    let name: String = cs[i + 1..i + 1 + end].iter().collect();
                    let (vname, subst) = match name.split_once(':') { Some((a, b)) => (a.to_string(), Some(b.to_string())), None => (name.clone(), None) };
                    if let Some(v) = self.var(&vname) {
                        let v = match subst.as_deref().and_then(|s| s.split_once('=')) {
                            Some((from, to)) => replace_ci(&v, from.trim_start_matches('*'), to),
                            None => match subst.as_deref().and_then(|s| s.strip_prefix('~')) {
                                Some(spec) => {
                                    let mut it = spec.split(',');
                                    let a: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                                    let ch: Vec<char> = v.chars().collect();
                                    let start = if a < 0 { (ch.len() as i64 + a).max(0) as usize } else { (a as usize).min(ch.len()) };
                                    let len = it.next().and_then(|x| x.parse::<i64>().ok());
                                    let end = match len { Some(l) if l < 0 => (ch.len() as i64 + l).max(start as i64) as usize, Some(l) => (start + l as usize).min(ch.len()), None => ch.len() };
                                    ch[start..end].iter().collect()
                                }
                                None => v,
                            },
                        };
                        out.push_str(&v);
                        i += end + 2;
                        continue;
                    } else if self.batch.is_some() {
                        // undefined variables expand to nothing inside batch files
                        i += end + 2;
                        continue;
                    }
                }
            }
            out.push(cs[i]);
            i += 1;
        }
        out
    }

    pub fn prompt_text(&self) -> String {
        let p = self.vars.get("PROMPT").cloned().unwrap_or_else(|| "$P$G".into());
        let mut o = String::new();
        let mut it = p.chars();
        while let Some(c) = it.next() {
            if c != '$' { o.push(c); continue; }
            match it.next().map(|c| c.to_ascii_uppercase()) {
                Some('P') => o.push_str(&to_win(&self.cwd)), Some('G') => o.push('>'), Some('L') => o.push('<'), Some('B') => o.push('|'),
                Some('Q') => o.push('='), Some('S') => o.push(' '), Some('_') => o.push_str("\r\n"), Some('$') => o.push('$'),
                Some('N') => o.push('C'), Some('D') => o.push_str(&chrono::Local::now().format("%a %m/%d/%Y").to_string()),
                Some('T') => o.push_str(&chrono::Local::now().format("%k:%M:%S.%2f").to_string()), Some('V') => o.push_str(&version()),
                Some('A') => o.push('&'), Some('C') => o.push('('), Some('F') => o.push(')'), Some('E') => o.push('\x1b'),
                Some('+') => o.push_str(&"+".repeat(self.dirstack.len())),
                _ => {}
            }
        }
        o
    }

    // -------------------------------------------------------------- execution
    pub fn run_line(&mut self, line: &str) {
        let trimmed = line.trim_start();
        let trimmed = trimmed.strip_prefix('@').unwrap_or(trimmed);
        if trimmed.trim().is_empty() { return; }
        let expanded = self.expand(trimmed);
        for (join, pipe) in split_ops(&expanded) {
            match join { Join::And if self.errorlevel != 0 => continue, Join::Or if self.errorlevel == 0 => continue, _ => {} }
            self.run_pipeline(&pipe);
            if self.exit.is_some() { return; }
        }
    }

    fn run_pipeline(&mut self, stages: &[String]) {
        if stages.len() == 1 {
            let mut so = std::io::stdout();
            self.run_simple(&parse_simple(&stages[0]), None, &mut so, true);
            return;
        }
        let mut data: Option<Vec<u8>> = None;
        for (i, st) in stages.iter().enumerate() {
            let last = i + 1 == stages.len();
            let s = parse_simple(st);
            if last {
                let mut so = std::io::stdout();
                self.run_simple(&s, data.take(), &mut so, true);
            } else {
                let mut buf: Vec<u8> = vec![];
                self.run_simple(&s, data.take(), &mut buf, false);
                data = Some(buf);
            }
        }
    }

    fn run_simple(&mut self, s: &Simple, input: Option<Vec<u8>>, out: Out, interactive: bool) {
        if s.words.is_empty() { return; }
        // redirections
        let mut file_out: Option<std::fs::File> = None;
        if let Some((f, append)) = &s.out {
            if f.eq_ignore_ascii_case("nul") { file_out = std::fs::OpenOptions::new().write(true).open("/dev/null").ok(); }
            else {
                let p = to_linux(&self.cwd, f);
                file_out = std::fs::OpenOptions::new().create(true).write(true).append(*append).truncate(!*append).open(&p).ok();
                if file_out.is_none() { wl!(out, "The system cannot find the path specified."); self.errorlevel = 1; return; }
            }
        }
        let input = match &s.input {
            Some(f) => match std::fs::read(to_linux(&self.cwd, f)) { Ok(d) => Some(d), Err(_) => { wl!(out, "The system cannot find the file specified."); self.errorlevel = 1; return; } },
            None => input,
        };
        let interactive = interactive && file_out.is_none();
        let mut nl = NlFix { inner: out };
        let target: &mut dyn Write = match file_out.as_mut() { Some(f) => f, None => &mut nl };
        let cmd = s.words[0].clone();
        let args: Vec<String> = s.words[1..].to_vec();
        self.dispatch(&cmd, &args, &s.raw, input, target, interactive, s);
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(&mut self, cmd: &str, args: &[String], raw: &str, input: Option<Vec<u8>>, o: Out, interactive: bool, s: &Simple) {
        let up = unquote(cmd).to_uppercase();
        // "echo." / "cd.." / "cd\" forms
        if up.starts_with("ECHO") && up.len() > 4 && ".:(/".contains(&up[4..5]) { let rest = &raw.trim_start()[5..]; wl!(o, "{rest}"); self.errorlevel = 0; return; }
        if (up.starts_with("CD") || up.starts_with("CHDIR")) && (up.ends_with("..") || up.ends_with('\\')) && !up.contains(' ') {
            let target = &cmd[if up.starts_with("CHDIR") { 5 } else { 2 }..];
            self.cd(target, o);
            return;
        }
        // drive switch "C:" / "D:"
        if up.len() == 2 && up.ends_with(':') { if &up[..1] != "C" { wl!(o, "The system cannot find the drive specified."); self.errorlevel = 1; } return; }
        let rest_raw = raw.trim_start().get(cmd.len()..).unwrap_or("").trim_start().to_string();
        let a = |i: usize| args.get(i).map(|s| s.as_str()).unwrap_or("");
        let has = |f: &str| args.iter().any(|x| x.eq_ignore_ascii_case(f));
        if has("/?") { self.help_for(&up, o); return; }
        self.errorlevel = 0;
        match up.as_str() {
            "REM" | "::" => {}
            "CLS" => { w!(o, "\x1b[3J\x1b[2J\x1b[H"); }
            "ECHO" => {
                let r = rest_raw.trim_end();
                match r.to_uppercase().as_str() {
                    "" => { wl!(o, "ECHO is {}.", if self.echo { "on" } else { "off" }); }
                    "ON" => self.echo = true,
                    "OFF" => self.echo = false,
                    _ => { wl!(o, "{}", strip_redirs(r)); }
                }
            }
            "CD" | "CHDIR" => { let t = args.iter().filter(|x| !x.eq_ignore_ascii_case("/d")).cloned().collect::<Vec<_>>().join(" "); self.cd(&t, o); }
            "DIR" => self.dir(args, o),
            "TYPE" => {
                if args.is_empty() { wl!(o, "The syntax of the command is incorrect."); self.errorlevel = 1; return; }
                let many = args.len() > 1 || args.iter().any(|x| x.contains('*'));
                for a in args {
                    for p in glob(&self.cwd, a) {
                        match std::fs::read(&p) {
                            Ok(d) => { if many { wl!(o); wl!(o, "{}", p.file_name().unwrap().to_string_lossy()); wl!(o); wl!(o); } let _ = o.write_all(&d); }
                            Err(_) => { wl!(o, "The system cannot find the file specified."); self.errorlevel = 1; }
                        }
                    }
                }
            }
            "MD" | "MKDIR" => {
                for a in args {
                    let p = to_linux(&self.cwd, a);
                    if p.exists() { wl!(o, "A subdirectory or file {} already exists.", unquote(a)); self.errorlevel = 1; }
                    else if std::fs::create_dir_all(&p).is_err() { wl!(o, "Access is denied."); self.errorlevel = 1; }
                }
            }
            "RD" | "RMDIR" => {
                let sub = has("/s");
                let quiet = has("/q");
                for a in args.iter().filter(|x| !x.starts_with('/')) {
                    let p = to_linux(&self.cwd, a);
                    if !p.is_dir() { wl!(o, "The system cannot find the file specified."); self.errorlevel = 2; continue; }
                    if sub {
                        if !quiet && !self.confirm(&format!("{}, Are you sure (Y/N)? ", unquote(a)), o) { continue; }
                        if std::fs::remove_dir_all(&p).is_err() { wl!(o, "Access is denied."); self.errorlevel = 5; }
                    } else if std::fs::remove_dir(&p).is_err() { wl!(o, "The directory is not empty."); self.errorlevel = 145; }
                }
            }
            "DEL" | "ERASE" => {
                let quiet = has("/q");
                let files: Vec<&String> = args.iter().filter(|x| !x.starts_with('/')).collect();
                if files.is_empty() { wl!(o, "The syntax of the command is incorrect."); self.errorlevel = 1; return; }
                for a in files {
                    let p0 = to_linux(&self.cwd, a);
                    let targets = if p0.is_dir() {
                        if !quiet && !self.confirm(&format!("{}\\*, Are you sure (Y/N)? ", to_win(&p0)), o) { continue; }
                        std::fs::read_dir(&p0).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect()).unwrap_or_default()
                    } else {
                        if (a.ends_with("*.*") || a.trim_matches('"') == "*") && !quiet && !self.confirm(&format!("{}, Are you sure (Y/N)? ", to_win(&self.cwd.join("*"))), o) { continue; }
                        glob(&self.cwd, a)
                    };
                    if targets.is_empty() || (targets.len() == 1 && !targets[0].exists()) { wl!(o, "Could Not Find {}", to_win(&p0)); continue; }
                    for t in targets { if t.is_file() && std::fs::remove_file(&t).is_err() { wl!(o, "{}", to_win(&t)); wl!(o, "Access is denied."); } }
                }
            }
            "COPY" | "MOVE" => {
                let files: Vec<&String> = args.iter().filter(|x| !x.starts_with('/')).collect();
                if files.is_empty() { wl!(o, "The syntax of the command is incorrect."); self.errorlevel = 1; return; }
                let (srcs, dst) = if files.len() == 1 { (vec![files[0]], self.cwd.clone()) } else { (files[..files.len() - 1].to_vec(), to_linux(&self.cwd, files[files.len() - 1])) };
                let mut n = 0;
                for s in srcs {
                    let list = glob(&self.cwd, s);
                    if list.iter().all(|p| !p.exists()) { wl!(o, "The system cannot find the file specified."); self.errorlevel = 1; continue; }
                    for src in list {
                        let target = if dst.is_dir() { dst.join(src.file_name().unwrap()) } else { dst.clone() };
                        if target.exists() && !has("/y") && src != target && !self.confirm(&format!("Overwrite {}? (Yes/No/All): ", to_win(&target)), o) { continue; }
                        let ok = if up == "MOVE" { std::fs::rename(&src, &target).is_ok() || (std::fs::copy(&src, &target).is_ok() && std::fs::remove_file(&src).is_ok()) }
                            else if src.is_dir() { false } else { std::fs::copy(&src, &target).is_ok() };
                        if ok { n += 1; if args.len() > 2 || s.contains('*') { wl!(o, "{}", to_win(&src)); } } else { wl!(o, "Access is denied."); self.errorlevel = 1; }
                    }
                }
                if up == "COPY" { wl!(o, "        {n} file(s) copied."); } else { wl!(o, "        {n} file(s) moved."); }
            }
            "REN" | "RENAME" => {
                if args.len() != 2 { wl!(o, "The syntax of the command is incorrect."); self.errorlevel = 1; return; }
                let src = to_linux(&self.cwd, &args[0]);
                if !src.exists() { wl!(o, "The system cannot find the file specified."); self.errorlevel = 1; return; }
                let dst = src.with_file_name(unquote(&args[1]));
                if dst.exists() { wl!(o, "A duplicate file name exists, or the file\r\ncannot be found."); self.errorlevel = 1; return; }
                if std::fs::rename(&src, &dst).is_err() { wl!(o, "Access is denied."); self.errorlevel = 1; }
            }
            "SET" => self.set(&rest_raw, o),
            "SETLOCAL" => self.locals.push((self.vars.clone(), self.cwd.clone())),
            "ENDLOCAL" => if let Some((v, c)) = self.locals.pop() { self.vars = v; self.cwd = c; },
            "PATH" => {
                if rest_raw.is_empty() { wl!(o, "PATH={}", self.vars.get("PATH").cloned().unwrap_or_default()); }
                else { let v = rest_raw.trim_start_matches('=').to_string(); self.vars.insert("PATH".into(), v.clone()); unsafe { std::env::set_var("PATH", v); } }
            }
            "PROMPT" => { self.vars.insert("PROMPT".into(), if rest_raw.is_empty() { "$P$G".into() } else { rest_raw.clone() }); }
            "TITLE" => { w!(o, "\x1b]2;{}\x07", rest_raw); }
            "COLOR" => {
                let c = a(0).to_uppercase();
                if c.is_empty() { w!(o, "\x1b]7700;\x07"); }
                else if c.len() == 2 && c.chars().all(|x| x.is_ascii_hexdigit()) && c.as_bytes()[0] != c.as_bytes()[1] { w!(o, "\x1b]7700;{c}\x07"); }
                else { self.errorlevel = 1; }
            }
            "VER" => { wl!(o); wl!(o, "{}", version()); }
            "VOL" => { wl!(o, " Volume in drive C has no label."); wl!(o, " Volume Serial Number is {}", serial(&self.cwd)); }
            "DATE" => {
                wl!(o, "{}The current date is: {}", if has("/t") { "" } else { "" }, chrono::Local::now().format("%a %m/%d/%Y"));
                if !has("/t") && interactive { w!(o, "Enter the new date: (mm-dd-yy) "); let _ = o.flush(); let _ = self.read_plain_line(); wl!(o, "A required privilege is not held by the client."); }
            }
            "TIME" => {
                wl!(o, "The current time is: {}", chrono::Local::now().format("%k:%M:%S.%2f"));
                if !has("/t") && interactive { w!(o, "Enter the new time: "); let _ = o.flush(); let _ = self.read_plain_line(); wl!(o, "A required privilege is not held by the client."); }
            }
            "PAUSE" => { w!(o, "Press any key to continue . . . "); let _ = o.flush(); self.wait_key(); wl!(o); }
            "EXIT" => {
                let code = args.iter().find(|x| !x.starts_with('/')).and_then(|x| x.parse().ok()).unwrap_or(self.errorlevel);
                if has("/b") && self.batch.is_some() { if let Some(b) = &mut self.batch { b.exit_b = true; } self.errorlevel = code; }
                else { self.exit = Some(code); }
            }
            "PUSHD" => {
                if args.is_empty() { for d in self.dirstack.iter().rev() { wl!(o, "{}", to_win(d)); } return; }
                let p = to_linux(&self.cwd, &args.join(" "));
                if p.is_dir() { self.dirstack.push(self.cwd.clone()); self.cwd = p; let _ = std::env::set_current_dir(&self.cwd); } else { wl!(o, "The system cannot find the path specified."); self.errorlevel = 1; }
            }
            "POPD" => if let Some(p) = self.dirstack.pop() { self.cwd = p; let _ = std::env::set_current_dir(&self.cwd); },
            "START" => {
                // START ["title"] [/D path] [/WAIT] command
                let mut it = args.iter().peekable();
                if it.peek().map(|x| x.starts_with('"')).unwrap_or(false) { it.next(); }
                let mut wait = false;
                let rest: Vec<String> = it.filter(|x| { if x.eq_ignore_ascii_case("/wait") { wait = true; false } else { !x.starts_with('/') } }).cloned().collect();
                if rest.is_empty() { crate::util::spawn(&["aerode-cmd"]); return; }
                let target = unquote(&rest[0]);
                let p = to_linux(&self.cwd, &target);
                if p.exists() && !is_exec(&p) { crate::util::open_path(&p); }
                else if target.contains("://") { crate::util::open_uri(&target); }
                else {
                    let exe = map_windows_program(&target).unwrap_or(target);
                    let mut c = Command::new(&exe);
                    c.args(rest[1..].iter().map(|x| unquote(x))).current_dir(&self.cwd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                    match c.spawn() { Ok(mut ch) => { if wait { let _ = ch.wait(); } } Err(_) => { wl!(o, "The system cannot find the file {exe}."); self.errorlevel = 9009; } }
                }
            }
            "CALL" => {
                if args.is_empty() { return; }
                let first = unquote(&args[0]);
                if let Some(label) = first.strip_prefix(':') { self.call_label(label, &args[1..]); return; }
                let line = args.join(" ");
                let saved = self.batch.take();
                self.run_line(&line);
                self.batch = saved;
            }
            "GOTO" => {
                let label = unquote(a(0)).trim_start_matches(':').to_lowercase();
                if let Some(b) = &mut self.batch {
                    if label == "eof" { b.pc = b.lines.len(); return; }
                    match b.lines.iter().position(|l| l.trim().strip_prefix(':').map(|x| x.split_whitespace().next().unwrap_or("").to_lowercase() == label).unwrap_or(false)) {
                        Some(i) => b.pc = i + 1,
                        None => { wl!(o, "The system cannot find the batch label specified - {}", a(0)); b.pc = b.lines.len(); self.errorlevel = 1; }
                    }
                }
            }
            "SHIFT" => if let Some(b) = &mut self.batch { if !b.args.is_empty() { b.args.remove(0); } },
            "IF" => self.if_cmd(&rest_raw, o),
            "FOR" => self.for_cmd(&rest_raw, o),
            "HELP" => {
                if !args.is_empty() { self.help_for(&a(0).to_uppercase(), o); return; }
                wl!(o, "For more information on a specific command, type HELP command-name");
                for (n, d) in BUILTINS { wl!(o, "{:<14}{}", n, d); }
                wl!(o);
                wl!(o, "Any other command runs the Linux program of that name (for example fish).");
            }
            "TREE" => {
                let root = if args.iter().any(|x| !x.starts_with('/')) { to_linux(&self.cwd, args.iter().find(|x| !x.starts_with('/')).unwrap()) } else { self.cwd.clone() };
                let files = has("/f");
                let ascii = has("/a");
                wl!(o, "Folder PATH listing");
                wl!(o, "Volume serial number is {}", serial(&root));
                wl!(o, "{}", to_win(&root));
                tree(&root, "", files, ascii, o, 0);
            }
            "MORE" => {
                let data = input.unwrap_or_else(|| args.first().and_then(|f| std::fs::read(to_linux(&self.cwd, f)).ok()).unwrap_or_default());
                let text = String::from_utf8_lossy(&data);
                let (_, rows) = line::term_size();
                for (i, l) in text.lines().enumerate() {
                    wl!(o, "{l}");
                    if interactive && (i + 1) % (rows.saturating_sub(1)).max(1) == 0 { w!(o, "-- More --"); let _ = o.flush(); self.wait_key(); w!(o, "\r          \r"); }
                }
            }
            "SORT" => {
                let data = input.unwrap_or_else(|| args.iter().find(|x| !x.starts_with('/')).and_then(|f| std::fs::read(to_linux(&self.cwd, f)).ok()).unwrap_or_default());
                let mut lines: Vec<String> = String::from_utf8_lossy(&data).lines().map(String::from).collect();
                lines.sort_by_key(|a| a.to_lowercase());
                if has("/r") { lines.reverse(); }
                for l in lines { wl!(o, "{l}"); }
            }
            "FIND" => {
                let insensitive = has("/i");
                let invert = has("/v");
                let count = has("/c");
                let needle = args.iter().find(|x| x.starts_with('"')).map(|x| unquote(x)).unwrap_or_default();
                let files: Vec<&String> = args.iter().filter(|x| !x.starts_with('/') && !x.starts_with('"')).collect();
                let check = |l: &str| { let hit = if insensitive { l.to_lowercase().contains(&needle.to_lowercase()) } else { l.contains(&needle) }; hit != invert };
                let mut found = false;
                if files.is_empty() {
                    let d = input.unwrap_or_default();
                    let t = String::from_utf8_lossy(&d);
                    let hits: Vec<&str> = t.lines().filter(|l| check(l)).collect();
                    found = !hits.is_empty();
                    if count { wl!(o, "{}", hits.len()); } else { for h in hits { wl!(o, "{h}"); } }
                } else {
                    for f in files {
                        let p = to_linux(&self.cwd, f);
                        let t = std::fs::read_to_string(&p).unwrap_or_default();
                        let hits: Vec<&str> = t.lines().filter(|l| check(l)).collect();
                        found |= !hits.is_empty();
                        if count { wl!(o, "---------- {}: {}", to_win(&p).to_uppercase(), hits.len()); } else { wl!(o); wl!(o, "---------- {}", to_win(&p).to_uppercase()); for h in hits { wl!(o, "{h}"); } }
                    }
                }
                self.errorlevel = if found { 0 } else { 1 };
            }
            "WHOAMI" => { wl!(o, "{}\\{}", gtk::glib::host_name().to_lowercase(), self.var("USERNAME").unwrap_or_default().to_lowercase()); }
            "HOSTNAME" => { wl!(o, "{}", gtk::glib::host_name()); }
            "WHERE" => {
                let mut any = false;
                for n in args {
                    let n = unquote(n);
                    for d in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
                        let p = d.join(&n);
                        if p.is_file() { wl!(o, "{}", to_win(&p)); any = true; }
                    }
                }
                if !any { wl!(o, "INFO: Could not find files for the given pattern(s)."); self.errorlevel = 1; }
            }
            "TIMEOUT" => {
                let secs: i64 = args.windows(2).find(|w| w[0].eq_ignore_ascii_case("/t")).and_then(|w| w[1].parse().ok()).or_else(|| a(0).parse().ok()).unwrap_or(0);
                for left in (1..=secs).rev() {
                    w!(o, "\rWaiting for {left:>2} seconds, press a key to continue ...");
                    let _ = o.flush();
                    let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
                    if !has("/nobreak") && unsafe { libc::poll(&mut fds, 1, 1000) } > 0 { let _ = line::read_key(); break; } else if has("/nobreak") { std::thread::sleep(std::time::Duration::from_secs(1)); }
                }
                wl!(o, "\rWaiting for  0 seconds, press a key to continue ...");
            }
            "ATTRIB" => {
                use std::os::unix::fs::PermissionsExt;
                let files: Vec<&String> = args.iter().filter(|x| !x.starts_with('/') && !x.starts_with('+') && !x.starts_with('-')).collect();
                let list = if files.is_empty() { glob(&self.cwd, "*") } else { files.iter().flat_map(|f| glob(&self.cwd, f)).collect() };
                for p in list {
                    let Ok(m) = std::fs::metadata(&p) else { continue };
                    let mut perm = m.permissions();
                    if has("+r") { perm.set_mode(perm.mode() & !0o222); let _ = std::fs::set_permissions(&p, perm.clone()); }
                    if has("-r") { perm.set_mode(perm.mode() | 0o200); let _ = std::fs::set_permissions(&p, perm.clone()); }
                    let ro = perm.mode() & 0o200 == 0;
                    let hidden = p.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false);
                    wl!(o, "{}    {}{}       {}", if m.is_file() { "A" } else { " " }, if hidden { "H" } else { " " }, if ro { "R" } else { " " }, to_win(&p));
                }
            }
            "TASKLIST" => {
                wl!(o);
                wl!(o, "{:<25} {:>8} {:<16} {:>11} {:>12}", "Image Name", "PID", "Session Name", "Session#", "Mem Usage");
                wl!(o, "{} {} {} {} {}", "=".repeat(25), "=".repeat(8), "=".repeat(16), "=".repeat(11), "=".repeat(12));
                let mut procs: Vec<(u32, String, u64)> = std::fs::read_dir("/proc").map(|rd| rd.flatten().filter_map(|e| {
                    let pid: u32 = e.file_name().to_string_lossy().parse().ok()?;
                    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?.trim().to_string();
                    let rss = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?.lines().find_map(|l| l.strip_prefix("VmRSS:").map(|v| v.split_whitespace().next().unwrap_or("0").parse::<u64>().unwrap_or(0))).unwrap_or(0);
                    Some((pid, name, rss))
                }).collect()).unwrap_or_default();
                procs.sort_by_key(|p| p.0);
                for (pid, name, rss) in procs { wl!(o, "{:<25} {:>8} {:<16} {:>11} {:>10} K", name.chars().take(25).collect::<String>(), pid, "Console", 1, thousands(rss)); }
            }
            "TASKKILL" => {
                let force = has("/f");
                let sig = if force { "-KILL" } else { "-TERM" };
                let mut i = 0;
                while i < args.len() {
                    let f = args[i].to_lowercase();
                    if (f == "/pid" || f == "/im") && i + 1 < args.len() {
                        let v = unquote(&args[i + 1]);
                        let ok = if f == "/pid" { Command::new("kill").args([sig, &v]).status().map(|s| s.success()).unwrap_or(false) }
                            else { Command::new("pkill").args([sig, "-x", v.trim_end_matches(".exe")]).status().map(|s| s.success()).unwrap_or(false) };
                        if ok { wl!(o, "SUCCESS: Sent termination signal to the process {} \"{}\".", if f == "/pid" { "with PID" } else { "" }, v); }
                        else { wl!(o, "ERROR: The process \"{v}\" not found."); self.errorlevel = 128; }
                        i += 2;
                    } else { i += 1; }
                }
            }
            "IPCONFIG" => ipconfig(has("/all"), o),
            "PING" => {
                let host = args.iter().find(|x| !x.starts_with('-') && !x.starts_with('/')).cloned().unwrap_or_default();
                if host.is_empty() { wl!(o, "Usage: ping target_name"); self.errorlevel = 1; return; }
                let n = args.windows(2).find(|w| w[0].eq_ignore_ascii_case("-n")).and_then(|w| w[1].parse::<u32>().ok()).unwrap_or(4);
                ping(&host, n, o, &mut self.errorlevel);
            }
            "CLIP" => {
                let d = input.unwrap_or_default();
                if let Ok(mut c) = Command::new("xclip").args(["-selection", "clipboard"]).stdin(Stdio::piped()).spawn() { if let Some(mut si) = c.stdin.take() { let _ = si.write_all(&d); } let _ = c.wait(); }
            }
            "SYSTEMINFO" => {
                let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
                let name = os.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).unwrap_or("Linux").trim_matches('"').to_string();
                let mem = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
                let kb = |k: &str| mem.lines().find_map(|l| l.strip_prefix(k).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse::<u64>().ok())).unwrap_or(0);
                wl!(o);
                for (k, v) in [("Host Name", gtk::glib::host_name().to_uppercase()), ("OS Name", name), ("OS Version", crate::util::output(&["uname", "-r"]).unwrap_or_default().trim().to_string()),
                    ("System Type", format!("{}-based PC", std::env::consts::ARCH)), ("Total Physical Memory", format!("{} MB", thousands(kb("MemTotal:") / 1024))),
                    ("Available Physical Memory", format!("{} MB", thousands(kb("MemAvailable:") / 1024)))] { wl!(o, "{:<27}{}", format!("{k}:"), v); }
            }
            "MKLINK" => {
                let parts: Vec<&String> = args.iter().filter(|x| !x.starts_with('/')).collect();
                if parts.len() != 2 { wl!(o, "The syntax of the command is incorrect."); self.errorlevel = 1; return; }
                let (link, target) = (to_linux(&self.cwd, parts[0]), to_linux(&self.cwd, parts[1]));
                let r = if has("/h") { std::fs::hard_link(&target, &link) } else { std::os::unix::fs::symlink(&target, &link) };
                if r.is_ok() { wl!(o, "{} created for {} <<===>> {}", if has("/h") { "Hardlink" } else if has("/d") || has("/j") { "Junction" } else { "symbolic link" }, unquote(parts[0]), unquote(parts[1])); } else { wl!(o, "Cannot create a file when that file already exists."); self.errorlevel = 1; }
            }
            _ => self.external(cmd, args, input, o, interactive, s),
        }
    }

    fn help_for(&self, up: &str, o: Out) {
        match BUILTINS.iter().find(|(n, _)| *n == up) {
            Some((n, d)) => { wl!(o, "{d}"); wl!(o); wl!(o, "{n} [options]"); }
            None => { wl!(o, "This command is not supported by the help utility.  Try \"{} --help\".", up.to_lowercase()); }
        }
    }

    fn cd(&mut self, target: &str, o: Out) {
        let t = target.trim();
        if t.is_empty() { wl!(o, "{}", to_win(&self.cwd)); return; }
        let p = to_linux(&self.cwd, t);
        if p.is_dir() { self.cwd = p; let _ = std::env::set_current_dir(&self.cwd); }
        else { wl!(o, "The system cannot find the path specified."); self.errorlevel = 1; }
    }

    fn dir(&mut self, args: &[String], o: Out) {
        let bare = args.iter().any(|x| x.eq_ignore_ascii_case("/b"));
        let wide = args.iter().any(|x| x.eq_ignore_ascii_case("/w"));
        let recurse = args.iter().any(|x| x.eq_ignore_ascii_case("/s"));
        let all = args.iter().any(|x| x.to_lowercase().starts_with("/a"));
        let order = args.iter().find(|x| x.to_lowercase().starts_with("/o")).map(|x| x[2..].trim_start_matches(':').to_lowercase());
        let targets: Vec<&String> = args.iter().filter(|x| !x.starts_with('/')).collect();
        let spec = targets.first().map(|s| s.as_str()).unwrap_or("");
        let (dir, pat) = {
            let p = to_linux(&self.cwd, spec);
            if spec.contains('*') || spec.contains('?') { (p.parent().map(|x| x.to_path_buf()).unwrap_or(self.cwd.clone()), p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or("*".into())) }
            else if p.is_dir() { (p, "*".to_string()) }
            else if p.exists() { (p.parent().unwrap().to_path_buf(), p.file_name().unwrap().to_string_lossy().into_owned()) }
            else { if !bare { wl!(o, " Volume in drive C has no label."); wl!(o, " Volume Serial Number is {}", serial(&self.cwd)); wl!(o); wl!(o, " Directory of {}", to_win(p.parent().unwrap_or(&self.cwd))); wl!(o); } wl!(o, "File Not Found"); self.errorlevel = 1; return; }
        };
        if !bare { wl!(o, " Volume in drive C has no label."); wl!(o, " Volume Serial Number is {}", serial(&dir)); }
        let (mut tf, mut tb, mut td) = (0u64, 0u64, 0u64);
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut entries: Vec<(String, std::fs::Metadata)> = rd.flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), e.metadata().ok()?))).collect();
            entries.retain(|(n, _)| wildcard_match(&pat, n) && (all || !n.starts_with('.')));
            match order.as_deref() {
                Some(x) if x.contains('s') => entries.sort_by_key(|(_, m)| m.len()),
                Some(x) if x.contains('d') => entries.sort_by_key(|(_, m)| m.modified().ok()),
                Some(x) if x.contains('g') || x.contains('n') => entries.sort_by(|a, b| b.1.is_dir().cmp(&a.1.is_dir()).then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))),
                _ => entries.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase())),
            }
            if order.as_deref().map(|x| x.contains('-')).unwrap_or(false) { entries.reverse(); }
            let (mut nf, mut nb, mut nd) = (0u64, 0u64, 0u64);
            if bare {
                for (n, _) in &entries { wl!(o, "{}", if recurse { to_win(&d.join(n)) } else { n.clone() }); }
            } else {
                wl!(o);
                wl!(o, " Directory of {}", to_win(&d));
                wl!(o);
                let dm = std::fs::metadata(&d).ok();
                let dot_time = dm.and_then(|m| m.modified().ok()).unwrap_or(std::time::SystemTime::now());
                if pat == "*" && d != Path::new("/") {
                    for n in [".", ".."] { if wide { w!(o, "{:<16}", format!("[{n}]")); } else { wl!(o, "{}    <DIR>          {n}", win_date(dot_time)); } nd += 1; }
                }
                let mut col = 2;
                for (n, m) in &entries {
                    let t = m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    if wide {
                        let s = if m.is_dir() { format!("[{n}]") } else { n.clone() };
                        w!(o, "{:<16}", s);
                        col += 1;
                        if col % 5 == 0 { wl!(o); }
                    } else if m.is_dir() { wl!(o, "{}    <DIR>          {n}", win_date(t)); }
                    else { wl!(o, "{} {:>17} {n}", win_date(t), thousands(m.len())); }
                    if m.is_dir() { nd += 1; } else { nf += 1; nb += m.len(); }
                }
                if wide && col % 5 != 0 { wl!(o); }
                wl!(o, "{:>16} File(s) {:>14} bytes", nf, thousands(nb));
                if !recurse { wl!(o, "{:>16} Dir(s) {:>15} bytes free", nd, thousands(free_bytes(&d))); }
            }
            tf += nf; tb += nb; td += nd;
            if recurse {
                let mut subs: Vec<PathBuf> = std::fs::read_dir(&d).map(|r| r.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) && (all || !e.file_name().to_string_lossy().starts_with('.'))).map(|e| e.path()).collect()).unwrap_or_default();
                subs.sort();
                subs.reverse();
                stack.extend(subs);
            }
        }
        if recurse && !bare {
            wl!(o);
            wl!(o, "     Total Files Listed:");
            wl!(o, "{:>16} File(s) {:>14} bytes", tf, thousands(tb));
            wl!(o, "{:>16} Dir(s) {:>15} bytes free", td, thousands(free_bytes(&dir)));
        }
    }

    fn set(&mut self, rest: &str, o: Out) {
        let rest = rest.trim();
        let lower = rest.to_lowercase();
        if rest.is_empty() { for (k, v) in &self.vars { wl!(o, "{k}={v}"); } return; }
        if let Some(e) = lower.strip_prefix("/a").map(|_| rest[2..].trim()) {
            let e = e.trim_matches('"');
            let (name, expr) = match e.split_once('=') { Some((n, x)) => (Some(n.trim().to_string()), x.to_string()), None => (None, e.to_string()) };
            // compound operators like +=
            let (name, expr) = match &name {
                Some(n) if n.ends_with(['+', '-', '*', '/', '%']) => { let op = n.chars().last().unwrap(); let n2 = n[..n.len() - 1].trim().to_string(); let cur = self.var(&n2).unwrap_or("0".into()); (Some(n2), format!("{cur}{op}({expr})")) }
                _ => (name, expr),
            };
            let me = &*self;
            let expr = expr.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|t| !t.is_empty() && t.chars().next().unwrap().is_alphabetic()).fold(expr.clone(), |acc, t| acc.replace(t, &me.var(t).unwrap_or("0".into())));
            match eval_arith(&expr) {
                Some(v) => { if let Some(n) = name { self.vars.insert(n.to_uppercase(), v.to_string()); } if self.batch.is_none() { w!(o, "{v}"); } }
                None => { wl!(o, "Missing operand."); self.errorlevel = 1; }
            }
            return;
        }
        if let Some(e) = lower.strip_prefix("/p").map(|_| rest[2..].trim()) {
            let (name, prompt) = e.split_once('=').unwrap_or((e, ""));
            w!(o, "{}", prompt.trim_matches('"'));
            let _ = o.flush();
            let v = self.read_plain_line().unwrap_or_default();
            if !v.is_empty() { self.vars.insert(name.trim().to_uppercase(), v); }
            return;
        }
        let rest = rest.trim_matches('"');
        match rest.split_once('=') {
            Some((k, v)) => {
                let k = k.trim().to_uppercase();
                if v.is_empty() { self.vars.remove(&k); unsafe { std::env::remove_var(&k); } } else { self.vars.insert(k.clone(), v.to_string()); if k == "PATH" { unsafe { std::env::set_var("PATH", v); } } }
            }
            None => {
                let p = rest.to_uppercase();
                let hits: Vec<_> = self.vars.iter().filter(|(k, _)| k.starts_with(&p)).collect();
                if hits.is_empty() { wl!(o, "Environment variable {rest} not defined"); self.errorlevel = 1; }
                for (k, v) in hits { wl!(o, "{k}={v}"); }
            }
        }
    }

    fn if_cmd(&mut self, rest: &str, o: Out) {
        let mut s = rest.trim().to_string();
        let mut ci = false;
        let mut not = false;
        loop {
            let l = s.to_lowercase();
            if l.starts_with("/i ") { ci = true; s = s[3..].trim_start().to_string(); } else if l.starts_with("not ") { not = true; s = s[4..].trim_start().to_string(); } else { break; }
        }
        let l = s.to_lowercase();
        let take_word = |s: &str| -> (String, String) {
            let s = s.trim_start();
            if let Some(r) = s.strip_prefix('"') { let end = r.find('"').map(|i| i + 2).unwrap_or(s.len()); (s[..end].to_string(), s[end..].to_string()) }
            else { let end = s.find(' ').unwrap_or(s.len()); (s[..end].to_string(), s[end..].to_string()) }
        };
        let (cond, then) = if l.starts_with("exist ") {
            let (w, r) = take_word(&s[6..]);
            (!glob(&self.cwd, &w).iter().all(|p| !p.exists()), r)
        } else if l.starts_with("defined ") {
            let (w, r) = take_word(&s[8..]);
            (self.var(&w).is_some(), r)
        } else if l.starts_with("errorlevel ") {
            let (w, r) = take_word(&s[11..]);
            (self.errorlevel >= w.parse().unwrap_or(0), r)
        } else if let Some(i) = s.find("==") {
            let a = s[..i].trim().to_string();
            let (b, r) = take_word(&s[i + 2..]);
            (if ci { a.to_lowercase() == b.to_lowercase() } else { a == b }, r)
        } else {
            // IF a OP b (EQU NEQ LSS LEQ GTR GEQ)
            let (a, r1) = take_word(&s);
            let (op, r2) = take_word(&r1);
            let (b, r3) = take_word(&r2);
            let (na, nb) = (a.trim_matches('"').parse::<i64>(), b.trim_matches('"').parse::<i64>());
            let ord = match (na, nb) { (Ok(x), Ok(y)) => x.cmp(&y), _ => if ci { a.to_lowercase().cmp(&b.to_lowercase()) } else { a.cmp(&b) } };
            let c = match op.to_uppercase().as_str() { "EQU" => ord.is_eq(), "NEQ" => ord.is_ne(), "LSS" => ord.is_lt(), "LEQ" => ord.is_le(), "GTR" => ord.is_gt(), "GEQ" => ord.is_ge(), _ => { wl!(o, "{op} was unexpected at this time."); self.errorlevel = 255; return; } };
            (c, r3)
        };
        let then = then.trim().to_string();
        // ( ... ) ELSE ( ... )
        let (t, e) = split_else(&then);
        let branch = if cond != not { Some(t) } else { e };
        if let Some(b) = branch { let b = b.trim().trim_start_matches('(').trim_end_matches(')').to_string(); for line in b.lines() { self.run_line(line); } }
    }

    fn for_cmd(&mut self, rest: &str, o: Out) {
        // FOR [/D|/L|/F ["opts"]] %%x IN (set) DO command
        let mut s = rest.trim().to_string();
        let mut mode = ' ';
        let mut fopts = String::new();
        if s.len() > 2 && s.starts_with('/') { mode = s.as_bytes()[1].to_ascii_lowercase() as char; s = s[2..].trim_start().to_string(); if mode == 'f' && s.starts_with('"') { let e = s[1..].find('"').map(|i| i + 2).unwrap_or(1); fopts = s[1..e - 1].to_string(); s = s[e..].trim_start().to_string(); } }
        let var_end = s.find(' ').unwrap_or(s.len());
        let var = s[..var_end].trim_start_matches('%').to_string();
        let lower = s.to_lowercase();
        let (Some(in_i), Some(do_i)) = (lower.find(" in "), lower.find(" do ")) else { wl!(o, "The syntax of the command is incorrect."); self.errorlevel = 1; return; };
        let set = s[in_i + 4..do_i].trim().trim_start_matches('(').trim_end_matches(')').to_string();
        let body = s[do_i + 4..].to_string();
        let mut items: Vec<String> = vec![];
        match mode {
            'l' => { let n: Vec<i64> = set.split([',', ' ']).filter_map(|x| x.trim().parse().ok()).collect(); if n.len() == 3 && n[1] != 0 { let mut i = n[0]; while (n[1] > 0 && i <= n[2]) || (n[1] < 0 && i >= n[2]) { items.push(i.to_string()); i += n[1]; } } }
            'f' => {
                let delims = fopts.split(' ').find_map(|x| x.strip_prefix("delims=")).map(|d| d.to_string()).unwrap_or_else(|| " \t".into());
                let token: usize = fopts.split(' ').find_map(|x| x.strip_prefix("tokens=")).and_then(|t| t.split(',').next()?.parse().ok()).unwrap_or(1);
                let text = if set.starts_with('\'') { String::from_utf8_lossy(&self.capture(set.trim_matches('\''))).into_owned() } else if set.starts_with('"') { set.trim_matches('"').to_string() } else { std::fs::read_to_string(to_linux(&self.cwd, &set)).unwrap_or_default() };
                for l in text.lines() {
                    if l.trim().is_empty() { continue; }
                    let parts: Vec<&str> = if delims.is_empty() { vec![l] } else { l.split(|c| delims.contains(c)).filter(|x| !x.is_empty()).collect() };
                    if let Some(t) = parts.get(token - 1) { items.push(t.to_string()); }
                }
            }
            _ => for w in set.split_whitespace() {
                if w.contains('*') || w.contains('?') {
                    for p in glob(&self.cwd, w) { if (mode == 'd') == p.is_dir() { items.push(p.file_name().unwrap().to_string_lossy().into_owned()); } }
                } else { items.push(w.to_string()); }
            },
        }
        for it in items {
            let cmd = body.replace(&format!("%%{var}"), &it).replace(&format!("%{var}"), &it);
            self.run_line(&cmd);
        }
    }

    /// Run a command line and capture its output (for FOR /F 'cmd').
    fn capture(&mut self, line: &str) -> Vec<u8> {
        let mut buf = vec![];
        for (_, pipe) in split_ops(&self.expand(line)) {
            let s = parse_simple(pipe.last().unwrap());
            self.run_simple(&s, None, &mut buf, false);
        }
        buf
    }

    fn call_label(&mut self, label: &str, args: &[String]) {
        let Some(b) = &self.batch else { return };
        let Some(i) = b.lines.iter().position(|l| l.trim().strip_prefix(':').map(|x| x.split_whitespace().next().unwrap_or("").eq_ignore_ascii_case(label)).unwrap_or(false)) else { return };
        let sub = Batch { lines: b.lines.clone(), pc: i + 1, args: args.to_vec(), path: b.path.clone(), exit_b: false };
        let saved = self.batch.replace(sub);
        self.run_batch_loop();
        self.batch = saved;
    }

    pub fn run_batch(&mut self, path: &Path, args: &[String]) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
        let saved = self.batch.replace(Batch { lines, pc: 0, args: args.to_vec(), path: path.to_path_buf(), exit_b: false });
        let echo = self.echo;
        self.run_batch_loop();
        self.batch = saved;
        if self.batch.is_none() { self.echo = echo; }
    }

    fn run_batch_loop(&mut self) {
        loop {
            let (line, quiet) = {
                let Some(b) = &mut self.batch else { return };
                if b.exit_b || b.pc >= b.lines.len() { return; }
                let l = b.lines[b.pc].clone();
                b.pc += 1;
                // join parenthesised blocks spanning lines
                let mut l = l;
                let mut depth = l.matches('(').count() as i32 - l.matches(')').count() as i32;
                while depth > 0 && b.pc < b.lines.len() { let n = b.lines[b.pc].clone(); b.pc += 1; depth += n.matches('(').count() as i32 - n.matches(')').count() as i32; l.push('\n'); l.push_str(&n); }
                let quiet = l.trim_start().starts_with('@');
                (l, quiet)
            };
            let t = line.trim();
            if t.is_empty() || t.starts_with(':') { continue; }
            if self.echo && !quiet {
                print!("\r\n{}{}\r\n", self.prompt_text(), t);
                let _ = std::io::stdout().flush();
            }
            // multi-line IF/FOR blocks: run as one logical line
            let joined = line.replace('\n', " & ").replace("( &", "(").replace("& )", ")");
            self.run_line(&joined);
            if self.exit.is_some() { return; }
        }
    }

    fn confirm(&mut self, q: &str, o: Out) -> bool {
        w!(o, "{q}");
        let _ = o.flush();
        let a = self.read_plain_line().unwrap_or_default();
        a.trim().to_lowercase().starts_with('y') || a.trim().to_lowercase().starts_with('a')
    }

    fn read_plain_line(&mut self) -> Option<String> {
        let mut s = String::new();
        loop {
            match line::read_key()? {
                line::Key::Enter => { print!("\r\n"); let _ = std::io::stdout().flush(); return Some(s); }
                line::Key::Back => { if s.pop().is_some() { print!("\x08 \x08"); let _ = std::io::stdout().flush(); } }
                line::Key::Char(c) => { s.push(c); print!("{c}"); let _ = std::io::stdout().flush(); }
                line::Key::CtrlC => { print!("^C\r\n"); return None; }
                _ => {}
            }
        }
    }

    fn wait_key(&self) { let _ = line::read_key(); }

    fn external(&mut self, cmd: &str, args: &[String], input: Option<Vec<u8>>, o: Out, interactive: bool, s: &Simple) {
        let name = unquote(cmd);
        // batch files
        let p = to_linux(&self.cwd, &name);
        let batch = [p.clone(), p.with_extension("bat"), p.with_extension("cmd")].into_iter().find(|c| c.is_file() && c.extension().map(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd")).unwrap_or(false));
        if let Some(b) = batch { self.run_batch(&b, args); return; }
        let exe = map_windows_program(&name).unwrap_or_else(|| {
            let n = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(&name).to_string();
            if n.contains('\\') || n.contains('/') { to_linux(&self.cwd, &n).to_string_lossy().into_owned() } else { n }
        });
        let resolvable = exe.contains('/') && Path::new(&exe).is_file() || std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|d| d.join(&exe).is_file());
        if !resolvable {
            // a document? open it with its program, like Windows
            if p.is_file() { crate::util::open_path(&p); return; }
            wl!(o, "'{}' is not recognized as an internal or external command,", name);
            wl!(o, "operable program or batch file.");
            self.errorlevel = 9009;
            return;
        }
        let mut c = Command::new(&exe);
        c.args(args.iter().map(|a| unquote(a))).current_dir(&self.cwd);
        for (k, v) in &self.vars { if k != "PATH" && k != "PROMPT" { c.env(k, v); } }
        if let Some(t) = &self.tty { t.restore(); }
        let piped_out = !interactive || s.out.is_some();
        c.stdin(if input.is_some() { Stdio::piped() } else { Stdio::inherit() });
        c.stdout(if piped_out { Stdio::piped() } else { Stdio::inherit() });
        if s.err_to_out && piped_out { c.stderr(Stdio::piped()); }
        else if let Some((f, a)) = &s.err {
            if let Ok(file) = if f.eq_ignore_ascii_case("nul") { std::fs::OpenOptions::new().write(true).open("/dev/null") } else { std::fs::OpenOptions::new().create(true).write(true).append(*a).truncate(!*a).open(to_linux(&self.cwd, f)) } { c.stderr(file); }
        }
        // let Ctrl+C reach the child, not us
        unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN); }
        match c.spawn() {
            Ok(mut ch) => {
                if let (Some(d), Some(mut si)) = (input, ch.stdin.take()) { std::thread::spawn(move || { let _ = si.write_all(&d); }); }
                if piped_out {
                    let mut so = ch.stdout.take();
                    let mut se = ch.stderr.take();
                    let mut buf = vec![];
                    if let Some(so) = so.as_mut() { let _ = so.read_to_end(&mut buf); }
                    if let Some(se) = se.as_mut() { let _ = se.read_to_end(&mut buf); }
                    let _ = o.write_all(&buf);
                }
                let st = ch.wait();
                self.errorlevel = st.ok().and_then(|s| s.code()).unwrap_or(1);
            }
            Err(e) => { wl!(o, "{}", e); self.errorlevel = 1; }
        }
        unsafe { libc::signal(libc::SIGINT, libc::SIG_DFL); }
        if let Some(t) = &self.tty { let _ = t; self.tty = line::Tty::raw(); }
    }

    // -------------------------------------------------------------- tab completion
    fn complete(&self, line: &str, cursor: usize, idx: usize) -> Option<(String, usize)> {
        let chars: Vec<char> = line.chars().collect();
        let upto: String = chars[..cursor.min(chars.len())].iter().collect();
        // the token being completed (quotes allowed)
        let mut start = 0;
        let mut q = false;
        for (i, c) in upto.char_indices() { if c == '"' { q = !q; } if c == ' ' && !q { start = i + 1; } }
        let tok = upto[start..].trim_matches('"').to_string();
        let (dir_part, prefix) = match tok.rfind(['\\', '/']) { Some(i) => (tok[..=i].to_string(), tok[i + 1..].to_string()), None => (String::new(), tok.clone()) };
        let dir = if dir_part.is_empty() { self.cwd.clone() } else { to_linux(&self.cwd, &dir_part) };
        let first_word = start == 0;
        let mut names: Vec<String> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_lowercase().starts_with(&prefix.to_lowercase()) && (!n.starts_with('.') || prefix.starts_with('.'))).collect();
        if first_word && dir_part.is_empty() && names.is_empty() { return None; }
        names.sort_by_key(|n| n.to_lowercase());
        if names.is_empty() { return None; }
        let i = if idx >= usize::MAX - 1 { names.len() - 1 } else { idx % names.len() };
        let mut full = format!("{dir_part}{}", names[i]);
        if full.contains(' ') { full = format!("\"{full}\""); }
        let rest: String = chars[cursor.min(chars.len())..].iter().collect();
        let new = format!("{}{full}", &upto[..start]);
        let pos = new.chars().count();
        Some((format!("{new}{rest}"), pos))
    }

    /// Interactive loop (the console session).
    pub fn repl(&mut self) -> i32 {
        self.tty = line::Tty::raw();
        loop {
            let p = self.prompt_text();
            print!("{p}");
            let _ = std::io::stdout().flush();
            let plen = p.rsplit('\n').next().unwrap_or(&p).chars().count();
            let mut ed = std::mem::replace(&mut self.editor, line::Editor { history: vec![], max_history: 0, discard_dups: false, insert: true, popup_fg: 5, popup_bg: 15 });
            let me: &Shell = self;
            let complete = |l: &str, c: usize, i: usize| me.complete(l, c, i);
            let line = ed.read_line(plen, &complete);
            self.editor = ed;
            let Some(line) = line else { break };
            if line == "\u{3}" { continue; }
            if let Some(t) = &self.tty { t.restore(); }
            self.run_line(&line);
            if self.exit.is_some() { break; }
            // cmd prints an empty line after every command's output
            print!("\r\n");
            let _ = std::io::stdout().flush();
            self.tty = line::Tty::raw();
        }
        if let Some(t) = &self.tty { t.restore(); }
        self.exit.unwrap_or(0)
    }
}

/// Converts lone \n to \r\n when writing to the terminal in raw mode.
struct NlFix<'a> { inner: &'a mut dyn Write }
impl Write for NlFix<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let mut out = Vec::with_capacity(b.len());
        let mut prev = 0u8;
        for &c in b { if c == b'\n' && prev != b'\r' { out.push(b'\r'); } out.push(c); prev = c; }
        self.inner.write_all(&out)?;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
}

fn strip_redirs(s: &str) -> String { parse_simple(&format!("echo {s}")).raw.trim_start_matches("echo ").to_string().split('>').next().unwrap_or("").trim_end().to_string() }

fn replace_ci(v: &str, from: &str, to: &str) -> String {
    if from.is_empty() { return v.to_string(); }
    let (lv, lf) = (v.to_lowercase(), from.to_lowercase());
    let mut out = String::new();
    let mut i = 0;
    while let Some(p) = lv[i..].find(&lf) { out.push_str(&v[i..i + p]); out.push_str(to); i += p + from.len(); }
    out.push_str(&v[i..]);
    out
}

fn split_else(s: &str) -> (String, Option<String>) {
    // find " else " outside parentheses/quotes
    let l = s.to_lowercase();
    let mut depth = 0i32;
    let mut q = false;
    for (i, c) in s.char_indices() {
        if c == '"' { q = !q; }
        if !q { if c == '(' { depth += 1 } if c == ')' { depth -= 1 } }
        if !q && depth == 0 && l[i..].starts_with(" else ") { return (s[..i].to_string(), Some(s[i + 6..].to_string())); }
        if !q && depth == 0 && l[i..].starts_with(")else") { return (s[..=i].to_string(), Some(s[i + 5..].to_string())); }
    }
    (s.to_string(), None)
}

fn eval_arith(e: &str) -> Option<i64> {
    // tiny precedence-climbing evaluator for SET /A
    fn parse(t: &[char], i: &mut usize, min: u8) -> Option<i64> {
        let skip = |t: &[char], i: &mut usize| { while *i < t.len() && t[*i] == ' ' { *i += 1; } };
        skip(t, i);
        let mut lhs = if *i < t.len() && t[*i] == '(' { *i += 1; let v = parse(t, i, 0)?; skip(t, i); *i += 1; v }
            else if *i < t.len() && t[*i] == '-' { *i += 1; -parse(t, i, 9)? }
            else if *i < t.len() && t[*i] == '!' { *i += 1; (parse(t, i, 9)? == 0) as i64 }
            else if *i < t.len() && t[*i] == '~' { *i += 1; !parse(t, i, 9)? }
            else {
                let s = *i;
                while *i < t.len() && (t[*i].is_ascii_alphanumeric()) { *i += 1; }
                let n: String = t[s..*i].iter().collect();
                if let Some(h) = n.strip_prefix("0x") { i64::from_str_radix(h, 16).ok()? } else { n.parse().ok()? }
            };
        loop {
            skip(t, i);
            if *i >= t.len() { break; }
            let (op, prec, len) = match (t[*i], t.get(*i + 1)) {
                ('<', Some('<')) => ("<<", 5, 2), ('>', Some('>')) => (">>", 5, 2),
                ('*', _) => ("*", 7, 1), ('/', _) => ("/", 7, 1), ('%', _) => ("%", 7, 1),
                ('+', _) => ("+", 6, 1), ('-', _) => ("-", 6, 1), ('&', _) => ("&", 4, 1), ('^', _) => ("^", 3, 1), ('|', _) => ("|", 2, 1),
                _ => break,
            };
            if prec < min { break; }
            *i += len;
            let rhs = parse(t, i, prec + 1)?;
            lhs = match op { "*" => lhs.wrapping_mul(rhs), "/" => if rhs == 0 { return None } else { lhs / rhs }, "%" => if rhs == 0 { return None } else { lhs % rhs },
                "+" => lhs.wrapping_add(rhs), "-" => lhs.wrapping_sub(rhs), "<<" => lhs << (rhs & 63), ">>" => lhs >> (rhs & 63), "&" => lhs & rhs, "^" => lhs ^ rhs, _ => lhs | rhs };
        }
        Some(lhs)
    }
    let t: Vec<char> = e.chars().collect();
    let mut i = 0;
    parse(&t, &mut i, 0)
}

fn is_exec(p: &Path) -> bool { use std::os::unix::fs::PermissionsExt; p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false) }

/// Windows program names → Aerode/Linux programs.
fn map_windows_program(n: &str) -> Option<String> {
    let l = n.to_lowercase();
    let l = l.trim_end_matches(".exe");
    Some(match l {
        "notepad" => "aerode-notepad", "calc" => "aerode-calc", "explorer" => "aerode-explorer", "taskmgr" => "aerode-taskmgr",
        "control" => "aerode-control", "snippingtool" => "aerode-snip", "cmd" => "aerode-cmd", "mspaint" => "kolourpaint",
        "write" | "wordpad" => "libreoffice", "iexplore" => "xdg-open", "regedit" => return None,
        _ => return None,
    }.to_string())
}

pub fn version() -> String {
    std::fs::read_to_string(crate::config::conf_dir().join("cmd-version.txt")).ok().and_then(|s| s.lines().next().map(String::from)).unwrap_or_else(|| "Microsoft Windows [Version 6.1.7601]".into())
}

/// Text shown when cmd starts (customise in ~/.config/aerode/cmd-banner.txt).
pub fn banner() -> String {
    std::fs::read_to_string(crate::config::conf_dir().join("cmd-banner.txt")).unwrap_or_else(|_| format!("{}\nAerode Command Processor. Type HELP for a list of commands.\n", version()))
}

fn serial(p: &Path) -> String {
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap_or_default();
    let id = if unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0 { st.f_fsid as u64 } else { 0x1234ABCD };
    let id = (id ^ (id >> 32)) as u32;
    format!("{:04X}-{:04X}", id >> 16, id & 0xffff)
}

fn free_bytes(p: &Path) -> u64 {
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap_or_default();
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0 { st.f_bavail as u64 * st.f_frsize as u64 } else { 0 }
}

fn tree(p: &Path, prefix: &str, files: bool, ascii: bool, o: Out, depth: usize) {
    if depth > 32 { return; }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|x| !x.file_name().unwrap().to_string_lossy().starts_with('.')).collect();
    entries.sort_by_key(|x| x.file_name().unwrap().to_string_lossy().to_lowercase());
    let dirs: Vec<&PathBuf> = entries.iter().filter(|x| x.is_dir()).collect();
    let (tee, ell, bar) = if ascii { ("+---", "\\---", "|   ") } else { ("├───", "└───", "│   ") };
    if files {
        for f in entries.iter().filter(|x| x.is_file()) { wl!(o, "{}{}{}", prefix, if dirs.is_empty() { "    " } else { bar }, f.file_name().unwrap().to_string_lossy()); }
        if entries.iter().any(|x| x.is_file()) { wl!(o, "{}{}", prefix, if dirs.is_empty() { "" } else { bar }); }
    }
    if depth == 0 && dirs.is_empty() { wl!(o, "No subfolders exist"); wl!(o); }
    for (i, d) in dirs.iter().enumerate() {
        let last = i + 1 == dirs.len();
        wl!(o, "{}{}{}", prefix, if last { ell } else { tee }, d.file_name().unwrap().to_string_lossy());
        tree(d, &format!("{}{}", prefix, if last { "    " } else { bar }), files, ascii, o, depth + 1);
    }
}

fn ipconfig(all: bool, o: Out) {
    wl!(o);
    wl!(o, "Windows IP Configuration");
    wl!(o);
    if all {
        wl!(o, "   Host Name . . . . . . . . . . . . : {}", gtk::glib::host_name());
        wl!(o, "   Primary Dns Suffix  . . . . . . . : ");
        wl!(o, "   Node Type . . . . . . . . . . . . : Hybrid");
        wl!(o, "   IP Routing Enabled. . . . . . . . : No");
        wl!(o);
    }
    let text = crate::util::output(&["ip", "-o", "addr"]).unwrap_or_default();
    let route = crate::util::output(&["ip", "route", "show", "default"]).unwrap_or_default();
    let mut ifaces: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 4 || f[1] == "lo" { continue; }
        let e = ifaces.entry(f[1].to_string()).or_default();
        if f[2] == "inet" { e.0.push(f[3].to_string()); } else if f[2] == "inet6" { e.1.push(f[3].to_string()); }
    }
    let links: HashMap<String, String> = std::fs::read_dir("/sys/class/net").map(|rd| rd.flatten().map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.clone(), std::fs::read_to_string(e.path().join("address")).unwrap_or_default().trim().replace(':', "-").to_uppercase())
    }).collect()).unwrap_or_default();
    for (name, (v4, v6)) in &ifaces {
        let wifi = Path::new(&format!("/sys/class/net/{name}/wireless")).exists();
        wl!(o, "{} adapter {}:", if wifi { "Wireless LAN" } else { "Ethernet" }, name);
        wl!(o);
        if v4.is_empty() && v6.is_empty() { wl!(o, "   Media State . . . . . . . . . . . : Media disconnected"); wl!(o); continue; }
        wl!(o, "   Connection-specific DNS Suffix  . : ");
        if all { wl!(o, "   Physical Address. . . . . . . . . : {}", links.get(name).cloned().unwrap_or_default()); }
        for a in v6.iter().filter(|a| a.starts_with("fe80")) { wl!(o, "   Link-local IPv6 Address . . . . . : {}", a.split('/').next().unwrap_or("")); }
        for a in v4 {
            let (ip, bits) = a.split_once('/').unwrap_or((a, "24"));
            let bits: u32 = bits.parse().unwrap_or(24);
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            wl!(o, "   IPv4 Address. . . . . . . . . . . : {ip}");
            wl!(o, "   Subnet Mask . . . . . . . . . . . : {}.{}.{}.{}", mask >> 24, (mask >> 16) & 255, (mask >> 8) & 255, mask & 255);
        }
        let gw = route.lines().find(|l| l.contains(&format!("dev {name}"))).and_then(|l| l.split_whitespace().nth(2)).unwrap_or("");
        wl!(o, "   Default Gateway . . . . . . . . . : {gw}");
        wl!(o);
    }
}

fn ping(host: &str, n: u32, o: Out, el: &mut i32) {
    let out = Command::new("ping").args(["-c", &n.to_string(), "-W", "4", host]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let Ok(mut ch) = out else { wl!(o, "Ping request could not find host {host}. Please check the name and try again."); *el = 1; return };
    let stdout = ch.stdout.take().unwrap();
    let reader = std::io::BufReader::new(stdout);
    use std::io::BufRead;
    let mut ip = host.to_string();
    let (mut sent, mut recv, mut times) = (0, 0, vec![]);
    for l in reader.lines().map_while(Result::ok) {
        if l.starts_with("PING") {
            ip = l.split('(').nth(1).and_then(|s| s.split(')').next()).unwrap_or(host).to_string();
            wl!(o);
            if ip == host { wl!(o, "Pinging {ip} with 32 bytes of data:"); } else { wl!(o, "Pinging {host} [{ip}] with 32 bytes of data:"); }
            let _ = o.flush();
        } else if l.contains("bytes from") {
            sent += 1; recv += 1;
            let t = l.split("time=").nth(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
            let ttl = l.split("ttl=").nth(1).and_then(|s| s.split_whitespace().next()).unwrap_or("64");
            times.push(t);
            wl!(o, "Reply from {ip}: bytes=32 time{}ms TTL={ttl}", if t < 1.0 { "<1".to_string() } else { format!("={}", t.round()) });
            let _ = o.flush();
        } else if l.contains("packets transmitted") {
            sent = l.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(sent);
        }
    }
    let ok = ch.wait().map(|s| s.success()).unwrap_or(false);
    if sent == 0 && !ok { wl!(o, "Ping request could not find host {host}. Please check the name and try again."); *el = 1; return; }
    for _ in recv..sent { wl!(o, "Request timed out."); }
    wl!(o);
    wl!(o, "Ping statistics for {ip}:");
    let lost = sent - recv;
    wl!(o, "    Packets: Sent = {sent}, Received = {recv}, Lost = {lost} ({}% loss),", if sent > 0 { lost * 100 / sent } else { 0 });
    if !times.is_empty() {
        let (mn, mx) = times.iter().fold((f64::MAX, 0f64), |(a, b), t| (a.min(*t), b.max(*t)));
        let avg = times.iter().sum::<f64>() / times.len() as f64;
        wl!(o, "Approximate round trip times in milli-seconds:");
        wl!(o, "    Minimum = {}ms, Maximum = {}ms, Average = {}ms", mn.round(), mx.round(), avg.round());
    }
    *el = if recv > 0 { 0 } else { 1 };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn paths() {
        assert_eq!(to_win(Path::new("/home/demid/Documents")), "C:\\Users\\demid\\Documents");
        assert_eq!(to_win(Path::new("/")), "C:\\");
        assert_eq!(to_linux(Path::new("/tmp"), "C:\\Users"), PathBuf::from("/home"));
        assert_eq!(to_linux(Path::new("/home"), ".."), PathBuf::from("/"));
    }
    #[test] fn wildcards() { assert!(wildcard_match("*.txt", "a.TXT")); assert!(wildcard_match("*.*", "README")); assert!(!wildcard_match("a?c", "abbc")); }
    #[test] fn ops() { let v = split_ops("echo a & echo b && echo c | find \"c\""); assert_eq!(v.len(), 3); assert_eq!(v[2].1.len(), 2); }
    #[test] fn redirs() { let s = parse_simple("dir /b > out.txt 2>&1"); assert_eq!(s.words, vec!["dir", "/b"]); assert_eq!(s.out, Some(("out.txt".into(), false))); assert!(s.err_to_out); }
    #[test] fn arith() { assert_eq!(eval_arith("2+3*4"), Some(14)); assert_eq!(eval_arith("(1+2)<<2"), Some(12)); }
    #[test] fn else_split() { let (a, b) = split_else("(echo yes) else (echo no)"); assert_eq!(a, "(echo yes)"); assert_eq!(b.as_deref(), Some("(echo no)")); }
}
