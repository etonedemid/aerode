//! Snipping Tool – Windows 7 Snipping Tool remake.
//!   aerode-snip           open the tool
//!   aerode-snip --full    Print Screen: copy the whole screen to the clipboard (and save a copy)
//!   aerode-snip --clip    Win+Shift+S: rectangular snip of the frozen screen straight to the clipboard
//!   aerode-snip --rect    start a rectangular snip immediately, then open it in the editor
//! X11 clipboard contents live in the process that owns CLIPBOARD, so after copying a snip the process
//! stays alive (windowless once its windows are closed) until another client copies something.
use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{cairo, gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use aerode::{config, theme, util, x11::Wm};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind { Free, Rect, Window, Full }

impl Kind {
    fn label(self) -> &'static str {
        match self { Kind::Free => "Free-form Snip", Kind::Rect => "Rectangular Snip", Kind::Window => "Window Snip", Kind::Full => "Full-screen Snip" }
    }
}

fn capture_screen() -> Option<Pixbuf> {
    let root = gdk::Screen::default()?.root_window()?;
    let (w, h) = (root.width(), root.height());
    let surf = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        cr.set_source_window(&root, 0.0, 0.0);
        cr.paint().ok()?;
    }
    gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h)
}

// ------------------------------------------------------------- options (Win7 Options dialog)
fn opt(name: &str, default: bool) -> bool {
    std::fs::read_to_string(config::conf_dir().join("snip.conf")).ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix(&format!("{name}=")).map(|v| v == "1")))
        .unwrap_or(default)
}

fn opt_str(name: &str) -> Option<String> {
    std::fs::read_to_string(config::conf_dir().join("snip.conf")).ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix(&format!("{name}=")).map(String::from)))
}

fn set_opt_str(name: &str, v: &str) {
    let p = config::conf_dir().join("snip.conf");
    let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().filter(|l| !l.starts_with(&format!("{name}="))).map(String::from).collect();
    lines.push(format!("{name}={v}"));
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(p, lines.join("\n"));
}

fn set_opt(name: &str, v: bool) { set_opt_str(name, if v { "1" } else { "0" }); }

/// Win7's ink colours (Options > Ink color, the pens' Customize dialog).
const COLORS: &[(&str, (f64, f64, f64))] = &[
    ("Red", (0.93, 0.11, 0.14)), ("Blue", (0.0, 0.35, 0.85)), ("Black", (0.0, 0.0, 0.0)), ("Green", (0.0, 0.6, 0.2)), ("Yellow", (1.0, 0.87, 0.0)),
    ("Orange", (1.0, 0.5, 0.0)), ("Purple", (0.5, 0.0, 0.6)), ("Pink", (1.0, 0.4, 0.7)), ("Light Blue", (0.4, 0.75, 1.0)), ("Dark Red", (0.6, 0.0, 0.0)),
    ("Dark Green", (0.0, 0.4, 0.1)), ("Dark Blue", (0.0, 0.1, 0.5)), ("Gray", (0.5, 0.5, 0.5)), ("White", (1.0, 1.0, 1.0)),
];

fn color_named(n: &str) -> (f64, f64, f64) { COLORS.iter().find(|(c, _)| *c == n).map(|c| c.1).unwrap_or(COLORS[0].1) }

fn ink_color() -> (f64, f64, f64) { color_named(&opt_str("ink_color").unwrap_or_else(|| "Red".into())) }

// ------------------------------------------------------------- selection overlay
/// Frames of the visible top-level windows, topmost first, for Window Snip.
fn window_rects() -> Vec<(i32, i32, i32, i32)> {
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let Ok(wm) = Wm::connect() else { return vec![] };
    let atom = |n: &[u8]| wm.conn.intern_atom(false, n).ok().and_then(|c| c.reply().ok()).map(|r| r.atom).unwrap_or(0);
    let (stacking, extents, state, hidden) = (atom(b"_NET_CLIENT_LIST_STACKING"), atom(b"_NET_FRAME_EXTENTS"), atom(b"_NET_WM_STATE"), atom(b"_NET_WM_STATE_HIDDEN"));
    let prop = |w: u32, a: u32, ty: AtomEnum| -> Vec<u32> {
        wm.conn.get_property(false, w, a, ty, 0, 1024).ok().and_then(|c| c.reply().ok()).and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default()
    };
    let mut ids = prop(wm.root, stacking, AtomEnum::WINDOW);
    if ids.is_empty() { ids = wm.client_ids(); }
    ids.into_iter().rev().filter_map(|id| {
        if prop(id, state, AtomEnum::ATOM).contains(&hidden) { return None; }
        let g = wm.conn.get_geometry(id).ok()?.reply().ok()?;
        let t = wm.conn.translate_coordinates(id, wm.root, 0, 0).ok()?.reply().ok()?;
        let e = prop(id, extents, AtomEnum::CARDINAL);
        let (l, r, top, b) = (*e.first().unwrap_or(&0) as i32, *e.get(1).unwrap_or(&0) as i32, *e.get(2).unwrap_or(&0) as i32, *e.get(3).unwrap_or(&0) as i32);
        Some((t.dst_x as i32 - l, t.dst_y as i32 - top, g.width as i32 + l + r, g.height as i32 + top + b))
    }).collect()
}

