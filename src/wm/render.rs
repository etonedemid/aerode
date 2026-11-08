//! The scene: windows with their frames and glass, panels, overlays, the pointer.
use super::blur::BlurElement;
use super::decor::{self, Params};
use super::{anim, is_app, Backend, Rect, Win, Wm};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement, RescaleRenderElement};
use smithay::backend::renderer::element::{AsRenderElements, Id, Kind as EKind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, ImportDma, Offscreen};
use smithay::desktop::layer_map_for_output;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::render_elements;
use smithay::utils::{Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;
use std::time::Instant;

render_elements! {
    pub El<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Mem=MemoryRenderBufferRenderElement<GlesRenderer>,
    Blur=BlurElement,
    Solid=SolidColorRenderElement,
    AnimSurface=RelocateRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
    AnimMem=RelocateRenderElement<RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>>,
}

/// A window's four border strips, painted once per change of look.
pub struct FrameCache {
    /// (buffer, x, y) relative to the frame's top-left
    pub strips: Vec<(MemoryRenderBuffer, i32, i32)>,
}

/// Stable ids so the damage tracker recognises glass from frame to frame.
pub struct GlassIds(pub [Id; 4]);
impl Default for GlassIds { fn default() -> GlassIds { GlassIds([Id::new(), Id::new(), Id::new(), Id::new()]) } }

/// A running window animation: the frame's rectangle moves from `from` to `to`, fading `alpha`.
pub struct WinAnim { pub from: Rect, pub to: Option<Rect>, pub a0: f32, pub a1: f32, pub tween: anim::Tween }

impl WinAnim {
    pub fn done(&self, now: Instant) -> bool { self.tween.done(now) }
}

/// The Aero Snap preview: glass with a white rim growing to its zone.
pub struct SnapSheet { pub from: Rect, pub to: Rect, pub tween: anim::Tween, pub buf: Option<MemoryRenderBuffer>, pub ids: GlassIds }

impl SnapSheet {
    pub fn animating(&self, now: Instant) -> bool { !self.tween.done(now) }
    fn rect(&self, now: Instant) -> Rect { anim::lerp_rect(self.from, self.to, self.tween.at(now)) }
}

const RIPPLE_SIZE: i32 = 112;

/// Cairo ARGB32 is exactly what `Fourcc::Argb8888` means, premultiplied and all.
pub fn surface_to_buffer(s: &mut cairo::ImageSurface) -> Option<MemoryRenderBuffer> {
    s.flush();
    let (w, h, stride) = (s.width(), s.height(), s.stride());
    let data = s.data().ok()?;
    if stride == w * 4 {
        return Some(MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (w, h), 1, Transform::Normal, None));
    }
    let mut packed = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize { packed.extend_from_slice(&data[y * stride as usize..y * stride as usize + w as usize * 4]); }
    Some(MemoryRenderBuffer::from_slice(&packed, Fourcc::Argb8888, (w, h), 1, Transform::Normal, None))
}

/// The application's icon: a PNG named after its app id in the icon directories.
pub fn theme_icon(app_id: &str) -> Option<cairo::ImageSurface> {
    if app_id.is_empty() { return None; }
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [format!("{home}/.local/share/icons/Aerode/48x48/apps"), format!("{home}/.local/share/icons/Aerode/32x32/apps"), format!("{home}/.local/share/icons/hicolor/48x48/apps"),
        "/usr/share/icons/hicolor/48x48/apps".into(), "/usr/share/icons/hicolor/64x64/apps".into(), "/usr/share/pixmaps".into()];
    let id = app_id.to_lowercase();
    let last = id.rsplit('.').next().unwrap_or(&id).to_string();
    for n in [id.as_str(), last.as_str(), "application-x-executable"] {
        for d in &dirs {
            if let Ok(mut f) = std::fs::File::open(format!("{d}/{n}.png")) {
                if let Ok(img) = cairo::ImageSurface::create_from_png(&mut f) { return Some(img); }
            }
        }
    }
    None
}

fn phys(x: i32, y: i32, s: f64) -> Point<i32, Physical> { Point::from(((x as f64 * s).round() as i32, (y as f64 * s).round() as i32)) }
fn phys_rect(r: Rect, s: f64) -> Rectangle<i32, Physical> {
    Rectangle::new(phys(r.x, r.y, s), Size::from((((r.w.max(0)) as f64 * s).round() as i32, ((r.h.max(0)) as f64 * s).round() as i32)))
}

