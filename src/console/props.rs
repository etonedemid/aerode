//! The console's Properties and Defaults dialogs (Options / Font / Layout / Colors), Windows 7 layout.
//! Properties change the current window and are remembered for windows with its title; Defaults
//! ("Console Windows Properties") are used by windows that have no settings of their own.
use gtk::cairo;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::host::{self, raster_fonts, rgb, Console, ConsoleConfig};

type Cfg = Rc<RefCell<ConsoleConfig>>;

fn frame(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Frame {
    let f = gtk::Frame::new(Some(title));
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.set_border_width(8);
    b.pack_start(child, false, false, 0);
    f.add(&b);
    f
}

fn spin(v: f64, min: f64, max: f64) -> gtk::SpinButton {
    let s = gtk::SpinButton::with_range(min, max, 1.0);
    s.set_value(v);
    s.set_width_chars(5);
    s
}

fn label(t: &str) -> gtk::Label { let l = gtk::Label::with_mnemonic(t); l.set_xalign(0.0); l }

/// Two-column "Label: [spin]" grid.
fn spin_grid(rows: &[(&str, &gtk::SpinButton)]) -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(4);
    g.set_column_spacing(12);
    for (i, (l, s)) in rows.iter().enumerate() {
        let lab = label(l);
        lab.set_mnemonic_widget(Some(*s));
        lab.set_hexpand(true);
        g.attach(&lab, 0, i as i32, 1, 1);
        g.attach(*s, 1, i as i32, 1, 1);
    }
    g
}

/// The sample listing of the Font and Colors tabs.
const SAMPLE: [&str; 6] = [
    "C:\\AERODE> dir",
    "SYSTEM       <DIR>     11-02-26  4:00p",
    "FONTS        <DIR>     11-02-26  4:00p",
    "README   TXT      2048 11-02-26  4:00p",
    "AERODE   EXE    337232 11-02-26  4:00p",
    "WALLPAPR JPG    614400 11-02-26  4:00p",
];

/// Draw the sample in the configuration's font and the given colours.
fn draw_sample(cr: &cairo::Context, c: &ConsoleConfig, fg: u8, bg: u8, pango: &gtk::pango::Context) {
    let (r, g, b) = rgb(c.palette[bg as usize & 15]);
    cr.set_source_rgb(r, g, b);
    cr.paint().ok();
    let (r, g, b) = rgb(c.palette[fg as usize & 15]);
    cr.set_source_rgb(r, g, b);
    let (_, chh) = host::cell_size(c, pango);
    if c.font == "Raster Fonts" {
        if let Some(f) = host::raster_font(&c.raster_size) {
            for (i, l) in SAMPLE.iter().enumerate() { host::draw_raster_text(cr, &f, 2.0, 2.0 + i as f64 * chh, l); }
            return;
        }
    }
    let l = pangocairo::functions::create_layout(cr);
    l.set_font_description(Some(&host::ttf_description(c)));
    for (i, s) in SAMPLE.iter().enumerate() {
        l.set_text(s);
        cr.move_to(2.0, 2.0 + i as f64 * chh);
        pangocairo::functions::show_layout(cr, &l);
    }
}

