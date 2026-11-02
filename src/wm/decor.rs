//! Window frame rendering from the DWM atlas inside aero.msstyles
//! (class "DWMWindow", ATLASRECT + SIZINGMARGINS + IMAGECOUNT per part; part ids are Win7's,
//! see msstyleEditor's PARTS_DWMWINDOW_WIN7).
//!
//! Layer order like Win7's DWM: glass (`wm::glass`) → reflection streaks (REFLECTIONMAP, screen
//! anchored with parallax) → LEFTHIGHLIGHTCORNER / SIDEHIGHLIGHT → the frame bitmaps (TOPFRAME,
//! LEFTFRAME, RIGHTFRAME, BOTTOMFRAME, which carry the outer rim, the inner highlight and the
//! client edge) → caption buttons → TEXTGLOW → caption text.
//! Falls back to plain cairo drawing when no theme was extracted.
use cairo::{Context, Format, ImageSurface};
use std::collections::HashMap;

use crate::dwmcolor::ThemeGlass;
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
/// SMALLBUTTONACTIVECLOSE of tool windows (natural size of the atlas part)
pub const SMALL_CLOSE: i32 = 15;
/// height of the SIDEHIGHLIGHT sheen below the caption (measured on Vista/7 captures at 96 dpi)
const SIDE_HL_H: f64 = 27.0;

// DWMWindow parts (Windows 7)
const BOTTOMFRAME: i32 = 1;
const BUTTONCLOSEGLOW: i32 = 11;
const BUTTONGLOW: i32 = 16;
const LEFTFRAME: i32 = 33;
const LEFTHIGHLIGHTCORNER: i32 = 34;
const REFLECTIONMAP: i32 = 40;
const RIGHTFRAME: i32 = 41;
const SIDEHIGHLIGHT: i32 = 43;
const SMALLBOTTOMFRAME: i32 = 44;
const SMALLBUTTONACTIVECLOSE: i32 = 45;
const SMALLBUTTONINACTIVECLOSE: i32 = 46;
const SMALLBUTTONCLOSEGLOW: i32 = 47;
const SMALLBUTTONCLOSEGLYPH96: i32 = 48;
const SMALLLEFTFRAME: i32 = 52;
const SMALLRIGHTFRAME: i32 = 53;
const SMALLTOPFRAME: i32 = 54;
const TEXTGLOW: i32 = 55;
const TOPFRAME: i32 = 56;
const TOPFRAMENOSHADOW: i32 = 58;
const BOTTOMFRAMENOSHADOW: i32 = 59;

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
    /// tool window: the SMALL* frame (square corners) and the small close button
    pub small: bool,
    pub buttons: &'a [(Btn, BState)],
    /// hover glow of buttons in the middle of a fade (0 normal … 1 hot); the others show their state
    pub glow: &'a [(Btn, f64)],
    pub composited: bool,
    /// fills the current clip with this frame's glass (`wm::glass::Glass::fill`) and returns the
    /// opacity of the reflection streaks
    pub glass: &'a dyn Fn(&Context) -> f64,
    /// frame position on screen (glass and reflection are screen-anchored)
    pub screen_pos: (i32, i32),
    /// the virtual screen the reflection map is stretched over
    pub screen_size: (i32, i32),
}

struct Part {
    surf: ImageSurface,
    w: i32,
    fh: i32, // height of one frame (image count stacked vertically)
    margins: [i32; 4], // l r t b
    count: i32,
}

/// One cell row/column of a nine-grid: source offset/length → destination offset/length.
type Seg = (f64, f64, f64, f64);

/// `[a fixed][middle][b fixed]` of source `[s0, s1)` laid into destination `[d0, d1)`.
/// The middle is stretched from its centre texel so lines at its ends stay one pixel wide (the
/// DWM bitmaps keep their client-edge lines there); it is dropped when there is no room.
fn axis(s0: i32, s1: i32, a: i32, b: i32, d0: f64, d1: f64, out: &mut Vec<Seg>) {
    let n = (d1 - d0).max(0.0);
    let (a, b) = (a.max(0) as f64, b.max(0) as f64);
    let (s0, s1) = (s0 as f64, s1 as f64);
    if n < a + b {
        let h1 = (n / 2.0).floor().min(a);
        let h2 = n - h1;
        if h1 > 0.0 { out.push((s0, h1, d0, h1)); }
        if h2 > 0.0 { out.push((s1 - h2, h2, d0 + h1, h2)); }
        return;
    }
    if a > 0.0 { out.push((s0, a, d0, a)); }
    stretch(s0 + a, s1 - b - s0 - a, d0 + a, n - a - b, out);
    if b > 0.0 { out.push((s1 - b, b, d1 - b, b)); }
}