impl Wm {
    pub fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        self.with_renderer(|r| r.import_dmabuf(dmabuf, None).is_ok()).unwrap_or(false)
    }

    pub fn with_renderer<T>(&mut self, f: impl FnOnce(&mut GlesRenderer) -> T) -> Option<T> {
        match &mut self.backend {
            Backend::None => None,
            Backend::Winit(w) => Some(f(w.backend.renderer())),
            Backend::Udev(u) => u.renderer.as_mut().map(f),
        }
    }

    // ------------------------------------------------------------- frames
    fn paint_frame(&self, c: &Win) -> FrameCache {
        let fr = self.frame_rect(c);
        let (fw, fh) = (fr.w, fr.h);
        let btns = self.buttons_for(c);
        let now = Instant::now();
        let glow = c.glow.levels(now);
        let (active, maximized) = (self.active == c.id, c.maximized);
        let focus_level = c.focus_fade.filter(|t| !t.done(now)).map(|t| t.at(now)).filter(|l| *l > 0.0 && *l < 1.0);
        let glass_on = |cr: &cairo::Context| self.glass.fill(cr, fr.x as f64, fr.y as f64, true, maximized, true);
        let glass_off = |cr: &cairo::Context| self.glass.fill(cr, fr.x as f64, fr.y as f64, false, maximized, true);
        let screen_size = self.screen_size();
        let params = |active: bool| Params {
            w: fw, h: fh, title: &c.title, icon: c.icon.as_ref(), active, maximized, small: c.kind == super::Kind::Utility,
            buttons: &btns, glow: &glow, composited: true, glass: if active { &glass_on } else { &glass_off }, screen_pos: (fr.x, fr.y), screen_size,
        };
        let (b, t) = (self.border_of(c), self.top_of(c));
        let mut strips = Vec::new();
        for (x, y, sw, sh) in [(0, 0, fw, t), (0, fh - b, fw, b), (0, t, b, fh - t - b), (fw - b, t, b, fh - t - b)] {
            if sw <= 0 || sh <= 0 { continue; }
            let Ok(mut surf) = cairo::ImageSurface::create(cairo::Format::ARgb32, sw, sh) else { continue };
            {
                let Ok(cr) = cairo::Context::new(&surf) else { continue };
                cr.translate(-x as f64, -y as f64);
                cr.rectangle(x as f64, y as f64, sw as f64, sh as f64);
                cr.clip();
                match focus_level {
                    Some(l) => {
                        cr.push_group();
                        self.decor.render(&cr, &params(false));
                        cr.pop_group_to_source().ok();
                        cr.paint_with_alpha(1.0 - l).ok();
                        cr.push_group();
                        self.decor.render(&cr, &params(true));
                        cr.pop_group_to_source().ok();
                        cr.set_operator(cairo::Operator::Add);
                        cr.paint_with_alpha(l).ok();
                    }
                    None => self.decor.render(&cr, &params(active)),
                }
                // the rounded corners, in the picture as well as in the blur
                if !maximized && c.kind != super::Kind::Utility { cut_corners(&cr, fw, fh, decor::RADIUS); }
            }
            if let Some(buf) = surface_to_buffer(&mut surf) { strips.push((buf, x, y)); }
        }
        FrameCache { strips }
    }

    /// The frame of `w` turns active (`on`) or inactive: DWM cross-fades the two looks.
    pub fn fade_frame(&mut self, w: u32, on: bool) {
        let enabled = self.effect("animate_controls");
        let Some(c) = self.wins.get_mut(&w) else { return };
        c.frame_cache = None;
        if enabled && c.mapped {
            let (from, to) = if on { (0.0, 1.0) } else { (1.0, 0.0) };
            c.focus_fade = Some(anim::Tween::new(from, to, anim::FOCUS_FADE, anim::Ease::Linear));
        }
        self.needs_redraw = true;
    }

    // ------------------------------------------------------------- animations
    pub fn begin_geom_anim(&mut self, w: u32) {
        if !self.window_anims() { return; }
        let Some(c) = self.wins.get(&w) else { return };
        if !c.mapped { return; }
        let from = self.frame_rect(c);
        self.wins.get_mut(&w).unwrap().anim = Some(WinAnim { from, to: None, a0: 1.0, a1: 1.0, tween: anim::Tween::new(0.0, 1.0, anim::SNAP_GROW, anim::Ease::InOut) });
        self.needs_redraw = true;
    }

    pub fn begin_open_anim(&mut self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        let fr = self.frame_rect(c);
        let (dx, dy) = (fr.w / 20, fr.h / 20);
        let from = Rect { x: fr.x + dx, y: fr.y + dy, w: fr.w - 2 * dx, h: fr.h - 2 * dy };
        self.wins.get_mut(&w).unwrap().anim = Some(WinAnim { from, to: None, a0: 0.0, a1: 1.0, tween: anim::Tween::new(0.0, 1.0, anim::PROP_LINGER, anim::Ease::Out) });
    }

    pub fn begin_minimize_anim(&mut self, w: u32, restoring: bool) {
        let Some(c) = self.wins.get(&w) else { return };
        let fr = self.frame_rect(c);
        let slot = c.icon_geometry.unwrap_or(Rect { x: fr.x + fr.w / 2 - 30, y: self.screen_size().1 - 40, w: 60, h: 40 });
        let (from, to, a0, a1) = if restoring { (slot, None, 0.0, 1.0) } else { (fr, Some(slot), 1.0, 0.0) };
        self.wins.get_mut(&w).unwrap().anim = Some(WinAnim { from, to, a0, a1, tween: anim::Tween::new(0.0, 1.0, anim::PROP_LINGER, anim::Ease::InOut) });
    }

    // ------------------------------------------------------------- snap preview, ripple
    pub fn show_snap_preview(&mut self, w: u32, side: i8, px: i32, py: i32) {
        let Some(c) = self.wins.get(&w) else { return };
        let wa = self.workarea_for(self.monitor_of(&Rect { x: px, y: py, w: 1, h: 1 }));
        let r = match side {
            -1 => Rect { w: wa.w / 2, ..wa },
            1 => Rect { x: wa.x + wa.w - wa.w / 2, w: wa.w / 2, ..wa },
            2 => { let fr = self.frame_rect(c); Rect { x: fr.x, w: fr.w, ..wa } }
            _ => wa,
        };
        self.snap_wa = Some(wa);
        let to = Rect { x: r.x + 8, y: r.y + 8, w: r.w - 16, h: r.h - 16 };
        let animate = self.window_anims();
        let now = Instant::now();
        let from = match &self.snap_preview {
            Some(s) => s.2.rect(now),
            None if animate => Rect { x: px - 20, y: py - 20, w: 40, h: 40 },
            None => to,
        };
        let buf = self.paint_sheet(to);
        let dur = if animate { anim::SNAP_GROW } else { std::time::Duration::ZERO };
        let ids = self.snap_preview.take().map(|s| s.2.ids).unwrap_or_default();
        self.snap_preview = Some((w, side, SnapSheet { from, to, tween: anim::Tween::new(0.0, 1.0, dur, anim::Ease::Out), buf, ids }));
        if animate { self.ripple = Some((px, py, anim::Tween::new(0.0, 1.0, anim::RIPPLE, anim::Ease::Linear))); }
        self.needs_redraw = true;
    }

    /// Glass with a white rim (the Peek sheet).
    pub fn paint_sheet(&self, r: Rect) -> Option<MemoryRenderBuffer> {
        let mut s = cairo::ImageSurface::create(cairo::Format::ARgb32, r.w.max(1), r.h.max(1)).ok()?;
        {
            let cr = cairo::Context::new(&s).ok()?;
            let rad = 6.0f64.min(r.w as f64 / 2.0).min(r.h as f64 / 2.0);
            crate::glass::rounded(&cr, 0.5, 0.5, r.w as f64 - 1.0, r.h as f64 - 1.0, rad);
            cr.save().ok();
            cr.clip_preserve();
            self.glass.fill(&cr, r.x as f64, r.y as f64, true, false, true);
            cr.restore().ok();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.8);
            cr.set_line_width(2.0);
            cr.stroke().ok();
        }
        surface_to_buffer(&mut s)
    }

    pub fn hide_snap_preview(&mut self) {
        self.snap_preview = None;
        self.snap_wa = None;
        self.needs_redraw = true;
    }

    pub fn drop_snap_preview(&mut self) -> Option<i8> {
        let side = self.snap_preview.take().map(|s| s.1);
        self.needs_redraw = true;
        side
    }

    fn paint_ripple(t: f64) -> Option<MemoryRenderBuffer> {
        let mut s = cairo::ImageSurface::create(cairo::Format::ARgb32, RIPPLE_SIZE, RIPPLE_SIZE).ok()?;
        {
            let cr = cairo::Context::new(&s).ok()?;
            let c = RIPPLE_SIZE as f64 / 2.0;
            let e = anim::Ease::Out.apply(t);
            let fade = 1.0 - t.clamp(0.0, 1.0);
            let g = cairo::RadialGradient::new(c, c, 0.0, c, c, 10.0 + 22.0 * e);
            g.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.75 * fade);
            g.add_color_stop_rgba(1.0, 0.75, 0.88, 1.0, 0.0);
            cr.set_source(&g).ok();
            cr.paint().ok();
            let r = 8.0 + (c - 10.0) * e;
            let ring = cairo::RadialGradient::new(c, c, (r - 7.0).max(0.0), c, c, r + 2.0);
            ring.add_color_stop_rgba(0.0, 1.0, 1.0, 1.0, 0.0);
            ring.add_color_stop_rgba(0.75, 0.85, 0.94, 1.0, 0.7 * fade);
            ring.add_color_stop_rgba(1.0, 1.0, 1.0, 1.0, 0.0);
            cr.set_source(&ring).ok();
            cr.arc(c, c, r + 2.0, 0.0, std::f64::consts::TAU);
            cr.fill().ok();
        }
        surface_to_buffer(&mut s)
    }

    // ------------------------------------------------------------- the scene
    /// Repaint frames that changed. Needs &mut self apart from building the element list.
    pub fn prepare_frames(&mut self) {
        let ids: Vec<u32> = self.wins.iter().filter(|(_, c)| c.framed && c.mapped && !c.fullscreen && c.frame_cache.is_none() && !c.minimized).map(|(i, _)| *i).collect();
        for id in ids {
            let cache = self.paint_frame(&self.wins[&id]);
            self.wins.get_mut(&id).unwrap().frame_cache = Some(cache);
        }
    }

    /// Everything on `output`, front to back.
    pub fn elements(&mut self, renderer: &mut GlesRenderer, output: &Output) -> Vec<El> {
        let scale = Scale::from(output.current_scale().fractional_scale());
        let s = scale.x;
        let Some(og) = self.space.output_geometry(output) else { return vec![] };
        let mut out: Vec<El> = Vec::new();
        let oo = (og.loc.x, og.loc.y);
        self.cursor_elements(renderer, output, s, oo, &mut out);

        if let Some(_) = &self.lock {
            self.layer_elements(renderer, output, &[Layer::Overlay], s, oo, true, &mut out);
            return out;
        }

        // Alt+Tab, ripple, snap sheet (above other windows' frames but below the dragged window)
        if let Some(sw) = &self.switcher {
            if let Some(buf) = &sw.buf {
                let at = phys(sw.pos.0 - oo.0, sw.pos.1 - oo.1, s).to_f64();
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, buf, None, None, None, EKind::Unspecified) { out.push(El::Mem(e)); }
                let r = Rect { x: sw.pos.0 - oo.0, y: sw.pos.1 - oo.1, w: sw.w, h: sw.h };
                let geo = phys_rect(r, s);
                out.push(El::Blur(BlurElement::new(self.switcher_id.clone(), geo, geo, 7.0, 1.0)));
            }
        }
        self.layer_elements(renderer, output, &[Layer::Overlay], s, oo, false, &mut out);
        let now = Instant::now();
        if let Some((rx, ry, t)) = &self.ripple {
            if let Some(buf) = Self::paint_ripple(t.at(now)) {
                let at = phys(rx - RIPPLE_SIZE / 2 - oo.0, ry - RIPPLE_SIZE / 2 - oo.1, s).to_f64();
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, &buf, None, None, None, EKind::Unspecified) { out.push(El::Mem(e)); }
            }
        }

        // a fullscreen window on top covers the panels of its monitor
        let top_full = self.stack.iter().rev().filter_map(|w| self.wins.get(w)).find(|c| self.should_show(c) && self.frame_on_output(c, &og)).is_some_and(|c| c.fullscreen);
        if !top_full { self.layer_elements(renderer, output, &[Layer::Top], s, oo, false, &mut out); }

        let dragged = self.drag.map(|d| d.0);
        let stack = self.stack.clone();
        let mut covered = false;
        for id in stack.iter().rev() {
            let Some(c) = self.wins.get(id) else { continue };
            let animating = c.anim.is_some();
            if !(self.should_show(c) || (animating && c.mapped)) { continue; }
            if !self.frame_on_output(c, &og) && !animating { continue; }
            if covered { break; }
            self.window_elements(renderer, c, s, oo, &mut out);
            if Some(*id) == dragged { self.sheet_elements(renderer, s, oo, &mut out); }
            // nothing shows through an opaque fullscreen window
            if c.fullscreen && !animating && self.frame_on_output(c, &og) { covered = true; }
        }
        if dragged.is_none_or(|d| !self.wins.contains_key(&d)) { self.sheet_elements(renderer, s, oo, &mut out); }
        if !covered { self.layer_elements(renderer, output, &[Layer::Bottom, Layer::Background], s, oo, false, &mut out); }
        out
    }

    fn frame_on_output(&self, c: &Win, og: &Rectangle<i32, smithay::utils::Logical>) -> bool {
        self.frame_rect(c).smithay().overlaps(*og)
    }

    fn sheet_elements(&self, renderer: &mut GlesRenderer, s: f64, oo: (i32, i32), out: &mut Vec<El>) {
        let Some((_, _, sheet)) = &self.snap_preview else { return };
        let Some(buf) = &sheet.buf else { return };
        let r = sheet.rect(Instant::now());
        let r = Rect { x: r.x - oo.0, y: r.y - oo.1, ..r };
        let (kx, ky) = (r.w as f64 / sheet.to.w.max(1) as f64, r.h as f64 / sheet.to.h.max(1) as f64);
        let at = phys(r.x, r.y, s);
        if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at.to_f64(), buf, None, None, None, EKind::Unspecified) {
            let scaled = RescaleRenderElement::from_element(e, at, Scale::from((kx, ky)));
            out.push(El::AnimMem(RelocateRenderElement::from_element(scaled, (0, 0), Relocate::Relative)));
        }
        let geo = phys_rect(r, s);
        out.push(El::Blur(BlurElement::new(sheet.ids.0[0].clone(), geo, geo, 6.0 * s as f32, 1.0)));
    }

    /// One window: client on top of its frame on top of the glass, moved and faded by its animation.
    fn window_elements(&self, renderer: &mut GlesRenderer, c: &Win, s: f64, oo: (i32, i32), out: &mut Vec<El>) {
        let fr = self.frame_rect(c);
        let now = Instant::now();
        // Aero Peek: the others fade to a glass outline
        let mut alpha = 1.0f32;
        let mut peek_sheet = false;
        if let Some((target, t)) = &self.peek {
            if *target != c.id && is_app(c) && c.transient_for != *target {
                alpha = t.at(now) as f32;
                peek_sheet = true;
            }
        }
        // where the animation has the frame now
        let (cur, a_factor) = match &c.anim {
            Some(a) => {
                let t = a.tween.at(now);
                let to = a.to.unwrap_or(fr);
                (anim::lerp_rect(a.from, to, t), a.a0 + (a.a1 - a.a0) * t as f32)
            }
            None => (fr, 1.0),
        };
        alpha *= a_factor;
        if peek_sheet {
            if let Some(buf) = self.peek_sheets.get(&c.id) {
                let at = phys(fr.x - oo.0, fr.y - oo.1, s).to_f64();
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, buf, Some(1.0 - alpha), None, None, EKind::Unspecified) { out.push(El::Mem(e)); }
                let geo = phys_rect(Rect { x: fr.x - oo.0, y: fr.y - oo.1, ..fr }, s);
                out.push(El::Blur(BlurElement::new(self.peek_id(c.id), geo, geo, 6.0 * s as f32, 1.0 - alpha)));
            }
            if alpha <= 0.01 { return; }
        }
        let scale = Scale::from(s);
        let surf_loc = phys(c.geom.x - oo.0, c.geom.y - oo.1, s) - c.window.geometry().loc.to_physical_precise_round(s);
        let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = AsRenderElements::<GlesRenderer>::render_elements(&c.window, renderer, surf_loc, scale, alpha);
        let (kx, ky) = (cur.w as f64 / fr.w.max(1) as f64, cur.h as f64 / fr.h.max(1) as f64);
        let origin = phys(fr.x - oo.0, fr.y - oo.1, s);
        let offset = phys(cur.x - fr.x, cur.y - fr.y, s);
        let animated = c.anim.is_some() && cur != fr;
        for e in elems {
            if animated {
                let scaled = RescaleRenderElement::from_element(e, origin, Scale::from((kx, ky)));
                out.push(El::AnimSurface(RelocateRenderElement::from_element(scaled, offset, Relocate::Relative)));
            } else { out.push(El::Surface(e)); }
        }
        if !c.framed || c.fullscreen { return; }
        let Some(cache) = &c.frame_cache else { return };
        for (buf, x, y) in &cache.strips {
            let at = phys(fr.x - oo.0 + x, fr.y - oo.1 + y, s).to_f64();
            let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, buf, Some(alpha), None, None, EKind::Unspecified) else { continue };
            if animated {
                let scaled = RescaleRenderElement::from_element(e, origin, Scale::from((kx, ky)));
                out.push(El::AnimMem(RelocateRenderElement::from_element(scaled, offset, Relocate::Relative)));
            } else { out.push(El::Mem(e)); }
        }
        if animated { return; }
        // the glass under the strips; a frame's client area is covered anyway and must not bleed in
        let (b, t) = (self.border_of(c), self.top_of(c));
        let shape = phys_rect(Rect { x: fr.x - oo.0, y: fr.y - oo.1, ..fr }, s);
        let radius = if c.maximized || c.kind == super::Kind::Utility { 0.0 } else { decor::RADIUS as f32 * s as f32 };
        let glass_alpha = alpha;
        let rects = [(0, 0, fr.w, t), (0, fr.h - b, fr.w, b), (0, t, b, fr.h - t - b), (fr.w - b, t, b, fr.h - t - b)];
        for (i, (x, y, w, h)) in rects.into_iter().enumerate() {
            if w <= 0 || h <= 0 { continue; }
            let geo = phys_rect(Rect { x: fr.x - oo.0 + x, y: fr.y - oo.1 + y, w, h }, s);
            out.push(El::Blur(BlurElement::new(c.glass_ids.0[i].clone(), geo, shape, radius, glass_alpha)));
        }
    }

    fn layer_elements(&self, renderer: &mut GlesRenderer, output: &Output, layers: &[Layer], s: f64, _oo: (i32, i32), only_lock: bool, out: &mut Vec<El>) {
        let map = layer_map_for_output(output);
        let scale = Scale::from(s);
        for &ly in layers {
            for l in map.layers_on(ly).rev() {
                let Some(g) = map.layer_geometry(l) else { continue };
                if only_lock && !self.lock_surface_allowed(l.wl_surface()) { continue; }
                let at = phys(g.loc.x, g.loc.y, s);
                let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = AsRenderElements::<GlesRenderer>::render_elements(l, renderer, at, scale, 1.0);
                out.extend(elems.into_iter().map(El::Surface));
                // the shell asks for glass by namespace: aerode-glass or aerode-glass-r<radius>
                if let Some(rest) = l.namespace().strip_prefix("aerode-glass") {
                    let (radius, top) = parse_glass(rest);
                    if g.size.h <= top || g.size.w <= 0 { continue; }
                    let geo = phys_rect(Rect { x: g.loc.x, y: g.loc.y + top, w: g.size.w, h: g.size.h - top }, s);
                    let id = l.user_data().get_or_insert(Id::new).clone();
                    out.push(El::Blur(BlurElement::new(id, geo, geo, radius * s as f32, 1.0)));
                }
            }
        }
    }

    fn cursor_elements(&mut self, renderer: &mut GlesRenderer, output: &Output, s: f64, oo: (i32, i32), out: &mut Vec<El>) {
        let pos = self.seat.get_pointer().unwrap().current_location();
        let (px, py) = (pos.x as i32 - oo.0, pos.y as i32 - oo.1);
        let Some(og) = self.space.output_geometry(output) else { return };
        if !og.contains((pos.x as i32, pos.y as i32)) { return; }
        let millis = self.start.elapsed().as_millis();
        // drag-and-drop icon under the pointer
        if let Some(icon) = &self.dnd_icon {
            let at = phys(px, py, s);
            let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(renderer, icon, at, Scale::from(s), 1.0, EKind::Unspecified);
            out.extend(elems.into_iter().map(El::Surface));
        }
        let name: Option<String> = match (&self.frame_cursor, &self.cursor_status) {
            (Some(n), _) => Some(n.to_string()),
            (None, CursorImageStatus::Hidden) => return,
            (None, CursorImageStatus::Named(i)) => Some(i.name().to_string()),
            (None, CursorImageStatus::Surface(_)) => None,
        };
        if let Some(n) = name {
            if let Some((buf, hot)) = self.cursors.get(&n, millis) {
                let at = phys(px - hot.0, py - hot.1, s).to_f64();
                if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, &buf, None, None, None, EKind::Cursor) { out.insert(0, El::Mem(e)); }
            }
        } else if let CursorImageStatus::Surface(surf) = &self.cursor_status {
            let hot = smithay::wayland::compositor::with_states(surf, |st| st.data_map.get::<smithay::input::pointer::CursorImageSurfaceData>().map(|a| a.lock().unwrap().hotspot)).unwrap_or_default();
            let at = phys(px - hot.x, py - hot.y, s);
            let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(renderer, surf, at, Scale::from(s), 1.0, EKind::Cursor);
            for e in elems.into_iter().rev() { out.insert(0, El::Surface(e)); }
        }
    }

    /// Who wants to hear when this frame is on screen (present-wait in Vulkan blocks on this).
    pub fn take_feedback(&self, output: &Output, states: &smithay::backend::renderer::element::RenderElementStates) -> smithay::desktop::utils::OutputPresentationFeedback {
        use smithay::desktop::utils::{surface_presentation_feedback_flags_from_states, OutputPresentationFeedback};
        let mut fb = OutputPresentationFeedback::new(output);
        let o = output.clone();
        for w in self.space.elements() {
            w.take_presentation_feedback(&mut fb, |_, _| Some(o.clone()), |s, _| surface_presentation_feedback_flags_from_states(s, states));
        }
        let map = layer_map_for_output(output);
        for l in map.layers() {
            l.take_presentation_feedback(&mut fb, |_, _| Some(o.clone()), |s, _| surface_presentation_feedback_flags_from_states(s, states));
        }
        fb
    }

    /// After a frame reached the screen: clients may draw the next one.
    pub fn send_frames(&mut self, output: &Output) {
        let time = self.start.elapsed();
        let o2 = output.clone();
        for w in self.space.elements() { w.send_frame(output, time, Some(std::time::Duration::ZERO), |_, _| Some(o2.clone())); }
        let map = layer_map_for_output(output);
        for l in map.layers() { l.send_frame(output, time, Some(std::time::Duration::ZERO), |_, _| Some(o2.clone())); }
        drop(map);
        if let CursorImageStatus::Surface(s) = &self.cursor_status {
            smithay::desktop::utils::send_frames_surface_tree(s, output, time, Some(std::time::Duration::ZERO), |_, _| Some(o2.clone()));
        }
        if let Some(s) = &self.dnd_icon {
            smithay::desktop::utils::send_frames_surface_tree(s, output, time, Some(std::time::Duration::ZERO), |_, _| Some(o2.clone()));
        }
    }

    pub fn redraw_outputs(&mut self) {
        match &self.backend {
            Backend::None => {}
            Backend::Winit(_) => super::winit::redraw(self),
            Backend::Udev(_) => super::udev::redraw(self),
        }
    }

    // ------------------------------------------------------------- pictures for the shell
    /// Render elements offscreen and write them to a file for the asking client.
    fn snapshot(&mut self, size: (i32, i32), elements: impl FnOnce(&mut Wm, &mut GlesRenderer) -> Vec<El>) -> Option<serde_json::Value> {
        let renderer_ptr: *mut GlesRenderer = self.with_renderer(|r| r as *mut GlesRenderer)?;
        // SAFETY: the renderer lives in self.backend and `elements` only reads window state, never the backend
        let renderer = unsafe { &mut *renderer_ptr };
        let elems = elements(self, renderer);
        let buf_size = Size::from(size);
        let mut tex: GlesTexture = Offscreen::create_buffer(renderer, Fourcc::Argb8888, buf_size).ok()?;
        let mut fb = renderer.bind(&mut tex).ok()?;
        let mut dt = OutputDamageTracker::new(size, 1.0, Transform::Normal);
        dt.render_output(renderer, &mut fb, 0, &elems, [0.0, 0.0, 0.0, 0.0]).ok()?;
        let map = renderer.copy_framebuffer(&fb, Rectangle::from_size(Size::<i32, smithay::utils::Buffer>::from(size)), Fourcc::Argb8888).ok()?;
        let bytes = renderer.map_texture(&map).ok()?.to_vec();
        let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
        self.snap_seq += 1;
        let path = std::path::PathBuf::from(dir).join(format!("aerode-snap-{}-{}", std::process::id(), self.snap_seq));
        // only ours to read
        use std::os::unix::fs::OpenOptionsExt;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path).ok()?;
        f.write_all(&bytes).ok()?;
        Some(serde_json::json!({"path": path, "w": size.0, "h": size.1}))
    }

    /// The whole virtual screen, as shown (cursor excluded).
    pub fn screenshot(&mut self) -> Option<serde_json::Value> {
        let size = self.screen_size();
        self.prepare_frames();
        self.snapshot(size, |wm, r| {
            let mut all = Vec::new();
            let saved = std::mem::replace(&mut wm.frame_cursor, None);
            let status = std::mem::replace(&mut wm.cursor_status, CursorImageStatus::Hidden);
            for o in wm.outputs() { all.extend(wm.elements(r, &o)); }
            wm.frame_cursor = saved;
            wm.cursor_status = status;
            all
        })
    }

    /// One window with its frame, fitted into w x h; minimised windows too (their last buffer).
    pub fn thumbnail(&mut self, id: u32, w: i32, h: i32) -> Option<serde_json::Value> {
        let c = self.wins.get(&id)?;
        let fr = self.frame_rect(c);
        let k = (w as f64 / fr.w.max(1) as f64).min(h as f64 / fr.h.max(1) as f64).min(1.0);
        let size = (((fr.w as f64 * k).round() as i32).max(1), ((fr.h as f64 * k).round() as i32).max(1));
        self.prepare_frames();
        self.snapshot(size, |wm, r| {
            let c = &wm.wins[&id];
            let mut raw = Vec::new();
            let saved_peek = wm.peek.take();
            wm.window_elements(r, c, 1.0, (fr.x, fr.y), &mut raw);
            wm.peek = saved_peek;
            let mut out = Vec::new();
            for e in raw {
                match e {
                    El::Surface(e) => out.push(El::AnimSurface(RelocateRenderElement::from_element(RescaleRenderElement::from_element(e, Point::from((0, 0)), Scale::from(k)), (0, 0), Relocate::Relative))),
                    El::Mem(e) => out.push(El::AnimMem(RelocateRenderElement::from_element(RescaleRenderElement::from_element(e, Point::from((0, 0)), Scale::from(k)), (0, 0), Relocate::Relative))),
                    _ => {}
                }
            }
            out
        })
    }
}

/// `-r<radius>` and `-t<top inset>` of a glass namespace.
fn parse_glass(rest: &str) -> (f32, i32) {
    let mut out = (0.0, 0);
    for part in rest.split('-') {
        if let Some(v) = part.strip_prefix('r').and_then(|n| n.parse().ok()) { out.0 = v; }
        if let Some(v) = part.strip_prefix('t').and_then(|n| n.parse().ok()) { out.1 = v; }
    }
    out
}

#[test]
fn glass_namespace() {
    assert_eq!(parse_glass("-r6-t40"), (6.0, 40));
    assert_eq!(parse_glass("-panel"), (0.0, 0));
    assert_eq!(parse_glass(""), (0.0, 0));
}

/// Clear what lies outside the frame's rounded rectangle.
fn cut_corners(cr: &cairo::Context, w: i32, h: i32, r: f64) {
    cr.save().ok();
    cr.set_operator(cairo::Operator::Clear);
    cr.set_fill_rule(cairo::FillRule::EvenOdd);
    cr.rectangle(0.0, 0.0, w as f64, h as f64);
    crate::glass::rounded(cr, 0.0, 0.0, w as f64, h as f64, r);
    cr.fill().ok();
    cr.restore().ok();
}

