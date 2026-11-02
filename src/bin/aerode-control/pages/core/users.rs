//! User Accounts: "Make changes to your user account" with the Win7 task list, Change/Create/
//! Remove your password (passwd driven through a pseudo terminal, so it checks the current
//! password itself), Change your picture (the Win7 user tiles aerode-extract saved, Browse…;
//! written to ~/.face(.icon) and AccountsService), Change your account name (chfn), Change your
//! account type (the wheel group), Manage another account (create with useradd -m, change,
//! delete keeping or deleting the files), User Account Control Settings (the four-level slider:
//! stored in config.json, read by aerode-polkit; "Never notify" installs a polkit rule) and
//! Manage your credentials. Changes to the system go through pkexec.
use super::*;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Account {
    pub user: String,
    pub real: String,
    pub uid: u32,
    pub home: String,
    pub admin: bool,
    /// Some(true) password set, Some(false) none, None unknown
    pub password: Option<bool>,
    pub picture: Option<PathBuf>,
}

impl Account {
    pub fn shown(&self) -> &str { if self.real.trim().is_empty() { &self.user } else { &self.real } }
    pub fn kind(&self) -> &'static str { if self.admin { "Administrator" } else { "Standard user" } }
}

fn me() -> String { glib::user_name().to_string_lossy().into_owned() }

fn admins_group() -> Vec<String> {
    std::fs::read_to_string("/etc/group").unwrap_or_default().lines().filter(|l| l.starts_with("wheel:") || l.starts_with("sudo:"))
        .flat_map(|l| l.rsplit(':').next().unwrap_or("").split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).filter(|s| !s.is_empty()).collect()
}

/// AccountsService: (PasswordMode, IconFile) of a user, when the daemon runs.
fn accounts_service(uid: u32) -> (Option<u32>, Option<PathBuf>) {
    let path = format!("/org/freedesktop/Accounts/User{uid}");
    let get = |p: &str| sh(&["busctl", "--system", "get-property", "org.freedesktop.Accounts", &path, "org.freedesktop.Accounts.User", p]);
    let mode = get("PasswordMode").split_whitespace().nth(1).and_then(|v| v.parse().ok());
    let icon = get("IconFile").split('"').nth(1).filter(|s| !s.is_empty()).map(PathBuf::from).filter(|p| p.exists());
    (mode, icon)
}

fn picture_of(user: &str, home: &str, acc_icon: Option<PathBuf>) -> Option<PathBuf> {
    let mine = user == me();
    let face = Path::new(home).join(".face");
    if mine && face.exists() { return Some(face); }
    if let Some(i) = acc_icon { return Some(i); }
    let a = Path::new("/var/lib/AccountsService/icons").join(user);
    if a.exists() { return Some(a); }
    for f in [face, Path::new(home).join(".face.icon")] { if std::fs::File::open(&f).is_ok() { return Some(f); } }
    None
}

/// Every person's account (uid 1000…59999 with a login shell), worker thread.
pub fn accounts() -> Vec<Account> {
    let admins = admins_group();
    let mut v = vec![];
    let my = me();
    for l in std::fs::read_to_string("/etc/passwd").unwrap_or_default().lines() {
        let f: Vec<&str> = l.split(':').collect();
        if f.len() < 7 { continue; }
        let uid: u32 = f[2].parse().unwrap_or(0);
        let shell = f[6];
        if f[0] != my && (!(1000..60000).contains(&uid) || shell.ends_with("nologin") || shell.ends_with("false")) { continue; }
        let (mode, icon) = accounts_service(uid);
        let mut password = mode.map(|m| m != 2);
        if f[0] == my && password.is_none() {
            // our own status: "user P 2024-01-01 …" (P = password, NP = none, L = locked)
            let (_, o) = run(&["env", "LC_ALL=C", "passwd", "-S"]);
            password = match o.split_whitespace().nth(1) { Some("P") | Some("PS") => Some(true), Some("NP") => Some(false), _ => None };
        }
        v.push(Account { user: f[0].into(), real: f[4].split(',').next().unwrap_or("").to_string(), uid, home: f[5].into(),
            admin: admins.iter().any(|a| a == f[0]) || uid == 0, password, picture: picture_of(f[0], f[5], icon) });
    }
    v
}

fn current_account() -> Account {
    let user = me();
    let home = config::home().to_string_lossy().into_owned();
    let uid = unsafe { libc::getuid() };
    Account { user: user.clone(), real: glib::real_name().to_string_lossy().replace("Unknown", ""), uid, home: home.clone(),
        admin: admins_group().contains(&user) || uid == 0, password: None, picture: picture_of(&user, &home, None) }
}

// --------------------------------------------------------------------------- page routing

impl Cp {
    pub(crate) fn page_users(self: &Rc<Self>) -> gtk::Widget {
        css();
        let holder = vbox(0);
        let subp = sub(self, "users").unwrap_or_default();
        let (h, me2, sp) = (holder.clone(), self.clone(), subp.clone());
        // what we know at once; the rest (password state, AccountsService) follows from a thread
        let fill = move |all: Vec<Account>| {
            let mine = all.iter().find(|a| a.user == me()).cloned().unwrap_or_else(current_account);
            let w = route(&me2, &sp, &mine, &all);
            for c in h.children() { h.remove(&c); }
            h.pack_start(&w, true, true, 0);
            h.show_all();
        };
        if subp.is_empty() { side_common(self); }
        if subp.is_empty() || subp == "uac" {
            // pages that don't need the list: no flicker
            fill(vec![current_account()]);
            if subp.is_empty() {
                let f = fill.clone();
                bg(accounts, move |all| f(all));
            }
        } else {
            holder.pack_start(&label("Please wait...", "cp-hint"), false, false, 0);
            bg(accounts, move |all| fill(all));
        }
        holder.upcast()
    }
}

