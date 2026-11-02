//! Fonts: the Win7 Fonts folder (family tiles with "Abg" drawn in the font, family folders,
//! Preview / Delete / Show / Hide, installing .ttf/.otf/.ttc files), Font settings and the
//! ClearType Text Tuner. Backed by fontconfig (fc-list, fc-query, fc-cache, a rejectfont file for
//! hidden fonts) and the session's Xft settings (xsettingsd + X resources).
use super::*;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::{cairo, pango};
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
struct Face { family: String, style: String, file: String, format: String, version: f64, foundry: String, langs: String }

#[derive(Clone, Default)]
struct Family { name: String, faces: Vec<Face>, hidden: bool }

const TILE: i32 = 72;
const SAMPLE_TEXT: &str = "Lorem ipsum dolor sit amet, consectetuer adipiscing elit. Maecenas porttitor congue massa. Fusce posuere, magna sed pulvinar ultricies, purus lectus malesuada libero, sit amet commodo magna eros quis urna.";

/// Where fonts the user installs go (fontconfig reads it), plus the legacy ~/.fonts.
fn user_font_dirs() -> Vec<PathBuf> { vec![glib::user_data_dir().join("fonts"), config::home().join(".fonts")] }
/// Fonts the user installed may be deleted; the Windows fonts aerode-extract put in the font folder
/// (Segoe UI and friends, which the desktop needs) are protected like Win7's system fonts.
fn is_user_font(file: &str) -> bool {
    user_font_dirs().iter().any(|d| Path::new(file).starts_with(d) && !Path::new(file).starts_with(d.join("aerode")) && !Path::new(file).starts_with(d.join("aero7")))
}

/// The first two letters Win7 shows for a script the font is made for ("Abg" for Latin).
fn sample_for(f: &Family) -> String {
    let langs = f.faces.first().map(|x| x.langs.clone()).unwrap_or_default();
    if langs.is_empty() || langs.split('|').any(|l| l == "en") { return "Abg".into(); }
    let first = langs.split('|').next().unwrap_or("en");
    let s = pango::Language::from_string(first).sample_string().to_string();
    let s: String = s.chars().filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation()).take(3).collect();
    if s.is_empty() { "Abg".into() } else { s }
}

fn font_desc(family: &str, style: &str, px: f64) -> pango::FontDescription {
    let mut d = pango::FontDescription::from_string(&format!("{family}, {style}"));
    d.set_family(family);
    d.set_absolute_size(px * pango::SCALE as f64);
    d
}

/// The Win7 font icon: a white page (or a stack of pages for a family) with a folded corner and
/// the sample letters in the font; hidden fonts are faded.
fn tile(f: &Family, face: Option<&Face>) -> Option<Pixbuf> {
    let surf = cairo::ImageSurface::create(cairo::Format::ARgb32, TILE, TILE).ok()?;
    {
        let cr = cairo::Context::new(&surf).ok()?;
        let pages = if face.is_none() && f.faces.len() > 1 { 3 } else { 1 };
        let (pw, ph) = (46.0, 56.0);
        for i in (0..pages).rev() {
            let (x, y) = (12.0 + i as f64 * 4.0, 6.0 + i as f64 * 3.0 - (pages - 1) as f64 * 3.0 + 4.0);
            let fold = 10.0;
            cr.new_path();
            cr.move_to(x, y);
            cr.line_to(x + pw - fold, y);
            cr.line_to(x + pw, y + fold);
            cr.line_to(x + pw, y + ph);
            cr.line_to(x, y + ph);
            cr.close_path();
            let g = cairo::LinearGradient::new(x, y, x + pw, y + ph);
            g.add_color_stop_rgb(0.0, 1.0, 1.0, 1.0);
            g.add_color_stop_rgb(1.0, 0.93, 0.94, 0.96);
            cr.set_source(&g).ok();
            cr.fill_preserve().ok();
            cr.set_source_rgb(0.62, 0.64, 0.68);
            cr.set_line_width(1.0);
            cr.stroke().ok();
            // the folded corner
            cr.move_to(x + pw - fold, y);
            cr.line_to(x + pw - fold, y + fold);
            cr.line_to(x + pw, y + fold);
            cr.close_path();
            cr.set_source_rgb(0.85, 0.87, 0.9);
            cr.fill_preserve().ok();
            cr.set_source_rgb(0.62, 0.64, 0.68);
            cr.stroke().ok();
        }
        let style = face.map(|x| x.style.as_str()).unwrap_or("Regular");
        let layout = pangocairo::functions::create_layout(&cr);
        layout.set_font_description(Some(&font_desc(&f.name, style, 20.0)));
        layout.set_text(&sample_for(f));
        let (_, log) = layout.pixel_extents();
        let (x0, y0) = if pages > 1 { (12.0, 4.0) } else { (12.0, 10.0) };
        let scale = if log.width() > 40 { 40.0 / log.width() as f64 } else { 1.0 };
        cr.save().ok();
        cr.translate(x0 + (pw - log.width() as f64 * scale) / 2.0 - 2.0, y0 + (ph - log.height() as f64 * scale) / 2.0);
        cr.scale(scale, scale);
        cr.set_source_rgb(0.0, 0.0, 0.0);
        pangocairo::functions::show_layout(&cr, &layout);
        cr.restore().ok();
    }
    surf.flush();
    let pb = gtk::gdk::pixbuf_get_from_surface(&surf, 0, 0, TILE, TILE)?;
    if f.hidden {
        // Win7 shows hidden fonts faded
        let faded = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, TILE, TILE)?;
        faded.fill(0);
        pb.composite(&faded, 0, 0, TILE, TILE, 0.0, 0.0, 1.0, 1.0, gtk::gdk_pixbuf::InterpType::Nearest, 100);
        return Some(faded);
    }
    Some(pb)
}

fn parse_fc_list(out: &str) -> Vec<Face> {
    out.lines().filter_map(|l| {
        let p: Vec<&str> = l.split('\t').collect();
        if p.len() < 7 || p[0].is_empty() { return None; }
        Some(Face { family: p[0].to_string(), style: p[1].to_string(), file: p[2].to_string(), format: p[3].to_string(),
            version: p[4].parse::<f64>().unwrap_or(0.0) / 65536.0, foundry: p[5].to_string(), langs: p[6].to_string() })
    }).collect()
}

