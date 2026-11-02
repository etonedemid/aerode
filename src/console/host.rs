//! The console window ("conhost"): renders a Term with Windows 7 console
//! fonts/colours/behaviour and connects it to a program through a PTY.
use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
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

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct ConsoleConfig {
    /// "Raster Fonts" or a TrueType family (Consolas, Lucida Console)
    pub font: String,
    /// raster: "8x12"; TrueType: pixel height
    pub raster_size: String,
    pub ttf_size: u32,
    /// "Bold fonts" (TrueType only)
    pub bold: bool,
    pub cursor_size: u32, // 25 / 50 / 100
    /// Display Options: Full Screen instead of Window
    pub full_screen: bool,
    pub quick_edit: bool,
    pub insert_mode: bool,
    pub buffer_w: usize,
    pub buffer_h: usize,
    pub win_w: usize,
    pub win_h: usize,
    /// Window Position (screen pixels) unless the system positions the window
    pub system_pos: bool,
    pub win_x: i32,
    pub win_y: i32,
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
            font: "Raster Fonts".into(), raster_size: "8x12".into(), ttf_size: 14, bold: false, cursor_size: 25, full_screen: false,
            quick_edit: false, insert_mode: true, buffer_w: 80, buffer_h: 300, win_w: 80, win_h: 25, system_pos: true, win_x: 0, win_y: 0,
            palette: WIN_PALETTE, screen_fg: 7, screen_bg: 0, popup_fg: 5, popup_bg: 15,
            history_size: 50, history_buffers: 4, discard_duplicates: false, shell: "cmd".into(),
        }
    }
}

