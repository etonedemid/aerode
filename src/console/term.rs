//! Terminal state machine with Windows console semantics:
//! a fixed-size screen buffer (default 300 lines) with a viewport inside it,
//! plus enough VT/xterm support to run fish, vim, htop, etc.
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Color { Default, Idx(u8), Rgb(u8, u8, u8) }

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub reverse: bool,
    pub underline: bool,
    pub dim: bool,
    pub strike: bool,
}

impl Default for Cell {
    fn default() -> Self { Cell { ch: ' ', fg: Color::Default, bg: Color::Default, bold: false, reverse: false, underline: false, dim: false, strike: false } }
}

/// The second cell of a double-width character (never drawn, skipped in copied text).
pub const WIDE_TAIL: char = '\0';

/// Cells a character takes: 0 for combining marks and joiners, 2 for East Asian wide and emoji.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x300 { return 1; }
    if matches!(u, 0x300..=0x36F | 0x200B..=0x200F | 0x2060 | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF | 0xE0000..=0xE0FFF) { return 0; }
    if matches!(u, 0x1100..=0x115F | 0x231A..=0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD..=0x25FE | 0x2614..=0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1
        | 0x26AA..=0x26AB | 0x26BD..=0x26BE | 0x26C4..=0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2..=0x26F3 | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A..=0x270B | 0x2728 | 0x274C
        | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55 | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xA960..=0xA97F | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6
        | 0x1F004 | 0x1F0CF | 0x1F18E | 0x1F191..=0x1F19A | 0x1F200..=0x1F64F | 0x1F680..=0x1F6FF | 0x1F900..=0x1F9FF | 0x1FA70..=0x1FAFF | 0x20000..=0x3FFFD) { return 2; }
    1
}

#[derive(Clone, Copy, PartialEq)]
pub enum CursorShape { Small, Medium, Large }

pub struct Term {
    pub cols: usize,
    pub rows: usize,           // window/viewport height
    pub buf_height: usize,     // Windows "screen buffer" height
    lines: VecDeque<Vec<Cell>>, // whole buffer (len <= buf_height)
    pub vt: usize,             // buffer line where the VT screen (row 0) starts
    pub cx: usize,
    pub cy: usize,             // relative to vt
    saved: (usize, usize, Cell),
    pen: Cell,
    pub wrap_pending: bool,
    scroll_top: usize,
    scroll_bot: usize,         // inclusive, relative to vt
    pub autowrap: bool,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// xterm mouse reporting: 0 off, 1 clicks (1000), 2 clicks + drags (1002), 3 all motion (1003)
    pub mouse: u8,
    /// SGR (1006) mouse coordinates
    pub mouse_sgr: bool,
    /// focus in/out reports (1004)
    pub focus_events: bool,
    last_ch: char,
    pub cursor_visible: bool,
    pub cursor_shape: Option<CursorShape>,
    pub title: Option<String>,
    alt: Option<(VecDeque<Vec<Cell>>, usize, usize, usize)>, // saved main buffer when on alt screen
    pub replies: Vec<u8>,      // responses to send back to the program (DSR, DA)
    pub bell: bool,
    pub dirty: bool,
    tabs: Vec<bool>,
    snapshot: Option<(VecDeque<Vec<Cell>>, usize, usize, usize)>,
    /// `color XY` from the cmd shell: (text, background) as Windows indices
    pub console_color: Option<Option<(u8, u8)>>,
}

impl Term {
    pub fn new(cols: usize, rows: usize, buf_height: usize) -> Term {
        let mut t = Term {
            cols, rows, buf_height: buf_height.max(rows), lines: VecDeque::new(), vt: 0, cx: 0, cy: 0,
            saved: (0, 0, Cell::default()), pen: Cell::default(), wrap_pending: false, scroll_top: 0, scroll_bot: rows - 1,
            autowrap: true, app_cursor: false, bracketed_paste: false, mouse: 0, mouse_sgr: false, focus_events: false, last_ch: ' ', cursor_visible: true, cursor_shape: None, title: None,
            alt: None, replies: vec![], bell: false, dirty: true, tabs: vec![], snapshot: None, console_color: None,
        };
        t.reset_tabs();
        t.ensure_lines();
        t
    }