/// The frozen, whitewashed screen the user snips from.
/// `tool`: started from the Snipping Tool (its overlay and selection-ink options apply), not Win+Shift+S.
fn select(kind: Kind, shot: Pixbuf, tool: bool, done: Rc<dyn Fn(Option<Pixbuf>)>) {
    if kind == Kind::Full { done(Some(shot)); return; }
    let (sw, sh) = (shot.width(), shot.height());
    let overlay = !tool || opt("overlay", true);
    let ink = ink_color();
    let show_ink = tool && opt("ink", true);
    let win = gtk::Window::new(gtk::WindowType::Popup);
    win.move_(0, 0);
    win.set_size_request(sw, sh);
    win.set_app_paintable(true);
    let area = gtk::DrawingArea::new();
    area.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::POINTER_MOTION_MASK);
    win.add(&area);
    let pts: Rc<RefCell<Vec<(f64, f64)>>> = Default::default();
    let dragging = Rc::new(Cell::new(false));
    let hover: Rc<Cell<Option<(i32, i32, i32, i32)>>> = Default::default();
    let rects = Rc::new(if kind == Kind::Window { window_rects() } else { vec![] });
    // the window under (x, y); outside every window the desktop, i.e. the whole screen (Win7 snips the desktop window)
    let window_at = { let rects = rects.clone(); move |x: f64, y: f64| rects.iter().find(|r| x as i32 >= r.0 && y as i32 >= r.1 && (x as i32) < r.0 + r.2 && (y as i32) < r.1 + r.3).copied().unwrap_or((0, 0, sw, sh)) };
    {
        let (shot, pts, hover) = (shot.clone(), pts.clone(), hover.clone());
        area.connect_draw(move |_, cr| {
            cr.set_source_pixbuf(&shot, 0.0, 0.0);
            cr.paint().ok();
            // whitewash everything outside the current selection
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            cr.rectangle(0.0, 0.0, sw as f64, sh as f64);
            let p = pts.borrow();
            let path = |cr: &cairo::Context| match kind {
                Kind::Rect if p.len() >= 2 => {
                    let (a, b) = (p[0], p[p.len() - 1]);
                    cr.rectangle(a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs());
                    true
                }
                Kind::Free if p.len() >= 2 => {
                    cr.move_to(p[0].0, p[0].1);
                    for q in p.iter().skip(1) { cr.line_to(q.0, q.1); }
                    true
                }
                Kind::Window => if let Some((x, y, w, h)) = hover.get() { cr.rectangle(x as f64, y as f64, w as f64, h as f64); true } else { false },
                _ => false,
            };
            path(cr);
            if kind == Kind::Free { cr.close_path(); }
            if overlay {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
                cr.fill().ok();
            } else { cr.new_path(); }
            cr.set_source_rgb(ink.0, ink.1, ink.2);
            cr.set_line_width(2.0);
            if path(cr) { cr.stroke().ok(); }
            glib::Propagation::Proceed
        });
    }
    let finish = {
        let (win, pts, shot, done, hover) = (win.clone(), pts.clone(), shot.clone(), done.clone(), hover.clone());
        Rc::new(move |cancel: bool| {
            // once only (Esc, a right click and the main window's Cancel can all end it)
            if !win.is_visible() { return; }
            // hide at once (close() only queues a delete event, which GTK drops while a dialog has a grab)
            win.hide();
            { let w = win.clone(); glib::idle_add_local_once(move || unsafe { w.destroy() }); }
            ACTIVE.with(|a| a.borrow_mut().take());
            main_idle();
            if cancel { done(None); return; }
            let p = pts.borrow();
            let bbox = match kind {
                Kind::Window => hover.get(),
                _ if p.len() >= 2 => {
                    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                    let use_pts: Vec<(f64, f64)> = if kind == Kind::Rect { vec![p[0], p[p.len() - 1]] } else { p.clone() };
                    for q in &use_pts { x0 = x0.min(q.0); y0 = y0.min(q.1); x1 = x1.max(q.0); y1 = y1.max(q.1); }
                    Some((x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
                }
                _ => None,
            };
            let Some((x, y, w, h)) = bbox else { done(None); return };
            let (w, h) = (w + x.min(0), h + y.min(0));
            let (x, y) = (x.max(0), y.max(0));
            let (w, h) = (w.min(shot.width() - x), h.min(shot.height() - y));
            if w < 3 || h < 3 { done(None); return; }
            let sub = shot.new_subpixbuf(x, y, w, h);
            let Ok(surf) = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h) else { done(None); return };
            {
                let Ok(cr) = cairo::Context::new(&surf) else { done(None); return };
                if kind == Kind::Free {
                    // everything outside the free-form path is white (as in Win7)
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    cr.paint().ok();
                    cr.move_to(p[0].0 - x as f64, p[0].1 - y as f64);
                    for q in p.iter().skip(1) { cr.line_to(q.0 - x as f64, q.1 - y as f64); }
                    cr.close_path();
                    cr.save().ok();
                    cr.clip_preserve();
                    cr.set_source_pixbuf(&sub, 0.0, 0.0);
                    cr.paint().ok();
                    cr.restore().ok();
                } else {
                    cr.set_source_pixbuf(&sub, 0.0, 0.0);
                    cr.paint().ok();
                    cr.rectangle(1.0, 1.0, w as f64 - 2.0, h as f64 - 2.0);
                }
                // "Show selection ink after snips are captured"
                if show_ink {
                    cr.set_source_rgb(ink.0, ink.1, ink.2);
                    cr.set_line_width(2.0);
                    cr.stroke().ok();
                }
            }
            done(gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h));
        })
    };
    {
        let (pts, dragging, area2, hover, window_at) = (pts.clone(), dragging.clone(), area.clone(), hover.clone(), window_at.clone());
        let fin = finish.clone();
        area.connect_button_press_event(move |_, e| {
            if e.button() == 3 { fin(true); return glib::Propagation::Stop; }
            if kind == Kind::Window { hover.set(Some(window_at(e.position().0, e.position().1))); fin(false); return glib::Propagation::Stop; }
            dragging.set(true);
            pts.borrow_mut().clear();
            pts.borrow_mut().push(e.position());
            area2.queue_draw();
            glib::Propagation::Stop
        });
    }
    {
        let (pts, dragging, hover) = (pts.clone(), dragging.clone(), hover.clone());
        area.connect_motion_notify_event(move |a, e| {
            let (x, y) = e.position();
            if kind == Kind::Window {
                let h = Some(window_at(x, y));
                if h != hover.get() { hover.set(h); a.queue_draw(); }
            } else if dragging.get() {
                let mut p = pts.borrow_mut();
                if kind == Kind::Rect && p.len() > 1 { p.truncate(1); }
                p.push((x, y));
                a.queue_draw();
            }
            glib::Propagation::Stop
        });
    }
    {
        let (dragging, fin) = (dragging.clone(), finish.clone());
        area.connect_button_release_event(move |_, _| {
            if dragging.replace(false) { fin(false); }
            glib::Propagation::Stop
        });
    }
    {
        let fin = finish.clone();
        win.connect_key_press_event(move |_, e| { if e.keyval() == gdk::keys::constants::Escape { fin(true); } glib::Propagation::Stop });
    }
    win.show_all();
    // a free-form outline needs every motion event
    if let Some(w) = area.window() { w.set_event_compression(false); }
    {
        let fin = finish.clone();
        ACTIVE.with(|a| *a.borrow_mut() = Some(Rc::new(move || fin(true))));
    }
    // the Snipping Tool's own window stays on top of the overlay, as in Win7 (it was hidden while the
    // screen was captured), with Cancel enabled and the instruction for this snip type
    if tool { main_capturing(kind); }
    // pointer + keyboard grab (crosshair cursor, Esc). Started by the WM's Win+Shift+S binding, the WM's key
    // grab is still active while the keys are held and our grab fails - retry until they are released
    fn grab(win: &gtk::Window) -> bool {
        let (Some(d), Some(gw)) = (gdk::Display::default(), win.window()) else { return true };
        let Some(seat) = d.default_seat() else { return true };
        let cur = gdk::Cursor::from_name(&d, "crosshair");
        seat.grab(&gw, gdk::SeatCapabilities::ALL, true, cur.as_ref(), None, None) == gdk::GrabStatus::Success
    }
    if !grab(&win) {
        let (win, tries) = (win.clone(), Cell::new(0));
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            tries.set(tries.get() + 1);
            if !win.is_visible() || grab(&win) || tries.get() > 100 { glib::ControlFlow::Break } else { glib::ControlFlow::Continue }
        });
    }
}

thread_local! {
    static CLIP_HOLD: RefCell<Option<gio::ApplicationHoldGuard>> = const { RefCell::new(None) };
    /// the selection in progress: calling it cancels the snip
    static ACTIVE: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    /// the main (capture) window
    static MAIN: RefCell<Option<MainUi>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct MainUi { win: gtk::ApplicationWindow, cancel: gtk::ToolButton, instr: gtk::Label }

const IDLE_TEXT: &str = "Click New to capture a snip with the type you used last,\nor pick a snip type from the arrow next to New.";

/// The main window while a snip is being selected: on top of the overlay, Cancel enabled.
fn main_capturing(kind: Kind) {
    let Some(m) = MAIN.with(|m| m.borrow().clone()) else { return };
    m.instr.set_text(match kind {
        Kind::Window => "Click the window you want to capture.",
        Kind::Free => "Drag the cursor around the shape you want to capture.",
        _ => "Drag the cursor around the area you want to capture.",
    });
    m.cancel.set_sensitive(true);
    m.win.set_keep_above(true);
    m.win.show();
    m.win.present();
    // the overlay is override-redirect, so the WM only stacks frames below it: raise our frame over
    // it ourselves once the WM has mapped it again
    // (a few times: the WM may restack the frame after mapping it - focus, its map animation)
    for ms in [60, 250, 600, 1200] {
        let w = m.win.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || { if ACTIVE.with(|a| a.borrow().is_some()) { raise_frame(&w); } });
    }
}