/// The page for `users/<sub>` (account pages of others are `acct/<user>[/<what>]`).
fn route(cp: &Rc<Cp>, subp: &str, mine: &Account, all: &[Account]) -> gtk::Widget {
    let parts: Vec<&str> = subp.split('/').collect();
    let (target, what): (Account, &str) = match parts.as_slice() {
        ["acct", u, rest @ ..] => match all.iter().find(|a| a.user == *u) {
            Some(a) => (a.clone(), rest.first().copied().unwrap_or("")),
            None => return label("That account does not exist anymore.", "").upcast(),
        },
        [w, ..] => (mine.clone(), *w),
        [] => (mine.clone(), ""),
    };
    let own = target.user == mine.user;
    match what {
        "password" => password_page(cp, &target, own),
        "removepw" => remove_password_page(cp, &target, own),
        "picture" => picture_page(cp, &target, own),
        "name" => name_page(cp, &target, own),
        "type" => type_page(cp, &target, own, all),
        "manage" => manage_page(cp, all, mine),
        "create" => create_page(cp),
        "delete" => delete_page(cp, &target),
        "uac" => uac_page(cp),
        _ if !own => account_page(cp, &target),
        _ => main_page(cp, mine),
    }
}

/// Page id of an action for `a` (own account: users/<what>, others: users/acct/<user>/<what>).
fn pid(a: &Account, what: &str) -> String {
    if a.user == me() { format!("users/{what}") } else if what.is_empty() { format!("users/acct/{}", a.user) } else { format!("users/acct/{}/{what}", a.user) }
}

fn side_common(cp: &Rc<Cp>) {
    { let c = cp.clone(); side_action(cp, "Manage your credentials", move || go(&c, "credentials")); }
    { let c = cp.clone(); side_action(cp, "Manage another account", move || go(&c, "users/manage")); }
}

/// Picture, name, account type and "Password protected" (Win7 user tile block).
fn tile_block(a: &Account, big: bool) -> gtk::Box {
    let h = hbox(10);
    h.set_valign(gtk::Align::Start);
    h.pack_start(&user_tile(a.picture.as_deref(), if big { 64 } else { 48 }), false, false, 0);
    let v = vbox(1);
    v.set_valign(gtk::Align::Center);
    v.pack_start(&label(a.shown(), "ua-name"), false, false, 0);
    v.pack_start(&label(a.kind(), "dim"), false, false, 0);
    if a.password == Some(true) { v.pack_start(&label("Password protected", "dim"), false, false, 0); }
    h.pack_start(&v, false, false, 0);
    h
}

// ---------------------------------------------------------------------- main / others

fn main_page(cp: &Rc<Cp>, a: &Account) -> gtk::Widget {
    let v = vbox(6);
    v.pack_start(&title("Make changes to your user account"), false, false, 0);
    let row = hbox(40);
    let links = vbox(5);
    let nav = |t: &str, page: String| { let c = cp.clone(); link(t, move || go(&c, &page)) };
    if a.password == Some(false) { links.pack_start(&nav("Create a password for your account", pid(a, "password")), false, false, 0); }
    else {
        links.pack_start(&nav("Change your password", pid(a, "password")), false, false, 0);
        links.pack_start(&nav("Remove your password", pid(a, "removepw")), false, false, 0);
    }
    links.pack_start(&nav("Change your picture", pid(a, "picture")), false, false, 0);
    links.pack_start(&nav("Change your account name", pid(a, "name")), false, false, 0);
    for (t, p) in [("Change your account type", "type"), ("Manage another account", "manage"), ("Change User Account Control settings", "uac")] {
        let c = cp.clone();
        let page = pid(a, p);
        links.pack_start(&shield_link(t, move || go(&c, &page)), false, false, 0);
    }
    row.pack_start(&links, false, false, 0);
    row.pack_start(&tile_block(a, true), false, false, 0);
    v.pack_start(&row, false, false, 0);
    v.upcast()
}

/// "Change an Account": what an administrator can do to someone else's account.
fn account_page(cp: &Rc<Cp>, a: &Account) -> gtk::Widget {
    sub_crumb(cp, "Manage Accounts");
    side_common(cp);
    let v = vbox(6);
    v.pack_start(&title(&format!("Make changes to {}'s account", a.shown())), false, false, 0);
    let row = hbox(40);
    let links = vbox(5);
    let nav = |t: &str, what: &str| { let (c, page) = (cp.clone(), pid(a, what)); link(t, move || go(&c, &page)) };
    links.pack_start(&nav("Change the account name", "name"), false, false, 0);
    if a.password == Some(false) { links.pack_start(&nav("Create a password", "password"), false, false, 0); }
    else { links.pack_start(&nav("Change the password", "password"), false, false, 0); links.pack_start(&nav("Remove the password", "removepw"), false, false, 0); }
    links.pack_start(&nav("Change the picture", "picture"), false, false, 0);
    links.pack_start(&nav("Change the account type", "type"), false, false, 0);
    links.pack_start(&nav("Delete the account", "delete"), false, false, 0);
    links.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 4);
    { let c = cp.clone(); links.pack_start(&link("Manage another account", move || go(&c, "users/manage")), false, false, 0); }
    row.pack_start(&links, false, false, 0);
    row.pack_start(&tile_block(a, true), false, false, 0);
    v.pack_start(&row, false, false, 0);
    v.upcast()
}

