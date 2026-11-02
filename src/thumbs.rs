//! Windows 7 taskbar thumbnails and Aero Peek.
//!
//! Pointing at a running taskbar button for a moment shows a glass popup above it with a live
//! thumbnail of each of its windows (title and icon above, a red close button on the one under
//! the pointer). Resting on a thumbnail peeks at that window (every other window fades to a glass
//! outline, done by aerode-wm on `_AERODE_PEEK`); a click switches to it, a middle click closes it.
//! Too many windows for the screen's width turn into Win7's list of titles.
//!
//! Thumbnails come from the compositor's off-screen pixmap of each window (XComposite
//! NameWindowPixmap of the WM frame), so they work for covered windows; without a compositor a
//! visible window is copied from the screen, and a minimised one shows its last thumbnail or its
//! icon.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::cairo;
use gtk::gdk;
use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use gtk::prelude::*;
use x11rb::connection::Connection;
use x11rb::protocol::composite::ConnectionExt as _;
use x11rb::protocol::xproto::{self, ConnectionExt as _};

use crate::glass;

/// Win7 defaults at 96 dpi: thumbnails fit 200 × 120, the popup appears after 400 ms (the
/// Extended UI hover time) and Aero Peek after 500 ms on a thumbnail.
const THUMB_W: f64 = 200.0;
const THUMB_H: f64 = 120.0;
const PAD: f64 = 8.0;
const HEADER: f64 = 24.0;
const GAP: f64 = 4.0;
const SHOW_DELAY: u64 = 400;
const HIDE_DELAY: u64 = 300;
const PEEK_DELAY: u64 = 500;
const LIST_ROW: f64 = 26.0;
const LIST_W: f64 = 300.0;

#[derive(Clone)]
pub struct Win {
    pub id: u32,
    pub title: String,
    pub icon: Option<Pixbuf>,
    pub minimized: bool,
}

struct State {
    popup: gtk::Window,
    area: gtk::DrawingArea,
    wm: RefCell<Option<Rc<crate::x11::Wm>>>,
    wins: RefCell<Vec<Win>>,
    anchor: RefCell<Option<gtk::Widget>>,
    /// item under the pointer, and whether it is on its close button
    hot: Cell<Option<usize>>,
    hot_close: Cell<bool>,
    list: Cell<bool>,
    show_src: RefCell<Option<glib::SourceId>>,
    hide_src: RefCell<Option<glib::SourceId>>,
    peek_src: RefCell<Option<glib::SourceId>>,
    live_src: RefCell<Option<glib::SourceId>>,
    peeking: Cell<bool>,
    /// last thumbnail of every window (minimised windows have no live image)
    cache: RefCell<HashMap<u32, cairo::ImageSurface>>,
    x: RefCell<Option<X>>,
}

struct X {
    conn: x11rb::rust_connection::RustConnection,
    root: u32,
    check_atom: u32,
    peek_atom: u32,
    /// XRender picture formats (rgb24, argb32): thumbnails are scaled on the server
    formats: Option<(u32, u32)>,
}

/// The standard RGB24 and ARGB32 picture formats.
fn render_formats(conn: &x11rb::rust_connection::RustConnection) -> Option<(u32, u32)> {
    use x11rb::protocol::render::{ConnectionExt as _, PictType};
    let r = conn.render_query_pict_formats().ok()?.reply().ok()?;
    let find = |depth: u8, alpha: u16| r.formats.iter().find(|f| f.type_ == PictType::DIRECT && f.depth == depth
        && f.direct.red_shift == 16 && f.direct.green_shift == 8 && f.direct.blue_shift == 0 && f.direct.alpha_mask == alpha).map(|f| f.id);
    Some((find(24, 0)?, find(32, 0xff)?))
}

thread_local! {
    static STATE: RefCell<Option<Rc<State>>> = const { RefCell::new(None) };
}

