//! The navigation pane: Favorites, Libraries, Computer, Network – folders expand lazily, and the
//! tree follows the current folder (Folder Options: "Show all folders", "Automatically expand to
//! current folder").
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::{config, util};

const N_ICON: u32 = 0;
const N_NAME: u32 = 1;
const N_TARGET: u32 = 2;
const N_LOADED: u32 = 3;

pub struct NavPane {
    pub view: gtk::TreeView,
    store: gtk::TreeStore,
    guard: Cell<bool>,
    pub show_hidden: Cell<bool>,
    all_folders: bool,
    vm: gio::VolumeMonitor,
}

fn icon(name: &str) -> Option<Pixbuf> { util::pixbuf_or(name, 16, "folder") }

fn uri(p: &Path) -> String { gio::File::for_path(p).uri().to_string() }

pub fn library_dir(n: &str) -> PathBuf {
    match n {
        "Music" => util::special_dir(glib::UserDirectory::Music, "Music"),
        "Pictures" => util::special_dir(glib::UserDirectory::Pictures, "Pictures"),
        "Videos" => util::special_dir(glib::UserDirectory::Videos, "Videos"),
        _ => util::special_dir(glib::UserDirectory::Documents, "Documents"),
    }
}

/// Icon for a folder path: the Win7 special folders have their own.
pub fn folder_icon(p: &Path) -> &'static str {
    let specials = [(glib::UserDirectory::Documents, "Documents", "folder-documents"), (glib::UserDirectory::Music, "Music", "folder-music"),
        (glib::UserDirectory::Pictures, "Pictures", "folder-pictures"), (glib::UserDirectory::Videos, "Videos", "folder-videos"),
        (glib::UserDirectory::Downloads, "Downloads", "folder-download"), (glib::UserDirectory::Desktop, "Desktop", "user-desktop")];
    for (d, f, i) in specials { if util::special_dir(d, f) == p { return i; } }
    if p == config::home() { return "user-home"; }
    if p == Path::new("/") { return util::first_icon(&["drive-harddisk-system", "drive-harddisk"]); }
    "folder"
}

