//! Window frame rendering from the DWM atlas inside aero.msstyles
//! (class "DWMWindow", ATLASRECT + SIZINGMARGINS + IMAGECOUNT per part).
//! Falls back to plain cairo drawing when no theme was extracted.
use cairo::{Context, Format, ImageSurface};
use std::collections::HashMap;

use crate::msstyles::{self, Theme};

/// Frame metrics at 96 dpi (Windows 7 defaults).
pub const BORDER: i32 = 8;
pub const CAPTION: i32 = 22;
pub const TOP: i32 = BORDER + CAPTION;
pub const MAX_TOP: i32 = 22; // caption height when maximised (borders off-screen)
pub const BTN_H: i32 = 20;
pub const MIN_W: i32 = 26;
pub const MAX_W: i32 = 27;
pub const CLOSE_W: i32 = 47;
pub const CLOSE_ALONE_W: i32 = 43;
pub const RADIUS: f64 = 6.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Btn { Min, Max, Close, Help }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BState { Normal = 1, Hot = 2, Pressed = 3, Disabled = 4 }

pub struct Params<'a> {
    pub w: i32,
    pub h: i32,
    pub title: &'a str,
    pub icon: Option<&'a ImageSurface>,
    pub active: bool,
    pub maximized: bool,
    pub buttons: &'a [(Btn, BState)],
    pub composited: bool,
    /// colourisation (r,g,b,alpha 0..1)
    pub color: [f64; 4],
    /// for the non-composited fallback: painter of what lies behind (screen coords)
    pub backdrop: Option<&'a dyn Fn(&Context, f64, f64)>,
    pub screen_pos: (i32, i32),
}

struct Part {
    surf: ImageSurface,
    rect: [i32; 4], // l t r b
    margins: [i32; 4], // l r t b
    count: i32,
}

pub struct Decor {
    parts: HashMap<i32, Part>,
}

fn rgba_to_surface(img: &image::RgbaImage) -> ImageSurface {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let mut s = ImageSurface::create(Format::ARgb32, w, h).unwrap();
    let stride = s.stride() as usize;
    {
        let mut d = s.data().unwrap();
        for (y, row) in img.rows().enumerate() {
            for (x, p) in row.enumerate() {
                let [r, g, b, a] = p.0;
                let pm = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
                let o = y * stride + x * 4;
                d[o..o + 4].copy_from_slice(&[pm(b), pm(g), pm(r), a]);
            }
        }
    }
    s
}

pub fn rgba_surface(img: &image::RgbaImage) -> ImageSurface { rgba_to_surface(img) }

impl Decor {
    pub fn load() -> Decor {
        let mut parts = HashMap::new();
        let path = crate::config::data_dir().join("theme/aero.msstyles");
        if let Ok(t) = Theme::open(&path) {
            if let Some(atlas) = t.stream(971).or_else(|| {
                // the atlas id differs between builds: take the biggest STREAM
                None
            }) {
                let atlas_s = rgba_to_surface(&atlas);
                for p in 1..80 {
                    if let Some(r) = t.rect("DWMWindow", p, 0, msstyles::ATLASRECT) {
                        let (w, h) = (r[2] - r[0], r[3] - r[1]);
                        if w <= 0 || h <= 0 { continue; }
                        let s = ImageSurface::create(Format::ARgb32, w, h).unwrap();
                        {
                            let cr = Context::new(&s).unwrap();
                            cr.set_source_surface(&atlas_s, -r[0] as f64, -r[1] as f64).ok();
                            cr.paint().ok();
                        }
                        parts.insert(p, Part {
                            surf: s,
                            rect: r,
                            margins: t.margins("DWMWindow", p, 0, msstyles::SIZINGMARGINS).unwrap_or([0; 4]),
                            count: t.int("DWMWindow", p, 0, msstyles::IMAGECOUNT).unwrap_or(1).max(1),
                        });
                    }
                }
            }
        }
        Decor { parts }
    }

