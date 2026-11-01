//! Notepad – Windows 7 Notepad remake.
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use aero7::{config, sounds, theme, util};

#[derive(Clone, Copy, PartialEq)]
enum Enc { Utf8, Utf8Bom, Utf16Le, Utf16Be, Ansi }

impl Enc {
    fn label(self) -> &'static str {
        match self { Enc::Ansi => "ANSI", Enc::Utf16Le => "Unicode", Enc::Utf16Be => "Unicode big endian", Enc::Utf8 | Enc::Utf8Bom => "UTF-8" }
    }
}

struct State {
    path: Option<PathBuf>,
    enc: Enc,
    dirty: bool,
    find: String,
    replace: String,
    match_case: bool,
    down: bool,
}

struct App {
    win: gtk::ApplicationWindow,
    view: gtk::TextView,
    buf: gtk::TextBuffer,
    status: gtk::Label,
    statusbar: gtk::Box,
    wrap: gtk::CheckMenuItem,
    css: gtk::CssProvider,
    st: RefCell<State>,
}

fn decode(bytes: &[u8]) -> (String, Enc) {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { return (String::from_utf8_lossy(&bytes[3..]).into_owned(), Enc::Utf8Bom); }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return (String::from_utf16_lossy(&u), Enc::Utf16Le);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return (String::from_utf16_lossy(&u), Enc::Utf16Be);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Enc::Utf8),
        // "ANSI": treat as Windows-1252 / Latin-1
        Err(_) => (bytes.iter().map(|b| *b as char).collect(), Enc::Ansi),
    }
}

fn encode(text: &str, enc: Enc) -> Vec<u8> {
    let text = text.replace('\n', "\r\n").replace("\r\r\n", "\r\n");
    match enc {
        Enc::Utf8 => text.into_bytes(),
        Enc::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
        Enc::Utf16Le => { let mut v = vec![0xFF, 0xFE]; for u in text.encode_utf16() { v.extend(u.to_le_bytes()); } v }
        Enc::Utf16Be => { let mut v = vec![0xFE, 0xFF]; for u in text.encode_utf16() { v.extend(u.to_be_bytes()); } v }
        Enc::Ansi => text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect(),
    }
}

