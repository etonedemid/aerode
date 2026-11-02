//! Region and Language (intl.cpl): Formats (the format locale with live examples of its short
//! and long date and time, Additional settings… for numbers and currency; saved as the LC_*
//! lines of ~/.config/locale.conf, short time also drives the taskbar clock), Location (the
//! country, config.json), Keyboards and Languages (Change keyboards… = Text Services and Input
//! Languages; the display language = LANG) and Administrative (copy the settings to the
//! Welcome screen and new user accounts, the system locale with `pkexec localectl`).
use super::*;

/// English names of the languages glibc has locales for.
const LANGS: &[(&str, &str)] = &[
    ("af", "Afrikaans"), ("am", "Amharic"), ("ar", "Arabic"), ("as", "Assamese"), ("az", "Azerbaijani"), ("be", "Belarusian"), ("bg", "Bulgarian"), ("bn", "Bengali"),
    ("bo", "Tibetan"), ("br", "Breton"), ("bs", "Bosnian"), ("ca", "Catalan"), ("cs", "Czech"), ("cy", "Welsh"), ("da", "Danish"), ("de", "German"), ("el", "Greek"),
    ("en", "English"), ("eo", "Esperanto"), ("es", "Spanish"), ("et", "Estonian"), ("eu", "Basque"), ("fa", "Persian"), ("fi", "Finnish"), ("fil", "Filipino"),
    ("fo", "Faroese"), ("fr", "French"), ("fy", "Frisian"), ("ga", "Irish"), ("gd", "Scottish Gaelic"), ("gl", "Galician"), ("gu", "Gujarati"), ("he", "Hebrew"),
    ("hi", "Hindi"), ("hr", "Croatian"), ("hu", "Hungarian"), ("hy", "Armenian"), ("id", "Indonesian"), ("is", "Icelandic"), ("it", "Italian"), ("ja", "Japanese"),
    ("ka", "Georgian"), ("kk", "Kazakh"), ("km", "Khmer"), ("kn", "Kannada"), ("ko", "Korean"), ("ku", "Kurdish"), ("ky", "Kyrgyz"), ("lb", "Luxembourgish"),
    ("lo", "Lao"), ("lt", "Lithuanian"), ("lv", "Latvian"), ("mk", "Macedonian"), ("ml", "Malayalam"), ("mn", "Mongolian"), ("mr", "Marathi"), ("ms", "Malay"),
    ("mt", "Maltese"), ("my", "Burmese"), ("nb", "Norwegian, Bokmål"), ("ne", "Nepali"), ("nl", "Dutch"), ("nn", "Norwegian, Nynorsk"), ("oc", "Occitan"),
    ("or", "Oriya"), ("pa", "Punjabi"), ("pl", "Polish"), ("ps", "Pashto"), ("pt", "Portuguese"), ("ro", "Romanian"), ("ru", "Russian"), ("rw", "Kinyarwanda"),
    ("se", "Sami, Northern"), ("si", "Sinhala"), ("sk", "Slovak"), ("sl", "Slovenian"), ("sq", "Albanian"), ("sr", "Serbian"), ("sv", "Swedish"), ("sw", "Kiswahili"),
    ("ta", "Tamil"), ("te", "Telugu"), ("tg", "Tajik"), ("th", "Thai"), ("tk", "Turkmen"), ("tr", "Turkish"), ("tt", "Tatar"), ("ug", "Uyghur"), ("uk", "Ukrainian"),
    ("ur", "Urdu"), ("uz", "Uzbek"), ("vi", "Vietnamese"), ("wo", "Wolof"), ("xh", "isiXhosa"), ("yi", "Yiddish"), ("yo", "Yoruba"), ("zh", "Chinese"), ("zu", "isiZulu"),
];

/// ISO 3166 code → English country name (tzdata's table).
fn countries() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_to_string("/usr/share/zoneinfo/iso3166.tab").unwrap_or_default().lines()
        .filter(|l| !l.starts_with('#')).filter_map(|l| { let (c, n) = l.split_once('\t')?; Some((c.trim().to_string(), n.trim().to_string())) }).collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

