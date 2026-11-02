//! Date and Time: the Windows 7 dialog. Date and Time (analog clock, Change date and time through
//! timedatectl + pkexec, time zone with its daylight saving note), Additional Clocks (two more
//! clocks for the taskbar clock's tooltip and flyout, kept in config) and Internet Time
//! (systemd-timesyncd: synchronize on/off, server, Update now).
use super::*;
use super::super::admin::{columns, elevate_sh, img, q, report, scrolled, shield_button, sval};
use super::super::desktop::{check, group, Sheet};
use aerode::tzinfo::{self, Zone};
use std::cell::Cell;

fn vbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, s) }
fn hbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, s) }

fn pixbuf(n: &str) -> Option<gtk::gdk_pixbuf::Pixbuf> { config::data_file(&format!("clock/{n}.png")).and_then(|p| gtk::gdk_pixbuf::Pixbuf::from_file(p).ok()) }

/// An analog clock that ticks; `offset` (seconds) is added to the local time.
fn analog(size: i32, offset: Rc<Cell<i64>>) -> gtk::DrawingArea {
    let face = gtk::DrawingArea::new();
    face.set_size_request(size, size);
    let (dial, glass) = (pixbuf("5000"), pixbuf("5001"));
    face.connect_draw(move |_, cr| {
        let t = chrono::Local::now().naive_local() + chrono::Duration::seconds(offset.get());
        aerode::flyouts::draw_clock_at(cr, size as f64, dial.as_ref(), glass.as_ref(), t);
        glib::Propagation::Stop
    });
    let f = face.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        if f.parent().is_none() && !f.is_visible() { return glib::ControlFlow::Break; }
        f.queue_draw();
        glib::ControlFlow::Continue
    });
    face
}

/// Every zone of the tz database with its Windows-style label, sorted like Windows (by offset).
fn zones() -> Vec<(String, String)> {
    let list = super::super::admin::sh(&["timedatectl", "list-timezones", "--no-pager"]);
    let mut names: Vec<String> = list.lines().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if names.is_empty() {
        // no timedatectl: zone1970.tab
        names = std::fs::read_to_string(tzinfo::zoneinfo_dir().join("zone1970.tab")).unwrap_or_default().lines()
            .filter(|l| !l.starts_with('#')).filter_map(|l| l.split('\t').nth(2).map(String::from)).collect();
        names.push("UTC".into());
    }
    let now = tzinfo::now();
    let mut v: Vec<(i32, String, String)> = names.into_iter().filter_map(|n| {
        let z = Zone::load(&n)?;
        let off = z.std_offset(now);
        Some((off, format!("{} {}", tzinfo::utc_label(off), n.replace('_', " ")), n))
    }).collect();
    v.sort();
    v.into_iter().map(|(_, l, n)| (n, l)).collect()
}

fn zone_label(name: &str) -> String {
    let off = Zone::load(name).map(|z| z.std_offset(tzinfo::now())).unwrap_or(0);
    format!("{} {}", tzinfo::utc_label(off), name.replace('_', " "))
}