impl NavPane {
    pub fn new(all_folders: bool, show_hidden: bool) -> Rc<NavPane> {
        let store = gtk::TreeStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL]);
        let view = gtk::TreeView::with_model(&store);
        view.set_headers_visible(false);
        view.set_enable_search(false);
        view.set_show_expanders(true);
        view.set_level_indentation(0);
        let col = gtk::TreeViewColumn::new();
        let ri = gtk::CellRendererPixbuf::new();
        let rt = gtk::CellRendererText::new();
        rt.set_ellipsize(gtk::pango::EllipsizeMode::End);
        TreeViewColumnExt::pack_start(&col, &ri, false);
        TreeViewColumnExt::pack_start(&col, &rt, true);
        TreeViewColumnExt::add_attribute(&col, &ri, "pixbuf", N_ICON as i32);
        TreeViewColumnExt::add_attribute(&col, &rt, "text", N_NAME as i32);
        view.append_column(&col);
        let nav = Rc::new(NavPane { view: view.clone(), store, guard: Cell::new(false), show_hidden: Cell::new(show_hidden), all_folders, vm: gio::VolumeMonitor::get() });
        nav.build();
        {
            let n = Rc::downgrade(&nav);
            view.connect_test_expand_row(move |_, it, _| {
                if let Some(n) = n.upgrade() { n.load_children(it); }
                glib::Propagation::Proceed
            });
        }
        // drives come and go
        for sig in ["mount-added", "mount-removed"] {
            let n = Rc::downgrade(&nav);
            nav.vm.connect_local(sig, false, move |_| { if let Some(n) = n.upgrade() { n.refresh_computer(); } None });
        }
        nav
    }

    fn add(&self, parent: Option<&gtk::TreeIter>, icon_name: &str, name: &str, target: &str, expandable: bool) -> gtk::TreeIter {
        let it = self.store.append(parent);
        self.store.set(&it, &[(N_ICON, &icon(icon_name)), (N_NAME, &name), (N_TARGET, &target), (N_LOADED, &!expandable)]);
        if expandable {
            // placeholder so the expander shows; replaced on first expansion
            let d = self.store.append(Some(&it));
            self.store.set(&d, &[(N_NAME, &""), (N_TARGET, &""), (N_LOADED, &true)]);
        }
        it
    }

    fn build(&self) {
        let fav = self.add(None, util::first_icon(&["starred", "emblem-favorite", "folder"]), "Favorites", "", false);
        self.add(Some(&fav), "user-desktop", "Desktop", &uri(&util::special_dir(glib::UserDirectory::Desktop, "Desktop")), false);
        self.add(Some(&fav), "folder-download", "Downloads", &uri(&util::special_dir(glib::UserDirectory::Downloads, "Downloads")), false);
        self.add(Some(&fav), util::first_icon(&["folder-recent", "document-open-recent", "folder"]), "Recent Places", "recent:", false);
        let parent = if self.all_folders {
            Some(self.add(None, "user-desktop", "Desktop", &uri(&util::special_dir(glib::UserDirectory::Desktop, "Desktop")), false))
        } else { None };
        let libs = self.add(parent.as_ref(), util::first_icon(&["folder-libraries", "system-file-manager", "folder-documents"]), "Libraries", "libraries:", false);
        for (n, i) in [("Documents", "folder-documents"), ("Music", "folder-music"), ("Pictures", "folder-pictures"), ("Videos", "folder-videos")] {
            self.add(Some(&libs), i, n, &format!("library:{n}"), true);
        }
        if self.all_folders {
            self.add(parent.as_ref(), "user-home", &glib::user_name().to_string_lossy(), &uri(&config::home()), true);
        }
        let comp = self.add(parent.as_ref(), "computer", "Computer", "computer:///", false);
        self.fill_computer(&comp);
        self.add(parent.as_ref(), "network-workgroup", "Network", "network:///", false);
        if self.all_folders {
            self.add(parent.as_ref(), "preferences-system", "Control Panel", "control:", false);
            self.add(parent.as_ref(), "user-trash", "Recycle Bin", "trash:///", false);
        }
        // Win7 opens Favorites, Libraries and Computer, not the folders inside
        let m: gtk::TreeModel = self.store.clone().upcast();
        if let Some(it) = m.iter_first() {
            loop {
                if let Some(p) = m.path(&it) { self.view.expand_row(&p, false); }
                if !m.iter_next(&it) { break; }
            }
        }
        if let Some(p) = parent.and_then(|p| m.path(&p)) { self.view.expand_row(&p, false); }
    }

    fn fill_computer(&self, comp: &gtk::TreeIter) {
        self.add(Some(comp), util::first_icon(&["drive-harddisk-system", "drive-harddisk"]), "Local Disk (/)", &uri(Path::new("/")), true);
        for mnt in self.vm.mounts() {
            let Some(p) = mnt.root().path() else { continue };
            if p == Path::new("/") { continue; }
            let ic = if mnt.can_eject() || p.starts_with("/run/media") || p.starts_with("/media") { "drive-removable-media" } else { "drive-harddisk" };
            self.add(Some(comp), ic, &mnt.name(), &uri(&p), true);
        }
        self.store.set_value(comp, N_LOADED, &true.to_value());
    }

    fn refresh_computer(&self) {
        let Some(comp) = self.find("computer:///") else { return };
        let was_open = self.store.path(&comp).map(|p| self.view.row_expanded(&p)).unwrap_or(false);
        while let Some(c) = self.store.iter_children(Some(&comp)) { self.store.remove(&c); }
        self.fill_computer(&comp);
        if was_open { if let Some(p) = self.store.path(&comp) { self.view.expand_row(&p, false); } }
    }

    fn target(&self, it: &gtk::TreeIter) -> String { self.store.value(it, N_TARGET as i32).get().unwrap_or_default() }

    fn dir_of(target: &str) -> Option<PathBuf> {
        if let Some(n) = target.strip_prefix("library:") { return Some(library_dir(n)); }
        Self::path_of(target)
    }

    /// Folder nodes hold file URIs (lossless for names that are not UTF-8); plain paths work too.
    fn path_of(target: &str) -> Option<PathBuf> {
        if target.starts_with("file://") { return gio::File::for_uri(target).path(); }
        target.starts_with('/').then(|| PathBuf::from(target))
    }

    /// Fill in the subfolders of a node the first time it is opened.
    fn load_children(&self, it: &gtk::TreeIter) {
        if self.store.value(it, N_LOADED as i32).get::<bool>().unwrap_or(true) { return; }
        self.store.set_value(it, N_LOADED, &true.to_value());
        let Some(dir) = Self::dir_of(&self.target(it)) else { return };
        let mut subs: Vec<(String, PathBuf)> = std::fs::read_dir(&dir).map(|r| r.flatten().filter(|e| {
            e.file_type().map(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())).unwrap_or(false)
        }).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).filter(|(n, _)| self.show_hidden.get() || !n.starts_with('.')).collect()).unwrap_or_default();
        subs.sort_by(|a, b| super::natural_cmp(&a.0.to_lowercase(), &b.0.to_lowercase()));
        // remove the placeholder
        while let Some(c) = self.store.iter_children(Some(it)) { self.store.remove(&c); }
        for (name, p) in subs.into_iter().take(2000) {
            self.add(Some(it), folder_icon(&p), &name, &uri(&p), true);
        }
    }

    /// Forget loaded subfolders (F5 or Folder Options) so they are read again on expansion.
    pub fn reset_folders(&self) {
        let m: gtk::TreeModel = self.store.clone().upcast();
        let mut stale = vec![];
        m.foreach(|m, p, it| {
            let t: String = m.value(it, N_TARGET as i32).get().unwrap_or_default();
            if Self::dir_of(&t).is_some() && t != "computer:///" && m.iter_has_child(it) && !self.view.row_expanded(p) { stale.push(it.clone()); }
            false
        });
        for it in stale {
            while let Some(c) = self.store.iter_children(Some(&it)) { self.store.remove(&c); }
            let d = self.store.append(Some(&it));
            self.store.set(&d, &[(N_NAME, &""), (N_TARGET, &""), (N_LOADED, &true)]);
            self.store.set_value(&it, N_LOADED, &false.to_value());
        }
    }

    fn find(&self, target: &str) -> Option<gtk::TreeIter> {
        let m: gtk::TreeModel = self.store.clone().upcast();
        let mut found = None;
        m.foreach(|m, _, it| {
            let t: String = m.value(it, N_TARGET as i32).get().unwrap_or_default();
            if t == target { found = Some(it.clone()); true } else { false }
        });
        found
    }

    /// Call `f(target)` when the user picks a node.
    pub fn connect_navigate(self: &Rc<Self>, f: impl Fn(String) + 'static) {
        let n = Rc::downgrade(self);
        self.view.selection().connect_changed(move |s| {
            let Some(n) = n.upgrade() else { return };
            if n.guard.get() { return; }
            let Some((m, it)) = s.selected() else { return };
            let t: String = m.value(&it, N_TARGET as i32).get().unwrap_or_default();
            if !t.is_empty() { f(t); }
        });
        // a click on the already selected node goes there again (e.g. back from a subfolder)
        let n = Rc::downgrade(self);
        let f2 = Rc::new(());
        let _ = f2;
        self.view.connect_row_activated(move |v, p, _| {
            if let Some(n) = n.upgrade() { if !n.guard.get() { if v.row_expanded(p) { v.collapse_row(p); } else { v.expand_row(p, false); } } }
        });
    }

    /// Highlight the node for the current location; with `expand` open the tree down to it.
    pub fn select(&self, target: &str, expand: bool) {
        self.guard.set(true);
        let sel = self.view.selection();
        let mut it = self.find(target);
        if let (true, true, Some(want)) = (it.is_none(), expand, Self::path_of(target)) {
            // deepest node whose folder contains the target, preferring Computer's tree
            let m: gtk::TreeModel = self.store.clone().upcast();
            let mut best: Option<(usize, gtk::TreeIter)> = None;
            m.foreach(|m, _, i| {
                let t: String = m.value(i, N_TARGET as i32).get().unwrap_or_default();
                if let Some(tp) = Self::path_of(&t).filter(|tp| want.starts_with(tp)) {
                    let depth = tp.components().count();
                    if best.as_ref().map(|(d, _)| depth > *d).unwrap_or(true) { best = Some((depth, i.clone())); }
                }
                false
            });
            if let Some((_, mut cur)) = best {
                loop {
                    let ct = self.target(&cur);
                    if ct == target || Self::path_of(&ct).as_ref() == Some(&want) { it = Some(cur); break; }
                    self.load_children(&cur);
                    if let Some(p) = self.store.path(&cur) { self.view.expand_row(&p, false); }
                    let mut next = None;
                    if let Some(c) = self.store.iter_children(Some(&cur)) {
                        loop {
                            let t = self.target(&c);
                            if Self::path_of(&t).map(|tp| want.starts_with(tp)).unwrap_or(false) { next = Some(c.clone()); break; }
                            if !self.store.iter_next(&c) { break; }
                        }
                    }
                    match next { Some(n) => cur = n, None => break }
                }
            }
        }
        match it.and_then(|it| self.store.path(&it)) {
            Some(p) => {
                self.view.expand_to_path(&p);
                if p.depth() > 1 { let mut parent = p.clone(); parent.up(); if !self.view.row_expanded(&parent) { self.view.expand_row(&parent, false); } }
                sel.select_path(&p);
                self.view.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0);
            }
            None => sel.unselect_all(),
        }
        self.guard.set(false);
    }

    /// The folder a drop at (x, y) (widget coordinates) would go to.
    pub fn drop_target(&self, x: i32, y: i32) -> Option<(gtk::TreePath, PathBuf)> {
        let (p, _) = self.view.dest_row_at_pos(x, y)?;
        let p = p?;
        let it = self.store.iter(&p)?;
        let t = self.target(&it);
        let d = if t == "trash:///" { PathBuf::from("trash:") } else { Self::dir_of(&t)? };
        Some((p, d))
    }
}