const FC_FORMAT: &str = "%{family[0]}\t%{style[0]}\t%{file}\t%{fontformat}\t%{fontversion}\t%{foundry}\t%{lang}\n";

/// Families from fc-list, plus the ones Hide took away (they no longer show up there).
fn families(out: &str, hidden: &[String], lang_ok: &Option<HashSet<String>>) -> Vec<Family> {
    let mut map: BTreeMap<String, Family> = BTreeMap::new();
    for f in parse_fc_list(out) {
        let key = f.family.to_lowercase();
        let e = map.entry(key).or_insert_with(|| Family { name: f.family.clone(), ..Default::default() });
        // one face per style; a copy the user installed wins, so it can be deleted again
        match e.faces.iter().position(|x| x.style == f.style) {
            None => e.faces.push(f),
            Some(i) => if is_user_font(&f.file) { e.faces[i] = f; },
        }
    }
    for h in hidden {
        map.entry(h.to_lowercase()).or_insert_with(|| Family { name: h.clone(), ..Default::default() });
    }
    let mut v: Vec<Family> = map.into_values().collect();
    for f in v.iter_mut() {
        f.hidden = hidden.iter().any(|h| h.eq_ignore_ascii_case(&f.name))
            || lang_ok.as_ref().map(|ok| !ok.contains(&f.name.to_lowercase())).unwrap_or(false);
        // Regular first, like the Win7 family folder
        f.faces.sort_by_key(|x| (x.style != "Regular" && x.style != "Book" && x.style != "Normal", x.style.clone()));
    }
    v
}

/// The language the user types in, for "Hide fonts based on language settings".
fn ui_lang() -> String {
    let l = std::env::var("LC_ALL").ok().filter(|s| !s.is_empty()).or_else(|| std::env::var("LANG").ok()).unwrap_or_else(|| "en".into());
    let l = l.split(['.', '@']).next().unwrap_or("en").replace('_', "-").to_lowercase();
    if l == "c" || l == "posix" || l.is_empty() { "en".into() } else { l.split('-').next().unwrap_or("en").to_string() }
}

/// Fonts > Hide: fontconfig rejects the family for every program.
fn write_hidden(hidden: &[String]) {
    let dir = glib::user_config_dir().join("fontconfig/conf.d");
    let file = dir.join("90-aerode-hidden-fonts.conf");
    if hidden.is_empty() { let _ = std::fs::remove_file(&file); return; }
    let _ = std::fs::create_dir_all(&dir);
    let mut x = String::from("<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n<!-- written by Control Panel > Fonts (Hide) -->\n<fontconfig>\n  <selectfont>\n    <rejectfont>\n");
    for h in hidden {
        x += &format!("      <pattern><patelt name=\"family\"><string>{}</string></patelt></pattern>\n", glib::markup_escape_text(h));
    }
    x += "    </rejectfont>\n  </selectfont>\n</fontconfig>\n";
    let _ = std::fs::write(file, x);
}

/// Script names for the details pane's "Designed for".
fn scripts(langs: &str) -> String {
    let ls: Vec<&str> = langs.split('|').collect();
    let mut out = vec![];
    for (lang, name) in [("en", "Latin"), ("el", "Greek"), ("ru", "Cyrillic"), ("he", "Hebrew"), ("ar", "Arabic"), ("hi", "Devanagari"), ("th", "Thai"),
        ("ja", "Japanese"), ("ko", "Korean"), ("zh-cn", "Chinese (Simplified)"), ("zh-tw", "Chinese (Traditional)"), ("ka", "Georgian"), ("hy", "Armenian"),
        ("bn", "Bengali"), ("ta", "Tamil"), ("am", "Ethiopic")] {
        if ls.contains(&lang) { out.push(name); }
    }
    if out.is_empty() { out.push("Symbol"); }
    out.join("; ")
}

fn category(f: &Family) -> &'static str {
    let n = f.name.to_lowercase();
    if n.contains("emoji") || n.contains("symbol") || n.contains("dings") || n.contains("awesome") || n.contains("icons") { "Symbol" }
    else if n.contains("mono") || n.contains("code") || n.contains("courier") || n.contains("console") { "Text (monospaced)" }
    else if n.contains("script") || n.contains("hand") || n.contains("display") { "Display" } else { "Text" }
}

struct Fonts {
    cp: Rc<Cp>,
    all: RefCell<Vec<Family>>,
    /// Some(family) while a family folder is open
    open_family: RefCell<Option<String>>,
    store: gtk::ListStore,
    iv: gtk::IconView,
    crumb: gtk::Box,
    count: gtk::Label,
    details: gtk::Label,
    details_icon: gtk::Image,
    hide_btn: gtk::Button,
    generation: Cell<u32>,
}

impl Fonts {
    fn load(self: &Rc<Self>) {
        self.count.set_text("Loading fonts...");
        let me = self.clone();
        util::output_async(vec!["fc-list".into(), "--format".into(), FC_FORMAT.into()], move |out| {
            let out = out.unwrap_or_default();
            let cfg = config::get().fonts;
            if cfg.hide_by_language {
                let me2 = me.clone();
                util::output_async(vec!["fc-list".into(), format!(":lang={}", ui_lang()), "--format".into(), "%{family[0]}\n".into()], move |ok| {
                    let ok: Option<HashSet<String>> = ok.filter(|s| !s.trim().is_empty()).map(|s| s.lines().map(|l| l.to_lowercase()).collect());
                    *me2.all.borrow_mut() = families(&out, &config::get().fonts.hidden, &ok);
                    me2.fill();
                });
            } else {
                *me.all.borrow_mut() = families(&out, &cfg.hidden, &None);
                me.fill();
            }
        });
    }

