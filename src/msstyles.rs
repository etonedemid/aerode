//! Reader for Windows 7 `aero.msstyles` (extracted locally from the user's ISO).
//!
//! VARIANT/NORMAL is a flat list of property records:
//!   u32 name_id, u32 type_id, u32 class_id, i32 part, i32 state,
//!   u32 short_flag, u32 reserved, u32 size, then `size` bytes padded to 8.
//! When short_flag != 0 the value is stored inline in that field and size is 0.
//! class_id indexes the UTF-16 class-name list in CMAP.  Image properties
//! (FILENAME type) hold the id of an IMAGE resource (a PNG).
use std::collections::HashMap;
use std::path::Path;

use crate::pe::Pe;

// type ids (tmschema.h)
pub const T_ENUM: u32 = 200;
pub const T_STRING: u32 = 201;
pub const T_INT: u32 = 202;
pub const T_BOOL: u32 = 203;
pub const T_COLOR: u32 = 204;
pub const T_MARGINS: u32 = 205;
pub const T_FILENAME: u32 = 206;
pub const T_SIZE: u32 = 207;
pub const T_POSITION: u32 = 208;
pub const T_RECT: u32 = 209;
pub const T_FONT: u32 = 210;
pub const T_INTLIST: u32 = 211;

// property ids
pub const IMAGECOUNT: u32 = 2401;
pub const IMAGEFILE: u32 = 3001;
pub const IMAGEFILE1: u32 = 3002;
pub const GLYPHIMAGEFILE: u32 = 3009;
pub const SIZINGMARGINS: u32 = 3601;
pub const CONTENTMARGINS: u32 = 3602;
pub const BORDERCOLOR: u32 = 3801;
pub const FILLCOLOR: u32 = 3802;
pub const TEXTCOLOR: u32 = 3803;
pub const HOTTRACKING: u32 = 1627; // TMT_HOTTRACKING system colour
pub const BGTYPE: u32 = 4001;
pub const SIZINGTYPE: u32 = 4004;
pub const IMAGELAYOUT: u32 = 4011; // TMT_IMAGELAYOUT (4008 is TMT_OFFSETTYPE)
pub const ATLASIMAGE: u32 = 8000;
pub const ATLASRECT: u32 = 8002;