    pub fn themed(&self) -> bool { self.parts.contains_key(&7) }

    /// Draw frame `idx` of part `p`, nine-sliced into (x, y, w, h).
    fn nine(&self, cr: &Context, p: i32, idx: i32, x: f64, y: f64, w: f64, h: f64) -> bool {
        let Some(part) = self.parts.get(&p) else { return false };
        let pw = part.rect[2] - part.rect[0];
        let fh = (part.rect[3] - part.rect[1]) / part.count;
        let sy = fh * idx.clamp(0, part.count - 1);
        let [ml, mr, mt, mb] = part.margins;
        let (ml, mr, mt, mb) = (ml as f64, mr as f64, mt as f64, mb as f64);
        let (sw, sh) = (pw as f64, fh as f64);
        let xs = [(0.0, ml, x, ml), (ml, sw - ml - mr, x + ml, w - ml - mr), (sw - mr, mr, x + w - mr, mr)];
        let ys = [(0.0, mt, y, mt), (mt, sh - mt - mb, y + mt, h - mt - mb), (sh - mb, mb, y + h - mb, mb)];
        for (sx0, sww, dx, dw) in xs {
            for (sy0, shh, dy, dh) in ys {
                if sww <= 0.0 || shh <= 0.0 || dw <= 0.0 || dh <= 0.0 { continue; }
                cr.save().ok();
                cr.rectangle(dx, dy, dw, dh);
                cr.clip();
                cr.translate(dx, dy);
                cr.scale(dw / sww, dh / shh);
                cr.set_source_surface(&part.surf, -sx0, -(sy as f64) - sy0).ok();
                cr.source().set_filter(cairo::Filter::Bilinear);
                cr.paint().ok();
                cr.restore().ok();
            }
        }
        true
    }

    /// Draw frame `idx` of part `p` at natural size centred in the box.
    fn glyph(&self, cr: &Context, p: i32, idx: i32, cx: f64, cy: f64) {
        let Some(part) = self.parts.get(&p) else { return };
        let pw = (part.rect[2] - part.rect[0]) as f64;
        let fh = ((part.rect[3] - part.rect[1]) / part.count) as f64;
        let (x, y) = ((cx - pw / 2.0).round(), (cy - fh / 2.0).round());
        cr.save().ok();
        cr.rectangle(x, y, pw, fh);
        cr.clip();
        cr.set_source_surface(&part.surf, x, y - fh * idx.clamp(0, part.count - 1) as f64).ok();
        cr.paint().ok();
        cr.restore().ok();
    }

    /// Button rectangles (x, y, w, h) in frame coordinates, right-to-left.
    pub fn button_rects(&self, w: i32, maximized: bool, buttons: &[(Btn, BState)]) -> Vec<(Btn, [i32; 4])> {
        let mut x = w - if maximized { 2 } else { BORDER - 2 };
        let y = if maximized { 0 } else { 1 };
        let h = if maximized { BTN_H - 2 } else { BTN_H };
        let only_close = buttons.len() == 1;
        let mut out = vec![];
        for (b, _) in buttons.iter().rev() {
            let bw = match b { Btn::Close => if only_close { CLOSE_ALONE_W } else { CLOSE_W }, Btn::Max => MAX_W, Btn::Min | Btn::Help => MIN_W };
            x -= bw;
            out.push((*b, [x, y, bw, h]));
        }
        out
    }