fn cancel(src: &RefCell<Option<glib::SourceId>>) {
    if let Some(s) = src.borrow_mut().take() { s.remove(); }
}

fn state() -> Rc<State> {
    if let Some(s) = STATE.with(|s| s.borrow().clone()) { return s; }
    let popup = gtk::Window::new(gtk::WindowType::Popup);
    popup.set_type_hint(gdk::WindowTypeHint::Tooltip);
    popup.set_app_paintable(true);
    glass::enable_rgba(&popup);
    let area = gtk::DrawingArea::new();
    area.add_events(gdk::EventMask::POINTER_MOTION_MASK | gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK
        | gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
    popup.add(&area);
    let x = x11rb::connect(None).ok().and_then(|(conn, scr)| {
        let root = conn.setup().roots[scr].root;
        let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).ok()?.reply().ok().map(|r| r.atom);
        let (check_atom, peek_atom) = (atom("_NET_SUPPORTING_WM_CHECK")?, atom("_AERODE_PEEK")?);
        let formats = render_formats(&conn);
        Some(X { conn, root, check_atom, peek_atom, formats })
    });
    let st = Rc::new(State {
        popup, area, wm: RefCell::new(None), wins: RefCell::new(vec![]), anchor: RefCell::new(None),
        hot: Cell::new(None), hot_close: Cell::new(false), list: Cell::new(false),
        show_src: RefCell::new(None), hide_src: RefCell::new(None), peek_src: RefCell::new(None), live_src: RefCell::new(None),
        peeking: Cell::new(false), cache: RefCell::new(HashMap::new()), x: RefCell::new(x),
    });
    {
        let s = st.clone();
        st.area.connect_draw(move |a, cr| { draw(&s, a, cr); glib::Propagation::Stop });
    }
    {
        let s = st.clone();
        st.area.connect_motion_notify_event(move |_, e| { pointer(&s, e.position()); glib::Propagation::Proceed });
    }
    {
        let s = st.clone();
        st.area.connect_enter_notify_event(move |_, _| { cancel(&s.hide_src); glib::Propagation::Proceed });
    }
    {
        let s = st.clone();
        st.area.connect_leave_notify_event(move |_, e| {
            if e.detail() == gdk::NotifyType::Inferior { return glib::Propagation::Proceed; }
            set_hot(&s, None, false);
            schedule_hide(&s);
            glib::Propagation::Proceed
        });
    }
    {
        let s = st.clone();
        st.area.connect_button_release_event(move |_, e| { click(&s, e.position(), e.button()); glib::Propagation::Stop });
    }
    STATE.with(|x| *x.borrow_mut() = Some(st.clone()));
    st
}

// ---------------------------------------------------------------- public API (the taskbar)

/// The pointer entered a running taskbar button.
pub fn hover(wm: &Rc<crate::x11::Wm>, anchor: &gtk::Widget, wins: Vec<Win>) {
    let st = state();
    cancel(&st.hide_src);
    cancel(&st.show_src);
    if wins.is_empty() { hide(); return; }
    *st.wm.borrow_mut() = Some(wm.clone());
    if st.popup.is_visible() {
        // moving along the taskbar while a popup is open switches at once
        open(&st, anchor, wins);
        return;
    }
    let (s, a) = (st.clone(), anchor.clone());
    *st.show_src.borrow_mut() = Some(glib::timeout_add_local_once(std::time::Duration::from_millis(SHOW_DELAY), move || {
        s.show_src.borrow_mut().take();
        open(&s, &a, wins);
    }));
}

/// Show the thumbnails right away (a click on a button that groups several windows).
pub fn show_now(wm: &Rc<crate::x11::Wm>, anchor: &gtk::Widget, wins: Vec<Win>) {
    let st = state();
    cancel(&st.hide_src);
    cancel(&st.show_src);
    *st.wm.borrow_mut() = Some(wm.clone());
    open(&st, anchor, wins);
}

