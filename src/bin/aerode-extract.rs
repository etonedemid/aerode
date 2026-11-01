//! aerode-extract – pull the Windows 7 artwork out of your own ISO.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = aerode::extract::run(&args) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
