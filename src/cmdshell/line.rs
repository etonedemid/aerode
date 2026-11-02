//! cmd.exe line editing (what conhost does in "cooked" mode on Windows):
//! history, F1–F9, Insert/overwrite, Esc, Tab completion, the F7 popup.
use std::io::{Read, Write};

pub struct Tty { saved: libc::termios }

impl Tty {
    pub fn raw() -> Option<Tty> {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 { return None; }
            let saved = t;
            t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
            t.c_iflag &= !(libc::IXON | libc::ICRNL);
            t.c_cc[libc::VMIN] = 1;
            t.c_cc[libc::VTIME] = 0;
            libc::tcsetattr(0, libc::TCSANOW, &t);
            Some(Tty { saved })
        }
    }
    pub fn restore(&self) { unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.saved); } }
}

pub fn term_size() -> (usize, usize) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 { (ws.ws_col as usize, ws.ws_row as usize) } else { (80, 25) }
    }
}

#[derive(Debug, PartialEq)]
pub enum Key { Char(char), Enter, Back, Del, Left, Right, Up, Down, Home, End, PgUp, PgDn, Ins, Esc, Tab, ShiftTab, F(u8), AltF(u8), CtrlLeft, CtrlRight, CtrlC, CtrlZ, Other }

pub fn read_key() -> Option<Key> {
    let mut b = [0u8; 1];
    let mut stdin = std::io::stdin();
    if stdin.read(&mut b).ok()? == 0 { return None; }
    Some(match b[0] {
        b'\r' | b'\n' => Key::Enter,
        0x7f | 0x08 => Key::Back,
        b'\t' => Key::Tab,
        0x03 => Key::CtrlC,
        0x1a => Key::CtrlZ,
        0x1b => {
            // escape sequence? (non-blocking peek with poll)
            let mut fds = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut fds, 1, 30) } <= 0 { return Some(Key::Esc); }
            let mut seq = vec![];
            loop {
                if stdin.read(&mut b).ok()? == 0 { break; }
                seq.push(b[0]);
                if seq.len() >= 2 && (b[0].is_ascii_alphabetic() || b[0] == b'~') { break; }
                if seq.len() > 8 { break; }
            }
            let s = String::from_utf8_lossy(&seq).to_string();
            match s.as_str() {
                "[A" | "OA" => Key::Up, "[B" | "OB" => Key::Down, "[C" | "OC" => Key::Right, "[D" | "OD" => Key::Left,
                "[H" | "OH" | "[1~" => Key::Home, "[F" | "OF" | "[4~" => Key::End,
                "[2~" => Key::Ins, "[3~" => Key::Del, "[5~" => Key::PgUp, "[6~" => Key::PgDn, "[Z" => Key::ShiftTab,
                "OP" => Key::F(1), "OQ" => Key::F(2), "OR" => Key::F(3), "OS" => Key::F(4),
                "[15~" => Key::F(5), "[17~" => Key::F(6), "[18~" => Key::F(7), "[19~" => Key::F(8), "[20~" => Key::F(9),
                "[18;3~" => Key::AltF(7), "[1;5C" => Key::CtrlRight, "[1;5D" => Key::CtrlLeft,
                _ => Key::Other,
            }
        }
        c if c < 0x20 => Key::Other,
        c => {
            // UTF-8 continuation bytes
            let n = if c >= 0xF0 { 3 } else if c >= 0xE0 { 2 } else if c >= 0xC0 { 1 } else { 0 };
            let mut buf = vec![c];
            for _ in 0..n { if stdin.read(&mut b).ok()? == 0 { break; } buf.push(b[0]); }
            Key::Char(String::from_utf8_lossy(&buf).chars().next().unwrap_or('?'))
        }
    })
}

fn out(s: &str) { let mut o = std::io::stdout(); let _ = o.write_all(s.as_bytes()); let _ = o.flush(); }

pub struct Editor {
    pub history: Vec<String>,
    pub max_history: usize,
    pub discard_dups: bool,
    pub insert: bool,
    pub popup_fg: u8,
    pub popup_bg: u8,
}

/// Windows colour index → SGR code
fn sgr_win(idx: u8, bg: bool) -> u8 {
    const WIN_TO_ANSI: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];
    let base = WIN_TO_ANSI[(idx & 7) as usize];
    match (idx >= 8, bg) { (false, false) => 30 + base, (true, false) => 90 + base, (false, true) => 40 + base, (true, true) => 100 + base }
}

