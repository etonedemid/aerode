//! HomeGroup: Windows 7 homegroup sharing backed by Samba user shares (`net usershare`). Sharing a
//! library = a user share of that folder; "Create a homegroup" turns on user shares and the Samba
//! service and sets the network password (one UAC prompt).
use gtk::prelude::*;
use gtk::glib;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use aerode::util;
use crate::{link, section, text, title, Cp};
use super::common::*;

/// Turn on Samba user shares (run as root): usershare directory and group, the [global] settings,
/// the account in sambashare, smb + nmb running, optionally the network password from stdin and
/// the first shares. Args: user, then share name/path pairs.
const ENABLE: &str = r#"set -e
user="$1"; shift
conf=/etc/samba/smb.conf
mkdir -p /etc/samba /var/lib/samba/usershares
getent group sambashare >/dev/null || groupadd -r sambashare
chgrp sambashare /var/lib/samba/usershares
chmod 1770 /var/lib/samba/usershares
[ -f "$conf" ] || printf '[global]\n   workgroup = WORKGROUP\n   server string = %%h\n   server role = standalone server\n   map to guest = Bad User\n' > "$conf"
if ! grep -q '^[[:space:]]*usershare path' "$conf"; then
  if grep -q '^\[global\]' "$conf"; then
    sed -i '/^\[global\]/a\   usershare path = /var/lib/samba/usershares\n   usershare max shares = 100\n   usershare allow guests = yes\n   usershare owner only = yes' "$conf"
  else
    printf '[global]\n   usershare path = /var/lib/samba/usershares\n   usershare max shares = 100\n   usershare allow guests = yes\n   usershare owner only = yes\n\n' | cat - "$conf" > "$conf.new" && mv "$conf.new" "$conf"
  fi
fi
gpasswd -a "$user" sambashare >/dev/null
if [ -n "$AERODE_SET_PASSWORD" ]; then smbpasswd -a -s "$user" >/dev/null; fi
if [ -f /usr/lib/systemd/system/smb.service ]; then systemctl enable --now smb nmb; else systemctl enable --now smbd nmbd; fi
while [ $# -ge 2 ]; do runuser -u "$user" -- net usershare add "$1" "$2" "" Everyone:R guest_ok=n || true; shift 2; done
"#;

fn user() -> String { glib::user_name().to_string_lossy().into_owned() }

/// The repair the HomeGroup troubleshooter uses.
pub(super) fn enable_argv() -> Vec<String> { sv(&["sh", "-c", ENABLE, "sh", &user()]) }

#[derive(Clone, Debug, Default)]
struct Share { name: String, path: String, comment: String, write: bool, guest: bool }

#[derive(Clone, Debug, Default)]
struct Status {
    installed: bool,
    running: bool,
    /// None = user shares work; Some(why) = they don't
    usershares: Option<String>,
    shares: Vec<Share>,
    netbios: String,
    workgroup: String,
    /// "Printers": the CUPS printers are shared (None = printing is not installed)
    printers: Option<bool>,
    /// "Stream my pictures, music, and videos…": Rygel runs (None = Rygel is not installed)
    streaming: Option<bool>,
}

/// Rygel's systemd user unit (else it is started from autostart).
const RYGEL_UNIT: &str = "/usr/lib/systemd/user/rygel.service";

fn rygel_autostart() -> PathBuf { glib::user_config_dir().join("autostart/rygel.desktop") }

/// Printer sharing as CUPS reports it for the first printer (cupsd.conf itself is root-only).
fn printers_shared() -> Option<bool> {
    if !util::which("lpstat") { return None; }
    let first = exec(&sv(&["lpstat", "-e"]), None).stdout.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())?;
    let o = exec(&sv(&["lpoptions", "-p", &first]), None).stdout;
    Some(o.contains("printer-is-shared=true") || o.contains("printer-is-shared=1"))
}

fn streaming() -> Option<bool> {
    if !util::which("rygel") { return None; }
    if Path::new(RYGEL_UNIT).exists() {
        return Some(exec(&sv(&["systemctl", "--user", "is-active", "rygel"]), None).stdout.trim() == "active");
    }
    Some(rygel_autostart().exists())
}

