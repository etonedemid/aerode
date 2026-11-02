//! Power Options: Select a power plan (Balanced / Power saver / High performance and plans you
//! create, mapped onto power-profiles-daemon), Change settings for the plan (display-off and sleep
//! timers and plan brightness, on battery / plugged in – kept in config and applied by the shell's
//! watcher), Change advanced power settings, System Settings (power and sleep buttons and the lid
//! through a pkexec-written /etc/systemd/logind.conf.d/aerode.conf, Require a password on wakeup)
//! and Create a power plan.
use super::*;
use super::super::admin::{bg, confirm, elevate_sh, img, q, report, see_also, shield_link};
use super::super::desktop::radios;
use aerode::config::{CustomPlan, PowerPlan};
use aerode::power::{self as pw, PLANS, TIMEOUTS};

fn vbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, s) }
fn hbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, s) }

fn has_lid() -> bool {
    std::fs::read_dir("/proc/acpi/button/lid").map(|mut r| r.next().is_some()).unwrap_or(false)
        || std::path::Path::new("/sys/class/power_supply").read_dir().map(|rd| rd.flatten().any(|e| std::fs::read_to_string(e.path().join("type")).map(|t| t.trim() == "Battery").unwrap_or(false))).unwrap_or(false)
}

fn timer_combo(cur: u32) -> gtk::ComboBoxText {
    let c = gtk::ComboBoxText::new();
    for (m, l) in TIMEOUTS { c.append(Some(&m.to_string()), l); }
    if !c.set_active_id(Some(&cur.to_string())) { c.append(Some(&cur.to_string()), &format!("{cur} minutes")); c.set_active_id(Some(&cur.to_string())); }
    c
}
fn combo_val(c: &gtk::ComboBoxText) -> u32 { c.active_id().and_then(|s| s.parse().ok()).unwrap_or(0) }

pub fn crumb(id: &str) -> String {
    match id {
        "power-buttons" | "power-lid" | "power-password" => "System Settings".into(),
        "power-create" => "Create a Power Plan".into(),
        _ => "Edit Plan Settings".into(),
    }
}

pub fn subpage(cp: &Rc<Cp>, id: &str) -> Option<gtk::Widget> {
    Some(match id {
        "power-buttons" | "power-lid" | "power-password" => page_buttons(cp),
        "power-create" => page_create(cp),
        "power-plan" => page_plan(cp, &config::get().hw.power.plan),
        id => page_plan(cp, id.strip_prefix("power-plan:")?),
    })
}

fn side(cp: &Rc<Cp>) {
    css();
    let o = config::get().hw.power;
    cp.side_link("Require a password on wakeup", "power-password");
    cp.side_link("Choose what the power buttons do", "power-buttons");
    if pw::has_battery() || has_lid() { cp.side_link("Choose what closing the lid does", "power-lid"); }
    cp.side_link("Create a power plan", "power-create");
    let id = format!("power-plan:{}", o.plan);
    for t in ["Choose when to turn off the display", "Change when the computer sleeps"] {
        let (me, id) = (cp.clone(), id.clone());
        super::super::admin::side_action(cp, t, move || me.open(&id));
    }
    see_also(cp, &[("Personalization", "personalization"), ("Windows Mobility Center", "mobility"), ("User Accounts", "users")]);
}

