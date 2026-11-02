//! Administrative Tools: the Win7 folder of shortcuts (Details view: Name, Date modified, Type,
//! Size) – double-click opens the tool – and Windows Memory Diagnostic (memtest86+ from the boot
//! menu).
use gtk::prelude::*;
use gtk::{gdk_pixbuf::Pixbuf, glib};
use std::rc::Rc;

use super::super::super::Cp;
use super::*;

/// (name, icon names, description, what to open)
const ITEMS: &[(&str, &[&str], &str, &str)] = &[
    ("Computer Management", &["aerode-compmgmt", "computer"], "Manages disks and provides access to other tools to manage local and remote computers.", "!compmgmt"),
    ("Disk Management", &["aerode-diskmgmt", "drive-harddisk"], "Manages disks, partitions and volumes.", "!diskmgmt"),
    ("Event Viewer", &["aerode-eventvwr", "utilities-system-monitor"], "View monitoring and troubleshooting messages from Windows and other programs.", "!eventvwr"),
    ("Performance Monitor", &["utilities-system-monitor-perf", "utilities-system-monitor"], "Diagnose performance issues and collect performance data.", "@aerode-taskmgr"),
    ("Print Management", &["printer"], "Manages local printers and printer queues.", "@print"),
    ("Services", &["aerode-services", "applications-system"], "Starts, stops, and configures Windows services.", "services"),
    ("System Configuration", &["aerode-msconfig", "preferences-system"], "Perform advanced troubleshooting and system configuration (startup programs and services).", "startup"),
    ("System Information", &["aerode-msinfo", "dialog-information"], "Displays detailed information about your hardware and software.", "!sysinfo"),
    ("Task Scheduler", &["aerode-taskschd", "x-office-calendar"], "Schedule computer tasks to run automatically.", "!taskschd"),
    ("Windows Firewall with Advanced Security", &["aerode-firewall", "security-medium"], "Configure policies that provide enhanced network security for Windows computers.", "@firewall"),
    ("Windows Memory Diagnostic", &["aerode-mdsched", "media-flash"], "Check your computer for memory problems.", "!mdsched"),
];