/// Put a toplevel's frame (its ancestor that is a child of the root) at the top of the stack.
fn raise_frame(w: &gtk::ApplicationWindow) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt, StackMode};
    let Some(xid) = w.window().and_then(|g| g.downcast::<gdkx11::X11Window>().ok()).map(|x| x.xid() as u32) else { return };
    let Ok(wm) = Wm::connect() else { return };
    let mut win = xid;
    for _ in 0..8 {
        let Some(t) = wm.conn.query_tree(win).ok().and_then(|c| c.reply().ok()) else { return };
        if t.parent == wm.root || t.parent == 0 { break; }
        win = t.parent;
    }
    let _ = wm.conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    let _ = wm.conn.flush();
    // a round trip, so the request is not lost when the connection closes
    let _ = wm.conn.get_input_focus().ok().and_then(|c| c.reply().ok());
}

/// The main window between snips.
fn main_idle() {
    let Some(m) = MAIN.with(|m| m.borrow().clone()) else { return };
    m.instr.set_text(IDLE_TEXT);
    m.cancel.set_sensitive(false);
    m.win.set_keep_above(false);
}

fn cancel_active() { if let Some(f) = ACTIVE.with(|a| a.borrow().clone()) { f(); } }

/// Put a snip on the clipboard and keep serving it. X11 clipboard data is held by the owning process and
/// vanishes when it exits; `store()` only hands it over when a clipboard manager runs (none does in an
/// aerode session). So the app is held - windowless once the editor is closed - until another client
/// owns CLIPBOARD.
fn copy_to_clipboard(img: &Pixbuf) {
    let cb = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    cb.set_image(img);
    cb.store();
    let Some(app) = gio::Application::default() else { return };
    if CLIP_HOLD.with(|h| h.borrow().is_some()) { return; }
    CLIP_HOLD.with(|h| *h.borrow_mut() = Some(app.hold()));
    static WATCH: std::sync::Once = std::sync::Once::new();
    WATCH.call_once(|| {
        // GTK reports our own set_image as an owner change too, and wraps a foreign new owner in a FOREIGN
        // GdkWindow (so selection_owner_get alone would still answer): let go only for someone else's window
        cb.connect_local("owner-change", false, |_| {
            let ours = gdk::selection_owner_get(&gdk::SELECTION_CLIPBOARD).is_some_and(|w| w.window_type() != gdk::WindowType::Foreign);
            if !ours { drop(CLIP_HOLD.with(|h| h.borrow_mut().take())); }
            None
        });
    });
}

// ------------------------------------------------------------- file formats
/// GIF89a writer (gdk-pixbuf cannot write GIF): exact palette when the snip has ≤ 256 colours
/// (typical for screenshots), median-cut quantisation otherwise.
fn write_gif(img: &Pixbuf, path: &std::path::Path) -> std::io::Result<()> {
    use std::collections::HashMap;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (n, stride) = (img.n_channels() as usize, img.rowstride() as usize);
    let bytes = img.read_pixel_bytes();
    let mut px: Vec<u32> = Vec::with_capacity(w * h);
    for y in 0..h { for x in 0..w { let o = y * stride + x * n; px.push(((bytes[o] as u32) << 16) | ((bytes[o + 1] as u32) << 8) | bytes[o + 2] as u32); } }
    // exact palette?
    let mut exact: HashMap<u32, u8> = HashMap::new();
    let mut palette: Vec<u32> = vec![];
    for &c in &px {
        if exact.contains_key(&c) { continue; }
        if palette.len() == 256 { palette.clear(); break; }
        exact.insert(c, palette.len() as u8);
        palette.push(c);
    }
    let indices: Vec<u8> = if !palette.is_empty() {
        px.iter().map(|c| exact[c]).collect()
    } else {
        // median cut over a 5-bit-per-channel histogram
        let mut hist: HashMap<u16, (u32, [u64; 3])> = HashMap::new();
        let key = |c: u32| (((c >> 19) & 31) << 10 | ((c >> 11) & 31) << 5 | ((c >> 3) & 31)) as u16;
        for &c in &px { let e = hist.entry(key(c)).or_insert((0, [0; 3])); e.0 += 1; e.1[0] += (c >> 16 & 255) as u64; e.1[1] += (c >> 8 & 255) as u64; e.1[2] += (c & 255) as u64; }
        let mut boxes: Vec<Vec<(u16, u32, [u64; 3])>> = vec![hist.into_iter().map(|(k, (n, s))| (k, n, s)).collect()];
        let comp = |k: u16, ch: usize| ((k >> (10 - ch * 5)) & 31) as u32;
        while boxes.len() < 256 {
            let Some((bi, ch)) = boxes.iter().enumerate().filter(|(_, b)| b.len() > 1).map(|(i, b)| {
                let ranges: Vec<u32> = (0..3).map(|ch| { let (mn, mx) = b.iter().fold((31, 0), |(a, z), e| (a.min(comp(e.0, ch)), z.max(comp(e.0, ch)))); mx - mn }).collect();
                let ch = (0..3).max_by_key(|c| ranges[*c]).unwrap();
                let weight: u64 = b.iter().map(|e| e.1 as u64).sum();
                (i, ch, ranges[ch] as u64 * weight)
            }).max_by_key(|x| x.2).map(|(i, ch, _)| (i, ch)) else { break };
            let mut b = boxes.swap_remove(bi);
            b.sort_by_key(|e| comp(e.0, ch));
            let total: u64 = b.iter().map(|e| e.1 as u64).sum();
            let mut acc = 0;
            let mut cut = 1;
            for (i, e) in b.iter().enumerate() { acc += e.1 as u64; if acc * 2 >= total { cut = (i + 1).clamp(1, b.len() - 1); break; } }
            let rest = b.split_off(cut);
            boxes.push(b);
            boxes.push(rest);
        }
        palette = boxes.iter().map(|b| {
            let n: u64 = b.iter().map(|e| e.1 as u64).sum::<u64>().max(1);
            let s = b.iter().fold([0u64; 3], |a, e| [a[0] + e.2[0], a[1] + e.2[1], a[2] + e.2[2]]);
            ((s[0] / n) as u32) << 16 | ((s[1] / n) as u32) << 8 | (s[2] / n) as u32
        }).collect();
        let mut cache: HashMap<u16, u8> = HashMap::new();
        let near = |c: u32| -> u8 {
            let (r, g, b) = ((c >> 16 & 255) as i32, (c >> 8 & 255) as i32, (c & 255) as i32);
            palette.iter().enumerate().min_by_key(|(_, p)| { let (pr, pg, pb) = ((*p >> 16 & 255) as i32, (*p >> 8 & 255) as i32, (**p & 255) as i32); (r - pr).pow(2) * 3 + (g - pg).pow(2) * 4 + (b - pb).pow(2) * 2 }).map(|(i, _)| i as u8).unwrap_or(0)
        };
        px.iter().map(|&c| *cache.entry(key(c)).or_insert_with(|| near(c))).collect()
    };
    while palette.len() < 256 { palette.push(0); }
    let mut out = Vec::with_capacity(w * h / 2);
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&[0xF7, 0, 0]); // global colour table, 8 bits
    for c in &palette { out.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, *c as u8]); }
    out.push(0x2C);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.push(0);
    out.push(8); // LZW minimum code size
    for chunk in lzw(&indices, 8).chunks(255) { out.push(chunk.len() as u8); out.extend_from_slice(chunk); }
    out.push(0);
    out.push(0x3B);
    std::fs::write(path, out)
}

