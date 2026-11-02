//! Print a PE's string table (RT_STRING): strdump <pe or .mui> [first-id [last-id]]
use aerode::pe::{Pe, ResId};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = Pe::open(std::path::Path::new(&a[1])).unwrap();
    let lo: u32 = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
    let hi: u32 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(u32::MAX);
    for n in p.names(6u16) {
        let ResId::Id(block) = n else { continue };
        let Some(d) = p.get(6u16, block) else { continue };
        // a block holds 16 strings, each a u16 length + UTF-16 text; ids (block-1)*16 + i
        let mut off = 0usize;
        for i in 0..16u32 {
            if off + 2 > d.len() { break; }
            let len = u16::from_le_bytes([d[off], d[off + 1]]) as usize;
            off += 2;
            let end = (off + len * 2).min(d.len());
            let s: Vec<u16> = d[off..end].chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
            off = end;
            let id = (block as u32 - 1) * 16 + i;
            if len > 0 && id >= lo && id <= hi { println!("{id}\t{}", String::from_utf16_lossy(&s).replace('\n', "\\n").replace('\r', "\\r")); }
        }
    }
}