/// The pointer left the taskbar button.
pub fn leave() {
    let Some(st) = STATE.with(|s| s.borrow().clone()) else { return };
    cancel(&st.show_src);
    if st.popup.is_visible() { schedule_hide(&st); }
}

pub fn hide() {
    let Some(st) = STATE.with(|s| s.borrow().clone()) else { return };
    cancel(&st.show_src);
    cancel(&st.hide_src);
    cancel(&st.live_src);
    set_hot(&st, None, false);
    peek(&st, None);
    st.popup.hide();
    st.anchor.borrow_mut().take();
}

pub fn visible_for(anchor: &gtk::Widget) -> bool {
    STATE.with(|s| s.borrow().as_ref().map(|st| st.popup.is_visible() && st.anchor.borrow().as_ref() == Some(anchor)).unwrap_or(false))
}

/// Aero Peek at the desktop (the Show desktop button): every window fades to its outline.
pub fn peek_desktop(on: bool) {
    let st = state();
    send_peek(&st, 0, on);
    st.peeking.set(on);
}

// ---------------------------------------------------------------- behaviour

fn schedule_hide(st: &Rc<State>) {
    cancel(&st.hide_src);
    let s = st.clone();
    *st.hide_src.borrow_mut() = Some(glib::timeout_add_local_once(std::time::Duration::from_millis(HIDE_DELAY), move || {
        s.hide_src.borrow_mut().take();
        hide();
    }));
}

fn send_peek(st: &State, target: u32, on: bool) {
    let x = st.x.borrow();
    let Some(x) = x.as_ref() else { return };
    let check = x.conn.get_property(false, x.root, x.check_atom, xproto::AtomEnum::WINDOW, 0, 1).ok()
        .and_then(|c| c.reply().ok()).and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0);
    if check == 0 { return; }
    let ev = xproto::ClientMessageEvent::new(32, check, x.peek_atom, [target, on as u32, 0, 0, 0]);
    let _ = x.conn.send_event(false, check, xproto::EventMask::NO_EVENT, ev);
    let _ = x.conn.flush();
}

/// Peek at `win` (None: stop peeking).
fn peek(st: &Rc<State>, win: Option<u32>) {
    cancel(&st.peek_src);
    match win {
        Some(w) => send_peek(st, w, true),
        None => if st.peeking.get() { send_peek(st, 0, false) },
    }
    st.peeking.set(win.is_some());
}

fn set_hot(st: &Rc<State>, hot: Option<usize>, close: bool) {
    if st.hot.get() == hot && st.hot_close.get() == close { return; }
    let changed_item = st.hot.get() != hot;
    st.hot.set(hot);
    st.hot_close.set(close);
    st.area.queue_draw();
    if !changed_item { return; }
    // Aero Peek follows the thumbnail under the pointer after a short rest
    cancel(&st.peek_src);
    match hot.and_then(|i| st.wins.borrow().get(i).cloned()) {
        Some(w) if !w.minimized => {
            if st.peeking.get() { peek(st, Some(w.id)); return; }
            let s = st.clone();
            *st.peek_src.borrow_mut() = Some(glib::timeout_add_local_once(std::time::Duration::from_millis(PEEK_DELAY), move || {
                s.peek_src.borrow_mut().take();
                peek(&s, Some(w.id));
            }));
        }
        _ => peek(st, None),
    }
}