/// "Window Preview": the screen with the console window drawn to scale.
fn window_preview(cfg: &Cfg, pango: gtk::pango::Context) -> gtk::DrawingArea {
    let da = gtk::DrawingArea::new();
    da.set_size_request(150, 110);
    let cfg = cfg.clone();
    da.connect_draw(move |da, cr| {
        let c = cfg.borrow();
        let r = crate::util::primary_geometry();
        let (sw, sh) = (r.width().max(1) as f64, r.height().max(1) as f64);
        let (aw, ah) = (da.allocated_width() as f64, da.allocated_height() as f64);
        let k = ((aw - 4.0) / sw).min((ah - 4.0) / sh);
        let (ox, oy) = ((aw - sw * k) / 2.0, (ah - sh * k) / 2.0);
        // the desktop
        cr.set_source_rgb(0.0, 0.5, 0.5);
        cr.rectangle(ox, oy, sw * k, sh * k);
        cr.fill().ok();
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.set_line_width(1.0);
        cr.rectangle(ox + 0.5, oy + 0.5, sw * k - 1.0, sh * k - 1.0);
        cr.stroke().ok();
        // the window: client area + frame + title bar
        let (cw, chh) = host::cell_size(&c, &pango);
        let (ww, wh) = (c.win_w.min(c.buffer_w) as f64 * cw + 17.0 + 16.0, c.win_h.min(c.buffer_h) as f64 * chh + 30.0 + 8.0);
        let (wx, wy) = if c.system_pos { ((sw - ww) / 2.0, (sh - wh) / 3.0) } else { (c.win_x as f64, c.win_y as f64) };
        let (x, y, w, h) = (ox + wx * k, oy + wy * k, (ww * k).max(6.0), (wh * k).max(6.0));
        cr.save().ok();
        cr.rectangle(ox, oy, sw * k, sh * k);
        cr.clip();
        cr.set_source_rgb(0.75, 0.75, 0.75);
        cr.rectangle(x, y, w, h);
        cr.fill().ok();
        cr.set_source_rgb(0.0, 0.0, 0.5);
        cr.rectangle(x + 1.0, y + 1.0, w - 2.0, (30.0 * k).max(2.0));
        cr.fill().ok();
        let (r, g, b) = rgb(c.palette[c.screen_bg as usize & 15]);
        cr.set_source_rgb(r, g, b);
        cr.rectangle(x + 1.0, y + 1.0 + (30.0 * k).max(2.0), w - 2.0, h - 2.0 - (30.0 * k).max(2.0));
        cr.fill().ok();
        cr.restore().ok();
        glib::Propagation::Proceed
    });
    da
}