/// "de_DE.UTF-8" → "German (Germany)".
pub fn locale_name(loc: &str, cc: &[(String, String)]) -> String {
    let base = loc.split(['.', '@']).next().unwrap_or(loc);
    if matches!(base, "C" | "POSIX" | "") { return "Neutral (C)".into(); }
    let (lang, country) = base.split_once('_').unwrap_or((base, ""));
    let l = LANGS.iter().find(|(c, _)| *c == lang).map(|x| x.1.to_string()).unwrap_or_else(|| lang.to_string());
    let c = cc.iter().find(|(c, _)| c == country).map(|x| x.1.clone()).unwrap_or_else(|| country.to_string());
    let mut s = if c.is_empty() { l } else { format!("{l} ({c})") };
    if let Some(m) = loc.split_once('@').map(|x| x.1) { s.push_str(&format!(" [{m}]")); }
    s
}

/// The UTF-8 locales this system has generated (`locale -a`), normalised to xx_YY.UTF-8.
fn generated() -> Vec<String> {
    let mut v: Vec<String> = sh(&["locale", "-a"]).lines().map(|l| l.trim()).filter(|l| l.contains('_') && (l.ends_with(".utf8") || l.ends_with(".UTF-8") || l.contains(".utf8@")))
        .map(|l| l.replace(".utf8", ".UTF-8")).collect();
    // locales compiled into $LOCPATH are usable too (glibc looks there first; `locale -a` doesn't list them)
    if let Some(lp) = std::env::var_os("LOCPATH") {
        for d in std::env::split_paths(&lp) {
            if let Ok(rd) = std::fs::read_dir(d) { for e in rd.flatten() { let n = e.file_name().to_string_lossy().replace(".utf8", ".UTF-8"); if n.contains('_') && n.contains(".UTF-8") { v.push(n); } } }
        }
    }
    v.sort();
    v.dedup();
    v
}

/// Every locale glibc can generate (/usr/share/i18n/SUPPORTED, UTF-8 only).
fn supported() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_to_string("/usr/share/i18n/SUPPORTED").unwrap_or_default().lines()
        .filter_map(|l| l.split_whitespace().next()).filter(|l| l.contains(".UTF-8") && l.contains('_')).map(|s| s.to_string()).collect();
    v.sort();
    v.dedup();
    v
}

/// LANG and the LC_* values in effect for us: ~/.config/locale.conf, else the environment.
fn user_locale() -> (String, String) {
    let conf = std::fs::read_to_string(glib::user_config_dir().join("locale.conf")).unwrap_or_default();
    let get = |k: &str| conf.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}="))).map(|s| s.trim_matches('"').to_string());
    let env = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
    let lang = get("LANG").or_else(|| env("LANG")).unwrap_or_else(|| "en_US.UTF-8".into());
    let fmt = get("LC_TIME").or_else(|| env("LC_TIME")).or_else(|| env("LC_ALL")).unwrap_or_else(|| lang.clone());
    (lang, fmt)
}

fn system_locale() -> String {
    let s = sh(&["localectl", "status"]);
    s.lines().find_map(|l| l.trim().strip_prefix("System Locale:")).and_then(|v| v.split_whitespace().find_map(|kv| kv.strip_prefix("LANG="))).map(|s| s.to_string())
        .or_else(|| std::fs::read_to_string("/etc/locale.conf").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("LANG=").map(|s| s.trim_matches('"').to_string()))))
        .unwrap_or_else(|| "C".into())
}

/// Write the format (LC_*) and/or display language (LANG) lines of ~/.config/locale.conf.
fn write_user_locale(lang: Option<&str>, fmt: Option<&str>) {
    let p = glib::user_config_dir().join("locale.conf");
    let cur = std::fs::read_to_string(&p).unwrap_or_default();
    let fmt_keys = ["LC_TIME", "LC_NUMERIC", "LC_MONETARY", "LC_MEASUREMENT", "LC_PAPER", "LC_ADDRESS", "LC_TELEPHONE", "LC_NAME"];
    let mut out: Vec<String> = cur.lines().filter(|l| {
        let k = l.split('=').next().unwrap_or("").trim();
        !(lang.is_some() && k == "LANG") && !(fmt.is_some() && fmt_keys.contains(&k))
    }).map(String::from).collect();
    if let Some(l) = lang { out.insert(0, format!("LANG={l}")); }
    if !out.iter().any(|l| l.starts_with("LANG=")) { out.insert(0, format!("LANG={}", user_locale().0)); }
    if let Some(f) = fmt { for k in fmt_keys { out.push(format!("{k}={f}")); } }
    let _ = std::fs::write(&p, out.join("\n") + "\n");
}

#[derive(Clone, Debug, Default)]
pub struct Formats { pub short_date: String, pub long_date: String, pub short_time: String, pub long_time: String, pub ex: [String; 4], pub first_day: String,
    pub decimal: String, pub group: String, pub currency: String, pub am: String, pub pm: String, pub measure: String }