fn open(st: &Rc<State>, anchor: &gtk::Widget, wins: Vec<Win>) {
    for w in &wins { if !w.minimized { capture(st, w.id); } }
    *st.wins.borrow_mut() = wins;
    *st.anchor.borrow_mut() = Some(anchor.clone());
    st.hot.set(None);
    let (w, h) = layout_size(st, anchor);
    st.area.set_size_request(w as i32, h as i32);
    st.popup.resize(w as i32, h as i32);
    // centred above the button, on its monitor
    let (ax, ay) = anchor.window().map(|gw| { let (_, x, y) = gw.origin(); (x, y) }).unwrap_or((0, 0));
    let al = anchor.allocation();
    let (bx, by) = if anchor.has_window() { (ax, ay) } else { (ax + al.x(), ay + al.y()) };
    let mon = anchor.display().monitor_at_point(bx + al.width() / 2, by).map(|m| m.geometry());
    let (mx, mw) = mon.map(|g| (g.x(), g.width())).unwrap_or((0, 1920));
    let x = (bx + al.width() / 2 - w as i32 / 2).clamp(mx + 2, (mx + mw - w as i32 - 2).max(mx + 2));
    let y = by - h as i32 - 4;
    st.popup.move_(x, y);
    st.popup.show_all();
    st.area.queue_draw();
    // live: refresh the thumbnails while the popup is up
    if st.live_src.borrow().is_none() {
        let s = st.clone();
        *st.live_src.borrow_mut() = Some(glib::timeout_add_local(std::time::Duration::from_millis(1000), move || {
            if !s.popup.is_visible() { s.live_src.borrow_mut().take(); return glib::ControlFlow::Break; }
            let ids: Vec<u32> = s.wins.borrow().iter().filter(|w| !w.minimized).map(|w| w.id).collect();
            if !s.list.get() { for id in ids { capture(&s, id); } }
            s.area.queue_draw();
            glib::ControlFlow::Continue
        }));
    }
}

fn layout_size(st: &State, anchor: &gtk::Widget) -> (f64, f64) {
    let n = st.wins.borrow().len().max(1) as f64;
    let screen_w = anchor.display().monitor_at_window(&anchor.window().unwrap_or_else(|| gdk::Window::default_root_window()))
        .map(|m| m.geometry().width() as f64).unwrap_or(1920.0);
    let cell_w = THUMB_W + 2.0 * PAD;
    let thumbs_w = n * cell_w + (n + 1.0) * GAP;
    let list = thumbs_w > screen_w - 40.0;
    st.list.set(list);
    if list { (LIST_W, n * LIST_ROW + 2.0 * PAD) } else { (thumbs_w, HEADER + THUMB_H + 2.0 * PAD + 2.0 * GAP) }
}

/// (cell rect, thumbnail area, close button) of item `i` in thumbnail mode.
fn cell(i: usize) -> ([f64; 4], [f64; 4], [f64; 4]) {
    let cw = THUMB_W + 2.0 * PAD;
    let x = GAP + i as f64 * (cw + GAP);
    let cellr = [x, GAP, cw, HEADER + THUMB_H + 2.0 * PAD];
    let thumb = [x + PAD, GAP + PAD + HEADER, THUMB_W, THUMB_H];
    let close = [x + cw - PAD - 14.0, GAP + PAD + 2.0, 14.0, 14.0];
    (cellr, thumb, close)
}

fn hit(st: &State, (px, py): (f64, f64)) -> Option<(usize, bool)> {
    let n = st.wins.borrow().len();
    if st.list.get() {
        let i = ((py - PAD) / LIST_ROW).floor();
        if i < 0.0 || i as usize >= n || px < 0.0 { return None; }
        return Some((i as usize, px > LIST_W - PAD - 18.0));
    }
    (0..n).find_map(|i| {
        let (c, _, x) = cell(i);
        let inside = |r: [f64; 4]| px >= r[0] && py >= r[1] && px < r[0] + r[2] && py < r[1] + r[3];
        inside(c).then(|| (i, inside(x)))
    })
}

fn pointer(st: &Rc<State>, pos: (f64, f64)) {
    match hit(st, pos) {
        Some((i, close)) => set_hot(st, Some(i), close),
        None => set_hot(st, None, false),
    }
}