fn manage_page(cp: &Rc<Cp>, all: &[Account], mine: &Account) -> gtk::Widget {
    sub_crumb(cp, "Manage Accounts");
    let v = vbox(8);
    v.pack_start(&title("Choose the account you would like to change"), false, false, 0);
    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_max_children_per_line(2);
    grid.set_min_children_per_line(2);
    grid.set_column_spacing(20);
    grid.set_row_spacing(10);
    let frame = vbox(0);
    frame.style_context().add_class("cp-list");
    frame.set_border_width(0);
    grid.set_border_width(10);
    frame.pack_start(&grid, false, false, 0);
    for a in all {
        let b = gtk::Button::new();
        b.style_context().add_class("ua-account");
        b.set_relief(gtk::ReliefStyle::None);
        b.add(&tile_block(a, false));
        let (c, page) = (cp.clone(), if a.user == mine.user { "users".to_string() } else { pid(a, "") });
        b.connect_clicked(move |_| go(&c, &page));
        b.set_size_request(260, -1);
        grid.add(&b);
    }
    v.pack_start(&frame, false, false, 0);
    { let c = cp.clone(); v.pack_start(&link("Create a new account", move || go(&c, "users/create")), false, false, 4); }
    v.pack_start(&link("What is a user account?", || { util::open_uri("https://wiki.archlinux.org/title/Users_and_groups"); }), false, false, 0);
    v.pack_start(&section("Additional things you can do"), false, false, 0);
    { let c = cp.clone(); v.pack_start(&link("Go to the main User Accounts page", move || go(&c, "users")), false, false, 0); }
    v.upcast()
}

// ---------------------------------------------------------------------------- password

/// Run `passwd` for ourselves in a pseudo terminal: current, new, new. (success, message)
fn passwd_self(current: &str, new: &str) -> (bool, String) {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};
    let sys = native_pty_system();
    let Ok(pair) = sys.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }) else { return (false, "No pseudo terminal is available.".into()) };
    let mut cmd = CommandBuilder::new("passwd");
    cmd.env("LC_ALL", "C");
    let Ok(mut child) = pair.slave.spawn_command(cmd) else { return (false, "passwd was not found.".into()) };
    drop(pair.slave);
    let Ok(mut reader) = pair.master.try_clone_reader() else { return (false, String::new()) };
    let Ok(mut writer) = pair.master.take_writer() else { return (false, String::new()) };
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || { let mut b = [0u8; 512]; while let Ok(n) = reader.read(&mut b) { if n == 0 || tx.send(b[..n].to_vec()).is_err() { break; } } });
    // prompts: "Current password:", "New password:", "Retype new password:"; a prompt seen twice
    // means passwd refused the answer (wrong password, BAD PASSWORD …): stop there
    let (mut buf, mut log, mut msg) = (String::new(), String::new(), String::new());
    let (mut cur_done, mut new_done, mut retype_done) = (false, false, false);
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(15)) {
            Ok(chunk) => {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                let tail = buf.trim_end().to_lowercase();
                if !tail.ends_with("password:") { continue; }
                let prompt = tail.lines().last().unwrap_or("").to_string();
                let seen = if prompt.contains("retype") || prompt.contains("again") { std::mem::replace(&mut retype_done, true) }
                    else if prompt.contains("new") { std::mem::replace(&mut new_done, true) } else { std::mem::replace(&mut cur_done, true) };
                log.push_str(&buf);
                buf.clear();
                if seen { msg = log.clone(); let _ = child.kill(); break; }
                let ans = if prompt.contains("new") || prompt.contains("retype") || prompt.contains("again") { new } else { current };
                let _ = writeln!(writer, "{ans}");
                log.push('\n');
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => { let _ = child.kill(); msg = "passwd did not answer.".into(); break; }
            Err(_) => { log.push_str(&buf); break; }
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    if msg.is_empty() { msg = log; }
    let msg = msg.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.to_lowercase().ends_with("password:") && !l.starts_with("Changing password")).collect::<Vec<_>>().join("\n");
    (ok, msg)
}

/// `pkexec chpasswd` with "user:password" on stdin (an administrator resets someone's password).
fn chpasswd(user: &str, pass: &str) -> (bool, String) {
    use std::io::Write;
    let mut c = match std::process::Command::new("pkexec").arg("chpasswd").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn() {
        Ok(c) => c, Err(e) => return (false, e.to_string()),
    };
    if let Some(mut i) = c.stdin.take() { let _ = writeln!(i, "{user}:{pass}"); }
    match c.wait_with_output() { Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned()), Err(e) => (false, e.to_string()) }
}

/// Check our own password (pam_unix's helper; no root needed for our own account).
fn check_own_password(pass: &str) -> Option<bool> {
    use std::io::Write;
    let helper = ["/usr/bin/unix_chkpwd", "/usr/sbin/unix_chkpwd", "/sbin/unix_chkpwd"].into_iter().find(|p| Path::new(p).exists())?;
    let mut c = std::process::Command::new(helper).arg(me()).arg("nonull").stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().ok()?;
    if let Some(mut i) = c.stdin.take() { let _ = i.write_all(pass.as_bytes()); let _ = i.write_all(&[0]); }
    Some(c.wait().ok()?.success())
}

fn set_hint(uid: u32, hint: &str) {
    let path = format!("/org/freedesktop/Accounts/User{uid}");
    util::spawn(&["busctl", "--system", "call", "org.freedesktop.Accounts", &path, "org.freedesktop.Accounts.User", "SetPasswordHint", "s", hint]);
}

fn cue(e: &gtk::Entry, t: &str) { e.set_placeholder_text(Some(t)); e.set_width_chars(28); e.set_halign(gtk::Align::Start); }