fn open(cp: &Rc<Cp>, target: &str) {
    if let Some(id) = target.strip_prefix('!') { spawn_tool(id); return; }
    match target {
        "@aerode-taskmgr" => { util::spawn(&["aerode-taskmgr"]); }
        "@print" => { if !util::spawn(&["system-config-printer"]) { util::open_uri("http://localhost:631/printers/"); } }
        "@firewall" => { for t in [&["firewall-config"][..], &["gufw"], &["plasma-firewall"]] { if util::spawn(t) { return; } } cp.open("firewall"); }
        page => cp.open(page),
    }
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    let v = vbox(0);
    v.style_context().add_class("adm");
    // Explorer command bar
    let bar = hbox(4);
    bar.style_context().add_class("commandbar");
    let organize = gtk::MenuButton::new();
    organize.set_label("Organize ▾");
    organize.set_relief(gtk::ReliefStyle::None);
    let om = gtk::Menu::new();
    for l in ["Cut", "Copy", "Paste", "Undo", "Redo", "Select all", "Layout", "Folder and search options", "Delete", "Rename", "Remove properties", "Properties", "Close"] { let mi = gtk::MenuItem::with_label(l); mi.set_sensitive(l == "Select all" || l == "Close"); om.append(&mi); }
    om.show_all();
    organize.set_popup(Some(&om));
    bar.pack_start(&organize, false, false, 0);
    let openb = gtk::Button::with_label("Open");
    openb.set_relief(gtk::ReliefStyle::None);
    openb.set_no_show_all(true);
    bar.pack_start(&openb, false, false, 0);
    let burn = gtk::Button::with_label("Burn");
    burn.set_relief(gtk::ReliefStyle::None);
    burn.set_sensitive(false);
    bar.pack_start(&burn, false, false, 0);
    v.pack_start(&bar, false, false, 0);
    // shortcuts are "dated" when aerode was installed, like Win7's 7/14/2009 files
    let date = std::env::current_exe().ok().and_then(|p| mtime(&p)).map(|t| { use chrono::TimeZone; chrono::Local.timestamp_opt(t, 0).single().map(|d| d.format("%-m/%-d/%Y %-I:%M %p").to_string()).unwrap_or_default() }).unwrap_or_default();
    let store = gtk::ListStore::new(&[Pixbuf::static_type(), glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING, glib::Type::STRING]);
    for (name, icon, desc, target) in ITEMS { store.insert_with_values(None, &[(0, &pix(icon, 16)), (1, name), (2, &date), (3, &"Shortcut"), (4, &"2 KB"), (5, target), (6, desc), (7, &util::first_icon(icon).to_string())]); }
    let tv = gtk::TreeView::with_model(&store);
    icon_column(&tv, "Name", 0, 1, 300);
    columns(&tv, &[("Date modified", 2, 150), ("Type", 3, 90), ("Size", 4, 0)]);
    tv.set_tooltip_column(6);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.add(&tv);
    sw.set_min_content_height(330);
    v.pack_start(&sw, true, true, 0);
    // details pane at the bottom (Explorer)
    let det = hbox(10);
    det.style_context().add_class("exp-details");
    let dicon = gtk::Image::from_pixbuf(pix(&["aerode-admintools", "applications-system"], 48).as_ref());
    det.pack_start(&dicon, false, false, 0);
    let dl = vbox(2);
    let dname = label(&format!("{} items", ITEMS.len()), "name");
    let ddesc = label("", "");
    dl.pack_start(&dname, false, false, 0);
    dl.pack_start(&ddesc, false, false, 0);
    det.pack_start(&dl, true, true, 0);
    v.pack_start(&det, false, false, 0);
    { let (dicon, dname, ddesc, openb) = (dicon.clone(), dname.clone(), ddesc.clone(), openb.clone()); tv.selection().connect_changed(move |s| match s.selected() {
        Some((m, it)) => { dname.set_text(&sval(&m, &it, 1)); ddesc.set_text(&format!("Shortcut   {}", sval(&m, &it, 6))); dicon.set_from_pixbuf(util::pixbuf(&sval(&m, &it, 7), 48).as_ref()); openb.show(); }
        None => { dname.set_text(&format!("{} items", ITEMS.len())); ddesc.set_text(""); openb.hide(); }
    }); }
    { let cp = cp.clone(); tv.connect_row_activated(move |tv, p, _| { if let Some(m) = tv.model() { if let Some(it) = m.iter(p) { open(&cp, &sval(&m, &it, 5)); } } }); }
    { let (cp, tv) = (cp.clone(), tv.clone()); openb.connect_clicked(move |_| if let Some((m, it)) = tv.selection().selected() { open(&cp, &sval(&m, &it, 5)); }); }
    v.upcast()
}

// ------------------------------------------------------------------------------- memtest

/// The boot entry that starts memtest86+: ("systemd-boot", id) or ("grub", title).
fn memtest_entry() -> Option<(&'static str, String)> {
    let list = sh(&["bootctl", "list", "--no-pager"]);
    for block in list.split("\n\n") {
        if block.to_lowercase().contains("memtest") {
            if let Some(id) = block.lines().find_map(|l| l.trim().strip_prefix("id:")) { return Some(("systemd-boot", id.trim().to_string())); }
        }
    }
    let cfg = std::fs::read_to_string("/boot/grub/grub.cfg").unwrap_or_default();
    for l in cfg.lines() {
        let t = l.trim();
        if t.starts_with("menuentry") && t.to_lowercase().contains("memtest") {
            let title = t.split(['\'', '"']).nth(1).unwrap_or("").to_string();
            if !title.is_empty() { return Some(("grub", title)); }
        }
    }
    None
}

