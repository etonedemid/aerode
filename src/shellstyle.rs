//! Shell chrome painted from the user's own aero.msstyles (taskbar, Start menu).
//!
//! Windows 7 stores the IMAGE resources of aero.msstyles (and the 32-bit bitmaps of explorer.exe)
//! with *premultiplied* alpha: no pixel has a colour channel above its alpha. They go into cairo
//! ARGB32 surfaces (premultiplied too) unchanged; treating them as straight alpha would darken
//! every translucent edge and highlight.
//!
//! Parts are drawn like uxtheme's ST_STRETCH: the SIZINGMARGINS (left, right, top, bottom) stay
//! unscaled and the middle stretches.
use gtk::cairo;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::msstyles::{self, Value};

type Key = (String, i32, i32, u32);

thread_local! {
    static FRAMES: RefCell<HashMap<Key, Option<cairo::ImageSurface>>> = RefCell::new(HashMap::new());
}

/// Whether an aero.msstyles was extracted (otherwise callers keep their CSS look).
pub fn available() -> bool { crate::glass::theme().is_some() }

/// A premultiplied RGBA image as a cairo surface (channels clamped to alpha).
pub fn premul_surface(img: &image::RgbaImage) -> Option<cairo::ImageSurface> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let mut s = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    let stride = s.stride() as usize;
    {
        let mut d = s.data().ok()?;
        for (y, row) in img.rows().enumerate() {
            for (x, p) in row.enumerate() {
                let [r, g, b, a] = p.0;
                let o = y * stride + x * 4;
                d[o..o + 4].copy_from_slice(&[b.min(a), g.min(a), r.min(a), a]);
            }
        }
    }
    Some(s)
}

/// A premultiplied PNG from the extracted data dir (explorer.exe bitmaps such as the user tile frame).
pub fn data_surface(name: &str) -> Option<cairo::ImageSurface> {
    FRAMES.with(|f| {
        f.borrow_mut().entry((format!("file:{name}"), 0, 0, 0)).or_insert_with(|| {
            let p = crate::config::data_file(name)?;
            premul_surface(&image::open(p).ok()?.to_rgba8())
        }).clone()
    })
}

/// State `state` of the image in property `prop` (IMAGEFILE, IMAGEFILE1, …) of (class, part):
/// the strip is cut into IMAGECOUNT frames along IMAGELAYOUT.
pub fn frame_of(class: &str, part: i32, state: i32, prop: u32) -> Option<cairo::ImageSurface> {
    FRAMES.with(|f| {
        f.borrow_mut().entry((class.to_string(), part, state, prop)).or_insert_with(|| {
            let t = crate::glass::theme()?;
            let id = match t.get(class, part, state, prop)? { Value::Image(i) => *i, _ => return None };
            let img = t.image_res(id)?;
            let count = t.int(class, part, state, msstyles::IMAGECOUNT).unwrap_or(1).max(1) as u32;
            let horiz = t.int(class, part, state, msstyles::IMAGELAYOUT).unwrap_or(0) == 1;
            let (w, h) = (img.width(), img.height());
            let (fw, fh) = if horiz { (w / count, h) } else { (w, h / count) };
            let idx = ((state - 1).max(0) as u32).min(count - 1);
            let (x, y) = if horiz { (fw * idx, 0) } else { (0, fh * idx) };
            premul_surface(&image::imageops::crop_imm(&img, x, y, fw, fh).to_image())
        }).clone()
    })
}

/// The background image (IMAGEFILE, else IMAGEFILE1) of (class, part, state).
pub fn frame(class: &str, part: i32, state: i32) -> Option<cairo::ImageSurface> {
    frame_of(class, part, state, msstyles::IMAGEFILE).or_else(|| frame_of(class, part, state, msstyles::IMAGEFILE1))
}

/// SIZINGMARGINS (left, right, top, bottom) of (class, part, state).
pub fn margins(class: &str, part: i32, state: i32) -> [i32; 4] {
    crate::glass::theme().and_then(|t| t.margins(class, part, state, msstyles::SIZINGMARGINS)).unwrap_or([0; 4])
}

/// CONTENTMARGINS (left, right, top, bottom) of (class, part, state).
pub fn content_margins(class: &str, part: i32, state: i32) -> [i32; 4] {
    crate::glass::theme().and_then(|t| t.margins(class, part, state, msstyles::CONTENTMARGINS)).unwrap_or([0; 4])
}

/// A colour property of (class, part, state) as cairo RGB.
pub fn color(class: &str, part: i32, state: i32, prop: u32) -> Option<[f64; 3]> {
    crate::glass::theme().and_then(|t| t.color(class, part, state, prop)).map(|c| c.map(|v| v as f64 / 255.0))
}