fn password_page(cp: &Rc<Cp>, a: &Account, own: bool) -> gtk::Widget {
    let create = a.password == Some(false);
    let t = match (own, create) { (true, true) => "Create a password for your account".to_string(), (true, false) => "Change your password".to_string(),
        (false, true) => format!("Create a password for {}'s account", a.shown()), (false, false) => format!("Change {}'s password", a.shown()) };
    sub_crumb(cp, if own { "Change Your Password" } else { "Change Password" });
    let v = vbox(8);
    v.pack_start(&title(&t), false, false, 0);
    v.pack_start(&tile_block(a, false), false, false, 4);
    if !own && !create {
        let w = label(&format!("You are resetting the password for {0}. If you do this, {0} will lose all stored passwords for Web sites or network resources (the keyring cannot be unlocked with the new password).", a.shown()), "");
        w.set_max_width_chars(70);
        v.pack_start(&w, false, false, 0);
    }
    let cur = gtk::Entry::new();
    cur.set_visibility(false);
    cue(&cur, "Current password");
    if own && !create { v.pack_start(&cur, false, false, 0); }
    let (n1, n2) = (gtk::Entry::new(), gtk::Entry::new());
    for (e, c) in [(&n1, "New password"), (&n2, "Confirm new password")] { e.set_visibility(false); cue(e, c); v.pack_start(e, false, false, 0); }
    v.pack_start(&text("If your password contains capital letters, they must be typed the same way every time you log on."), false, false, 0);
    v.pack_start(&link("How to create a strong password", || { util::open_uri("https://wiki.archlinux.org/title/Security#Passwords"); }), false, false, 0);
    let hint = gtk::Entry::new();
    cue(&hint, "Type a password hint");
    let has_as = Path::new("/var/lib/AccountsService").exists();
    if own {
        if !has_as { hint.set_sensitive(false); hint.set_tooltip_text(Some("Password hints are kept by AccountsService, which is not installed.")); }
        v.pack_start(&hint, false, false, 0);
        v.pack_start(&text("The password hint will be visible to everyone who uses this computer."), false, false, 0);
        v.pack_start(&link("What is a password hint?", || { util::open_uri("https://wiki.archlinux.org/title/AccountsService"); }), false, false, 0);
    }
    let ok = gtk::Button::with_label(if create { "Create password" } else { "Change password" });
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "users")); }
    let (c, user, uid) = (cp.clone(), a.user.clone(), a.uid);
    ok.connect_clicked(move |b| {
        let parent = win_of(b);
        if n1.text() != n2.text() { inform(parent.as_ref(), "User Accounts", "The passwords you typed do not match.", "Please retype the new password in both boxes."); return; }
        if n1.text().is_empty() { inform(parent.as_ref(), "User Accounts", "Type a new password.", ""); return; }
        b.set_sensitive(false);
        let (cur_t, new_t, hint_t, user2) = (cur.text().to_string(), n1.text().to_string(), hint.text().to_string(), user.clone());
        let (c2, b2) = (c.clone(), b.clone());
        bg(move || if own && !create { passwd_self(&cur_t, &new_t) } else { chpasswd(&user2, &new_t) }, move |(done, msg)| {
            b2.set_sensitive(true);
            if !done {
                let wrong = msg.contains("Authentication token manipulation") || msg.contains("Authentication failure");
                if wrong { inform(win_of(&b2).as_ref(), "User Accounts", "The password you typed is incorrect.", "Please retype your current password. The letters in passwords must be typed using the correct case."); }
                else if !msg.contains("Not authorized") && !msg.contains("dismissed") { util::error("Your password could not be changed.", msg.trim()); }
                return;
            }
            if own && !hint_t.is_empty() { set_hint(uid, &hint_t); }
            back(&c2, "users");
        });
    });
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

fn remove_password_page(cp: &Rc<Cp>, a: &Account, own: bool) -> gtk::Widget {
    sub_crumb(cp, if own { "Remove Your Password" } else { "Remove Password" });
    let v = vbox(8);
    v.pack_start(&title(&if own { "Remove your password".to_string() } else { format!("Remove {}'s password", a.shown()) }), false, false, 0);
    v.pack_start(&tile_block(a, false), false, false, 4);
    let question = text(&if own { "Are you sure you want to remove your password?".to_string() } else { format!("Are you sure you want to remove {}'s password?", a.shown()) });
    v.pack_start(&question, false, false, 0);
    v.pack_start(&text(&if own { "If you remove your password, anyone can log on to your account and see your files. Programs that keep passwords in your keyring may stop working.".to_string() }
        else { format!("If you remove the password, anyone can log on to {}'s account.", a.shown()) }), false, false, 0);
    let cur = gtk::Entry::new();
    cur.set_visibility(false);
    cue(&cur, "Current password");
    if own {
        v.pack_start(&text("To verify that you are the person who owns this account, please type your current password:"), false, false, 0);
        v.pack_start(&cur, false, false, 0);
    }
    let ok = gtk::Button::with_label("Remove Password");
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "users")); }
    let (c, user) = (cp.clone(), a.user.clone());
    ok.connect_clicked(move |b| {
        if own {
            let t = cur.text().to_string();
            if check_own_password(&t) == Some(false) {
                inform(win_of(b).as_ref(), "User Accounts", "The password you typed is incorrect.", "Please retype your current password. The letters in passwords must be typed using the correct case.");
                return;
            }
        }
        let c2 = c.clone();
        elevate(&["passwd", "-d", &user], move |ok, out| { if report("The password could not be removed.", ok, &out) { back(&c2, "users"); } });
    });
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

// ----------------------------------------------------------------------------- picture

fn avatars() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(config::data_dir().join("avatars")).map(|rd| rd.flatten().map(|e| e.path())
        .filter(|p| p.file_name().map(|n| n.to_string_lossy().starts_with("usertile")).unwrap_or(false)).collect()).unwrap_or_default();
    // usertile10 … usertile45, in Win7's order
    v.sort_by_key(|p| p.file_stem().and_then(|s| s.to_string_lossy().trim_start_matches("usertile").parse::<u32>().ok()).unwrap_or(999));
    v
}