fn click(st: &Rc<State>, pos: (f64, f64), button: u32) {
    let Some((i, close)) = hit(st, pos) else { return };
    let Some(w) = st.wins.borrow().get(i).cloned() else { return };
    let Some(wm) = st.wm.borrow().clone() else { return };
    if close || button == 2 {
        // close it and drop it from the popup (Win7 keeps the popup for the others)
        wm.close(w.id);
        st.wins.borrow_mut().retain(|x| x.id != w.id);
        st.cache.borrow_mut().remove(&w.id);
        peek(st, None);
        let left = st.wins.borrow().clone();
        match st.anchor.borrow().clone() {
            Some(a) if !left.is_empty() => open(st, &a, left),
            _ => hide(),
        }
    } else if button == 1 {
        hide();
        wm.activate(w.id);
    }
}

// ---------------------------------------------------------------- capture

/// Grab the window's current image into the thumbnail cache.
fn capture(st: &State, win: u32) {
    let x = st.x.borrow();
    let Some(x) = x.as_ref() else { return };
    // the WM frame (the top-level) has the glass caption too, like Win7's thumbnails
    let top = x.conn.query_tree(win).ok().and_then(|c| c.reply().ok())
        .map(|t| if t.parent != t.root && t.parent != 0 { t.parent } else { win }).unwrap_or(win);
    let Some(g) = x.conn.get_geometry(top).ok().and_then(|c| c.reply().ok()) else { return };
    let (w, h) = (g.width as i32, g.height as i32);
    if w < 2 || h < 2 { return; }
    // with a compositor every top-level has an off-screen pixmap; without one, copy the screen
    let k = (THUMB_W / w as f64).min(THUMB_H / h as f64).min(1.0);
    let (tw, th) = ((w as f64 * k).round().max(1.0) as i32, (h as f64 * k).round().max(1.0) as i32);
    if let Some(small) = capture_scaled(x, top, g.depth, w, h, tw, th) {
        st.cache.borrow_mut().insert(win, small);
        return;
    }
    let pix = x.conn.generate_id().ok();
    let named = pix.map(|p| x.conn.composite_name_window_pixmap(top, p).ok().map(|c| c.check().is_ok()).unwrap_or(false)).unwrap_or(false);
    let drawable = if named { pix.unwrap() } else { top };
    let img = x.conn.get_image(xproto::ImageFormat::Z_PIXMAP, drawable, 0, 0, w as u16, h as u16, !0).ok().and_then(|c| c.reply().ok());
    if named { let _ = x.conn.free_pixmap(pix.unwrap()); }
    let _ = x.conn.flush();
    let Some(img) = img else { return };
    let argb = img.depth == 32;
    let fmt = if argb { cairo::Format::ARgb32 } else { cairo::Format::Rgb24 };
    let stride = (w * 4) as usize;
    if img.data.len() < stride * h as usize { return; }
    let Ok(full) = cairo::ImageSurface::create_for_data(img.data, fmt, w, h, stride as i32) else { return };
    // scale into the 200 × 120 box once, here, instead of on every draw
    let Ok(small) = cairo::ImageSurface::create(cairo::Format::ARgb32, tw, th) else { return };
    {
        let Ok(cr) = cairo::Context::new(&small) else { return };
        cr.scale(k, k);
        cr.set_source_surface(&full, 0.0, 0.0).ok();
        cr.source().set_filter(cairo::Filter::Good);
        cr.paint().ok();
    }
    st.cache.borrow_mut().insert(win, small);
}