/// Win7's note under the time zone.
fn dst_note(zone: &str) -> String {
    let Some(z) = Zone::load(zone) else { return String::new() };
    let Some((t, from, to)) = z.next_change(tzinfo::now()) else { return "This time zone does not observe Daylight Saving Time.".into() };
    let when = chrono::DateTime::from_timestamp(t + from as i64, 0).map(|d| d.naive_utc()).unwrap_or_default();
    let delta = (to - from).abs();
    let amount = if delta % 3600 == 0 { format!("{} hour{}", delta / 3600, if delta == 3600 { "" } else { "s" }) } else { format!("{} minutes", delta / 60) };
    if to < from {
        format!("Daylight Saving Time ends on {} at {}. The clock is set to go back {amount} at that time.", when.format("%A, %B %-d, %Y"), when.format("%-I:%M %p"))
    } else {
        format!("Daylight Saving Time begins on {} at {}. The clock is set to go forward {amount} at that time.", when.format("%A, %B %-d, %Y"), when.format("%-I:%M %p"))
    }
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget { page_tab(cp, 0) }

pub fn page_tab(cp: &Rc<Cp>, tab: u32) -> gtk::Widget {
    css();
    let sheet = Sheet::new(cp);
    let c = config::get().hw;
    let win = cpwin(cp);

    // ---------------------------------------------------------------- Date and Time
    let t = vbox(8);
    let top = hbox(16);
    top.pack_start(&analog(130, Rc::new(Cell::new(0))), false, false, 0);
    let col = vbox(4);
    col.set_valign(gtk::Align::Center);
    let date_l = lbl("");
    let time_l = lbl("");
    for (k, w) in [("Date:", &date_l), ("Time:", &time_l)] { col.pack_start(&lbl(k), false, false, 0); w.set_margin_start(16); col.pack_start(w, false, false, 0); }
    let tick = { let (d, t) = (date_l.clone(), time_l.clone()); move || { let n = chrono::Local::now(); d.set_text(&n.format("%A, %B %-d, %Y").to_string()); t.set_text(&n.format("%-I:%M:%S %p").to_string()); } };
    tick();
    { let d = date_l.clone(); glib::timeout_add_local(std::time::Duration::from_millis(500), move || { if d.parent().is_none() { return glib::ControlFlow::Break; } tick(); glib::ControlFlow::Continue }); }
    let change = shield_button("_Change date and time...");
    change.set_margin_top(8);
    change.set_halign(gtk::Align::Start);
    col.pack_start(&change, false, false, 0);
    top.pack_start(&col, true, true, 0);
    t.pack_start(&top, false, false, 0);
    let tzbox = vbox(6);
    let zone = tzinfo::local_zone();
    let zl = lbl(&zone_label(&zone));
    tzbox.pack_start(&zl, false, false, 0);
    let chz = gtk::Button::with_mnemonic("Change _time zone...");
    chz.set_halign(gtk::Align::End);
    tzbox.pack_start(&chz, false, false, 0);
    let note = wrap(&dst_note(&zone), 54);
    tzbox.pack_start(&note, false, false, 0);
    let notify = check("_Notify me when the clock changes", c.dst_notify);
    sheet.watch_check(&notify);
    notify.set_sensitive(Zone::load(&zone).map(|z| z.observes_dst(tzinfo::now())).unwrap_or(false));
    tzbox.pack_start(&notify, false, false, 0);
    t.pack_start(&group("Time zone", &tzbox), false, false, 4);
    t.pack_start(&link("Get more time zone information online", || { util::open_uri("https://www.timeanddate.com/time/zones/"); }), false, false, 0);
    t.pack_start(&link("How do I set the clock and time zone?", || { util::open_uri("https://wiki.archlinux.org/title/System_time"); }), false, false, 0);
    { let w = win.clone(); change.connect_clicked(move |_| set_date_time(&w)); }
    { let (w, zl, note, notify) = (win.clone(), zl.clone(), note.clone(), notify.clone()); chz.connect_clicked(move |_| {
        let cur = tzinfo::local_zone();
        if let Some(z) = pick_zone(&w, "Time Zone Settings", &cur, true) {
            if z == cur { return; }
            let (zl, note, notify, z2) = (zl.clone(), note.clone(), notify.clone(), z.clone());
            elevate_sh(&format!("timedatectl set-timezone {}", q(&z)), move |ok, out| {
                if report("The time zone could not be changed.", ok, &out) {
                    zl.set_text(&zone_label(&z2));
                    note.set_text(&dst_note(&z2));
                    notify.set_sensitive(Zone::load(&z2).map(|z| z.observes_dst(tzinfo::now())).unwrap_or(false));
                }
            });
        }
    }); }
    sheet.tab("Date and Time", &t);

    // ---------------------------------------------------------------- Additional Clocks
    let t = vbox(8);
    t.pack_start(&wrap("Additional clocks can display the time in other time zones. You can view them by clicking on or hovering over the taskbar clock.", 54), false, false, 0);
    let all = Rc::new(RefCell::new(None::<Vec<(String, String)>>));
    let mut rows = vec![];
    for i in 0..2 {
        let cur = c.clocks.get(i).cloned().unwrap_or_default();
        let b = vbox(4);
        let show = check("_Show this clock", cur.show);
        b.pack_start(&show, false, false, 0);
        let inner = vbox(4);
        inner.set_margin_start(20);
        inner.pack_start(&lbl("Select time _zone:"), false, false, 0);
        let zrow = hbox(6);
        let zent = gtk::Entry::new();
        zent.set_editable(false);
        let ztz = Rc::new(RefCell::new(if cur.tz.is_empty() { tzinfo::local_zone() } else { cur.tz.clone() }));
        zent.set_text(&zone_label(&ztz.borrow()));
        zent.set_hexpand(true);
        zrow.pack_start(&zent, true, true, 0);
        let pick = gtk::Button::from_icon_name(Some("pan-down-symbolic"), gtk::IconSize::Menu);
        zrow.pack_start(&pick, false, false, 0);
        inner.pack_start(&zrow, false, false, 0);
        inner.pack_start(&lbl("_Enter display name:"), false, false, 0);
        let name = gtk::Entry::new();
        name.set_text(if cur.name.is_empty() { if i == 0 { "Clock 1" } else { "Clock 2" } } else { &cur.name });
        name.set_max_length(15);
        inner.pack_start(&name, false, false, 0);
        inner.set_sensitive(cur.show);
        { let inner = inner.clone(); show.connect_toggled(move |c| inner.set_sensitive(c.is_active())); }
        b.pack_start(&inner, false, false, 0);
        sheet.watch_check(&show);
        { let d = sheet.dirty(); name.connect_changed(move |_| d()); }
        let open = { let (w, ztz, zent, d, _all) = (win.clone(), ztz.clone(), zent.clone(), sheet.dirty(), all.clone()); Rc::new(move || {
            let cur = ztz.borrow().clone();
            if let Some(z) = pick_zone(&w, "Select time zone", &cur, false) { if z != cur { zent.set_text(&zone_label(&z)); *ztz.borrow_mut() = z; d(); } }
        }) };
        { let o = open.clone(); pick.connect_clicked(move |_| o()); }
        { let o = open.clone(); zent.connect_button_press_event(move |_, _| { o(); glib::Propagation::Stop }); }
        t.pack_start(&b, false, false, 6);
        if i == 0 { t.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 0); }
        rows.push((show, ztz, name));
    }
    sheet.tab("Additional Clocks", &t);

    // ---------------------------------------------------------------- Internet Time
    let t = vbox(10);
    let status = wrap("", 54);
    let next = lbl("");
    let last = wrap("", 54);
    t.pack_start(&status, false, false, 0);
    t.pack_start(&next, false, false, 0);
    t.pack_start(&last, false, false, 0);
    let change_ntp = shield_button("Change _settings...");
    change_ntp.set_halign(gtk::Align::End);
    t.pack_start(&change_ntp, false, false, 8);
    let refresh = { let (status, next, last) = (status.clone(), next.clone(), last.clone()); Rc::new(move || {
        let (status, next, last) = (status.clone(), next.clone(), last.clone());
        super::super::admin::bg(ntp_state, move |s| {
            if s.ntp {
                status.set_text(&format!("This computer is set to automatically synchronize with '{}'.", if s.server.is_empty() { "the time servers of your distribution" } else { &s.server }));
                next.set_text(&if s.poll_secs > 0 { format!("Next synchronization: {}", super::super::admin::when_words(tzinfo::now() + s.poll_secs)) } else { String::new() });
                last.set_text(&if s.synced { format!("The clock was successfully synchronized with {} {}.", if s.server.is_empty() { "the time server" } else { &s.server }, if s.when > 0 { format!("on {}", super::super::admin::when(s.when)) } else { "recently".into() }) } else { "The clock has not been synchronized yet.".into() });
            } else {
                status.set_text("This computer is not set to automatically synchronize.");
                next.set_text("");
                last.set_text("");
            }
        });
    }) };
    refresh();
    { let (w, r) = (win.clone(), refresh.clone()); change_ntp.connect_clicked(move |_| internet_time(&w, r.clone())); }
    sheet.tab("Internet Time", &t);

    sheet.on_apply(move || {
        let clocks: Vec<aerode::config::ExtraClock> = rows.iter().map(|(s, z, n)| aerode::config::ExtraClock { show: s.is_active(), tz: z.borrow().clone(), name: n.text().to_string() }).collect();
        let dn = notify.is_active();
        config::update(|c| { c.hw.clocks = clocks; c.hw.dst_notify = dn; });
        // the taskbar clock re-reads them
        util::spawn(&["aerode", "reload"]);
        true
    });
    let w = sheet.widget();
    { let nb = sheet.nb.clone(); glib::idle_add_local_once(move || nb.set_current_page(Some(tab))); }
    w
}