fn picture_page(cp: &Rc<Cp>, a: &Account, own: bool) -> gtk::Widget {
    sub_crumb(cp, if own { "Change Your Picture" } else { "Change Picture" });
    let v = vbox(8);
    v.pack_start(&title(&if own { "Choose a new picture for your account".to_string() } else { format!("Choose a new picture for {}'s account", a.shown()) }), false, false, 0);
    let top = hbox(10);
    let cur_img = gtk::Image::new();
    set_picture(&cur_img, a.picture.as_deref(), 64);
    let fr = vbox(0);
    fr.style_context().add_class("ua-tile");
    fr.set_valign(gtk::Align::Start);
    fr.pack_start(&cur_img, false, false, 0);
    top.pack_start(&fr, false, false, 0);
    let tv = vbox(1);
    tv.set_valign(gtk::Align::Center);
    tv.pack_start(&label(a.shown(), "ua-name"), false, false, 0);
    tv.pack_start(&label(a.kind(), "dim"), false, false, 0);
    top.pack_start(&tv, false, false, 0);
    v.pack_start(&top, false, false, 0);
    v.pack_start(&text(&if own { "Your picture will appear on the Welcome screen and on the Start menu.".to_string() } else { format!("{}'s picture will appear on the Welcome screen and on the Start menu.", a.shown()) }), false, false, 0);
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::Single);
    flow.set_max_children_per_line(9);
    flow.set_min_children_per_line(6);
    flow.set_column_spacing(4);
    flow.set_row_spacing(4);
    let paths = avatars();
    for p in &paths {
        if let Ok(pb) = Pixbuf::from_file_at_scale(p, 56, 56, true) {
            let i = gtk::Image::from_pixbuf(Some(&pb));
            i.set_tooltip_text(p.file_stem().map(|s| s.to_string_lossy().into_owned()).as_deref());
            flow.insert(&i, -1);
        }
    }
    let box_ = vbox(0);
    box_.style_context().add_class("cp-list");
    flow.set_border_width(8);
    box_.pack_start(&flow, false, false, 0);
    if paths.is_empty() { box_.pack_start(&label("No pictures were found. Run aerode-extract with your Windows 7 installation disc to get them, or browse for one.", "cp-hint"), false, false, 8); }
    v.pack_start(&box_, false, false, 0);
    let chosen: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    { let (ch, paths, ci) = (chosen.clone(), paths.clone(), cur_img.clone()); flow.connect_selected_children_changed(move |f| {
        if let Some(c) = f.selected_children().first() { let p = paths.get(c.index() as usize).cloned(); set_picture(&ci, p.as_deref(), 64); *ch.borrow_mut() = p; }
    }); }
    let ok = gtk::Button::with_label("Change Picture");
    let cancel = gtk::Button::with_label("Cancel");
    let apply: Rc<dyn Fn(PathBuf)> = {
        let (c, user, home, own) = (cp.clone(), a.user.clone(), a.home.clone(), own);
        let uid = a.uid;
        Rc::new(move |p: PathBuf| set_account_picture(&c, &user, &home, uid, own, &p))
    };
    {
        let (apply, flow2) = (apply.clone(), flow.clone());
        v.pack_start(&link("Browse for more pictures...", move || {
            let fc = gtk::FileChooserNative::new(Some("Open"), win_of(&flow2).as_ref(), gtk::FileChooserAction::Open, Some("_Open"), Some("_Cancel"));
            let f = gtk::FileFilter::new();
            f.set_name(Some("Image Files"));
            f.add_pixbuf_formats();
            fc.add_filter(f);
            if let Some(pics) = glib::user_special_dir(glib::UserDirectory::Pictures) { let _ = fc.set_current_folder(pics); }
            if fc.run() == gtk::ResponseType::Accept { if let Some(p) = fc.filename() { apply(p); } }
        }), false, false, 0);
    }
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "users")); }
    { let (ch, apply) = (chosen.clone(), apply.clone()); ok.connect_clicked(move |_| { if let Some(p) = ch.borrow().clone() { apply(p); } }); }
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

/// Save the picture as a 128 px PNG (Win7 user tiles are 128 px) where display managers and our
/// shell look: ~/.face and ~/.face.icon, and AccountsService's icon.
fn set_account_picture(cp: &Rc<Cp>, user: &str, home: &str, uid: u32, own: bool, src: &Path) {
    let Ok(pb) = Pixbuf::from_file_at_scale(src, 128, 128, true) else { util::error("The picture could not be opened.", &src.to_string_lossy()); return };
    let tmp = super::super::admin::cache_dir().join(format!("usertile-{user}.png"));
    if pb.savev(&tmp, "png", &[]).is_err() { util::error("The picture could not be saved.", ""); return; }
    let t = tmp.to_string_lossy().into_owned();
    let c = cp.clone();
    if own {
        for f in [".face", ".face.icon"] { let _ = std::fs::copy(&tmp, Path::new(home).join(f)); }
        // AccountsService lets us change our own icon without a prompt; without the daemon, copy it as root
        let path = format!("/org/freedesktop/Accounts/User{uid}");
        let t2 = t.clone();
        let user = user.to_string();
        bg(move || run(&["busctl", "--system", "call", "org.freedesktop.Accounts", &path, "org.freedesktop.Accounts.User", "SetIconFile", "s", &t2]).0, move |ok| {
            if !ok && Path::new("/var/lib/AccountsService").exists() { as_icon_root(&user, &t, |_| {}); }
            util::spawn(&["aerode", "reload"]);
            back(&c, "users");
        });
    } else {
        let script = format!("install -Dm644 {t} {h}/.face && cp {h}/.face {h}/.face.icon && chown {u}: {h}/.face {h}/.face.icon{as_}",
            t = q(&t), h = q(home), u = q(user),
            as_ = if Path::new("/var/lib/AccountsService").exists() { format!(" && install -Dm644 {} /var/lib/AccountsService/icons/{}", q(&t), q(user)) } else { String::new() });
        elevate_sh(&script, move |ok, out| { if report("The picture could not be changed.", ok, &out) { back(&c, "users"); } });
    }
}