    pub fn render(&self, cr: &Context, p: &Params) {
        let (w, h) = (p.w as f64, p.h as f64);
        let top = if p.maximized { MAX_TOP } else { TOP } as f64;
        let b = if p.maximized { 0.0 } else { BORDER as f64 };
        let r = if p.maximized { 0.0 } else { RADIUS };
        cr.save().ok();
        cr.rectangle(0.0, 0.0, w, h);
        cr.clip();
        if p.composited {
            cr.set_operator(cairo::Operator::Source);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
            cr.paint().ok();
            cr.set_operator(cairo::Operator::Over);
        }
        // ---- glass body
        crate::glass::rounded_top(cr, 0.0, 0.0, w, h, r, 2.0);
        cr.save().ok();
        cr.clip_preserve();
        cr.new_path();
        let [cr_, cg, cb, ca] = p.color;
        if !p.composited {
            if let Some(bd) = p.backdrop { bd(cr, p.screen_pos.0 as f64, p.screen_pos.1 as f64); }
            let a = if p.active { 0.55 } else { 0.35 };
            cr.set_source_rgba(cr_, cg, cb, a);
            cr.paint().ok();
            cr.set_source_rgba(1.0, 1.0, 1.0, if p.active { 0.18 } else { 0.38 });
            cr.paint().ok();
        } else {
            let a = if p.active { ca } else { ca * 0.55 };
            cr.set_source_rgba(cr_ * 0.9, cg * 0.9, cb * 0.9, a);
            cr.paint().ok();
            if !p.active {
                cr.set_source_rgba(0.85, 0.88, 0.92, 0.35);
                cr.paint().ok();
            }
        }
        // reflection streak (part 34 when themed)
        if let Some(refl) = self.parts.get(&34) {
            cr.set_source_surface(&refl.surf, 0.0, 0.0).ok();
            cr.paint_with_alpha(if p.composited { if p.active { 0.32 } else { 0.2 } } else if p.active { 0.9 } else { 0.6 }).ok();
        } else {
            let g = cairo::LinearGradient::new(0.0, 0.0, w * 0.4, h * 0.4);
            g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.35);
            g.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
            cr.set_source(&g).ok();
            cr.paint().ok();
        }
        cr.restore().ok();
        // ---- outer dark border + inner highlight
        cr.set_line_width(1.0);
        crate::glass::rounded_top(cr, 0.5, 0.5, w - 1.0, h - 1.0, r, 2.0);
        cr.set_source_rgba(0.0, 0.0, 0.0, if p.active { 0.75 } else { 0.45 });
        cr.stroke().ok();
        crate::glass::rounded_top(cr, 1.5, 1.5, w - 3.0, h - 3.0, (r - 1.0).max(0.0), 1.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
        cr.stroke().ok();
        // ---- client edge: dark line then light line around the client area
        if !p.maximized {
            cr.rectangle(b - 1.5, top - 1.5, w - 2.0 * b + 3.0, h - top - b + 3.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
            cr.stroke().ok();
            cr.rectangle(b - 0.5, top - 0.5, w - 2.0 * b + 1.0, h - top - b + 1.0);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
            cr.stroke().ok();
        }
        // ---- icon + title with the Aero white glow
        let mut tx = b + 2.0;
        if let Some(icon) = p.icon {
            let iy = ((top - b) - 16.0) / 2.0 + if p.maximized { 0.0 } else { b - 1.0 };
            cr.save().ok();
            cr.translate(tx, iy.round());
            let s = 16.0 / icon.width().max(1) as f64;
            cr.scale(s, s);
            cr.set_source_surface(icon, 0.0, 0.0).ok();
            cr.source().set_filter(cairo::Filter::Good);
            cr.paint().ok();
            cr.restore().ok();
            tx += 22.0;
        }
        let rects = self.button_rects(p.w, p.maximized, p.buttons);
        let tmax = rects.last().map(|r| r.1[0] as f64).unwrap_or(w) - tx - 8.0;
        if tmax > 10.0 {
            let layout = pangocairo::functions::create_layout(cr);
            let mut fd = pangocairo::pango::FontDescription::from_string("Segoe UI 9");
            fd.set_absolute_size(12.0 * pangocairo::pango::SCALE as f64);
            layout.set_font_description(Some(&fd));
            layout.set_text(p.title);
            layout.set_width((tmax * pangocairo::pango::SCALE as f64) as i32);
            layout.set_ellipsize(pangocairo::pango::EllipsizeMode::End);
            let (_, lh) = layout.pixel_size();
            let (lw, _) = layout.pixel_size();
            let ty = if p.maximized { ((top - lh as f64) / 2.0).round() } else { (b + (CAPTION as f64 - lh as f64) / 2.0).round() };
            // Aero caption glow: the text shape, blurred, painted white behind the text
            let _ = lw;
            // a soft haze rather than a solid halo: one pass, wide falloff, partial opacity
            let ga = if p.active { 0.72 } else { 0.45 };
            const GR: usize = 8;
            if let Some(mask) = text_glow(&layout, GR) {
                cr.save().ok();
                cr.set_source_rgba(1.0, 1.0, 1.0, ga);
                cr.mask_surface(&mask, tx - (2 * GR) as f64, ty - (2 * GR) as f64).ok();
                cr.restore().ok();
            }
            cr.move_to(tx, ty);
            cr.set_source_rgba(0.0, 0.0, 0.0, if p.active { 1.0 } else { 0.55 });
            pangocairo::functions::show_layout(cr, &layout);
        }
        // ---- caption buttons
        let n = p.buttons.len();
        for (i, (btn, rect)) in rects.iter().enumerate() {
            let state = p.buttons.iter().find(|(b, _)| b == btn).map(|x| x.1).unwrap_or(BState::Normal);
            let [x, y, bw, bh] = rect.map(|v| v as f64);
            let idx = state as i32 - 1;
            let alone = n == 1;
            let leftmost = i == rects.len() - 1;
            let (bg, glyph_part) = match btn {
                Btn::Close => (if alone { if p.active { 9 } else { 10 } } else if p.active { 7 } else { 8 }, 12),
                Btn::Max => (if p.active { 5 } else { 6 }, if p.maximized { 29 } else { 21 }),
                Btn::Min => (if leftmost { if p.active { 3 } else { 4 } } else if p.active { 5 } else { 6 }, 25),
                Btn::Help => (if p.active { 3 } else { 4 }, 17),
            };
            if self.themed() {
                if state == BState::Hot {
                    let glow = if *btn == Btn::Close { 11 } else { 16 };
                    self.nine(cr, glow, 0, x - 8.0, y - 7.0, bw + 16.0, bh + 15.0);
                }
                self.nine(cr, bg, idx, x, y, bw, bh);
                let gidx = if p.active { idx } else { 3.min(idx.max(0)) };
                self.glyph(cr, glyph_part, if !p.active && state == BState::Normal { 3 } else { gidx }, x + bw / 2.0, y + bh / 2.0);
            } else {
                fallback_button(cr, *btn, state, p.active, p.maximized, x, y, bw, bh);
            }
        }
        cr.restore().ok();
    }
}

fn fallback_button(cr: &Context, b: Btn, s: BState, active: bool, maxed: bool, x: f64, y: f64, w: f64, h: f64) {
    let (top, bot) = match (b, s) {
        (Btn::Close, BState::Hot) => ((0.98, 0.62, 0.52), (0.85, 0.20, 0.10)),
        (Btn::Close, BState::Pressed) => ((0.75, 0.35, 0.25), (0.55, 0.10, 0.03)),
        (Btn::Close, _) if active => ((0.90, 0.60, 0.52), (0.76, 0.27, 0.17)),
        (_, BState::Hot) => ((0.85, 0.94, 1.0), (0.25, 0.60, 0.90)),
        (_, BState::Pressed) => ((0.55, 0.70, 0.85), (0.10, 0.35, 0.60)),
        _ => ((0.92, 0.95, 0.98), (0.70, 0.78, 0.88)),
    };
    crate::glass::rounded(cr, x + 0.5, y + 0.5, w - 1.0, h - 1.0, 3.0);
    let g = cairo::LinearGradient::new(0.0, y, 0.0, y + h);
    g.add_color_stop_rgba(0.0, top.0, top.1, top.2, if active { 1.0 } else { 0.5 });
    g.add_color_stop_rgba(0.5, (top.0 + bot.0) / 2.0, (top.1 + bot.1) / 2.0, (top.2 + bot.2) / 2.0, if active { 1.0 } else { 0.5 });
    g.add_color_stop_rgba(1.0, bot.0, bot.1, bot.2, if active { 1.0 } else { 0.5 });
    cr.set_source(&g).ok();
    cr.fill_preserve().ok();
    cr.set_source_rgba(0.1, 0.15, 0.25, 0.8);
    cr.set_line_width(1.0);
    cr.stroke().ok();
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.set_line_width(2.0);
    match b {
        Btn::Close => { cr.move_to(cx - 4.0, cy - 4.0); cr.line_to(cx + 4.0, cy + 4.0); cr.move_to(cx + 4.0, cy - 4.0); cr.line_to(cx - 4.0, cy + 4.0); }
        Btn::Min => { cr.rectangle(cx - 4.0, cy + 2.0, 8.0, 2.0); }
        Btn::Max => { if maxed { cr.rectangle(cx - 4.0, cy - 2.0, 6.0, 6.0); cr.rectangle(cx - 2.0, cy - 4.0, 6.0, 6.0); } else { cr.rectangle(cx - 5.0, cy - 4.0, 10.0, 8.0); } }
        Btn::Help => { cr.arc(cx, cy - 1.0, 3.0, 3.6, 7.0); }
    }
    cr.stroke().ok();
}

/// Render a layout into an A8 surface (padded by 2r) and blur it – the DWM text glow.
fn text_glow(layout: &pangocairo::pango::Layout, r: usize) -> Option<ImageSurface> {
    let (lw, lh) = layout.pixel_size();
    let pad = 2 * r as i32;
    let (w, h) = (lw + 2 * pad, lh + 2 * pad);
    let mut s = ImageSurface::create(Format::A8, w, h).ok()?;
    {
        let cr = Context::new(&s).ok()?;
        cr.move_to(pad as f64, pad as f64);
        pangocairo::functions::layout_path(&cr, layout);
        cr.set_line_width(1.5);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.stroke_preserve().ok();
        cr.fill().ok();
    }
    s.flush();
    let stride = s.stride() as usize;
    {
        let mut d = s.data().ok()?;
        let (w, h) = (w as usize, h as usize);
        let mut buf: Vec<f32> = (0..h).flat_map(|y| (0..w).map(move |x| (y, x))).map(|(y, x)| d[y * stride + x] as f32).collect();
        let mut tmp = buf.clone();
        for _ in 0..3 {
            for (src, dst, horiz) in [(&buf, &mut tmp, true)] { blur_pass(src, dst, w, h, r / 2 + 1, horiz); }
            let t2 = tmp.clone();
            blur_pass(&t2, &mut buf, w, h, r / 2 + 1, false);
        }
        for y in 0..h { for x in 0..w { d[y * stride + x] = (buf[y * w + x] * 1.25).min(255.0) as u8; } }
    }
    s.mark_dirty();
    Some(s)
}

fn blur_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horiz: bool) {
    let (len, lines) = if horiz { (w, h) } else { (h, w) };
    let idx = |l: usize, i: usize| if horiz { l * w + i } else { i * w + l };
    let norm = 1.0 / (2 * r + 1) as f32;
    for l in 0..lines {
        let mut acc = 0.0;
        for k in 0..=2 * r { let i = (k as isize - r as isize).clamp(0, len as isize - 1) as usize; acc += src[idx(l, i)]; }
        for i in 0..len {
            dst[idx(l, i)] = acc * norm;
            let add = (i + r + 1).min(len - 1);
            let sub = (i as isize - r as isize).max(0) as usize;
            acc += src[idx(l, add)] - src[idx(l, sub)];
        }
    }
}
