use x11rb::connection::Connection;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
fn main() {
    let (c, _) = x11rb::connect(None).unwrap();
    c.xkb_use_extension(1, 0).unwrap().reply().unwrap();
    c.xkb_select_events(xkb::ID::USE_CORE_KBD.into(), xkb::EventType::from(0u16), xkb::EventType::STATE_NOTIFY | xkb::EventType::NEW_KEYBOARD_NOTIFY,
        xkb::MapPart::from(0u16), xkb::MapPart::from(0u16), &xkb::SelectEventsAux::new()).unwrap().check().unwrap();
    c.flush().unwrap();
    let t = std::time::Instant::now();
    while t.elapsed().as_secs() < 6 {
        while let Ok(Some(e)) = c.poll_for_event() { println!("{:?}", match e { x11rb::protocol::Event::XkbStateNotify(s) => format!("state group={}", u8::from(s.group)), o => format!("{o:?}").chars().take(60).collect() }); }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
