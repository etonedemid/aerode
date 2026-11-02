//! Notepad – Windows 7 Notepad remake.
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use aerode::{config, sounds, theme, util};

/// The encodings of Win7's Save As dialog.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Enc {
    /// "ANSI" = the system code page, which is UTF-8 (without a byte order mark) on Linux
    Ansi,
    /// "ANSI" for a file that is not valid UTF-8: kept as 8-bit Windows-1252 so its bytes survive a save
    Legacy,
    /// "Unicode" = UTF-16 little endian with BOM
    Utf16Le,
    /// "Unicode big endian"
    Utf16Be,
    /// "UTF-8" = UTF-8 with the byte order mark, as Win7 writes it
    Utf8,
}

impl Enc {
    fn combo_index(self) -> u32 { match self { Enc::Ansi | Enc::Legacy => 0, Enc::Utf16Le => 1, Enc::Utf16Be => 2, Enc::Utf8 => 3 } }
}

/// Line ends: files keep the style they had (Linux files stay LF), new documents use LF.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Eol { Lf, CrLf }

/// Windows-1252 0x80..0x9F (the undefined five map to the C1 controls so they round-trip).
const CP1252: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F,
    0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

fn decode(bytes: &[u8]) -> (String, Enc) {
    let utf16 = |b: &[u8], le: bool| {
        let u: Vec<u16> = b.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&u)
    };
    if let Some(r) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) { return (String::from_utf8_lossy(r).into_owned(), Enc::Utf8); }
    if let Some(r) = bytes.strip_prefix(&[0xFF, 0xFE]) { return (utf16(r, true), Enc::Utf16Le); }
    if let Some(r) = bytes.strip_prefix(&[0xFE, 0xFF]) { return (utf16(r, false), Enc::Utf16Be); }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Enc::Ansi),
        Err(_) => (bytes.iter().map(|&b| if (0x80..0xA0).contains(&b) { char::from_u32(CP1252[b as usize - 0x80] as u32).unwrap_or('?') } else { b as char }).collect(), Enc::Legacy),
    }
}

/// Characters that cannot be stored in Windows-1252.
fn legacy_lossy(text: &str) -> bool { text.chars().any(|c| legacy_byte(c).is_none()) }

fn legacy_byte(c: char) -> Option<u8> {
    let u = c as u32;
    if u < 0x80 || (0xA0..0x100).contains(&u) { return Some(u as u8); }
    CP1252.iter().position(|v| *v as u32 == u).map(|i| 0x80 + i as u8)
}

fn encode(text: &str, enc: Enc, eol: Eol) -> Vec<u8> {
    let text = text.replace("\r\n", "\n");
    let text = if eol == Eol::CrLf { text.replace('\n', "\r\n") } else { text };
    match enc {
        Enc::Ansi => text.into_bytes(),
        Enc::Utf8 => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
        Enc::Utf16Le => { let mut v = vec![0xFF, 0xFE]; for u in text.encode_utf16() { v.extend(u.to_le_bytes()); } v }
        Enc::Utf16Be => { let mut v = vec![0xFE, 0xFF]; for u in text.encode_utf16() { v.extend(u.to_be_bytes()); } v }
        Enc::Legacy => text.chars().map(|c| legacy_byte(c).unwrap_or(b'?')).collect(),
    }
}

/// Load-time text clean-up: CRLF → LF for the buffer, NULs shown as spaces (as Win7 Notepad does,
/// and a GtkTextBuffer cannot hold them). Returns the line-end style to write back.
fn normalize(text: String) -> (String, Eol) {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let eol = if crlf > 0 && crlf >= lf { Eol::CrLf } else { Eol::Lf };
    (text.replace("\r\n", "\n").replace('\0', " "), eol)
}

/// Write `data` to `path` without risking the old contents: write a temporary file next to it and
/// rename it over the original (keeping its permissions); fall back to writing in place when the
/// folder is not writable. Symbolic links are followed so the link itself stays a link.
fn write_safely(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.notepad-{}", std::process::id()));
    // a read-only file stays read-only (renaming over it would bypass that)
    if target.exists() {
        let c = std::ffi::CString::new(target.as_os_str().as_encoded_bytes()).unwrap_or_default();
        if unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0 { return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)); }
    }
    let attempt = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        if let Ok(m) = std::fs::metadata(&target) {
            let _ = std::fs::set_permissions(&tmp, m.permissions());
            use std::os::unix::fs::MetadataExt;
            unsafe {
                let c = std::ffi::CString::new(tmp.as_os_str().as_encoded_bytes()).unwrap_or_default();
                libc::chown(c.as_ptr(), m.uid(), m.gid());
            }
        }
        std::fs::rename(&tmp, &target)
    })();
    match attempt {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            // e.g. a read-only folder with a writable file: write in place
            if e.kind() == std::io::ErrorKind::PermissionDenied || e.kind() == std::io::ErrorKind::ReadOnlyFilesystem {
                let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&target)?;
                f.write_all(data)?;
                f.sync_all()
            } else { Err(e) }
        }
    }
}

// ------------------------------------------------------------------ undo
/// One recorded change, in character offsets.
#[derive(Clone, Debug)]
enum Op { Ins(i32, String), Del(i32, String) }

/// Win7 Notepad has a single undo step that toggles (Undo twice = redo). A step is one user action,
/// with runs of typing or of Backspace/Delete merged like the Windows edit control does.
#[derive(Default)]
struct Undo {
    group: Vec<Op>,
    /// 0 none, 1 typing, 2 deleting, 3 anything else
    kind: u8,
    /// where the next keystroke must happen to continue the run
    end: i32,
    in_action: bool,
    fresh: bool,
    mute: bool,
}

impl Undo {
    fn record(&mut self, op: Op) {
        if self.mute { return; }
        let single = |s: &str| s.chars().count() == 1 && s != "\n";
        let first = self.fresh || !self.in_action;
        self.fresh = false;
        if !first {
            if let Op::Ins(at, s) = &op { if single(s) { self.kind = 1; self.end = at + 1; } else { self.kind = 3; } } else { self.kind = 3; }
            self.group.push(op);
            return;
        }
        match &op {
            Op::Ins(at, s) if single(s) => {
                if self.kind != 1 || *at != self.end { self.group.clear(); }
                self.kind = 1;
                self.end = at + 1;
            }
            Op::Del(at, s) if single(s) => {
                // Backspace deletes the char before the last one, Delete the one at the same place
                if self.kind != 2 || !(*at + 1 == self.end || *at == self.end) { self.group.clear(); }
                self.kind = 2;
                self.end = *at;
            }
            _ => { self.group.clear(); self.kind = 3; }
        }
        self.group.push(op);
    }

