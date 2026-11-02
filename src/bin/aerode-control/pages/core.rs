//! Control Panel applets of the core group, one file each under core/: Programs and Features
//! (+ View installed updates, Windows Features), User Accounts (+ User Account Control
//! Settings), Network and Sharing Center (+ wireless networks, Network Connections, adapter
//! status, advanced sharing), Default Programs, Devices and Printers, Region and Language,
//! Ease of Access Center, Startup Programs and Services (msconfig / services.msc).
//!
//! Sub pages have their own page id `<applet>/<sub>` (`aerode-control users/picture`), so task
//! links, Back and the command line can open them directly; [`sub`] tells a page builder which
//! one was asked for.
pub(crate) use gtk::prelude::*;
pub(crate) use gtk::{gdk, gdk_pixbuf::Pixbuf, gio, glib};
pub(crate) use std::cell::{Cell, RefCell};
pub(crate) use std::rc::Rc;

pub(crate) use aerode::{config, util};
pub(crate) use super::super::{kv_grid, link, section, text, title, Cp};
pub(crate) use super::admin::{
    bg, bold, columns, confirm, elevate, elevate_sh, hbox, icon_column, img, inform, label, pix, q, report, run, scrolled,
    see_also, sh, shield_button, shield_link, side_action, side_shield, spawn_tool, sval, terminal, vbox, win_of,
};
pub(crate) use super::desktop::{check, combo, group, radios, Sheet};

mod access;
mod defaults;
mod devices;
mod network;
mod programs;
mod region;
mod startup;
mod users;

pub use access::apply_at_logon;
pub use startup::services_window;