/// The defaults ("Defaults" in the window menu): ~/.config/aerode/console.json.
pub fn load_config() -> ConsoleConfig {
    std::fs::read_to_string(config::conf_dir().join("console.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_config(c: &ConsoleConfig) {
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(config::conf_dir().join("console.json"), serde_json::to_string_pretty(c).unwrap());
}

fn windows_file() -> std::path::PathBuf { config::conf_dir().join("console-windows.json") }

fn load_windows() -> BTreeMap<String, ConsoleConfig> {
    std::fs::read_to_string(windows_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Settings of the console windows with this title. Like Win7, "Properties" are remembered per window
/// title (HKCU\Console\<title>) and fall back to the defaults.
pub fn load_for(key: &str) -> ConsoleConfig {
    let mut c = load_windows().remove(key).unwrap_or_else(load_config);
    // the shell is a default for new windows only
    c.shell = load_config().shell;
    c
}

pub fn save_for(key: &str, c: &ConsoleConfig) {
    let mut all = load_windows();
    all.insert(key.to_string(), c.clone());
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(windows_file(), serde_json::to_string_pretty(&all).unwrap());
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
    // Win7 lists the sizes by height, then width (4 x 6, 6 x 8, 8 x 8, 16 x 8, 5 x 12 ...)
    v.sort_by_key(|f| (f.height, f.width));
    v.dedup_by_key(|f| (f.width, f.height));
    v
}

/// The raster font for a "WxH" size (8x12 if missing).
pub fn raster_font(size: &str) -> Option<RasterFont> {
    let fonts = raster_fonts();
    fonts.iter().find(|f| format!("{}x{}", f.width, f.height) == size).or_else(|| fonts.iter().find(|f| f.width == 8 && f.height == 12)).or(fonts.first()).cloned()
}

/// A8 mask of one raster glyph.
pub fn glyph_mask(f: &RasterFont, oem: u8) -> Option<cairo::ImageSurface> {
    let (w, h) = (f.width as i32, f.height as i32);
    let mut s = cairo::ImageSurface::create(cairo::Format::A8, w, h).ok()?;
    let stride = s.stride() as usize;
    {
        let mut d = s.data().ok()?;
        let g = f.glyph(oem);
        for y in 0..h as usize { for x in 0..w as usize { if g.get(y * w as usize + x).copied().unwrap_or(false) { d[y * stride + x] = 255; } } }
    }
    s.mark_dirty();
    Some(s)
}

/// Text in a raster font at (x, y) (top-left), in the current source colour.
pub fn draw_raster_text(cr: &cairo::Context, f: &RasterFont, x: f64, y: f64, text: &str) {
    for (i, c) in text.chars().enumerate() {
        if c == ' ' { continue; }
        if let Some(m) = glyph_mask(f, font::to_cp437(c)) { cr.mask_surface(&m, x + i as f64 * f.width as f64, y).ok(); }
    }
}

pub fn ttf_description(c: &ConsoleConfig) -> gtk::pango::FontDescription {
    let fam = if c.font == "Raster Fonts" { "Lucida Console".to_string() } else { c.font.clone() };
    let mut fd = gtk::pango::FontDescription::from_string(&format!("{fam}, Lucida Console, Consolas, monospace"));
    fd.set_absolute_size(c.ttf_size as f64 * gtk::pango::SCALE as f64);
    if c.bold { fd.set_weight(gtk::pango::Weight::Bold); }
    fd
}

/// Character cell size of a configuration's font.
pub fn cell_size(c: &ConsoleConfig, ctx: &gtk::pango::Context) -> (f64, f64) {
    if c.font == "Raster Fonts" {
        if let Some(f) = raster_font(&c.raster_size) { return (f.width as f64, f.height as f64); }
    }
    let layout = gtk::pango::Layout::new(ctx);
    layout.set_font_description(Some(&ttf_description(c)));
    layout.set_text("M");
    let (w, h) = layout.pixel_size();
    (w.max(1) as f64, h.max(1) as f64)
}

pub fn rgb(c: u32) -> (f64, f64, f64) { (((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0) }

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

/// Mark/Select state. `a` is the anchor, `b` the cursor (buffer coordinates); while not `selecting`
/// the console is in Mark mode and only shows the blinking mark cursor at `b`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sel { None, Marking { ax: usize, ay: usize, bx: usize, by: usize, selecting: bool } }

pub struct Console {
    pub win: gtk::Window,
    area: gtk::DrawingArea,
    scroll: gtk::Scrollbar,
    hscroll: gtk::Scrollbar,
    term: RefCell<Term>,
    parser: RefCell<vte::Parser>,
    writer: RefCell<Option<Box<dyn Write + Send>>>,
    master: RefCell<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    cfg: RefCell<ConsoleConfig>,
    /// the window's key for remembered Properties (its initial title, like Win7)
    key: String,
    /// `color XY` from the program overrides the configured screen colours until `color`
    color_override: Cell<Option<(u8, u8)>>,
    raster: RefCell<Option<RasterFont>>,
    glyphs: RefCell<HashMap<u8, cairo::ImageSurface>>,
    cell: Cell<(f64, f64)>,
    view_top: Cell<usize>,
    view_left: Cell<usize>,
    follow: Cell<bool>,
    blink_on: Cell<bool>,
    sel: Cell<Sel>,
    scroll_mode: Cell<bool>,
    base_title: RefCell<String>,
    focused: Cell<bool>,
    find_text: RefCell<(String, bool, bool)>, // (text, match case, up)
}

/// The window menu's console items (see sysmenu.rs): Edit ▸, Defaults, Properties, as in Win7.
const SYSMENU_ITEMS: &[(&str, &str, &str)] = &[
    ("mark", "_Edit/Mar_k", ""), ("copy", "_Edit/Cop_y", "Enter"), ("paste", "_Edit/_Paste", ""), ("selectall", "_Edit/_Select All", ""),
    ("scroll", "_Edit/Scro_ll", ""), ("find", "_Edit/_Find...", ""), ("defaults", "Def_aults", ""), ("properties", "_Properties", ""),
];

impl Console {
    pub fn new(app: Option<&gtk::Application>, argv: Vec<String>, title: &str) -> Rc<Console> {
        let cfg = load_for(title);
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        if let Some(a) = app { win.set_application(Some(a)); }
        win.set_icon_name(Some(util::first_icon(&["utilities-terminal"])));
        win.set_title(title);
        win.add_events(gdk::EventMask::PROPERTY_CHANGE_MASK);
        let grid = gtk::Grid::new();
        let area = gtk::DrawingArea::new();
        area.set_can_focus(true);
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::POINTER_MOTION_MASK | gdk::EventMask::SCROLL_MASK | gdk::EventMask::KEY_PRESS_MASK | gdk::EventMask::FOCUS_CHANGE_MASK);
        let adj = gtk::Adjustment::new(0.0, 0.0, cfg.buffer_h as f64, 1.0, cfg.win_h as f64, cfg.win_h as f64);
        let scroll = gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(&adj));
        let hadj = gtk::Adjustment::new(0.0, 0.0, cfg.buffer_w as f64, 1.0, cfg.win_w as f64, cfg.win_w as f64);
        let hscroll = gtk::Scrollbar::new(gtk::Orientation::Horizontal, Some(&hadj));
        hscroll.set_no_show_all(true);
        grid.attach(&area, 0, 0, 1, 1);
        grid.attach(&scroll, 1, 0, 1, 1);
        grid.attach(&hscroll, 0, 1, 1, 1);
        win.add(&grid);
        let term = Term::new(cfg.buffer_w, cfg.win_h, cfg.buffer_h);
        let c = Rc::new(Console {
            win: win.clone(), area: area.clone(), scroll: scroll.clone(), hscroll: hscroll.clone(), term: RefCell::new(term), parser: RefCell::new(vte::Parser::new()),
            writer: RefCell::new(None), master: RefCell::new(None), cfg: RefCell::new(cfg.clone()), key: title.to_string(), color_override: Cell::new(None),
            raster: RefCell::new(None), glyphs: RefCell::new(HashMap::new()),
            cell: Cell::new((8.0, 12.0)), view_top: Cell::new(0), view_left: Cell::new(0), follow: Cell::new(true), blink_on: Cell::new(true), sel: Cell::new(Sel::None),
            scroll_mode: Cell::new(false), base_title: RefCell::new(title.to_string()), focused: Cell::new(true),
            find_text: RefCell::new((String::new(), false, true)),
        });
        c.apply_font();
        c.wire();
        c.spawn(argv);
        grid.show_all();
        c.fit_window();
        if !cfg.system_pos { win.move_(cfg.win_x, cfg.win_y); }
        win.show();
        if cfg.full_screen { win.fullscreen(); }
        c.publish_menu();
        area.grab_focus();
        c
    }

    // ------------------------------------------------------------ fonts
    fn apply_font(&self) {
        let cfg = self.cfg.borrow().clone();
        self.glyphs.borrow_mut().clear();
        *self.raster.borrow_mut() = if cfg.font == "Raster Fonts" { raster_font(&cfg.raster_size) } else { None };
        self.cell.set(cell_size(&cfg, &self.area.pango_context()));
    }

    fn glyph_surface(&self, oem: u8) -> Option<cairo::ImageSurface> {
        if let Some(s) = self.glyphs.borrow().get(&oem) { return Some(s.clone()); }
        let s = glyph_mask(self.raster.borrow().as_ref()?, oem)?;
        self.glyphs.borrow_mut().insert(oem, s.clone());
        Some(s)
    }

    fn screen_colors(&self) -> (u8, u8) {
        self.color_override.get().unwrap_or_else(|| { let c = self.cfg.borrow(); (c.screen_fg, c.screen_bg) })
    }

    fn color_of(&self, c: Color, fg: bool, bold: bool) -> u32 {
        let (sfg, sbg) = self.screen_colors();
        let cfg = self.cfg.borrow();
        let pal = &cfg.palette;
        let idx = match c {
            Color::Default => if fg { sfg } else { sbg },
            Color::Idx(n) if n < 8 => ANSI_TO_WIN[n as usize] + if bold && fg { 8 } else { 0 },
            Color::Idx(n) if n < 16 => ANSI_TO_WIN[(n - 8) as usize] + 8,
            Color::Idx(n) => { let (r, g, b) = xterm256(n); nearest(pal, r, g, b) }
            Color::Rgb(r, g, b) => nearest(pal, r, g, b),
        };
        pal[idx as usize & 15]
    }

    // ------------------------------------------------------------ geometry
    /// Visible (columns, rows) of the text area.
    fn grid_size(&self) -> (usize, usize) {
        let (cw, ch) = self.cell.get();
        let a = self.area.allocation();
        (((a.width() as f64) / cw).floor().max(1.0) as usize, ((a.height() as f64) / ch).floor().max(1.0) as usize)
    }

    /// Size the window to win_w × win_h cells; it can never be larger than the buffer (like Win7).
    fn fit_window(&self) {
        let cfg = self.cfg.borrow().clone();
        let (cw, ch) = self.cell.get();
        let sbw = self.scroll.preferred_width().1;
        let need_h = cfg.win_w < cfg.buffer_w;
        self.hscroll.set_visible(need_h);
        let sbh = if need_h { self.hscroll.preferred_height().1 } else { 0 };
        let w = (cfg.win_w.min(cfg.buffer_w) as f64 * cw) as i32;
        let h = (cfg.win_h.min(cfg.buffer_h) as f64 * ch) as i32;
        let geom = gdk::Geometry::new(cw as i32 * 14 + sbw, ch as i32 + sbh, (cfg.buffer_w as f64 * cw) as i32 + sbw, (cfg.buffer_h as f64 * ch) as i32 + sbh,
            sbw, sbh, cw as i32, ch as i32, 0.0, 0.0, gdk::Gravity::NorthWest);
        self.win.set_geometry_hints(None::<&gtk::Widget>, Some(&geom), gdk::WindowHints::MIN_SIZE | gdk::WindowHints::MAX_SIZE | gdk::WindowHints::RESIZE_INC | gdk::WindowHints::BASE_SIZE);
        self.win.resize(w + sbw, h + sbh);
    }

    fn on_resize(&self) {
        let (cols, rows) = self.grid_size();
        let cfg = self.cfg.borrow().clone();
        let cols = cols.min(cfg.buffer_w).max(1);
        let rows = rows.min(cfg.buffer_h).max(1);
        // the window size the Properties dialog shows is the current one
        { let mut c = self.cfg.borrow_mut(); c.win_w = cols; c.win_h = rows; }
        self.hscroll.set_visible(cols < cfg.buffer_w);
        let hadj = self.hscroll.adjustment();
        hadj.set_upper(cfg.buffer_w as f64);
        hadj.set_page_size(cols as f64);
        hadj.set_page_increment(cols as f64);
        if self.view_left.get() + cols > cfg.buffer_w { self.view_left.set(cfg.buffer_w - cols); hadj.set_value(self.view_left.get() as f64); }
        {
            let mut t = self.term.borrow_mut();
            if t.cols == cfg.buffer_w && t.rows == rows { drop(t); self.update_scrollbar(); return; }
            t.resize(cfg.buffer_w, rows, cfg.buffer_h);
        }
        if let Some(m) = self.master.borrow().as_ref() {
            let _ = m.resize(PtySize { rows: rows as u16, cols: cfg.buffer_w as u16, pixel_width: 0, pixel_height: 0 });
        }
        self.update_scrollbar();
    }

    fn update_scrollbar(&self) {
        let t = self.term.borrow();
        let adj = self.scroll.adjustment();
        adj.set_upper(t.total_lines() as f64);
        adj.set_page_size(t.rows as f64);
        adj.set_page_increment(t.rows as f64);
        if self.follow.get() {
            // keep the cursor line in view (Windows scrolls the window to the cursor)
            let want = (t.vt + t.cy + 1).saturating_sub(t.rows).max(t.vt.min(self.view_top.get().max(t.vt))).min(t.vt);
            self.view_top.set(want);
            adj.set_value(want as f64);
            // and the cursor column
            let (cols, _) = self.grid_size();
            let (l, cx) = (self.view_left.get(), t.cx);
            if cx < l || cx >= l + cols {
                let nl = cx.saturating_sub(cols.saturating_sub(1)).min(t.cols.saturating_sub(cols));
                self.view_left.set(nl);
                self.hscroll.adjustment().set_value(nl as f64);
            }
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
        cmd.env("AERODE_CONSOLE_KEY", &self.key);
        cmd.cwd(config::home());
        let mut child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => { util::error(&format!("'{}' could not be started.", argv[0]), &e.to_string()); return; }
        };
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        *self.writer.borrow_mut() = pair.master.take_writer().ok();
        *self.master.borrow_mut() = Some(pair.master);
        // Event-driven: the reader thread wakes the GTK main loop at once (no polling cap). The channel is
        // bounded so that, while the console is paused (Mark/Select/Scroll), the program blocks on its
        // output like on Windows instead of us buffering without limit.
        let (tx, rx) = async_channel::bounded::<Option<Vec<u8>>>(8);
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
            loop {
                // paused: don't take more output (the reader thread then blocks, and with it the program)
                loop {
                    let Some(c) = me.upgrade() else { return };
                    if !c.paused() { break; }
                    drop(c);
                    glib::timeout_future(std::time::Duration::from_millis(40)).await;
                }
                let Ok(first) = rx.recv().await else { break };
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

    /// Win7 stops the program's output while text is being marked, selected or the window scrolled.
    fn paused(&self) -> bool { self.sel.get() != Sel::None || self.scroll_mode.get() }

    fn after_output(&self) {
        let (replies, bell, title, color) = { let mut t = self.term.borrow_mut(); (std::mem::take(&mut t.replies), std::mem::take(&mut t.bell), t.title.take(), t.console_color.take()) };
        // `color XY` recolours the whole console, like Windows; `color` alone restores the configured colours
        if let Some(col) = color { self.color_override.set(col); }
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

    /// Edit ▸ Paste: the clipboard text is typed into the program.
    pub fn paste(&self) {
        let cb = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        if let Some(t) = cb.wait_for_text() {
            let t = t.replace("\r\n", "\r").replace('\n', "\r");
            let bp = self.term.borrow().bracketed_paste;
            if bp { self.send(b"\x1b[200~"); }
            self.send(t.as_bytes());
            if bp { self.send(b"\x1b[201~"); }
            self.follow.set(true);
        }
    }

    // ------------------------------------------------------------ selection / title
    fn refresh_title(&self) {
        let base = self.base_title.borrow().clone();
        let prefix = match self.sel.get() {
            Sel::Marking { selecting: true, .. } => "Select ",
            Sel::Marking { .. } => "Mark ",
            Sel::None if self.scroll_mode.get() => "Scroll ",
            _ => "",
        };
        self.win.set_title(&format!("{prefix}{base}"));
        self.publish_menu();
    }

    /// The window menu items, with Copy greyed unless something is selected.
    fn publish_menu(&self) {
        let Some(w) = self.win.window() else { return };
        let selecting = matches!(self.sel.get(), Sel::Marking { selecting: true, .. });
        let text: String = SYSMENU_ITEMS.iter().map(|(id, l, s)| format!("{}{id}\t{l}\t{s}\n", if *id == "copy" && !selecting { "!" } else { "" })).collect();
        gdk::property_change(&w, &gdk::Atom::intern("_AERODE_SYSMENU_ITEMS"), &gdk::Atom::intern("UTF8_STRING"), 8, gdk::PropMode::Replace, gdk::ChangeData::UChars(text.as_bytes()));
    }

    fn set_sel(&self, s: Sel) {
        let was = self.paused();
        self.sel.set(s);
        if was && !self.paused() { self.follow.set(true); self.update_scrollbar(); }
        self.refresh_title();
        self.area.queue_draw();
    }

    /// Edit ▸ Mark: the mark cursor starts in the top left corner of the window.
    pub fn start_mark(&self) {
        self.scroll_mode.set(false);
        self.follow.set(false);
        let (x, y) = (self.view_left.get(), self.view_top.get());
        self.blink_on.set(true);
        self.set_sel(Sel::Marking { ax: x, ay: y, bx: x, by: y, selecting: false });
    }

    /// Edit ▸ Select All: the whole screen buffer.
    pub fn select_all(&self) {
        self.scroll_mode.set(false);
        self.follow.set(false);
        let (cols, total) = { let t = self.term.borrow(); (t.cols, t.total_lines()) };
        self.set_sel(Sel::Marking { ax: 0, ay: 0, bx: cols - 1, by: total - 1, selecting: true });
    }

    /// Edit ▸ Scroll: arrow keys and Page Up/Down scroll the window until Esc or Enter.
    pub fn start_scroll(&self) {
        self.set_sel(Sel::None);
        self.follow.set(false);
        self.scroll_mode.set(true);
        self.refresh_title();
    }

    fn selection_rect(&self) -> Option<(usize, usize, usize, usize)> {
        match self.sel.get() { Sel::Marking { ax, ay, bx, by, selecting: true } => Some((ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))), _ => None }
    }

    /// Edit ▸ Copy (Enter): the selected block goes to the clipboard and the selection ends.
    pub fn copy_selection(&self) -> bool {
        let Some((x0, y0, x1, y1)) = self.selection_rect() else { return false };
        let text = self.term.borrow().text_block(x0, y0, x1, y1);
        let text = text.trim_end_matches('\n').to_string();
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_text(&text);
        self.end_mark();
        true
    }

    fn end_mark(&self) { self.set_sel(Sel::None); }

    fn cell_at(&self, x: f64, y: f64) -> (usize, usize) {
        let (cw, ch) = self.cell.get();
        let (cols, total) = { let t = self.term.borrow(); (t.cols, t.total_lines()) };
        let cx = (self.view_left.get() + (x / cw).floor().max(0.0) as usize).min(cols - 1);
        let cy = (self.view_top.get() + (y / ch).floor().max(0.0) as usize).min(total.saturating_sub(1));
        (cx, cy)
    }

    /// Scroll so buffer cell (x, y) is visible.
    fn reveal(&self, x: usize, y: usize) {
        let (cols, rows) = self.grid_size();
        let (top, left) = (self.view_top.get(), self.view_left.get());
        if y < top { self.scroll.adjustment().set_value(y as f64); } else if y >= top + rows { self.scroll.adjustment().set_value((y + 1 - rows) as f64); }
        if x < left { self.hscroll.adjustment().set_value(x as f64); } else if x >= left + cols { self.hscroll.adjustment().set_value((x + 1 - cols) as f64); }
    }

    fn line_string(&self, y: usize) -> String {
        self.term.borrow().line(y).map(|l| l.iter().map(|c| c.ch).collect()).unwrap_or_default()
    }

    /// Edit ▸ Find...: the Win7 console Find box (Match case, Direction Up/Down). A hit is selected.
    pub fn find(self: &Rc<Self>) {
        let d = gtk::Dialog::new();
        d.set_title("Find");
        d.set_transient_for(Some(&self.win));
        d.set_resizable(false);
        d.set_type_hint(gdk::WindowTypeHint::Dialog);
        let g = gtk::Grid::new();
        g.set_border_width(10);
        g.set_row_spacing(8);
        g.set_column_spacing(8);
        let lab = gtk::Label::with_mnemonic("Fi_nd what:");
        lab.set_xalign(0.0);
        let e = gtk::Entry::new();
        e.set_width_chars(28);
        e.set_activates_default(true);
        lab.set_mnemonic_widget(Some(&e));
        let (prev, prev_case, prev_up) = self.find_text.borrow().clone();
        e.set_text(&prev);
        let case = gtk::CheckButton::with_mnemonic("Match _case");
        case.set_active(prev_case);
        let dirf = gtk::Frame::new(Some("Direction"));
        let dirb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        dirb.set_border_width(4);
        let up = gtk::RadioButton::with_mnemonic("_Up");
        let down = gtk::RadioButton::with_mnemonic_from_widget(&up, "_Down");
        if prev_up { up.set_active(true) } else { down.set_active(true) }
        dirb.pack_start(&up, false, false, 0);
        dirb.pack_start(&down, false, false, 0);
        dirf.add(&dirb);
        let btns = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let next = gtk::Button::with_mnemonic("_Find Next");
        next.set_can_default(true);
        let cancel = gtk::Button::with_label("Cancel");
        btns.pack_start(&next, false, false, 0);
        btns.pack_start(&cancel, false, false, 0);
        g.attach(&lab, 0, 0, 1, 1);
        g.attach(&e, 1, 0, 2, 1);
        g.attach(&btns, 3, 0, 1, 2);
        g.attach(&case, 0, 1, 2, 1);
        g.attach(&dirf, 2, 1, 1, 1);
        d.content_area().add(&g);
        d.set_default(Some(&next));
        { let e2 = e.clone(); let n = next.clone(); n.set_sensitive(!e.text().is_empty()); e2.connect_changed(move |e| n.set_sensitive(!e.text().is_empty())); }
        { let d = d.clone(); cancel.connect_clicked(move |_| d.close()); }
        {
            let me = self.clone();
            let (d2, e, case, up) = (d.clone(), e.clone(), case.clone(), up.clone());
            next.connect_clicked(move |_| {
                let q = e.text().to_string();
                if q.is_empty() { return; }
                *me.find_text.borrow_mut() = (q.clone(), case.is_active(), up.is_active());
                if !me.find_next(&q, case.is_active(), up.is_active()) {
                    let m = gtk::MessageDialog::new(Some(&d2), gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::Ok, &format!("Cannot find \"{q}\""));
                    m.set_title("Find");
                    m.run();
                    m.close();
                }
            });
        }
        d.show_all();
    }

    /// Search from the selection (or the cursor) in the given direction, wrapping around the buffer.
    fn find_next(&self, q: &str, match_case: bool, up: bool) -> bool {
        let norm = |s: &str| -> Vec<char> { if match_case { s.chars().collect() } else { s.to_lowercase().chars().collect() } };
        let qc = norm(q);
        if qc.is_empty() { return false; }
        let total = self.term.borrow().total_lines();
        let mut hits: Vec<(usize, usize)> = vec![];
        for y in 0..total {
            let lc = norm(&self.line_string(y));
            if lc.len() < qc.len() { continue; }
            for x in 0..=lc.len() - qc.len() { if lc[x..x + qc.len()] == qc[..] { hits.push((y, x)); } }
        }
        let start = match self.sel.get() {
            Sel::Marking { ax, ay, bx, by, .. } => (ay.min(by), ax.min(bx)),
            Sel::None => { let t = self.term.borrow(); (t.vt + t.cy, t.cx) }
        };
        let hit = if up { hits.iter().rev().find(|h| **h < start).or(hits.last()) } else { hits.iter().find(|h| **h > start).or(hits.first()) };
        let Some(&(y, x)) = hit else { return false };
        self.follow.set(false);
        self.scroll_mode.set(false);
        self.set_sel(Sel::Marking { ax: x, ay: y, bx: x + qc.len() - 1, by: y, selecting: true });
        self.reveal(x, y);
        true
    }

    /// Mouse/keyboard selection of the word around (x, y) (QuickEdit double click).
    fn select_word(&self, x: usize, y: usize) {
        let line: Vec<char> = self.line_string(y).chars().collect();
        if line.get(x).map(|c| *c == ' ').unwrap_or(true) { self.set_sel(Sel::Marking { ax: x, ay: y, bx: x, by: y, selecting: true }); return; }
        let mut a = x;
        while a > 0 && line[a - 1] != ' ' { a -= 1; }
        let mut b = x;
        while b + 1 < line.len() && line[b + 1] != ' ' { b += 1; }
        self.set_sel(Sel::Marking { ax: a, ay: y, bx: b, by: y, selecting: true });
    }

    /// The right-click menu without QuickEdit: the Edit items, as in Win7.
    fn edit_menu(self: &Rc<Self>, ev: &gdk::EventButton) {
        let m = gtk::Menu::new();
        let add = |m: &gtk::Menu, l: &str, acc: &str, f: Box<dyn Fn()>| {
            let it = gtk::MenuItem::new();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
            let lab = gtk::Label::with_mnemonic(l);
            lab.set_xalign(0.0);
            lab.set_mnemonic_widget(Some(&it));
            row.pack_start(&lab, true, true, 0);
            if !acc.is_empty() { let a = gtk::Label::new(Some(acc)); a.style_context().add_class("accelerator"); row.pack_end(&a, false, false, 0); }
            it.add(&row);
            it.connect_activate(move |_| f());
            m.append(&it);
            it
        };
        for (id, label, acc) in SYSMENU_ITEMS.iter().filter(|(l, _, _)| !["defaults", "properties"].contains(l)) {
            let c = self.clone();
            let id = id.to_string();
            let is_copy = id == "copy";
            let it = add(&m, label.trim_start_matches("_Edit/"), acc, Box::new(move || c.command(&id)));
            if is_copy { it.set_sensitive(self.selection_rect().is_some()); }
        }
        m.show_all();
        m.popup_at_pointer(Some(ev));
    }

    /// A window/context menu command.
    pub fn command(self: &Rc<Self>, id: &str) {
        match id {
            "mark" => self.start_mark(),
            "copy" => { self.copy_selection(); }
            "paste" => self.paste(),
            "selectall" => self.select_all(),
            "scroll" => self.start_scroll(),
            "find" => self.find(),
            "defaults" => super::props::show(self, true),
            "properties" => super::props::show(self, false),
            _ => {}
        }
    }

    // ------------------------------------------------------------ input
    /// Keys in Mark/Select mode: arrows (+Shift to select), Home/End, Page Up/Down, Ctrl+Home/End,
    /// Enter copies, Esc cancels. Everything else is swallowed while marking.
    fn mark_key(&self, k: gdk::keys::Key, shift: bool, ctrl: bool) -> bool {
        use gdk::keys::constants as K;
        let Sel::Marking { mut ax, mut ay, bx, by, mut selecting } = self.sel.get() else { return false };
        if k == K::Return || k == K::KP_Enter { self.copy_selection(); return true; }
        if k == K::Escape { self.end_mark(); return true; }
        let (cols, total) = { let t = self.term.borrow(); (t.cols as i64, t.total_lines() as i64) };
        let page = self.grid_size().1 as i64;
        let (x, y) = (bx as i64, by as i64);
        let (nx, ny) = match k {
            k if k == K::Left => (x - 1, y), k if k == K::Right => (x + 1, y),
            k if k == K::Up => (x, y - 1), k if k == K::Down => (x, y + 1),
            k if k == K::Home && ctrl => (0, 0), k if k == K::End && ctrl => (x, total - 1),
            k if k == K::Home => (0, y), k if k == K::End => (cols - 1, y),
            k if k == K::Page_Up => (x, y - page), k if k == K::Page_Down => (x, y + page),
            _ => return true,
        };
        let (nx, ny) = (nx.clamp(0, cols - 1) as usize, ny.clamp(0, total - 1) as usize);
        if shift { if !selecting { selecting = true; } } else { ax = nx; ay = ny; selecting = false; }
        self.blink_on.set(true);
        self.set_sel(Sel::Marking { ax, ay, bx: nx, by: ny, selecting });
        self.reveal(nx, ny);
        true
    }

    fn key(self: &Rc<Self>, e: &gdk::EventKey) -> bool {
        use gdk::keys::constants as K;
        let k = e.keyval();
        let st = e.state();
        let ctrl = st.contains(gdk::ModifierType::CONTROL_MASK);
        let alt = st.contains(gdk::ModifierType::MOD1_MASK);
        let shift = st.contains(gdk::ModifierType::SHIFT_MASK);
        if [K::Shift_L, K::Shift_R, K::Control_L, K::Control_R, K::Alt_L, K::Alt_R, K::Super_L, K::Super_R].contains(&k) { return false; }
        if self.sel.get() != Sel::None { return self.mark_key(k, shift, ctrl); }
        if self.scroll_mode.get() {
            let adj = self.scroll.adjustment();
            let max = adj.upper() - adj.page_size();
            match k {
                k if k == K::Up => adj.set_value((adj.value() - 1.0).max(0.0)),
                k if k == K::Down => adj.set_value((adj.value() + 1.0).min(max)),
                k if k == K::Page_Up => adj.set_value((adj.value() - adj.page_size()).max(0.0)),
                k if k == K::Page_Down => adj.set_value((adj.value() + adj.page_size()).min(max)),
                k if k == K::Home && ctrl => adj.set_value(0.0),
                k if k == K::End && ctrl => adj.set_value(max),
                k if k == K::Left => { let h = self.hscroll.adjustment(); h.set_value(h.value() - 1.0); }
                k if k == K::Right => { let h = self.hscroll.adjustment(); h.set_value(h.value() + 1.0); }
                _ => { self.scroll_mode.set(false); self.follow.set(true); self.update_scrollbar(); self.refresh_title(); }
            }
            return true;
        }
        if alt && (k == K::Return || k == K::KP_Enter) {
            if self.win.window().map(|w| w.state().contains(gdk::WindowState::FULLSCREEN)).unwrap_or(false) { self.win.unfullscreen() } else { self.win.fullscreen() }
            return true;
        }
        if shift && (k == K::Page_Up || k == K::Page_Down) && !self.term.borrow().on_alt_screen() {
            let adj = self.scroll.adjustment();
            adj.set_value((adj.value() + if k == K::Page_Up { -adj.page_size() } else { adj.page_size() }).clamp(0.0, adj.upper() - adj.page_size()));
            return true;
        }
        if (ctrl && shift && (k == K::V || k == K::v)) || (shift && k == K::Insert) { self.paste(); return true; }
        let app = self.term.borrow().app_cursor;
        let mods = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
        let csi = |c: &str| if mods > 1 { format!("\x1b[1;{mods}{c}") } else if app { format!("\x1bO{c}") } else { format!("\x1b[{c}") };
        let tilde = |n: u8| if mods > 1 { format!("\x1b[{n};{mods}~") } else { format!("\x1b[{n}~") };
        let seq: Option<String> = match k {
            k if k == K::Up || k == K::KP_Up => Some(csi("A")), k if k == K::Down || k == K::KP_Down => Some(csi("B")),
            k if k == K::Right || k == K::KP_Right => Some(csi("C")), k if k == K::Left || k == K::KP_Left => Some(csi("D")),
            k if k == K::Home || k == K::KP_Home => Some(csi("H")), k if k == K::End || k == K::KP_End => Some(csi("F")),
            k if k == K::Insert || k == K::KP_Insert => Some(tilde(2)), k if k == K::Delete || k == K::KP_Delete => Some(tilde(3)),
            k if k == K::Page_Up || k == K::KP_Page_Up => Some(tilde(5)), k if k == K::Page_Down || k == K::KP_Page_Down => Some(tilde(6)),
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
            if ctrl && (ch.is_ascii_alphabetic() || "@[\\]^_ 2345678/".contains(ch)) {
                let b = match ch { ' ' | '@' | '2' => 0, '3' => 0x1b, '4' => 0x1c, '5' => 0x1d, '6' => 0x1e, '7' | '/' => 0x1f, '8' => 0x7f, c => (c.to_ascii_uppercase() as u8) & 0x1f };
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
                let follow = v >= t.vt && !c.paused();
                drop(t);
                c.follow.set(follow);
                c.view_top.set(v);
                c.area.queue_draw();
            });
        }
        {
            let c = self.clone();
            self.hscroll.adjustment().connect_value_changed(move |a| { c.view_left.set(a.value().round().max(0.0) as usize); c.area.queue_draw(); });
        }
        {
            let c = self.clone();
            self.area.connect_scroll_event(move |_, e| {
                let adj = c.scroll.adjustment();
                let d = match e.direction() { gdk::ScrollDirection::Up => -3.0, gdk::ScrollDirection::Down => 3.0, _ => e.delta().1 * 3.0 };
                adj.set_value((adj.value() + d).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
                glib::Propagation::Stop
            });
        }
        {
            let c = self.clone();
            self.area.connect_button_press_event(move |a, e| {
                a.grab_focus();
                let quick = c.cfg.borrow().quick_edit;
                let marking = c.sel.get() != Sel::None;
                let (x, y) = c.cell_at(e.position().0, e.position().1);
                match e.button() {
                    3 => {
                        // with a selection, right click copies it (both modes); QuickEdit pastes otherwise
                        if c.selection_rect().is_some() { c.copy_selection(); }
                        else if marking { c.end_mark(); }
                        else if quick { c.paste(); } else { c.edit_menu(e); }
                    }
                    1 if (quick || marking) && e.event_type() == gdk::EventType::DoubleButtonPress => c.select_word(x, y),
                    1 if quick || marking => { c.follow.set(false); c.set_sel(Sel::Marking { ax: x, ay: y, bx: x, by: y, selecting: true }); }
                    _ => {}
                }
                glib::Propagation::Stop
            });
        }
        {
            let c = self.clone();
            self.area.connect_motion_notify_event(move |a, e| {
                if e.state().contains(gdk::ModifierType::BUTTON1_MASK) {
                    if let Sel::Marking { ax, ay, .. } = c.sel.get() {
                        // dragging past the edge scrolls the window
                        let (px, py) = e.position();
                        let (w, h) = (a.allocated_width() as f64, a.allocated_height() as f64);
                        let (x, y) = c.cell_at(px.clamp(0.0, w - 1.0), py.clamp(0.0, h - 1.0));
                        let (x, y) = (if px >= w && x + 1 < c.term.borrow().cols { x + 1 } else if px < 0.0 { x.saturating_sub(1) } else { x },
                                      if py >= h { (y + 1).min(c.term.borrow().total_lines() - 1) } else if py < 0.0 { y.saturating_sub(1) } else { y });
                        c.sel.set(Sel::Marking { ax, ay, bx: x, by: y, selecting: true });
                        c.reveal(x, y);
                        a.queue_draw();
                    }
                }
                glib::Propagation::Stop
            });
        }
        { let c = self.clone(); self.area.connect_focus_in_event(move |a, _| { c.focused.set(true); a.queue_draw(); glib::Propagation::Proceed }); }
        { let c = self.clone(); self.area.connect_focus_out_event(move |a, _| { c.focused.set(false); a.queue_draw(); glib::Propagation::Proceed }); }
        // the window menu's Edit/Defaults/Properties (sysmenu.rs writes the chosen id)
        {
            let c = self.clone();
            self.win.connect_property_notify_event(move |w, e| {
                if e.atom() == gdk::Atom::intern("_AERODE_SYSMENU_CMD") && e.state() == gdk::PropertyState::NewValue {
                    if let Some(gw) = w.window() {
                        let v = gdk::property_get(&gw, &gdk::Atom::intern("_AERODE_SYSMENU_CMD"), &gdk::Atom::intern("UTF8_STRING"), 0, 256, 1);
                        if let Some((_, _, data)) = v {
                            let id = String::from_utf8_lossy(&data).to_string();
                            let c = c.clone();
                            // after the menu has gone (its grab would steal a dialog's focus)
                            glib::idle_add_local_once(move || c.command(&id));
                        }
                    }
                }
                glib::Propagation::Proceed
            });
        }
        { let c = self.clone(); self.win.connect_realize(move |_| c.publish_menu()); }
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
        let screen_bg = cfg.palette[self.screen_colors().1 as usize & 15];
        let (r, g, b) = rgb(screen_bg);
        cr.set_source_rgb(r, g, b);
        cr.paint().ok();
        let (top, left) = (self.view_top.get(), self.view_left.get());
        let rows = (a.height() as f64 / ch).ceil() as usize;
        let vis_cols = (a.width() as f64 / cw).ceil() as usize;
        let sel = self.sel.get();
        let block = match sel { Sel::Marking { ax, ay, bx, by, selecting: true } => Some((ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))), _ => None };
        // Mark mode: a blinking block "mark cursor"
        let mark = match sel { Sel::Marking { bx, by, selecting: false, .. } if self.blink_on.get() => Some((bx, by)), _ => None };
        let layout = if self.raster.borrow().is_none() {
            let l = pangocairo::functions::create_layout(cr);
            l.set_font_description(Some(&ttf_description(&cfg)));
            Some(l)
        } else { None };
        for row in 0..rows {
            let y = top + row;
            let Some(line) = t.line(y) else { continue };
            for (x, cell) in line.iter().enumerate().skip(left).take(vis_cols) {
                let mut fg = self.color_of(cell.fg, true, cell.bold);
                let mut bg = self.color_of(cell.bg, false, false);
                if cell.reverse { std::mem::swap(&mut fg, &mut bg); }
                // selections are shown inverted, like the Windows console
                let inv = block.is_some_and(|(x0, y0, x1, y1)| x >= x0 && x <= x1 && y >= y0 && y <= y1) || mark == Some((x, y));
                if inv { fg ^= 0xFFFFFF; bg ^= 0xFFFFFF; }
                let (px, py) = ((x - left) as f64 * cw, row as f64 * ch);
                if bg != screen_bg {
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
        if t.cursor_visible && self.blink_on.get() && self.focused.get() && abs >= top && abs < top + rows && t.cx >= left && sel == Sel::None {
            let frac = match t.cursor_shape { Some(CursorShape::Large) => 1.0, Some(CursorShape::Medium) => 0.5, Some(CursorShape::Small) => 0.25, None => cfg.cursor_size as f64 / 100.0 };
            let h = (ch * frac).round().max(2.0);
            let (px, py) = ((t.cx - left) as f64 * cw, (abs - top) as f64 * ch + ch - h);
            let (r, g, b) = rgb(self.color_of(Color::Default, true, false));
            cr.set_source_rgb(r, g, b);
            cr.set_operator(cairo::Operator::Difference);
            cr.rectangle(px, py, cw, h);
            cr.fill().ok();
            cr.set_operator(cairo::Operator::Over);
        }
    }

    // ------------------------------------------------------------ properties support
    pub fn config(&self) -> ConsoleConfig {
        let mut c = self.cfg.borrow().clone();
        // Window Position shows where the window is now
        let (x, y) = self.win.position();
        c.win_x = x;
        c.win_y = y;
        c
    }

    pub fn title_key(&self) -> &str { &self.key }

    /// Properties OK: apply to this window and remember them for windows with the same title.
    pub fn set_config(&self, c: ConsoleConfig) {
        save_for(&self.key, &c);
        self.color_override.set(None);
        *self.cfg.borrow_mut() = c.clone();
        self.apply_font();
        let rows = c.win_h.min(c.buffer_h);
        self.term.borrow_mut().resize(c.buffer_w, rows, c.buffer_h);
        if let Some(m) = self.master.borrow().as_ref() { let _ = m.resize(PtySize { rows: rows as u16, cols: c.buffer_w as u16, pixel_width: 0, pixel_height: 0 }); }
        self.fit_window();
        if !c.system_pos { self.win.move_(c.win_x, c.win_y); }
        let fs = self.win.window().map(|w| w.state().contains(gdk::WindowState::FULLSCREEN)).unwrap_or(false);
        if c.full_screen && !fs { self.win.fullscreen(); } else if !c.full_screen && fs { self.win.unfullscreen(); }
        self.follow.set(true);
        self.update_scrollbar();
        self.area.queue_draw();
    }
}
