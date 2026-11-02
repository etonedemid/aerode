//! Explorer's icon views (Extra large / Large / Medium / Small icons, List, Tiles): a virtualised
//! grid drawn like Win7's list view. Only the visible items are drawn and every item has the same
//! size, so a 10 000-file folder costs nothing to lay out – GtkIconView measured every item again
//! after every row change (seconds per thumbnail batch in a big picture folder).
//!
//! It shows a GtkListStore (the same model as the Details view) and keeps selection, focus and
//! anchor as row indices that follow the model's insert/delete/reorder signals.
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

const M: f64 = 6.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Layout {
    /// the icon above a name of up to two lines, row after row
    Icons { icon: i32, cw: i32 },
    /// a 16 px icon left of the name, row after row
    Small,
    /// a 16 px icon and the name, column after column, scrolling sideways
    List,
    /// a 48 px icon with name, type and size to its right
    Tiles,
}

/// Model columns the grid reads.
#[derive(Clone, Copy)]
pub struct Cols { pub icon: u32, pub name: u32, pub markup: u32, pub sensitive: u32 }

struct Press { index: Option<i32>, x: f64, y: f64, defer_select: bool, dragging: bool, event: gdk::Event }

struct Band { x0: f64, y0: f64, x1: f64, y1: f64, base: BTreeSet<i32> }

struct Rename { index: i32, done: Box<dyn Fn(Option<String>)> }

struct Geo { cw: f64, base_w: f64, ch: f64, per: i32, column_major: bool }

pub struct Grid {
    /// what goes into the window
    pub root: gtk::Grid,
    /// the drawing surface: events, focus, drag and drop
    pub area: gtk::DrawingArea,
    vadj: gtk::Adjustment,
    hadj: gtk::Adjustment,
    vbar: gtk::Scrollbar,
    hbar: gtk::Scrollbar,
    entry: gtk::Entry,
    cols: Cols,
    model: RefCell<Option<gtk::ListStore>>,
    model_handlers: RefCell<Vec<glib::SignalHandlerId>>,
    n: Cell<i32>,
    layout: Cell<Layout>,
    sel: RefCell<BTreeSet<i32>>,
    cursor: Cell<i32>,
    anchor: Cell<i32>,
    hover: Cell<i32>,
    drop: Cell<i32>,
    single_click: Cell<bool>,
    press: RefCell<Option<Press>>,
    band: RefCell<Option<Band>>,
    rename: RefCell<Option<Rename>>,
    autoscroll: RefCell<Option<glib::SourceId>>,
    activated: RefCell<Vec<Rc<dyn Fn(i32)>>>,
    changed: RefCell<Vec<Rc<dyn Fn()>>>,
    tooltip: RefCell<Option<Box<dyn Fn(i32) -> Option<String>>>>,
    line_h: Cell<f64>,
    /// item to keep in view across the next re-layout (a scroll bar coming or going reflows)
    keep_visible: Cell<i32>,
}

unsafe extern "C" fn reordered_trampoline<F: Fn(&[i32]) + 'static>(model: *mut gtk::ffi::GtkTreeModel, _path: *mut gtk::ffi::GtkTreePath,
    iter: *mut gtk::ffi::GtkTreeIter, new_order: *mut i32, f: glib::ffi::gpointer) {
    // only the top level matters (a list)
    if !iter.is_null() || new_order.is_null() { return; }
    unsafe {
        let n = gtk::ffi::gtk_tree_model_iter_n_children(model, std::ptr::null_mut());
        let f: &F = &*(f as *const F);
        f(std::slice::from_raw_parts(new_order, n.max(0) as usize));
    }
}

fn connect_rows_reordered<F: Fn(&[i32]) + 'static>(store: &gtk::ListStore, f: F) -> glib::SignalHandlerId {
    use glib::translate::ToGlibPtr;
    unsafe {
        let obj: *mut glib::gobject_ffi::GObject = store.upcast_ref::<glib::Object>().to_glib_none().0;
        let tramp: unsafe extern "C" fn(*mut gtk::ffi::GtkTreeModel, *mut gtk::ffi::GtkTreePath, *mut gtk::ffi::GtkTreeIter, *mut i32, glib::ffi::gpointer) = reordered_trampoline::<F>;
        glib::signal::connect_raw(obj, c"rows-reordered".as_ptr() as *const _, Some(std::mem::transmute::<*const (), unsafe extern "C" fn()>(tramp as *const ())), Box::into_raw(Box::new(f)))
    }
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
    cr.close_path();
}

