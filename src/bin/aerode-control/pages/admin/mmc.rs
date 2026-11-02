//! A Microsoft Management Console look-alike: menu bar (File Action View Help), toolbar, console
//! tree on the left, the snap-in's result pane in the middle and the Actions pane on the right.
//! Snap-ins add tree nodes with an id and register a handler for an id prefix; selecting a node
//! calls the handler, which fills the centre (`set_center`) and the Actions pane (`set_actions`).
use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf::Pixbuf, glib};
use std::cell::RefCell;
use std::rc::Rc;

use super::*;

pub type Handler = Rc<dyn Fn(&Rc<Mmc>, &str)>;

/// One entry of the Actions pane / Action menu. `f == None` shows it greyed out.
#[derive(Clone)]
pub struct Act {
    pub label: String,
    pub icon: &'static [&'static str],
    pub f: Option<Rc<dyn Fn()>>,
}

pub fn act(label: &str, icon: &'static [&'static str], f: impl Fn() + 'static) -> Act { Act { label: label.into(), icon, f: Some(Rc::new(f)) } }
pub fn act_off(label: &str) -> Act { Act { label: label.into(), icon: &[], f: None } }
pub const I_REFRESH: &[&str] = &["view-refresh"];
pub const I_PROPS: &[&str] = &["document-properties", "preferences-system"];
pub const I_OPEN: &[&str] = &["folder-open"];
pub const I_SAVE: &[&str] = &["document-save"];
pub const I_COPY: &[&str] = &["edit-copy"];
pub const I_FIND: &[&str] = &["edit-find"];
pub const I_DELETE: &[&str] = &["edit-delete"];
pub const I_RUN: &[&str] = &["media-playback-start", "system-run"];
pub const I_STOP: &[&str] = &["media-playback-stop", "process-stop"];
pub const I_FILTER: &[&str] = &["view-filter", "edit-find"];

pub struct Mmc {
    pub win: gtk::Window,
    pub store: gtk::TreeStore,
    pub tree: gtk::TreeView,
    tree_pane: gtk::ScrolledWindow,
    center: gtk::Box,
    actions: gtk::Box,
    actions_pane: gtk::Box,
    action_menu: gtk::Menu,
    pub view_menu: gtk::Menu,
    pub toolbar: gtk::Box,
    pub status: gtk::Label,
    handlers: RefCell<Vec<(String, Handler)>>,
    hist: RefCell<(Vec<String>, usize)>,
    back: gtk::Button,
    fwd: gtk::Button,
    navigating: std::cell::Cell<bool>,
    pub current: RefCell<String>,
    cur_actions: RefCell<Vec<(String, Vec<Act>)>>,
    /// snap-in state that lives as long as the window
    kept: RefCell<Vec<Rc<dyn std::any::Any>>>,
}

fn tool_button(icons: &[&str], tip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_image(Some(&img(icons, 16)));
    b.set_relief(gtk::ReliefStyle::None);
    b.set_tooltip_text(Some(tip));
    b.set_focus_on_click(false);
    b
}