    /// The tiles of the current folder (all families, or the faces of one family).
    fn fill(self: &Rc<Self>) {
        self.store.clear();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        for c in self.crumb.children() { self.crumb.remove(&c); }
        let fam = self.open_family.borrow().clone();
        let all = self.all.borrow().clone();
        // (family, face index or -1)
        let mut items: Vec<(Family, i32)> = vec![];
        match &fam {
            None => { for f in &all { items.push((f.clone(), -1)); } }
            Some(name) => {
                let me = self.clone();
                self.crumb.pack_start(&link("Fonts", move || { *me.open_family.borrow_mut() = None; me.fill(); }), false, false, 0);
                self.crumb.pack_start(&gtk::Label::new(Some(&format!("▸  {name}"))), false, false, 0);
                if let Some(f) = all.iter().find(|f| &f.name == name) { for i in 0..f.faces.len() { items.push((f.clone(), i as i32)); } }
            }
        }
        self.crumb.show_all();
        self.count.set_text(&format!("{} item{}", items.len(), if items.len() == 1 { "" } else { "s" }));
        let placeholder = util::pixbuf("font-x-generic", 48);
        for (f, i) in &items {
            let label = if *i < 0 { f.name.clone() } else { format!("{} {}", f.name, f.faces[*i as usize].style) };
            self.store.insert_with_values(None, &[(0, &placeholder), (1, &label), (2, &f.name), (3, i)]);
        }
        // draw the real tiles a few at a time so the window stays responsive
        let me = self.clone();
        let idx = Cell::new(0usize);
        glib::idle_add_local(move || {
            if me.generation.get() != generation { return glib::ControlFlow::Break; }
            let Some(it) = me.store.iter_nth_child(None, idx.get() as i32) else { return glib::ControlFlow::Break };
            for _ in 0..16 {
                let (f, i) = &items[idx.get()];
                let face = if *i >= 0 { f.faces.get(*i as usize) } else { None };
                if let Some(pb) = tile(f, face) { me.store.set_value(&it, 0, &pb.to_value()); }
                idx.set(idx.get() + 1);
                if idx.get() >= items.len() || !me.store.iter_next(&it) { return glib::ControlFlow::Break; }
            }
            glib::ControlFlow::Continue
        });
        self.update_details();
    }

    /// Selected (family, face index or -1).
    fn selected(&self) -> Vec<(String, i32)> {
        self.iv.selected_items().iter().filter_map(|p| self.store.iter(p)).map(|it| {
            (self.store.value(&it, 2).get::<String>().unwrap_or_default(), self.store.value(&it, 3).get::<i32>().unwrap_or(-1))
        }).collect()
    }

    fn family(&self, name: &str) -> Option<Family> { self.all.borrow().iter().find(|f| f.name == name).cloned() }

    fn update_details(&self) {
        let sel = self.selected();
        let Some((name, i)) = sel.first().cloned() else {
            self.details.set_markup(&format!("{} fonts installed", self.all.borrow().len()));
            self.details_icon.set_from_pixbuf(util::pixbuf("preferences-desktop-font", 48).as_ref());
            self.hide_btn.set_label("Hide");
            return;
        };
        let Some(f) = self.family(&name) else { return };
        let face = if i >= 0 { f.faces.get(i as usize).cloned() } else { None };
        if let Some(pb) = tile(&f, face.as_ref()) { self.details_icon.set_from_pixbuf(Some(&pb)); }
        let styles = match &face { Some(x) => x.style.clone(), None => f.faces.iter().map(|x| x.style.clone()).collect::<Vec<_>>().join("; ") };
        let first = face.clone().or_else(|| f.faces.first().cloned()).unwrap_or_default();
        let esc = |s: &str| glib::markup_escape_text(s).to_string();
        let mut m = format!("<span size='large'>{}</span>   <span foreground='#4b5b73'>Font style:</span> {}\n", esc(&f.name), esc(&styles));
        m += &format!("<span foreground='#4b5b73'>Show/hide:</span> {}   <span foreground='#4b5b73'>Designed for:</span> {}   <span foreground='#4b5b73'>Category:</span> {}\n",
            if f.hidden { "Hide" } else { "Show" }, esc(&scripts(&first.langs)), category(&f));
        m += &format!("<span foreground='#4b5b73'>Designer/foundry:</span> {}", esc(if first.foundry.is_empty() || first.foundry == "unknown" { "(unknown)" } else { &first.foundry }));
        self.details.set_markup(&m);
        self.hide_btn.set_label(if f.hidden && sel.len() == 1 { "Show" } else { "Hide" });
    }

    fn activate(self: &Rc<Self>) {
        let Some((name, i)) = self.selected().first().cloned() else { return };
        let Some(f) = self.family(&name) else { return };
        // a family of several faces is a folder in Win7
        if i < 0 && f.faces.len() > 1 { *self.open_family.borrow_mut() = Some(name); self.fill(); return; }
        let face = f.faces.get(i.max(0) as usize).cloned();
        if let Some(face) = face { preview(&self.cp, &face, None); }
    }

    fn preview_selected(self: &Rc<Self>) {
        for (name, i) in self.selected() {
            let Some(f) = self.family(&name) else { continue };
            if i >= 0 { if let Some(x) = f.faces.get(i as usize) { preview(&self.cp, x, None); } continue; }
            for x in f.faces.iter().take(4) { preview(&self.cp, x, None); }
        }
    }

    fn delete_selected(self: &Rc<Self>) {
        let sel = self.selected();
        if sel.is_empty() { return; }
        let mut files: Vec<String> = vec![];
        for (name, i) in &sel {
            let Some(f) = self.family(name) else { continue };
            let faces: Vec<Face> = if *i >= 0 { f.faces.get(*i as usize).cloned().into_iter().collect() } else { f.faces.clone() };
            if let Some(sys) = faces.iter().find(|x| !is_user_font(&x.file)) {
                let _ = sys;
                let m = gtk::MessageDialog::new(Some(&parent_window(&self.cp)), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::Ok,
                    &format!("{} is a protected system font and cannot be deleted.", f.name));
                m.set_title("Delete Font");
                m.run();
                m.close();
                return;
            }
            files.extend(faces.into_iter().map(|x| x.file));
        }
        files.sort();
        files.dedup();
        let m = gtk::MessageDialog::new(Some(&parent_window(&self.cp)), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::YesNo,
            if sel.len() == 1 { "Are you sure you want to permanently delete this font?" } else { "Are you sure you want to permanently delete these fonts?" });
        m.set_title("Delete Font");
        let yes = m.run() == gtk::ResponseType::Yes;
        m.close();
        if !yes { return; }
        for f in &files {
            if let Err(e) = std::fs::remove_file(f) { util::error(&format!("Could not delete {f}"), &e.to_string()); }
        }
        self.refresh_cache();
    }