/// Turn media streaming (Rygel, a DLNA/UPnP media server) on or off for this user.
fn set_streaming(on: bool) -> Vec<Vec<String>> {
    if Path::new(RYGEL_UNIT).exists() {
        return vec![sv(&["systemctl", "--user", if on { "enable" } else { "disable" }, "--now", "rygel"])];
    }
    let f = rygel_autostart();
    if on {
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        if !dry_run() { let _ = std::fs::write(&f, "[Desktop Entry]\nType=Application\nName=Media streaming (Rygel)\nExec=rygel\nNoDisplay=true\n"); }
        vec![sv(&["sh", "-c", "pgrep -u \"$(id -u)\" -x rygel >/dev/null || (setsid rygel >/dev/null 2>&1 &)"])]
    } else {
        if !dry_run() { let _ = std::fs::remove_file(&f); }
        vec![sv(&["sh", "-c", "pkill -u \"$(id -u)\" -x rygel; true"])]
    }
}

/// Share (or stop sharing) every CUPS printer: the server setting and each printer's flag.
const SHARE_PRINTERS: &str = r#"set -e
if [ "$1" = on ]; then cupsctl --share-printers; v=true; else cupsctl --no-share-printers; v=false; fi
for p in $(lpstat -e); do lpadmin -p "$p" -o printer-is-shared=$v; done
"#;

fn parse_info(s: &str) -> Vec<Share> {
    let mut v: Vec<Share> = vec![];
    for l in s.lines() {
        let l = l.trim();
        if let Some(n) = l.strip_prefix('[').and_then(|x| x.strip_suffix(']')) { v.push(Share { name: n.to_string(), ..Default::default() }); continue; }
        let Some(sh) = v.last_mut() else { continue };
        let Some((k, val)) = l.split_once('=') else { continue };
        match k.trim() {
            "path" => sh.path = val.trim().to_string(),
            "comment" => sh.comment = val.trim().to_string(),
            "usershare_acl" => sh.write = val.split(',').any(|a| a.ends_with(":F") || a.ends_with(":f")),
            "guest_ok" => sh.guest = val.trim() == "y",
            _ => {}
        }
    }
    v
}

fn status() -> Status {
    let installed = util::which("smbd") || ["/usr/bin/smbd", "/usr/sbin/smbd"].iter().any(|p| Path::new(p).exists());
    let mut st = Status { installed, ..Default::default() };
    let host = glib::host_name().to_string();
    st.netbios = host.split('.').next().unwrap_or(&host).to_uppercase().chars().take(15).collect();
    st.workgroup = "WORKGROUP".into();
    st.printers = printers_shared();
    st.streaming = streaming();
    if !installed { return st; }
    let param = |p: &str| exec(&sv(&["testparm", "-s", "--parameter-name", p]), None).stdout.trim().to_string();
    let nb = param("netbios name");
    if !nb.is_empty() { st.netbios = nb; }
    let wg = param("workgroup");
    if !wg.is_empty() { st.workgroup = wg; }
    let unit = if Path::new("/usr/lib/systemd/system/smb.service").exists() { "smb" } else { "smbd" };
    st.running = exec(&sv(&["systemctl", "is-active", unit]), None).stdout.trim() == "active";
    let o = exec(&sv(&["net", "usershare", "info"]), None);
    if o.ok { st.shares = parse_info(&o.stdout); } else {
        let why = o.text();
        st.usershares = Some(if why.is_empty() { "Sharing folders is not set up on this computer.".into() } else { why.lines().next().unwrap_or("").to_string() });
    }
    st
}

/// The four Windows 7 libraries.
fn libraries() -> Vec<(&'static str, PathBuf)> {
    vec![
        ("Pictures", util::special_dir(glib::UserDirectory::Pictures, "Pictures")),
        ("Music", util::special_dir(glib::UserDirectory::Music, "Music")),
        ("Videos", util::special_dir(glib::UserDirectory::Videos, "Videos")),
        ("Documents", util::special_dir(glib::UserDirectory::Documents, "Documents")),
    ]
}

pub fn page_homegroup(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    let holder = vbox(0);
    holder.pack_start(&loading("Checking homegroup and sharing status…"), false, false, 0);
    load(cp, &holder);
    holder.upcast()
}

fn load(cp: &Rc<Cp>, holder: &gtk::Box) {
    let (me, h) = (cp.clone(), holder.clone());
    background(status, move |st| {
        let page = if !st.installed { not_installed(&me, &h) } else if st.usershares.is_some() || !st.running && st.shares.is_empty() { no_homegroup(&me, &h, &st) } else { settings(&me, &h, &st) };
        fill(&h, &page);
    });
}