impl Mmc {
    /// A console window. `tree` = false hides the console tree (devmgmt.msc, diskmgmt.msc),
    /// `actions` = false hides the Actions pane.
    pub fn new(app: &gtk::Application, title: &str, icon: &[&str], tree: bool, with_actions: bool) -> Rc<Mmc> {
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.set_application(Some(app));
        win.set_title(title);
        win.set_default_size(980, 640);
        win.set_icon(pix(icon, 48).as_ref());
        win.style_context().add_class("adm");
        let root = vbox(0);
        // ------------------------------------------------ menu bar
        let bar = gtk::MenuBar::new();
        let menu = |name: &str| { let mi = gtk::MenuItem::with_mnemonic(name); let m = gtk::Menu::new(); mi.set_submenu(Some(&m)); bar.append(&mi); m };
        let file = menu("_File");
        let action_menu = menu("_Action");
        let view_menu = menu("_View");
        let help = menu("_Help");
        let exit = gtk::MenuItem::with_mnemonic("E_xit");
        { let w = win.clone(); exit.connect_activate(move |_| w.close()); }
        file.append(&exit);
        let about = gtk::MenuItem::with_mnemonic(&format!("_About {title}"));
        { let (w, t) = (win.clone(), title.to_string()); about.connect_activate(move |_| {
            inform(Some(&w), &format!("About {t}"), &t, "Aerode – a Windows 7 desktop for Linux.\nThis console shows information from the Linux system it runs on.");
        }); }
        let topics = gtk::MenuItem::with_mnemonic("_Help Topics");
        topics.set_sensitive(false);
        help.append(&topics);
        help.append(&gtk::SeparatorMenuItem::new());
        help.append(&about);
        root.pack_start(&bar, false, false, 0);
        // ------------------------------------------------ toolbar
        let toolbar = hbox(0);
        toolbar.set_border_width(1);
        let back = tool_button(&["go-previous", "go-previous-symbolic"], "Back");
        let fwd = tool_button(&["go-next", "go-next-symbolic"], "Forward");
        back.set_sensitive(false);
        fwd.set_sensitive(false);
        toolbar.pack_start(&back, false, false, 0);
        toolbar.pack_start(&fwd, false, false, 0);
        toolbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 3);
        let tree_btn = tool_button(&["view-dual", "view-list", "folder"], "Show/Hide Console Tree");
        toolbar.pack_start(&tree_btn, false, false, 0);
        let help_btn = tool_button(&["help-browser", "help-contents"], "Help");
        help_btn.set_sensitive(false);
        let act_btn = tool_button(&["view-list-details", "view-list", "document-properties"], "Show/Hide Action Pane");
        root.pack_start(&toolbar, false, false, 0);
        root.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0);
        // ------------------------------------------------ panes
        let store = gtk::TreeStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
        let tv = gtk::TreeView::with_model(&store);
        tv.set_headers_visible(false);
        tv.style_context().add_class("mmc-tree");
        let col = gtk::TreeViewColumn::new();
        let pr = gtk::CellRendererPixbuf::new();
        let tr = gtk::CellRendererText::new();
        TreeViewColumnExt::pack_start(&col, &pr, false);
        TreeViewColumnExt::add_attribute(&col, &pr, "pixbuf", 0);
        TreeViewColumnExt::pack_start(&col, &tr, true);
        TreeViewColumnExt::add_attribute(&col, &tr, "text", 1);
        tv.append_column(&col);
        let tree_pane = scrolled(&tv);
        tree_pane.set_size_request(210, -1);
        tree_pane.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        let center = vbox(0);
        let actions = vbox(0);
        let actions_pane = vbox(0);
        actions_pane.style_context().add_class("mmc-actions");
        let at = gtk::Label::new(Some("Actions"));
        at.set_xalign(0.0);
        at.style_context().add_class("mmc-actions-title");
        actions_pane.pack_start(&at, false, false, 0);
        let asw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        asw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        asw.add(&actions);
        actions_pane.pack_start(&asw, true, true, 0);
        actions_pane.set_size_request(200, -1);
        let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
        inner.pack1(&center, true, false);
        inner.pack2(&actions_pane, false, false);
        let outer = gtk::Paned::new(gtk::Orientation::Horizontal);
        outer.pack1(&tree_pane, false, false);
        outer.pack2(&inner, true, false);
        root.pack_start(&outer, true, true, 0);
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.style_context().add_class("mmc-status");
        root.pack_start(&status, false, false, 0);
        win.add(&root);
        let m = Rc::new(Mmc {
            win, store, tree: tv, tree_pane: tree_pane.clone(), center, actions, actions_pane: actions_pane.clone(), action_menu, view_menu, toolbar: toolbar.clone(),
            status, handlers: RefCell::new(vec![]), hist: RefCell::new((vec![], 0)), back: back.clone(), fwd: fwd.clone(),
            navigating: std::cell::Cell::new(false), current: RefCell::new(String::new()), cur_actions: RefCell::new(vec![]), kept: RefCell::new(vec![]),
        });
        // the console lives as long as its window (signal handlers only hold weak references)
        { let me = RefCell::new(Some(m.clone())); m.win.connect_destroy(move |_| { if let Some(m) = me.borrow_mut().take() { m.kept.borrow_mut().clear(); } }); }
        // the toolbar's help / action-pane buttons come after the snap-in's own buttons
        toolbar.pack_end(&act_btn, false, false, 0);
        toolbar.pack_end(&help_btn, false, false, 0);
        { let ap = actions_pane.clone(); act_btn.connect_clicked(move |_| ap.set_visible(!ap.is_visible())); }
        { let tp = tree_pane.clone(); tree_btn.connect_clicked(move |_| tp.set_visible(!tp.is_visible())); }
        {
            let me = Rc::downgrade(&m);
            m.tree.selection().connect_changed(move |s| {
                let Some(m) = me.upgrade() else { return };
                if let Some((model, it)) = s.selected() { let id = sval(&model, &it, 2); m.dispatch(&id); }
            });
        }
        {
            let me = Rc::downgrade(&m);
            m.tree.connect_button_press_event(move |tv, ev| {
                if ev.button() == 3 {
                    if let (Some(m), Some((Some(path), _, _, _))) = (me.upgrade(), tv.path_at_pos(ev.position().0 as i32, ev.position().1 as i32)) {
                        tv.selection().select_path(&path);
                        m.popup_actions(ev);
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        { let me = Rc::downgrade(&m); back.connect_clicked(move |_| if let Some(m) = me.upgrade() { m.go(-1) }); }
        { let me = Rc::downgrade(&m); fwd.connect_clicked(move |_| if let Some(m) = me.upgrade() { m.go(1) }); }
        if !tree { tree_pane.set_no_show_all(true); tree_pane.hide(); }
        if !with_actions { actions_pane.set_no_show_all(true); actions_pane.hide(); }
        // keep the panes at Win7-like widths
        outer.set_position(if tree { 210 } else { 0 });
        { let i = inner.clone(); m.win.connect_map(move |w| { let width = w.allocated_width(); i.set_position((width - 230).max(400)); }); }
        m
    }

    /// Keep `v` alive until the window closes.
    pub fn keep(&self, v: Rc<dyn std::any::Any>) { self.kept.borrow_mut().push(v); }

    /// Add a node under `parent`; returns its iterator.
    pub fn add(&self, parent: Option<&gtk::TreeIter>, id: &str, label: &str, icon: &[&str]) -> gtk::TreeIter {
        let pb = pix(icon, 16);
        self.store.insert_with_values(parent, None, &[(0, &pb), (1, &label), (2, &id)])
    }

    /// Selecting a node whose id starts with `prefix` calls `f` (longest prefix wins).
    pub fn handle(&self, prefix: &str, f: impl Fn(&Rc<Mmc>, &str) + 'static) {
        self.handlers.borrow_mut().push((prefix.to_string(), Rc::new(f)));
    }

    fn dispatch(self: &Rc<Self>, id: &str) {
        if *self.current.borrow() == id { return; }
        *self.current.borrow_mut() = id.to_string();
        if !self.navigating.get() {
            let mut h = self.hist.borrow_mut();
            let pos = h.1;
            h.0.truncate(pos);
            h.0.push(id.to_string());
            h.1 = h.0.len();
        }
        { let h = self.hist.borrow(); self.back.set_sensitive(h.1 > 1); self.fwd.set_sensitive(h.1 < h.0.len()); }
        let f = self.handlers.borrow().iter().filter(|(p, _)| id.starts_with(p.as_str())).max_by_key(|(p, _)| p.len()).map(|(_, f)| f.clone());
        self.status.set_text("");
        if let Some(f) = f { f(self, id); }
    }

    fn go(self: &Rc<Self>, d: i32) {
        let target = {
            let mut h = self.hist.borrow_mut();
            let n = h.1 as i32 + d;
            if n < 1 || n as usize > h.0.len() { return; }
            h.1 = n as usize;
            h.0[n as usize - 1].clone()
        };
        self.navigating.set(true);
        self.select(&target);
        self.navigating.set(false);
        let h = self.hist.borrow();
        self.back.set_sensitive(h.1 > 1);
        self.fwd.set_sensitive(h.1 < h.0.len());
    }

    /// Select the node with this id (expanding its parents).
    pub fn select(self: &Rc<Self>, id: &str) {
        let mut found = None;
        self.store.foreach(|m, path, it| { if sval(m, it, 2) == id { found = Some(path.clone()); true } else { false } });
        if let Some(p) = found {
            self.tree.expand_to_path(&p);
            self.tree.selection().select_path(&p);
            self.tree.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, false, 0.0, 0.0);
        } else {
            self.dispatch(id);
        }
    }


    /// Replace the result pane.
    pub fn set_center(&self, w: &impl IsA<gtk::Widget>) {
        for c in self.center.children() { self.center.remove(&c); }
        self.center.pack_start(w, true, true, 0);
        self.center.show_all();
    }

    /// Replace the Actions pane (and the Action menu): groups of (title, actions).
    pub fn set_actions(&self, groups: Vec<(String, Vec<Act>)>) {
        for c in self.actions.children() { self.actions.remove(&c); }
        for c in self.action_menu.children() { self.action_menu.remove(&c); }
        for (gi, (title, acts)) in groups.iter().enumerate() {
            let head = hbox(4);
            head.style_context().add_class("mmc-act-head");
            let t = gtk::Label::new(Some(title));
            t.set_xalign(0.0);
            t.set_ellipsize(gtk::pango::EllipsizeMode::End);
            head.pack_start(&t, true, true, 0);
            let arrow = gtk::Label::new(Some("▴"));
            head.pack_end(&arrow, false, false, 0);
            let eb = gtk::EventBox::new();
            eb.add(&head);
            self.actions.pack_start(&eb, false, false, 0);
            let items = vbox(0);
            items.style_context().add_class("mmc-act");
            items.set_margin_start(2);
            for a in acts {
                let b = gtk::Button::new();
                let h = hbox(6);
                let icon = if a.icon.is_empty() { gtk::Image::new() } else { img(a.icon, 16) };
                icon.set_size_request(16, 16);
                h.pack_start(&icon, false, false, 0);
                let l = gtk::Label::new(Some(&a.label));
                l.set_xalign(0.0);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                h.pack_start(&l, true, true, 0);
                if a.label.ends_with('▸') { l.set_text(a.label.trim_end_matches('▸').trim_end()); h.pack_end(&gtk::Label::new(Some("▸")), false, false, 0); }
                b.add(&h);
                b.set_relief(gtk::ReliefStyle::None);
                match &a.f { Some(f) => { let f = f.clone(); b.connect_clicked(move |_| f()); } None => b.set_sensitive(false) }
                items.pack_start(&b, false, false, 0);
                // Action menu
                let mi = gtk::MenuItem::with_label(a.label.trim_end_matches('▸').trim_end());
                if !a.icon.is_empty() { util::menu_icon(&mi, util::first_icon(a.icon)); }
                match &a.f { Some(f) => { let f = f.clone(); mi.connect_activate(move |_| f()); } None => mi.set_sensitive(false) }
                self.action_menu.append(&mi);
            }
            if gi + 1 < groups.len() { self.action_menu.append(&gtk::SeparatorMenuItem::new()); }
            self.actions.pack_start(&items, false, false, 2);
            { let (items, arrow) = (items.clone(), arrow.clone()); eb.connect_button_press_event(move |_, _| {
                let v = !items.is_visible(); items.set_visible(v); arrow.set_text(if v { "▴" } else { "▾" }); glib::Propagation::Stop
            }); }
        }
        self.actions.show_all();
        self.action_menu.show_all();
        *self.cur_actions.borrow_mut() = groups;
    }

    /// The current actions as a context menu (right-click in the tree or a result list).
    pub fn popup_actions(&self, ev: &gdk::EventButton) {
        let menu = gtk::Menu::new();
        let groups = self.cur_actions.borrow().clone();
        // the item group (the last one) first, like the context menus of Win7 consoles
        for (gi, (_, acts)) in groups.iter().rev().enumerate() {
            if gi > 0 { menu.append(&gtk::SeparatorMenuItem::new()); }
            for a in acts.iter().filter(|a| !a.label.starts_with("Help")) {
                let mi = gtk::MenuItem::with_label(a.label.trim_end_matches('▸').trim_end());
                match &a.f { Some(f) => { let f = f.clone(); mi.connect_activate(move |_| f()); } None => mi.set_sensitive(false) }
                menu.append(&mi);
            }
        }
        menu.show_all();
        menu.popup_at_pointer(Some(ev));
    }

    /// Add a snap-in button to the toolbar (after the standard ones).
    pub fn tool(&self, icons: &[&str], tip: &str, f: impl Fn() + 'static) -> gtk::Button {
        let b = tool_button(icons, tip);
        b.connect_clicked(move |_| f());
        self.toolbar.pack_start(&b, false, false, 0);
        b
    }
    pub fn tool_sep(&self) { self.toolbar.pack_start(&gtk::Separator::new(gtk::Orientation::Vertical), false, false, 3); }

    pub fn show(self: &Rc<Self>, first: &str) {
        let rc = gtk::MenuItem::with_mnemonic("_Refresh");
        { let me = Rc::downgrade(self); rc.connect_activate(move |_| if let Some(m) = me.upgrade() { m.refresh(); }); }
        self.view_menu.append(&rc);
        self.win.show_all();
        self.tree.expand_row(&gtk::TreePath::new_first(), false);
        self.select(first);
    }

    /// Run the handler of the current node again.
    pub fn refresh(self: &Rc<Self>) {
        let id = self.current.borrow().clone();
        *self.current.borrow_mut() = String::new();
        self.navigating.set(true);
        self.dispatch(&id);
        self.navigating.set(false);
    }

    pub fn tree_visible(&self) -> bool { self.tree_pane.is_visible() }
    pub fn actions_visible(&self) -> bool { self.actions_pane.is_visible() }
}

/// The title strip above a result list ("System   Number of events: 1,234").
pub fn head(title: &str, extra: &str) -> (gtk::Box, gtk::Label) {
    let h = hbox(16);
    h.style_context().add_class("mmc-head");
    let t = gtk::Label::new(Some(title));
    t.style_context().add_class("t");
    h.pack_start(&t, false, false, 0);
    let e = gtk::Label::new(Some(extra));
    e.set_xalign(0.0);
    e.set_ellipsize(gtk::pango::EllipsizeMode::End);
    h.pack_start(&e, true, true, 0);
    (h, e)
}

/// "1,234"
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() { if i > 0 && (s.len() - i) % 3 == 0 { out.push(','); } out.push(c); }
    out
}

/// A collapsible group box like the ones on the overview pages of Event Viewer / Task Scheduler.
pub fn group(title: &str, body: &impl IsA<gtk::Widget>) -> gtk::Box {
    let v = vbox(0);
    let h = hbox(4);
    h.style_context().add_class("mmc-group");
    let t = gtk::Label::new(Some(title));
    t.set_xalign(0.0);
    h.pack_start(&t, true, true, 0);
    let arrow = gtk::Label::new(Some("▴"));
    h.pack_end(&arrow, false, false, 0);
    let eb = gtk::EventBox::new();
    eb.add(&h);
    v.pack_start(&eb, false, false, 0);
    let inner = vbox(0);
    inner.set_border_width(6);
    inner.pack_start(body, true, true, 0);
    v.pack_start(&inner, true, true, 0);
    { let (inner, arrow) = (inner.clone(), arrow.clone()); eb.connect_button_press_event(move |_, _| {
        let vis = !inner.is_visible(); inner.set_visible(vis); arrow.set_text(if vis { "▴" } else { "▾" }); glib::Propagation::Stop
    }); }
    v
}

/// Computer Management (compmgmt.msc): System Tools, Storage, Services and Applications.
pub fn compmgmt(app: &gtk::Application) -> gtk::Window {
    let m = Mmc::new(app, "Computer Management", &["aerode-compmgmt", "computer"], true, true);
    let root = m.add(None, "cm", "Computer Management (Local)", &["aerode-compmgmt", "computer"]);
    let tools = m.add(Some(&root), "cm:tools", "System Tools", &["aerode-systools", "applications-system"]);
    super::taskschd::attach(&m, Some(&tools));
    super::eventvwr::attach(&m, Some(&tools));
    let shares = m.add(Some(&tools), "cm:shares", "Shared Folders", &["folder-publicshare"]);
    let lug = m.add(Some(&tools), "cm:lug", "Local Users and Groups", &["system-users"]);
    m.add(Some(&lug), "cm:lug:users", "Users", &["folder"]);
    m.add(Some(&lug), "cm:lug:groups", "Groups", &["folder"]);
    m.add(Some(&tools), "cm:perf", "Performance", &["utilities-system-monitor-perf", "utilities-system-monitor"]);
    super::devmgr::attach(&m, Some(&tools));
    let storage = m.add(Some(&root), "cm:storage", "Storage", &["aerode-diskmgmt", "drive-harddisk"]);
    super::diskmgmt::attach(&m, Some(&storage));
    let sa = m.add(Some(&root), "cm:sa", "Services and Applications", &["aerode-services", "applications-system"]);
    m.add(Some(&sa), "cm:services", "Services", &["aerode-services", "applications-system"]);
    let _ = shares;
    m.handle("cm", |m, id| {
        let rows: Vec<(&str, &str)> = match id {
            "cm" => vec![("System Tools", "cm:tools"), ("Storage", "cm:storage"), ("Services and Applications", "cm:sa")],
            "cm:tools" => vec![("Task Scheduler", "ts"), ("Event Viewer", "ev"), ("Shared Folders", "cm:shares"), ("Local Users and Groups", "cm:lug"), ("Performance", "cm:perf"), ("Device Manager", "dm")],
            "cm:storage" => vec![("Disk Management", "dk")],
            "cm:sa" => vec![("Services", "cm:services")],
            "cm:lug" => vec![("Users", "cm:lug:users"), ("Groups", "cm:lug:groups")],
            _ => vec![],
        };
        let title = match id { "cm" => "Computer Management (Local)", "cm:tools" => "System Tools", "cm:storage" => "Storage", "cm:sa" => "Services and Applications", "cm:lug" => "Local Users and Groups", _ => "" };
        if !rows.is_empty() {
            m.set_center(&folder_list(m, &rows));
            m.set_actions(vec![(title.to_string(), vec![act_off("More Actions ▸")])]);
            return;
        }
        match id {
            "cm:lug:users" => users_list(m),
            "cm:lug:groups" => groups_list(m),
            "cm:shares" => shares_list(m),
            "cm:perf" => {
                let v = vbox(10);
                v.set_border_width(16);
                v.pack_start(&bold("Performance Monitor"), false, false, 0);
                v.pack_start(&label("Watch processor, memory, disk and network use as it happens in Task Manager's Performance tab.", ""), false, false, 0);
                let b = gtk::Button::with_label("Open Performance Monitor");
                b.set_halign(gtk::Align::Start);
                b.connect_clicked(|_| { util::spawn(&["aerode-taskmgr"]); });
                v.pack_start(&b, false, false, 0);
                m.set_center(&v);
                m.set_actions(vec![("Performance".into(), vec![act("Open Performance Monitor", &["utilities-system-monitor-perf", "utilities-system-monitor"], || { util::spawn(&["aerode-taskmgr"]); })])]);
            }
            "cm:services" => services_list(m),
            _ => {}
        }
    });
    m.show("cm");
    m.win.clone()
}

/// A result pane listing the child nodes (what MMC shows for a folder node).
fn folder_list(m: &Rc<Mmc>, rows: &[(&str, &str)]) -> gtk::Widget {
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING]);
    for (name, id) in rows {
        // reuse the tree's icon for the node
        let mut pb: Option<Pixbuf> = None;
        m.store.foreach(|md, _, it| { if sval(md, it, 2) == *id { pb = md.value(it, 0).get().ok(); true } else { false } });
        store.insert_with_values(None, &[(0, &pb), (1, name), (2, id)]);
    }
    let tv = gtk::TreeView::with_model(&store);
    icon_column(&tv, "Name", 0, 1, 0);
    let me = Rc::downgrade(m);
    tv.connect_row_activated(move |tv, path, _| {
        let (Some(m), Some(model)) = (me.upgrade(), tv.model()) else { return };
        if let Some(it) = model.iter(path) { let id = sval(&model, &it, 2); m.select(&id); }
    });
    scrolled(&tv).upcast()
}

