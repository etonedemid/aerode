//! Real Aero glass: a render element that replaces what is behind it with a dual-Kawase blur of
//! the framebuffer. It always reports its whole area as damaged, so what lies under it is repainted
//! fresh each frame and never blurred twice.
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{ffi, GlesError, GlesFrame, GlesRenderer};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Frame;
use smithay::utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform};
use std::cell::RefCell;
use std::ffi::CString;

#[derive(Debug, Clone)]
pub struct BlurElement {
    pub id: Id,
    pub geo: Rectangle<i32, Physical>,
    /// the whole shape (a window's frame): rounded corners are cut from this
    pub shape: Rectangle<i32, Physical>,
    pub radius: f32,
    pub commit: CommitCounter,
    pub alpha: f32,
}

impl BlurElement {
    pub fn new(id: Id, geo: Rectangle<i32, Physical>, shape: Rectangle<i32, Physical>, radius: f32, alpha: f32) -> BlurElement {
        BlurElement { id, geo, shape, radius, commit: CommitCounter::default(), alpha }
    }
}

impl Element for BlurElement {
    fn id(&self) -> &Id { &self.id }
    fn current_commit(&self) -> CommitCounter { self.commit }
    fn src(&self) -> Rectangle<f64, BufferCoords> { Rectangle::from_size((self.geo.size.w as f64, self.geo.size.h as f64).into()) }
    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> { self.geo }
    fn damage_since(&self, _: Scale<f64>, _: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        DamageSet::from_slice(&[Rectangle::from_size(self.geo.size)])
    }
    fn opaque_regions(&self, _: Scale<f64>) -> OpaqueRegions<i32, Physical> { OpaqueRegions::default() }
    fn alpha(&self) -> f32 { 1.0 }
    fn kind(&self) -> Kind { Kind::Unspecified }
}

const VS: &str = r#"#version 100
attribute vec2 pos;
varying vec2 uv;
void main() { uv = pos * 0.5 + 0.5; gl_Position = vec4(pos, 0.0, 1.0); }
"#;

// Dual Kawase: `half` is half a texel of the source, uvmin/uvmax the used part of the texture.
const DOWN: &str = r#"#version 100
precision mediump float;
uniform sampler2D tex; uniform vec2 half_; uniform vec4 rect; varying vec2 uv;
vec4 at(vec2 p) { return texture2D(tex, clamp(rect.xy + p * rect.zw, rect.xy, rect.xy + rect.zw)); }
void main() {
  vec4 s = at(uv) * 4.0;
  s += at(uv - half_) + at(uv + half_) + at(uv + vec2(half_.x, -half_.y)) + at(uv - vec2(half_.x, -half_.y));
  gl_FragColor = s / 8.0;
}
"#;

const UP: &str = r#"#version 100
precision mediump float;
uniform sampler2D tex; uniform vec2 half_; uniform vec4 rect; varying vec2 uv;
vec4 at(vec2 p) { return texture2D(tex, clamp(rect.xy + p * rect.zw, rect.xy, rect.xy + rect.zw)); }
void main() {
  vec4 s = at(uv + vec2(-half_.x * 2.0, 0.0));
  s += at(uv + vec2(-half_.x, half_.y)) * 2.0;
  s += at(uv + vec2(0.0, half_.y * 2.0));
  s += at(uv + vec2(half_.x, half_.y)) * 2.0;
  s += at(uv + vec2(half_.x * 2.0, 0.0));
  s += at(uv + vec2(half_.x, -half_.y)) * 2.0;
  s += at(uv + vec2(0.0, -half_.y * 2.0));
  s += at(uv + vec2(-half_.x, -half_.y)) * 2.0;
  gl_FragColor = s / 12.0;
}
"#;

// the result, cut to the rounded shape (`shape` = x, y, w, h in framebuffer pixels, as gl_FragCoord)
const FINAL: &str = r#"#version 100
precision mediump float;
uniform sampler2D tex; uniform vec4 rect; uniform vec4 shape; uniform float radius; uniform float alpha; varying vec2 uv;
void main() {
  vec2 c = shape.xy + shape.zw * 0.5;
  vec2 d = abs(gl_FragCoord.xy - c) - (shape.zw * 0.5 - vec2(radius));
  float dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - radius;
  float a = clamp(0.5 - dist, 0.0, 1.0) * alpha;
  gl_FragColor = vec4(texture2D(tex, rect.xy + uv * rect.zw).rgb, 1.0) * a;
}
"#;

struct Prog { id: u32, tex: i32, half: i32, rect: i32, shape: i32, radius: i32, alpha: i32, pos: i32 }

struct Gl { down: Prog, up: Prog, fin: Prog, fbo: u32, tex: [u32; 3], size: (i32, i32) }

thread_local! { static GL: RefCell<Option<Gl>> = const { RefCell::new(None) }; }

const LEVELS: usize = 3;