/// Windows Memory Diagnostic.
pub fn mdsched_window(app: &gtk::Application) -> gtk::Window {
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_application(Some(app));
    w.set_title("Windows Memory Diagnostic");
    w.set_icon(pix(&["aerode-mdsched", "media-flash"], 48).as_ref());
    w.set_resizable(false);
    w.set_default_size(470, -1);
    w.style_context().add_class("adm");
    let v = vbox(10);
    v.set_border_width(16);
    let h = gtk::Label::new(None);
    h.set_markup("<span size='large' foreground='#1e3287'>Check your computer for memory problems</span>");
    h.set_xalign(0.0);
    v.pack_start(&h, false, false, 0);
    v.pack_start(&label("Memory problems can cause your computer to lose information or stop working.", ""), false, false, 0);
    let entry = memtest_entry();
    let cmd = |title: &str, sub: &str| {
        let b = gtk::Button::new();
        b.set_relief(gtk::ReliefStyle::None);
        let c = vbox(1);
        let t = gtk::Label::new(None);
        t.set_markup(&format!("<span foreground='#1e3287' size='large'>➜ {}</span>", glib::markup_escape_text(title)));
        t.set_xalign(0.0);
        c.pack_start(&t, false, false, 0);
        if !sub.is_empty() { let s = label(sub, ""); s.set_margin_start(22); c.pack_start(&s, false, false, 0); }
        b.add(&c);
        b
    };
    match entry.clone() {
        Some((kind, id)) => {
            let now_b = cmd("Restart now and check for problems (recommended)", "Save your work and close any open programs before restarting.");
            let next_b = cmd("Check for problems the next time I start my computer", "");
            v.pack_start(&now_b, false, false, 0);
            v.pack_start(&next_b, false, false, 0);
            let set_next = { let (kind, id) = (kind, id.clone()); move |reboot: bool| {
                let mut script = if kind == "systemd-boot" { format!("bootctl set-oneshot {}", q(&id)) } else { format!("grub-reboot {}", q(&id)) };
                if reboot { script += " && systemctl reboot"; }
                elevate_sh(&script, move |ok, out| { if report("The memory test could not be scheduled.", ok, &out) && !reboot { inform(None, "Windows Memory Diagnostic", "Windows will check your memory the next time you start your computer.", ""); } });
            } };
            { let (s, w) = (set_next.clone(), w.clone()); now_b.connect_clicked(move |_| { if confirm(Some(&w), "Windows Memory Diagnostic", "Restart now and check for problems?", "Save your work first. The test runs until you stop it with Esc; then the computer starts normally.", "Restart") { s(true); } }); }
            { let (s, w) = (set_next.clone(), w.clone()); next_b.connect_clicked(move |_| { s(false); w.close(); }); }
        }
        None => {
            v.pack_start(&label("Memtest86+ is not in the boot menu. Windows Memory Diagnostic uses it to test the memory before the operating system starts.", ""), false, false, 0);
            let inst = cmd("Install the memory test", "Installs memtest86+ and adds it to the boot menu (needs administrator permission).");
            v.pack_start(&inst, false, false, 0);
            let w2 = w.clone();
            inst.connect_clicked(move |_| {
                let efi = std::path::Path::new("/sys/firmware/efi").exists();
                let pkg = if efi { "memtest86+-efi" } else { "memtest86+" };
                let regen = if std::path::Path::new("/boot/grub/grub.cfg").exists() { " && grub-mkconfig -o /boot/grub/grub.cfg" } else { "" };
                if !confirm(Some(&w2), "Windows Memory Diagnostic", &format!("Install {pkg}?"), "With systemd-boot, copy the memtest EFI file to the EFI partition and add a loader entry afterwards (see the Arch Wiki).", "Install") { return; }
                let w3 = w2.clone();
                elevate_sh(&format!("pacman -S --needed --noconfirm {pkg}{regen}"), move |ok, out| { if report("The memory test could not be installed.", ok, &out) { w3.close(); } });
            });
        }
    }
    let bb = hbox(0);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_size_request(80, -1);
    bb.pack_end(&cancel, false, false, 0);
    { let w = w.clone(); cancel.connect_clicked(move |_| w.close()); }
    v.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 4);
    v.pack_start(&bb, false, false, 0);
    w.add(&v);
    w.show_all();
    w
}
