//! Taskbar and Start Menu Properties: Taskbar / Start Menu / Toolbars tabs, Customize Start Menu.
use super::*;
use aerode::startmenu::{start_item, POWER_ACTIONS, START_ITEMS};
use std::collections::HashMap;

pub fn page_taskbar(cp: &Rc<Cp>) -> gtk::Widget {
    cp.side_link("Personalization", "personalization");
    let o = config::get().taskbar;
    let sheet = Sheet::new(cp);

    // ---------------------------------------------------------------- Taskbar
    let t = vbox(10);
    let app = vbox(6);
    let locked = check("_Lock the taskbar", o.locked);
    let autohide = check("A_uto-hide the taskbar", o.autohide);
    let small = check("Use _small icons", o.small_icons);
    for c in [&locked, &autohide, &small] { app.pack_start(c, false, false, 0); sheet.watch_check(c); }
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    let loc = combo(&[("bottom", "Bottom"), ("left", "Left"), ("right", "Right"), ("top", "Top")], &o.location);
    let buttons = combo(&[("always", "Always combine, hide labels"), ("full", "Combine when taskbar is full"), ("never", "Never combine")], &o.buttons);
    for (i, (l, w)) in [("Taskbar _location on screen:", &loc), ("Taskbar _buttons:", &buttons)].into_iter().enumerate() {
        let lab = gtk::Label::with_mnemonic(l);
        lab.set_mnemonic_widget(Some(w));
        lab.set_xalign(0.0);
        g.attach(&lab, 0, i as i32, 1, 1);
        w.set_hexpand(true);
        g.attach(w, 1, i as i32, 1, 1);
        sheet.watch_combo(w);
    }
    app.pack_start(&g, false, false, 4);
    t.pack_start(&group("Taskbar appearance", &app), false, false, 0);
    let na = hbox(10);
    let nal = text("Customize which icons and notifications appear in the notification area.");
    nal.set_max_width_chars(36);
    na.pack_start(&nal, true, true, 0);
    let customize = gtk::Button::with_mnemonic("_Customize...");
    customize.set_valign(gtk::Align::Center);
    // the notification area settings live with the tray; open them when that applet exists
    let notif = crate::registry::applets().into_iter().find(|a| a.id.contains("notif") || a.id.contains("trayicons")).map(|a| a.id);
    match notif {
        Some(id) => { let cp = cp.clone(); customize.connect_clicked(move |_| cp.open(id)); }
        None => { customize.set_sensitive(false); customize.set_tooltip_text(Some("Notification area settings are not available yet.")); }
    }
    na.pack_start(&customize, false, false, 0);
    t.pack_start(&group("Notification area", &na), false, false, 0);
    let peek_box = vbox(6);
    peek_box.pack_start(&text("Temporarily view the desktop when you move your mouse to the Show desktop button at end of the taskbar."), false, false, 0);
    let peek = check("Use _Aero Peek to preview the desktop", o.aero_peek);
    sheet.watch_check(&peek);
    peek_box.pack_start(&peek, false, false, 0);
    t.pack_start(&group("Preview desktop with Aero Peek", &peek_box), false, false, 0);
    sheet.tab("Taskbar", &t);

    // ---------------------------------------------------------------- Start Menu
    let s = vbox(10);
    let cust_row = hbox(10);
    let cl = text("To customize how links, icons, and menus look and behave in the Start menu, click Customize.");
    cl.set_max_width_chars(36);
    cust_row.pack_start(&cl, true, true, 0);
    let cust = gtk::Button::with_mnemonic("_Customize...");
    cust.set_valign(gtk::Align::Center);
    cust_row.pack_start(&cust, false, false, 0);
    s.pack_start(&cust_row, false, false, 4);
    let pr = hbox(10);
    let pl = gtk::Label::with_mnemonic("_Power button action:");
    pr.pack_start(&pl, false, false, 0);
    let power = combo(POWER_ACTIONS, &o.power_button);
    // like Win7, Sleep and Hibernate are only offered when the machine can do them (asked off the GTK thread)
    for (id, what) in [("sleep", "CanSuspend"), ("hibernate", "CanHibernate")] {
        let power = power.clone();
        util::output_async(["busctl", "call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", what].iter().map(|s| s.to_string()).collect(), move |r| {
            let Some(r) = r else { return };
            if r.contains("\"yes\"") || r.contains("\"challenge\"") || power.active_id().as_deref() == Some(id) { return; }
            // find the row by walking the model (rows before it may already be gone)
            if let Some(model) = power.model() {
                if let Some(it) = model.iter_first() {
                    let mut i = 0;
                    loop {
                        let v: Option<String> = model.value(&it, 1).get().ok();
                        if v.as_deref() == Some(id) { ComboBoxTextExt::remove(&power, i); break; }
                        i += 1;
                        if !model.iter_next(&it) { break; }
                    }
                }
            }
        });
    }
    pl.set_mnemonic_widget(Some(&power));
    sheet.watch_combo(&power);
    pr.pack_start(&power, false, false, 0);
    s.pack_start(&pr, false, false, 0);
    let priv_box = vbox(6);
    let track_p = check("Store and display recently opened _programs in the Start menu", o.track_programs);
    let track_i = check("Store and display recently opened _items in the Start menu and the taskbar", o.track_items);
    for c in [&track_p, &track_i] { priv_box.pack_start(c, false, false, 0); sheet.watch_check(c); }
    s.pack_start(&group("Privacy", &priv_box), false, false, 0);
    sheet.tab("Start Menu", &s);
    // the Customize dialog edits copies that Apply saves
    let items: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(o.start_items.clone()));
    let counts = Rc::new(std::cell::Cell::new((o.recent_programs, o.jump_items)));
    { let (cp, items, counts, d) = (cp.clone(), items.clone(), counts.clone(), sheet.dirty()); cust.connect_clicked(move |_| {
        if customize_start(&parent_window(&cp), &items, &counts) { d(); }
    }); }

    // ---------------------------------------------------------------- Toolbars
    let tb = vbox(8);
    tb.pack_start(&text("Select which toolbars to add to the taskbar."), false, false, 0);
    let lb = vbox(2);
    lb.set_border_width(4);
    let mut tool_checks = vec![];
    for (id, l) in [("address", "Address"), ("links", "Links"), ("tabletpc", "Tablet PC Input Panel"), ("desktop", "Desktop")] {
        let c = check(l, o.toolbars.iter().any(|t| t == id));
        sheet.watch_check(&c);
        lb.pack_start(&c, false, false, 0);
        tool_checks.push((id, c));
    }
    let frame = gtk::Frame::new(None);
    frame.set_shadow_type(gtk::ShadowType::In);
    let white = gtk::EventBox::new();
    white.style_context().add_class("view");
    white.add(&lb);
    frame.add(&white);
    frame.set_size_request(-1, 200);
    tb.pack_start(&frame, false, false, 0);
    sheet.tab("Toolbars", &tb);

    sheet.on_apply(move || {
        let was_tracking = config::get().taskbar;
        let small_on = small.is_active();
        let (rp, ji) = counts.get();
        let it = items.borrow().clone();
        let tools: Vec<String> = tool_checks.iter().filter(|(_, c)| c.is_active()).map(|(id, _)| id.to_string()).collect();
        config::update(|c| {
            let t = &mut c.taskbar;
            t.locked = locked.is_active();
            t.autohide = autohide.is_active();
            t.small_icons = small_on;
            t.location = loc.active_id().map(|s| s.to_string()).unwrap_or_else(|| "bottom".into());
            t.buttons = buttons.active_id().map(|s| s.to_string()).unwrap_or_else(|| "always".into());
            t.aero_peek = peek.is_active();
            t.power_button = power.active_id().map(|s| s.to_string()).unwrap_or_else(|| "shutdown".into());
            t.track_programs = track_p.is_active();
            t.track_items = track_i.is_active();
            t.recent_programs = rp;
            t.jump_items = ji;
            t.start_items = it;
            t.toolbars = tools;
            // Win7: 40 px with large icons, 30 px with small ones
            c.taskbar_height = if small_on { 30 } else { 40 };
            // turning a privacy option off clears what was stored
            if !c.taskbar.track_programs { c.usage.clear(); }
        });
        if was_tracking.track_items != track_i.is_active() { write_xsettings(); }
        reload_shell();
        true
    });
    sheet.widget()
}