/// Win7 "Date and Time Settings": calendar, clock and time; sets both through timedatectl.
fn set_date_time(parent: &gtk::Window) {
    let d = ok_cancel(Some(parent), "Date and Time Settings");
    let c = d.content_area();
    c.pack_start(&lbl("Set the date and time:"), false, false, 0);
    let row = hbox(20);
    let left = vbox(4);
    left.pack_start(&lbl("_Date:"), false, false, 0);
    let cal = gtk::Calendar::new();
    cal.style_context().add_class("w7cal");
    left.pack_start(&cal, false, false, 0);
    row.pack_start(&left, false, false, 0);
    let right = vbox(6);
    right.pack_start(&lbl("_Time:"), false, false, 0);
    let offset = Rc::new(Cell::new(0i64));
    right.pack_start(&analog(130, offset.clone()), false, false, 0);
    let time = gtk::SpinButton::with_range(0.0, 86399.0, 1.0);
    time.set_numeric(false);
    time.set_wrap(true);
    time.set_width_chars(11);
    let fmt = |secs: f64| { let s = secs as i64; let t = chrono::NaiveTime::from_num_seconds_from_midnight_opt((s.rem_euclid(86400)) as u32, 0).unwrap_or_default(); t.format("%-I:%M:%S %p").to_string() };
    let now_secs = || { use chrono::Timelike; chrono::Local::now().num_seconds_from_midnight() as f64 };
    let editing = Rc::new(Cell::new(false));
    time.connect_output(move |s| { s.set_text(&fmt(s.value())); glib::Propagation::Stop });
    time.connect_input(|s| {
        let t = s.text().trim().to_uppercase();
        let parsed = ["%I:%M:%S %p", "%I:%M %p", "%H:%M:%S", "%H:%M"].iter().find_map(|f| chrono::NaiveTime::parse_from_str(&t, f).ok());
        parsed.map(|p| { use chrono::Timelike; Ok(p.num_seconds_from_midnight() as f64) })
    });
    time.set_value(now_secs());
    { let (o, e) = (offset.clone(), editing.clone()); time.connect_value_changed(move |s| { if e.get() { o.set(s.value() as i64 - now_secs() as i64); } }); }
    { let e = editing.clone(); time.connect_key_press_event(move |_, _| { e.set(true); glib::Propagation::Proceed }); }
    { let e = editing.clone(); time.connect_button_press_event(move |_, _| { e.set(true); glib::Propagation::Proceed }); }
    // until edited, the time keeps running
    { let (t, e) = (time.clone(), editing.clone()); glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        if t.parent().is_none() { return glib::ControlFlow::Break; }
        if !e.get() { t.set_value(now_secs()); }
        glib::ControlFlow::Continue
    }); }
    right.pack_start(&time, false, false, 0);
    row.pack_start(&right, false, false, 0);
    c.pack_start(&row, false, false, 0);
    { let p = parent.clone(); let d2 = d.clone(); c.pack_start(&link("Change calendar settings", move || { d2.close(); let _ = &p; super::super::admin::spawn_tool("region"); }), false, false, 0); }
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        let (y, m, day) = cal.date();
        let secs = time.value() as i64 + if editing.get() { 0 } else { (now_secs() - time.value()) as i64 };
        let when = format!("{y:04}-{:02}-{day:02} {:02}:{:02}:{:02}", m + 1, secs / 3600 % 24, secs / 60 % 60, secs % 60);
        // with network time on, timedatectl refuses: turn it off first (like Windows stops syncing)
        let script = format!("if [ \"$(timedatectl show -p NTP --value)\" = yes ]; then timedatectl set-ntp false; fi; timedatectl set-time {}", q(&when));
        elevate_sh(&script, |ok, out| { report("Unable to set the time.", ok, &out); });
    }
    d.close();
}