    fn toggle_hidden(self: &Rc<Self>) {
        let sel = self.selected();
        if sel.is_empty() { return; }
        let show = self.hide_btn.label().map(|l| l == "Show").unwrap_or(false);
        let names: Vec<String> = sel.into_iter().map(|s| s.0).collect();
        config::update(|c| {
            let h = &mut c.fonts.hidden;
            for n in &names { h.retain(|x| !x.eq_ignore_ascii_case(n)); if !show { h.push(n.clone()); } }
            h.sort();
        });
        write_hidden(&config::get().fonts.hidden);
        let hidden = config::get().fonts.hidden;
        for f in self.all.borrow_mut().iter_mut() { if names.contains(&f.name) { f.hidden = hidden.iter().any(|h| h.eq_ignore_ascii_case(&f.name)); } }
        // repaint the changed tiles only
        if let Some(it) = self.store.iter_first() {
            loop {
                let n: String = self.store.value(&it, 2).get().unwrap_or_default();
                if names.contains(&n) {
                    let i: i32 = self.store.value(&it, 3).get().unwrap_or(-1);
                    if let Some(f) = self.family(&n) {
                        let face = if i >= 0 { f.faces.get(i as usize).cloned() } else { None };
                        if let Some(pb) = tile(&f, face.as_ref()) { self.store.set_value(&it, 0, &pb.to_value()); }
                    }
                }
                if !self.store.iter_next(&it) { break; }
            }
        }
        self.update_details();
    }

    fn install_dialog(self: &Rc<Self>) {
        let fc = gtk::FileChooserNative::new(Some("Install Fonts"), Some(&parent_window(&self.cp)), gtk::FileChooserAction::Open, Some("Install"), Some("Cancel"));
        fc.set_select_multiple(true);
        let ff = gtk::FileFilter::new();
        ff.set_name(Some("Font files (*.ttf; *.otf; *.ttc; *.fon)"));
        for p in ["*.ttf", "*.TTF", "*.otf", "*.OTF", "*.ttc", "*.TTC", "*.otc", "*.pfb", "*.fon", "*.pcf.gz", "*.woff2", "*.woff"] { ff.add_pattern(p); }
        fc.add_filter(ff);
        if fc.run() == gtk::ResponseType::Accept {
            let files: Vec<PathBuf> = fc.filenames();
            let me = self.clone();
            install(&self.cp, &files, Rc::new(move || me.refresh_cache()));
        }
    }

    /// fc-cache after installing/deleting, then list again (off the GTK thread).
    fn refresh_cache(self: &Rc<Self>) {
        self.count.set_text("Updating the font cache...");
        let me = self.clone();
        let dir = user_font_dirs()[0].to_string_lossy().into_owned();
        util::output_async(vec!["fc-cache".into(), "-f".into(), dir], move |_| me.load());
    }
}

/// Copy (or link, with "Allow fonts to be installed using a shortcut") font files into the user font
/// folder, asking before replacing an installed one.
fn install(cp: &Rc<Cp>, files: &[PathBuf], done: Rc<dyn Fn()>) {
    let dir = user_font_dirs()[0].clone();
    if let Err(e) = std::fs::create_dir_all(&dir) { util::error("Could not create the font folder", &e.to_string()); return; }
    let link = config::get().fonts.install_as_shortcut;
    let mut any = false;
    for f in files {
        let Some(name) = f.file_name() else { continue };
        let dst = dir.join(name);
        if dst == *f { continue; }
        if dst.exists() {
            let m = gtk::MessageDialog::new(Some(&parent_window(cp)), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo,
                &format!("The font {} is already installed. Do you want to replace it?", name.to_string_lossy()));
            m.set_title("Install Font");
            let yes = m.run() == gtk::ResponseType::Yes;
            m.close();
            if !yes { continue; }
            let _ = std::fs::remove_file(&dst);
        }
        let r = if link { std::os::unix::fs::symlink(f, &dst) } else { std::fs::copy(f, &dst).map(|_| ()) };
        match r { Ok(()) => any = true, Err(e) => util::error(&format!("Could not install {}", name.to_string_lossy()), &e.to_string()) }
    }
    if any { done(); }
}