fn reload(cp: &Rc<Cp>, holder: &gtk::Box) {
    fill(holder, &loading("Checking homegroup and sharing status…"));
    load(cp, holder);
}

fn about(parent: &gtk::Window) {
    message(Some(parent), gtk::MessageType::Info, "HomeGroup", "What is a homegroup?",
        "A homegroup makes it easy to share your pictures, music, videos and documents with the other computers on your home network. On this computer it is Windows file sharing (SMB) provided by Samba, so Windows, Mac and Linux computers can open the shared folders: in Windows Explorer type \\\\COMPUTERNAME in the address bar.");
}

fn other_links(cp: &Rc<Cp>, v: &gtk::Box) {
    let me = cp.clone();
    v.pack_start(&link("Tell me more about homegroups", move || about(&me.win.clone().upcast())), false, false, 0);
    let me = cp.clone();
    v.pack_start(&link("Change advanced sharing settings...", move || me.open("network")), false, false, 0);
    let me = cp.clone();
    v.pack_start(&link("Start the HomeGroup troubleshooter", move || super::troubleshoot::run(Some(&me), "homegroup")), false, false, 0);
}

fn banner(icon: &str, msg: &str) -> gtk::Box {
    let h = hbox(12);
    h.pack_start(&util::image(icon, 48), false, false, 0);
    let l = label(msg);
    l.style_context().add_class("xt-name");
    h.pack_start(&l, true, true, 0);
    h
}

fn not_installed(cp: &Rc<Cp>, holder: &gtk::Box) -> gtk::Widget {
    let v = vbox(10);
    v.pack_start(&title("Share with other home computers running Windows 7"), false, false, 0);
    v.pack_start(&banner("folder-publicshare", "File and printer sharing is not installed on this computer."), false, false, 0);
    v.pack_start(&text("With a homegroup, you can share files and printers with other computers running Windows 7. You can also stream media to devices."), false, false, 0);
    v.pack_start(&text("To share with other computers, Windows needs the Samba file sharing service. Install the samba package, and then create a homegroup."), false, false, 0);
    v.pack_start(&crate::kv_grid(&[("Package to install:", "samba".into()), ("Command:", "sudo pacman -S samba".into())]), false, false, 0);
    other_links(cp, &v);
    let install = shield_button("_Install file sharing");
    let cancel = gtk::Button::with_label("Cancel");
    { let me = cp.clone(); cancel.connect_clicked(move |_| me.open("cat:network")); }
    let (me, h) = (cp.clone(), holder.clone());
    install.connect_clicked(move |b| {
        let w: gtk::Window = me.win.clone().upcast();
        if !ask(Some(&w), gtk::MessageType::Question, "HomeGroup", "Install file sharing (Samba)?", "The samba package is downloaded and installed. This needs an Internet connection and an administrator password.", "_Install", "Cancel") { return; }
        b.set_sensitive(false);
        fill(&h, &loading("Installing Samba…"));
        let (me, h) = (me.clone(), h.clone());
        admin_change(&["pacman", "-S", "--needed", "--noconfirm", "samba"], move |o| {
            if !o.ok && !o.cancelled() { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "HomeGroup", "Samba could not be installed.", &o.text()); }
            reload(&me, &h);
        });
    });
    let row = button_row(&[&install, &cancel]);
    row.set_margin_top(20);
    v.pack_start(&row, false, false, 0);
    v.upcast()
}

fn no_homegroup(cp: &Rc<Cp>, holder: &gtk::Box, st: &Status) -> gtk::Widget {
    let v = vbox(10);
    v.pack_start(&title("Share with other home computers running Windows 7"), false, false, 0);
    v.pack_start(&banner("folder-publicshare", "There is currently no homegroup on this computer."), false, false, 0);
    v.pack_start(&text("With a homegroup, you can share files and printers with other computers running Windows 7. You can also stream media to devices."), false, false, 0);
    v.pack_start(&text("The homegroup is protected with a password, and you'll always be able to choose what you share."), false, false, 0);
    if let Some(why) = &st.usershares { v.pack_start(&grey(&format!("Folder sharing is turned off: {why}")), false, false, 0); }
    else if !st.running { v.pack_start(&grey("The file sharing service (Samba) is not running."), false, false, 0); }
    other_links(cp, &v);
    let create = shield_button("C_reate a homegroup");
    let cancel = gtk::Button::with_label("Cancel");
    { let me = cp.clone(); cancel.connect_clicked(move |_| me.open("cat:network")); }
    { let (me, h) = (cp.clone(), holder.clone()); create.connect_clicked(move |_| create_wizard(&me, &h)); }
    let row = button_row(&[&create, &cancel]);
    row.set_margin_top(20);
    v.pack_start(&row, false, false, 0);
    v.upcast()
}

