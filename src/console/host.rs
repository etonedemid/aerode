//! The console window ("conhost"): renders a Term with Windows 7 console
//! fonts/colours/behaviour and connects it to a program through a PTY.
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::rc::Rc;

use super::font::{self, RasterFont};
use super::term::{Color, CursorShape, Term};
use crate::{config, sounds, util};

/// The Windows 7 console colour table (index order is the Windows one).
pub const WIN_PALETTE: [u32; 16] = [
    0x000000, 0x000080, 0x008000, 0x008080, 0x800000, 0x800080, 0x808000, 0xC0C0C0,
    0x808080, 0x0000FF, 0x00FF00, 0x00FFFF, 0xFF0000, 0xFF00FF, 0xFFFF00, 0xFFFFFF,
];

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct ConsoleConfig {
    /// "Raster Fonts" or a TrueType family (Consolas, Lucida Console)
    pub font: String,
    /// raster: "8x12"; TrueType: pixel height
    pub raster_size: String,
    pub ttf_size: u32,
    pub cursor_size: u32, // 25 / 50 / 100
    pub quick_edit: bool,
    pub insert_mode: bool,
    pub buffer_w: usize,
    pub buffer_h: usize,
    pub win_w: usize,
    pub win_h: usize,
    pub palette: [u32; 16],
    pub screen_fg: u8,
    pub screen_bg: u8,
    pub popup_fg: u8,
    pub popup_bg: u8,
    pub history_size: u32,
    pub history_buffers: u32,
    pub discard_duplicates: bool,
    /// "cmd" (the Aerode cmd.exe), "fish", "bash", or any command line
    pub shell: String,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        ConsoleConfig {
            font: "Raster Fonts".into(), raster_size: "8x12".into(), ttf_size: 14, cursor_size: 25, quick_edit: false, insert_mode: true,
            buffer_w: 80, buffer_h: 300, win_w: 80, win_h: 25, palette: WIN_PALETTE, screen_fg: 7, screen_bg: 0, popup_fg: 5, popup_bg: 15,
            history_size: 50, history_buffers: 4, discard_duplicates: false, shell: "cmd".into(),
        }
    }
}