/// The Win7 font viewer: Print / Install, name, version, outline type, the alphabet and the
/// pangram at 12–72 points. `file` is set when previewing a font file that is not installed.
fn preview(cp: &Rc<Cp>, face: &Face, file: Option<PathBuf>) {
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_transient_for(Some(&parent_window(cp)));
    let kind = match face.format.as_str() { "CFF" => "OpenType", "TrueType" => "TrueType", "" => "Font", x => x };
    w.set_title(&format!("{} ({kind})", if face.style == "Regular" || face.style.is_empty() { face.family.clone() } else { format!("{} {}", face.family, face.style) }));
    w.set_default_size(720, 620);
    w.set_icon_name(Some("preferences-desktop-font"));
    let root = vbox(0);
    let bar = hbox(4);
    bar.style_context().add_class("commandbar");
    let print = gtk::Button::with_label("Print");
    let inst = gtk::Button::with_label("Install");
    inst.set_sensitive(file.is_some());
    bar.pack_start(&print, false, false, 0);
    bar.pack_start(&inst, false, false, 0);
    root.pack_start(&bar, false, false, 0);
    let area = gtk::DrawingArea::new();
    area.set_size_request(680, 900);
    let face2 = face.clone();
    area.connect_draw(move |a, cr| {
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok();
        let w = a.allocated_width() as f64;
        let mut y = 14.0;
        let ui = |cr: &cairo::Context, t: &str, y: &mut f64, size: f64| {
            let l = pangocairo::functions::create_layout(cr);
            l.set_font_description(Some(&pango::FontDescription::from_string(&format!("Segoe UI {size}"))));
            l.set_text(t);
            cr.move_to(16.0, *y);
            cr.set_source_rgb(0.0, 0.0, 0.0);
            pangocairo::functions::show_layout(cr, &l);
            *y += l.pixel_size().1 as f64 + 2.0;
        };
        ui(cr, &format!("Font name: {}", face2.family), &mut y, 10.0);
        ui(cr, &format!("Version: Version {:.2}", face2.version), &mut y, 10.0);
        ui(cr, match face2.format.as_str() { "CFF" => "OpenType Layout, PostScript Outlines", "TrueType" => "OpenType Layout, TrueType Outlines", _ => "Bitmap font" }, &mut y, 10.0);
        y += 6.0;
        cr.set_source_rgb(0.8, 0.8, 0.8);
        cr.rectangle(16.0, y, w - 32.0, 1.0);
        cr.fill().ok();
        y += 10.0;
        // like Win7 the long lines simply run off the right edge
        let sample = |cr: &cairo::Context, t: &str, px: f64, x: f64, y: &mut f64| {
            let l = pangocairo::functions::create_layout(cr);
            l.set_font_description(Some(&font_desc(&face2.family, &face2.style, px)));
            l.set_text(t);
            cr.move_to(x, *y);
            cr.set_source_rgb(0.0, 0.0, 0.0);
            pangocairo::functions::show_layout(cr, &l);
            l.pixel_size().1 as f64
        };
        for t in ["abcdefghijklmnopqrstuvwxyz", "ABCDEFGHIJKLMNOPQRSTUVWXYZ", "1234567890.:,; ' \" (!?) +-*/="] {
            let h = sample(cr, t, 16.0, 16.0, &mut y);
            y += h + 2.0;
        }
        y += 8.0;
        cr.set_source_rgb(0.8, 0.8, 0.8);
        cr.rectangle(16.0, y, w - 32.0, 1.0);
        cr.fill().ok();
        y += 10.0;
        for pt in [12, 18, 24, 36, 48, 60, 72] {
            let px = pt as f64 * 96.0 / 72.0;
            let mut yy = y;
            ui(cr, &pt.to_string(), &mut yy, 8.0);
            let h = sample(cr, "The quick brown fox jumps over the lazy dog. 1234567890", px, 52.0, &mut y);
            y += h + 6.0;
        }
        glib::Propagation::Stop
    });
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.add(&area);
    root.pack_start(&sw, true, true, 0);
    w.add(&root);
    {
        let (w2, face) = (w.clone(), face.clone());
        print.connect_clicked(move |_| {
            // the same page, through the GTK print dialog
            let op = gtk::PrintOperation::new();
            op.set_n_pages(1);
            let face = face.clone();
            op.connect_draw_page(move |_, ctx, _| {
                let Some(cr) = ctx.cairo_context() else { return };
                let l = pangocairo::functions::create_layout(&cr);
                let mut y = 0.0;
                for pt in [12, 18, 24, 36, 48] {
                    l.set_font_description(Some(&font_desc(&face.family, &face.style, pt as f64)));
                    l.set_text("The quick brown fox jumps over the lazy dog. 1234567890");
                    cr.move_to(0.0, y);
                    pangocairo::functions::show_layout(&cr, &l);
                    y += l.pixel_size().1 as f64 + 8.0;
                }
            });
            let _ = op.run(gtk::PrintOperationAction::PrintDialog, Some(&w2));
        });
    }
    if let Some(f) = file {
        let (cp, w2) = (cp.clone(), w.clone());
        inst.connect_clicked(move |b| {
            let b = b.clone();
            install(&cp, &[f.clone()], Rc::new(move || {
                b.set_sensitive(false);
                b.set_label("Installed");
                util::spawn(&["fc-cache", "-f", &user_font_dirs()[0].to_string_lossy()]);
            }));
            let _ = &w2;
        });
    }
    w.show_all();
}