    fn reset_tabs(&mut self) { self.tabs = (0..self.cols).map(|i| i % 8 == 0 && i > 0).collect(); }

    fn blank(&self) -> Vec<Cell> { vec![Cell { bg: self.pen.bg, ..Cell::default() }; self.cols] }
    fn empty_line(&self) -> Vec<Cell> { vec![Cell::default(); self.cols] }

    /// Make sure the buffer covers the VT screen.
    fn ensure_lines(&mut self) {
        while self.lines.len() < self.vt + self.rows { let l = self.empty_line(); self.lines.push_back(l); }
    }

    pub fn total_lines(&self) -> usize { self.buf_height.max(self.lines.len()) }

    /// Line `i` of the whole buffer (blank if never written, like the Windows buffer).
    pub fn line(&self, i: usize) -> Option<&Vec<Cell>> { self.lines.get(i) }

    pub fn resize(&mut self, cols: usize, rows: usize, buf_height: usize) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        for l in self.lines.iter_mut() { l.resize(cols, Cell::default()); }
        self.cols = cols;
        self.buf_height = buf_height.max(rows);
        // keep the cursor visible: move the viewport if needed
        let abs = self.vt + self.cy;
        self.rows = rows;
        if abs >= self.vt + rows { self.vt = abs + 1 - rows; }
        self.cy = abs - self.vt;
        while self.lines.len() > self.buf_height && self.vt > 0 { self.lines.pop_front(); self.vt -= 1; }
        self.scroll_top = 0;
        self.scroll_bot = rows - 1;
        self.cx = self.cx.min(cols - 1);
        self.ensure_lines();
        self.reset_tabs();
        self.dirty = true;
    }

    fn row_mut(&mut self, y: usize) -> &mut Vec<Cell> {
        let i = self.vt + y;
        self.ensure_lines();
        &mut self.lines[i]
    }

    /// Scroll the region [top, bot] up by one line.
    fn scroll_up(&mut self, top: usize, bot: usize) {
        if top == 0 && bot == self.rows - 1 && self.alt.is_none() {
            // full-screen scroll: grow into the buffer (Windows keeps the history)
            if self.vt + self.rows < self.buf_height {
                self.vt += 1;
            } else {
                self.lines.pop_front();
            }
            self.ensure_lines();
            let b = self.blank();
            let i = self.vt + self.rows - 1;
            self.lines[i] = b;
        } else {
            let (a, b) = (self.vt + top, self.vt + bot);
            self.ensure_lines();
            self.lines.remove(a);
            let bl = self.blank();
            self.lines.insert(b, bl);
        }
    }

    fn scroll_down(&mut self, top: usize, bot: usize) {
        let (a, b) = (self.vt + top, self.vt + bot);
        self.ensure_lines();
        self.lines.remove(b);
        let bl = self.blank();
        self.lines.insert(a, bl);
    }

    fn linefeed(&mut self) {
        if self.cy == self.scroll_bot { self.scroll_up(self.scroll_top, self.scroll_bot); }
        else if self.cy < self.rows - 1 { self.cy += 1; }
    }

    pub fn put(&mut self, c: char) {
        let w = char_width(c);
        if w == 0 { return; } // ponytail: combining marks and joiners are dropped, cells hold one char
        if self.wrap_pending && self.autowrap {
            self.cx = 0;
            self.linefeed();
            self.wrap_pending = false;
        }
        // a wide character that doesn't fit on the rest of the line goes to the next one
        if w == 2 && self.cx + 1 >= self.cols && self.cols > 1 {
            if !self.autowrap { return; }
            self.cx = 0;
            self.linefeed();
        }
        let (cx, cy, cols) = (self.cx, self.cy, self.cols);
        let cell = Cell { ch: c, ..self.pen };
        let row = self.row_mut(cy);
        // overwriting half of a wide character leaves no half glyph behind
        if row[cx].ch == WIDE_TAIL && cx > 0 { row[cx - 1].ch = ' '; }
        if let Some(n) = row.get_mut(cx + w) && n.ch == WIDE_TAIL { n.ch = ' '; }
        row[cx] = cell;
        if w == 2 && cx + 1 < cols { row[cx + 1] = Cell { ch: WIDE_TAIL, ..cell }; }
        self.last_ch = c;
        if self.cx + w >= self.cols { self.cx = self.cols - 1; self.wrap_pending = true; } else { self.cx += w; }
        self.dirty = true;
    }

    /// Clear the whole Windows buffer (cls).
    pub fn clear_buffer(&mut self) {
        self.lines.clear();
        self.vt = 0;
        self.cx = 0;
        self.cy = 0;
        self.ensure_lines();
        self.dirty = true;
    }

    fn erase_cells(&mut self, y: usize, from: usize, to: usize) {
        let b = Cell { bg: self.pen.bg, ..Cell::default() };
        let row = self.row_mut(y);
        let n = row.len();
        for c in row.iter_mut().take(to.min(n)).skip(from) { *c = b; }
    }

    pub fn reset(&mut self) {
        let (c, r, h) = (self.cols, self.rows, self.buf_height);
        *self = Term::new(c, r, h);
    }

    /// Plain text of a rectangular selection (Windows console "Mark" is a block).
    pub fn text_block(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> String {
        let mut out = String::new();
        for y in y0..=y1 {
            let line = self.lines.get(y);
            let s: String = (x0..=x1.min(self.cols - 1)).map(|x| line.and_then(|l| l.get(x)).map(|c| c.ch).unwrap_or(' ')).filter(|c| *c != WIDE_TAIL).collect();
            out.push_str(s.trim_end());
            if y != y1 { out.push('\n'); }
        }
        out
    }

    fn sgr(&mut self, params: &[u16]) {
        let mut it = params.iter().copied().peekable();
        if params.is_empty() { self.pen = Cell::default(); return; }
        while let Some(p) = it.next() {
            match p {
                0 => { let ch = self.pen.ch; self.pen = Cell::default(); self.pen.ch = ch; }
                1 => self.pen.bold = true,
                2 => self.pen.dim = true,
                9 => self.pen.strike = true,
                21 => self.pen.underline = true,
                29 => self.pen.strike = false,
                4 => self.pen.underline = true,
                7 => self.pen.reverse = true,
                22 => { self.pen.bold = false; self.pen.dim = false; }
                24 => self.pen.underline = false,
                27 => self.pen.reverse = false,
                30..=37 => self.pen.fg = Color::Idx((p - 30) as u8),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Idx((p - 40) as u8),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Idx((p - 90 + 8) as u8),
                100..=107 => self.pen.bg = Color::Idx((p - 100 + 8) as u8),
                38 | 48 => {
                    let c = match it.next() {
                        Some(5) => it.next().map(|n| Color::Idx(n as u8)),
                        Some(2) => { let r = it.next().unwrap_or(0) as u8; let g = it.next().unwrap_or(0) as u8; let b = it.next().unwrap_or(0) as u8; Some(Color::Rgb(r, g, b)) }
                        _ => None,
                    };
                    if let Some(c) = c { if p == 38 { self.pen.fg = c } else { self.pen.bg = c } }
                }
                _ => {}
            }
        }
    }

    fn set_mode(&mut self, private: bool, params: &[u16], on: bool) {
        for p in params {
            match (private, p) {
                (true, 1) => self.app_cursor = on,
                (true, 7) => self.autowrap = on,
                (true, 25) => self.cursor_visible = on,
                (true, 2004) => self.bracketed_paste = on,
                (true, 1000) => self.mouse = if on { 1 } else { 0 },
                (true, 1002) => self.mouse = if on { 2 } else { 0 },
                (true, 1003) => self.mouse = if on { 3 } else { 0 },
                (true, 1006) => self.mouse_sgr = on,
                (true, 1004) => self.focus_events = on,
                // private: snapshot/restore the screen (cmd's F7/F9 popups draw over text)
                (true, 7700) => {
                    if on { self.snapshot = Some((self.lines.clone(), self.vt, self.cx, self.cy)); }
                    else if let Some((l, vt, cx, cy)) = self.snapshot.take() { self.lines = l; self.vt = vt; self.cx = cx; self.cy = cy; }
                    self.dirty = true;
                }
                (true, 47) | (true, 1047) | (true, 1049) => {
                    if on && self.alt.is_none() {
                        if *p == 1049 { self.saved = (self.cx, self.cy, self.pen); }
                        let main = std::mem::take(&mut self.lines);
                        self.alt = Some((main, self.vt, self.cx, self.cy));
                        self.vt = 0;
                        self.ensure_lines();
                    } else if !on
                        && let Some((main, vt, cx, cy)) = self.alt.take() {
                            self.lines = main; self.vt = vt; self.cx = cx; self.cy = cy;
                            if *p == 1049 { self.cx = self.saved.0; self.cy = self.saved.1; }
                            // the window may have been resized meanwhile: the main screen's lines get the
                            // new width and the cursor stays on the (possibly shorter) screen
                            let cols = self.cols;
                            for l in self.lines.iter_mut() { l.resize(cols, Cell::default()); }
                            self.cx = self.cx.min(cols - 1);
                            if self.cy >= self.rows { self.vt += self.cy + 1 - self.rows; self.cy = self.rows - 1; }
                            self.ensure_lines();
                        }
                    self.dirty = true;
                }
                _ => {}
            }
        }
    }

    pub fn on_alt_screen(&self) -> bool { self.alt.is_some() }
}