// ------------------------------------------------------------------ Select a power plan

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    side(cp);
    let v = vbox(6);
    v.pack_start(&title("Select a power plan"), false, false, 0);
    let intro = text("Power plans can help you maximize your computer's performance or conserve energy. Make a plan active by selecting it, or choose a plan and customize it by changing its power settings.");
    v.pack_start(&intro, false, false, 0);
    v.pack_start(&link("Tell me more about power plans", || { util::open_uri("https://wiki.archlinux.org/title/Power_management"); }), false, false, 0);
    let o = config::get().hw.power;
    let battery = pw::has_battery();
    // (id, name, description)
    let mut preferred: Vec<(String, String, String)> = PLANS.iter().take(2).map(|p| (p.0.to_string(), if p.0 == "balanced" { "Balanced (recommended)".to_string() } else { p.1.to_string() }, p.2.to_string())).collect();
    for c in &o.custom { preferred.push((c.id.clone(), c.name.clone(), format!("Based on {}.", pw::plan_name(&o, &c.base)))); }
    let more: Vec<(String, String, String)> = vec![(PLANS[2].0.into(), PLANS[2].1.into(), PLANS[2].2.into())];
    v.pack_start(&heading(if battery { "Plans shown on the battery meter" } else { "Preferred plans" }), false, false, 0);
    let mut group: Vec<gtk::RadioButton> = vec![];
    let mut buttons: Vec<(String, gtk::RadioButton)> = vec![];
    let mut add = |list: &gtk::Box, (id, name, desc): &(String, String, String)| {
        let row = hbox(10);
        let r = match group.first() { None => gtk::RadioButton::new(), Some(g) => gtk::RadioButton::from_widget(g) };
        let l = gtk::Label::new(None);
        l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(name)));
        r.add(&l);
        group.push(r.clone());
        row.pack_start(&r, false, false, 0);
        let me = cp.clone();
        let target = format!("power-plan:{id}");
        row.pack_end(&link("Change plan settings", move || me.open(&target)), false, false, 30);
        list.pack_start(&row, false, false, 0);
        let d = text(desc);
        d.set_margin_start(26);
        d.style_context().add_class("hw-dim");
        list.pack_start(&d, false, false, 0);
        buttons.push((id.clone(), r));
    };
    let pl = vbox(4);
    pl.set_margin_start(16);
    for p in &preferred { add(&pl, p); }
    v.pack_start(&pl, false, false, 0);
    // "Show additional plans" ⌄
    let exp = gtk::Expander::new(None);
    let el = gtk::Label::new(None);
    el.set_markup("<span foreground='#1e3287'>Show additional plans</span>");
    exp.set_label_widget(Some(&el));
    exp.set_expanded(o.show_more || more.iter().any(|m| m.0 == o.plan));
    let ml = vbox(4);
    ml.set_margin_start(16);
    ml.set_margin_top(6);
    for p in &more { add(&ml, p); }
    exp.add(&ml);
    exp.set_margin_top(10);
    exp.connect_expanded_notify(|e| { let x = e.is_expanded(); config::update(|c| c.hw.power.show_more = x); });
    v.pack_start(&exp, false, false, 0);
    // select the active plan (power-profiles-daemon may have been switched elsewhere: ask in the background)
    let setting = Rc::new(std::cell::Cell::new(true));
    let mark = { let (b, s) = (buttons.clone(), setting.clone()); move |active: &str| {
        s.set(true);
        if let Some((_, r)) = b.iter().find(|(id, _)| id == active) { r.set_active(true); }
        s.set(false);
    } };
    mark(&o.plan);
    { let (o, mark) = (o.clone(), mark.clone()); bg(move || pw::active_plan(&o), move |a| mark(&a)); }
    for (id, r) in &buttons {
        let (id, s) = (id.clone(), setting.clone());
        r.connect_toggled(move |r| if r.is_active() && !s.get() { pw::set_plan(&id); });
    }
    // laptops: the brightness slider at the bottom
    let br = hbox(8);
    br.set_margin_top(24);
    let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
    slider.set_draw_value(false);
    slider.set_size_request(260, -1);
    br.pack_start(&lbl("Screen brightness:"), false, false, 0);
    br.pack_start(&img(&["display-brightness-low-symbolic", "display-brightness-symbolic", "display-brightness"], 16), false, false, 0);
    br.pack_start(&slider, false, false, 0);
    br.pack_start(&img(&["display-brightness-high-symbolic", "display-brightness-symbolic", "display-brightness"], 16), false, false, 0);
    br.set_no_show_all(true);
    v.pack_start(&br, false, false, 0);
    { let (br, slider) = (br.clone(), slider.clone()); bg(aerode::sys::backlight, move |b| {
        let Some(b) = b else { return };
        slider.set_value(b as f64);
        slider.connect_value_changed(|s| { util::spawn(&["brightnessctl", "set", &format!("{}%", s.value() as u32)]); });
        br.set_no_show_all(false);
        br.show_all();
    }); }
    v.upcast()
}

// ------------------------------------------------------------------ Change settings for the plan

/// Rows of the plan editor: display, sleep (and brightness on laptops), on battery / plugged in.
struct PlanEditor { grid: gtk::Grid, disp: [gtk::ComboBoxText; 2], sleep: [gtk::ComboBoxText; 2], bright: Option<[gtk::Scale; 2]> }