fn as_icon_root(user: &str, file: &str, done: impl FnOnce(bool) + 'static) {
    let ini = format!("/var/lib/AccountsService/users/{user}");
    let script = format!("install -Dm644 {f} /var/lib/AccountsService/icons/{u} && (grep -q '^Icon=' {i} 2>/dev/null && sed -i 's|^Icon=.*|Icon=/var/lib/AccountsService/icons/{u}|' {i} || printf '[User]\\nIcon=/var/lib/AccountsService/icons/{u}\\n' >> {i})",
        f = q(file), u = user, i = q(&ini));
    elevate_sh(&script, move |ok, _| done(ok));
}

// ------------------------------------------------------------------------- name and type

fn name_page(cp: &Rc<Cp>, a: &Account, own: bool) -> gtk::Widget {
    sub_crumb(cp, if own { "Change Your Name" } else { "Rename Account" });
    let v = vbox(8);
    v.pack_start(&title(&if own { "Type a new account name for your account".to_string() } else { format!("Type a new account name for {}'s account", a.shown()) }), false, false, 0);
    v.pack_start(&tile_block(a, false), false, false, 4);
    let e = gtk::Entry::new();
    cue(&e, "New account name");
    e.set_text(a.shown());
    v.pack_start(&e, false, false, 0);
    v.pack_start(&text(&if own { "This name will appear on the Welcome screen and on the Start menu.".to_string() } else { format!("{}'s name will appear on the Welcome screen and on the Start menu.", a.shown()) }), false, false, 0);
    let note = label(&format!("The user name used to log on stays \"{}\".", a.user), "cp-hint");
    v.pack_start(&note, false, false, 0);
    let ok = gtk::Button::with_label("Change Name");
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "users")); }
    let (c, user) = (cp.clone(), a.user.clone());
    ok.connect_clicked(move |b| {
        let n = e.text().trim().to_string();
        if n.is_empty() || n.contains([':', ',', '=']) { inform(win_of(b).as_ref(), "User Accounts", "The name cannot contain any of these characters: : , =", ""); return; }
        let c2 = c.clone();
        elevate(&["chfn", "-f", &n, &user], move |ok, out| { if report("The account name could not be changed.", ok, &out) { util::spawn(&["aerode", "reload"]); back(&c2, "users"); } });
    });
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

const STD_DESC: &str = "Standard account users can use most software and change system settings that do not affect other users or the security of the computer.";
const ADMIN_DESC: &str = "Administrators have complete access to the computer and can make any desired changes. Based on notification settings, administrators may be asked to provide their password or confirmation before making changes that affect other users.";

fn type_radios(v: &gtk::Box, admin: bool) -> Vec<gtk::RadioButton> {
    let r = radios(&["_Standard user", "_Administrator"], if admin { 1 } else { 0 });
    for (b, d) in r.iter().zip([STD_DESC, ADMIN_DESC]) {
        v.pack_start(b, false, false, 0);
        let l = label(d, "");
        l.set_margin_start(24);
        l.set_max_width_chars(70);
        v.pack_start(&l, false, false, 0);
    }
    r
}

fn type_page(cp: &Rc<Cp>, a: &Account, own: bool, all: &[Account]) -> gtk::Widget {
    sub_crumb(cp, if own { "Change Your Account Type" } else { "Change Account Type" });
    let v = vbox(8);
    v.pack_start(&title(&format!("Choose a new account type for {}", a.shown())), false, false, 0);
    v.pack_start(&tile_block(a, false), false, false, 4);
    let r = type_radios(&v, a.admin);
    v.pack_start(&text("We recommend protecting every account with a strong password."), false, false, 0);
    v.pack_start(&link("Why is a standard account recommended?", || { util::open_uri("https://wiki.archlinux.org/title/Sudo"); }), false, false, 0);
    let ok = shield_button("Change Account Type");
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "users")); }
    let admins = all.iter().filter(|x| x.admin).count();
    let (c, user, was) = (cp.clone(), a.user.clone(), a.admin);
    ok.connect_clicked(move |b| {
        let admin = r[1].is_active();
        if admin == was { back(&c, "users"); return; }
        if !admin && admins <= 1 {
            inform(win_of(b).as_ref(), "User Accounts", "At least one administrator account must exist on this computer.", "Make another account an administrator first.");
            return;
        }
        let c2 = c.clone();
        let argv: [&str; 4] = if admin { ["gpasswd", "-a", user.as_str(), "wheel"] } else { ["gpasswd", "-d", user.as_str(), "wheel"] };
        elevate(&argv, move |ok, out| { if report("The account type could not be changed.", ok, &out) { back(&c2, "users"); } });
    });
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

// --------------------------------------------------------------------- create / delete