/// A searchable list of every time zone; returns the chosen IANA name.
fn pick_zone(parent: &gtk::Window, title: &str, current: &str, dst_check: bool) -> Option<String> {
    let d = ok_cancel(Some(parent), title);
    d.set_default_size(440, 460);
    d.set_resizable(true);
    let c = d.content_area();
    c.pack_start(&lbl(if dst_check { "Set the time zone:" } else { "Select time zone:" }), false, false, 0);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Type a city or region"));
    c.pack_start(&search, false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::STRING, glib::Type::STRING]);
    let filter = gtk::TreeModelFilter::new(&store, None);
    { let s = search.clone(); filter.set_visible_func(move |m, it| {
        let q = s.text().to_lowercase();
        q.is_empty() || q.split_whitespace().all(|w| sval(m, it, 0).to_lowercase().contains(w))
    }); }
    { let f = filter.clone(); search.connect_search_changed(move |_| f.refilter()); }
    let tv = gtk::TreeView::with_model(&filter);
    columns(&tv, &[("Time zone", 0, 0)]);
    tv.set_headers_visible(false);
    let sw = scrolled(&tv);
    sw.set_vexpand(true);
    c.pack_start(&sw, true, true, 0);
    let auto = check("_Automatically adjust clock for Daylight Saving Time", true);
    auto.set_sensitive(false);
    if dst_check { c.pack_start(&auto, false, false, 0); }
    let now_l = lbl(&format!("Current date and time: {}", chrono::Local::now().format("%A, %B %-d, %Y, %-I:%M %p")));
    if dst_check { c.pack_start(&now_l, false, false, 0); }
    { let d = d.clone(); tv.connect_row_activated(move |_, _, _| d.response(gtk::ResponseType::Ok)); }
    { let auto = auto.clone(); tv.selection().connect_changed(move |s| {
        if let Some((m, it)) = s.selected() { auto.set_active(Zone::load(&sval(&m, &it, 1)).map(|z| z.observes_dst(tzinfo::now())).unwrap_or(false)); }
    }); }
    let cur = current.to_string();
    { let (store, tv, filter) = (store.clone(), tv.clone(), filter.clone()); super::super::admin::bg(zones, move |list| {
        for (name, label) in &list {
            let it = store.insert_with_values(None, &[(0, label), (1, name)]);
            if *name == cur {
                if let Some(fit) = filter.convert_child_iter_to_iter(&it) {
                    tv.selection().select_iter(&fit);
                    if let Some(p) = filter.path(&fit) { tv.scroll_to_cell(Some(&p), None::<&gtk::TreeViewColumn>, true, 0.4, 0.0); }
                }
            }
        }
    }); }
    d.show_all();
    let r = d.run();
    let pick = tv.selection().selected().map(|(m, it)| sval(&m, &it, 1));
    d.close();
    if r == gtk::ResponseType::Ok { pick } else { None }
}