impl Editor {
    fn set_cursor_shape(&self) { out(if self.insert { "\x1b[4 q" } else { "\x1b[2 q" }); }

    fn push_history(&mut self, line: &str) {
        if line.trim().is_empty() { return; }
        if self.discard_dups { self.history.retain(|h| h != line); }
        else if self.history.last().map(|h| h == line).unwrap_or(false) { return; }
        self.history.push(line.to_string());
        if self.history.len() > self.max_history { self.history.remove(0); }
    }

    /// Read one line. `prompt_len` = columns already used by the prompt.
    pub fn read_line(&mut self, prompt_len: usize, complete: &dyn Fn(&str, usize, usize) -> Option<(String, usize)>) -> Option<String> {
        let mut buf: Vec<char> = vec![];
        let mut pos = 0usize;
        let mut hist_i = self.history.len();
        let mut comp_state: Option<(String, usize, usize)> = None; // (original line, cycle index, cursor)
        self.set_cursor_shape();
        // a prompt ending on a row's last column leaves the cursor waiting to wrap: wrap it now, so the
        // input starts at column 0 of the next row like on Windows
        let (cols, _) = term_size();
        if prompt_len > 0 && prompt_len % cols == 0 { out(" \r"); }
        loop {
            let (cols, _) = term_size();
            let k = read_key()?;
            let old_len = buf.len();
            let old_pos = pos;
            if k != Key::Tab && k != Key::ShiftTab { comp_state = None; }
            match k {
                Key::Enter => {
                    self.move_to(prompt_len, old_pos, buf.len(), cols);
                    out("\r\n");
                    let s: String = buf.iter().collect();
                    self.push_history(&s);
                    out("\x1b[0 q");
                    return Some(s);
                }
                Key::CtrlC => { self.move_to(prompt_len, pos, buf.len(), cols); out("^C\r\n"); return Some(String::new()).map(|_| "\u{3}".to_string()); }
                Key::CtrlZ => { buf.insert(pos, '\u{1a}'); pos += 1; }
                Key::Char(c) => {
                    if self.insert || pos >= buf.len() { buf.insert(pos, c); } else { buf[pos] = c; }
                    pos += 1;
                }
                Key::Back => if pos > 0 { pos -= 1; buf.remove(pos); },
                Key::Del => if pos < buf.len() { buf.remove(pos); },
                Key::Left => pos = pos.saturating_sub(1),
                Key::Right => {
                    // at end of line: copies the next char of the previous command (like F1)
                    if pos < buf.len() { pos += 1 } else if let Some(prev) = self.history.last() { if let Some(c) = prev.chars().nth(pos) { buf.push(c); pos += 1; } }
                }
                Key::Home => pos = 0,
                Key::End => pos = buf.len(),
                Key::CtrlLeft => { while pos > 0 && buf[pos - 1] == ' ' { pos -= 1; } while pos > 0 && buf[pos - 1] != ' ' { pos -= 1; } }
                Key::CtrlRight => { while pos < buf.len() && buf[pos] != ' ' { pos += 1; } while pos < buf.len() && buf[pos] == ' ' { pos += 1; } }
                Key::Ins => { self.insert = !self.insert; self.set_cursor_shape(); }
                Key::Esc => { buf.clear(); pos = 0; }
                Key::Up | Key::F(5) => if hist_i > 0 { hist_i -= 1; buf = self.history[hist_i].chars().collect(); pos = buf.len(); },
                Key::Down => if hist_i + 1 < self.history.len() { hist_i += 1; buf = self.history[hist_i].chars().collect(); pos = buf.len(); },
                Key::PgUp => if !self.history.is_empty() { hist_i = 0; buf = self.history[0].chars().collect(); pos = buf.len(); },
                Key::PgDn => if !self.history.is_empty() { hist_i = self.history.len() - 1; buf = self.history[hist_i].chars().collect(); pos = buf.len(); },
                Key::F(1) => if let Some(prev) = self.history.last() { if let Some(c) = prev.chars().nth(pos) { if pos < buf.len() { buf[pos] = c } else { buf.push(c) } pos += 1; } },
                Key::F(3) => if let Some(prev) = self.history.last() { let rest: Vec<char> = prev.chars().skip(buf.len()).collect(); buf.extend(rest); pos = buf.len(); },
                Key::F(2) | Key::F(4) => {
                    let f2 = k == Key::F(2);
                    let ch = self.prompt_box(if f2 { "Enter char to copy up to: " } else { "Enter char to delete up to: " });
                    if let Some(c) = ch {
                        if f2 {
                            if let Some(prev) = self.history.last() {
                                let p: Vec<char> = prev.chars().collect();
                                if let Some(end) = p.iter().skip(pos + 1).position(|x| *x == c).map(|i| i + pos + 1) {
                                    for (i, ch) in p[pos..end].iter().enumerate() { if pos + i < buf.len() { buf[pos + i] = *ch } else { buf.push(*ch) } }
                                    pos = end;
                                }
                            }
                        } else if let Some(end) = buf.iter().skip(pos + 1).position(|x| *x == c).map(|i| i + pos + 1) { buf.drain(pos..end); }
                    }
                }
                Key::F(6) => { buf.insert(pos, '\u{1a}'); pos += 1; }
                Key::F(7) => if let Some(i) = self.history_popup() { hist_i = i; buf = self.history[i].chars().collect(); pos = buf.len(); },
                Key::AltF(7) => { self.history.clear(); hist_i = 0; }
                Key::F(8) => {
                    // search history backwards for entries starting with the typed text
                    let prefix: String = buf[..pos].iter().collect();
                    let start = if hist_i == 0 { self.history.len() } else { hist_i };
                    if let Some(i) = (0..start).rev().find(|i| self.history[*i].starts_with(&prefix)) { hist_i = i; buf = self.history[i].chars().collect(); }
                }
                Key::F(9) => {
                    if let Some(n) = self.prompt_number("Enter command number: ") { if n < self.history.len() { hist_i = n; buf = self.history[n].chars().collect(); pos = buf.len(); } }
                }
                Key::Tab | Key::ShiftTab => {
                    let back = k == Key::ShiftTab;
                    let (orig, idx, cur) = comp_state.clone().unwrap_or_else(|| (buf.iter().collect(), usize::MAX, pos));
                    let next = if idx == usize::MAX { if back { usize::MAX - 1 } else { 0 } } else if back { idx.wrapping_sub(1) } else { idx + 1 };
                    if let Some((line, p)) = complete(&orig, cur, next) { buf = line.chars().collect(); pos = p; comp_state = Some((orig, next, cur)); }
                }
                _ => {}
            }
            self.redraw(prompt_len, &buf, old_pos, pos, old_len, cols);
        }
    }