/// "Jane Smith" → "jane.smith"-style log on name (lower case letters, digits, _ - .).
pub fn login_name(display: &str) -> String {
    let mut s: String = display.trim().to_lowercase().chars().map(|c| if c.is_whitespace() { '.' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')).collect();
    while s.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.') { s.remove(0); }
    s.truncate(32);
    s
}

fn create_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "Create New Account");
    let v = vbox(8);
    v.pack_start(&title("Name the account and choose an account type"), false, false, 0);
    v.pack_start(&text("This name will appear on the Welcome screen and on the Start menu."), false, false, 0);
    let e = gtk::Entry::new();
    cue(&e, "New account name");
    v.pack_start(&e, false, false, 0);
    let login = label("", "cp-hint");
    v.pack_start(&login, false, false, 0);
    { let l = login.clone(); e.connect_changed(move |e| { let n = login_name(&e.text()); l.set_text(&if n.is_empty() { String::new() } else { format!("Log on name: {n}") }); }); }
    let r = type_radios(&v, false);
    v.pack_start(&text("We recommend that you protect every account with a strong password."), false, false, 0);
    v.pack_start(&link("Why is a standard account recommended?", || { util::open_uri("https://wiki.archlinux.org/title/Sudo"); }), false, false, 0);
    let ok = shield_button("Create Account");
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "users/manage")); }
    let c = cp.clone();
    ok.connect_clicked(move |b| {
        let shown = e.text().trim().to_string();
        let user = login_name(&shown);
        if user.is_empty() { inform(win_of(b).as_ref(), "User Accounts", "Type a name for the new account.", ""); return; }
        if std::fs::read_to_string("/etc/passwd").unwrap_or_default().lines().any(|l| l.split(':').next() == Some(&user)) {
            inform(win_of(b).as_ref(), "User Accounts", &format!("The name {user} is already used by another account."), "Type a different name.");
            return;
        }
        let mut argv = vec!["useradd", "-m", "-s", "/bin/bash", "-c", shown.as_str()];
        if r[1].is_active() { argv.extend(["-G", "wheel"]); }
        argv.push(user.as_str());
        let c2 = c.clone();
        let page = format!("users/acct/{user}");
        elevate(&argv, move |ok, out| { if report("The account could not be created.", ok, &out) { back(&c2, "users/manage"); go(&c2, &page); } });
    });
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

fn delete_page(cp: &Rc<Cp>, a: &Account) -> gtk::Widget {
    sub_crumb(cp, "Delete Account");
    let n = a.shown().to_string();
    let v = vbox(8);
    if a.user == me() {
        v.pack_start(&title("You cannot delete the account you are logged on with"), false, false, 0);
        return v.upcast();
    }
    let stage: Rc<Cell<u8>> = Rc::new(Cell::new(0));
    let body = vbox(8);
    v.pack_start(&body, false, false, 0);
    let (c, user, home) = (cp.clone(), a.user.clone(), a.home.clone());
    let draw: Rc<RefCell<Option<Rc<dyn Fn(bool)>>>> = Rc::new(RefCell::new(None));
    let d2 = draw.clone();
    let f: Rc<dyn Fn(bool)> = Rc::new(move |keep_files: bool| {
        for ch in body.children() { body.remove(&ch); }
        if stage.get() == 0 {
            body.pack_start(&title(&format!("Do you want to keep {n}'s files?")), false, false, 0);
            let t = text(&format!("Before you delete {n}'s account, Windows can automatically save the contents of {n}'s desktop and Documents, Downloads, Music, Pictures and Videos folders to a new folder called '{n}' on your desktop. However, Windows cannot save {n}'s e-mail messages and other settings."));
            body.pack_start(&t, false, false, 0);
            let (del, kp, cancel) = (gtk::Button::with_label("Delete Files"), gtk::Button::with_label("Keep Files"), gtk::Button::with_label("Cancel"));
            for (b, k) in [(&del, false), (&kp, true)] { let (st, d) = (stage.clone(), d2.clone()); b.connect_clicked(move |_| { st.set(1); if let Some(f) = d.borrow().clone() { f(k); } }); }
            { let c = c.clone(); cancel.connect_clicked(move |_| back(&c, "users/manage")); }
            body.pack_start(&button_row(&[&del, &kp, &cancel]), false, false, 0);
        } else {
            body.pack_start(&title(&format!("Are you sure you want to delete {n}'s account?")), false, false, 0);
            body.pack_start(&text(&if keep_files { format!("Windows will save the contents of {n}'s desktop and Documents, Downloads, Music, Pictures and Videos folders to a folder called '{n}' on your desktop, then delete {n}'s account.") }
                else { format!("All of {n}'s files will be deleted, and then {n}'s account will be deleted.") }), false, false, 0);
            let (ok, cancel) = (shield_button("Delete Account"), gtk::Button::with_label("Cancel"));
            { let c = c.clone(); cancel.connect_clicked(move |_| back(&c, "users/manage")); }
            let (c, user, home, n) = (c.clone(), user.clone(), home.clone(), n.clone());
            ok.connect_clicked(move |_| {
                let desk = util::special_dir(glib::UserDirectory::Desktop, "Desktop").join(&n);
                let me_ = me();
                let script = if keep_files {
                    format!("mkdir -p {d} && for f in Desktop Documents Downloads Music Pictures Videos; do [ -d {h}/\"$f\" ] && cp -a {h}/\"$f\" {d}/; done; chown -R {me}: {d} && userdel -r {u}",
                        d = q(&desk.to_string_lossy()), h = q(&home), me = q(&me_), u = q(&user))
                } else { format!("userdel -r {}", q(&user)) };
                let c2 = c.clone();
                elevate_sh(&script, move |ok, out| {
                    // userdel -r: exit 12 = could not remove the mail spool/home, the account is gone though
                    if ok || out.contains("not found") || out.contains("mail spool") { go(&c2, "users/manage"); } else { report("The account could not be deleted.", ok, &out); }
                });
            });
            body.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
        }
        body.show_all();
    });
    *draw.borrow_mut() = Some(f.clone());
    f(false);
    v.upcast()
}

