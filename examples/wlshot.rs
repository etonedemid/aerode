//! Save what the compositor shows as a PNG: `wlshot out.png`; `wlshot out.png <window id> [w h]` for a thumbnail.
use aerode::wmipc::{call, read_image, Req};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let req = match a.get(2).and_then(|s| s.parse().ok()) {
        Some(id) => Req::Thumb { id, w: a.get(3).and_then(|s| s.parse().ok()).unwrap_or(400), h: a.get(4).and_then(|s| s.parse().ok()).unwrap_or(300) },
        None => Req::Screenshot,
    };
    let s = call(&req).and_then(|v| read_image(&v)).expect("no picture");
    s.write_to_png(&mut std::fs::File::create(&a[1]).unwrap()).unwrap();
}