fn simple_table(cols: &[(&str, i32)], rows: Vec<Vec<String>>, icon: &[&str]) -> (gtk::TreeView, gtk::ListStore) {
    let mut types = vec![Pixbuf::static_type()];
    types.extend(std::iter::repeat(glib::Type::STRING).take(cols.len()));
    let store = gtk::ListStore::new(&types);
    let pb = pix(icon, 16);
    for r in rows {
        let it = store.append();
        store.set_value(&it, 0, &pb.to_value());
        for (i, v) in r.iter().enumerate().take(cols.len()) { store.set_value(&it, i as u32 + 1, &v.to_value()); }
    }
    let tv = gtk::TreeView::with_model(&store);
    icon_column(&tv, cols[0].0, 0, 1, cols[0].1);
    let rest: Vec<(&str, i32, i32)> = cols.iter().enumerate().skip(1).map(|(i, (t, w))| (*t, i as i32 + 1, *w)).collect();
    columns(&tv, &rest);
    (tv, store)
}

fn users_list(m: &Rc<Mmc>) {
    let mut rows = vec![];
    for l in std::fs::read_to_string("/etc/passwd").unwrap_or_default().lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() < 7 { continue; }
        let uid: u32 = f[2].parse().unwrap_or(0);
        let shell = f[6];
        let login = !(shell.ends_with("nologin") || shell.ends_with("false"));
        if uid != 0 && !(1000..60000).contains(&uid) && !login { continue; }
        let full = f[4].split(',').next().unwrap_or("").to_string();
        let desc = if uid == 0 { "Built-in account for administering the computer".to_string() } else if login { format!("Home folder {}", f[5]) } else { "System account".into() };
        rows.push(vec![f[0].to_string(), full, desc]);
    }
    let (tv, _) = simple_table(&[("Name", 160), ("Full Name", 180), ("Description", 0)], rows, &["avatar-default", "system-users"]);
    m.set_center(&scrolled(&tv));
    let me = m.clone();
    m.set_actions(vec![("Users".into(), vec![act_off("More Actions ▸")]), ("Selected Items".into(), vec![act("Manage user accounts", &["system-users"], move || { let _ = &me; util::spawn(&["aerode-control", "users"]); })])]);
}

