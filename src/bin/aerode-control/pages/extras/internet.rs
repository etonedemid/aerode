//! Internet Options: the "Internet Properties" dialog. General (the default browser's own home
//! page / history / search settings), Connections (VPN and dial-up connections from
//! NetworkManager, LAN settings = the system proxy written to ~/.config/environment.d/proxy.conf,
//! GNOME and KDE), Programs (default browser and e-mail program), Advanced (browser flags).
use gtk::prelude::*;
use gtk::{gio, glib};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use aerode::{config, util};
use crate::Cp;
use super::common::*;

pub fn page_internet(cp: &Rc<Cp>) -> gtk::Widget {
    css();
    let me = cp.clone();
    standalone(cp, move |parent| dialog(&me, parent))
}

/// Open a Control Panel page: in this window when it is showing, else in a new one.
fn open_cp(cp: &Rc<Cp>, page: &str) {
    if cp.win.is_visible() { cp.open(page); cp.win.present(); } else { util::spawn(&["aerode-control", page]); }
}

#[derive(Clone, Copy, PartialEq)]
enum Family { Mozilla, Chromium, Other }

fn family(app: &gio::AppInfo) -> Family {
    let id = app.id().map(|s| s.to_lowercase()).unwrap_or_default();
    if ["firefox", "librewolf", "waterfox", "floorp", "zen", "mozilla"].iter().any(|k| id.contains(k)) { Family::Mozilla }
    else if ["chrom", "brave", "vivaldi", "opera", "edge", "thorium"].iter().any(|k| id.contains(k)) { Family::Chromium }
    else { Family::Other }
}

fn browser() -> Option<gio::AppInfo> { gio::AppInfo::default_for_type("x-scheme-handler/http", false) }

/// Open one of the default browser's own settings pages.
fn browser_page(moz: &str, chrome: &str) {
    let Some(app) = browser() else { util::error("No web browser is installed.", "Install a web browser such as Firefox."); return };
    let url = match family(&app) { Family::Mozilla => moz, Family::Chromium => chrome, Family::Other => "" };
    let ctx = gtk::gdk::Display::default().and_then(|d| d.app_launch_context());
    let r = if url.is_empty() { app.launch(&[], ctx.as_ref()) } else { app.launch_uris(&[url], ctx.as_ref()) };
    if let Err(e) = r { util::error(&format!("Windows cannot open '{}'.", app.display_name()), &e.to_string()); }
}

fn dialog(cp: &Rc<Cp>, parent: Option<&gtk::Window>) -> gtk::Window {
    let d = gtk::Window::new(gtk::WindowType::Toplevel);
    d.set_title("Internet Properties");
    d.set_icon_name(Some(util::first_icon(&["web-browser", "applications-internet", "preferences-system-network"])));
    d.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
    d.set_resizable(false);
    d.set_default_size(410, 520);
    if let Some(p) = parent { d.set_transient_for(Some(p)); d.set_position(gtk::WindowPosition::CenterOnParent); } else { d.set_position(gtk::WindowPosition::Center); }
    let root = vbox(8);
    root.set_border_width(8);
    let nb = gtk::Notebook::new();
    let prof = browser().and_then(|b| profile(&b));
    let bp = prof.as_ref().map(read_prefs).unwrap_or_default();
    let state = Rc::new(State { proxy: RefCell::new(load_proxy()), adv: RefCell::new(load_advanced()), prof, browser: RefCell::new(bp.clone()), browser_saved: RefCell::new(bp) });
    nb.append_page(&general(cp, &state), Some(&gtk::Label::new(Some("General"))));
    nb.append_page(&connections(cp, &d, &state), Some(&gtk::Label::new(Some("Connections"))));
    nb.append_page(&programs(cp), Some(&gtk::Label::new(Some("Programs"))));
    nb.append_page(&advanced(&d, &state), Some(&gtk::Label::new(Some("Advanced"))));
    root.pack_start(&nb, true, true, 0);
    let ok = gtk::Button::with_label("OK");
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_mnemonic("_Apply");
    { let (d, st) = (d.clone(), state.clone()); ok.connect_clicked(move |_| { if save(&d, &st) { d.close(); } }); }
    { let d = d.clone(); cancel.connect_clicked(move |_| d.close()); }
    { let (d, st) = (d.clone(), state.clone()); apply.connect_clicked(move |_| { save(&d, &st); }); }
    root.pack_start(&button_row(&[&ok, &cancel, &apply]), false, false, 0);
    d.add(&root);
    d
}

struct State {
    proxy: RefCell<Proxy>,
    adv: RefCell<Advanced>,
    /// the default browser's profile (home page, history and the Advanced tab's browser settings)
    prof: Option<Prof>,
    browser: RefCell<BrowserPrefs>,
    browser_saved: RefCell<BrowserPrefs>,
}

/// Returns false when nothing could be saved (the browser is running).
fn save(d: &gtk::Window, st: &Rc<State>) -> bool {
    let p = st.proxy.borrow().clone();
    let a = st.adv.borrow().clone();
    let b = st.browser.borrow().clone();
    let changed_proxy = p != load_proxy();
    let changed_adv = a != load_advanced();
    let changed_browser = b != *st.browser_saved.borrow();
    if changed_browser {
        if let Some(prof) = &st.prof {
            if let Some(pid) = prof.running() {
                message(Some(d), gtk::MessageType::Warning, "Internet Properties", &format!("Close {} to change its settings.", prof.name),
                    &format!("{} is running (process {pid}) and would overwrite the new home page and browsing settings when it closes. Close it, then click Apply again.", prof.name));
                return false;
            }
            let was = st.browser_saved.borrow().clone();
            if let Err(e) = write_prefs(prof, &was, &b) {
                message(Some(d), gtk::MessageType::Error, "Internet Properties", "The browser settings could not be saved.", &e);
                return false;
            }
            *st.browser_saved.borrow_mut() = b;
        }
    }
    if changed_proxy { apply_proxy(&p); }
    if changed_adv { apply_advanced(&a); }
    // like Windows, Apply/OK say nothing when it worked
    true
}

// ------------------------------------------------------------------ General

