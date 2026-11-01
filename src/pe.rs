//! Tiny PE resource reader + DIB/ICO/CUR decoders (no external tools needed).
use anyhow::{bail, Context, Result};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResId {
    Id(u16),
    Name(String),
}

impl From<u16> for ResId {
    fn from(v: u16) -> Self { ResId::Id(v) }
}
impl From<&str> for ResId {
    fn from(v: &str) -> Self { ResId::Name(v.to_uppercase()) }
}

pub const RT_CURSOR: u16 = 1;
pub const RT_BITMAP: u16 = 2;
pub const RT_ICON: u16 = 3;
pub const RT_GROUP_ICON: u16 = 14;

pub struct Pe {
    data: Vec<u8>,
    rsrc_rva: u32,
    rsrc_off: usize,
    sections: Vec<(u32, u32, u32)>, // va, size, raw offset
}

fn u16le(d: &[u8], o: usize) -> u16 { u16::from_le_bytes([d[o], d[o + 1]]) }
fn u32le(d: &[u8], o: usize) -> u32 { u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) }
fn i32le(d: &[u8], o: usize) -> i32 { u32le(d, o) as i32 }

impl Pe {
    pub fn open(path: &std::path::Path) -> Result<Pe> {
        Pe::parse(std::fs::read(path).with_context(|| format!("reading {}", path.display()))?)
    }

    pub fn parse(data: Vec<u8>) -> Result<Pe> {
        if data.len() < 0x40 || &data[0..2] != b"MZ" { bail!("not a PE file"); }
        let pe = u32le(&data, 0x3c) as usize;
        if data.get(pe..pe + 4) != Some(b"PE\0\0") { bail!("bad PE signature"); }
        let nsec = u16le(&data, pe + 6) as usize;
        let opt_size = u16le(&data, pe + 20) as usize;
        let opt = pe + 24;
        let magic = u16le(&data, opt);
        let dd = opt + if magic == 0x20b { 112 } else { 96 };
        let rsrc_rva = u32le(&data, dd + 2 * 8);
        let sec = opt + opt_size;
        let mut sections = vec![];
        for i in 0..nsec {
            let s = sec + i * 40;
            sections.push((u32le(&data, s + 12), u32le(&data, s + 8).max(u32le(&data, s + 16)), u32le(&data, s + 20)));
        }
        let mut p = Pe { data, rsrc_rva, rsrc_off: 0, sections };
        p.rsrc_off = p.rva_to_off(rsrc_rva).context("no resource section")?;
        Ok(p)
    }

    fn rva_to_off(&self, rva: u32) -> Option<usize> {
        self.sections.iter().find(|(va, sz, _)| rva >= *va && rva < va + sz).map(|(va, _, raw)| (rva - va + raw) as usize)
    }

    fn dir_entries(&self, dir_off: usize) -> Vec<(ResId, u32)> {
        let d = &self.data;
        if dir_off + 16 > d.len() { return vec![]; }
        let n = u16le(d, dir_off + 12) as usize + u16le(d, dir_off + 14) as usize;
        (0..n).filter_map(|i| {
            let e = dir_off + 16 + i * 8;
            if e + 8 > d.len() { return None; }
            let name = u32le(d, e);
            let id = if name & 0x8000_0000 != 0 {
                let so = self.rsrc_off + (name & 0x7fff_ffff) as usize;
                let len = u16le(d, so) as usize;
                let s: Vec<u16> = (0..len).map(|k| u16le(d, so + 2 + k * 2)).collect();
                ResId::Name(String::from_utf16_lossy(&s).to_uppercase())
            } else {
                ResId::Id(name as u16)
            };
            Some((id, u32le(d, e + 4)))
        }).collect()
    }

    fn subdir(&self, off: u32) -> Option<usize> {
        (off & 0x8000_0000 != 0).then(|| self.rsrc_off + (off & 0x7fff_ffff) as usize)
    }

    /// All resource names of a type.
    pub fn names(&self, ty: impl Into<ResId>) -> Vec<ResId> {
        let ty = ty.into();
        let Some((_, o)) = self.dir_entries(self.rsrc_off).into_iter().find(|(i, _)| *i == ty) else { return vec![] };
        self.subdir(o).map(|s| self.dir_entries(s).into_iter().map(|(i, _)| i).collect()).unwrap_or_default()
    }

    pub fn types(&self) -> Vec<ResId> {
        self.dir_entries(self.rsrc_off).into_iter().map(|(i, _)| i).collect()
    }

    /// Raw bytes of resource (type, name), first language.
    pub fn get(&self, ty: impl Into<ResId>, name: impl Into<ResId>) -> Option<&[u8]> {
        let (ty, name) = (ty.into(), name.into());
        let (_, o) = self.dir_entries(self.rsrc_off).into_iter().find(|(i, _)| *i == ty)?;
        let (_, o) = self.dir_entries(self.subdir(o)?).into_iter().find(|(i, _)| *i == name)?;
        let (_, o) = self.dir_entries(self.subdir(o)?).into_iter().next()?;
        let leaf = self.rsrc_off + o as usize;
        let rva = u32le(&self.data, leaf);
        let size = u32le(&self.data, leaf + 4) as usize;
        let off = self.rva_to_off(rva)?;
        self.data.get(off..off + size)
    }