fn stretch(s0: f64, m: f64, d0: f64, md: f64, out: &mut Vec<Seg>) {
    if m <= 0.0 || md <= 0.0 { return; }
    if md >= m {
        let h1 = ((m - 1.0) / 2.0).floor();
        let h2 = m - 1.0 - h1;
        if h1 > 0.0 { out.push((s0, h1, d0, h1)); }
        out.push((s0 + h1, 1.0, d0 + h1, md - h1 - h2));
        if h2 > 0.0 { out.push((s0 + m - h2, h2, d0 + md - h2, h2)); }
    } else {
        let h1 = (md / 2.0).floor();
        let h2 = md - h1;
        if h1 > 0.0 { out.push((s0, h1, d0, h1)); }
        out.push((s0 + m - h2, h2, d0 + h1, h2));
    }
}

pub struct Decor {
    parts: HashMap<i32, Part>,
    /// colourisation constants the theme supplies (TOPFRAME COLORIZATIONOPACITY, REFLECTIONMAP OPACITY)
    pub theme: ThemeGlass,
    /// TOPFRAME OPACITY per state: the caption text glow
    glow_opacity: [f64; 4],
    /// SIDEHIGHLIGHT / LEFTHIGHLIGHTCORNER OPACITY (active, inactive)
    side_opacity: [f64; 2],
    corner_opacity: [f64; 2],
    /// TEXTGLOW CONTENTMARGINS (l, r, t, b): how far the caption glow reaches past the text
    glow_margins: (f64, f64, f64, f64),
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
        let mut theme = ThemeGlass::default();
        let (mut glow_opacity, mut side_opacity, mut corner_opacity) = ([0.8, 0.6, 0.8, 0.8], [1.0, 0.5], [0.75, 0.75]);
        let mut glow_margins = (17.0, 17.0, 8.0, 10.0);
        let path = crate::config::data_dir().join("theme/aero.msstyles");
        if let Ok(t) = Theme::open(&path) {
            // the atlas id differs between builds; Win7 SP1's aero.msstyles has it at 971
            if let Some(atlas) = t.stream(971) {
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
                        let count = t.int("DWMWindow", p, 0, msstyles::IMAGECOUNT).unwrap_or(1).max(1);
                        parts.insert(p, Part {
                            surf: s, w, fh: h / count, count,
                            margins: t.margins("DWMWindow", p, 0, msstyles::SIZINGMARGINS).unwrap_or([0; 4]),
                        });
                    }
                }
            }
            // per-state values (state 1 active, 2 inactive, 3 / 4 the same maximised)
            const OPACITY: u32 = 2430;
            const COLORIZATIONOPACITY: u32 = 2432;
            let get = |part: i32, state: i32, prop: u32| t.int("DWMWindow", part, state, prop).map(|v| v as f64 / 100.0);
            for i in 0..4 {
                if let Some(v) = get(TOPFRAME, i as i32 + 1, COLORIZATIONOPACITY) { theme.opacity[i] = v; }
                if let Some(v) = get(REFLECTIONMAP, i as i32 + 1, OPACITY) { theme.reflection[i] = v; }
                if let Some(v) = get(TOPFRAME, i as i32 + 1, OPACITY) { glow_opacity[i] = v; }
            }
            for i in 0..2 {
                if let Some(v) = get(SIDEHIGHLIGHT, i as i32 + 1, OPACITY) { side_opacity[i] = v; }
                if let Some(v) = get(LEFTHIGHLIGHTCORNER, i as i32 + 1, OPACITY) { corner_opacity[i] = v; }
            }
            if let Some(m) = t.margins("DWMWindow", TEXTGLOW, 0, msstyles::CONTENTMARGINS) {
                glow_margins = (m[0] as f64, m[1] as f64, m[2] as f64, m[3] as f64);
            }
        }
        Decor { parts, theme, glow_opacity, side_opacity, corner_opacity, glow_margins }
    }

    pub fn themed(&self) -> bool { self.parts.contains_key(&7) && self.parts.contains_key(&TOPFRAME) }

    pub fn reflection_map(&self) -> Option<&ImageSurface> { self.parts.get(&REFLECTIONMAP).map(|p| &p.surf) }

    /// Draw the cells `xs × ys` of frame `idx` of part `p` (source coordinates within the frame).
    fn cells(&self, cr: &Context, p: i32, idx: i32, xs: &[Seg], ys: &[Seg]) -> bool {
        let Some(part) = self.parts.get(&p) else { return false };
        let fy = (part.fh * idx.clamp(0, part.count - 1)) as f64;
        for &(sx, sw, dx, dw) in xs {
            for &(sy, sh, dy, dh) in ys {
                if sw <= 0.0 || sh <= 0.0 || dw <= 0.0 || dh <= 0.0 { continue; }
                cr.save().ok();
                cr.rectangle(dx, dy, dw, dh);
                cr.clip();
                cr.translate(dx, dy);
                cr.scale(dw / sw, dh / sh);
                cr.set_source_surface(&part.surf, -sx, -(fy + sy)).ok();
                cr.source().set_filter(cairo::Filter::Nearest);
                cr.paint().ok();
                cr.restore().ok();
            }
        }
        true
    }

    /// Nine-slice frame `idx` of part `p` into (x, y, w, h) by its SIZINGMARGINS. `gutter` drops
    /// the 1-px duplicated border every DWM frame bitmap carries for texture filtering.
    fn sliced(&self, cr: &Context, p: i32, idx: i32, x: f64, y: f64, w: f64, h: f64, gutter: bool) -> bool {
        let Some(part) = self.parts.get(&p) else { return false };
        let g = gutter as i32;
        let [ml, mr, mt, mb] = part.margins;
        let (mut xs, mut ys) = (vec![], vec![]);
        axis(g, part.w - g, ml - g, mr - g, x, x + w, &mut xs);
        axis(g, part.fh - g, mt - g, mb - g, y, y + h, &mut ys);
        self.cells(cr, p, idx, &xs, &ys)
    }

    /// TOPFRAME / BOTTOMFRAME: 2·n+1 texels wide, a miniature of the frame whose left and right
    /// halves match LEFTFRAME / RIGHTFRAME and whose centre column (with the client edge) spans
    /// the width. Slice each half like its side part, stretch the centre (vertically by its own
    /// SIZINGMARGINS).
    fn edge(&self, cr: &Context, p: i32, side: (i32, i32), idx: i32, x: f64, y: f64, w: f64, h: f64, b: f64) -> bool {
        let (Some(part), Some(l), Some(r)) = (self.parts.get(&p), self.parts.get(&side.0), self.parts.get(&side.1)) else { return false };
        let c = part.w / 2;
        let mut xs = vec![];
        axis(1, c, l.margins[0] - 1, l.margins[1] - 1, x, x + b, &mut xs);
        xs.push((c as f64, 1.0, x + b, (w - 2.0 * b).max(0.0)));
        axis(c + 1, part.w - 1, r.margins[0] - 1, r.margins[1] - 1, x + w - b, x + w, &mut xs);
        let mut ys = vec![];
        axis(1, part.fh - 1, part.margins[2] - 1, part.margins[3] - 1, y, y + h, &mut ys);
        self.cells(cr, p, idx, &xs, &ys)
    }

    /// Draw frame `idx` of part `p`, nine-sliced into (x, y, w, h) with bilinear filtering (glows).
    fn nine(&self, cr: &Context, p: i32, idx: i32, x: f64, y: f64, w: f64, h: f64) -> bool {
        let Some(part) = self.parts.get(&p) else { return false };
        let fy = part.fh * idx.clamp(0, part.count - 1);
        let [ml, mr, mt, mb] = part.margins;
        let (ml, mr, mt, mb) = (ml as f64, mr as f64, mt as f64, mb as f64);
        let (sw, sh) = (part.w as f64, part.fh as f64);
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
                cr.set_source_surface(&part.surf, -sx0, -(fy as f64) - sy0).ok();
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
        let pw = part.w as f64;
        let fh = part.fh as f64;
        let (x, y) = ((cx - pw / 2.0).round(), (cy - fh / 2.0).round());
        cr.save().ok();
        cr.rectangle(x, y, pw, fh);
        cr.clip();
        cr.set_source_surface(&part.surf, x, y - fh * idx.clamp(0, part.count - 1) as f64).ok();
        cr.paint().ok();
        cr.restore().ok();
    }

    /// Button rectangles (x, y, w, h) in frame coordinates, right-to-left.
    pub fn button_rects(&self, w: i32, maximized: bool, small: bool, buttons: &[(Btn, BState)]) -> Vec<(Btn, [i32; 4])> {
        if small && self.parts.contains_key(&SMALLBUTTONACTIVECLOSE) {
            // tool windows: the small close button, centred in the caption
            let x = w - BORDER - SMALL_CLOSE - 1;
            let y = (TOP - SMALL_CLOSE) / 2;
            return buttons.iter().filter(|(b, _)| *b == Btn::Close).map(|(b, _)| (*b, [x, y, SMALL_CLOSE, SMALL_CLOSE])).collect();
        }
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
        let st = match (p.active, p.maximized) { (true, false) => 0, (false, false) => 1, (true, true) => 2, (false, true) => 3 };
        let idx = if p.active { 0 } else { 1 };
        let (sx, sy) = (p.screen_pos.0 as f64, p.screen_pos.1 as f64);
        // a maximised window's borders lie off-screen: draw the full frame shifted by them
        let off = if p.maximized { BORDER as f64 } else { 0.0 };
        let (vw, vh) = (w + 2.0 * off, h + 2.0 * off);
        let (b, top) = (BORDER as f64, TOP as f64);
        let themed = self.themed();
        cr.save().ok();
        cr.rectangle(0.0, 0.0, w, h);
        cr.clip();
        // ---- glass (the X shape / picom cut the rounded corners)
        if p.composited {
            cr.save().ok();
            cr.set_operator(cairo::Operator::Source);
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
            cr.paint().ok();
            cr.restore().ok();
        }
        // only the visible border strips: the client covers the rest (and fake glass is per pixel)
        let (vt, vb) = if p.maximized { (MAX_TOP as f64, 0.0) } else { (top, b) };
        let strips = [(0.0, 0.0, w, vt), (0.0, vt, vb, h - vt - vb), (w - vb, vt, vb, h - vt - vb), (0.0, h - vb, w, vb)];
        for (x, y, sw, sh) in strips {
            if sw <= 0.0 || sh <= 0.0 { continue; }
            cr.save().ok();
            cr.rectangle(x, y, sw, sh);
            cr.clip();
            let refl = (p.glass)(cr);
            if let Some(map) = self.reflection_map() {
                paint_reflection(cr, map, p.screen_size, (sx - off, sy - off, vw, vh), sx, sy, refl);
            }
            cr.restore().ok();
        }
        cr.save().ok();
        cr.translate(-off, -off);
        if themed && !p.small {
            // the sheen in the top-left corner (drawn from inside its 1-px gutter)
            if let Some(part) = self.parts.get(&LEFTHIGHLIGHTCORNER) {
                cr.save().ok();
                cr.rectangle(0.0, 0.0, (part.w - 1) as f64, (part.fh - 1) as f64);
                cr.clip();
                cr.set_source_surface(&part.surf, -1.0, -1.0 - (part.fh * idx.min(part.count - 1)) as f64).ok();
                cr.paint_with_alpha(self.corner_opacity[idx as usize]).ok();
                cr.restore().ok();
            }
            // SIDEHIGHLIGHT: the light the side borders catch just below the caption
            if !p.maximized {
                if let Some(part) = self.parts.get(&SIDEHIGHLIGHT) {
                    for x0 in [0.0, vw - b] {
                        cr.save().ok();
                        cr.rectangle(x0, top, b, SIDE_HL_H);
                        cr.clip();
                        cr.translate(x0, top);
                        cr.scale(b / part.w as f64, SIDE_HL_H / part.fh as f64);
                        cr.set_source_surface(&part.surf, 0.0, -(part.fh * idx.min(part.count - 1)) as f64).ok();
                        cr.source().set_filter(cairo::Filter::Good);
                        cr.paint_with_alpha(self.side_opacity[idx as usize]).ok();
                        cr.restore().ok();
                    }
                }
            }
        }
        // ---- the frame bitmaps
        let drawn = themed && if p.small {
            self.edge(cr, SMALLTOPFRAME, (SMALLLEFTFRAME, SMALLRIGHTFRAME), idx, 0.0, 0.0, vw, top, b)
                && self.sliced(cr, SMALLLEFTFRAME, idx, 0.0, top, b, vh - top - b, true)
                && self.sliced(cr, SMALLRIGHTFRAME, idx, vw - b, top, b, vh - top - b, true)
                && self.edge(cr, SMALLBOTTOMFRAME, (SMALLLEFTFRAME, SMALLRIGHTFRAME), idx, 0.0, vh - b, vw, b, b)
        } else {
            let (tp, bp) = if p.maximized { (TOPFRAMENOSHADOW, BOTTOMFRAMENOSHADOW) } else { (TOPFRAME, BOTTOMFRAME) };
            self.edge(cr, tp, (LEFTFRAME, RIGHTFRAME), idx, 0.0, 0.0, vw, top, b)
                && self.sliced(cr, LEFTFRAME, idx, 0.0, top, b, vh - top - b, true)
                && self.sliced(cr, RIGHTFRAME, idx, vw - b, top, b, vh - top - b, true)
                && self.edge(cr, bp, (LEFTFRAME, RIGHTFRAME), idx, 0.0, vh - b, vw, b, b)
        };
        cr.restore().ok();
        if !drawn { fallback_rims(cr, p, w, h); }
        // ---- icon + title with the Aero text glow
        let bx = if p.maximized { 0.0 } else { b };
        let mut tx = bx + 2.0;
        if let Some(icon) = p.icon {
            let iy = if p.maximized { ((MAX_TOP as f64 - 16.0) / 2.0).round() } else { (b + (CAPTION as f64 - 16.0) / 2.0 - 3.0).round() };
            cr.save().ok();
            cr.translate(tx, iy);
            let s = 16.0 / icon.width().max(1) as f64;
            cr.scale(s, s);
            cr.set_source_surface(icon, 0.0, 0.0).ok();
            cr.source().set_filter(cairo::Filter::Good);
            cr.paint().ok();
            cr.restore().ok();
            tx += 22.0;
        }
        let rects = self.button_rects(p.w, p.maximized, p.small, p.buttons);
        let tmax = rects.last().map(|r| r.1[0] as f64).unwrap_or(w) - tx - 8.0;
        if tmax > 10.0 {
            let layout = pangocairo::functions::create_layout(cr);
            let mut fd = pangocairo::pango::FontDescription::from_string("Segoe UI 9");
            fd.set_absolute_size(12.0 * pangocairo::pango::SCALE as f64);
            layout.set_font_description(Some(&fd));
            layout.set_text(p.title);
            layout.set_width((tmax * pangocairo::pango::SCALE as f64) as i32);
            layout.set_ellipsize(pangocairo::pango::EllipsizeMode::End);
            let (_, lr) = layout.pixel_extents();
            let lh = lr.height() as f64;
            let ty = if p.maximized { ((MAX_TOP as f64 - lh) / 2.0).round() } else { (b + (CAPTION as f64 - lh) / 2.0 - 3.0).round() };
            // (Win7 puts the caption's baseline level with the bottom of the caption buttons, a
            // little above the caption band's middle)
            // TEXTGLOW: the text box (+1 px) grown by the part's CONTENTMARGINS, nine-sliced
            if self.parts.contains_key(&TEXTGLOW) && !p.title.is_empty() {
                let (cl, cr_, ct, cb) = self.glow_margins;
                let gx = tx + lr.x() as f64 - 1.0 - cl;
                let gy = ty + lr.y() as f64 - 1.0 - ct;
                let gw = lr.width() as f64 + 2.0 + cl + cr_;
                let gh = lh + 2.0 + ct + cb;
                cr.save().ok();
                // the glow stops at the text's left edge (Win7 clips it to the caption text area)
                cr.rectangle(tx - 4.0, 0.0, w, h);
                cr.clip();
                cr.push_group();
                self.nine(cr, TEXTGLOW, 0, gx, gy, gw, gh);
                cr.pop_group_to_source().ok();
                cr.paint_with_alpha(self.glow_opacity[st]).ok();
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
            let level = p.glow.iter().find(|(b, _)| b == btn).map(|x| x.1.clamp(0.0, 1.0));
            match (state, level) {
                // DWM cross-fades the normal and the hot button (glow included) while the pointer
                // comes and goes; pressing and disabling switch at once
                (BState::Normal | BState::Hot, Some(l)) if l > 0.0 && l < 1.0 => {
                    cr.push_group();
                    cr.push_group();
                    self.caption_button(cr, p, i, n, rects.len(), *btn, rect, BState::Normal);
                    cr.pop_group_to_source().ok();
                    cr.paint_with_alpha(1.0 - l).ok();
                    cr.push_group();
                    self.caption_button(cr, p, i, n, rects.len(), *btn, rect, BState::Hot);
                    cr.pop_group_to_source().ok();
                    cr.set_operator(cairo::Operator::Add);
                    cr.paint_with_alpha(l).ok();
                    cr.set_operator(cairo::Operator::Over);
                    cr.pop_group_to_source().ok();
                    cr.paint().ok();
                }
                (BState::Normal | BState::Hot, Some(l)) => self.caption_button(cr, p, i, n, rects.len(), *btn, rect, if l >= 1.0 { BState::Hot } else { BState::Normal }),
                _ => self.caption_button(cr, p, i, n, rects.len(), *btn, rect, state),
            }
        }
        cr.restore().ok();
    }

    /// One caption button (`i` of `count` drawn right to left, `n` buttons in all) in `state`.
    fn caption_button(&self, cr: &Context, p: &Params, i: usize, n: usize, count: usize, btn: Btn, rect: &[i32; 4], state: BState) {
        let [x, y, bw, bh] = rect.map(|v| v as f64);
        let si = state as i32 - 1;
        if p.small && self.parts.contains_key(&SMALLBUTTONACTIVECLOSE) {
            if state == BState::Hot { self.nine(cr, SMALLBUTTONCLOSEGLOW, 0, x - 5.0, y - 6.0, bw + 10.0, bh + 12.0); }
            self.nine(cr, if p.active { SMALLBUTTONACTIVECLOSE } else { SMALLBUTTONINACTIVECLOSE }, si, x, y, bw, bh);
            self.glyph(cr, SMALLBUTTONCLOSEGLYPH96, if !p.active && state == BState::Normal { 3 } else { si }, x + bw / 2.0, y + bh / 2.0);
            return;
        }
        let alone = n == 1;
        let leftmost = i == count - 1;
        let (bg, glyph_part) = match btn {
            Btn::Close => (if alone { if p.active { 9 } else { 10 } } else if p.active { 7 } else { 8 }, 12),
            Btn::Max => (if p.active { 5 } else { 6 }, if p.maximized { 29 } else { 21 }),
            Btn::Min => (if leftmost { if p.active { 3 } else { 4 } } else if p.active { 5 } else { 6 }, 25),
            Btn::Help => (if p.active { 3 } else { 4 }, 17),
        };
        if self.parts.contains_key(&7) {
            if state == BState::Hot {
                let glow = if btn == Btn::Close { BUTTONCLOSEGLOW } else { BUTTONGLOW };
                self.nine(cr, glow, 0, x - 8.0, y - 7.0, bw + 16.0, bh + 15.0);
            }
            self.nine(cr, bg, si, x, y, bw, bh);
            let gidx = if p.active { si } else { 3.min(si.max(0)) };
            self.glyph(cr, glyph_part, if !p.active && state == BState::Normal { 3 } else { gidx }, x + bw / 2.0, y + bh / 2.0);
        } else {
            fallback_button(cr, btn, state, p.active, p.maximized, x, y, bw, bh);
        }
    }

    /// The frame area a caption button's paint (its hover glow included) can touch, in frame
    /// coordinates: what a glow fade has to repaint.
    pub fn button_paint_rect(rect: &[i32; 4]) -> [i32; 4] { [rect[0] - 9, rect[1] - 8, rect[2] + 18, rect[3] + 17] }
}

