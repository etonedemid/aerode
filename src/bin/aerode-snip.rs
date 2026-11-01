//! Snipping Tool – Windows 7 Snipping Tool remake.
//!   aerode-snip           open the tool
//!   aerode-snip --full    Print Screen: copy the whole screen to the clipboard (and save a copy)
//!   aerode-snip --rect    start a rectangular snip immediately (Win+Shift+S)
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

fn ink_color() -> (f64, f64, f64) { (0.9, 0.1, 0.1) }

/// The frozen, whitewashed screen the user snips from.
fn select(kind: Kind, shot: Pixbuf, done: Rc<dyn Fn(Option<Pixbuf>)>) {
    if kind == Kind::Full { done(Some(shot)); return; }
    let (sw, sh) = (shot.width(), shot.height());
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
    // window rectangles (frame extents included) for Window Snip
    let rects: Vec<(i32, i32, i32, i32)> = Wm::connect().ok().map(|wm| {
        wm.client_ids().into_iter().rev().filter_map(|id| {
            use x11rb::connection::Connection;
            use x11rb::protocol::xproto::ConnectionExt;
            let g = wm.conn.get_geometry(id).ok()?.reply().ok()?;
            let t = wm.conn.translate_coordinates(id, wm.root, 0, 0).ok()?.reply().ok()?;
            let _ = wm.conn.flush();
            let fe: Vec<u32> = wm.conn.get_property(false, id, wm.atoms._NET_WM_STRUT, x11rb::protocol::xproto::AtomEnum::CARDINAL, 0, 0).ok().map(|_| vec![]).unwrap_or_default();
            let _ = fe;
            Some((t.dst_x as i32 - 8, t.dst_y as i32 - 30, g.width as i32 + 16, g.height as i32 + 38))
        }).collect()
    }).unwrap_or_default();
    {
        let (shot, pts, hover) = (shot.clone(), pts.clone(), hover.clone());
        area.connect_draw(move |_, cr| {
            cr.set_source_pixbuf(&shot, 0.0, 0.0);
            cr.paint().ok();
            // whitewash everything outside the current selection
            cr.set_fill_rule(cairo::FillRule::EvenOdd);
            cr.rectangle(0.0, 0.0, sw as f64, sh as f64);
            let p = pts.borrow();
            match kind {
                Kind::Rect if p.len() >= 2 => {
                    let (a, b) = (p[0], p[p.len() - 1]);
                    cr.rectangle(a.0.min(b.0), a.1.min(b.1), (a.0 - b.0).abs(), (a.1 - b.1).abs());
                }
                Kind::Free if p.len() >= 2 => {
                    cr.move_to(p[0].0, p[0].1);
                    for q in p.iter().skip(1) { cr.line_to(q.0, q.1); }
                    cr.close_path();
                }
                Kind::Window => if let Some((x, y, w, h)) = hover.get() { cr.rectangle(x as f64, y as f64, w as f64, h as f64); },
                _ => {}
            }
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
            cr.fill().ok();
            let (r, g, b) = ink_color();
            cr.set_source_rgb(r, g, b);
            cr.set_line_width(2.0);
            match kind {
                Kind::Rect if p.len() >= 2 => {
                    let (a, bb) = (p[0], p[p.len() - 1]);
                    cr.rectangle(a.0.min(bb.0), a.1.min(bb.1), (a.0 - bb.0).abs(), (a.1 - bb.1).abs());
                    cr.stroke().ok();
                }
                Kind::Free if p.len() >= 2 => {
                    cr.move_to(p[0].0, p[0].1);
                    for q in p.iter().skip(1) { cr.line_to(q.0, q.1); }
                    cr.stroke().ok();
                }
                Kind::Window => if let Some((x, y, w, h)) = hover.get() { cr.rectangle(x as f64, y as f64, w as f64, h as f64); cr.stroke().ok(); },
                _ => {}
            }
            glib::Propagation::Proceed
        });
    }
    let finish = {
        let (win, pts, shot, done, hover) = (win.clone(), pts.clone(), shot.clone(), done.clone(), hover.clone());
        Rc::new(move |cancel: bool| {
            win.close();
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
            let (x, y) = (x.max(0), y.max(0));
            let (w, h) = (w.min(shot.width() - x), h.min(shot.height() - y));
            if w < 3 || h < 3 { done(None); return; }
            let sub = shot.new_subpixbuf(x, y, w, h);
            if kind == Kind::Free {
                // mask everything outside the free-form path with white (as Win7 does)
                let surf = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h).unwrap();
                {
                    let cr = cairo::Context::new(&surf).unwrap();
                    cr.set_source_rgb(1.0, 1.0, 1.0);
                    cr.paint().ok();
                    cr.move_to(p[0].0 - x as f64, p[0].1 - y as f64);
                    for q in p.iter().skip(1) { cr.line_to(q.0 - x as f64, q.1 - y as f64); }
                    cr.close_path();
                    cr.clip();
                    cr.set_source_pixbuf(&sub, 0.0, 0.0);
                    cr.paint().ok();
                }
                done(gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h));
            } else {
                done(Some(sub.copy().unwrap_or(sub)));
            }
        })
    };
    {
        let (pts, dragging, area2) = (pts.clone(), dragging.clone(), area.clone());
        let fin = finish.clone();
        area.connect_button_press_event(move |_, e| {
            if e.button() == 3 { fin(true); return glib::Propagation::Stop; }
            if kind == Kind::Window { fin(false); return glib::Propagation::Stop; }
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
                hover.set(rects.iter().find(|r| x as i32 >= r.0 && y as i32 >= r.1 && (x as i32) < r.0 + r.2 && (y as i32) < r.1 + r.3).copied());
                a.queue_draw();
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
    if let (Some(seat), Some(gw)) = (gdk::Display::default().and_then(|d| d.default_seat()), win.window()) {
        let cur = gdk::Cursor::from_name(&gdk::Display::default().unwrap(), "crosshair");
        let _ = seat.grab(&gw, gdk::SeatCapabilities::ALL, true, cur.as_ref(), None, None);
    }
}

// ------------------------------------------------------------- editor
#[derive(Clone)]
struct Stroke { pts: Vec<(f64, f64)>, rgba: (f64, f64, f64, f64), width: f64 }

#[derive(Clone, Copy, PartialEq)]
enum Tool { Pen, Highlighter, Eraser }

struct Editor {
    win: gtk::Window,
    img: RefCell<Pixbuf>,
    strokes: RefCell<Vec<Stroke>>,
    tool: Cell<Tool>,
    pen: Cell<(f64, f64, f64)>,
    area: gtk::DrawingArea,
}

impl Editor {
    fn flatten(&self) -> Pixbuf {
        let img = self.img.borrow();
        let (w, h) = (img.width(), img.height());
        let surf = cairo::ImageSurface::create(cairo::Format::Rgb24, w, h).unwrap();
        {
            let cr = cairo::Context::new(&surf).unwrap();
            cr.set_source_pixbuf(&img, 0.0, 0.0);
            cr.paint().ok();
            draw_strokes(&cr, &self.strokes.borrow());
        }
        gdk::pixbuf_get_from_surface(&surf, 0, 0, w, h).unwrap()
    }

    fn copy(&self) {
        let cb = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        cb.set_image(&self.flatten());
        cb.store();
    }

    fn save(&self) {
        let fc = gtk::FileChooserDialog::with_buttons(Some("Save As"), Some(&self.win), gtk::FileChooserAction::Save,
            &[("Save", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
        fc.set_do_overwrite_confirmation(true);
        for (name, pat) in [("Portable Network Graphic file (PNG) (*.PNG)", "*.png"), ("JPEG file (*.JPG)", "*.jpg"), ("GIF file (*.GIF)", "*.gif"), ("BMP file (*.BMP)", "*.bmp")] {
            let f = gtk::FileFilter::new();
            f.set_name(Some(name));
            f.add_pattern(pat);
            fc.add_filter(f);
        }
        fc.set_current_name("Capture.PNG");
        let _ = fc.set_current_folder(util::special_dir(glib::UserDirectory::Pictures, "Pictures"));
        if fc.run() == gtk::ResponseType::Accept {
            if let Some(mut p) = fc.filename() {
                let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                let ty = match ext.as_str() { "jpg" | "jpeg" => "jpeg", "bmp" => "bmp", "gif" => "png", "png" => "png", _ => { p.set_extension("png"); "png" } };
                if let Err(e) = self.flatten().savev(&p, ty, &[]) { util::error("The snip could not be saved.", &e.to_string()); }
            }
        }
        fc.close();
    }
}

fn draw_strokes(cr: &cairo::Context, strokes: &[Stroke]) {
    for s in strokes {
        if s.pts.is_empty() { continue; }
        cr.set_source_rgba(s.rgba.0, s.rgba.1, s.rgba.2, s.rgba.3);
        cr.set_line_width(s.width);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.move_to(s.pts[0].0, s.pts[0].1);
        if s.pts.len() == 1 { cr.line_to(s.pts[0].0 + 0.1, s.pts[0].1); }
        for p in s.pts.iter().skip(1) { cr.line_to(p.0, p.1); }
        cr.stroke().ok();
    }
}

fn tool_button(icon: &str, label: &str) -> gtk::ToolButton {
    let b = gtk::ToolButton::new(Some(&util::image(icon, 16)), Some(label));
    b.set_is_important(true);
    WidgetExt::set_tooltip_text(&b, Some(label));
    b
}

fn open_editor(app: &gtk::Application, shot: Pixbuf, main: Option<gtk::Window>) {
    let win = gtk::ApplicationWindow::new(app);
    win.set_title("Snipping Tool");
    win.set_icon_name(Some(util::first_icon(&["accessories-screenshot"])));
    let (iw, ih) = (shot.width(), shot.height());
    let g = util::primary_geometry();
    win.set_default_size((iw + 40).clamp(640, g.width() - 80), (ih + 110).clamp(300, g.height() - 120));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menubar = gtk::MenuBar::new();
    root.pack_start(&menubar, false, false, 0);
    let tb = gtk::Toolbar::new();
    tb.set_style(gtk::ToolbarStyle::BothHoriz);
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
    let ed = Rc::new(Editor { win: win.clone().upcast(), img: RefCell::new(shot), strokes: RefCell::new(vec![]), tool: Cell::new(Tool::Pen), pen: Cell::new((0.9, 0.1, 0.1)), area: area.clone() });
    {
        let e = ed.clone();
        area.connect_draw(move |_, cr| {
            cr.set_source_pixbuf(&e.img.borrow(), 0.0, 0.0);
            cr.paint().ok();
            draw_strokes(cr, &e.strokes.borrow());
            glib::Propagation::Proceed
        });
    }
    {
        let e = ed.clone();
        area.connect_button_press_event(move |a, ev| {
            let p = ev.position();
            match e.tool.get() {
                Tool::Pen => { let c = e.pen.get(); e.strokes.borrow_mut().push(Stroke { pts: vec![p], rgba: (c.0, c.1, c.2, 1.0), width: 2.0 }); }
                Tool::Highlighter => e.strokes.borrow_mut().push(Stroke { pts: vec![p], rgba: (1.0, 0.95, 0.0, 0.45), width: 14.0 }),
                Tool::Eraser => erase(&e, p),
            }
            a.queue_draw();
            glib::Propagation::Stop
        });
    }
    {
        let e = ed.clone();
        area.connect_motion_notify_event(move |a, ev| {
            let p = ev.position();
            if e.tool.get() == Tool::Eraser { erase(&e, p); } else if let Some(s) = e.strokes.borrow_mut().last_mut() { s.pts.push(p); }
            a.queue_draw();
            glib::Propagation::Stop
        });
    }
    fn erase(e: &Editor, p: (f64, f64)) {
        e.strokes.borrow_mut().retain(|s| !s.pts.iter().any(|q| (q.0 - p.0).hypot(q.1 - p.1) < s.width / 2.0 + 6.0));
        e.area.queue_draw();
    }
    // toolbar: New Snip | Save | Copy | Send | Pen ▾ | Highlighter | Eraser
    let new_b = tool_button("accessories-screenshot", "New Snip");
    { let (a, w) = (app.clone(), win.clone()); new_b.connect_clicked(move |_| { w.close(); start(&a, current_kind(), None); }); }
    tb.insert(&new_b, -1);
    tb.insert(&gtk::SeparatorToolItem::new(), -1);
    let save_b = tool_button("document-save", "Save Snip");
    { let e = ed.clone(); save_b.connect_clicked(move |_| e.save()); }
    tb.insert(&save_b, -1);
    let copy_b = tool_button("edit-copy", "Copy");
    { let e = ed.clone(); copy_b.connect_clicked(move |_| e.copy()); }
    tb.insert(&copy_b, -1);
    let send_b = tool_button("mail-send", "Send Snip");
    {
        let e = ed.clone();
        send_b.connect_clicked(move |_| {
            let p = glib::tmp_dir().join("Capture.png");
            let _ = e.flatten().savev(&p, "png", &[]);
            if !util::spawn(&["xdg-email", "--attach", &p.to_string_lossy()]) { util::open_uri("mailto:"); }
        });
    }
    tb.insert(&send_b, -1);
    tb.insert(&gtk::SeparatorToolItem::new(), -1);
    let pen = gtk::MenuToolButton::new(Some(&util::image("draw-freehand", 16)), Some("Pen"));
    let pen_menu = gtk::Menu::new();
    for (name, c) in [("Red Pen", (0.9, 0.1, 0.1)), ("Blue Pen", (0.1, 0.25, 0.85)), ("Black Pen", (0.0, 0.0, 0.0)), ("Custom Pen...", (-1.0, 0.0, 0.0))] {
        let it = gtk::MenuItem::with_label(name);
        let (e, w) = (ed.clone(), win.clone());
        it.connect_activate(move |_| {
            e.tool.set(Tool::Pen);
            if c.0 < 0.0 {
                let d = gtk::ColorChooserDialog::new(Some("Customize"), Some(&w));
                if d.run() == gtk::ResponseType::Ok { let r = d.rgba(); e.pen.set((r.red(), r.green(), r.blue())); }
                d.close();
            } else { e.pen.set(c); }
        });
        pen_menu.append(&it);
    }
    pen_menu.show_all();
    pen.set_menu(&pen_menu);
    { let e = ed.clone(); pen.connect_clicked(move |_| e.tool.set(Tool::Pen)); }
    tb.insert(&pen, -1);
    let hl = tool_button("format-text-highlight", "Highlighter");
    { let e = ed.clone(); hl.connect_clicked(move |_| e.tool.set(Tool::Highlighter)); }
    tb.insert(&hl, -1);
    let er = tool_button("draw-eraser", "Eraser");
    { let e = ed.clone(); er.connect_clicked(move |_| e.tool.set(Tool::Eraser)); }
    tb.insert(&er, -1);
    // menus
    let accel = gtk::AccelGroup::new();
    win.add_accel_group(&accel);
    let add = |m: &gtk::Menu, l: &str, acc: &str, f: Box<dyn Fn()>| {
        let it = gtk::MenuItem::with_mnemonic(l);
        if !acc.is_empty() { let (k, md) = gtk::accelerator_parse(acc); it.add_accelerator("activate", &accel, k, md, gtk::AccelFlags::VISIBLE); }
        it.connect_activate(move |_| f());
        m.append(&it);
    };
    let m = gtk::Menu::new();
    { let (a, w) = (app.clone(), win.clone()); add(&m, "_New Snip", "<Control>n", Box::new(move || { w.close(); start(&a, current_kind(), None); })); }
    { let e = ed.clone(); add(&m, "Save _As...", "<Control>s", Box::new(move || e.save())); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); add(&m, "E_xit", "", Box::new(move || w.close())); }
    let top = gtk::MenuItem::with_mnemonic("_File"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    { let e = ed.clone(); add(&m, "_Copy", "<Control>c", Box::new(move || e.copy())); }
    let top = gtk::MenuItem::with_mnemonic("_Edit"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    { let e = ed.clone(); add(&m, "_Pen", "", Box::new(move || e.tool.set(Tool::Pen))); }
    { let e = ed.clone(); add(&m, "_Highlighter", "", Box::new(move || e.tool.set(Tool::Highlighter))); }
    { let e = ed.clone(); add(&m, "_Eraser", "", Box::new(move || e.tool.set(Tool::Eraser))); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); add(&m, "_Options...", "", Box::new(move || options(w.upcast_ref()))); }
    let top = gtk::MenuItem::with_mnemonic("_Tools"); top.set_submenu(Some(&m)); menubar.append(&top);
    let m = gtk::Menu::new();
    { let w = win.clone(); add(&m, "_About Snipping Tool", "", Box::new(move || {
        let d = gtk::AboutDialog::new(); d.set_transient_for(Some(&w)); d.set_program_name("Snipping Tool");
        d.set_comments(Some("Aerode Snipping Tool – capture and annotate screen snips"));
        d.set_logo_icon_name(Some(util::first_icon(&["accessories-screenshot"]))); d.run(); d.close();
    })); }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);
    // Win7 copies every snip to the clipboard by default
    if opt("copy", true) { ed.copy(); }
    if let Some(m) = main { m.close(); }
    win.show_all();
    win.present();
}

fn opt(name: &str, default: bool) -> bool {
    std::fs::read_to_string(config::conf_dir().join("snip.conf")).ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix(&format!("{name}=")).map(|v| v == "1")))
        .unwrap_or(default)
}

fn set_opt(name: &str, v: bool) {
    let p = config::conf_dir().join("snip.conf");
    let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().filter(|l| !l.starts_with(&format!("{name}="))).map(String::from).collect();
    lines.push(format!("{name}={}", v as u8));
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(p, lines.join("\n"));
}

fn current_kind() -> Kind {
    match std::fs::read_to_string(config::conf_dir().join("snip.kind")).unwrap_or_default().trim() {
        "free" => Kind::Free, "window" => Kind::Window, "full" => Kind::Full, _ => Kind::Rect,
    }
}

fn options(parent: &gtk::Window) {
    let d = gtk::Dialog::with_buttons(Some("Snipping Tool Options"), Some(parent), gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(4);
    let items = [("copy", "Always copy snips to the Clipboard", true), ("ink", "Show selection ink after snips are captured", true),
        ("instr", "Display instruction text", true), ("prompt", "Prompt to save snips before exiting", true)];
    let checks: Vec<(&str, gtk::CheckButton)> = items.iter().map(|(k, l, def)| { let b = gtk::CheckButton::with_label(l); b.set_active(opt(k, *def)); c.pack_start(&b, false, false, 0); (*k, b) }).collect();
    d.show_all();
    if d.run() == gtk::ResponseType::Ok { for (k, b) in checks { set_opt(k, b.is_active()); } }
    d.close();
}

/// Begin a snip: hide our windows, grab the screen, let the user select.
fn start(app: &gtk::Application, kind: Kind, main: Option<gtk::Window>) {
    if let Some(m) = &main { m.hide(); }
    let app = app.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
        let Some(shot) = capture_screen() else { return };
        let (a, m) = (app.clone(), main.clone());
        select(kind, shot, Rc::new(move |res| match res {
            Some(img) => open_editor(&a, img, m.clone()),
            None => if let Some(m) = &m { m.show(); },
        }));
    });
}

fn main_window(app: &gtk::Application) {
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
    let kinds = gtk::Menu::new();
    for k in [Kind::Free, Kind::Rect, Kind::Window, Kind::Full] {
        let it = gtk::CheckMenuItem::with_label(k.label());
        it.set_draw_as_radio(true);
        it.set_active(current_kind() == k);
        let (a, w) = (app.clone(), win.clone());
        it.connect_activate(move |_| {
            let _ = std::fs::create_dir_all(config::conf_dir());
            let _ = std::fs::write(config::conf_dir().join("snip.kind"), match k { Kind::Free => "free", Kind::Rect => "rect", Kind::Window => "window", Kind::Full => "full" });
            start(&a, k, Some(w.clone().upcast()));
        });
        kinds.append(&it);
    }
    kinds.show_all();
    new_b.set_menu(&kinds);
    { let (a, w) = (app.clone(), win.clone()); new_b.connect_clicked(move |_| start(&a, current_kind(), Some(w.clone().upcast()))); }
    tb.insert(&new_b, -1);
    let cancel = gtk::ToolButton::new(None::<&gtk::Widget>, Some("Cancel"));
    cancel.set_is_important(true);
    cancel.set_sensitive(false);
    tb.insert(&cancel, -1);
    let opts = gtk::ToolButton::new(None::<&gtk::Widget>, Some("Options"));
    opts.set_is_important(true);
    { let w = win.clone(); opts.connect_clicked(move |_| options(w.upcast_ref())); }
    tb.insert(&opts, -1);
    root.pack_start(&tb, false, false, 0);
    if opt("instr", true) {
        let l = gtk::Label::new(Some("Choose a snip type from the arrow next to New,\nor click New to snip with the last type you used."));
        l.set_margin_start(12);
        l.set_margin_end(12);
        l.set_margin_top(10);
        l.set_margin_bottom(12);
        l.set_xalign(0.0);
        root.pack_start(&l, false, false, 0);
    }
    win.add(&root);
    win.show_all();
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--full") {
        // Print Screen: whole screen → clipboard (and Pictures/Screenshots), like Win7 PrtScn
        gtk::init().expect("gtk");
        let Some(shot) = capture_screen() else { return glib::ExitCode::FAILURE };
        let dir = util::special_dir(glib::UserDirectory::Pictures, "Pictures").join("Screenshots");
        let _ = std::fs::create_dir_all(&dir);
        let _ = shot.savev(dir.join(chrono::Local::now().format("Screenshot %Y-%m-%d %H.%M.%S.png").to_string()), "png", &[]);
        let cb = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
        cb.set_image(&shot);
        cb.store();
        // keep serving the clipboard until someone else owns it (or 10 minutes pass)
        cb.connect_local("owner-change", false, |_| { gtk::main_quit(); None });
        glib::timeout_add_seconds_local_once(600, gtk::main_quit);
        gtk::main();
        return glib::ExitCode::SUCCESS;
    }
    let quick = args.iter().any(|a| a == "--rect");
    let app = gtk::Application::new(Some("org.aerode.SnippingTool"), gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(move |app| {
        theme::init_app();
        if quick { let h = app.hold(); start(app, Kind::Rect, None); std::mem::forget(h); } else { main_window(app); }
    });
    app.run_with_args::<&str>(&[])
}