fn p0(params: &[u16], i: usize, def: u16) -> u16 { params.get(i).copied().filter(|v| *v != 0).unwrap_or(def) }

impl vte::Perform for Term {
    fn print(&mut self, c: char) { self.put(c); }

    fn execute(&mut self, b: u8) {
        match b {
            b'\n' | 0x0b | 0x0c => { self.linefeed(); self.wrap_pending = false; }
            b'\r' => { self.cx = 0; self.wrap_pending = false; }
            0x08 => { if self.cx > 0 { self.cx -= 1; } self.wrap_pending = false; }
            b'\t' => {
                let mut x = self.cx + 1;
                while x < self.cols - 1 && !self.tabs.get(x).copied().unwrap_or(false) { x += 1; }
                self.cx = x.min(self.cols - 1);
            }
            0x07 => self.bell = true,
            _ => {}
        }
        self.dirty = true;
    }

    fn csi_dispatch(&mut self, params: &vte::Params, inter: &[u8], _ignore: bool, action: char) {
        let ps: Vec<u16> = params.iter().map(|p| p.first().copied().unwrap_or(0)).collect();
        let private = inter.first() == Some(&b'?');
        let (rows, cols) = (self.rows, self.cols);
        // only moving the cursor or erasing ends a pending wrap: less changes colours at the right
        // margin and expects its next character on the next line
        if !matches!(action, 'm' | 'h' | 'l' | 'n' | 'c' | 'q' | 's' | 't' | 'g') { self.wrap_pending = false; }
        match action {
            'A' => self.cy = self.cy.saturating_sub(p0(&ps, 0, 1) as usize).max(if self.cy >= self.scroll_top { self.scroll_top } else { 0 }),
            'B' | 'e' => self.cy = (self.cy + p0(&ps, 0, 1) as usize).min(rows - 1),
            'C' | 'a' => self.cx = (self.cx + p0(&ps, 0, 1) as usize).min(cols - 1),
            'D' => self.cx = self.cx.saturating_sub(p0(&ps, 0, 1) as usize),
            'E' => { self.cy = (self.cy + p0(&ps, 0, 1) as usize).min(rows - 1); self.cx = 0; }
            'F' => { self.cy = self.cy.saturating_sub(p0(&ps, 0, 1) as usize); self.cx = 0; }
            'G' | '`' => self.cx = (p0(&ps, 0, 1) as usize - 1).min(cols - 1),
            'd' => self.cy = (p0(&ps, 0, 1) as usize - 1).min(rows - 1),
            'H' | 'f' => { self.cy = (p0(&ps, 0, 1) as usize - 1).min(rows - 1); self.cx = (p0(&ps, 1, 1) as usize - 1).min(cols - 1); }
            'J' => match ps.first().copied().unwrap_or(0) {
                0 => { let (x, y) = (self.cx, self.cy); self.erase_cells(y, x, cols); for r in y + 1..rows { self.erase_cells(r, 0, cols); } }
                1 => { let (x, y) = (self.cx, self.cy); for r in 0..y { self.erase_cells(r, 0, cols); } self.erase_cells(y, 0, x + 1); }
                2 => for r in 0..rows { self.erase_cells(r, 0, cols); },
                3 => self.clear_buffer(),
                _ => {}
            },
            'K' => { let (x, y) = (self.cx, self.cy); match ps.first().copied().unwrap_or(0) { 0 => self.erase_cells(y, x, cols), 1 => self.erase_cells(y, 0, x + 1), _ => self.erase_cells(y, 0, cols) } }
            'X' => { let (x, y) = (self.cx, self.cy); let n = p0(&ps, 0, 1) as usize; self.erase_cells(y, x, x + n); }
            'P' => { let (x, y) = (self.cx, self.cy); let n = (p0(&ps, 0, 1) as usize).min(cols - x); let b = Cell { bg: self.pen.bg, ..Cell::default() }; let row = self.row_mut(y); for _ in 0..n { row.remove(x); row.push(b); } }
            '@' => { let (x, y) = (self.cx, self.cy); let n = (p0(&ps, 0, 1) as usize).min(cols - x); let b = Cell { bg: self.pen.bg, ..Cell::default() }; let row = self.row_mut(y); for _ in 0..n { row.insert(x, b); row.pop(); } }
            'L' => if self.cy >= self.scroll_top && self.cy <= self.scroll_bot { for _ in 0..p0(&ps, 0, 1) { self.scroll_down(self.cy, self.scroll_bot); } },
            'M' => if self.cy >= self.scroll_top && self.cy <= self.scroll_bot { for _ in 0..p0(&ps, 0, 1) { let (a, b) = (self.cy, self.scroll_bot); let (vt, l) = (self.vt, self.blank()); self.lines.remove(vt + a); self.lines.insert(vt + b, l); } },
            'S' => for _ in 0..p0(&ps, 0, 1) { self.scroll_up(self.scroll_top, self.scroll_bot) },
            'T' => for _ in 0..p0(&ps, 0, 1) { self.scroll_down(self.scroll_top, self.scroll_bot) },
            'r' => {
                let t = p0(&ps, 0, 1) as usize - 1;
                let b = (p0(&ps, 1, rows as u16) as usize - 1).min(rows - 1);
                if t < b { self.scroll_top = t; self.scroll_bot = b; }
                self.cx = 0; self.cy = 0;
            }
            'm' if !private && inter.is_empty() => {
                // vte gives sub-params for 38:2:r:g:b; flatten them
                // (38:2::r:g:b has an empty colour space id, 4:0 is "no underline", 4:3 a curly one)
                let flat: Vec<u16> = params.iter().flat_map(|p| match p {
                    [c @ (38 | 48), 2, rest @ ..] if rest.len() >= 3 => { let r = &rest[rest.len() - 3..]; vec![*c, 2, r[0], r[1], r[2]] }
                    [4, 0] => vec![24],
                    [4, _] => vec![4],
                    [58, ..] => vec![],
                    p => p.to_vec(),
                }).collect();
                self.sgr(&flat);
            }
            'b' => { let c = self.last_ch; for _ in 0..p0(&ps, 0, 1).min(cols as u16) { self.put(c); } }
            'Z' => for _ in 0..p0(&ps, 0, 1) { let mut x = self.cx.saturating_sub(1); while x > 0 && !self.tabs.get(x).copied().unwrap_or(false) { x -= 1; } self.cx = x; },
            'I' => for _ in 0..p0(&ps, 0, 1) { self.execute(b'\t'); },
            'h' => self.set_mode(private, &ps, true),
            'l' => self.set_mode(private, &ps, false),
            's' if !private => self.saved = (self.cx, self.cy, self.pen),
            'u' if !private => { self.cx = self.saved.0; self.cy = self.saved.1; }
            'n' if ps.first() == Some(&6) => { let r = format!("\x1b[{};{}R", self.cy + 1, self.cx + 1); self.replies.extend(r.bytes()); }
            'n' if ps.first() == Some(&5) => self.replies.extend(b"\x1b[0n"),
            'c' if inter.is_empty() || inter == b">" => self.replies.extend(if inter == b">" { &b"\x1b[>0;10;1c"[..] } else { &b"\x1b[?1;2c"[..] }),
            'q' if inter == b" " => self.cursor_shape = match ps.first().copied().unwrap_or(0) { 0 => None, 1 | 2 => Some(CursorShape::Large), _ => Some(CursorShape::Small) },
            'g' => match ps.first().copied().unwrap_or(0) { 0 => { let x = self.cx; if let Some(t) = self.tabs.get_mut(x) { *t = false } } 3 => self.tabs.iter_mut().for_each(|t| *t = false), _ => {} },
            _ => {}
        }
        self.dirty = true;
    }