pub fn page_fonts(cp: &Rc<Cp>) -> gtk::Widget {
    // side pane, as in Win7
    let v = vbox(0);
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::I32]);
    let iv = gtk::IconView::with_model(&store);
    let details = gtk::Label::new(None);
    let details_icon = gtk::Image::new();
    let hide_btn = gtk::Button::with_label("Hide");
    let crumb = hbox(6);
    let count = gtk::Label::new(None);
    let f = Rc::new(Fonts { cp: cp.clone(), all: RefCell::new(vec![]), open_family: RefCell::new(None), store: store.clone(), iv: iv.clone(),
        crumb: crumb.clone(), count: count.clone(), details: details.clone(), details_icon: details_icon.clone(), hide_btn: hide_btn.clone(), generation: Cell::new(0) });
    let views = gtk::Stack::new();
    {
        let (me, views, f) = (cp.clone(), views.clone(), f.clone());
        let s = link("Font settings", move || {
            if let Some(old) = views.child_by_name("settings") { views.remove(&old); }
            let (views2, f2, me2) = (views.clone(), f.clone(), me.clone());
            let w = font_settings(&me, Rc::new(move |reload| {
                views2.set_visible_child_name("folder");
                me2.crumb.set_text("Control Panel ▸ Appearance and Personalization ▸ Fonts");
                if reload { f2.load(); }
            }));
            w.show_all();
            views.add_named(&w, "settings");
            views.set_visible_child_name("settings");
            me.crumb.set_text("Control Panel ▸ Appearance and Personalization ▸ Fonts ▸ Font settings");
        });
        cp.side.pack_start(&s, false, false, 2);
    }
    cp.side.pack_start(&link("Get more font information online", || { util::open_uri("https://wiki.archlinux.org/title/Fonts"); }), false, false, 2);
    { let me = cp.clone(); cp.side.pack_start(&link("Adjust ClearType text", move || cleartype_tuner(&me)), false, false, 2); }
    {
        let charmap = ["gucharmap", "kcharselect", "gnome-characters"].into_iter().find(|c| util::which(c));
        let l = link("Find a character", move || { if let Some(c) = charmap { util::spawn(&[c]); } });
        l.set_sensitive(charmap.is_some());
        cp.side.pack_start(&l, false, false, 2);
    }
    cp.side_link("Change font size", "display");
    let see = gtk::Label::new(Some("See also"));
    see.set_xalign(0.0);
    see.style_context().add_class("side-h");
    see.set_margin_top(20);
    cp.side.pack_start(&see, false, false, 0);
    cp.side_link("Text Services and Input Language", "region");
    cp.side_link("Personalization", "personalization");

    let head = hbox(12);
    head.pack_start(&util::image("preferences-desktop-font", 48), false, false, 0);
    let ht = vbox(2);
    ht.pack_start(&title("Preview, delete, or show and hide the fonts installed on your computer"), false, false, 0);
    ht.pack_start(&crumb, false, false, 0);
    head.pack_start(&ht, true, true, 0);
    v.pack_start(&head, false, false, 0);
    // command bar: Organize, Preview, Delete, Hide/Show
    let bar = hbox(2);
    bar.style_context().add_class("commandbar");
    let org = gtk::MenuButton::new();
    org.set_label("Organize ▾");
    org.set_relief(gtk::ReliefStyle::None);
    let menu = gtk::Menu::new();
    let add = |m: &gtk::Menu, l: &str, cb: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.connect_activate(move |_| cb()); m.append(&it); };
    { let f = f.clone(); add(&menu, "_Install new font...", Box::new(move || f.install_dialog())); }
    { let iv = iv.clone(); add(&menu, "Select _all", Box::new(move || iv.select_all())); }
    { let f = f.clone(); add(&menu, "_Refresh", Box::new(move || f.load())); }
    menu.show_all();
    org.set_popup(Some(&menu));
    let prev = gtk::Button::with_label("Preview");
    let del = gtk::Button::with_label("Delete");
    for b in [&prev, &del, &hide_btn] { b.set_relief(gtk::ReliefStyle::None); }
    bar.pack_start(&org, false, false, 0);
    bar.pack_start(&prev, false, false, 0);
    bar.pack_start(&del, false, false, 0);
    bar.pack_start(&hide_btn, false, false, 0);
    bar.pack_end(&count, false, false, 6);
    v.pack_start(&bar, false, false, 6);
    iv.set_pixbuf_column(0);
    iv.set_text_column(1);
    iv.set_item_width(84);
    iv.set_selection_mode(gtk::SelectionMode::Multiple);
    iv.set_row_spacing(2);
    iv.set_column_spacing(2);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.set_size_request(-1, 360);
    sw.add(&iv);
    v.pack_start(&sw, true, true, 0);
    // details pane
    let dp = hbox(12);
    dp.style_context().add_class("exp-details");
    dp.pack_start(&details_icon, false, false, 0);
    details.set_xalign(0.0);
    details.set_line_wrap(true);
    dp.pack_start(&details, true, true, 0);
    v.pack_start(&dp, false, false, 0);
    let sync = { let (iv, p, d, h) = (iv.clone(), prev.clone(), del.clone(), hide_btn.clone()); move || {
        let any = !iv.selected_items().is_empty();
        for b in [&p, &d, &h] { b.set_sensitive(any); }
    } };
    sync();
    { let (f, sync) = (f.clone(), sync.clone()); iv.connect_selection_changed(move |_| { f.update_details(); sync(); }); }
    { let f = f.clone(); iv.connect_item_activated(move |_, _| f.activate()); }
    { let f = f.clone(); prev.connect_clicked(move |_| f.preview_selected()); }
    { let f = f.clone(); del.connect_clicked(move |_| f.delete_selected()); }
    { let f = f.clone(); hide_btn.connect_clicked(move |_| f.toggle_hidden()); }
    { let f = f.clone(); iv.connect_key_press_event(move |_, e| {
        if e.keyval() == gtk::gdk::keys::constants::Delete { f.delete_selected(); return glib::Propagation::Stop; }
        glib::Propagation::Proceed
    }); }
    // drop font files to install them, like the Win7 Fonts folder
    iv.drag_dest_set(gtk::DestDefaults::ALL, &[gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::OTHER_APP, 0)], gtk::gdk::DragAction::COPY);
    { let (f, cp) = (f.clone(), cp.clone()); iv.connect_drag_data_received(move |_, _, _, _, data, _, _| {
        let files: Vec<PathBuf> = data.uris().iter().filter_map(|u| gio::File::for_uri(u).path()).collect();
        if !files.is_empty() { let f2 = f.clone(); install(&cp, &files, Rc::new(move || f2.refresh_cache())); }
    }); }
    f.load();
    // aerode-control fonts FILE.ttf: the font viewer with Install, like double-clicking a font file in Win7
    if let Some(file) = std::env::args().nth(2).map(PathBuf::from).filter(|p| p.is_file()) {
        let cp2 = cp.clone();
        util::output_async(vec!["fc-query".into(), "--format".into(), FC_FORMAT.into(), file.to_string_lossy().into_owned()], move |out| {
            let face = out.as_deref().map(parse_fc_list).and_then(|v| v.into_iter().next())
                .unwrap_or_else(|| Face { family: file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), ..Default::default() });
            preview(&cp2, &face, Some(file.clone()));
        });
    }
    views.add_named(&v, "folder");
    views.set_visible_child_name("folder");
    views.upcast()
}

// ------------------------------------------------------------------ Font settings