// ------------------------------------------------------------------- UAC settings

const UAC_RULE: &str = "/etc/polkit-1/rules.d/49-aerode-never-notify.rules";

/// (headline, bullets, recommendation) for each slider level, bottom (0) to top (3).
const UAC_TEXT: [(&str, &[&str], &str); 4] = [
    ("Never notify me when:", &["Programs try to install software or make changes to my computer", "I make changes to Windows settings"], "Not recommended."),
    ("Notify me only when programs try to make changes to my computer (do not dim my desktop)", &["Don't notify me when I make changes to Windows settings"], "Not recommended. Choose this only if it takes a long time to dim the desktop on your computer."),
    ("Default - Notify me only when programs try to make changes to my computer", &["Don't notify me when I make changes to Windows settings"], "Recommended if you use familiar programs and visit familiar websites."),
    ("Always notify me when:", &["Programs try to install software or make changes to my computer", "I make changes to Windows settings"], "Recommended if you routinely install new software and visit unfamiliar websites."),
];

fn uac_page(cp: &Rc<Cp>) -> gtk::Widget {
    sub_crumb(cp, "User Account Control Settings");
    let cur = config::get().uac_level.unwrap_or(2).min(3);
    let v = vbox(6);
    v.pack_start(&title("Choose when to be notified about changes to your computer"), false, false, 0);
    v.pack_start(&text("User Account Control helps prevent potentially harmful programs from making changes to your computer."), false, false, 0);
    v.pack_start(&link("Tell me more about User Account Control settings", || { util::open_uri("https://wiki.archlinux.org/title/Polkit"); }), false, false, 0);
    let row = hbox(18);
    row.set_margin_top(10);
    row.set_margin_start(20);
    let col = vbox(0);
    col.pack_start(&label("Always notify", "uac-end"), false, false, 0);
    let scale = gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, 3.0, 1.0);
    scale.set_inverted(true);
    scale.set_draw_value(false);
    scale.set_digits(0);
    scale.set_round_digits(0);
    for i in 0..4 { scale.add_mark(i as f64, gtk::PositionType::Left, None); }
    scale.set_value(cur as f64);
    scale.set_size_request(-1, 200);
    scale.set_halign(gtk::Align::Center);
    col.pack_start(&scale, false, false, 4);
    col.pack_start(&label("Never notify", "uac-end"), false, false, 0);
    row.pack_start(&col, false, false, 0);
    let desc = vbox(6);
    desc.style_context().add_class("uac-desc");
    desc.set_size_request(320, 180);
    desc.set_valign(gtk::Align::Center);
    row.pack_start(&desc, false, false, 0);
    let shield = img(&["security-medium", "aerode-uac-shield"], 32);
    shield.set_valign(gtk::Align::Center);
    row.pack_start(&shield, false, false, 0);
    v.pack_start(&row, false, false, 0);
    let show = { let desc = desc.clone(); move |lvl: u32| {
        for c in desc.children() { desc.remove(&c); }
        let (h, bullets, rec) = UAC_TEXT[lvl as usize];
        desc.pack_start(&bold(h), false, false, 0);
        for b in bullets { let l = label(&format!("•  {b}"), ""); l.set_margin_start(8); l.set_max_width_chars(44); desc.pack_start(&l, false, false, 0); }
        let r = label(rec, "");
        r.set_margin_top(10);
        r.set_max_width_chars(48);
        desc.pack_start(&r, false, false, 0);
        desc.show_all();
    } };
    show(cur);
    let ok = shield_button("OK");
    ok.set_sensitive(false);
    {
        let (show, ok) = (show.clone(), ok.clone());
        scale.connect_value_changed(move |s| {
            let l = s.value().round().clamp(0.0, 3.0) as u32;
            if (s.value() - l as f64).abs() > 0.01 { s.set_value(l as f64); return; }
            show(l);
            ok.set_sensitive(l != cur);
        });
    }
    let cancel = gtk::Button::with_label("Cancel");
    { let c = cp.clone(); cancel.connect_clicked(move |_| back(&c, "users")); }
    let c = cp.clone();
    ok.connect_clicked(move |b| {
        let l = scale.value().round() as u32;
        let had_rule = Path::new(UAC_RULE).exists();
        let save = { let c = c.clone(); move || { config::update(|cf| cf.uac_level = Some(l)); back(&c, "users"); } };
        if l == 0 {
            if !confirm(win_of(b).as_ref(), "User Account Control Settings", "Turn off notifications?",
                "Programs started by an administrator will be able to make changes to the computer without asking. Not recommended.", "_Yes") { return; }
            let rule = "// aerode: User Account Control Settings - Never notify (administrators are not asked)\npolkit.addRule(function(action, subject) {\n    if (subject.local && subject.active && subject.isInGroup(\"wheel\")) { return polkit.Result.YES; }\n});\n";
            elevate_sh(&format!("mkdir -p /etc/polkit-1/rules.d && printf '%s' {} > {}", q(rule), UAC_RULE), move |ok, out| { if report("User Account Control settings could not be changed.", ok, &out) { save(); } });
        } else if had_rule {
            elevate(&["rm", "-f", UAC_RULE], move |ok, out| { if report("User Account Control settings could not be changed.", ok, &out) { save(); } });
        } else { save(); }
    });
    v.pack_start(&button_row(&[&ok, &cancel]), false, false, 0);
    v.upcast()
}

#[cfg(test)]
mod tests {
    #[test]
    fn login() {
        assert_eq!(super::login_name("Jane Smith"), "jane.smith");
        assert_eq!(super::login_name(" 12bob"), "bob");
        assert_eq!(super::login_name("Ödön"), "dn");
    }
}