fn general(cp: &Rc<Cp>, st: &Rc<State>) -> gtk::Widget {
    let v = vbox(10);
    v.set_border_width(10);
    let b = browser();
    let name = b.as_ref().map(|a| a.display_name().to_string()).unwrap_or_else(|| "No web browser".into());
    let fam = b.as_ref().map(family).unwrap_or(Family::Other);
    // Home page: the default browser's own start pages, one per line (Win7: home page tabs)
    let hp = vbox(6);
    let row = hbox(8);
    if let Some(a) = &b { let im = util::app_image(a, 32); im.set_valign(gtk::Align::Start); row.pack_start(&im, false, false, 0); }
    let hv = vbox(4);
    hv.pack_start(&label("To create home page tabs, type each address on its own line."), false, false, 0);
    let tv = gtk::TextView::new();
    tv.set_wrap_mode(gtk::WrapMode::None);
    let buf = tv.buffer().unwrap();
    buf.set_text(&st.browser.borrow().home.join("\n"));
    let sw = scrolled(&tv, 52);
    hv.pack_start(&sw, false, false, 0);
    row.pack_start(&hv, true, true, 0);
    hp.pack_start(&row, false, false, 0);
    let bb = hbox(6);
    let (cur, def, blank) = (gtk::Button::with_mnemonic("Use _current"), gtk::Button::with_mnemonic("Use de_fault"), gtk::Button::with_mnemonic("Use _blank"));
    for x in [&cur, &def, &blank] { bb.pack_start(x, true, true, 0); }
    let bbr = hbox(0);
    bbr.pack_end(&bb, false, false, 0);
    hp.pack_start(&bbr, false, false, 0);
    // "Use current": the pages of the browser's last session
    let current = st.prof.as_ref().map(current_pages).unwrap_or_default();
    cur.set_sensitive(!current.is_empty());
    { let buf = buf.clone(); cur.connect_clicked(move |_| buf.set_text(&current.join("\n"))); }
    let default_page = match fam { Family::Mozilla => "about:home", Family::Chromium => "chrome://newtab/", Family::Other => "about:blank" };
    { let buf = buf.clone(); def.connect_clicked(move |_| buf.set_text(default_page)); }
    { let buf = buf.clone(); blank.connect_clicked(move |_| buf.set_text("about:blank")); }
    {
        let st = st.clone();
        buf.connect_changed(move |b| {
            let t = b.text(&b.start_iter(), &b.end_iter(), false).map(|s| s.to_string()).unwrap_or_default();
            st.browser.borrow_mut().home = t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        });
    }
    if st.prof.is_none() {
        for w in [&tv.clone().upcast::<gtk::Widget>(), &bb.clone().upcast()] { w.set_sensitive(false); }
        hp.pack_start(&grey(&format!("{name} keeps its home page in its own settings.")), false, false, 0);
        let chg = gtk::Button::with_mnemonic("_Change home page...");
        chg.connect_clicked(|_| browser_page("about:preferences#home", "chrome://settings/onStartup"));
        chg.set_sensitive(b.is_some());
        chg.set_halign(gtk::Align::End);
        hp.pack_start(&chg, false, false, 0);
    } else {
        hp.pack_start(&grey(&format!("These settings are for {name}, your default web browser.")), false, false, 0);
    }
    v.pack_start(&group("Home page", &hp), false, false, 0);

    let bh = vbox(6);
    bh.pack_start(&label("Delete temporary files, history, cookies, saved passwords, and web form information."), false, false, 0);
    let on_exit = gtk::CheckButton::with_mnemonic("Delete brows_ing history on exit");
    on_exit.set_active(st.browser.borrow().clear_on_exit);
    on_exit.set_sensitive(st.prof.as_ref().map(|p| p.mozilla).unwrap_or(false));
    { let st = st.clone(); on_exit.connect_toggled(move |c| st.browser.borrow_mut().clear_on_exit = c.is_active()); }
    bh.pack_start(&on_exit, false, false, 0);
    let bb = hbox(6);
    let del = gtk::Button::with_mnemonic("_Delete...");
    del.connect_clicked(|_| browser_page("about:preferences#privacy", "chrome://settings/clearBrowserData"));
    let set = gtk::Button::with_mnemonic("_Settings");
    set.connect_clicked(|_| browser_page("about:preferences#privacy", "chrome://settings/privacy"));
    bb.pack_end(&set, false, false, 0);
    bb.pack_end(&del, false, false, 0);
    bh.pack_start(&bb, false, false, 0);
    v.pack_start(&group("Browsing history", &bh), false, false, 0);

    let se = hbox(6);
    se.pack_start(&label("Change search defaults."), true, true, 0);
    let s = gtk::Button::with_mnemonic("Se_ttings");
    s.connect_clicked(|_| browser_page("about:preferences#search", "chrome://settings/search"));
    se.pack_end(&s, false, false, 0);
    v.pack_start(&group("Search", &se), false, false, 0);

    let tb = hbox(6);
    tb.pack_start(&label("Change how webpages are displayed in tabs."), true, true, 0);
    let t = gtk::Button::with_mnemonic("Sett_ings");
    t.connect_clicked(|_| browser_page("about:preferences#general", "chrome://settings/appearance"));
    tb.pack_end(&t, false, false, 0);
    v.pack_start(&group("Tabs", &tb), false, false, 0);

    let ap = hbox(6);
    for (l, page) in [("C_olors", "personalization"), ("La_nguages", "region"), ("Fo_nts", "fonts"), ("Acc_essibility", "access")] {
        let b = gtk::Button::with_mnemonic(l);
        let me = cp.clone();
        b.connect_clicked(move |_| if page == "personalization" { util::spawn(&["aerode", "personalize"]); } else { open_cp(&me, page); });
        ap.pack_start(&b, true, true, 0);
    }
    v.pack_start(&group("Appearance", &ap), false, false, 0);
    v.upcast()
}

// ------------------------------------------------------------------ the browser's own settings

/// The default browser's profile folder: Firefox-family prefs.js, or Chromium-family Preferences.
#[derive(Clone, Debug)]
struct Prof { name: String, mozilla: bool, dir: PathBuf }