/// "Create a Homegroup": what to share, the network password, then one UAC prompt.
fn create_wizard(cp: &Rc<Cp>, holder: &gtk::Box) {
    let wiz = wizard("Create a Homegroup", "folder-publicshare", Some(&cp.win.clone().upcast()), (560, 430), true);
    wiz.back.set_sensitive(false);
    let v = vbox(10);
    v.pack_start(&instr("Share with other home computers running Windows 7"), false, false, 0);
    v.pack_start(&label("Your computer can share files and printers with other computers running Windows 7, and you can stream media to devices. The homegroup is protected with a password, and you'll always be able to choose what you share."), false, false, 0);
    v.pack_start(&label("Select what you want to share:"), false, false, 6);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(40);
    grid.set_row_spacing(6);
    let mut checks = vec![];
    for (i, (name, path)) in libraries().into_iter().enumerate() {
        let c = gtk::CheckButton::with_label(name);
        c.set_active(name != "Documents" && path.is_dir());
        c.set_sensitive(path.is_dir());
        grid.attach(&c, (i % 2) as i32, (i / 2) as i32, 1, 1);
        checks.push((c, name, path));
    }
    v.pack_start(&grid, false, false, 0);
    wiz.page(&v);
    let next = gtk::Button::with_mnemonic("_Next");
    let cancel = gtk::Button::with_label("Cancel");
    { let w = wiz.win.clone(); cancel.connect_clicked(move |_| w.close()); }
    wiz.set_buttons(&[&next, &cancel]);
    let (wz, me, h) = (wiz.clone(), cp.clone(), holder.clone());
    next.connect_clicked(move |_| {
        let picked: Vec<(String, String)> = checks.iter().filter(|(c, _, _)| c.is_active()).map(|(_, n, p)| (n.to_string(), p.to_string_lossy().into_owned())).collect();
        password_page(&wz, &me, &h, picked);
    });
    wiz.win.show_all();
}

fn password_page(wiz: &Rc<Wizard>, cp: &Rc<Cp>, holder: &gtk::Box, picked: Vec<(String, String)>) {
    let v = vbox(10);
    v.pack_start(&instr("Type a password for your homegroup"), false, false, 0);
    v.pack_start(&label(&format!("Other computers open your shared folders with your user name ({}) and this password. It can be different from the password you use to log on.", user())), false, false, 0);
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(10);
    let (p1, p2) = (gtk::Entry::new(), gtk::Entry::new());
    for (i, (l, e)) in [("Password:", &p1), ("Confirm password:", &p2)].into_iter().enumerate() {
        e.set_visibility(false);
        e.set_input_purpose(gtk::InputPurpose::Password);
        let lab = gtk::Label::new(Some(l));
        lab.set_xalign(0.0);
        g.attach(&lab, 0, i as i32, 1, 1);
        g.attach(e, 1, i as i32, 1, 1);
    }
    v.pack_start(&g, false, false, 0);
    v.pack_start(&grey("Leave both boxes empty to keep the password you set before."), false, false, 0);
    let err = grey("");
    v.pack_start(&err, false, false, 0);
    wiz.page(&v);
    let finish = shield_button("_Finish");
    let cancel = gtk::Button::with_label("Cancel");
    { let w = wiz.win.clone(); cancel.connect_clicked(move |_| w.close()); }
    wiz.set_buttons(&[&finish, &cancel]);
    let (wz, me, h) = (wiz.clone(), cp.clone(), holder.clone());
    finish.connect_clicked(move |b| {
        let (a, c) = (p1.text().to_string(), p2.text().to_string());
        if a != c { err.set_text("The passwords don't match."); return; }
        b.set_sensitive(false);
        let mut argv = vec!["pkexec".to_string(), "env".to_string()];
        if !a.is_empty() { argv.push("AERODE_SET_PASSWORD=1".into()); }
        argv.extend(enable_argv());
        for (n, p) in &picked { argv.push(n.clone()); argv.push(p.clone()); }
        let stdin = (!a.is_empty()).then(|| format!("{a}\n{a}\n"));
        fill(&wz.body, &loading("Creating the homegroup…"));
        let (wz2, me2, h2) = (wz.clone(), me.clone(), h.clone());
        change(argv, stdin, move |o| {
            if o.ok {
                message(Some(&wz2.win), gtk::MessageType::Info, "Create a Homegroup", "Your homegroup is ready.",
                    "Log off and log on again once, so that you can share more folders from Windows Explorer.");
                wz2.win.close();
            } else if o.cancelled() { wz2.win.close(); }
            else {
                message(Some(&wz2.win), gtk::MessageType::Error, "Create a Homegroup", "Windows couldn't create the homegroup.", &o.text());
                wz2.win.close();
            }
            reload(&me2, &h2);
        });
    });
}