/// GIF LZW (variable code size, LSB-first).
fn lzw(data: &[u8], min: u8) -> Vec<u8> {
    use std::collections::HashMap;
    let clear = 1u16 << min;
    let eoi = clear + 1;
    let mut out = vec![];
    let (mut acc, mut nbits) = (0u32, 0u32);
    let mut emit = |code: u16, size: u32, out: &mut Vec<u8>| {
        acc |= (code as u32) << nbits;
        nbits += size;
        while nbits >= 8 { out.push(acc as u8); acc >>= 8; nbits -= 8; }
    };
    let mut size = min as u32 + 1;
    let mut next = eoi;
    let mut dict: HashMap<(u16, u8), u16> = HashMap::new();
    emit(clear, size, &mut out);
    let mut cur: Option<u16> = None;
    for &k in data {
        let Some(p) = cur else { cur = Some(k as u16); continue };
        if let Some(&c) = dict.get(&(p, k)) { cur = Some(c); continue; }
        emit(p, size, &mut out);
        next += 1;
        dict.insert((p, k), next);
        if next as u32 >= (1 << size) && size < 12 { size += 1; }
        if next == 4095 {
            emit(clear, size, &mut out);
            dict.clear();
            size = min as u32 + 1;
            next = eoi;
        }
        cur = Some(k as u16);
    }
    if let Some(p) = cur { emit(p, size, &mut out); }
    emit(eoi, size, &mut out);
    if nbits > 0 { out.push(acc as u8); }
    out
}

/// "Single file HTML (MHT)": a MIME web archive with the snip as a PNG.
fn write_mht(img: &Pixbuf, path: &std::path::Path) -> Result<(), String> {
    let png = img.save_to_bufferv("png", &[]).map_err(|e| e.to_string())?;
    let b64 = glib::base64_encode(&png);
    let mut lines = String::new();
    for chunk in b64.as_bytes().chunks(76) { lines.push_str(std::str::from_utf8(chunk).unwrap_or("")); lines.push_str("\r\n"); }
    let b = "----=_NextPart_000_0000_AERODE.SNIP";
    let doc = format!("From: <Saved by Snipping Tool>\r\nSubject: Snipping Tool\r\nDate: {}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related;\r\n\ttype=\"text/html\";\r\n\tboundary=\"{b}\"\r\n\r\nThis is a multi-part message in MIME format.\r\n\r\n--{b}\r\nContent-Type: text/html;\r\n\tcharset=\"utf-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body><img src=\"image001.png\" width=\"{}\" height=\"{}\"></body></html>\r\n\r\n--{b}\r\nContent-Type: image/png\r\nContent-Transfer-Encoding: base64\r\nContent-Location: image001.png\r\n\r\n{lines}\r\n--{b}--\r\n",
        chrono::Local::now().to_rfc2822(), img.width(), img.height());
    std::fs::write(path, doc).map_err(|e| e.to_string())
}

// ------------------------------------------------------------- editor
#[derive(Clone)]
struct Stroke { pts: Vec<(f64, f64)>, rgba: (f64, f64, f64, f64), width: f64, chisel: bool }

#[derive(Clone, Copy, PartialEq)]
enum Tool { Pen, Highlighter, Eraser }

/// The pen: colour, width, round or chisel tip (Tools > Pen > Customize).
#[derive(Clone, Copy)]
struct Pen { color: (f64, f64, f64), width: f64, chisel: bool }

struct Editor {
    win: gtk::ApplicationWindow,
    img: RefCell<Pixbuf>,
    strokes: RefCell<Vec<Stroke>>,
    tool: Cell<Tool>,
    pen: Cell<Pen>,
    area: gtk::DrawingArea,
    saved: Cell<bool>,
    drawing: Cell<bool>,
    tool_buttons: RefCell<Vec<(Tool, gtk::ToolItem)>>,
}

