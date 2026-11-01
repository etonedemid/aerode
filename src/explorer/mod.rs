//! Windows Explorer remake.
pub mod ops;
pub mod props;

use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::{config, glass, sounds, util};

#[derive(Clone, Debug, PartialEq)]
pub enum Loc {
    Path(PathBuf),
    Computer,
    Trash,
    Network,
    Library(&'static str),
    Search(PathBuf, String),
}

impl Loc {
    pub fn parse(s: &str) -> Loc {
        match s {
            "computer:///" | "computer:" | "Computer" => Loc::Computer,
            "trash:///" | "trash:" => Loc::Trash,
            "network:///" | "network:" => Loc::Network,
            s if s.starts_with("file://") => gio::File::for_uri(s).path().map(Loc::Path).unwrap_or(Loc::Computer),
            s if s.starts_with("library:") => match &s[8..] { "Music" => Loc::Library("Music"), "Pictures" => Loc::Library("Pictures"), "Videos" => Loc::Library("Videos"), _ => Loc::Library("Documents") },
            s => Loc::Path(PathBuf::from(util::shellexpand(s))),
        }
    }

    fn title(&self) -> String {
        match self {
            Loc::Path(p) if p == &config::home() => glib::user_name().to_string_lossy().into_owned(),
            Loc::Path(p) if p == Path::new("/") => "File System".into(),
            Loc::Path(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
            Loc::Computer => "Computer".into(),
            Loc::Trash => "Recycle Bin".into(),
            Loc::Network => "Network".into(),
            Loc::Library(n) => n.to_string(),
            Loc::Search(_, q) => format!("Search Results in {q}"),
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Loc::Computer => "computer", Loc::Trash => "user-trash", Loc::Network => "network-workgroup",
            Loc::Library("Music") => "folder-music", Loc::Library("Pictures") => "folder-pictures",
            Loc::Library("Videos") => "folder-videos", Loc::Library(_) => "folder-documents",
            Loc::Search(..) => "system-search",
            Loc::Path(p) if p == &config::home() => "user-home",
            _ => "folder",
        }
    }

    fn dir(&self) -> Option<PathBuf> {
        match self {
            Loc::Path(p) => Some(p.clone()),
            Loc::Library(n) => Some(library_dir(n)),
            Loc::Trash => Some(glib::user_data_dir().join("Trash/files")),
            _ => None,
        }
    }
}

fn library_dir(n: &str) -> PathBuf {
    match n {
        "Music" => util::special_dir(glib::UserDirectory::Music, "Music"),
        "Pictures" => util::special_dir(glib::UserDirectory::Pictures, "Pictures"),
        "Videos" => util::special_dir(glib::UserDirectory::Videos, "Videos"),
        _ => util::special_dir(glib::UserDirectory::Documents, "Documents"),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum View { XLarge, Large, Medium, Small, List, Details, Tiles }

impl View {
    fn icon_size(self) -> i32 { match self { View::XLarge => 256, View::Large => 96, View::Medium => 48, View::Tiles => 48, _ => 16 } }
    fn label(self) -> &'static str {
        match self { View::XLarge => "Extra Large Icons", View::Large => "Large Icons", View::Medium => "Medium Icons", View::Small => "Small Icons", View::List => "List", View::Details => "Details", View::Tiles => "Tiles" }
    }
    const ALL: [View; 7] = [View::XLarge, View::Large, View::Medium, View::Small, View::List, View::Details, View::Tiles];
}

// model columns
const C_ICON: u32 = 0;
const C_NAME: u32 = 1;
const C_DATE: u32 = 2;
const C_TYPE: u32 = 3;
const C_SIZE: u32 = 4;
const C_PATH: u32 = 5;
const C_ISDIR: u32 = 6;
const C_SIZEN: u32 = 7;
const C_DATEN: u32 = 8;
const C_TILE: u32 = 9;
const C_SORTNAME: u32 = 10;

pub struct Explorer {
    pub win: gtk::ApplicationWindow,
    loc: RefCell<Loc>,
    back: RefCell<Vec<Loc>>,
    fwd: RefCell<Vec<Loc>>,
    store: gtk::ListStore,
    filter: gtk::TreeModelFilter,
    sorted: gtk::TreeModelSort,
    icons: gtk::IconView,
    details: gtk::TreeView,
    stack: gtk::Stack,
    crumbs: gtk::Box,
    addr_entry: gtk::Entry,
    addr_stack: gtk::Stack,
    search: gtk::Entry,
    back_b: gtk::Button,
    fwd_b: gtk::Button,
    cmd_dyn: gtk::Box,
    nav: gtk::TreeView,
    nav_store: gtk::TreeStore,
    info_icon: gtk::Image,
    info_name: gtk::Label,
    info_text: gtk::Label,
    lib_header: gtk::Box,
    lib_title: gtk::Label,
    preview: gtk::Image,
    preview_box: gtk::Box,
    computer: gtk::Box,
    view: Cell<View>,
    show_hidden: Cell<bool>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    thumbs: RefCell<HashMap<PathBuf, Pixbuf>>,
    search_gen: Cell<u32>,
    vm: gio::VolumeMonitor,
}

pub const CSS: &str = r#"
.exp-header { padding: 4px 6px 5px 6px; }
.exp-header button.nav { min-width: 26px; min-height: 26px; padding: 0; border-radius: 50%;
  background-image: radial-gradient(circle at 50% 35%, #cfe9ff, #4f9be3 55%, #1f5fae); border: 1px solid #23466f; box-shadow: inset 0 1px 0 rgba(255,255,255,0.7); }
.exp-header button.nav:hover { background-image: radial-gradient(circle at 50% 35%, #e9f6ff, #6fbaf6 55%, #2a75c7); }
.exp-header button.nav:disabled { background-image: radial-gradient(circle at 50% 35%, #f4f6f8, #c8d0da 60%, #9aa6b4); border-color: #7d8896; }
.exp-header button.nav image { color: #fff; -gtk-icon-shadow: 0 1px 1px rgba(0,0,0,0.5); }
.exp-addr { background-color: rgba(255,255,255,0.92); border: 1px solid rgba(0,0,0,0.55); border-radius: 2px; min-height: 22px; }
.exp-addr button { background: none; border: 1px solid transparent; border-radius: 0; box-shadow: none; padding: 0 3px; min-height: 20px; color: #000; }
.exp-addr button:hover { background-image: linear-gradient(to bottom, #eaf6fd, #bfe2f8); border-color: #3c7fb1; }
.exp-addr entry { border: none; background: transparent; box-shadow: none; }
.exp-search { background-color: rgba(255,255,255,0.92); border: 1px solid rgba(0,0,0,0.55); border-radius: 2px; min-height: 22px; font-style: italic; }
.commandbar { background-image: linear-gradient(to bottom, #f5faff, #e5eef9 50%, #d9e6f6 51%, #e2ecf8); border-top: 1px solid #ffffff; border-bottom: 1px solid #a0afc3; padding: 2px 6px; }
.commandbar button { color: #1e395b; background: none; border: 1px solid transparent; box-shadow: none; padding: 2px 8px; }
.commandbar button:hover { background-image: linear-gradient(to bottom, #fbfdff, #e4effb 50%, #d4e6f9 51%, #dcecfd); border-color: #a7cdf0; border-radius: 3px; }
.exp-nav { background-color: #ffffff; border-right: 1px solid #d6e5f5; }
.exp-nav treeview.view { background-color: #ffffff; }
.exp-nav treeview.view:selected { background-image: linear-gradient(to bottom, #dcebfc, #c1dbfc); color: #000; }
.exp-details { background-image: linear-gradient(to bottom, #f6f9fd, #e8eef6); border-top: 1px solid #c4d1e2; padding: 6px 10px; }
.exp-details .name { font-size: 11pt; color: #000; }
.exp-details label { color: #4b5b73; }
.exp-lib { background-image: linear-gradient(to bottom, #ffffff, #f2f7fc); border-bottom: 1px solid #dfe9f5; padding: 8px 14px; }
.exp-lib .title { font-size: 12pt; color: #1e395b; }
.drive-tile { padding: 6px; border: 1px solid transparent; border-radius: 3px; }
.drive-tile:hover { background-image: linear-gradient(to bottom, #f1f8fd, #d5e9f9); border-color: #7da2ce; }
.drive-group { color: #1e3287; font-size: 10pt; padding: 8px 6px 2px 6px; }
"#;

fn fmt_date(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).map(|d| d.with_timezone(&chrono::Local).format("%-m/%-d/%Y %-I:%M %p").to_string()).unwrap_or_default()
}

fn kb(n: u64) -> String {
    let k = n.div_ceil(1024);
    let s = k.to_string();
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { o.push(','); } o.push(c); }
    format!("{o} KB")
}

impl Explorer {
    pub fn new(app: &gtk::Application, start: Loc) -> Rc<Explorer> {
        crate::theme::load_css(CSS);
        let win = gtk::ApplicationWindow::new(app);
        win.set_default_size(900, 600);
        win.set_icon_name(Some(util::first_icon(&["system-file-manager", "folder"])));
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // ---------- glass header: back/forward, address bar, search
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.style_context().add_class("exp-header");
        header.set_app_paintable(true);
        let back_b = gtk::Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
        let fwd_b = gtk::Button::from_icon_name(Some("go-next-symbolic"), gtk::IconSize::Button);
        for b in [&back_b, &fwd_b] { b.style_context().add_class("nav"); b.set_valign(gtk::Align::Center); }
        back_b.set_tooltip_text(Some("Back"));
        fwd_b.set_tooltip_text(Some("Forward"));
        header.pack_start(&back_b, false, false, 0);
        header.pack_start(&fwd_b, false, false, 0);
        let addr_stack = gtk::Stack::new();
        let addr = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        addr.style_context().add_class("exp-addr");
        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let crumb_ev = gtk::EventBox::new();
        crumb_ev.add(&crumbs);
        addr.pack_start(&crumb_ev, true, true, 0);
        let refresh = gtk::Button::from_icon_name(Some("view-refresh-symbolic"), gtk::IconSize::Menu);
        refresh.set_tooltip_text(Some("Refresh"));
        addr.pack_end(&refresh, false, false, 0);
        let addr_entry = gtk::Entry::new();
        addr_entry.style_context().add_class("exp-addr");
        addr_stack.add_named(&addr, "crumbs");
        addr_stack.add_named(&addr_entry, "entry");
        header.pack_start(&addr_stack, true, true, 0);
        let search = gtk::Entry::new();
        search.style_context().add_class("exp-search");
        search.set_width_chars(24);
        search.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("system-search-symbolic"));
        header.pack_end(&search, false, false, 0);
        root.pack_start(&header, false, false, 0);

        // ---------- command bar
        let cmd = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        cmd.style_context().add_class("commandbar");
        let organize = gtk::MenuButton::new();
        organize.set_label("Organize ▾");
        organize.set_relief(gtk::ReliefStyle::None);
        cmd.pack_start(&organize, false, false, 0);
        let cmd_dyn = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        cmd.pack_start(&cmd_dyn, false, false, 0);
        let views_b = gtk::MenuButton::new();
        views_b.add(&util::image("view-grid-symbolic", 16));
        views_b.set_relief(gtk::ReliefStyle::None);
        views_b.set_tooltip_text(Some("Change your view."));
        let prev_b = gtk::ToggleButton::new();
        prev_b.add(&util::image("view-dual-symbolic", 16));
        prev_b.set_relief(gtk::ReliefStyle::None);
        prev_b.set_tooltip_text(Some("Show the preview pane."));
        let help_b = gtk::Button::from_icon_name(Some("help-browser"), gtk::IconSize::Menu);
        help_b.set_relief(gtk::ReliefStyle::None);
        help_b.connect_clicked(|_| { util::open_uri("https://wiki.archlinux.org/title/File_manager_functionality"); });
        cmd.pack_end(&help_b, false, false, 0);
        cmd.pack_end(&prev_b, false, false, 0);
        cmd.pack_end(&views_b, false, false, 0);
        root.pack_start(&cmd, false, false, 0);

        // ---------- body: nav | (lib header + files) | preview
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        let nav_store = gtk::TreeStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
        let nav = gtk::TreeView::with_model(&nav_store);
        nav.set_headers_visible(false);
        nav.set_enable_tree_lines(false);
        let col = gtk::TreeViewColumn::new();
        let ri = gtk::CellRendererPixbuf::new();
        let rt = gtk::CellRendererText::new();
        TreeViewColumnExt::pack_start(&col, &ri, false);
        TreeViewColumnExt::pack_start(&col, &rt, true);
        TreeViewColumnExt::add_attribute(&col, &ri, "pixbuf", 0);
        TreeViewColumnExt::add_attribute(&col, &rt, "text", 1);
        nav.append_column(&col);
        let nav_sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        nav_sw.style_context().add_class("exp-nav");
        nav_sw.add(&nav);
        nav_sw.set_size_request(190, -1);
        paned.pack1(&nav_sw, false, false);

        let center = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let lib_header = gtk::Box::new(gtk::Orientation::Vertical, 2);
        lib_header.style_context().add_class("exp-lib");
        let lib_title = gtk::Label::new(None);
        lib_title.set_xalign(0.0);
        lib_title.style_context().add_class("title");
        lib_header.pack_start(&lib_title, false, false, 0);
        let lib_sub = gtk::Label::new(Some("Includes: 1 location"));
        lib_sub.set_xalign(0.0);
        lib_header.pack_start(&lib_sub, false, false, 0);
        center.pack_start(&lib_header, false, false, 0);

        // model: icon, name, date, type, size, path, isdir, size#, date#, tile markup, sortname
        let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING,
            glib::Type::STRING, glib::Type::BOOL, glib::Type::U64, glib::Type::I64, glib::Type::STRING, glib::Type::STRING]);
        let filter = gtk::TreeModelFilter::new(&store, None);
        let sorted = gtk::TreeModelSort::new(&filter);
        // folders first, then by name (Windows sort)
        sorted.set_sort_func(gtk::SortColumn::Index(C_SORTNAME), |m, a, b| {
            let da: bool = m.value(a, C_ISDIR as i32).get().unwrap_or(false);
            let db: bool = m.value(b, C_ISDIR as i32).get().unwrap_or(false);
            if da != db { return if da { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater }; }
            let na: String = m.value(a, C_SORTNAME as i32).get().unwrap_or_default();
            let nb: String = m.value(b, C_SORTNAME as i32).get().unwrap_or_default();
            natural_cmp(&na, &nb)
        });
        sorted.set_sort_column_id(gtk::SortColumn::Index(C_SORTNAME), gtk::SortType::Ascending);
        let icons = gtk::IconView::with_model(&sorted);
        icons.set_pixbuf_column(C_ICON as i32);
        icons.set_text_column(C_NAME as i32);
        icons.set_selection_mode(gtk::SelectionMode::Multiple);
        icons.set_item_padding(4);
        icons.set_column_spacing(2);
        icons.set_row_spacing(2);
        IconViewExt::set_margin(&icons, 6);
        let details = gtk::TreeView::with_model(&sorted);
        details.selection().set_mode(gtk::SelectionMode::Multiple);
        details.set_rubber_banding(true);
        {
            let c = gtk::TreeViewColumn::new();
            c.set_title("Name");
            let ri = gtk::CellRendererPixbuf::new();
            let rt = gtk::CellRendererText::new();
            rt.set_ellipsize(gtk::pango::EllipsizeMode::End);
            TreeViewColumnExt::pack_start(&c, &ri, false);
            TreeViewColumnExt::pack_start(&c, &rt, true);
            TreeViewColumnExt::add_attribute(&c, &ri, "pixbuf", C_ICON as i32);
            TreeViewColumnExt::add_attribute(&c, &rt, "text", C_NAME as i32);
            c.set_resizable(true);
            c.set_fixed_width(260);
            c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            c.set_sort_column_id(C_SORTNAME as i32);
            details.append_column(&c);
            for (title, col, sortc, w, right) in [("Date modified", C_DATE, C_DATEN, 140, false), ("Type", C_TYPE, C_TYPE, 140, false), ("Size", C_SIZE, C_SIZEN, 90, true)] {
                let c = gtk::TreeViewColumn::new();
                c.set_title(title);
                let r = gtk::CellRendererText::new();
                r.set_property("foreground", "#6d6d6d");
                if right { r.set_xalign(1.0); }
                TreeViewColumnExt::pack_start(&c, &r, true);
                TreeViewColumnExt::add_attribute(&c, &r, "text", col as i32);
                c.set_resizable(true);
                c.set_fixed_width(w);
                c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
                c.set_sort_column_id(sortc as i32);
                details.append_column(&c);
            }
        }
        let stack = gtk::Stack::new();
        let sw1 = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw1.add(&icons);
        let sw2 = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw2.add(&details);
        let computer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let sw3 = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw3.add(&computer);
        stack.add_named(&sw1, "icons");
        stack.add_named(&sw2, "details");
        stack.add_named(&sw3, "computer");
        center.pack_start(&stack, true, true, 0);
        let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
        inner.pack1(&center, true, false);
        let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        preview_box.set_size_request(260, -1);
        let preview = gtk::Image::new();
        preview.set_vexpand(true);
        preview_box.pack_start(&preview, true, true, 0);
        inner.pack2(&preview_box, false, false);
        paned.pack2(&inner, true, false);
        root.pack_start(&paned, true, true, 0);

        // ---------- details pane
        let dp = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        dp.style_context().add_class("exp-details");
        let info_icon = gtk::Image::new();
        dp.pack_start(&info_icon, false, false, 0);
        let iv = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let info_name = gtk::Label::new(None);
        info_name.set_xalign(0.0);
        info_name.style_context().add_class("name");
        let info_text = gtk::Label::new(None);
        info_text.set_xalign(0.0);
        iv.pack_start(&info_name, false, false, 0);
        iv.pack_start(&info_text, false, false, 0);
        dp.pack_start(&iv, true, true, 0);
        dp.set_size_request(-1, 66);
        root.pack_start(&dp, false, false, 0);
        win.add(&root);

        let ex = Rc::new(Explorer {
            win: win.clone(), loc: RefCell::new(start.clone()), back: RefCell::new(vec![]), fwd: RefCell::new(vec![]),
            store, filter: filter.clone(), sorted, icons: icons.clone(), details: details.clone(), stack, crumbs, addr_entry: addr_entry.clone(),
            addr_stack: addr_stack.clone(), search: search.clone(), back_b: back_b.clone(), fwd_b: fwd_b.clone(), cmd_dyn, nav: nav.clone(),
            nav_store, info_icon, info_name, info_text, lib_header, lib_title, preview, preview_box: preview_box.clone(), computer,
            view: Cell::new(load_view()), show_hidden: Cell::new(false), monitor: RefCell::new(None), thumbs: RefCell::new(HashMap::new()),
            search_gen: Cell::new(0), vm: gio::VolumeMonitor::get(),
        });

        // glass behind the header (same look as the frame when not composited)
        {
            let w = win.clone();
            header.connect_draw(move |h, cr| {
                let (x, y) = h.window().map(|gw| { let (_, x, y) = gw.origin(); (x, y) }).unwrap_or((0, 0));
                let (ww, hh) = (h.allocated_width() as f64, h.allocated_height() as f64);
                cr.set_source_pixbuf(&glass::wallpaper().blurred, -x as f64, -y as f64);
                cr.paint().ok();
                let c = aerode_frame_color();
                cr.set_source_rgba(c[0], c[1], c[2], if w.is_active() { 0.55 } else { 0.35 });
                cr.paint().ok();
                cr.set_source_rgba(1.0, 1.0, 1.0, if w.is_active() { 0.18 } else { 0.38 });
                cr.paint().ok();
                let _ = (ww, hh);
                glib::Propagation::Proceed
            });
            let h2 = header.clone();
            win.connect_configure_event(move |_, _| { h2.queue_draw(); false });
            let h3 = header.clone();
            win.connect_is_active_notify(move |_| h3.queue_draw());
        }
        // filter: hidden files + live search
        {
            let e = Rc::downgrade(&ex);
            filter.set_visible_func(move |m, it| {
                let Some(e) = e.upgrade() else { return true };
                let name: String = m.value(it, C_NAME as i32).get().unwrap_or_default();
                if !e.show_hidden.get() && name.starts_with('.') { return false; }
                let q = e.search.text().to_lowercase();
                if !q.is_empty() && !matches!(*e.loc.borrow(), Loc::Search(..)) { return name.to_lowercase().contains(&q); }
                true
            });
        }
        ex.build_nav();
        ex.wire(&organize, &views_b, &prev_b, &refresh, &crumb_ev);
        preview_box.set_visible(false);
        root.show_all();
        preview_box.hide();
        ex.go(start, false);
        win.show();
        ex
    }

    // ------------------------------------------------------------ navigation
    pub fn go(self: &Rc<Self>, loc: Loc, record: bool) {
        if let Loc::Path(p) = &loc {
            if p.is_file() { util::open_path(p); return; }
            if !p.exists() { util::error(&format!("Windows can't find '{}'.", p.display()), "Check the spelling and try again."); return; }
        }
        if record {
            let cur = self.loc.borrow().clone();
            if cur != loc { self.back.borrow_mut().push(cur); self.fwd.borrow_mut().clear(); }
            sounds::play(sounds::Event::StartNavigation);
        }
        *self.loc.borrow_mut() = loc.clone();
        if !matches!(loc, Loc::Search(..)) { self.search.set_text(""); }
        self.search.set_placeholder_text(Some(&format!("Search {}", loc.title())));
        self.win.set_title(&loc.title());
        self.back_b.set_sensitive(!self.back.borrow().is_empty());
        self.fwd_b.set_sensitive(!self.fwd.borrow().is_empty());
        self.build_crumbs();
        self.lib_header.set_visible(matches!(loc, Loc::Library(_)));
        if let Loc::Library(n) = &loc { self.lib_title.set_text(&format!("{n} library")); }
        self.load();
        self.update_cmd();
        self.update_info();
    }

    fn go_back(self: &Rc<Self>) {
        let Some(l) = self.back.borrow_mut().pop() else { return };
        let cur = self.loc.borrow().clone();
        self.fwd.borrow_mut().push(cur);
        self.go(l, false);
    }

    fn go_fwd(self: &Rc<Self>) {
        let Some(l) = self.fwd.borrow_mut().pop() else { return };
        let cur = self.loc.borrow().clone();
        self.back.borrow_mut().push(cur);
        self.go(l, false);
    }

    fn go_up(self: &Rc<Self>) {
        let loc = self.loc.borrow().clone();
        let up = match loc {
            Loc::Path(p) if p == config::home() => Some(Loc::Computer),
            Loc::Path(p) => p.parent().map(|q| Loc::Path(q.to_path_buf())).or(Some(Loc::Computer)),
            Loc::Library(_) | Loc::Trash | Loc::Network => None,
            Loc::Search(b, _) => Some(Loc::Path(b)),
            Loc::Computer => None,
        };
        if let Some(u) = up { self.go(u, true); }
    }

    // ------------------------------------------------------------ address bar
    fn build_crumbs(self: &Rc<Self>) {
        for c in self.crumbs.children() { self.crumbs.remove(&c); }
        let loc = self.loc.borrow().clone();
        self.crumbs.pack_start(&util::image(loc.icon(), 16), false, false, 4);
        let mut parts: Vec<(String, Loc)> = vec![];
        match &loc {
            Loc::Path(p) => {
                let home = config::home();
                if p.starts_with(&home) {
                    parts.push((glib::user_name().to_string_lossy().into_owned(), Loc::Path(home.clone())));
                    let mut acc = home.clone();
                    for c in p.strip_prefix(&home).unwrap().components() { acc = acc.join(c); parts.push((c.as_os_str().to_string_lossy().into_owned(), Loc::Path(acc.clone()))); }
                } else {
                    parts.push(("Computer".into(), Loc::Computer));
                    parts.push(("File System".into(), Loc::Path(PathBuf::from("/"))));
                    let mut acc = PathBuf::from("/");
                    for c in p.components().skip(1) { acc = acc.join(c); parts.push((c.as_os_str().to_string_lossy().into_owned(), Loc::Path(acc.clone()))); }
                }
            }
            Loc::Library(n) => { parts.push(("Libraries".into(), Loc::Library("Documents"))); parts.push((n.to_string(), loc.clone())); }
            Loc::Search(b, _) => { parts.push(("Search Results".into(), loc.clone())); let _ = b; }
            l => parts.push((l.title(), l.clone())),
        }
        // Win7 collapses the leftmost crumbs when space runs out; keep the last 6
        let skip = parts.len().saturating_sub(6);
        if skip > 0 { self.crumbs.pack_start(&gtk::Label::new(Some("«")), false, false, 2); }
        for (name, target) in parts.into_iter().skip(skip) {
            let arrow = gtk::MenuButton::new();
            arrow.set_label("▸");
            arrow.set_relief(gtk::ReliefStyle::None);
            arrow.set_focus_on_click(false);
            // dropdown: subfolders of this crumb
            if let Some(d) = target.dir() {
                let m = gtk::Menu::new();
                let mut subs: Vec<PathBuf> = std::fs::read_dir(&d).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.file_name().unwrap().to_string_lossy().starts_with('.')).collect()).unwrap_or_default();
                subs.sort_by(|a, b| natural_cmp(&a.to_string_lossy().to_lowercase(), &b.to_string_lossy().to_lowercase()));
                for s in subs.into_iter().take(40) {
                    let it = gtk::MenuItem::with_label(&s.file_name().unwrap().to_string_lossy());
                    let e = self.clone();
                    it.connect_activate(move |_| e.go(Loc::Path(s.clone()), true));
                    m.append(&it);
                }
                m.show_all();
                arrow.set_popup(Some(&m));
            }
            let b = gtk::Button::with_label(&name);
            let (e, t) = (self.clone(), target.clone());
            b.connect_clicked(move |_| e.go(t.clone(), true));
            self.crumbs.pack_start(&arrow, false, false, 0);
            self.crumbs.pack_start(&b, false, false, 0);
        }
        self.crumbs.show_all();
        self.addr_stack.set_visible_child_name("crumbs");
    }

    // ------------------------------------------------------------ listing
    fn icon_for(&self, info: &gio::FileInfo, path: &Path, size: i32) -> Option<Pixbuf> {
        let ct = info.content_type().map(|c| c.to_string()).unwrap_or_default();
        if size >= 48 && (ct.starts_with("image/")) {
            if let Some(t) = self.thumbs.borrow().get(path) { return Some(t.clone()); }
        }
        // special folders get their Win7 icons
        let special = [(util::special_dir(glib::UserDirectory::Documents, "Documents"), "folder-documents"),
            (util::special_dir(glib::UserDirectory::Music, "Music"), "folder-music"), (util::special_dir(glib::UserDirectory::Pictures, "Pictures"), "folder-pictures"),
            (util::special_dir(glib::UserDirectory::Videos, "Videos"), "folder-videos"), (util::special_dir(glib::UserDirectory::Downloads, "Downloads"), "folder-download"),
            (util::special_dir(glib::UserDirectory::Desktop, "Desktop"), "user-desktop")];
        for (p, i) in special { if p == path { if let Some(pb) = util::pixbuf(i, size) { return Some(pb); } } }
        util::gicon_pixbuf(info.icon().as_ref(), size)
    }

    pub fn load(self: &Rc<Self>) {
        self.store.clear();
        *self.monitor.borrow_mut() = None;
        let loc = self.loc.borrow().clone();
        if loc == Loc::Computer { self.show_computer(); return; }
        if loc == Loc::Network { self.show_network(); return; }
        self.apply_view();
        if let Loc::Search(base, q) = loc { self.run_search(base, q); return; }
        let Some(dir) = loc.dir() else { return };
        let gf = gio::File::for_path(&dir);
        let size = self.view.get().icon_size();
        let attrs = "standard::*,time::modified,thumbnail::path";
        match gf.enumerate_children(attrs, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
            Ok(en) => {
                let mut thumbs_needed = vec![];
                while let Ok(Some(info)) = en.next_file(gio::Cancellable::NONE) {
                    let p = dir.join(info.name());
                    self.add_row(&info, &p, size);
                    let ct = info.content_type().map(|c| c.to_string()).unwrap_or_default();
                    if size >= 48 && ct.starts_with("image/") && !self.thumbs.borrow().contains_key(&p) { thumbs_needed.push(p); }
                }
                self.load_thumbs(thumbs_needed);
            }
            Err(e) => util::error(&format!("{} is not accessible.", dir.display()), e.message()),
        }
        if let Ok(m) = gf.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
            let e = Rc::downgrade(self);
            let pending = Rc::new(Cell::new(false));
            m.connect_changed(move |_, _, _, _| {
                if pending.replace(true) { return; }
                let (e, p) = (e.clone(), pending.clone());
                glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || { p.set(false); if let Some(e) = e.upgrade() { e.reload_keep_selection(); } });
            });
            *self.monitor.borrow_mut() = Some(m);
        }
        self.update_info();
    }

    fn reload_keep_selection(self: &Rc<Self>) {
        let sel = self.selected();
        self.load();
        if sel.is_empty() { return; }
        self.for_each_row(|m, it, path| {
            let p: String = m.value(it, C_PATH as i32).get().unwrap_or_default();
            if sel.contains(&PathBuf::from(&p)) {
                match self.view.get() { View::Details => self.details.selection().select_path(path), _ => self.icons.select_path(path) }
            }
        });
    }

    fn for_each_row(&self, mut f: impl FnMut(&gtk::TreeModel, &gtk::TreeIter, &gtk::TreePath)) {
        let m: gtk::TreeModel = self.sorted.clone().upcast();
        m.foreach(|m, p, it| { f(m, it, p); false });
    }

    fn add_row(&self, info: &gio::FileInfo, p: &Path, size: i32) {
        let is_dir = info.file_type() == gio::FileType::Directory;
        let name = info.display_name().to_string();
        let ct = info.content_type().map(|c| c.to_string()).unwrap_or_default();
        let ty = if is_dir { "File folder".to_string() } else { gio::content_type_get_description(&ct).to_string() };
        let mtime = info.attribute_uint64("time::modified") as i64;
        let bytes = info.size() as u64;
        let size_s = if is_dir { String::new() } else { kb(bytes) };
        let tile = format!("{}\n<span foreground='#6d6d6d'>{}{}</span>", glib::markup_escape_text(&name), glib::markup_escape_text(&ty),
            if is_dir { String::new() } else { format!("\n{}", util::human_size(bytes)) });
        let pb = self.icon_for(info, p, size);
        let it = self.store.append();
        self.store.set(&it, &[(C_ICON, &pb), (C_NAME, &name), (C_DATE, &fmt_date(mtime)), (C_TYPE, &ty), (C_SIZE, &size_s),
            (C_PATH, &p.to_string_lossy().to_string()), (C_ISDIR, &is_dir), (C_SIZEN, &bytes), (C_DATEN, &mtime), (C_TILE, &tile), (C_SORTNAME, &name.to_lowercase())]);
    }

    fn load_thumbs(self: &Rc<Self>, paths: Vec<PathBuf>) {
        if paths.is_empty() { return; }
        let size = self.view.get().icon_size();
        let e = Rc::downgrade(self);
        let queue = RefCell::new(paths);
        glib::idle_add_local(move || {
            let Some(e) = e.upgrade() else { return glib::ControlFlow::Break };
            let Some(p) = queue.borrow_mut().pop() else { return glib::ControlFlow::Break };
            if let Ok(pb) = Pixbuf::from_file_at_scale(&p, size, size, true) {
                let pb = framed_thumb(&pb, size);
                e.thumbs.borrow_mut().insert(p.clone(), pb.clone());
                let ps = p.to_string_lossy().to_string();
                if let Some(it) = e.store.iter_first() {
                    loop {
                        let v: String = e.store.value(&it, C_PATH as i32).get().unwrap_or_default();
                        if v == ps { e.store.set_value(&it, C_ICON, &pb.to_value()); break; }
                        if !e.store.iter_next(&it) { break; }
                    }
                }
            }
            glib::ControlFlow::Continue
        });
    }

    fn run_search(self: &Rc<Self>, base: PathBuf, q: String) {
        let generation = self.search_gen.get() + 1;
        self.search_gen.set(generation);
        let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
        let ql = q.to_lowercase();
        std::thread::spawn(move || {
            let mut stack = vec![base];
            let mut n = 0;
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if name.starts_with('.') { continue; }
                    if name.contains(&ql) { if tx.send(e.path()).is_err() { return; } n += 1; if n > 5000 { return; } }
                    if e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) { stack.push(e.path()); }
                }
            }
        });
        let e = Rc::downgrade(self);
        let size = self.view.get().icon_size();
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            let Some(e) = e.upgrade() else { return glib::ControlFlow::Break };
            if e.search_gen.get() != generation { return glib::ControlFlow::Break; }
            loop {
                match rx.try_recv() {
                    Ok(p) => {
                        if let Ok(info) = gio::File::for_path(&p).query_info("standard::*,time::modified", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
                            e.add_row(&info, &p, size);
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => { e.update_info(); return glib::ControlFlow::Continue; }
                    Err(_) => { e.update_info(); return glib::ControlFlow::Break; }
                }
            }
        });
    }

    // ------------------------------------------------------------ Computer view
    fn show_computer(self: &Rc<Self>) {
        self.stack.set_visible_child_name("computer");
        for c in self.computer.children() { self.computer.remove(&c); }
        let mut disks: Vec<(String, PathBuf, &str)> = vec![("Local Disk (/)".into(), PathBuf::from("/"), "drive-harddisk")];
        let home = config::home();
        disks.push((format!("Home ({})", home.display()), home, "drive-harddisk"));
        let mut removable: Vec<(String, Option<PathBuf>, Option<gio::Volume>, String)> = vec![];
        for m in self.vm.mounts() {
            let Some(p) = m.root().path() else { continue };
            if p == Path::new("/") { continue; }
            let icon = if m.can_eject() || m.can_unmount() && p.starts_with("/run/media") { "drive-removable-media" } else { "drive-harddisk" };
            if icon == "drive-harddisk" { disks.push((m.name().to_string(), p, "drive-harddisk")); }
            else { removable.push((m.name().to_string(), Some(p), None, icon.to_string())); }
        }
        for v in self.vm.volumes() {
            if v.get_mount().is_some() { continue; }
            removable.push((v.name().to_string(), None, Some(v.clone()), if v.can_eject() { "drive-removable-media".into() } else { "drive-harddisk".into() }));
        }
        let group = |title: &str, n: usize| {
            let l = gtk::Label::new(Some(&format!("{title} ({n})")));
            l.set_xalign(0.0);
            l.style_context().add_class("drive-group");
            l
        };
        self.computer.pack_start(&group("Hard Disk Drives", disks.len()), false, false, 0);
        self.computer.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(4);
        for (name, p, icon) in disks { flow.insert(&self.drive_tile(&name, icon, Some(p), None), -1); }
        self.computer.pack_start(&flow, false, false, 0);
        if !removable.is_empty() {
            self.computer.pack_start(&group("Devices with Removable Storage", removable.len()), false, false, 0);
            self.computer.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
            let flow = gtk::FlowBox::new();
            flow.set_selection_mode(gtk::SelectionMode::None);
            for (name, p, vol, icon) in removable { flow.insert(&self.drive_tile(&name, &icon, p, vol), -1); }
            self.computer.pack_start(&flow, false, false, 0);
        }
        self.computer.show_all();
        self.info_icon.set_from_pixbuf(util::pixbuf("computer", 48).as_ref());
        self.info_name.set_text(&glib::host_name());
        let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))).unwrap_or_default();
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next().and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())).unwrap_or(0);
        self.info_text.set_text(&format!("Memory: {:.2} GB    Processor: {cpu}", mem as f64 / 1048576.0));
    }

    fn drive_tile(self: &Rc<Self>, name: &str, icon: &str, path: Option<PathBuf>, vol: Option<gio::Volume>) -> gtk::Widget {
        let b = gtk::Button::new();
        b.set_relief(gtk::ReliefStyle::None);
        b.style_context().add_class("drive-tile");
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        h.pack_start(&util::image(icon, 48), false, false, 0);
        let v = gtk::Box::new(gtk::Orientation::Vertical, 3);
        let l = gtk::Label::new(Some(name));
        l.set_xalign(0.0);
        v.pack_start(&l, false, false, 0);
        if let Some(p) = &path {
            if let Ok(info) = gio::File::for_path(p).query_filesystem_info("filesystem::size,filesystem::free", gio::Cancellable::NONE) {
                let (size, free) = (info.attribute_uint64("filesystem::size"), info.attribute_uint64("filesystem::free"));
                let bar = gtk::LevelBar::for_interval(0.0, 1.0);
                let used = if size > 0 { 1.0 - free as f64 / size as f64 } else { 0.0 };
                bar.set_value(used);
                bar.set_size_request(190, 14);
                // Win7 turns the bar red when a drive is >90% full
                bar.add_offset_value("full", 0.9);
                v.pack_start(&bar, false, false, 0);
                let t = gtk::Label::new(Some(&format!("{} free of {}", util::human_size(free), util::human_size(size))));
                t.set_xalign(0.0);
                t.style_context().add_class("dim-label");
                v.pack_start(&t, false, false, 0);
            }
        }
        h.pack_start(&v, false, false, 0);
        b.add(&h);
        let e = self.clone();
        b.connect_clicked(move |_| {
            if let Some(p) = &path { e.go(Loc::Path(p.clone()), true); }
            else if let Some(v) = &vol {
                let e2 = e.clone();
                v.mount(gio::MountMountFlags::NONE, gio::MountOperation::NONE, gio::Cancellable::NONE, move |r| {
                    if r.is_ok() { e2.load(); } else if let Err(err) = r { util::error("Could not mount the drive", err.message()); }
                });
            }
        });
        b.upcast()
    }

    fn show_network(self: &Rc<Self>) {
        self.apply_view();
        let size = self.view.get().icon_size();
        // gvfs network browsing if available
        let gf = gio::File::for_uri("network:///");
        if let Ok(en) = gf.enumerate_children("standard::*", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
            while let Ok(Some(info)) = en.next_file(gio::Cancellable::NONE) {
                let p = PathBuf::from(format!("network:///{}", info.name().display()));
                self.add_row(&info, &p, size);
            }
        }
        self.update_info();
    }

    // ------------------------------------------------------------ views
    fn apply_view(&self) {
        let v = self.view.get();
        if v == View::Details { self.stack.set_visible_child_name("details"); return; }
        self.stack.set_visible_child_name("icons");
        let ic = &self.icons;
        match v {
            View::Tiles => { ic.set_item_orientation(gtk::Orientation::Horizontal); ic.set_markup_column(C_TILE as i32); ic.set_item_width(220); }
            View::Small | View::List => { ic.set_markup_column(-1); ic.set_text_column(C_NAME as i32); ic.set_item_orientation(gtk::Orientation::Horizontal); ic.set_item_width(if v == View::List { 200 } else { 160 }); }
            _ => { ic.set_markup_column(-1); ic.set_text_column(C_NAME as i32); ic.set_item_orientation(gtk::Orientation::Vertical); ic.set_item_width(v.icon_size().max(72) + 8); }
        }
    }

    fn set_view(self: &Rc<Self>, v: View) {
        self.view.set(v);
        let _ = std::fs::create_dir_all(config::conf_dir());
        let _ = std::fs::write(config::conf_dir().join("explorer-view"), format!("{}", View::ALL.iter().position(|x| *x == v).unwrap_or(5)));
        self.thumbs.borrow_mut().clear();
        self.load();
    }

    // ------------------------------------------------------------ selection
    pub fn selected(&self) -> Vec<PathBuf> {
        let m: gtk::TreeModel = self.sorted.clone().upcast();
        let paths: Vec<gtk::TreePath> = if self.view.get() == View::Details { self.details.selection().selected_rows().0 } else { self.icons.selected_items() };
        paths.iter().filter_map(|p| m.iter(p)).map(|it| PathBuf::from(m.value(&it, C_PATH as i32).get::<String>().unwrap_or_default())).collect()
    }

    fn update_info(self: &Rc<Self>) {
        let sel = self.selected();
        let loc = self.loc.borrow().clone();
        if loc == Loc::Computer { return; }
        if sel.is_empty() {
            let n = self.sorted.iter_n_children(None);
            self.info_icon.set_from_pixbuf(util::pixbuf(loc.icon(), 48).as_ref());
            self.info_name.set_text(&format!("{n} item{}", if n == 1 { "" } else { "s" }));
            self.info_text.set_text("");
            self.preview.clear();
        } else if sel.len() == 1 {
            let p = &sel[0];
            let info = gio::File::for_path(p).query_info("standard::*,time::modified", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok();
            let pb = info.as_ref().and_then(|i| self.icon_for(i, p, 48));
            self.info_icon.set_from_pixbuf(pb.as_ref());
            self.info_name.set_text(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            if let Some(i) = info {
                let ct = i.content_type().map(|c| c.to_string()).unwrap_or_default();
                let ty = if i.file_type() == gio::FileType::Directory { "File folder".to_string() } else { gio::content_type_get_description(&ct).to_string() };
                let mt = fmt_date(i.attribute_uint64("time::modified") as i64);
                let size = if i.file_type() == gio::FileType::Directory { String::new() } else { format!("    Size: {}", util::human_size(i.size() as u64)) };
                self.info_text.set_text(&format!("{ty}    Date modified: {mt}{size}"));
                if self.preview_box.is_visible() {
                    if ct.starts_with("image/") { self.preview.set_from_pixbuf(Pixbuf::from_file_at_scale(p, 240, 400, true).ok().as_ref()); }
                    else { self.preview.set_from_pixbuf(util::pixbuf_or(&gio::content_type_get_generic_icon_name(&ct).map(|s| s.to_string()).unwrap_or_default(), 128, "text-x-generic").as_ref()); }
                }
            }
        } else {
            let total: u64 = sel.iter().filter_map(|p| std::fs::metadata(p).ok()).filter(|m| m.is_file()).map(|m| m.len()).sum();
            self.info_icon.set_from_pixbuf(util::pixbuf("text-x-generic", 48).as_ref());
            self.info_name.set_text(&format!("{} items selected", sel.len()));
            self.info_text.set_text(&format!("Size: {}", util::human_size(total)));
        }
        self.update_cmd();
    }

    /// Context-sensitive command bar buttons (Win7 changes them with the selection).
    fn update_cmd(self: &Rc<Self>) {
        for c in self.cmd_dyn.children() { self.cmd_dyn.remove(&c); }
        let sel = self.selected();
        let loc = self.loc.borrow().clone();
        let mut add = |label: &str, f: Box<dyn Fn()>| {
            let b = gtk::Button::with_label(label);
            b.set_relief(gtk::ReliefStyle::None);
            b.connect_clicked(move |_| f());
            self.cmd_dyn.pack_start(&b, false, false, 0);
        };
        if loc == Loc::Trash {
            let e = self.clone();
            add("Empty the Recycle Bin", Box::new(move || { ops::empty_trash(Some(e.win.upcast_ref())); e.load(); }));
            if !sel.is_empty() {
                let e = self.clone();
                add("Restore this item", Box::new(move || { for p in e.selected() { ops::restore(&p.file_name().unwrap().to_string_lossy()); } e.load(); }));
            } else {
                let e = self.clone();
                add("Restore all items", Box::new(move || {
                    if let Ok(rd) = std::fs::read_dir(glib::user_data_dir().join("Trash/files")) { for f in rd.flatten() { ops::restore(&f.file_name().to_string_lossy()); } }
                    e.load();
                }));
            }
        } else if loc == Loc::Computer {
            add("System properties", Box::new(|| { util::spawn(&["aerode-control", "system"]); }));
            add("Uninstall or change a program", Box::new(|| { util::spawn(&["aerode-control", "programs"]); }));
            add("Open Control Panel", Box::new(|| { util::spawn(&["aerode-control"]); }));
        } else {
            if sel.len() == 1 {
                let e = self.clone();
                add("Open", Box::new(move || e.activate_selected()));
                if sel[0].is_file() {
                    let p = sel[0].clone();
                    add("Print", Box::new(move || { util::spawn(&["lp", &p.to_string_lossy()]); }));
                    let p = sel[0].clone();
                    add("E-mail", Box::new(move || { if !util::spawn(&["xdg-email", "--attach", &p.to_string_lossy()]) { util::open_uri("mailto:"); } }));
                }
            }
            if sel.is_empty() {
                let e = self.clone();
                add("New folder", Box::new(move || e.new_folder()));
            }
        }
        self.cmd_dyn.show_all();
    }

    fn activate_selected(self: &Rc<Self>) {
        let sel = self.selected();
        if sel.len() == 1 && (sel[0].is_dir() || sel[0].to_string_lossy().starts_with("network:")) {
            let p = sel[0].clone();
            if p.to_string_lossy().starts_with("network:") { util::open_uri(&p.to_string_lossy()); return; }
            self.go(Loc::Path(p), true);
            return;
        }
        for p in sel {
            if p.extension().map(|e| e == "desktop").unwrap_or(false) {
                if let Some(a) = gio::DesktopAppInfo::from_filename(&p) { util::launch(a.upcast_ref()); continue; }
            }
            util::open_path(&p);
        }
    }

    fn cur_dir(&self) -> Option<PathBuf> {
        match &*self.loc.borrow() { Loc::Trash => None, l => l.dir() }
    }

    fn new_folder(self: &Rc<Self>) {
        let Some(d) = self.cur_dir() else { return };
        let mut p = d.join("New folder");
        let mut i = 2;
        while p.exists() { p = d.join(format!("New folder ({i})")); i += 1; }
        if let Err(e) = std::fs::create_dir(&p) { util::error("Could not create the folder", &e.to_string()); return; }
        self.load();
        self.select_and_rename(&p);
    }

    fn select_and_rename(self: &Rc<Self>, target: &Path) {
        let ts = target.to_string_lossy().to_string();
        let mut found: Option<gtk::TreePath> = None;
        self.for_each_row(|m, it, path| { let p: String = m.value(it, C_PATH as i32).get().unwrap_or_default(); if p == ts { found = Some(path.clone()); } });
        if let Some(path) = found {
            if self.view.get() == View::Details { self.details.selection().unselect_all(); self.details.selection().select_path(&path); }
            else { self.icons.unselect_all(); self.icons.select_path(&path); }
            self.rename();
        }
    }

    fn rename(self: &Rc<Self>) {
        let sel = self.selected();
        let Some(p) = sel.first().cloned() else { return };
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let pop = gtk::Popover::new(Some(&self.stack));
        let e = gtk::Entry::new();
        e.set_text(&name);
        pop.add(&e);
        pop.set_border_width(4);
        e.show();
        pop.popup();
        // select the name without extension, like Windows
        let stem_len = name.rfind('.').filter(|i| *i > 0 && p.is_file()).unwrap_or(name.chars().count()) as i32;
        e.select_region(0, stem_len);
        let (ex, pop2) = (self.clone(), pop.clone());
        e.connect_activate(move |e| {
            let new = e.text().to_string();
            if !new.is_empty() && new != name {
                if new.contains('/') { util::error("A file name can't contain the character /", ""); return; }
                let t = p.with_file_name(&new);
                if t.exists() { util::error(&format!("There is already a file with the name \"{new}\"."), ""); return; }
                if let Err(err) = std::fs::rename(&p, &t) { util::error("Could not rename", &err.to_string()); }
            }
            pop2.popdown();
            ex.load();
        });
    }

    fn copy(&self, cut: bool) { let s = self.selected(); if !s.is_empty() { ops::set_clipboard(&s, cut); } }

    fn paste(self: &Rc<Self>) {
        let Some(dest) = self.cur_dir() else { return };
        let e = self.clone();
        ops::get_clipboard(move |files, cut| {
            if files.is_empty() { return; }
            let e2 = e.clone();
            ops::transfer(files, dest, if cut { ops::Kind::Move } else { ops::Kind::Copy }, move || e2.load());
            if cut { gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).clear(); }
        });
    }

    fn delete(self: &Rc<Self>, permanent: bool) {
        let s = self.selected();
        let perm = permanent || *self.loc.borrow() == Loc::Trash;
        if ops::delete(&s, perm, Some(self.win.upcast_ref())) { self.load(); }
    }

    // ------------------------------------------------------------ menus
    fn context_menu(self: &Rc<Self>, ev: &gdk::EventButton) {
        let m = gtk::Menu::new();
        let sel = self.selected();
        let add = |m: &gtk::Menu, l: &str, f: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.connect_activate(move |_| f()); m.append(&it); it };
        if sel.is_empty() {
            let view = gtk::Menu::new();
            for v in View::ALL {
                let it = gtk::CheckMenuItem::with_label(v.label());
                it.set_draw_as_radio(true);
                it.set_active(self.view.get() == v);
                let e = self.clone();
                it.connect_activate(move |_| e.set_view(v));
                view.append(&it);
            }
            let vi = gtk::MenuItem::with_mnemonic("_View"); vi.set_submenu(Some(&view)); m.append(&vi);
            let sort = gtk::Menu::new();
            for (l, c) in [("Name", C_SORTNAME), ("Date modified", C_DATEN), ("Type", C_TYPE), ("Size", C_SIZEN)] {
                let it = gtk::MenuItem::with_label(l);
                let s = self.sorted.clone();
                it.connect_activate(move |_| s.set_sort_column_id(gtk::SortColumn::Index(c), gtk::SortType::Ascending));
                sort.append(&it);
            }
            let si = gtk::MenuItem::with_mnemonic("S_ort by"); si.set_submenu(Some(&sort)); m.append(&si);
            { let e = self.clone(); add(&m, "R_efresh", Box::new(move || e.load())); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let e = self.clone(); let it = add(&m, "_Paste", Box::new(move || e.paste())); it.set_sensitive(self.cur_dir().is_some()); }
            m.append(&gtk::SeparatorMenuItem::new());
            let new = gtk::Menu::new();
            { let e = self.clone(); add(&new, "_Folder", Box::new(move || e.new_folder())); }
            {
                let e = self.clone();
                add(&new, "Text Document", Box::new(move || {
                    if let Some(d) = e.cur_dir() {
                        let mut p = d.join("New Text Document.txt");
                        let mut i = 2;
                        while p.exists() { p = d.join(format!("New Text Document ({i}).txt")); i += 1; }
                        let _ = std::fs::write(&p, "");
                        e.load();
                        e.select_and_rename(&p);
                    }
                }));
            }
            let ni = gtk::MenuItem::with_mnemonic("Ne_w"); ni.set_submenu(Some(&new)); m.append(&ni);
            m.append(&gtk::SeparatorMenuItem::new());
            if let Some(d) = self.cur_dir() {
                // Win7 shows this with Shift+right-click; we always offer it
                let d2 = d.clone();
                add(&m, "Open command window here", Box::new(move || {
                    if let Some(t) = util::terminal() {
                        let _ = std::env::set_current_dir(&d2);
                        util::launch(&t);
                    }
                }));
                let d3 = d.clone();
                let e = self.clone();
                add(&m, "P_roperties", Box::new(move || props::show(&d3, Some(e.win.upcast_ref()))));
            }
        } else {
            { let e = self.clone(); let it = add(&m, "_Open", Box::new(move || e.activate_selected())); it.style_context().add_class("bold"); }
            if sel.len() == 1 && sel[0].is_file() {
                let ct = gio::content_type_guess(Some(&sel[0]), &[]).0.to_string();
                let apps = gio::AppInfo::all_for_type(&ct);
                if !apps.is_empty() {
                    let ow = gtk::Menu::new();
                    for a in apps.into_iter().take(10) {
                        let it = gtk::MenuItem::new();
                        let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                        h.pack_start(&util::app_image(&a, 16), false, false, 0);
                        h.pack_start(&gtk::Label::new(Some(&a.display_name())), false, false, 0);
                        it.add(&h);
                        let p = sel[0].clone();
                        it.connect_activate(move |_| { let _ = a.launch(&[gio::File::for_path(&p)], None::<&gio::AppLaunchContext>); });
                        ow.append(&it);
                    }
                    ow.append(&gtk::SeparatorMenuItem::new());
                    let p = sel[0].clone();
                    let w = self.win.clone();
                    let choose = gtk::MenuItem::with_label("Choose default program...");
                    choose.connect_activate(move |_| {
                        let d = gtk::AppChooserDialog::new(Some(&w), gtk::DialogFlags::MODAL, &gio::File::for_path(&p));
                        if d.run() == gtk::ResponseType::Ok { if let Some(a) = d.app_info() { let _ = a.launch(&[gio::File::for_path(&p)], None::<&gio::AppLaunchContext>); } }
                        d.close();
                    });
                    ow.append(&choose);
                    let oi = gtk::MenuItem::with_mnemonic("Open wit_h"); oi.set_submenu(Some(&ow)); m.append(&oi);
                }
            }
            if sel.len() == 1 && sel[0].is_dir() {
                let p = sel[0].clone();
                add(&m, "Open in new window", Box::new(move || { util::spawn(&["aerode-explorer", &p.to_string_lossy()]); }));
            }
            m.append(&gtk::SeparatorMenuItem::new());
            let send = gtk::Menu::new();
            {
                let s = sel.clone();
                add(&send, "Compressed (zipped) folder", Box::new(move || {
                    let first = &s[0];
                    let zip = first.with_extension("zip");
                    let mut args = vec!["-r".to_string(), zip.to_string_lossy().into_owned()];
                    args.extend(s.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()));
                    let _ = std::process::Command::new("zip").args(&args).current_dir(first.parent().unwrap()).spawn();
                }));
                let s = sel.clone();
                add(&send, "Desktop (create shortcut)", Box::new(move || {
                    for p in &s {
                        let d = util::special_dir(glib::UserDirectory::Desktop, "Desktop").join(p.file_name().unwrap());
                        let _ = std::os::unix::fs::symlink(p, d);
                    }
                }));
            }
            let sti = gtk::MenuItem::with_mnemonic("Se_nd to"); sti.set_submenu(Some(&send)); m.append(&sti);
            m.append(&gtk::SeparatorMenuItem::new());
            { let e = self.clone(); add(&m, "Cu_t", Box::new(move || e.copy(true))); }
            { let e = self.clone(); add(&m, "_Copy", Box::new(move || e.copy(false))); }
            m.append(&gtk::SeparatorMenuItem::new());
            {
                let s = sel.clone();
                let e = self.clone();
                add(&m, "Create _shortcut", Box::new(move || {
                    for p in &s {
                        let link = p.with_file_name(format!("{} - Shortcut", p.file_name().unwrap().to_string_lossy()));
                        let _ = std::os::unix::fs::symlink(p, link);
                    }
                    e.load();
                }));
            }
            { let e = self.clone(); add(&m, "_Delete", Box::new(move || e.delete(false))); }
            { let e = self.clone(); let it = add(&m, "Rena_me", Box::new(move || e.rename())); it.set_sensitive(sel.len() == 1); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let (e, p) = (self.clone(), sel[0].clone()); add(&m, "P_roperties", Box::new(move || props::show(&p, Some(e.win.upcast_ref())))); }
        }
        m.show_all();
        m.popup_at_pointer(Some(ev));
    }

    fn build_nav(self: &Rc<Self>) {
        let s = &self.nav_store;
        let add = |parent: Option<&gtk::TreeIter>, icon: &str, name: &str, target: &str| {
            let it = s.append(parent);
            s.set(&it, &[(0, &util::pixbuf_or(icon, 16, "folder")), (1, &name), (2, &target)]);
            it
        };
        let fav = add(None, util::first_icon(&["starred", "emblem-favorite", "folder"]), "Favorites", "");
        add(Some(&fav), "user-desktop", "Desktop", &util::special_dir(glib::UserDirectory::Desktop, "Desktop").to_string_lossy());
        add(Some(&fav), "folder-download", "Downloads", &util::special_dir(glib::UserDirectory::Downloads, "Downloads").to_string_lossy());
        add(Some(&fav), "folder-recent", "Recent Places", "recent:");
        let libs = add(None, "folder-documents", "Libraries", "library:Documents");
        for (n, i) in [("Documents", "folder-documents"), ("Music", "folder-music"), ("Pictures", "folder-pictures"), ("Videos", "folder-videos")] {
            add(Some(&libs), i, n, &format!("library:{n}"));
        }
        add(None, "user-home", &glib::user_name().to_string_lossy(), &config::home().to_string_lossy());
        let comp = add(None, "computer", "Computer", "computer:///");
        add(Some(&comp), "drive-harddisk", "Local Disk (/)", "/");
        for m in self.vm.mounts() {
            if let Some(p) = m.root().path() { if p != Path::new("/") { add(Some(&comp), "drive-removable-media", &m.name(), &p.to_string_lossy()); } }
        }
        add(None, "network-workgroup", "Network", "network:///");
        add(None, "user-trash", "Recycle Bin", "trash:///");
        self.nav.expand_all();
    }

    fn wire(self: &Rc<Self>, organize: &gtk::MenuButton, views_b: &gtk::MenuButton, prev_b: &gtk::ToggleButton, refresh: &gtk::Button, crumb_ev: &gtk::EventBox) {
        { let e = self.clone(); self.back_b.connect_clicked(move |_| e.go_back()); }
        { let e = self.clone(); self.fwd_b.connect_clicked(move |_| e.go_fwd()); }
        { let e = self.clone(); refresh.connect_clicked(move |_| e.load()); }
        // clicking empty address space switches to an editable path, like Win7
        {
            let e = self.clone();
            crumb_ev.connect_button_press_event(move |_, _| {
                let t = match &*e.loc.borrow() { Loc::Path(p) => p.display().to_string(), l => l.title() };
                e.addr_entry.set_text(&t);
                e.addr_stack.set_visible_child_name("entry");
                e.addr_entry.grab_focus();
                glib::Propagation::Stop
            });
        }
        {
            let e = self.clone();
            self.addr_entry.connect_activate(move |en| { let t = en.text().to_string(); e.go(Loc::parse(&t), true); });
        }
        {
            let e = self.clone();
            self.addr_entry.connect_focus_out_event(move |_, _| { e.addr_stack.set_visible_child_name("crumbs"); glib::Propagation::Proceed });
        }
        // search: instant filter, Enter = recursive search
        { let e = self.clone(); self.search.connect_changed(move |_| e.filter.refilter()); }
        {
            let e = self.clone();
            self.search.connect_activate(move |s| {
                let q = s.text().to_string();
                if q.is_empty() { return; }
                let base = e.cur_dir().unwrap_or_else(config::home);
                e.go(Loc::Search(base, q), true);
            });
        }
        // organize menu
        let m = gtk::Menu::new();
        let add = |m: &gtk::Menu, l: &str, f: Box<dyn Fn()>| { let it = gtk::MenuItem::with_mnemonic(l); it.connect_activate(move |_| f()); m.append(&it); };
        { let e = self.clone(); add(&m, "Cu_t", Box::new(move || e.copy(true))); }
        { let e = self.clone(); add(&m, "_Copy", Box::new(move || e.copy(false))); }
        { let e = self.clone(); add(&m, "_Paste", Box::new(move || e.paste())); }
        m.append(&gtk::SeparatorMenuItem::new());
        {
            let e = self.clone();
            add(&m, "Select _all", Box::new(move || if e.view.get() == View::Details { e.details.selection().select_all() } else { e.icons.select_all() }));
        }
        m.append(&gtk::SeparatorMenuItem::new());
        let layout = gtk::Menu::new();
        let dp = gtk::CheckMenuItem::with_label("Details pane");
        dp.set_active(true);
        { let w = self.info_name.parent().and_then(|p| p.parent()); dp.connect_toggled(move |c| if let Some(w) = &w { w.set_visible(c.is_active()) }); }
        layout.append(&dp);
        let pp = gtk::CheckMenuItem::with_label("Preview pane");
        { let pb = prev_b.clone(); pp.connect_toggled(move |c| pb.set_active(c.is_active())); }
        layout.append(&pp);
        let np = gtk::CheckMenuItem::with_label("Navigation pane");
        np.set_active(true);
        { let n = self.nav.parent(); np.connect_toggled(move |c| if let Some(n) = &n { n.set_visible(c.is_active()) }); }
        layout.append(&np);
        let li = gtk::MenuItem::with_mnemonic("_Layout"); li.set_submenu(Some(&layout)); m.append(&li);
        {
            let e = self.clone();
            let it = gtk::CheckMenuItem::with_label("Show hidden files and folders");
            it.connect_toggled(move |c| { e.show_hidden.set(c.is_active()); e.filter.refilter(); });
            m.append(&it);
        }
        m.append(&gtk::SeparatorMenuItem::new());
        { let e = self.clone(); add(&m, "_Delete", Box::new(move || e.delete(false))); }
        { let e = self.clone(); add(&m, "Rena_me", Box::new(move || e.rename())); }
        m.append(&gtk::SeparatorMenuItem::new());
        {
            let e = self.clone();
            add(&m, "P_roperties", Box::new(move || {
                let s = e.selected();
                let p = s.first().cloned().or_else(|| e.cur_dir());
                if let Some(p) = p { props::show(&p, Some(e.win.upcast_ref())); }
            }));
        }
        m.append(&gtk::SeparatorMenuItem::new());
        { let w = self.win.clone(); add(&m, "_Close", Box::new(move || w.close())); }
        m.show_all();
        organize.set_popup(Some(&m));
        // views menu
        let vm = gtk::Menu::new();
        for v in View::ALL {
            let it = gtk::CheckMenuItem::with_label(v.label());
            it.set_draw_as_radio(true);
            let e = self.clone();
            it.connect_activate(move |_| e.set_view(v));
            vm.append(&it);
        }
        {
            let e = self.clone();
            vm.connect_show(move |m| {
                for (i, c) in m.children().iter().enumerate() {
                    if let Ok(ci) = c.clone().downcast::<gtk::CheckMenuItem>() { ci.set_active(View::ALL.get(i) == Some(&e.view.get())); }
                }
            });
        }
        vm.show_all();
        views_b.set_popup(Some(&vm));
        { let (pbx, e) = (self.preview_box.clone(), self.clone()); prev_b.connect_toggled(move |b| { pbx.set_visible(b.is_active()); e.update_info(); }); }
        // nav tree
        {
            let e = self.clone();
            self.nav.selection().connect_changed(move |s| {
                let Some((m, it)) = s.selected() else { return };
                let t: String = m.value(&it, 2).get().unwrap_or_default();
                if t.is_empty() { return; }
                if t == "recent:" {
                    if let Some(rm) = gtk::RecentManager::default() {
                        // Recent Places: folders of recently used files
                        let mut dirs: Vec<PathBuf> = rm.items().iter().filter_map(|i| gio::File::for_uri(&i.uri()?).path()?.parent().map(|p| p.to_path_buf())).collect();
                        dirs.dedup();
                        if let Some(d) = dirs.first() { e.go(Loc::Path(d.clone()), true); }
                    }
                    return;
                }
                let loc = Loc::parse(&t);
                if *e.loc.borrow() != loc { e.go(loc, true); }
            });
        }
        // activation & selection in both views
        { let e = self.clone(); self.icons.connect_item_activated(move |_, _| e.activate_selected()); }
        { let e = self.clone(); self.details.connect_row_activated(move |_, _, _| e.activate_selected()); }
        { let e = self.clone(); self.icons.connect_selection_changed(move |_| e.update_info()); }
        { let e = self.clone(); self.details.selection().connect_changed(move |_| e.update_info()); }
        for w in [self.icons.clone().upcast::<gtk::Widget>(), self.details.clone().upcast()] {
            let e = self.clone();
            w.connect_button_press_event(move |w, ev| {
                if ev.button() == 3 {
                    // select what's under the pointer first
                    let (x, y) = ev.position();
                    if let Ok(iv) = w.clone().downcast::<gtk::IconView>() {
                        match iv.path_at_pos(x as i32, y as i32) { Some(p) => if !iv.path_is_selected(&p) { iv.unselect_all(); iv.select_path(&p); }, None => iv.unselect_all() }
                    } else if let Ok(tv) = w.clone().downcast::<gtk::TreeView>() {
                        match tv.path_at_pos(x as i32, y as i32) { Some((Some(p), ..)) => if !tv.selection().path_is_selected(&p) { tv.selection().unselect_all(); tv.selection().select_path(&p); }, _ => tv.selection().unselect_all() }
                    }
                    e.context_menu(ev);
                    return glib::Propagation::Stop;
                }
                if ev.button() == 8 { e.go_back(); return glib::Propagation::Stop; }
                if ev.button() == 9 { e.go_fwd(); return glib::Propagation::Stop; }
                glib::Propagation::Proceed
            });
        }
        // drag & drop (Win7: same drive = move, other drive = copy; Ctrl copy, Shift move)
        let targets = [gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0)];
        for w in [self.icons.clone().upcast::<gtk::Widget>(), self.details.clone().upcast()] {
            w.drag_source_set(gdk::ModifierType::BUTTON1_MASK, &targets, gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK);
            w.drag_dest_set(gtk::DestDefaults::ALL, &targets, gdk::DragAction::COPY | gdk::DragAction::MOVE);
        }
        for w in [self.icons.clone().upcast::<gtk::Widget>(), self.details.clone().upcast()] {
            let e = self.clone();
            w.connect_drag_data_get(move |_, _, data, _, _| {
                let uris: Vec<String> = e.selected().iter().map(|p| gio::File::for_path(p).uri().to_string()).collect();
                let refs: Vec<&str> = uris.iter().map(|s| s.as_str()).collect();
                data.set_uris(&refs);
            });
            let e = self.clone();
            w.connect_drag_data_received(move |w, ctx, x, y, data, _, time| {
                let files: Vec<PathBuf> = data.uris().iter().filter_map(|u| gio::File::for_uri(u).path()).collect();
                // drop onto a folder item, else into the current folder
                let mut dest = e.cur_dir();
                let m: gtk::TreeModel = e.sorted.clone().upcast();
                let hit = if let Ok(iv) = w.clone().downcast::<gtk::IconView>() { iv.path_at_pos(x, y) } else if let Ok(tv) = w.clone().downcast::<gtk::TreeView>() { tv.dest_row_at_pos(x, y).and_then(|(p, _)| p) } else { None };
                if let Some(it) = hit.and_then(|p| m.iter(&p)) {
                    if m.value(&it, C_ISDIR as i32).get::<bool>().unwrap_or(false) { dest = Some(PathBuf::from(m.value(&it, C_PATH as i32).get::<String>().unwrap_or_default())); }
                }
                if let Some(dest) = dest {
                    let same_fs = files.first().and_then(|f| { use std::os::unix::fs::MetadataExt; Some(std::fs::metadata(f).ok()?.dev() == std::fs::metadata(&dest).ok()?.dev()) }).unwrap_or(false);
                    let act = ctx.selected_action();
                    let kind = if act == gdk::DragAction::COPY && !same_fs { ops::Kind::Copy } else if act == gdk::DragAction::MOVE || same_fs { ops::Kind::Move } else { ops::Kind::Copy };
                    let e2 = e.clone();
                    ops::transfer(files, dest, kind, move || e2.load());
                }
                let _ = time;
                w.stop_signal_emission_by_name("drag-data-received");
            });
        }
        // keyboard
        {
            let e = self.clone();
            self.win.connect_key_press_event(move |_, ev| {
                use gdk::keys::constants as K;
                let k = ev.keyval();
                let st = ev.state();
                let ctrl = st.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = st.contains(gdk::ModifierType::SHIFT_MASK);
                let alt = st.contains(gdk::ModifierType::MOD1_MASK);
                let in_entry = e.search.has_focus() || e.addr_entry.has_focus();
                if in_entry && k != K::Escape && k != K::F5 { return glib::Propagation::Proceed; }
                match k {
                    k if k == K::BackSpace => e.go_up(),
                    k if k == K::Left && alt => e.go_back(),
                    k if k == K::Right && alt => e.go_fwd(),
                    k if k == K::Up && alt => e.go_up(),
                    k if k == K::F5 => e.load(),
                    k if k == K::F2 => e.rename(),
                    k if k == K::Delete => e.delete(shift),
                    k if k == K::Return && alt => { if let Some(p) = e.selected().first().cloned().or_else(|| e.cur_dir()) { props::show(&p, Some(e.win.upcast_ref())); } }
                    k if (k == K::c || k == K::C) && ctrl => e.copy(false),
                    k if (k == K::x || k == K::X) && ctrl => e.copy(true),
                    k if (k == K::v || k == K::V) && ctrl => e.paste(),
                    k if (k == K::a || k == K::A) && ctrl => if e.view.get() == View::Details { e.details.selection().select_all() } else { e.icons.select_all() },
                    k if (k == K::n || k == K::N) && ctrl && shift => e.new_folder(),
                    k if (k == K::n || k == K::N) && ctrl => { let t = match &*e.loc.borrow() { Loc::Path(p) => p.display().to_string(), _ => "computer:///".into() }; util::spawn(&["aerode-explorer", &t]); }
                    k if (k == K::e || k == K::E || k == K::f || k == K::F) && ctrl => e.search.grab_focus(),
                    k if (k == K::l || k == K::L) && ctrl || k == K::F4 => { let t = match &*e.loc.borrow() { Loc::Path(p) => p.display().to_string(), l => l.title() }; e.addr_entry.set_text(&t); e.addr_stack.set_visible_child_name("entry"); e.addr_entry.grab_focus(); }
                    k if (k == K::w || k == K::W) && ctrl => e.win.close(),
                    k if k == K::Escape => { e.search.set_text(""); e.addr_stack.set_visible_child_name("crumbs"); }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            });
        }
    }
}

fn load_view() -> View {
    std::fs::read_to_string(config::conf_dir().join("explorer-view")).ok().and_then(|s| s.trim().parse::<usize>().ok()).and_then(|i| View::ALL.get(i).copied()).unwrap_or(View::Details)
}

/// Win7 draws image thumbnails with a thin frame and drop shadow.
fn framed_thumb(pb: &Pixbuf, size: i32) -> Pixbuf {
    let (w, h) = (pb.width(), pb.height());
    let surf = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, size, size).unwrap();
    {
        let cr = gtk::cairo::Context::new(&surf).unwrap();
        let (x, y) = (((size - w) / 2) as f64, ((size - h) / 2) as f64);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.18);
        cr.rectangle(x + 2.0, y + 2.0, w as f64, h as f64);
        cr.fill().ok();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(x - 1.0, y - 1.0, w as f64 + 2.0, h as f64 + 2.0);
        cr.fill().ok();
        cr.set_source_pixbuf(pb, x, y);
        cr.paint().ok();
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
        cr.set_line_width(1.0);
        cr.rectangle(x - 0.5, y - 0.5, w as f64 + 1.0, h as f64 + 1.0);
        cr.stroke().ok();
    }
    gdk::pixbuf_get_from_surface(&surf, 0, 0, size, size).unwrap_or_else(|| pb.clone())
}

/// "file2" < "file10" like Windows Explorer.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new(); while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) { na.push(c); ai.next(); }
                let mut nb = String::new(); while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) { nb.push(c); bi.next(); }
                let o = na.trim_start_matches('0').len().cmp(&nb.trim_start_matches('0').len()).then(na.trim_start_matches('0').cmp(nb.trim_start_matches('0')));
                if o != std::cmp::Ordering::Equal { return o; }
            }
            (Some(x), Some(y)) => { if x != y { return x.cmp(&y); } ai.next(); bi.next(); }
        }
    }
}

/// Same colour the window frame uses (so the header glass matches).
fn aerode_frame_color() -> [f64; 4] { crate::wm::colorization(&config::get().tint) }