fn settings(cp: &Rc<Cp>, holder: &gtk::Box, st: &Status) -> gtk::Widget {
    let v = vbox(8);
    v.pack_start(&title("Change homegroup settings"), false, false, 0);
    v.pack_start(&banner("folder-publicshare", "This computer belongs to a homegroup."), false, false, 0);
    v.pack_start(&grey(&format!("Other computers on the network see this computer as \\\\{} (workgroup {}).", st.netbios, st.workgroup)), false, false, 0);
    if !st.running {
        let h = hbox(8);
        h.pack_start(&mark(Mark::Warn), false, false, 0);
        h.pack_start(&label("The file sharing service is not running, so other computers can't open your shared folders."), false, false, 0);
        let start = shield_button("_Start");
        let (me, hd) = (cp.clone(), holder.clone());
        start.connect_clicked(move |_| {
            let unit = if Path::new("/usr/lib/systemd/system/smb.service").exists() { "smb" } else { "smbd" };
            let (me, hd) = (me.clone(), hd.clone());
            admin_change(&["systemctl", "enable", "--now", unit], move |_| reload(&me, &hd));
        });
        h.pack_end(&start, false, false, 0);
        v.pack_start(&h, false, false, 0);
    }
    v.pack_start(&section("Share libraries and printers"), false, false, 0);
    let grid = gtk::Grid::new();
    grid.set_column_spacing(60);
    grid.set_row_spacing(6);
    grid.set_margin_start(16);
    let mut checks = vec![];
    for (i, (name, path)) in libraries().into_iter().enumerate() {
        let p = path.to_string_lossy().into_owned();
        let shared = st.shares.iter().find(|s| s.path.trim_end_matches('/') == p.trim_end_matches('/')).cloned();
        let c = gtk::CheckButton::with_label(name);
        c.set_active(shared.is_some());
        c.set_sensitive(path.is_dir());
        grid.attach(&c, (i % 3) as i32, (i / 3) as i32, 1, 1);
        checks.push((c, name, p, shared));
    }
    let printers = gtk::CheckButton::with_label("Printers");
    printers.set_active(st.printers.unwrap_or(false));
    printers.set_sensitive(st.printers.is_some());
    if st.printers.is_none() { printers.set_tooltip_text(Some("Printing (CUPS) is not installed.")); }
    grid.attach(&printers, 1, 1, 1, 1);
    v.pack_start(&grid, false, false, 0);
    { let me = cp.clone(); v.pack_start(&link("How do I share additional libraries? How do I exclude files and folders?", move || message(Some(&me.win.clone().upcast()), gtk::MessageType::Info, "HomeGroup", "Sharing other folders",
        "Use \"Share a folder...\" below to share any folder with Read or Read/Write access. Everything inside a shared folder is shared; move files you don't want to share to a folder that isn't shared.")), false, false, 0); }

    v.pack_start(&section("Share media with devices"), false, false, 0);
    let media = vbox(4);
    media.set_margin_start(16);
    let stream = gtk::CheckButton::with_mnemonic("Stream my _pictures, music, and videos to all devices on my home network");
    stream.set_active(st.streaming.unwrap_or(false));
    stream.set_sensitive(st.streaming.is_some());
    media.pack_start(&stream, false, false, 0);
    if st.streaming.is_none() {
        media.pack_start(&grey("Media streaming needs the Rygel media server (install the rygel package)."), false, false, 0);
    } else {
        let opts = link("Choose media streaming options...", || {
            if util::which("rygel-preferences") { util::spawn(&["rygel-preferences"]); }
            else { util::open_uri(&format!("file://{}", glib::user_config_dir().join("rygel.conf").display())); }
        });
        opts.set_margin_start(22);
        media.pack_start(&opts, false, false, 0);
    }
    media.pack_start(&label("Note: Shared media are not secure. Anyone connected to your network can receive your shared media."), false, false, 4);
    v.pack_start(&media, false, false, 0);

    v.pack_start(&section("Shared folders"), false, false, 0);
    let (tv, store) = table(5, &["Share name", "Folder", "Permission", "Guests"]);
    expand_column(&tv, 1);
    if let Some(c) = tv.column(3) { c.set_min_width(130); }
    for s in &st.shares {
        store.insert_with_values(None, &[(0, &s.name), (1, &s.path), (2, &(if s.write { "Read/Write" } else { "Read" }).to_string()), (3, &(if s.guest { "Allowed" } else { "Password required" }).to_string()), (4, &s.name)]);
    }
    let sw = scrolled(&tv, 110);
    sw.set_margin_start(16);
    v.pack_start(&sw, false, false, 0);
    let br = hbox(6);
    br.set_margin_start(16);
    let add = gtk::Button::with_mnemonic("Share a _folder...");
    let props = gtk::Button::with_mnemonic("_Properties...");
    let stop = gtk::Button::with_mnemonic("S_top sharing");
    for b in [&add, &props, &stop] { br.pack_start(b, false, false, 0); }
    v.pack_start(&br, false, false, 0);
    {
        let (me, hd) = (cp.clone(), holder.clone());
        add.connect_clicked(move |_| {
            let w: gtk::Window = me.win.clone().upcast();
            let fc = gtk::FileChooserNative::new(Some("Choose a folder to share"), Some(&w), gtk::FileChooserAction::SelectFolder, Some("Share"), Some("Cancel"));
            if fc.run() != gtk::ResponseType::Accept { return; }
            let Some(p) = fc.filename() else { return };
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Share".into());
            share_dialog(&me, &hd, &name, &p.to_string_lossy(), false, false, true);
        });
    }
    {
        let (me, hd, tv, shares) = (cp.clone(), holder.clone(), tv.clone(), st.shares.clone());
        props.connect_clicked(move |_| {
            let Some(n) = selected(&tv, 4) else { return };
            if let Some(s) = shares.iter().find(|s| s.name == n) { share_dialog(&me, &hd, &s.name, &s.path, s.write, s.guest, false); }
        });
    }
    {
        let (me, hd, tv) = (cp.clone(), holder.clone(), tv.clone());
        stop.connect_clicked(move |_| {
            let Some(n) = selected(&tv, 4) else { return };
            let w: gtk::Window = me.win.clone().upcast();
            if !ask(Some(&w), gtk::MessageType::Question, "File Sharing", &format!("Stop sharing \"{n}\"?"), "Other computers will no longer be able to open this folder. The files in it are not deleted.", "_Stop sharing", "Cancel") { return; }
            let (me, hd) = (me.clone(), hd.clone());
            change(sv(&["net", "usershare", "delete", &n]), None, move |o| {
                if !o.ok { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "File Sharing", "The folder could not be unshared.", &o.text()); }
                reload(&me, &hd);
            });
        });
    }

    v.pack_start(&section("Other homegroup actions"), false, false, 0);
    let oa = vbox(2);
    oa.set_margin_start(16);
    { let (me, nb) = (cp.clone(), st.netbios.clone()); oa.pack_start(&link("View or print the homegroup password", move || view_password(&me.win.clone().upcast(), &nb)), false, false, 0); }
    { let me = cp.clone(); oa.pack_start(&link("Change the password...", move || change_password(&me.win.clone().upcast())), false, false, 0); }
    {
        let (me, hd, shares) = (cp.clone(), holder.clone(), st.shares.clone());
        oa.pack_start(&link("Leave the homegroup...", move || {
            let w: gtk::Window = me.win.clone().upcast();
            if !ask(Some(&w), gtk::MessageType::Warning, "Leave the Homegroup", "If you leave the homegroup, other computers can't open the folders you share. Do you want to leave?",
                &format!("{} shared folder(s) will stop being shared. Your files are not deleted.", shares.len()), "_Leave the homegroup", "Cancel") { return; }
            let cmds: Vec<Vec<String>> = shares.iter().map(|s| sv(&["net", "usershare", "delete", &s.name])).collect();
            let (me, hd) = (me.clone(), hd.clone());
            background(move || { for c in &cmds { change_blocking(c, None); } }, move |_| reload(&me, &hd));
        }), false, false, 0);
    }
    other_links(cp, &oa);
    v.pack_start(&oa, false, false, 0);

    let save = gtk::Button::with_mnemonic("_Save changes");
    let cancel = gtk::Button::with_label("Cancel");
    { let me = cp.clone(); cancel.connect_clicked(move |_| me.open("cat:network")); }
    {
        let (me, hd) = (cp.clone(), holder.clone());
        let (printers, was_printers, stream, was_stream) = (printers.clone(), st.printers.unwrap_or(false), stream.clone(), st.streaming.unwrap_or(false));
        save.connect_clicked(move |_| {
            let mut cmds: Vec<Vec<String>> = vec![];
            if stream.is_sensitive() && stream.is_active() != was_stream { cmds.extend(set_streaming(stream.is_active())); }
            if printers.is_sensitive() && printers.is_active() != was_printers {
                cmds.push(sv(&["pkexec", "sh", "-c", SHARE_PRINTERS, "sh", if printers.is_active() { "on" } else { "off" }]));
            }
            for (c, name, path, shared) in &checks {
                match (c.is_active(), shared) {
                    (true, None) => cmds.push(sv(&["net", "usershare", "add", name, path, "", "Everyone:R", "guest_ok=n"])),
                    (false, Some(s)) => cmds.push(sv(&["net", "usershare", "delete", &s.name])),
                    _ => {}
                }
            }
            let (me, hd) = (me.clone(), hd.clone());
            background(move || cmds.iter().map(|c| change_blocking(c, None)).filter(|o| !o.ok).map(|o| o.text()).collect::<Vec<_>>(), move |errs| {
                if !errs.is_empty() { message(Some(&me.win.clone().upcast()), gtk::MessageType::Error, "HomeGroup", "Some libraries could not be shared.", &errs.join("\n")); }
                reload(&me, &hd);
            });
        });
    }
    let row = button_row(&[&save, &cancel]);
    row.set_margin_top(14);
    v.pack_start(&row, false, false, 0);
    v.upcast()
}

