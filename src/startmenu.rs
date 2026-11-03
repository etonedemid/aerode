//! The Windows 7 start menu.
//!
//! Painted from the user's aero.msstyles (StartPanelComposited::StartPanelPriv / StartPanelPriv) when
//! it was extracted, on Windows 7's 96 dpi metrics as measured on a Win7 SP1 screenshot: 411 px wide,
//! the left column (frame + white pane) 261 px, program items 38 px high (10 slots by default), the
//! "All Programs" row 31 px, the search band 54 px, right-column links 31 px high on a 35 px pitch from
//! 45 px below the top, and the 64 px user tile (explorer.exe bitmap 7013) sticking 26 px out above the
//! menu. Without msstyles the same layout is drawn by hand.
//!
//! Frequent programs with a jump list (Getting Started's tasks, a program's recent files) get the
//! arrow of StartPanelPriv 5; resting on one for 400 ms opens the list in a white pane that takes
//! the right column's place, the menu widening to 526 px and the Shut down button switching to its
//! light-band look (StartPanelPriv 38/24), as on Win7.
use gtk::prelude::*;
use gtk::{cairo, gdk, gdk_pixbuf::Pixbuf, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{config, glass, shellstyle, sys, util};

thread_local! {
    static HIDE_ALL: RefCell<Option<Rc<dyn Fn()>>> = RefCell::new(None);
}

/// Hide the Start menu if it is open (used when clicking the desktop).
pub fn hide_all() { if let Some(f) = HIDE_ALL.with(|h| h.borrow().clone()) { f(); } }

/// Customize Start Menu items: (key, Win7 label, "link" = link / menu / don't display or "check", default)
pub const START_ITEMS: &[(&str, &str, &str, &str)] = &[
    ("computer", "Computer", "link", "link"),
    ("control", "Control Panel", "link", "link"),
    ("defaults", "Default Programs", "check", "on"),
    ("devices", "Devices and Printers", "check", "on"),
    ("documents", "Documents", "link", "link"),
    ("downloads", "Downloads", "link", "hide"),
    ("dragdrop", "Enable context menus and dragging and dropping", "check", "on"),
    ("games", "Games", "link", "link"),
    ("help", "Help", "check", "on"),
    ("highlight", "Highlight newly installed programs", "check", "on"),
    ("music", "Music", "link", "link"),
    ("network", "Network", "check", "off"),
    ("hoveropen", "Open submenus when I pause on them with the mouse pointer", "check", "on"),
    ("user", "Personal folder", "link", "link"),
    ("pictures", "Pictures", "link", "link"),
    ("recent", "Recent Items", "check", "off"),
    // off in a fresh Win7 install (Win+R still opens Run)
    ("run", "Run command", "check", "off"),
    ("searchcp", "Search programs and Control Panel", "check", "on"),
    ("sortname", "Sort All Programs menu by name", "check", "on"),
    ("largeicons", "Use large icons", "check", "on"),
    ("videos", "Videos", "link", "hide"),
];

/// The Customize Start Menu value of `key` ("link" | "menu" | "hide" | "on" | "off").
pub fn start_item(key: &str) -> String {
    config::get().taskbar.start_items.get(key).cloned()
        .unwrap_or_else(|| START_ITEMS.iter().find(|i| i.0 == key).map(|i| i.3.to_string()).unwrap_or_else(|| "on".into()))
}

/// Taskbar and Start Menu Properties > Power button action: (id, label), in the Win7 list order
pub const POWER_ACTIONS: &[(&str, &str)] = &[("switchuser", "Switch user"), ("logoff", "Log off"), ("lock", "Lock"), ("restart", "Restart"),
    ("sleep", "Sleep"), ("hibernate", "Hibernate"), ("shutdown", "Shut down")];

fn power_action(id: &str) -> fn() {
    match id {
        "switchuser" => || { util::spawn(&["dm-tool", "switch-to-greeter"]); },
        "logoff" => sys::log_off, "lock" => sys::lock, "restart" => sys::restart,
        "sleep" => sys::sleep, "hibernate" => sys::hibernate, _ => sys::shutdown,
    }
}

// ---- Win7 metrics (96 dpi), relative to the menu's top-left corner
/// menu width
const W: i32 = 411;
/// left column: frame edge + white pane up to its right border and highlight
const P: i32 = 261;
/// the user tile sticks out above the menu by this much (the window starts there)
const OV: i32 = 26;
/// user tile frame (explorer.exe bitmap 7013, 64×64) and the picture inside it
const TILE_X: i32 = P + 45;
const TILE: i32 = 64;
/// search band at the bottom of the left column (StartPanelPriv 18)
const BAND: i32 = 54;
/// "All Programs" row (StartPanelPriv 2)
const MORE: i32 = 31;
/// first program item
const LIST_TOP: i32 = 10;
/// program items: x and width inside the white pane
const ITEM_X: i32 = 11;
const ITEM_W: i32 = 246;
/// right column links
const LINK_TOP: i32 = 45;
const LINK_H: i32 = 31;
const LINK_PITCH: i32 = 35;
/// extra space a separator adds between two link groups
const SEP_EXTRA: i32 = 3;
/// StartPanelPriv 8: the separator above "All Programs", at its true size
const MORE_SEP_X: i32 = 20;

/// menu width while a program's jump list is open: Win7 widens the menu so that the list (a white
/// pane, StartPanelPriv 7 + 10, closed by 15 at the bottom) gets as much room as the program list
const W_JUMP: i32 = 526;
/// top of the jump list's content: under the user tile
const JUMP_TOP: i32 = 42;
/// hover time before an item opens (or another item closes) a jump list (SPI_GETMENUSHOWDELAY)
const JUMP_DELAY: u64 = 400;

/// The menu's msstyles class (the composited one: aerode always draws glass, real or faked).
const SP: &str = "StartPanelComposited::StartPanelPriv";

/// The left pane of a fresh install, in Win7's order where aerode has the program (Win7: Getting
/// Started, Windows Media Center, Calculator, Sticky Notes, Snipping Tool, Paint, Remote Desktop
/// Connection, Magnifier, Solitaire). Programs that get used move ahead of these.
pub const DEFAULT_PROGRAMS: &[&str] = &["aerode-gettingstarted.desktop", "aerode-calc.desktop", "aerode-notepad.desktop",
    "aerode-snip.desktop", "aerode-cmd.desktop", "aerode-taskmgr.desktop", "aerode-explorer.desktop"];

pub struct StartMenu {
    win: gtk::Window,
    fixed: gtk::Fixed,
    sw: gtk::ScrolledWindow,
    list: gtk::Box,
    all: gtk::Button,
    search: gtk::Entry,
    search_icon: gtk::Image,
    /// the picture in the user tile: the account picture, or the hovered link's icon
    avatar: RefCell<Option<Pixbuf>>,
    mode: Cell<u8>, // 0 frequent, 1 all programs, 2 search
    all_btn_label: gtk::Label,
    all_arrow: gtk::Image,
    expanded: RefCell<Vec<String>>,
    pub on_hide: RefCell<Option<Box<dyn Fn()>>>,
    pub on_show: RefCell<Option<Box<dyn Fn()>>>,
    hidden_at: Cell<Option<std::time::Instant>>,
    /// the menu got keyboard focus since it was last shown (see the window-state handler)
    armed: Cell<bool>,
    /// right-column links by Customize Start Menu key, in groups split by separators
    links: RefCell<Vec<Vec<(&'static str, gtk::Widget)>>>,
    power_btn: RefCell<Option<(gtk::Button, gtk::Button)>>,
    /// laid out: (height of the part above the search band, menu height)
    geom: Cell<(i32, i32)>,
    /// y of each right-column separator
    seps: RefCell<Vec<i32>>,
    /// the open jump list: (program id, its pane, the program's item)
    jump: RefCell<Option<(String, gtk::Box, gtk::Button)>>,
    /// pending open/close of a jump list after the hover delay
    jump_timer: RefCell<Option<glib::SourceId>>,
}

pub(crate) type Act = Rc<dyn Fn()>;

/// One entry of a Start menu jump list.
pub(crate) struct JumpItem {
    pub label: String,
    pub icon: Option<gio::Icon>,
    pub tip: Option<String>,
    pub act: Act,
}

/// Getting Started's jump list (Win7: Personalize Windows, Transfer your files, Share with a
/// homegroup, Back up your files, Add new users, Change the size of the text…): (label, icon, command)
const GETTING_STARTED_TASKS: &[(&str, &str, &[&str])] = &[
    ("Personalize Windows", "preferences-desktop-wallpaper", &["aerode", "personalize"]),
    ("Transfer your files", "folder-remote", &["aerode-explorer"]),
    ("Use a homegroup to share with other computers in your home", "folder-publicshare", &["aerode-control", "homegroup"]),
    ("Change keyboards or other input methods", "preferences-desktop-keyboard", &["aerode-control", "region"]),
    ("Back up your files", "drive-harddisk", &["aerode-control", "backup"]),
    ("Add new users to your computer", "system-users", &["aerode-control", "users"]),
    ("Change the size of the text on your screen", "preferences-desktop-display", &["aerode-control", "display"]),
];

/// The recently used files, newest first: (entry, programs that opened it (lower case), id of the
/// default program for its type).
pub(crate) type RecentIndex = Vec<(gtk::RecentInfo, Vec<String>, Option<String>)>;

pub(crate) fn recent_index() -> RecentIndex {
    let Some(rm) = gtk::RecentManager::default() else { return vec![] };
    let mut items = rm.items();
    items.sort_by_key(|i| std::cmp::Reverse(i.modified()));
    let mut defaults: std::collections::HashMap<String, Option<String>> = Default::default();
    items.into_iter().take(300).map(|i| {
        let apps = i.applications().iter().map(|a| a.to_lowercase()).collect();
        let def = i.mime_type().and_then(|m| defaults.entry(m.to_string())
            .or_insert_with(|| gio::AppInfo::default_for_type(&m, false).and_then(|d| d.id()).map(|d| d.to_string())).clone());
        (i, apps, def)
    }).collect()
}

/// A program's Start menu jump list: Getting Started's tasks, else the recently opened files the
/// program opened or is the default program for (Win7's "Recent" category), newest first.
pub(crate) fn jump_items(app: &gio::AppInfo, recent: &RecentIndex) -> (&'static str, Vec<JumpItem>) {
    let id = app.id().map(|s| s.to_string()).unwrap_or_default();
    if id == "aerode-gettingstarted.desktop" {
        let v = GETTING_STARTED_TASKS.iter().map(|(l, icon, cmd)| {
            let cmd: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
            JumpItem { label: l.to_string(), icon: Some(gio::ThemedIcon::new(icon).upcast()), tip: None,
                act: Rc::new(move || { util::spawn(&cmd.iter().map(String::as_str).collect::<Vec<_>>()); }) }
        }).collect();
        return ("Tasks", v);
    }
    let tb = config::get().taskbar;
    if !tb.track_items { return ("Recent", vec![]); }
    let exe = app.executable().file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
    let names = [app.name().to_lowercase(), exe, id.trim_end_matches(".desktop").to_lowercase()];
    let mut out = vec![];
    for (i, apps, def) in recent {
        if out.len() >= tb.jump_items.max(1) as usize { break; }
        let Some(uri) = i.uri() else { continue };
        let ours = apps.iter().any(|a| names.contains(a)) || def.as_deref() == Some(id.as_str());
        if !ours || (uri.starts_with("file://") && !i.exists()) { continue; }
        let (a, u) = (app.clone(), uri.to_string());
        out.push(JumpItem {
            label: i.display_name().map(|s| s.to_string()).unwrap_or_else(|| uri.to_string()),
            icon: i.gicon(),
            tip: Some(gio::File::for_uri(&uri).path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| uri.to_string())),
            act: Rc::new(move || { let _ = a.launch_uris(&[u.as_str()], None::<&gio::AppLaunchContext>); }),
        });
    }
    ("Recent", out)
}

const CATS: &[(&str, &str, &str)] = &[
    ("Accessories", "Utility", "applications-accessories"),
    ("Development", "Development", "applications-development"),
    ("Education", "Education", "applications-science"),
    ("Games", "Game", "applications-games"),
    ("Graphics", "Graphics", "applications-graphics"),
    ("Internet", "Network", "applications-internet"),
    ("Multimedia", "AudioVideo", "applications-multimedia"),
    ("Office", "Office", "applications-office"),
    ("System Tools", "System", "applications-system"),
    ("Control Panel", "Settings", "preferences-system"),
];

fn avatar_pixbuf(size: i32) -> Option<Pixbuf> {
    let face = config::home().join(".face");
    let p = if face.exists() { Some(face) } else { config::data_file("avatar.png") };
    p.and_then(|p| Pixbuf::from_file_at_scale(p, size, size, false).ok())
        .or_else(|| util::pixbuf("avatar-default", size))
}

/// Whether a right-column link is shown (Customize Start Menu; Recent Items also needs the
/// "Store and display recently opened items" privacy option).
fn link_shown(key: &str, track_items: bool) -> bool {
    let v = start_item(key);
    v != "hide" && v != "off" && !(key == "recent" && !track_items)
}

/// Item height for the program list: 38 px with large (32 px) icons, 22 px with small ones.
fn item_height(icon: i32) -> i32 { icon + 6 }

impl StartMenu {
    pub fn new(app: &gtk::Application) -> Rc<StartMenu> {
        let themed = shellstyle::available();
        let win = gtk::Window::new(gtk::WindowType::Toplevel);
        win.set_application(Some(app));
        win.set_title("Start menu");
        win.set_decorated(false);
        win.set_type_hint(gdk::WindowTypeHint::Utility);
        win.set_skip_taskbar_hint(true);
        win.set_skip_pager_hint(true);
        win.set_keep_above(true);
        win.set_app_paintable(true);
        crate::glass::enable_rgba(&win);
        win.set_size_request(W, OV + 476);
        win.style_context().add_class("aerode");
        win.style_context().add_class("startmenu");
        if themed { win.style_context().add_class("w7"); }

        let fixed = gtk::Fixed::new();
        win.add(&fixed);

        // ---- left pane: program list, "All Programs", search box
        let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        sw.style_context().add_class("sm-list");
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        sw.add(&list);
        fixed.put(&sw, ITEM_X, OV + LIST_TOP);

        let all = gtk::Button::new();
        all.style_context().add_class("sm-item");
        all.style_context().add_class("sm-allprogs");
        let all_lbl = gtk::Label::new(Some("All Programs"));
        all_lbl.set_xalign(0.0);
        let ab = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let all_arrow = gtk::Image::new();
        all_arrow.set_size_request(18, 18);
        all_arrow.set_margin_start(7);
        all_arrow.set_margin_end(11);
        ab.pack_start(&all_arrow, false, false, 0);
        ab.pack_start(&all_lbl, true, true, 0);
        all.add(&ab);
        themed_item(&all, false);
        fixed.put(&all, ITEM_X, 0);

        let search = gtk::Entry::new();
        search.style_context().add_class("sm-search");
        search.set_has_frame(false);
        fixed.put(&search, 25, 0);
        // Win7's plain magnifier glyph at the right end of the box (ExplorerFrame's search glyph)
        let search_icon = util::image(util::first_icon(&["system-search-symbolic", "edit-find-symbolic", "edit-find"]), 16);
        fixed.put(&search_icon, 0, 0);

        let sm = Rc::new(StartMenu {
            win: win.clone(), fixed: fixed.clone(), sw, list, all: all.clone(), search: search.clone(), search_icon,
            avatar: RefCell::new(avatar_pixbuf(48)), mode: Cell::new(0),
            all_btn_label: all_lbl, all_arrow, expanded: RefCell::new(vec![]), on_hide: RefCell::new(None), on_show: RefCell::new(None), hidden_at: Cell::new(None),
            armed: Cell::new(false),
            links: RefCell::new(vec![]), power_btn: RefCell::new(None), geom: Cell::new((422, 476)), seps: RefCell::new(vec![]),
            jump: RefCell::new(None), jump_timer: RefCell::new(None),
        });

        // ---- right column links
        let user = glib::real_name().to_string_lossy().into_owned();
        let user = if user.is_empty() || user == "Unknown" { glib::user_name().to_string_lossy().into_owned() } else { user };
        let home = config::home();
        let ex = |p: std::path::PathBuf| -> Act { Rc::new(move || util::explore(&p.to_string_lossy())) };
        let ctrl = |page: &'static str| -> Act { Rc::new(move || { util::spawn(&["aerode-control", page]); }) };
        let recent: Act = Rc::new(|| {
            // Recent Items: the files GTK programs recorded, newest first
            let Some(rm) = gtk::RecentManager::default() else { return };
            let mut items = rm.items();
            items.sort_by_key(|i| std::cmp::Reverse(i.modified()));
            let m = gtk::Menu::new();
            for i in items.iter().take(config::get().taskbar.jump_items.max(1) as usize) {
                let Some(uri) = i.uri() else { continue };
                let it = gtk::MenuItem::with_label(&i.display_name().map(|s| s.to_string()).unwrap_or_else(|| uri.to_string()));
                let u = uri.to_string();
                it.connect_activate(move |_| { util::open_uri(&u); });
                m.append(&it);
            }
            if m.children().is_empty() { let e = gtk::MenuItem::with_label("(Empty)"); e.set_sensitive(false); m.append(&e); }
            m.show_all();
            m.popup_at_pointer(None);
        });
        let groups: Vec<Vec<(&'static str, String, &str, Act)>> = vec![
            vec![
                ("user", user.clone(), "user-home", ex(home.clone())),
                ("documents", "Documents".into(), "folder-documents", ex(util::special_dir(glib::UserDirectory::Documents, "Documents"))),
                ("pictures", "Pictures".into(), "folder-pictures", ex(util::special_dir(glib::UserDirectory::Pictures, "Pictures"))),
                ("music", "Music".into(), "folder-music", ex(util::special_dir(glib::UserDirectory::Music, "Music"))),
                ("videos", "Videos".into(), "folder-videos", ex(util::special_dir(glib::UserDirectory::Videos, "Videos"))),
                ("downloads", "Downloads".into(), "folder-download", ex(util::special_dir(glib::UserDirectory::Downloads, "Downloads"))),
            ],
            vec![
                ("games", "Games".into(), util::first_icon(&["folder-games", "applications-games"]), Rc::new(|| util::explore("aerode:games"))),
                ("recent", "Recent Items".into(), util::first_icon(&["document-open-recent", "folder-recent"]), recent),
                ("computer", "Computer".into(), "computer", Rc::new(|| util::explore("computer:///"))),
                ("network", "Network".into(), "network-workgroup", Rc::new(|| util::explore("network:///"))),
            ],
            vec![
                ("control", "Control Panel".into(), "preferences-system", ctrl("")),
                ("devices", "Devices and Printers".into(), "printer", ctrl("devices")),
                ("defaults", "Default Programs".into(), "preferences-desktop-default-applications", ctrl("defaults")),
                ("help", "Help and Support".into(), "help-browser", Rc::new(|| { util::open_uri("https://wiki.archlinux.org/"); })),
                ("run", "Run...".into(), "system-run", Rc::new(|| crate::dialogs::run_dialog())),
            ],
        ];
        for group in groups {
            let mut g = vec![];
            for (key, label, icon, act) in group {
                let b = gtk::Button::with_label(&label);
                b.style_context().add_class("sm-link");
                if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_xalign(0.0); }
                let s = sm.clone();
                b.connect_clicked(move |_| { s.hide(); act(); });
                // Win7 swaps the user picture for the hovered item's icon
                let (s, icon) = (Rc::downgrade(&sm), icon.to_string());
                b.connect_enter_notify_event(move |_, _| {
                    if let Some(s) = s.upgrade() { s.set_avatar(util::pixbuf(&icon, 48)); }
                    glib::Propagation::Proceed
                });
                let s = Rc::downgrade(&sm);
                b.connect_leave_notify_event(move |_, _| {
                    if let Some(s) = s.upgrade() { s.set_avatar(avatar_pixbuf(48)); }
                    glib::Propagation::Proceed
                });
                if themed {
                    b.connect_draw(|w, cr| {
                        // StartPanelPriv 36: the glassy hot-track rectangle of the right column
                        if w.state_flags().intersects(gtk::StateFlags::PRELIGHT | gtk::StateFlags::ACTIVE) {
                            shellstyle::paint(cr, "StartPanelPriv", 36, 1, 0.0, 0.0, w.allocated_width() as f64, w.allocated_height() as f64);
                        }
                        glib::Propagation::Proceed
                    });
                }
                fixed.put(&b, P + 5, 0);
                g.push((key, b.upcast::<gtk::Widget>()));
            }
            sm.links.borrow_mut().push(g);
        }

        // ---- shut down button + options
        let shut = gtk::Button::with_label("Shut down");
        shut.style_context().add_class("sm-power");
        let arrow = gtk::Button::new();
        arrow.style_context().add_class("sm-power");
        arrow.style_context().add_class("arrow");
        if themed {
            shut.connect_draw(|w, cr| {
                // StartPanelPriv 37: the Shut down button on glass (1 normal, 2 hot, 3 pressed); 38 on
                // the light band under an open jump list
                let part = if on_light(w.upcast_ref()) { 38 } else { 37 };
                shellstyle::paint(cr, "StartPanelPriv", part, button_state(w.upcast_ref()), 0.0, 0.0, w.allocated_width() as f64, w.allocated_height() as f64);
                glib::Propagation::Proceed
            });
            arrow.connect_draw(|w, cr| {
                // StartPanelPriv 23: the options arrow next to it, glyph in IMAGEFILE1 (96 dpi)
                let s = button_state(w.upcast_ref());
                let (aw, ah) = (w.allocated_width() as f64, w.allocated_height() as f64);
                // (24 with its grey glyph on the light band under a jump list)
                let part = if on_light(w.upcast_ref()) { 24 } else { 23 };
                shellstyle::paint(cr, "StartPanelPriv", part, s, 0.0, 0.0, aw, ah);
                if let Some(g) = shellstyle::frame_of("StartPanelPriv", part, s, crate::msstyles::IMAGEFILE1) {
                    cr.set_source_surface(&g, ((aw - g.width() as f64) / 2.0).floor(), ((ah - g.height() as f64) / 2.0).floor()).ok();
                    cr.paint().ok();
                }
                glib::Propagation::Proceed
            });
        } else {
            arrow.set_label("▸");
        }
        // Taskbar and Start Menu Properties > Power button action
        { let s = sm.clone(); shut.connect_clicked(move |_| { s.hide(); power_action(&config::get().taskbar.power_button)(); }); }
        {
            let s = sm.clone();
            arrow.connect_clicked(move |a| {
                let m = gtk::Menu::new();
                // the menu offers every action except the button's own
                let cur = config::get().taskbar.power_button;
                let mut items: Vec<(&str, &str)> = vec![("switchuser", "Switch user"), ("logoff", "Log off"), ("lock", "Lock"), ("", ""),
                    ("restart", "Restart"), ("sleep", "Sleep"), ("hibernate", "Hibernate"), ("shutdown", "Shut down")];
                items.retain(|i| i.0 != cur.as_str() && !(i.0 == "shutdown" && cur.is_empty()));
                for (id, l) in items {
                    if l.is_empty() { m.append(&gtk::SeparatorMenuItem::new()); continue; }
                    let f = power_action(id);
                    let it = gtk::MenuItem::with_label(l);
                    let s = s.clone();
                    it.connect_activate(move |_| { s.hide(); f(); });
                    m.append(&it);
                }
                m.show_all();
                m.popup_at_widget(a, gdk::Gravity::NorthEast, gdk::Gravity::NorthWest, None);
            });
        }
        fixed.put(&shut, P + 5, 0);
        fixed.put(&arrow, P + 5, 0);
        *sm.power_btn.borrow_mut() = Some((shut, arrow));

        // ---- behaviour
        { let s = sm.clone(); all.connect_clicked(move |_| { s.mode.set(if s.mode.get() == 1 { 0 } else { 1 }); s.populate(); }); }
        {
            let s = sm.clone();
            search.connect_changed(move |e| {
                s.mode.set(if e.text().is_empty() { 0 } else { 2 });
                s.populate();
                // the cue text ("Search programs and files") is painted by the window
                s.win.queue_draw();
            });
        }
        {
            let s = sm.clone();
            search.connect_activate(move |e| {
                let q = e.text().to_string();
                if let Some(first) = s.list.children().into_iter().find_map(|c| c.downcast::<gtk::Button>().ok()) {
                    first.clicked();
                } else if !q.is_empty() {
                    s.hide();
                    util::run_command(&q);
                }
            });
        }
        {
            let s = sm.clone();
            win.connect_key_press_event(move |_, e| {
                let k = e.keyval();
                if k == gdk::keys::constants::Escape || k == gdk::keys::constants::Super_L {
                    s.hide();
                    return glib::Propagation::Stop;
                }
                if !s.search.has_focus() && k.to_unicode().map(|c| !c.is_control()).unwrap_or(false) {
                    s.search.grab_focus_without_selecting();
                }
                glib::Propagation::Proceed
            });
        }
        // Close when the WM makes another window active (_NET_WM_STATE_FOCUSED, which GDK reports as the
        // FOCUSED window state), not on every focus-out: the WM's passive grab on the Windows key and the
        // menu's own popups (power options, item context menus) cause NotifyGrab focus-outs. Hiding on those
        // closed the menu on the key *press*, so the window behind it was activated while the key was still
        // held (its WM caption at the press, its own content only at the release) and a long press reopened
        // the menu; it also closed the menu as soon as the power-options arrow was clicked. Armed by the
        // first focus-in after show(), so a stale FOCUSED flag from the previous opening cannot close it.
        { let s = sm.clone(); win.connect_focus_in_event(move |_, _| { s.armed.set(true); glib::Propagation::Proceed }); }
        {
            let s = sm.clone();
            win.connect_window_state_event(move |_, e| {
                let lost = e.changed_mask().contains(gdk::WindowState::FOCUSED) && !e.new_window_state().contains(gdk::WindowState::FOCUSED);
                if lost && s.armed.get() { s.hide(); }
                glib::Propagation::Proceed
            });
        }
        {
            let s = Rc::downgrade(&sm);
            win.connect_draw(move |w, cr| {
                if let Some(s) = s.upgrade() {
                    let (x, y) = w.position();
                    s.paint(cr, x as f64, y as f64);
                }
                glib::Propagation::Proceed
            });
        }
        // the msstyles band holds the search box: keep the picture and cue in step with the colours
        { let w = win.clone(); glass::on_change(move || w.queue_draw()); }
        fixed.show_all();
        sm.apply_options();
        { let s2 = Rc::downgrade(&sm); HIDE_ALL.with(|h| *h.borrow_mut() = Some(Rc::new(move || if let Some(s) = s2.upgrade() { s.hide() }))); }
        sm
    }

    fn set_avatar(&self, p: Option<Pixbuf>) {
        *self.avatar.borrow_mut() = p;
        // only the tile needs repainting
        self.win.queue_draw_area(TILE_X, 0, TILE, TILE);
    }

    pub fn visible(&self) -> bool {
        self.win.is_visible()
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.visible() { self.hide(); return; }
        // a Windows-key press that reaches the menu itself (no WM key grab) hides it in the key handler;
        // the `aerode start` sent for that same press must not then reopen it
        if self.hidden_at.get().map(|t| t.elapsed().as_millis() < 400).unwrap_or(false) { return; }
        self.show()
    }

    pub fn show(self: &Rc<Self>) {
        crate::flyouts::close_open();
        self.mode.set(0);
        self.search.set_text("");
        self.expanded.borrow_mut().clear();
        self.apply_options();
        self.populate();
        let g = util::primary_geometry();
        let (_, hm) = self.geom.get();
        self.win.move_(g.x(), g.y() + g.height() - config::get().taskbar_height - hm - OV);
        self.armed.set(false);
        if let Some(f) = self.on_show.borrow().as_ref() { f(); }
        self.win.show();
        self.win.present_with_time(gtk::current_event_time());
        self.search.grab_focus();
    }

    /// Customize Start Menu items and the power button label (set in Control Panel), then the layout.
    fn apply_options(&self) {
        let tb = config::get().taskbar;
        for (key, w) in self.links.borrow().iter().flatten() {
            w.set_visible(link_shown(key, tb.track_items));
        }
        if let Some((b, _)) = self.power_btn.borrow().as_ref() {
            b.set_label(POWER_ACTIONS.iter().find(|a| a.0 == tb.power_button).map(|a| a.1).unwrap_or("Shut down"));
        }
        self.layout();
    }

    /// Place everything on the Win7 metrics; the menu grows when the right column needs more room.
    fn layout(&self) {
        let tb = config::get().taskbar;
        let large = start_item("largeicons") != "off";
        let slots = if tb.track_programs { tb.recent_programs as i32 } else { 0 };
        let left_hu = LIST_TOP + slots * item_height(if large { 32 } else { 16 }) + 1 + MORE;
        // right column: link groups with a separator between non-empty groups
        let mut y = LINK_TOP;
        let mut seps = vec![];
        let mut first = true;
        for group in self.links.borrow().iter() {
            let shown: Vec<&gtk::Widget> = group.iter().filter(|(k, _)| link_shown(k, tb.track_items)).map(|(_, w)| w).collect();
            if shown.is_empty() { continue; }
            if !first {
                // the 2 px line sits in the middle of the 7 px gap between the groups
                seps.push(y - 2);
                y += SEP_EXTRA;
            }
            first = false;
            for w in shown {
                self.fixed.move_(w, P + 5, OV + y);
                w.set_size_request(W - P - 12, LINK_H);
                y += LINK_PITCH;
            }
        }
        // the last link's bottom (y - 4) plus a margin, like the space under "Help and Support" in Win7
        let right_hu = y - 4 + 24;
        let hu = left_hu.max(right_hu);
        let hm = hu + BAND;
        *self.seps.borrow_mut() = seps;
        self.geom.set((hu, hm));

        let list_h = hu - MORE - 1 - LIST_TOP;
        self.fixed.move_(&self.sw, ITEM_X, OV + LIST_TOP);
        self.sw.set_size_request(ITEM_W, list_h);
        self.fixed.move_(&self.all, ITEM_X, OV + hu - MORE + 3);
        self.all.set_size_request(ITEM_W, MORE - 6);
        // the search box of StartPanelPriv 18 spans x 21..P-18, y +12..+34; text inside, magnifier right
        let (bx0, bx1, by) = (21, P - 18, OV + hu + 12);
        self.fixed.move_(&self.search_icon, bx1 - 4 - 16, by + 3);
        self.fixed.move_(&self.search, bx0 + 4, by + 1);
        self.search.set_size_request(bx1 - 4 - 16 - 4 - (bx0 + 4), 20);
        if let Some((b, a)) = self.power_btn.borrow().as_ref() {
            let (_, nat) = b.preferred_width();
            let bw = nat.max(50);
            self.fixed.move_(b, P + 5, OV + hu + 12);
            b.set_size_request(bw, 24);
            self.fixed.move_(a, P + 5 + bw, OV + hu + 12);
            a.set_size_request(23, 24);
        }
        if let Some((_, pane, _)) = self.jump.borrow().as_ref() {
            // the list fills the white pane (StartPanelPriv 7's and 10's content margins) below the
            // user tile, where the places list starts
            self.fixed.move_(pane, P + 9, OV + JUMP_TOP);
            pane.set_size_request(W_JUMP - P - 9 - 12, hu - JUMP_TOP - 2);
        }
        let (w, h) = (self.width(), OV + hm);
        self.win.set_size_request(w, h);
        self.win.resize(w, h);
        self.shape(hm);
    }

    /// Cut the window to the menu and the user tile: without a compositor the area around the tile
    /// would be opaque, and clicks there should reach what is behind.
    fn shape(&self, hm: i32) {
        let r = cairo::Region::create();
        // the rounded top corners of the frame (6 px radius)
        for (i, inset) in [5, 3, 2, 1, 1].iter().enumerate() {
            r.union_rectangle(&cairo::RectangleInt::new(*inset, OV + i as i32, self.width() - 2 * inset, 1)).ok();
        }
        r.union_rectangle(&cairo::RectangleInt::new(0, OV + 5, self.width(), hm - 5)).ok();
        r.union_rectangle(&cairo::RectangleInt::new(TILE_X + 1, 1, TILE - 2, TILE - 2)).ok();
        if !glass::composited() { self.win.shape_combine_region(Some(&r)); } else { self.win.shape_combine_region(None); }
        self.win.input_shape_combine_region(Some(&r));
    }

    /// The menu's width: wider while a jump list is open.
    fn width(&self) -> i32 { if self.jump.borrow().is_some() { W_JUMP } else { W } }

    fn cancel_jump_timer(&self) {
        if let Some(t) = self.jump_timer.borrow_mut().take() { t.remove(); }
    }

    /// The pointer rests on a program item: after the hover delay open its jump list (it has
    /// one), or close the open one (it has none); an item whose list is already open keeps it.
    fn hover_item(self: &Rc<Self>, app: &gio::AppInfo, item: &gtk::Button, has_list: bool) {
        self.cancel_jump_timer();
        let id = app.id().map(|s| s.to_string()).unwrap_or_default();
        let open = self.jump.borrow().as_ref().map(|j| j.0.clone());
        if open.as_deref() == Some(id.as_str()) || (!has_list && open.is_none()) { return; }
        let (s, a, b) = (Rc::downgrade(self), app.clone(), item.downgrade());
        let t = glib::timeout_add_local_once(std::time::Duration::from_millis(JUMP_DELAY), move || {
            let Some(s) = s.upgrade() else { return };
            s.jump_timer.borrow_mut().take();
            match b.upgrade() {
                Some(b) if has_list => s.open_jump(&a, &b),
                _ => s.close_jump(),
            }
        });
        *self.jump_timer.borrow_mut() = Some(t);
    }

    /// Show `app`'s jump list in place of the right column (the menu widens), with its item
    /// kept highlighted like Win7.
    fn open_jump(self: &Rc<Self>, app: &gio::AppInfo, item: &gtk::Button) {
        if self.jump.borrow().as_ref().map(|j| &j.2 == item).unwrap_or(false) { return; }
        let (title, items) = jump_items(app, &recent_index());
        if items.is_empty() { self.close_jump(); return; }
        self.close_jump_quiet();
        let pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane.style_context().add_class("sm-jump");
        let head = gtk::Label::new(Some(title));
        head.style_context().add_class("sm-heading");
        head.set_xalign(0.0);
        pane.pack_start(&head, false, false, 0);
        for it in items {
            let b = gtk::Button::new();
            b.style_context().add_class("sm-item");
            b.set_size_request(-1, item_height(16));
            let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            h.set_margin_start(6);
            h.pack_start(&gtk::Image::from_pixbuf(util::gicon_pixbuf(it.icon.as_ref(), 16).as_ref()), false, false, 0);
            let l = gtk::Label::new(Some(&it.label));
            l.set_xalign(0.0);
            l.set_ellipsize(pango::EllipsizeMode::End);
            h.pack_start(&l, true, true, 0);
            b.add(&h);
            themed_item(&b, true);
            if let Some(t) = &it.tip { b.set_tooltip_text(Some(t)); }
            let (s, act) = (self.clone(), it.act.clone());
            b.connect_clicked(move |_| { s.hide(); act(); });
            pane.pack_start(&b, false, false, 0);
        }
        self.fixed.put(&pane, P + 9, OV + JUMP_TOP);
        self.win.style_context().add_class("jump");
        for (_, w) in self.links.borrow().iter().flatten() { w.set_visible(false); }
        item.style_context().add_class("selected");
        item.queue_draw();
        *self.jump.borrow_mut() = Some((app.id().map(|s| s.to_string()).unwrap_or_default(), pane.clone(), item.clone()));
        self.layout();
        pane.show_all();
        self.win.queue_draw();
    }

    /// Close the jump list and bring the right column back.
    fn close_jump(&self) {
        if self.close_jump_quiet() {
            self.apply_options();
            self.win.queue_draw();
        }
    }

    fn close_jump_quiet(&self) -> bool {
        self.cancel_jump_timer();
        let Some((_, pane, item)) = self.jump.borrow_mut().take() else { return false };
        self.fixed.remove(&pane);
        self.win.style_context().remove_class("jump");
        item.style_context().remove_class("selected");
        item.queue_draw();
        true
    }

    pub fn hide(&self) {
        self.close_jump();
        if self.win.is_visible() {
            self.hidden_at.set(Some(std::time::Instant::now()));
            self.armed.set(false);
            self.win.hide();
            if let Some(f) = self.on_hide.borrow().as_ref() { f(); }
        }
    }

    // ------------------------------------------------------------------ painting

    fn paint(&self, cr: &cairo::Context, sx: f64, sy: f64) {
        let (hu, hm) = self.geom.get();
        let (w, hu, hm, p) = (self.width() as f64, hu as f64, hm as f64, P as f64);
        let jump = self.jump.borrow().is_some();
        cr.save().ok();
        cr.set_operator(cairo::Operator::Source);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        cr.paint().ok();
        cr.restore().ok();
        if !glass::composited() {
            // the tile's soft edges over the desktop (the window is shaped to the tile and the menu)
            let wp = glass::wallpaper();
            cr.save().ok();
            cr.rectangle(TILE_X as f64, 0.0, TILE as f64, OV as f64);
            cr.clip();
            gdk::prelude::GdkContextExt::set_source_pixbuf(cr, &wp.sharp, -sx, -sy);
            cr.paint().ok();
            cr.restore().ok();
        }
        cr.save().ok();
        cr.translate(0.0, OV as f64);
        cr.save().ok();
        glass::rounded_top(cr, 0.0, 0.0, w, hm, 6.0, 0.0);
        cr.clip();
        glass::paint_glass_fill(cr, w, hm, sx, sy + OV as f64, true, false);
        cr.restore().ok();
        if shellstyle::available() {
            // left column: the white pane (StartPanelPriv 20 cut before its right frame edge, then the
            // "All Programs" row 2), the search band (18); right column: glass (9) and its bottom (14)
            cr.save().ok();
            cr.rectangle(0.0, 0.0, p, hu - MORE as f64);
            cr.clip();
            shellstyle::paint(cr, SP, 20, 0, 0.0, 0.0, p + 7.0, hu - MORE as f64);
            cr.restore().ok();
            shellstyle::paint(cr, SP, 2, 0, 0.0, hu - MORE as f64, p, MORE as f64);
            shellstyle::paint(cr, SP, 18, 0, 0.0, hu, p, BAND as f64);
            shellstyle::paint(cr, SP, 8, 0, MORE_SEP_X as f64, hu - MORE as f64 - 1.0, 232.0, 1.0);
            if jump {
                // the jump list's white pane: 7 (left frame) + 10 (right end), closed at the bottom by
                // 15, whose glass below the pane holds the Shut down button
                shellstyle::paint(cr, SP, 7, 0, p, 0.0, w - p - 14.0, hu);
                shellstyle::paint(cr, SP, 10, 0, w - 14.0, 0.0, 14.0, hu);
                shellstyle::paint(cr, SP, 15, 0, p, hu, w - p, BAND as f64);
            } else {
                shellstyle::paint(cr, SP, 9, 0, p, 0.0, w - p, hu);
                shellstyle::paint(cr, SP, 14, 0, p, hu, w - p, BAND as f64);
                for y in self.seps.borrow().iter() {
                    shellstyle::paint(cr, SP, 11, 0, p + 15.0, *y as f64, w - p - 30.0, 2.0);
                }
            }
        } else {
            let seps = if jump { vec![] } else { self.seps.borrow().clone() };
            paint_fallback(cr, w, hu, hm, p, &seps, jump);
        }
        if self.search.text().is_empty() {
            // the search box's cue banner (SearchEditBox TEXTCOLOR, italic)
            let a = self.search.allocation();
            let layout = self.win.create_pango_layout(Some("Search programs and files"));
            let mut fd = layout.context().font_description().unwrap_or_default();
            fd.set_style(pango::Style::Italic);
            layout.set_font_description(Some(&fd));
            let (_, lh) = layout.pixel_size();
            cr.set_source_rgb(109.0 / 255.0, 109.0 / 255.0, 109.0 / 255.0);
            cr.move_to(a.x() as f64 + 2.0, a.y() as f64 - OV as f64 + ((a.height() - lh) / 2) as f64);
            pangocairo::functions::show_layout(cr, &layout);
        }
        cr.restore().ok();
        // the user tile: picture, then the frame (explorer.exe 7013) with its gloss on top
        let (tx, tile) = (TILE_X as f64, TILE as f64);
        if let Some(pb) = self.avatar.borrow().as_ref() {
            let (pw, ph) = (pb.width() as f64, pb.height() as f64);
            cr.save().ok();
            cr.rectangle(tx + 8.0, 8.0, 48.0, 48.0);
            cr.clip();
            cr.translate(tx + 8.0 + (48.0 - pw.min(48.0)) / 2.0, 8.0 + (48.0 - ph.min(48.0)) / 2.0);
            gdk::prelude::GdkContextExt::set_source_pixbuf(cr, pb, 0.0, 0.0);
            cr.paint().ok();
            cr.restore().ok();
        }
        if let Some(f) = shellstyle::data_surface("explorer/7013.png") {
            cr.set_source_surface(&f, tx, 0.0).ok();
            cr.paint().ok();
        } else {
            glass::rounded(cr, tx + 4.5, 4.5, tile - 9.0, tile - 9.0, 4.0);
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.85);
            cr.set_line_width(3.0);
            cr.stroke().ok();
        }
    }

    fn item(self: &Rc<Self>, app: &gio::AppInfo, size: i32, indent: i32) -> gtk::Button {
        let b = gtk::Button::new();
        b.style_context().add_class("sm-item");
        b.set_size_request(-1, item_height(size));
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        h.set_margin_start(indent);
        h.pack_start(&util::app_image(app, size), false, false, 0);
        let l = gtk::Label::new(Some(&app.display_name()));
        l.set_xalign(0.0);
        l.set_ellipsize(pango::EllipsizeMode::End);
        h.pack_start(&l, true, true, 0);
        b.add(&h);
        themed_item(&b, size < 32);
        if let Some(d) = app.description() { b.set_tooltip_text(Some(&d)); }
        let (s, a) = (self.clone(), app.clone());
        b.connect_clicked(move |_| { s.hide(); util::launch(&a); });
        let a = app.clone();
        b.connect_button_press_event(move |b, e| {
            if e.button() == 3 {
                let m = gtk::Menu::new();
                let open = gtk::MenuItem::with_label("Open");
                let a2 = a.clone();
                open.connect_activate(move |_| util::launch(&a2));
                m.append(&open);
                if let Some(id) = a.id() {
                    let pin = gtk::MenuItem::with_label("Pin to Taskbar");
                    let id = id.to_string();
                    pin.connect_activate(move |_| {
                        let mut p = util::pinned();
                        if !p.contains(&id) { p.push(id.clone()); config::update(|c| c.pinned = Some(p)); }
                    });
                    m.append(&pin);
                }
                m.show_all();
                m.popup_at_pointer(Some(e));
                let _ = b;
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        b
    }

    /// A program of the frequent list: with a jump list it gets the arrow at its right end
    /// (StartPanelPriv 5) and a split hot-track; resting on it opens the list, and so does a
    /// click on the arrow.
    fn mfu_item(self: &Rc<Self>, app: &gio::AppInfo, size: i32, recent: &RecentIndex) -> gtk::Button {
        let b = self.item(app, size, 0);
        let has_list = !jump_items(app, recent).1.is_empty();
        if has_list {
            b.style_context().add_class("split");
            if let Some(h) = b.child().and_then(|c| c.downcast::<gtk::Box>().ok()) {
                let arrow = match shellstyle::frame_of("StartPanelPriv", 5, 0, crate::msstyles::IMAGEFILE1) {
                    Some(s) => gtk::Image::from_pixbuf(gdk::pixbuf_get_from_surface(&s, 0, 0, s.width(), s.height()).as_ref()),
                    None => gtk::Image::from_icon_name(Some("pan-end-symbolic"), gtk::IconSize::Menu),
                };
                // centred in the arrow part of the split item
                arrow.set_size_request(SPLIT_W - 2, -1);
                h.pack_end(&arrow, false, false, 0);
            }
            let (s, a) = (Rc::downgrade(self), app.clone());
            b.connect_button_press_event(move |b, e| {
                let Some(s) = s.upgrade() else { return glib::Propagation::Proceed };
                if e.button() == 1 && e.position().0 >= (b.allocated_width() - SPLIT_W) as f64 {
                    s.cancel_jump_timer();
                    s.open_jump(&a, b);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        let (s, a) = (Rc::downgrade(self), app.clone());
        b.connect_enter_notify_event(move |b, _| {
            if let Some(s) = s.upgrade() { s.hover_item(&a, b, has_list); }
            glib::Propagation::Proceed
        });
        let s = Rc::downgrade(self);
        b.connect_leave_notify_event(move |_, e| {
            if e.detail() != gdk::NotifyType::Inferior { if let Some(s) = s.upgrade() { s.cancel_jump_timer(); } }
            glib::Propagation::Proceed
        });
        b
    }

    fn set_all_button(&self, label: &str, back: bool) {
        self.all_btn_label.set_text(label);
        // StartPanelPriv 4 = the "All Programs" arrow, 21 = the "Back" arrow (96 dpi image)
        let part = if back { 21 } else { 4 };
        match shellstyle::frame_of("StartPanelPriv", part, 1, crate::msstyles::IMAGEFILE1) {
            Some(s) => {
                let pb = gdk::pixbuf_get_from_surface(&s, 0, 0, s.width(), s.height());
                self.all_arrow.set_from_pixbuf(pb.as_ref());
            }
            None => self.all_arrow.set_from_icon_name(Some(if back { "pan-start-symbolic" } else { "pan-end-symbolic" }), gtk::IconSize::Menu),
        }
    }

    fn populate(self: &Rc<Self>) {
        self.close_jump();
        for c in self.list.children() { self.list.remove(&c); }
        let apps = util::all_apps();
        match self.mode.get() {
            0 => {
                self.set_all_button("All Programs", false);
                // Privacy: "Store and display recently opened programs" off = an empty list, like Win7
                let tb = config::get().taskbar;
                let n = if tb.track_programs { tb.recent_programs as usize } else { 0 };
                let isz = if start_item("largeicons") == "off" { 16 } else { 32 };
                let usage = config::get().usage;
                let mut freq: Vec<_> = apps.iter().filter(|a| a.id().map(|i| usage.contains_key(i.as_str())).unwrap_or(false)).cloned().collect();
                freq.sort_by_key(|a| std::cmp::Reverse(usage.get(a.id().unwrap().as_str()).copied().unwrap_or(0)));
                // until enough programs have been used: Win7's default list (Getting Started,
                // Calculator, Sticky Notes, Snipping Tool, Paint, …) mapped to aerode's programs
                for s in DEFAULT_PROGRAMS {
                    if freq.len() >= n { break; }
                    if let Some(a) = util::app_by_id(s) { if !freq.iter().any(|f| f.id() == a.id()) { freq.push(a); } }
                }
                if let Some(t) = util::terminal() { if freq.len() < n && !freq.iter().any(|f| f.id() == t.id()) { freq.push(t); } }
                let recent = recent_index();
                for a in freq.iter().take(n) {
                    self.list.pack_start(&self.mfu_item(a, isz, &recent), false, false, 0);
                }
            }
            1 => {
                self.set_all_button("Back", true);
                // like Win7: the loose programs first (alphabetical), then the folders
                let folders: Vec<(&str, Vec<gio::AppInfo>)> = CATS.iter().map(|(name, cat, _)| {
                    (*name, apps.iter().filter(|a| util::categories(a).split(';').any(|c| c == *cat)).cloned().collect::<Vec<_>>())
                }).filter(|(_, m)| !m.is_empty()).collect();
                let id = |a: &gio::AppInfo| a.id().map(|s| s.to_string()).unwrap_or_default();
                let used: std::collections::HashSet<String> = folders.iter().flat_map(|(_, m)| m.iter().map(id)).collect();
                for a in apps.iter().filter(|a| !used.contains(&id(a))) {
                    self.list.pack_start(&self.item(a, 16, 0), false, false, 0);
                }
                for (name, members) in folders {
                    let open = self.expanded.borrow().iter().any(|e| e == name);
                    let b = gtk::Button::new();
                    b.style_context().add_class("sm-item");
                    b.set_size_request(-1, item_height(16));
                    let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    h.pack_start(&util::image(util::first_icon(&["folder"]), 16), false, false, 0);
                    h.pack_start(&gtk::Label::new(Some(name)), false, false, 0);
                    b.add(&h);
                    themed_item(&b, true);
                    let (s, n) = (self.clone(), name.to_string());
                    b.connect_clicked(move |_| {
                        { let mut e = s.expanded.borrow_mut(); if let Some(i) = e.iter().position(|x| x == &n) { e.remove(i); } else { e.push(n.clone()); } }
                        s.populate();
                    });
                    self.list.pack_start(&b, false, false, 0);
                    if open {
                        for a in &members { self.list.pack_start(&self.item(a, 16, 18), false, false, 0); }
                    }
                }
            }
            _ => {
                self.set_all_button("See more results", false);
                let q = self.search.text().to_lowercase();
                let hits: Vec<_> = apps.iter().filter(|a| {
                    a.display_name().to_lowercase().contains(&q)
                        || a.description().map(|d| d.to_lowercase().contains(&q)).unwrap_or(false)
                        || a.downcast_ref::<gio::DesktopAppInfo>().and_then(|d| d.string("Keywords")).map(|k| k.to_lowercase().contains(&q)).unwrap_or(false)
                        || a.executable().to_string_lossy().to_lowercase().contains(&q)
                }).cloned().collect();
                let head = gtk::Label::new(Some(&format!("Programs ({})", hits.len())));
                head.style_context().add_class("sm-heading");
                head.set_xalign(0.0);
                self.list.pack_start(&head, false, false, 0);
                for a in hits.iter().take(8) { self.list.pack_start(&self.item(a, 16, 6), false, false, 0); }
                // quick file search in home (names only, shallow)
                let files = file_search(&q, 6);
                if !files.is_empty() {
                    let head = gtk::Label::new(Some(&format!("Files ({})", files.len())));
                    head.style_context().add_class("sm-heading");
                    head.set_xalign(0.0);
                    self.list.pack_start(&head, false, false, 0);
                    for f in files {
                        let b = gtk::Button::new();
                        b.style_context().add_class("sm-item");
                        b.set_size_request(-1, item_height(16));
                        let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                        h.set_margin_start(6);
                        let gf = gio::File::for_path(&f);
                        let icon = gf.query_info("standard::icon", gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE).ok().and_then(|i| i.icon());
                        h.pack_start(&gtk::Image::from_pixbuf(util::gicon_pixbuf(icon.as_ref(), 16).as_ref()), false, false, 0);
                        let l = gtk::Label::new(f.file_name().map(|s| s.to_string_lossy().into_owned()).as_deref());
                        l.set_ellipsize(pango::EllipsizeMode::Middle);
                        h.pack_start(&l, false, false, 0);
                        b.add(&h);
                        themed_item(&b, true);
                        b.set_tooltip_text(Some(&f.to_string_lossy()));
                        let s = self.clone();
                        b.connect_clicked(move |_| { s.hide(); util::open_path(&f); });
                        self.list.pack_start(&b, false, false, 0);
                    }
                }
            }
        }
        self.list.show_all();
    }
}

/// Whether a widget of the menu sits on the light band of an open jump list (the menu window has
/// the "jump" style class then).
fn on_light(w: &gtk::Widget) -> bool {
    w.toplevel().map(|t| t.style_context().has_class("jump")).unwrap_or(false)
}

/// msstyles state of a push button: 1 normal, 2 hot, 3 pressed.
fn button_state(w: &gtk::Widget) -> i32 {
    let st = w.state_flags();
    if st.contains(gtk::StateFlags::ACTIVE) { 3 } else if st.contains(gtk::StateFlags::PRELIGHT) { 2 } else { 1 }
}

/// Width of the arrow part of a frequent-list item with a jump list.
const SPLIT_W: i32 = 24;

/// Hot-track of a left-pane item: StartPanelPriv 29 (the blue rounded rectangle; 1 hot, 3 pressed
/// or showing its jump list) for program items, split into 28 + 31 when the item has a jump list
/// arrow, 16 (23 px high) for the small tree / search items.
fn themed_item(b: &gtk::Button, small: bool) {
    if !shellstyle::available() { return; }
    b.connect_draw(move |w, cr| {
        let st = w.state_flags();
        let hot = st.contains(gtk::StateFlags::PRELIGHT) || w.style_context().has_class("selected");
        let pressed = st.contains(gtk::StateFlags::ACTIVE);
        if hot || pressed {
            let (ww, hh) = (w.allocated_width() as f64, w.allocated_height() as f64);
            let state = if pressed || w.style_context().has_class("selected") { 3 } else { 1 };
            // (16: 1 normal, 2 hot, 3 newly installed, 5 light)
            if small { shellstyle::paint(cr, "StartPanelPriv", 16, 2, 0.0, 0.0, ww, hh); }
            else if w.style_context().has_class("split") {
                let sw = SPLIT_W as f64;
                shellstyle::paint(cr, "StartPanelPriv", 28, state, 0.0, 0.0, ww - sw, hh);
                shellstyle::paint(cr, "StartPanelPriv", 31, state, ww - sw, 0.0, sw, hh);
            } else { shellstyle::paint(cr, "StartPanelPriv", 29, state, 0.0, 0.0, ww, hh); }
        }
        glib::Propagation::Proceed
    });
}

/// The same layout without msstyles: white pane, search band and box, dark glass with a border.
fn paint_fallback(cr: &cairo::Context, w: f64, hu: f64, hm: f64, p: f64, seps: &[i32], jump: bool) {
    cr.set_line_width(1.0);
    // the right column's glass is darker than the frame
    cr.rectangle(p, 0.0, w - p, hm);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.3);
    cr.fill().ok();
    glass::rounded_top(cr, 0.5, 0.5, w - 1.0, hm - 1.0, 6.0, 0.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.65);
    cr.stroke().ok();
    glass::rounded_top(cr, 1.5, 1.5, w - 3.0, hm - 3.0, 5.0, 0.0);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.3);
    cr.stroke().ok();
    glass::rounded(cr, 8.5, 8.5, p - 10.0, hm - 17.0, 4.0);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
    cr.stroke_preserve().ok();
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.fill().ok();
    cr.rectangle(9.0, hu, p - 11.0, hm - hu - 9.0);
    cr.set_source_rgb(0.945, 0.961, 0.984);
    cr.fill().ok();
    cr.rectangle(9.0, hu, p - 11.0, 1.0);
    cr.set_source_rgb(0.8, 0.85, 0.918);
    cr.fill().ok();
    cr.rectangle(21.5, hu + 12.5, p - 18.0 - 22.0, 21.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.fill_preserve().ok();
    cr.set_source_rgb(0.66, 0.72, 0.78);
    cr.stroke().ok();
    cr.rectangle(MORE_SEP_X as f64, hu - MORE as f64 - 1.0, 232.0, 1.0);
    cr.set_source_rgb(0.839, 0.898, 0.961);
    cr.fill().ok();
    if jump {
        // the jump list's white pane over the right column
        glass::rounded(cr, p + 4.5, 8.5, w - p - 13.0, hu + 2.0 - 8.5, 4.0);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.6);
        cr.stroke_preserve().ok();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.fill().ok();
    }
    for y in seps {
        cr.rectangle(p + 15.0, *y as f64, w - p - 30.0, 1.0);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.15);
        cr.fill().ok();
        cr.rectangle(p + 15.0, *y as f64 + 1.0, w - p - 30.0, 1.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.1);
        cr.fill().ok();
    }
}

fn file_search(q: &str, limit: usize) -> Vec<std::path::PathBuf> {
    if q.len() < 2 { return vec![]; }
    let mut out = vec![];
    let mut stack = vec![(config::home(), 0)];
    let mut visited = 0;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            visited += 1;
            if visited > 20000 || out.len() >= limit { return out; }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with('.') { continue; }
            let p = e.path();
            if name.contains(q) { out.push(p.clone()); }
            if depth < 4 && e.file_type().map(|t| t.is_dir()).unwrap_or(false) { stack.push((p, depth + 1)); }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_programs_ship_with_aerode() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/applications");
        for id in DEFAULT_PROGRAMS {
            assert!(dir.join(id).exists(), "{id} is not in data/applications");
        }
        assert_eq!(DEFAULT_PROGRAMS[0], "aerode-gettingstarted.desktop");
    }

    #[test]
    fn jump_list_pane_fits_the_wider_menu() {
        // the list's pane (P + 9 .. W_JUMP - 12) is as wide as the program list, its items' text
        // clear of the user tile's column below JUMP_TOP
        assert_eq!(W_JUMP - P - 9 - 12, ITEM_W - 2);
        assert!(JUMP_TOP >= TILE - OV);
    }

    #[test]
    fn default_menu_matches_the_win7_metrics() {
        // 10 large-icon slots: list 10..390, separator at 390, "All Programs" 391..422, band to 476
        let hu = LIST_TOP + 10 * item_height(32) + 1 + MORE;
        assert_eq!(hu, 422);
        assert_eq!(hu + BAND, 476);
        assert_eq!(MORE_SEP_X + 232, 252);
        // the 10th right-column link (two separators before it) is centred on y = 381
        assert_eq!(LINK_TOP + 9 * LINK_PITCH + 2 * SEP_EXTRA + LINK_H / 2, 381);
    }
}
