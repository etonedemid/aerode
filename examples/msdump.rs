use aero7::msstyles::*;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let t = Theme::open(std::path::Path::new(&a[1])).unwrap();
    println!("{} props, {} classes", t.props.len(), t.classes().len());
    let filt = a.get(2).map(|s| s.to_lowercase());
    for p in &t.props {
        if let Some(f) = &filt { if !p.class.to_lowercase().contains(f.as_str()) { continue; } }
        println!("{:<40} p{:<3} s{:<2} {:>5} {:?}", p.class, p.part, p.state, p.name, p.value);
    }
}
