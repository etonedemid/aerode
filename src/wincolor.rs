//! Personalization > Window Color and Appearance (Win7 themecpl.dll's colour page).
//!
//! Every control previews at once: the edited colourisation is published as the root property
//! `_AERODE_COLORIZATION` that the WM, the shell and every glass painter watch. "Save changes"
//! writes config.json; "Cancel" (and closing the window without saving) puts the property back.
use gtk::prelude::*;
use gtk::{cairo, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::dwmcolor::{hsb_to_rgb, intensity_to_alpha, Colorization, SWATCHES};
use crate::{config, glass};

const CSS: &str = "
.wincolor { background-color: #fff; }
.wincolor .wc-title { color: #003399; font-size: 12pt; font-weight: normal; }
.wincolor .current-name { color: #808080; }
.wincolor .content-link { color: #0066cc; padding: 0; border: none; background: none; box-shadow: none; min-height: 0; }
.wincolor .content-link:hover { color: #3399ff; text-decoration: underline; }
.wincolor .content-link label { color: inherit; }
.wincolor .expando { padding: 0; border: none; background: none; box-shadow: none; min-height: 0; min-width: 0; }
.wincolor-footer { background-color: #f0f0f0; border-top: 1px solid #dfdfdf; }
.wincolor scale highlight { border: none; background: none; }
.wincolor-footer button { min-width: 73px; }
#wincolor-hue trough { background-image: linear-gradient(to right, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000); }
";

/// Win7 swatch tile: 59 px rounded square in a 77 px cell (the cell shows the selection).
const CELL: i32 = 77;
const TILE: f64 = 59.0;

struct Page {
    /// the property when the page was opened or last saved (Cancel restores it)
    saved: Cell<Option<Colorization>>,
    cur: Cell<Colorization>,
    /// mixer position (kept apart from the rgb so that e.g. hue survives brightness 0)
    hsb: Cell<[f64; 3]>,
    syncing: Cell<bool>,
    hover: Cell<Option<usize>>,
    tiles: Vec<gtk::DrawingArea>,
    current: gtk::Label,
    transparency: gtk::CheckButton,
    intensity: gtk::Scale,
    mixer: [gtk::Scale; 3],
    mixer_css: gtk::CssProvider,
}

fn rgb_of(argb: u32) -> u32 { argb & 0x00FF_FFFF }

fn swatch_index(c: &Colorization) -> Option<usize> { SWATCHES.iter().position(|(_, s)| rgb_of(*s) == rgb_of(c.color)) }

fn percent(argb: u32) -> f64 { ((argb >> 24) as f64 * 100.0 / 255.0).round() }

/// Inverse of `hsb_to_rgb` (hue 0..360, saturation and brightness 0..1).
fn rgb_to_hsb(rgb: u32) -> [f64; 3] {
    let [r, g, b] = [(rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF].map(|v| v as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        (60.0 * (g - b) / d).rem_euclid(360.0)
    } else if max == g {
        60.0 * (b - r) / d + 120.0
    } else {
        60.0 * (r - g) / d + 240.0
    };
    [h, s, max]
}

/// `hsb_to_rgb` (themecpl's mixer maths) without its quirk at whole sectors: at hue 0/60/…/360 its
/// fractional part wraps to 0 and picks the neighbouring sector's end (hue 0 or 360 gives magenta,
/// 60 red instead of yellow), so moving Brightness on Ruby would turn it purple.
fn mixer_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    let h = if h <= 0.0 { 1e-6 } else if (h / 60.0).fract() == 0.0 { h - 1e-6 } else { h };
    hsb_to_rgb(h, s, v)
}

fn hex(rgb: [f64; 3]) -> String {
    let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(rgb[0]), b(rgb[1]), b(rgb[2]))
}

/// The page, for the Personalization window `win`.
pub fn page(win: &gtk::Window) -> gtk::Widget {
    thread_local! { static CSS_DONE: Cell<bool> = const { Cell::new(false) }; }
    if !CSS_DONE.with(|c| c.replace(true)) { crate::theme::load_css(CSS); }

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.style_context().add_class("wincolor");
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.set_margin_start(29);
    body.set_margin_end(29);
    body.set_margin_top(14);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.add(&body);
    outer.pack_start(&sw, true, true, 0);

    let heading = gtk::Label::new(Some("Change the color of your window borders, Start menu, and taskbar"));
    heading.style_context().add_class("wc-title");
    heading.set_xalign(0.0);
    heading.set_line_wrap(true);
    body.pack_start(&heading, false, false, 0);

    // 16 swatches, two rows of eight
    let grid = gtk::Grid::new();
    grid.set_margin_top(10);
    // the selection box of the first column reaches into the page margin
    grid.set_margin_start(-((CELL as f64 - TILE) / 2.0) as i32);
    let tiles: Vec<gtk::DrawingArea> = SWATCHES.iter().enumerate().map(|(i, (name, _))| {
        let da = gtk::DrawingArea::new();
        da.set_size_request(CELL, CELL);
        da.set_tooltip_text(Some(name));
        da.add_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK | gtk::gdk::EventMask::ENTER_NOTIFY_MASK
            | gtk::gdk::EventMask::LEAVE_NOTIFY_MASK | gtk::gdk::EventMask::KEY_PRESS_MASK);
        da.set_can_focus(true);
        grid.attach(&da, (i % 8) as i32, (i / 8) as i32, 1, 1);
        da
    }).collect();
    body.pack_start(&grid, false, false, 0);

    let cur_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    cur_row.set_margin_top(10);
    cur_row.pack_start(&gtk::Label::new(Some("Current color:")), false, false, 0);
    let current = gtk::Label::new(None);
    current.style_context().add_class("current-name");
    cur_row.pack_start(&current, false, false, 0);
    body.pack_start(&cur_row, false, false, 0);

    let transparency = gtk::CheckButton::with_mnemonic("_Enable transparency");
    transparency.set_halign(gtk::Align::Start);
    transparency.set_margin_top(12);
    body.pack_start(&transparency, false, false, 0);

    // the sliders line up in one column: [label | track]
    let sliders = gtk::Grid::new();
    sliders.set_column_spacing(0);
    sliders.set_row_spacing(10);
    sliders.set_margin_top(16);
    let slider_row = |row: i32, mnemonic: &str, s: &gtk::Scale| {
        let l = gtk::Label::with_mnemonic(mnemonic);
        l.set_mnemonic_widget(Some(s));
        l.set_xalign(0.0);
        l.set_size_request(118, -1);
        s.set_draw_value(false);
        s.set_size_request(300, -1);
        s.set_hexpand(false);
        sliders.attach(&l, 0, row, 1, 1);
        sliders.attach(s, 1, row, 1, 1);
    };
    let intensity = gtk::Scale::with_range(gtk::Orientation::Horizontal, 10.0, 85.0, 1.0);
    slider_row(0, "Color _intensity:", &intensity);
    body.pack_start(&sliders, false, false, 0);

    // "Show color mixer": the task-dialog expando button
    let exp_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    exp_row.set_margin_top(12);
    let expando = gtk::Button::new();
    expando.style_context().add_class("expando");
    let exp_img = gtk::Image::new();
    expando.add(&exp_img);
    let exp_label = gtk::Label::with_mnemonic("Show color _mixer");
    exp_label.set_mnemonic_widget(Some(&expando));
    let exp_ev = gtk::EventBox::new();
    exp_ev.add(&exp_label);
    exp_row.pack_start(&expando, false, false, 0);
    exp_row.pack_start(&exp_ev, false, false, 0);
    body.pack_start(&exp_row, false, false, 0);

    let mixer_grid = gtk::Grid::new();
    mixer_grid.set_row_spacing(10);
    mixer_grid.set_margin_top(12);
    let hue = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 360.0, 1.0);
    hue.set_widget_name("wincolor-hue");
    let sat = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    sat.set_widget_name("wincolor-sat");
    let bri = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    bri.set_widget_name("wincolor-bri");
    for (row, (m, s)) in [("_Hue:", &hue), ("_Saturation:", &sat), ("_Brightness:", &bri)].into_iter().enumerate() {
        let l = gtk::Label::with_mnemonic(m);
        l.set_mnemonic_widget(Some(s));
        l.set_xalign(0.0);
        l.set_size_request(118, -1);
        s.set_draw_value(false);
        s.set_size_request(300, -1);
        mixer_grid.attach(&l, 0, row as i32, 1, 1);
        mixer_grid.attach(s, 1, row as i32, 1, 1);
    }
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::None);
    revealer.add(&mixer_grid);
    body.pack_start(&revealer, false, false, 0);

    let advanced = gtk::Button::with_label("Advanced appearance settings...");
    advanced.style_context().add_class("content-link");
    advanced.set_relief(gtk::ReliefStyle::None);
    advanced.set_halign(gtk::Align::Start);
    advanced.set_margin_top(18);
    advanced.set_margin_bottom(14);
    body.pack_start(&advanced, false, false, 0);

    // footer: Save changes / Cancel
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.style_context().add_class("wincolor-footer");
    let fin = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    fin.set_margin_top(10);
    fin.set_margin_bottom(10);
    fin.set_margin_end(18);
    let save = gtk::Button::with_label("Save changes");
    let cancel = gtk::Button::with_label("Cancel");
    fin.pack_end(&cancel, false, false, 0);
    fin.pack_end(&save, false, false, 0);
    foot.pack_end(&fin, true, true, 0);
    outer.pack_end(&foot, false, false, 0);

    let mixer_css = gtk::CssProvider::new();
    if let Some(s) = gtk::gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(&s, &mixer_css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    }
    let start = glass::colorization();
    let p = Rc::new(Page {
        saved: Cell::new(glass::read_property()),
        cur: Cell::new(start),
        hsb: Cell::new(rgb_to_hsb(start.color)),
        syncing: Cell::new(false),
        hover: Cell::new(None),
        tiles,
        current,
        transparency,
        intensity,
        mixer: [hue, sat, bri],
        mixer_css,
    });
    p.sync();

    // ---- swatches
    for (i, da) in p.tiles.iter().enumerate() {
        let pw = Rc::downgrade(&p);
        da.connect_draw(move |da, cr| {
            if let Some(p) = pw.upgrade() {
                let sel = swatch_index(&p.cur.get()) == Some(i);
                draw_tile(cr, TILE_RGB[i], sel, p.hover.get() == Some(i));
                if da.has_visible_focus() {
                    gtk::render_focus(&da.style_context(), cr, 2.0, 2.0, CELL as f64 - 4.0, CELL as f64 - 4.0);
                }
            }
            glib::Propagation::Proceed
        });
        let pw = Rc::downgrade(&p);
        da.connect_button_press_event(move |da, e| {
            if e.button() == 1 { if let Some(p) = pw.upgrade() { da.grab_focus(); p.pick_swatch(i); } }
            glib::Propagation::Stop
        });
        let pw = Rc::downgrade(&p);
        da.connect_key_press_event(move |_, e| {
            use gtk::gdk::keys::constants as K;
            let k = e.keyval();
            if k == K::space || k == K::Return { if let Some(p) = pw.upgrade() { p.pick_swatch(i); } return glib::Propagation::Stop; }
            glib::Propagation::Proceed
        });
        for enter in [true, false] {
            let pw = Rc::downgrade(&p);
            let f = move |da: &gtk::DrawingArea| if let Some(p) = pw.upgrade() {
                p.hover.set(if enter { Some(i) } else { None });
                da.queue_draw();
            };
            if enter { da.connect_enter_notify_event(move |da, _| { f(da); glib::Propagation::Proceed }); }
            else { da.connect_leave_notify_event(move |da, _| { f(da); glib::Propagation::Proceed }); }
        }
    }

    // ---- transparency, intensity
    {
        let pw = Rc::downgrade(&p);
        p.transparency.connect_toggled(move |b| if let Some(p) = pw.upgrade() {
            if p.syncing.get() { return; }
            let c = p.cur.get();
            p.set(Colorization::from_cpl(c.color, !b.is_active()));
        });
        let pw = Rc::downgrade(&p);
        p.intensity.connect_value_changed(move |s| if let Some(p) = pw.upgrade() {
            if p.syncing.get() { return; }
            let c = p.cur.get();
            p.set(Colorization::from_cpl(intensity_to_alpha(s.value().round() as u32) << 24 | rgb_of(c.color), c.opaque));
        });
    }

    // ---- colour mixer (changes only the rgb)
    for (k, s) in p.mixer.iter().enumerate() {
        let pw = Rc::downgrade(&p);
        s.connect_value_changed(move |s| if let Some(p) = pw.upgrade() {
            if p.syncing.get() { return; }
            let mut hsb = p.hsb.get();
            hsb[k] = if k == 0 { s.value() } else { s.value() / 100.0 };
            p.hsb.set(hsb);
            let rgb = mixer_rgb(hsb[0], hsb[1], hsb[2]);
            let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
            let c = p.cur.get();
            p.set_keep_hsb(Colorization::from_cpl(c.color & 0xFF00_0000 | b(rgb[0]) << 16 | b(rgb[1]) << 8 | b(rgb[2]), c.opaque));
        });
    }
    {
        let set_exp = {
            let (rev, img, lbl, btn) = (revealer.clone(), exp_img.clone(), exp_label.clone(), expando.clone());
            Rc::new(move |open: bool, state: i32| {
                rev.set_reveal_child(open);
                lbl.set_text_with_mnemonic(if open { "Hide color _mixer" } else { "Show color _mixer" });
                btn.set_tooltip_text(Some(if open { "Hide color mixer" } else { "Show color mixer" }));
                set_expando_image(&img, open, state);
            })
        };
        set_exp(false, 0);
        let hot = Rc::new(Cell::new(false));
        let toggle = { let r = revealer.clone(); let f = set_exp.clone(); move || { let o = !r.reveals_child(); f(o, 1); } };
        let t1 = toggle.clone();
        expando.connect_clicked(move |_| t1());
        exp_ev.connect_button_press_event(move |_, e| { if e.button() == 1 { toggle(); } glib::Propagation::Stop });
        for w in [expando.upcast_ref::<gtk::Widget>(), exp_ev.upcast_ref()] {
            w.add_events(gtk::gdk::EventMask::ENTER_NOTIFY_MASK | gtk::gdk::EventMask::LEAVE_NOTIFY_MASK);
            let (f, r, h) = (set_exp.clone(), revealer.clone(), hot.clone());
            w.connect_enter_notify_event(move |_, _| { h.set(true); f(r.reveals_child(), 1); glib::Propagation::Proceed });
            let (f, r, h) = (set_exp.clone(), revealer.clone(), hot.clone());
            w.connect_leave_notify_event(move |_, _| { h.set(false); f(r.reveals_child(), 0); glib::Propagation::Proceed });
        }
        let (f, r) = (set_exp.clone(), revealer.clone());
        expando.connect_button_press_event(move |_, e| { if e.button() == 1 { f(r.reveals_child(), 2); } glib::Propagation::Proceed });
    }

    // ---- links and buttons
    {
        let w = win.clone();
        advanced.connect_clicked(move |_| advanced_dialog(&w));
        let pw = Rc::downgrade(&p);
        save.connect_clicked(move |_| if let Some(p) = pw.upgrade() { p.save() });
        let pw = Rc::downgrade(&p);
        cancel.connect_clicked(move |_| if let Some(p) = pw.upgrade() { p.cancel() });
        // closing Personalization without saving discards the preview, like Win7
        let pw = p.clone();
        win.connect_destroy(move |_| if pw.cur.get() != pw.committed() { glass::publish(pw.saved.get().as_ref()); });
    }
    outer.upcast()
}

impl Page {
    /// What Cancel goes back to.
    fn committed(&self) -> Colorization { self.saved.get().unwrap_or_else(|| config::get().colorization.dwm()) }

    fn pick_swatch(&self, i: usize) {
        self.hsb.set(rgb_to_hsb(SWATCHES[i].1));
        self.set_keep_hsb(Colorization::from_cpl(SWATCHES[i].1, self.cur.get().opaque));
    }

    /// A new colourisation from the swatches, the checkbox or the intensity slider.
    fn set(&self, c: Colorization) {
        if rgb_of(c.color) != rgb_of(self.cur.get().color) { self.hsb.set(rgb_to_hsb(c.color)); }
        self.set_keep_hsb(c);
    }

    fn set_keep_hsb(&self, c: Colorization) {
        self.cur.set(c);
        self.sync();
        self.preview();
    }

    /// Controls follow `cur` (without feeding back into the handlers).
    fn sync(&self) {
        let c = self.cur.get();
        self.syncing.set(true);
        self.current.set_text(swatch_index(&c).map(|i| SWATCHES[i].0).unwrap_or("Custom"));
        self.transparency.set_active(!c.opaque);
        self.intensity.set_value(percent(c.color));
        let hsb = self.hsb.get();
        self.mixer[0].set_value(hsb[0]);
        self.mixer[1].set_value(hsb[1] * 100.0);
        self.mixer[2].set_value(hsb[2] * 100.0);
        self.syncing.set(false);
        // saturation runs from grey to the hue, brightness from black to it
        let full = mixer_rgb(hsb[0], 1.0, hsb[2]);
        let grey = mixer_rgb(hsb[0], 0.0, hsb[2]);
        let bright = mixer_rgb(hsb[0], hsb[1], 1.0);
        let css = format!(
            "#wincolor-sat trough {{ background-image: linear-gradient(to right, {}, {}); }}\n\
             #wincolor-bri trough {{ background-image: linear-gradient(to right, #000000, {}); }}",
            hex(grey), hex(full), hex(bright));
        let _ = self.mixer_css.load_from_data(css.as_bytes());
        self.tiles.iter().for_each(|t| t.queue_draw());
    }

    /// Live preview: every glass painter follows the property (they coalesce repaints themselves).
    fn preview(&self) { glass::publish(Some(&self.cur.get())); }

    fn save(&self) {
        let c = self.cur.get();
        config::update(|cf| cf.colorization = c.into());
        glass::publish(Some(&c));
        self.saved.set(Some(c));
    }

    fn cancel(&self) {
        let back = self.committed();
        glass::publish(self.saved.get().as_ref());
        self.hsb.set(rgb_to_hsb(back.color));
        self.cur.set(back);
        self.sync();
    }
}

/// Task-dialog expando button (aero.msstyles TaskDialog part 13): state 0 normal, 1 hot, 2 pressed.
fn set_expando_image(img: &gtk::Image, open: bool, state: i32) {
    let id = 693 + 3 * (state.clamp(0, 2) + if open { 3 } else { 0 }) as u16;
    if let Some(pb) = glass::theme_pixbuf(id) {
        img.set_from_pixbuf(Some(&pb));
    } else {
        img.set_from_icon_name(Some(if open { "pan-up-symbolic" } else { "pan-down-symbolic" }), gtk::IconSize::Button);
    }
}

/// The body colour of each Win7 swatch tile (themecpl draws its own tile art, not the colourisation:
/// measured below the gloss on Win7 screenshots), in the order of `SWATCHES`.
const TILE_RGB: [u32; 16] = [
    0xDAECFD, 0x4A7EC9, 0x61D7D7, 0x6BC660, 0xB0E168, 0xFCED8A, 0xFEBF5E, 0xDA5252,
    0xFE82CC, 0xFCD5F9, 0xA384C3, 0xD9C9DC, 0xCFC6AB, 0x947677, 0xB1B1B1, 0xEEEEEE,
];

/// One swatch: the Win7 glossy tile with the Explorer-style selection/hover box around it.
fn draw_tile(cr: &cairo::Context, rgb: u32, selected: bool, hover: bool) {
    let cell = CELL as f64;
    if selected || hover {
        let (fill, border) = if selected { ([0.867, 0.925, 0.992], [0.6, 0.718, 0.855]) } else { ([0.933, 0.961, 0.992], [0.722, 0.839, 0.984]) };
        glass::rounded(cr, 0.5, 0.5, cell - 1.0, cell - 1.0, 2.0);
        cr.set_source_rgb(fill[0], fill[1], fill[2]);
        cr.fill_preserve().ok();
        cr.set_source_rgb(border[0], border[1], border[2]);
        cr.set_line_width(1.0);
        cr.stroke().ok();
    }
    let o = ((cell - TILE) / 2.0).floor();
    let ch = |s: u32| ((rgb >> s) & 0xFF) as f64 / 255.0;
    let (r, g, b) = (ch(16), ch(8), ch(0));
    // soft outer shadow, grey frame
    glass::rounded(cr, o - 0.5, o - 0.5, TILE + 1.0, TILE + 1.0, 5.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.10);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    glass::rounded(cr, o + 0.5, o + 0.5, TILE - 1.0, TILE - 1.0, 4.0);
    cr.set_source_rgb(0.68, 0.69, 0.70);
    cr.set_line_width(2.0);
    cr.stroke_preserve().ok();
    cr.save().ok();
    cr.clip();
    let (x0, y0) = (o + 1.5, o + 1.5);
    let s = TILE - 3.0;
    cr.rectangle(x0, y0, s, s);
    cr.set_source_rgb(r, g, b);
    cr.fill().ok();
    // white wash from the top left
    let lg = cairo::LinearGradient::new(x0, y0, x0 + s * 0.6, y0 + s * 0.6);
    lg.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.85);
    lg.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
    cr.set_source(&lg).ok();
    cr.rectangle(x0, y0, s, s);
    cr.fill().ok();
    // the gloss: lighter above a curve from the bottom left to the top right
    cr.move_to(x0, y0);
    cr.line_to(x0 + s, y0);
    cr.line_to(x0 + s, y0 + s * 0.18);
    cr.curve_to(x0 + s * 0.55, y0 + s * 0.22, x0 + s * 0.2, y0 + s * 0.55, x0, y0 + s * 0.95);
    cr.close_path();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.28);
    cr.fill().ok();
    // a faint light band at the bottom
    let bg = cairo::LinearGradient::new(0.0, y0 + s - 8.0, 0.0, y0 + s);
    bg.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.0);
    bg.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.25);
    cr.set_source(&bg).ok();
    cr.rectangle(x0, y0 + s - 8.0, s, 8.0);
    cr.fill().ok();
    cr.restore().ok();
    // inner white edge
    glass::rounded(cr, o + 2.0, o + 2.0, TILE - 4.0, TILE - 4.0, 3.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
    cr.set_line_width(1.0);
    cr.stroke().ok();
}