pub fn load_config() -> ConsoleConfig {
    std::fs::read_to_string(config::conf_dir().join("console.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_config(c: &ConsoleConfig) {
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(config::conf_dir().join("console.json"), serde_json::to_string_pretty(c).unwrap());
}

/// Where the user's own Win7 raster fonts were extracted.
pub fn raster_fonts() -> Vec<RasterFont> {
    let dirs = [gtk::glib::user_data_dir().join("fonts/aerode"), std::path::PathBuf::from("/usr/share/fonts/aerode")];
    let mut v = vec![];
    for d in dirs {
        for f in ["vgaoem.fon", "dosapp.fon"] {
            v.extend(font::load_fon(&d.join(f)));
        }
        if !v.is_empty() { break; }
    }
    v.sort_by_key(|f| (f.width * f.height, f.width));
    v.dedup_by_key(|f| (f.width, f.height));
    v
}

fn rgb(c: u32) -> (f64, f64, f64) { (((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0) }

/// ANSI colour index (0-7 = black,red,green,yellow,blue,magenta,cyan,white) → Windows index.
const ANSI_TO_WIN: [u8; 8] = [0, 4, 2, 6, 1, 5, 3, 7];

fn nearest(pal: &[u32; 16], r: u8, g: u8, b: u8) -> u8 {
    (0..16).min_by_key(|i| {
        let c = pal[*i];
        let (dr, dg, db) = (((c >> 16) & 255) as i32 - r as i32, ((c >> 8) & 255) as i32 - g as i32, (c & 255) as i32 - b as i32);
        dr * dr * 3 + dg * dg * 4 + db * db * 2
    }).unwrap() as u8
}

fn xterm256(n: u8) -> (u8, u8, u8) {
    match n {
        16..=231 => { let n = n - 16; let s = |v: u8| if v == 0 { 0 } else { 55 + v * 40 }; (s(n / 36), s((n / 6) % 6), s(n % 6)) }
        _ => { let v = 8 + (n.saturating_sub(232)) * 10; (v, v, v) }
    }
}

enum Sel { None, Marking { ax: usize, ay: usize, bx: usize, by: usize, selecting: bool } }

pub struct Console {
    pub win: gtk::Window,
    area: gtk::DrawingArea,
    scroll: gtk::Scrollbar,
    term: RefCell<Term>,
    parser: RefCell<vte::Parser>,
    writer: RefCell<Option<Box<dyn Write + Send>>>,
    master: RefCell<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    cfg: RefCell<ConsoleConfig>,
    raster: RefCell<Option<RasterFont>>,
    glyphs: RefCell<HashMap<u8, cairo::ImageSurface>>,
    cell: Cell<(f64, f64)>,
    view_top: Cell<usize>,
    follow: Cell<bool>,
    blink_on: Cell<bool>,
    sel: RefCell<Sel>,
    scroll_mode: Cell<bool>,
    base_title: RefCell<String>,
    focused: Cell<bool>,
}

impl Console {
    pub fn new(app: Option<&gtk::Application>, argv: Vec<String>, title: &str) -> Rc<Console> {
        let cfg = load_config();
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        if let Some(a) = app { win.set_application(Some(a)); }
        win.set_icon_name(Some(util::first_icon(&["utilities-terminal"])));
        win.set_title(title);
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let area = gtk::DrawingArea::new();
        area.set_can_focus(true);
        area.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::POINTER_MOTION_MASK | gdk::EventMask::SCROLL_MASK | gdk::EventMask::KEY_PRESS_MASK | gdk::EventMask::FOCUS_CHANGE_MASK);
        let adj = gtk::Adjustment::new(0.0, 0.0, cfg.buffer_h as f64, 1.0, cfg.win_h as f64, cfg.win_h as f64);
        let scroll = gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(&adj));
        hbox.pack_start(&area, true, true, 0);
        hbox.pack_start(&scroll, false, false, 0);
        win.add(&hbox);
        let term = Term::new(cfg.win_w, cfg.win_h, cfg.buffer_h);
        let c = Rc::new(Console {
            win: win.clone(), area: area.clone(), scroll: scroll.clone(), term: RefCell::new(term), parser: RefCell::new(vte::Parser::new()),
            writer: RefCell::new(None), master: RefCell::new(None), cfg: RefCell::new(cfg), raster: RefCell::new(None), glyphs: RefCell::new(HashMap::new()),
            cell: Cell::new((8.0, 12.0)), view_top: Cell::new(0), follow: Cell::new(true), blink_on: Cell::new(true), sel: RefCell::new(Sel::None),
            scroll_mode: Cell::new(false), base_title: RefCell::new(title.to_string()), focused: Cell::new(true),
        });
        c.apply_font();
        c.wire();
        c.spawn(argv);
        hbox.show_all();
        c.fit_window();
        win.show();
        area.grab_focus();
        c
    }

    // ------------------------------------------------------------ fonts
    fn apply_font(&self) {
        let cfg = self.cfg.borrow().clone();
        self.glyphs.borrow_mut().clear();
        if cfg.font == "Raster Fonts" {
            let fonts = raster_fonts();
            let want = cfg.raster_size.clone();
            let f = fonts.iter().find(|f| format!("{}x{}", f.width, f.height) == want).or_else(|| fonts.iter().find(|f| f.width == 8 && f.height == 12)).or(fonts.first()).cloned();
            if let Some(f) = f {
                self.cell.set((f.width as f64, f.height as f64));
                *self.raster.borrow_mut() = Some(f);
                return;
            }
        }
        *self.raster.borrow_mut() = None;
        // TrueType: measure a cell
        let ctx = self.area.pango_context();
        let fam = if cfg.font == "Raster Fonts" { "Lucida Console".to_string() } else { cfg.font.clone() };
        let mut fd = gtk::pango::FontDescription::from_string(&format!("{fam}, Lucida Console, Consolas, monospace"));
        fd.set_absolute_size(cfg.ttf_size as f64 * gtk::pango::SCALE as f64);
        let layout = gtk::pango::Layout::new(&ctx);
        layout.set_font_description(Some(&fd));
        layout.set_text("M");
        let (w, h) = layout.pixel_size();
        self.cell.set((w.max(1) as f64, h.max(1) as f64));
    }

    fn glyph_surface(&self, oem: u8) -> Option<cairo::ImageSurface> {
        if let Some(s) = self.glyphs.borrow().get(&oem) { return Some(s.clone()); }
        let r = self.raster.borrow();
        let f = r.as_ref()?;
        let (w, h) = (f.width as i32, f.height as i32);
        let mut s = cairo::ImageSurface::create(cairo::Format::A8, w, h).ok()?;
        let stride = s.stride() as usize;
        {
            let mut d = s.data().ok()?;
            let g = f.glyph(oem);
            for y in 0..h as usize { for x in 0..w as usize { if g.get(y * w as usize + x).copied().unwrap_or(false) { d[y * stride + x] = 255; } } }
        }
        s.mark_dirty();
        self.glyphs.borrow_mut().insert(oem, s.clone());
        Some(s)
    }

    fn color_of(&self, c: Color, fg: bool, bold: bool) -> u32 {
        let cfg = self.cfg.borrow();
        let pal = &cfg.palette;
        let idx = match c {
            Color::Default => if fg { cfg.screen_fg } else { cfg.screen_bg },
            Color::Idx(n) if n < 8 => ANSI_TO_WIN[n as usize] + if bold && fg { 8 } else { 0 },
            Color::Idx(n) if n < 16 => ANSI_TO_WIN[(n - 8) as usize] + 8,
            Color::Idx(n) => { let (r, g, b) = xterm256(n); nearest(pal, r, g, b) }
            Color::Rgb(r, g, b) => nearest(pal, r, g, b),
        };
        pal[idx as usize & 15]
    }

    // ------------------------------------------------------------ geometry
    fn grid_size(&self) -> (usize, usize) {
        let (cw, ch) = self.cell.get();
        let a = self.area.allocation();
        (((a.width() as f64) / cw).floor().max(1.0) as usize, ((a.height() as f64) / ch).floor().max(1.0) as usize)
    }

    /// Size the window to win_w × win_h cells (and cap the width at the buffer, like Win7).
    fn fit_window(&self) {
        let cfg = self.cfg.borrow().clone();
        let (cw, ch) = self.cell.get();
        let sbw = self.scroll.preferred_width().1;
        let w = (cfg.win_w.min(cfg.buffer_w) as f64 * cw) as i32;
        let h = (cfg.win_h as f64 * ch) as i32;
        self.area.set_size_request(-1, -1);
        let geom = gdk::Geometry::new(cw as i32 * 14 + sbw, ch as i32 * 1, (cfg.buffer_w as f64 * cw) as i32 + sbw, (cfg.buffer_h as f64 * ch) as i32,
            sbw, 0, cw as i32, ch as i32, 0.0, 0.0, gdk::Gravity::NorthWest);
        self.win.set_geometry_hints(None::<&gtk::Widget>, Some(&geom), gdk::WindowHints::MIN_SIZE | gdk::WindowHints::MAX_SIZE | gdk::WindowHints::RESIZE_INC | gdk::WindowHints::BASE_SIZE);
        self.win.resize(w + sbw, h);
    }

    fn on_resize(&self) {
        let (cols, rows) = self.grid_size();
        let cfg = self.cfg.borrow().clone();
        let cols = cols.min(cfg.buffer_w).max(1);
        {
            let mut t = self.term.borrow_mut();
            if t.cols == cfg.buffer_w && t.rows == rows { return; }
            t.resize(cfg.buffer_w, rows, cfg.buffer_h);
        }
        let _ = cols;
        if let Some(m) = self.master.borrow().as_ref() {
            let _ = m.resize(PtySize { rows: rows as u16, cols: cfg.buffer_w as u16, pixel_width: 0, pixel_height: 0 });
        }
        self.update_scrollbar();
    }

    fn update_scrollbar(&self) {
        let t = self.term.borrow();
        let adj = self.scroll.adjustment();
        let total = t.total_lines() as f64;
        adj.set_upper(total);
        adj.set_page_size(t.rows as f64);
        adj.set_page_increment(t.rows as f64);
        if self.follow.get() {
            // keep the cursor line in view (Windows scrolls the window to the cursor)
            let want = (t.vt + t.cy + 1).saturating_sub(t.rows).max(t.vt.min(self.view_top.get().max(t.vt)));
            let want = want.min(t.vt);
            self.view_top.set(want);
            adj.set_value(want as f64);
        }
    }

    // ------------------------------------------------------------ pty
    fn spawn(self: &Rc<Self>, argv: Vec<String>) {
        let cfg = self.cfg.borrow().clone();
        let pty = NativePtySystem::default();
        let Ok(pair) = pty.openpty(PtySize { rows: cfg.win_h as u16, cols: cfg.buffer_w as u16, pixel_width: 0, pixel_height: 0 }) else {
            util::error("Could not create a console.", "No pseudo-terminal available."); return;
        };
        let mut cmd = CommandBuilder::new(&argv[0]);
        for a in &argv[1..] { cmd.arg(a); }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "");
        cmd.env("AERODE_CONSOLE", "1");
        cmd.cwd(config::home());
        let mut child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => { util::error(&format!("'{}' could not be started.", argv[0]), &e.to_string()); return; }
        };
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        *self.writer.borrow_mut() = pair.master.take_writer().ok();
        *self.master.borrow_mut() = Some(pair.master);
        // event-driven: the reader thread wakes the GTK main loop immediately (no polling cap)
        let (tx, rx) = async_channel::unbounded::<Option<Vec<u8>>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => { let _ = tx.send_blocking(None); break; }
                    Ok(n) => { if tx.send_blocking(Some(buf[..n].to_vec())).is_err() { break; } }
                }
            }
        });
        std::thread::spawn(move || { let _ = child.wait(); });
        let me = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(first) = rx.recv().await {
                let Some(c) = me.upgrade() else { break };
                // take everything already queued, then redraw once
                let mut chunks = vec![first];
                while let Ok(more) = rx.try_recv() { chunks.push(more); }
                let mut closed = false;
                {
                    let mut t = c.term.borrow_mut();
                    let mut p = c.parser.borrow_mut();
                    for ch in chunks { match ch { Some(bytes) => for b in bytes { p.advance(&mut *t, b); }, None => closed = true } }
                }
                c.after_output();
                if closed { c.win.close(); break; } // program exited → window closes (like cmd)
            }
        });
    }

    fn after_output(&self) {
        let (replies, bell, title, color) = { let mut t = self.term.borrow_mut(); (std::mem::take(&mut t.replies), std::mem::take(&mut t.bell), t.title.take(), t.console_color.take()) };
        if let Some(col) = color {
            // `color XY` recolours the whole console, like Windows
            let saved = load_config();
            let mut cfg = self.cfg.borrow_mut();
            match col { Some((f, b)) => { cfg.screen_fg = f; cfg.screen_bg = b; } None => { cfg.screen_fg = saved.screen_fg; cfg.screen_bg = saved.screen_bg; } }
        }
        if !replies.is_empty() { self.send(&replies); }
        if bell { sounds::play(sounds::Event::DefaultBeep); }
        if let Some(t) = title { *self.base_title.borrow_mut() = t; self.refresh_title(); }
        self.follow.set(true);
        self.update_scrollbar();
        self.area.queue_draw();
    }

    fn send(&self, bytes: &[u8]) {
        if let Some(w) = self.writer.borrow_mut().as_mut() { let _ = w.write_all(bytes); let _ = w.flush(); }
    }

    fn paste(&self) {
        let cb = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        if let Some(t) = cb.wait_for_text() {
            let t = t.replace("\r\n", "\r").replace('\n', "\r");
            let bp = self.term.borrow().bracketed_paste;
            if bp { self.send(b"\x1b[200~"); }
            self.send(t.as_bytes());
            if bp { self.send(b"\x1b[201~"); }
        }
    }

    // ------------------------------------------------------------ selection / title
    fn refresh_title(&self) {
        let base = self.base_title.borrow().clone();
        let prefix = match &*self.sel.borrow() {
            Sel::Marking { selecting: true, .. } => "Select ",
            Sel::Marking { .. } => "Mark ",
            Sel::None if self.scroll_mode.get() => "Scroll ",
            _ => "",
        };
        self.win.set_title(&format!("{prefix}{base}"));
    }

    fn start_mark(&self) {
        let t = self.term.borrow();
        let (x, y) = (t.cx, self.view_top.get());
        drop(t);
        *self.sel.borrow_mut() = Sel::Marking { ax: x, ay: y, bx: x, by: y, selecting: false };
        self.refresh_title();
        self.area.queue_draw();
    }

    fn copy_selection(&self) -> bool {
        let s = if let Sel::Marking { ax, ay, bx, by, selecting: true } = *self.sel.borrow() { Some((ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))) } else { None };
        let Some((x0, y0, x1, y1)) = s else { return false };
        let text = self.term.borrow().text_block(x0, y0, x1, y1);
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&text);
        self.end_mark();
        true
    }

    fn end_mark(&self) {
        *self.sel.borrow_mut() = Sel::None;
        self.refresh_title();
        self.area.queue_draw();
    }

    fn cell_at(&self, x: f64, y: f64) -> (usize, usize) {
        let (cw, ch) = self.cell.get();
        ((x / cw).floor().max(0.0) as usize, self.view_top.get() + (y / ch).floor().max(0.0) as usize)
    }

    fn find(self: &Rc<Self>) {
        let d = gtk::Dialog::with_buttons(Some("Find"), Some(&self.win), gtk::DialogFlags::empty(), &[("Find Next", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_border_width(10);
        row.pack_start(&gtk::Label::new(Some("Find what:")), false, false, 0);
        let e = gtk::Entry::new();
        e.set_activates_default(true);
        row.pack_start(&e, true, true, 0);
        let case = gtk::CheckButton::with_label("Match case");
        d.content_area().pack_start(&row, false, false, 0);
        d.content_area().pack_start(&case, false, false, 0);
        d.set_default_response(gtk::ResponseType::Ok);
        let me = self.clone();
        let from = Cell::new(self.view_top.get());
        d.connect_response(move |d, r| {
            if r != gtk::ResponseType::Ok { d.close(); return; }
            let q = e.text().to_string();
            let t = me.term.borrow();
            let total = t.total_lines();
            let mc = case.is_active();
            let found = (from.get()..total).chain(0..from.get()).find_map(|y| {
                let line: String = t.line(y)?.iter().map(|c| c.ch).collect();
                let pos = if mc { line.find(&q) } else { line.to_lowercase().find(&q.to_lowercase()) }?;
                Some((pos, y))
            });
            drop(t);
            match found {
                Some((x, y)) => {
                    let n = q.chars().count().max(1);
                    *me.sel.borrow_mut() = Sel::Marking { ax: x, ay: y, bx: x + n - 1, by: y, selecting: true };
                    from.set(y + 1);
                    me.follow.set(false);
                    me.view_top.set(y.saturating_sub(2));
                    me.scroll.adjustment().set_value(me.view_top.get() as f64);
                    me.refresh_title();
                    me.area.queue_draw();
                }
                None => { sounds::play(sounds::Event::DefaultBeep); }
            }
        });
        d.show_all();
    }

    fn edit_menu(self: &Rc<Self>, ev: &gdk::EventButton) {
        let m = gtk::Menu::new();
        let add = |m: &gtk::Menu, l: &str, f: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.connect_activate(move |_| f()); m.append(&it); it };
        { let c = self.clone(); add(&m, "Mar_k", Box::new(move || c.start_mark())); }
        { let c = self.clone(); let it = add(&m, "Cop_y\tEnter", Box::new(move || { c.copy_selection(); })); it.set_sensitive(matches!(*self.sel.borrow(), Sel::Marking { selecting: true, .. })); }
        { let c = self.clone(); add(&m, "_Paste", Box::new(move || c.paste())); }
        { let c = self.clone(); add(&m, "_Select All", Box::new(move || {
            let t = c.term.borrow();
            let last = t.vt + t.rows - 1;
            let cols = t.cols;
            drop(t);
            *c.sel.borrow_mut() = Sel::Marking { ax: 0, ay: 0, bx: cols - 1, by: last, selecting: true };
            c.refresh_title();
            c.area.queue_draw();
        })); }
        { let c = self.clone(); add(&m, "Sc_roll", Box::new(move || { c.scroll_mode.set(true); c.refresh_title(); })); }
        { let c = self.clone(); add(&m, "_Find...", Box::new(move || c.find())); }
        m.append(&gtk::SeparatorMenuItem::new());
        { let c = self.clone(); add(&m, "Propert_ies", Box::new(move || super::props::show(&c))); }
        m.show_all();
        m.popup_at_pointer(Some(ev));
    }

    // ------------------------------------------------------------ input
    fn key(self: &Rc<Self>, e: &gdk::EventKey) -> bool {
        use gdk::keys::constants as K;
        let k = e.keyval();
        let st = e.state();
        let ctrl = st.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = st.contains(gdk::ModifierType::MOD1_MASK);
        let shift = st.contains(gdk::ModifierType::SHIFT_MASK);
        // Mark mode: arrows move the selection, Shift extends, Enter copies, Esc cancels
        let marking = matches!(*self.sel.borrow(), Sel::Marking { .. });
        if marking {
            let mut s = self.sel.borrow_mut();
            if let Sel::Marking { ax, ay, bx, by, selecting } = &mut *s {
                let (cols, total) = { let t = self.term.borrow(); (t.cols, t.total_lines()) };
                let (dx, dy): (i64, i64) = match k { k if k == K::Left => (-1, 0), k if k == K::Right => (1, 0), k if k == K::Up => (0, -1), k if k == K::Down => (0, 1), _ => (0, 0) };
                if dx != 0 || dy != 0 {
                    let nx = (*bx as i64 + dx).clamp(0, cols as i64 - 1) as usize;
                    let ny = (*by as i64 + dy).clamp(0, total as i64 - 1) as usize;
                    if shift { *selecting = true; } else { *ax = nx; *ay = ny; *selecting = false; }
                    *bx = nx; *by = ny;
                    drop(s);
                    self.refresh_title();
                    self.area.queue_draw();
                    return true;
                }
            }
            drop(s);
            if k == K::Return { self.copy_selection(); return true; }
            if k == K::Escape { self.end_mark(); return true; }
            return true;
        }
        if self.scroll_mode.get() {
            let adj = self.scroll.adjustment();
            match k {
                k if k == K::Up => adj.set_value(adj.value() - 1.0),
                k if k == K::Down => adj.set_value(adj.value() + 1.0),
                k if k == K::Page_Up => adj.set_value(adj.value() - adj.page_size()),
                k if k == K::Page_Down => adj.set_value(adj.value() + adj.page_size()),
                _ => { self.scroll_mode.set(false); self.refresh_title(); }
            }
            return true;
        }
        if alt && k == K::Return {
            if self.win.window().map(|w| w.state().contains(gdk::WindowState::FULLSCREEN)).unwrap_or(false) { self.win.unfullscreen() } else { self.win.fullscreen() }
            return true;
        }
        if shift && (k == K::Page_Up || k == K::Page_Down) {
            let adj = self.scroll.adjustment();
            adj.set_value(adj.value() + if k == K::Page_Up { -adj.page_size() } else { adj.page_size() });
            return true;
        }
        if ctrl && shift && (k == K::C || k == K::c) { self.copy_selection(); return true; }
        if (ctrl && shift && (k == K::V || k == K::v)) || (shift && k == K::Insert) { self.paste(); return true; }
        let app = self.term.borrow().app_cursor;
        let mods = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
        let csi = |c: &str| if mods > 1 { format!("\x1b[1;{mods}{c}") } else if app { format!("\x1bO{c}") } else { format!("\x1b[{c}") };
        let tilde = |n: u8| if mods > 1 { format!("\x1b[{n};{mods}~") } else { format!("\x1b[{n}~") };
        let seq: Option<String> = match k {
            k if k == K::Up => Some(csi("A")), k if k == K::Down => Some(csi("B")),
            k if k == K::Right => Some(csi("C")), k if k == K::Left => Some(csi("D")),
            k if k == K::Home => Some(csi("H")), k if k == K::End => Some(csi("F")),
            k if k == K::Insert => Some(tilde(2)), k if k == K::Delete => Some(tilde(3)),
            k if k == K::Page_Up => Some(tilde(5)), k if k == K::Page_Down => Some(tilde(6)),
            k if k == K::F1 => Some(if mods > 1 { format!("\x1b[1;{mods}P") } else { "\x1bOP".into() }),
            k if k == K::F2 => Some(if mods > 1 { format!("\x1b[1;{mods}Q") } else { "\x1bOQ".into() }),
            k if k == K::F3 => Some(if mods > 1 { format!("\x1b[1;{mods}R") } else { "\x1bOR".into() }),
            k if k == K::F4 => Some(if mods > 1 { format!("\x1b[1;{mods}S") } else { "\x1bOS".into() }),
            k if k == K::F5 => Some(tilde(15)), k if k == K::F6 => Some(tilde(17)), k if k == K::F7 => Some(tilde(18)),
            k if k == K::F8 => Some(tilde(19)), k if k == K::F9 => Some(tilde(20)), k if k == K::F10 => Some(tilde(21)),
            k if k == K::F11 => Some(tilde(23)), k if k == K::F12 => Some(tilde(24)),
            k if k == K::Return || k == K::KP_Enter => Some(if alt { "\x1b\r".into() } else { "\r".into() }),
            k if k == K::BackSpace => Some(if ctrl { "\x08".into() } else if alt { "\x1b\x7f".into() } else { "\x7f".into() }),
            k if k == K::Tab => Some("\t".into()),
            k if k == K::ISO_Left_Tab => Some("\x1b[Z".into()),
            k if k == K::Escape => Some("\x1b".into()),
            _ => None,
        };
        if let Some(s) = seq { self.send(s.as_bytes()); self.follow.set(true); return true; }
        if let Some(ch) = k.to_unicode() {
            let mut out = Vec::new();
            if ctrl && (ch.is_ascii_alphabetic() || "@[\\]^_ ".contains(ch)) {
                let b = if ch == ' ' { 0 } else { (ch.to_ascii_uppercase() as u8) & 0x1f };
                if alt { out.push(0x1b); }
                out.push(b);
            } else {
                if alt { out.push(0x1b); }
                let mut tmp = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
            }
            self.send(&out);
            self.follow.set(true);
            return true;
        }
        false
    }

    fn wire(self: &Rc<Self>) {
        {
            let c = self.clone();
            self.area.connect_draw(move |_, cr| { c.draw(cr); glib::Propagation::Stop });
        }
        { let c = self.clone(); self.area.connect_size_allocate(move |_, _| c.on_resize()); }
        { let c = self.clone(); self.area.connect_key_press_event(move |_, e| if c.key(e) { glib::Propagation::Stop } else { glib::Propagation::Proceed }); }
        {
            let c = self.clone();
            self.scroll.adjustment().connect_value_changed(move |a| {
                let t = c.term.borrow();
                let v = a.value().round() as usize;
                c.follow.set(v >= t.vt);
                drop(t);
                c.view_top.set(v);
                c.area.queue_draw();
            });
        }
        {
            let c = self.clone();
            self.area.connect_scroll_event(move |_, e| {
                let adj = c.scroll.adjustment();
                let d = match e.direction() { gdk::ScrollDirection::Up => -3.0, gdk::ScrollDirection::Down => 3.0, _ => e.delta().1 * 3.0 };
                adj.set_value((adj.value() + d).clamp(0.0, adj.upper() - adj.page_size()));
                glib::Propagation::Stop
            });
        }
        {
            let c = self.clone();
            self.area.connect_button_press_event(move |a, e| {
                a.grab_focus();
                let quick = c.cfg.borrow().quick_edit;
                match e.button() {
                    3 => {
                        if quick { if !c.copy_selection() { c.paste(); } } else { c.edit_menu(e); }
                    }
                    1 if quick || matches!(*c.sel.borrow(), Sel::Marking { .. }) => {
                        let (x, y) = c.cell_at(e.position().0, e.position().1);
                        *c.sel.borrow_mut() = Sel::Marking { ax: x, ay: y, bx: x, by: y, selecting: true };
                        c.refresh_title();
                        a.queue_draw();
                    }
                    2 => c.paste(),
                    _ => {}
                }
                glib::Propagation::Stop
            });
        }
        {
            let c = self.clone();
            self.area.connect_motion_notify_event(move |a, e| {
                if e.state().contains(gdk::ModifierType::BUTTON1_MASK) {
                    let (x, y) = c.cell_at(e.position().0, e.position().1);
                    if let Sel::Marking { bx, by, .. } = &mut *c.sel.borrow_mut() { *bx = x; *by = y; }
                    a.queue_draw();
                }
                glib::Propagation::Stop
            });
        }
        { let c = self.clone(); self.area.connect_focus_in_event(move |a, _| { c.focused.set(true); a.queue_draw(); glib::Propagation::Proceed }); }
        { let c = self.clone(); self.area.connect_focus_out_event(move |a, _| { c.focused.set(false); a.queue_draw(); glib::Propagation::Proceed }); }
        // Win7 cursor blink rate (~530 ms)
        {
            let c = Rc::downgrade(self);
            glib::timeout_add_local(std::time::Duration::from_millis(530), move || {
                let Some(c) = c.upgrade() else { return glib::ControlFlow::Break };
                c.blink_on.set(!c.blink_on.get());
                c.area.queue_draw();
                glib::ControlFlow::Continue
            });
        }
        {
            // closing the window ends the program, like closing a Win7 console
            let c = self.clone();
            self.win.connect_destroy(move |_| { c.writer.borrow_mut().take(); c.master.borrow_mut().take(); });
        }
    }

    // ------------------------------------------------------------ drawing
    fn draw(&self, cr: &cairo::Context) {
        let cfg = self.cfg.borrow().clone();
        let (cw, ch) = self.cell.get();
        let t = self.term.borrow();
        let a = self.area.allocation();
        let (r, g, b) = rgb(cfg.palette[cfg.screen_bg as usize & 15]);
        cr.set_source_rgb(r, g, b);
        cr.paint().ok();
        let top = self.view_top.get();
        let rows = (a.height() as f64 / ch).ceil() as usize;
        let sel = match &*self.sel.borrow() { Sel::Marking { ax, ay, bx, by, .. } => Some((*ax.min(bx), *ay.min(by), *ax.max(bx), *ay.max(by))), _ => None };
        let ttf = self.raster.borrow().is_none();
        let layout = if ttf {
            let l = pangocairo::functions::create_layout(cr);
            let mut fd = gtk::pango::FontDescription::from_string(&format!("{}, Lucida Console, Consolas, monospace", cfg.font));
            fd.set_absolute_size(cfg.ttf_size as f64 * gtk::pango::SCALE as f64);
            l.set_font_description(Some(&fd));
            Some(l)
        } else { None };
        for row in 0..rows {
            let y = top + row;
            let Some(line) = t.line(y) else { continue };
            for (x, cell) in line.iter().enumerate() {
                let mut fg = self.color_of(cell.fg, true, cell.bold);
                let mut bg = self.color_of(cell.bg, false, false);
                if cell.reverse { std::mem::swap(&mut fg, &mut bg); }
                // selections are shown inverted, like the Windows console
                if let Some((x0, y0, x1, y1)) = sel { if x >= x0 && x <= x1 && y >= y0 && y <= y1 { fg ^= 0xFFFFFF; bg ^= 0xFFFFFF; } }
                let (px, py) = (x as f64 * cw, row as f64 * ch);
                if bg != cfg.palette[cfg.screen_bg as usize & 15] {
                    let (r, g, b) = rgb(bg);
                    cr.set_source_rgb(r, g, b);
                    cr.rectangle(px, py, cw, ch);
                    cr.fill().ok();
                }
                if cell.ch != ' ' {
                    let (r, g, b) = rgb(fg);
                    cr.set_source_rgb(r, g, b);
                    if let Some(l) = &layout {
                        l.set_text(&cell.ch.to_string());
                        cr.move_to(px, py);
                        pangocairo::functions::show_layout(cr, l);
                    } else if let Some(s) = self.glyph_surface(font::to_cp437(cell.ch)) {
                        cr.mask_surface(&s, px, py).ok();
                    }
                }
                if cell.underline {
                    let (r, g, b) = rgb(fg);
                    cr.set_source_rgb(r, g, b);
                    cr.rectangle(px, py + ch - 1.0, cw, 1.0);
                    cr.fill().ok();
                }
            }
        }
        // cursor: Win7 underscore (25%), larger in overwrite mode or when the app asks
        let abs = t.vt + t.cy;
        if t.cursor_visible && self.blink_on.get() && self.focused.get() && abs >= top && abs < top + rows && sel.is_none() {
            let frac = match t.cursor_shape { Some(CursorShape::Large) => 1.0, Some(CursorShape::Medium) => 0.5, Some(CursorShape::Small) => 0.25, None => cfg.cursor_size as f64 / 100.0 };
            let h = (ch * frac).round().max(2.0);
            let (px, py) = (t.cx as f64 * cw, (abs - top) as f64 * ch + ch - h);
            let (r, g, b) = rgb(self.color_of(Color::Default, true, false));
            cr.set_source_rgb(r, g, b);
            cr.set_operator(cairo::Operator::Difference);
            cr.rectangle(px, py, cw, h);
            cr.fill().ok();
            cr.set_operator(cairo::Operator::Over);
        }
    }

    // ------------------------------------------------------------ properties support
    pub fn config(&self) -> ConsoleConfig { self.cfg.borrow().clone() }

    pub fn set_config(&self, c: ConsoleConfig) {
        *self.cfg.borrow_mut() = c.clone();
        save_config(&c);
        self.apply_font();
        let rows = self.term.borrow().rows;
        self.term.borrow_mut().resize(c.buffer_w, rows, c.buffer_h);
        self.fit_window();
        self.update_scrollbar();
        self.area.queue_draw();
    }
}