/// Draw `src` into (x, y, w, h), keeping the sizing margins `m` (l, r, t, b) unscaled.
pub fn nine(cr: &cairo::Context, src: &cairo::ImageSurface, m: [i32; 4], x: f64, y: f64, w: f64, h: f64) {
    if w <= 0.0 || h <= 0.0 { return; }
    let (sw, sh) = (src.width() as f64, src.height() as f64);
    let [ml, mr, mt, mb] = m.map(|v| v.max(0) as f64);
    // too small for the margins: shrink them proportionally (uxtheme does the same)
    let (ml, mr) = if ml + mr > w { (w * ml / (ml + mr), w * mr / (ml + mr)) } else { (ml, mr) };
    let (mt, mb) = if mt + mb > h { (h * mt / (mt + mb), h * mb / (mt + mb)) } else { (mt, mb) };
    let [sl, sr, st, sb] = m.map(|v| v.max(0) as f64);
    let cols = [(0.0, sl, x, ml), (sl, sw - sl - sr, x + ml, w - ml - mr), (sw - sr, sr, x + w - mr, mr)];
    let rows = [(0.0, st, y, mt), (st, sh - st - sb, y + mt, h - mt - mb), (sh - sb, sb, y + h - mb, mb)];
    for (sx, cw, dx, dw) in cols {
        for (sy, ch, dy, dh) in rows {
            if cw <= 0.0 || ch <= 0.0 || dw <= 0.0 || dh <= 0.0 { continue; }
            cr.save().ok();
            cr.rectangle(dx, dy, dw, dh);
            cr.clip();
            if (cw - dw).abs() < 0.01 && (ch - dh).abs() < 0.01 {
                cr.set_source_surface(src, dx - sx, dy - sy).ok();
                cr.paint().ok();
            } else if let Ok(cell) = cairo::ImageSurface::create(cairo::Format::ARgb32, cw as i32, ch as i32) {
                // copy the cell so stretching can't bleed the neighbouring cells in
                if let Ok(c) = cairo::Context::new(&cell) {
                    c.set_operator(cairo::Operator::Source);
                    c.set_source_surface(src, -sx, -sy).ok();
                    c.paint().ok();
                }
                let pat = cairo::SurfacePattern::create(&cell);
                pat.set_extend(cairo::Extend::Pad);
                pat.set_filter(cairo::Filter::Bilinear);
                cr.translate(dx, dy);
                cr.scale(dw / cw, dh / ch);
                cr.set_source(&pat).ok();
                cr.paint().ok();
            }
            cr.restore().ok();
        }
    }
}

/// Draw (class, part, state) into the rectangle the way DrawThemeBackground does (SIZINGTYPE:
/// true size at the top-left, otherwise stretched with the sizing margins). False if the theme
/// has no such image.
pub fn paint(cr: &cairo::Context, class: &str, part: i32, state: i32, x: f64, y: f64, w: f64, h: f64) -> bool {
    let Some(src) = frame(class, part, state) else { return false };
    let truesize = crate::glass::theme().and_then(|t| t.int(class, part, state, msstyles::SIZINGTYPE)) == Some(0);
    if truesize {
        cr.set_source_surface(&src, x, y).ok();
        cr.paint().ok();
    } else {
        nine(cr, &src, margins(class, part, state), x, y, w, h);
    }
    true
}

/// Natural (frame) size of (class, part, state).
pub fn size(class: &str, part: i32, state: i32) -> Option<(i32, i32)> {
    frame(class, part, state).map(|s| (s.width(), s.height()))
}

/// Win7's travel buttons (Explorer, Control Panel, Aero wizards): paint `b` from aero.msstyles'
/// Navigation class – part 1 Back, 2 Forward, 3 the recent-pages arrow; states 1 normal, 2 hot,
/// 3 pressed, 4 disabled – the glossy, partly see-through orbs drawn over the glass. Without an
/// extracted theme the button keeps whatever it had.
pub fn nav_button(b: &gtk::Button, part: i32) {
    use gtk::prelude::*;
    let Some((w, h)) = size("Navigation", part, 1) else { return };
    // a from_icon_name button keeps its icon as the "image" property: clear that, not just the child
    b.set_image(None::<&gtk::Widget>);
    if let Some(c) = b.child() { b.remove(&c); }
    let css = gtk::CssProvider::new();
    let _ = css.load_from_data(b"button, button:hover, button:active, button:disabled { min-width: 0; min-height: 0; padding: 0; margin: 0; border: none; background: none; box-shadow: none; }");
    b.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 10);
    b.set_size_request(w, h);
    b.connect_draw(move |b, cr| {
        let f = b.state_flags();
        let state = if !b.is_sensitive() || f.contains(gtk::StateFlags::INSENSITIVE) { 4 }
            else if f.contains(gtk::StateFlags::ACTIVE) { 3 }
            else if f.contains(gtk::StateFlags::PRELIGHT) { 2 } else { 1 };
        let (aw, ah) = (b.allocated_width() as f64, b.allocated_height() as f64);
        let (x, y) = (((aw - w as f64) / 2.0).floor(), ((ah - h as f64) / 2.0).floor());
        if paint(cr, "Navigation", part, state, x, y, w as f64, h as f64) { gtk::glib::Propagation::Stop } else { gtk::glib::Propagation::Proceed }
    });
}
