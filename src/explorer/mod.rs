//! Windows Explorer remake.
mod grid;
pub mod listing;
pub mod nav;
pub mod ops;
pub mod props;
pub mod trash;

use gtk::gdk::prelude::GdkContextExt;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{config, glass, sounds, util};
use listing::{fmt_date, Entry, ScanMsg};
pub use nav::library_dir;

#[derive(Clone, Debug, PartialEq)]
pub enum Loc {
    Path(PathBuf),
    Computer,
    Trash,
    Network,
    Libraries,
    Library(&'static str),
    Recent,
    Search(PathBuf, String),
}

impl Loc {
    pub fn parse(s: &str) -> Loc {
        match s {
            "computer:///" | "computer:" | "computer://" | "Computer" => Loc::Computer,
            "trash:///" | "trash:" | "trash://" => Loc::Trash,
            "network:///" | "network:" | "network://" => Loc::Network,
            "libraries:" | "libraries:///" => Loc::Libraries,
            "recent:" | "recent:///" => Loc::Recent,
            s if s.starts_with("file://") => gio::File::for_uri(s).path().map(Loc::Path).unwrap_or(Loc::Computer),
            s if s.starts_with("library:") => match &s[8..] { "Music" => Loc::Library("Music"), "Pictures" => Loc::Library("Pictures"), "Videos" => Loc::Library("Videos"), _ => Loc::Library("Documents") },
            s => Loc::Path(PathBuf::from(util::shellexpand(s))),
        }
    }

    /// The string the navigation pane uses for this place.
    fn target(&self) -> String {
        match self {
            Loc::Path(p) => p.to_string_lossy().into_owned(),
            Loc::Computer => "computer:///".into(),
            Loc::Trash => "trash:///".into(),
            Loc::Network => "network:///".into(),
            Loc::Libraries => "libraries:".into(),
            Loc::Library(n) => format!("library:{n}"),
            Loc::Recent => "recent:".into(),
            Loc::Search(..) => String::new(),
        }
    }

    /// What the navigation pane calls this place (folders as file URIs: lossless).
    fn nav_target(&self) -> String {
        match self { Loc::Path(p) => uri_of(p), l => l.target() }
    }

    /// Key for remembered view settings.
    fn key(&self) -> String {
        match self { Loc::Search(..) => "search:".into(), l => l.target() }
    }

    fn title(&self) -> String {
        match self {
            Loc::Path(p) if p == &config::home() => glib::user_name().to_string_lossy().into_owned(),
            Loc::Path(p) if p == Path::new("/") => "Local Disk (/)".into(),
            Loc::Path(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
            Loc::Computer => "Computer".into(),
            Loc::Trash => "Recycle Bin".into(),
            Loc::Network => "Network".into(),
            Loc::Libraries => "Libraries".into(),
            Loc::Library(n) => n.to_string(),
            Loc::Recent => "Recent Places".into(),
            Loc::Search(b, _) => format!("Search Results in {}", Loc::Path(b.clone()).title()),
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Loc::Computer => "computer", Loc::Network => "network-workgroup",
            Loc::Trash => if trash::is_empty() { "user-trash" } else { "user-trash-full" },
            Loc::Libraries => util::first_icon(&["folder-libraries", "system-file-manager", "folder-documents"]),
            Loc::Library("Music") => "folder-music", Loc::Library("Pictures") => "folder-pictures",
            Loc::Library("Videos") => "folder-videos", Loc::Library(_) => "folder-documents",
            Loc::Recent => util::first_icon(&["folder-recent", "document-open-recent", "folder"]),
            Loc::Search(..) => "system-search",
            Loc::Path(p) => nav::folder_icon(p),
        }
    }

    /// The real folder shown (None for virtual places).
    fn dir(&self) -> Option<PathBuf> {
        match self {
            Loc::Path(p) => Some(p.clone()),
            Loc::Library(n) => Some(library_dir(n)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum View { XLarge, Large, Medium, Small, List, Details, Tiles, Content }

impl View {
    const ALL: [View; 8] = [View::XLarge, View::Large, View::Medium, View::Small, View::List, View::Details, View::Tiles, View::Content];
    fn icon_size(self) -> i32 { match self { View::XLarge => 256, View::Large => 96, View::Medium => 48, View::Tiles => 48, View::Content => 32, _ => 16 } }
    /// labels of the command bar Views menu
    fn label(self) -> &'static str {
        match self { View::XLarge => "Extra Large Icons", View::Large => "Large Icons", View::Medium => "Medium Icons", View::Small => "Small Icons", View::List => "List", View::Details => "Details", View::Tiles => "Tiles", View::Content => "Content" }
    }
    /// labels of the context menu View submenu
    fn menu_label(self) -> &'static str {
        match self { View::XLarge => "E_xtra large icons", View::Large => "_Large icons", View::Medium => "Medi_um icons", View::Small => "Small icon_s", View::List => "L_ist", View::Details => "_Details", View::Tiles => "_Tiles", View::Content => "Co_ntent" }
    }
    fn icon_name(self) -> &'static str {
        match self { View::Details => "view-list-details-symbolic", View::List => "view-list-symbolic", View::Content => "view-list-compact-symbolic", _ => "view-grid-symbolic" }
    }
    fn index(self) -> usize { View::ALL.iter().position(|v| *v == self).unwrap_or(5) }
}

// ---------------------------------------------------------------- remembered views

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, PartialEq)]
struct FolderView { v: usize, s: u32, d: bool }

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct ViewStore {
    folders: HashMap<String, FolderView>,
    order: Vec<String>,
    details_pane: Option<bool>,
    preview_pane: Option<bool>,
    nav_pane: Option<bool>,
}

thread_local! {
    static VIEWS: RefCell<Option<ViewStore>> = const { RefCell::new(None) };
}

fn views_file() -> PathBuf { config::conf_dir().join("explorer-view") }

fn with_views<R>(f: impl FnOnce(&mut ViewStore) -> R) -> R {
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        // Folder Options > Reset Folders deletes the file: start over then
        if v.is_none() || !views_file().exists() {
            // the old format was one number for every folder
            let text = std::fs::read_to_string(views_file()).unwrap_or_default();
            *v = Some(serde_json::from_str(&text).unwrap_or_default());
        }
        f(v.as_mut().unwrap())
    })
}

fn save_views() {
    VIEWS.with(|v| {
        if let Some(v) = v.borrow().as_ref() {
            let _ = std::fs::create_dir_all(config::conf_dir());
            if let Ok(s) = serde_json::to_string(v) { let _ = std::fs::write(views_file(), s); }
        }
    });
}

/// Win7 picks a view by folder type: pictures and videos get Large Icons, search results Content,
/// libraries Tiles, everything else Details.
fn default_view(loc: &Loc) -> View {
    let pics = [library_dir("Pictures"), library_dir("Videos")];
    match loc {
        Loc::Search(..) => View::Content,
        Loc::Libraries | Loc::Computer => View::Tiles,
        Loc::Library("Pictures") | Loc::Library("Videos") => View::Large,
        Loc::Path(p) if pics.iter().any(|d| p.starts_with(d)) => View::Large,
        _ => View::Details,
    }
}

// ---------------------------------------------------------------- model

const C_ICON: u32 = 0;
const C_NAME: u32 = 1;
const C_DATE: u32 = 2;
const C_TYPE: u32 = 3;
const C_SIZE: u32 = 4;
const C_URI: u32 = 5;
const C_ISDIR: u32 = 6;
const C_SIZEN: u32 = 7;
const C_DATEN: u32 = 8;
const C_TILE: u32 = 9;
const C_SORT: u32 = 10;
const C_CONTENT: u32 = 11;
const C_ORIG: u32 = 12;
const C_DEL: u32 = 13;
const C_DELN: u32 = 14;
const C_SENS: u32 = 15;
const C_EDIT: u32 = 16;
const C_CT: u32 = 17;
const C_CNAME: u32 = 18;

/// The file list model. It is not a GtkTreeSortable sort: rows are appended and then put in order
/// with one Rust sort + gtk_list_store_reorder (GTK's sorted insertion calls back per row and made
/// a 10 000-file folder take seconds).
fn new_store() -> gtk::ListStore {
    let s = glib::Type::STRING;
    gtk::ListStore::new(&[Pixbuf::static_type(), s, s, s, s, s, glib::Type::BOOL, glib::Type::U64, glib::Type::I64, s, s, s, s, s, glib::Type::I64, glib::Type::BOOL, s, s, s])
}

/// Windows order: folders together, then the column, ties by name (natural order); descending
/// reverses all of it (folders go last, like Explorer).
fn cmp_entries(a: &Entry, b: &Entry, col: u32, desc: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let del = |e: &Entry| e.trash.as_ref().map(|t| t.1).unwrap_or(0);
    let o = b.is_dir.cmp(&a.is_dir).then_with(|| match col {
        C_DATEN => a.mtime.cmp(&b.mtime),
        C_DELN => del(a).cmp(&del(b)),
        C_SIZEN => a.size.cmp(&b.size),
        C_TYPE => a.text.type_key.cmp(&b.text.type_key),
        C_ORIG => a.text.orig.to_lowercase().cmp(&b.text.orig.to_lowercase()),
        _ => Ordering::Equal,
    }).then_with(|| a.text.key.cmp(&b.text.key));
    if desc { o.reverse() } else { o }
}

fn uri_of(p: &Path) -> String { gio::File::for_path(p).uri().to_string() }

fn path_of_uri(u: &str) -> Option<PathBuf> {
    if u.starts_with("file://") { gio::File::for_uri(u).path() } else { None }
}

