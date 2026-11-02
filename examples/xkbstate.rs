use x11rb::protocol::xkb::ConnectionExt as _;
fn main() {
    let (c, _) = x11rb::connect(None).unwrap();
    c.xkb_use_extension(1, 0).unwrap().reply().unwrap();
    let s = c.xkb_get_state(x11rb::protocol::xkb::ID::USE_CORE_KBD.into()).unwrap().reply().unwrap();
    println!("group={} locked={} base={}", u8::from(s.group), u8::from(s.locked_group), s.base_group);
}