    /// Images of an RT_GROUP_ICON (or first group if `name` is None), as RGBA.
    pub fn group_icon(&self, name: Option<ResId>) -> Vec<image::RgbaImage> {
        let name = match name.or_else(|| self.names(RT_GROUP_ICON).into_iter().next()) { Some(n) => n, None => return vec![] };
        let Some(g) = self.get(RT_GROUP_ICON, name) else { return vec![] };
        if g.len() < 6 { return vec![]; }
        let n = u16le(g, 4) as usize;
        let mut out = vec![];
        for i in 0..n {
            let e = 6 + i * 14;
            if e + 14 > g.len() { break; }
            let id = u16le(g, e + 12);
            if let Some(img) = self.get(RT_ICON, id).and_then(|d| decode_icon_image(d).ok()) {
                out.push(img);
            }
        }
        out
    }
}

/// Decode an icon/cursor image entry: PNG, or DIB with XOR+AND masks.
pub fn decode_icon_image(d: &[u8]) -> Result<image::RgbaImage> {
    if d.starts_with(b"\x89PNG") {
        return Ok(image::load_from_memory(d)?.to_rgba8());
    }
    decode_dib(d, true)
}

/// Decode a DIB (BITMAPINFOHEADER + palette + bits). `icon` = height is doubled & has AND mask.
pub fn decode_dib(d: &[u8], icon: bool) -> Result<image::RgbaImage> {
    if d.len() < 40 { bail!("short DIB"); }
    let hsize = u32le(d, 0) as usize;
    let w = i32le(d, 4);
    let mut h = i32le(d, 8);
    let bpp = u16le(d, 14) as usize;
    let comp = u32le(d, 16);
    let mut ncol = u32le(d, 32) as usize;
    if icon { h /= 2; }
    let top_down = h < 0;
    let (w, h) = (w.unsigned_abs() as usize, h.unsigned_abs() as usize);
    if w == 0 || h == 0 || w > 4096 || h > 4096 { bail!("bad DIB size"); }
    if ncol == 0 && bpp <= 8 { ncol = 1 << bpp; }
    let mut pal_off = hsize;
    if comp == 3 && hsize == 40 { pal_off += 12; }
    let pal: Vec<[u8; 4]> = (0..ncol).filter_map(|i| d.get(pal_off + i * 4..pal_off + i * 4 + 4).map(|c| [c[2], c[1], c[0], 255])).collect();
    let bits = pal_off + ncol * 4;
    let stride = (w * bpp).div_ceil(32) * 4;
    let mask_off = bits + stride * h;
    let mstride = w.div_ceil(32) * 4;
    let mut img = image::RgbaImage::new(w as u32, h as u32);
    let mut any_alpha = false;
    for y in 0..h {
        let row = if top_down { y } else { h - 1 - y };
        let ro = bits + row * stride;
        for x in 0..w {
            let px = match bpp {
                32 => { let o = ro + x * 4; let a = *d.get(o + 3).unwrap_or(&0); if a != 0 { any_alpha = true; } [d[o + 2], d[o + 1], d[o], a] }
                24 => { let o = ro + x * 3; [d[o + 2], d[o + 1], d[o], 255] }
                8 => *pal.get(d[ro + x] as usize).unwrap_or(&[0, 0, 0, 255]),
                4 => { let b = d[ro + x / 2]; *pal.get(if x % 2 == 0 { b >> 4 } else { b & 15 } as usize).unwrap_or(&[0, 0, 0, 255]) }
                1 => { let b = d[ro + x / 8]; *pal.get(((b >> (7 - x % 8)) & 1) as usize).unwrap_or(&[0, 0, 0, 255]) }
                _ => bail!("unsupported bpp {bpp}"),
            };
            img.put_pixel(x as u32, y as u32, image::Rgba(px));
        }
    }
    // 32bpp without alpha, or <32bpp: use the AND mask
    if icon && (bpp < 32 || !any_alpha) {
        for y in 0..h {
            let row = if top_down { y } else { h - 1 - y };
            for x in 0..w {
                let b = d.get(mask_off + row * mstride + x / 8).copied().unwrap_or(0);
                let transparent = (b >> (7 - x % 8)) & 1 == 1;
                img.get_pixel_mut(x as u32, y as u32).0[3] = if transparent { 0 } else { 255 };
            }
        }
    } else if !icon && bpp == 32 && !any_alpha {
        for p in img.pixels_mut() { p.0[3] = 255; }
    }
    Ok(img)
}

/// Parse a .ico/.cur file: Vec<(image, hotspot_x, hotspot_y)>
pub fn decode_ico_file(d: &[u8]) -> Vec<(image::RgbaImage, u16, u16)> {
    if d.len() < 6 { return vec![]; }
    let n = u16le(d, 4) as usize;
    let mut v = vec![];
    for i in 0..n {
        let e = 6 + i * 16;
        if e + 16 > d.len() { break; }
        let (hx, hy) = (u16le(d, e + 4), u16le(d, e + 6));
        let size = u32le(d, e + 8) as usize;
        let off = u32le(d, e + 12) as usize;
        if let Some(Ok(img)) = d.get(off..off + size).map(decode_icon_image) {
            v.push((img, hx, hy));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dib_32bpp_icon_roundtrip() {
        // 2x2 32bpp icon, bottom-up, with AND mask
        let mut d = vec![0u8; 40];
        d[0] = 40; d[4] = 2; d[8] = 4; d[12] = 1; d[14] = 32;
        // bottom row then top row (BGRA)
        d.extend_from_slice(&[0, 0, 255, 255, 0, 255, 0, 255]); // bottom: red, green
        d.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]); // top: blue, transparent
        d.extend_from_slice(&[0; 8]); // AND mask
        let img = decode_dib(&d, true).unwrap();
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(img.get_pixel(1, 0).0[3], 0);
        assert_eq!(img.get_pixel(0, 1).0, [255, 0, 0, 255]);
    }
}