    fn esc_dispatch(&mut self, inter: &[u8], _ignore: bool, b: u8) {
        if !inter.is_empty() { return; }
        match b {
            b'7' => self.saved = (self.cx, self.cy, self.pen),
            b'8' => { self.cx = self.saved.0; self.cy = self.saved.1; self.pen = self.saved.2; }
            b'D' => self.linefeed(),
            b'E' => { self.cx = 0; self.linefeed(); }
            b'M' => { if self.cy == self.scroll_top { self.scroll_down(self.scroll_top, self.scroll_bot) } else if self.cy > 0 { self.cy -= 1 } }
            b'H' => { let x = self.cx; if let Some(t) = self.tabs.get_mut(x) { *t = true } }
            b'c' => self.reset(),
            _ => {}
        }
        self.dirty = true;
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell: bool) {
        if let (Some(k), Some(v)) = (params.first(), params.get(1)) {
            if *k == b"0" || *k == b"2" { self.title = Some(String::from_utf8_lossy(v).into_owned()); }
            // private: OSC 7700 ; XY  → Windows `color XY` (empty = default)
            if *k == b"7700" {
                let s = String::from_utf8_lossy(v).to_string();
                let h = |c: char| c.to_digit(16).map(|d| d as u8);
                let mut it = s.chars();
                self.console_color = Some(match (it.next().and_then(h), it.next().and_then(h)) { (Some(b), Some(f)) => Some((f, b)), _ => None });
            }
        }
    }