impl Prof {
    /// The browser's process while it has this profile open (it rewrites its settings on exit).
    fn running(&self) -> Option<u32> {
        let lock = if self.mozilla { self.dir.join("lock") } else { self.dir.parent()?.join("SingletonLock") };
        let target = std::fs::read_link(lock).ok()?.to_string_lossy().into_owned();
        // Firefox: "127.0.1.1:+1234"; Chromium: "hostname-1234"
        let pid: u32 = target.rsplit(['+', '-']).next()?.parse().ok()?;
        std::path::Path::new(&format!("/proc/{pid}")).exists().then_some(pid)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct BrowserPrefs {
    home: Vec<String>,
    clear_on_exit: bool,
    /// Advanced tab (Firefox prefs): animations, caret browsing, Do Not Track, pictures
    animations: bool,
    caret: bool,
    dnt: bool,
    pictures: bool,
}

/// Where the Firefox-family browsers keep profiles.ini, and the Chromium family their user data.
const MOZ_DIRS: &[(&str, &str)] = &[("librewolf", ".librewolf"), ("waterfox", ".waterfox"), ("floorp", ".floorp"), ("zen", ".zen"), ("firefox", ".mozilla/firefox"), ("mozilla", ".mozilla/firefox")];
const CHROME_DIRS: &[(&str, &str)] = &[("google-chrome", "google-chrome"), ("chromium", "chromium"), ("brave", "BraveSoftware/Brave-Browser"), ("vivaldi", "vivaldi"), ("microsoft-edge", "microsoft-edge"), ("opera", "opera"), ("thorium", "thorium")];

fn profile(app: &gio::AppInfo) -> Option<Prof> {
    let id = app.id().map(|s| s.to_lowercase()).unwrap_or_default();
    let name = app.display_name().to_string();
    if let Some((_, d)) = MOZ_DIRS.iter().find(|(k, _)| id.contains(k)) {
        let base = config::home().join(d);
        let ini = std::fs::read_to_string(base.join("profiles.ini")).ok()?;
        let dir = mozilla_default(&ini)?;
        let dir = if dir.is_absolute() { dir } else { base.join(dir) };
        return dir.is_dir().then_some(Prof { name, mozilla: true, dir });
    }
    if let Some((_, d)) = CHROME_DIRS.iter().find(|(k, _)| id.contains(k)) {
        let dir = glib::user_config_dir().join(d).join("Default");
        return dir.join("Preferences").is_file().then_some(Prof { name, mozilla: false, dir });
    }
    None
}

/// The profile Firefox starts with: the [Install…] section's Default=, else the profile marked
/// Default=1, else the only one.
fn mozilla_default(ini: &str) -> Option<PathBuf> {
    let mut sections: Vec<(String, HashMap<String, String>)> = vec![];
    for l in ini.lines() {
        let l = l.trim();
        if let Some(n) = l.strip_prefix('[').and_then(|x| x.strip_suffix(']')) { sections.push((n.to_string(), HashMap::new())); }
        else if let (Some((k, v)), Some(last)) = (l.split_once('='), sections.last_mut()) { last.1.insert(k.trim().to_string(), v.trim().to_string()); }
    }
    if let Some(p) = sections.iter().find(|(n, _)| n.starts_with("Install")).and_then(|(_, m)| m.get("Default")) { return Some(PathBuf::from(p)); }
    let profs: Vec<&HashMap<String, String>> = sections.iter().filter(|(n, _)| n.starts_with("Profile")).map(|(_, m)| m).collect();
    let pick = profs.iter().find(|m| m.get("Default").map(|d| d == "1").unwrap_or(false)).or(if profs.len() == 1 { profs.first() } else { None })?;
    pick.get("Path").map(PathBuf::from)
}

/// `user_pref("name", value);` lines of prefs.js.
fn moz_prefs(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for l in text.lines() {
        let Some(rest) = l.trim().strip_prefix("user_pref(\"") else { continue };
        let Some((name, val)) = rest.split_once("\",") else { continue };
        let val = val.trim().trim_end_matches(';').trim().trim_end_matches(')').trim();
        let val = val.strip_prefix('"').and_then(|v| v.strip_suffix('"')).map(|v| v.replace("\\\"", "\"").replace("\\\\", "\\")).unwrap_or_else(|| val.to_string());
        m.insert(name.to_string(), val);
    }
    m
}

fn read_prefs(p: &Prof) -> BrowserPrefs {
    if p.mozilla {
        let m = moz_prefs(&std::fs::read_to_string(p.dir.join("prefs.js")).unwrap_or_default());
        let get = |k: &str| m.get(k).map(|s| s.as_str());
        BrowserPrefs {
            home: get("browser.startup.homepage").unwrap_or("about:home").split('|').map(String::from).collect(),
            clear_on_exit: get("privacy.sanitize.sanitizeOnShutdown") == Some("true"),
            animations: get("image.animation_mode") != Some("none"),
            caret: get("accessibility.browsewithcaret") == Some("true"),
            dnt: get("privacy.donottrackheader.enabled") == Some("true"),
            pictures: get("permissions.default.image") != Some("2"),
        }
    } else {
        let v: serde_json::Value = std::fs::read_to_string(p.dir.join("Preferences")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let urls: Vec<String> = v["session"]["startup_urls"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
        let home = if v["session"]["restore_on_startup"].as_i64() == Some(4) && !urls.is_empty() { urls }
            else { vec![v["homepage"].as_str().unwrap_or("chrome://newtab/").to_string()] };
        BrowserPrefs { home, clear_on_exit: false, animations: true, caret: false, dnt: v["enable_do_not_track"].as_bool().unwrap_or(false), pictures: true }
    }
}

fn js_str(s: &str) -> String { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) }

/// Save while the browser is closed: prefs.js (Firefox reads it at start and keeps changes the
/// user makes there later) or Chromium's Preferences.
/// Only what was changed here (`was` = as read) is written, so the browser's other settings stay.
fn write_prefs(p: &Prof, was: &BrowserPrefs, b: &BrowserPrefs) -> Result<(), String> {
    if dry_run() { eprintln!("[aerode dry run] browser settings for {}: {b:?}", p.dir.display()); return Ok(()); }
    let home = if b.home.is_empty() { vec!["about:blank".to_string()] } else { b.home.clone() };
    if p.mozilla {
        let f = p.dir.join("prefs.js");
        let old = std::fs::read_to_string(&f).unwrap_or_default();
        let mut set: Vec<(&str, String)> = vec![];
        if b.home != was.home { set.push(("browser.startup.homepage", js_str(&home.join("|")))); set.push(("browser.startup.page", "1".into())); }
        if b.clear_on_exit != was.clear_on_exit {
            for k in ["privacy.sanitize.sanitizeOnShutdown", "privacy.clearOnShutdown.history", "privacy.clearOnShutdown.cookies", "privacy.clearOnShutdown.cache", "privacy.clearOnShutdown.formdata", "privacy.clearOnShutdown.downloads"] {
                set.push((k, b.clear_on_exit.to_string()));
            }
        }
        if b.animations != was.animations { set.push(("image.animation_mode", js_str(if b.animations { "normal" } else { "none" }))); }
        if b.caret != was.caret { set.push(("accessibility.browsewithcaret", b.caret.to_string())); }
        if b.dnt != was.dnt { set.push(("privacy.donottrackheader.enabled", b.dnt.to_string())); }
        if b.pictures != was.pictures { set.push(("permissions.default.image", if b.pictures { "1".into() } else { "2".into() })); }
        let ours = |l: &str| set.iter().any(|(k, _)| l.trim().starts_with(&format!("user_pref(\"{k}\",")));
        let mut lines: Vec<String> = old.lines().filter(|l| !ours(l)).map(String::from).collect();
        for (k, v) in &set { lines.push(format!("user_pref(\"{k}\", {v});")); }
        std::fs::write(&f, lines.join("\n") + "\n").map_err(|e| e.to_string())
    } else {
        let f = p.dir.join("Preferences");
        let mut v: serde_json::Value = std::fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str(&s).ok()).ok_or("The browser's Preferences file can't be read.")?;
        v["homepage"] = serde_json::json!(home[0]);
        v["homepage_is_newtabpage"] = serde_json::json!(home[0].starts_with("chrome://newtab"));
        v["session"]["restore_on_startup"] = serde_json::json!(4);
        v["session"]["startup_urls"] = serde_json::json!(home);
        v["enable_do_not_track"] = serde_json::json!(b.dnt);
        std::fs::write(&f, serde_json::to_string(&v).unwrap_or_default()).map_err(|e| e.to_string())
    }
}

/// "Use current": the page last visited in Firefox, from its history (its session file is
/// compressed; Chromium's is binary, so nothing is offered for it).
fn current_pages(p: &Prof) -> Vec<String> {
    if !p.mozilla { return vec![]; }
    let db = p.dir.join("places.sqlite");
    if !db.exists() || !util::which("sqlite3") { return vec![]; }
    // a copy: the browser keeps the database locked while it runs
    let tmp = std::env::temp_dir().join(format!("aerode-places-{}.sqlite", std::process::id()));
    if std::fs::copy(&db, &tmp).is_err() { return vec![]; }
    let o = exec(&sv(&["sqlite3", &tmp.to_string_lossy(), "SELECT url FROM moz_places WHERE url LIKE 'http%' ORDER BY last_visit_date DESC LIMIT 1"]), None);
    let _ = std::fs::remove_file(&tmp);
    o.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

// ------------------------------------------------------------------ Connections + proxy

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Debug)]
#[serde(default)]
struct Proxy {
    detect: bool,
    pac: bool,
    pac_url: String,
    manual: bool,
    /// HTTP, Secure, FTP, Socks: (host, port)
    http: (String, String),
    https: (String, String),
    ftp: (String, String),
    socks: (String, String),
    same: bool,
    bypass_local: bool,
    /// "Do not use proxy server for addresses beginning with:" (semicolon separated)
    exceptions: String,
}

fn proxy_file() -> PathBuf { config::conf_dir().join("proxy.json") }
fn env_dir() -> PathBuf { glib::user_config_dir().join("environment.d") }

fn split_url(u: &str) -> (String, String) {
    let u = u.trim().trim_end_matches('/');
    let no_scheme = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
    let no_auth = no_scheme.rsplit_once('@').map(|(_, r)| r).unwrap_or(no_scheme);
    match no_auth.rsplit_once(':') { Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), p.to_string()), _ => (no_auth.to_string(), String::new()) }
}