    fn reset(&mut self) { self.group.clear(); self.kind = 0; }
}

// ------------------------------------------------------------------ app
struct State {
    path: Option<PathBuf>,
    enc: Enc,
    eol: Eol,
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
    undo: RefCell<Undo>,
    find_dlg: RefCell<Option<gtk::Dialog>>,
    page_setup: RefCell<Option<gtk::PageSetup>>,
    print_settings: RefCell<Option<gtk::PrintSettings>>,
    loading: Cell<bool>,
}

fn message(parent: &impl IsA<gtk::Window>, kind: gtk::MessageType, text: &str) {
    // the icon's sound comes from util::message_dialog_sounds (error → Critical Stop…)
    let d = gtk::MessageDialog::new(Some(parent), gtk::DialogFlags::MODAL, kind, gtk::ButtonsType::Ok, text);
    d.set_title("Notepad");
    d.run();
    d.close();
}

impl App {
    fn title_name(&self) -> String {
        self.st.borrow().path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into())
    }

    fn update_title(&self) { self.win.set_title(&format!("{} - Notepad", self.title_name())); }

    fn text(&self) -> String {
        let (s, e) = self.buf.bounds();
        self.buf.text(&s, &e, true).map(|g| g.to_string()).unwrap_or_default()
    }

    fn cursor(&self) -> gtk::TextIter { self.buf.iter_at_mark(&self.buf.get_insert().unwrap()) }

    fn font_desc() -> String { std::fs::read_to_string(config::conf_dir().join("notepad-font")).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Lucida Console 10".into()) }

    fn set_font(&self, desc: &str) {
        let fd = pango::FontDescription::from_string(desc);
        let fam = fd.family().map(|f| f.to_string()).unwrap_or_else(|| "Lucida Console".into());
        let size = fd.size() as f64 / pango::SCALE as f64;
        let weight = if fd.weight() == pango::Weight::Bold { "bold" } else { "normal" };
        let style = if fd.style() == pango::Style::Italic { "italic" } else { "normal" };
        let css = format!("textview, textview text {{ font-family: \"{fam}\", \"Lucida Console\", \"Consolas\", monospace; font-size: {}pt; font-weight: {weight}; font-style: {style}; }}", if size > 0.0 { size } else { 10.0 });
        let _ = self.css.load_from_data(css.as_bytes());
        let _ = std::fs::create_dir_all(config::conf_dir());
        let _ = std::fs::write(config::conf_dir().join("notepad-font"), desc);
    }

    fn update_status(&self) {
        let it = self.cursor();
        self.status.set_text(&format!("Ln {}, Col {}", it.line() + 1, it.line_offset() + 1));
    }

    /// Replace the whole document (New / Open): not undoable, not a modification.
    fn set_document(&self, text: &str) {
        self.loading.set(true);
        self.undo.borrow_mut().mute = true;
        self.buf.set_text(text);
        self.undo.borrow_mut().mute = false;
        self.undo.borrow_mut().reset();
        self.loading.set(false);
    }

    /// Win7 asks before discarding changes. Returns false if the user cancelled (or saving failed).
    fn maybe_save(self: &Rc<Self>) -> bool {
        if !self.st.borrow().dirty { return true; }
        let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Other, gtk::ButtonsType::None, "");
        d.set_title("Notepad");
        let shown = self.st.borrow().path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Untitled".into());
        d.set_markup(&format!("<span size='large' foreground='#003399'>Do you want to save changes to {}?</span>", glib::markup_escape_text(&shown)));
        d.add_button("_Save", gtk::ResponseType::Yes);
        d.add_button("Do_n't Save", gtk::ResponseType::No);
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        d.set_default_response(gtk::ResponseType::Yes);
        // task-dialog style: compact buttons at the bottom right
        if let Some(bb) = d.widget_for_response(gtk::ResponseType::Yes).and_then(|b| b.parent()).and_then(|p| p.downcast::<gtk::ButtonBox>().ok()) {
            bb.set_layout(gtk::ButtonBoxStyle::End);
            bb.set_homogeneous(true);
            for c in bb.children() { c.set_size_request(86, -1); c.set_hexpand(false); }
        }
        let r = d.run();
        d.close();
        match r {
            gtk::ResponseType::Yes => self.save(false),
            gtk::ResponseType::No => true,
            _ => false,
        }
    }

    fn new_doc(self: &Rc<Self>) {
        if !self.maybe_save() { return; }
        self.set_document("");
        let mut s = self.st.borrow_mut();
        s.path = None;
        s.dirty = false;
        s.enc = Enc::Ansi;
        s.eol = Eol::Lf;
        drop(s);
        self.update_title();
    }

    /// Open a file. `from_cmdline`: a missing file offers to create it (Win7: "Do you want to create a
    /// new file?"); returns false when the user chose Cancel there.
    fn load(self: &Rc<Self>, p: PathBuf, from_cmdline: bool) -> bool {
        if from_cmdline && !p.exists() {
            let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::None,
                &format!("Cannot find the {} file.\n\nDo you want to create a new file?", p.display()));
            d.add_buttons(&[("_Yes", gtk::ResponseType::Yes), ("_No", gtk::ResponseType::No), ("Cancel", gtk::ResponseType::Cancel)]);
            d.set_default_response(gtk::ResponseType::Yes);
            d.set_title("Notepad");
            let r = d.run();
            d.close();
            match r {
                gtk::ResponseType::Yes => {
                    if let Err(e) = std::fs::write(&p, b"") { message(&self.win, gtk::MessageType::Warning, &format!("{}\n{}", p.display(), win_err(&e))); return true; }
                }
                gtk::ResponseType::No => return true,
                _ => return false,
            }
        }
        if p.is_dir() { message(&self.win, gtk::MessageType::Warning, &format!("{}\nAccess is denied.", p.display())); return true; }
        match std::fs::read(&p) {
            Ok(bytes) => {
                let (text, enc) = decode(&bytes);
                let (mut text, eol) = normalize(text);
                // ".LOG" files get a time stamp appended on every open
                let log = text.starts_with(".LOG");
                if log { if !text.ends_with('\n') { text.push('\n'); } text.push_str(&format!("{}\n", time_date())); }
                self.set_document(&text);
                let it = if log { self.buf.end_iter() } else { self.buf.start_iter() };
                self.buf.place_cursor(&it);
                let mut st = self.st.borrow_mut();
                st.path = Some(p.clone());
                st.enc = enc;
                st.eol = eol;
                st.dirty = log;
                drop(st);
                if log { self.view.scroll_to_mark(&self.buf.get_insert().unwrap(), 0.0, false, 0.0, 0.0); }
                if let Some(rm) = gtk::RecentManager::default() { rm.add_item(&gio::File::for_path(&p).uri()); }
            }
            Err(e) => {
                let why = match e.kind() { std::io::ErrorKind::NotFound => format!("Cannot find the {} file.", p.display()), _ => win_err(&e) };
                message(&self.win, gtk::MessageType::Warning, &format!("{}\n{}", p.display(), why));
            }
        }
        self.update_title();
        self.update_status();
        true
    }

    fn save(self: &Rc<Self>, save_as: bool) -> bool {
        let path = self.st.borrow().path.clone();
        let (path, enc) = match (path, save_as) {
            (Some(p), false) => (p, self.st.borrow().enc),
            _ => match self.save_dialog() { Some(x) => x, None => return false },
        };
        let text = self.text();
        if enc == Enc::Legacy && legacy_lossy(&text) {
            let d = gtk::MessageDialog::new(Some(&self.win), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::OkCancel,
                "This file contains characters in Unicode format which will be lost if you save this file as an ANSI encoded text file. To keep the Unicode information, click Cancel below and then select one of the Unicode options from the Encoding drop down list. Continue?");
            d.set_title("Notepad");
            let r = d.run();
            d.close();
            if r != gtk::ResponseType::Ok { return false; }
        }
        let eol = self.st.borrow().eol;
        match write_safely(&path, &encode(&text, enc, eol)) {
            Ok(()) => {
                let mut st = self.st.borrow_mut();
                st.path = Some(path.clone());
                st.enc = enc;
                st.dirty = false;
                drop(st);
                self.update_title();
                if let Some(rm) = gtk::RecentManager::default() { rm.add_item(&gio::File::for_path(&path).uri()); }
                true
            }
            Err(e) => {
                // the document stays modified, so closing asks again – nothing is lost
                let why = match e.kind() {
                    std::io::ErrorKind::PermissionDenied => format!("Access to {} was denied.", path.display()),
                    _ => format!("{}\n{}", path.display(), win_err(&e)),
                };
                message(&self.win, gtk::MessageType::Warning, &format!("Notepad could not save the file.\n\n{why}"));
                false
            }
        }
    }

    fn save_dialog(&self) -> Option<(PathBuf, Enc)> {
        let fc = gtk::FileChooserDialog::with_buttons(Some("Save As"), Some(&self.win), gtk::FileChooserAction::Save,
            &[("_Save", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
        fc.set_do_overwrite_confirmation(true);
        fc.set_default_size(780, 520);
        let txt = gtk::FileFilter::new(); txt.set_name(Some("Text Documents (*.txt)")); txt.add_pattern("*.txt"); txt.add_pattern("*.TXT"); fc.add_filter(txt.clone());
        let all = gtk::FileFilter::new(); all.set_name(Some("All Files (*.*)")); all.add_pattern("*"); fc.add_filter(all.clone());
        let enc = gtk::ComboBoxText::new();
        for l in ["ANSI", "Unicode", "Unicode big endian", "UTF-8"] { enc.append_text(l); }
        let cur = self.st.borrow().enc;
        enc.set_active(Some(cur.combo_index()));
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.pack_start(&gtk::Label::new(Some("Encoding:")), false, false, 0);
        row.pack_start(&enc, false, false, 0);
        row.show_all();
        fc.set_extra_widget(&row);
        match self.st.borrow().path.as_ref() {
            Some(p) => {
                let _ = fc.set_filename(p);
                if p.extension().map(|e| !e.eq_ignore_ascii_case("txt")).unwrap_or(true) { fc.set_filter(&all); }
            }
            None => { fc.set_current_name("*.txt"); let _ = fc.set_current_folder(util::special_dir(glib::UserDirectory::Documents, "Documents")); }
        }
        loop {
            let r = fc.run();
            if r != gtk::ResponseType::Accept { fc.close(); return None; }
            let Some(mut p) = fc.filename() else { continue };
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // "*.txt" in the name box is a filter in Windows, not a name
            if name.contains('*') || name.contains('?') || name.is_empty() { fc.set_current_name("*.txt"); continue; }
            // Notepad adds .txt when "Text Documents" is the type and no extension was typed
            let is_txt_filter = fc.filter().map(|f| f == txt).unwrap_or(true);
            if is_txt_filter && p.extension().is_none() {
                p.set_extension("txt");
                if p.exists() {
                    let d = gtk::MessageDialog::new(Some(&fc), gtk::DialogFlags::MODAL, gtk::MessageType::Warning, gtk::ButtonsType::YesNo,
                        &format!("{} already exists.\nDo you want to replace it?", p.file_name().unwrap().to_string_lossy()));
                    d.set_title("Confirm Save As");
                    let r = d.run();
                    d.close();
                    if r != gtk::ResponseType::Yes { continue; }
                }
            }
            fc.close();
            let e = match enc.active() {
                Some(0) => if cur == Enc::Legacy { Enc::Legacy } else { Enc::Ansi },
                Some(1) => Enc::Utf16Le, Some(2) => Enc::Utf16Be, _ => Enc::Utf8,
            };
            return Some((p, e));
        }
    }

    fn open(self: &Rc<Self>) {
        if !self.maybe_save() { return; }
        let fc = gtk::FileChooserDialog::with_buttons(Some("Open"), Some(&self.win), gtk::FileChooserAction::Open,
            &[("_Open", gtk::ResponseType::Accept), ("Cancel", gtk::ResponseType::Cancel)]);
        fc.set_default_size(780, 520);
        let f = gtk::FileFilter::new(); f.set_name(Some("Text Documents (*.txt)")); f.add_pattern("*.txt"); f.add_pattern("*.TXT"); fc.add_filter(f.clone());
        let a = gtk::FileFilter::new(); a.set_name(Some("All Files (*.*)")); a.add_pattern("*"); fc.add_filter(a.clone());
        let dir = self.st.borrow().path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| util::special_dir(glib::UserDirectory::Documents, "Documents"));
        let _ = fc.set_current_folder(dir);
        let r = fc.run();
        let p = fc.filename();
        fc.close();
        if r == gtk::ResponseType::Accept { if let Some(p) = p { self.load(p, false); } }
    }

    fn find_next(self: &Rc<Self>) -> bool {
        let (needle, case, down) = { let s = self.st.borrow(); (s.find.clone(), s.match_case, s.down) };
        if needle.is_empty() { self.find_dialog(false); return false; }
        let flags = gtk::TextSearchFlags::TEXT_ONLY | if case { gtk::TextSearchFlags::empty() } else { gtk::TextSearchFlags::CASE_INSENSITIVE };
        let (start, end) = self.buf.selection_bounds().unwrap_or_else(|| { let i = self.cursor(); (i.clone(), i) });
        let hit = if down { end.forward_search(&needle, flags, None) } else { start.backward_search(&needle, flags, None) };
        match hit {
            Some((a, b)) => {
                self.buf.select_range(&a, &b);
                self.view.scroll_to_iter(&mut a.clone(), 0.1, false, 0.0, 0.0);
                true
            }
            None => {
                let parent: gtk::Window = self.find_dlg.borrow().clone().map(|d| d.upcast()).unwrap_or_else(|| self.win.clone().upcast());
                message(&parent, gtk::MessageType::Info, &format!("Cannot find \"{needle}\""));
                false
            }
        }
    }

    /// Replace every match in one undo step; returns how many were replaced.
    fn replace_all(&self, find: &str, with: &str, case: bool) -> usize {
        if find.is_empty() { return 0; }
        let flags = gtk::TextSearchFlags::TEXT_ONLY | if case { gtk::TextSearchFlags::empty() } else { gtk::TextSearchFlags::CASE_INSENSITIVE };
        let mut n = 0;
        self.buf.begin_user_action();
        let mut from = self.buf.start_iter().offset();
        loop {
            let it = self.buf.iter_at_offset(from);
            let Some((mut a, mut b)) = it.forward_search(find, flags, None) else { break };
            let at = a.offset();
            self.buf.delete(&mut a, &mut b);
            let mut ins = self.buf.iter_at_offset(at);
            self.buf.insert(&mut ins, with);
            from = at + with.chars().count() as i32;
            n += 1;
        }
        self.buf.end_user_action();
        n
    }

    fn find_dialog(self: &Rc<Self>, replace: bool) {
        // one Find/Replace window at a time (Find ↔ Replace switches)
        let old = self.find_dlg.borrow_mut().take();
        if let Some(d) = old { d.close(); }
        let d = gtk::Dialog::new();
        d.set_title(if replace { "Replace" } else { "Find" });
        d.set_transient_for(Some(&self.win));
        d.set_resizable(false);
        d.set_destroy_with_parent(true);
        d.set_type_hint(gdk::WindowTypeHint::Dialog);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(8);
        grid.set_column_spacing(8);
        grid.set_border_width(10);
        let what = gtk::Entry::new();
        what.set_width_chars(28);
        let sel = self.buf.selection_bounds().map(|(a, b)| self.buf.text(&a, &b, false).map(|g| g.to_string()).unwrap_or_default()).filter(|s| !s.contains('\n') && !s.is_empty());
        what.set_text(&sel.unwrap_or_else(|| self.st.borrow().find.clone()));
        let with = gtk::Entry::new();
        with.set_text(&self.st.borrow().replace);
        let l1 = gtk::Label::with_mnemonic("Fi_nd what:");
        l1.set_mnemonic_widget(Some(&what));
        l1.set_xalign(0.0);
        grid.attach(&l1, 0, 0, 1, 1);
        grid.attach(&what, 1, 0, 1, 1);
        if replace {
            let l2 = gtk::Label::with_mnemonic("Re_place with:");
            l2.set_mnemonic_widget(Some(&with));
            l2.set_xalign(0.0);
            grid.attach(&l2, 0, 1, 1, 1);
            grid.attach(&with, 1, 1, 1, 1);
        }
        let case = gtk::CheckButton::with_mnemonic("Match _case");
        case.set_active(self.st.borrow().match_case);
        case.set_valign(gtk::Align::End);
        grid.attach(&case, 0, 3, 1, 1);
        let up = gtk::RadioButton::with_mnemonic("_Up");
        let down = gtk::RadioButton::with_mnemonic_from_widget(&up, "_Down");
        if self.st.borrow().down { down.set_active(true) } else { up.set_active(true) }
        if !replace {
            let dir = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            dir.set_border_width(4);
            dir.pack_start(&up, false, false, 0);
            dir.pack_start(&down, false, false, 0);
            let fr = gtk::Frame::new(Some("Direction"));
            fr.add(&dir);
            fr.set_halign(gtk::Align::End);
            grid.attach(&fr, 1, 2, 1, 2);
        }
        let btns = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let next = gtk::Button::with_mnemonic("_Find Next");
        btns.pack_start(&next, false, false, 0);
        let (rep, rep_all) = (gtk::Button::with_mnemonic("_Replace"), gtk::Button::with_mnemonic("Replace _All"));
        if replace { btns.pack_start(&rep, false, false, 0); btns.pack_start(&rep_all, false, false, 0); }
        let cancel = gtk::Button::with_label("Cancel");
        btns.pack_start(&cancel, false, false, 0);
        grid.attach(&btns, 2, 0, 1, 4);
        d.content_area().add(&grid);
        let enable = {
            let (next, rep, rep_all) = (next.clone(), rep.clone(), rep_all.clone());
            move |e: &gtk::Entry| { let on = !e.text().is_empty(); next.set_sensitive(on); rep.set_sensitive(on); rep_all.set_sensitive(on); }
        };
        enable(&what);
        what.connect_changed(enable);
        let sync = {
            let (app, what, with, case, down) = (self.clone(), what.clone(), with.clone(), case.clone(), down.clone());
            Rc::new(move || {
                let mut s = app.st.borrow_mut();
                s.find = what.text().to_string();
                s.replace = with.text().to_string();
                s.match_case = case.is_active();
                // the Replace dialog always searches down
                s.down = down.is_active() || replace;
            })
        };
        { let (a, s) = (self.clone(), sync.clone()); next.connect_clicked(move |_| { s(); a.find_next(); }); }
        { let (a, s, n) = (self.clone(), sync.clone(), next.clone()); what.connect_activate(move |_| { if n.is_sensitive() { s(); a.find_next(); } }); }
        {
            let (a, s) = (self.clone(), sync.clone());
            rep.connect_clicked(move |_| {
                s();
                let (find, rw, mc) = { let st = a.st.borrow(); (st.find.clone(), st.replace.clone(), st.match_case) };
                if let Some((mut x, mut y)) = a.buf.selection_bounds() {
                    let sel = a.buf.text(&x, &y, false).map(|g| g.to_string()).unwrap_or_default();
                    let eq = if mc { sel == find } else { sel.to_lowercase() == find.to_lowercase() };
                    if eq {
                        a.buf.begin_user_action();
                        a.buf.delete(&mut x, &mut y);
                        a.buf.insert(&mut x, &rw);
                        a.buf.end_user_action();
                    }
                }
                a.find_next();
            });
        }
        {
            let (a, s) = (self.clone(), sync.clone());
            rep_all.connect_clicked(move |_| {
                s();
                let (f, r, case) = { let st = a.st.borrow(); (st.find.clone(), st.replace.clone(), st.match_case) };
                if a.replace_all(&f, &r, case) == 0 {
                    let d: gtk::Window = a.find_dlg.borrow().clone().map(|d| d.upcast()).unwrap_or_else(|| a.win.clone().upcast());
                    message(&d, gtk::MessageType::Info, &format!("Cannot find \"{f}\""));
                }
            });
        }
        { let d2 = d.clone(); cancel.connect_clicked(move |_| d2.close()); }
        {
            let d2 = d.clone();
            d.connect_key_press_event(move |_, e| { if e.keyval() == gdk::keys::constants::Escape { d2.close(); return glib::Propagation::Stop; } glib::Propagation::Proceed });
        }
        { let a = Rc::downgrade(self); d.connect_destroy(move |d| { if let Some(a) = a.upgrade() { let mut f = a.find_dlg.borrow_mut(); if f.as_ref() == Some(d) { *f = None; } } }); }
        next.set_can_default(true);
        d.set_default(Some(&next));
        d.show_all();
        what.grab_focus();
        *self.find_dlg.borrow_mut() = Some(d);
    }

    fn goto_dialog(self: &Rc<Self>) {
        let d = gtk::Dialog::with_buttons(Some("Go To Line"), Some(&self.win), gtk::DialogFlags::MODAL,
            &[("Go To", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
        d.set_resizable(false);
        let c = d.content_area();
        c.set_border_width(10);
        c.set_spacing(6);
        let l = gtk::Label::with_mnemonic("_Line number:");
        l.set_xalign(0.0);
        let e = gtk::Entry::new();
        e.set_input_purpose(gtk::InputPurpose::Digits);
        e.set_text(&(self.cursor().line() + 1).to_string());
        e.set_activates_default(true);
        l.set_mnemonic_widget(Some(&e));
        // only digits, like Win7's number edit box
        e.connect_insert_text(|e, t, _| { if !t.chars().all(|c| c.is_ascii_digit()) { e.stop_signal_emission_by_name("insert-text"); gdk::Display::default().map(|d| d.beep()); } });
        c.pack_start(&l, false, false, 0);
        c.pack_start(&e, false, false, 0);
        d.set_default_response(gtk::ResponseType::Ok);
        d.show_all();
        loop {
            if d.run() != gtk::ResponseType::Ok { break; }
            let line: i64 = e.text().parse().unwrap_or(0);
            if line < 1 || line > self.buf.line_count() as i64 {
                // a MessageBox without an icon: Default Beep
                sounds::play(sounds::Event::DefaultBeep);
                let m = gtk::MessageDialog::new(Some(&d), gtk::DialogFlags::MODAL, gtk::MessageType::Other, gtk::ButtonsType::Ok, "The line number is beyond the total number of lines");
                m.set_title("Notepad - Goto Line");
                m.run();
                m.close();
                continue;
            }
            let it = self.buf.iter_at_line(line as i32 - 1);
            self.buf.place_cursor(&it);
            self.view.scroll_to_iter(&mut it.clone(), 0.1, false, 0.0, 0.0);
            break;
        }
        d.close();
    }

    fn page_setup(self: &Rc<Self>) {
        let settings = self.print_settings.borrow().clone().unwrap_or_else(gtk::PrintSettings::new);
        let setup = self.page_setup.borrow().clone();
        let s = gtk::print_run_page_setup_dialog(Some(&self.win), setup.as_ref(), &settings);
        if let Some(s) = s { *self.page_setup.borrow_mut() = Some(s); }
    }

    /// Print through the system print dialog with Win7's default header ("&f") and footer ("Page &p"),
    /// in the Notepad font, wrapping long lines to the page.
    fn print(self: &Rc<Self>) {
        let op = gtk::PrintOperation::new();
        op.set_job_name(&self.title_name());
        if let Some(s) = self.page_setup.borrow().as_ref() { op.set_default_page_setup(Some(s)); }
        if let Some(s) = self.print_settings.borrow().as_ref() { op.set_print_settings(Some(s)); }
        let text = self.text();
        let name = self.title_name();
        let font = pango::FontDescription::from_string(&App::font_desc());
        // (layout, [(first line, top) per page], line baselines in pango units, header height)
        type Pag = (Option<pango::Layout>, Vec<(usize, i32)>, Vec<i32>, f64);
        let pag: Rc<RefCell<Pag>> = Rc::new(RefCell::new((None, vec![], vec![], 0.0)));
        {
            let (pag, font) = (pag.clone(), font.clone());
            op.connect_begin_print(move |op, ctx| {
                let Some(layout) = ctx.create_pango_layout() else { return };
                layout.set_font_description(Some(&font));
                layout.set_width((ctx.width() * pango::SCALE as f64) as i32);
                layout.set_wrap(pango::WrapMode::WordChar);
                layout.set_text(&text);
                let line_h = ctx.create_pango_layout().map(|l| { l.set_font_description(Some(&font)); l.set_text("Xg"); l.pixel_size().1 as f64 }).unwrap_or(14.0);
                let header = line_h * 2.0;
                let avail = ((ctx.height() - 2.0 * header).max(line_h) * pango::SCALE as f64) as i32;
                let mut pages: Vec<(usize, i32)> = vec![(0, 0)];
                let mut baselines = vec![];
                let mut it = layout.iter();
                let mut i = 0usize;
                loop {
                    let (_, logical) = it.line_extents();
                    baselines.push(it.baseline());
                    let page_top = pages.last().unwrap().1;
                    if logical.y() + logical.height() - page_top > avail && i > pages.last().unwrap().0 { pages.push((i, logical.y())); }
                    if !it.next_line() { break; }
                    i += 1;
                }
                op.set_n_pages(pages.len() as i32);
                *pag.borrow_mut() = (Some(layout), pages, baselines, header);
            });
        }
        op.connect_draw_page(move |_, ctx, page| {
            let Some(cr) = ctx.cairo_context() else { return };
            let p = pag.borrow();
            let (Some(layout), pages, baselines, header) = (&p.0, &p.1, &p.2, p.3) else { return };
            cr.set_source_rgb(0.0, 0.0, 0.0);
            let hf = |s: &str, y: f64| {
                let Some(l) = ctx.create_pango_layout() else { return };
                l.set_font_description(Some(&font));
                l.set_text(s);
                let (w, _) = l.pixel_size();
                cr.move_to((ctx.width() - w as f64) / 2.0, y);
                pangocairo::functions::show_layout(&cr, &l);
            };
            hf(&name, 0.0);
            hf(&format!("Page {}", page + 1), ctx.height() - header / 2.0);
            let page = page as usize;
            let Some(&(first, top)) = pages.get(page) else { return };
            let last = pages.get(page + 1).map(|x| x.0).unwrap_or(baselines.len());
            for li in first..last {
                let Some(line) = layout.line_readonly(li as i32) else { continue };
                cr.move_to(0.0, header + (baselines[li] - top) as f64 / pango::SCALE as f64);
                pangocairo::functions::show_layout_line(&cr, &line);
            }
        });
        match op.run(gtk::PrintOperationAction::PrintDialog, Some(&self.win)) {
            Ok(gtk::PrintOperationResult::Apply) => { *self.print_settings.borrow_mut() = op.print_settings(); }
            Err(e) => message(&self.win, gtk::MessageType::Warning, &format!("Notepad could not print.\n\n{e}")),
            _ => {}
        }
    }

    fn undo(&self) {
        let group = std::mem::take(&mut self.undo.borrow_mut().group);
        if group.is_empty() { return; }
        self.undo.borrow_mut().mute = true;
        let mut inverse = Vec::with_capacity(group.len());
        let mut cursor = 0;
        for op in group.iter().rev() {
            match op {
                Op::Ins(at, s) => {
                    let (mut a, mut b) = (self.buf.iter_at_offset(*at), self.buf.iter_at_offset(at + s.chars().count() as i32));
                    self.buf.delete(&mut a, &mut b);
                    inverse.push(Op::Del(*at, s.clone()));
                    cursor = *at;
                }
                Op::Del(at, s) => {
                    let mut a = self.buf.iter_at_offset(*at);
                    self.buf.insert(&mut a, s);
                    inverse.push(Op::Ins(*at, s.clone()));
                    cursor = at + s.chars().count() as i32;
                }
            }
        }
        let mut u = self.undo.borrow_mut();
        u.mute = false;
        u.group = inverse;
        u.kind = 3;
        drop(u);
        let it = self.buf.iter_at_offset(cursor);
        self.buf.place_cursor(&it);
        self.view.scroll_to_mark(&self.buf.get_insert().unwrap(), 0.0, false, 0.0, 0.0);
    }

    fn delete(&self) {
        // Edit > Delete (Del): the selection, or the character after the cursor
        if !self.buf.delete_selection(true, true) {
            let mut a = self.cursor();
            if a.is_end() { return; }
            let mut b = a.clone();
            b.forward_cursor_position();
            self.buf.begin_user_action();
            self.buf.delete_interactive(&mut a, &mut b, true);
            self.buf.end_user_action();
        }
    }

    fn save_geometry(&self) {
        if self.win.window().map(|w| w.state().intersects(gdk::WindowState::MAXIMIZED | gdk::WindowState::FULLSCREEN)).unwrap_or(false) { return; }
        let (x, y) = self.win.position();
        let (w, h) = self.win.size();
        let _ = std::fs::create_dir_all(config::conf_dir());
        let _ = std::fs::write(config::conf_dir().join("notepad-window"), format!("{x} {y} {w} {h}"));
    }
}

/// An I/O error in Windows words.
fn win_err(e: &std::io::Error) -> String {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "The system cannot find the path specified.".into(),
        K::PermissionDenied => "Access is denied.".into(),
        K::StorageFull => "There is not enough space on the disk.".into(),
        K::ReadOnlyFilesystem => "The media is write protected.".into(),
        _ => { let s = e.to_string(); s.split(" (os error").next().unwrap_or(&s).to_string() }
    }
}

fn time_date() -> String { chrono::Local::now().format("%-I:%M %p %-m/%-d/%Y").to_string() }

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

/// A menu item that only shows its shortcut: the key itself is handled by the text view
/// (an accelerator for Del would swallow the Delete key).
fn menu_item_hint(menu: &gtk::Menu, label: &str, accel: &str, f: impl Fn() + 'static) -> gtk::MenuItem {
    let it = gtk::MenuItem::new();
    let l = gtk::AccelLabel::new("");
    l.set_text_with_mnemonic(label);
    l.set_xalign(0.0);
    let (k, m) = gtk::accelerator_parse(accel);
    l.set_accel(k, m);
    it.add(&l);
    it.connect_activate(move |_| f());
    menu.append(&it);
    it
}

fn build(app: &gtk::Application, file: Option<PathBuf>) {
    theme::init_app();
    let win = gtk::ApplicationWindow::new(app);
    // Win7 remembers the window placement
    let geom: Vec<i32> = std::fs::read_to_string(config::conf_dir().join("notepad-window")).unwrap_or_default().split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if let [x, y, w, h] = geom[..] {
        win.set_default_size(w.max(200), h.max(150));
        if app.windows().len() <= 1 { win.move_(x, y); }
    } else {
        win.set_default_size(800, 560);
    }
    win.set_icon_name(Some(util::first_icon(&["accessories-text-editor", "text-editor"])));
    let accel = gtk::AccelGroup::new();
    win.add_accel_group(&accel);
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let menubar = gtk::MenuBar::new();
    vbox.pack_start(&menubar, false, false, 0);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Always, gtk::PolicyType::Always);
    sw.set_overlay_scrolling(false);
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
        st: RefCell::new(State { path: None, enc: Enc::Ansi, eol: Eol::Lf, dirty: false, find: String::new(), replace: String::new(), match_case: false, down: true }),
        undo: RefCell::new(Undo::default()), find_dlg: RefCell::new(None), page_setup: RefCell::new(None), print_settings: RefCell::new(None), loading: Cell::new(false),
    });
    a.set_font(&App::font_desc());

    // ---- File
    let m = gtk::Menu::new();
    { let a = a.clone(); menu_item(&m, "_New", Some((&accel, "<Control>n")), move || a.new_doc()); }
    { let a = a.clone(); menu_item(&m, "_Open...", Some((&accel, "<Control>o")), move || a.open()); }
    { let a = a.clone(); menu_item(&m, "_Save", Some((&accel, "<Control>s")), move || { a.save(false); }); }
    { let a = a.clone(); menu_item(&m, "Save _As...", None, move || { a.save(true); }); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let a = a.clone(); menu_item(&m, "Page Set_up...", None, move || a.page_setup()); }
    { let a = a.clone(); menu_item(&m, "_Print...", Some((&accel, "<Control>p")), move || a.print()); }
    m.append(&gtk::SeparatorMenuItem::new());
    { let w = win.clone(); menu_item(&m, "E_xit", None, move || w.close()); }
    let top = gtk::MenuItem::with_mnemonic("_File"); top.set_submenu(Some(&m)); menubar.append(&top);

    // ---- Edit
    let m = gtk::Menu::new();
    let undo = { let a = a.clone(); menu_item(&m, "_Undo", Some((&accel, "<Control>z")), move || a.undo()) };
    m.append(&gtk::SeparatorMenuItem::new());
    let cut = { let v = view.clone(); menu_item(&m, "Cu_t", Some((&accel, "<Control>x")), move || v.emit_cut_clipboard()) };
    let copy = { let v = view.clone(); menu_item(&m, "_Copy", Some((&accel, "<Control>c")), move || v.emit_copy_clipboard()) };
    { let v = view.clone(); menu_item(&m, "_Paste", Some((&accel, "<Control>v")), move || v.emit_paste_clipboard()); }
    let del = { let a = a.clone(); menu_item_hint(&m, "De_lete", "Delete", move || a.delete()) };
    m.append(&gtk::SeparatorMenuItem::new());
    let find = { let a = a.clone(); menu_item(&m, "_Find...", Some((&accel, "<Control>f")), move || a.find_dialog(false)) };
    let find_next = { let a = a.clone(); menu_item(&m, "Find _Next", Some((&accel, "F3")), move || { a.find_next(); }) };
    { let a = a.clone(); menu_item(&m, "_Replace...", Some((&accel, "<Control>h")), move || a.find_dialog(true)); }
    let goto = { let a = a.clone(); menu_item(&m, "_Go To...", Some((&accel, "<Control>g")), move || a.goto_dialog()) };
    m.append(&gtk::SeparatorMenuItem::new());
    { let a = a.clone(); menu_item(&m, "Select _All", Some((&accel, "<Control>a")), move || { let (s, e) = a.buf.bounds(); a.buf.select_range(&s, &e); }); }
    { let a = a.clone(); menu_item(&m, "Time/_Date", Some((&accel, "F5")), move || { a.buf.begin_user_action(); a.buf.delete_selection(true, true); a.buf.insert_at_cursor(&time_date()); a.buf.end_user_action(); }); }
    let top = gtk::MenuItem::with_mnemonic("_Edit"); top.set_submenu(Some(&m)); menubar.append(&top);
    {
        // Win7 greys out what cannot be done right now
        let (a, undo, cut, copy, del, find, find_next) = (a.clone(), undo.clone(), cut.clone(), copy.clone(), del.clone(), find.clone(), find_next.clone());
        top.connect_activate(move |_| {
            let sel = a.buf.has_selection();
            undo.set_sensitive(!a.undo.borrow().group.is_empty());
            cut.set_sensitive(sel);
            copy.set_sensitive(sel);
            del.set_sensitive(sel);
            let has_text = a.buf.char_count() > 0;
            find.set_sensitive(has_text);
            find_next.set_sensitive(has_text);
        });
    }
    // …and makes everything available again once the menu closes, so the shortcuts keep working
    {
        let items = [undo.clone(), cut.clone(), copy.clone(), del.clone(), find.clone(), find_next.clone()];
        m.connect_deactivate(move |_| for i in &items { i.set_sensitive(true); });
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
            set_flag("notepad-wrap", on);
        });
    }
    {
        let a = a.clone();
        menu_item(&m, "_Font...", None, move || {
            let d = gtk::FontChooserDialog::new(Some("Font"), Some(&a.win));
            d.set_font(&App::font_desc());
            if d.run() == gtk::ResponseType::Ok { if let Some(f) = d.font() { a.set_font(&f); } }
            d.close();
        });
    }
    let top = gtk::MenuItem::with_mnemonic("F_ormat"); top.set_submenu(Some(&m)); menubar.append(&top);

    // ---- View
    let m = gtk::Menu::new();
    let sb = gtk::CheckMenuItem::with_mnemonic("_Status Bar");
    sb.set_active(config_flag("notepad-status"));
    { let a = a.clone(); sb.connect_toggled(move |s| { set_flag("notepad-status", s.is_active()); a.statusbar.set_visible(s.is_active() && !a.wrap.is_active()); a.update_status(); }); }
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
            d.set_comments(Some("Aerode Notepad – a Windows 7 style text editor for Linux"));
            d.set_logo_icon_name(Some(util::first_icon(&["accessories-text-editor"])));
            d.run();
            d.close();
        });
    }
    let top = gtk::MenuItem::with_mnemonic("_Help"); top.set_submenu(Some(&m)); menubar.append(&top);

    // ---- behaviour
    {
        let a2 = a.clone();
        a.buf.connect_insert_text(move |_, it, t| { a2.undo.borrow_mut().record(Op::Ins(it.offset(), t.to_string())); });
    }
    {
        let a2 = a.clone();
        a.buf.connect_delete_range(move |b, s, e| {
            let t = b.text(s, e, true).map(|g| g.to_string()).unwrap_or_default();
            a2.undo.borrow_mut().record(Op::Del(s.offset().min(e.offset()), t));
        });
    }
    { let a2 = a.clone(); a.buf.connect_begin_user_action(move |_| { let mut u = a2.undo.borrow_mut(); u.in_action = true; u.fresh = true; }); }
    { let a2 = a.clone(); a.buf.connect_end_user_action(move |_| { a2.undo.borrow_mut().in_action = false; }); }
    {
        let a2 = a.clone();
        a.buf.connect_changed(move |_| {
            if !a2.loading.get() && !a2.st.borrow().dirty { a2.st.borrow_mut().dirty = true; }
            a2.update_status();
        });
    }
    { let a2 = a.clone(); a.buf.connect_mark_set(move |_, _, m| { if m.name().as_deref() == Some("insert") { a2.update_status(); } }); }
    {
        let a2 = a.clone();
        win.connect_delete_event(move |_, _| {
            if !a2.maybe_save() { return glib::Propagation::Stop; }
            a2.save_geometry();
            glib::Propagation::Proceed
        });
    }
    // Win7's text-box context menu: Undo, Cut, Copy, Paste, Delete, Select All
    {
        let a2 = a.clone();
        view.connect_populate_popup(move |_, w| {
            let Some(menu) = w.downcast_ref::<gtk::Menu>() else { return };
            for c in menu.children() { menu.remove(&c); }
            let sel = a2.buf.has_selection();
            let add = |l: &str, on: bool, f: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.set_sensitive(on); it.connect_activate(move |_| f()); menu.append(&it); };
            { let a = a2.clone(); add("_Undo", !a2.undo.borrow().group.is_empty(), Box::new(move || a.undo())); }
            menu.append(&gtk::SeparatorMenuItem::new());
            { let v = a2.view.clone(); add("Cu_t", sel, Box::new(move || v.emit_cut_clipboard())); }
            { let v = a2.view.clone(); add("_Copy", sel, Box::new(move || v.emit_copy_clipboard())); }
            { let v = a2.view.clone(); add("_Paste", true, Box::new(move || v.emit_paste_clipboard())); }
            { let a = a2.clone(); add("_Delete", sel, Box::new(move || a.delete())); }
            menu.append(&gtk::SeparatorMenuItem::new());
            { let a = a2.clone(); add("Select _All", a2.buf.char_count() > 0, Box::new(move || { let (s, e) = a.buf.bounds(); a.buf.select_range(&s, &e); })); }
            menu.show_all();
        });
    }
    // drag & drop a file onto Notepad
    view.drag_dest_add_uri_targets();
    {
        let a2 = a.clone();
        view.connect_drag_data_received(move |v, _, _, _, data, _, _| {
            if let Some(u) = data.uris().first() {
                if let Some(p) = gio::File::for_uri(u).path() { if a2.maybe_save() { a2.load(p, false); } }
                v.stop_signal_emission_by_name("drag-data-received");
            }
        });
    }
    vbox.show_all();
    wrap.set_active(config_flag("notepad-wrap"));
    statusbar.set_visible(config_flag("notepad-status") && !wrap.is_active());
    goto.set_sensitive(!wrap.is_active());
    a.update_title();
    win.show();
    view.grab_focus();
    if let Some(p) = file {
        if !a.load(p, true) { win.close(); }
    }
}

