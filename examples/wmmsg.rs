//! Send a client message to aerode-wm's check window (_NET_SUPPORTING_WM_CHECK), for testing.
//!   cargo run --example wmmsg -- _AERODE_RESTART            # restart in place
//!   cargo run --example wmmsg -- _AERODE_RESTART 0xDEAD     # debug builds: test the crash recovery
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().ok_or("usage: wmmsg ATOM [data0 .. data4]")?;
    let num = |s: &str| if let Some(h) = s.strip_prefix("0x") { u32::from_str_radix(h, 16) } else { s.parse() };
    let mut data = [0u32; 5];
    for (i, a) in args.iter().skip(1).take(5).enumerate() { data[i] = num(a)?; }
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).map(|c| c.reply().map(|r| r.atom));
    // WMMSG_WINDOW=0x... sends to another window instead (testing)
    let check = match std::env::var("WMMSG_WINDOW").ok().and_then(|v| num(&v).ok()) {
        Some(w) => w,
        None => conn.get_property(false, root, atom("_NET_SUPPORTING_WM_CHECK")??, AtomEnum::WINDOW, 0, 1)?.reply()?
            .value32().and_then(|mut v| v.next()).ok_or("no window manager check window")?,
    };
    let ev = ClientMessageEvent::new(32, check, atom(name)??, data);
    conn.send_event(false, check, EventMask::NO_EVENT, ev)?;
    // a round trip before exiting: a connection closed right after the request can lose it
    conn.get_input_focus()?.reply()?;
    Ok(())
}