/// The window scaled to tw × th by the X server (XRender, "good" filter), so only the thumbnail
/// crosses the wire – a 4K window would otherwise be 33 MB per refresh.
fn capture_scaled(x: &X, top: u32, depth: u8, w: i32, h: i32, tw: i32, th: i32) -> Option<cairo::ImageSurface> {
    use x11rb::protocol::render::{ConnectionExt as _, CreatePictureAux, PictOp, Transform};
    let (rgb24, argb32) = x.formats?;
    let c = &x.conn;
    let src_fmt = match depth { 32 => argb32, 24 => rgb24, _ => return None };
    // the compositor's pixmap when there is one, else the window itself with its children
    let pix = c.generate_id().ok()?;
    let named = c.composite_name_window_pixmap(top, pix).ok().map(|ck| ck.check().is_ok()).unwrap_or(false);
    let src_draw = if named { pix } else { top };
    let (src, dst, dst_pix) = (c.generate_id().ok()?, c.generate_id().ok()?, c.generate_id().ok()?);
    let fx = |v: f64| (v * 65536.0).round() as i32;
    let (sx, sy) = (w as f64 / tw as f64, h as f64 / th as f64);
    let ok = (|| -> Option<()> {
        c.render_create_picture(src, src_draw, src_fmt, &CreatePictureAux::new().subwindowmode(xproto::SubwindowMode::INCLUDE_INFERIORS)).ok()?.check().ok()?;
        c.render_set_picture_transform(src, Transform { matrix11: fx(sx), matrix12: 0, matrix13: 0, matrix21: 0, matrix22: fx(sy), matrix23: 0, matrix31: 0, matrix32: 0, matrix33: fx(1.0) }).ok()?;
        c.render_set_picture_filter(src, b"good", &[]).ok()?;
        c.create_pixmap(32, dst_pix, x.root, tw as u16, th as u16).ok()?.check().ok()?;
        c.render_create_picture(dst, dst_pix, argb32, &CreatePictureAux::new()).ok()?;
        c.render_composite(PictOp::SRC, src, x11rb::NONE, dst, 0, 0, 0, 0, 0, 0, tw as u16, th as u16).ok()?;
        Some(())
    })();
    let img = ok.and_then(|_| c.get_image(xproto::ImageFormat::Z_PIXMAP, dst_pix, 0, 0, tw as u16, th as u16, !0).ok()?.reply().ok());
    let _ = c.render_free_picture(src);
    let _ = c.render_free_picture(dst);
    let _ = c.free_pixmap(dst_pix);
    if named { let _ = c.free_pixmap(pix); }
    let _ = c.flush();
    let img = img?;
    let stride = (tw * 4) as usize;
    if img.data.len() < stride * th as usize { return None; }
    let mut data = img.data;
    // a 24-bit source leaves the alpha byte undefined: make it opaque
    if depth != 32 { for px in data.chunks_exact_mut(4) { px[3] = 255; } }
    cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, tw, th, stride as i32).ok()
}

// ---------------------------------------------------------------- drawing