impl Editor {
    fn flatten(&self) -> Option<Pixbuf> {
        let img = self.img.borrow();
        let (w, h) = (img.width(), img.height());
        let surf = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h).ok()?;
        {
            let cr = cairo::Context::new(&surf).ok()?;
            cr.set_source_pixbuf(&img, 0.0, 0.0);
            cr.paint().ok();
            draw_strokes(&cr, &self.strokes.borrow());
        }
        gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h)
    }

    fn copy(&self) { if let Some(p) = self.flatten() { copy_to_clipboard(&p); } }

    /// Save As with Win7's file types; the type chosen in the dialog decides the format when no
    /// extension is typed. Returns true when the snip was written.
    fn save(&self) -> bool {
        let fc = gtk::FileChooserDialog::with_buttons(Some("Save As"), Some(&self.win), gtk::FileChooserAction::Save,
            &[("_Save", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
        fc.set_do_overwrite_confirmation(true);
        fc.set_default_size(780, 520);
        let types = [("Portable Network Graphic file (PNG) (*.PNG)", "png"), ("GIF file (*.GIF)", "gif"), ("JPEG file (*.JPG)", "jpg"), ("Single file HTML (MHT) (*.MHT)", "mht")];
        let mut filters = vec![];
        for (name, ext) in types {
            let f = gtk::FileFilter::new();
            f.set_name(Some(name));
            f.add_pattern(&format!("*.{ext}"));
            f.add_pattern(&format!("*.{}", ext.to_uppercase()));
            if ext == "jpg" { f.add_pattern("*.jpeg"); f.add_pattern("*.JPEG"); }
            fc.add_filter(f.clone());
            filters.push((f, ext));
        }
        let last = opt_str("save_type").unwrap_or_else(|| "png".into());
        if let Some((f, _)) = filters.iter().find(|(_, e)| *e == last) { fc.set_filter(f); }
        fc.set_current_name(&format!("Capture.{}", last.to_uppercase()));
        let _ = fc.set_current_folder(opt_str("save_dir").map(std::path::PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(|| util::special_dir(glib::UserDirectory::Pictures, "Pictures")));
        // the file name follows the chosen type, like Windows
        {
            let (fc2, filters) = (fc.clone(), filters.clone());
            fc.connect_notify_local(Some("filter"), move |_, _| {
                let Some(cur) = fc2.filter() else { return };
                let Some((_, ext)) = filters.iter().find(|(f, _)| *f == cur) else { return };
                let name = fc2.current_name().map(|n| n.to_string()).unwrap_or_default();
                let stem = std::path::Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Capture".into());
                fc2.set_current_name(&format!("{stem}.{}", ext.to_uppercase()));
            });
        }
        let mut ok = false;
        if fc.run() == gtk::ResponseType::Accept {
            if let Some(mut p) = fc.filename() {
                let chosen = fc.filter().and_then(|f| filters.iter().find(|(x, _)| *x == f).map(|x| x.1)).unwrap_or("png");
                let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let ty = match ext.as_str() { "jpg" | "jpeg" => "jpg", "gif" => "gif", "png" => "png", "mht" | "mhtml" => "mht", "bmp" => "bmp", _ => { p.set_extension(chosen.to_uppercase()); chosen } };
                let res = match self.flatten() {
                    None => Err("The snip could not be prepared.".to_string()),
                    Some(img) => match ty {
                        "gif" => write_gif(&img, &p).map_err(|e| e.to_string()),
                        "mht" => write_mht(&img, &p),
                        "jpg" => img.savev(&p, "jpeg", &[("quality", "95")]).map_err(|e| e.to_string()),
                        "bmp" => img.savev(&p, "bmp", &[]).map_err(|e| e.to_string()),
                        _ => img.savev(&p, "png", &[]).map_err(|e| e.to_string()),
                    },
                };
                match res {
                    Ok(()) => {
                        ok = true;
                        self.saved.set(true);
                        set_opt_str("save_type", ty);
                        if let Some(d) = p.parent() { set_opt_str("save_dir", &d.to_string_lossy()); }
                    }
                    Err(e) => util::error("The snip could not be saved.", &e),
                }
            }
        }
        fc.close();
        ok
    }

    /// "Prompt to save snips before exiting": false when the user cancelled.
    fn maybe_save(&self) -> bool {
        if self.saved.get() || !opt("prompt", true) { return true; }
        let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Other, gtk::ButtonsType::None, "");
        d.set_title("Snipping Tool");
        d.set_markup("<span size='large' foreground='#003399'>Do you want to save changes to Snipping Tool?</span>");
        d.add_button("_Yes", gtk::ResponseType::Yes);
        d.add_button("_No", gtk::ResponseType::No);
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        d.set_default_response(gtk::ResponseType::Yes);
        let r = d.run();
        d.close();
        match r { gtk::ResponseType::Yes => self.save(), gtk::ResponseType::No => true, _ => false }
    }

    fn set_tool(&self, t: Tool) {
        self.tool.set(t);
        // Pen, Highlighter and Eraser work like radio buttons; the chosen one looks pressed
        for (k, b) in self.tool_buttons.borrow().iter() {
            if let Some(tb) = b.downcast_ref::<gtk::ToggleToolButton>() { if tb.is_active() != (*k == t) { tb.set_active(*k == t); } }
            if *k == t { b.style_context().add_class("snip-tool-on") } else { b.style_context().remove_class("snip-tool-on") }
        }
        let name = match t { Tool::Pen => "pencil", Tool::Highlighter => "pencil", Tool::Eraser => "crosshair" };
        if let (Some(d), Some(w)) = (gdk::Display::default(), self.area.window()) { w.set_cursor(gdk::Cursor::from_name(&d, name).as_ref()); }
    }

    fn send(&self, attach: bool) {
        let Some(img) = self.flatten() else { return };
        let p = glib::tmp_dir().join(format!("Capture-{}.png", std::process::id()));
        if img.savev(&p, "png", &[]).is_err() { return; }
        let ok = if attach { util::spawn(&["xdg-email", "--attach", &p.to_string_lossy()]) } else { util::spawn(&["xdg-email", "--body", "See the attached snip.", "--attach", &p.to_string_lossy()]) };
        if !ok { util::open_uri("mailto:"); }
    }
}

fn draw_strokes(cr: &cairo::Context, strokes: &[Stroke]) {
    for s in strokes {
        if s.pts.is_empty() { continue; }
        cr.set_source_rgba(s.rgba.0, s.rgba.1, s.rgba.2, s.rgba.3);
        cr.set_line_width(s.width);
        cr.set_line_cap(if s.chisel { cairo::LineCap::Square } else { cairo::LineCap::Round });
        cr.set_line_join(if s.chisel { cairo::LineJoin::Bevel } else { cairo::LineJoin::Round });
        cr.move_to(s.pts[0].0, s.pts[0].1);
        if s.pts.len() == 1 { cr.line_to(s.pts[0].0 + 0.1, s.pts[0].1); }
        for p in s.pts.iter().skip(1) { cr.line_to(p.0, p.1); }
        cr.stroke().ok();
    }
}

fn tool_button(icon: &str, label: &str, tip: &str, important: bool) -> gtk::ToolButton {
    let b = gtk::ToolButton::new(Some(&util::image(icon, 16)), Some(label));
    b.set_is_important(important);
    WidgetExt::set_tooltip_text(&b, Some(tip));
    b
}

fn load_pen() -> Pen {
    let custom = opt_str("pen").unwrap_or_default();
    let mut it = custom.split(',');
    let color = it.next().filter(|s| !s.is_empty()).map(color_named).unwrap_or(COLORS[0].1);
    let width = it.next().and_then(|w| w.parse().ok()).unwrap_or(2.0);
    let chisel = it.next() == Some("chisel");
    Pen { color, width, chisel }
}

/// Tools > Pen > Customize: colour, thickness and tip, like Win7's "Customize" dialog.
fn customize_pen(parent: &gtk::Window, cur: Pen) -> Option<(Pen, String)> {
    let d = gtk::Dialog::with_buttons(Some("Customize"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_resizable(false);
    d.set_default_response(gtk::ResponseType::Ok);
    let g = gtk::Grid::new();
    g.set_border_width(12);
    g.set_row_spacing(8);
    g.set_column_spacing(10);
    let color = gtk::ComboBoxText::new();
    for (n, _) in COLORS { color.append(Some(n), n); }
    let cur_name = COLORS.iter().find(|(_, c)| *c == cur.color).map(|c| c.0).unwrap_or("Red");
    color.set_active_id(Some(cur_name));
    let thick = gtk::ComboBoxText::new();
    for (id, l) in [("1", "Fine point pen"), ("2", "Medium point pen"), ("4", "Thick point pen")] { thick.append(Some(id), l); }
    thick.set_active_id(Some(if cur.width <= 1.0 { "1" } else if cur.width <= 2.0 { "2" } else { "4" }));
    let tip = gtk::ComboBoxText::new();
    tip.append(Some("round"), "Round tip pen");
    tip.append(Some("chisel"), "Chisel tip pen");
    tip.set_active_id(Some(if cur.chisel { "chisel" } else { "round" }));
    for (i, (l, w)) in [("Color:", &color), ("Thickness:", &thick), ("Tip:", &tip)].iter().enumerate() {
        let lab = gtk::Label::new(Some(l));
        lab.set_xalign(0.0);
        g.attach(&lab, 0, i as i32, 1, 1);
        g.attach(*w, 1, i as i32, 1, 1);
    }
    d.content_area().add(&g);
    d.show_all();
    let r = d.run();
    let res = (r == gtk::ResponseType::Ok).then(|| {
        let name = color.active_id().map(|s| s.to_string()).unwrap_or_else(|| "Red".into());
        let w: f64 = thick.active_id().and_then(|s| s.parse().ok()).unwrap_or(2.0);
        let chisel = tip.active_id().as_deref() == Some("chisel");
        (Pen { color: color_named(&name), width: w, chisel }, format!("{name},{w},{}", if chisel { "chisel" } else { "round" }))
    });
    d.close();
    res
}

fn open_editor(app: &gtk::Application, shot: Pixbuf, main: Option<gtk::Window>) {
    let win = gtk::ApplicationWindow::new(app);
    win.set_title("Snipping Tool");
    win.set_icon_name(Some(util::first_icon(&["accessories-screenshot"])));
    // the chosen drawing tool looks pressed (Win7's checked toolbar button)
    {
        let css = gtk::CssProvider::new();
        let _ = css.load_from_data(b".snip-tool-on > button, .snip-tool-on > box > button:first-child, toolbar togglebutton:checked, .snip-tool-on button:checked {
            background-image: linear-gradient(to bottom, #e6f2fa, #c9e3f6); border: 1px solid #7eb4dc; border-radius: 3px;
            box-shadow: inset 0 1px 2px rgba(0, 0, 0, 0.18); }");
        gtk::StyleContext::add_provider_for_screen(&WidgetExt::screen(&win).unwrap(), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    }
    let (iw, ih) = (shot.width(), shot.height());
    let g = util::primary_geometry();
    win.set_default_size((iw + 40).clamp(480, g.width() - 80), (ih + 110).clamp(300, g.height() - 120));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menubar = gtk::MenuBar::new();
    root.pack_start(&menubar, false, false, 0);
    let tb = gtk::Toolbar::new();
    tb.set_style(gtk::ToolbarStyle::BothHoriz);
    tb.set_icon_size(gtk::IconSize::SmallToolbar);
    // every tool stays visible (the window can't get narrower than the toolbar), as in Win7
    tb.set_show_arrow(false);
    root.pack_start(&tb, false, false, 0);
    let area = gtk::DrawingArea::new();
    area.set_size_request(iw, ih);
    area.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::BUTTON_MOTION_MASK);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    holder.set_halign(gtk::Align::Start);
    holder.set_valign(gtk::Align::Start);
    holder.set_margin_start(6);
    holder.set_margin_top(6);
    holder.pack_start(&area, false, false, 0);
    sw.add(&holder);
    root.pack_start(&sw, true, true, 0);
    win.add(&root);
    let ed = Rc::new(Editor { win: win.clone(), img: RefCell::new(shot), strokes: RefCell::new(vec![]), tool: Cell::new(Tool::Pen), pen: Cell::new(load_pen()),
        area: area.clone(), saved: Cell::new(false), drawing: Cell::new(false), tool_buttons: RefCell::new(vec![]) });
    {
        let e = ed.clone();
        area.connect_draw(move |_, cr| {
            cr.set_source_pixbuf(&e.img.borrow(), 0.0, 0.0);
            cr.paint().ok();
            draw_strokes(cr, &e.strokes.borrow());
            glib::Propagation::Proceed
        });
    }
    /// The eraser removes a whole stroke when it touches any part of it (Win7 erases per stroke).
    fn erase(e: &Editor, p: (f64, f64)) {
        let before = e.strokes.borrow().len();
        e.strokes.borrow_mut().retain(|s| {
            let r = s.width / 2.0 + 6.0;
            let near = |a: (f64, f64), b: (f64, f64)| {
                let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                let len2 = dx * dx + dy * dy;
                let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
                (a.0 + t * dx - p.0).hypot(a.1 + t * dy - p.1) < r
            };
            let hit = if s.pts.len() == 1 { near(s.pts[0], s.pts[0]) } else { s.pts.windows(2).any(|w| near(w[0], w[1])) };
            !hit
        });
        if e.strokes.borrow().len() != before { e.saved.set(false); }
        e.area.queue_draw();
    }
    {
        let e = ed.clone();
        area.connect_button_press_event(move |a, ev| {
            if ev.button() != 1 { return glib::Propagation::Proceed; }
            let p = ev.position();
            e.drawing.set(true);
            match e.tool.get() {
                Tool::Pen => { let pen = e.pen.get(); e.strokes.borrow_mut().push(Stroke { pts: vec![p], rgba: (pen.color.0, pen.color.1, pen.color.2, 1.0), width: pen.width, chisel: pen.chisel }); e.saved.set(false); }
                Tool::Highlighter => { e.strokes.borrow_mut().push(Stroke { pts: vec![p], rgba: (1.0, 0.95, 0.0, 0.45), width: 14.0, chisel: true }); e.saved.set(false); }
                Tool::Eraser => erase(&e, p),
            }
            a.queue_draw();
            glib::Propagation::Stop
        });
    }
    {
        let e = ed.clone();
        area.connect_motion_notify_event(move |a, ev| {
            if !e.drawing.get() { return glib::Propagation::Proceed; }
            let p = ev.position();
            if e.tool.get() == Tool::Eraser { erase(&e, p); } else if let Some(s) = e.strokes.borrow_mut().last_mut() { s.pts.push(p); }
            a.queue_draw();
            glib::Propagation::Stop
        });
    }
    { let e = ed.clone(); area.connect_button_release_event(move |_, _| { e.drawing.set(false); glib::Propagation::Stop }); }
    // every motion event, so ink follows the pointer smoothly (GTK merges them by default)
    { let e = ed.clone(); area.connect_realize(move |a| { if let Some(w) = a.window() { w.set_event_compression(false); } e.set_tool(e.tool.get()) }); }

    let new_snip = {
        let (a, e) = (app.clone(), ed.clone());
        // the markup window goes and the Snipping Tool window comes back in capture mode, as in Win7
        // (from an idle callback: chosen from a menu, the menu's grab must be gone before the save
        // question opens, or that dialog never shows)
        Rc::new(move |k: Kind| {
            let (a, e) = (a.clone(), e.clone());
            glib::idle_add_local_once(move || { if !e.maybe_save() { return; } e.saved.set(true); e.win.close(); main_window(&a, Some(k)); });
        })
    };
    // toolbar (Win7): New Snip ▾ | Save Snip | Copy | Send Snip ▾ | Pen ▾ | Highlighter | Eraser
    let new_b = gtk::MenuToolButton::new(Some(&util::image("accessories-screenshot", 16)), Some("New Snip"));
    new_b.set_is_important(true);
    WidgetExt::set_tooltip_text(&new_b, Some("New Snip (Ctrl+N)"));
    {
        let kinds = gtk::Menu::new();
        for k in [Kind::Free, Kind::Rect, Kind::Window, Kind::Full] {
            let it = gtk::MenuItem::with_label(k.label());
            let ns = new_snip.clone();
            it.connect_activate(move |_| { remember_kind(k); ns(k); });
            kinds.append(&it);
        }
        kinds.show_all();
        new_b.set_menu(&kinds);
        let ns = new_snip.clone();
        new_b.connect_clicked(move |_| ns(current_kind()));
    }
    tb.insert(&new_b, -1);
    let save_b = tool_button("document-save", "Save Snip", "Save Snip (Ctrl+S)", false);
    { let e = ed.clone(); save_b.connect_clicked(move |_| { e.save(); }); }
    tb.insert(&save_b, -1);
    let copy_b = tool_button("edit-copy", "Copy", "Copy (Ctrl+C)", false);
    { let e = ed.clone(); copy_b.connect_clicked(move |_| e.copy()); }
    tb.insert(&copy_b, -1);
    let send_b = gtk::MenuToolButton::new(Some(&util::image("mail-send", 16)), Some("Send Snip"));
    WidgetExt::set_tooltip_text(&send_b, Some("Send Snip"));
    {
        let m = gtk::Menu::new();
        for (l, attach) in [("E-mail recipient", false), ("E-mail recipient (as attachment)", true)] {
            let it = gtk::MenuItem::with_label(l);
            let e = ed.clone();
            it.connect_activate(move |_| e.send(attach));
            m.append(&it);
        }
        m.show_all();
        send_b.set_menu(&m);
        let e = ed.clone();
        send_b.connect_clicked(move |_| e.send(false));
    }
    tb.insert(&send_b, -1);
    tb.insert(&gtk::SeparatorToolItem::new(), -1);
    let pen = gtk::MenuToolButton::new(Some(&util::image("draw-freehand", 16)), Some("Pen"));
    WidgetExt::set_tooltip_text(&pen, Some("Pen"));
    let pen_menu = gtk::Menu::new();
    let build_pen_menu = |menu: &gtk::Menu| {
        for (name, c) in [("Red Pen", Some("Red")), ("Blue Pen", Some("Blue")), ("Black Pen", Some("Black")), ("Custom Pen", None)] {
            let it = gtk::MenuItem::with_label(name);
            let e = ed.clone();
            it.connect_activate(move |_| {
                match c { Some(n) => e.pen.set(Pen { color: color_named(n), width: 2.0, chisel: false }), None => e.pen.set(load_pen()) }
                e.set_tool(Tool::Pen);
            });
            menu.append(&it);
        }
        menu.append(&gtk::SeparatorMenuItem::new());
        let it = gtk::MenuItem::with_mnemonic("_Customize...");
        let e = ed.clone();
        it.connect_activate(move |_| {
            let e = e.clone();
            glib::idle_add_local_once(move || if let Some((p, s)) = customize_pen(e.win.upcast_ref(), load_pen()) { set_opt_str("pen", &s); e.pen.set(p); e.set_tool(Tool::Pen); });
        });
        menu.append(&it);
    };
    build_pen_menu(&pen_menu);
    pen_menu.show_all();
    pen.set_menu(&pen_menu);
    { let e = ed.clone(); pen.connect_clicked(move |_| e.set_tool(Tool::Pen)); }
    tb.insert(&pen, -1);
    let hl = gtk::ToggleToolButton::new();
    hl.set_icon_widget(Some(&util::image("format-text-highlight", 16)));
    hl.set_label(Some("Highlighter"));
    WidgetExt::set_tooltip_text(&hl, Some("Highlighter"));
    { let e = ed.clone(); hl.connect_clicked(move |b| { if b.is_active() { e.set_tool(Tool::Highlighter) } else if e.tool.get() == Tool::Highlighter { e.set_tool(Tool::Pen) } }); }
    tb.insert(&hl, -1);
    let er = gtk::ToggleToolButton::new();
    er.set_icon_widget(Some(&util::image("draw-eraser", 16)));
    er.set_label(Some("Eraser"));
    WidgetExt::set_tooltip_text(&er, Some("Eraser"));
    { let e = ed.clone(); er.connect_clicked(move |b| { if b.is_active() { e.set_tool(Tool::Eraser) } else if e.tool.get() == Tool::Eraser { e.set_tool(Tool::Pen) } }); }
    tb.insert(&er, -1);
    ed.tool_buttons.borrow_mut().extend([(Tool::Pen, pen.clone().upcast::<gtk::ToolItem>()), (Tool::Highlighter, hl.upcast::<gtk::ToolItem>()), (Tool::Eraser, er.upcast::<gtk::ToolItem>())]);
    ed.set_tool(Tool::Pen);
    // menus
    let accel = gtk::AccelGroup::new();
    win.add_accel_group(&accel);
    let add = |m: &gtk::Menu, l: &str, acc: &str, f: Box<dyn Fn()>| {
        let it = gtk::MenuItem::with_mnemonic(l);
        if !acc.is_empty() { let (k, md) = gtk::accelerator_parse(acc); it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE); }
        // after the menu has closed: its grab would keep a dialog opened from here from showing
        let f: Rc<dyn Fn()> = Rc::from(f);
        it.connect_activate(move |_| { let f = f.clone(); glib::idle_add_local_once(move || f()); });
        m.append(&it);
        it
    };
    let m = gtk::Menu::new();
    { let ns = new_snip.clone(); add(&m, "_New Snip", "<Control>n", Box::new(move || ns(current_kind()))); }
    { let e = ed.clone(); add(&m, "Save _As...", "<Control>s", Box::new(move || { e.save(); })); }
    m.append(&gtk::SeparatorMenuItem::new());
    let sm = gtk::Menu::new();
    { let e = ed.clone(); add(&sm, "_E-mail recipient", "", Box::new(move || e.send(false))); }
    { let e = ed.clone(); add(&sm, "E-mail recipient (as _attachment)", "", Box::new(move || e.send(true))); }
    let st = gtk::MenuItem::with_mnemonic("Sen_d To"); st.set_submenu(Some(&sm)); m.append(&st);
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); add(&m, "E_xit", "", Box::new(move || w.close())); }
    let top = gtk::MenuItem::with_mnemonic("_File"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    { let e = ed.clone(); add(&m, "_Copy", "<Control>c", Box::new(move || e.copy())); }
    let top = gtk::MenuItem::with_mnemonic("_Edit"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    let pm = gtk::Menu::new();
    build_pen_menu(&pm);
    let pi = gtk::MenuItem::with_mnemonic("_Pen"); pi.set_submenu(Some(&pm)); m.append(&pi);
    { let e = ed.clone(); add(&m, "_Highlighter", "", Box::new(move || e.set_tool(Tool::Highlighter))); }
    { let e = ed.clone(); add(&m, "_Eraser", "", Box::new(move || e.set_tool(Tool::Eraser))); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); add(&m, "_Options...", "", Box::new(move || options(w.upcast_ref()))); }
    let top = gtk::MenuItem::with_mnemonic("_Tools"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    add(&m, "Snipping Tool _Help", "F1", Box::new(|| { util::open_uri("https://wiki.archlinux.org/title/Screen_capture"); }));
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); add(&m, "_About Snipping Tool", "", Box::new(move || {
        let d = gtk::AboutDialog::new(); d.set_transient_for(Some(&w)); d.set_program_name("Snipping Tool");
        d.set_comments(Some("Aerode Snipping Tool – capture and annotate screen snips"));
        d.set_logo_icon_name(Some(util::first_icon(&["accessories-screenshot"]))); d.run(); d.close();
    })); }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);
    { let e = ed.clone(); win.connect_delete_event(move |_, _| if e.maybe_save() { glib::Propagation::Proceed } else { glib::Propagation::Stop }); }
    // Win7 copies every snip to the clipboard by default
    if opt("copy", true) { ed.copy(); }
    if let Some(m) = main { m.close(); }
    win.show_all();
    win.present();
}

fn current_kind() -> Kind {
    match std::fs::read_to_string(config::conf_dir().join("snip.kind")).unwrap_or_default().trim() {
        "free" => Kind::Free, "window" => Kind::Window, "full" => Kind::Full, _ => Kind::Rect,
    }
}

fn remember_kind(k: Kind) {
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(config::conf_dir().join("snip.kind"), match k { Kind::Free => "free", Kind::Rect => "rect", Kind::Window => "window", Kind::Full => "full" });
}

/// Win7's Snipping Tool Options dialog.
fn options(parent: &gtk::Window) {
    let d = gtk::Dialog::with_buttons(Some("Snipping Tool Options"), Some(parent), gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_resizable(false);
    d.set_default_response(gtk::ResponseType::Ok);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(8);
    let frame = |title: &str| { let f = gtk::Frame::new(Some(title)); let b = gtk::Box::new(gtk::Orientation::Vertical, 4); b.set_border_width(8); f.add(&b); c.pack_start(&f, false, false, 0); b };
    let app = frame("Application");
    // (key, label, default, inverted: the stored option is the opposite of the box)
    let items = [("instr", "Hide instruction text", true, true), ("copy", "Always copy snips to the Clipboard", true, false),
        ("url", "Include URL below snips (HTML only)", true, false),
        ("prompt", "Prompt to save snips before exiting", true, false), ("overlay", "Show screen overlay when Snipping Tool is active", true, false)];
    let checks: Vec<(&str, bool, gtk::CheckButton)> = items.iter().map(|(k, l, def, inv)| {
        let b = gtk::CheckButton::with_label(l);
        b.set_active(opt(k, *def) != *inv);
        app.pack_start(&b, false, false, 0);
        (*k, *inv, b)
    }).collect();
    let sel = frame("Selections");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.pack_start(&gtk::Label::new(Some("Ink color:")), false, false, 0);
    let ink = gtk::ComboBoxText::new();
    for (n, _) in COLORS { ink.append(Some(n), n); }
    ink.set_active_id(Some(&opt_str("ink_color").unwrap_or_else(|| "Red".into())));
    row.pack_start(&ink, false, false, 0);
    sel.pack_start(&row, false, false, 0);
    let show_ink = gtk::CheckButton::with_label("Show selection ink after snips are captured");
    show_ink.set_active(opt("ink", true));
    sel.pack_start(&show_ink, false, false, 0);
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        for (k, inv, b) in checks { set_opt(k, b.is_active() != inv); }
        set_opt("ink", show_ink.is_active());
        if let Some(id) = ink.active_id() { set_opt_str("ink_color", &id); }
    }
    d.close();
}

/// Where a finished snip goes.
#[derive(Clone, Copy, PartialEq)]
enum Dest { Editor, Clipboard }

/// Begin a snip: hide our windows, grab the screen, let the user select. The app is held until the
/// selection ends: "New Snip" in the editor has already closed the last window, the quick modes have none.
fn start(app: &gtk::Application, kind: Kind, main: Option<gtk::Window>, dest: Dest) {
    // a new snip type chosen while selecting: start over with it
    cancel_active();
    if let Some(m) = &main { m.hide(); }
    let (app, hold) = (app.clone(), app.hold());
    // our own windows need a moment to leave the screen; Win+Shift+S freezes the screen at once
    let delay = if dest == Dest::Clipboard { 0 } else { 250 };
    glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
        let Some(shot) = capture_screen() else { if let Some(m) = &main { m.show(); } return };
        let (a, m, hold) = (app.clone(), main.clone(), RefCell::new(Some(hold)));
        select(kind, shot, dest == Dest::Editor, Rc::new(move |res| {
            match res {
                Some(img) if dest == Dest::Clipboard => copy_to_clipboard(&img),
                Some(img) => open_editor(&a, img, m.clone()),
                None => if let Some(m) = &m { m.show(); },
            }
            drop(hold.borrow_mut().take());
        }));
    });
}

