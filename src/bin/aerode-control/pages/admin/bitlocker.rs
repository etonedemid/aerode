//! BitLocker Drive Encryption on LUKS: every volume with its state (On / Locked / Off), Unlock
//! (password → udisksctl unlock + mount), Lock (unmount + udisksctl lock), Manage BitLocker
//! (change/add password, back up the header = the recovery key, details). Encrypting or
//! decrypting a drive in place is left to the partition tools, with a clear explanation.
use gtk::prelude::*;
use gtk::glib;
use std::rc::Rc;

use super::super::super::{link, section, text, title, Cp};
use super::diskmgmt::{self, Disk, Vol};
use super::*;

const SYSTEM: [&str; 6] = ["/", "/boot", "/boot/efi", "/efi", "/home", "/usr"];

fn mounts(v: &Vol) -> Vec<String> { v.mounts.iter().cloned().chain(v.inner.iter().flat_map(|i| i.mounts.clone())).filter(|m| !m.is_empty() && !m.starts_with('[')).collect() }

pub fn page(cp: &Rc<Cp>) -> gtk::Widget {
    let holder = vbox(0);
    holder.style_context().add_class("adm");
    see_also_fn(cp, vec![("TPM Administration", Box::new(tpm_info)), ("Disk Management", Box::new(|| spawn_tool("diskmgmt")))]);
    holder.pack_start(&label("Loading…", "dim"), false, false, 0);
    let (cp2, h) = (cp.clone(), holder.clone());
    bg(diskmgmt::load, move |disks| fill(&cp2, &h, disks));
    holder.upcast()
}

fn fill(cp: &Rc<Cp>, holder: &gtk::Box, disks: Vec<Disk>) {
    for c in holder.children() { holder.remove(&c); }
    let v = vbox(8);
    v.pack_start(&title("Help protect your files and folders by encrypting your drives"), false, false, 0);
    v.pack_start(&text("BitLocker Drive Encryption helps prevent unauthorized access to any files stored on the drives shown below. You are able to use the computer normally, but unauthorized users cannot read or use your files."), false, false, 0);
    v.pack_start(&link("What should I know about BitLocker before I turn it on?", || { util::open_uri("https://wiki.archlinux.org/title/Dm-crypt"); }), false, false, 0);
    let (mut fixed, mut removable): (Vec<(Vol, bool)>, Vec<(Vol, bool)>) = (vec![], vec![]);
    for d in disks.iter().filter(|d| !d.rom) {
        let vols: Vec<Vol> = if let Some(w) = &d.whole { vec![w.clone()] } else { d.parts.iter().filter(|p| !p.fstype.is_empty() && p.fstype != "swap" && !matches!(p.parttype.as_str(), "0x5" | "0xf" | "0x85")).cloned().collect() };
        for vol in vols { if d.removable { removable.push((vol, true)) } else { fixed.push((vol, false)) } }
    }
    // the system drive first, like C:
    fixed.sort_by_key(|(v, _)| !mounts(v).iter().any(|m| m == "/"));
    v.pack_start(&section("BitLocker Drive Encryption - Hard Disk Drives"), false, false, 0);
    let g1 = vbox(10);
    g1.set_margin_start(14);
    for (vol, rm) in &fixed { g1.pack_start(&drive_row(cp, holder, vol, *rm), false, false, 0); }
    if fixed.is_empty() { g1.pack_start(&label("No hard disk volumes were found.", "dim"), false, false, 0); }
    v.pack_start(&g1, false, false, 0);
    v.pack_start(&section("BitLocker Drive Encryption - BitLocker To Go"), false, false, 0);
    let g2 = vbox(10);
    g2.set_margin_start(14);
    for (vol, rm) in &removable { g2.pack_start(&drive_row(cp, holder, vol, *rm), false, false, 0); }
    if removable.is_empty() { g2.pack_start(&label("Insert a removable drive to use BitLocker To Go.", ""), false, false, 0); }
    v.pack_start(&g2, false, false, 0);
    holder.pack_start(&v, true, true, 0);
    holder.show_all();
}

