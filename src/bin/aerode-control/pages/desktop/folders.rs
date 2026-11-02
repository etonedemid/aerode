//! Folder Options: General / View / Search, saved in config.folder (Explorer and the desktop follow).
use super::*;
use aerode::config::FolderOptions;

/// View > Advanced settings, in the Win7 order. Rows: ("folder", label, "") opens a group,
/// ("check", label, key), ("radio", label, "key=value") – radios of one key form a group.
const ADVANCED: &[(&str, &str, &str)] = &[
    ("folder", "Files and Folders", ""),
    ("check", "Always show icons, never thumbnails", "icons_only"),
    ("check", "Always show menus", "always_menus"),
    ("check", "Display file icon on thumbnails", "file_icon_on_thumbs"),
    ("check", "Display file size information in folder tips", "size_in_tips"),
    ("check", "Display the full path in the title bar (Classic theme only)", "full_path_title"),
    ("sub", "Hidden files and folders", ""),
    ("radio", "Don't show hidden files, folders, or drives", "show_hidden=0"),
    ("radio", "Show hidden files, folders, and drives", "show_hidden=1"),
    ("check", "Hide empty drives in the Computer folder", "hide_empty_drives"),
    ("check", "Hide extensions for known file types", "hide_ext"),
    ("check", "Hide protected operating system files (Recommended)", "hide_protected"),
    ("check", "Launch folder windows in a separate process", "separate_process"),
    ("check", "Restore previous folder windows at logon", "restore_windows"),
    ("check", "Show drive letters", "drive_letters"),
    ("check", "Show encrypted or compressed NTFS files in color", "color_compressed"),
    ("check", "Show pop-up description for folder and desktop items", "popup_desc"),
    ("check", "Show preview handlers in preview pane", "preview_handlers"),
    ("check", "Use check boxes to select items", "checkboxes"),
    ("check", "Use Sharing Wizard (Recommended)", "sharing_wizard"),
    ("sub", "When typing into list view", ""),
    ("radio", "Automatically type into the Search Box", "type_to_search=1"),
    ("radio", "Select the typed item in the view", "type_to_search=0"),
];

fn get(o: &FolderOptions, key: &str) -> bool {
    match key {
        "icons_only" => o.icons_only, "always_menus" => o.always_menus, "file_icon_on_thumbs" => o.file_icon_on_thumbs,
        "size_in_tips" => o.size_in_tips, "full_path_title" => o.full_path_title, "show_hidden" => o.show_hidden,
        "hide_empty_drives" => o.hide_empty_drives, "hide_ext" => o.hide_ext, "hide_protected" => o.hide_protected,
        "separate_process" => o.separate_process, "restore_windows" => o.restore_windows, "drive_letters" => o.drive_letters,
        "color_compressed" => o.color_compressed, "popup_desc" => o.popup_desc, "preview_handlers" => o.preview_handlers,
        "checkboxes" => o.checkboxes, "sharing_wizard" => o.sharing_wizard, "type_to_search" => o.type_to_search,
        _ => false,
    }
}

fn set(o: &mut FolderOptions, key: &str, v: bool) {
    match key {
        "icons_only" => o.icons_only = v, "always_menus" => o.always_menus = v, "file_icon_on_thumbs" => o.file_icon_on_thumbs = v,
        "size_in_tips" => o.size_in_tips = v, "full_path_title" => o.full_path_title = v, "show_hidden" => o.show_hidden = v,
        "hide_empty_drives" => o.hide_empty_drives = v, "hide_ext" => o.hide_ext = v, "hide_protected" => o.hide_protected = v,
        "separate_process" => o.separate_process = v, "restore_windows" => o.restore_windows = v, "drive_letters" => o.drive_letters = v,
        "color_compressed" => o.color_compressed = v, "popup_desc" => o.popup_desc = v, "preview_handlers" => o.preview_handlers = v,
        "checkboxes" => o.checkboxes = v, "sharing_wizard" => o.sharing_wizard = v, "type_to_search" => o.type_to_search = v,
        _ => {}
    }
}

/// A group box with the Win7 picture on the left.
fn pic_group(label: &str, icon: &str, body: &gtk::Box) -> gtk::Frame {
    let h = hbox(12);
    let img = util::image(icon, 32);
    img.set_valign(gtk::Align::Start);
    h.pack_start(&img, false, false, 0);
    h.pack_start(body, true, true, 0);
    group(label, &h)
}