pub const CSS: &str = r#"
.exp-header { padding: 4px 6px 5px 6px; }
.exp-header button.nav { min-width: 26px; min-height: 26px; padding: 0; border-radius: 50%;
  background-image: radial-gradient(circle at 50% 35%, #cfe9ff, #4f9be3 55%, #1f5fae); border: 1px solid #23466f; box-shadow: inset 0 1px 0 rgba(255,255,255,0.7); }
.exp-header button.nav:hover { background-image: radial-gradient(circle at 50% 35%, #e9f6ff, #6fbaf6 55%, #2a75c7); }
.exp-header button.nav:disabled { background-image: radial-gradient(circle at 50% 35%, #f4f6f8, #c8d0da 60%, #9aa6b4); border-color: #7d8896; }
.exp-header button.nav image { color: #fff; -gtk-icon-shadow: 0 1px 1px rgba(0,0,0,0.5); }
.exp-header button.recent { min-width: 12px; min-height: 20px; padding: 0; border: none; background: none; box-shadow: none; }
.exp-addr { background-color: rgba(255,255,255,0.92); border: 1px solid rgba(0,0,0,0.55); border-radius: 2px; min-height: 22px; }
.exp-addr button { background: none; border: 1px solid transparent; border-radius: 0; box-shadow: none; padding: 0 3px; min-height: 20px; color: #000; }
.exp-addr button:hover, .exp-addr button.open { background-image: linear-gradient(to bottom, #eaf6fd, #bfe2f8); border-color: #3c7fb1; }
.exp-addr button.crumb-arrow { padding: 0 2px; min-width: 9px; }
.exp-addr entry { border: none; background: transparent; box-shadow: none; }
.exp-addr.searching { background-image: linear-gradient(to right, rgba(140,220,120,0.55), rgba(255,255,255,0.92)); }
.exp-search { background-color: rgba(255,255,255,0.92); border: 1px solid rgba(0,0,0,0.55); border-radius: 2px; min-height: 22px; }
.exp-search.empty { font-style: italic; }
.commandbar { background-image: linear-gradient(to bottom, #f5faff, #e5eef9 50%, #d9e6f6 51%, #e2ecf8); border-top: 1px solid #ffffff; border-bottom: 1px solid #a0afc3; padding: 2px 6px; }
.commandbar button { color: #1e395b; background: none; border: 1px solid transparent; box-shadow: none; padding: 2px 8px; }
.commandbar button:hover, .commandbar button:checked { background-image: linear-gradient(to bottom, #fbfdff, #e4effb 50%, #d4e6f9 51%, #dcecfd); border-color: #a7cdf0; border-radius: 3px; }
.commandbar .viewsplit button { padding: 2px 3px; }
.exp-nav { background-color: #ffffff; border-right: 1px solid #d6e5f5; }
.exp-nav treeview.view { background-color: #ffffff; }
.exp-nav treeview.view:selected { background-image: linear-gradient(to bottom, #dcebfc, #c1dbfc); color: #000; }
.exp-files treeview.view:selected { background-image: linear-gradient(to bottom, #dcebfc, #c1dbfc); color: #000; border: none; border-radius: 0; }
.exp-files treeview.view:selected:backdrop { background-image: linear-gradient(to bottom, #f8f8f8, #e5e5e5); border: none; }
.exp-files treeview.view:hover { background-image: linear-gradient(to bottom, #fafbfd, #ebf3fd); border: none; }
.exp-rename { border: 1px solid #000; border-radius: 0; background-color: #fff; padding: 0 2px; min-height: 18px; box-shadow: none; }
.exp-files treeview.view header button { background-image: linear-gradient(to bottom, #ffffff, #f1f5fb); color: #4c607a; border-color: #dde4ee; }
.exp-details { background-image: linear-gradient(to bottom, #f6f9fd, #e8eef6); border-top: 1px solid #c4d1e2; padding: 6px 10px; }
.exp-details .name { font-size: 11pt; color: #000; }
.exp-details .key { color: #5a6779; }
.exp-details label { color: #1e1e1e; }
.exp-details .sub { color: #5a6779; }
.exp-lib { background-image: linear-gradient(to bottom, #ffffff, #f2f7fc); border-bottom: 1px solid #dfe9f5; padding: 8px 14px; }
.exp-lib .title { font-size: 12pt; color: #1e395b; }
.exp-empty { color: #6d6d6d; }
.exp-preview { background-color: #ffffff; border-left: 1px solid #d6e5f5; }
.exp-preview .dim { color: #6d6d6d; }
.drive-tile { padding: 6px; border: 1px solid transparent; border-radius: 3px; }
flowboxchild:selected .drive-tile, flowboxchild:selected { background-image: linear-gradient(to bottom, #dcebfc, #c1dbfc); border-color: #7da2ce; border-radius: 3px; }
flowboxchild:hover .drive-tile { background-image: linear-gradient(to bottom, #f1f8fd, #d5e9f9); border-color: #b8d6fb; }
.drive-group { color: #1e3287; font-size: 10pt; padding: 8px 6px 2px 6px; }
levelbar.drive trough { min-height: 12px; border: 1px solid #bcbcbc; background-image: linear-gradient(to bottom, #e6e6e6, #f5f5f5); border-radius: 0; padding: 0; }
levelbar.drive block.filled { background-image: linear-gradient(to bottom, #3b9dea, #2a7cc8 50%, #1f69b7); border-radius: 0; }
levelbar.drive block.filled.full { background-image: linear-gradient(to bottom, #f05b5b, #d62b2b 50%, #c01a1a); }
levelbar.drive block.empty { background: none; }
progressbar.win7-copy-progress trough { min-height: 14px; }
progressbar.win7-copy-progress progress { min-height: 14px; background-image: linear-gradient(to bottom, #bff0b0, #4fc436 50%, #33a91c); border-color: #2f8a2f; }
button.command-link { border: 1px solid transparent; background: none; box-shadow: none; padding: 6px; }
button.command-link:hover { border-color: #c6d6ea; background-image: linear-gradient(to bottom, #f8fbff, #edf4fd); }
"#;

struct Row {
    it: gtk::TreeIter,
    e: Entry,
}

pub struct Explorer {
    pub win: gtk::ApplicationWindow,
    loc: RefCell<Loc>,
    back: RefCell<Vec<Loc>>,
    fwd: RefCell<Vec<Loc>>,
    store: RefCell<gtk::ListStore>,
    /// the files shown: their row and what was read about them
    rows: RefCell<HashMap<PathBuf, Row>>,
    /// rows that are not files (libraries, network computers) keep the order they were added in
    virtual_rows: Cell<bool>,
    /// rows were added or changed since the last sort
    unsorted: Cell<bool>,
    /// the model views show while a folder is read / when they are not the visible view
    empty_model: gtk::ListStore,
    thumb_queue: RefCell<Vec<listing::ThumbResult>>,
    /// Extra large … Small icons, List and Tiles
    icons: Rc<grid::Grid>,
    details: gtk::TreeView,
    det_name: gtk::TreeViewColumn,
    det_text: gtk::CellRendererText,
    det_cols: HashMap<&'static str, gtk::TreeViewColumn>,
    content: gtk::TreeView,
    con_col: gtk::TreeViewColumn,
    con_text: gtk::CellRendererText,
    stack: gtk::Stack,
    empty_label: gtk::Label,
    crumbs: gtk::Box,
    addr: gtk::Box,
    addr_entry: gtk::Entry,
    addr_stack: gtk::Stack,
    addr_completion: gtk::ListStore,
    addr_dir: RefCell<PathBuf>,
    search: gtk::Entry,
    back_b: gtk::Button,
    fwd_b: gtk::Button,
    cmd_dyn: gtk::Box,
    views_b: gtk::Button,
    nav: Rc<nav::NavPane>,
    nav_sw: gtk::ScrolledWindow,
    info_icon: gtk::Image,
    info_name: gtk::Label,
    info_sub: gtk::Label,
    info_grid: gtk::Grid,
    details_pane: gtk::Box,
    lib_header: gtk::Box,
    lib_title: gtk::Label,
    preview: gtk::Image,
    preview_text: gtk::TextView,
    preview_sw: gtk::ScrolledWindow,
    preview_msg: gtk::Label,
    preview_box: gtk::Box,
    prev_b: gtk::ToggleButton,
    computer: gtk::Box,
    view: Cell<View>,
    opts: RefCell<config::FolderOptions>,
    _conf_monitor: RefCell<Option<gio::FileMonitor>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    pending: RefCell<HashSet<PathBuf>>,
    pending_scheduled: Cell<bool>,
    generation: Cell<u32>,
    scan_cancel: RefCell<Arc<AtomicBool>>,
    loading: Cell<bool>,
    icon_cache: RefCell<HashMap<(String, i32), Option<Pixbuf>>>,
    /// thumbnails made in this window: (path, size) -> (file mtime, framed picture)
    thumb_cache: RefCell<HashMap<(PathBuf, i32), (i64, Pixbuf)>>,
    thumbs_scheduled: Cell<bool>,
    attached: Cell<bool>,
    flush_busy: Cell<bool>,
    thumbs: RefCell<Option<listing::Thumbnailer>>,
    search_timer: RefCell<Option<glib::SourceId>>,
    typeahead: RefCell<(String, Option<Instant>)>,
    renaming: RefCell<Option<(PathBuf, String)>>,
    select_after: RefCell<Vec<PathBuf>>,
    rename_after: RefCell<Option<PathBuf>>,
    info_scheduled: Cell<bool>,
    sort_state: Cell<(u32, bool)>,
    drop_target: RefCell<Option<(PathBuf, gdk::DragAction)>>,
    vm: gio::VolumeMonitor,
}

thread_local! {
    /// files being dragged out of an Explorer window of this process
    static DRAGGING: RefCell<Vec<PathBuf>> = const { RefCell::new(vec![]) };
}

impl Explorer {
    pub fn new(app: &gtk::Application, start: Loc) -> Rc<Explorer> {
        crate::theme::load_css(CSS);
        let win = gtk::ApplicationWindow::new(app);
        win.set_default_size(920, 620);
        win.set_icon_name(Some(util::first_icon(&["system-file-manager", "folder"])));
        // the header is glass extended into the window: translucent under a compositor
        glass::enable_rgba(win.upcast_ref());
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let opts = config::get().folder;

        // ---------- glass header: back/forward, recent pages, address bar, search
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.style_context().add_class("exp-header");
        header.set_app_paintable(true);
        let back_b = gtk::Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
        let fwd_b = gtk::Button::from_icon_name(Some("go-next-symbolic"), gtk::IconSize::Button);
        for b in [&back_b, &fwd_b] { b.style_context().add_class("nav"); b.set_valign(gtk::Align::Center); b.set_focus_on_click(false); }
        back_b.set_tooltip_text(Some("Back"));
        fwd_b.set_tooltip_text(Some("Forward"));
        let recent_b = gtk::Button::new();
        recent_b.add(&gtk::Image::from_icon_name(Some("pan-down-symbolic"), gtk::IconSize::Menu));
        recent_b.style_context().add_class("recent");
        recent_b.set_valign(gtk::Align::Center);
        recent_b.set_tooltip_text(Some("Recent Pages"));
        recent_b.set_focus_on_click(false);
        // Win7's travel buttons are aero.msstyles' Navigation parts (1 Back, 2 Forward, 3 the
        // recent-pages arrow): glossy, partly see-through orbs drawn over the glass header
        for (b, part) in [(&back_b, 1), (&fwd_b, 2), (&recent_b, 3)] { crate::shellstyle::nav_button(b, part); }
        header.pack_start(&back_b, false, false, 0);
        header.pack_start(&fwd_b, false, false, 0);
        header.pack_start(&recent_b, false, false, 0);
        let addr_stack = gtk::Stack::new();
        let addr = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        addr.style_context().add_class("exp-addr");
        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let crumb_ev = gtk::EventBox::new();
        crumb_ev.add(&crumbs);
        addr.pack_start(&crumb_ev, true, true, 0);
        let refresh = gtk::Button::from_icon_name(Some("view-refresh-symbolic"), gtk::IconSize::Menu);
        refresh.set_tooltip_text(Some("Refresh"));
        refresh.set_focus_on_click(false);
        addr.pack_end(&refresh, false, false, 0);
        let addr_entry = gtk::Entry::new();
        addr_entry.style_context().add_class("exp-addr");
        let addr_completion = gtk::ListStore::new(&[glib::Type::STRING]);
        {
            let c = gtk::EntryCompletion::new();
            c.set_model(Some(&addr_completion));
            c.set_text_column(0);
            c.set_minimum_key_length(1);
            c.set_popup_single_match(true);
            c.set_inline_selection(true);
            addr_entry.set_completion(Some(&c));
        }
        addr_stack.add_named(&addr, "crumbs");
        addr_stack.add_named(&addr_entry, "entry");
        header.pack_start(&addr_stack, true, true, 2);
        let search = gtk::Entry::new();
        search.style_context().add_class("exp-search");
        search.style_context().add_class("empty");
        search.set_width_chars(26);
        search.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("system-search-symbolic"));
        header.pack_end(&search, false, false, 0);
        root.pack_start(&header, false, false, 0);

        // ---------- command bar
        let cmd = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        cmd.style_context().add_class("commandbar");
        let organize = gtk::MenuButton::new();
        organize.set_label("Organize ▾");
        organize.set_relief(gtk::ReliefStyle::None);
        organize.set_focus_on_click(false);
        cmd.pack_start(&organize, false, false, 0);
        let cmd_dyn = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        cmd.pack_start(&cmd_dyn, false, false, 0);
        // "Change your view": the button cycles views, the arrow opens the slider menu
        let views_split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        views_split.style_context().add_class("viewsplit");
        let views_b = gtk::Button::new();
        views_b.set_relief(gtk::ReliefStyle::None);
        views_b.set_tooltip_text(Some("Change your view."));
        views_b.set_focus_on_click(false);
        let views_arrow = gtk::Button::new();
        views_arrow.add(&gtk::Image::from_icon_name(Some("pan-down-symbolic"), gtk::IconSize::Menu));
        views_arrow.set_relief(gtk::ReliefStyle::None);
        views_arrow.set_tooltip_text(Some("More options"));
        views_arrow.set_focus_on_click(false);
        views_split.pack_start(&views_b, false, false, 0);
        views_split.pack_start(&views_arrow, false, false, 0);
        let prev_b = gtk::ToggleButton::new();
        prev_b.add(&util::image("view-dual-symbolic", 16));
        prev_b.set_relief(gtk::ReliefStyle::None);
        prev_b.set_tooltip_text(Some("Show the preview pane."));
        prev_b.set_focus_on_click(false);
        let help_b = gtk::Button::from_icon_name(Some("help-browser"), gtk::IconSize::Menu);
        help_b.set_relief(gtk::ReliefStyle::None);
        help_b.set_tooltip_text(Some("Get help"));
        help_b.connect_clicked(|_| { util::open_uri("https://wiki.archlinux.org/title/File_manager_functionality"); });
        cmd.pack_end(&help_b, false, false, 0);
        cmd.pack_end(&prev_b, false, false, 0);
        cmd.pack_end(&views_split, false, false, 0);
        root.pack_start(&cmd, false, false, 0);

        // ---------- body: nav | (lib header + files) | preview
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        let navp = nav::NavPane::new(opts.nav_all_folders, opts.show_hidden);
        let nav_sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        nav_sw.style_context().add_class("exp-nav");
        nav_sw.add(&navp.view);
        nav_sw.set_size_request(190, -1);
        nav_sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        paned.pack1(&nav_sw, false, false);

        let center = gtk::Box::new(gtk::Orientation::Vertical, 0);
        center.style_context().add_class("exp-files");
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

        let store = new_store();
        // icon views: a virtualised grid (see grid.rs)
        let icons = grid::Grid::new(grid::Cols { icon: C_ICON, name: C_NAME, markup: C_TILE, sensitive: C_SENS });
        let details = gtk::TreeView::with_model(&store);
        details.selection().set_mode(gtk::SelectionMode::Multiple);
        details.set_rubber_banding(true);
        details.set_enable_search(false);
        details.set_fixed_height_mode(true);
        let det_text = gtk::CellRendererText::new();
        det_text.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let det_name = gtk::TreeViewColumn::new();
        let mut det_cols = HashMap::new();
        {
            let c = &det_name;
            c.set_title("Name");
            let ri = gtk::CellRendererPixbuf::new();
            TreeViewColumnExt::pack_start(c, &ri, false);
            TreeViewColumnExt::pack_start(c, &det_text, true);
            TreeViewColumnExt::add_attribute(c, &ri, "pixbuf", C_ICON as i32);
            TreeViewColumnExt::add_attribute(c, &ri, "sensitive", C_SENS as i32);
            TreeViewColumnExt::add_attribute(c, &det_text, "text", C_NAME as i32);
            c.set_resizable(true);
            c.set_fixed_width(260);
            c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            c.set_clickable(true);
            unsafe { c.set_data("sort", C_SORT); }
            c.set_reorderable(true);
            details.append_column(c);
            for (key, title, col, sortc, w, right) in [("orig", "Original Location", C_ORIG, C_ORIG, 200, false), ("deleted", "Date Deleted", C_DEL, C_DELN, 140, false),
                ("date", "Date modified", C_DATE, C_DATEN, 140, false), ("type", "Type", C_TYPE, C_TYPE, 150, false), ("size", "Size", C_SIZE, C_SIZEN, 90, true),
                ("folder", "Folder", C_ORIG, C_ORIG, 260, false)] {
                let c = gtk::TreeViewColumn::new();
                c.set_title(title);
                let r = gtk::CellRendererText::new();
                r.set_property("foreground", "#6d6d6d");
                r.set_ellipsize(gtk::pango::EllipsizeMode::End);
                if right { r.set_xalign(1.0); }
                TreeViewColumnExt::pack_start(&c, &r, true);
                TreeViewColumnExt::add_attribute(&c, &r, "text", col as i32);
                c.set_resizable(true);
                c.set_fixed_width(w);
                c.set_sizing(gtk::TreeViewColumnSizing::Fixed);
                c.set_clickable(true);
                unsafe { c.set_data("sort", sortc); }
                c.set_reorderable(true);
                details.append_column(&c);
                det_cols.insert(key, c);
            }
        }
        // Content view: icon | name and type | date and size
        let content = gtk::TreeView::with_model(&store);
        content.set_headers_visible(false);
        content.selection().set_mode(gtk::SelectionMode::Multiple);
        content.set_rubber_banding(true);
        content.set_enable_search(false);
        content.set_grid_lines(gtk::TreeViewGridLines::Horizontal);
        let con_text = gtk::CellRendererText::new();
        let con_col = gtk::TreeViewColumn::new();
        {
            let ri = gtk::CellRendererPixbuf::new();
            ri.set_padding(6, 6);
            TreeViewColumnExt::pack_start(&con_col, &ri, false);
            TreeViewColumnExt::add_attribute(&con_col, &ri, "pixbuf", C_ICON as i32);
            TreeViewColumnExt::add_attribute(&con_col, &ri, "sensitive", C_SENS as i32);
            con_text.set_ellipsize(gtk::pango::EllipsizeMode::End);
            TreeViewColumnExt::pack_start(&con_col, &con_text, true);
            TreeViewColumnExt::add_attribute(&con_col, &con_text, "markup", C_CNAME as i32);
            con_col.set_expand(true);
            // fixed sizes: rows are not measured one by one (10 000-file folders)
            con_col.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            con_col.set_fixed_width(320);
            content.append_column(&con_col);
            let c2 = gtk::TreeViewColumn::new();
            let r2 = gtk::CellRendererText::new();
            r2.set_xalign(0.0);
            TreeViewColumnExt::pack_start(&c2, &r2, true);
            TreeViewColumnExt::add_attribute(&c2, &r2, "markup", C_CONTENT as i32);
            c2.set_sizing(gtk::TreeViewColumnSizing::Fixed);
            c2.set_fixed_width(250);
            content.append_column(&c2);
            content.set_fixed_height_mode(true);
        }
        let stack = gtk::Stack::new();
        let mk_sw = |w: &gtk::Widget| { let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>); sw.add(w); sw };
        let computer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.add_named(&icons.root, "icons");
        stack.add_named(&mk_sw(details.upcast_ref()), "details");
        stack.add_named(&mk_sw(content.upcast_ref()), "content");
        stack.add_named(&mk_sw(computer.upcast_ref()), "computer");
        let overlay = gtk::Overlay::new();
        overlay.add(&stack);
        let empty_label = gtk::Label::new(None);
        empty_label.style_context().add_class("exp-empty");
        empty_label.set_halign(gtk::Align::Center);
        empty_label.set_valign(gtk::Align::Start);
        empty_label.set_margin_top(40);
        empty_label.set_no_show_all(true);
        overlay.add_overlay(&empty_label);
        overlay.set_overlay_pass_through(&empty_label, true);
        center.pack_start(&overlay, true, true, 0);
        let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
        inner.pack1(&center, true, false);
        // preview pane (Alt+P)
        let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        preview_box.style_context().add_class("exp-preview");
        preview_box.set_size_request(260, -1);
        let preview = gtk::Image::new();
        preview.set_vexpand(true);
        let preview_text = gtk::TextView::new();
        preview_text.set_editable(false);
        preview_text.set_cursor_visible(false);
        preview_text.set_wrap_mode(gtk::WrapMode::WordChar);
        preview_text.set_left_margin(8);
        preview_text.set_right_margin(8);
        preview_text.set_top_margin(6);
        let preview_sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        preview_sw.add(&preview_text);
        preview_sw.set_vexpand(true);
        let preview_msg = gtk::Label::new(Some("Select a file to preview."));
        preview_msg.style_context().add_class("dim");
        preview_msg.set_vexpand(true);
        preview_box.pack_start(&preview, true, true, 0);
        preview_box.pack_start(&preview_sw, true, true, 0);
        preview_box.pack_start(&preview_msg, true, true, 0);
        inner.pack2(&preview_box, false, false);
        paned.pack2(&inner, true, false);
        root.pack_start(&paned, true, true, 0);

        // ---------- details pane
        let dp = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        dp.style_context().add_class("exp-details");
        let info_icon = gtk::Image::new();
        info_icon.set_size_request(64, 56);
        dp.pack_start(&info_icon, false, false, 0);
        let iv = gtk::Box::new(gtk::Orientation::Vertical, 2);
        iv.set_valign(gtk::Align::Center);
        let info_name = gtk::Label::new(None);
        info_name.set_xalign(0.0);
        info_name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        info_name.set_max_width_chars(36);
        info_name.style_context().add_class("name");
        let info_sub = gtk::Label::new(None);
        info_sub.set_xalign(0.0);
        info_sub.style_context().add_class("sub");
        iv.pack_start(&info_name, false, false, 0);
        iv.pack_start(&info_sub, false, false, 0);
        dp.pack_start(&iv, false, false, 0);
        let info_grid = gtk::Grid::new();
        info_grid.set_row_spacing(2);
        info_grid.set_column_spacing(6);
        info_grid.set_valign(gtk::Align::Center);
        info_grid.set_margin_start(20);
        dp.pack_start(&info_grid, true, true, 0);
        dp.set_size_request(-1, 70);
        root.pack_start(&dp, false, false, 0);
        win.add(&root);

        // one Win7 frame around each selected / hot row (GTK draws cells one by one)
        for tv in [&details, &content] { row_frames(tv); }
        let ex = Rc::new(Explorer {
            win: win.clone(), loc: RefCell::new(start.clone()), back: RefCell::new(vec![]), fwd: RefCell::new(vec![]),
            store: RefCell::new(store), rows: RefCell::new(HashMap::new()), virtual_rows: Cell::new(false), unsorted: Cell::new(false), empty_model: new_store(), thumb_queue: RefCell::new(vec![]),
            icons: icons.clone(), details: details.clone(), det_name, det_text, det_cols, content: content.clone(), con_col, con_text,
            stack, empty_label, crumbs, addr: addr.clone(), addr_entry: addr_entry.clone(), addr_stack: addr_stack.clone(), addr_completion, addr_dir: RefCell::new(PathBuf::new()),
            search: search.clone(), back_b: back_b.clone(), fwd_b: fwd_b.clone(), cmd_dyn, views_b: views_b.clone(), nav: navp, nav_sw: nav_sw.clone(),
            info_icon, info_name, info_sub, info_grid, details_pane: dp.clone(), lib_header, lib_title,
            preview, preview_text, preview_sw, preview_msg, preview_box: preview_box.clone(), prev_b: prev_b.clone(), computer,
            view: Cell::new(View::Details), opts: RefCell::new(opts), _conf_monitor: RefCell::new(None), monitor: RefCell::new(None),
            pending: RefCell::new(HashSet::new()), pending_scheduled: Cell::new(false), generation: Cell::new(0),
            scan_cancel: RefCell::new(Arc::new(AtomicBool::new(false))), loading: Cell::new(false),
            icon_cache: RefCell::new(HashMap::new()), thumb_cache: RefCell::new(HashMap::new()), thumbs: RefCell::new(None), thumbs_scheduled: Cell::new(false), attached: Cell::new(false), flush_busy: Cell::new(false),
            search_timer: RefCell::new(None), typeahead: RefCell::new((String::new(), None)), renaming: RefCell::new(None),
            select_after: RefCell::new(vec![]), rename_after: RefCell::new(None), info_scheduled: Cell::new(false), sort_state: Cell::new((C_SORT, false)), drop_target: RefCell::new(None),
            vm: gio::VolumeMonitor::get(),
        });
        {
            let e = Rc::downgrade(&ex);
            *ex.thumbs.borrow_mut() = Some(listing::Thumbnailer::new(move |r| { if let Some(e) = e.upgrade() { e.on_thumb(r); } }));
        }

        // the header is the frame's glass continued (Win7 extends it into the client area)
        {
            header.connect_draw(move |h, cr| {
                glass::paint_widget_glass(h, cr);
                glib::Propagation::Proceed
            });
            let h2 = header.clone();
            win.connect_configure_event(move |_, _| { h2.queue_draw(); false });
            let h3 = header.clone();
            win.connect_window_state_event(move |_, e| {
                if e.changed_mask().intersects(gdk::WindowState::FOCUSED | gdk::WindowState::MAXIMIZED) { h3.queue_draw(); }
                glib::Propagation::Proceed
            });
        }
        ex.wire(&organize, &views_arrow, &prev_b, &refresh, &crumb_ev, &recent_b);
        // a closed window stops its folder reading / search at once
        {
            let e = Rc::downgrade(&ex);
            win.connect_destroy(move |_| {
                let Some(e) = e.upgrade() else { return };
                e.scan_cancel.borrow().store(true, AtomicOrdering::Relaxed);
                e.generation.set(e.generation.get().wrapping_add(1));
                *e.monitor.borrow_mut() = None;
            });
        }
        ex.apply_folder_options();
        // Folder Options applies to open windows too
        if let Ok(m) = gio::File::for_path(config::conf_dir().join("config.json")).monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) {
            let e = Rc::downgrade(&ex);
            m.connect_changed(move |_, _, _, ev| {
                if !matches!(ev, gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::Created | gio::FileMonitorEvent::MovedIn | gio::FileMonitorEvent::Renamed) { return; }
                let Some(e) = e.upgrade() else { return };
                config::reload();
                let fo = config::get().folder;
                if *e.opts.borrow() == fo { return; }
                *e.opts.borrow_mut() = fo.clone();
                e.nav.show_hidden.set(fo.show_hidden);
                e.nav.reset_folders();
                e.apply_folder_options();
                e.thumb_cache.borrow_mut().clear();
                e.reload_keep_selection();
            });
            *ex._conf_monitor.borrow_mut() = Some(m);
        }
        let (dpv, ppv, npv) = with_views(|v| (v.details_pane.unwrap_or(true), v.preview_pane.unwrap_or(false), v.nav_pane.unwrap_or(true)));
        root.show_all();
        dp.set_visible(dpv);
        nav_sw.set_visible(npv);
        preview_box.set_visible(false);
        prev_b.set_active(ppv);
        ex.go(start, false);
        win.show();
        // the keyboard goes to the file list, like Win7
        ex.focus_view();
        ex
    }

    fn parent(&self) -> Option<gtk::Window> { Some(self.win.clone().upcast()) }

    /// Select `p` once it shows up in the listing (aerode-explorer --select).
    pub fn select_when_shown(self: &Rc<Self>, p: PathBuf) {
        self.select_after.borrow_mut().push(p);
        self.after_batch();
    }

    // ------------------------------------------------------------ navigation
    pub fn go(self: &Rc<Self>, loc: Loc, record: bool) { self.navigate(loc, record, record) }

    /// `record`: a new history entry; `sound`: Start Navigation, which Win7's browser frame plays
    /// for every navigation the user starts, Back and Forward included.
    fn navigate(self: &Rc<Self>, loc: Loc, record: bool, sound: bool) {
        let loc = match loc {
            Loc::Path(p) => {
                let p = if p.is_absolute() { p } else { std::env::current_dir().map(|c| c.join(&p)).unwrap_or(p) };
                if p.is_file() { self.open_file(&p); return; }
                if !p.exists() {
                    util::error(&format!("Windows can't find '{}'. Check the spelling and try again.", p.display()), "");
                    return;
                }
                // Win7 stays where it is when a folder can't be opened
                if let Err(e) = std::fs::read_dir(&p) {
                    let why = if e.kind() == std::io::ErrorKind::PermissionDenied { "Access is denied.".to_string() } else { e.to_string() };
                    util::error("Location is not available", &format!("{} is not accessible.\n\n{why}", p.display()));
                    return;
                }
                Loc::Path(p)
            }
            l => l,
        };
        if let Some(t) = self.search_timer.borrow_mut().take() { t.remove(); }
        if record {
            let cur = self.loc.borrow().clone();
            if cur != loc { self.back.borrow_mut().push(cur); self.fwd.borrow_mut().clear(); }
        }
        if sound { sounds::play(sounds::Event::StartNavigation); }
        let prev = self.loc.replace(loc.clone());
        if prev != loc { self.thumb_cache.borrow_mut().clear(); }
        if !matches!(loc, Loc::Search(..)) && !self.search.text().is_empty() { self.set_search_text(""); }
        self.search.set_placeholder_text(Some(&format!("Search {}", match &loc { Loc::Search(b, _) => Loc::Path(b.clone()).title(), l => l.title() })));
        // Folder Options > Display the full path in the title bar
        match &loc {
            Loc::Path(p) if self.opts.borrow().full_path_title => self.win.set_title(&p.display().to_string()),
            _ => self.win.set_title(&loc.title()),
        }
        self.back_b.set_sensitive(!self.back.borrow().is_empty());
        self.fwd_b.set_sensitive(!self.fwd.borrow().is_empty());
        self.build_crumbs();
        self.lib_header.set_visible(matches!(loc, Loc::Library(_)));
        if let Loc::Library(n) = &loc { self.lib_title.set_text(&format!("{n} library")); }
        if !matches!(loc, Loc::Search(..)) { self.nav.select(&loc.nav_target(), self.opts.borrow().nav_expand); }
        // remembered view for this folder
        let fv = with_views(|v| v.folders.get(&loc.key()).copied());
        self.view.set(fv.and_then(|f| View::ALL.get(f.v).copied()).unwrap_or_else(|| default_view(&loc)));
        self.load();
        self.update_cmd();
    }

    fn go_back(self: &Rc<Self>) {
        let Some(l) = self.back.borrow_mut().pop() else { return };
        let cur = self.loc.borrow().clone();
        self.fwd.borrow_mut().push(cur);
        self.navigate(l, false, true);
    }

    fn go_fwd(self: &Rc<Self>) {
        let Some(l) = self.fwd.borrow_mut().pop() else { return };
        let cur = self.loc.borrow().clone();
        self.back.borrow_mut().push(cur);
        self.navigate(l, false, true);
    }

    fn go_up(self: &Rc<Self>) {
        let loc = self.loc.borrow().clone();
        let up = match loc {
            Loc::Path(p) if p == config::home() => Some(Loc::Path(p.parent().map(|x| x.to_path_buf()).unwrap_or_else(|| PathBuf::from("/")))),
            Loc::Path(p) if p == Path::new("/") => Some(Loc::Computer),
            Loc::Path(p) => p.parent().map(|q| Loc::Path(q.to_path_buf())).or(Some(Loc::Computer)),
            Loc::Library(_) => Some(Loc::Libraries),
            Loc::Search(b, _) => Some(Loc::Path(b)),
            _ => None,
        };
        if let Some(u) = up { self.go(u, true); }
    }

    /// Open a folder in another Explorer window of this process.
    fn open_window(&self, loc: Loc) {
        if let Some(app) = self.win.application() { let _ = Explorer::new(&app, loc); }
    }

    // ------------------------------------------------------------ address bar
    fn crumb_parts(&self, loc: &Loc) -> Vec<(String, Loc)> {
        let mut parts: Vec<(String, Loc)> = vec![];
        match loc {
            Loc::Path(p) => {
                let home = config::home();
                if p.starts_with(&home) {
                    parts.push((glib::user_name().to_string_lossy().into_owned(), Loc::Path(home.clone())));
                    let mut acc = home.clone();
                    if let Ok(rest) = p.strip_prefix(&home) {
                        for c in rest.components() { acc = acc.join(c); parts.push((c.as_os_str().to_string_lossy().into_owned(), Loc::Path(acc.clone()))); }
                    }
                } else {
                    parts.push(("Computer".into(), Loc::Computer));
                    parts.push(("Local Disk (/)".into(), Loc::Path(PathBuf::from("/"))));
                    let mut acc = PathBuf::from("/");
                    for c in p.components().skip(1) { acc = acc.join(c); parts.push((c.as_os_str().to_string_lossy().into_owned(), Loc::Path(acc.clone()))); }
                }
            }
            Loc::Library(n) => { parts.push(("Libraries".into(), Loc::Libraries)); parts.push((n.to_string(), loc.clone())); }
            Loc::Search(b, _) => { parts = self.crumb_parts(&Loc::Path(b.clone())); parts.push(("Search Results".into(), loc.clone())); }
            l => parts.push((l.title(), l.clone())),
        }
        parts
    }

    /// Children listed by the ▸ arrow after a crumb (None = the desktop-level places).
    fn crumb_children(&self, of: Option<&Loc>) -> Vec<(String, Loc, &'static str)> {
        let hidden = self.opts.borrow().show_hidden;
        let dir_children = |d: &Path| -> Vec<(String, Loc, &'static str)> {
            let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(d).map(|r| r.flatten().filter(|e| e.file_type().map(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())).unwrap_or(false))
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).filter(|(n, _)| hidden || !n.starts_with('.')).collect()).unwrap_or_default();
            v.sort_by(|a, b| natural_cmp(&a.0.to_lowercase(), &b.0.to_lowercase()));
            v.into_iter().take(500).map(|(n, p)| { let i = nav::folder_icon(&p); (n, Loc::Path(p), i) }).collect()
        };
        match of {
            None => vec![
                ("Libraries".into(), Loc::Libraries, Loc::Libraries.icon()),
                (glib::user_name().to_string_lossy().into_owned(), Loc::Path(config::home()), "user-home"),
                ("Computer".into(), Loc::Computer, "computer"),
                ("Network".into(), Loc::Network, "network-workgroup"),
                ("Recycle Bin".into(), Loc::Trash, Loc::Trash.icon()),
            ],
            Some(Loc::Computer) => {
                let mut v = vec![("Local Disk (/)".to_string(), Loc::Path(PathBuf::from("/")), util::first_icon(&["drive-harddisk-system", "drive-harddisk"]))];
                for m in self.vm.mounts() { if let Some(p) = m.root().path() { if p != Path::new("/") { v.push((m.name().to_string(), Loc::Path(p), "drive-removable-media")); } } }
                v
            }
            Some(Loc::Libraries) => ["Documents", "Music", "Pictures", "Videos"].iter().map(|n| (n.to_string(), Loc::Library(n), Loc::Library(n).icon())).collect(),
            Some(l) => l.dir().map(|d| dir_children(&d)).unwrap_or_default(),
        }
    }

    fn build_crumbs(self: &Rc<Self>) {
        for c in self.crumbs.children() { self.crumbs.remove(&c); }
        let loc = self.loc.borrow().clone();
        let icon = gtk::Button::new();
        icon.add(&util::image(loc.icon(), 16));
        icon.set_focus_on_click(false);
        { let e = self.clone(); icon.connect_clicked(move |_| e.edit_address()); }
        self.crumbs.pack_start(&icon, false, false, 0);
        let parts = self.crumb_parts(&loc);
        // Win7 collapses the leftmost crumbs into the first arrow's menu when space runs out
        let avail = (self.addr.allocated_width() - 60).max(300);
        let mut used = 0;
        let mut keep = 0;
        for (n, _) in parts.iter().rev() {
            used += n.chars().count() as i32 * 7 + 26;
            if used > avail && keep > 0 { break; }
            keep += 1;
        }
        let skip = parts.len() - keep;
        let hidden: Vec<(String, Loc)> = parts[..skip].to_vec();
        // first arrow: desktop-level places (plus the collapsed crumbs)
        let first = self.crumb_arrow(None, hidden, parts.get(skip).map(|p| p.1.clone()));
        if skip > 0 { first.set_label("«"); }
        self.crumbs.pack_start(&first, false, false, 0);
        let n = parts.len();
        for (i, (name, target)) in parts.into_iter().enumerate().skip(skip) {
            let b = gtk::Button::with_label(&name);
            b.set_focus_on_click(false);
            let (e, t) = (self.clone(), target.clone());
            b.connect_clicked(move |_| e.go(t.clone(), true));
            self.crumbs.pack_start(&b, false, false, 0);
            // the arrow after a crumb lists its subfolders, with the next crumb in bold
            if matches!(target, Loc::Search(..) | Loc::Trash | Loc::Network | Loc::Recent) { continue; }
            let next = self.crumb_parts(&loc).get(i + 1).map(|p| p.1.clone());
            if i + 1 == n && target.dir().map(|d| !has_subdirs(&d)).unwrap_or(false) { continue; }
            let a = self.crumb_arrow(Some(target), vec![], next);
            self.crumbs.pack_start(&a, false, false, 0);
        }
        self.crumbs.show_all();
        self.addr_stack.set_visible_child_name("crumbs");
    }

    fn crumb_arrow(self: &Rc<Self>, of: Option<Loc>, collapsed: Vec<(String, Loc)>, current: Option<Loc>) -> gtk::Button {
        let a = gtk::Button::with_label("▸");
        a.style_context().add_class("crumb-arrow");
        a.set_focus_on_click(false);
        let e = self.clone();
        a.connect_clicked(move |a| {
            let orig = a.label().map(|l| l.to_string()).unwrap_or_default();
            let m = gtk::Menu::new();
            for (name, l) in collapsed.iter().rev() {
                let it = menu_item_icon(name, l.icon());
                let (e2, l2) = (e.clone(), l.clone());
                it.connect_activate(move |_| e2.go(l2.clone(), true));
                m.append(&it);
            }
            if !collapsed.is_empty() { m.append(&gtk::SeparatorMenuItem::new()); }
            for (name, l, icon) in e.crumb_children(of.as_ref()) {
                let it = menu_item_icon(&name, icon);
                if current.as_ref() == Some(&l) {
                    if let Some(lb) = it.child().and_then(|c| c.downcast::<gtk::Box>().ok()).and_then(|b| b.children().last().cloned()).and_then(|w| w.downcast::<gtk::Label>().ok()) {
                        lb.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&name)));
                    }
                }
                let (e2, l2) = (e.clone(), l.clone());
                it.connect_activate(move |_| e2.go(l2.clone(), true));
                m.append(&it);
            }
            if m.children().is_empty() { return; }
            m.show_all();
            a.set_label("▾");
            a.style_context().add_class("open");
            let a2 = a.clone();
            m.connect_deactivate(move |_| { a2.set_label(&orig); a2.style_context().remove_class("open"); });
            m.set_attach_widget(Some(a));
            m.popup_at_widget(a, gdk::Gravity::SouthWest, gdk::Gravity::NorthWest, None);
        });
        a
    }

    fn edit_address(&self) {
        let t = match &*self.loc.borrow() { Loc::Path(p) => p.display().to_string(), Loc::Library(n) => format!("Libraries\\{n}"), l => l.title() };
        self.addr_entry.set_text(&t);
        self.addr_stack.set_visible_child_name("entry");
        self.addr_entry.grab_focus();
        self.addr_entry.select_region(0, -1);
    }

    /// Address bar autocompletion: the subfolders of the folder being typed.
    fn update_completion(&self) {
        let t = self.addr_entry.text().to_string();
        let t = util::shellexpand(&t);
        let Some(i) = t.rfind('/') else { return };
        let dir = PathBuf::from(if i == 0 { "/" } else { &t[..i] });
        if *self.addr_dir.borrow() == dir { return; }
        *self.addr_dir.borrow_mut() = dir.clone();
        self.addr_completion.clear();
        let hidden = self.opts.borrow().show_hidden || t[i + 1..].starts_with('.');
        let mut v: Vec<String> = std::fs::read_dir(&dir).map(|r| r.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| hidden || !n.starts_with('.')).collect()).unwrap_or_default();
        v.sort_by(|a, b| natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
        for n in v.into_iter().take(400) {
            let full = if dir == Path::new("/") { format!("/{n}") } else { format!("{}/{n}", dir.display()) };
            self.addr_completion.insert_with_values(None, &[(0, &full)]);
        }
    }

    fn address_go(self: &Rc<Self>, text: &str) {
        let t = text.trim();
        let low = t.to_lowercase();
        let loc = match low.as_str() {
            "" => return,
            "computer" | "my computer" => Loc::Computer,
            "recycle bin" => Loc::Trash,
            "network" => Loc::Network,
            "libraries" => Loc::Libraries,
            "documents" | "libraries\\documents" => Loc::Library("Documents"),
            "music" | "libraries\\music" => Loc::Library("Music"),
            "pictures" | "libraries\\pictures" => Loc::Library("Pictures"),
            "videos" | "libraries\\videos" => Loc::Library("Videos"),
            "desktop" => Loc::Path(util::special_dir(glib::UserDirectory::Desktop, "Desktop")),
            "control panel" => { util::spawn(&["aerode-control"]); self.addr_stack.set_visible_child_name("crumbs"); return; }
            _ if low.starts_with("http://") || low.starts_with("https://") || low.starts_with("www.") => {
                util::open_uri(&if low.starts_with("www.") { format!("http://{t}") } else { t.to_string() });
                self.addr_stack.set_visible_child_name("crumbs");
                return;
            }
            _ => {
                let p = util::shellexpand(t);
                // relative names are looked up in the current folder
                if !p.starts_with('/') && !p.contains("://") && !p.contains(':') {
                    if let Some(d) = self.cur_dir() { Loc::Path(d.join(&p)) } else { Loc::parse(&p) }
                } else { Loc::parse(&p) }
            }
        };
        self.go(loc, true);
    }

    // ------------------------------------------------------------ listing
    fn store(&self) -> gtk::ListStore { self.store.borrow().clone() }

    fn icon_key_pixbuf(&self, key: &str, size: i32, f: impl FnOnce() -> Option<Pixbuf>) -> Option<Pixbuf> {
        let k = (key.to_string(), size);
        if let Some(p) = self.icon_cache.borrow().get(&k) { return p.clone(); }
        let p = f();
        self.icon_cache.borrow_mut().insert(k, p.clone());
        p
    }

    fn entry_icon(&self, e: &Entry, size: i32) -> Option<Pixbuf> {
        let base = if e.is_dir {
            let name = nav::folder_icon(&e.path);
            self.icon_key_pixbuf(&format!("dir:{name}"), size, || util::pixbuf_or(name, size, "folder"))
        } else if let Some(i) = &e.icon_name {
            self.icon_key_pixbuf(&format!("desk:{i}"), size, || util::pixbuf_or(i, size, "application-x-executable"))
        } else if e.broken {
            self.icon_key_pixbuf("broken", size, || util::pixbuf_or("text-x-generic", size, "unknown"))
        } else {
            let ct = e.ctype.clone();
            self.icon_key_pixbuf(&ct, size, || {
                let gi = gio::content_type_get_icon(&ct);
                util::gicon_pixbuf(Some(&gi), size).or_else(|| util::pixbuf_or("text-x-generic", size, "unknown"))
            })
        };
        if !e.is_link { return base; }
        // shortcuts get the arrow overlay
        let key = format!("link:{}:{}", if e.is_dir { nav::folder_icon(&e.path).to_string() } else { e.icon_name.clone().unwrap_or_else(|| e.ctype.clone()) }, e.broken);
        self.icon_key_pixbuf(&key, size, || base.map(|b| shortcut_overlay(&b, size)))
    }

    fn scan_opts(&self) -> listing::ScanOpts {
        let o = self.opts.borrow();
        listing::ScanOpts { hide_ext: o.hide_ext, show_hidden: o.show_hidden }
    }

    fn cached_thumb(&self, e: &Entry, size: i32) -> Option<Pixbuf> {
        self.thumb_cache.borrow().get(&(e.path.clone(), size)).filter(|(m, _)| *m == e.mtime).map(|(_, p)| p.clone())
    }

    fn row_values(&self, e: &Entry, size: i32) -> Vec<(u32, glib::Value)> {
        let t = &e.text;
        let ghost = e.hidden || ops::is_cut(&e.path);
        let pb = self.cached_thumb(e, size).or_else(|| self.entry_icon(e, size));
        let deln = e.trash.as_ref().map(|t| t.1).unwrap_or(0);
        vec![(C_ICON, pb.to_value()), (C_NAME, t.name.to_value()), (C_DATE, t.date.to_value()), (C_TYPE, e.desc.to_value()), (C_SIZE, t.size.to_value()),
            (C_URI, t.uri.to_value()), (C_ISDIR, e.is_dir.to_value()), (C_SIZEN, e.size.to_value()), (C_DATEN, e.mtime.to_value()), (C_TILE, t.tile.to_value()),
            (C_CONTENT, t.content.to_value()), (C_ORIG, t.orig.to_value()), (C_DEL, t.del.to_value()),
            (C_DELN, deln.to_value()), (C_SENS, (!ghost).to_value()), (C_EDIT, e.name.to_value()), (C_CT, e.ctype.to_value()), (C_CNAME, t.content_name.to_value())]
    }

    /// Add a file to the view, or update it when it is already shown.
    fn upsert(&self, e: Entry) {
        if e.hidden && !self.opts.borrow().show_hidden { self.remove_path(&e.path); return; }
        let size = self.view.get().icon_size();
        let vals = self.row_values(&e, size);
        let refs: Vec<(u32, &dyn ToValue)> = vals.iter().map(|(c, v)| (*c, v as &dyn ToValue)).collect();
        let store = self.store();
        let existing = self.rows.borrow().get(&e.path).map(|r| r.it.clone());
        self.unsorted.set(true);
        match existing {
            Some(it) => {
                // an unchanged row is left alone (a row change also ends an in-place rename, and
                // makes the icon view lay itself out again); the row being renamed waits
                let same = self.rows.borrow().get(&e.path).map(|r| r.e.same_look(&e)).unwrap_or(false);
                let editing = self.renaming.borrow().as_ref().map(|(p, _)| p == &e.path).unwrap_or(false);
                if !same && !editing { store.set(&it, &refs); }
                if !editing { if let Some(r) = self.rows.borrow_mut().get_mut(&e.path) { r.e = e; } }
            }
            None => {
                let it = store.insert_with_values(None, &refs);
                self.rows.borrow_mut().insert(e.path.clone(), Row { it, e });
            }
        }
    }

    fn remove_path(&self, p: &Path) {
        let r = self.rows.borrow_mut().remove(p);
        if let Some(r) = r { self.store().remove(&r.it); }
    }

    /// Thumbnails are made for what can be seen (and a little around it), like Win7: scrolling
    /// asks for the next ones, everything queued for rows scrolled away is forgotten.
    fn schedule_thumbs(self: &Rc<Self>) {
        if self.thumbs_scheduled.replace(true) { return; }
        let e = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(90), move || { if let Some(e) = e.upgrade() { e.thumbs_scheduled.set(false); e.request_visible_thumbs(); } });
    }

    fn request_visible_thumbs(&self) {
        let size = self.view.get().icon_size();
        if size < 32 || self.opts.borrow().icons_only || self.rows.borrow().is_empty() { return; }
        let range = match self.active_tree() {
            Some(t) => if t.model().as_ref() == Some(self.store().upcast_ref()) { t.visible_range() } else { None },
            None => if self.icons.model().as_ref() == Some(&self.store()) { self.icons.visible_range() } else { None },
        };
        let n = self.store().iter_n_children(None);
        let (a, b) = match range {
            Some((a, b)) => (a.indices().first().copied().unwrap_or(0), b.indices().first().copied().unwrap_or(0)),
            None => (0, 60.min(n - 1)),
        };
        let (a, b) = ((a - 20).max(0), (b + 40).min(n - 1));
        let Some(t) = self.thumbs.borrow().as_ref().map(|t| (t.new_generation(), t.tx_clone())) else { return };
        let (generation, tx) = t;
        let store = self.store();
        let rows = self.rows.borrow();
        for i in a..=b {
            let Some(it) = store.iter_nth_child(None, i) else { break };
            let Some(p) = path_of_uri(&store.value(&it, C_URI as i32).get::<String>().unwrap_or_default()) else { continue };
            let Some(r) = rows.get(&p) else { continue };
            let e = &r.e;
            if e.broken { continue; }
            let Some(kind) = listing::thumb_kind(&e.ctype) else { continue };
            if self.cached_thumb(e, size).is_some() { continue; }
            let _ = tx.send(listing::ThumbReq { generation, path: e.path.clone(), mtime: e.mtime, size, kind });
        }
    }

    fn on_thumb(self: &Rc<Self>, r: listing::ThumbResult) {
        // applied together: every row change makes the icon view lay itself out again
        let first = { let mut q = self.thumb_queue.borrow_mut(); q.push(r); q.len() == 1 };
        if !first { return; }
        let e = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(80), move || { if let Some(e) = e.upgrade() { e.apply_thumbs(); } });
    }

    fn apply_thumbs(self: &Rc<Self>) {
        let q: Vec<listing::ThumbResult> = self.thumb_queue.borrow_mut().drain(..).collect();
        let size = self.view.get().icon_size();
        let store = self.store();
        let sel = self.selected();
        let mut info = false;
        // a picture folder viewed for long keeps a bounded number of thumbnails
        if self.thumb_cache.borrow().len() > 4000 { self.thumb_cache.borrow_mut().clear(); }
        for r in q {
            if r.size != size { continue; }
            let framed = framed_thumb(&listing::result_pixbuf(&r), size, r.kind == listing::ThumbKind::Video);
            if let Some(row) = self.rows.borrow().get(&r.path) {
                if row.e.mtime != r.mtime { continue; }
                store.set_value(&row.it, C_ICON, &framed.to_value());
            }
            if sel.first() == Some(&r.path) { info = true; }
            self.thumb_cache.borrow_mut().insert((r.path, size), (r.mtime, framed));
        }
        if info { self.schedule_info(); }
    }

    /// New icon size: the rows get icons of that size (and thumbnails are asked for again).
    fn rerender_icons(&self) {
        let size = self.view.get().icon_size();
        let store = self.store();
        let rows = self.rows.borrow();
        for r in rows.values() {
            let pb = self.cached_thumb(&r.e, size).or_else(|| self.entry_icon(&r.e, size));
            store.set_value(&r.it, C_ICON, &pb.to_value());
        }
    }

    /// (Re)load the current place. Folders are read by a worker thread; the view fills in slices
    /// so the window keeps drawing and taking input.
    pub fn load(self: &Rc<Self>) {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.scan_cancel.borrow().store(true, AtomicOrdering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.scan_cancel.borrow_mut() = cancel.clone();
        if let Some(t) = self.thumbs.borrow().as_ref() { t.new_generation(); }
        self.thumb_queue.borrow_mut().clear();
        *self.monitor.borrow_mut() = None;
        self.pending.borrow_mut().clear();
        self.rows.borrow_mut().clear();
        self.virtual_rows.set(false);
        self.empty_label.hide();
        self.addr.style_context().remove_class("searching");
        let loc = self.loc.borrow().clone();
        // a fresh model, filled before the views see it (sorting once instead of per row)
        let store = new_store();
        let fv = with_views(|v| v.folders.get(&loc.key()).copied());
        let (sc, desc) = match (fv, &loc) {
            (Some(f), _) => (f.s, f.d),
            (None, Loc::Trash) => (C_DELN, true),
            _ => (C_SORT, false),
        };
        self.sort_state.set((sc, desc));
        self.show_sort_indicator();
        *self.store.borrow_mut() = store.clone();
        self.set_models(false);
        self.apply_view();
        self.lib_header.set_visible(matches!(loc, Loc::Library(_)));
        if loc == Loc::Computer { self.show_computer(); return; }
        if loc == Loc::Network { self.virtual_rows.set(true); self.set_models(true); self.show_network(generation); return; }
        if loc == Loc::Libraries {
            self.virtual_rows.set(true);
            let size = self.view.get().icon_size();
            for n in ["Documents", "Music", "Pictures", "Videos"] {
                let l = Loc::Library(n);
                let pb = util::pixbuf_or(l.icon(), size, "folder");
                let tile = format!("{n}\n<span foreground='#6d6d6d'>Library</span>");
                store.insert_with_values(None, &[(C_ICON, &pb), (C_NAME, &n), (C_URI, &format!("library:{n}")), (C_ISDIR, &true), (C_TILE, &tile), (C_CNAME, &tile),
                    (C_SORT, &n.to_lowercase()), (C_TYPE, &"Library"), (C_SENS, &true), (C_EDIT, &n), (C_CT, &"inode/directory")]);
            }
            self.set_models(true);
            self.schedule_info();
            return;
        }
        let so = self.scan_opts();
        let rx = match &loc {
            Loc::Search(base, q) => {
                let o = self.opts.borrow();
                self.addr.style_context().add_class("searching");
                listing::search(base.clone(), q.clone(), o.search_partial, o.search_subfolders, so, cancel)
            }
            Loc::Trash => listing::scan_trash(so, cancel),
            Loc::Recent => listing::scan_paths(recent_folders(), so, cancel),
            l => match l.dir() { Some(d) => listing::scan_dir(d, so, cancel), None => return },
        };
        self.loading.set(true);
        self.watch(&loc);
        let e = self.clone();
        let started = Instant::now();
        glib::MainContext::default().spawn_local(async move {
            let mut attached = false;
            while let Ok(m) = rx.recv().await {
                if e.generation.get() != generation { return; }
                match m {
                    ScanMsg::Batch(v) => {
                        let mut it = v.into_iter();
                        loop {
                            let t0 = Instant::now();
                            let mut more = false;
                            for en in it.by_ref() {
                                e.upsert(en);
                                if t0.elapsed() > Duration::from_millis(12) { more = true; break; }
                            }
                            if !more { break; }
                            // let the window draw and take input between slices
                            glib::timeout_future_with_priority(glib::Priority::DEFAULT_IDLE, Duration::ZERO).await;
                            if e.generation.get() != generation { return; }
                        }
                        if !attached { attached = true; e.resort(); e.set_models(true); }
                        e.after_batch();
                        e.schedule_thumbs();
                    }
                    ScanMsg::Error(err) => {
                        let what = match &*e.loc.borrow() { Loc::Path(p) => p.display().to_string(), l => l.title() };
                        util::error("Location is not available", &format!("{what} is not accessible.\n\n{err}"));
                    }
                    ScanMsg::Done => {
                        if std::env::var_os("AERODE_STALL_PROBE").is_some() { eprintln!("listed {} items in {:?}", e.rows.borrow().len(), started.elapsed()); }
                        if !attached { e.set_models(true); }
                        e.loading.set(false);
                        e.addr.style_context().remove_class("searching");
                        e.after_batch();
                        e.show_empty_message();
                        e.schedule_thumbs();
                        return;
                    }
                }
            }
        });
    }

    /// Only the visible view shows the folder's model (a hidden GtkIconView would lay out every
    /// item on every change); `attach` false shows the empty model while a folder is read.
    fn set_models(&self, attach: bool) {
        self.attached.set(attach);
        let store = self.store();
        let v = self.view.get();
        let pick = |active: bool| -> gtk::ListStore { if attach && active { store.clone() } else { self.empty_model.clone() } };
        let icons = pick(!matches!(v, View::Details | View::Content));
        let det = pick(v == View::Details);
        let con = pick(v == View::Content);
        self.icons.set_model(Some(&icons));
        if self.details.model().as_ref() != Some(det.upcast_ref()) { self.details.set_model(Some(&det)); }
        if self.content.model().as_ref() != Some(con.upcast_ref()) { self.content.set_model(Some(&con)); }
    }

    fn show_empty_message(&self) {
        let n = self.store().iter_n_children(None);
        if n > 0 || matches!(*self.loc.borrow(), Loc::Computer) { self.empty_label.hide(); return; }
        self.empty_label.set_text(match &*self.loc.borrow() { Loc::Search(..) => "No items match your search.", Loc::Trash => "", _ => "This folder is empty." });
        self.empty_label.show();
    }

    /// Put the rows in the current sort order (keys come from the entries, not the model).
    fn resort(&self) {
        if self.virtual_rows.get() || !self.unsorted.replace(false) { return; }
        let store = self.store();
        let n = store.iter_n_children(None) as usize;
        if n < 2 { return; }
        let (col, desc) = self.sort_state.get();
        let order: Vec<u32> = {
            let rows = self.rows.borrow();
            if rows.len() != n { return; }
            let mut keys: Vec<(u32, &Entry)> = Vec::with_capacity(n);
            for r in rows.values() {
                let Some(i) = store.path(&r.it).and_then(|p| p.indices().first().copied()) else { return };
                keys.push((i as u32, &r.e));
            }
            keys.sort_by(|a, b| cmp_entries(a.1, b.1, col, desc));
            if keys.iter().enumerate().all(|(i, k)| k.0 == i as u32) { return; }
            keys.iter().map(|k| k.0).collect()
        };
        store.reorder(&order);
    }

    fn set_sort(self: &Rc<Self>, col: u32, desc: bool) {
        self.sort_state.set((col, desc));
        self.unsorted.set(true);
        self.show_sort_indicator();
        self.resort();
        self.remember(Some((col, desc)));
        self.schedule_thumbs();
    }

    fn show_sort_indicator(&self) {
        let (col, desc) = self.sort_state.get();
        for c in self.details.columns() {
            let sc = unsafe { c.data::<u32>("sort").map(|x| *x.as_ref()) };
            c.set_sort_indicator(sc == Some(col));
            c.set_sort_order(if desc { gtk::SortType::Descending } else { gtk::SortType::Ascending });
        }
    }

    /// Selection wishes (pasted items, new folder to rename) once their rows exist.
    fn after_batch(self: &Rc<Self>) {
        self.resort();
        let want: Vec<PathBuf> = self.select_after.borrow().clone();
        if !want.is_empty() && self.attached.get() {
            let found: Vec<gtk::TreeIter> = { let rows = self.rows.borrow(); want.iter().filter_map(|p| rows.get(p).map(|r| r.it.clone())).collect() };
            if !found.is_empty() {
                { let rows = self.rows.borrow(); self.select_after.borrow_mut().retain(|p| !rows.contains_key(p)); }
                self.unselect_all();
                let store = self.store();
                for (i, it) in found.iter().enumerate() {
                    let Some(tp) = store.path(it) else { continue };
                    self.select_tree_path(&tp);
                    if i == 0 { self.set_cursor(&tp); }
                }
            }
        }
        let ren = self.rename_after.borrow().clone();
        if let (Some(p), true) = (ren, self.attached.get()) {
            let it = self.rows.borrow().get(&p).map(|r| r.it.clone());
            if let Some(it) = it {
                *self.rename_after.borrow_mut() = None;
                let Some(tp) = self.store().path(&it) else { return };
                self.unselect_all();
                self.select_tree_path(&tp);
                self.set_cursor(&tp);
                let e = self.clone();
                glib::idle_add_local_once(move || e.rename());
            }
        }
        if self.store().iter_n_children(None) > 0 { self.empty_label.set_visible(false); }
        self.schedule_info();
    }

    /// Live updates (gio file monitor): changed files are re-read off the GUI thread, coalesced.
    fn watch(self: &Rc<Self>, loc: &Loc) {
        let dir = match loc { Loc::Trash => Some(trash::home_trash().join("files")), l => l.dir() };
        let Some(dir) = dir else { return };
        let Ok(m) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else { return };
        let e = Rc::downgrade(self);
        let generation = self.generation.get();
        m.connect_changed(move |_, f, other, ev| {
            let Some(e) = e.upgrade() else { return };
            if e.generation.get() != generation { return; }
            if matches!(ev, gio::FileMonitorEvent::PreUnmount | gio::FileMonitorEvent::Unmounted) { return; }
            {
                let mut p = e.pending.borrow_mut();
                if let Some(x) = f.path() { p.insert(x); }
                if let Some(x) = other.and_then(|o| o.path()) { p.insert(x); }
            }
            e.schedule_flush(generation);
        });
        *self.monitor.borrow_mut() = Some(m);
    }

    fn schedule_flush(self: &Rc<Self>, generation: u32) {
        if self.pending_scheduled.replace(true) { return; }
        let e2 = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(150), move || { if let Some(e) = e2.upgrade() { e.flush_changes(generation); } });
    }

    fn flush_changes(self: &Rc<Self>, generation: u32) {
        self.pending_scheduled.set(false);
        if self.generation.get() != generation { return; }
        // one read at a time, so a later state never gets overwritten by an earlier one
        if self.flush_busy.get() { self.schedule_flush(generation); return; }
        let loc = self.loc.borrow().clone();
        let Some(dir) = (match &loc { Loc::Trash => None, l => l.dir() }) else { self.pending.borrow_mut().clear(); self.reload_keep_selection(); return };
        // the folder itself went away: Windows goes to the closest parent that exists
        if !dir.exists() {
            let mut p = dir.clone();
            while !p.exists() { if !p.pop() { break; } }
            self.go(Loc::Path(p), false);
            return;
        }
        let changed: Vec<PathBuf> = self.pending.borrow_mut().drain().filter(|p| p != &dir && p.parent() == Some(dir.as_path())).collect();
        if changed.is_empty() { return; }
        if changed.len() > 2000 || changed.iter().any(|p| p.file_name() == Some(std::ffi::OsStr::new(".hidden"))) { self.reload_keep_selection(); return; }
        self.flush_busy.set(true);
        let rx = listing::stat_paths(dir, changed, self.scan_opts());
        let e = self.clone();
        glib::MainContext::default().spawn_local(async move {
            let r = rx.recv().await;
            e.flush_busy.set(false);
            let Ok(v) = r else { return };
            if e.generation.get() != generation { return; }
            for (p, en) in v {
                match en { Some(en) => e.upsert(en), None => e.remove_path(&p) }
            }
            e.after_batch();
            e.show_empty_message();
            e.schedule_thumbs();
        });
    }

    fn reload_keep_selection(self: &Rc<Self>) {
        let sel = self.selected();
        *self.select_after.borrow_mut() = sel;
        self.load();
    }

    /// Recycle Bin, search results… keep the view but read everything again.
    pub fn refresh(self: &Rc<Self>) {
        self.icon_cache.borrow_mut().clear();
        self.nav.reset_folders();
        if matches!(*self.loc.borrow(), Loc::Path(_)) { self.build_crumbs(); }
        self.reload_keep_selection();
    }

    // ------------------------------------------------------------ Computer view
    fn show_computer(self: &Rc<Self>) {
        self.stack.set_visible_child_name("computer");
        for c in self.computer.children() { self.computer.remove(&c); }
        let mut disks: Vec<(String, PathBuf, &str)> = vec![("Local Disk (/)".into(), PathBuf::from("/"), util::first_icon(&["drive-harddisk-system", "drive-harddisk"]))];
        let mut removable: Vec<(String, Option<PathBuf>, Option<gio::Volume>, Option<gio::Mount>, String)> = vec![];
        let hide_empty = self.opts.borrow().hide_empty_drives;
        for m in self.vm.mounts() {
            let Some(p) = m.root().path() else { continue };
            if p == Path::new("/") { continue; }
            let rem = m.can_eject() || p.starts_with("/run/media") || p.starts_with("/media");
            if rem { removable.push((m.name().to_string(), Some(p), None, Some(m.clone()), "drive-removable-media".into())); }
            else { disks.push((m.name().to_string(), p, "drive-harddisk")); }
        }
        for v in self.vm.volumes() {
            if v.get_mount().is_some() { continue; }
            if hide_empty && !v.can_mount() { continue; }
            removable.push((v.name().to_string(), None, Some(v.clone()), None, if v.can_eject() { "drive-removable-media".into() } else { "drive-harddisk".into() }));
        }
        let group = |title: &str, n: usize| {
            let l = gtk::Label::new(Some(&format!("{title} ({n})")));
            l.set_xalign(0.0);
            l.style_context().add_class("drive-group");
            l
        };
        let mk_flow = || {
            let flow = gtk::FlowBox::new();
            flow.set_selection_mode(gtk::SelectionMode::Single);
            flow.set_activate_on_single_click(self.opts.borrow().single_click);
            flow.set_max_children_per_line(4);
            flow.set_homogeneous(true);
            flow.set_valign(gtk::Align::Start);
            flow
        };
        self.computer.pack_start(&group("Hard Disk Drives", disks.len()), false, false, 0);
        self.computer.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
        let flow = mk_flow();
        for (name, p, icon) in disks { flow.insert(&self.drive_tile(&name, icon, Some(p), None, None), -1); }
        self.computer.pack_start(&flow, false, false, 0);
        let mut flows = vec![flow];
        if !removable.is_empty() {
            self.computer.pack_start(&group("Devices with Removable Storage", removable.len()), false, false, 0);
            self.computer.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
            let flow = mk_flow();
            for (name, p, vol, mnt, icon) in removable { flow.insert(&self.drive_tile(&name, &icon, p, vol, mnt), -1); }
            self.computer.pack_start(&flow, false, false, 0);
            flows.push(flow);
        }
        // one selection across both groups; double-click (or Enter) opens
        for f in &flows {
            let others: Vec<gtk::FlowBox> = flows.iter().filter(|x| *x != f).cloned().collect();
            f.connect_selected_children_changed(move |f| { if !f.selected_children().is_empty() { for o in &others { o.unselect_all(); } } });
            f.connect_child_activated(|_, c| { if let Some(w) = c.child() { if let Ok(b) = w.downcast::<gtk::EventBox>() { b.activate(); open_drive_child(&b); } } });
        }
        self.computer.show_all();
        self.info_icon.set_from_pixbuf(util::pixbuf("computer", 48).as_ref());
        self.info_name.set_text(&glib::host_name());
        self.clear_info_grid();
        let cpu = std::fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| s.lines().find_map(|l| l.strip_prefix("model name").map(|v| v.trim_start_matches([' ', '\t', ':']).to_string()))).unwrap_or_default();
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| s.lines().next().and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())).unwrap_or(0);
        self.info_sub.set_text("");
        self.add_info_row(0, 0, "Workgroup:", "WORKGROUP");
        self.add_info_row(1, 0, "Memory:", &format!("{:.2} GB", mem as f64 / 1048576.0));
        self.add_info_row(0, 1, "Processor:", &cpu);
        self.update_cmd();
    }

    fn drive_tile(self: &Rc<Self>, name: &str, icon: &str, path: Option<PathBuf>, vol: Option<gio::Volume>, mnt: Option<gio::Mount>) -> gtk::Widget {
        let ev = gtk::EventBox::new();
        ev.style_context().add_class("drive-tile");
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        h.set_border_width(4);
        h.pack_start(&util::image(icon, 48), false, false, 0);
        let v = gtk::Box::new(gtk::Orientation::Vertical, 3);
        v.set_valign(gtk::Align::Center);
        let l = gtk::Label::new(Some(name));
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_max_width_chars(28);
        v.pack_start(&l, false, false, 0);
        if let Some(p) = &path {
            // filesystem info can hang on network drives: ask asynchronously
            let bar = gtk::LevelBar::for_interval(0.0, 1.0);
            bar.style_context().add_class("drive");
            bar.set_size_request(190, 14);
            // Win7 turns the bar red when a drive is >90% full
            bar.add_offset_value("full", 0.9);
            bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 0.0);
            bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, 0.0);
            let t = gtk::Label::new(None);
            t.set_xalign(0.0);
            t.style_context().add_class("dim-label");
            v.pack_start(&bar, false, false, 0);
            v.pack_start(&t, false, false, 0);
            let (b2, t2) = (bar.clone(), t.clone());
            gio::File::for_path(p).query_filesystem_info_async("filesystem::size,filesystem::free", glib::Priority::DEFAULT, gio::Cancellable::NONE, move |r| {
                let Ok(info) = r else { b2.hide(); return };
                let (size, free) = (info.attribute_uint64("filesystem::size"), info.attribute_uint64("filesystem::free"));
                let used = if size > 0 { 1.0 - free as f64 / size as f64 } else { 0.0 };
                b2.set_value(used);
                t2.set_text(&format!("{} free of {}", util::human_size(free), util::human_size(size)));
            });
        }
        h.pack_start(&v, true, true, 0);
        ev.add(&h);
        ev.set_tooltip_text(path.as_ref().map(|p| p.display().to_string()).as_deref());
        // what opening does, kept on the widget for FlowBox activation
        let e = self.clone();
        let open: Rc<dyn Fn()> = Rc::new({
            let path = path.clone();
            let vol = vol.clone();
            move || {
                if let Some(p) = &path { e.go(Loc::Path(p.clone()), true); }
                else if let Some(v) = &vol {
                    let e2 = e.clone();
                    v.mount(gio::MountMountFlags::NONE, Some(&gtk::MountOperation::new(Some(&e.win))), gio::Cancellable::NONE, move |r| {
                        match r { Ok(()) => e2.load(), Err(err) => util::error("Could not open the drive", err.message()) }
                    });
                }
            }
        });
        unsafe { ev.set_data("open", open.clone()); }
        // right-click: Open, Eject, Properties
        {
            let e = self.clone();
            let (path, mnt, vol) = (path.clone(), mnt.clone(), vol.clone());
            ev.connect_button_press_event(move |w, ev| {
                if ev.button() == 3 {
                    if let Some(c) = w.parent().and_then(|p| p.downcast::<gtk::FlowBoxChild>().ok()) {
                        if let Some(f) = c.parent().and_then(|p| p.downcast::<gtk::FlowBox>().ok()) { f.select_child(&c); }
                    }
                    let m = gtk::Menu::new();
                    let o = gtk::MenuItem::with_mnemonic("_Open");
                    let op = open.clone();
                    o.connect_activate(move |_| op());
                    m.append(&o);
                    if let Some(p) = path.clone() {
                        let it = gtk::MenuItem::with_mnemonic("Open in new _window");
                        let e2 = e.clone();
                        it.connect_activate(move |_| e2.open_window(Loc::Path(p.clone())));
                        m.append(&it);
                    }
                    if let Some(mt) = mnt.clone().filter(|m| m.can_eject() || m.can_unmount()) {
                        m.append(&gtk::SeparatorMenuItem::new());
                        let it = gtk::MenuItem::with_mnemonic("_Eject");
                        let e2 = e.clone();
                        it.connect_activate(move |_| {
                            let e3 = e2.clone();
                            let done = move |r: Result<(), glib::Error>| { match r { Ok(()) => e3.load(), Err(err) => util::error("Problem Ejecting Device", err.message()) } };
                            if mt.can_eject() { mt.eject_with_operation(gio::MountUnmountFlags::NONE, gio::MountOperation::NONE, gio::Cancellable::NONE, done); }
                            else { mt.unmount_with_operation(gio::MountUnmountFlags::NONE, gio::MountOperation::NONE, gio::Cancellable::NONE, done); }
                        });
                        m.append(&it);
                    } else if let Some(v) = vol.clone().filter(|v| v.can_eject()) {
                        m.append(&gtk::SeparatorMenuItem::new());
                        let it = gtk::MenuItem::with_mnemonic("_Eject");
                        it.connect_activate(move |_| { v.eject_with_operation(gio::MountUnmountFlags::NONE, gio::MountOperation::NONE, gio::Cancellable::NONE, |_| {}); });
                        m.append(&it);
                    }
                    if let Some(p) = path.clone() {
                        m.append(&gtk::SeparatorMenuItem::new());
                        let it = gtk::MenuItem::with_mnemonic("P_roperties");
                        let w = e.win.clone();
                        it.connect_activate(move |_| props::show(&[p.clone()], Some(w.upcast_ref())));
                        m.append(&it);
                    }
                    m.show_all();
                    m.popup_at_pointer(Some(ev));
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        ev.upcast()
    }

    fn show_network(self: &Rc<Self>, generation: u32) {
        // gvfs network browsing, read off the GUI thread (it can take seconds)
        let (tx, rx) = async_channel::bounded::<Vec<(String, String, Vec<String>)>>(1);
        std::thread::spawn(move || {
            let mut v = vec![];
            let gf = gio::File::for_uri("network:///");
            if let Ok(en) = gf.enumerate_children("standard::name,standard::display-name,standard::icon,standard::target-uri", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE) {
                while let Ok(Some(info)) = en.next_file(gio::Cancellable::NONE) {
                    let target = info.attribute_string("standard::target-uri").map(|s| s.to_string()).unwrap_or_else(|| format!("network:///{}", info.name().display()));
                    let icons = info.icon().and_then(|i| i.downcast::<gio::ThemedIcon>().ok()).map(|t| t.names().iter().map(|s| s.to_string()).collect()).unwrap_or_default();
                    v.push((info.display_name().to_string(), target, icons));
                }
            }
            let _ = tx.send_blocking(v);
        });
        let e = Rc::downgrade(self);
        self.loading.set(true);
        glib::MainContext::default().spawn_local(async move {
            let Ok(items) = rx.recv().await else { return };
            let Some(e) = e.upgrade() else { return };
            if e.generation.get() != generation { return; }
            let size = e.view.get().icon_size();
            let store = e.store();
            for (name, target, icons) in &items {
                let refs: Vec<&str> = icons.iter().map(|s| s.as_str()).collect();
                let pb = util::pixbuf(util::first_icon(&refs), size).or_else(|| util::pixbuf_or("network-server", size, "folder-remote"));
                store.insert_with_values(None, &[(C_ICON, &pb), (C_NAME, name), (C_URI, target), (C_ISDIR, &true), (C_TILE, &glib::markup_escape_text(name).to_string()), (C_CNAME, &glib::markup_escape_text(name).to_string()),
                    (C_SORT, &name.to_lowercase()), (C_TYPE, &"Computer"), (C_SENS, &true), (C_EDIT, name), (C_CT, &"inode/directory")]);
            }
            e.loading.set(false);
            if items.is_empty() {
                e.empty_label.set_text("Network discovery found no computers or devices.\n(Network browsing needs gvfs-smb and avahi.)");
                e.empty_label.show();
            }
            e.schedule_info();
        });
    }

    // ------------------------------------------------------------ views
    fn apply_view(&self) {
        let v = self.view.get();
        self.views_b.set_image(Some(&util::image(v.icon_name(), 16)));
        self.views_b.set_always_show_image(true);
        let loc = self.loc.borrow().clone();
        // Details columns per place (Recycle Bin: Original Location, Date Deleted; search: Folder)
        let trash = loc == Loc::Trash;
        let search = matches!(loc, Loc::Search(..));
        for (k, c) in &self.det_cols {
            c.set_visible(match *k { "orig" | "deleted" => trash, "folder" => search, _ => true });
        }
        // Win7's column order: the Recycle Bin has Name, Original Location, Date Deleted, Size,
        // Item type, Date modified; folders Name, Date modified, Type, Size (search: + Folder)
        let order: &[&str] = if trash { &["orig", "deleted", "size", "type", "date"] } else { &["date", "type", "size", "folder"] };
        let mut prev = self.det_name.clone();
        for k in order {
            if let Some(c) = self.det_cols.get(k) { self.details.move_column_after(c, Some(&prev)); prev = c.clone(); }
        }
        if let Some(c) = self.det_cols.get("type") { c.set_title(if trash { "Item type" } else { "Type" }); }
        match v {
            View::Details => { self.stack.set_visible_child_name("details"); return; }
            View::Content => { self.stack.set_visible_child_name("content"); return; }
            _ => self.stack.set_visible_child_name("icons"),
        }
        self.icons.set_layout(match v {
            View::XLarge => grid::Layout::Icons { icon: 256, cw: 270 },
            View::Large => grid::Layout::Icons { icon: 96, cw: 112 },
            View::Medium => grid::Layout::Icons { icon: 48, cw: 80 },
            View::Small => grid::Layout::Small,
            View::List => grid::Layout::List,
            _ => grid::Layout::Tiles,
        });
    }

    fn remember(&self, sort: Option<(u32, bool)>) {
        let loc = self.loc.borrow().clone();
        let key = loc.key();
        let (s, d) = sort.unwrap_or_else(|| self.sort_state.get());
        let fv = FolderView { v: self.view.get().index(), s, d };
        with_views(|vs| {
            if vs.folders.get(&key) == Some(&fv) { return; }
            vs.folders.insert(key.clone(), fv);
            vs.order.retain(|k| k != &key);
            vs.order.push(key);
            // Windows keeps the last 5000 folders
            while vs.order.len() > 5000 { let k = vs.order.remove(0); vs.folders.remove(&k); }
        });
        save_views();
    }

    fn set_view(self: &Rc<Self>, v: View) {
        if self.view.get() == v { return; }
        let old = self.view.get().icon_size();
        let sel = self.selected_tree_paths();
        let cursor = self.cursor_path();
        let attached = self.attached.get();
        self.view.set(v);
        self.remember(None);
        let loc = self.loc.borrow().clone();
        if loc == Loc::Computer { self.apply_view(); return; }
        // libraries and network computers are few: read them again at the new size
        if old != v.icon_size() && self.virtual_rows.get() { self.reload_keep_selection(); return; }
        // the model moves to the new view; icons of the new size are drawn from what was read
        self.set_models(false);
        if old != v.icon_size() { self.rerender_icons(); }
        self.apply_view();
        self.set_models(attached);
        for p in &sel { self.select_tree_path(p); }
        if let Some(c) = cursor.or_else(|| sel.first().cloned()) { self.set_cursor_only(&c); }
        self.schedule_thumbs();
    }

    fn step_view(self: &Rc<Self>, dir: i32) {
        let i = (self.view.get().index() as i32 + dir).clamp(0, View::ALL.len() as i32 - 1);
        self.set_view(View::ALL[i as usize]);
    }

    // ------------------------------------------------------------ selection
    fn active_tree(&self) -> Option<&gtk::TreeView> {
        match self.view.get() { View::Details => Some(&self.details), View::Content => Some(&self.content), _ => None }
    }

    fn selected_tree_paths(&self) -> Vec<gtk::TreePath> {
        // while a folder loads the views have no model yet (and gtk-rs panics on a NULL model)
        match self.active_tree() {
            Some(t) => if t.model().is_some() { t.selection().selected_rows().0 } else { vec![] },
            None => self.icons.selected_items(),
        }
    }

    fn cursor_path(&self) -> Option<gtk::TreePath> {
        match self.active_tree() {
            Some(t) => t.cursor().0,
            None => self.icons.cursor(),
        }
    }

    /// Move the focus row without changing the selection, and scroll to it.
    fn set_cursor_only(&self, p: &gtk::TreePath) {
        let sel = self.selected_tree_paths();
        self.set_cursor(p);
        // GtkTreeView.set_cursor selects the row: put the selection back
        self.unselect_all();
        for s in &sel { self.select_tree_path(s); }
    }

    fn select_tree_path(&self, p: &gtk::TreePath) {
        match self.active_tree() { Some(t) => t.selection().select_path(p), None => self.icons.select_path(p) }
    }

    fn unselect_all(&self) {
        match self.active_tree() { Some(t) => t.selection().unselect_all(), None => self.icons.unselect_all() }
    }

    fn select_all(&self) {
        match self.active_tree() { Some(t) => t.selection().select_all(), None => self.icons.select_all() }
    }

    fn set_cursor(&self, p: &gtk::TreePath) {
        match self.active_tree() {
            Some(t) => { t.set_cursor(p, None::<&gtk::TreeViewColumn>, false); t.scroll_to_cell(Some(p), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0); }
            None => self.icons.set_cursor(p),
        }
    }

    fn uri_at(&self, p: &gtk::TreePath) -> Option<String> {
        let s = self.store();
        let it = s.iter(p)?;
        s.value(&it, C_URI as i32).get::<String>().ok()
    }

    /// Selected items as URIs (files and virtual items like libraries).
    fn selected_uris(&self) -> Vec<String> {
        self.selected_tree_paths().iter().filter_map(|p| self.uri_at(p)).collect()
    }

    /// Selected files and folders.
    pub fn selected(&self) -> Vec<PathBuf> {
        self.selected_uris().iter().filter_map(|u| path_of_uri(u)).collect()
    }

    fn schedule_info(self: &Rc<Self>) {
        if self.info_scheduled.replace(true) { return; }
        let e = Rc::downgrade(self);
        glib::idle_add_local_once(move || { if let Some(e) = e.upgrade() { e.info_scheduled.set(false); e.update_info(); e.update_cmd(); } });
    }

    fn clear_info_grid(&self) {
        for c in self.info_grid.children() { self.info_grid.remove(&c); }
    }

    fn add_info_row(&self, row: i32, col: i32, k: &str, v: &str) {
        let kl = gtk::Label::new(Some(k));
        kl.set_xalign(1.0);
        kl.style_context().add_class("key");
        let vl = gtk::Label::new(Some(v));
        vl.set_xalign(0.0);
        vl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        vl.set_max_width_chars(34);
        self.info_grid.attach(&kl, col * 2, row, 1, 1);
        self.info_grid.attach(&vl, col * 2 + 1, row, 1, 1);
        kl.show();
        vl.show();
    }

    /// The details pane at the bottom (and the preview pane).
    fn update_info(self: &Rc<Self>) {
        let loc = self.loc.borrow().clone();
        if loc == Loc::Computer { return; }
        self.clear_info_grid();
        let store = self.store();
        let paths = self.selected_tree_paths();
        let get_s = |it: &gtk::TreeIter, c: u32| store.value(it, c as i32).get::<String>().unwrap_or_default();
        if paths.is_empty() {
            let n = store.iter_n_children(None);
            self.info_icon.set_from_pixbuf(util::pixbuf(loc.icon(), 48).as_ref());
            match &loc {
                Loc::Library(n2) => {
                    self.info_name.set_text(&format!("{n2} library"));
                    self.info_sub.set_text(&format!("{n} item{}", if n == 1 { "" } else { "s" }));
                    self.add_info_row(0, 0, "Library includes:", "1 location");
                }
                Loc::Search(_, _) if self.loading.get() => { self.info_name.set_text("Searching..."); self.info_sub.set_text(&format!("{n} items")); }
                _ => { self.info_name.set_text(&format!("{n} item{}", if n == 1 { "" } else { "s" })); self.info_sub.set_text(""); }
            }
            self.update_preview(None);
            return;
        }
        let iters: Vec<gtk::TreeIter> = paths.iter().filter_map(|p| store.iter(p)).collect();
        if iters.len() == 1 {
            let it = &iters[0];
            let pb: Option<Pixbuf> = store.value(it, C_ICON as i32).get().ok().flatten();
            // a bigger icon (or the thumbnail) for the pane
            let uri = get_s(it, C_URI);
            let path = path_of_uri(&uri);
            let ct = get_s(it, C_CT);
            let big = path.as_ref().and_then(|p| self.thumb_cache.borrow().iter().find(|((q, _), _)| q == p).map(|(_, v)| v.1.clone()))
                .map(|t| scale_fit(&t, 56))
                .or_else(|| {
                    let isdir = store.value(it, C_ISDIR as i32).get::<bool>().unwrap_or(false);
                    if isdir { path.as_ref().map(|p| nav::folder_icon(p)).and_then(|n| util::pixbuf_or(n, 48, "folder")) }
                    else { let gi = gio::content_type_get_icon(&ct); util::gicon_pixbuf(Some(&gi), 48) }
                }).or(pb);
            self.info_icon.set_from_pixbuf(big.as_ref());
            self.info_name.set_text(&get_s(it, C_NAME));
            self.info_sub.set_text(&get_s(it, C_TYPE));
            let isdir = store.value(it, C_ISDIR as i32).get::<bool>().unwrap_or(false);
            if loc == Loc::Trash {
                self.add_info_row(0, 0, "Original location:", &get_s(it, C_ORIG));
                self.add_info_row(1, 0, "Date deleted:", &get_s(it, C_DEL));
                if !isdir { self.add_info_row(2, 0, "Size:", &util::human_size(store.value(it, C_SIZEN as i32).get::<u64>().unwrap_or(0))); }
            } else if let Some(p) = &path {
                self.add_info_row(0, 0, "Date modified:", &get_s(it, C_DATE));
                if !isdir { self.add_info_row(1, 0, "Size:", &util::human_size(store.value(it, C_SIZEN as i32).get::<u64>().unwrap_or(0))); }
                if let Ok(m) = std::fs::metadata(p) {
                    if let Ok(t) = m.created() { self.add_info_row(if isdir { 1 } else { 2 }, 0, "Date created:", &chrono::DateTime::<chrono::Local>::from(t).format("%-m/%-d/%Y %-I:%M %p").to_string()); }
                }
                if ct.starts_with("image/") {
                    if let Some((_, w, h)) = Pixbuf::file_info(p) { self.add_info_row(0, 1, "Dimensions:", &format!("{w} x {h}")); }
                }
                if matches!(loc, Loc::Search(..)) { self.add_info_row(1, 1, "Folder:", &get_s(it, C_ORIG)); }
            }
            self.update_preview(path.as_deref().map(|p| (p, ct.as_str(), isdir)));
        } else {
            let total: u64 = iters.iter().map(|it| store.value(it, C_SIZEN as i32).get::<u64>().unwrap_or(0)).sum();
            let dates: Vec<i64> = iters.iter().map(|it| store.value(it, C_DATEN as i32).get::<i64>().unwrap_or(0)).filter(|d| *d > 0).collect();
            self.info_icon.set_from_pixbuf(util::pixbuf_or("document-multiple", 48, "text-x-generic").as_ref());
            self.info_name.set_text(&format!("{} items selected", iters.len()));
            self.info_sub.set_text("");
            if let (Some(a), Some(b)) = (dates.iter().min(), dates.iter().max()) {
                let (da, db) = (fmt_date(*a), fmt_date(*b));
                let d = |s: &str| s.split(' ').next().unwrap_or("").to_string();
                self.add_info_row(0, 0, "Date modified:", &if d(&da) == d(&db) { d(&da) } else { format!("{} - {}", d(&da), d(&db)) });
            }
            self.add_info_row(1, 0, "Size:", &util::human_size(total));
            self.update_preview(None);
        }
    }

    fn update_preview(&self, sel: Option<(&Path, &str, bool)>) {
        if !self.preview_box.is_visible() { return; }
        self.preview.hide();
        self.preview_sw.hide();
        self.preview_msg.show();
        let Some((p, ct, isdir)) = sel else { self.preview_msg.set_text("Select a file to preview."); return };
        if isdir { self.preview_msg.set_text("No preview available."); return; }
        if ct.starts_with("image/") {
            let w = (self.preview_box.allocated_width() - 16).max(120);
            let h = (self.preview_box.allocated_height() - 16).max(120);
            let (img, msg) = (self.preview.clone(), self.preview_msg.clone());
            let want = p.to_path_buf();
            self.preview_msg.set_text("");
            let current = Rc::new(want.clone());
            unsafe { self.preview.set_data("want", want.clone()); }
            load_image_async(want, w, h, move |pb| {
                let still = unsafe { img.data::<PathBuf>("want").map(|x| x.as_ref() == current.as_ref()).unwrap_or(false) };
                if !still { return; }
                match pb { Some(pb) => { img.set_from_pixbuf(Some(&pb)); img.show(); msg.hide(); } None => msg.set_text("No preview available.") }
            });
            return;
        }
        let texty = ct.starts_with("text/") || matches!(ct, "application/json" | "application/xml" | "application/x-shellscript" | "application/javascript" | "application/toml" | "application/x-yaml" | "application/x-desktop");
        if texty {
            let mut buf = vec![0u8; 64 * 1024];
            let n = std::fs::File::open(p).and_then(|mut f| { use std::io::Read; f.read(&mut buf) }).unwrap_or(0);
            buf.truncate(n);
            self.preview_text.buffer().map(|b| b.set_text(&String::from_utf8_lossy(&buf)));
            self.preview_sw.show_all();
            self.preview_msg.hide();
            return;
        }
        self.preview_msg.set_text("No preview available.");
    }

    /// Context-sensitive command bar buttons (Win7 changes them with the selection).
    fn update_cmd(self: &Rc<Self>) {
        for c in self.cmd_dyn.children() { self.cmd_dyn.remove(&c); }
        let sel = self.selected();
        let uris = self.selected_uris();
        let loc = self.loc.borrow().clone();
        let add = |label: &str, f: Box<dyn Fn()>| {
            let b = gtk::Button::with_label(label);
            b.set_relief(gtk::ReliefStyle::None);
            b.set_focus_on_click(false);
            b.connect_clicked(move |_| f());
            self.cmd_dyn.pack_start(&b, false, false, 0);
            b
        };
        match &loc {
            Loc::Trash => {
                let e = self.clone();
                add("Empty the Recycle Bin", Box::new(move || { let e2 = e.clone(); ops::empty_trash(e.parent().as_ref(), move || e2.refresh()); })).set_sensitive(!trash::is_empty());
                let e = self.clone();
                let label = if sel.is_empty() { "Restore all items" } else if sel.len() == 1 { "Restore this item" } else { "Restore the selected items" };
                add(label, Box::new(move || e.restore_selected()));
            }
            Loc::Computer => {
                add("System properties", Box::new(|| { util::spawn(&["aerode-control", "system"]); }));
                add("Uninstall or change a program", Box::new(|| { util::spawn(&["aerode-control", "programs"]); }));
                add("Open Control Panel", Box::new(|| { util::spawn(&["aerode-control"]); }));
            }
            Loc::Libraries => { add("New library", Box::new(|| { util::error("New library", "Libraries can't be created yet."); })); }
            _ => {
                if sel.len() == 1 || (uris.len() == 1 && sel.is_empty()) {
                    let e = self.clone();
                    let isfile = sel.first().map(|p| p.is_file()).unwrap_or(false);
                    if isfile {
                        // "Open ▾" lists the programs for this type
                        let p = sel[0].clone();
                        let b = add("Open ▾", Box::new(|| {}));
                        let w = self.win.clone();
                        b.connect_clicked(move |b| { let m = open_with_menu(&p, &w); m.show_all(); m.popup_at_widget(b, gdk::Gravity::SouthWest, gdk::Gravity::NorthWest, None); });
                        let p = sel[0].clone();
                        add("Print", Box::new(move || { if !util::spawn(&["lp", &p.to_string_lossy()]) { util::error("Print", "No printer software (cups) was found."); } }));
                        let p = sel[0].clone();
                        add("E-mail", Box::new(move || { if !util::spawn(&["xdg-email", "--attach", &p.to_string_lossy()]) { util::open_uri("mailto:"); } }));
                    } else {
                        add("Open", Box::new(move || e.activate_selected()));
                    }
                }
                if !sel.is_empty() && sel.iter().all(|p| p.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false)) && sel.len() == 1 {
                    let (e, p) = (self.clone(), sel[0].clone());
                    add("Extract all files", Box::new(move || { let e2 = e.clone(); ops::extract(&p, move |t| if let Some(t) = t { e2.select_after.borrow_mut().push(t); }); }));
                }
                if let Loc::Library("Pictures") = loc { let e = self.clone(); add("Slide show", Box::new(move || e.slide_show())); }
                if self.cur_dir().is_some() {
                    let e = self.clone();
                    add("New folder", Box::new(move || e.new_folder()));
                }
            }
        }
        self.cmd_dyn.show_all();
    }

    fn slide_show(&self) {
        let imgs: Vec<PathBuf> = {
            let store = self.store();
            let mut v = vec![];
            if let Some(it) = store.iter_first() { loop { let ct: String = store.value(&it, C_CT as i32).get().unwrap_or_default(); if ct.starts_with("image/") { if let Some(p) = path_of_uri(&store.value(&it, C_URI as i32).get::<String>().unwrap_or_default()) { v.push(p); } } if !store.iter_next(&it) { break; } } }
            v
        };
        if let Some(first) = self.selected().into_iter().find(|p| imgs.contains(p)).or_else(|| imgs.first().cloned()) { self.open_file(&first); }
    }

    // ------------------------------------------------------------ opening
    fn activate_selected(self: &Rc<Self>) {
        let uris = self.selected_uris();
        if uris.is_empty() { return; }
        let loc = self.loc.borrow().clone();
        if loc == Loc::Trash {
            // items in the Recycle Bin can't be opened, Windows shows their properties
            props::show(&self.selected(), self.parent().as_ref());
            return;
        }
        let dirs: Vec<&String> = uris.iter().filter(|u| u.starts_with("library:") || path_of_uri(u).map(|p| p.is_dir()).unwrap_or(false)).collect();
        if uris.len() == 1 {
            let u = &uris[0];
            if let Some(n) = u.strip_prefix("library:") { self.go(Loc::parse(&format!("library:{n}")), true); return; }
            if !u.starts_with("file://") { self.open_network(u); return; }
        }
        if dirs.len() == 1 && uris.len() == 1 {
            let p = path_of_uri(dirs[0]).unwrap_or_default();
            // Folder Options > Open each folder in its own window
            if self.opts.borrow().own_window { self.open_window(Loc::Path(p)); return; }
            self.go(Loc::Path(p), true);
            return;
        }
        for u in &uris {
            match path_of_uri(u) {
                Some(p) if p.is_dir() => self.open_window(Loc::Path(p)),
                Some(p) => self.open_file(&p),
                None => {}
            }
        }
    }

    fn open_network(self: &Rc<Self>, uri: &str) {
        let f = gio::File::for_uri(uri);
        if let Some(p) = f.path() { self.go(Loc::Path(p), true); return; }
        let e = self.clone();
        let f2 = f.clone();
        f.mount_enclosing_volume(gio::MountMountFlags::NONE, Some(&gtk::MountOperation::new(Some(&self.win))), gio::Cancellable::NONE, move |r| {
            if let Some(p) = f2.path() { e.go(Loc::Path(p), true); }
            else if let Err(err) = r { if !err.matches(gio::IOErrorEnum::AlreadyMounted) { util::error("Windows cannot access this location", err.message()); } else { util::open_uri(&f2.uri()); } }
            else { util::open_uri(&f2.uri()); }
        });
    }

    /// Open a file with its program (Win7 runs shortcuts and programs, asks for anything unknown).
    fn open_file(&self, p: &Path) {
        open_file(p, Some(self.win.upcast_ref()));
    }

    /// Folder Options that change how the views react (single-click to open).
    fn apply_folder_options(&self) {
        let single = self.opts.borrow().single_click;
        self.icons.set_activate_on_single_click(single);
        self.details.set_activate_on_single_click(single);
        self.content.set_activate_on_single_click(single);
    }

    fn cur_dir(&self) -> Option<PathBuf> {
        self.loc.borrow().dir()
    }

    // ------------------------------------------------------------ file commands
    fn new_folder(self: &Rc<Self>) {
        let Some(d) = self.cur_dir() else { return };
        if let Some(p) = ops::create_new(&d, true) { self.show_new_item(&p, true); }
    }

    fn new_text(self: &Rc<Self>) {
        let Some(d) = self.cur_dir() else { return };
        if let Some(p) = ops::create_new(&d, false) { self.show_new_item(&p, true); }
    }

    /// Put a just-created item in the view right away (the file monitor would be later) and select it.
    fn show_new_item(self: &Rc<Self>, p: &Path, rename: bool) {
        if p.parent() == self.cur_dir().as_deref() {
            if let Some(mut en) = listing::stat_entry(p, None) { en.render(self.opts.borrow().hide_ext); self.upsert(en); }
        }
        if rename { *self.rename_after.borrow_mut() = Some(p.to_path_buf()); } else { self.select_after.borrow_mut().push(p.to_path_buf()); }
        self.after_batch();
    }

    fn rename(self: &Rc<Self>) {
        let paths = self.selected_tree_paths();
        let Some(tp) = paths.first().cloned() else { return };
        let store = self.store();
        let Some(it) = store.iter(&tp) else { return };
        let Some(path) = path_of_uri(&store.value(&it, C_URI as i32).get::<String>().unwrap_or_default()) else { return };
        if *self.loc.borrow() == Loc::Trash { return; }
        let real: String = store.value(&it, C_EDIT as i32).get().unwrap_or_default();
        let shown: String = store.value(&it, C_NAME as i32).get().unwrap_or_default();
        // with extensions hidden only the visible name is edited; the extension stays
        let edit = if real != shown && real.starts_with(&shown) && real[shown.len()..].starts_with('.') && path.extension().is_some() && !path.is_dir() { shown.clone() } else { real.clone() };
        self.unselect_all();
        self.select_tree_path(&tp);
        match self.view.get() {
            View::Details | View::Content => {
                *self.renaming.borrow_mut() = Some((path, edit));
                if self.view.get() == View::Details { self.det_text.set_editable(true); self.details.set_cursor_on_cell(&tp, Some(&self.det_name), Some(&self.det_text), true); }
                else { self.con_text.set_editable(true); self.content.set_cursor_on_cell(&tp, Some(&self.con_col), Some(&self.con_text), true); }
            }
            _ => {
                let stem = rename_stem(&(path.clone(), edit.clone()));
                let e = Rc::downgrade(self);
                // start_rename first ends a rename still open (its callback clears `renaming`)
                self.icons.start_rename(&tp, &edit, stem, move |t| { if let Some(e) = e.upgrade() { e.finish_rename(t); } });
                *self.renaming.borrow_mut() = Some((path, edit));
            }
        }
    }

    fn finish_rename(self: &Rc<Self>, new_text: Option<String>) {
        for r in [&self.det_text, &self.con_text] { r.set_editable(false); }
        let Some((path, edit)) = self.renaming.borrow_mut().take() else { return };
        let Some(t) = new_text else { return };
        let real = ops::display_name(&path);
        // the hidden extension is kept
        let new_name = match real.get(edit.len()..) { Some(ext) if edit != real && real.starts_with(&edit) => format!("{t}{ext}"), _ => t };
        if let Some(newp) = ops::rename(&path, &new_name, self.parent().as_ref()) {
            self.remove_path(&path);
            self.show_new_item(&newp, false);
        }
    }

    fn copy(&self, cut: bool) {
        let s = self.selected();
        if s.is_empty() { return; }
        ops::set_clipboard(&s, cut);
        // cut items are drawn faded until pasted
        let store = self.store();
        for (p, r) in self.rows.borrow().iter() {
            let ghost = ops::is_cut(p) || r.e.hidden;
            store.set_value(&r.it, C_SENS, &(!ghost).to_value());
        }
    }

    fn paste(self: &Rc<Self>, shortcut: bool) {
        let Some(dest) = self.cur_dir() else { return };
        let e = self.clone();
        ops::get_clipboard(move |files, cut| {
            if files.is_empty() { return; }
            if shortcut {
                let made = ops::create_shortcuts(&files, &dest);
                for m in made { e.show_new_item(&m, false); }
                return;
            }
            let e2 = e.clone();
            ops::transfer(files, dest.clone(), if cut { ops::Kind::Move } else { ops::Kind::Copy }, e.parent().as_ref(), move |made| {
                e2.select_after.borrow_mut().extend(made);
                e2.after_batch();
            });
            // a cut is used up by the paste
            if cut { gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).clear(); ops::clear_cut(); }
        });
    }

    fn delete(self: &Rc<Self>, permanent: bool) {
        let s = self.selected();
        if s.is_empty() { return; }
        let e = self.clone();
        ops::delete(s, permanent, self.parent().as_ref(), true, move || e.schedule_info());
    }

    fn undo(self: &Rc<Self>) {
        let e = self.clone();
        ops::undo(self.parent().as_ref(), move || e.schedule_info());
    }

    fn restore_selected(self: &Rc<Self>) {
        let sel = self.selected();
        let items: Vec<trash::Trashed> = if sel.is_empty() { trash::list() } else { sel.iter().filter_map(|p| trash::lookup(p)).collect() };
        if items.is_empty() { return; }
        let e = self.clone();
        ops::restore(items, self.parent().as_ref(), move || e.refresh());
    }

    fn properties(&self) {
        let s = self.selected();
        if !s.is_empty() { props::show(&s, self.parent().as_ref()); return; }
        match &*self.loc.borrow() {
            Loc::Computer => { util::spawn(&["aerode-control", "system"]); }
            l => if let Some(d) = l.dir() { props::show(&[d], self.parent().as_ref()); },
        }
    }

    // ------------------------------------------------------------ menus
    fn add_item(m: &gtk::Menu, label: &str, f: impl Fn() + 'static) -> gtk::MenuItem {
        let it = gtk::MenuItem::with_mnemonic(label);
        it.connect_activate(move |_| f());
        m.append(&it);
        it
    }

    fn view_menu(self: &Rc<Self>) -> gtk::Menu {
        let view = gtk::Menu::new();
        for v in View::ALL {
            let it = gtk::CheckMenuItem::with_mnemonic(v.menu_label());
            it.set_draw_as_radio(true);
            it.set_active(self.view.get() == v);
            let e = self.clone();
            it.connect_activate(move |i| if i.is_active() { e.set_view(v) });
            view.append(&it);
        }
        view
    }

    fn sort_menu(self: &Rc<Self>) -> gtk::Menu {
        let sort = gtk::Menu::new();
        let (cur, desc) = self.sort_state.get();
        let order = if desc { gtk::SortType::Descending } else { gtk::SortType::Ascending };
        let cols: Vec<(&str, u32)> = if *self.loc.borrow() == Loc::Trash {
            vec![("_Name", C_SORT), ("_Original Location", C_ORIG), ("Date _Deleted", C_DELN), ("_Size", C_SIZEN), ("_Item type", C_TYPE), ("Date _modified", C_DATEN)]
        } else { vec![("_Name", C_SORT), ("Date _modified", C_DATEN), ("_Type", C_TYPE), ("_Size", C_SIZEN)] };
        for (l, c) in cols {
            let it = gtk::CheckMenuItem::with_mnemonic(l);
            it.set_draw_as_radio(true);
            it.set_active(c == cur);
            let e = self.clone();
            it.connect_activate(move |i| if i.is_active() { e.set_sort(c, desc) });
            sort.append(&it);
        }
        sort.append(&gtk::SeparatorMenuItem::new());
        for (l, t) in [("_Ascending", gtk::SortType::Ascending), ("_Descending", gtk::SortType::Descending)] {
            let it = gtk::CheckMenuItem::with_mnemonic(l);
            it.set_draw_as_radio(true);
            it.set_active(order == t);
            let e = self.clone();
            it.connect_activate(move |i| if i.is_active() { e.set_sort(cur, t == gtk::SortType::Descending) });
            sort.append(&it);
        }
        sort
    }

    fn context_menu(self: &Rc<Self>, ev: Option<&gdk::EventButton>) {
        let m = gtk::Menu::new();
        let sel = self.selected();
        let uris = self.selected_uris();
        let loc = self.loc.borrow().clone();
        let win = self.win.clone();
        let sub = |m: &gtk::Menu, l: &str, s: &gtk::Menu| { let i = gtk::MenuItem::with_mnemonic(l); i.set_submenu(Some(s)); m.append(&i); i };
        if uris.is_empty() {
            sub(&m, "_View", &self.view_menu());
            sub(&m, "S_ort by", &self.sort_menu());
            let group = gtk::Menu::new();
            let none = gtk::CheckMenuItem::with_mnemonic("(_None)");
            none.set_draw_as_radio(true);
            none.set_active(true);
            group.append(&none);
            sub(&m, "Group _by", &group);
            { let e = self.clone(); Self::add_item(&m, "R_efresh", move || e.refresh()); }
            m.append(&gtk::SeparatorMenuItem::new());
            if loc == Loc::Trash {
                let e = self.clone();
                Self::add_item(&m, "Empty Recycle _Bin", move || { let e2 = e.clone(); ops::empty_trash(e.parent().as_ref(), move || e2.refresh()); }).set_sensitive(!trash::is_empty());
                let e = self.clone();
                Self::add_item(&m, "Restore all items", move || e.restore_selected());
            } else {
                let writable = self.cur_dir().is_some();
                let has = writable && ops::clipboard_has_files();
                { let e = self.clone(); Self::add_item(&m, "_Paste", move || e.paste(false)).set_sensitive(has); }
                { let e = self.clone(); Self::add_item(&m, "Paste _shortcut", move || e.paste(true)).set_sensitive(has); }
                if let Some(l) = ops::undo_label() {
                    let e = self.clone();
                    let it = Self::add_item(&m, &format!("_{l}"), move || e.undo());
                    if let Some(lb) = it.child().and_then(|c| c.downcast::<gtk::AccelLabel>().ok()) { lb.set_accel(*gdk::keys::constants::z, gdk::ModifierType::CONTROL_MASK); }
                }
                if writable {
                    m.append(&gtk::SeparatorMenuItem::new());
                    let new = gtk::Menu::new();
                    { let e = self.clone(); let i = Self::add_item(&new, "_Folder", move || e.new_folder()); util::menu_icon(&i, "folder"); }
                    { let e = self.clone(); let i = Self::add_item(&new, "_Shortcut", move || e.new_shortcut()); util::menu_icon(&i, "emblem-symbolic-link"); }
                    new.append(&gtk::SeparatorMenuItem::new());
                    { let e = self.clone(); let i = Self::add_item(&new, "Text Document", move || e.new_text()); util::menu_icon(&i, "text-x-generic"); }
                    sub(&m, "Ne_w", &new);
                    m.append(&gtk::SeparatorMenuItem::new());
                    if let Some(d) = self.cur_dir() {
                        // Win7 shows this with Shift+right-click; we always offer it
                        let d2 = d.clone();
                        Self::add_item(&m, "Open command _window here", move || open_terminal(&d2));
                        let w = win.clone();
                        Self::add_item(&m, "P_roperties", move || props::show(&[d.clone()], Some(w.upcast_ref())));
                    }
                }
            }
        } else if loc == Loc::Trash {
            { let e = self.clone(); Self::add_item(&m, "R_estore", move || e.restore_selected()); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let e = self.clone(); Self::add_item(&m, "Cu_t", move || e.copy(true)); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let e = self.clone(); Self::add_item(&m, "_Delete", move || e.delete(true)); }
            m.append(&gtk::SeparatorMenuItem::new());
            { let e = self.clone(); Self::add_item(&m, "P_roperties", move || e.properties()); }
        } else if sel.is_empty() {
            // libraries / network items
            { let e = self.clone(); let it = Self::add_item(&m, "_Open", move || e.activate_selected()); bold(&it); }
            if let Some(n) = uris[0].strip_prefix("library:") {
                let (e, l) = (self.clone(), Loc::parse(&format!("library:{n}")));
                Self::add_item(&m, "Open in new _window", move || e.open_window(l.clone()));
            }
        } else {
            let one = sel.len() == 1;
            let first = sel[0].clone();
            let is_dir = one && first.is_dir();
            { let e = self.clone(); let it = Self::add_item(&m, "_Open", move || e.activate_selected()); bold(&it); }
            if is_dir {
                let (e, p) = (self.clone(), first.clone());
                Self::add_item(&m, "Open in new _window", move || e.open_window(Loc::Path(p.clone())));
            }
            if one && !is_dir {
                let ct = gio::content_type_guess(Some(&first), &[]).0.to_string();
                let apps = gio::AppInfo::all_for_type(&ct);
                if apps.len() > 1 { sub(&m, "Open wit_h", &open_with_menu(&first, &win)); }
                else {
                    let (p, w) = (first.clone(), win.clone());
                    Self::add_item(&m, "Open wit_h...", move || choose_program(&p, &w));
                }
                if first.extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false) {
                    let (e, p) = (self.clone(), first.clone());
                    Self::add_item(&m, "E_xtract All...", move || { let e2 = e.clone(); ops::extract(&p, move |t| if let Some(t) = t { e2.select_after.borrow_mut().push(t); }); });
                }
            }
            m.append(&gtk::SeparatorMenuItem::new());
            let send = gtk::Menu::new();
            {
                let (s, e) = (sel.clone(), self.clone());
                let i = Self::add_item(&send, "Compressed (zipped) folder", move || { let e2 = e.clone(); ops::zip(&s, move |z| if let Some(z) = z { *e2.rename_after.borrow_mut() = Some(z); }); });
                util::menu_icon(&i, "package-x-generic");
                let s = sel.clone();
                let i = Self::add_item(&send, "Desktop (create shortcut)", move || { ops::create_shortcuts(&s, &util::special_dir(glib::UserDirectory::Desktop, "Desktop")); });
                util::menu_icon(&i, "user-desktop");
                let (s, w) = (sel.clone(), win.clone());
                let i = Self::add_item(&send, "Documents", move || ops::transfer(s.clone(), library_dir("Documents"), ops::Kind::Copy, Some(w.upcast_ref()), |_| {}));
                util::menu_icon(&i, "folder-documents");
                let s = sel.clone();
                let i = Self::add_item(&send, "Mail recipient", move || {
                    let mut args = vec!["xdg-email".to_string()];
                    for p in s.iter().filter(|p| p.is_file()) { args.push("--attach".into()); args.push(p.to_string_lossy().into_owned()); }
                    let refs: Vec<&str> = args.iter().map(|a| a.as_str()).collect();
                    if !util::spawn(&refs) { util::open_uri("mailto:"); }
                });
                util::menu_icon(&i, "mail-send");
                // removable drives
                for mnt in self.vm.mounts() {
                    let Some(p) = mnt.root().path() else { continue };
                    if !(mnt.can_eject() || p.starts_with("/run/media") || p.starts_with("/media")) { continue; }
                    let (s, w) = (sel.clone(), win.clone());
                    let i = Self::add_item(&send, &mnt.name(), move || ops::transfer(s.clone(), p.clone(), ops::Kind::Copy, Some(w.upcast_ref()), |_| {}));
                    util::menu_icon(&i, "drive-removable-media");
                }
            }
            sub(&m, "Se_nd to", &send);
            m.append(&gtk::SeparatorMenuItem::new());
            { let e = self.clone(); Self::add_item(&m, "Cu_t", move || e.copy(true)); }
            { let e = self.clone(); Self::add_item(&m, "_Copy", move || e.copy(false)); }
            if is_dir && ops::clipboard_has_files() {
                let (e, d) = (self.clone(), first.clone());
                Self::add_item(&m, "_Paste", move || {
                    let (e2, d2) = (e.clone(), d.clone());
                    ops::get_clipboard(move |files, cut| { if !files.is_empty() { ops::transfer(files, d2.clone(), if cut { ops::Kind::Move } else { ops::Kind::Copy }, e2.parent().as_ref(), |_| {}); } });
                });
            }
            m.append(&gtk::SeparatorMenuItem::new());
            if let Some(d) = self.cur_dir() {
                let (s, e) = (sel.clone(), self.clone());
                Self::add_item(&m, "Create _shortcut", move || { for l in ops::create_shortcuts(&s, &d) { e.show_new_item(&l, false); } });
            }
            { let e = self.clone(); Self::add_item(&m, "_Delete", move || e.delete(false)); }
            { let e = self.clone(); Self::add_item(&m, "Rena_me", move || e.rename()).set_sensitive(one); }
            if one && first.is_symlink() {
                if let Ok(t) = std::fs::read_link(&first) {
                    let t = if t.is_absolute() { t } else { first.parent().map(|p| p.join(&t)).unwrap_or(t) };
                    let e = self.clone();
                    Self::add_item(&m, "Open _file location", move || { if let Some(par) = t.parent() { e.select_after.borrow_mut().push(t.clone()); e.go(Loc::Path(par.to_path_buf()), true); } });
                }
            }
            if matches!(loc, Loc::Search(..) | Loc::Recent) {
                let e = self.clone();
                let p = first.clone();
                Self::add_item(&m, "Open _folder location", move || { if let Some(par) = p.parent() { e.select_after.borrow_mut().push(p.clone()); e.go(Loc::Path(par.to_path_buf()), true); } });
            }
            m.append(&gtk::SeparatorMenuItem::new());
            { let e = self.clone(); Self::add_item(&m, "P_roperties", move || e.properties()); }
        }
        m.show_all();
        m.set_attach_widget(Some(&self.win));
        match ev {
            Some(ev) => m.popup_at_pointer(Some(ev)),
            None => {
                let w: gtk::Widget = match self.active_tree() { Some(t) => t.clone().upcast(), None => self.icons.area.clone().upcast() };
                m.popup_at_widget(&w, gdk::Gravity::Center, gdk::Gravity::NorthWest, None);
            }
        }
    }

    fn new_shortcut(self: &Rc<Self>) {
        let Some(d) = self.cur_dir() else { return };
        if let Some(t) = crate::desktop::prompt("Create Shortcut", "Type the location of the item:", "") {
            let t = PathBuf::from(util::shellexpand(t.trim()));
            if t.symlink_metadata().is_err() { util::error("Create Shortcut", &format!("The file {} cannot be found.", t.display())); return; }
            for l in ops::create_shortcuts(&[t], &d) { self.show_new_item(&l, true); }
        }
    }

    // ------------------------------------------------------------ type-to-select
    fn typeahead(self: &Rc<Self>, c: char) {
        let now = Instant::now();
        let prefix = {
            let mut t = self.typeahead.borrow_mut();
            if t.1.map(|l| now.duration_since(l) > Duration::from_millis(1000)).unwrap_or(true) { t.0.clear(); }
            t.0.extend(c.to_lowercase());
            t.1 = Some(now);
            t.0.clone()
        };
        let store = self.store();
        let n = store.iter_n_children(None);
        if n == 0 { return; }
        // repeating one letter cycles through the items starting with it
        let same = prefix.chars().all(|x| Some(x) == prefix.chars().next());
        let key = if same { prefix.chars().next().map(|x| x.to_string()).unwrap_or_default() } else { prefix.clone() };
        let cur = self.selected_tree_paths().first().and_then(|p| p.indices().first().copied()).unwrap_or(-1);
        let start = if same && prefix.chars().count() > 1 || (same && cur >= 0 && self.name_at(&store, cur).starts_with(&key)) { cur + 1 } else { cur.max(0) };
        for k in 0..n {
            let i = (start + k).rem_euclid(n);
            if self.name_at(&store, i).starts_with(&key) {
                let tp = gtk::TreePath::from_indicesv(&[i]);
                self.unselect_all();
                self.select_tree_path(&tp);
                self.set_cursor(&tp);
                return;
            }
        }
    }

    fn name_at(&self, store: &gtk::ListStore, i: i32) -> String {
        store.iter_nth_child(None, i).map(|it| store.value(&it, C_NAME as i32).get::<String>().unwrap_or_default().to_lowercase()).unwrap_or_default()
    }

    // ------------------------------------------------------------ search
    fn set_search_text(&self, t: &str) {
        self.search.set_text(t);
    }

    fn on_search_changed(self: &Rc<Self>) {
        let q = self.search.text().to_string();
        let sc = self.search.style_context();
        if q.is_empty() { sc.add_class("empty"); } else { sc.remove_class("empty"); }
        self.search.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some(if q.is_empty() { "system-search-symbolic" } else { "edit-clear-symbolic" }));
        if let Some(t) = self.search_timer.borrow_mut().take() { t.remove(); }
        let e = Rc::downgrade(self);
        // Windows searches as you type, after a short pause
        let id = glib::timeout_add_local_once(Duration::from_millis(350), move || {
            let Some(e) = e.upgrade() else { return };
            e.search_timer.borrow_mut().take();
            let q = e.search.text().trim().to_string();
            let loc = e.loc.borrow().clone();
            match (&loc, q.is_empty()) {
                (Loc::Search(b, _), true) => { let b = b.clone(); e.back.borrow_mut().pop(); e.go(Loc::Path(b), false); }
                (Loc::Search(b, old), false) => if *old != q { let b = b.clone(); e.replace_loc(Loc::Search(b, q)); },
                (_, false) => {
                    let base = e.cur_dir().or_else(|| if loc == Loc::Computer || loc == Loc::Libraries { Some(config::home()) } else { None });
                    if let Some(b) = base {
                        let text = e.search.text().to_string();
                        e.go(Loc::Search(b, q), true);
                        // go() clears the box for non-search places; keep what was typed
                        if e.search.text() != text { e.search.set_text(&text); }
                    }
                }
                _ => {}
            }
        });
        *self.search_timer.borrow_mut() = Some(id);
    }

    /// Change the place without a new history entry (refining a search).
    fn replace_loc(self: &Rc<Self>, loc: Loc) {
        *self.loc.borrow_mut() = loc.clone();
        self.win.set_title(&loc.title());
        self.build_crumbs();
        self.load();
    }

    // ------------------------------------------------------------ drag and drop
    fn drop_dest_at(&self, w: &gtk::Widget, x: i32, y: i32) -> (Option<PathBuf>, Option<gtk::TreePath>) {
        let store = self.store();
        let hit = if *w == self.icons.area.clone().upcast::<gtk::Widget>() { self.icons.path_at_pos(x, y) }
            else if let Some(tv) = w.downcast_ref::<gtk::TreeView>() { tv.dest_row_at_pos(x, y).and_then(|(p, _)| p) } else { None };
        if let Some(tp) = hit {
            if let Some(it) = store.iter(&tp) {
                let isdir: bool = store.value(&it, C_ISDIR as i32).get().unwrap_or(false);
                if let Some(p) = path_of_uri(&store.value(&it, C_URI as i32).get::<String>().unwrap_or_default()) {
                    if isdir { return (Some(p), Some(tp)); }
                }
                if let Some(n) = store.value(&it, C_URI as i32).get::<String>().unwrap_or_default().strip_prefix("library:") { return (Some(library_dir(n)), Some(tp)); }
            }
        }
        (self.cur_dir(), None)
    }

    fn wire_dnd(self: &Rc<Self>) {
        let targets = [gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0)];
        let actions = gdk::DragAction::COPY | gdk::DragAction::MOVE | gdk::DragAction::LINK;
        // drags start on items only (empty space rubber-bands); not bound by gtk-rs
        unsafe {
            use glib::translate::{IntoGlib, ToGlibPtr};
            let t = std::ffi::CString::new("text/uri-list").unwrap_or_default();
            let entry = gtk::ffi::GtkTargetEntry { target: t.as_ptr() as *mut _, flags: 0, info: 0 };
            let (mask, acts) = (gdk::ModifierType::BUTTON1_MASK.into_glib(), actions.into_glib());
            for tv in [&self.details, &self.content] { gtk::ffi::gtk_tree_view_enable_model_drag_source(tv.to_glib_none().0, mask, &entry, 1, acts); }
        }
        let widgets: Vec<gtk::Widget> = vec![self.icons.area.clone().upcast(), self.details.clone().upcast(), self.content.clone().upcast(), self.nav.view.clone().upcast()];

        for w in &widgets {
            w.drag_dest_set(gtk::DestDefaults::empty(), &targets, actions);
            let e = Rc::downgrade(self);
            w.connect_drag_data_get(move |_, _, data, _, _| {
                let Some(e) = e.upgrade() else { return };
                let uris: Vec<String> = e.selected().iter().map(|p| uri_of(p)).collect();
                let refs: Vec<&str> = uris.iter().map(|s| s.as_str()).collect();
                data.set_uris(&refs);
            });
            let e = Rc::downgrade(self);
            w.connect_drag_begin(move |_, _| { if let Some(e) = e.upgrade() { DRAGGING.with(|d| *d.borrow_mut() = e.selected()); } });
            w.connect_drag_end(|_, _| DRAGGING.with(|d| d.borrow_mut().clear()));
            // never let GTK delete rows from the model after a move
            w.connect_drag_data_delete(|w, _| w.stop_signal_emission_by_name("drag-data-delete"));
            let e = Rc::downgrade(self);
            w.connect_drag_motion(move |w, ctx, x, y, time| {
                let Some(e) = e.upgrade() else { return false };
                // GTK derives these from where it thinks the window is; a window manager that did not
                // send the ICCCM synthetic ConfigureNotify leaves that at (0, 0): ask the X server
                let (x, y) = pointer_in(w).unwrap_or((x, y));
                let (dest, hl) = if let Some(nv) = w.downcast_ref::<gtk::TreeView>().filter(|t| **t == e.nav.view) {
                    match e.nav.drop_target(x, y) {
                        Some((p, d)) => { nv.set_drag_dest_row(Some(&p), gtk::TreeViewDropPosition::IntoOrBefore); (Some(d), None) }
                        None => { nv.set_drag_dest_row(None, gtk::TreeViewDropPosition::Before); (None, None) }
                    }
                } else { e.drop_dest_at(w, x, y) };
                // highlight the folder under the pointer
                if *w == e.icons.area.clone().upcast::<gtk::Widget>() { e.icons.set_drop_highlight(hl.as_ref()); }
                else if let Some(tv) = w.downcast_ref::<gtk::TreeView>().filter(|t| **t != e.nav.view) { tv.set_drag_dest_row(hl.as_ref(), gtk::TreeViewDropPosition::IntoOrBefore); }
                let dragging = DRAGGING.with(|d| d.borrow().clone());
                let act = match &dest {
                    None => gdk::DragAction::empty(),
                    // a folder onto itself or the folder it is already in
                    Some(d) if dragging.iter().any(|s| d.starts_with(s)) => gdk::DragAction::empty(),
                    Some(d) if hl.is_none() && !dragging.is_empty() && dragging.iter().all(|s| s.parent() == Some(d.as_path())) => gdk::DragAction::empty(),
                    Some(d) => drop_action(w, ctx, d, &dragging),
                };
                *e.drop_target.borrow_mut() = dest.map(|d| (d, act));
                ctx.drag_status(act, time);
                true
            });
            let e = Rc::downgrade(self);
            w.connect_drag_leave(move |w, _, _| {
                let Some(e) = e.upgrade() else { return };
                if *w == e.icons.area.clone().upcast::<gtk::Widget>() { e.icons.set_drop_highlight(None); }
                else if let Some(tv) = w.downcast_ref::<gtk::TreeView>() { tv.set_drag_dest_row(None, gtk::TreeViewDropPosition::Before); }
                let _ = e;
            });
            w.connect_drag_drop(move |w, ctx, _, _, time| {
                w.drag_get_data(ctx, &gdk::Atom::intern("text/uri-list"), time);
                true
            });
            let e = Rc::downgrade(self);
            w.connect_drag_data_received(move |w, ctx, _, _, data, _, time| {
                w.stop_signal_emission_by_name("drag-data-received");
                let Some(e) = e.upgrade() else { return };
                let files: Vec<PathBuf> = data.uris().iter().filter_map(|u| gio::File::for_uri(u).path()).collect();
                let target = e.drop_target.borrow_mut().take();
                let ok = match target {
                    Some((dest, act)) if !files.is_empty() && !act.is_empty() => { e.drop_files(files, dest, act); true }
                    _ => false,
                };
                ctx.drag_finish(ok, false, time);
                if *w == e.icons.area.clone().upcast::<gtk::Widget>() { e.icons.set_drop_highlight(None); }
            });
        }
    }

    fn drop_files(self: &Rc<Self>, files: Vec<PathBuf>, dest: PathBuf, act: gdk::DragAction) {
        if dest == Path::new("trash:") { let e = self.clone(); ops::delete(files, false, self.parent().as_ref(), false, move || e.schedule_info()); return; }
        if act.contains(gdk::DragAction::LINK) {
            let made = ops::create_shortcuts(&files, &dest);
            for m in made { self.show_new_item(&m, false); }
            return;
        }
        let kind = if act.contains(gdk::DragAction::MOVE) { ops::Kind::Move } else { ops::Kind::Copy };
        let e = self.clone();
        let here = self.cur_dir() == Some(dest.clone());
        ops::transfer(files, dest, kind, self.parent().as_ref(), move |made| { if here { e.select_after.borrow_mut().extend(made); e.after_batch(); } });
    }

    // ------------------------------------------------------------ wiring
    fn wire(self: &Rc<Self>, organize: &gtk::MenuButton, views_arrow: &gtk::Button, prev_b: &gtk::ToggleButton, refresh: &gtk::Button, crumb_ev: &gtk::EventBox, recent_b: &gtk::Button) {
        { let e = self.clone(); self.back_b.connect_clicked(move |_| e.go_back()); }
        { let e = self.clone(); self.fwd_b.connect_clicked(move |_| e.go_fwd()); }
        {
            // Recent Pages: the history around the current page
            let e = self.clone();
            recent_b.connect_clicked(move |b| {
                let m = gtk::Menu::new();
                let fwd: Vec<Loc> = e.fwd.borrow().iter().rev().cloned().collect();
                let back: Vec<Loc> = e.back.borrow().iter().rev().cloned().collect();
                let cur = e.loc.borrow().clone();
                for (i, l) in fwd.iter().enumerate() {
                    let it = menu_item_icon(&l.title(), l.icon());
                    let (e2, n) = (e.clone(), fwd.len() - i);
                    it.connect_activate(move |_| for _ in 0..n { e2.go_fwd() });
                    m.append(&it);
                }
                let c = gtk::CheckMenuItem::with_label(&cur.title());
                c.set_active(true);
                c.set_draw_as_radio(true);
                m.append(&c);
                for (i, l) in back.iter().enumerate().take(15) {
                    let it = menu_item_icon(&l.title(), l.icon());
                    let e2 = e.clone();
                    it.connect_activate(move |_| for _ in 0..=i { e2.go_back() });
                    m.append(&it);
                }
                m.show_all();
                m.popup_at_widget(b, gdk::Gravity::SouthWest, gdk::Gravity::NorthWest, None);
            });
        }
        { let e = self.clone(); refresh.connect_clicked(move |_| e.refresh()); }
        // clicking empty address space switches to an editable path, like Win7
        {
            let e = self.clone();
            crumb_ev.connect_button_press_event(move |_, ev| { if ev.button() == 1 { e.edit_address(); } glib::Propagation::Stop });
        }
        { let e = self.clone(); self.addr_entry.connect_activate(move |en| { let t = en.text().to_string(); e.address_go(&t); }); }
        { let e = self.clone(); self.addr_entry.connect_changed(move |_| e.update_completion()); }
        { let e = self.clone(); self.addr_entry.connect_focus_out_event(move |_, _| { e.addr_stack.set_visible_child_name("crumbs"); glib::Propagation::Proceed }); }
        { let e = self.clone(); self.addr.connect_size_allocate(move |_, a| {
            // re-fit the crumbs when the window is resized a lot
            let w = a.width();
            let last = unsafe { e.addr.data::<i32>("w").map(|x| *x.as_ref()).unwrap_or(0) };
            if (w - last).abs() > 80 { unsafe { e.addr.set_data("w", w); } let e2 = e.clone(); glib::idle_add_local_once(move || e2.build_crumbs()); }
        }); }
        // search box: instant results in this folder and subfolders
        { let e = self.clone(); self.search.connect_changed(move |_| e.on_search_changed()); }
        {
            let e = self.clone();
            self.search.connect_icon_press(move |s, pos, _| if pos == gtk::EntryIconPosition::Secondary && !s.text().is_empty() { s.set_text(""); let _ = &e; });
        }
        {
            let e = self.clone();
            self.search.connect_activate(move |s| {
                // Enter: search now
                if let Some(t) = e.search_timer.borrow_mut().take() { t.remove(); }
                let q = s.text().trim().to_string();
                if q.is_empty() { return; }
                let loc = e.loc.borrow().clone();
                match loc {
                    Loc::Search(b, _) => e.replace_loc(Loc::Search(b, q)),
                    l => if let Some(b) = l.dir() { let t = s.text().to_string(); e.go(Loc::Search(b, q), true); if e.search.text() != t { e.search.set_text(&t); } },
                }
            });
        }
        // organize menu
        let m = gtk::Menu::new();
        { let e = self.clone(); Self::add_item(&m, "Cu_t", move || e.copy(true)); }
        { let e = self.clone(); Self::add_item(&m, "_Copy", move || e.copy(false)); }
        { let e = self.clone(); Self::add_item(&m, "_Paste", move || e.paste(false)); }
        { let e = self.clone(); Self::add_item(&m, "_Undo", move || e.undo()); }
        m.append(&gtk::SeparatorMenuItem::new());
        { let e = self.clone(); Self::add_item(&m, "Select _all", move || e.select_all()); }
        m.append(&gtk::SeparatorMenuItem::new());
        let layout = gtk::Menu::new();
        let mb = gtk::CheckMenuItem::with_label("Menu bar");
        mb.set_sensitive(false);
        layout.append(&mb);
        let dp = gtk::CheckMenuItem::with_label("Details pane");
        dp.set_active(self.details_pane.is_visible() || with_views(|v| v.details_pane.unwrap_or(true)));
        { let w = self.details_pane.clone(); dp.connect_toggled(move |c| { w.set_visible(c.is_active()); with_views(|v| v.details_pane = Some(c.is_active())); save_views(); }); }
        layout.append(&dp);
        let pp = gtk::CheckMenuItem::with_label("Preview pane");
        pp.set_active(prev_b.is_active());
        { let pb = prev_b.clone(); pp.connect_toggled(move |c| if pb.is_active() != c.is_active() { pb.set_active(c.is_active()) }); }
        { let pp2 = pp.clone(); prev_b.connect_toggled(move |b| if pp2.is_active() != b.is_active() { pp2.set_active(b.is_active()) }); }
        layout.append(&pp);
        let np = gtk::CheckMenuItem::with_label("Navigation pane");
        np.set_active(with_views(|v| v.nav_pane.unwrap_or(true)));
        { let n = self.nav_sw.clone(); np.connect_toggled(move |c| { n.set_visible(c.is_active()); with_views(|v| v.nav_pane = Some(c.is_active())); save_views(); }); }
        layout.append(&np);
        let li = gtk::MenuItem::with_mnemonic("_Layout"); li.set_submenu(Some(&layout)); m.append(&li);
        {
            let e = self.clone();
            let it = gtk::CheckMenuItem::with_label("Show hidden files and folders");
            it.set_active(self.opts.borrow().show_hidden);
            it.connect_toggled(move |c| {
                let on = c.is_active();
                if e.opts.borrow().show_hidden == on { return; }
                e.opts.borrow_mut().show_hidden = on;
                e.nav.show_hidden.set(on);
                config::update(|cf| cf.folder.show_hidden = on);
                e.reload_keep_selection();
            });
            m.append(&it);
            let e2 = self.clone();
            m.connect_show(move |_| it.set_active(e2.opts.borrow().show_hidden));
        }
        Self::add_item(&m, "Folder and _search options", || { util::spawn(&["aerode-control", "folderoptions"]); });
        m.append(&gtk::SeparatorMenuItem::new());
        { let e = self.clone(); Self::add_item(&m, "_Delete", move || e.delete(false)); }
        { let e = self.clone(); Self::add_item(&m, "Rena_me", move || e.rename()); }
        { let e = self.clone(); Self::add_item(&m, "P_roperties", move || e.properties()); }
        m.append(&gtk::SeparatorMenuItem::new());
        { let w = self.win.clone(); Self::add_item(&m, "_Close", move || w.close()); }
        m.show_all();
        organize.set_popup(Some(&m));
        // views: the button cycles List → Details → Tiles → Content → Large Icons; the arrow opens the slider
        {
            let e = self.clone();
            self.views_b.connect_clicked(move |_| {
                let next = match e.view.get() { View::List => View::Details, View::Details => View::Tiles, View::Tiles => View::Content, View::Content => View::Large, _ => View::List };
                e.set_view(next);
            });
        }
        { let e = self.clone(); views_arrow.connect_clicked(move |b| e.views_popover(b)); }
        {
            let e = self.clone();
            prev_b.connect_toggled(move |b| {
                e.preview_box.set_visible(b.is_active());
                with_views(|v| v.preview_pane = Some(b.is_active()));
                save_views();
                e.update_info();
            });
        }
        // nav tree
        {
            let e = Rc::downgrade(self);
            self.nav.connect_navigate(move |t| {
                let Some(e) = e.upgrade() else { return };
                if t == "control:" { util::spawn(&["aerode-control"]); return; }
                let loc = Loc::parse(&t);
                if *e.loc.borrow() != loc { e.go(loc, true); }
            });
        }
        // activation & selection in all views
        { let e = Rc::downgrade(self); self.icons.connect_item_activated(move |_| { if let Some(e) = e.upgrade() { e.activate_selected(); } }); }
        { let e = self.clone(); self.details.connect_row_activated(move |_, _, _| e.activate_selected()); }
        { let e = self.clone(); self.content.connect_row_activated(move |_, _, _| e.activate_selected()); }
        { let e = Rc::downgrade(self); self.icons.connect_selection_changed(move || { if let Some(e) = e.upgrade() { e.schedule_info(); } }); }
        {
            // Win7 infotips (Folder Options: "Show pop-up description for folder and desktop items")
            let e = Rc::downgrade(self);
            self.icons.set_tooltip_fn(move |i| {
                let e = e.upgrade()?;
                if !e.opts.borrow().popup_desc { return None; }
                let store = e.store();
                let it = store.iter_nth_child(None, i)?;
                let p = path_of_uri(&store.value(&it, C_URI as i32).get::<String>().ok()?)?;
                let rows = e.rows.borrow();
                let en = &rows.get(&p)?.e;
                Some(if en.is_dir { format!("Date modified: {}", en.text.date) } else {
                    format!("Type: {}\nSize: {}\nDate modified: {}", en.desc, util::human_size(en.size), en.text.date)
                })
            });
        }
        { let e = self.clone(); self.details.selection().connect_changed(move |_| e.schedule_info()); }
        { let e = self.clone(); self.content.selection().connect_changed(move |_| e.schedule_info()); }
        // in-place rename
        for r in [&self.det_text, &self.con_text] {
            let e = Rc::downgrade(self);
            r.connect_editing_started(move |_, ed, _| {
                let Some(e) = e.upgrade() else { return };
                let Ok(en) = ed.clone().downcast::<gtk::Entry>() else { return };
                let Some((p, edit)) = e.renaming.borrow().clone() else { return };
                en.set_text(&edit);
                // the name without the extension is selected, like Windows
                let stem = rename_stem(&(p, edit));
                let en2 = en.clone();
                en.select_region(0, stem);
                glib::idle_add_local_once(move || en2.select_region(0, stem));
            });
            let e = Rc::downgrade(self);
            r.connect_edited(move |_, _, text| { if let Some(e) = e.upgrade() { e.finish_rename(Some(text.to_string())); } });
            let e = Rc::downgrade(self);
            r.connect_editing_canceled(move |_| { if let Some(e) = e.upgrade() { e.finish_rename(None); } });
        }
        let views: Vec<gtk::Widget> = vec![self.icons.area.clone().upcast(), self.details.clone().upcast(), self.content.clone().upcast()];
        for w in &views {
            let e = self.clone();
            w.connect_button_press_event(move |w, ev| {
                if ev.button() == 3 && ev.event_type() == gdk::EventType::ButtonPress {
                    // select what's under the pointer first
                    let (x, y) = ev.position();
                    if *w == e.icons.area.clone().upcast::<gtk::Widget>() {
                        let iv = &e.icons;
                        iv.area.grab_focus();
                        match iv.path_at_pos(x as i32, y as i32) { Some(p) => if !iv.path_is_selected(&p) { iv.unselect_all(); iv.select_path(&p); iv.set_cursor(&p); }, None => iv.unselect_all() }
                    } else if let Some(tv) = w.downcast_ref::<gtk::TreeView>() {
                        match tv.path_at_pos(x as i32, y as i32) { Some((Some(p), ..)) => if !tv.selection().path_is_selected(&p) { tv.selection().unselect_all(); tv.selection().select_path(&p); }, _ => tv.selection().unselect_all() }
                    }
                    e.update_info();
                    e.context_menu(Some(ev));
                    return glib::Propagation::Stop;
                }
                if ev.button() == 8 { e.go_back(); return glib::Propagation::Stop; }
                if ev.button() == 9 { e.go_fwd(); return glib::Propagation::Stop; }
                // a click on empty space in the list clears the selection (TreeView keeps it otherwise)
                if ev.button() == 1 && ev.event_type() == gdk::EventType::ButtonPress {
                    if let Some(tv) = w.downcast_ref::<gtk::TreeView>() {
                        let (x, y) = ev.position();
                        let mods = ev.state().intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK);
                        match tv.path_at_pos(x as i32, y as i32) {
                            None | Some((None, ..)) => if !mods { tv.selection().unselect_all(); },
                            // pressing on a name selects it at once, so it can be dragged away in
                            // the same motion (GtkTreeView would start a rubber band on an
                            // unselected row; Win7 does that only from the other columns)
                            Some((Some(p), col, ..)) => {
                                let on_name = *tv == e.content || col.as_ref() == Some(&e.det_name);
                                if on_name && !mods && !tv.selection().path_is_selected(&p) {
                                    tv.selection().unselect_all();
                                    tv.selection().select_path(&p);
                                    tv.set_cursor(&p, None::<&gtk::TreeViewColumn>, false);
                                }
                            }
                        }
                    }
                }
                glib::Propagation::Proceed
            });
            // Ctrl+wheel changes the view, like Windows
            let e = self.clone();
            w.add_events(gdk::EventMask::SCROLL_MASK | gdk::EventMask::SMOOTH_SCROLL_MASK);
            w.connect_scroll_event(move |_, ev| {
                if !ev.state().contains(gdk::ModifierType::CONTROL_MASK) { return glib::Propagation::Proceed; }
                let d = match ev.direction() { gdk::ScrollDirection::Up => -1, gdk::ScrollDirection::Down => 1, gdk::ScrollDirection::Smooth => { let (_, dy) = ev.delta(); if dy < 0.0 { -1 } else if dy > 0.0 { 1 } else { 0 } } _ => 0 };
                if d != 0 { e.step_view(d); }
                glib::Propagation::Stop
            });
        }
        for c in self.details.columns() {
            let e = Rc::downgrade(self);
            c.connect_clicked(move |c| {
                let Some(e) = e.upgrade() else { return };
                let Some(sc) = (unsafe { c.data::<u32>("sort").map(|x| *x.as_ref()) }) else { return };
                let (cur, desc) = e.sort_state.get();
                e.set_sort(sc, if cur == sc { !desc } else { false });
            });
        }
        self.wire_dnd();
        // thumbnails follow what is visible
        {
            let e = Rc::downgrade(self);
            self.icons.area.connect_size_allocate(move |_, _| { if let Some(e) = e.upgrade() { e.schedule_thumbs(); } });
            let e = Rc::downgrade(self);
            self.icons.connect_scrolled(move || { if let Some(e) = e.upgrade() { e.schedule_thumbs(); } });
        }
        for w in [self.details.clone().upcast::<gtk::Widget>(), self.content.clone().upcast()] {
            if let Some(sw) = w.parent().and_then(|p| p.downcast::<gtk::ScrolledWindow>().ok()) {
                let e = Rc::downgrade(self);
                sw.vadjustment().connect_value_changed(move |_| { if let Some(e) = e.upgrade() { e.schedule_thumbs(); } });
                let e = Rc::downgrade(self);
                sw.connect_size_allocate(move |_, _| { if let Some(e) = e.upgrade() { e.schedule_thumbs(); } });
            }
        }
        // keyboard
        {
            let e = self.clone();
            self.win.connect_key_press_event(move |w, ev| {
                use gdk::keys::constants as K;
                let k = ev.keyval();
                let st = ev.state() & gtk::accelerator_get_default_mod_mask();
                let ctrl = st.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = st.contains(gdk::ModifierType::SHIFT_MASK);
                let alt = st.contains(gdk::ModifierType::MOD1_MASK);
                let focus = w.focused_widget();
                let in_entry = focus.as_ref().map(|f| f.is::<gtk::Editable>()).unwrap_or(false);
                let lower = k.to_lower();
                // type-to-select starts over after any command key
                if !ev.is_modifier() && (ctrl || alt || k.to_unicode().map(|c| c.is_control()).unwrap_or(true)) { e.typeahead.borrow_mut().0.clear(); }
                if in_entry {
                    // typing in the address bar, search box or a rename box: only window keys
                    let in_addr = focus.as_ref() == Some(e.addr_entry.upcast_ref());
                    let in_search = focus.as_ref() == Some(e.search.upcast_ref());
                    match k {
                        k if k == K::Escape && in_addr => { e.addr_stack.set_visible_child_name("crumbs"); e.focus_view(); return glib::Propagation::Stop; }
                        k if k == K::Escape && in_search => { e.search.set_text(""); e.focus_view(); return glib::Propagation::Stop; }
                        k if k == K::Down && in_search => { e.focus_view(); return glib::Propagation::Stop; }
                        k if (k == K::Left || k == K::Right) && alt => {}
                        k if k == K::F4 || k == K::F3 || (lower == K::d && alt) || (ctrl && (lower == K::l || lower == K::e || lower == K::f)) => {}
                        _ => return glib::Propagation::Proceed,
                    }
                }
                match k {
                    k if k == K::BackSpace && !alt => e.go_back(),
                    k if k == K::Left && alt => e.go_back(),
                    k if k == K::Right && alt => e.go_fwd(),
                    k if k == K::Up && alt => e.go_up(),
                    k if k == K::F5 || (ctrl && lower == K::r) => e.refresh(),
                    k if k == K::F2 => e.rename(),
                    k if k == K::F11 => { if e.win.window().map(|g| g.state().contains(gdk::WindowState::FULLSCREEN)).unwrap_or(false) { e.win.unfullscreen() } else { e.win.fullscreen() } }
                    k if (k == K::Delete || k == K::KP_Delete) => e.delete(shift),
                    k if k == K::Return && alt => e.properties(),
                    k if (k == K::Menu) || (k == K::F10 && shift) => e.context_menu(None),
                    _ if lower == K::p && alt => e.prev_b.set_active(!e.prev_b.is_active()),
                    _ if lower == K::d && alt => e.edit_address(),
                    _ if lower == K::c && ctrl => e.copy(false),
                    _ if lower == K::x && ctrl => e.copy(true),
                    _ if lower == K::v && ctrl => e.paste(false),
                    _ if lower == K::z && ctrl => e.undo(),
                    _ if lower == K::a && ctrl => e.select_all(),
                    _ if lower == K::n && ctrl && shift => e.new_folder(),
                    _ if lower == K::n && ctrl => { let l = e.loc.borrow().clone(); e.open_window(l); }
                    k if ((lower == K::e || lower == K::f) && ctrl) || k == K::F3 => { e.search.grab_focus(); }
                    k if (lower == K::l && ctrl) || k == K::F4 => e.edit_address(),
                    _ if lower == K::w && ctrl => e.win.close(),
                    k if k == K::Escape => { if !e.search.text().is_empty() { e.search.set_text(""); } }
                    // Ctrl+Shift+1…8: Extra large icons … Content (Win7)
                    k if ctrl && shift && view_key(k).is_some() => { if let Some(v) = view_key(k) { e.set_view(v); } }
                    k if (k == K::equal || k == K::plus || k == K::KP_Add) && ctrl => e.step_view(-1),
                    k if (k == K::minus || k == K::KP_Subtract) && ctrl => e.step_view(1),
                    _ => {
                        // type-to-select (or type into the search box, Folder Options)
                        if !ctrl && !alt {
                            if let Some(c) = k.to_unicode().filter(|c| !c.is_control() && (*c != ' ' || !e.typeahead.borrow().0.is_empty())) {
                                if e.opts.borrow().type_to_search {
                                    e.search.grab_focus();
                                    let pos = e.search.text().chars().count() as i32;
                                    let mut p = pos;
                                    e.search.insert_text(&c.to_string(), &mut p);
                                    e.search.set_position(p);
                                } else { e.typeahead(c); }
                                return glib::Propagation::Stop;
                            }
                        }
                        return glib::Propagation::Proceed;
                    }
                }
                glib::Propagation::Stop
            });
        }
    }

    fn focus_view(&self) {
        match self.active_tree() { Some(t) => t.grab_focus(), None => self.icons.area.grab_focus() }
    }

    /// The Views drop-down: a vertical slider next to the view names (Win7).
    fn views_popover(self: &Rc<Self>, b: &gtk::Button) {
        let pop = gtk::Popover::new(Some(b));
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        h.set_border_width(6);
        let scale = gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, 7.0, 1.0);
        scale.set_draw_value(false);
        scale.set_inverted(false);
        scale.set_round_digits(0);
        for i in 0..8 { scale.add_mark(i as f64, gtk::PositionType::Left, None); }
        scale.set_value(self.view.get().index() as f64);
        scale.set_size_request(-1, 8 * 26);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.set_homogeneous(true);
        for v in View::ALL {
            let it = gtk::ModelButton::new();
            it.set_property("text", v.label());
            it.set_property("role", gtk::ButtonRole::Radio);
            it.set_property("active", v == self.view.get());
            let e = self.clone();
            let p2 = pop.clone();
            it.connect_clicked(move |_| { p2.popdown(); e.set_view(v); });
            list.pack_start(&it, true, true, 0);
        }
        {
            let e = self.clone();
            scale.connect_value_changed(move |s| {
                let i = s.value().round() as usize;
                if let Some(v) = View::ALL.get(i) { if *v != e.view.get() { e.set_view(*v); } }
            });
        }
        h.pack_start(&scale, false, false, 0);
        h.pack_start(&list, true, true, 0);
        pop.add(&h);
        h.show_all();
        pop.popup();
    }
}

// ---------------------------------------------------------------- helpers

/// Draw the border of the selection and of the row under the pointer as one rounded box per row,
/// the way Win7's list view does (the CSS gives the cells only the gradient).
fn row_frames(tv: &gtk::TreeView) {
    let hot: Rc<RefCell<Option<gtk::TreePath>>> = Rc::new(RefCell::new(None));
    {
        let hot = hot.clone();
        tv.connect_motion_notify_event(move |tv, ev| {
            // motion over the rows comes from the bin window, already in bin coordinates (converting
            // them again subtracted the header's height: the frame sat one row above the pointer);
            // over the column headers there is no hot row
            let (x, y) = ev.position();
            let p = if ev.window().is_some() && ev.window() == tv.bin_window() {
                tv.path_at_pos(x as i32, y as i32).and_then(|(p, ..)| p)
            } else { None };
            if *hot.borrow() != p { *hot.borrow_mut() = p; tv.queue_draw(); }
            glib::Propagation::Proceed
        });
    }
    {
        let hot = hot.clone();
        tv.connect_leave_notify_event(move |tv, _| { if hot.borrow_mut().take().is_some() { tv.queue_draw(); } glib::Propagation::Proceed });
    }
    tv.connect_local("draw", true, move |args| {
        let tv = args.first().and_then(|v| v.get::<gtk::TreeView>().ok());
        let cr = args.get(1).and_then(|v| v.get::<gtk::cairo::Context>().ok());
        if let (Some(tv), Some(cr)) = (tv, cr) { draw_row_frames(&tv, &cr, hot.borrow().as_ref()); }
        // "draw" wants a gboolean: let the other handlers run
        Some(false.to_value())
    });
}

fn draw_row_frames(tv: &gtk::TreeView, cr: &gtk::cairo::Context, hot: Option<&gtk::TreePath>) -> Option<()> {
    {
        let (a, b) = tv.visible_range()?;
        let (a, b) = (a.indices().first().copied()?, b.indices().first().copied()?);
        let width: i32 = tv.columns().iter().filter(|c| c.is_visible()).map(|c| c.width()).sum();
        let focused = tv.has_focus() && tv.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()).map(|w| w.is_active()).unwrap_or(true);
        let sel = tv.selection();
        let (_, header) = tv.convert_bin_window_to_widget_coords(0, 0);
        cr.save().ok();
        cr.rectangle(0.0, header as f64, tv.allocated_width() as f64, (tv.allocated_height() - header) as f64);
        cr.clip();
        for i in a..=b {
            let p = gtk::TreePath::from_indicesv(&[i]);
            let selected = sel.path_is_selected(&p);
            let is_hot = hot == Some(&p);
            if !selected && !is_hot { continue; }
            let r = tv.background_area(Some(&p), None::<&gtk::TreeViewColumn>);
            let (x, y) = tv.convert_bin_window_to_widget_coords(-tv.visible_rect().x(), r.y());
            let (w, h) = (width as f64 - 1.0, r.height() as f64 - 1.0);
            let c = if selected && focused { (0.49, 0.64, 0.81) } else if selected { (0.85, 0.85, 0.85) } else { (0.72, 0.84, 0.98) };
            let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
            let rad = 2.5;
            cr.new_sub_path();
            cr.arc(x + w - rad, y + rad, rad, -std::f64::consts::FRAC_PI_2, 0.0);
            cr.arc(x + w - rad, y + h - rad, rad, 0.0, std::f64::consts::FRAC_PI_2);
            cr.arc(x + rad, y + h - rad, rad, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
            cr.arc(x + rad, y + rad, rad, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
            cr.close_path();
            cr.set_source_rgb(c.0, c.1, c.2);
            cr.set_line_width(1.0);
            cr.stroke().ok();
        }
        cr.restore().ok();
    }
    Some(())
}

/// The pointer in `w`'s coordinates, straight from the X server.
fn pointer_in(w: &gtk::Widget) -> Option<(i32, i32)> {
    let gw = w.window()?;
    let dev = gw.display().default_seat()?.pointer()?;
    let (_, px, py, _) = gw.device_position(&dev);
    // widgets without their own GdkWindow share the parent's: make it relative to the allocation
    if w.has_window() { Some((px, py)) } else { let a = w.allocation(); Some((px - a.x(), py - a.y())) }
}

/// How much of a name F2 selects: the name without its extension (folders: all of it).
fn rename_stem(r: &(PathBuf, String)) -> i32 {
    let (p, edit) = r;
    let n = if p.is_dir() || edit.starts_with('.') && edit.matches('.').count() == 1 { edit.chars().count() } else { trash::split_ext(edit).0.chars().count() };
    n as i32
}

/// The view of a Ctrl+Shift+digit key (with Shift held the digit keys give !, @, # … on US layouts).
fn view_key(k: gdk::keys::Key) -> Option<View> {
    use gdk::keys::constants as K;
    let groups: [&[gdk::keys::Key]; 8] = [
        &[K::_1, K::exclam, K::KP_1, K::KP_End], &[K::_2, K::at, K::quotedbl, K::KP_2, K::KP_Down], &[K::_3, K::numbersign, K::sterling, K::KP_3, K::KP_Next],
        &[K::_4, K::dollar, K::KP_4, K::KP_Left], &[K::_5, K::percent, K::KP_5, K::KP_Begin], &[K::_6, K::asciicircum, K::KP_6, K::KP_Right],
        &[K::_7, K::ampersand, K::KP_7, K::KP_Home], &[K::_8, K::asterisk, K::KP_8, K::KP_Up],
    ];
    groups.iter().position(|g| g.contains(&k)).map(|i| View::ALL[i])
}

fn has_subdirs(d: &Path) -> bool {
    std::fs::read_dir(d).map(|r| r.flatten().take(5000).any(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))).unwrap_or(false)
}

/// The default command of a context menu is bold.
fn bold(it: &gtk::MenuItem) {
    if let Some(l) = it.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
        let raw = l.label().to_string();
        l.set_markup_with_mnemonic(&format!("<b>{}</b>", glib::markup_escape_text(&raw)));
    }
}

fn scale_fit(p: &Pixbuf, max: i32) -> Pixbuf {
    let (w, h) = (p.width(), p.height());
    if w <= max && h <= max { return p.clone(); }
    let f = (max as f64 / w as f64).min(max as f64 / h as f64);
    p.scale_simple(((w as f64 * f) as i32).max(1), ((h as f64 * f) as i32).max(1), gtk::gdk_pixbuf::InterpType::Bilinear).unwrap_or_else(|| p.clone())
}

fn menu_item_icon(label: &str, icon: &str) -> gtk::MenuItem {
    let it = gtk::MenuItem::new();
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    h.pack_start(&util::image(icon, 16), false, false, 0);
    h.pack_start(&gtk::Label::new(Some(label)), false, false, 0);
    it.add(&h);
    it
}

fn recent_folders() -> Vec<PathBuf> {
    let Some(rm) = gtk::RecentManager::default() else { return vec![] };
    let mut items: Vec<(i64, PathBuf)> = rm.items().iter().filter_map(|i| {
        let p = gio::File::for_uri(&i.uri()?).path()?;
        let d = if i.mime_type().map(|m| m == "inode/directory").unwrap_or(false) { p } else { p.parent()?.to_path_buf() };
        Some((i.modified() as i64, d))
    }).collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = HashSet::new();
    items.into_iter().map(|(_, d)| d).filter(|d| d.is_dir() && seen.insert(d.clone())).take(40).collect()
}

/// Which action a drop does (Win7): Ctrl copies, Shift moves, Alt (or Ctrl+Shift) makes shortcuts;
/// without keys: move on the same drive, copy to another drive.
pub(crate) fn drop_action(w: &gtk::Widget, ctx: &gdk::DragContext, dest: &Path, sources: &[PathBuf]) -> gdk::DragAction {
    use std::os::unix::fs::MetadataExt;
    let dev = ctx.device();
    let mods = w.window().map(|gw| gw.device_position(&dev).3).unwrap_or_else(gdk::ModifierType::empty);
    let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = mods.contains(gdk::ModifierType::SHIFT_MASK);
    let alt = mods.contains(gdk::ModifierType::MOD1_MASK);
    let offered = ctx.actions();
    let want = if alt || (ctrl && shift) { gdk::DragAction::LINK } else if ctrl { gdk::DragAction::COPY } else if shift { gdk::DragAction::MOVE } else {
        let dd = std::fs::metadata(dest).map(|m| m.dev()).ok();
        let sd = sources.first().and_then(|s| std::fs::symlink_metadata(s).ok()).map(|m| m.dev());
        match (sd, dd) {
            (Some(a), Some(b)) if a == b => gdk::DragAction::MOVE,
            (Some(_), _) => gdk::DragAction::COPY,
            // a drag from another program: what it suggests
            _ => ctx.suggested_action(),
        }
    };
    if offered.contains(want) { want } else { ctx.suggested_action() }
}

/// "Open with ▸": the programs for this type, then "Choose default program...".
fn open_with_menu(p: &Path, w: &gtk::ApplicationWindow) -> gtk::Menu {
    let ow = gtk::Menu::new();
    let ct = gio::content_type_guess(Some(p), &[]).0.to_string();
    let def = gio::AppInfo::default_for_type(&ct, false);
    let mut apps = gio::AppInfo::all_for_type(&ct);
    // the default first
    if let Some(d) = &def { apps.sort_by_key(|a| !a.equal(d)); }
    for a in apps.into_iter().take(12) {
        let it = gtk::MenuItem::new();
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        h.pack_start(&util::app_image(&a, 16), false, false, 0);
        h.pack_start(&gtk::Label::new(Some(&a.display_name())), false, false, 0);
        it.add(&h);
        let p = p.to_path_buf();
        it.connect_activate(move |_| { launch_with(&a, &p); });
        ow.append(&it);
    }
    ow.append(&gtk::SeparatorMenuItem::new());
    let p = p.to_path_buf();
    let w = w.clone();
    let choose = gtk::MenuItem::with_label("Choose default program...");
    choose.connect_activate(move |_| choose_program(&p, &w));
    ow.append(&choose);
    ow
}

fn launch_with(a: &gio::AppInfo, p: &Path) {
    if let Some(rm) = gtk::RecentManager::default() { rm.add_item(&uri_of(p)); }
    if let Err(e) = a.launch(&[gio::File::for_path(p)], None::<&gio::AppLaunchContext>) {
        util::error(&format!("Windows cannot open '{}'.", ops::display_name(p)), e.message());
    }
}

/// Win7 "Open with" dialog: pick a program, "Always use the selected program to open this kind of file".
fn choose_program(p: &Path, w: &impl IsA<gtk::Window>) {
    let d = gtk::AppChooserDialog::new(Some(w), gtk::DialogFlags::MODAL, &gio::File::for_path(p));
    d.set_title("Open with");
    let always = gtk::CheckButton::with_label("Always use the selected program to open this kind of file");
    always.set_active(true);
    d.content_area().pack_end(&always, false, false, 6);
    always.show();
    if d.run() == gtk::ResponseType::Ok {
        if let Some(a) = d.app_info() {
            if always.is_active() {
                let ct = gio::content_type_guess(Some(p), &[]).0.to_string();
                let _ = a.set_as_default_for_type(&ct);
            }
            launch_with(&a, p);
        }
    }
    d.close();
}

fn is_runnable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let exec = std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
    if !exec { return false; }
    let ct = gio::content_type_guess(Some(p), &[]).0.to_string();
    let head = std::fs::File::open(p).and_then(|mut f| { use std::io::Read; let mut b = [0u8; 4]; f.read_exact(&mut b).map(|_| b) }).unwrap_or_default();
    // programs (ELF binaries, AppImages) run; scripts open in their editor like Windows .txt
    &head == b"\x7fELF" || ct == "application/vnd.appimage"
}

/// Open a file like Windows: shortcuts (.desktop) launch, programs run, documents open in their
/// program; unknown types get the "Open with" dialog.
pub fn open_file(p: &Path, parent: Option<&gtk::Window>) {
    if p.extension().map(|e| e == "desktop").unwrap_or(false) {
        if let Some(a) = gio::DesktopAppInfo::from_filename(p) { util::launch(a.upcast_ref()); return; }
    }
    if is_runnable(p) {
        let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
        match std::process::Command::new(p).current_dir(dir).stdin(std::process::Stdio::null()).spawn() {
            Ok(mut c) => { std::thread::spawn(move || { let _ = c.wait(); }); }
            Err(e) => util::error(&format!("Windows cannot open '{}'.", ops::display_name(p)), &e.to_string()),
        }
        return;
    }
    let ct = listing::stat_entry(p, None).map(|e| e.ctype).unwrap_or_else(|| gio::content_type_guess(Some(p), &[]).0.to_string());
    match gio::AppInfo::default_for_type(&ct, false) {
        Some(a) => launch_with(&a, p),
        None => {
            // Windows can't open this file: choose a program
            let w = parent.cloned().unwrap_or_else(|| gtk::Window::new(gtk::WindowType::Toplevel));
            choose_program(p, &w);
        }
    }
}

fn open_terminal(dir: &Path) {
    if let Some(t) = util::terminal() {
        let id = t.id().map(|s| s.to_string()).unwrap_or_default();
        if id.contains("aerode-cmd") || t.executable().ends_with("aerode-cmd") {
            let _ = std::process::Command::new("aerode-cmd").current_dir(dir).spawn();
            return;
        }
        if let Some(exe) = t.executable().to_str() {
            let _ = std::process::Command::new(exe).current_dir(dir).spawn();
            return;
        }
    }
    let _ = std::process::Command::new("aerode-cmd").current_dir(dir).spawn();
}

fn open_drive_child(b: &gtk::EventBox) {
    if let Some(f) = unsafe { b.data::<Rc<dyn Fn()>>("open") } { let f = unsafe { f.as_ref() }.clone(); f(); }
}

fn load_image_async(p: PathBuf, w: i32, h: i32, cb: impl FnOnce(Option<Pixbuf>) + 'static) {
    let (tx, rx) = async_channel::bounded::<Option<(glib::Bytes, i32, i32, bool, i32)>>(1);
    std::thread::spawn(move || {
        let r = Pixbuf::from_file_at_scale(&p, w, h, true).ok().and_then(|pb| pb.apply_embedded_orientation()).map(|pb| (pb.read_pixel_bytes(), pb.width(), pb.height(), pb.has_alpha(), pb.rowstride()));
        let _ = tx.send_blocking(r);
    });
    glib::MainContext::default().spawn_local(async move {
        let r = rx.recv().await.ok().flatten();
        cb(r.map(|(b, w, h, a, s)| Pixbuf::from_bytes(&b, gtk::gdk_pixbuf::Colorspace::Rgb, a, 8, w, h, s)));
    });
}

/// The Win7 shortcut arrow in the lower left corner.
fn shortcut_overlay(base: &Pixbuf, size: i32) -> Pixbuf {
    let Ok(surf) = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, size, size) else { return base.clone() };
    if let Ok(cr) = gtk::cairo::Context::new(&surf) {
        cr.set_source_pixbuf(base, ((size - base.width()) / 2) as f64, ((size - base.height()) / 2) as f64);
        cr.paint().ok();
        let s = (size as f64 * 0.42).max(9.0).min(48.0);
        let (x, y) = (0.5, size as f64 - s - 0.5);
        cr.rectangle(x, y, s, s);
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.fill_preserve().ok();
        cr.set_source_rgba(0.45, 0.5, 0.58, 1.0);
        cr.set_line_width(1.0);
        cr.stroke().ok();
        // curved blue arrow
        cr.set_source_rgb(0.12, 0.38, 0.78);
        cr.set_line_width((s / 6.0).max(1.5));
        cr.move_to(x + s * 0.25, y + s * 0.82);
        cr.curve_to(x + s * 0.25, y + s * 0.45, x + s * 0.45, y + s * 0.35, x + s * 0.72, y + s * 0.35);
        cr.stroke().ok();
        cr.move_to(x + s * 0.86, y + s * 0.35);
        cr.line_to(x + s * 0.58, y + s * 0.14);
        cr.line_to(x + s * 0.58, y + s * 0.56);
        cr.close_path();
        cr.fill().ok();
    }
    gdk::pixbuf_get_from_surface(&surf, 0, 0, size, size).unwrap_or_else(|| base.clone())
}

/// Win7 draws image thumbnails with a thin frame and drop shadow, videos on a film strip.
fn framed_thumb(pb: &Pixbuf, size: i32, film: bool) -> Pixbuf {
    let (w, h) = (pb.width().min(size - 4), pb.height().min(size - 4));
    let pb = if w != pb.width() || h != pb.height() { pb.scale_simple(w.max(1), h.max(1), gtk::gdk_pixbuf::InterpType::Bilinear).unwrap_or_else(|| pb.clone()) } else { pb.clone() };
    let (w, h) = (pb.width(), pb.height());
    let Ok(surf) = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, size, size) else { return pb };
    {
        let Ok(cr) = gtk::cairo::Context::new(&surf) else { return pb };
        // thumbnails sit on the bottom of the cell like Win7
        let (x, y) = (((size - w) / 2) as f64, (size - h - 2).max(0) as f64);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.18);
        cr.rectangle(x + 2.0, y + 2.0, w as f64, h as f64);
        cr.fill().ok();
        if !film {
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.rectangle(x - 1.0, y - 1.0, w as f64 + 2.0, h as f64 + 2.0);
            cr.fill().ok();
        }
        cr.set_source_pixbuf(&pb, x, y);
        cr.paint().ok();
        if film {
            // sprocket strips on both sides
            let sw = (w as f64 * 0.08).clamp(3.0, 14.0);
            for sx in [x, x + w as f64 - sw] {
                cr.set_source_rgb(0.05, 0.05, 0.05);
                cr.rectangle(sx, y, sw, h as f64);
                cr.fill().ok();
                cr.set_source_rgb(0.85, 0.85, 0.85);
                let hole = (sw * 0.5).max(1.5);
                let mut hy = y + hole * 0.6;
                while hy + hole < y + h as f64 {
                    cr.rectangle(sx + (sw - hole) / 2.0, hy, hole, hole * 0.8);
                    hy += hole * 1.8;
                }
                cr.fill().ok();
            }
        }
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
        cr.set_line_width(1.0);
        cr.rectangle(x - 0.5, y - 0.5, w as f64 + 1.0, h as f64 + 1.0);
        cr.stroke().ok();
    }
    gdk::pixbuf_get_from_surface(&surf, 0, 0, size, size).unwrap_or(pb)
}

/// "file2" < "file10" like Windows Explorer.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["file10.txt", "file2.txt", "File1.txt", "a", "file02.txt"];
        v.sort_by(|a, b| natural_cmp(&a.to_lowercase(), &b.to_lowercase()));
        assert_eq!(v, vec!["a", "File1.txt", "file2.txt", "file02.txt", "file10.txt"]);
    }

    #[test]
    fn kb_sizes() {
        use super::listing::kb;
        assert_eq!(kb(0), "0 KB");
        assert_eq!(kb(1), "1 KB");
        assert_eq!(kb(1024 * 1500), "1,500 KB");
    }

    #[test]
    fn loc_parse() {
        assert_eq!(Loc::parse("trash:///"), Loc::Trash);
        assert_eq!(Loc::parse("library:Pictures"), Loc::Library("Pictures"));
        assert_eq!(Loc::parse("file:///tmp/a%20b"), Loc::Path(PathBuf::from("/tmp/a b")));
        assert_eq!(Loc::parse("libraries:"), Loc::Libraries);
    }
}