fn tpm_info() {
    let ver = std::fs::read_to_string("/sys/class/tpm/tpm0/tpm_version_major").map(|s| s.trim().to_string()).ok();
    match ver {
        Some(v) => inform(None, "TPM Management on Local Computer", &format!("The TPM is ready for use (TPM {v}.0)."), "A LUKS2 volume can be unlocked by the TPM at startup: systemd-cryptenroll --tpm2-device=auto /dev/…"),
        None => inform(None, "TPM Management on Local Computer", "Compatible TPM cannot be found", "Compatible Trusted Platform Module (TPM) cannot be found on this computer. Verify that this computer has a TPM 1.2 or later and that it is turned on in the BIOS."),
    }
}

fn drive_row(cp: &Rc<Cp>, holder: &gtk::Box, v: &Vol, removable: bool) -> gtk::Widget {
    let luks = v.fstype == "crypto_LUKS";
    let unlocked = luks && !v.inner.is_empty();
    let h = hbox(12);
    let icon_names: &[&str] = match (luks, unlocked, removable) { (true, true, _) => &["aerode-bitlocker-unlocked", "drive-harddisk"], (true, false, _) => &["aerode-bitlocker-locked", "changes-prevent", "drive-harddisk"], (_, _, true) => &["drive-removable-media", "drive-harddisk"], _ if mounts(v).iter().any(|m| m == "/") => &["drive-harddisk-system", "drive-harddisk"], _ => &["drive-harddisk"] };
    let icon = img(icon_names, 32);
    h.pack_start(&icon, false, false, 0);
    let col = vbox(2);
    let title = { let t = diskmgmt::vol_title(v); if t.is_empty() { format!("{} ({})", if v.label.is_empty() { "Local Disk" } else { &v.label }, v.name) } else { t } };
    col.pack_start(&bold(&title), false, false, 0);
    let state = if unlocked { "On" } else if luks { "Locked" } else { "Off" };
    col.pack_start(&label(state, if luks { "ok-text" } else { "" }), false, false, 0);
    h.pack_start(&col, true, true, 0);
    let links = vbox(2);
    links.set_valign(gtk::Align::Center);
    let path = v.path.clone();
    let reload: Rc<dyn Fn()> = { let (cp, holder) = (cp.clone(), holder.clone()); Rc::new(move || { let (cp, h) = (cp.clone(), holder.clone()); bg(diskmgmt::load, move |d| fill(&cp, &h, d)); }) };
    if luks && !unlocked {
        let (p, r, t) = (path.clone(), reload.clone(), title.clone());
        links.pack_start(&link("Unlock Drive", move || unlock(&p, &t, r.clone())), false, false, 0);
    }
    if unlocked {
        let sys = mounts(v).iter().any(|m| SYSTEM.contains(&m.as_str()));
        if !sys {
            let (vol, r) = (v.clone(), reload.clone());
            links.pack_start(&link("Lock Drive", move || lock(&vol, r.clone())), false, false, 0);
        }
        let (p, t) = (path.clone(), title.clone());
        links.pack_start(&link("Manage BitLocker", move || manage(&p, &t)), false, false, 0);
        let t = title.clone();
        links.pack_start(&shield_link("Turn Off BitLocker", move || turn_off(&t)), false, false, 0);
    }
    if !luks {
        let (p, t) = (path.clone(), title.clone());
        links.pack_start(&shield_link("Turn On BitLocker", move || turn_on(&p, &t, removable)), false, false, 0);
    }
    h.pack_end(&links, false, false, 0);
    h.upcast()
}