/// The Win7 reflection streaks (DWMWindow part 40 REFLECTIONMAP) over the current clip: the map
/// is stretched over the whole virtual screen and slides 13 % with the window (parallax).
/// `win` = the window rectangle on screen (x, y, w, h), (sx, sy) = screen position of the origin.
pub fn paint_reflection(cr: &Context, map: &ImageSurface, screen: (i32, i32), win: (f64, f64, f64, f64), sx: f64, sy: f64, opacity: f64) {
    if opacity <= 0.0 { return; }
    let (sw, sh) = (screen.0.max(1) as f64, screen.1.max(1) as f64);
    let (rw, rh) = (map.width().max(1) as f64, map.height().max(1) as f64);
    let shift = (win.0 + win.2 / 2.0 - sw / 2.0) * 0.13;
    cr.save().ok();
    cr.translate(shift - sx, -sy);
    cr.scale(sw / rw, sh / rh);
    cr.set_source_surface(map, 0.0, 0.0).ok();
    cr.source().set_filter(cairo::Filter::Bilinear);
    cr.source().set_extend(cairo::Extend::Pad);
    cr.paint_with_alpha(opacity.min(1.0)).ok();
    cr.restore().ok();
}

/// Hand-drawn rims for when no aero.msstyles was extracted.
fn fallback_rims(cr: &Context, p: &Params, w: f64, h: f64) {
    let (top, b) = if p.maximized { (MAX_TOP as f64, 0.0) } else { (TOP as f64, BORDER as f64) };
    let r = if p.maximized || p.small { 0.0 } else { RADIUS };
    cr.set_line_width(1.0);
    crate::glass::rounded_top(cr, 0.5, 0.5, w - 1.0, h - 1.0, r, 2.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, if p.active { 0.75 } else { 0.45 });
    cr.stroke().ok();
    crate::glass::rounded_top(cr, 1.5, 1.5, w - 3.0, h - 3.0, (r - 1.0).max(0.0), 1.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.45);
    cr.stroke().ok();
    if !p.maximized {
        cr.rectangle(b - 1.5, top - 1.5, w - 2.0 * b + 3.0, h - top - b + 3.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.55);
        cr.stroke().ok();
        cr.rectangle(b - 0.5, top - 0.5, w - 2.0 * b + 1.0, h - top - b + 1.0);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        cr.stroke().ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_frame_slices_to_win7_rim() {
        // LEFTFRAME 11 px (gutter + B W G G G G G W D + gutter), margins 6 / 2, into the 8-px border:
        // B W G G G | G W | D  (the stretch zone keeps its end texel, the client-edge highlight)
        let mut xs = vec![];
        axis(1, 10, 5, 1, 0.0, 8.0, &mut xs);
        assert_eq!(xs, vec![(1.0, 5.0, 0.0, 5.0), (6.0, 1.0, 5.0, 1.0), (8.0, 1.0, 6.0, 1.0), (9.0, 1.0, 7.0, 1.0)]);
        // TOPFRAME rows 1..26, margins 6 / 2, into the 30-px caption: the white client-edge row (24)
        // lands on y = 28, the dark one (25) on y = 29
        let mut ys = vec![];
        axis(1, 26, 5, 1, 0.0, 30.0, &mut ys);
        let at = |y: f64| ys.iter().find(|s| y >= s.2 && y < s.2 + s.3).map(|s| s.0 + ((y - s.2) * s.1 / s.3).floor()).unwrap();
        assert_eq!((at(0.0), at(28.0), at(29.0), at(27.0)), (1.0, 24.0, 25.0, 23.0));
    }
}
