//! The compositor's own pointer images from the XCursor theme (frames, drags, CursorShape clients).
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::Transform;
use std::collections::HashMap;
use xcursor::parser::{parse_xcursor, Image};
use xcursor::CursorTheme;

struct Frame { buf: MemoryRenderBuffer, hot: (i32, i32), delay: u32 }

pub struct Cursors {
    theme: CursorTheme,
    size: u32,
    loaded: HashMap<String, Vec<Frame>>,
}

impl Cursors {
    pub fn load() -> Cursors {
        let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".into());
        let size = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(24);
        Cursors { theme: CursorTheme::load(&name), size, loaded: HashMap::new() }
    }

    pub fn set_theme(&mut self, name: &str, size: u32) {
        self.theme = CursorTheme::load(name);
        self.size = size;
        self.loaded.clear();
    }

    /// The first frame as RGBA bytes, for XWayland's default cursor: (pixels, size, hotspot).
    pub fn rgba(&self, name: &str) -> Option<(Vec<u8>, (u32, u32), (u32, u32))> {
        let images = parse_xcursor(&std::fs::read(self.theme.load_icon(name)?).ok()?)?;
        let i = images.iter().min_by_key(|i| (self.size as i32 - i.size as i32).abs())?;
        Some((i.pixels_rgba.clone(), (i.width, i.height), (i.xhot, i.yhot)))
    }

    /// The picture of `name` at this moment: (buffer, hotspot).
    pub fn get(&mut self, name: &str, millis: u128) -> Option<(MemoryRenderBuffer, (i32, i32))> {
        if !self.loaded.contains_key(name) {
            let frames = self.read(name).or_else(|| self.read("left_ptr")).or_else(|| self.read("default")).unwrap_or_else(fallback);
            self.loaded.insert(name.to_string(), frames);
        }
        let frames = self.loaded.get(name)?;
        let total: u32 = frames.iter().map(|f| f.delay).sum();
        let mut t = if total > 0 { (millis % total as u128) as u32 } else { 0 };
        for f in frames {
            if t < f.delay || total == 0 { return Some((f.buf.clone(), f.hot)); }
            t -= f.delay;
        }
        frames.first().map(|f| (f.buf.clone(), f.hot))
    }

    fn read(&self, name: &str) -> Option<Vec<Frame>> {
        let path = self.theme.load_icon(name)?;
        let images = parse_xcursor(&std::fs::read(path).ok()?)?;
        let nearest = images.iter().min_by_key(|i| (self.size as i32 - i.size as i32).abs())?;
        let (nw, nh) = (nearest.width, nearest.height);
        let frames: Vec<Frame> = images.iter().filter(|i| i.width == nw && i.height == nh).map(frame).collect();
        (!frames.is_empty()).then_some(frames)
    }
}

fn frame(i: &Image) -> Frame {
    Frame {
        // pixels_rgba is already byte-order R,G,B,A
        buf: MemoryRenderBuffer::from_slice(&i.pixels_rgba, Fourcc::Abgr8888, (i.width as i32, i.height as i32), 1, Transform::Normal, None),
        hot: (i.xhot as i32, i.yhot as i32),
        delay: i.delay,
    }
}

/// No cursor theme at all: a plain arrow, so the pointer is never invisible.
fn fallback() -> Vec<Frame> {
    const W: usize = 12;
    const H: usize = 19;
    let mut px = vec![0u8; W * H * 4];
    for y in 0..H { for x in 0..W {
        let inside = x <= y && x < W && y < 16 && (y < 12 || x < 5);
        if inside {
            let edge = x == 0 || x == y || y == 15;
            let v = if edge { 0 } else { 255 };
            px[(y * W + x) * 4..(y * W + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }}
    vec![Frame { buf: MemoryRenderBuffer::from_slice(&px, Fourcc::Abgr8888, (W as i32, H as i32), 1, Transform::Normal, None), hot: (0, 0), delay: 0 }]
}