/// strftime notation → Win7's (M/d/yyyy, dddd, MMMM dd, yyyy, h:mm tt).
pub fn win_notation(f: &str) -> String {
    let mut out = String::new();
    let mut it = f.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' { out.push(c); continue; }
        let mut flag = None;
        if matches!(it.peek(), Some('-') | Some('_') | Some('0') | Some('E') | Some('O')) { flag = it.next(); }
        let nopad = flag == Some('-');
        let s = match it.next() {
            Some('d') => if nopad { "d" } else { "dd" }, Some('e') => "d", Some('m') => if nopad { "M" } else { "MM" }, Some('y') => "yy", Some('Y') => "yyyy",
            Some('b') | Some('h') => "MMM", Some('B') => "MMMM", Some('a') => "ddd", Some('A') => "dddd",
            Some('H') => if nopad { "H" } else { "HH" }, Some('k') => "H", Some('I') => if nopad { "h" } else { "hh" }, Some('l') => "h",
            Some('M') => "mm", Some('S') => "ss", Some('p') | Some('P') => "tt", Some('Z') | Some('z') => "", Some('%') => "%",
            Some('D') => "MM/dd/yy", Some('F') => "yyyy-MM-dd", Some('T') => "HH:mm:ss", Some('R') => "HH:mm", Some('r') => "hh:mm:ss tt",
            Some(o) => { out.push('%'); out.push(o); "" }
            None => "",
        };
        out.push_str(s);
    }
    out.trim().to_string()
}

/// The locale's formats and examples, read with that locale active on this (worker) thread.
pub fn formats(loc: &str) -> Formats {
    let mut f = Formats::default();
    let Ok(cl) = std::ffi::CString::new(loc) else { return f };
    unsafe {
        let l = libc::newlocale(libc::LC_ALL_MASK, cl.as_ptr(), std::ptr::null_mut());
        if l.is_null() { return f; }
        let old = libc::uselocale(l);
        let info = |i: libc::nl_item| std::ffi::CStr::from_ptr(libc::nl_langinfo(i)).to_string_lossy().into_owned();
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        let fmt = |p: &str| -> String {
            let Ok(c) = std::ffi::CString::new(p) else { return String::new() };
            let mut buf = [0u8; 256];
            let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), c.as_ptr(), &tm);
            String::from_utf8_lossy(&buf[..n]).into_owned()
        };
        let d_fmt = info(libc::D_FMT);
        let ampm = !info(libc::AM_STR).is_empty();
        let t_fmt = if ampm { info(libc::T_FMT_AMPM) } else { info(libc::T_FMT) };
        // short date: the locale's; Win7 en-US drops leading zeros (M/d/yyyy)
        let short_d = if d_fmt == "%m/%d/%Y" || d_fmt == "%m/%d/%y" { "%-m/%-d/%Y".to_string() } else { d_fmt.replace("%y", "%Y") };
        let long_d = if d_fmt.starts_with("%m") { "%A, %B %d, %Y".to_string() } else if d_fmt.starts_with("%Y") || d_fmt.starts_with("%F") { "%Y %B %-d, %A".to_string() } else { "%A, %-d %B %Y".to_string() };
        let long_t = if ampm { t_fmt.replace("%I", "%-I") } else { t_fmt.clone() };
        let short_t = long_t.replace(":%S", "");
        f.short_date = win_notation(&short_d);
        f.long_date = win_notation(&long_d);
        f.short_time = win_notation(&short_t);
        f.long_time = win_notation(&long_t);
        f.ex = [fmt(&short_d), fmt(&long_d), fmt(&short_t), fmt(&long_t)];
        f.decimal = info(libc::RADIXCHAR);
        f.group = info(libc::THOUSEP);
        f.currency = info(libc::CRNCYSTR).trim_start_matches(['-', '+', '.']).to_string();
        f.am = info(libc::AM_STR);
        f.pm = info(libc::PM_STR);
        libc::uselocale(old);
        libc::freelocale(l);
    }
    let (_, first) = run(&["env", &format!("LC_ALL={loc}"), "locale", "first_weekday"]);
    f.first_day = match first.trim() { "2" => "Monday", "7" => "Saturday", _ => "Sunday" }.into();
    let (_, m) = run(&["env", &format!("LC_ALL={loc}"), "locale", "measurement"]);
    f.measure = if m.trim() == "2" { "U.S.".into() } else { "Metric".into() };
    f
}