/// Win7's "Share with ▸ Homegroup (Read) / Homegroup (Read/Write)" as a dialog.
fn share_dialog(cp: &Rc<Cp>, holder: &gtk::Box, name: &str, path: &str, write: bool, guest: bool, new: bool) {
    let w: gtk::Window = cp.win.clone().upcast();
    let d = gtk::Dialog::with_buttons(Some("File Sharing"), Some(&w), gtk::DialogFlags::MODAL, &[(if new { "_Share" } else { "OK" }, gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(420, -1);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    c.pack_start(&instr("Choose how other computers can use this folder"), false, false, 0);
    c.pack_start(&crate::kv_grid(&[("Folder:", path.to_string())]), false, false, 0);
    let row = hbox(8);
    row.pack_start(&gtk::Label::new(Some("Share name:")), false, false, 0);
    let e = gtk::Entry::new();
    e.set_text(name);
    e.set_sensitive(new);
    row.pack_start(&e, true, true, 0);
    c.pack_start(&row, false, false, 0);
    let r = gtk::RadioButton::with_label("Homegroup (Read)");
    let rw = gtk::RadioButton::with_label_from_widget(&r, "Homegroup (Read/Write)");
    rw.set_active(write);
    c.pack_start(&r, false, false, 0);
    c.pack_start(&rw, false, false, 0);
    let g = gtk::CheckButton::with_label("Allow guests to open the folder without a password");
    g.set_active(guest);
    c.pack_start(&g, false, false, 0);
    c.pack_start(&grey("Read/Write lets people on other computers change and delete files in this folder."), false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    let resp = d.run();
    let (n, wr, gu) = (e.text().trim().to_string(), rw.is_active(), g.is_active());
    d.close();
    if resp != gtk::ResponseType::Ok || n.is_empty() { return; }
    let acl = if wr { "Everyone:F" } else { "Everyone:R" };
    let argv = sv(&["net", "usershare", "add", &n, path, "", acl, if gu { "guest_ok=y" } else { "guest_ok=n" }]);
    let (me, hd) = (cp.clone(), holder.clone());
    let p = path.to_string();
    change(argv, None, move |o| {
        let w: gtk::Window = me.win.clone().upcast();
        if !o.ok { message(Some(&w), gtk::MessageType::Error, "File Sharing", "The folder could not be shared.", &o.text()); }
        else if gu {
            // guests come in as "nobody": they need to be able to read the folder
            let m = std::fs::metadata(&p).map(|m| { use std::os::unix::fs::PermissionsExt; m.permissions().mode() }).unwrap_or(0);
            if m & 0o005 != 0o005 { message(Some(&w), gtk::MessageType::Warning, "File Sharing", "Guests may not be able to open this folder.", "Its permissions don't allow other users to read it. Use a password instead, or allow others to read the folder in its Properties."); }
        }
        reload(&me, &hd);
    });
}

/// "View and print your homegroup password": Samba keeps only a hash of it, so this shows how
/// other computers connect instead.
fn view_password(parent: &gtk::Window, netbios: &str) {
    let d = gtk::Dialog::with_buttons(Some("View and print your homegroup password"), Some(parent), gtk::DialogFlags::MODAL, &[("_Print this page", gtk::ResponseType::Apply), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(460, -1);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(10);
    c.pack_start(&instr("View and print your homegroup password"), false, false, 0);
    c.pack_start(&label("Use this information to connect other computers to your homegroup:"), false, false, 0);
    let card = vbox(6);
    card.style_context().add_class("xt-card");
    card.set_border_width(12);
    card.pack_start(&crate::kv_grid(&[("Computer:", format!("\\\\{netbios}")), ("User name:", user()), ("Password:", "The password you chose for the homegroup".into())]), false, false, 0);
    c.pack_start(&card, false, false, 0);
    c.pack_start(&grey("The password is stored encrypted and can't be shown. If you forgot it, change it with \"Change the password...\"."), false, false, 0);
    c.pack_start(&label("On a Windows computer, open Windows Explorer and type the computer name above into the address bar. On a Mac, choose Go ▸ Connect to Server and type smb:// and the computer name."), false, false, 0);
    d.show_all();
    let r = d.run();
    d.close();
    if r == gtk::ResponseType::Apply {
        let text = format!("Homegroup\n\nComputer: \\\\{netbios}\nUser name: {}\n", user());
        let f = std::env::temp_dir().join("homegroup.txt");
        if std::fs::write(&f, text).is_ok() { util::spawn(&["lp", &f.to_string_lossy()]); }
    }
}

fn change_password(parent: &gtk::Window) {
    let d = gtk::Dialog::with_buttons(Some("Change Your Homegroup Password"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.set_border_width(12);
    c.set_spacing(8);
    c.pack_start(&instr("Type a new password for your homegroup"), false, false, 0);
    c.pack_start(&label(&format!("Other computers will need user name {} and this password to open your shared folders.", user())), false, false, 0);
    let (a, b) = (gtk::Entry::new(), gtk::Entry::new());
    for (l, e) in [("New password:", &a), ("Confirm password:", &b)] {
        e.set_visibility(false);
        let h = hbox(8);
        let lab = gtk::Label::new(Some(l));
        lab.set_width_chars(18);
        lab.set_xalign(0.0);
        h.pack_start(&lab, false, false, 0);
        h.pack_start(e, true, true, 0);
        c.pack_start(&h, false, false, 0);
    }
    d.show_all();
    let r = d.run();
    let (p1, p2) = (a.text().to_string(), b.text().to_string());
    d.close();
    if r != gtk::ResponseType::Ok || p1.is_empty() { return; }
    if p1 != p2 { message(Some(parent), gtk::MessageType::Error, "HomeGroup", "The passwords don't match.", ""); return; }
    let parent = parent.clone();
    change(sv(&["pkexec", "smbpasswd", "-a", "-s", &user()]), Some(format!("{p1}\n{p1}\n")), move |o| {
        if o.ok { message(Some(&parent), gtk::MessageType::Info, "HomeGroup", "The homegroup password was changed.", ""); }
        else if !o.cancelled() { message(Some(&parent), gtk::MessageType::Error, "HomeGroup", "The password could not be changed.", &o.text()); }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn usershare_info() {
        let v = super::parse_info("[Pictures]\npath=/home/u/Pictures\ncomment=\nusershare_acl=Everyone:R,\nguest_ok=n\n[Drop]\npath=/home/u/Drop\ncomment=x\nusershare_acl=Everyone:F,\nguest_ok=y\n");
        assert_eq!(v.len(), 2);
        assert!(!v[0].write && !v[0].guest);
        assert!(v[1].write && v[1].guest);
        assert_eq!(v[1].path, "/home/u/Drop");
    }
}