/// The Snipping Tool window: New ▾ | Cancel | Options and the instruction text. `capture`: start a snip
/// of that type right away - Win7 opens the tool in capture mode, and New Snip in the markup window
/// comes back here.
fn main_window(app: &gtk::Application, capture: Option<Kind>) {
    theme::init_app();
    let win = gtk::ApplicationWindow::new(app);
    win.set_title("Snipping Tool");
    win.set_resizable(false);
    win.set_icon_name(Some(util::first_icon(&["accessories-screenshot"])));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let tb = gtk::Toolbar::new();
    tb.set_style(gtk::ToolbarStyle::BothHoriz);
    let new_b = gtk::MenuToolButton::new(Some(&util::image("accessories-screenshot", 16)), Some("New"));
    new_b.set_is_important(true);
    WidgetExt::set_tooltip_text(&new_b, Some("Create a new snip (Ctrl+N)"));
    let kinds = gtk::Menu::new();
    let mut group: Option<gtk::RadioMenuItem> = None;
    for k in [Kind::Free, Kind::Rect, Kind::Window, Kind::Full] {
        let it = match &group { None => gtk::RadioMenuItem::with_label(k.label()), Some(g) => gtk::RadioMenuItem::with_label_from_widget(g, Some(k.label())) };
        if group.is_none() { group = Some(it.clone()); }
        it.set_active(current_kind() == k);
        let (a, w) = (app.clone(), win.clone());
        it.connect_activate(move |i| {
            if !i.is_active() { return; }
            remember_kind(k);
            start(&a, k, Some(w.clone().upcast()), Dest::Editor);
        });
        kinds.append(&it);
    }
    kinds.show_all();
    new_b.set_menu(&kinds);
    { let (a, w) = (app.clone(), win.clone()); new_b.connect_clicked(move |_| start(&a, current_kind(), Some(w.clone().upcast()), Dest::Editor)); }
    tb.insert(&new_b, -1);
    let cancel = gtk::ToolButton::new(None::<&gtk::Widget>, Some("Cancel"));
    cancel.set_is_important(true);
    cancel.set_sensitive(false);
    WidgetExt::set_tooltip_text(&cancel, Some("Cancel snip (Esc)"));
    cancel.connect_clicked(|_| cancel_active());
    tb.insert(&cancel, -1);
    let opts = gtk::ToolButton::new(None::<&gtk::Widget>, Some("Options"));
    opts.set_is_important(true);
    WidgetExt::set_tooltip_text(&opts, Some("Snipping Tool Options"));
    tb.insert(&opts, -1);
    root.pack_start(&tb, false, false, 0);
    let instr = gtk::Label::new(Some(IDLE_TEXT));
    instr.set_margin_start(12);
    instr.set_margin_end(12);
    instr.set_margin_top(10);
    instr.set_margin_bottom(12);
    instr.set_xalign(0.0);
    instr.set_width_chars(52);
    instr.set_lines(2);
    root.pack_start(&instr, false, false, 0);
    {
        let (w, instr) = (win.clone(), instr.clone());
        opts.connect_clicked(move |_| { cancel_active(); options(w.upcast_ref()); instr.set_visible(opt("instr", true)); w.resize(1, 1); });
    }
    // Ctrl+N: New, Esc: Cancel
    {
        let (a, w) = (app.clone(), win.clone());
        win.connect_key_press_event(move |_, e| {
            let ctrl = e.state().contains(gdk::ModifierType::CONTROL_MASK);
            if ctrl && (e.keyval() == gdk::keys::constants::n || e.keyval() == gdk::keys::constants::N) { start(&a, current_kind(), Some(w.clone().upcast()), Dest::Editor); return glib::Propagation::Stop; }
            if e.keyval() == gdk::keys::constants::Escape { cancel_active(); return glib::Propagation::Stop; }
            glib::Propagation::Proceed
        });
    }
    win.connect_delete_event(|_, _| { cancel_active(); glib::Propagation::Proceed });
    win.connect_destroy(|w| MAIN.with(|m| { if m.borrow().as_ref().is_some_and(|x| x.win.upcast_ref::<gtk::Widget>() == w.upcast_ref::<gtk::Widget>()) { m.borrow_mut().take(); } }));
    win.add(&root);
    // the window keeps its size when the text changes to a one-line capture instruction
    instr.set_size_request(-1, instr.preferred_height().1);
    win.show_all();
    instr.set_visible(opt("instr", true));
    MAIN.with(|m| *m.borrow_mut() = Some(MainUi { win: win.clone(), cancel, instr }));
    if let Some(k) = capture { start(app, k, Some(win.upcast()), Dest::Editor); }
}