/// Win7 "Change font settings"; `back(reload)` returns to the Fonts folder.
fn font_settings(cp: &Rc<Cp>, back: Rc<dyn Fn(bool)>) -> gtk::Widget {
    let o = config::get().fonts;
    let v = vbox(10);
    v.pack_start(&title("Change font settings"), false, false, 0);
    let g1 = vbox(4);
    let lang = check("_Hide fonts based on language settings", o.hide_by_language);
    g1.pack_start(&lang, false, false, 0);
    let t1 = text("Fonts that are not designed for your input language are shown as hidden in the Fonts folder.");
    t1.set_margin_start(24);
    t1.style_context().add_class("dim-label");
    g1.pack_start(&t1, false, false, 0);
    v.pack_start(&group("Font settings", &g1), false, false, 0);
    let g2 = vbox(4);
    let link_inst = check("_Allow fonts to be installed using a shortcut (advanced)", o.install_as_shortcut);
    g2.pack_start(&link_inst, false, false, 0);
    let t2 = text("Instead of copying the font file into the font folder, a shortcut to the file is created. The font is unavailable if the file is moved or its drive is removed.");
    t2.set_margin_start(24);
    t2.style_context().add_class("dim-label");
    g2.pack_start(&t2, false, false, 0);
    v.pack_start(&group("Installation settings", &g2), false, false, 0);
    let g3 = vbox(6);
    let smooth = check("_Smooth edges of screen fonts", o.antialias);
    let hint = check("Use font _hinting", o.hinting);
    g3.pack_start(&smooth, false, false, 0);
    g3.pack_start(&hint, false, false, 0);
    let hr = hbox(8);
    hr.set_margin_start(24);
    hr.pack_start(&gtk::Label::new(Some("Hinting:")), false, false, 0);
    let style = combo(&[("hintslight", "Slight"), ("hintmedium", "Medium"), ("hintfull", "Full")], &o.hintstyle);
    hr.pack_start(&style, false, false, 0);
    g3.pack_start(&hr, false, false, 0);
    { let s = style.clone(); hint.connect_toggled(move |c| s.set_sensitive(c.is_active())); }
    style.set_sensitive(o.hinting);
    { let me = cp.clone(); g3.pack_start(&link("Adjust ClearType text", move || cleartype_tuner(&me)), false, false, 0); }
    v.pack_start(&group("Text rendering", &g3), false, false, 0);
    let restore = gtk::Button::with_mnemonic("_Restore default font settings");
    restore.set_halign(gtk::Align::Start);
    {
        let (lang, link_inst, smooth, hint, style) = (lang.clone(), link_inst.clone(), smooth.clone(), hint.clone(), style.clone());
        restore.connect_clicked(move |_| {
            let d = config::FontOptions::default();
            lang.set_active(d.hide_by_language);
            link_inst.set_active(d.install_as_shortcut);
            smooth.set_active(d.antialias);
            hint.set_active(d.hinting);
            style.set_active_id(Some(&d.hintstyle));
        });
    }
    v.pack_start(&restore, false, false, 0);
    let row = hbox(6);
    row.set_halign(gtk::Align::End);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    for b in [&ok, &cancel] { b.set_size_request(75, -1); row.pack_start(b, false, false, 0); }
    v.pack_start(&row, false, false, 6);
    { let back = back.clone(); ok.connect_clicked(move |_| {
        let before = config::get().fonts;
        config::update(|c| {
            c.fonts.hide_by_language = lang.is_active();
            c.fonts.install_as_shortcut = link_inst.is_active();
            c.fonts.antialias = smooth.is_active();
            c.fonts.hinting = hint.is_active();
            c.fonts.hintstyle = if hint.is_active() { style.active_id().map(|s| s.to_string()).unwrap_or_else(|| "hintfull".into()) } else { "hintnone".into() };
        });
        let after = config::get().fonts;
        if (before.antialias, before.hinting, &before.hintstyle) != (after.antialias, after.hinting, &after.hintstyle) { write_xsettings(); }
        back(before.hide_by_language != after.hide_by_language);
    }); }
    { let b = back.clone(); cancel.connect_clicked(move |_| b(false)); }
    v.upcast()
}

// ------------------------------------------------------------------ ClearType Text Tuner

/// One text sample rendered with explicit font options (the tuner's clickable boxes).
fn sample_box(antialias: cairo::Antialias, order: cairo::SubpixelOrder, hint: cairo::HintStyle) -> gtk::DrawingArea {
    let a = gtk::DrawingArea::new();
    a.set_size_request(176, 128);
    a.connect_draw(move |a, cr| {
        let (w, h) = (a.allocated_width() as f64, a.allocated_height() as f64);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok();
        let mut fo = cairo::FontOptions::new().ok();
        if let Some(fo) = fo.as_mut() {
            fo.set_antialias(antialias);
            fo.set_subpixel_order(order);
            fo.set_hint_style(hint);
            fo.set_hint_metrics(cairo::HintMetrics::On);
        }
        let l = pangocairo::functions::create_layout(cr);
        pangocairo::functions::context_set_font_options(&l.context(), fo.as_ref());
        l.context_changed();
        l.set_font_description(Some(&pango::FontDescription::from_string("Segoe UI 9")));
        l.set_width(((w - 16.0) * pango::SCALE as f64) as i32);
        l.set_wrap(pango::WrapMode::Word);
        l.set_text(SAMPLE_TEXT);
        cr.set_source_rgb(0.0, 0.0, 0.0);
        cr.move_to(8.0, 8.0);
        pangocairo::functions::show_layout(cr, &l);
        let _ = h;
        glib::Propagation::Stop
    });
    a
}

