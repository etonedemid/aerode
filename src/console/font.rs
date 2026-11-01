//! Windows raster font (.FON = NE executable with RT_FONT/FNT resources) reader,
//! used to draw the console exactly like Win7's "Raster Fonts".
use std::path::Path;

#[derive(Clone)]
pub struct RasterFont {
    pub width: u32,
    pub height: u32,
    pub ascent: u32,
    pub first: u8,
    pub last: u8,
    pub default_char: u8,
    /// glyph bitmaps, one bool per pixel row-major, indexed by OEM code
    glyphs: Vec<Vec<bool>>,
}

fn u16le(d: &[u8], o: usize) -> u16 { u16::from_le_bytes([d[o], d[o + 1]]) }
fn u32le(d: &[u8], o: usize) -> u32 { u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) }

/// Parse one FNT resource (version 2 or 3, fixed pitch).
pub fn parse_fnt(d: &[u8]) -> Option<RasterFont> {
    if d.len() < 0x76 { return None; }
    let ver = u16le(d, 0);
    if ver != 0x200 && ver != 0x300 { return None; }
    if u16le(d, 0x42) & 1 != 0 { return None; } // vector font
    let pix_w = u16le(d, 0x56) as u32;
    let height = u16le(d, 0x58) as u32;
    let ascent = u16le(d, 0x4A) as u32;
    let first = d[0x5F];
    let last = d[0x60];
    let default_char = first.wrapping_add(d[0x61]);
    let table = if ver == 0x300 { 0x94 } else { 0x76 };
    let entry = if ver == 0x300 { 6 } else { 4 };
    let mut glyphs = vec![vec![]; 256];
    let mut width = pix_w;
    for (i, ch) in (first..=last).enumerate() {
        let e = table + i * entry;
        if e + entry > d.len() { break; }
        let w = u16le(d, e) as u32;
        let off = if ver == 0x300 { u32le(d, e + 2) as usize } else { u16le(d, e + 2) as usize };
        if width == 0 { width = w; }
        let mut px = vec![false; (w * height) as usize];
        let cols = w.div_ceil(8) as usize;
        for c in 0..cols {
            for y in 0..height as usize {
                let Some(&byte) = d.get(off + c * height as usize + y) else { continue };
                for bit in 0..8 {
                    let x = c * 8 + bit;
                    if x < w as usize && byte & (0x80 >> bit) != 0 { px[y * w as usize + x] = true; }
                }
            }
        }
        glyphs[ch as usize] = px;
    }
    Some(RasterFont { width, height, ascent, first, last, default_char, glyphs })
}

/// All fixed-pitch FNT fonts inside a .FON file.
pub fn load_fon(path: &Path) -> Vec<RasterFont> {
    let Ok(d) = std::fs::read(path) else { return vec![] };
    if d.len() < 0x40 || &d[0..2] != b"MZ" { return parse_fnt(&d).into_iter().collect(); }
    let ne = u32le(&d, 0x3C) as usize;
    if d.get(ne..ne + 2) != Some(b"NE") { return vec![]; }
    let rt = ne + u16le(&d, ne + 0x24) as usize;
    let shift = u16le(&d, rt) as u32;
    let mut p = rt + 2;
    let mut out = vec![];
    while p + 8 <= d.len() {
        let ty = u16le(&d, p);
        if ty == 0 { break; }
        let count = u16le(&d, p + 2) as usize;
        p += 8;
        for i in 0..count {
            let r = p + i * 12;
            if r + 12 > d.len() { break; }
            if ty == 0x8008 {
                let off = (u16le(&d, r) as usize) << shift;
                let len = (u16le(&d, r + 2) as usize) << shift;
                if let Some(f) = d.get(off..(off + len).min(d.len())).and_then(parse_fnt) { out.push(f); }
            }
        }
        p += count * 12;
    }
    out
}

impl RasterFont {
    /// Pixel lookup for an OEM (code page 437) character.
    pub fn glyph(&self, oem: u8) -> &[bool] {
        let g = &self.glyphs[oem as usize];
        if g.is_empty() { &self.glyphs[self.default_char as usize] } else { g }
    }
}

/// Unicode → code page 437 (the console's OEM code page in the US).
pub fn to_cp437(c: char) -> u8 {
    let u = c as u32;
    if (0x20..0x7f).contains(&u) { return u as u8; }
    const HI: [u16; 128] = [
        0xC7, 0xFC, 0xE9, 0xE2, 0xE4, 0xE0, 0xE5, 0xE7, 0xEA, 0xEB, 0xE8, 0xEF, 0xEE, 0xEC, 0xC4, 0xC5,
        0xC9, 0xE6, 0xC6, 0xF4, 0xF6, 0xF2, 0xFB, 0xF9, 0xFF, 0xD6, 0xDC, 0xA2, 0xA3, 0xA5, 0x20A7, 0x192,
        0xE1, 0xED, 0xF3, 0xFA, 0xF1, 0xD1, 0xAA, 0xBA, 0xBF, 0x2310, 0xAC, 0xBD, 0xBC, 0xA1, 0xAB, 0xBB,
        0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557, 0x255D, 0x255C, 0x255B, 0x2510,
        0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F, 0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567,
        0x2568, 0x2564, 0x2565, 0x2559, 0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
        0x3B1, 0xDF, 0x393, 0x3C0, 0x3A3, 0x3C3, 0xB5, 0x3C4, 0x3A6, 0x398, 0x3A9, 0x3B4, 0x221E, 0x3C6, 0x3B5, 0x2229,
        0x2261, 0xB1, 0x2265, 0x2264, 0x2320, 0x2321, 0xF7, 0x2248, 0xB0, 0x2219, 0xB7, 0x221A, 0x207F, 0xB2, 0x25A0, 0xA0,
    ];
    if let Some(i) = HI.iter().position(|v| *v as u32 == u) { return 0x80 + i as u8; }
    // a few common substitutions
    match c {
        '\u{2018}' | '\u{2019}' => b'\'', '\u{201C}' | '\u{201D}' => b'"', '\u{2013}' | '\u{2014}' => b'-',
        '\u{2026}' => b'.', '\u{25CF}' | '\u{2022}' => 0x07, '\u{263A}' => 0x01, '\u{2192}' => 0x1A, '\u{2190}' => 0x1B,
        '\u{2191}' => 0x18, '\u{2193}' => 0x19, '\u{2665}' => 0x03, '\u{2666}' => 0x04, '\u{2663}' => 0x05, '\u{2660}' => 0x06,
        _ => b'?',
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cp437_box() { assert_eq!(super::to_cp437('│'), 0xB3); assert_eq!(super::to_cp437('é'), 0x82); assert_eq!(super::to_cp437('A'), 65); }
}
