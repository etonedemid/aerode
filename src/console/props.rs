//! Console Properties dialog (Options / Font / Layout / Colors), Windows 7 layout.
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::host::{raster_fonts, Console, ConsoleConfig, WIN_PALETTE};

fn frame(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    let f = gtk::Frame::new(Some(title));
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.set_border_width(8);
    b.pack_start(child, false, false, 0);
    f.add(&b);
    f
}

fn spin(v: usize, max: f64) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(1.0, max, 1.0);
    s.set_value(v as f64);
    s
}

pub fn show(con: &Rc<Console>) {
    let cfg = Rc::new(RefCell::new(con.config()));
    let title = con.win.title().map(|t| t.to_string()).unwrap_or_default();
    let d = gtk::Dialog::new();
    d.set_title(&format!("\"{}\" Properties", title.trim_start_matches("Mark ").trim_start_matches("Select ")));
    d.set_transient_for(Some(&con.win));
    d.set_resizable(false);
    let nb = gtk::Notebook::new();
    nb.set_border_width(6);
    d.content_area().pack_start(&nb, true, true, 0);

    // ---------------- Options
    let opt = gtk::Box::new(gtk::Orientation::Vertical, 8);
    opt.set_border_width(10);
    let cur = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let small = gtk::RadioButton::with_label("Small");
    let medium = gtk::RadioButton::with_label_from_widget(&small, "Medium");
    let large = gtk::RadioButton::with_label_from_widget(&small, "Large");
    match cfg.borrow().cursor_size { 100 => large.set_active(true), 50 => medium.set_active(true), _ => small.set_active(true) }
    for (r, v) in [(&small, 25u32), (&medium, 50), (&large, 100)] {
        let c = cfg.clone();
        r.connect_toggled(move |r| if r.is_active() { c.borrow_mut().cursor_size = v });
        cur.pack_start(r, false, false, 0);
    }
    let hist = gtk::Grid::new();
    hist.set_row_spacing(4);
    hist.set_column_spacing(8);
    let hs = spin(cfg.borrow().history_size as usize, 999.0);
    let hb = spin(cfg.borrow().history_buffers as usize, 999.0);
    let dd = gtk::CheckButton::with_label("Discard Old Duplicates");
    dd.set_active(cfg.borrow().discard_duplicates);
    hist.attach(&gtk::Label::new(Some("Buffer Size:")), 0, 0, 1, 1);
    hist.attach(&hs, 1, 0, 1, 1);
    hist.attach(&gtk::Label::new(Some("Number of Buffers:")), 0, 1, 1, 1);
    hist.attach(&hb, 1, 1, 1, 1);
    hist.attach(&dd, 0, 2, 2, 1);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.pack_start(&frame("Cursor Size", &cur), true, true, 0);
    top.pack_start(&frame("Command History", &hist), true, true, 0);
    opt.pack_start(&top, false, false, 0);
    let edit = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let qe = gtk::CheckButton::with_label("QuickEdit Mode");
    qe.set_active(cfg.borrow().quick_edit);
    let im = gtk::CheckButton::with_label("Insert Mode");
    im.set_active(cfg.borrow().insert_mode);
    edit.pack_start(&qe, false, false, 0);
    edit.pack_start(&im, false, false, 0);
    opt.pack_start(&frame("Edit Options", &edit), false, false, 0);
    // not in Win7, but needed to run fish & friends in this window
    let sh = gtk::ComboBoxText::with_entry();
    for s in ["cmd", "fish", "bash", "zsh"] { sh.append(Some(s), if s == "cmd" { "cmd (Windows Command Processor)" } else { s }); }
    let cur_shell = cfg.borrow().shell.clone();
    if ["cmd", "fish", "bash", "zsh"].contains(&cur_shell.as_str()) { sh.set_active_id(Some(&cur_shell)); }
    else if let Some(e) = sh.child().and_then(|c| c.downcast::<gtk::Entry>().ok()) { e.set_text(&cur_shell); }
    opt.pack_start(&frame("Shell (takes effect in new windows)", &sh), false, false, 0);
    nb.append_page(&opt, Some(&gtk::Label::new(Some("Options"))));

    // ---------------- Font
    let fontp = gtk::Box::new(gtk::Orientation::Vertical, 8);
    fontp.set_border_width(10);
    let rasters = raster_fonts();
    let sizes = gtk::ListBox::new();
    sizes.set_selection_mode(gtk::SelectionMode::Single);
    let fonts = gtk::ListBox::new();
    for f in ["Consolas", "Lucida Console", "Raster Fonts"] {
        let r = gtk::ListBoxRow::new();
        let l = gtk::Label::new(Some(&format!("{} {f}", if f == "Raster Fonts" { "▪" } else { "TT" })));
        l.set_xalign(0.0);
        r.add(&l);
        unsafe { r.set_data("font", f.to_string()); }
        fonts.add(&r);
    }
    let fill_sizes = {
        let (sizes, cfg) = (sizes.clone(), cfg.clone());
        let rasters = rasters.clone();
        move || {
            for c in sizes.children() { sizes.remove(&c); }
            let c = cfg.borrow().clone();
            let items: Vec<String> = if c.font == "Raster Fonts" { rasters.iter().map(|f| format!("{} x {}", f.width, f.height)).collect() }
                else { [5, 6, 7, 8, 10, 12, 14, 16, 18, 20, 24, 28, 36, 72].iter().map(|n| n.to_string()).collect() };
            for it in items {
                let r = gtk::ListBoxRow::new();
                let l = gtk::Label::new(Some(&it));
                l.set_xalign(0.0);
                r.add(&l);
                let selected = if c.font == "Raster Fonts" { it.replace(' ', "") == c.raster_size } else { it == c.ttf_size.to_string() };
                sizes.add(&r);
                if selected { sizes.select_row(Some(&r)); }
                unsafe { r.set_data("v", it.clone()); }
            }
            sizes.show_all();
        }
    };
    fill_sizes();
    for r in fonts.children() {
        let r = r.downcast::<gtk::ListBoxRow>().unwrap();
        if unsafe { r.data::<String>("font").map(|p| p.as_ref().clone()) } == Some(cfg.borrow().font.clone()) { fonts.select_row(Some(&r)); }
    }
    let preview = gtk::DrawingArea::new();
    preview.set_size_request(240, 90);
    {
        let cfg = cfg.clone();
        preview.connect_draw(move |_, cr| {
            let c = cfg.borrow();
            let (r, g, b) = rgbf(c.palette[c.screen_bg as usize & 15]);
            cr.set_source_rgb(r, g, b);
            cr.paint().ok();
            let (r, g, b) = rgbf(c.palette[c.screen_fg as usize & 15]);
            cr.set_source_rgb(r, g, b);
            cr.select_font_face(if c.font == "Raster Fonts" { "Lucida Console" } else { &c.font }, gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(12.0);
            for (i, l) in ["C:\\WINDOWS> dir", "SYSTEM       <DIR>", "SYSTEM32     <DIR>", "README   TXT     26926"].iter().enumerate() { cr.move_to(4.0, 14.0 + i as f64 * 14.0); cr.show_text(l).ok(); }
            glib::Propagation::Proceed
        });
    }
    {
        let (cfg, fill, p) = (cfg.clone(), fill_sizes.clone(), preview.clone());
        fonts.connect_row_selected(move |_, r| {
            if let Some(r) = r { if let Some(f) = unsafe { r.data::<String>("font").map(|p| p.as_ref().clone()) } { cfg.borrow_mut().font = f; fill(); p.queue_draw(); } }
        });
    }
    {
        let (cfg, p) = (cfg.clone(), preview.clone());
        sizes.connect_row_selected(move |_, r| {
            let Some(r) = r else { return };
            let Some(v) = (unsafe { r.data::<String>("v").map(|p| p.as_ref().clone()) }) else { return };
            let mut c = cfg.borrow_mut();
            if c.font == "Raster Fonts" { c.raster_size = v.replace(' ', ""); } else if let Ok(n) = v.parse() { c.ttf_size = n; }
            drop(c);
            p.queue_draw();
        });
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let wsw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    wsw.set_size_request(120, 110);
    wsw.add(&sizes);
    row.pack_start(&frame("Window Preview", &gtk::Label::new(Some(" "))), true, true, 0);
    row.pack_start(&frame("Size", &wsw), false, false, 0);
    fontp.pack_start(&row, false, false, 0);
    fontp.pack_start(&frame("Font", &fonts), false, false, 0);
    fontp.pack_start(&frame("Selected Font", &preview), false, false, 0);
    nb.append_page(&fontp, Some(&gtk::Label::new(Some("Font"))));

    // ---------------- Layout
    let lay = gtk::Box::new(gtk::Orientation::Vertical, 8);
    lay.set_border_width(10);
    let (bw, bh) = (spin(cfg.borrow().buffer_w, 9999.0), spin(cfg.borrow().buffer_h, 9999.0));
    let (ww, wh) = (spin(cfg.borrow().win_w, 9999.0), spin(cfg.borrow().win_h, 9999.0));
    let g1 = gtk::Grid::new();
    g1.set_row_spacing(4);
    g1.set_column_spacing(8);
    g1.attach(&gtk::Label::new(Some("Width:")), 0, 0, 1, 1); g1.attach(&bw, 1, 0, 1, 1);
    g1.attach(&gtk::Label::new(Some("Height:")), 0, 1, 1, 1); g1.attach(&bh, 1, 1, 1, 1);
    let g2 = gtk::Grid::new();
    g2.set_row_spacing(4);
    g2.set_column_spacing(8);
    g2.attach(&gtk::Label::new(Some("Width:")), 0, 0, 1, 1); g2.attach(&ww, 1, 0, 1, 1);
    g2.attach(&gtk::Label::new(Some("Height:")), 0, 1, 1, 1); g2.attach(&wh, 1, 1, 1, 1);
    // Win7 keeps window width <= buffer width
    { let ww = ww.clone(); bw.connect_value_changed(move |b| if ww.value() > b.value() { ww.set_value(b.value()) }); }
    { let bw = bw.clone(); ww.connect_value_changed(move |w| if w.value() > bw.value() { bw.set_value(w.value()) }); }
    lay.pack_start(&frame("Screen Buffer Size", &g1), false, false, 0);
    lay.pack_start(&frame("Window Size", &g2), false, false, 0);
    let pos = gtk::CheckButton::with_label("Let system position window");
    pos.set_active(true);
    lay.pack_start(&frame("Window Position", &pos), false, false, 0);
    nb.append_page(&lay, Some(&gtk::Label::new(Some("Layout"))));

    // ---------------- Colors
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.set_border_width(10);
    let target = Rc::new(Cell::new(0u8)); // 0 text, 1 bg, 2 popup text, 3 popup bg
    let radios = gtk::Grid::new();
    radios.set_column_spacing(16);
    let mut first: Option<gtk::RadioButton> = None;
    let swatch_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let (rs, gs, bs) = (gtk::SpinButton::with_range(0.0, 255.0, 1.0), gtk::SpinButton::with_range(0.0, 255.0, 1.0), gtk::SpinButton::with_range(0.0, 255.0, 1.0));
    let selected_idx = Rc::new(Cell::new(cfg.borrow().screen_fg));
    let sync_rgb = {
        let (cfg, rs, gs, bs, sel) = (cfg.clone(), rs.clone(), gs.clone(), bs.clone(), selected_idx.clone());
        move || { let c = cfg.borrow().palette[sel.get() as usize & 15]; rs.set_value(((c >> 16) & 255) as f64); gs.set_value(((c >> 8) & 255) as f64); bs.set_value((c & 255) as f64); }
    };
    for (i, l) in ["Screen Text", "Screen Background", "Popup Text", "Popup Background"].iter().enumerate() {
        let r = match &first { None => gtk::RadioButton::with_label(l), Some(f) => gtk::RadioButton::with_label_from_widget(f, l) };
        if first.is_none() { first = Some(r.clone()); }
        let (t, cfg, sel, sy) = (target.clone(), cfg.clone(), selected_idx.clone(), sync_rgb.clone());
        r.connect_toggled(move |r| if r.is_active() {
            t.set(i as u8);
            let c = cfg.borrow();
            sel.set(match i { 0 => c.screen_fg, 1 => c.screen_bg, 2 => c.popup_fg, _ => c.popup_bg });
            drop(c);
            sy();
        });
        radios.attach(&r, (i % 2) as i32, (i / 2) as i32, 1, 1);
    }
    col.pack_start(&radios, false, false, 0);
    for i in 0..16u8 {
        let b = gtk::Button::new();
        let da = gtk::DrawingArea::new();
        da.set_size_request(18, 18);
        let c2 = cfg.clone();
        da.connect_draw(move |_, cr| { let (r, g, b) = rgbf(c2.borrow().palette[i as usize]); cr.set_source_rgb(r, g, b); cr.paint().ok(); glib::Propagation::Proceed });
        b.add(&da);
        b.set_relief(gtk::ReliefStyle::None);
        let (t, cfg, sel, sy, pv) = (target.clone(), cfg.clone(), selected_idx.clone(), sync_rgb.clone(), preview.clone());
        b.connect_clicked(move |_| {
            sel.set(i);
            let mut c = cfg.borrow_mut();
            match t.get() { 0 => c.screen_fg = i, 1 => c.screen_bg = i, 2 => c.popup_fg = i, _ => c.popup_bg = i }
            drop(c);
            sy();
            pv.queue_draw();
        });
        swatch_box.pack_start(&b, false, false, 0);
    }
    col.pack_start(&swatch_box, false, false, 0);
    let rgbrow = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for (l, s) in [("Red:", &rs), ("Green:", &gs), ("Blue:", &bs)] { rgbrow.pack_start(&gtk::Label::new(Some(l)), false, false, 0); rgbrow.pack_start(s, false, false, 0); }
    let apply_rgb = {
        let (cfg, rs, gs, bs, sel, sw) = (cfg.clone(), rs.clone(), gs.clone(), bs.clone(), selected_idx.clone(), swatch_box.clone());
        move || { cfg.borrow_mut().palette[sel.get() as usize & 15] = ((rs.value() as u32) << 16) | ((gs.value() as u32) << 8) | bs.value() as u32; sw.queue_draw(); }
    };
    for s in [&rs, &gs, &bs] { let a = apply_rgb.clone(); s.connect_value_changed(move |_| a()); }
    col.pack_start(&frame("Selected Color Values", &rgbrow), false, false, 0);
    sync_rgb();
    let reset = gtk::Button::with_label("Restore default colors");
    { let (cfg, sw) = (cfg.clone(), swatch_box.clone()); reset.connect_clicked(move |_| { let mut c = cfg.borrow_mut(); c.palette = WIN_PALETTE; c.screen_fg = 7; c.screen_bg = 0; c.popup_fg = 5; c.popup_bg = 15; drop(c); sw.queue_draw(); }); }
    reset.set_halign(gtk::Align::Start);
    col.pack_start(&reset, false, false, 0);
    nb.append_page(&col, Some(&gtk::Label::new(Some("Colors"))));

    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let con = con.clone();
    d.connect_response(move |d, r| {
        if r == gtk::ResponseType::Ok {
            let mut c: ConsoleConfig = cfg.borrow().clone();
            c.history_size = hs.value() as u32;
            c.history_buffers = hb.value() as u32;
            c.discard_duplicates = dd.is_active();
            c.quick_edit = qe.is_active();
            c.insert_mode = im.is_active();
            c.buffer_w = bw.value() as usize;
            c.buffer_h = bh.value() as usize;
            c.win_w = (ww.value() as usize).min(c.buffer_w);
            c.win_h = (wh.value() as usize).min(c.buffer_h);
            c.shell = sh.active_id().map(|s| s.to_string()).or_else(|| sh.child().and_then(|e| e.downcast::<gtk::Entry>().ok()).map(|e| e.text().to_string())).filter(|s| !s.is_empty()).unwrap_or_else(|| "cmd".into());
            con.set_config(c);
        }
        d.close();
    });
    d.show_all();
    let _ = gdk::Display::default();
}

fn rgbf(c: u32) -> (f64, f64, f64) { (((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0) }