    /// Move the cursor (which is at `from` in the input, placed explicitly) to the end of the text: the
    /// row of its last character (the terminal wraps lazily, so a full row keeps the cursor on it).
    fn move_to(&self, prompt_len: usize, from: usize, to: usize, cols: usize) {
        let r0 = (prompt_len + from) / cols;
        let end = prompt_len + to;
        let full = to > 0 && end % cols == 0;
        let r1 = if full { end / cols - 1 } else { end / cols };
        if r1 > r0 { out(&format!("\x1b[{}B", r1 - r0)); } else if r0 > r1 { out(&format!("\x1b[{}A", r0 - r1)); }
        out(&format!("\r\x1b[{}G", if full { cols } else { end % cols + 1 }));
    }

    /// Rewrite the input after the prompt (which may span several rows) and put the cursor at `pos`.
    /// Positions count from the start of the prompt; the cursor sits at `old_pos`, placed explicitly.
    fn redraw(&self, prompt_len: usize, buf: &[char], old_pos: usize, pos: usize, old_len: usize, cols: usize) {
        let col = |p: usize| p % cols + 1;
        // to the start of the input: the prompt's last row
        let (start_row, cur_row) = (prompt_len / cols, (prompt_len + old_pos) / cols);
        if cur_row > start_row { out(&format!("\x1b[{}A", cur_row - start_row)); }
        out(&format!("\r\x1b[{}G", col(prompt_len)));
        let s: String = buf.iter().map(|c| if *c == '\u{1a}' { '→' } else { *c }).collect();
        out(&s);
        let pad = old_len.saturating_sub(buf.len());
        if pad > 0 { out(&" ".repeat(pad)); }
        // where the terminal cursor is now: a write that ended on a row's last column leaves it on that
        // row (pending wrap)
        let written = buf.len().max(old_len);
        let end = prompt_len + written;
        let end_row = if written > 0 && end % cols == 0 { end / cols - 1 } else { end / cols };
        let want = (prompt_len + pos) / cols;
        if want > end_row { out(&"\r\n".repeat(want - end_row)); } else if end_row > want { out(&format!("\x1b[{}A", end_row - want)); }
        out(&format!("\r\x1b[{}G", col(prompt_len + pos)));
    }