pub(crate) fn cleartype_tuner(cp: &Rc<Cp>) {
    let o = config::get().fonts;
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_title("ClearType Text Tuner");
    w.set_transient_for(Some(&parent_window(cp)));
    w.set_modal(true);
    w.set_default_size(620, 470);
    w.set_icon_name(Some("preferences-desktop-font"));
    let root = vbox(0);
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None);
    // Win7's wizard pages are white above a grey button strip
    let white = gtk::EventBox::new();
    white.style_context().add_class("view");
    white.add(&stack);
    root.pack_start(&white, true, true, 0);
    let row = hbox(6);
    row.set_border_width(10);
    row.set_halign(gtk::Align::End);
    let next = gtk::Button::with_mnemonic("_Next");
    let cancel = gtk::Button::with_label("Cancel");
    for b in [&next, &cancel] { b.set_size_request(75, -1); row.pack_start(b, false, false, 0); }
    root.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
    root.pack_start(&row, false, false, 0);
    w.add(&root);
    let page = |head: &str| -> gtk::Box {
        let p = vbox(12);
        p.set_border_width(24);
        let h = gtk::Label::new(None);
        h.set_markup(&format!("<span size='large' foreground='#1e3287'>{}</span>", glib::markup_escape_text(head)));
        h.set_xalign(0.0);
        p.pack_start(&h, false, false, 0);
        p
    };
    // choices: (rgba, hintstyle)
    let choice = Rc::new(RefCell::new((if o.rgba == "none" { "rgb".to_string() } else { o.rgba.clone() }, o.hintstyle.clone())));
    let p0 = page("Make the text on your screen easier to read");
    p0.pack_start(&text("ClearType makes text easier to read on LCD screens by lighting the red, green and blue parts of each pixel separately. This wizard helps you choose the setting that looks best on your monitor."), false, false, 0);
    let on = check("Turn on _ClearType", o.rgba != "none" && o.antialias);
    p0.pack_start(&on, false, false, 8);
    stack.add_named(&p0, "0");
    let p1 = page("Windows is making sure that your monitor is set to its native resolution.");
    let res = text("Checking your monitor...");
    p1.pack_start(&res, false, false, 0);
    stack.add_named(&p1, "1");
    {
        let res = res.clone();
        util::output_async(vec!["xrandr".into(), "--query".into()], move |out| {
            let out = out.unwrap_or_default();
            let mut lines = out.lines();
            let mut msg = String::from("Your monitor is set to its native resolution.");
            while let Some(l) = lines.next() {
                if !l.contains(" connected") { continue; }
                let name = l.split_whitespace().next().unwrap_or("");
                let cur = l.split_whitespace().find(|w| w.contains('x') && w.contains('+')).map(|w| w.split('+').next().unwrap_or("").to_string()).unwrap_or_default();
                let native = lines.next().and_then(|m| m.split_whitespace().next()).unwrap_or("").to_string();
                msg += &format!("\n\nDisplay name: {name}\nNative resolution: {native}");
                if !cur.is_empty() && !native.is_empty() && cur != native { msg = format!("Your monitor is not set to its native resolution ({native}). Text is sharpest at the native resolution.\n\nDisplay name: {name}\nCurrent resolution: {cur}"); }
                break;
            }
            res.set_text(&msg);
        });
    }
    // sample pages: each a row of boxes, the chosen one framed
    type Opt = (String, cairo::Antialias, cairo::SubpixelOrder, cairo::HintStyle);
    let hs = |s: &str| match s { "hintslight" => cairo::HintStyle::Slight, "hintmedium" => cairo::HintStyle::Medium, "hintnone" => cairo::HintStyle::None, _ => cairo::HintStyle::Full };
    let ord = |s: &str| if s == "bgr" { cairo::SubpixelOrder::Bgr } else { cairo::SubpixelOrder::Rgb };
    let mk_samples = {
        let choice = choice.clone();
        move |head: &str, opts: Vec<Opt>, field: usize| -> gtk::Box {
            let p = vbox(12);
            p.set_border_width(24);
            let h = gtk::Label::new(None);
            h.set_markup(&format!("<span size='large' foreground='#1e3287'>{}</span>", glib::markup_escape_text(head)));
            h.set_xalign(0.0);
            p.pack_start(&h, false, false, 0);
            let g = gtk::FlowBox::new();
            g.set_selection_mode(gtk::SelectionMode::Single);
            g.set_max_children_per_line(3);
            g.set_min_children_per_line(opts.len().min(3) as u32);
            g.set_valign(gtk::Align::Start);
            g.set_halign(gtk::Align::Center);
            g.set_activate_on_single_click(true);
            let current = { let c = choice.borrow(); if field == 0 { c.0.clone() } else { c.1.clone() } };
            for (id, aa, so, hst) in &opts {
                let b = sample_box(*aa, *so, *hst);
                let fr = gtk::Frame::new(None);
                fr.add(&b);
                g.add(&fr);
                if *id == current { if let Some(ch) = g.children().last().and_then(|c| c.clone().downcast::<gtk::FlowBoxChild>().ok()) { g.select_child(&ch); } }
            }
            if g.selected_children().is_empty() { if let Some(c) = g.child_at_index(0) { g.select_child(&c); } }
            let (choice, ids) = (choice.clone(), opts.iter().map(|o| o.0.clone()).collect::<Vec<_>>());
            g.connect_selected_children_changed(move |g| {
                if let Some(c) = g.selected_children().first() {
                    if let Some(id) = ids.get(c.index() as usize) { let mut ch = choice.borrow_mut(); if field == 0 { ch.0 = id.clone() } else { ch.1 = id.clone() } }
                }
            });
            p.pack_start(&g, false, false, 0);
            p
        }
    };
    let hint0 = choice.borrow().1.clone();
    let p2 = mk_samples("Click the text sample that looks best to you (1 of 2)", vec![
        ("rgb".into(), cairo::Antialias::Subpixel, cairo::SubpixelOrder::Rgb, hs(&hint0)),
        ("bgr".into(), cairo::Antialias::Subpixel, cairo::SubpixelOrder::Bgr, hs(&hint0)),
    ], 0);
    stack.add_named(&p2, "2");
    let p3_holder = vbox(0);
    stack.add_named(&p3_holder, "3");
    let p4 = page("You have finished tuning the text on your monitor.");
    p4.pack_start(&text("The settings take effect in programs you start from now on; programs that are already running pick them up when they are restarted."), false, false, 0);
    stack.add_named(&p4, "4");
    let step = Rc::new(Cell::new(0));
    {
        let (stack, step, on, choice, w2, next2, p3_holder) = (stack.clone(), step.clone(), on.clone(), choice.clone(), w.clone(), next.clone(), p3_holder.clone());
        let mk = mk_samples.clone();
        next.connect_clicked(move |_| {
            let s = step.get();
            // pages: 0 intro, 1 native resolution, 2 stripe order (ClearType only), 3 hinting, 4 done
            let n = match s {
                0 => 1,
                1 if on.is_active() => 2,
                1 | 2 => {
                    // the hinting samples use the stripe order picked on the page before
                    let (rgba, _) = choice.borrow().clone();
                    let (aa, so, head) = if on.is_active() { (cairo::Antialias::Subpixel, ord(&rgba), "Click the text sample that looks best to you (2 of 2)") }
                        else { (cairo::Antialias::Gray, cairo::SubpixelOrder::Default, "Click the text sample that looks best to you (1 of 1)") };
                    for c in p3_holder.children() { p3_holder.remove(&c); }
                    let p3 = mk(head, vec![
                        ("hintslight".into(), aa, so, cairo::HintStyle::Slight),
                        ("hintmedium".into(), aa, so, cairo::HintStyle::Medium),
                        ("hintfull".into(), aa, so, cairo::HintStyle::Full),
                    ], 1);
                    p3_holder.pack_start(&p3, true, true, 0);
                    p3_holder.show_all();
                    3
                }
                3 => 4,
                _ => {
                    let (rgba, hint) = choice.borrow().clone();
                    let ct = on.is_active();
                    config::update(|c| {
                        c.fonts.antialias = true;
                        c.fonts.rgba = if ct { rgba } else { "none".into() };
                        c.fonts.hinting = true;
                        c.fonts.hintstyle = hint;
                    });
                    write_xsettings();
                    w2.close();
                    return;
                }
            };
            step.set(n);
            stack.set_visible_child_name(&n.to_string());
            next2.set_label(if n == 4 { "Finish" } else { "_Next" });
            next2.set_use_underline(true);
        });
    }
    { let w2 = w.clone(); cancel.connect_clicked(move |_| w2.close()); }
    w.show_all();
}
