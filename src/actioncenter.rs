//! The Action Center flag in the notification area, like Windows 7: a white flag, with a red X
//! while an important message is waiting. Pointing at it says "Solve PC issues: …"; clicking it
//! opens the flyout with the messages (each opens Action Center) and "Open Action Center".
//!
//! The messages are Control Panel's Action Center checks (firewall, updates, disk space, failed
//! services, backup, crashed programs…), read from `aerode-control --action-center-summary` in the
//! background a minute after login, every 30 minutes and whenever the flyout is opened.
use gtk::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use crate::flyouts::{close_open, control, link, popup, tray_button};
use crate::taskbar::Taskbar;
use crate::util;

#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub important: bool,
    pub kind: String,
    pub title: String,
    pub text: String,
}

/// Parse the summary lines (`important<TAB>kind<TAB>title<TAB>text`).
pub fn parse(out: &str) -> Vec<Msg> {
    out.lines().filter_map(|l| {
        let mut f = l.splitn(4, '\t');
        Some(Msg { important: f.next()? == "1", kind: f.next()?.to_string(), title: f.next()?.to_string(), text: f.next().unwrap_or("").to_string() })
    }).collect()
}

/// Win7's wording: "Solve PC issues: 1 important message" / "… 2 messages", or no issues.
pub fn summary(msgs: &[Msg]) -> (String, Option<String>) {
    let imp = msgs.iter().filter(|m| m.important).count();
    let n = msgs.len();
    let plural = |k: usize, w: &str| format!("{k} {w}{}", if k == 1 { "" } else { "s" });
    match (imp, n) {
        (_, 0) => ("No current issues detected".into(), None),
        (0, n) => (format!("Solve PC issues: {}", plural(n, "message")), None),
        (i, n) if i == n => (format!("Solve PC issues: {}", plural(i, "important message")), None),
        (i, n) => (format!("Solve PC issues: {}", plural(i, "important message")), Some(plural(n, "total message"))),
    }
}

pub fn attach(tb: &Rc<Taskbar>) {
    let flag = util::first_icon(&["aerode-ac-flag", "emblem-important", "dialog-information"]);
    let (b, img) = tray_button(tb, flag, "Action Center");
    let msgs: Rc<RefCell<Vec<Msg>>> = Rc::new(RefCell::new(vec![]));
    // refresh, then (for a click) open the flyout with the fresh list
    let refresh: Rc<dyn Fn(Option<gtk::Widget>)> = {
        let (b, img, msgs) = (b.clone(), img.clone(), msgs.clone());
        Rc::new(move |open: Option<gtk::Widget>| {
            let (b, img, msgs) = (b.clone(), img.clone(), msgs.clone());
            util::output_async(vec!["aerode-control".into(), "--action-center-summary".into()], move |out| {
                if let Some(out) = out {
                    let list = parse(&out);
                    let icon = if list.iter().any(|m| m.important) {
                        util::first_icon(&["aerode-ac-flag-error", "aerode-ac-flag", "dialog-error"])
                    } else {
                        util::first_icon(&["aerode-ac-flag", "emblem-important", "dialog-information"])
                    };
                    if let Some(p) = util::pixbuf(icon, 16) { img.set_from_pixbuf(Some(&p)); }
                    let (head, total) = summary(&list);
                    b.set_tooltip_text(Some(&match total { Some(t) => format!("{head}\n{t}"), None => head }));
                    *msgs.borrow_mut() = list;
                }
                if let Some(a) = open { show(&a, &msgs.borrow()); }
            });
        })
    };
    // Win7 waits a little after logon before the first check
    { let r = refresh.clone(); glib::timeout_add_seconds_local_once(60, move || r(None)); }
    { let r = refresh.clone(); glib::timeout_add_seconds_local(30 * 60, move || { r(None); glib::ControlFlow::Continue }); }
    b.connect_clicked(move |b| {
        if close_open() { return; }
        refresh(Some(b.clone().upcast()));
    });
}

fn show(anchor: &gtk::Widget, msgs: &[Msg]) {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.set_size_request(320, -1);
    let (head, total) = summary(msgs);
    let h = gtk::Label::new(None);
    h.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&head)));
    h.set_halign(gtk::Align::Start);
    body.pack_start(&h, false, false, 0);
    if let Some(t) = total {
        let l = gtk::Label::new(Some(&t));
        l.set_halign(gtk::Align::Start);
        body.pack_start(&l, false, false, 0);
    }
    if !msgs.is_empty() {
        body.pack_start(&gtk::Separator::new(gtk::Orientation::Horizontal), false, false, 4);
    }
    // the important ones first, like Win7
    let mut sorted: Vec<&Msg> = msgs.iter().collect();
    sorted.sort_by_key(|m| !m.important);
    for m in sorted {
        let row = gtk::Button::new();
        row.set_relief(gtk::ReliefStyle::None);
        row.style_context().add_class("ac-message");
        let hb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = if m.important { util::first_icon(&["aerode-shield-error", "dialog-error"]) } else { util::first_icon(&["aerode-shield-warning", "dialog-warning"]) };
        let im = util::image(icon, 32);
        im.set_valign(gtk::Align::Start);
        hb.pack_start(&im, false, false, 0);
        let vb = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let t = gtk::Label::new(Some(&m.title));
        t.set_halign(gtk::Align::Start);
        t.style_context().add_class("ac-title");
        vb.pack_start(&t, false, false, 0);
        let d = gtk::Label::new(Some(&m.text));
        d.set_halign(gtk::Align::Start);
        d.set_xalign(0.0);
        d.set_line_wrap(true);
        d.set_max_width_chars(40);
        vb.pack_start(&d, false, false, 0);
        hb.pack_start(&vb, true, true, 0);
        row.add(&hb);
        row.connect_clicked(|_| { close_open(); control("actioncenter"); });
        body.pack_start(&row, false, false, 0);
    }
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    foot.pack_start(&link("Open Action Center", || control("actioncenter")), false, false, 0);
    popup(anchor, body.upcast_ref(), Some(foot.upcast_ref()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wording() {
        let m = |imp: bool| Msg { important: imp, kind: "x".into(), title: "t".into(), text: "".into() };
        assert_eq!(summary(&[]).0, "No current issues detected");
        assert_eq!(summary(&[m(false), m(false)]), ("Solve PC issues: 2 messages".into(), None));
        assert_eq!(summary(&[m(true)]), ("Solve PC issues: 1 important message".into(), None));
        assert_eq!(summary(&[m(true), m(false), m(false)]), ("Solve PC issues: 1 important message".into(), Some("3 total messages".into())));
        let p = parse("1\tfirewall\tNetwork firewall\tNo firewall program is installed.\n0\tbackup\tSet up backup\tYour files are not being backed up.\n");
        assert_eq!(p.len(), 2);
        assert!(p[0].important && p[0].kind == "firewall" && !p[1].important);
    }
}