/// Win7 "Customize Start Menu": link / menu / don't display for folders, checkboxes for the rest,
/// and the Start menu size. Returns true when OK was pressed.
fn customize_start(parent: &gtk::Window, items: &Rc<RefCell<HashMap<String, String>>>, counts: &Rc<std::cell::Cell<(u32, u32)>>) -> bool {
    let d = gtk::Dialog::with_buttons(Some("Customize Start Menu"), Some(parent), gtk::DialogFlags::MODAL,
        &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(420, 560);
    let c = d.content_area();
    c.set_spacing(8);
    c.set_border_width(10);
    c.pack_start(&text("You can customize how links, icons, and menus look and behave on the Start menu."), false, false, 0);
    let list = vbox(1);
    list.set_border_width(4);
    let icon_of = |k: &str| match k {
        "computer" => "computer", "control" => "preferences-system", "defaults" => "preferences-desktop-default-applications", "devices" => "printer",
        "documents" => "folder-documents", "downloads" => "folder-download", "games" => "applications-games", "help" => "help-browser",
        "music" => "folder-music", "network" => "network-workgroup", "user" => "user-home", "pictures" => "folder-pictures",
        "recent" => "document-open-recent", "run" => "system-run", "videos" => "folder-videos", "searchcp" => "system-search", _ => "",
    };
    // (key, the value each widget stands for, widget)
    let widgets: Rc<RefCell<Vec<(String, &'static str, gtk::ToggleButton)>>> = Rc::new(RefCell::new(vec![]));
    let fill = {
        let (list, widgets, items) = (list.clone(), widgets.clone(), items.clone());
        move |use_defaults: bool| {
            for ch in list.children() { list.remove(&ch); }
            widgets.borrow_mut().clear();
            for (key, label, kind, def) in START_ITEMS {
                let cur = if use_defaults { def.to_string() } else { items.borrow().get(*key).cloned().unwrap_or_else(|| start_item(key)) };
                let icon = icon_of(key);
                if *kind == "link" {
                    let h = hbox(6);
                    if !icon.is_empty() { h.pack_start(&util::image(icon, 16), false, false, 0); }
                    h.pack_start(&gtk::Label::new(Some(label)), false, false, 0);
                    list.pack_start(&h, false, false, 2);
                    let rs = radios(&["Display as a link", "Display as a menu", "Don't display this item"], match cur.as_str() { "menu" => 1, "hide" => 2, _ => 0 });
                    for (r, val) in rs.iter().zip(["link", "menu", "hide"]) {
                        r.set_margin_start(24);
                        list.pack_start(r, false, false, 0);
                        widgets.borrow_mut().push((key.to_string(), val, r.clone().upcast()));
                    }
                } else {
                    let cb = gtk::CheckButton::new();
                    let h = hbox(6);
                    if !icon.is_empty() { h.pack_start(&util::image(icon, 16), false, false, 0); }
                    h.pack_start(&gtk::Label::new(Some(label)), false, false, 0);
                    cb.add(&h);
                    cb.set_active(cur == "on");
                    list.pack_start(&cb, false, false, 2);
                    widgets.borrow_mut().push((key.to_string(), "on", cb.upcast()));
                }
            }
            list.show_all();
        }
    };
    fill(false);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_shadow_type(gtk::ShadowType::In);
    let white = gtk::EventBox::new();
    white.style_context().add_class("view");
    white.add(&list);
    sw.add(&white);
    c.pack_start(&sw, true, true, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    let (rp, ji) = counts.get();
    let recent = gtk::SpinButton::with_range(0.0, 30.0, 1.0);
    recent.set_value(rp as f64);
    let jump = gtk::SpinButton::with_range(0.0, 60.0, 1.0);
    jump.set_value(ji as f64);
    for (i, (l, w)) in [("Number of recent programs to display:", &recent), ("Number of recent items to display in Jump Lists:", &jump)].into_iter().enumerate() {
        let lab = gtk::Label::new(Some(l));
        lab.set_xalign(0.0);
        lab.set_hexpand(true);
        g.attach(&lab, 0, i as i32, 1, 1);
        g.attach(w, 1, i as i32, 1, 1);
    }
    c.pack_start(&group("Start menu size", &g), false, false, 0);
    let def = gtk::Button::with_mnemonic("_Use Default Settings");
    def.set_halign(gtk::Align::Start);
    { let (fill, recent, jump) = (fill.clone(), recent.clone(), jump.clone()); def.connect_clicked(move |_| { fill(true); recent.set_value(10.0); jump.set_value(10.0); }); }
    c.pack_start(&def, false, false, 0);
    d.show_all();
    let ok = d.run() == gtk::ResponseType::Ok;
    if ok {
        let mut m = HashMap::new();
        for (key, val, w) in widgets.borrow().iter() {
            let is_check = START_ITEMS.iter().any(|i| i.0 == key && i.2 == "check");
            if is_check { m.insert(key.clone(), if w.is_active() { "on".to_string() } else { "off".to_string() }); }
            else if w.is_active() { m.insert(key.clone(), val.to_string()); }
        }
        *items.borrow_mut() = m;
        counts.set((recent.value() as u32, jump.value() as u32));
    }
    d.close();
    ok
}