impl Cp {
    pub(crate) fn page_region(self: &Rc<Self>) -> gtk::Widget {
        css();
        let sheet = Sheet::new(self);
        sheet.nb.set_size_request(470, 470);
        let cc = Rc::new(countries());
        let (lang, fmt) = user_locale();
        let have = generated();
        let mut all = have.clone();
        for s in supported() { if !all.contains(&s) { all.push(s); } }
        if !all.contains(&fmt) { all.insert(0, fmt.clone()); }
        let mut named: Vec<(String, String)> = all.iter().map(|l| (l.clone(), format!("{}{}", locale_name(l, &cc), if have.contains(l) || l == &fmt { "" } else { "  (not installed)" }))).collect();
        named.sort_by(|a, b| a.1.cmp(&b.1));

        // ---------------------------------------------------------------- Formats
        let fv = vbox(8);
        fv.pack_start(&label("Format:", ""), false, false, 0);
        let fcombo = long_combo();
        for (id, n) in &named { fcombo.append(Some(id), n); }
        fcombo.set_active_id(Some(&fmt));
        fv.pack_start(&fcombo, false, false, 0);
        let g = gtk::Grid::new();
        g.set_row_spacing(6);
        g.set_column_spacing(10);
        let mk = |t: &str| { let c = gtk::ComboBoxText::new(); c.set_hexpand(true); let _ = t; c };
        let (sd, ld, st, lt, fd) = (mk("sd"), mk("ld"), mk("st"), mk("lt"), mk("fd"));
        for (i, (l, c)) in [("Short date:", &sd), ("Long date:", &ld), ("Short time:", &st), ("Long time:", &lt), ("First day of week:", &fd)].into_iter().enumerate() {
            let lab = label(l, ""); lab.set_line_wrap(false);
            g.attach(&lab, 0, i as i32, 1, 1);
            g.attach(c, 1, i as i32, 1, 1);
        }
        let dt = vbox(6);
        dt.pack_start(&g, false, false, 0);
        dt.pack_start(&link("What does the notation mean?", || notation_help()), false, false, 0);
        fv.pack_start(&group("Date and time formats", &dt), false, false, 0);
        let eg = gtk::Grid::new();
        eg.set_row_spacing(4);
        eg.set_column_spacing(14);
        let ex: Vec<gtk::Label> = (0..4).map(|_| label("", "")).collect();
        for (i, k) in ["Short date:", "Long date:", "Short time:", "Long time:"].iter().enumerate() { eg.attach(&label(k, ""), 0, i as i32, 1, 1); eg.attach(&ex[i], 1, i as i32, 1, 1); }
        fv.pack_start(&group("Examples", &eg), false, false, 0);
        let add = gtk::Button::with_mnemonic("Additional _settings...");
        add.set_halign(gtk::Align::End);
        fv.pack_start(&add, false, false, 0);
        fv.pack_start(&link("Go online to learn about changing languages and regional formats", || { util::open_uri("https://wiki.archlinux.org/title/Locale"); }), false, false, 0);
        sheet.tab("Formats", &fv);
        let cur_fmt: Rc<RefCell<Formats>> = Rc::new(RefCell::new(Formats::default()));
        let clock24 = Rc::new(Cell::new(config::get().clock_24h));
        let refresh: Rc<dyn Fn()> = {
            let (fcombo, sd, ld, st, lt, fd, ex, cur_fmt, clock24) = (fcombo.clone(), sd.clone(), ld.clone(), st.clone(), lt.clone(), fd.clone(), ex.clone(), cur_fmt.clone(), clock24.clone());
            Rc::new(move || {
                let Some(loc) = fcombo.active_id().map(|s| s.to_string()) else { return };
                let (sd, ld, st, lt, fd, ex, cur_fmt, clock24) = (sd.clone(), ld.clone(), st.clone(), lt.clone(), fd.clone(), ex.clone(), cur_fmt.clone(), clock24.clone());
                bg(move || formats(&loc), move |f| {
                    for c in [&sd, &ld, &st, &lt, &fd] { c.remove_all(); }
                    sd.append(Some("d"), &f.short_date); sd.set_active(Some(0));
                    ld.append(Some("d"), &f.long_date); ld.set_active(Some(0));
                    // the taskbar clock follows the short time: 12-hour or 24-hour
                    let twelve = if f.short_time.contains("tt") { f.short_time.clone() } else { "h:mm tt".into() };
                    let h24 = if f.short_time.contains("tt") { "HH:mm".to_string() } else { f.short_time.clone() };
                    st.append(Some("12"), &twelve); st.append(Some("24"), &h24);
                    st.set_active_id(Some(if clock24.get() { "24" } else { "12" }));
                    lt.append(Some("d"), &f.long_time); lt.set_active(Some(0));
                    fd.append(Some("d"), &f.first_day); fd.set_active(Some(0));
                    for (l, t) in ex.iter().zip(f.ex.iter()) { l.set_text(t); }
                    *cur_fmt.borrow_mut() = f;
                });
            })
        };
        refresh();
        { let (r, d) = (refresh.clone(), sheet.dirty()); fcombo.connect_changed(move |_| { r(); d(); }); }
        {
            let (d, clock24, ex, cur_fmt) = (sheet.dirty(), clock24.clone(), ex.clone(), cur_fmt.clone());
            st.connect_changed(move |c| {
                let Some(id) = c.active_id() else { return };
                let is24 = id == "24";
                if is24 != clock24.get() {
                    clock24.set(is24);
                    let now = chrono::Local::now();
                    let _ = &cur_fmt;
                    ex[2].set_text(&if is24 { now.format("%H:%M").to_string() } else { now.format("%-I:%M %p").to_string() });
                    d();
                }
            });
        }
        for c in [&sd, &ld, &lt, &fd] { c.set_sensitive(true); }
        { let (cf, s) = (cur_fmt.clone(), self.clone()); add.connect_clicked(move |_| customize_format(&s, &cf.borrow())); }

        // ---------------------------------------------------------------- Location
        let lv = vbox(8);
        lv.pack_start(&text("Some software, including Windows, may provide you with additional content for a particular location. Some services provide local information such as news and weather."), false, false, 0);
        let lg = vbox(4);
        lg.pack_start(&label("Current location:", ""), false, false, 0);
        let loc_combo = long_combo();
        for (c, n) in cc.iter() { loc_combo.append(Some(c), n); }
        let cur_loc = { let l = config::get().location; if l.is_empty() { fmt.split(['.', '@']).next().unwrap_or("").split('_').nth(1).unwrap_or("US").to_string() } else { l } };
        loc_combo.set_active_id(Some(&cur_loc));
        lg.pack_start(&loc_combo, false, false, 0);
        lv.pack_start(&group("Home location", &lg), false, false, 0);
        sheet.watch_combo(&loc_combo);
        sheet.tab("Location", &lv);

        // ---------------------------------------------------------------- Keyboards and Languages
        let kv = vbox(10);
        let kb = vbox(8);
        kb.pack_start(&text("To change your keyboard or input language click Change keyboards."), false, false, 0);
        let ck = gtk::Button::with_mnemonic("Change _keyboards...");
        ck.set_halign(gtk::Align::End);
        { let s = self.clone(); ck.connect_clicked(move |_| text_services(&s)); }
        kb.pack_start(&ck, false, false, 0);
        kb.pack_start(&link("How do I change the keyboard layout for the Welcome screen?", || { util::open_uri("https://wiki.archlinux.org/title/Xorg/Keyboard_configuration#Using_localectl"); }), false, false, 0);
        kv.pack_start(&group("Keyboards and other input languages", &kb), false, false, 0);
        let dlv = vbox(8);
        dlv.pack_start(&text("Choose a display language:"), false, false, 0);
        let lang_combo = long_combo();
        for l in &have { lang_combo.append(Some(l), &locale_name(l, &cc)); }
        if !have.contains(&lang) { lang_combo.append(Some(&lang), &locale_name(&lang, &cc)); }
        lang_combo.set_active_id(Some(&lang));
        dlv.pack_start(&lang_combo, false, false, 0);
        sheet.watch_combo(&lang_combo);
        dlv.pack_start(&link("How can I install additional languages?", || { util::open_uri("https://wiki.archlinux.org/title/Locale#Generating_locales"); }), false, false, 0);
        kv.pack_start(&group("Display language", &dlv), false, false, 0);
        sheet.tab("Keyboards and Languages", &kv);

        // ---------------------------------------------------------------- Administrative
        let av = vbox(10);
        let wv = vbox(8);
        wv.pack_start(&text("View and copy your international settings to the welcome screen, system accounts and new user accounts."), false, false, 0);
        let copy_b = shield_button("Co_py settings...");
        copy_b.set_halign(gtk::Align::End);
        { let (s, cc) = (self.clone(), cc.clone()); copy_b.connect_clicked(move |_| copy_settings(&s, &cc)); }
        wv.pack_start(&copy_b, false, false, 0);
        av.pack_start(&group("Welcome screen and new user accounts", &wv), false, false, 0);
        let nu = vbox(8);
        nu.pack_start(&text("This setting (system locale) controls the language used when displaying text in programs that do not support Unicode."), false, false, 0);
        nu.pack_start(&label("Current language for non-Unicode programs:", ""), false, false, 0);
        let sys_l = bold("");
        nu.pack_start(&sys_l, false, false, 0);
        { let (l, cc) = (sys_l.clone(), cc.clone()); bg(system_locale, move |s| l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&locale_name(&s, &cc))))); }
        let chg = shield_button("C_hange system locale...");
        chg.set_halign(gtk::Align::End);
        { let (s, cc, have2, l) = (self.clone(), cc.clone(), have.clone(), sys_l.clone()); chg.connect_clicked(move |_| change_system_locale(&s, &cc, &have2, &l)); }
        nu.pack_start(&chg, false, false, 0);
        av.pack_start(&group("Language for non-Unicode programs", &nu), false, false, 0);
        av.pack_start(&link("What is system locale?", || { util::open_uri("https://wiki.archlinux.org/title/Locale#Setting_the_system_locale"); }), false, false, 0);
        sheet.tab("Administrative", &av);

        // ---------------------------------------------------------------- apply
        {
            let (fcombo, loc_combo, lang_combo, clock24, me) = (fcombo.clone(), loc_combo.clone(), lang_combo.clone(), clock24.clone(), self.clone());
            let (fmt0, lang0, have) = (fmt.clone(), lang.clone(), have.clone());
            sheet.on_apply(move || {
                let f = fcombo.active_id().map(|s| s.to_string()).unwrap_or_default();
                let l = lang_combo.active_id().map(|s| s.to_string()).unwrap_or_default();
                let missing: Vec<String> = [&f, &l].iter().filter(|x| !x.is_empty() && !have.contains(*x) && **x != &fmt0 && **x != &lang0).map(|s| s.to_string()).collect();
                if !missing.is_empty() {
                    if !confirm(Some(&me.win.clone().upcast()), "Region and Language", "This format is not installed on this computer.",
                        &format!("Windows can install it now ({}). This needs administrator permission.", missing.join(", ")), "_Install") { return false; }
                    let script = missing.iter().map(|m| format!("sed -i 's/^#[[:space:]]*\\({} UTF-8\\)/\\1/' /etc/locale.gen", m.replace('.', "\\."))).collect::<Vec<_>>().join("; ") + "; locale-gen";
                    elevate_sh(&script, |ok, out| { report("The format could not be installed.", ok, &out); });
                }
                write_user_locale(if l != lang0 { Some(&l) } else { None }, if f != fmt0 { Some(&f) } else { None });
                let loc = loc_combo.active_id().map(|s| s.to_string()).unwrap_or_default();
                let c24 = clock24.get();
                let changed_clock = config::get().clock_24h != c24;
                config::update(|c| { c.location = loc; c.clock_24h = c24; });
                if changed_clock { util::spawn(&["aerode", "reload"]); }
                if l != lang0 {
                    inform(Some(&me.win.clone().upcast()), "Change Display Language", "You must log off for display language changes to take effect.", "Programs started after you log on again use the new language.");
                }
                true
            });
        }
        sheet.widget()
    }
}