impl PlanEditor {
    fn new(p: &PowerPlan) -> PlanEditor {
        let battery = pw::has_battery();
        let g = gtk::Grid::new();
        g.set_row_spacing(10);
        g.set_column_spacing(16);
        g.set_margin_start(10);
        if battery {
            for (i, (icon, t)) in [(&["battery-full", "battery-good", "battery"][..], "On battery"), (&["ac-adapter", "battery-full-charging", "battery-full-charged"][..], "Plugged in")].iter().enumerate() {
                let h = hbox(4);
                h.set_halign(gtk::Align::Center);
                h.pack_start(&img(icon, 24), false, false, 0);
                h.pack_start(&lbl(t), false, false, 0);
                g.attach(&h, 2 + i as i32, 0, 1, 1);
            }
        }
        let disp = [timer_combo(p.display_dc), timer_combo(p.display_ac)];
        let sleep = [timer_combo(p.sleep_dc), timer_combo(p.sleep_ac)];
        let has_bl = aerode::sys::backlight().is_some();
        let bright = (battery && has_bl).then(|| [p.bright_dc, p.bright_ac].map(|v| {
            let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
            s.set_draw_value(false);
            s.set_value(v.max(1) as f64);
            s.set_size_request(140, -1);
            s
        }));
        let row = |r: i32, icon: &[&str], t: &str, ws: [&gtk::Widget; 2]| {
            g.attach(&img(icon, 24), 0, r, 1, 1);
            g.attach(&lbl(t), 1, r, 1, 1);
            // a desktop has no battery column: only the plugged in value
            if battery { g.attach(ws[0], 2, r, 1, 1); g.attach(ws[1], 3, r, 1, 1); } else { g.attach(ws[1], 2, r, 1, 1); }
        };
        row(1, &["video-display", "preferences-desktop-display"], "Turn off the display:", [disp[0].upcast_ref(), disp[1].upcast_ref()]);
        row(2, &["system-suspend", "weather-clear-night"], "Put the computer to sleep:", [sleep[0].upcast_ref(), sleep[1].upcast_ref()]);
        if let Some(b) = &bright { row(3, &["display-brightness", "display-brightness-symbolic"], "Adjust plan brightness:", [b[0].upcast_ref(), b[1].upcast_ref()]); }
        PlanEditor { grid: g, disp, sleep, bright }
    }

    fn value(&self, old: &PowerPlan) -> PowerPlan {
        let battery = pw::has_battery();
        let mut p = *old;
        p.display_ac = combo_val(&self.disp[1]);
        p.sleep_ac = combo_val(&self.sleep[1]);
        if battery { p.display_dc = combo_val(&self.disp[0]); p.sleep_dc = combo_val(&self.sleep[0]); }
        if let Some(b) = &self.bright { p.bright_dc = b[0].value() as u32; p.bright_ac = b[1].value() as u32; }
        p
    }

    fn set(&self, p: &PowerPlan) {
        for (c, v) in [(&self.disp[0], p.display_dc), (&self.disp[1], p.display_ac), (&self.sleep[0], p.sleep_dc), (&self.sleep[1], p.sleep_ac)] { c.set_active_id(Some(&v.to_string())); }
        if let Some(b) = &self.bright { b[0].set_value(p.bright_dc as f64); b[1].set_value(p.bright_ac as f64); }
    }
}

