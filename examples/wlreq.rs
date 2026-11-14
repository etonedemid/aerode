//! Send one JSON request to the compositor and print the reply: `wlreq '{"op":"state"}'`.
fn main() {
    let a = std::env::args().nth(1).expect("json");
    let req: aerode::wmipc::Req = serde_json::from_str(&a).expect("bad request");
    println!("{}", aerode::wmipc::call(&req).unwrap_or_default());
}