unsafe fn compile(gl: &ffi::Gles2, kind: u32, src: &str) -> Result<u32, GlesError> {
    unsafe {
        let s = gl.CreateShader(kind);
        let c = CString::new(src).unwrap();
        gl.ShaderSource(s, 1, &c.as_ptr(), std::ptr::null());
        gl.CompileShader(s);
        let mut ok = 0;
        gl.GetShaderiv(s, ffi::COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut log = vec![0u8; 2048];
            let mut n = 0;
            gl.GetShaderInfoLog(s, 2048, &mut n, log.as_mut_ptr() as *mut _);
            eprintln!("aerode-wm: blur shader: {}", String::from_utf8_lossy(&log[..n.max(0) as usize]));
            return Err(GlesError::ShaderCompileError);
        }
        Ok(s)
    }
}

unsafe fn program(gl: &ffi::Gles2, frag: &str) -> Result<Prog, GlesError> {
    unsafe {
        let (v, f) = (compile(gl, ffi::VERTEX_SHADER, VS)?, compile(gl, ffi::FRAGMENT_SHADER, frag)?);
        let p = gl.CreateProgram();
        gl.AttachShader(p, v);
        gl.AttachShader(p, f);
        gl.LinkProgram(p);
        gl.DeleteShader(v);
        gl.DeleteShader(f);
        let mut ok = 0;
        gl.GetProgramiv(p, ffi::LINK_STATUS, &mut ok);
        if ok == 0 { return Err(GlesError::ProgramLinkError); }
        let u = |n: &str| { let c = CString::new(n).unwrap(); gl.GetUniformLocation(p, c.as_ptr()) };
        let pos = { let c = CString::new("pos").unwrap(); gl.GetAttribLocation(p, c.as_ptr()) };
        Ok(Prog { id: p, tex: u("tex"), half: u("half_"), rect: u("rect"), shape: u("shape"), radius: u("radius"), alpha: u("alpha"), pos })
    }
}

fn level_size(w: i32, h: i32, l: usize) -> (i32, i32) { ((w >> l).max(1), (h >> l).max(1)) }

unsafe fn ensure(gl: &ffi::Gles2, cell: &mut Option<Gl>, size: (i32, i32)) -> Result<(), GlesError> {
    unsafe {
        if cell.is_none() {
            let mut fbo = 0;
            gl.GenFramebuffers(1, &mut fbo);
            let mut tex = [0u32; LEVELS];
            gl.GenTextures(LEVELS as i32, tex.as_mut_ptr());
            *cell = Some(Gl { down: program(gl, DOWN)?, up: program(gl, UP)?, fin: program(gl, FINAL)?, fbo, tex, size: (0, 0) });
        }
        let g = cell.as_mut().unwrap();
        if g.size != size {
            for l in 0..LEVELS {
                let (w, h) = level_size(size.0, size.1, l);
                gl.BindTexture(ffi::TEXTURE_2D, g.tex[l]);
                gl.TexImage2D(ffi::TEXTURE_2D, 0, ffi::RGBA as i32, w, h, 0, ffi::RGBA, ffi::UNSIGNED_BYTE, std::ptr::null());
                for (k, v) in [(ffi::TEXTURE_MIN_FILTER, ffi::LINEAR), (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR), (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE), (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE)] {
                    gl.TexParameteri(ffi::TEXTURE_2D, k, v as i32);
                }
            }
            g.size = size;
        }
        Ok(())
    }
}

const QUAD: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];

unsafe fn quad(gl: &ffi::Gles2, p: &Prog) {
    unsafe {
        gl.BindBuffer(ffi::ARRAY_BUFFER, 0);
        gl.EnableVertexAttribArray(p.pos as u32);
        gl.VertexAttribPointer(p.pos as u32, 2, ffi::FLOAT, ffi::FALSE, 0, QUAD.as_ptr() as *const _);
        gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
        gl.DisableVertexAttribArray(p.pos as u32);
    }
}

