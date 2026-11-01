fn main() {
    for f in std::env::args().skip(1) {
        let fonts = aero7::console::font::load_fon(std::path::Path::new(&f));
        for (i, ft) in fonts.iter().enumerate() {
            println!("{f} #{i}: {}x{} chars {}..{}", ft.width, ft.height, ft.first, ft.last);
        }
        if let Some(ft) = fonts.iter().find(|f| f.width == 8 && f.height == 12) {
            let s = "C:\\Users>dir";
            for y in 0..ft.height {
                let mut line = String::new();
                for ch in s.chars() { let g = ft.glyph(aero7::console::font::to_cp437(ch)); for x in 0..ft.width { line.push(if g[(y * ft.width + x) as usize] { '#' } else { ' ' }); } }
                println!("{line}");
            }
        }
    }
}