    fn colors(&self) -> String { format!("\x1b[{};{}m", sgr_win(self.popup_fg, false), sgr_win(self.popup_bg, true)) }

    /// Draw a popup box (Windows draws these in the "popup" colours), centred.
    fn draw_box(&self, title: &str, lines: &[String], sel: Option<usize>, scroll: usize, height: usize) -> (usize, usize) {
        let (cols, rows) = term_size();
        let inner = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(title.chars().count() + 2).clamp(20, cols.saturating_sub(4));
        let w = inner + 2;
        let h = height + 2;
        let x = (cols.saturating_sub(w)) / 2 + 1;
        let y = (rows.saturating_sub(h)) / 2 + 1;
        let c = self.colors();
        let mut s = String::new();
        let t = if title.is_empty() { "═".repeat(inner) } else { format!("{}{}", title, "═".repeat(inner.saturating_sub(title.chars().count()))) };
        s += &format!("\x1b[{y};{x}H{c}╔{t}╗");
        for i in 0..height {
            let line = lines.get(scroll + i).cloned().unwrap_or_default();
            let line: String = line.chars().take(inner).collect();
            let pad = inner - line.chars().count();
            let body = if Some(scroll + i) == sel { format!("\x1b[7m{line}{}\x1b[27m", " ".repeat(pad)) } else { format!("{line}{}", " ".repeat(pad)) };
            s += &format!("\x1b[{};{x}H║{body}║", y + 1 + i);
        }
        s += &format!("\x1b[{};{x}H╚{}╝\x1b[0m", y + h - 1, "═".repeat(inner));
        out(&s);
        (x, y)
    }

    /// F7: scrollable history list; Enter picks, Esc cancels.
    fn history_popup(&self) -> Option<usize> {
        if self.history.is_empty() { return None; }
        let items: Vec<String> = self.history.iter().enumerate().map(|(i, h)| format!("{i}: {h}")).collect();
        let (_, rows) = term_size();
        let height = items.len().min(rows.saturating_sub(6)).clamp(1, 20);
        let mut sel = items.len() - 1;
        let mut scroll = sel.saturating_sub(height - 1);
        out("\x1b7\x1b[?7700h\x1b[?25l");
        let res = loop {
            self.draw_box("", &items, Some(sel), scroll, height);
            match read_key() {
                Some(Key::Up) => sel = sel.saturating_sub(1),
                Some(Key::Down) => if sel + 1 < items.len() { sel += 1 },
                Some(Key::PgUp) | Some(Key::Home) => sel = 0,
                Some(Key::PgDn) | Some(Key::End) => sel = items.len() - 1,
                Some(Key::Enter) | Some(Key::Right) | Some(Key::Left) => break Some(sel),
                Some(Key::Esc) | None => break None,
                _ => {}
            }
            if sel < scroll { scroll = sel } else if sel >= scroll + height { scroll = sel + 1 - height }
        };
        out("\x1b[?7700l\x1b8\x1b[?25h");
        res
    }

    fn prompt_box(&self, text: &str) -> Option<char> {
        out("\x1b7\x1b[?7700h");
        self.draw_box("", &[text.to_string()], None, 0, 1);
        let k = read_key();
        out("\x1b[?7700l\x1b8");
        match k { Some(Key::Char(c)) => Some(c), _ => None }
    }

    fn prompt_number(&self, text: &str) -> Option<usize> {
        out("\x1b7\x1b[?7700h");
        let mut num = String::new();
        loop {
            self.draw_box("", &[format!("{text}{num}")], None, 0, 1);
            match read_key() {
                Some(Key::Char(c)) if c.is_ascii_digit() && num.len() < 4 => num.push(c),
                Some(Key::Back) => { num.pop(); }
                Some(Key::Enter) => break,
                _ => { num.clear(); break; }
            }
        }
        out("\x1b[?7700l\x1b8");
        num.parse().ok()
    }
}