#[derive(Clone, Debug)]
pub enum Value {
    Int(i32),
    Bool(bool),
    Color([u8; 3]),
    Margins([i32; 4]), // left, right, top, bottom
    Rect([i32; 4]),    // left, top, right, bottom
    Pos([i32; 2]),
    Str(String),
    Image(u16),
    IntList(Vec<i32>),
    Raw(u32, Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Prop {
    pub name: u32,
    pub ty: u32,
    pub class: String,
    pub part: i32,
    pub state: i32,
    pub value: Value,
}

pub struct Theme {
    pe: Pe,
    pub props: Vec<Prop>,
    index: HashMap<(String, i32, i32, u32), usize>,
    images: std::cell::RefCell<HashMap<u16, Option<image::RgbaImage>>>,
}

fn rd(d: &[u8], o: usize) -> u32 { u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) }

impl Theme {
    pub fn open(path: &Path) -> anyhow::Result<Theme> {
        let pe = Pe::open(path)?;
        let cmap = pe.get("CMAP", "CMAP").ok_or_else(|| anyhow::anyhow!("no CMAP"))?;
        let u: Vec<u16> = cmap.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let classes: Vec<String> = u.split(|c| *c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
        let v = pe.get("VARIANT", "NORMAL").ok_or_else(|| anyhow::anyhow!("no VARIANT"))?.to_vec();
        let mut props = vec![];
        let mut o = 0;
        while o + 32 <= v.len() {
            let (name, ty, cls) = (rd(&v, o), rd(&v, o + 4), rd(&v, o + 8) as usize);
            let (part, state) = (rd(&v, o + 12) as i32, rd(&v, o + 16) as i32);
            let short = rd(&v, o + 20);
            // inline (short) values carry no body even though `size` is set
            let size = if short != 0 { 0 } else { rd(&v, o + 28) as usize };
            let body = o + 32;
            if body + size > v.len() { break; }
            let d = &v[body..body + size];
            let i = |k: usize| if d.len() >= k * 4 + 4 { rd(d, k * 4) as i32 } else { short as i32 };
            let value = match ty {
                T_INT | T_ENUM | T_SIZE => Value::Int(i(0)),
                T_BOOL => Value::Bool(i(0) != 0),
                T_COLOR => { let c = i(0) as u32; Value::Color([c as u8, (c >> 8) as u8, (c >> 16) as u8]) }
                T_MARGINS => Value::Margins([i(0), i(1), i(2), i(3)]),
                T_RECT => Value::Rect([i(0), i(1), i(2), i(3)]),
                T_POSITION => Value::Pos([i(0), i(1)]),
                T_FILENAME => Value::Image(i(0) as u16),
                T_STRING => Value::Str(String::from_utf16_lossy(&d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect::<Vec<_>>())),
                T_INTLIST => { let n = i(0).max(0) as usize; Value::IntList((1..=n).map(i).collect()) }
                _ => Value::Raw(ty, d.to_vec()),
            };
            props.push(Prop { name, ty, class: classes.get(cls).cloned().unwrap_or_default(), part, state, value });
            o = body + size.div_ceil(8) * 8;
        }
        let mut index = HashMap::new();
        for (k, p) in props.iter().enumerate() {
            index.entry((p.class.to_lowercase(), p.part, p.state, p.name)).or_insert(k);
        }
        Ok(Theme { pe, props, index, images: Default::default() })
    }

    /// Look up a property with Windows' fallback order: state → part default → class default.
    pub fn get(&self, class: &str, part: i32, state: i32, name: u32) -> Option<&Value> {
        let c = class.to_lowercase();
        for (p, s) in [(part, state), (part, 0), (0, 0)] {
            if let Some(i) = self.index.get(&(c.clone(), p, s, name)) {
                return Some(&self.props[*i].value);
            }
        }
        // "App::Class" falls back to the plain class
        c.split_once("::").and_then(|(_, base)| self.get(base, part, state, name))
    }

    pub fn int(&self, class: &str, part: i32, state: i32, name: u32) -> Option<i32> {
        match self.get(class, part, state, name)? { Value::Int(v) => Some(*v), Value::Bool(b) => Some(*b as i32), _ => None }
    }
    pub fn margins(&self, class: &str, part: i32, state: i32, name: u32) -> Option<[i32; 4]> {
        match self.get(class, part, state, name)? { Value::Margins(m) => Some(*m), _ => None }
    }
    pub fn rect(&self, class: &str, part: i32, state: i32, name: u32) -> Option<[i32; 4]> {
        match self.get(class, part, state, name)? { Value::Rect(m) => Some(*m), _ => None }
    }
    pub fn color(&self, class: &str, part: i32, state: i32, name: u32) -> Option<[u8; 3]> {
        match self.get(class, part, state, name)? { Value::Color(c) => Some(*c), _ => None }
    }

    pub fn image_res(&self, id: u16) -> Option<image::RgbaImage> {
        self.images.borrow_mut().entry(id).or_insert_with(|| {
            self.pe.get("IMAGE", id).and_then(|d| image::load_from_memory(d).ok()).map(|i| i.to_rgba8())
        }).clone()
    }

    pub fn stream(&self, id: u16) -> Option<image::RgbaImage> {
        self.pe.get("STREAM", id).and_then(|d| image::load_from_memory(d).ok()).map(|i| i.to_rgba8())
    }

    /// The image strip for (class, part, state) plus the frame index for `state`.
    pub fn part_image(&self, class: &str, part: i32, state: i32) -> Option<(image::RgbaImage, usize, i32, bool)> {
        let id = match self.get(class, part, state, IMAGEFILE).or_else(|| self.get(class, part, state, IMAGEFILE1))? {
            Value::Image(i) => *i,
            _ => return None,
        };
        let img = self.image_res(id)?;
        let count = self.int(class, part, state, IMAGECOUNT).unwrap_or(1).max(1);
        let horizontal = self.int(class, part, state, IMAGELAYOUT).unwrap_or(0) == 1;
        let idx = ((state - 1).max(0) as usize).min(count as usize - 1);
        Some((img, idx, count, horizontal))
    }

    /// One state's frame cut out of the part's image strip.
    pub fn frame(&self, class: &str, part: i32, state: i32) -> Option<image::RgbaImage> {
        let (img, idx, count, horiz) = self.part_image(class, part, state)?;
        let (w, h) = (img.width(), img.height());
        let (fw, fh) = if horiz { (w / count as u32, h) } else { (w, h / count as u32) };
        let (x, y) = if horiz { (fw * idx as u32, 0) } else { (0, fh * idx as u32) };
        Some(image::imageops::crop_imm(&img, x, y, fw, fh).to_image())
    }

    pub fn classes(&self) -> Vec<String> {
        let mut v: Vec<String> = self.props.iter().map(|p| p.class.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}