    fn hook(&mut self, _: &vte::Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feed(t: &mut Term, s: &str) { let mut p = vte::Parser::new(); for b in s.bytes() { p.advance(t, b); } }
    fn row(t: &Term, y: usize) -> String { t.line(t.vt + y).unwrap().iter().map(|c| c.ch).collect::<String>().trim_end().to_string() }

    #[test]
    fn text_and_newlines() {
        let mut t = Term::new(20, 5, 300);
        feed(&mut t, "hello\r\nworld");
        assert_eq!(row(&t, 0), "hello");
        assert_eq!(row(&t, 1), "world");
    }

    #[test]
    fn buffer_grows_like_windows() {
        let mut t = Term::new(10, 3, 300);
        for i in 0..5 { feed(&mut t, &format!("l{i}\r\n")); }
        // nothing lost: the viewport moved down inside the 300-line buffer
        assert_eq!(t.line(0).unwrap()[0].ch, 'l');
        assert_eq!(t.vt, 3);
    }

    #[test]
    fn alt_screen_survives_resize() {
        // vim in a 25-row window that is made 14 rows high, then quits
        let mut t = Term::new(80, 25, 300);
        for i in 0..15 { feed(&mut t, &format!("line{i}\r\n")); }
        feed(&mut t, "$ vim");
        feed(&mut t, "\x1b[?1049h\x1b[2J\x1b[24;1Hstatus");
        t.resize(80, 14, 300);
        feed(&mut t, "\x1b[1;14r\x1b[14;1H~");
        feed(&mut t, "\x1b[r\x1b[?1049l\r\n$ ");
        // the main screen comes back with the prompt on screen, its text intact
        assert!(t.vt + t.cy < t.vt + t.rows);
        assert_eq!(row(&t, t.cy), "$");
        assert_eq!(t.line(t.vt + t.cy - 1).unwrap().iter().map(|c| c.ch).collect::<String>().trim_end(), "$ vim");
        for _ in 0..5 { feed(&mut t, "x\r\n"); }
        assert_eq!(t.line(12).unwrap().iter().map(|c| c.ch).collect::<String>().trim_end(), "line12");
        assert_eq!(t.cy, t.rows - 1);
    }

    #[test]
    fn cursor_moves_and_erase() {
        let mut t = Term::new(20, 5, 300);
        feed(&mut t, "abcdef\x1b[1;3H\x1b[K");
        assert_eq!(row(&t, 0), "ab");
        feed(&mut t, "\x1b[2;5HX");
        assert_eq!(row(&t, 1), "    X");
    }

    #[test]
    fn wide_and_combining() {
        let mut t = Term::new(6, 3, 300);
        feed(&mut t, "a日e\u{301}b");
        let l = t.line(0).unwrap();
        assert_eq!((l[0].ch, l[1].ch, l[2].ch, l[3].ch, l[4].ch), ('a', '日', WIDE_TAIL, 'e', 'b'));
        // no room for a wide character at the last column: it wraps whole
        feed(&mut t, "\r\n12345語");
        assert_eq!(row(&t, 1), "12345");
        assert_eq!(t.line(t.vt + 2).unwrap()[0].ch, '語');
        // overwriting the first half clears the second
        feed(&mut t, "\rx");
        assert_eq!(t.line(t.vt + 2).unwrap()[1].ch, ' ');
    }

    #[test]
    fn colon_colours_and_modes() {
        let mut t = Term::new(20, 5, 300);
        feed(&mut t, "\x1b[38:2::10:20:30;48:5:200;4:3mX\x1b[?1002;1006h\x1b[3b");
        let l = t.line(0).unwrap();
        assert_eq!((l[0].fg, l[0].bg, l[0].underline), (Color::Rgb(10, 20, 30), Color::Idx(200), true));
        assert_eq!((t.mouse, t.mouse_sgr), (2, true));
        assert_eq!(row(&t, 0), "XXXX");
    }

    #[test]
    fn colour_change_keeps_pending_wrap() {
        // what less sends for a coloured line that wraps: last column, SGR, space, backspace
        let mut t = Term::new(5, 3, 300);
        feed(&mut t, "abcde\x1b[m \x08fg");
        assert_eq!(row(&t, 0), "abcde");
        assert_eq!(row(&t, 1), "fg");
    }

    #[test]
    fn dsr_reply() {
        let mut t = Term::new(20, 5, 300);
        feed(&mut t, "ab\x1b[6n");
        assert_eq!(t.replies, b"\x1b[1;3R");
    }

    #[test]
    fn colors() {
        let mut t = Term::new(20, 5, 300);
        feed(&mut t, "\x1b[91;44mX\x1b[0mY");
        let l = t.line(0).unwrap();
        assert_eq!(l[0].fg, Color::Idx(9));
        assert_eq!(l[0].bg, Color::Idx(4));
        assert_eq!(l[1].fg, Color::Default);
    }
}