/// Win7's unlock dialog → udisksctl unlock (password on stdin) and mount.
fn unlock(dev: &str, title: &str, reload: Rc<dyn Fn()>) {
    let d = gtk::Dialog::with_buttons(Some(&format!("BitLocker Drive Encryption ({title})")), None::<&gtk::Window>, gtk::DialogFlags::MODAL, &[("_Unlock", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_size(440, -1);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(8);
    let h = hbox(10);
    h.pack_start(&img(&["aerode-bitlocker-locked", "changes-prevent"], 32), false, false, 0);
    let t = gtk::Label::new(None);
    t.set_markup("<span size='large' foreground='#1e3287'>This drive is protected by BitLocker Drive Encryption</span>");
    t.set_line_wrap(true);
    t.set_xalign(0.0);
    h.pack_start(&t, true, true, 0);
    c.pack_start(&h, false, false, 0);
    c.pack_start(&label("Type your password to unlock this drive", ""), false, false, 0);
    let pw = gtk::Entry::new();
    pw.set_visibility(false);
    pw.set_activates_default(true);
    c.pack_start(&pw, false, false, 0);
    let show = gtk::CheckButton::with_label("Show password characters as I type them");
    { let pw = pw.clone(); show.connect_toggled(move |s| pw.set_visibility(s.is_active())); }
    c.pack_start(&show, false, false, 0);
    let auto = gtk::CheckButton::with_label("Automatically unlock on this computer from now on");
    auto.set_sensitive(false);
    c.pack_start(&auto, false, false, 0);
    c.pack_start(&link("I forgot my password", || { util::open_uri("https://wiki.archlinux.org/title/Dm-crypt/Device_encryption#Backup_and_restore"); }), false, false, 0);
    let err = label("", "bad-text");
    c.pack_start(&err, false, false, 0);
    d.set_default_response(gtk::ResponseType::Ok);
    d.show_all();
    let dev = dev.to_string();
    d.connect_response(move |d, r| {
        if r != gtk::ResponseType::Ok { d.close(); return; }
        let password = pw.text().to_string();
        let (d2, err2, reload2, dev2) = (d.clone(), err.clone(), reload.clone(), dev.clone());
        d.set_sensitive(false);
        err.set_text("Unlocking…");
        bg(move || {
            use std::io::Write;
            let child = std::process::Command::new("udisksctl").args(["unlock", "-b", &dev2, "--key-file", "/dev/stdin"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn();
            let Ok(mut child) = child else { return (false, "udisksctl was not found (install udisks2).".to_string()) };
            if let Some(mut i) = child.stdin.take() { let _ = i.write_all(password.as_bytes()); }
            let o = child.wait_with_output();
            let Ok(o) = o else { return (false, "udisksctl failed".into()) };
            let out = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
            if !o.status.success() { return (false, out); }
            // "Unlocked /dev/sdb1 as /dev/dm-3."
            if let Some(mapped) = out.split(" as ").nth(1).map(|s| s.trim().trim_end_matches('.').to_string()) { let _ = run(&["udisksctl", "mount", "-b", &mapped]); }
            (true, out)
        }, move |(ok, out)| {
            d2.set_sensitive(true);
            if ok { d2.close(); reload2(); }
            else { err2.set_text(if out.contains("passphrase") || out.contains("Failed to activate") || out.contains("No key available") { "The password is incorrect. Try again." } else { out.trim() }); }
        });
    });
}

fn lock(v: &Vol, reload: Rc<dyn Fn()>) {
    let inner: Vec<String> = v.inner.iter().filter(|i| !mounts(&Vol { mounts: i.mounts.clone(), ..Default::default() }).is_empty()).map(|i| i.path.clone()).collect();
    let dev = v.path.clone();
    bg(move || {
        for i in &inner { let (ok, out) = run(&["udisksctl", "unmount", "-b", i]); if !ok { return (false, out); } }
        run(&["udisksctl", "lock", "-b", &dev])
    }, move |(ok, out)| { if report("The drive could not be locked. Close any files or programs that use it.", ok, &out) { reload(); } });
}

fn manage(dev: &str, title: &str) {
    let d = gtk::Dialog::with_buttons(Some(&format!("BitLocker Drive Encryption ({title})")), None::<&gtk::Window>, gtk::DialogFlags::MODAL, &[("Close", gtk::ResponseType::Close)]);
    d.set_default_size(460, -1);
    let c = d.content_area();
    c.set_border_width(14);
    c.set_spacing(8);
    let t = gtk::Label::new(None);
    t.set_markup("<span size='large' foreground='#1e3287'>Select options to manage BitLocker</span>");
    t.set_xalign(0.0);
    c.pack_start(&t, false, false, 0);
    let dev = dev.to_string();
    let item = |text: &str, sub: &str, f: Box<dyn Fn()>| {
        let b = gtk::Button::new();
        b.set_relief(gtk::ReliefStyle::None);
        let v = vbox(1);
        v.pack_start(&bold(&format!("➜  {text}")), false, false, 0);
        if !sub.is_empty() { let l = label(sub, "dim"); l.set_margin_start(22); v.pack_start(&l, false, false, 0); }
        b.add(&v);
        b.connect_clicked(move |_| f());
        c.pack_start(&b, false, false, 0);
    };
    { let d = dev.clone(); item("Change password to unlock the drive", "", Box::new(move || { terminal(&format!("sudo cryptsetup luksChangeKey {}", q(&d))); })); }
    { let d = dev.clone(); item("Add a password", "Another password (key slot) that also unlocks the drive", Box::new(move || { terminal(&format!("sudo cryptsetup luksAddKey {}", q(&d))); })); }
    { let d = dev.clone(); item("Save or print recovery key again", "Save a copy of the encryption header – with it and a password the drive can be recovered if the header gets damaged.", Box::new(move || {
        let fc = gtk::FileChooserNative::new(Some("Save BitLocker recovery key as"), None::<&gtk::Window>, gtk::FileChooserAction::Save, Some("_Save"), Some("_Cancel"));
        fc.set_current_name("BitLocker Recovery Key.img");
        if fc.run() != gtk::ResponseType::Accept { return; }
        let Some(p) = fc.filename() else { return };
        let p = p.to_string_lossy().to_string();
        let user = glib::user_name().to_string_lossy().into_owned();
        elevate_sh(&format!("cryptsetup luksHeaderBackup {} --header-backup-file {} && chown {} {}", q(&d), q(&p), q(&user), q(&p)), |ok, out| { if report("The recovery key could not be saved.", ok, &out) { inform(None, "BitLocker Drive Encryption", "Your recovery key was saved.", "Keep it somewhere safe – not on the encrypted drive."); } });
    })); }
    { let d = dev.clone(); item("Show encryption details", "", Box::new(move || {
        elevate(&["cryptsetup", "luksDump", &d], |ok, out| {
            if !report("The details could not be read.", ok, &out) { return; }
            let dl = gtk::Dialog::with_buttons(Some("BitLocker details"), None::<&gtk::Window>, gtk::DialogFlags::empty(), &[("Close", gtk::ResponseType::Close)]);
            dl.set_default_size(560, 420);
            let tv = text_box(&out);
            tv.set_monospace(true);
            dl.content_area().pack_start(&scrolled(&tv), true, true, 0);
            dl.connect_response(|d, _| d.close());
            dl.show_all();
        });
    })); }
    d.show_all();
    d.run();
    d.close();
}

fn turn_on(dev: &str, title: &str, removable: bool) {
    let d = gtk::MessageDialog::new(None::<&gtk::Window>, gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::None, &format!("Encrypt {title}"));
    d.set_title("BitLocker Drive Encryption");
    d.set_secondary_text(Some(&format!("{dev} is not encrypted.\n\nTo encrypt {} drive, back up its files, then format it as an encrypted (LUKS) volume in Disks or KDE Partition Manager and copy the files back. Encrypting a drive in place (cryptsetup reencrypt) is possible but risky without a backup.",
        if removable { "a removable" } else { "a" })));
    d.add_button("Open partition editor", gtk::ResponseType::Ok);
    d.add_button("Learn more", gtk::ResponseType::Help);
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    let r = d.run();
    d.close();
    match r {
        gtk::ResponseType::Ok => { if !util::spawn(&["gnome-disks", "--block-device", dev]) { super::diskmgmt::partition_tool(dev); } }
        gtk::ResponseType::Help => { util::open_uri("https://wiki.archlinux.org/title/Dm-crypt/Device_encryption#Encrypt_an_existing_unencrypted_file_system"); }
        _ => {}
    }
}

fn turn_off(title: &str) {
    inform(None, "BitLocker Drive Encryption", &format!("Decrypting {title}"), "To turn off encryption, copy the files to another drive, format this one without encryption and copy them back – or decrypt it in place with cryptsetup reencrypt --decrypt (LUKS2; see the Arch Wiki). Back up first: an interrupted decryption can lose data.");
}