/// "Advanced appearance settings...": Win7 opens the classic Window Color and Appearance dialog,
/// whose settings only apply to the Basic, Classic and high-contrast themes. A stub here.
fn advanced_dialog(parent: &gtk::Window) {
    thread_local! { static OPEN: RefCell<Option<gtk::Dialog>> = const { RefCell::new(None) }; }
    if let Some(d) = OPEN.with(|o| o.borrow().clone()) { d.present(); return; }
    let d = gtk::Dialog::new();
    d.set_title("Window Color and Appearance");
    d.set_transient_for(Some(parent));
    d.set_modal(true);
    d.set_resizable(false);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(10);
    let note = gtk::Label::new(Some(
        "If you select a theme other than Windows 7 Basic or Windows Classic, the colors, fonts, \
         and sizes selected here will not be used. Aerode draws every Aero element from the window \
         color, so there is nothing to change here."));
    note.set_line_wrap(true);
    note.set_max_width_chars(52);
    note.set_xalign(0.0);
    c.pack_start(&note, false, false, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.pack_start(&gtk::Label::with_mnemonic("_Item:"), false, false, 0);
    let item = gtk::ComboBoxText::new();
    for i in ["Desktop", "Active Title Bar", "Active Window Border", "Application Background", "Border Padding", "Caption Buttons",
        "Disabled Item", "Hyperlink", "Icon", "Inactive Title Bar", "Inactive Window Border", "Menu", "Message Box", "Palette Title",
        "Scrollbar", "Selected Items", "ToolTip", "Window"] { item.append_text(i); }
    item.set_active(Some(0));
    item.set_sensitive(false);
    row.pack_start(&item, true, true, 0);
    c.pack_start(&row, false, false, 0);
    d.add_button("OK", gtk::ResponseType::Ok);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.connect_response(|d, _| d.close());
    d.connect_destroy(|_| OPEN.with(|o| *o.borrow_mut() = None));
    d.show_all();
    OPEN.with(|o| *o.borrow_mut() = Some(d));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsb_round_trip() {
        for (_, argb) in SWATCHES {
            let [h, s, v] = rgb_to_hsb(argb);
            let rgb = mixer_rgb(h, s, v);
            let b = |x: f64| (x * 255.0).round() as u32;
            assert_eq!(b(rgb[0]) << 16 | b(rgb[1]) << 8 | b(rgb[2]), argb & 0xFFFFFF, "{argb:08X} -> {h} {s} {v}");
        }
    }

    #[test]
    fn intensity_percent_round_trip() {
        for (_, argb) in SWATCHES {
            assert_eq!(intensity_to_alpha(percent(argb) as u32), argb >> 24);
        }
    }
}