pub fn show(con: &Rc<Console>, defaults: bool) {
    let cfg: Cfg = Rc::new(RefCell::new(if defaults { host::load_config() } else { con.config() }));
    let d = gtk::Dialog::new();
    d.set_title(&if defaults { "Console Windows Properties".to_string() } else { format!("\"{}\" Properties", con.title_key()) });
    d.set_transient_for(Some(&con.win));
    d.set_modal(true);
    d.set_resizable(false);
    d.set_type_hint(gdk::WindowTypeHint::Dialog);
    let nb = gtk::Notebook::new();
    nb.set_border_width(6);
    d.content_area().pack_start(&nb, true, true, 0);
    let pango = con.win.pango_context();
    // everything that previews the configuration
    let previews: Rc<RefCell<Vec<gtk::Widget>>> = Default::default();
    let redraw = { let p = previews.clone(); move || for w in p.borrow().iter() { w.queue_draw(); } };

    // ---------------- Options
    let opt = gtk::Grid::new();
    opt.set_border_width(10);
    opt.set_row_spacing(8);
    opt.set_column_spacing(8);
    opt.set_column_homogeneous(true);
    let radio_box = |items: &[&str], active: usize, on: Rc<dyn Fn(usize)>| -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let mut first: Option<gtk::RadioButton> = None;
        for (i, l) in items.iter().enumerate() {
            let r = match &first { None => gtk::RadioButton::with_mnemonic(l), Some(f) => gtk::RadioButton::with_mnemonic_from_widget(f, l) };
            if first.is_none() { first = Some(r.clone()); }
            if i == active { r.set_active(true); }
            let on = on.clone();
            r.connect_toggled(move |r| if r.is_active() { on(i) });
            b.pack_start(&r, false, false, 0);
        }
        b
    };
    let cur_i = match cfg.borrow().cursor_size { 100 => 2, 50 => 1, _ => 0 };
    let cursor = { let c = cfg.clone(); radio_box(&["_Small", "_Medium", "_Large"], cur_i, Rc::new(move |i| c.borrow_mut().cursor_size = [25, 50, 100][i])) };
    let disp = { let c = cfg.clone(); radio_box(&["_Window", "F_ull Screen"], cfg.borrow().full_screen as usize, Rc::new(move |i| c.borrow_mut().full_screen = i == 1)) };
    let hs = spin(cfg.borrow().history_size as f64, 1.0, 999.0);
    let hb = spin(cfg.borrow().history_buffers as f64, 1.0, 999.0);
    let hist = gtk::Box::new(gtk::Orientation::Vertical, 4);
    hist.pack_start(&spin_grid(&[("_Buffer Size:", &hs), ("_Number of Buffers:", &hb)]), false, false, 0);
    let dd = gtk::CheckButton::with_mnemonic("_Discard Old Duplicates");
    dd.set_active(cfg.borrow().discard_duplicates);
    hist.pack_start(&dd, false, false, 0);
    let edit = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let qe = gtk::CheckButton::with_mnemonic("_QuickEdit Mode");
    qe.set_active(cfg.borrow().quick_edit);
    let im = gtk::CheckButton::with_mnemonic("_Insert Mode");
    im.set_active(cfg.borrow().insert_mode);
    edit.pack_start(&qe, false, false, 0);
    edit.pack_start(&im, false, false, 0);
    opt.attach(&frame("Cursor Size", &cursor), 0, 0, 1, 1);
    opt.attach(&frame("Display Options", &disp), 1, 0, 1, 1);
    opt.attach(&frame("Command History", &hist), 0, 1, 1, 1);
    opt.attach(&frame("Edit Options", &edit), 1, 1, 1, 1);
    // Aerode: which program new console windows run (a default, so only in Defaults)
    let sh = gtk::ComboBoxText::with_entry();
    if defaults {
        for s in ["cmd", "fish", "bash", "zsh"] { sh.append(Some(s), if s == "cmd" { "cmd (Windows Command Processor)" } else { s }); }
        let cur_shell = cfg.borrow().shell.clone();
        if ["cmd", "fish", "bash", "zsh"].contains(&cur_shell.as_str()) { sh.set_active_id(Some(&cur_shell)); }
        else if let Some(e) = sh.child().and_then(|c| c.downcast::<gtk::Entry>().ok()) { e.set_text(&cur_shell); }
        opt.attach(&frame("Program run by new console windows", &sh), 0, 2, 2, 1);
    }
    nb.append_page(&opt, Some(&gtk::Label::with_mnemonic("_Options")));

    // ---------------- Font
    let fontp = gtk::Grid::new();
    fontp.set_border_width(10);
    fontp.set_row_spacing(6);
    fontp.set_column_spacing(10);
    let rasters = raster_fonts();
    let sizes = gtk::ListBox::new();
    sizes.set_selection_mode(gtk::SelectionMode::Single);
    let fonts = gtk::ListBox::new();
    for f in ["Consolas", "Lucida Console", "Raster Fonts"] {
        let r = gtk::ListBoxRow::new();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let tt = gtk::Label::new(None);
        tt.set_markup(if f == "Raster Fonts" { "<span font='8'>  </span>" } else { "<span font='8' weight='bold' foreground='#7a7a7a'>T</span><span font='8' weight='bold' foreground='#404040' rise='-2000'>T</span>" });
        tt.set_width_chars(3);
        row.pack_start(&tt, false, false, 0);
        let l = gtk::Label::new(Some(f));
        l.set_xalign(0.0);
        row.pack_start(&l, true, true, 0);
        r.add(&row);
        unsafe { r.set_data("font", f.to_string()); }
        fonts.add(&r);
    }
    let bold = gtk::CheckButton::with_mnemonic("_Bold fonts");
    bold.set_active(cfg.borrow().bold);
    let sel_frame = gtk::Frame::new(None);
    let each = gtk::Label::new(None);
    each.set_xalign(0.0);
    let update_font_labels = {
        let (cfg, sel_frame, each, bold, pango) = (cfg.clone(), sel_frame.clone(), each.clone(), bold.clone(), pango.clone());
        move || {
            let c = cfg.borrow();
            sel_frame.set_label(Some(&format!("Selected Font : {}", if c.font == "Raster Fonts" { "Terminal" } else { &c.font })));
            let (w, h) = host::cell_size(&c, &pango);
            each.set_text(&format!("Each character is:\n{:>3} screen pixels wide\n{:>3} screen pixels high", w as u32, h as u32));
            bold.set_sensitive(c.font != "Raster Fonts");
        }
    };
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
                l.set_margin_start(4);
                r.add(&l);
                unsafe { r.set_data("v", it.clone()); }
                sizes.add(&r);
                let selected = if c.font == "Raster Fonts" { it.replace(' ', "") == c.raster_size } else { it == c.ttf_size.to_string() };
                if selected { sizes.select_row(Some(&r)); }
            }
            sizes.show_all();
        }
    };
    fill_sizes();
    update_font_labels();
    for r in fonts.children() {
        let r = r.downcast::<gtk::ListBoxRow>().unwrap();
        if unsafe { r.data::<String>("font").map(|p| p.as_ref().clone()) } == Some(cfg.borrow().font.clone()) { fonts.select_row(Some(&r)); }
    }
    let sample = gtk::DrawingArea::new();
    sample.set_size_request(250, 92);
    {
        let (cfg, pango) = (cfg.clone(), pango.clone());
        sample.connect_draw(move |_, cr| { let c = cfg.borrow(); draw_sample(cr, &c, c.screen_fg, c.screen_bg, &pango); glib::Propagation::Proceed });
    }
    {
        let (cfg, fill, up, rd) = (cfg.clone(), fill_sizes.clone(), update_font_labels.clone(), redraw.clone());
        fonts.connect_row_selected(move |_, r| {
            let Some(f) = r.and_then(|r| unsafe { r.data::<String>("font").map(|p| p.as_ref().clone()) }) else { return };
            if cfg.borrow().font == f { return; }
            cfg.borrow_mut().font = f;
            fill(); up(); rd();
        });
    }
    {
        let (cfg, up, rd) = (cfg.clone(), update_font_labels.clone(), redraw.clone());
        sizes.connect_row_selected(move |_, r| {
            let Some(v) = r.and_then(|r| unsafe { r.data::<String>("v").map(|p| p.as_ref().clone()) }) else { return };
            { let mut c = cfg.borrow_mut(); if c.font == "Raster Fonts" { c.raster_size = v.replace(' ', ""); } else if let Ok(n) = v.parse() { c.ttf_size = n; } }
            up(); rd();
        });
    }
    { let (cfg, up, rd) = (cfg.clone(), update_font_labels.clone(), redraw.clone()); bold.connect_toggled(move |b| { cfg.borrow_mut().bold = b.is_active(); up(); rd(); }); }
    let wsw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    wsw.set_size_request(110, 110);
    wsw.set_shadow_type(gtk::ShadowType::In);
    wsw.add(&sizes);
    let fsw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    fsw.set_size_request(-1, 66);
    fsw.set_shadow_type(gtk::ShadowType::In);
    fsw.add(&fonts);
    let prev1 = window_preview(&cfg, pango.clone());
    fontp.attach(&frame("Window Preview", &prev1), 0, 0, 1, 2);
    let sl = label("_Size");
    sl.set_mnemonic_widget(Some(&sizes));
    fontp.attach(&sl, 1, 0, 1, 1);
    fontp.attach(&wsw, 1, 1, 1, 1);
    let fl = label("_Font");
    fl.set_mnemonic_widget(Some(&fonts));
    fontp.attach(&fl, 0, 2, 2, 1);
    let frow = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    frow.pack_start(&fsw, true, true, 0);
    frow.pack_start(&bold, false, false, 0);
    fontp.attach(&frow, 0, 3, 2, 1);
    let sbox = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    sbox.set_border_width(8);
    sbox.pack_start(&sample, false, false, 0);
    sbox.pack_start(&each, false, false, 0);
    sel_frame.add(&sbox);
    fontp.attach(&sel_frame, 0, 4, 2, 1);
    nb.append_page(&fontp, Some(&gtk::Label::with_mnemonic("_Font")));

    // ---------------- Layout
    let lay = gtk::Grid::new();
    lay.set_border_width(10);
    lay.set_row_spacing(8);
    lay.set_column_spacing(10);
    let c0 = cfg.borrow().clone();
    let (bw, bh) = (spin(c0.buffer_w as f64, 1.0, 9999.0), spin(c0.buffer_h as f64, 1.0, 9999.0));
    let (ww, wh) = (spin(c0.win_w as f64, 1.0, 9999.0), spin(c0.win_h as f64, 1.0, 9999.0));
    let (wl, wt) = (spin(c0.win_x as f64, -9999.0, 9999.0), spin(c0.win_y as f64, -9999.0, 9999.0));
    let syspos = gtk::CheckButton::with_mnemonic("Let system _position window");
    syspos.set_active(c0.system_pos);
    wl.set_sensitive(!c0.system_pos);
    wt.set_sensitive(!c0.system_pos);
    // Win7 keeps the window inside the buffer: a smaller buffer shrinks the window and vice versa
    { let ww = ww.clone(); bw.connect_value_changed(move |b| if ww.value() > b.value() { ww.set_value(b.value()) }); }
    { let wh = wh.clone(); bh.connect_value_changed(move |b| if wh.value() > b.value() { wh.set_value(b.value()) }); }
    { let bw = bw.clone(); ww.connect_value_changed(move |w| if w.value() > bw.value() { bw.set_value(w.value()) }); }
    { let bh = bh.clone(); wh.connect_value_changed(move |w| if w.value() > bh.value() { bh.set_value(w.value()) }); }
    {
        let (cfg, rd) = (cfg.clone(), redraw.clone());
        let (bw2, bh2, ww2, wh2, wl2, wt2) = (bw.clone(), bh.clone(), ww.clone(), wh.clone(), wl.clone(), wt.clone());
        let sync = Rc::new(move || {
            let mut c = cfg.borrow_mut();
            c.buffer_w = bw2.value() as usize; c.buffer_h = bh2.value() as usize;
            c.win_w = ww2.value() as usize; c.win_h = wh2.value() as usize;
            c.win_x = wl2.value() as i32; c.win_y = wt2.value() as i32;
            drop(c);
            rd();
        });
        for s in [&bw, &bh, &ww, &wh, &wl, &wt] { let f = sync.clone(); s.connect_value_changed(move |_| f()); }
    }
    {
        let (cfg, rd, wl, wt) = (cfg.clone(), redraw.clone(), wl.clone(), wt.clone());
        syspos.connect_toggled(move |b| { cfg.borrow_mut().system_pos = b.is_active(); wl.set_sensitive(!b.is_active()); wt.set_sensitive(!b.is_active()); rd(); });
    }
    let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
    left.pack_start(&frame("Screen Buffer Size", &spin_grid(&[("_Width:", &bw), ("_Height:", &bh)])), false, false, 0);
    left.pack_start(&frame("Window Size", &spin_grid(&[("W_idth:", &ww), ("H_eight:", &wh)])), false, false, 0);
    let posb = gtk::Box::new(gtk::Orientation::Vertical, 4);
    posb.pack_start(&spin_grid(&[("_Left:", &wl), ("_Top:", &wt)]), false, false, 0);
    posb.pack_start(&syspos, false, false, 0);
    left.pack_start(&frame("Window Position", &posb), false, false, 0);
    lay.attach(&left, 0, 0, 1, 1);
    let prev2 = window_preview(&cfg, pango.clone());
    prev2.set_size_request(170, 150);
    lay.attach(&frame("Window Preview", &prev2), 1, 0, 1, 1);
    nb.append_page(&lay, Some(&gtk::Label::with_mnemonic("_Layout")));

    // ---------------- Colors
    let col = gtk::Grid::new();
    col.set_border_width(10);
    col.set_row_spacing(8);
    col.set_column_spacing(16);
    let target = Rc::new(Cell::new(0u8)); // 0 screen text, 1 screen background, 2 popup text, 3 popup background
    let current = |c: &ConsoleConfig, t: u8| match t { 0 => c.screen_fg, 1 => c.screen_bg, 2 => c.popup_fg, _ => c.popup_bg };
    let (rs, gs, bs) = (spin(0.0, 0.0, 255.0), spin(0.0, 0.0, 255.0), spin(0.0, 0.0, 255.0));
    let syncing = Rc::new(Cell::new(false));
    let sync_rgb = {
        let (cfg, rs, gs, bs, t, syncing) = (cfg.clone(), rs.clone(), gs.clone(), bs.clone(), target.clone(), syncing.clone());
        Rc::new(move || {
            let c = cfg.borrow();
            let v = c.palette[current(&c, t.get()) as usize & 15];
            drop(c);
            syncing.set(true);
            rs.set_value(((v >> 16) & 255) as f64); gs.set_value(((v >> 8) & 255) as f64); bs.set_value((v & 255) as f64);
            syncing.set(false);
        })
    };
    let radios = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let mut first: Option<gtk::RadioButton> = None;
    for (i, l) in ["Screen _Text", "Screen _Background", "_Popup Text", "Pop_up Background"].iter().enumerate() {
        let r = match &first { None => gtk::RadioButton::with_mnemonic(l), Some(f) => gtk::RadioButton::with_mnemonic_from_widget(f, l) };
        if first.is_none() { first = Some(r.clone()); }
        let (t, sy, rd) = (target.clone(), sync_rgb.clone(), redraw.clone());
        r.connect_toggled(move |r| if r.is_active() { t.set(i as u8); sy(); rd(); });
        radios.pack_start(&r, false, false, 0);
    }
    col.attach(&radios, 0, 0, 1, 1);
    let rgbg = spin_grid(&[("_Red:", &rs), ("_Green:", &gs), ("B_lue:", &bs)]);
    col.attach(&frame("Selected Color Values", &rgbg), 1, 0, 1, 1);
    // the 16 swatches; the one in use for the chosen item is framed
    let swatches = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    for i in 0..16u8 {
        let da = gtk::DrawingArea::new();
        da.set_size_request(22, 22);
        da.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
        {
            let (cfg, t) = (cfg.clone(), target.clone());
            da.connect_draw(move |da, cr| {
                let c = cfg.borrow();
                let (w, h) = (da.allocated_width() as f64, da.allocated_height() as f64);
                if current(&c, t.get()) == i {
                    cr.set_source_rgb(0.0, 0.0, 0.0);
                    cr.set_line_width(1.0);
                    cr.rectangle(0.5, 0.5, w - 1.0, h - 1.0);
                    cr.stroke().ok();
                }
                cr.set_source_rgb(0.5, 0.5, 0.5);
                cr.rectangle(2.5, 2.5, w - 5.0, h - 5.0);
                cr.stroke().ok();
                let (r, g, b) = rgb(c.palette[i as usize]);
                cr.set_source_rgb(r, g, b);
                cr.rectangle(3.0, 3.0, w - 6.0, h - 6.0);
                cr.fill().ok();
                glib::Propagation::Proceed
            });
        }
        {
            let (cfg, t, sy, rd) = (cfg.clone(), target.clone(), sync_rgb.clone(), redraw.clone());
            da.connect_button_press_event(move |_, _| {
                { let mut c = cfg.borrow_mut(); match t.get() { 0 => c.screen_fg = i, 1 => c.screen_bg = i, 2 => c.popup_fg = i, _ => c.popup_bg = i } }
                sy(); rd();
                glib::Propagation::Stop
            });
        }
        previews.borrow_mut().push(da.clone().upcast());
        swatches.pack_start(&da, false, false, 0);
    }
    col.attach(&swatches, 0, 1, 2, 1);
    {
        let (cfg, rs2, gs2, bs2, t, syncing, rd) = (cfg.clone(), rs.clone(), gs.clone(), bs.clone(), target.clone(), syncing.clone(), redraw.clone());
        let apply = Rc::new(move || {
            if syncing.get() { return; }
            let mut c = cfg.borrow_mut();
            let idx = current(&c, t.get()) as usize & 15;
            c.palette[idx] = ((rs2.value() as u32) << 16) | ((gs2.value() as u32) << 8) | bs2.value() as u32;
            drop(c);
            rd();
        });
        for s in [&rs, &gs, &bs] { let a = apply.clone(); s.connect_value_changed(move |_| a()); }
    }
    let mk_sample = |popup: bool| {
        let da = gtk::DrawingArea::new();
        da.set_size_request(-1, 70);
        let (cfg, pango) = (cfg.clone(), pango.clone());
        da.connect_draw(move |_, cr| {
            let c = cfg.borrow();
            let (f, b) = if popup { (c.popup_fg, c.popup_bg) } else { (c.screen_fg, c.screen_bg) };
            draw_sample(cr, &c, f, b, &pango);
            glib::Propagation::Proceed
        });
        da
    };
    let (ss, ps) = (mk_sample(false), mk_sample(true));
    col.attach(&frame("Selected Screen Colors", &ss), 0, 2, 2, 1);
    col.attach(&frame("Selected Popup Colors", &ps), 0, 3, 2, 1);
    sync_rgb();
    nb.append_page(&col, Some(&gtk::Label::with_mnemonic("_Colors")));
    previews.borrow_mut().extend([prev1.upcast::<gtk::Widget>(), prev2.upcast(), sample.upcast(), ss.upcast(), ps.upcast()]);

    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.set_default_response(gtk::ResponseType::Ok);
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
            c.win_x = wl.value() as i32;
            c.win_y = wt.value() as i32;
            if defaults {
                c.shell = sh.active_id().map(|s| s.to_string()).or_else(|| sh.child().and_then(|e| e.downcast::<gtk::Entry>().ok()).map(|e| e.text().to_string())).filter(|s| !s.is_empty()).unwrap_or_else(|| "cmd".into());
                host::save_config(&c);
            } else {
                con.set_config(c);
            }
        }
        d.close();
    });
    d.show_all();
}
