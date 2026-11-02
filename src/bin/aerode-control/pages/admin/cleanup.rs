//! Disk Cleanup (cleanmgr): "You can use Disk Cleanup to free up to X of disk space" with the
//! categories and their sizes (computed on a worker thread): package cache (paccache), thumbnails,
//! Recycle Bin, temporary files older than a week, crash dumps, system logs (journal), unused
//! packages (orphans). The user's own files are removed directly, the system ones under one
//! pkexec prompt.
use gtk::prelude::*;
use gtk::glib;
use std::path::Path;

use super::*;

#[derive(Clone, Debug)]
pub struct Item { pub key: &'static str, pub name: &'static str, pub desc: &'static str, pub size: Option<u64>, pub on: bool, pub system: bool, pub folder: String }

fn du(p: &Path) -> u64 { sh(&["du", "-sb", &p.to_string_lossy()]).split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0) }

/// "disk space saved: 1.23 GiB" (paccache dry run) → bytes
fn saved(out: &str) -> u64 {
    let Some(s) = out.split("disk space saved:").nth(1) else { return 0 };
    let mut it = s.split_whitespace();
    let n: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
    (n * match it.next().map(|u| u.trim_end_matches(')')) { Some("KiB") => 1024.0, Some("MiB") => 1048576.0, Some("GiB") => 1073741824.0, Some("TiB") => 1099511627776.0, _ => 1.0 }) as u64
}

/// Sizes of everything that can go (worker thread).
pub fn scan() -> Vec<Item> {
    let cache = glib::user_cache_dir();
    let mut v = vec![];
    let pkg = if util::which("paccache") { saved(&sh(&["paccache", "-dk2"])) + saved(&sh(&["paccache", "-duk0"])) } else { 0 };
    v.push(Item { key: "pkgcache", name: "Downloaded Program Files", desc: "Old versions of installed packages and packages that are no longer installed, kept by pacman in /var/cache/pacman/pkg. The two newest versions of each installed package are kept so you can still go back.", size: if util::which("paccache") { Some(pkg) } else { None }, on: true, system: true, folder: "/var/cache/pacman/pkg".into() });
    let thumbs = cache.join("thumbnails");
    v.push(Item { key: "thumbs", name: "Thumbnails", desc: "Windows keeps a copy of all of your picture, video, and document thumbnails so they can be displayed quickly when you open a folder. If you delete these thumbnails, they will be automatically recreated as needed.", size: Some(du(&thumbs)), on: true, system: false, folder: thumbs.to_string_lossy().into() });
    let trash = aerode::config::home().join(".local/share/Trash");
    v.push(Item { key: "trash", name: "Recycle Bin", desc: "The Recycle Bin contains files you have deleted from your computer. These files are not permanently removed until you empty the Recycle Bin.", size: Some(du(&trash)), on: false, system: false, folder: "trash:///".into() });
    // temporary files: ~/.cache entries untouched for a week (besides thumbnails)
    let mut tmp = 0;
    if let Ok(rd) = std::fs::read_dir(&cache) {
        for e in rd.flatten() {
            if e.file_name() == "thumbnails" || e.file_name() == "aerode" { continue; }
            if mtime(&e.path()).map(|t| now() - t > 7 * 86400).unwrap_or(false) { tmp += du(&e.path()); }
        }
    }
    v.push(Item { key: "temp", name: "Temporary files", desc: "Programs sometimes store temporary information in your cache folder (~/.cache). Entries that have not been used for more than a week are deleted.", size: Some(tmp), on: false, system: false, folder: cache.to_string_lossy().into() });
    let dumps = Path::new("/var/lib/systemd/coredump");
    v.push(Item { key: "dumps", name: "System error memory dump files", desc: "Remove memory dumps of programs that stopped working (systemd-coredump). They are only useful when you investigate the crash.", size: Some(du(dumps)), on: true, system: true, folder: dumps.to_string_lossy().into() });
    let usage = sh(&["journalctl", "--disk-usage"]);
    let j = usage.split("take up ").nth(1).and_then(|s| s.split_whitespace().next()).map(|s| {
        let (n, u) = s.split_at(s.find(|c: char| c.is_alphabetic()).unwrap_or(s.len()));
        (n.parse::<f64>().unwrap_or(0.0) * match u { "K" => 1024.0, "M" => 1048576.0, "G" => 1073741824.0, "T" => 1099511627776.0, _ => 1.0 }) as u64
    }).unwrap_or(0);
    v.push(Item { key: "logs", name: "Setup Log Files", desc: "System logs (the systemd journal) are trimmed to the newest 200 MB. Older events will no longer be shown in Event Viewer.", size: Some(j.saturating_sub(200 * 1048576)), on: false, system: true, folder: "/var/log/journal".into() });
    let orphans = sh(&["pacman", "-Qtdq"]);
    let names: Vec<&str> = orphans.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut osize = 0;
    if !names.is_empty() {
        let mut a = vec!["pacman", "-Qi"];
        a.extend(names.iter());
        for l in sh(&a).lines() { if let Some(s) = l.strip_prefix("Installed Size") { let s = s.trim_start_matches([' ', ':']); let mut it = s.split_whitespace(); let n: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0); osize += (n * match it.next() { Some("KiB") => 1024.0, Some("MiB") => 1048576.0, Some("GiB") => 1073741824.0, _ => 1.0 }) as u64; } }
    }
    v.push(Item { key: "orphans", name: "Windows Update Cleanup (unused packages)", desc: "Packages that were installed as dependencies and are no longer needed by any other package (pacman -Qtdq).", size: Some(osize), on: false, system: true, folder: String::new() });
    v
}