fn config_flag(name: &str) -> bool { config::conf_dir().join(name).exists() }
fn set_flag(name: &str, on: bool) {
    let p = config::conf_dir().join(name);
    let _ = std::fs::create_dir_all(config::conf_dir());
    if on { let _ = std::fs::write(p, ""); } else { let _ = std::fs::remove_file(p); }
}

fn main() -> glib::ExitCode {
    util::log_panics("notepad");
    let app = gtk::Application::new(Some("org.aerode.Notepad"), gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(|app| { util::message_dialog_sounds(); build(app, None) });
    app.connect_open(|app, files, _| { util::message_dialog_sounds(); for f in files { build(app, f.path()); } });
    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodings_round_trip() {
        for (bytes, enc) in [(&b"a\xe9b"[..], Enc::Legacy), (b"\xef\xbb\xbfhi", Enc::Utf8), ("ok é".as_bytes(), Enc::Ansi), (b"\xff\xfeh\0i\0", Enc::Utf16Le), (b"\xfe\xff\0h\0i", Enc::Utf16Be)] {
            let (t, e) = decode(bytes);
            assert_eq!(e, enc);
            assert_eq!(encode(&t, e, Eol::Lf), bytes);
        }
        assert_eq!(decode(b"\x80").0, "€");
    }
    #[test]
    fn line_ends_kept() {
        let (t, eol) = normalize("a\r\nb\r\n".into());
        assert_eq!((t.as_str(), eol), ("a\nb\n", Eol::CrLf));
        assert_eq!(encode(&t, Enc::Ansi, eol), b"a\r\nb\r\n");
        let (t, eol) = normalize("a\nb\n".into());
        assert_eq!(encode(&t, Enc::Ansi, eol), b"a\nb\n");
    }
    #[test]
    fn undo_groups_typing() {
        let mut u = Undo::default();
        for (i, c) in "abc".chars().enumerate() { u.in_action = true; u.fresh = true; u.record(Op::Ins(i as i32, c.to_string())); u.in_action = false; }
        assert_eq!(u.group.len(), 3);
        u.in_action = true; u.fresh = true; u.record(Op::Del(2, "c".into())); u.in_action = false;
        assert_eq!(u.group.len(), 1);
    }
    #[test]
    fn safe_write() {
        let d = std::env::temp_dir().join(format!("np-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("x.txt");
        std::fs::write(&f, "old").unwrap();
        write_safely(&f, b"new").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