#[derive(Default)]
struct Ntp { ntp: bool, synced: bool, server: String, when: i64, poll_secs: i64 }

/// systemd-timesyncd's state (worker thread).
fn ntp_state() -> Ntp {
    let sh = |a: &[&str]| super::super::admin::sh(a);
    let show = sh(&["timedatectl", "show"]);
    let get = |s: &str, k: &str| s.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).unwrap_or("").to_string();
    let ts = sh(&["timedatectl", "show-timesync", "--all"]);
    // NTPMessage={ … ReceiveTimestamp=Mon 2026-11-02 10:24:01 CET … }
    let when = get(&ts, "NTPMessage").split(", ").find_map(|p| p.strip_prefix("ReceiveTimestamp=")).and_then(|t| {
        let t = t.trim_end_matches(" }").trim();
        let parts: Vec<&str> = t.split_whitespace().collect();
        chrono::NaiveDateTime::parse_from_str(&format!("{} {}", parts.get(1)?, parts.get(2)?), "%Y-%m-%d %H:%M:%S").ok()
    }).and_then(|n| n.and_local_timezone(chrono::Local).single()).map(|d| d.timestamp()).unwrap_or(0);
    let poll = get(&ts, "PollIntervalUSec");
    let poll_secs = parse_span(&poll);
    Ntp { ntp: get(&show, "NTP") == "yes", synced: get(&show, "NTPSynchronized") == "yes", server: get(&ts, "ServerName"), when, poll_secs }
}