/// What the selected categories do.
fn clean_user(keys: &[&str]) -> String {
    let mut errs = String::new();
    let cache = glib::user_cache_dir();
    if keys.contains(&"thumbs") { if let Ok(rd) = std::fs::read_dir(cache.join("thumbnails")) { for e in rd.flatten() { let _ = std::fs::remove_dir_all(e.path()).or_else(|_| std::fs::remove_file(e.path())); } } }
    if keys.contains(&"trash") { let (ok, out) = run(&["gio", "trash", "--empty"]); if !ok { let t = aerode::config::home().join(".local/share/Trash"); for d in ["files", "info", "expunged"] { let _ = std::fs::remove_dir_all(t.join(d)); } let _ = out; } }
    if keys.contains(&"temp") {
        if let Ok(rd) = std::fs::read_dir(&cache) {
            for e in rd.flatten() {
                if e.file_name() == "thumbnails" || e.file_name() == "aerode" { continue; }
                if mtime(&e.path()).map(|t| now() - t > 7 * 86400).unwrap_or(false) { if let Err(x) = std::fs::remove_dir_all(e.path()).or_else(|_| std::fs::remove_file(e.path())) { errs += &format!("{}: {x}\n", e.path().display()); } }
            }
        }
    }
    errs
}

fn system_script(keys: &[&str]) -> String {
    let mut s = vec![];
    if keys.contains(&"pkgcache") { s.push("paccache -rk2 && paccache -ruk0".to_string()); }
    if keys.contains(&"dumps") { s.push("rm -f /var/lib/systemd/coredump/*".into()); }
    if keys.contains(&"logs") { s.push("journalctl --vacuum-size=200M".into()); }
    if keys.contains(&"orphans") { s.push("o=$(pacman -Qtdq); [ -z \"$o\" ] || pacman -Rns --noconfirm $o".into()); }
    s.join("; ")
}

pub fn window(app: &gtk::Application) -> gtk::Window {
    let w = gtk::Window::new(gtk::WindowType::Toplevel);
    w.set_application(Some(app));
    w.set_icon(pix(&["aerode-cleanmgr", "user-trash-full"], 48).as_ref());
    w.style_context().add_class("adm");
    w.set_resizable(false);
    w.set_position(gtk::WindowPosition::CenterAlways);
    // first the "calculating" box, like Win7
    w.set_title("Disk Cleanup");
    let calc = vbox(10);
    calc.set_border_width(16);
    let root_name = std::fs::read_to_string("/proc/mounts").unwrap_or_default().lines().find_map(|l| { let f: Vec<&str> = l.split_whitespace().collect(); (f.len() > 1 && f[1] == "/").then(|| f[0].to_string()) }).unwrap_or_else(|| "/".into());
    calc.pack_start(&label(&format!("Disk Cleanup is calculating how much space you will be able to free on ({root_name}). This may take a few minutes to complete."), ""), false, false, 0);
    let pb = gtk::ProgressBar::new();
    calc.pack_start(&pb, false, false, 0);
    let scanning = label("Scanning: Downloaded Program Files", "");
    calc.pack_start(&scanning, false, false, 0);
    let cancel0 = gtk::Button::with_label("Cancel");
    cancel0.set_halign(gtk::Align::End);
    { let w = w.clone(); cancel0.connect_clicked(move |_| w.close()); }
    calc.pack_start(&cancel0, false, false, 0);
    calc.set_size_request(420, -1);
    w.add(&calc);
    w.show_all();
    let p2 = pb.clone();
    let tick = glib::timeout_add_local(std::time::Duration::from_millis(90), move || { p2.pulse(); glib::ControlFlow::Continue });
    let tick = std::cell::RefCell::new(Some(tick));
    let w2 = w.clone();
    bg(scan, move |items| {
        if let Some(t) = tick.borrow_mut().take() { t.remove(); }
        w2.remove(&calc);
        w2.set_title(&format!("Disk Cleanup for ({root_name})"));
        main_dialog(&w2, &root_name, items);
    });
    w
}

