//! Control Panel – Windows 7 Control Panel remake backed by Linux tools.
//!   aerode-control [page]   pages: system display personalization sound datetime power mouse keyboard
//!                          programs users network defaults devices region access startup services
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use aerode::{theme, util};

mod pages;
mod registry;

const CSS: &str = r#"
.cp-main { background-color: #ffffff; }
.cp-title { font-size: 12pt; color: #1e3287; }
.cp-cat { color: #157b14; font-size: 11pt; background: none; border: none; box-shadow: none; padding: 0; }
.cp-cat:hover { color: #1fa41e; text-decoration: underline; }
button.cp-link { color: #0066cc; background: none; border: none; box-shadow: none; padding: 1px 0; min-height: 0; }
button.cp-link:hover { color: #3399ff; }
button.cp-link label { color: #0066cc; }
.cp-sidebar { background-image: linear-gradient(to bottom, #e6f0fb, #cfdff4 60%, #e7eff9); border-right: 1px solid #c4d4ea; padding: 14px 10px; }
.cp-sidebar label.side-h { color: #1e3287; }
.cp-header { padding: 4px 6px 5px 6px; }
.cp-addr { background-color: rgba(255,255,255,0.92); border: 1px solid rgba(0,0,0,0.55); border-radius: 2px; min-height: 22px; padding: 0 6px; }
.cp-addr label { color: #000; }
.cp-section { color: #1e3287; font-size: 10pt; }
"#;

struct Cp {
    win: gtk::ApplicationWindow,
    side: gtk::Box,
    body: gtk::Box,
    crumb: gtk::Label,
    history: RefCell<Vec<String>>,
}

fn link(text: &str, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.style_context().add_class("cp-link");
    b.set_halign(gtk::Align::Start);
    b.set_relief(gtk::ReliefStyle::None);
    if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_xalign(0.0); l.set_line_wrap(true); }
    b.connect_clicked(move |_| f());
    b
}

fn title(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.style_context().add_class("cp-title");
    l.set_margin_bottom(10);
    l
}

fn section(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.style_context().add_class("cp-section");
    l.set_margin_top(10);
    l
}

fn text(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(80);
    l
}

fn kv_grid(rows: &[(&str, String)]) -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(24);
    g.set_margin_start(16);
    for (i, (k, v)) in rows.iter().enumerate() {
        let a = gtk::Label::new(Some(k));
        a.set_xalign(0.0);
        a.style_context().add_class("dim-label");
        let b = gtk::Label::new(Some(v));
        b.set_xalign(0.0);
        b.set_selectable(true);
        b.set_line_wrap(true);
        g.attach(&a, 0, i as i32, 1, 1);
        g.attach(&b, 1, i as i32, 1, 1);
    }
    g
}

fn out(argv: &[&str]) -> String { util::output(argv).unwrap_or_default() }

/// Win7 "Text Services and Input Languages": default input language, installed keyboards
/// (Add / Remove / Move Up / Move Down), the hot key between input languages, Apply.
fn input_languages() -> gtk::Widget {
    use aerode::kbdlayout::{self, Layout};
    let (cur, opts) = kbdlayout::current();
    let list: Rc<RefCell<Vec<Layout>>> = Rc::new(RefCell::new(cur));
    let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
    v.set_margin_start(12);
    let name = |l: &Layout| {
        let (_, lang) = kbdlayout::language(l);
        format!("{lang} - {}", kbdlayout::describe(&l.layout, &l.variant))
    };
    // default input language = the first layout
    let defl = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    defl.pack_start(&gtk::Label::new(Some("Default input language:")), false, false, 0);
    let default_combo = gtk::ComboBoxText::new();
    defl.pack_start(&default_combo, true, true, 0);
    v.pack_start(&defl, false, false, 0);
    v.pack_start(&text("Installed services – the keyboards you can switch between:"), false, false, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let lb = gtk::ListBox::new();
    lb.style_context().add_class("frame");
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_size_request(380, 130);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&lb);
    row.pack_start(&sw, true, true, 0);
    let btns = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let (add, remove, up, down) = (gtk::Button::with_mnemonic("A_dd..."), gtk::Button::with_mnemonic("_Remove"), gtk::Button::with_mnemonic("Move _Up"), gtk::Button::with_mnemonic("Move Dow_n"));
    for b in [&add, &remove, &up, &down] { btns.pack_start(b, false, false, 0); }
    row.pack_start(&btns, false, false, 0);
    v.pack_start(&row, false, false, 0);
    let hk = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    hk.pack_start(&gtk::Label::new(Some("Hot key to switch between input languages:")), false, false, 0);
    let hot = gtk::ComboBoxText::new();
    let cur_hot = aerode::config::get().kbd_switch.unwrap_or_else(|| { let h = kbdlayout::hotkey(&opts); if h.is_empty() { kbdlayout::DEFAULT_SWITCH.to_string() } else { h } });
    for (label, opt) in kbdlayout::HOTKEYS { hot.append(Some(opt), label); }
    if !hot.set_active_id(Some(&cur_hot)) { if cur_hot.is_empty() { hot.set_active_id(Some("")); } else { hot.append(Some(&cur_hot), &cur_hot); hot.set_active_id(Some(&cur_hot)); } }
    hk.pack_start(&hot, false, false, 0);
    v.pack_start(&hk, false, false, 0);
    let apply = gtk::Button::with_mnemonic("_Apply");
    apply.set_halign(gtk::Align::End);
    v.pack_start(&apply, false, false, 0);

    let refresh: Rc<dyn Fn()> = {
        let (list, lb, default_combo) = (list.clone(), lb.clone(), default_combo.clone());
        Rc::new(move || {
            for c in lb.children() { lb.remove(&c); }
            default_combo.remove_all();
            for l in list.borrow().iter() {
                let (abbr, _) = kbdlayout::language(l);
                let r = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let a = gtk::Label::new(Some(&abbr));
                a.style_context().add_class("dim-label");
                a.set_width_chars(3);
                r.pack_start(&a, false, false, 4);
                let t = gtk::Label::new(Some(&name(l)));
                t.set_xalign(0.0);
                r.pack_start(&t, true, true, 0);
                lb.add(&r);
                default_combo.append_text(&kbdlayout::language(l).1);
            }
            default_combo.set_active(Some(0));
            lb.show_all();
        })
    };
    refresh();
    let selected = { let lb = lb.clone(); move || lb.selected_row().map(|r| r.index() as usize) };
    { let (list, refresh, lb) = (list.clone(), refresh.clone(), lb.clone()); default_combo.connect_changed(move |c| {
        let Some(i) = c.active().map(|i| i as usize) else { return };
        if i == 0 { return; }
        { let mut l = list.borrow_mut(); if i < l.len() { let x = l.remove(i); l.insert(0, x); } }
        refresh();
        if let Some(r) = lb.row_at_index(0) { lb.select_row(Some(&r)); }
    }); }
    { let (list, refresh, sel) = (list.clone(), refresh.clone(), selected.clone()); remove.connect_clicked(move |_| {
        if let Some(i) = sel() { let mut l = list.borrow_mut(); if l.len() > 1 { l.remove(i); } }
        refresh();
    }); }
    for (b, dir) in [(&up, -1i32), (&down, 1)] {
        let (list, refresh, sel, lb) = (list.clone(), refresh.clone(), selected.clone(), lb.clone());
        b.connect_clicked(move |_| {
            let Some(i) = sel() else { return };
            let j = i as i32 + dir;
            { let mut l = list.borrow_mut(); if j < 0 || j as usize >= l.len() { return; } l.swap(i, j as usize); }
            refresh();
            if let Some(r) = lb.row_at_index(j) { lb.select_row(Some(&r)); }
        });
    }
    { let (list, refresh) = (list.clone(), refresh.clone()); add.connect_clicked(move |b| {
        if let Some(l) = add_input_language(b.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()).as_ref()) {
            if !list.borrow().contains(&l) { list.borrow_mut().push(l); }
            refresh();
        }
    }); }
    { let (list, hot) = (list.clone(), hot.clone()); apply.connect_clicked(move |_| {
        kbdlayout::apply(&list.borrow(), hot.active_id().as_deref().unwrap_or(""));
    }); }
    v.upcast()
}

/// "Add Input Language": every XKB layout with its variants, searchable.
fn add_input_language(parent: Option<&gtk::Window>) -> Option<aerode::kbdlayout::Layout> {
    let (layouts, variants) = aerode::kbdlayout::catalog();
    let d = gtk::Dialog::with_buttons(Some("Add Input Language"), parent, gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(420, 460);
    let c = d.content_area();
    c.set_spacing(6);
    c.set_border_width(10);
    c.pack_start(&text("Select the language and keyboard layout to add."), false, false, 0);
    let search = gtk::SearchEntry::new();
    c.pack_start(&search, false, false, 0);
    // columns: description, layout, variant
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    let mut sorted = layouts.clone();
    sorted.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    for (code, desc) in &sorted {
        let it = store.insert_with_values(None, None, &[(0, desc), (1, code), (2, &String::new())]);
        for (_, var, vdesc) in variants.iter().filter(|(l, _, _)| l == code) {
            store.insert_with_values(Some(&it), None, &[(0, vdesc), (1, code), (2, var)]);
        }
    }
    let filter = gtk::TreeModelFilter::new(&store, None);
    {
        let search = search.clone();
        filter.set_visible_func(move |m, it| {
            let q = search.text().to_lowercase();
            if q.is_empty() { return true; }
            let hit = |it: &gtk::TreeIter| m.value(it, 0).get::<String>().unwrap_or_default().to_lowercase().contains(&q)
                || m.value(it, 1).get::<String>().unwrap_or_default().to_lowercase() == q;
            if hit(it) { return true; }
            // keep a layout row when one of its variants matches
            if let Some(ch) = m.iter_children(Some(it)) { loop { if hit(&ch) { return true; } if !m.iter_next(&ch) { break; } } }
            false
        });
    }
    { let f = filter.clone(); search.connect_search_changed(move |_| f.refilter()); }
    let tv = gtk::TreeView::with_model(&filter);
    tv.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let cell = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &cell, true);
    TreeViewColumnExt::add_attribute(&col, &cell, "text", 0);
    tv.append_column(&col);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.add(&tv);
    c.pack_start(&sw, true, true, 0);
    { let d = d.clone(); tv.connect_row_activated(move |_, _, _| d.response(gtk::ResponseType::Ok)); }
    d.show_all();
    let r = d.run();
    let pick = tv.selection().selected().map(|(m, it)| aerode::kbdlayout::Layout {
        layout: m.value(&it, 1).get::<String>().unwrap_or_default(),
        variant: m.value(&it, 2).get::<String>().unwrap_or_default(),
    });
    d.close();
    if r == gtk::ResponseType::Ok { pick.filter(|l| !l.layout.is_empty()) } else { None }
}

/// Run a privileged command through polkit (shows our UAC prompt).
fn admin(argv: &[&str]) -> bool {
    let mut a = vec!["pkexec"];
    a.extend_from_slice(argv);
    std::process::Command::new(a[0]).args(&a[1..]).status().map(|s| s.success()).unwrap_or(false)
}

impl Cp {
    fn open(self: &Rc<Self>, page: &str) {
        for c in self.body.children() { self.body.remove(&c); }
        for c in self.side.children() { self.side.remove(&c); }
        self.history.borrow_mut().push(page.to_string());
        let all = registry::applets();
        let (crumb, w): (String, gtk::Widget) = if page == "personalization" {
            util::spawn(&["aerode", "personalize"]);
            ("Appearance and Personalization".into(), self.page_category("appearance"))
        } else if let Some(c) = page.strip_prefix("cat:") {
            (registry::category_name(c).into(), self.page_category(c))
        } else if page == "all" || page == "all-small" {
            ("All Control Panel Items".into(), self.page_all(page == "all-small"))
        } else if let Some(q) = page.strip_prefix("search:") {
            (format!("Search Results for \"{q}\""), self.page_search(q))
        } else if let Some(a) = all.iter().find(|a| a.id == page) {
            (format!("{} ▸ {}", registry::category_name(a.cat), a.name), (a.build)(self))
        } else {
            ("All Control Panel Items".into(), self.page_home())
        };
        self.crumb.set_text(&format!("Control Panel ▸ {crumb}"));
        self.win.set_title(crumb.rsplit(" ▸ ").next().unwrap_or("Control Panel"));
        if page.is_empty() || page == "home" { self.win.set_title("Control Panel"); self.crumb.set_text("Control Panel ▸"); }
        // the left task pane (Win7 shows "Control Panel Home" on every sub page)
        if !(page.is_empty() || page == "home") {
            let me = self.clone();
            let home = link("Control Panel Home", move || me.open("home"));
            self.side.pack_start(&home, false, false, 0);
            // above the page's own task links
            self.side.reorder_child(&home, 0);
            self.side.show_all();
            self.side.set_visible(true);
        } else { self.side.set_visible(false); }
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        let pad = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pad.set_border_width(18);
        pad.pack_start(&w, true, true, 0);
        sw.add(&pad);
        self.body.pack_start(&sw, true, true, 0);
        self.body.show_all();
    }

    fn side_link(self: &Rc<Self>, t: &str, page: &'static str) {
        let me = self.clone();
        self.side.pack_start(&link(t, move || me.open(page)), false, false, 2);
    }

    // ----------------------------------------------------------------- home
    fn page_home(self: &Rc<Self>) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.pack_start(&title("Adjust your computer's settings"), false, false, 0);
        top.pack_end(&self.view_by("category"), false, false, 0);
        v.pack_start(&top, false, false, 0);
        let grid = gtk::Grid::new();
        grid.set_column_spacing(40);
        grid.set_row_spacing(18);
        type L = (&'static str, &'static str);
        let cats: [(&str, &str, &str, &[L]); 8] = [
            ("System and Security", "security-high", "system", &[("Review your computer's status", "system"), ("Startup programs", "startup"), ("Find and fix problems", "services")]),
            ("User Accounts and Family Safety", "user-home", "users", &[("Add or remove user accounts", "users"), ("Change your account picture", "users")]),
            ("Network and Internet", "network-workgroup", "network", &[("View network status and tasks", "network"), ("Choose homegroup and sharing options", "network")]),
            ("Appearance and Personalization", "preferences-desktop-wallpaper", "appearance", &[("Change the theme", "personalization"), ("Change desktop background", "personalization"), ("Adjust screen resolution", "display")]),
            ("Hardware and Sound", "printer", "hardware", &[("View devices and printers", "devices"), ("Add a device", "devices")]),
            ("Clock, Language, and Region", "preferences-system-time", "clock", &[("Change keyboards or other input methods", "region")]),
            ("Programs", "system-software-install", "programs", &[("Uninstall a program", "programs")]),
            ("Ease of Access", "preferences-desktop-accessibility", "access", &[("Let Windows suggest settings", "access"), ("Optimize visual display", "access")]),
        ];
        for (i, (name, icon, cat, links)) in cats.iter().enumerate() {
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            h.pack_start(&util::image(icon, 48), false, false, 0);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let t = gtk::Button::with_label(name);
            t.style_context().add_class("cp-cat");
            t.set_halign(gtk::Align::Start);
            t.set_relief(gtk::ReliefStyle::None);
            let (me, c) = (self.clone(), format!("cat:{cat}"));
            t.connect_clicked(move |_| me.open(&c));
            col.pack_start(&t, false, false, 0);
            for (l, p) in links.iter() { let (me, p) = (self.clone(), *p); col.pack_start(&link(l, move || me.open(p)), false, false, 0); }
            h.pack_start(&col, false, false, 0);
            grid.attach(&h, (i % 2) as i32, (i / 2) as i32, 1, 1);
        }
        v.pack_start(&grid, false, false, 0);
        v.upcast()
    }

    fn page_category(self: &Rc<Self>, cat: &str) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.pack_end(&self.view_by("category"), false, false, 0);
        v.pack_start(&top, false, false, 0);
        for a in registry::applets().into_iter().filter(|a| a.cat == cat) {
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            h.pack_start(&util::image(a.icon, 48), false, false, 0);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 1);
            let t = gtk::Button::with_label(a.name);
            t.style_context().add_class("cp-cat");
            t.set_halign(gtk::Align::Start);
            t.set_relief(gtk::ReliefStyle::None);
            { let (me, id) = (self.clone(), a.id); t.connect_clicked(move |_| me.open(id)); }
            col.pack_start(&t, false, false, 0);
            let tasks = gtk::FlowBox::new();
            tasks.set_selection_mode(gtk::SelectionMode::None);
            tasks.set_max_children_per_line(3);
            tasks.set_column_spacing(14);
            for (l, p) in a.tasks.iter() { let (me, p) = (self.clone(), *p); tasks.add(&link(l, move || me.open(p))); }
            col.pack_start(&tasks, false, false, 0);
            h.pack_start(&col, true, true, 0);
            v.pack_start(&h, false, false, 0);
        }
        v.upcast()
    }

    /// Win7 "View by: Category / Large icons / Small icons" in the top right.
    fn view_by(self: &Rc<Self>, current: &str) -> gtk::Widget {
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        h.pack_start(&gtk::Label::new(Some("View by:")), false, false, 0);
        let c = gtk::ComboBoxText::new();
        for (id, l) in [("category", "Category"), ("all", "Large icons"), ("all-small", "Small icons")] { c.append(Some(id), l); }
        c.set_active_id(Some(current));
        let me = self.clone();
        c.connect_changed(move |c| if let Some(id) = c.active_id() { me.open(if id == "category" { "home" } else { id.as_str() }); });
        h.pack_start(&c, false, false, 0);
        h.upcast()
    }

    /// "All Control Panel Items": every applet by name, large (32 px) or small (16 px) icons.
    fn page_all(self: &Rc<Self>, small: bool) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.pack_start(&title("Adjust your computer's settings"), false, false, 0);
        top.pack_end(&self.view_by(if small { "all-small" } else { "all" }), false, false, 0);
        v.pack_start(&top, false, false, 0);
        let mut all = registry::applets();
        all.sort_by(|a, b| a.name.cmp(b.name));
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_homogeneous(true);
        flow.set_min_children_per_line(if small { 4 } else { 3 });
        flow.set_max_children_per_line(if small { 4 } else { 3 });
        flow.set_row_spacing(if small { 2 } else { 8 });
        for a in all {
            let b = gtk::Button::new();
            b.set_relief(gtk::ReliefStyle::None);
            b.style_context().add_class("cp-item");
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            h.pack_start(&util::image(a.icon, if small { 16 } else { 32 }), false, false, 0);
            let l = gtk::Label::new(Some(a.name));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            h.pack_start(&l, true, true, 0);
            b.add(&h);
            let (me, id) = (self.clone(), a.id);
            b.connect_clicked(move |_| me.open(id));
            flow.add(&b);
        }
        v.pack_start(&flow, false, false, 0);
        v.upcast()
    }

    /// Search: every applet and task whose name or keywords match.
    fn page_search(self: &Rc<Self>, q: &str) -> gtk::Widget {
        let v = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let q = q.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut n = 0;
        for a in registry::applets() {
            let hay = format!("{} {} {}", a.name.to_lowercase(), a.keywords, a.tasks.iter().map(|t| t.0.to_lowercase()).collect::<Vec<_>>().join(" "));
            if words.is_empty() || !words.iter().all(|w| hay.contains(w)) { continue; }
            n += 1;
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            h.pack_start(&util::image(a.icon, 32), false, false, 0);
            let col = gtk::Box::new(gtk::Orientation::Vertical, 1);
            { let (me, id) = (self.clone(), a.id); let t = link(a.name, move || me.open(id)); t.style_context().add_class("cp-cat"); col.pack_start(&t, false, false, 0); }
            for (l, p) in a.tasks.iter().filter(|t| words.iter().any(|w| t.0.to_lowercase().contains(w)) || words.iter().any(|w| a.keywords.contains(w))) {
                let (me, p) = (self.clone(), *p);
                col.pack_start(&link(l, move || me.open(p)), false, false, 0);
            }
            h.pack_start(&col, true, true, 0);
            v.pack_start(&h, false, false, 0);
        }
        if n == 0 { v.pack_start(&text("No items match your search."), false, false, 0); }
        v.upcast()
    }

    // ----------------------------------------------------------------- system
}

fn build(app: &gtk::Application, page: String) {
    theme::init_app();
    theme::load_css(aerode::explorer::CSS);
    theme::load_css(CSS);
    // Event Viewer, Device Manager, Disk Management… have their own window, like MMC consoles
    if pages::admin::standalone(app, &page) { return; }
    let win = gtk::ApplicationWindow::new(app);
    win.set_default_size(880, 620);
    win.set_icon_name(Some(util::first_icon(&["preferences-system"])));
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    // glass header: back/forward + breadcrumb + search
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.style_context().add_class("cp-header");
    header.style_context().add_class("exp-header");
    let back = gtk::Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
    back.style_context().add_class("nav");
    let fwd = gtk::Button::from_icon_name(Some("go-next-symbolic"), gtk::IconSize::Button);
    fwd.style_context().add_class("nav");
    fwd.set_sensitive(false);
    header.pack_start(&back, false, false, 0);
    header.pack_start(&fwd, false, false, 0);
    let addr = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    addr.style_context().add_class("cp-addr");
    addr.pack_start(&util::image("preferences-system", 16), false, false, 0);
    let crumb = gtk::Label::new(Some("Control Panel ▸"));
    crumb.set_xalign(0.0);
    addr.pack_start(&crumb, true, true, 0);
    header.pack_start(&addr, true, true, 0);
    let search = gtk::Entry::new();
    search.set_placeholder_text(Some("Search Control Panel"));
    search.style_context().add_class("exp-search");
    search.set_icon_from_icon_name(gtk::EntryIconPosition::Secondary, Some("system-search-symbolic"));
    header.pack_end(&search, false, false, 0);
    // the frame's glass continued into the window, translucent under a compositor; redrawn when
    // Personalize changes the window colour (enable_rgba) and when the window gains/loses focus
    aerode::glass::enable_rgba(win.upcast_ref());
    header.connect_draw(|h, cr| {
        aerode::glass::paint_widget_glass(h, cr);
        glib::Propagation::Proceed
    });
    {
        let h = header.clone();
        win.connect_window_state_event(move |_, e| {
            if e.changed_mask().intersects(gdk::WindowState::FOCUSED | gdk::WindowState::MAXIMIZED) { h.queue_draw(); }
            glib::Propagation::Proceed
        });
    }
    { let h = header.clone(); win.connect_configure_event(move |_, _| { h.queue_draw(); false }); }
    root.pack_start(&header, false, false, 0);
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    main.style_context().add_class("cp-main");
    let side = gtk::Box::new(gtk::Orientation::Vertical, 4);
    side.style_context().add_class("cp-sidebar");
    side.set_size_request(200, -1);
    main.pack_start(&side, false, false, 0);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.pack_start(&body, true, true, 0);
    root.pack_start(&main, true, true, 0);
    win.add(&root);
    let cp = Rc::new(Cp { win: win.clone(), side, body, crumb, history: RefCell::new(vec![]) });
    { let c = cp.clone(); back.connect_clicked(move |_| {
        let prev = { let mut h = c.history.borrow_mut(); h.pop(); h.pop() };
        c.open(&prev.unwrap_or_default());
    }); }
    {
        // search: results page that follows what you type (Win7 searches as you type)
        let c = cp.clone();
        search.connect_changed(move |s| {
            let q = s.text().trim().to_string();
            if q.is_empty() { return; }
            if c.history.borrow().last().map(|h| h.starts_with("search:")).unwrap_or(false) { c.history.borrow_mut().pop(); }
            c.open(&format!("search:{q}"));
        });
    }
    root.show_all();
    cp.open(&page);
    win.show();
    let _ = gdk::Display::default();
}

fn main() -> glib::ExitCode {
    let page = std::env::args().nth(1).unwrap_or_default();
    if page == "--action-center-summary" { return action_center_summary(); }
    let app = gtk::Application::new(Some("org.aerode.ControlPanel"), gio::ApplicationFlags::NON_UNIQUE);
    app.connect_activate(move |app| build(app, page.clone()));
    app.run_with_args::<&str>(&[])
}

/// `aerode-control --action-center-summary`: Action Center's current messages, one per line
/// (`important<TAB>kind<TAB>title<TAB>text`), for the flag in the notification area.
fn action_center_summary() -> glib::ExitCode {
    if gtk::init().is_err() { return glib::ExitCode::FAILURE; }
    let app = gtk::Application::new(None, gio::ApplicationFlags::NON_UNIQUE);
    let _ = app.register(None::<&gio::Cancellable>);
    // the messages' buttons want a Control Panel to open pages in; this one is never shown
    let cp = Rc::new(Cp {
        win: gtk::ApplicationWindow::new(&app), side: gtk::Box::new(gtk::Orientation::Vertical, 0),
        body: gtk::Box::new(gtk::Orientation::Vertical, 0), crumb: gtk::Label::new(None), history: RefCell::new(vec![]),
    });
    let s = pages::admin::actioncenter::gather();
    let (sec, mnt) = pages::admin::actioncenter::messages(&cp, &s, Rc::new(|| {}));
    let clean = |t: &str| t.replace(['\t', '\n'], " ");
    for m in sec.iter().chain(mnt.iter()) {
        println!("{}\t{}\t{}\t{}", m.red as u8, m.key, clean(m.title.trim_end_matches("  (Important)")), clean(&m.text));
    }
    glib::ExitCode::SUCCESS
}