const CSS: &str = r#"
/* Win7 Control Panel task links with a 32 px icon and a grey description under them */
.cp-task-desc { color: #555555; }
.cp-hint { color: #6d6d6d; }
/* Programs and Features / Devices and Printers: the command bar above the list */
.cp-cmdbar { background-image: linear-gradient(to bottom, #fafcfd, #e6eef7 50%, #dce6f4 51%, #dde9f7); border-bottom: 1px solid #a0afc3; border-top: 1px solid #ffffff; padding: 2px 6px; min-height: 28px; }
.cp-cmdbar button { padding: 2px 8px; border: 1px solid transparent; border-radius: 3px; background: none; box-shadow: none; min-height: 0; }
.cp-cmdbar button:hover { border-color: #a5b8d1; background-image: linear-gradient(to bottom, #fbfdff, #e4eefa 50%, #d4e3f6 51%, #e3eefb); }
.cp-cmdbar button label { color: #1e395b; }
.cp-cmdbar button:disabled label { color: #8d99a8; }
/* the details pane under a list (Programs and Features, Devices and Printers) */
.cp-details { background-image: linear-gradient(to bottom, #f9fcfe, #eaf2fb); border-top: 1px solid #c6d4e6; padding: 8px 12px; }
.cp-details .k { color: #5a6779; }
.cp-details .n { font-size: 11pt; }
/* list frame (white, thin blue-grey border) */
.cp-list { border: 1px solid #c7d2e0; background-color: #ffffff; }
.cp-list treeview.view { background-color: #ffffff; }
/* Devices and Printers group headers and tiles */
.dp-view { background-color: #ffffff; }
.dp-head { color: #1e3287; }
.dp-rule { background-image: linear-gradient(to right, #c6d4ea, #ffffff); min-height: 1px; }
button.dp-tile { padding: 4px; border: 1px solid transparent; border-radius: 3px; background: none; box-shadow: none; }
button.dp-tile:hover { border-color: #b8d6fb; background-image: linear-gradient(to bottom, #fafbfd, #ebf3fd); }
button.dp-tile.sel { border-color: #84acdd; background-image: linear-gradient(to bottom, #dcebfc, #c1dcfc); }
button.dp-tile label { color: #000000; }
/* Network and Sharing Center */
.nsc-node label { color: #000000; }
.nsc-line { background-color: #3c7fb1; min-height: 2px; }
.nsc-line-bad { background-color: #c4161c; min-height: 2px; }
.nsc-panel { border-top: 1px solid #d5dfef; padding-top: 6px; }
.nsc-k { color: #555555; }
/* User Accounts: the user tile (picture with the glass frame) */
.ua-tile { border: 1px solid #9eb5d2; border-radius: 4px; padding: 3px; background-image: linear-gradient(to bottom, #ffffff, #e2ebf6); }
.ua-account { padding: 4px; border: 1px solid transparent; border-radius: 3px; background: none; box-shadow: none; }
.ua-account:hover { border-color: #b8d6fb; background-image: linear-gradient(to bottom, #fafbfd, #ebf3fd); }
.ua-account label { color: #000000; }
.ua-name { font-size: 11pt; }
/* User Account Control Settings: description box next to the slider */
.uac-desc { border: 1px solid #c8d3e2; background-color: #ffffff; padding: 10px 12px; }
.uac-end { color: #1e3287; }
/* Ease of Access Center: the big quick-access buttons */
button.eoa-tool { padding: 4px 8px; border: 1px solid transparent; border-radius: 3px; background: none; box-shadow: none; }
button.eoa-tool:hover { border-color: #b8d6fb; background-image: linear-gradient(to bottom, #fafbfd, #ebf3fd); }
button.eoa-tool label { color: #0066cc; }
.eoa-box { border: 1px solid #c8d3e2; background-image: linear-gradient(to bottom, #ffffff, #f2f6fb); padding: 8px 10px; }
.eoa-h { color: #1e3287; font-size: 10pt; }
/* command links (green arrow, dark blue text; a pale blue box under the pointer) */
button.cp-cmdlink { padding: 5px 8px 6px 6px; border: 1px solid transparent; border-radius: 3px; background: none; box-shadow: none; }
button.cp-cmdlink:hover { border-color: #c6dcf4; background-image: linear-gradient(to bottom, #f5faff, #e3effd); }
button.cp-cmdlink:active { border-color: #a6c6ec; background-image: linear-gradient(to bottom, #e3effd, #d3e5fb); }
button.cp-cmdlink label.cl-title { color: #151c55; font-size: 10.5pt; }
button.cp-cmdlink:hover label.cl-title { color: #0b2a9b; }
button.cp-cmdlink label.cl-note { color: #151c55; }
/* Set Program Access and Computer Defaults: the configuration rows */
.pa-list { border: 1px solid #c8d3e2; }
.pa-row { background-image: linear-gradient(to bottom, #ffffff, #eef3fa); border-bottom: 1px solid #dbe3ee; padding: 3px 4px 3px 8px; }
.pa-body { padding: 8px 12px 10px 30px; border-bottom: 1px solid #dbe3ee; }
button.pa-chev { padding: 0 6px; min-height: 0; border: 1px solid transparent; border-radius: 3px; background: none; box-shadow: none; }
button.pa-chev:hover { border-color: #a5b8d1; background-image: linear-gradient(to bottom, #fbfdff, #dcebfc); }
button.pa-chev label { color: #1e3287; }
/* the list under a long drop-down (Win7: white, thin dark border, blue selection) */
window.cp-listpop { border: 1px solid #646464; background-color: #ffffff; }
window.cp-listpop treeview.view { background-color: #ffffff; padding: 1px 4px; }
window.cp-listpop treeview.view:selected, window.cp-listpop treeview.view:selected:focus { background-color: #3399ff; background-image: none; border-color: #3399ff; color: #ffffff; }
/* Region and Language examples */
.rl-ex { border: 1px solid #d5dfef; padding: 6px 10px; }
"#;

pub(crate) fn css() {
    super::admin::css();
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if DONE.with(|d| d.replace(true)) { return; }
    aerode::theme::load_css(CSS);
}

/// The sub page asked for (`users/picture` → "picture") when the page id has one.
pub(crate) fn sub(cp: &Rc<Cp>, applet: &str) -> Option<String> {
    let h = cp.history.borrow();
    let last = h.last()?;
    last.strip_prefix(applet)?.strip_prefix('/').map(|s| s.to_string()).filter(|s| !s.is_empty())
}

/// Add the sub page's level to the breadcrumb (after `Cp::open` has written the applet's).
pub(crate) fn sub_crumb(cp: &Rc<Cp>, crumb: &str) {
    let (me, c) = (cp.clone(), crumb.to_string());
    glib::idle_add_local_once(move || {
        let cur = me.crumb.text().to_string();
        me.crumb.set_text(&format!("{cur} ▸ {c}"));
        me.win.set_title(&c);
    });
}

/// Leave a sub page after its job is done (Win7 goes back to where you came from); `fallback`
/// when the sub page was opened directly.
pub(crate) fn back(cp: &Rc<Cp>, fallback: &str) {
    let prev = { let mut h = cp.history.borrow_mut(); h.pop(); h.pop() };
    let me = cp.clone();
    let p = prev.unwrap_or_else(|| fallback.to_string());
    // not from inside a handler of the page being replaced
    glib::idle_add_local_once(move || me.open(&p));
}

/// Open another page later (from a handler of the current page).
pub(crate) fn go(cp: &Rc<Cp>, page: &str) {
    let (me, p) = (cp.clone(), page.to_string());
    glib::idle_add_local_once(move || me.open(&p));
}

/// Win7 "big" task link: 32 px icon, blue link, grey description under it (Default Programs,
/// Network and Sharing Center, Ease of Access).
pub(crate) fn task(icon: &[&str], name: &str, desc: &str, f: impl Fn() + 'static) -> gtk::Box {
    let h = hbox(10);
    let i = img(icon, 32);
    i.set_valign(gtk::Align::Start);
    h.pack_start(&i, false, false, 0);
    let v = vbox(1);
    v.pack_start(&link(name, f), false, false, 0);
    if !desc.is_empty() {
        let d = label(desc, "cp-task-desc");
        d.set_max_width_chars(70);
        v.pack_start(&d, false, false, 0);
    }
    h.pack_start(&v, true, true, 0);
    h
}

/// Win7's green command link arrow (aero.msstyles Button / BP_COMMANDLINKGLYPH; 1 normal, 2 hot,
/// 3 pressed), None without the extracted theme.
pub(crate) fn command_glyph(state: i32) -> Option<Pixbuf> {
    let f = aerode::glass::theme()?.frame("Button", 7, state)?;
    let (w, h) = (f.width() as i32, f.height() as i32);
    Some(Pixbuf::from_bytes(&glib::Bytes::from_owned(f.into_raw()), gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, w, h, w * 4))
}

/// A Win7 command link: the green arrow, the action in dark blue and a note under it; the arrow
/// and a light blue box light up under the pointer (Default Programs, Network and Sharing Center
/// wizards, User Accounts).
pub(crate) fn command_link(name: &str, note: &str, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::new();
    b.style_context().add_class("cp-cmdlink");
    b.set_relief(gtk::ReliefStyle::None);
    b.set_halign(gtk::Align::Start);
    let h = hbox(6);
    let arrow = gtk::Image::new();
    let (n, hot) = (command_glyph(1), command_glyph(2));
    match &n { Some(p) => arrow.set_from_pixbuf(Some(p)), None => arrow.set_from_pixbuf(pix(&["go-next"], 16).as_ref()) }
    arrow.set_valign(gtk::Align::Start);
    arrow.set_margin_top(1);
    h.pack_start(&arrow, false, false, 0);
    let v = vbox(1);
    let t = gtk::Label::new(Some(name));
    t.set_xalign(0.0);
    t.style_context().add_class("cl-title");
    v.pack_start(&t, false, false, 0);
    if !note.is_empty() {
        let d = gtk::Label::new(Some(note));
        d.set_xalign(0.0);
        d.set_line_wrap(true);
        d.set_max_width_chars(60);
        d.style_context().add_class("cl-note");
        v.pack_start(&d, false, false, 0);
    }
    h.pack_start(&v, true, true, 0);
    b.add(&h);
    if let (Some(n), Some(hot)) = (n, hot) {
        let a = arrow.clone();
        b.connect_state_flags_changed(move |b, _| a.set_from_pixbuf(Some(if b.state_flags().contains(gtk::StateFlags::PRELIGHT) { &hot } else { &n })));
    }
    b.connect_clicked(move |_| f());
    b
}

/// A drop-down for hundreds of entries (formats, countries, locales): Win7 opens a short list
/// with a scroll bar under the box instead of a menu as tall as the screen.
/// It stays a ComboBoxText (ids, "changed") for the code using it; only the popup differs.
pub(crate) fn long_combo() -> gtk::ComboBoxText {
    let c = gtk::ComboBoxText::new();
    // GTK's menu is taken down as soon as it opens (mouse or keyboard) and the list shown instead
    c.connect_popup_shown_notify(|c| {
        if !c.is_popup_shown() { return; }
        c.popdown();
        let me = c.clone();
        glib::idle_add_local_once(move || list_popup(&me));
    });
    c
}

/// The list under a [`long_combo`]: 20 rows, the current one selected and scrolled into view;
/// a click picks, Escape or a click elsewhere closes it.
fn list_popup(c: &gtk::ComboBoxText) {
    let Some(model) = c.model() else { return };
    let Some(top) = c.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()) else { return };
    let Some(gw) = c.window() else { return };
    let w = gtk::Window::new(gtk::WindowType::Popup);
    w.set_type_hint(gdk::WindowTypeHint::Combo);
    w.set_transient_for(Some(&top));
    w.set_attached_to(Some(c));
    w.style_context().add_class("cp-listpop");
    let tv = gtk::TreeView::with_model(&model);
    tv.set_headers_visible(false);
    tv.set_hover_selection(true);
    tv.set_activate_on_single_click(true);
    let col = gtk::TreeViewColumn::new();
    let r = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &r, true);
    // a ComboBoxText's model: 0 = text, 1 = id
    TreeViewColumnExt::add_attribute(&col, &r, "text", 0);
    tv.append_column(&col);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.set_shadow_type(gtk::ShadowType::None);
    let a = c.allocation();
    // up to 20 rows: a short list gets its own height, a long one scrolls
    const ROW: i32 = 25;
    let rows = model.iter_n_children(None).clamp(1, 20);
    sw.set_size_request(a.width(), -1);
    sw.set_propagate_natural_height(true);
    sw.set_max_content_height(20 * ROW);
    sw.add(&tv);
    w.add(&sw);
    let (_, ox, oy) = gw.origin();
    let (x, y) = (ox + a.x(), oy + a.y() + a.height());
    // above the box when there is no room under it
    let mon = c.display().monitor_at_window(&gw).map(|m| m.workarea());
    let h = rows * ROW + 2;
    let y = match mon { Some(m) if y + h > m.y() + m.height() => (oy + a.y() - h).max(m.y()), _ => y };
    w.move_(x, y);
    let close: Rc<dyn Fn()> = { let w = w.clone(); Rc::new(move || { if let Some(s) = w.display().default_seat() { s.ungrab(); } w.grab_remove(); w.close(); }) };
    {
        let (cc, f) = (c.clone(), close.clone());
        tv.connect_row_activated(move |_, p, _| { if let Some(it) = cc.model().and_then(|m| m.iter(p)) { cc.set_active_iter(Some(&it)); } f(); });
    }
    { let f = close.clone(); w.connect_key_press_event(move |_, e| { if e.keyval() == gdk::keys::constants::Escape { f(); return glib::Propagation::Stop; } glib::Propagation::Proceed }); }
    {
        let f = close.clone();
        w.connect_button_press_event(move |w, e| {
            let (px, py) = e.root();
            let (wx, wy) = w.position();
            let (ww, wh) = w.size();
            if px < wx as f64 || py < wy as f64 || px >= (wx + ww) as f64 || py >= (wy + wh) as f64 { f(); return glib::Propagation::Stop; }
            glib::Propagation::Proceed
        });
    }
    w.show_all();
    w.grab_add();
    if let (Some(s), Some(win)) = (w.display().default_seat(), w.window()) {
        let _ = s.grab(&win, gdk::SeatCapabilities::ALL, true, None, None, None);
    }
    tv.grab_focus();
    if let Some(p) = c.active_iter().and_then(|it| model.path(&it)) {
        tv.set_cursor(&p, None::<&gtk::TreeViewColumn>, false);
        let tv2 = tv.clone();
        glib::idle_add_local_once(move || tv2.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, true, 0.0, 0.0));
    }
}

/// A Win7 command bar button (flat until hovered).
pub(crate) fn cmd_button(label: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_relief(gtk::ReliefStyle::None);
    b.set_focus_on_click(false);
    b
}

/// A command bar never widens the window: what doesn't fit is cut off (Win7 hides it behind »).
pub(crate) fn clip(bar: &gtk::Box) -> gtk::ScrolledWindow {
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
    sw.set_shadow_type(gtk::ShadowType::None);
    sw.add(bar);
    sw
}

/// "Organize ▾" style button that pops up a menu.
pub(crate) fn menu_button(label: &str, menu: &gtk::Menu) -> gtk::Button {
    let b = cmd_button(&format!("{label} ▾"));
    let m = menu.clone();
    b.connect_clicked(move |b| { m.show_all(); m.popup_at_widget(b, gdk::Gravity::SouthWest, gdk::Gravity::NorthWest, None); });
    b
}

pub(crate) fn menu_item(menu: &gtk::Menu, label: &str, f: impl Fn() + 'static) -> gtk::MenuItem {
    let mi = gtk::MenuItem::with_mnemonic(label);
    mi.connect_activate(move |_| f());
    menu.append(&mi);
    mi
}

/// The Win7 user picture with its frame.
pub(crate) fn user_tile(path: Option<&std::path::Path>, size: i32) -> gtk::Widget {
    let f = gtk::Box::new(gtk::Orientation::Vertical, 0);
    f.style_context().add_class("ua-tile");
    let i = gtk::Image::new();
    set_picture(&i, path, size);
    f.pack_start(&i, false, false, 0);
    f.set_halign(gtk::Align::Start);
    f.set_valign(gtk::Align::Start);
    f.upcast()
}

pub(crate) fn set_picture(i: &gtk::Image, path: Option<&std::path::Path>, size: i32) {
    let pb = path.and_then(|p| Pixbuf::from_file_at_scale(p, size, size, true).ok())
        .or_else(|| config::data_file("avatar.png").and_then(|p| Pixbuf::from_file_at_scale(p, size, size, true).ok()))
        .or_else(|| pix(&["avatar-default", "system-users"], size));
    i.set_from_pixbuf(pb.as_ref());
}

/// OK/Cancel-style row of buttons aligned right (Win7 pages end with them).
pub(crate) fn button_row(buttons: &[&gtk::Button]) -> gtk::Box {
    let h = hbox(8);
    h.set_halign(gtk::Align::End);
    h.set_margin_top(14);
    for b in buttons { b.set_size_request(90, -1); h.pack_start(*b, false, false, 0); }
    h
}

/// Format a number with thousands separators (Win7 "Bytes: 1,234,567").
pub(crate) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 { out.push(','); }
        out.push(c);
    }
    out
}

/// A Win7 dialog with content area padding and the given buttons.
pub(crate) fn dialog(parent: Option<&gtk::Window>, title: &str, buttons: &[(&str, gtk::ResponseType)]) -> gtk::Dialog {
    let d = gtk::Dialog::with_buttons(Some(title), parent, gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT, buttons);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    d.set_skip_taskbar_hint(false);
    d
}

#[cfg(test)]
mod tests {
    #[test]
    fn thousands() {
        assert_eq!(super::thousands(0), "0");
        assert_eq!(super::thousands(999), "999");
        assert_eq!(super::thousands(1000), "1,000");
        assert_eq!(super::thousands(1234567), "1,234,567");
    }
}