fn main_dialog(w: &gtk::Window, root: &str, items: Vec<Item>) {
    let v = vbox(8);
    v.set_border_width(10);
    let nb = gtk::Notebook::new();
    let p = vbox(8);
    p.set_border_width(10);
    let total: u64 = items.iter().filter_map(|i| i.size).sum();
    let h = hbox(10);
    h.pack_start(&img(&["aerode-cleanmgr", "user-trash-full"], 32), false, false, 0);
    let summary = label(&format!("You can use Disk Cleanup to free up to {} of disk space on ({root}).", gb(total)), "");
    summary.set_max_width_chars(48);
    h.pack_start(&summary, true, true, 0);
    p.pack_start(&h, false, false, 0);
    p.pack_start(&label("Files to delete:", ""), false, false, 0);
    let store = gtk::ListStore::new(&[glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING, glib::Type::U32]);
    for (i, it) in items.iter().enumerate() { store.insert_with_values(None, &[(0, &it.on), (1, &it.name), (2, &it.size.map(gb).unwrap_or_else(|| "–".into())), (3, &(i as u32))]); }
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    let tog = gtk::CellRendererToggle::new();
    let c = gtk::TreeViewColumn::new();
    TreeViewColumnExt::pack_start(&c, &tog, false);
    TreeViewColumnExt::add_attribute(&c, &tog, "active", 0);
    let pr = gtk::CellRendererPixbuf::new();
    TreeViewColumnExt::pack_start(&c, &pr, false);
    pr.set_property("pixbuf", pix(&["user-trash", "edit-delete"], 16));
    let tr = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&c, &tr, true);
    TreeViewColumnExt::add_attribute(&c, &tr, "text", 1);
    c.set_expand(true);
    tv.append_column(&c);
    let sc = gtk::TreeViewColumn::new();
    let sr = gtk::CellRendererText::new();
    sr.set_xalign(1.0);
    TreeViewColumnExt::pack_start(&sc, &sr, true);
    TreeViewColumnExt::add_attribute(&sc, &sr, "text", 2);
    tv.append_column(&sc);
    { let store = store.clone(); tog.connect_toggled(move |_, path| { if let Some(it) = store.iter(&path) { let v: bool = store.value(&it, 0).get().unwrap_or(false); store.set_value(&it, 0, &(!v).to_value()); } }); }
    let sw = scrolled(&tv);
    sw.set_min_content_height(120);
    sw.set_max_content_height(120);
    p.pack_start(&sw, false, false, 0);
    let gain = label("", "");
    p.pack_start(&gain, false, false, 0);
    let upd_gain = { let (store, gain, items) = (store.clone(), gain.clone(), items.clone()); move || {
        let mut n = 0u64;
        store.foreach(|m, _, it| { let on: bool = m.value(it, 0).get().unwrap_or(false); let i: u32 = m.value(it, 3).get().unwrap_or(0); if on { n += items[i as usize].size.unwrap_or(0); } false });
        gain.set_text(&format!("Total amount of disk space you gain:   {}", gb(n)));
    } };
    upd_gain();
    { let u = upd_gain.clone(); store.connect_row_changed(move |_, _, _| u()); }
    let fr = gtk::Frame::new(Some("Description"));
    let desc = label("", "");
    desc.set_max_width_chars(46);
    desc.set_size_request(-1, 54);
    desc.set_yalign(0.0);
    let fb = vbox(6);
    fb.set_border_width(8);
    fb.pack_start(&desc, true, true, 0);
    let view = gtk::Button::with_mnemonic("_View Files");
    view.set_halign(gtk::Align::End);
    fb.pack_start(&view, false, false, 0);
    fr.add(&fb);
    p.pack_start(&fr, false, false, 0);
    let folder = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    { let (desc, items, folder, view) = (desc.clone(), items.clone(), folder.clone(), view.clone()); tv.selection().connect_changed(move |s| if let Some((m, it)) = s.selected() {
        let i: u32 = m.value(&it, 3).get().unwrap_or(0);
        let item = &items[i as usize];
        desc.set_text(&if item.system { format!("{}\n\nDeleting these files asks for administrator permission.", item.desc) } else { item.desc.to_string() });
        *folder.borrow_mut() = item.folder.clone();
        view.set_sensitive(!item.folder.is_empty());
    }); }
    tv.selection().select_path(&gtk::TreePath::new_first());
    { let folder = folder.clone(); view.connect_clicked(move |_| { let f = folder.borrow().clone(); if f.contains("://") { util::open_uri(&f); } else { util::explore(&f); } }); }
    let csf = shield_button("Clean up _system files");
    csf.set_halign(gtk::Align::Start);
    csf.set_tooltip_text(Some("System categories are already listed; deleting them asks for administrator permission."));
    csf.set_sensitive(false);
    p.pack_start(&csf, false, false, 0);
    p.pack_start(&link_label("How does Disk Cleanup work?", || { util::open_uri("https://wiki.archlinux.org/title/Pacman#Cleaning_the_package_cache"); }), false, false, 0);
    nb.append_page(&p, Some(&gtk::Label::new(Some("Disk Cleanup"))));
    // More Options
    let mo = vbox(10);
    mo.set_border_width(10);
    for (t, txt, act) in [("Programs and Features", "You can free more disk space by removing programs that you do not use.", "programs"), ("System Restore and Shadow Copies", "You can free more disk space by removing all but the most recent restore point.", "backup")] {
        let f = gtk::Frame::new(Some(t));
        let b = hbox(10);
        b.set_border_width(8);
        let l = label(txt, ""); l.set_max_width_chars(38); b.pack_start(&l, true, true, 0);
        let btn = gtk::Button::with_mnemonic("_Clean up...");
        btn.set_valign(gtk::Align::End);
        btn.connect_clicked(move |_| { util::spawn(&["aerode-control", act]); });
        b.pack_start(&btn, false, false, 0);
        f.add(&b);
        mo.pack_start(&f, false, false, 0);
    }
    nb.append_page(&mo, Some(&gtk::Label::new(Some("More Options"))));
    v.pack_start(&nb, true, true, 0);
    let bb = hbox(8);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    ok.set_size_request(80, -1);
    cancel.set_size_request(80, -1);
    bb.pack_end(&cancel, false, false, 0);
    bb.pack_end(&ok, false, false, 0);
    v.pack_start(&bb, false, false, 0);
    { let w = w.clone(); cancel.connect_clicked(move |_| w.close()); }
    { let (w, store, items) = (w.clone(), store.clone(), items.clone()); ok.connect_clicked(move |_| {
        let mut keys: Vec<&'static str> = vec![];
        store.foreach(|m, _, it| { let on: bool = m.value(it, 0).get().unwrap_or(false); let i: u32 = m.value(it, 3).get().unwrap_or(0); if on { keys.push(items[i as usize].key); } false });
        if keys.is_empty() { w.close(); return; }
        let d = gtk::MessageDialog::new(Some(&w), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::None, "Are you sure you want to permanently delete these files?");
        d.set_title("Disk Cleanup");
        d.add_button("Delete Files", gtk::ResponseType::Ok);
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        let r = d.run();
        d.close();
        if r != gtk::ResponseType::Ok { return; }
        w.hide();
        let w2 = w.clone();
        let user_keys = keys.clone();
        let sys = system_script(&keys);
        bg(move || clean_user(&user_keys), move |errs| {
            let done = { let w2 = w2.clone(); move |e: String| { if !e.trim().is_empty() { util::error("Some files could not be deleted.", e.trim()); } w2.close(); } };
            if sys.is_empty() { done(errs); } else { elevate_sh(&sys, move |ok, out| done(if ok { errs } else if out.contains("Not authorized") || out.contains("dismissed") { errs } else { errs + &out })); }
        });
    }); }
    w.add(&v);
    w.show_all();
}

fn link_label(t: &str, f: impl Fn() + 'static) -> gtk::Button { super::super::super::link(t, f) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paccache_out() {
        assert_eq!(saved("==> finished dry run: 12 candidates (disk space saved: 1.50 GiB)"), 1610612736);
        assert_eq!(saved("==> no candidate packages found for pruning"), 0);
        assert!(system_script(&["pkgcache", "logs"]).contains("paccache -rk2"));
    }
}