fn page_plan(cp: &Rc<Cp>, id: &str) -> gtk::Widget {
    css();
    let o = config::get().hw.power;
    let id = id.to_string();
    let v = vbox(8);
    v.pack_start(&title(&format!("Change settings for the plan: {}", pw::plan_name(&o, &id))), false, false, 0);
    v.pack_start(&text("Choose the sleep and display settings that you want your computer to use."), false, false, 0);
    let cur = pw::settings(&o, &id);
    let ed = PlanEditor::new(&cur);
    ed.grid.set_margin_top(10);
    v.pack_start(&ed.grid, false, false, 0);
    let links = vbox(4);
    links.set_margin_top(16);
    links.set_margin_start(10);
    { let (cp2, id2) = (cp.clone(), id.clone()); links.pack_start(&link("Change advanced power settings", move || advanced(&cpwin(&cp2), &id2)), false, false, 0); }
    let ed = Rc::new(ed);
    { let (ed, cp2, id2) = (ed.clone(), cp.clone(), id.clone()); links.pack_start(&link("Restore default settings for this plan", move || {
        if confirm(Some(&cpwin(&cp2)), "Power Options", "Are you sure you want to restore this plan's default settings?", "", "Yes") {
            let o = config::get().hw.power;
            let d = pw::defaults(&pw::base_of(&o, &id2));
            config::update(|c| { c.hw.power.plans.remove(&id2); });
            ed.set(&d);
            if config::get().hw.power.plan == id2 { pw::apply_now(); }
        }
    }), false, false, 0); }
    if o.custom.iter().any(|c| c.id == id) && o.plan != id {
        let (cp2, id2) = (cp.clone(), id.clone());
        links.pack_start(&link("Delete this plan", move || {
            if confirm(Some(&cpwin(&cp2)), "Power Options", "Are you sure you want to delete this plan?", "You cannot restore a plan after you delete it.", "OK") {
                config::update(|c| { c.hw.power.custom.retain(|x| x.id != id2); c.hw.power.plans.remove(&id2); });
                cp2.open("power");
            }
        }), false, false, 0);
    }
    v.pack_start(&links, false, false, 0);
    let bb = hbox(6);
    bb.set_halign(gtk::Align::End);
    bb.set_margin_top(20);
    let save = gtk::Button::with_mnemonic("_Save changes");
    let cancel = gtk::Button::with_label("Cancel");
    bb.pack_start(&save, false, false, 0);
    bb.pack_start(&cancel, false, false, 0);
    v.pack_start(&bb, false, false, 0);
    { let (ed, cp2, id2) = (ed.clone(), cp.clone(), id.clone()); save.connect_clicked(move |_| {
        let o = config::get().hw.power;
        let p = ed.value(&pw::settings(&o, &id2));
        config::update(|c| { c.hw.power.plans.insert(id2.clone(), p); });
        if config::get().hw.power.plan == id2 { pw::apply_now(); }
        cp2.open("power");
    }); }
    { let cp2 = cp.clone(); cancel.connect_clicked(move |_| cp2.open("power")); }
    v.upcast()
}

