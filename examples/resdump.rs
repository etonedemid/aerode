// dump PNGFILE and group icons of a PE to a folder: resdump <pe> <outdir>
use aerode::pe::{self, Pe, ResId};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let p = Pe::open(std::path::Path::new(&a[1])).unwrap();
    let out = std::path::Path::new(&a[2]);
    std::fs::create_dir_all(out).unwrap();
    for n in p.names("PNGFILE") {
        if let ResId::Id(i) = n { std::fs::write(out.join(format!("png_{i}.png")), p.get("PNGFILE", i).unwrap()).unwrap(); }
    }
    for n in p.names(pe::RT_GROUP_ICON).into_iter().take(80) {
        let label = match &n { ResId::Id(i) => i.to_string(), ResId::Name(s) => s.clone() };
        let imgs = p.group_icon(Some(n));
        if let Some(img) = imgs.iter().filter(|i| i.width() <= 32).max_by_key(|i| i.width()).or(imgs.first()) {
            img.save(out.join(format!("ico_{label}.png"))).unwrap();
        }
    }
}