fn main() -> glib::ExitCode {
    util::log_panics("snip");
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let (full, clip, rect) = (has("--full"), has("--clip"), has("--rect"));
    let app = gtk::Application::new(Some("org.aerode.SnippingTool"), gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(move |app| {
        if full {
            // Print Screen: whole screen → clipboard (and Pictures/Screenshots), like Win7 PrtScn
            let Some(shot) = capture_screen() else { return };
            let dir = util::special_dir(glib::UserDirectory::Pictures, "Pictures").join("Screenshots");
            let _ = std::fs::create_dir_all(&dir);
            let _ = shot.savev(dir.join(chrono::Local::now().format("Screenshot %Y-%m-%d %H.%M.%S.png").to_string()), "png", &[]);
            copy_to_clipboard(&shot);
            return;
        }
        theme::init_app();
        if clip || rect {
            start(app, Kind::Rect, None, if clip { Dest::Clipboard } else { Dest::Editor });
        } else {
            main_window(app, Some(current_kind()));
        }
    });
    app.run_with_args::<&str>(&[])
}

#[cfg(test)]
mod tests {
    #[test]
    fn lzw_round_trip_shape() {
        // a run of one colour compresses to far fewer bytes than its length
        let data = vec![7u8; 10_000];
        let out = super::lzw(&data, 8);
        assert!(out.len() < 400, "{}", out.len());
        // first code is CLEAR (256) in 9 bits: low byte 0x00, bit 8 set in the next byte
        assert_eq!(out[0], 0x00);
        assert_eq!(out[1] & 1, 1);
    }
}