/// Win7 "Power Options > Advanced settings": the plan's settings as a tree.
fn advanced(parent: &gtk::Window, plan: &str) {
    let o = config::get().hw.power;
    let s = DlgSheet::new(Some(parent), "Power Options", true);
    let t = vbox(8);
    let top = hbox(10);
    top.pack_start(&img(&["preferences-system-power", "battery-full"], 32), false, false, 0);
    top.pack_start(&wrap("Select the power plan that you want to customize, and then choose settings that reflect how you want your computer to manage power.", 46), false, false, 0);
    t.pack_start(&top, false, false, 0);
    let plans = gtk::ComboBoxText::new();
    let mut ids: Vec<(String, String)> = PLANS.iter().map(|p| (p.0.to_string(), p.1.to_string())).collect();
    for c in &o.custom { ids.push((c.id.clone(), c.name.clone())); }
    for (id, name) in &ids { plans.append(Some(id), &if *id == o.plan { format!("{name} [Active]") } else { name.clone() }); }
    plans.set_active_id(Some(plan));
    t.pack_start(&plans, false, false, 0);
    let battery = pw::has_battery();
    // columns: label, minutes (-1 = group/info row), which setting ("display_dc" …), editable
    let store = gtk::TreeStore::new(&[glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL]);
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    let col = gtk::TreeViewColumn::new();
    let r0 = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &r0, false);
    TreeViewColumnExt::add_attribute(&col, &r0, "text", 0);
    let cm = gtk::ListStore::new(&[glib::Type::STRING]);
    for (_, l) in TIMEOUTS { cm.insert_with_values(None, &[(0, l)]); }
    let r1 = gtk::CellRendererCombo::new();
    r1.set_property("model", &cm);
    r1.set_property("text-column", 0i32);
    r1.set_property("has-entry", false);
    TreeViewColumnExt::pack_start(&col, &r1, true);
    TreeViewColumnExt::add_attribute(&col, &r1, "text", 1);
    TreeViewColumnExt::add_attribute(&col, &r1, "editable", 3);
    tv.append_column(&col);
    let working: Rc<RefCell<std::collections::HashMap<String, PowerPlan>>> = Rc::new(RefCell::new(std::collections::HashMap::new()));
    let fill = { let (store, tv, working, o) = (store.clone(), tv.clone(), working.clone(), o.clone()); Rc::new(move |plan: &str| {
        store.clear();
        let p = working.borrow().get(plan).copied().unwrap_or_else(|| pw::settings(&o, plan));
        let group = |name: &str| store.insert_with_values(None, None, &[(0, &name), (1, &""), (2, &""), (3, &false)]);
        let item = |parent: &gtk::TreeIter, name: &str| store.insert_with_values(Some(parent), None, &[(0, &name), (1, &""), (2, &""), (3, &false)]);
        let val = |parent: &gtk::TreeIter, key: &str, label: &str, m: u32| {
            store.insert_with_values(Some(parent), None, &[(0, &format!("{label}:")), (1, &minutes_label(m)), (2, &key), (3, &true)]);
        };
        let pair = |parent: &gtk::TreeIter, k: &str, dc: u32, ac: u32| {
            if battery { val(parent, &format!("{k}_dc"), "On battery", dc); val(parent, &format!("{k}_ac"), "Plugged in", ac); } else { val(parent, &format!("{k}_ac"), "Setting", ac); }
        };
        let g = group("Sleep");
        let i = item(&g, "Sleep after");
        pair(&i, "sleep", p.sleep_dc, p.sleep_ac);
        let g = group("Display");
        let i = item(&g, "Turn off display after");
        pair(&i, "display", p.display_dc, p.display_ac);
        let g = group("Power buttons and lid");
        let b = logind_buttons();
        let names = |v: &str| BUTTON_ACTIONS.iter().find(|a| a.0 == v).map(|a| a.1).unwrap_or(v).to_string();
        if has_lid() {
            let i = item(&g, "Lid close action");
            store.insert_with_values(Some(&i), None, &[(0, &"On battery:"), (1, &names(&b.lid)), (2, &""), (3, &false)]);
            store.insert_with_values(Some(&i), None, &[(0, &"Plugged in:"), (1, &names(&b.lid_ac)), (2, &""), (3, &false)]);
        }
        let i = item(&g, "Power button action");
        store.insert_with_values(Some(&i), None, &[(0, &"Setting:"), (1, &names(&b.power)), (2, &""), (3, &false)]);
        let i = item(&g, "Sleep button action");
        store.insert_with_values(Some(&i), None, &[(0, &"Setting:"), (1, &names(&b.sleep)), (2, &""), (3, &false)]);
        if battery {
            let g = group("Battery");
            let i = item(&g, "Critical battery action");
            let act = std::fs::read_to_string("/etc/UPower/UPower.conf").ok().and_then(|s| s.lines().find_map(|l| l.trim().strip_prefix("CriticalPowerAction=").map(|v| v.trim().to_string()))).unwrap_or_else(|| "HybridSleep".into());
            store.insert_with_values(Some(&i), None, &[(0, &"Setting:"), (1, &act), (2, &""), (3, &false)]);
        }
        tv.expand_all();
    }) };
    fill(plan);
    {
        let (store, working, plans, o, d) = (store.clone(), working.clone(), plans.clone(), o.clone(), s.dirty());
        r1.connect_edited(move |_, path, text| {
            let Some(it) = store.iter(&path) else { return };
            let key: String = store.value(&it, 2).get().unwrap_or_default();
            let Some(m) = TIMEOUTS.iter().find(|t| t.1 == text).map(|t| t.0) else { return };
            let plan = plans.active_id().map(|s| s.to_string()).unwrap_or_default();
            let mut w = working.borrow_mut();
            let p = w.entry(plan.clone()).or_insert_with(|| pw::settings(&o, &plan));
            match key.as_str() { "sleep_dc" => p.sleep_dc = m, "sleep_ac" => p.sleep_ac = m, "display_dc" => p.display_dc = m, "display_ac" => p.display_ac = m, _ => return }
            store.set(&it, &[(1, &text.to_string())]);
            d();
        });
    }
    { let f = fill.clone(); plans.connect_changed(move |c| if let Some(id) = c.active_id() { f(&id); }); }
    let sw = super::super::admin::scrolled(&tv);
    sw.set_size_request(380, 230);
    t.pack_start(&sw, true, true, 0);
    let rd = gtk::Button::with_mnemonic("Restore plan _defaults");
    rd.set_halign(gtk::Align::End);
    { let (working, plans, f, o, d) = (working.clone(), plans.clone(), fill.clone(), o.clone(), s.dirty()); rd.connect_clicked(move |_| {
        let plan = plans.active_id().map(|s| s.to_string()).unwrap_or_default();
        working.borrow_mut().insert(plan.clone(), pw::defaults(&pw::base_of(&o, &plan)));
        f(&plan);
        d();
    }); }
    t.pack_start(&rd, false, false, 0);
    s.tab("Advanced settings", &t);
    s.on_apply(move || {
        let w = working.borrow().clone();
        config::update(|c| for (k, v) in &w { c.hw.power.plans.insert(k.clone(), *v); });
        pw::apply_now();
        true
    });
    s.show(0);
}