fn groups_list(m: &Rc<Mmc>) {
    let mut rows = vec![];
    for l in std::fs::read_to_string("/etc/group").unwrap_or_default().lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() < 4 { continue; }
        let members = f[3].replace(',', ", ");
        let desc = match f[0] {
            "wheel" => "Administrators: members can run administrative tasks (sudo, polkit)".into(),
            "users" => "Users".into(),
            "audio" | "video" | "input" | "storage" | "optical" | "network" | "lp" | "scanner" => format!("Direct access to {} devices", f[0]),
            _ => String::new(),
        };
        rows.push(vec![f[0].to_string(), if members.is_empty() { desc } else if desc.is_empty() { format!("Members: {members}") } else { format!("{desc} — Members: {members}") }]);
    }
    let (tv, _) = simple_table(&[("Name", 200), ("Description", 0)], rows, &["system-users"]);
    m.set_center(&scrolled(&tv));
    m.set_actions(vec![("Groups".into(), vec![act_off("More Actions ▸")])]);
}

fn shares_list(m: &Rc<Mmc>) {
    // Samba shares from smb.conf and `net usershare`
    let mut rows = vec![];
    let conf = std::fs::read_to_string("/etc/samba/smb.conf").unwrap_or_default();
    let mut cur: Option<(String, String, String)> = None;
    let flush = |c: Option<(String, String, String)>, rows: &mut Vec<Vec<String>>| if let Some((n, p, d)) = c { if n != "global" { rows.push(vec![n, p, "Windows".into(), d]); } };
    for l in conf.lines().map(|l| l.trim()) {
        if l.starts_with('#') || l.starts_with(';') { continue; }
        if l.starts_with('[') && l.ends_with(']') { flush(cur.take(), &mut rows); cur = Some((l[1..l.len() - 1].to_string(), String::new(), String::new())); continue; }
        if let (Some(c), Some((k, v))) = (cur.as_mut(), l.split_once('=')) {
            match k.trim().to_lowercase().as_str() { "path" => c.1 = v.trim().into(), "comment" => c.2 = v.trim().into(), _ => {} }
        }
    }
    flush(cur.take(), &mut rows);
    let (tv, store) = simple_table(&[("Share Name", 140), ("Folder Path", 220), ("Type", 80), ("Description", 0)], rows, &["folder-publicshare"]);
    m.set_center(&scrolled(&tv));
    m.set_actions(vec![("Shares".into(), vec![act_off("More Actions ▸")])]);
    bg(|| sh(&["net", "usershare", "info"]), move |out| {
        let pb = pix(&["folder-publicshare"], 16);
        let mut name = String::new();
        let (mut path, mut comment) = (String::new(), String::new());
        let add = |n: &str, p: &str, c: &str| { if !n.is_empty() { store.insert_with_values(None, &[(0, &pb), (1, &n), (2, &p), (3, &"Windows"), (4, &c)]); } };
        for l in out.lines() {
            if l.starts_with('[') { add(&name, &path, &comment); name = l.trim_matches(['[', ']']).into(); path.clear(); comment.clear(); }
            else if let Some(v) = l.strip_prefix("path=") { path = v.into(); }
            else if let Some(v) = l.strip_prefix("comment=") { comment = v.into(); }
        }
        add(&name, &path, &comment);
    });
}