fn notation_help() {
    inform(None, "Date and time notation", "What does the notation mean?",
        "d, dd = day;  ddd, dddd = day of week;  M = month;  y = year\nh/H = 12/24 hour;  m = minute;  s = second;  tt = A.M. or P.M.\n\nh = 12 hour   H = 24 hour\nhh, mm, ss = display leading zero\nh, m, s = do not display leading zero\n\ndd = 03, MM = 01, MMM = Jan, MMMM = January, yy = 26, yyyy = 2026");
}

/// "Customize Format": the locale's numbers, currency, time and date (read only on Linux).
fn customize_format(cp: &Rc<Cp>, f: &Formats) {
    let d = dialog(Some(&cp.win.clone().upcast()), "Customize Format", &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let nb = gtk::Notebook::new();
    let sample = |pos: &str, neg: &str| vec![("Positive:", pos.to_string()), ("Negative:", neg.to_string())];
    let grp = if f.group.is_empty() { "" } else { &f.group };
    let num = format!("123{grp}456{}789", f.decimal);
    let tab = |rows: Vec<(&str, String)>, ex: Vec<(&str, String)>| {
        let v = vbox(8);
        v.set_border_width(10);
        v.pack_start(&group("Example", &kv_grid(&ex)), false, false, 0);
        v.pack_start(&kv_grid(&rows), false, false, 0);
        v
    };
    nb.append_page(&tab(vec![("Decimal symbol:", f.decimal.clone()), ("Digit grouping symbol:", if f.group.is_empty() { "(none)".into() } else { f.group.clone() }), ("Digit grouping:", format!("123{grp}456{grp}789")), ("Negative sign symbol:", "-".into()), ("Measurement system:", f.measure.clone())],
        sample(&num, &format!("-{num}"))), Some(&gtk::Label::new(Some("Numbers"))));
    nb.append_page(&tab(vec![("Currency symbol:", f.currency.clone()), ("Decimal symbol:", f.decimal.clone()), ("Digit grouping symbol:", f.group.clone())],
        sample(&format!("{}123{grp}456{}78", f.currency, f.decimal), &format!("-{}123{grp}456{}78", f.currency, f.decimal))), Some(&gtk::Label::new(Some("Currency"))));
    nb.append_page(&tab(vec![("Short time:", f.short_time.clone()), ("Long time:", f.long_time.clone()), ("AM symbol:", f.am.clone()), ("PM symbol:", f.pm.clone())],
        vec![("Short time:", f.ex[2].clone()), ("Long time:", f.ex[3].clone())]), Some(&gtk::Label::new(Some("Time"))));
    nb.append_page(&tab(vec![("Short date:", f.short_date.clone()), ("Long date:", f.long_date.clone()), ("First day of week:", f.first_day.clone())],
        vec![("Short date:", f.ex[0].clone()), ("Long date:", f.ex[1].clone())]), Some(&gtk::Label::new(Some("Date"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.content_area().pack_start(&label("These settings come from the format's locale. To use other symbols, choose another format.", "cp-hint"), false, false, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
}

/// "Text Services and Input Languages" (the keyboards; Apply writes them).
fn text_services(cp: &Rc<Cp>) {
    let d = dialog(Some(&cp.win.clone().upcast()), "Text Services and Input Languages", &[("_Close", gtk::ResponseType::Close)]);
    d.set_default_size(470, 420);
    let nb = gtk::Notebook::new();
    let g = crate::input_languages();
    g.set_margin_top(8);
    g.set_margin_end(8);
    g.set_margin_bottom(8);
    nb.append_page(&g, Some(&gtk::Label::new(Some("General"))));
    let lb = vbox(8);
    lb.set_border_width(10);
    lb.pack_start(&label("The input language is shown on the taskbar, in the notification area.", ""), false, false, 0);
    let r = radios(&["_Floating On Desktop", "_Docked in the taskbar", "_Hidden"], 1);
    for b in &r { b.set_sensitive(false); lb.pack_start(b, false, false, 0); }
    nb.append_page(&lb, Some(&gtk::Label::new(Some("Language Bar"))));
    d.content_area().pack_start(&nb, true, true, 0);
    d.connect_response(|d, _| d.close());
    d.show_all();
}

/// "Welcome screen and new user accounts settings".
fn copy_settings(cp: &Rc<Cp>, cc: &[(String, String)]) {
    let d = dialog(Some(&cp.win.clone().upcast()), "Welcome screen and new user accounts settings", &[]);
    d.set_default_size(500, -1);
    let c = d.content_area();
    let (lang, fmt) = user_locale();
    let kb = config::get().kbd_layouts.join(", ");
    let kb = if kb.is_empty() { sh(&["setxkbmap", "-query"]).lines().find_map(|l| l.strip_prefix("layout:")).unwrap_or("").trim().to_string() } else { kb };
    let sys = system_locale();
    let x11 = sh(&["localectl", "status"]).lines().find_map(|l| l.trim().strip_prefix("X11 Layout:")).unwrap_or("").trim().to_string();
    let block = |t: &str, rows: Vec<(&str, String)>| { let v = vbox(4); v.pack_start(&bold(t), false, false, 0); v.pack_start(&kv_grid(&rows), false, false, 0); v };
    c.pack_start(&label("View the settings for the current user, the Welcome screen (system accounts) and new user accounts.", ""), false, false, 0);
    c.pack_start(&block("Current user", vec![("Display language:", locale_name(&lang, cc)), ("Input language:", kb.clone()), ("Format:", locale_name(&fmt, cc)), ("Location:", config::get().location)]), false, false, 0);
    c.pack_start(&block("Welcome screen", vec![("Display language:", locale_name(&sys, cc)), ("Input language:", x11)]), false, false, 0);
    let skel = std::fs::read_to_string("/etc/skel/.config/locale.conf").ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("LANG=").map(String::from))).unwrap_or_else(|| sys.clone());
    c.pack_start(&block("New user accounts", vec![("Display language:", locale_name(&skel, cc))]), false, false, 0);
    c.pack_start(&label("Copy your current settings to:", ""), false, false, 0);
    let w = check("_Welcome screen and system accounts", false);
    let n = check("_New user accounts", false);
    c.pack_start(&w, false, false, 0);
    c.pack_start(&n, false, false, 0);
    let ok = d.add_button("OK", gtk::ResponseType::Ok);
    if let Ok(b) = ok.downcast::<gtk::Button>() { super::super::admin::shield(&b); }
    d.add_button("Cancel", gtk::ResponseType::Cancel);
    d.show_all();
    let r = d.run();
    let (ww, nn) = (w.is_active(), n.is_active());
    d.close();
    if r != gtk::ResponseType::Ok || (!ww && !nn) { return; }
    let mut script = vec![];
    if ww {
        script.push(format!("localectl set-locale LANG={}", q(&lang)));
        if !kb.is_empty() { script.push(format!("localectl set-x11-keymap {}", q(&kb.replace(' ', "")))); }
    }
    if nn {
        let src = glib::user_config_dir().join("locale.conf");
        if src.exists() { script.push(format!("install -Dm644 {} /etc/skel/.config/locale.conf", q(&src.to_string_lossy()))); }
        else { script.push(format!("mkdir -p /etc/skel/.config && printf 'LANG=%s\\n' {} > /etc/skel/.config/locale.conf", q(&lang))); }
    }
    elevate_sh(&script.join(" && "), |ok, out| { report("The settings could not be copied.", ok, &out); });
}

/// "Region and Language Settings": the system locale (LANG in /etc/locale.conf).
fn change_system_locale(cp: &Rc<Cp>, cc: &[(String, String)], have: &[String], shown: &gtk::Label) {
    let d = dialog(Some(&cp.win.clone().upcast()), "Region and Language Settings", &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    let c = d.content_area();
    c.pack_start(&text("Select which language (system locale) to use when displaying text in programs that do not support Unicode. This setting affects all user accounts on the computer."), false, false, 0);
    c.pack_start(&label("Current system locale:", ""), false, false, 0);
    let combo = long_combo();
    let cur = system_locale();
    let mut v: Vec<(String, String)> = have.iter().map(|l| (l.clone(), locale_name(l, cc))).collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    for (id, n) in &v { combo.append(Some(id), n); }
    if !combo.set_active_id(Some(&cur)) { combo.append(Some(&cur), &locale_name(&cur, cc)); combo.set_active_id(Some(&cur)); }
    c.pack_start(&combo, false, false, 0);
    d.show_all();
    let r = d.run();
    let pick = combo.active_id().map(|s| s.to_string()).unwrap_or_default();
    d.close();
    if r != gtk::ResponseType::Ok || pick == cur || pick.is_empty() { return; }
    let (l, n, win) = (shown.clone(), locale_name(&pick, cc), cp.win.clone());
    elevate(&["localectl", "set-locale", &format!("LANG={pick}")], move |ok, out| {
        if !report("The system locale could not be changed.", ok, &out) { return; }
        l.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&n)));
        let d = gtk::MessageDialog::new(Some(&win), gtk::DialogFlags::MODAL, gtk::MessageType::Info, gtk::ButtonsType::None, "You must restart your computer in order for the new settings to take effect.");
        d.set_title("Change System Locale");
        d.add_button("_Restart now", gtk::ResponseType::Accept);
        d.add_button("Cancel", gtk::ResponseType::Cancel);
        let r = d.run();
        d.close();
        if r == gtk::ResponseType::Accept { aerode::sys::restart(); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notation() {
        assert_eq!(win_notation("%-m/%-d/%Y"), "M/d/yyyy");
        assert_eq!(win_notation("%A, %B %d, %Y"), "dddd, MMMM dd, yyyy");
        assert_eq!(win_notation("%-I:%M %p"), "h:mm tt");
        assert_eq!(win_notation("%H:%M:%S"), "HH:mm:ss");
        assert_eq!(win_notation("%d.%m.%Y"), "dd.MM.yyyy");
    }
    #[test]
    fn names() {
        let cc = vec![("US".to_string(), "United States".to_string()), ("DE".to_string(), "Germany".to_string())];
        assert_eq!(locale_name("en_US.UTF-8", &cc), "English (United States)");
        assert_eq!(locale_name("de_DE.UTF-8", &cc), "German (Germany)");
    }
}