// ------------------------------------------------------------------ System Settings

const BUTTON_ACTIONS: &[(&str, &str)] = &[("ignore", "Do nothing"), ("suspend", "Sleep"), ("hibernate", "Hibernate"), ("poweroff", "Shut down"), ("lock", "Lock"), ("hybrid-sleep", "Hybrid sleep"), ("suspend-then-hibernate", "Sleep, then hibernate")];

#[derive(Clone, Debug, PartialEq)]
struct Buttons { power: String, sleep: String, lid: String, lid_ac: String }

/// The effective logind settings (defaults, logind.conf, then the drop-ins in order).
fn logind_buttons() -> Buttons {
    let mut b = Buttons { power: "poweroff".into(), sleep: "suspend".into(), lid: "suspend".into(), lid_ac: String::new() };
    let mut files = vec![std::path::PathBuf::from("/usr/lib/systemd/logind.conf"), std::path::PathBuf::from("/etc/systemd/logind.conf")];
    let mut drop: Vec<std::path::PathBuf> = ["/usr/lib/systemd/logind.conf.d", "/run/systemd/logind.conf.d", "/etc/systemd/logind.conf.d"].iter()
        .flat_map(|d| std::fs::read_dir(d).map(|r| r.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default()).collect();
    // drop-ins apply in file name order across directories
    drop.sort_by_key(|p| p.file_name().map(|n| n.to_owned()));
    files.extend(drop);
    for f in files {
        let Ok(s) = std::fs::read_to_string(&f) else { continue };
        for l in s.lines() {
            let l = l.trim();
            if l.starts_with('#') { continue; }
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.trim().to_string();
            match k.trim() { "HandlePowerKey" => b.power = v, "HandleSuspendKey" => b.sleep = v, "HandleLidSwitch" => b.lid = v, "HandleLidSwitchExternalPower" => b.lid_ac = v, _ => {} }
        }
    }
    if b.lid_ac.is_empty() { b.lid_ac = b.lid.clone(); }
    b
}

fn action_combo(cur: &str, choices: &[&str]) -> gtk::ComboBoxText {
    let c = gtk::ComboBoxText::new();
    for (id, l) in BUTTON_ACTIONS.iter().filter(|a| choices.contains(&a.0) || a.0 == cur) { c.append(Some(id), l); }
    c.set_active_id(Some(cur));
    c
}

fn page_buttons(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    let v = vbox(8);
    v.pack_start(&title("Define power buttons and turn on password protection"), false, false, 0);
    v.pack_start(&text("Choose the power settings that you want for your computer. The changes you make to the settings on this page apply to all of your power plans."), false, false, 0);
    v.pack_start(&heading("Power and sleep buttons and lid settings"), false, false, 0);
    let b = logind_buttons();
    let battery = pw::has_battery();
    let lid = has_lid();
    let g = gtk::Grid::new();
    g.set_row_spacing(10);
    g.set_column_spacing(16);
    g.set_margin_start(10);
    if battery {
        for (i, (icon, t)) in [(&["battery-full", "battery-good"][..], "On battery"), (&["ac-adapter", "battery-full-charging"][..], "Plugged in")].iter().enumerate() {
            let h = hbox(4);
            h.set_halign(gtk::Align::Center);
            h.pack_start(&img(icon, 24), false, false, 0);
            h.pack_start(&lbl(t), false, false, 0);
            g.attach(&h, 2 + i as i32, 0, 1, 1);
        }
    }
    let full = ["ignore", "suspend", "hibernate", "poweroff"];
    let power = [action_combo(&b.power, &full), action_combo(&b.power, &full)];
    let sleep = [action_combo(&b.sleep, &["ignore", "suspend", "hibernate"]), action_combo(&b.sleep, &["ignore", "suspend", "hibernate"])];
    let lidc = [action_combo(&b.lid, &full), action_combo(&b.lid_ac, &full)];
    // logind has one setting for the power and the sleep button: both columns follow each other
    for pair in [&power, &sleep] {
        for k in 0..2 { let other = pair[1 - k].clone(); pair[k].connect_changed(move |c| { if other.active_id() != c.active_id() { other.set_active_id(c.active_id().as_deref()); } }); }
    }
    let mut r = 1;
    let mut row = |icon: &[&str], t: &str, ws: &[gtk::ComboBoxText; 2]| {
        g.attach(&img(icon, 24), 0, r, 1, 1);
        g.attach(&lbl(t), 1, r, 1, 1);
        if battery { g.attach(&ws[0], 2, r, 1, 1); g.attach(&ws[1], 3, r, 1, 1); } else { g.attach(&ws[1], 2, r, 1, 1); }
        r += 1;
    };
    row(&["system-shutdown"], "When I press the power button:", &power);
    row(&["system-suspend", "weather-clear-night"], "When I press the sleep button:", &sleep);
    if lid { row(&["computer-laptop"], "When I close the lid:", &lidc); }
    v.pack_start(&g, false, false, 0);
    v.pack_start(&heading("Password protection on wakeup"), false, false, 0);
    let pwbox = vbox(4);
    pwbox.set_margin_start(10);
    let req = config::get().hw.power.require_password;
    let rs = radios(&["_Require a password (recommended)", "_Don't require a password"], if req { 0 } else { 1 });
    let radios_box = vbox(4);
    radios_box.pack_start(&rs[0], false, false, 0);
    let d1 = wrap("When your computer wakes from sleep, no one can access your data without entering the correct password to unlock the computer.", 70);
    d1.set_margin_start(26);
    radios_box.pack_start(&d1, false, false, 0);
    { let me = cp.clone(); let l = link("Create or change your user account password", move || me.open("users")); l.set_margin_start(26); radios_box.pack_start(&l, false, false, 0); }
    radios_box.pack_start(&rs[1], false, false, 4);
    let d2 = wrap("When your computer wakes from sleep, anyone can access your data because the computer isn't locked.", 70);
    d2.set_margin_start(26);
    radios_box.pack_start(&d2, false, false, 0);
    radios_box.set_sensitive(false);
    let unlock = { let rb = radios_box.clone(); shield_link("Change settings that are currently unavailable", move || rb.set_sensitive(true)) };
    pwbox.pack_start(&unlock, false, false, 0);
    pwbox.pack_start(&radios_box, false, false, 4);
    v.pack_start(&pwbox, false, false, 0);
    let bb = hbox(6);
    bb.set_halign(gtk::Align::End);
    bb.set_margin_top(16);
    let save = gtk::Button::with_mnemonic("_Save changes");
    let cancel = gtk::Button::with_label("Cancel");
    bb.pack_start(&save, false, false, 0);
    bb.pack_start(&cancel, false, false, 0);
    v.pack_start(&bb, false, false, 0);
    { let cp2 = cp.clone(); cancel.connect_clicked(move |_| cp2.open("power")); }
    { let cp2 = cp.clone(); save.connect_clicked(move |_| {
        let want_pw = rs[0].is_active();
        config::update(|c| c.hw.power.require_password = want_pw);
        let id = |c: &gtk::ComboBoxText| c.active_id().map(|s| s.to_string()).unwrap_or_default();
        let nb = Buttons { power: id(&power[1]), sleep: id(&sleep[1]), lid: id(&lidc[0]), lid_ac: id(&lidc[1]) };
        if nb != b {
            let conf = format!("# written by aerode (Control Panel > Power Options > System Settings)\n[Login]\nHandlePowerKey={}\nHandleSuspendKey={}\n{}",
                nb.power, nb.sleep, if lid { format!("HandleLidSwitch={}\nHandleLidSwitchExternalPower={}\n", nb.lid, nb.lid_ac) } else { String::new() });
            let script = format!("mkdir -p /etc/systemd/logind.conf.d && printf '%s' {} > /etc/systemd/logind.conf.d/aerode.conf && systemctl kill -s HUP systemd-logind", q(&conf));
            let cp3 = cp2.clone();
            elevate_sh(&script, move |ok, out| { if report("The power button settings could not be saved.", ok, &out) { cp3.open("power"); } });
        } else {
            cp2.open("power");
        }
    }); }
    v.upcast()
}

// ------------------------------------------------------------------ Create a power plan

fn page_create(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    let holder = vbox(0);
    let v = vbox(8);
    v.pack_start(&title("Create a power plan"), false, false, 0);
    v.pack_start(&text("To create your own plan, start by selecting one of the following plans that contains the settings closest to what you want."), false, false, 0);
    let rs = radios(&["Balanced (recommended)", "Power saver", "High performance"], 0);
    let list = vbox(4);
    list.set_margin_start(16);
    list.set_margin_top(8);
    for (r, p) in rs.iter().zip(PLANS.iter()) {
        if let Some(l) = r.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&l.text()))); }
        list.pack_start(r, false, false, 0);
        let d = text(p.2);
        d.set_margin_start(26);
        d.style_context().add_class("hw-dim");
        list.pack_start(&d, false, false, 0);
    }
    v.pack_start(&list, false, false, 0);
    let o = config::get().hw.power;
    let n = (1..).find(|i| !o.custom.iter().any(|c| c.name == format!("My Custom Plan {i}"))).unwrap_or(1);
    let nr = hbox(8);
    nr.set_margin_top(16);
    nr.pack_start(&lbl("Plan name:"), false, false, 0);
    let name = gtk::Entry::new();
    name.set_text(&format!("My Custom Plan {n}"));
    name.set_width_chars(40);
    nr.pack_start(&name, false, false, 0);
    v.pack_start(&nr, false, false, 0);
    let bb = hbox(6);
    bb.set_halign(gtk::Align::End);
    bb.set_margin_top(20);
    let next = gtk::Button::with_mnemonic("_Next");
    let cancel = gtk::Button::with_label("Cancel");
    bb.pack_start(&next, false, false, 0);
    bb.pack_start(&cancel, false, false, 0);
    v.pack_start(&bb, false, false, 0);
    holder.pack_start(&v, true, true, 0);
    { let cp2 = cp.clone(); cancel.connect_clicked(move |_| cp2.open("power")); }
    { let name2 = name.clone(); let nx = next.clone(); name.connect_changed(move |_| nx.set_sensitive(!name2.text().trim().is_empty())); }
    { let (cp2, holder2) = (cp.clone(), holder.clone()); next.connect_clicked(move |_| {
        let base = PLANS[rs.iter().position(|r| r.is_active()).unwrap_or(0)].0.to_string();
        let plan_name = name.text().trim().to_string();
        // step 2: the plan's display and sleep settings, then Create
        for c in holder2.children() { holder2.remove(&c); }
        let v = vbox(8);
        v.pack_start(&title(&format!("Change settings for the plan: {plan_name}")), false, false, 0);
        v.pack_start(&text("Choose the sleep and display settings that you want your computer to use."), false, false, 0);
        let ed = PlanEditor::new(&pw::defaults(&base));
        ed.grid.set_margin_top(10);
        v.pack_start(&ed.grid, false, false, 0);
        let bb = hbox(6);
        bb.set_halign(gtk::Align::End);
        bb.set_margin_top(20);
        let create = gtk::Button::with_mnemonic("C_reate");
        let cancel = gtk::Button::with_label("Cancel");
        bb.pack_start(&create, false, false, 0);
        bb.pack_start(&cancel, false, false, 0);
        v.pack_start(&bb, false, false, 0);
        holder2.pack_start(&v, true, true, 0);
        holder2.show_all();
        { let cp3 = cp2.clone(); cancel.connect_clicked(move |_| cp3.open("power")); }
        let cp3 = cp2.clone();
        let base2 = base.clone();
        create.connect_clicked(move |_| {
            let p = ed.value(&pw::defaults(&base2));
            let o = config::get().hw.power;
            let id = (1..).map(|i| format!("custom-{i}")).find(|id| !o.custom.iter().any(|c| &c.id == id)).unwrap();
            let (pn, b) = (plan_name.clone(), base2.clone());
            config::update(|c| {
                c.hw.power.custom.push(CustomPlan { id: id.clone(), name: pn, base: b });
                c.hw.power.plans.insert(id.clone(), p);
            });
            // Win7 makes the new plan the active one
            pw::set_plan(&id);
            cp3.open("power");
        });
    }); }
    holder.upcast()
}