impl RenderElement<GlesRenderer> for BlurElement {
    fn draw(&self, frame: &mut GlesFrame<'_, '_>, _src: Rectangle<f64, BufferCoords>, dst: Rectangle<i32, Physical>, damage: &[Rectangle<i32, Physical>], _opaque: &[Rectangle<i32, Physical>]) -> Result<(), GlesError> {
        let tf: Transform = frame.transformation();
        let (radius, alpha, shape_in) = (self.radius, self.alpha, self.shape);
        let damage: Vec<Rectangle<i32, Physical>> = damage.to_vec();
        frame.with_context(move |gl| unsafe {
            let (mut old_fbo, mut vp, mut active) = (0, [0; 4], 0);
            gl.GetIntegerv(ffi::FRAMEBUFFER_BINDING, &mut old_fbo);
            gl.GetIntegerv(ffi::VIEWPORT, vp.as_mut_ptr());
            gl.GetIntegerv(ffi::ACTIVE_TEXTURE, &mut active);
            // the output plane -> GL framebuffer coordinates (smithay's viewport is the whole framebuffer)
            let fb_size: Size<i32, Physical> = Size::from((vp[2], vp[3]));
            let out = tf.invert().transform_size(fb_size);
            let gl_rect = |r: Rectangle<i32, Physical>| -> Rectangle<i32, Physical> { tf.transform_rect_in(r, &out) };
            let (fb, shape) = (gl_rect(dst), gl_rect(shape_in));
            let scissors: Vec<Rectangle<i32, Physical>> = damage.iter().map(|d| {
                gl_rect(Rectangle::new(Point::from((d.loc.x + dst.loc.x, d.loc.y + dst.loc.y)), d.size))
            }).collect();
            gl.ActiveTexture(ffi::TEXTURE0);
            gl.Disable(ffi::SCISSOR_TEST);
            gl.Disable(ffi::BLEND);
            let size = (fb_size.w, fb_size.h);
            GL.with(|cell| -> Result<(), GlesError> {
                let mut cell = cell.borrow_mut();
                ensure(gl, &mut cell, size)?;
                let g = cell.as_ref().unwrap();
                // what is behind us
                gl.BindTexture(ffi::TEXTURE_2D, g.tex[0]);
                let (x0, y0) = (fb.loc.x.clamp(0, size.0), fb.loc.y.clamp(0, size.1));
                let (cw, ch) = ((fb.size.w).min(size.0 - x0), (fb.size.h).min(size.1 - y0));
                if cw <= 0 || ch <= 0 { return Ok(()); }
                gl.CopyTexSubImage2D(ffi::TEXTURE_2D, 0, 0, 0, x0, y0, cw, ch);
                gl.BindFramebuffer(ffi::FRAMEBUFFER, g.fbo);
                // rect of level l in its texture's uv
                let sz = |l: usize| { let t = level_size(size.0, size.1, l); ((cw >> l).max(1), (ch >> l).max(1), t.0, t.1) };
                let uv = |l: usize| { let (w, h, tw, th) = sz(l); [0.0f32, 0.0, w as f32 / tw as f32, h as f32 / th as f32] };
                let pass = |prog: &Prog, from: usize, to: usize| {
                    let (w, h, _, _) = sz(to);
                    let (sw, sh, _, _) = sz(from);
                    gl.FramebufferTexture2D(ffi::FRAMEBUFFER, ffi::COLOR_ATTACHMENT0, ffi::TEXTURE_2D, g.tex[to], 0);
                    gl.Viewport(0, 0, w, h);
                    gl.UseProgram(prog.id);
                    gl.BindTexture(ffi::TEXTURE_2D, g.tex[from]);
                    gl.Uniform1i(prog.tex, 0);
                    gl.Uniform2f(prog.half, 0.5 / sw as f32, 0.5 / sh as f32);
                    let r = uv(from);
                    gl.Uniform4f(prog.rect, r[0], r[1], r[2], r[3]);
                    quad(gl, prog);
                };
                for l in 0..LEVELS - 1 { pass(&g.down, l, l + 1); }
                for l in (0..LEVELS - 1).rev() { pass(&g.up, l + 1, l); }
                // back to the screen, only where damaged
                gl.BindFramebuffer(ffi::FRAMEBUFFER, old_fbo as u32);
                gl.Viewport(fb.loc.x, fb.loc.y, fb.size.w, fb.size.h);
                let p = &g.fin;
                gl.UseProgram(p.id);
                gl.BindTexture(ffi::TEXTURE_2D, g.tex[0]);
                gl.Uniform1i(p.tex, 0);
                let r = uv(0);
                gl.Uniform4f(p.rect, r[0], r[1], r[2], r[3]);
                gl.Uniform4f(p.shape, shape.loc.x as f32, shape.loc.y as f32, shape.size.w as f32, shape.size.h as f32);
                gl.Uniform1f(p.radius, radius);
                gl.Uniform1f(p.alpha, alpha);
                gl.Enable(ffi::BLEND);
                gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
                gl.Enable(ffi::SCISSOR_TEST);
                for s in &scissors {
                    gl.Scissor(s.loc.x, s.loc.y, s.size.w, s.size.h);
                    quad(gl, p);
                }
                Ok(())
            })?;
            gl.Disable(ffi::SCISSOR_TEST);
            gl.Enable(ffi::BLEND);
            gl.BlendFunc(ffi::ONE, ffi::ONE_MINUS_SRC_ALPHA);
            gl.BindFramebuffer(ffi::FRAMEBUFFER, old_fbo as u32);
            gl.Viewport(vp[0], vp[1], vp[2], vp[3]);
            gl.ActiveTexture(active as u32);
            gl.UseProgram(0);
            Ok::<(), GlesError>(())
        })?
    }
    fn underlying_storage(&self, _: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> { None }
}