impl App {
    fn title_name(&self) -> String {
        self.st.borrow().path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into())
    }

    fn update_title(&self) {
        self.win.set_title(&format!("{} - Notepad", self.title_name()));
    }

    fn text(&self) -> String {
        let (s, e) = self.buf.bounds();
        self.buf.text(&s, &e, true).map(|g| g.to_string()).unwrap_or_default()
    }

    fn set_font(&self, desc: &str) {
        let fd = gtk::pango::FontDescription::from_string(desc);
        let fam = fd.family().map(|f| f.to_string()).unwrap_or_else(|| "Lucida Console".into());
        let size = fd.size() as f64 / gtk::pango::SCALE as f64;
        let weight = if fd.weight() == gtk::pango::Weight::Bold { "bold" } else { "normal" };
        let style = if fd.style() == gtk::pango::Style::Italic { "italic" } else { "normal" };
        let css = format!("textview, textview text {{ font-family: \"{fam}\", \"Lucida Console\", \"Consolas\", monospace; font-size: {}pt; font-weight: {weight}; font-style: {style}; }}", if size > 0.0 { size } else { 10.0 });
        let _ = self.css.load_from_data(css.as_bytes());
        let _ = std::fs::create_dir_all(config::conf_dir());
        let _ = std::fs::write(config::conf_dir().join("notepad-font"), desc);
    }

    fn update_status(&self) {
        let it = self.buf.iter_at_mark(&self.buf.get_insert().unwrap());
        self.status.set_text(&format!("Ln {}, Col {}", it.line() + 1, it.line_offset() + 1));
    }

    /// Win7 asks before discarding changes. Returns false if the user cancelled.
    fn maybe_save(self: &Rc<Self>) -> bool {
        if !self.st.borrow().dirty { return true; }
        let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Other, gtk::ButtonsType::None, "");
        d.set_title("Notepad");
        d.set_markup(&format!("<span size='large' foreground='#003399'>Do you want to save changes to {}?</span>",
            glib::markup_escape_text(&self.st.borrow().path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Untitled".into()))));
        d.add_button("Save", gtk::ResponseType::Yes);
        d.add_button("Don't Save", gtk::ResponseType::No);
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        d.set_default_response(gtk::ResponseType::Yes);
        let r = d.run();
        d.close();
        match r {
            gtk::ResponseType::Yes => self.save(false),
            gtk::ResponseType::No => true,
            _ => false,
        }
    }

    fn load(self: &Rc<Self>, p: PathBuf) {
        match std::fs::read(&p) {
            Ok(bytes) => {
                let (mut text, enc) = decode(&bytes);
                text = text.replace("\r\n", "\n");
                // ".LOG" files get a time stamp appended on every open
                let log = text.starts_with(".LOG");
                if log { text.push_str(&format!("\n{}\n", chrono::Local::now().format("%-I:%M %p %-m/%-d/%Y"))); }
                self.buf.set_text(&text);
                let (s, e) = if log { (self.buf.end_iter(), self.buf.end_iter()) } else { (self.buf.start_iter(), self.buf.start_iter()) };
                self.buf.select_range(&s, &e);
                let mut st = self.st.borrow_mut();
                st.path = Some(p.clone());
                st.enc = enc;
                st.dirty = log;
                drop(st);
                if let Some(rm) = gtk::RecentManager::default() { rm.add_item(&gio::File::for_path(&p).uri()); }
            }
            Err(e) => {
                sounds::play(sounds::Event::Exclamation);
                let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::Ok,
                    &format!("{}\nCannot find the {} file.\n\n{}", p.display(), p.display(), e));
                d.run();
                d.close();
            }
        }
        self.update_title();
    }

    fn save(self: &Rc<Self>, save_as: bool) -> bool {
        let path = self.st.borrow().path.clone();
        let (path, enc) = match (path, save_as) {
            (Some(p), false) => (p, self.st.borrow().enc),
            _ => match self.save_dialog() { Some(x) => x, None => return false },
        };
        match std::fs::write(&path, encode(&self.text(), enc)) {
            Ok(_) => {
                let mut st = self.st.borrow_mut();
                st.path = Some(path);
                st.enc = enc;
                st.dirty = false;
                drop(st);
                self.update_title();
                true
            }
            Err(e) => { util::error(&format!("Cannot save {}", path.display()), &e.to_string()); false }
        }
    }

    fn save_dialog(&self) -> Option<(PathBuf, Enc)> {
        let fc = gtk::FileChooserDialog::with_buttons(Some("Save As"), Some(&self.win), gtk::FileChooserAction::Save,
            &[("Save", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
        fc.set_do_overwrite_confirmation(true);
        let f = gtk::FileFilter::new(); f.set_name(Some("Text Documents (*.txt)")); f.add_pattern("*.txt"); fc.add_filter(f.clone());
        let a = gtk::FileFilter::new(); a.set_name(Some("All Files")); a.add_pattern("*"); fc.add_filter(a.clone());
        let enc = gtk::ComboBoxText::new();
        for e in [Enc::Ansi, Enc::Utf16Le, Enc::Utf16Be, Enc::Utf8] { enc.append_text(e.label()); }
        enc.set_active(Some(match self.st.borrow().enc { Enc::Ansi => 0, Enc::Utf16Le => 1, Enc::Utf16Be => 2, _ => 3 }));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.pack_start(&gtk::Label::new(Some("Encoding:")), false, false, 0);
        row.pack_start(&enc, false, false, 0);
        row.show_all();
        fc.set_extra_widget(&row);
        match self.st.borrow().path.as_ref() {
            Some(p) => { let _ = fc.set_filename(p); }
            None => { fc.set_current_name("*.txt"); let _ = fc.set_current_folder(util::special_dir(glib::UserDirectory::Documents, "Documents")); }
        }
        let r = fc.run();
        let mut p = fc.filename();
        fc.close();
        if r != gtk::ResponseType::Accept { return None; }
        // Notepad adds .txt when the "Text Documents" filter is active and no extension was typed
        if let Some(pp) = &mut p { if pp.extension().is_none() { pp.set_extension("txt"); } }
        let e = match enc.active() { Some(0) => Enc::Ansi, Some(1) => Enc::Utf16Le, Some(2) => Enc::Utf16Be, _ => Enc::Utf8 };
        p.map(|p| (p, e))
    }

    fn open(self: &Rc<Self>) {
        if !self.maybe_save() { return; }
        let fc = gtk::FileChooserDialog::with_buttons(Some("Open"), Some(&self.win), gtk::FileChooserAction::Open,
            &[("Open", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
        let f = gtk::FileFilter::new(); f.set_name(Some("Text Documents (*.txt)")); f.add_pattern("*.txt"); fc.add_filter(f.clone());
        let a = gtk::FileFilter::new(); a.set_name(Some("All Files (*.*)")); a.add_pattern("*"); fc.add_filter(a.clone());
        let _ = fc.set_current_folder(util::special_dir(glib::UserDirectory::Documents, "Documents"));
        let r = fc.run();
        let p = fc.filename();
        fc.close();
        if r == gtk::ResponseType::Accept { if let Some(p) = p { self.load(p); } }
    }

    fn find_next(self: &Rc<Self>) -> bool {
        let (needle, case, down) = { let s = self.st.borrow(); (s.find.clone(), s.match_case, s.down) };
        if needle.is_empty() { self.find_dialog(false); return false; }
        let flags = if case { gtk::TextSearchFlags::empty() } else { gtk::TextSearchFlags::CASE_INSENSITIVE };
        let (start, end) = self.buf.selection_bounds().unwrap_or_else(|| { let i = self.buf.iter_at_mark(&self.buf.get_insert().unwrap()); (i.clone(), i) });
        let hit = if down { end.forward_search(&needle, flags, None) } else { start.backward_search(&needle, flags, None) };
        match hit {
            Some((a, b)) => {
                self.buf.select_range(&a, &b);
                self.view.scroll_to_iter(&mut a.clone(), 0.1, false, 0.0, 0.0);
                true
            }
            None => {
                sounds::play(sounds::Event::Asterisk);
                let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::Ok,
                    &format!("Cannot find \"{needle}\""));
                d.set_title("Notepad");
                d.run();
                d.close();
                false
            }
        }
    }

    fn find_dialog(self: &Rc<Self>, replace: bool) {
        let d = gtk::Dialog::new();
        d.set_title(if replace { "Replace" } else { "Find" });
        d.set_transient_for(Some(&self.win));
        d.set_resizable(false);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(8);
        grid.set_column_spacing(8);
        grid.set_border_width(10);
        let what = gtk::Entry::new();
        if let Some((a, b)) = self.buf.selection_bounds() { what.set_text(&self.buf.text(&a, &b, false).unwrap_or_default()); } else { what.set_text(&self.st.borrow().find); }
        let with = gtk::Entry::new();
        with.set_text(&self.st.borrow().replace);
        grid.attach(&gtk::Label::new(Some("Find what:")), 0, 0, 1, 1);
        grid.attach(&what, 1, 0, 1, 1);
        if replace { grid.attach(&gtk::Label::new(Some("Replace with:")), 0, 1, 1, 1); grid.attach(&with, 1, 1, 1, 1); }
        let case = gtk::CheckButton::with_label("Match case");
        case.set_active(self.st.borrow().match_case);
        grid.attach(&case, 0, 3, 2, 1);
        let dir = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let up = gtk::RadioButton::with_label("Up");
        let down = gtk::RadioButton::with_label_from_widget(&up, "Down");
        if self.st.borrow().down { down.set_active(true) } else { up.set_active(true) }
        dir.pack_start(&gtk::Label::new(Some("Direction")), false, false, 0);
        dir.pack_start(&up, false, false, 0);
        dir.pack_start(&down, false, false, 0);
        if !replace { grid.attach(&dir, 1, 3, 1, 1); }
        let btns = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let next = gtk::Button::with_label("Find Next");
        btns.pack_start(&next, false, false, 0);
        let (rep, rep_all) = (gtk::Button::with_label("Replace"), gtk::Button::with_label("Replace All"));
        if replace { btns.pack_start(&rep, false, false, 0); btns.pack_start(&rep_all, false, false, 0); }
        let cancel = gtk::Button::with_label("Cancel");
        btns.pack_start(&cancel, false, false, 0);
        grid.attach(&btns, 2, 0, 1, 4);
        d.content_area().add(&grid);
        let sync = {
            let (app, what, with, case, down) = (self.clone(), what.clone(), with.clone(), case.clone(), down.clone());
            move || {
                let mut s = app.st.borrow_mut();
                s.find = what.text().to_string();
                s.replace = with.text().to_string();
                s.match_case = case.is_active();
                s.down = down.is_active() || replace;
            }
        };
        let sync = Rc::new(sync);
        { let (a, s) = (self.clone(), sync.clone()); next.connect_clicked(move |_| { s(); a.find_next(); }); }
        { let (a, s) = (self.clone(), sync.clone()); what.connect_activate(move |_| { s(); a.find_next(); }); }
        {
            let (a, s) = (self.clone(), sync.clone());
            rep.connect_clicked(move |_| {
                s();
                let rw = a.st.borrow().replace.clone();
                if let Some((mut x, mut y)) = a.buf.selection_bounds() {
                    let sel = a.buf.text(&x, &y, false).map(|g| g.to_string()).unwrap_or_default();
                    let st = a.st.borrow();
                    let eq = if st.match_case { sel == st.find } else { sel.to_lowercase() == st.find.to_lowercase() };
                    drop(st);
                    if eq { a.buf.delete(&mut x, &mut y); a.buf.insert_at_cursor(&rw); }
                }
                a.find_next();
            });
        }
        {
            let (a, s) = (self.clone(), sync.clone());
            rep_all.connect_clicked(move |_| {
                s();
                let st = a.st.borrow();
                let (f, r, case) = (st.find.clone(), st.replace.clone(), st.match_case);
                drop(st);
                if f.is_empty() { return; }
                let t = a.text();
                let out = if case { t.replace(&f, &r) } else {
                    let (lt, lf) = (t.to_lowercase(), f.to_lowercase());
                    let mut o = String::new();
                    let mut i = 0;
                    while let Some(pos) = lt[i..].find(&lf) { o.push_str(&t[i..i + pos]); o.push_str(&r); i += pos + f.len(); }
                    o.push_str(&t[i..]);
                    o
                };
                a.buf.set_text(&out);
            });
        }
        { let d2 = d.clone(); cancel.connect_clicked(move |_| d2.close()); }
        d.show_all();
    }

    fn goto_dialog(self: &Rc<Self>) {
        let d = gtk::Dialog::with_buttons(Some("Go To Line"), Some(&self.win), gtk::DialogFlags::MODAL,
            &[("Go To", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
        let c = d.content_area();
        c.set_border_width(10);
        c.set_spacing(6);
        let l = gtk::Label::new(Some("Line number:"));
        l.set_xalign(0.0);
        let e = gtk::SpinButton::with_range(1.0, (self.buf.line_count()).max(1) as f64, 1.0);
        e.set_value((self.buf.iter_at_mark(&self.buf.get_insert().unwrap()).line() + 1) as f64);
        e.set_activates_default(true);
        c.pack_start(&l, false, false, 0);
        c.pack_start(&e, false, false, 0);
        d.set_default_response(gtk::ResponseType::Ok);
        d.show_all();
        if d.run() == gtk::ResponseType::Ok {
            let line = e.value() as i32 - 1;
            if line >= self.buf.line_count() {
                util::error("The line number is beyond the total number of lines", "");
            } else { let it = self.buf.iter_at_line(line);
                self.buf.place_cursor(&it);
                self.view.scroll_to_iter(&mut it.clone(), 0.1, false, 0.0, 0.0);
            }
        }
        d.close();
    }

    fn print(self: &Rc<Self>) {
        let op = gtk::PrintOperation::new();
        let text = self.text();
        let name = self.title_name();
        let lines: Rc<RefCell<Vec<String>>> = Default::default();
        let per_page = Rc::new(std::cell::Cell::new(60));
        {
            let (lines, per_page, text) = (lines.clone(), per_page.clone(), text.clone());
            op.connect_begin_print(move |op, ctx| {
                let h = ctx.height() - 72.0;
                let pp = ((h / 13.0) as usize).max(1);
                per_page.set(pp);
                *lines.borrow_mut() = text.lines().map(String::from).collect();
                op.set_n_pages(((lines.borrow().len() + pp - 1) / pp).max(1) as i32);
            });
        }
        op.connect_draw_page(move |_, ctx, page| {
            let Some(cr) = ctx.cairo_context() else { return };
            cr.select_font_face("Lucida Console", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(10.0);
            cr.set_source_rgb(0.0, 0.0, 0.0);
            // header = file name, footer = page number (Win7 defaults "&f" / "Page &p")
            cr.move_to(ctx.width() / 2.0 - 40.0, 12.0);
            let _ = cr.show_text(&name);
            let pp = per_page.get();
            for (i, l) in lines.borrow().iter().skip(page as usize * pp).take(pp).enumerate() {
                cr.move_to(0.0, 36.0 + i as f64 * 13.0);
                let _ = cr.show_text(l);
            }
            cr.move_to(ctx.width() / 2.0 - 20.0, ctx.height() - 6.0);
            let _ = cr.show_text(&format!("Page {}", page + 1));
        });
        let _ = op.run(gtk::PrintOperationAction::PrintDialog, Some(&self.win));
    }
}

fn menu_item(menu: &gtk::Menu, label: &str, accel: Option<(&gtk::AccelGroup, &str)>, f: impl Fn() + 'static) -> gtk::MenuItem {
    let it = gtk::MenuItem::with_mnemonic(label);
    if let Some((g, a)) = accel {
        let (k, m) = gtk::accelerator_parse(a);
        it.add_accelerator("activate", g, k, m, gtk::AccelFlags::VISIBLE);
    }
    it.connect_activate(move |_| f());
    menu.append(&it);
    it
}

fn build(app: &gtk::Application, file: Option<PathBuf>) {
    theme::init_app();
    let win = gtk::ApplicationWindow::new(app);
    win.set_default_size(800, 560);
    win.set_icon_name(Some(util::first_icon(&["accessories-text-editor", "text-editor"])));
    let accel = gtk::AccelGroup::new();
    win.add_accel_group(&accel);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menubar = gtk::MenuBar::new();
    vbox.pack_start(&menubar, false, false, 0);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    let view = gtk::TextView::new();
    view.set_left_margin(2);
    view.set_top_margin(1);
    sw.add(&view);
    vbox.pack_start(&sw, true, true, 0);
    let statusbar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    statusbar.style_context().add_class("statusbar");
    let status = gtk::Label::new(Some("Ln 1, Col 1"));
    status.set_width_chars(22);
    status.set_xalign(0.0);
    statusbar.pack_end(&status, false, false, 0);
    statusbar.pack_end(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 0);
    vbox.pack_start(&statusbar, false, false, 0);
    win.add(&vbox);
    let css = gtk::CssProvider::new();
    view.style_context().add_provider(&css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
    let wrap = gtk::CheckMenuItem::with_mnemonic("_Word Wrap");
    let a = Rc::new(App {
        win: win.clone(), buf: view.buffer().unwrap(), view: view.clone(), status, statusbar: statusbar.clone(), wrap: wrap.clone(), css,
        st: RefCell::new(State { path: None, enc: Enc::Utf8, dirty: false, find: String::new(), replace: String::new(), match_case: false, down: true }),
    });
    a.set_font(&std::fs::read_to_string(config::conf_dir().join("notepad-font")).unwrap_or_else(|_| "Lucida Console 10".into()));

    // ---- File
    let m = gtk::Menu::new();
    { let a = a.clone(); menu_item(&m, "_New", Some((&accel, "<Control>n")), move || { if a.maybe_save() { a.buf.set_text(""); let mut s = a.st.borrow_mut(); s.path = None; s.dirty = false; s.enc = Enc::Utf8; drop(s); a.update_title(); } }); }
    { let a = a.clone(); menu_item(&m, "_Open...", Some((&accel, "<Control>o")), move || a.open()); }
    { let a = a.clone(); menu_item(&m, "_Save", Some((&accel, "<Control>s")), move || { a.save(false); }); }
    { let a = a.clone(); menu_item(&m, "Save _As...", None, move || { a.save(true); }); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); menu_item(&m, "Page Set_up...", None, move || { let s = gtk::PageSetup::new(); let _ = gtk::print_run_page_setup_dialog(Some(&w), Some(&s), &gtk::PrintSettings::new()); }); }
    { let a = a.clone(); menu_item(&m, "_Print...", Some((&accel, "<Control>p")), move || a.print()); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); menu_item(&m, "E_xit", None, move || w.close()); }
    let top = gtk::MenuItem::with_mnemonic("_File"); top.set_submenu(Some(&m)); menubar.append(&top);

    // ---- Edit
    let m = gtk::Menu::new();
    let undo_stack: Rc<RefCell<Vec<String>>> = Default::default();
    let undo = { let (a, u) = (a.clone(), undo_stack.clone()); menu_item(&m, "_Undo", Some((&accel, "<Control>z")), move || {
        // Win7 Notepad has a single-level undo that toggles
        let cur = a.text();
        let prev = u.borrow_mut().pop();
        if let Some(p) = prev { a.buf.set_text(&p); u.borrow_mut().push(cur); }
    }) };
    m.append(&gtk::SeparatorMenuItem::new());
    { let v = view.clone(); menu_item(&m, "Cu_t", Some((&accel, "<Control>x")), move || v.emit_cut_clipboard()); }
    { let v = view.clone(); menu_item(&m, "_Copy", Some((&accel, "<Control>c")), move || v.emit_copy_clipboard()); }
    { let v = view.clone(); menu_item(&m, "_Paste", Some((&accel, "<Control>v")), move || v.emit_paste_clipboard()); }
    { let a = a.clone(); menu_item(&m, "De_lete", Some((&accel, "Delete")), move || { a.buf.delete_selection(true, true); }); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let a = a.clone(); menu_item(&m, "_Find...", Some((&accel, "<Control>f")), move || a.find_dialog(false)); }
    { let a = a.clone(); menu_item(&m, "Find _Next", Some((&accel, "F3")), move || { a.find_next(); }); }
    { let a = a.clone(); menu_item(&m, "_Replace...", Some((&accel, "<Control>h")), move || a.find_dialog(true)); }
    let goto = { let a = a.clone(); menu_item(&m, "_Go To...", Some((&accel, "<Control>g")), move || a.goto_dialog()) };
    m.append(&gtk::SeparatorMenuItem::new());
    { let a = a.clone(); menu_item(&m, "Select _All", Some((&accel, "<Control>a")), move || { let (s, e) = a.buf.bounds(); a.buf.select_range(&s, &e); }); }
    { let a = a.clone(); menu_item(&m, "Time/_Date", Some((&accel, "F5")), move || { a.buf.insert_at_cursor(&chrono::Local::now().format("%-I:%M %p %-m/%-d/%Y").to_string()); }); }
    let top = gtk::MenuItem::with_mnemonic("_Edit"); top.set_submenu(Some(&m)); menubar.append(&top);
    {
        let (a, undo) = (a.clone(), undo.clone());
        top.connect_activate(move |_| undo.set_sensitive(!a.st.borrow().path.is_none() || a.buf.char_count() > 0));
    }

    // ---- Format
    let m = gtk::Menu::new();
    m.append(&wrap);
    {
        let (a, goto) = (a.clone(), goto.clone());
        wrap.connect_toggled(move |w| {
            let on = w.is_active();
            a.view.set_wrap_mode(if on { gtk::WrapMode::WordChar } else { gtk::WrapMode::None });
            // like Win7: Go To and the status bar are unavailable with word wrap
            goto.set_sensitive(!on);
            a.statusbar.set_visible(!on && config_flag("notepad-status"));
        });
    }
    {
        let a = a.clone();
        menu_item(&m, "_Font...", None, move || {
            let d = gtk::FontChooserDialog::new(Some("Font"), Some(&a.win));
            if let Ok(f) = std::fs::read_to_string(config::conf_dir().join("notepad-font")) { d.set_font(&f); }
            if d.run() == gtk::ResponseType::Ok { if let Some(f) = d.font() { a.set_font(&f); } }
            d.close();
        });
    }
    let top = gtk::MenuItem::with_mnemonic("F_ormat"); top.set_submenu(Some(&m)); menubar.append(&top);

    // ---- View
    let m = gtk::Menu::new();
    let sb = gtk::CheckMenuItem::with_mnemonic("_Status Bar");
    sb.set_active(config_flag("notepad-status"));
    { let a = a.clone(); sb.connect_toggled(move |s| { set_flag("notepad-status", s.is_active()); a.statusbar.set_visible(s.is_active() && !a.wrap.is_active()); }); }
    m.append(&sb);
    let top = gtk::MenuItem::with_mnemonic("_View"); top.set_submenu(Some(&m)); menubar.append(&top);
    {
        let (sb, wrap) = (sb.clone(), wrap.clone());
        top.connect_activate(move |_| sb.set_sensitive(!wrap.is_active()));
    }

    // ---- Help
    let m = gtk::Menu::new();
    menu_item(&m, "View _Help", None, || { util::open_uri("https://wiki.archlinux.org/"); });
    m.append(&gtk::SeparatorMenuItem::new());
    {
        let w = win.clone();
        menu_item(&m, "_About Notepad", None, move || {
            let d = gtk::AboutDialog::new();
            d.set_transient_for(Some(&w));
            d.set_program_name("Notepad");
            d.set_comments(Some("Aero7 Notepad – a Windows 7 style text editor for Linux"));
            d.set_logo_icon_name(Some(util::first_icon(&["accessories-text-editor"])));
            d.run();
            d.close();
        });
    }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);

    // ---- behaviour
    {
        let (a, u) = (a.clone(), undo_stack.clone());
        let last = Rc::new(RefCell::new(String::new()));
        a.buf.connect_begin_user_action(move |b| {
            let (s, e) = b.bounds();
            let t = b.text(&s, &e, true).map(|g| g.to_string()).unwrap_or_default();
            if *last.borrow() != t { u.borrow_mut().clear(); u.borrow_mut().push(t.clone()); *last.borrow_mut() = t; }
        });
    }
    { let a2 = a.clone(); a.buf.connect_changed(move |_| { if !a2.st.borrow().dirty { a2.st.borrow_mut().dirty = true; } a2.update_status(); }); }
    { let a2 = a.clone(); a.buf.connect_mark_set(move |_, _, _| a2.update_status()); }
    { let a2 = a.clone(); win.connect_delete_event(move |_, _| if a2.maybe_save() { glib::Propagation::Proceed } else { glib::Propagation::Stop }); }
    // drag & drop a file onto Notepad
    view.drag_dest_add_uri_targets();
    {
        let a2 = a.clone();
        view.connect_drag_data_received(move |v, _, _, _, data, _, _| {
            if let Some(u) = data.uris().first() {
                if let Some(p) = gio::File::for_uri(u).path() { if a2.maybe_save() { a2.load(p); } }
                v.stop_signal_emission_by_name("drag-data-received");
            }
        });
    }
    vbox.show_all();
    statusbar.set_visible(config_flag("notepad-status"));
    if let Some(p) = file { a.load(p); } else { a.update_title(); }
    a.st.borrow_mut().dirty = false;
    win.show();
    let _ = gdk::Display::default();
}

fn config_flag(name: &str) -> bool { config::conf_dir().join(name).exists() }
fn set_flag(name: &str, on: bool) {
    let p = config::conf_dir().join(name);
    let _ = std::fs::create_dir_all(config::conf_dir());
    if on { let _ = std::fs::write(p, ""); } else { let _ = std::fs::remove_file(p); }
}

fn main() -> glib::ExitCode {
    let app = gtk::Application::new(Some("org.aero7.Notepad"), gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(|app| build(app, None));
    app.connect_open(|app, files, _| { for f in files { build(app, f.path()); } });
    app.run()
}