fn hex(c: u32) -> (f64, f64, f64) { (((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0) }

/// Win7's item highlight: hot, selected, selected+hot, selected without focus, drop target.
fn item_bg(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, selected: bool, hot: bool, focused: bool) {
    let (top, bottom, border) = match (selected, hot, focused) {
        (true, true, _) => (0xd8e6fa, 0xbad3f5, 0x7da2ce),
        (true, false, true) => (0xdcebfc, 0xc1dbfc, 0x7da2ce),
        (true, false, false) => (0xf8f8f8, 0xe5e5e5, 0xd9d9d9),
        (false, true, _) => (0xfafbfd, 0xebf3fd, 0xb8d6fb),
        _ => return,
    };
    let g = gtk::cairo::LinearGradient::new(0.0, y, 0.0, y + h);
    let (r, gg, b) = hex(top); g.add_color_stop_rgb(0.0, r, gg, b);
    let (r, gg, b) = hex(bottom); g.add_color_stop_rgb(1.0, r, gg, b);
    rounded(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 2.5);
    let _ = cr.set_source(&g);
    cr.fill_preserve().ok();
    let (r, gg, b) = hex(border);
    cr.set_source_rgb(r, gg, b);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    // the inner white line
    rounded(cr, x + 1.5, y + 1.5, w - 3.0, h - 3.0, 1.5);
    cr.set_source_rgba(1.0, 1.0, 1.0, if selected { 0.45 } else { 0.7 });
    cr.stroke().ok();
}

impl Grid {
    pub fn new(cols: Cols) -> Rc<Grid> {
        let root = gtk::Grid::new();
        let area = gtk::DrawingArea::new();
        area.set_can_focus(true);
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.add_events(gdk::EventMask::BUTTON_PRESS_MASK | gdk::EventMask::BUTTON_RELEASE_MASK | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::SCROLL_MASK | gdk::EventMask::SMOOTH_SCROLL_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK | gdk::EventMask::KEY_PRESS_MASK
            | gdk::EventMask::FOCUS_CHANGE_MASK);
        area.style_context().add_class("exp-grid");
        area.set_has_tooltip(true);
        let overlay = gtk::Overlay::new();
        overlay.add(&area);
        let entry = gtk::Entry::new();
        entry.style_context().add_class("exp-rename");
        entry.set_halign(gtk::Align::Start);
        entry.set_valign(gtk::Align::Start);
        entry.set_no_show_all(true);
        overlay.add_overlay(&entry);
        let vadj = gtk::Adjustment::new(0.0, 0.0, 1.0, 20.0, 100.0, 1.0);
        let hadj = gtk::Adjustment::new(0.0, 0.0, 1.0, 20.0, 100.0, 1.0);
        let vbar = gtk::Scrollbar::new(gtk::Orientation::Vertical, Some(&vadj));
        let hbar = gtk::Scrollbar::new(gtk::Orientation::Horizontal, Some(&hadj));
        vbar.set_no_show_all(true);
        hbar.set_no_show_all(true);
        root.attach(&overlay, 0, 0, 1, 1);
        root.attach(&vbar, 1, 0, 1, 1);
        root.attach(&hbar, 0, 1, 1, 1);
        let g = Rc::new(Grid {
            root, area: area.clone(), vadj: vadj.clone(), hadj: hadj.clone(), vbar, hbar, entry: entry.clone(), cols,
            model: RefCell::new(None), model_handlers: RefCell::new(vec![]), n: Cell::new(0), layout: Cell::new(Layout::Icons { icon: 48, cw: 76 }),
            sel: RefCell::new(BTreeSet::new()), cursor: Cell::new(-1), anchor: Cell::new(-1), hover: Cell::new(-1), drop: Cell::new(-1),
            single_click: Cell::new(false), press: RefCell::new(None), band: RefCell::new(None), rename: RefCell::new(None),
            autoscroll: RefCell::new(None), activated: RefCell::new(vec![]), changed: RefCell::new(vec![]), tooltip: RefCell::new(None), line_h: Cell::new(0.0), keep_visible: Cell::new(-1),
        });
        let w = Rc::downgrade(&g);
        area.connect_draw(move |_, cr| { if let Some(g) = w.upgrade() { g.draw(cr); } glib::Propagation::Stop });
        let w = Rc::downgrade(&g);
        area.connect_size_allocate(move |_, _| {
            let Some(g) = w.upgrade() else { return };
            g.update_adjustments();
            let k = g.keep_visible.get();
            if k >= 0 { g.scroll_to(k); }
        });
        for adj in [&vadj, &hadj] {
            let w = Rc::downgrade(&g);
            adj.connect_value_changed(move |_| { if let Some(g) = w.upgrade() { g.area.queue_draw(); g.place_entry(); } });
        }
        for bar in [&g.vbar, &g.hbar] {
            let w = Rc::downgrade(&g);
            bar.connect_button_press_event(move |_, _| { if let Some(g) = w.upgrade() { g.keep_visible.set(-1); } glib::Propagation::Proceed });
        }
        let w = Rc::downgrade(&g);
        area.connect_button_press_event(move |_, ev| w.upgrade().map(|g| g.on_press(ev)).unwrap_or(glib::Propagation::Proceed));
        let w = Rc::downgrade(&g);
        area.connect_button_release_event(move |_, ev| w.upgrade().map(|g| g.on_release(ev)).unwrap_or(glib::Propagation::Proceed));
        let w = Rc::downgrade(&g);
        area.connect_motion_notify_event(move |_, ev| w.upgrade().map(|g| g.on_motion(ev)).unwrap_or(glib::Propagation::Proceed));
        let w = Rc::downgrade(&g);
        area.connect_leave_notify_event(move |_, _| { if let Some(g) = w.upgrade() { if g.hover.replace(-1) >= 0 { g.area.queue_draw(); } } glib::Propagation::Proceed });
        let w = Rc::downgrade(&g);
        area.connect_scroll_event(move |_, ev| w.upgrade().map(|g| g.on_scroll(ev)).unwrap_or(glib::Propagation::Proceed));
        let w = Rc::downgrade(&g);
        area.connect_key_press_event(move |_, ev| w.upgrade().map(|g| g.on_key(ev)).unwrap_or(glib::Propagation::Proceed));
        area.connect_focus_in_event(|a, _| { a.queue_draw(); glib::Propagation::Proceed });
        area.connect_focus_out_event(|a, _| { a.queue_draw(); glib::Propagation::Proceed });
        let w = Rc::downgrade(&g);
        area.connect_drag_begin(move |_, ctx| {
            let Some(g) = w.upgrade() else { return };
            let i = g.press.borrow().as_ref().and_then(|p| p.index);
            if let Some(pb) = i.and_then(|i| g.pixbuf(i)) {
                let (pw, ph) = (pb.width(), pb.height());
                ctx.drag_set_icon_pixbuf(&pb, pw / 2, ph / 2);
            }
        });
        let w = Rc::downgrade(&g);
        area.connect_drag_end(move |_, _| { if let Some(g) = w.upgrade() { g.press.borrow_mut().take(); } });
        let w = Rc::downgrade(&g);
        area.connect_query_tooltip(move |_, x, y, kb, tip| {
            let Some(g) = w.upgrade() else { return false };
            if kb || g.press.borrow().is_some() || g.rename.borrow().is_some() { return false; }
            let Some(i) = g.index_at(x as f64, y as f64) else { return false };
            let text = g.tooltip.borrow().as_ref().and_then(|f| f(i));
            match text { Some(t) => { tip.set_text(Some(&t)); true } None => false }
        });
        // renaming in place
        let w = Rc::downgrade(&g);
        entry.connect_activate(move |_| { if let Some(g) = w.upgrade() { g.finish_rename(true); } });
        let w = Rc::downgrade(&g);
        entry.connect_key_press_event(move |_, ev| {
            if ev.keyval() == gdk::keys::constants::Escape { if let Some(g) = w.upgrade() { g.finish_rename(false); } return glib::Propagation::Stop; }
            glib::Propagation::Proceed
        });
        let w = Rc::downgrade(&g);
        entry.connect_focus_out_event(move |_, _| { if let Some(g) = w.upgrade() { g.finish_rename(true); } glib::Propagation::Proceed });
        g
    }

    // ------------------------------------------------------------ model

    pub fn set_model(self: &Rc<Self>, m: Option<&gtk::ListStore>) {
        if self.model.borrow().as_ref() == m { return; }
        self.finish_rename(false);
        if let Some(old) = self.model.borrow_mut().take() {
            for h in self.model_handlers.borrow_mut().drain(..) { old.disconnect(h); }
        }
        let had_sel = !self.sel.borrow().is_empty();
        self.keep_visible.set(-1);
        self.sel.borrow_mut().clear();
        self.cursor.set(-1);
        self.anchor.set(-1);
        self.hover.set(-1);
        self.drop.set(-1);
        self.press.borrow_mut().take();
        self.band.borrow_mut().take();
        self.n.set(m.map(|m| m.iter_n_children(None)).unwrap_or(0));
        if let Some(m) = m {
            let mut hs = vec![];
            let w = Rc::downgrade(self);
            hs.push(m.connect_row_inserted(move |_, p, _| { if let Some(g) = w.upgrade() { g.on_inserted(p.indices().first().copied().unwrap_or(0)); } }));
            let w = Rc::downgrade(self);
            hs.push(m.connect_row_deleted(move |_, p| { if let Some(g) = w.upgrade() { g.on_deleted(p.indices().first().copied().unwrap_or(0)); } }));
            let w = Rc::downgrade(self);
            hs.push(m.connect_row_changed(move |_, p, _| { if let Some(g) = w.upgrade() { g.on_changed(p.indices().first().copied().unwrap_or(0)); } }));
            let w = Rc::downgrade(self);
            hs.push(connect_rows_reordered(m, move |order| { if let Some(g) = w.upgrade() { g.on_reordered(order); } }));
            *self.model_handlers.borrow_mut() = hs;
            *self.model.borrow_mut() = Some(m.clone());
        }
        self.vadj.set_value(0.0);
        self.hadj.set_value(0.0);
        self.update_adjustments();
        self.area.queue_draw();
        if had_sel { self.emit_changed(); }
    }

    pub fn model(&self) -> Option<gtk::ListStore> { self.model.borrow().clone() }

    fn on_inserted(&self, i: i32) {
        self.n.set(self.n.get() + 1);
        let shift = |s: &BTreeSet<i32>| -> BTreeSet<i32> { s.iter().map(|&x| if x >= i { x + 1 } else { x }).collect() };
        if self.sel.borrow().range(i..).next().is_some() { let s = shift(&self.sel.borrow()); *self.sel.borrow_mut() = s; }
        for c in [&self.cursor, &self.anchor, &self.hover, &self.drop] { if c.get() >= i { c.set(c.get() + 1); } }
        if let Some(r) = self.rename.borrow_mut().as_mut() { if r.index >= i { r.index += 1; } }
        self.update_adjustments();
        self.queue_item(i);
    }

    fn on_deleted(&self, i: i32) {
        self.n.set((self.n.get() - 1).max(0));
        let removed = self.sel.borrow_mut().remove(&i);
        if self.sel.borrow().range(i..).next().is_some() {
            let s: BTreeSet<i32> = self.sel.borrow().iter().map(|&x| if x > i { x - 1 } else { x }).collect();
            *self.sel.borrow_mut() = s;
        }
        let n = self.n.get();
        for c in [&self.cursor, &self.anchor] {
            let v = c.get();
            if v > i { c.set(v - 1); } else if v == i { c.set(if i < n { i } else { n - 1 }); }
        }
        for c in [&self.hover, &self.drop] { let v = c.get(); if v == i { c.set(-1); } else if v > i { c.set(v - 1); } }
        let cancel = { let mut r = self.rename.borrow_mut(); match r.as_mut() { Some(x) if x.index == i => true, Some(x) => { if x.index > i { x.index -= 1; } false } None => false } };
        if cancel { self.finish_rename(false); }
        self.update_adjustments();
        self.area.queue_draw();
        if removed { self.emit_changed(); }
    }

    fn on_changed(&self, i: i32) { self.queue_item(i); }

    fn on_reordered(&self, order: &[i32]) {
        // order[new] = old
        let mut inv = vec![0i32; order.len()];
        for (new, &old) in order.iter().enumerate() { if (old as usize) < inv.len() { inv[old as usize] = new as i32; } }
        let map = |x: i32| -> i32 { if x >= 0 && (x as usize) < inv.len() { inv[x as usize] } else { x } };
        let s: BTreeSet<i32> = self.sel.borrow().iter().map(|&x| map(x)).collect();
        *self.sel.borrow_mut() = s;
        for c in [&self.cursor, &self.anchor, &self.hover, &self.drop] { c.set(map(c.get())); }
        if let Some(r) = self.rename.borrow_mut().as_mut() { r.index = map(r.index); }
        self.area.queue_draw();
        self.place_entry();
    }

    fn iter(&self, i: i32) -> Option<(gtk::ListStore, gtk::TreeIter)> {
        let m = self.model.borrow().clone()?;
        let it = m.iter_nth_child(None, i)?;
        Some((m, it))
    }

    fn pixbuf(&self, i: i32) -> Option<Pixbuf> {
        let (m, it) = self.iter(i)?;
        m.value(&it, self.cols.icon as i32).get::<Option<Pixbuf>>().ok().flatten()
    }

    // ------------------------------------------------------------ layout

    pub fn set_layout(&self, l: Layout) {
        if self.layout.get() == l { return; }
        self.finish_rename(false);
        self.layout.set(l);
        self.hadj.set_value(0.0);
        self.update_adjustments();
        if self.cursor.get() >= 0 { self.scroll_to(self.cursor.get()); }
        self.area.queue_draw();
    }

    pub fn set_activate_on_single_click(&self, on: bool) { self.single_click.set(on); }

    fn line_height(&self) -> f64 {
        if self.line_h.get() <= 0.0 {
            let m = self.area.pango_context().metrics(None, None);
            self.line_h.set((((m.ascent() + m.descent()) as f64) / pango::SCALE as f64).ceil().max(12.0));
        }
        self.line_h.get()
    }

    fn geo(&self) -> Geo {
        let lh = self.line_height();
        let (w, h) = (self.area.allocated_width() as f64, self.area.allocated_height() as f64);
        let (cw, ch) = match self.layout.get() {
            Layout::Icons { icon, cw } => (cw as f64, icon as f64 + 6.0 + 2.0 * lh + 9.0),
            Layout::Small => (210.0, lh.max(16.0) + 6.0),
            Layout::List => (250.0, lh.max(16.0) + 4.0),
            Layout::Tiles => (254.0, (3.0 * lh + 10.0).max(58.0)),
        };
        match self.layout.get() {
            Layout::List => Geo { cw, base_w: cw, ch, per: (((h - 2.0 * M) / ch).floor() as i32).max(1), column_major: true },
            l => {
                let per = (((w - 2.0 * M) / cw).floor() as i32).max(1);
                // icon views spread the columns over the width, like Explorer
                let spread = if matches!(l, Layout::Icons { .. }) { ((w - 2.0 * M - per as f64 * cw) / per as f64).max(0.0) } else { 0.0 };
                Geo { cw: cw + spread, base_w: cw, ch, per, column_major: false }
            }
        }
    }

    /// The highlight box of item i, in content coordinates.
    fn item_box(&self, g: &Geo, i: i32) -> (f64, f64, f64, f64) {
        let (a, b) = ((i / g.per) as f64, (i % g.per) as f64);
        let (x, y) = if g.column_major { (M + a * g.cw, M + b * g.ch) } else { (M + b * g.cw, M + a * g.ch) };
        (x + (g.cw - g.base_w) / 2.0 + 1.0, y + 1.0, g.base_w - 2.0, g.ch - 2.0)
    }

    fn update_adjustments(&self) {
        let g = self.geo();
        let (w, h) = (self.area.allocated_width() as f64, self.area.allocated_height() as f64);
        let n = self.n.get();
        let lines = ((n + g.per - 1) / g.per).max(0) as f64;
        if g.column_major {
            let cw = 2.0 * M + lines * g.cw;
            let v = self.hadj.value().min((cw - w).max(0.0));
            self.hadj.configure(v, 0.0, cw.max(w), g.cw / 2.0, (w * 0.9).max(1.0), w.max(1.0));
            self.vadj.configure(0.0, 0.0, h.max(1.0), 1.0, 1.0, h.max(1.0));
            self.hbar.set_visible(cw > w + 0.5);
            self.vbar.set_visible(false);
        } else {
            let chh = 2.0 * M + lines * g.ch;
            let v = self.vadj.value().min((chh - h).max(0.0));
            self.vadj.configure(v, 0.0, chh.max(h), (g.ch / 2.0).max(16.0), (h * 0.9).max(1.0), h.max(1.0));
            self.hadj.configure(0.0, 0.0, w.max(1.0), 1.0, 1.0, w.max(1.0));
            self.vbar.set_visible(chh > h + 0.5);
            self.hbar.set_visible(false);
        }
    }

    /// The item under widget coordinates (x, y).
    pub fn index_at(&self, x: f64, y: f64) -> Option<i32> {
        let g = self.geo();
        let (cx, cy) = (x + self.hadj.value(), y + self.vadj.value());
        let (a, b) = (((cx - M) / g.cw).floor() as i32, ((cy - M) / g.ch).floor() as i32);
        if a < 0 || b < 0 { return None; }
        let i = if g.column_major { if b >= g.per { return None; } a * g.per + b } else { if a >= g.per { return None; } b * g.per + a };
        if i >= self.n.get() { return None; }
        let (bx, by, bw, bh) = self.item_box(&g, i);
        (cx >= bx && cx < bx + bw && cy >= by && cy < by + bh).then_some(i)
    }

    pub fn path_at_pos(&self, x: i32, y: i32) -> Option<gtk::TreePath> {
        self.index_at(x as f64, y as f64).map(|i| gtk::TreePath::from_indicesv(&[i]))
    }

    /// First and last item that can be seen.
    pub fn visible_range(&self) -> Option<(gtk::TreePath, gtk::TreePath)> {
        let (a, b) = self.visible_indices();
        (b >= a && self.n.get() > 0).then(|| (gtk::TreePath::from_indicesv(&[a]), gtk::TreePath::from_indicesv(&[b])))
    }

    fn visible_indices(&self) -> (i32, i32) {
        let g = self.geo();
        let n = self.n.get();
        let (lo, hi, step) = if g.column_major {
            let v = self.hadj.value();
            (((v - M) / g.cw).floor(), ((v + self.area.allocated_width() as f64 - M) / g.cw).floor(), g.per)
        } else {
            let v = self.vadj.value();
            (((v - M) / g.ch).floor(), ((v + self.area.allocated_height() as f64 - M) / g.ch).floor(), g.per)
        };
        let a = (lo.max(0.0) as i32) * step;
        let b = ((hi.max(0.0) as i32 + 1) * step - 1).min(n - 1);
        (a.min(n.max(1) - 1).max(0), b)
    }

    fn queue_item(&self, i: i32) {
        let (a, b) = self.visible_indices();
        if i >= a && i <= b { self.area.queue_draw(); }
    }

    pub fn scroll_to(&self, i: i32) {
        if i < 0 || i >= self.n.get() { return; }
        self.keep_visible.set(i);
        let g = self.geo();
        let (x, y, w, h) = self.item_box(&g, i);
        let (adj, lo, len) = if g.column_major { (&self.hadj, x - 1.0, w + 2.0) } else { (&self.vadj, y - 1.0, h + 2.0) };
        let (v, page) = (adj.value(), adj.page_size());
        if lo < v { adj.set_value((lo - M).max(0.0)); } else if lo + len > v + page { adj.set_value((lo + len - page + M).min(adj.upper() - page)); }
    }

    // ------------------------------------------------------------ selection

    fn emit_changed(&self) {
        let cbs: Vec<Rc<dyn Fn()>> = self.changed.borrow().clone();
        for c in cbs { c(); }
    }

    pub fn connect_selection_changed(&self, f: impl Fn() + 'static) { self.changed.borrow_mut().push(Rc::new(f)); }

    pub fn connect_item_activated(&self, f: impl Fn(i32) + 'static) { self.activated.borrow_mut().push(Rc::new(f)); }

    /// Called whenever the view scrolls.
    pub fn connect_scrolled(&self, f: impl Fn() + 'static) {
        let f = Rc::new(f);
        for adj in [&self.vadj, &self.hadj] { let f = f.clone(); adj.connect_value_changed(move |_| f()); }
    }

    /// Win7 infotips: the text for an item, or None.
    pub fn set_tooltip_fn(&self, f: impl Fn(i32) -> Option<String> + 'static) { *self.tooltip.borrow_mut() = Some(Box::new(f)); }

    fn activate(&self, i: i32) {
        let cbs: Vec<Rc<dyn Fn(i32)>> = self.activated.borrow().clone();
        for c in cbs { c(i); }
    }

    pub fn selected_items(&self) -> Vec<gtk::TreePath> {
        self.sel.borrow().iter().map(|&i| gtk::TreePath::from_indicesv(&[i])).collect()
    }

    pub fn path_is_selected(&self, p: &gtk::TreePath) -> bool {
        p.indices().first().map(|i| self.sel.borrow().contains(i)).unwrap_or(false)
    }

    pub fn select_path(&self, p: &gtk::TreePath) {
        let Some(&i) = p.indices().first() else { return };
        if i < 0 || i >= self.n.get() { return; }
        if self.sel.borrow_mut().insert(i) { self.area.queue_draw(); self.emit_changed(); }
    }

    pub fn unselect_all(&self) {
        if self.sel.borrow().is_empty() { return; }
        self.sel.borrow_mut().clear();
        self.area.queue_draw();
        self.emit_changed();
    }

    pub fn select_all(&self) {
        let n = self.n.get();
        if self.sel.borrow().len() == n as usize { return; }
        *self.sel.borrow_mut() = (0..n).collect();
        self.area.queue_draw();
        self.emit_changed();
    }

    /// Focus item (the dotted one), scrolled into view; the selection is left alone.
    pub fn set_cursor(&self, p: &gtk::TreePath) {
        let Some(&i) = p.indices().first() else { return };
        if i < 0 || i >= self.n.get() { return; }
        self.cursor.set(i);
        self.anchor.set(i);
        self.scroll_to(i);
        self.area.queue_draw();
    }

    pub fn cursor(&self) -> Option<gtk::TreePath> {
        let c = self.cursor.get();
        (c >= 0 && c < self.n.get()).then(|| gtk::TreePath::from_indicesv(&[c]))
    }

    /// Highlight the folder a drop would go into.
    pub fn set_drop_highlight(&self, p: Option<&gtk::TreePath>) {
        let i = p.and_then(|p| p.indices().first().copied()).unwrap_or(-1);
        if self.drop.replace(i) != i { self.area.queue_draw(); }
    }

    fn select_only(&self, i: i32) -> bool {
        let mut s = self.sel.borrow_mut();
        if s.len() == 1 && s.contains(&i) { return false; }
        s.clear();
        s.insert(i);
        true
    }

    fn select_range(&self, a: i32, b: i32, keep: bool) {
        let (lo, hi) = (a.min(b).max(0), a.max(b).min(self.n.get() - 1));
        let mut s = self.sel.borrow_mut();
        if !keep { s.clear(); }
        for i in lo..=hi { s.insert(i); }
    }

    // ------------------------------------------------------------ events

    fn on_press(self: &Rc<Self>, ev: &gdk::EventButton) -> glib::Propagation {
        if self.rename.borrow().is_some() { self.finish_rename(true); }
        self.area.grab_focus();
        if ev.button() != 1 { return glib::Propagation::Proceed; }
        let (x, y) = ev.position();
        let idx = self.index_at(x, y);
        if ev.event_type() == gdk::EventType::DoubleButtonPress {
            self.press.borrow_mut().take();
            if let Some(i) = idx { if !self.single_click.get() { self.activate(i); } }
            return glib::Propagation::Stop;
        }
        if ev.event_type() != gdk::EventType::ButtonPress { return glib::Propagation::Stop; }
        let st = ev.state();
        let (ctrl, shift) = (st.contains(gdk::ModifierType::CONTROL_MASK), st.contains(gdk::ModifierType::SHIFT_MASK));
        let mut changed = false;
        let mut defer = false;
        match idx {
            Some(i) => {
                let was = self.sel.borrow().contains(&i);
                if shift {
                    let a = if self.anchor.get() >= 0 { self.anchor.get() } else { i };
                    let before = self.sel.borrow().clone();
                    self.select_range(a, i, ctrl);
                    changed = *self.sel.borrow() != before;
                } else if ctrl {
                    { let mut s = self.sel.borrow_mut(); if was { s.remove(&i); } else { s.insert(i); } }
                    changed = true;
                    self.anchor.set(i);
                } else if !was {
                    changed = self.select_only(i);
                    self.anchor.set(i);
                } else {
                    // a click on a selected item: the others go when the button comes up without a drag
                    defer = true;
                    self.anchor.set(i);
                }
                self.cursor.set(i);
            }
            None => {
                if !ctrl && !shift && !self.sel.borrow().is_empty() { self.sel.borrow_mut().clear(); changed = true; }
                let (cx, cy) = (x + self.hadj.value(), y + self.vadj.value());
                *self.band.borrow_mut() = Some(Band { x0: cx, y0: cy, x1: cx, y1: cy, base: if ctrl { self.sel.borrow().clone() } else { BTreeSet::new() } });
            }
        }
        *self.press.borrow_mut() = Some(Press { index: idx, x, y, defer_select: defer, dragging: false, event: (**ev).clone() });
        self.area.queue_draw();
        if changed { self.emit_changed(); }
        glib::Propagation::Stop
    }

    fn on_release(self: &Rc<Self>, ev: &gdk::EventButton) -> glib::Propagation {
        if ev.button() != 1 { return glib::Propagation::Proceed; }
        if let Some(t) = self.autoscroll.borrow_mut().take() { t.remove(); }
        if self.band.borrow_mut().take().is_some() { self.area.queue_draw(); }
        let p = self.press.borrow_mut().take();
        if let Some(p) = p {
            if let (Some(i), false) = (p.index, p.dragging) {
                let st = ev.state();
                let mods = st.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
                if p.defer_select && self.select_only(i) { self.area.queue_draw(); self.emit_changed(); }
                if self.single_click.get() && !mods && ev.event_type() == gdk::EventType::ButtonRelease { self.activate(i); }
            }
        }
        glib::Propagation::Stop
    }

    fn on_motion(self: &Rc<Self>, ev: &gdk::EventMotion) -> glib::Propagation {
        let (x, y) = ev.position();
        let pressed = ev.state().contains(gdk::ModifierType::BUTTON1_MASK);
        if !pressed {
            self.press.borrow_mut().take();
            if self.band.borrow_mut().take().is_some() { self.area.queue_draw(); }
        }
        if self.band.borrow().is_some() {
            self.update_band(x, y);
            self.ensure_autoscroll();
            return glib::Propagation::Stop;
        }
        let drag = {
            let mut p = self.press.borrow_mut();
            match p.as_mut() {
                Some(p) if p.index.is_some() && !p.dragging && ((x - p.x).powi(2) + (y - p.y).powi(2)).sqrt() > 8.0 => { p.dragging = true; Some((p.event.clone(), p.x, p.y)) }
                _ => None,
            }
        };
        if let Some((event, px, py)) = drag {
            let targets = gtk::TargetList::new(&[gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0)]);
            self.area.drag_begin_with_coordinates(&targets, gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK, 1, Some(&event), px as i32, py as i32);
            return glib::Propagation::Stop;
        }
        let h = self.index_at(x, y).unwrap_or(-1);
        if self.hover.replace(h) != h { self.area.queue_draw(); }
        glib::Propagation::Proceed
    }

    fn update_band(&self, x: f64, y: f64) {
        let g = self.geo();
        let (cx, cy) = (x + self.hadj.value(), y + self.vadj.value());
        let (x0, y0, base) = {
            let mut b = self.band.borrow_mut();
            let Some(b) = b.as_mut() else { return };
            b.x1 = cx;
            b.y1 = cy;
            (b.x0, b.y0, b.base.clone())
        };
        let (l, r, t, bt) = (x0.min(cx), x0.max(cx), y0.min(cy), y0.max(cy));
        let mut hits = base;
        let n = self.n.get();
        // only the rows / columns the band touches
        let (lo, hi) = if g.column_major { (((l - M) / g.cw).floor(), ((r - M) / g.cw).floor()) } else { (((t - M) / g.ch).floor(), ((bt - M) / g.ch).floor()) };
        let (lo, hi) = (lo.max(0.0) as i32, hi.max(0.0) as i32);
        for line in lo..=hi {
            for k in 0..g.per {
                let i = line * g.per + k;
                if i >= n { break; }
                let (bx, by, bw, bh) = self.item_box(&g, i);
                if bx < r && bx + bw > l && by < bt && by + bh > t { hits.insert(i); }
            }
        }
        let changed = *self.sel.borrow() != hits;
        if changed { *self.sel.borrow_mut() = hits; }
        self.area.queue_draw();
        if changed { self.emit_changed(); }
    }

    /// Rubber band at the edge: keep scrolling while the pointer is there.
    fn ensure_autoscroll(self: &Rc<Self>) {
        if self.autoscroll.borrow().is_some() { return; }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
            let Some(g) = w.upgrade() else { return glib::ControlFlow::Break };
            if g.band.borrow().is_none() { g.autoscroll.borrow_mut().take(); return glib::ControlFlow::Break; }
            let Some((x, y)) = g.pointer() else { return glib::ControlFlow::Continue };
            let col = g.geo().column_major;
            let (adj, pos, len) = if col { (&g.hadj, x, g.area.allocated_width() as f64) } else { (&g.vadj, y, g.area.allocated_height() as f64) };
            let d = if pos < 0.0 { pos } else if pos > len { pos - len } else { 0.0 };
            if d != 0.0 {
                adj.set_value((adj.value() + d.clamp(-60.0, 60.0)).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0)));
                g.update_band(x, y);
            }
            glib::ControlFlow::Continue
        });
        *self.autoscroll.borrow_mut() = Some(id);
    }

    fn pointer(&self) -> Option<(f64, f64)> {
        let gw = self.area.window()?;
        let dev = gw.display().default_seat()?.pointer()?;
        let (_, x, y, _) = gw.device_position(&dev);
        Some((x as f64, y as f64))
    }

    fn on_scroll(&self, ev: &gdk::EventScroll) -> glib::Propagation {
        if ev.state().contains(gdk::ModifierType::CONTROL_MASK) { return glib::Propagation::Proceed; }
        let g = self.geo();
        let (dx, dy) = match ev.direction() {
            gdk::ScrollDirection::Up => (0.0, -1.0),
            gdk::ScrollDirection::Down => (0.0, 1.0),
            gdk::ScrollDirection::Left => (-1.0, 0.0),
            gdk::ScrollDirection::Right => (1.0, 0.0),
            _ => ev.delta(),
        };
        // a wheel notch scrolls about three text lines (Win7); List scrolls sideways
        let step = (g.ch * 0.75).max(3.0 * self.line_height());
        let (adj, d) = if g.column_major { (&self.hadj, if dy != 0.0 { dy } else { dx }) } else { (&self.vadj, dy) };
        self.keep_visible.set(-1);
        if d != 0.0 { adj.set_value((adj.value() + d * step).clamp(0.0, (adj.upper() - adj.page_size()).max(0.0))); }
        glib::Propagation::Stop
    }

    fn on_key(&self, ev: &gdk::EventKey) -> glib::Propagation {
        use gdk::keys::constants as K;
        let k = ev.keyval();
        let st = ev.state() & gtk::accelerator_get_default_mod_mask();
        let (ctrl, shift) = (st.contains(gdk::ModifierType::CONTROL_MASK), st.contains(gdk::ModifierType::SHIFT_MASK));
        let n = self.n.get();
        if n == 0 { return glib::Propagation::Proceed; }
        let g = self.geo();
        let cur = self.cursor.get();
        let page = if g.column_major { ((self.area.allocated_width() as f64 / g.cw).floor() as i32).max(1) * g.per } else { ((self.area.allocated_height() as f64 / g.ch).floor() as i32).max(1) * g.per };
        // List runs down the columns: Up/Down step by one, Left/Right by a column
        let (horiz, vert) = if g.column_major { (g.per, 1) } else { (1, g.per) };
        let target = match k {
            k if k == K::Left || k == K::KP_Left => Some(cur - horiz),
            k if k == K::Right || k == K::KP_Right => Some(cur + horiz),
            k if k == K::Up || k == K::KP_Up => Some(cur - vert),
            k if k == K::Down || k == K::KP_Down => Some(cur + vert),
            k if k == K::Home || k == K::KP_Home => Some(0),
            k if k == K::End || k == K::KP_End => Some(n - 1),
            k if k == K::Page_Up || k == K::KP_Page_Up => Some(cur - page),
            k if k == K::Page_Down || k == K::KP_Page_Down => Some(cur + page),
            _ => None,
        };
        if let Some(t) = target {
            let t = if cur < 0 { 0 } else { t.clamp(0, n - 1) };
            if shift {
                let a = if self.anchor.get() >= 0 { self.anchor.get() } else { t };
                self.anchor.set(a);
                self.select_range(a, t, ctrl);
            } else if !ctrl {
                self.select_only(t);
                self.anchor.set(t);
            }
            self.cursor.set(t);
            self.scroll_to(t);
            self.area.queue_draw();
            self.emit_changed();
            return glib::Propagation::Stop;
        }
        if k == K::Return || k == K::KP_Enter || k == K::ISO_Enter {
            if cur >= 0 && self.sel.borrow().is_empty() { self.sel.borrow_mut().insert(cur); self.emit_changed(); }
            if !self.sel.borrow().is_empty() { self.activate(cur.max(0)); }
            return glib::Propagation::Stop;
        }
        if k == K::space && cur >= 0 && (ctrl || !self.sel.borrow().contains(&cur)) {
            { let mut s = self.sel.borrow_mut(); if ctrl && s.contains(&cur) { s.remove(&cur); } else { s.insert(cur); } }
            self.area.queue_draw();
            self.emit_changed();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    // ------------------------------------------------------------ drawing

    fn draw(&self, cr: &gtk::cairo::Context) {
        let (w, h) = (self.area.allocated_width() as f64, self.area.allocated_height() as f64);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok();
        let n = self.n.get();
        if n == 0 { return; }
        let g = self.geo();
        let (hv, vv) = (self.hadj.value(), self.vadj.value());
        let (a, b) = self.visible_indices();
        let focused = self.area.has_focus();
        let lh = self.line_height();
        for i in a..=b {
            let (bx, by, bw, bh) = self.item_box(&g, i);
            let (x, y) = (bx - hv, by - vv);
            if x > w || y > h || x + bw < 0.0 || y + bh < 0.0 { continue; }
            self.draw_item(cr, i, x, y, bw, bh, lh, focused);
        }
        if let Some(bd) = self.band.borrow().as_ref() {
            let (l, t) = (bd.x0.min(bd.x1) - hv, bd.y0.min(bd.y1) - vv);
            let (rw, rh) = ((bd.x1 - bd.x0).abs(), (bd.y1 - bd.y0).abs());
            cr.rectangle(l.floor() + 0.5, t.floor() + 0.5, rw.floor(), rh.floor());
            cr.set_source_rgba(0.2, 0.6, 1.0, 0.25);
            cr.fill_preserve().ok();
            cr.set_source_rgb(0.2, 0.6, 1.0);
            cr.set_line_width(1.0);
            cr.stroke().ok();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_item(&self, cr: &gtk::cairo::Context, i: i32, x: f64, y: f64, w: f64, h: f64, lh: f64, focused: bool) {
        let Some((m, it)) = self.iter(i) else { return };
        let pb: Option<Pixbuf> = m.value(&it, self.cols.icon as i32).get::<Option<Pixbuf>>().ok().flatten();
        let sens: bool = m.value(&it, self.cols.sensitive as i32).get().unwrap_or(true);
        let selected = self.sel.borrow().contains(&i);
        let hot = self.hover.get() == i || self.drop.get() == i;
        item_bg(cr, x, y, w, h, selected, hot, focused);
        if focused && self.cursor.get() == i && !selected && !hot {
            rounded(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 2.5);
            cr.set_source_rgb(0.49, 0.64, 0.81);
            cr.set_line_width(1.0);
            cr.stroke().ok();
        }
        let paint_icon = |pb: &Pixbuf, ix: f64, iy: f64| {
            cr.save().ok();
            cr.set_source_pixbuf(pb, ix.round(), iy.round());
            if sens { cr.paint().ok(); } else { cr.paint_with_alpha(0.5).ok(); }
            cr.restore().ok();
        };
        let pctx = self.area.pango_context();
        let layout = pango::Layout::new(&pctx);
        let ctx = self.area.style_context();
        let fg = ctx.color(gtk::StateFlags::NORMAL);
        match self.layout.get() {
            Layout::Icons { icon, .. } => {
                let icon = icon as f64;
                if let Some(pb) = &pb {
                    let (pw, ph) = (pb.width() as f64, pb.height() as f64);
                    // icons and thumbnails stand on the bottom of the icon box
                    paint_icon(pb, x + (w - pw) / 2.0, y + 3.0 + (icon - ph).max(0.0));
                }
                let name: String = m.value(&it, self.cols.name as i32).get().unwrap_or_default();
                // broken where it must, but without the hyphens Pango would add
                layout.set_markup(&format!("<span insert_hyphens=\"false\">{}</span>", glib::markup_escape_text(&name)));
                layout.set_width(((w - 6.0) * pango::SCALE as f64) as i32);
                layout.set_wrap(pango::WrapMode::WordChar);
                layout.set_ellipsize(pango::EllipsizeMode::End);
                layout.set_height(-2);
                layout.set_alignment(pango::Alignment::Center);
                cr.move_to(x + 3.0, y + 3.0 + icon + 4.0);
            }
            Layout::Small | Layout::List => {
                if let Some(pb) = &pb { paint_icon(pb, x + 3.0, y + (h - pb.height() as f64) / 2.0); }
                let name: String = m.value(&it, self.cols.name as i32).get().unwrap_or_default();
                layout.set_text(&name);
                layout.set_width(((w - 26.0).max(1.0) * pango::SCALE as f64) as i32);
                layout.set_ellipsize(pango::EllipsizeMode::End);
                cr.move_to(x + 23.0, y + (h - lh) / 2.0);
            }
            Layout::Tiles => {
                if let Some(pb) = &pb { paint_icon(pb, x + 4.0 + (48.0 - pb.width() as f64).max(0.0) / 2.0, y + (h - pb.height() as f64) / 2.0); }
                let markup: String = m.value(&it, self.cols.markup as i32).get().unwrap_or_default();
                layout.set_markup(&markup);
                layout.set_width(((w - 64.0).max(1.0) * pango::SCALE as f64) as i32);
                layout.set_ellipsize(pango::EllipsizeMode::End);
                let (_, lr) = layout.pixel_extents();
                cr.move_to(x + 58.0, y + (h - lr.height() as f64) / 2.0);
            }
        }
        cr.set_source_rgba(fg.red(), fg.green(), fg.blue(), fg.alpha());
        pangocairo::functions::show_layout(cr, &layout);
    }

    // ------------------------------------------------------------ rename in place

    /// Edit the name of item i in place: `select` characters are selected (the name without its
    /// extension); `done` gets the new text, or None when cancelled.
    pub fn start_rename(self: &Rc<Self>, p: &gtk::TreePath, text: &str, select: i32, done: impl Fn(Option<String>) + 'static) {
        let Some(&i) = p.indices().first() else { return };
        self.finish_rename(false);
        self.scroll_to(i);
        *self.rename.borrow_mut() = Some(Rename { index: i, done: Box::new(done) });
        self.entry.set_text(text);
        self.place_entry();
        self.entry.show();
        self.entry.grab_focus();
        self.entry.select_region(0, select);
        let e = self.entry.clone();
        glib::idle_add_local_once(move || e.select_region(0, select));
    }

    fn place_entry(&self) {
        let Some(i) = self.rename.borrow().as_ref().map(|r| r.index) else { return };
        let g = self.geo();
        let (bx, by, bw, bh) = self.item_box(&g, i);
        let (x, y) = (bx - self.hadj.value(), by - self.vadj.value());
        let (ex, ey, ew) = match self.layout.get() {
            Layout::Icons { icon, .. } => (x - 4.0, y + 3.0 + icon as f64 + 1.0, bw + 8.0),
            Layout::Small | Layout::List => (x + 20.0, y + (bh - 24.0) / 2.0, bw - 18.0),
            Layout::Tiles => (x + 54.0, y + 4.0, bw - 56.0),
        };
        self.entry.set_margin_start(ex.max(0.0) as i32);
        self.entry.set_margin_top(ey.max(0.0) as i32);
        self.entry.set_size_request(ew.max(60.0) as i32, -1);
    }

    fn finish_rename(&self, commit: bool) {
        let Some(r) = self.rename.borrow_mut().take() else { return };
        let text = self.entry.text().to_string();
        self.entry.hide();
        self.area.grab_focus();
        (r.done)(if commit { Some(text) } else { None });
    }
}

impl Drop for Grid {
    fn drop(&mut self) {
        if let Some(t) = self.autoscroll.borrow_mut().take() { t.remove(); }
    }
}