pub fn page_folderoptions(cp: &Rc<Cp>) -> gtk::Widget {
    cp.side_link("Personalization", "personalization");
    let opts = Rc::new(RefCell::new(config::get().folder));
    let sheet = Sheet::new(cp);
    // every control is re-read from `opts` by `load`, so Restore Defaults just resets `opts`
    let loaders: Rc<RefCell<Vec<Box<dyn Fn(&FolderOptions)>>>> = Rc::new(RefCell::new(vec![]));
    let d = sheet.dirty();

    // ---------------------------------------------------------------- General
    let general = vbox(10);
    let b1 = vbox(4);
    let browse = radios(&["Open each folder in the _same window", "Open each folder in its _own window"], opts.borrow().own_window as usize);
    for r in &browse { b1.pack_start(r, false, false, 0); }
    { let (o, d) = (opts.clone(), d.clone()); browse[1].connect_toggled(move |r| { o.borrow_mut().own_window = r.is_active(); d(); }); }
    { let b = browse.clone(); loaders.borrow_mut().push(Box::new(move |o| b[o.own_window as usize].set_active(true))); }
    general.pack_start(&pic_group("Browse folders", "folder-open", &b1), false, false, 0);
    let b2 = vbox(4);
    let click = radios(&["Single-click to open an item (_point to select)", "_Double-click to open an item (single-click to select)"], if opts.borrow().single_click { 0 } else { 1 });
    let under = radios(&["Underline icon titles consistent with my _browser", "Underline icon titles only when I poin_t at them"], opts.borrow().underline_hover as usize);
    b2.pack_start(&click[0], false, false, 0);
    for r in &under { r.set_margin_start(20); r.set_sensitive(opts.borrow().single_click); b2.pack_start(r, false, false, 0); }
    b2.pack_start(&click[1], false, false, 0);
    { let (o, d, u) = (opts.clone(), d.clone(), under.clone()); click[0].connect_toggled(move |r| {
        o.borrow_mut().single_click = r.is_active();
        for x in &u { x.set_sensitive(r.is_active()); }
        d();
    }); }
    { let (o, d) = (opts.clone(), d.clone()); under[1].connect_toggled(move |r| { o.borrow_mut().underline_hover = r.is_active(); d(); }); }
    { let (c, u) = (click.clone(), under.clone()); loaders.borrow_mut().push(Box::new(move |o| {
        c[if o.single_click { 0 } else { 1 }].set_active(true);
        u[o.underline_hover as usize].set_active(true);
        for x in &u { x.set_sensitive(o.single_click); }
    })); }
    general.pack_start(&pic_group("Click items as follows", "input-mouse", &b2), false, false, 0);
    let b3 = vbox(4);
    let all = check("Show _all folders", opts.borrow().nav_all_folders);
    let expand = check("Automatically _expand to current folder", opts.borrow().nav_expand);
    b3.pack_start(&all, false, false, 0);
    b3.pack_start(&expand, false, false, 0);
    { let (o, d) = (opts.clone(), d.clone()); all.connect_toggled(move |c| { o.borrow_mut().nav_all_folders = c.is_active(); d(); }); }
    { let (o, d) = (opts.clone(), d.clone()); expand.connect_toggled(move |c| { o.borrow_mut().nav_expand = c.is_active(); d(); }); }
    { let (a, e) = (all.clone(), expand.clone()); loaders.borrow_mut().push(Box::new(move |o| { a.set_active(o.nav_all_folders); e.set_active(o.nav_expand); })); }
    general.pack_start(&pic_group("Navigation pane", "view-list-tree", &b3), false, false, 0);
    let rd = gtk::Button::with_mnemonic("_Restore Defaults");
    rd.set_halign(gtk::Align::End);
    {
        let (o, l) = (opts.clone(), loaders.clone());
        rd.connect_clicked(move |_| {
            let def = FolderOptions::default();
            { let mut cur = o.borrow_mut(); cur.own_window = def.own_window; cur.single_click = def.single_click; cur.underline_hover = def.underline_hover; cur.nav_all_folders = def.nav_all_folders; cur.nav_expand = def.nav_expand; }
            let snap = o.borrow().clone();
            for f in l.borrow().iter() { f(&snap); }
        });
    }
    general.pack_start(&rd, false, false, 0);
    sheet.tab("General", &general);

    // ---------------------------------------------------------------- View
    let view = vbox(10);
    let fv = vbox(6);
    fv.pack_start(&text("You can apply the view (such as Details or Icons) that you are using for this folder to all folders of this type."), false, false, 0);
    let fvb = hbox(6);
    let apply_all = gtk::Button::with_mnemonic("Apply to Fo_lders");
    let reset = gtk::Button::with_mnemonic("_Reset Folders");
    fvb.pack_start(&apply_all, false, false, 0);
    fvb.pack_start(&reset, false, false, 0);
    fv.pack_start(&fvb, false, false, 0);
    {
        let cp2 = cp.clone();
        apply_all.connect_clicked(move |_| {
            // Explorer keeps one view for every folder, so all folders already match
            let m = gtk::MessageDialog::new(Some(&parent_window(&cp2)), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo,
                "Do you want all folders of this type to match this folder's view settings?");
            m.set_title("Folder Views");
            m.run();
            m.close();
        });
    }
    {
        let cp2 = cp.clone();
        reset.connect_clicked(move |_| {
            let m = gtk::MessageDialog::new(Some(&parent_window(&cp2)), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo,
                "Do you want to reset all folders of this type to their default view settings?");
            m.set_title("Folder Views");
            if m.run() == gtk::ResponseType::Yes { let _ = std::fs::remove_file(config::conf_dir().join("explorer-view")); }
            m.close();
        });
    }
    view.pack_start(&pic_group("Folder views", "folder", &fv), false, false, 0);
    view.pack_start(&gtk::Label::new(Some("Advanced settings:")), false, false, 0);
    if let Some(l) = view.children().last().and_then(|w| w.clone().downcast::<gtk::Label>().ok()) { l.set_xalign(0.0); }
    // columns: label, kind, key, active, toggle visible, radio, icon, icon visible
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL, glib::Type::BOOL, glib::Type::BOOL, gtk::gdk_pixbuf::Pixbuf::static_type(), glib::Type::BOOL]);
    let fill = {
        let store = store.clone();
        move |o: &FolderOptions| {
            store.clear();
            let mut top: Option<gtk::TreeIter> = None;
            let mut sub: Option<gtk::TreeIter> = None;
            for (kind, label, key) in ADVANCED {
                let (on, parent) = match *kind {
                    "folder" => { sub = None; (false, None) }
                    "sub" => { (false, top.clone()) }
                    "radio" => {
                        let (k, v) = key.split_once('=').unwrap_or((key, "1"));
                        (get(o, k) == (v == "1"), sub.clone().or(top.clone()))
                    }
                    _ => { sub = None; (get(o, key), top.clone()) }
                };
                let icon = match *kind { "folder" | "sub" => util::pixbuf("folder", 16), _ => None };
                let it = store.insert_with_values(parent.as_ref(), None, &[(0, label), (1, kind), (2, key), (3, &on), (4, &(*kind == "check" || *kind == "radio")), (5, &(*kind == "radio")), (6, &icon), (7, &icon.is_some())]);
                match *kind { "folder" => top = Some(it), "sub" => sub = Some(it), _ => {} }
            }
        }
    };
    fill(&opts.borrow());
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let tog = gtk::CellRendererToggle::new();
    let pix = gtk::CellRendererPixbuf::new();
    let txt = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &tog, false);
    TreeViewColumnExt::pack_start(&col, &pix, false);
    TreeViewColumnExt::pack_start(&col, &txt, true);
    TreeViewColumnExt::add_attribute(&col, &tog, "active", 3);
    TreeViewColumnExt::add_attribute(&col, &tog, "visible", 4);
    TreeViewColumnExt::add_attribute(&col, &tog, "radio", 5);
    TreeViewColumnExt::add_attribute(&col, &pix, "pixbuf", 6);
    TreeViewColumnExt::add_attribute(&col, &pix, "visible", 7);
    TreeViewColumnExt::add_attribute(&col, &txt, "text", 0);
    tv.append_column(&col);
    tv.set_enable_tree_lines(true);
    tv.expand_all();
    let toggle_at = {
        let (store, o, d) = (store.clone(), opts.clone(), d.clone());
        move |path: &gtk::TreePath| {
            let Some(it) = store.iter(path) else { return };
            let kind: String = store.value(&it, 1).get().unwrap_or_default();
            let key: String = store.value(&it, 2).get().unwrap_or_default();
            match kind.as_str() {
                "check" => {
                    let on = !store.value(&it, 3).get::<bool>().unwrap_or(false);
                    store.set_value(&it, 3, &on.to_value());
                    set(&mut o.borrow_mut(), &key, on);
                }
                "radio" => {
                    let (k, v) = key.split_once('=').unwrap_or((&key, "1"));
                    set(&mut o.borrow_mut(), k, v == "1");
                    // siblings of the same key follow
                    if let Some(parent) = store.iter_parent(&it) {
                        if let Some(ch) = store.iter_children(Some(&parent)) {
                            loop {
                                let ck: String = store.value(&ch, 2).get().unwrap_or_default();
                                if ck.split_once('=').map(|x| x.0) == Some(k) { store.set_value(&ch, 3, &(ck == key).to_value()); }
                                if !store.iter_next(&ch) { break; }
                            }
                        }
                    }
                }
                _ => return,
            }
            d();
        }
    };
    { let t = toggle_at.clone(); tog.connect_toggled(move |_, p| t(&p)); }
    { let t = toggle_at.clone(); tv.connect_row_activated(move |_, p, _| t(p)); }
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_size_request(-1, 250);
    sw.add(&tv);
    view.pack_start(&sw, true, true, 0);
    let rdv = gtk::Button::with_mnemonic("Restore _Defaults");
    rdv.set_halign(gtk::Align::End);
    {
        let (o, fill, tv, d) = (opts.clone(), fill.clone(), tv.clone(), d.clone());
        rdv.connect_clicked(move |_| {
            let def = FolderOptions::default();
            {
                let mut cur = o.borrow_mut();
                for (kind, _, key) in ADVANCED {
                    let k = key.split_once('=').map(|x| x.0).unwrap_or(key);
                    if *kind == "check" || *kind == "radio" { set(&mut cur, k, get(&def, k)); }
                }
            }
            fill(&o.borrow());
            tv.expand_all();
            d();
        });
    }
    view.pack_start(&rdv, false, false, 0);
    sheet.tab("View", &view);

    // ---------------------------------------------------------------- Search
    let se = vbox(10);
    let w1 = vbox(4);
    let what = radios(&["In _indexed locations, search filenames and contents. In non-indexed locations, search file names only.", "Always search file _names and contents (this might take several minutes)"], opts.borrow().search_contents as usize);
    for r in &what { if let Some(l) = r.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_line_wrap(true); l.set_max_width_chars(48); } w1.pack_start(r, false, false, 0); }
    { let (o, d) = (opts.clone(), d.clone()); what[1].connect_toggled(move |r| { o.borrow_mut().search_contents = r.is_active(); d(); }); }
    se.pack_start(&group("What to search", &w1), false, false, 0);
    let mk = |b: &gtk::Box, label: &str, on: bool, f: fn(&mut FolderOptions, bool)| {
        let c = check(label, on);
        if let Some(l) = c.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_line_wrap(true); l.set_max_width_chars(48); }
        let (o, d) = (opts.clone(), d.clone());
        c.connect_toggled(move |c| { f(&mut o.borrow_mut(), c.is_active()); d(); });
        b.pack_start(&c, false, false, 0);
        c
    };
    let w2 = vbox(4);
    let o0 = opts.borrow().clone();
    let s1 = mk(&w2, "Include _subfolders in search results when searching in file folders", o0.search_subfolders, |o, v| o.search_subfolders = v);
    let s2 = mk(&w2, "Find _partial matches", o0.search_partial, |o, v| o.search_partial = v);
    let s3 = mk(&w2, "Use natural _language search", o0.search_natural, |o, v| o.search_natural = v);
    let s4 = mk(&w2, "Don't use the inde_x when searching in file folders for system files (searches might take longer)", o0.search_no_index, |o, v| o.search_no_index = v);
    se.pack_start(&group("How to search", &w2), false, false, 0);
    let w3 = vbox(4);
    let s5 = mk(&w3, "Include system _directories", o0.search_system_dirs, |o, v| o.search_system_dirs = v);
    let s6 = mk(&w3, "Include _compressed files (ZIP, CAB...)", o0.search_compressed, |o, v| o.search_compressed = v);
    se.pack_start(&group("When searching non-indexed locations", &w3), false, false, 0);
    let rds = gtk::Button::with_mnemonic("Restore _Defaults");
    rds.set_halign(gtk::Align::End);
    {
        let w = what.clone();
        rds.connect_clicked(move |_| {
            let def = FolderOptions::default();
            w[def.search_contents as usize].set_active(true);
            for (c, v) in [(&s1, def.search_subfolders), (&s2, def.search_partial), (&s3, def.search_natural), (&s4, def.search_no_index), (&s5, def.search_system_dirs), (&s6, def.search_compressed)] { c.set_active(v); }
        });
    }
    se.pack_start(&rds, false, false, 0);
    sheet.tab("Search", &se);

    sheet.on_apply(move || {
        let o = opts.borrow().clone();
        config::update(|c| c.folder = o);
        // the desktop follows hidden files, extensions and single-click; Explorer watches config.json
        reload_shell();
        true
    });
    sheet.widget()
}