/// "34min 8s" / "1h 8min" / "2048000000" (µs) → seconds.
fn parse_span(s: &str) -> i64 {
    if let Ok(us) = s.trim().parse::<i64>() { return us / 1_000_000; }
    s.split_whitespace().map(|p| {
        let n: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        let v: i64 = n.parse().unwrap_or(0);
        match &p[n.len()..] { "h" => v * 3600, "min" => v * 60, "s" => v, "d" => v * 86400, "ms" | "us" => 0, _ => 0 }
    }).sum()
}

/// Win7 "Internet Time Settings".
fn internet_time(parent: &gtk::Window, refresh: Rc<dyn Fn()>) {
    let d = ok_cancel(Some(parent), "Internet Time Settings");
    let c = d.content_area();
    let top = hbox(10);
    top.pack_start(&img(&["preferences-system-time"], 32), false, false, 0);
    top.pack_start(&lbl("Configure Internet time settings:"), false, false, 0);
    c.pack_start(&top, false, false, 0);
    let sync = check("S_ynchronize with an Internet time server", false);
    c.pack_start(&sync, false, false, 0);
    let row = hbox(8);
    row.set_margin_start(20);
    row.pack_start(&lbl("S_erver:"), false, false, 0);
    let server = gtk::ComboBoxText::with_entry();
    for s in ["pool.ntp.org", "time.windows.com", "time.nist.gov", "time.google.com", "time.cloudflare.com"] { server.append_text(s); }
    server.set_hexpand(true);
    row.pack_start(&server, true, true, 0);
    let now_b = gtk::Button::with_mnemonic("_Update now");
    row.pack_start(&now_b, false, false, 0);
    c.pack_start(&row, false, false, 0);
    let msg = wrap("", 50);
    c.pack_start(&msg, false, false, 6);
    let entry = server.child().and_then(|e| e.downcast::<gtk::Entry>().ok());
    let initial = Rc::new(RefCell::new((false, String::new())));
    { let (sync, entry, row, initial) = (sync.clone(), entry.clone(), row.clone(), initial.clone()); super::super::admin::bg(ntp_state, move |s| {
        sync.set_active(s.ntp);
        row.set_sensitive(s.ntp);
        if let Some(e) = &entry { e.set_text(&s.server); }
        *initial.borrow_mut() = (s.ntp, s.server);
    }); }
    { let row = row.clone(); sync.connect_toggled(move |c| row.set_sensitive(c.is_active())); }
    { let (msg, r) = (msg.clone(), refresh.clone()); now_b.connect_clicked(move |_| {
        msg.set_text("Please wait while Windows synchronizes with the time server...");
        let (msg, r) = (msg.clone(), r.clone());
        elevate_sh("systemctl restart systemd-timesyncd.service", move |ok, out| {
            msg.set_text(if ok { "The clock is being synchronized with the time server." } else { "An error occurred while Windows was synchronizing with the time server." });
            if !ok { report("The clock could not be synchronized.", ok, &out); }
            r();
        });
    }); }
    d.show_all();
    if d.run() == gtk::ResponseType::Ok {
        let (was_on, was_server) = initial.borrow().clone();
        let on = sync.is_active();
        let srv = entry.map(|e| e.text().trim().to_string()).unwrap_or_default();
        let mut script = vec![];
        if on && !srv.is_empty() && srv != was_server && srv.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c)) {
            script.push(format!("mkdir -p /etc/systemd/timesyncd.conf.d && printf '[Time]\\nNTP=%s\\n' {} > /etc/systemd/timesyncd.conf.d/aerode.conf && systemctl restart systemd-timesyncd.service", q(&srv)));
        }
        if on != was_on { script.push(format!("timedatectl set-ntp {}", on)); }
        if !script.is_empty() {
            let r = refresh.clone();
            elevate_sh(&script.join(" && "), move |ok, out| { report("The Internet time settings could not be changed.", ok, &out); r(); });
        }
    }
    d.close();
}

#[cfg(test)]
mod tests {
    #[test]
    fn spans() {
        assert_eq!(super::parse_span("34min 8s"), 2048);
        assert_eq!(super::parse_span("1h 8min"), 4080);
        assert_eq!(super::parse_span("2048000000"), 2048);
    }
}