fn load_proxy() -> Proxy {
    if let Some(p) = std::fs::read_to_string(proxy_file()).ok().and_then(|s| serde_json::from_str(&s).ok()) { return p; }
    // nothing saved by us yet: take what the desktop or the environment says
    let mut p = Proxy { bypass_local: true, ..Default::default() };
    let gs = |schema: &str, key: &str| util::output(&["gsettings", "get", schema, key]).map(|s| s.trim().trim_matches('\'').to_string()).unwrap_or_default();
    if util::which("gsettings") && util::output(&["gsettings", "list-keys", "org.gnome.system.proxy"]).is_some() {
        match gs("org.gnome.system.proxy", "mode").as_str() {
            "manual" => p.manual = true,
            "auto" => { p.pac_url = gs("org.gnome.system.proxy", "autoconfig-url"); if p.pac_url.is_empty() { p.detect = true; } else { p.pac = true; } }
            _ => {}
        }
        for (k, slot) in [("http", 0), ("https", 1), ("ftp", 2), ("socks", 3)] {
            let schema = format!("org.gnome.system.proxy.{k}");
            let h = gs(&schema, "host");
            let port = gs(&schema, "port");
            let v = (h, if port == "0" { String::new() } else { port });
            match slot { 0 => p.http = v, 1 => p.https = v, 2 => p.ftp = v, _ => p.socks = v }
        }
        if p.manual || p.pac || p.detect { return p; }
    }
    if let Ok(u) = std::env::var("http_proxy").or_else(|_| std::env::var("HTTP_PROXY")) {
        if !u.is_empty() { p.manual = true; p.http = split_url(&u); }
    }
    if let Ok(u) = std::env::var("https_proxy").or_else(|_| std::env::var("HTTPS_PROXY")) { if !u.is_empty() { p.https = split_url(&u); } }
    if let Ok(u) = std::env::var("all_proxy").or_else(|_| std::env::var("ALL_PROXY")) { if !u.is_empty() { p.manual = true; p.socks = split_url(&u); } }
    p.same = p.manual && p.https == p.http;
    p
}