fn services_list(m: &Rc<Mmc>) {
    let (h, extra) = head("Services", "Loading…");
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let tv = gtk::TreeView::with_model(&store);
    icon_column(&tv, "Name", 0, 1, 220);
    columns(&tv, &[("Description", 2, 300), ("Status", 3, 80), ("Startup Type", 4, 100), ("Log On As", 5, 0)]);
    let v = vbox(0);
    v.pack_start(&h, false, false, 0);
    v.pack_start(&scrolled(&tv), true, true, 0);
    m.set_center(&v);
    m.set_actions(vec![("Services".into(), vec![act("Open Services", &["aerode-services", "applications-system"], || spawn_tool("services"))])]);
    bg(|| {
        let units = sh(&["systemctl", "list-units", "--type=service", "--all", "--no-legend", "--plain", "--no-pager"]);
        let files = sh(&["systemctl", "list-unit-files", "--type=service", "--no-legend", "--plain", "--no-pager"]);
        (units, files)
    }, move |(units, files)| {
        let state: std::collections::HashMap<&str, &str> = files.lines().filter_map(|l| { let mut it = l.split_whitespace(); Some((it.next()?, it.next()?)) }).collect();
        let pb = pix(&["aerode-services", "applications-system"], 16);
        let mut n = 0;
        for l in units.lines() {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 { continue; }
            let st = match state.get(f[0]).copied().unwrap_or("static") { "enabled" => "Automatic", "disabled" => "Disabled", "masked" => "Disabled", _ => "Manual" };
            store.insert_with_values(None, &[(0, &pb), (1, &f[0].trim_end_matches(".service")), (2, &f[4..].join(" ")), (3, &if f[3] == "running" { "Started" } else { "" }), (4, &st), (5, &"Local System")]);
            n += 1;
        }
        extra.set_text(&format!("{n} services"));
    });
}
