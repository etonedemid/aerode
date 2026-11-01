use aero7::pe::{Pe, ResId};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pe = Pe::open(std::path::Path::new(&a[1])).unwrap();
    for t in pe.types() {
        let names = pe.names(t.clone());
        let sizes: usize = names.iter().filter_map(|n| pe.get(t.clone(), n.clone()).map(|d| d.len())).sum();
        let show: Vec<String> = names.iter().take(8).map(|n| match n { ResId::Id(i) => i.to_string(), ResId::Name(s) => s.clone() }).collect();
        println!("{:?}: {} items, {} bytes  e.g. {:?}", t, names.len(), sizes, show);
    }
}
#[allow(dead_code)]
fn _x() {}
