//! Open a plain X11 window of a given EWMH type, for testing aerode-wm's frames.
//!   cargo run --example wintype -- [normal|dialog|utility|splash] [title] [x y w h]
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let kind = args.first().map(String::as_str).unwrap_or("normal");
    let title = args.get(1).cloned().unwrap_or_else(|| format!("{kind} window"));
    let num = |i: usize, d: i32| args.get(i).and_then(|v| v.parse().ok()).unwrap_or(d);
    let (x, y, w, h) = (num(2, 200), num(3, 150), num(4, 320), num(5, 200));
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).map(|c| c.reply().map(|r| r.atom));
    let win = conn.generate_id()?;
    conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, root, x as i16, y as i16, w as u16, h as u16, 0,
        WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new().background_pixel(0xF0F0F0).event_mask(EventMask::STRUCTURE_NOTIFY))?;
    let ty = atom(&format!("_NET_WM_WINDOW_TYPE_{}", kind.to_uppercase()))??;
    conn.change_property32(PropMode::REPLACE, win, atom("_NET_WM_WINDOW_TYPE")??, AtomEnum::ATOM, &[ty])?;
    conn.change_property8(PropMode::REPLACE, win, atom("_NET_WM_NAME")??, atom("UTF8_STRING")??, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"wintype\0Wintype\0")?;
    // the WM places it where asked
    conn.change_property32(PropMode::REPLACE, win, AtomEnum::WM_NORMAL_HINTS, AtomEnum::WM_SIZE_HINTS,
        &[1 | 4, x as u32, y as u32, w as u32, h as u32, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
    conn.map_window(win)?;
    conn.flush()?;
    loop {
        if let x11rb::protocol::Event::DestroyNotify(_) = conn.wait_for_event()? { return Ok(()); }
    }
}
