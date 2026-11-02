//! cursorprobe X0 Y0 X1 Y1 STEP [DELAY_MS]: warps the pointer over a grid and prints, for every point,
//! the cursor actually displayed (XFixes name, hotspot, size, pixel hash) and the window under it.
use std::hash::{Hash, Hasher};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;
fn main() {
    let a: Vec<i32> = std::env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let (x0, y0, x1, y1, step) = (a[0], a[1], a[2], a[3], a[4].max(1));
    let delay = a.get(5).copied().unwrap_or(12) as u64;
    let (c, s) = x11rb::connect(None).unwrap();
    let root = c.setup().roots[s].root;
    c.xfixes_query_version(5, 0).unwrap().reply().unwrap();
    let mut y = y0;
    while y <= y1 {
        let mut x = x0;
        while x <= x1 {
            c.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, x as i16, y as i16).unwrap();
            c.get_input_focus().unwrap().reply().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(delay));
            let r = c.xfixes_get_cursor_image_and_name().unwrap().reply().unwrap();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            r.cursor_image.hash(&mut h);
            let p = c.query_pointer(root).unwrap().reply().unwrap();
            // deepest window under the pointer
            let (mut w, mut cw) = (root, p.child);
            while cw != 0 { w = cw; cw = c.query_pointer(w).unwrap().reply().map(|r| r.child).unwrap_or(0); }
            println!("{x} {y} name={} hot={},{} size={}x{} hash={:016x} win=0x{w:x}", String::from_utf8_lossy(&r.name), r.xhot, r.yhot, r.width, r.height, h.finish());
            x += step;
        }
        y += step;
    }
}