impl Proxy {
    fn servers(&self) -> [(&'static str, (String, String)); 4] {
        let pick = |x: &(String, String)| if self.same { self.http.clone() } else { x.clone() };
        [("http", self.http.clone()), ("https", pick(&self.https)), ("ftp", pick(&self.ftp)), ("socks", if self.same { (String::new(), String::new()) } else { self.socks.clone() })]
    }
    fn exceptions(&self) -> Vec<String> {
        let mut v: Vec<String> = self.exceptions.split([';', ',']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if self.bypass_local { for l in ["localhost", "127.0.0.0/8", "::1"] { if !v.iter().any(|x| x == l) { v.insert(0, l.to_string()); } } }
        v
    }
}

/// Write the proxy everywhere programs look for it.
fn apply_proxy(p: &Proxy) {
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(proxy_file(), serde_json::to_string_pretty(p).unwrap_or_default());
    let servers = p.servers();
    let url = |scheme: &str, (h, port): &(String, String)| -> Option<String> {
        if h.is_empty() { return None; }
        let base = if h.contains("://") { h.clone() } else { format!("{scheme}://{h}") };
        Some(if port.is_empty() { format!("{base}/") } else { format!("{base}:{port}/") })
    };
    // 1. environment.d (systemd user services, and the aerode session at the next logon)
    let env = env_dir().join("proxy.conf");
    let mut vars: Vec<(String, String)> = vec![];
    if p.manual {
        if let Some(u) = url("http", &servers[0].1) { vars.push(("http_proxy".into(), u)); }
        if let Some(u) = url("http", &servers[1].1) { vars.push(("https_proxy".into(), u)); }
        if let Some(u) = url("http", &servers[2].1) { vars.push(("ftp_proxy".into(), u)); }
        if let Some(u) = url("socks5", &servers[3].1) { vars.push(("all_proxy".into(), u.trim_end_matches('/').to_string())); }
        if !vars.is_empty() { vars.push(("no_proxy".into(), p.exceptions().join(","))); }
    }
    let all_keys = ["http_proxy", "https_proxy", "ftp_proxy", "all_proxy", "no_proxy"];
    if vars.is_empty() { let _ = std::fs::remove_file(&env); } else {
        let _ = std::fs::create_dir_all(env_dir());
        let mut s = String::from("# written by aerode (Control Panel > Internet Options > Connections > LAN settings)\n");
        for (k, v) in &vars { s.push_str(&format!("{k}={v}\n{}={v}\n", k.to_uppercase())); }
        let _ = std::fs::write(&env, s);
    }
    let mut cmds: Vec<Vec<String>> = vec![];
    // 2. the running systemd user manager (D-Bus activated programs started from now on)
    if util::which("systemctl") {
        let mut unset = sv(&["systemctl", "--user", "unset-environment"]);
        for k in all_keys { unset.push(k.into()); unset.push(k.to_uppercase()); }
        cmds.push(unset);
        if !vars.is_empty() {
            let mut set = sv(&["systemctl", "--user", "set-environment"]);
            for (k, v) in &vars { set.push(format!("{k}={v}")); set.push(format!("{}={v}", k.to_uppercase())); }
            cmds.push(set);
        }
    }
    // 3. GNOME / GTK programs (GSettings) – also what Chromium and Firefox read with "system proxy"
    let mode = if p.pac && !p.pac_url.is_empty() { "auto" } else if p.detect { "auto" } else if p.manual { "manual" } else { "none" };
    if util::which("gsettings") && util::output(&["gsettings", "list-keys", "org.gnome.system.proxy"]).is_some() {
        let g = |schema: &str, key: &str, val: &str| sv(&["gsettings", "set", schema, key, val]);
        cmds.push(g("org.gnome.system.proxy", "mode", mode));
        cmds.push(g("org.gnome.system.proxy", "autoconfig-url", if p.pac { &p.pac_url } else { "" }));
        cmds.push(g("org.gnome.system.proxy", "use-same-proxy", if p.same { "true" } else { "false" }));
        let ign = format!("[{}]", p.exceptions().iter().map(|e| format!("'{}'", e.replace('\'', ""))).collect::<Vec<_>>().join(", "));
        cmds.push(g("org.gnome.system.proxy", "ignore-hosts", &ign));
        for (k, (h, port)) in &servers {
            let schema = format!("org.gnome.system.proxy.{k}");
            cmds.push(g(&schema, "host", h));
            cmds.push(g(&schema, "port", if port.is_empty() { "0" } else { port }));
        }
    }
    // 4. KDE programs (kioslaverc)
    if let Some(kw) = ["kwriteconfig6", "kwriteconfig5"].into_iter().find(|k| util::which(k)) {
        let kde_type = if p.pac && !p.pac_url.is_empty() { "2" } else if p.detect { "3" } else if p.manual { "1" } else { "0" };
        let w = |key: &str, val: &str| sv(&[kw, "--file", "kioslaverc", "--group", "Proxy Settings", "--key", key, val]);
        cmds.push(w("ProxyType", kde_type));
        cmds.push(w("Proxy Config Script", if p.pac { &p.pac_url } else { "" }));
        for (k, (h, port)) in &servers {
            let scheme = if *k == "socks" { "socks" } else { "http" };
            let v = if h.is_empty() { String::new() } else if h.contains("://") { format!("{h} {port}") } else { format!("{scheme}://{h} {port}") };
            cmds.push(w(&format!("{k}Proxy"), v.trim()));
        }
        cmds.push(w("NoProxyFor", &p.exceptions().join(",")));
        if util::which("dbus-send") { cmds.push(sv(&["dbus-send", "--type=signal", "/KIO/Scheduler", "org.kde.KIO.Scheduler.reparseSlaveConfiguration", "string:"])); }
    }
    background(move || { for c in &cmds { exec(c, None); } }, |_| {});
}

fn lan_settings(parent: &gtk::Window, st: &Rc<State>) {
    let p = st.proxy.borrow().clone();
    let d = gtk::Dialog::with_buttons(Some("Local Area Network (LAN) Settings"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_resizable(false);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(10);
    let ac = vbox(6);
    ac.pack_start(&label("Automatic configuration may override manual settings. To ensure the use of manual settings, disable automatic configuration."), false, false, 0);
    let detect = gtk::CheckButton::with_mnemonic("Automatically _detect settings");
    detect.set_active(p.detect);
    ac.pack_start(&detect, false, false, 0);
    let pac = gtk::CheckButton::with_mnemonic("Use automatic configuration _script");
    pac.set_active(p.pac);
    ac.pack_start(&pac, false, false, 0);
    let ar = hbox(8);
    ar.set_margin_start(20);
    ar.pack_start(&gtk::Label::new(Some("Address")), false, false, 0);
    let pac_url = gtk::Entry::new();
    pac_url.set_text(&p.pac_url);
    ar.pack_start(&pac_url, true, true, 0);
    ac.pack_start(&ar, false, false, 0);
    c.pack_start(&group("Automatic configuration", &ac), false, false, 0);
    let ps = vbox(6);
    let manual = gtk::CheckButton::with_mnemonic("Use a pro_xy server for your LAN (These settings will not apply to dial-up or VPN connections).");
    if let Some(l) = manual.child().and_then(|c| c.downcast::<gtk::Label>().ok()) { l.set_line_wrap(true); l.set_max_width_chars(46); }
    manual.set_active(p.manual);
    ps.pack_start(&manual, false, false, 0);
    let pr = hbox(6);
    pr.set_margin_start(20);
    pr.pack_start(&gtk::Label::new(Some("Address:")), false, false, 0);
    let host = gtk::Entry::new();
    host.set_text(&p.http.0);
    host.set_width_chars(18);
    pr.pack_start(&host, true, true, 0);
    pr.pack_start(&gtk::Label::new(Some("Port:")), false, false, 0);
    let port = gtk::Entry::new();
    port.set_text(if p.http.1.is_empty() && p.http.0.is_empty() { "80" } else { &p.http.1 });
    port.set_width_chars(5);
    pr.pack_start(&port, false, false, 0);
    let adv = gtk::Button::with_mnemonic("Ad_vanced");
    pr.pack_start(&adv, false, false, 0);
    ps.pack_start(&pr, false, false, 0);
    let bypass = gtk::CheckButton::with_mnemonic("_Bypass proxy server for local addresses");
    bypass.set_margin_start(20);
    bypass.set_active(p.bypass_local);
    ps.pack_start(&bypass, false, false, 0);
    c.pack_start(&group("Proxy server", &ps), false, false, 0);
    c.pack_start(&grey("Browsers and GNOME and KDE programs use the new settings right away; other programs after you log off and log on again."), false, false, 0);
    let sens = { let (pac, pac_url, manual, pr, bypass) = (pac.clone(), pac_url.clone(), manual.clone(), pr.clone(), bypass.clone()); move || {
        pac_url.set_sensitive(pac.is_active());
        pr.set_sensitive(manual.is_active());
        bypass.set_sensitive(manual.is_active());
    } };
    sens();
    { let s = sens.clone(); pac.connect_toggled(move |_| s()); }
    { let s = sens.clone(); manual.connect_toggled(move |_| s()); }
    // the Advanced dialog edits a working copy shared with this one
    let work = Rc::new(RefCell::new(p.clone()));
    {
        let (d2, work, host, port) = (d.clone(), work.clone(), host.clone(), port.clone());
        adv.connect_clicked(move |_| {
            work.borrow_mut().http = (host.text().trim().to_string(), port.text().trim().to_string());
            if let Some(n) = proxy_advanced(d2.upcast_ref(), &work.borrow()) {
                host.set_text(&n.http.0);
                port.set_text(&n.http.1);
                *work.borrow_mut() = n;
            }
        });
    }
    d.show_all();
    let r = d.run();
    if r == gtk::ResponseType::Ok {
        let mut n = work.borrow().clone();
        n.detect = detect.is_active();
        n.pac = pac.is_active();
        n.pac_url = pac_url.text().trim().to_string();
        n.manual = manual.is_active();
        n.http = (host.text().trim().to_string(), port.text().trim().to_string());
        n.bypass_local = bypass.is_active();
        *st.proxy.borrow_mut() = n;
    }
    d.close();
}

/// "Proxy Settings": a server per protocol and the exceptions.
fn proxy_advanced(parent: &gtk::Window, p: &Proxy) -> Option<Proxy> {
    let d = gtk::Dialog::with_buttons(Some("Proxy Settings"), Some(parent), gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_resizable(false);
    let c = d.content_area();
    c.set_border_width(10);
    c.set_spacing(10);
    let g = gtk::Grid::new();
    g.set_row_spacing(6);
    g.set_column_spacing(6);
    for (i, t) in ["Type", "Proxy address to use", "", "Port"].iter().enumerate() {
        let l = gtk::Label::new(Some(t));
        l.set_xalign(0.0);
        g.attach(&l, i as i32, 0, 1, 1);
    }
    let rows: Vec<(gtk::Entry, gtk::Entry)> = [("HTTP:", &p.http), ("Secure:", &p.https), ("FTP:", &p.ftp), ("Socks:", &p.socks)].iter().enumerate().map(|(i, (t, v))| {
        let l = gtk::Label::new(Some(t));
        l.set_xalign(0.0);
        let h = gtk::Entry::new();
        h.set_text(&v.0);
        h.set_width_chars(22);
        let port = gtk::Entry::new();
        port.set_text(&v.1);
        port.set_width_chars(5);
        g.attach(&l, 0, i as i32 + 1, 1, 1);
        g.attach(&h, 1, i as i32 + 1, 1, 1);
        g.attach(&gtk::Label::new(Some(":")), 2, i as i32 + 1, 1, 1);
        g.attach(&port, 3, i as i32 + 1, 1, 1);
        (h, port)
    }).collect();
    let sv_box = vbox(6);
    sv_box.pack_start(&g, false, false, 0);
    let same = gtk::CheckButton::with_mnemonic("_Use the same proxy server for all protocols");
    same.set_active(p.same);
    sv_box.pack_start(&same, false, false, 0);
    c.pack_start(&group("Servers", &sv_box), false, false, 0);
    let sens = { let (same, rows) = (same.clone(), rows.clone()); move || {
        for (i, (h, port)) in rows.iter().enumerate().skip(1) {
            h.set_sensitive(!same.is_active());
            port.set_sensitive(!same.is_active());
            if same.is_active() && i < 3 { h.set_text(&rows[0].0.text()); port.set_text(&rows[0].1.text()); }
            if same.is_active() && i == 3 { h.set_text(""); port.set_text(""); }
        }
    } };
    sens();
    { let s = sens.clone(); same.connect_toggled(move |_| s()); }
    let ex = vbox(6);
    ex.pack_start(&label("Do not use proxy server for addresses beginning with:"), false, false, 0);
    let tv = gtk::TextView::new();
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    tv.buffer().map(|b| b.set_text(&p.exceptions));
    ex.pack_start(&scrolled(&tv, 60), false, false, 0);
    ex.pack_start(&label("Use semicolons ( ; ) to separate entries."), false, false, 0);
    c.pack_start(&group("Exceptions", &ex), false, false, 0);
    d.show_all();
    let r = d.run();
    let out = (r == gtk::ResponseType::Ok).then(|| {
        let get = |i: usize| (rows[i].0.text().trim().to_string(), rows[i].1.text().trim().to_string());
        let text = tv.buffer().and_then(|b| b.text(&b.start_iter(), &b.end_iter(), false)).map(|s| s.to_string()).unwrap_or_default();
        Proxy { http: get(0), https: get(1), ftp: get(2), socks: get(3), same: same.is_active(), exceptions: text.trim().to_string(), ..p.clone() }
    });
    d.close();
    out
}

fn connections(cp: &Rc<Cp>, d: &gtk::Window, st: &Rc<State>) -> gtk::Widget {
    let v = vbox(10);
    v.set_border_width(10);
    let top = hbox(10);
    top.pack_start(&util::image(util::first_icon(&["network-workgroup", "preferences-system-network"]), 32), false, false, 0);
    top.pack_start(&label("To set up an Internet connection, click Setup."), true, true, 0);
    let setup = gtk::Button::with_mnemonic("Set_up");
    { let me = cp.clone(); setup.connect_clicked(move |_| open_cp(&me, "network")); }
    top.pack_end(&setup, false, false, 0);
    v.pack_start(&top, false, false, 0);
    // VPN and dial-up connections (NetworkManager)
    let dv = hbox(8);
    let (tv, store) = table(3, &["Name", "Type"]);
    tv.set_headers_visible(false);
    let fill_vpn = { let store = store.clone(); move || {
        store.clear();
        let st = store.clone();
        exec_async(sv(&["nmcli", "-t", "-f", "NAME,TYPE,UUID", "connection", "show"]), None, move |o| {
            for l in o.stdout.lines() {
                let f = aerode::sys::nm_split(l);
                if f.len() < 3 { continue; }
                let kind = match f[1].as_str() { "vpn" => "VPN", "wireguard" => "WireGuard VPN", "pppoe" => "Broadband (PPPoE)", "gsm" | "cdma" => "Mobile broadband", "bluetooth" => "Bluetooth", _ => continue };
                st.insert_with_values(None, &[(0, &f[0]), (1, &kind.to_string()), (2, &f[2])]);
            }
        });
    } };
    fill_vpn();
    dv.pack_start(&scrolled(&tv, 90), true, true, 0);
    let bs = vbox(4);
    let editor = util::which("nm-connection-editor");
    let (add, addvpn, remove, settings) = (gtk::Button::with_mnemonic("A_dd..."), gtk::Button::with_mnemonic("Add V_PN..."), gtk::Button::with_mnemonic("_Remove..."), gtk::Button::with_mnemonic("_Settings"));
    for b in [&add, &addvpn, &remove, &settings] { bs.pack_start(b, false, false, 0); }
    add.set_sensitive(editor);
    addvpn.set_sensitive(editor);
    // Remove and Settings act on the selected connection
    remove.set_sensitive(false);
    settings.set_sensitive(false);
    { let (remove, settings) = (remove.clone(), settings.clone()); tv.selection().connect_changed(move |s| {
        let any = s.selected().is_some();
        remove.set_sensitive(any);
        settings.set_sensitive(any && editor);
    }); }
    add.connect_clicked(|_| { util::spawn(&["nm-connection-editor", "--create", "--type=pppoe"]); });
    addvpn.connect_clicked(|_| { util::spawn(&["nm-connection-editor", "--create", "--type=vpn"]); });
    { let tv = tv.clone(); settings.connect_clicked(move |_| if let Some(u) = selected(&tv, 2) { util::spawn(&["nm-connection-editor", &format!("--edit={u}")]); }); }
    {
        let (tv, d, fill_vpn) = (tv.clone(), d.clone(), fill_vpn.clone());
        remove.connect_clicked(move |_| {
            let (Some(n), Some(u)) = (selected(&tv, 0), selected(&tv, 2)) else { return };
            if !ask(Some(&d), gtk::MessageType::Warning, "Internet Properties", &format!("Are you sure you want to remove \"{n}\"?"), "", "_Yes", "_No") { return; }
            let f = fill_vpn.clone();
            change(sv(&["nmcli", "connection", "delete", "uuid", &u]), None, move |_| f());
        });
    }
    dv.pack_start(&bs, false, false, 0);
    let dvv = vbox(6);
    dvv.pack_start(&dv, false, false, 0);
    if !editor { dvv.pack_start(&grey("Install nm-connection-editor to add and change these connections."), false, false, 0); }
    v.pack_start(&group("Dial-up and Virtual Private Network settings", &dvv), false, false, 0);
    let lan = hbox(8);
    lan.pack_start(&label("LAN Settings do not apply to dial-up connections. Choose Settings above for dial-up settings."), true, true, 0);
    let lb = gtk::Button::with_mnemonic("_LAN settings");
    lb.set_valign(gtk::Align::End);
    { let (d, st) = (d.clone(), st.clone()); lb.connect_clicked(move |_| lan_settings(&d, &st)); }
    lan.pack_end(&lb, false, false, 0);
    v.pack_start(&group("Local Area Network (LAN) settings", &lan), false, false, 0);
    v.upcast()
}

// ------------------------------------------------------------------ Programs

/// A combo of the programs that handle a type, the default selected.
fn app_combo(ty: &str) -> (gtk::ComboBoxText, Option<String>) {
    let c = gtk::ComboBoxText::new();
    let def = gio::AppInfo::default_for_type(ty, false).and_then(|a| a.id()).map(|s| s.to_string());
    for a in gio::AppInfo::all_for_type(ty) { if let Some(id) = a.id() { c.append(Some(&id), &a.display_name()); } }
    if let Some(d) = &def {
        // a default from mimeapps.list that the MIME cache doesn't list
        if !c.set_active_id(Some(d)) {
            if let Some(a) = util::app_by_id(d) { c.append(Some(d), &a.display_name()); c.set_active_id(Some(d)); }
        }
    }
    (c, def)
}

fn set_default(id: &str, types: &[&str], browser: bool) {
    if browser && util::which("xdg-settings") {
        let id = id.to_string();
        change(sv(&["xdg-settings", "set", "default-web-browser", &id]), None, |_| {});
        return;
    }
    if let Some(app) = util::app_by_id(id) { for t in types { let _ = app.set_as_default_for_type(t); } }
}

fn programs(cp: &Rc<Cp>) -> gtk::Widget {
    let v = vbox(10);
    v.set_border_width(10);
    let wb = vbox(6);
    let (combo, def) = app_combo("x-scheme-handler/http");
    let status = label("");
    let upd = { let status = status.clone(); move |id: Option<String>| {
        let n = id.and_then(|i| util::app_by_id(&i)).map(|a| a.display_name().to_string());
        status.set_text(&match n { Some(n) => format!("{n} is your default web browser."), None => "No default web browser is set.".into() });
    } };
    upd(def.clone());
    let row = hbox(8);
    row.pack_start(&combo, true, true, 0);
    let make = gtk::Button::with_mnemonic("_Make default");
    { let (combo, upd) = (combo.clone(), upd.clone()); make.connect_clicked(move |_| if let Some(id) = combo.active_id() {
        set_default(&id, &["x-scheme-handler/http", "x-scheme-handler/https", "text/html", "application/xhtml+xml"], true);
        upd(Some(id.to_string()));
    }); }
    row.pack_end(&make, false, false, 0);
    wb.pack_start(&row, false, false, 0);
    wb.pack_start(&status, false, false, 0);
    v.pack_start(&group("Default web browser", &wb), false, false, 0);

    let ma = hbox(8);
    ma.pack_start(&label("Enable and disable browser add-ons installed in your web browser."), true, true, 0);
    let mb = gtk::Button::with_mnemonic("Manage _add-ons");
    mb.connect_clicked(|_| browser_page("about:addons", "chrome://extensions"));
    ma.pack_end(&mb, false, false, 0);
    v.pack_start(&group("Manage add-ons", &ma), false, false, 0);

    let ip = vbox(6);
    ip.pack_start(&label("Choose the programs you want to use for other Internet services, such as e-mail."), false, false, 0);
    let er = hbox(8);
    er.pack_start(&gtk::Label::new(Some("E-mail:")), false, false, 0);
    let (mail, _) = app_combo("x-scheme-handler/mailto");
    er.pack_start(&mail, true, true, 0);
    mail.connect_changed(|c| if let Some(id) = c.active_id() { set_default(&id, &["x-scheme-handler/mailto"], false); });
    ip.pack_start(&er, false, false, 0);
    let sp = gtk::Button::with_mnemonic("Set _programs");
    sp.set_halign(gtk::Align::End);
    { let me = cp.clone(); sp.connect_clicked(move |_| open_cp(&me, "defaults")); }
    ip.pack_start(&sp, false, false, 0);
    v.pack_start(&group("Internet programs", &ip), false, false, 0);

    let fa = hbox(8);
    fa.pack_start(&label("Choose the file types that you want your web browser to open by default."), true, true, 0);
    let sa = gtk::Button::with_mnemonic("Set _associations");
    { let me = cp.clone(); sa.connect_clicked(move |_| open_cp(&me, "defaults")); }
    fa.pack_end(&sa, false, false, 0);
    v.pack_start(&group("File associations", &fa), false, false, 0);
    v.upcast()
}

// ------------------------------------------------------------------ Advanced

#[derive(Clone, Default, PartialEq, Debug)]
struct Advanced { software_render: bool, smooth: bool, keyring: bool }

/// The flags files Arch's Chromium-based browser launchers read.
fn flag_files() -> Vec<PathBuf> {
    ["chromium-flags.conf", "chrome-flags.conf", "brave-flags.conf", "vivaldi-stable.conf"].iter().map(|f| glib::user_config_dir().join(f)).collect()
}

fn keyring_flag() -> &'static str {
    if config::home().join(".local/share/kwalletd/kdewallet.kwl").exists() {
        if util::which("kwalletd6") || std::env::var("KDE_SESSION_VERSION").map(|v| v == "6").unwrap_or(false) { "--password-store=kwallet6" } else { "--password-store=kwallet5" }
    } else { "--password-store=gnome-libsecret" }
}

fn load_advanced() -> Advanced {
    let flags = std::fs::read_to_string(&flag_files()[0]).unwrap_or_default();
    let env = std::fs::read_to_string(env_dir().join("aerode-browser.conf")).unwrap_or_default();
    Advanced {
        software_render: flags.lines().any(|l| l.trim() == "--disable-gpu"),
        smooth: env.lines().any(|l| l.trim() == "MOZ_USE_XINPUT2=1") || flags.lines().any(|l| l.trim() == "--enable-smooth-scrolling"),
        keyring: flags.lines().any(|l| l.trim().starts_with("--password-store=")),
    }
}

fn apply_advanced(a: &Advanced) {
    let mut want: Vec<String> = vec![];
    if a.software_render { want.push("--disable-gpu".into()); }
    if a.smooth { want.push("--enable-smooth-scrolling".into()); }
    if a.keyring { want.push(keyring_flag().into()); }
    let ours = |l: &str| { let l = l.trim(); l == "--disable-gpu" || l == "--enable-smooth-scrolling" || l.starts_with("--password-store=") };
    for (i, f) in flag_files().into_iter().enumerate() {
        let old = std::fs::read_to_string(&f).unwrap_or_default();
        if old.is_empty() && want.is_empty() { continue; }
        // only the first file is created; the others are kept in step when they exist
        if old.is_empty() && i > 0 && !f.exists() { continue; }
        let mut lines: Vec<String> = old.lines().filter(|l| !ours(l)).map(String::from).collect();
        lines.extend(want.iter().cloned());
        let _ = std::fs::write(&f, lines.join("\n") + "\n");
    }
    let env = env_dir().join("aerode-browser.conf");
    if a.smooth {
        let _ = std::fs::create_dir_all(env_dir());
        let _ = std::fs::write(&env, "# written by aerode (Control Panel > Internet Options > Advanced)\nMOZ_USE_XINPUT2=1\n");
    } else { let _ = std::fs::remove_file(&env); }
}

fn advanced(d: &gtk::Window, st: &Rc<State>) -> gtk::Widget {
    let v = vbox(8);
    v.set_border_width(10);
    v.pack_start(&gtk::Label::new(Some("Settings")).tap(|l| l.set_xalign(0.0)), false, false, 0);
    // columns: checked, is a setting (not a category), text, key, can be changed for this browser
    let store = gtk::TreeStore::new(&[glib::Type::BOOL, glib::Type::BOOL, glib::Type::STRING, glib::Type::STRING, glib::Type::BOOL]);
    let tv = gtk::TreeView::with_model(&store);
    tv.set_headers_visible(false);
    // Win7: categories are plain headings (always open), settings indented below them
    tv.set_show_expanders(false);
    tv.set_level_indentation(18);
    let col = gtk::TreeViewColumn::new();
    let tog = gtk::CellRendererToggle::new();
    TreeViewColumnExt::pack_start(&col, &tog, false);
    TreeViewColumnExt::add_attribute(&col, &tog, "active", 0);
    TreeViewColumnExt::add_attribute(&col, &tog, "visible", 1);
    TreeViewColumnExt::add_attribute(&col, &tog, "activatable", 4);
    TreeViewColumnExt::add_attribute(&col, &tog, "sensitive", 4);
    let txt = gtk::CellRendererText::new();
    TreeViewColumnExt::pack_start(&col, &txt, true);
    TreeViewColumnExt::add_attribute(&col, &txt, "text", 2);
    TreeViewColumnExt::add_attribute(&col, &txt, "sensitive", 4);
    tv.append_column(&col);
    let fill = { let (store, st, tv) = (store.clone(), st.clone(), tv.clone()); move || {
        store.clear();
        let a = st.adv.borrow().clone();
        let b = st.browser.borrow().clone();
        // the default browser's own settings: Firefox's prefs (Do Not Track also for Chromium)
        let moz = st.prof.as_ref().map(|p| p.mozilla).unwrap_or(false);
        let chrome = st.prof.as_ref().map(|p| !p.mozilla).unwrap_or(false);
        let groups: [(&str, Vec<(&str, &str, bool, bool)>); 5] = [
            ("Accelerated graphics", vec![("gpu", "Use software rendering instead of GPU rendering*", a.software_render, true)]),
            ("Accessibility", vec![("caret", "Move system caret with focus/selection changes (caret browsing)", b.caret, moz)]),
            ("Browsing", vec![("smooth", "Enable smooth scrolling*", a.smooth, true)]),
            ("Multimedia", vec![("anim", "Play animations in webpages*", b.animations, moz), ("pics", "Show pictures", b.pictures, moz)]),
            ("Security", vec![
                ("dnt", "Send Do Not Track requests to sites you visit", b.dnt, moz || chrome),
                ("keyring", "Save passwords in Credential Manager (system keyring) for Chromium-based browsers*", a.keyring, true),
            ]),
        ];
        for (g, items) in groups {
            let it = store.insert_with_values(None, None, &[(0, &false), (1, &false), (2, &g), (3, &""), (4, &true)]);
            for (k, t, on, can) in items { store.insert_with_values(Some(&it), None, &[(0, &on), (1, &true), (2, &t), (3, &k), (4, &can)]); }
        }
        tv.expand_all();
    } };
    fill();
    {
        let (store, st) = (store.clone(), st.clone());
        tog.connect_toggled(move |_, path| {
            let Some(it) = store.iter(&path) else { return };
            let on = !store.value(&it, 0).get::<bool>().unwrap_or(false);
            store.set_value(&it, 0, &on.to_value());
            let key: String = store.value(&it, 3).get().unwrap_or_default();
            let (mut a, mut b) = (st.adv.borrow_mut(), st.browser.borrow_mut());
            match key.as_str() {
                "gpu" => a.software_render = on, "smooth" => a.smooth = on, "keyring" => a.keyring = on,
                "caret" => b.caret = on, "anim" => b.animations = on, "pics" => b.pictures = on, "dnt" => b.dnt = on,
                _ => {}
            }
        });
    }
    v.pack_start(&scrolled(&tv, 230), true, true, 0);
    v.pack_start(&label("*Takes effect after you restart your web browser"), false, false, 0);
    let restore = gtk::Button::with_mnemonic("Restore advanced _settings");
    restore.set_halign(gtk::Align::End);
    { let (st, fill) = (st.clone(), fill.clone()); restore.connect_clicked(move |_| {
        *st.adv.borrow_mut() = Advanced::default();
        { let mut b = st.browser.borrow_mut(); b.caret = false; b.animations = true; b.pictures = true; b.dnt = false; }
        fill();
    }); }
    v.pack_start(&restore, false, false, 0);
    let rs = vbox(6);
    rs.pack_start(&label("Resets the proxy and browser settings made here to their default condition."), false, false, 0);
    let rr = hbox(8);
    rr.pack_start(&label("You should only use this if your web browser can't connect or is in an unusable state."), true, true, 0);
    let reset = gtk::Button::with_mnemonic("R_eset...");
    rr.pack_end(&reset, false, false, 0);
    rs.pack_start(&rr, false, false, 0);
    {
        let (d, st, fill) = (d.clone(), st.clone(), fill.clone());
        reset.connect_clicked(move |_| {
            if !ask(Some(&d), gtk::MessageType::Warning, "Reset Internet Settings", "Are you sure you want to reset all Internet settings?",
                "The proxy server is turned off and the browser settings made on the Advanced tab are removed. Browsers' own settings, bookmarks and passwords are not changed.", "_Reset", "Cancel") { return; }
            *st.proxy.borrow_mut() = Proxy { bypass_local: true, ..Default::default() };
            *st.adv.borrow_mut() = Advanced::default();
            apply_proxy(&st.proxy.borrow());
            apply_advanced(&st.adv.borrow());
            fill();
            message(Some(&d), gtk::MessageType::Info, "Reset Internet Settings", "Internet settings were reset.", "Restart your web browser, and log off and on again, for the changes to take effect.");
        });
    }
    v.pack_start(&group("Reset Internet settings", &rs), false, false, 0);
    v.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls() {
        assert_eq!(split_url("http://proxy.lan:3128/"), ("proxy.lan".to_string(), "3128".to_string()));
        assert_eq!(split_url("socks5://user:pw@h:1080"), ("h".to_string(), "1080".to_string()));
        assert_eq!(split_url("proxy"), ("proxy".to_string(), String::new()));
        let p = Proxy { bypass_local: true, exceptions: "*.corp; 10.0.0.0/8".into(), ..Default::default() };
        assert_eq!(p.exceptions(), vec!["::1", "127.0.0.0/8", "localhost", "*.corp", "10.0.0.0/8"]);
    }
    #[test]
    fn firefox_profile_and_prefs() {
        let ini = "[Install4F96D1932A9F858E]\nDefault=abcd.default-release\nLocked=1\n\n[Profile1]\nName=old\nIsRelative=1\nPath=zzzz.default\nDefault=1\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=abcd.default-release\n";
        assert_eq!(mozilla_default(ini), Some(PathBuf::from("abcd.default-release")));
        assert_eq!(mozilla_default("[Profile0]\nPath=only.default\n"), Some(PathBuf::from("only.default")));
        assert_eq!(mozilla_default("[Profile0]\nPath=a\n[Profile1]\nPath=b\nDefault=1\n"), Some(PathBuf::from("b")));
        let m = moz_prefs("// x\nuser_pref(\"browser.startup.homepage\", \"https://a.org/|https://b.org/\");\nuser_pref(\"image.animation_mode\", \"none\");\nuser_pref(\"permissions.default.image\", 2);\nuser_pref(\"x.quote\", \"say \\\"hi\\\"\");\n");
        assert_eq!(m.get("browser.startup.homepage").map(|s| s.as_str()), Some("https://a.org/|https://b.org/"));
        assert_eq!(m.get("permissions.default.image").map(|s| s.as_str()), Some("2"));
        assert_eq!(m.get("x.quote").map(|s| s.as_str()), Some("say \"hi\""));
        assert_eq!(js_str("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