fn draw(st: &State, area: &gtk::DrawingArea, cr: &cairo::Context) {
    let (w, h) = (area.allocated_width() as f64, area.allocated_height() as f64);
    let (sx, sy) = area.window().map(|gw| { let (_, x, y) = gw.origin(); (x as f64, y as f64) }).unwrap_or((0.0, 0.0));
    glass::paint_glass(cr, w, h, sx, sy, 6.0, true);
    let wins = st.wins.borrow();
    let layout = pangocairo::functions::create_layout(cr);
    let mut fd = gtk::pango::FontDescription::from_string("Segoe UI 9");
    fd.set_absolute_size(12.0 * gtk::pango::SCALE as f64);
    layout.set_font_description(Some(&fd));
    layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if st.list.get() {
        for (i, win) in wins.iter().enumerate() {
            let y = PAD + i as f64 * LIST_ROW;
            if st.hot.get() == Some(i) { highlight(cr, PAD / 2.0, y, w - PAD, LIST_ROW); }
            icon(cr, win.icon.as_ref(), PAD + 4.0, y + (LIST_ROW - 16.0) / 2.0);
            layout.set_width(((w - 2.0 * PAD - 50.0) * gtk::pango::SCALE as f64) as i32);
            title(cr, &layout, &win.title, PAD + 26.0, y + (LIST_ROW - 15.0) / 2.0);
            if st.hot.get() == Some(i) { close_button(cr, [w - PAD - 18.0, y + (LIST_ROW - 14.0) / 2.0, 14.0, 14.0], st.hot_close.get()); }
        }
        return;
    }
    let cache = st.cache.borrow();
    for (i, win) in wins.iter().enumerate() {
        let (c, t, x) = cell(i);
        if st.hot.get() == Some(i) { highlight(cr, c[0], c[1], c[2], c[3]); }
        icon(cr, win.icon.as_ref(), c[0] + PAD, c[1] + PAD + 2.0);
        layout.set_width(((c[2] - 2.0 * PAD - 22.0 - 18.0) * gtk::pango::SCALE as f64) as i32);
        title(cr, &layout, &win.title, c[0] + PAD + 22.0, c[1] + PAD + 2.0);
        match cache.get(&win.id) {
            Some(img) => {
                let (iw, ih) = (img.width() as f64, img.height() as f64);
                let (ix, iy) = ((t[0] + (t[2] - iw) / 2.0).round(), (t[1] + (t[3] - ih) / 2.0).round());
                cr.set_source_surface(img, ix, iy).ok();
                cr.paint().ok();
            }
            None => {
                // no image (minimised before we ever saw it): the big icon, like Win7
                if let Some(p) = win.icon.as_ref().map(|p| crate::util::scale(p, 48)) {
                    cr.set_source_pixbuf(&p, t[0] + (t[2] - 48.0) / 2.0, t[1] + (t[3] - 48.0) / 2.0);
                    cr.paint().ok();
                }
            }
        }
        if st.hot.get() == Some(i) { close_button(cr, x, st.hot_close.get()); }
    }
}

/// The light glass box Win7 draws around the thumbnail under the pointer.
fn highlight(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    glass::rounded(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 3.0);
    let g = cairo::LinearGradient::new(0.0, y, 0.0, y + h);
    g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.45);
    g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.2);
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    glass::rounded(cr, x - 0.5, y - 0.5, w + 1.0, h + 1.0, 4.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
    cr.stroke().ok();
}

fn icon(cr: &cairo::Context, p: Option<&Pixbuf>, x: f64, y: f64) {
    let Some(p) = p else { return };
    let p = if p.width() != 16 { crate::util::scale(p, 16) } else { p.clone() };
    cr.set_source_pixbuf(&p, x, y);
    cr.paint().ok();
}

/// Black title with the white Aero glow.
fn title(cr: &cairo::Context, layout: &gtk::pango::Layout, text: &str, x: f64, y: f64) {
    layout.set_text(text);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.35);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-1.0, -1.0), (1.0, 1.0), (-1.0, 1.0), (1.0, -1.0)] {
        cr.move_to(x + dx, y + dy);
        pangocairo::functions::show_layout(cr, layout);
    }
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.move_to(x, y);
    pangocairo::functions::show_layout(cr, layout);
}

/// The small red close button of the thumbnail under the pointer.
fn close_button(cr: &cairo::Context, r: [f64; 4], hot: bool) {
    let [x, y, w, h] = r;
    glass::rounded(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 2.0);
    let g = cairo::LinearGradient::new(0.0, y, 0.0, y + h);
    if hot {
        g.add_color_stop_rgb(0.0, 0.98, 0.62, 0.52);
        g.add_color_stop_rgb(0.5, 0.90, 0.32, 0.22);
        g.add_color_stop_rgb(1.0, 0.85, 0.20, 0.10);
    } else {
        g.add_color_stop_rgb(0.0, 0.90, 0.55, 0.47);
        g.add_color_stop_rgb(1.0, 0.75, 0.25, 0.17);
    }
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgba(0.35, 0.05, 0.0, 0.8);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(1.6);
    let (cx, cy, k) = (x + w / 2.0, y + h / 2.0, 2.8);
    cr.move_to(cx - k, cy - k);
    cr.line_to(cx + k, cy + k);
    cr.move_to(cx + k, cy - k);
    cr.line_to(cx - k, cy + k);
    cr.stroke().ok();
}
