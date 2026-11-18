//! The screen-sharing picker: xdg-desktop-portal-wlr's chooser (chooser_type=dmenu). Reads its
//! "Monitor: NAME DESC" / "Window: TITLE (IDENT)" lines on stdin, shows them with live pictures in a Win7
//! dialog and prints the chosen line; nothing (and exit 1) when cancelled.
//! `--demo` lists the current screens and windows without a portal.

use aerode::{theme, wmclient, wmipc};
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::io::Read;
use std::rc::Rc;

const CSS: &str = r#"
.share-body { background-color: #ffffff; padding: 12px 16px 8px 16px; }
.share-instruction { font-size: 12pt; color: #003399; }
.share-group { color: #1e3287; margin-top: 10px; }
.share-rule { background-color: #e2e6ef; min-height: 1px; margin-bottom: 6px; }
.share-tile { padding: 6px; border: 1px solid transparent; border-radius: 3px; }
.share-tile:hover { background-image: linear-gradient(to bottom, #f5fafd, #e8f3fd); border-color: #b8d6fb; }
flowboxchild:selected .share-tile, flowboxchild:selected { background-color: transparent; }
flowboxchild:selected .share-tile { background-image: linear-gradient(to bottom, #dcebfc, #c1dcfc); border-color: #7da2ce; }
flowboxchild:selected label { color: #000000; }
.share-foot { background-color: #f0f0f0; border-top: 1px solid #dfdfdf; padding: 10px 12px; }
"#;

const TW: i32 = 200;
const TH: i32 = 124;

enum Kind { Screen(String), Window(String) }

/// Which thing a portal line names: the output name, or the window's share identifier.
fn parse(line: &str) -> Option<Kind> {
    if let Some(rest) = line.strip_prefix("Monitor: ") {
        return Some(Kind::Screen(rest.split(' ').next()?.to_string()));
    }
    let rest = line.strip_prefix("Window: ")?;
    let open = rest.rfind(" (")?;
    Some(Kind::Window(rest[open + 2..].strip_suffix(')')?.to_string()))
}

#[test]
fn portal_lines() {
    assert!(matches!(parse("Monitor: DP-1 Dell Inc. U2412M"), Some(Kind::Screen(n)) if n == "DP-1"));
    assert!(matches!(parse("Window: a (b) (XyZ12)"), Some(Kind::Window(i)) if i == "XyZ12"));
    assert!(parse("garbage").is_none());
}

/// A picture scaled into the tile, or a grey box when there is none.
fn picture(img: Option<cairo::ImageSurface>) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_size_request(TW, TH);
    d.connect_draw(move |_, cr| {
        match &img {
            Some(s) => {
                let k = (TW as f64 / s.width() as f64).min(TH as f64 / s.height() as f64);
                let (w, h) = (s.width() as f64 * k, s.height() as f64 * k);
                cr.translate((TW as f64 - w) / 2.0, (TH as f64 - h) / 2.0);
                cr.rectangle(-1.0, -1.0, w + 2.0, h + 2.0);
                cr.set_source_rgb(0.62, 0.68, 0.76);
                cr.fill().ok();
                cr.scale(k, k);
                cr.set_source_surface(s, 0.0, 0.0).ok();
                cr.paint().ok();
            }
            None => { cr.set_source_rgb(0.9, 0.9, 0.9); cr.rectangle(0.0, 0.0, TW as f64, TH as f64); cr.fill().ok(); }
        }
        glib::Propagation::Stop
    });
    d
}

fn tile(img: Option<cairo::ImageSurface>, icon: Option<gtk::gdk_pixbuf::Pixbuf>, icon_name: &str, text: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.style_context().add_class("share-tile");
    b.pack_start(&picture(img), false, false, 0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let i = match icon { Some(p) => gtk::Image::from_pixbuf(Some(&p)), None => gtk::Image::from_icon_name(Some(icon_name), gtk::IconSize::Menu) };
    row.pack_start(&i, false, false, 0);
    let l = gtk::Label::new(Some(text));
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(24);
    l.set_xalign(0.0);
    row.pack_start(&l, true, true, 0);
    b.pack_start(&row, false, false, 0);
    b
}

fn group(body: &gtk::Box, title: &str) -> gtk::FlowBox {
    let l = gtk::Label::new(Some(title));
    l.set_xalign(0.0);
    l.style_context().add_class("share-group");
    body.pack_start(&l, false, false, 0);
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.style_context().add_class("share-rule");
    body.pack_start(&rule, false, false, 0);
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::Single);
    f.set_homogeneous(true);
    f.set_min_children_per_line(3);
    f.set_max_children_per_line(3);
    f.set_activate_on_single_click(false);
    body.pack_start(&f, false, false, 0);
    f
}

fn main() -> std::process::ExitCode {
    let mut lines: Vec<String> = Vec::new();
    if std::env::args().any(|a| a == "--demo") {
        let st = wmipc::state();
        for o in outputs() { lines.push(format!("Monitor: {} {} {}", o.name, o.make, o.model)); }
        for w in st.windows.iter().rev().filter(|w| !w.share_id.is_empty()) { lines.push(format!("Window: {} ({})", w.title, w.share_id)); }
    } else {
        let mut s = String::new();
        let _ = std::io::stdin().read_to_string(&mut s);
        lines = s.lines().filter(|l| !l.is_empty()).map(String::from).collect();
    }

    gtk::init().expect("gtk");
    theme::init_app();
    theme::load_css(CSS);
    let chosen: Rc<RefCell<Option<String>>> = Rc::default();

    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title("Share Screen");
    win.set_icon_name(Some("video-display"));
    win.set_position(gtk::WindowPosition::Center);
    win.set_resizable(false);
    win.set_keep_above(true);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.style_context().add_class("share-body");
    let head = gtk::Label::new(Some(if lines.is_empty() { "There is nothing to share." } else { "Choose what to share" }));
    head.set_xalign(0.0);
    head.style_context().add_class("share-instruction");
    body.pack_start(&head, false, false, 0);
    let sub = gtk::Label::new(Some("The program that asked can see what you choose until it stops sharing."));
    sub.set_xalign(0.0);
    sub.set_margin_top(4);
    body.pack_start(&sub, false, false, 0);

    let st = wmipc::state();
    let wm = wmclient::Wm::connect().ok();
    let outs = outputs();
    let shot = wmipc::call(&wmipc::Req::Screenshot).as_ref().and_then(wmipc::read_image);
    let (mut screens, mut windows): (Vec<(String, gtk::Box)>, Vec<(String, gtk::Box)>) = (vec![], vec![]);
    for (n, line) in lines.iter().enumerate() {
        match parse(line) {
            Some(Kind::Screen(name)) => {
                let o = outs.iter().find(|o| o.name == name);
                // the output's part of the whole-screen picture
                let img = o.zip(shot.as_ref()).and_then(|(o, s)| {
                    let c = cairo::ImageSurface::create(cairo::Format::ARgb32, o.w.max(1), o.h.max(1)).ok()?;
                    let cr = cairo::Context::new(&c).ok()?;
                    cr.set_source_surface(s, -o.x as f64, -o.y as f64).ok()?;
                    cr.paint().ok()?;
                    drop(cr);
                    Some(c)
                });
                let label = if outs.len() > 1 { format!("Screen {} ({name})", screens.len() + 1) } else { "Entire screen".to_string() };
                screens.push((line.clone(), tile(img, None, "video-display", &label)));
            }
            Some(Kind::Window(ident)) => {
                let w = st.windows.iter().find(|w| w.share_id == ident);
                let img = w.and_then(|w| wmipc::call(&wmipc::Req::Thumb { id: w.id, w: TW * 2, h: TH * 2 })).as_ref().and_then(wmipc::read_image);
                let icon = w.zip(wm.as_ref()).and_then(|(w, wm)| wm.icon(w.id, 16));
                let title = w.map(|w| w.title.clone()).unwrap_or_else(|| line.trim_start_matches("Window: ").to_string());
                windows.push((line.clone(), tile(img, icon, "application-x-executable", &title)));
            }
            None => eprintln!("aerode-sharepicker: unknown line {n}: {line}"),
        }
    }

    let share = gtk::Button::with_mnemonic("_Share");
    share.set_sensitive(false);
    let boxes: Rc<RefCell<Vec<gtk::FlowBox>>> = Rc::default();
    for (title, items) in [("Screens", screens), ("Windows", windows)] {
        if items.is_empty() { continue; }
        let f = group(&body, title);
        let keys: Vec<String> = items.iter().map(|i| i.0.clone()).collect();
        for (_, t) in items { f.add(&t); }
        let (c, s, all) = (chosen.clone(), share.clone(), boxes.clone());
        f.connect_selected_children_changed(move |f| {
            let Some(child) = f.selected_children().into_iter().next() else { return };
            // one choice across both groups
            for other in all.borrow().iter().filter(|o| *o != f) { other.unselect_all(); }
            *c.borrow_mut() = keys.get(child.index() as usize).cloned();
            s.set_sensitive(true);
        });
        let s = share.clone();
        f.connect_child_activated(move |_, _| s.clicked());
        boxes.borrow_mut().push(f);
    }

    outer.pack_start(&body, true, true, 0);
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.style_context().add_class("share-foot");
    let cancel = gtk::Button::with_label("Cancel");
    share.set_size_request(86, -1);
    cancel.set_size_request(86, -1);
    foot.pack_end(&cancel, false, false, 0);
    foot.pack_end(&share, false, false, 0);
    outer.pack_start(&foot, false, false, 0);
    win.add(&outer);

    let done = Rc::new(std::cell::Cell::new(false));
    { let (w, d) = (win.clone(), done.clone()); share.connect_clicked(move |_| { d.set(true); w.close(); }); }
    { let w = win.clone(); cancel.connect_clicked(move |_| w.close()); }
    win.connect_key_press_event(|w, e| {
        if e.keyval() == gtk::gdk::keys::constants::Escape { w.close(); }
        glib::Propagation::Proceed
    });
    win.connect_destroy(|_| gtk::main_quit());
    win.show_all();
    share.set_can_default(true);
    share.grab_default();
    gtk::main();

    match chosen.borrow().as_ref().filter(|_| done.get()) {
        Some(line) => { println!("{line}"); std::process::ExitCode::SUCCESS }
        None => std::process::ExitCode::FAILURE,
    }
}

fn outputs() -> Vec<wmipc::Output> {
    wmipc::call(&wmipc::Req::Outputs).and_then(|v| serde_json::from_value(v.get("outputs")?.clone()).ok()).unwrap_or_default()
}
