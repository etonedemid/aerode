//! The shell's view of the compositor: the task list, activate, minimise... over `crate::wmipc`.
use crate::wmipc::{self, Req};
use gtk::gdk_pixbuf::Pixbuf;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub struct Client {
    pub id: u32,
    pub title: String,
    pub instance: String,
    pub class: String,
    pub pid: u32,
    pub minimized: bool,
    pub urgent: bool,
    pub desktop: u32,
}

pub struct Wm;

impl Wm {
    /// Fails outside an Aerode session.
    pub fn connect() -> anyhow::Result<Wm> {
        wmipc::call(&Req::Locked).map(|_| Wm).ok_or_else(|| anyhow::anyhow!("aerode-wm is not running here"))
    }

    pub fn state(&self) -> wmipc::State { wmipc::state() }
    pub fn client_ids(&self) -> Vec<u32> { self.state().windows.iter().map(|w| w.id).collect() }
    pub fn active(&self) -> u32 { self.state().active }
    pub fn showing_desktop(&self) -> bool { self.state().showing_desktop }
    pub fn current_desktop(&self) -> u32 { self.state().current_desktop }
    pub fn num_desktops(&self) -> u32 { self.state().num_desktops }

    /// Clients that belong on the taskbar.
    pub fn clients(&self) -> Vec<Client> {
        self.state().windows.into_iter().filter(|w| w.taskbar).map(|w| Client {
            id: w.id, title: w.title, instance: w.app_id.clone(), class: w.app_id, pid: w.pid, minimized: w.minimized, urgent: w.urgent, desktop: w.desktop,
        }).collect()
    }

    /// The icon of a window without a .desktop entry: its app id through the icon theme.
    pub fn icon(&self, w: u32, size: u32) -> Option<Pixbuf> {
        let st = self.state();
        let id = st.windows.iter().find(|x| x.id == w)?.app_id.to_lowercase();
        let theme = gtk::IconTheme::default()?;
        theme.load_icon(&id, size as i32, gtk::IconLookupFlags::FORCE_SIZE).ok().flatten()
    }

    /// (maximized, minimized, resizable, can minimize)
    pub fn window_state(&self, w: u32) -> (bool, bool, bool, bool) {
        let st = self.state();
        st.windows.iter().find(|x| x.id == w).map(|x| (x.maximized, x.minimized, x.resizable, x.can_minimize)).unwrap_or((false, false, true, true))
    }

    pub fn keyboard_moveresize(&self, w: u32, size: bool) { wmipc::send(Req::KeyboardMoveResize { id: w, size }); }
    pub fn activate(&self, w: u32) { wmipc::send(Req::Activate { id: w }); }
    pub fn minimize(&self, w: u32) { wmipc::send(Req::Minimize { id: w }); }
    pub fn close(&self, w: u32) { wmipc::send(Req::Close { id: w }); }
    pub fn set_maximized(&self, w: u32, on: bool) { wmipc::send(Req::SetMaximized { id: w, on }); }
    pub fn set_showing_desktop(&self, on: bool) { wmipc::send(Req::SetShowingDesktop { on }); }
    pub fn set_desktop(&self, d: u32) { wmipc::send(Req::SetDesktop { n: d }); }
    pub fn place_outer(&self, w: u32, x: i32, y: i32, width: i32, height: i32) { wmipc::send(Req::Place { id: w, x, y, w: width, h: height, outer: true }); }
    pub fn move_resize(&self, w: u32, x: i32, y: i32, width: i32, height: i32) { wmipc::send(Req::Place { id: w, x, y, w: width, h: height, outer: false }); }
    pub fn set_icon_geometry(&self, win: u32, x: i32, y: i32, w: i32, h: i32) { wmipc::send(Req::IconGeometry { id: win, x, y, w, h }); }

    /// Tell the compositor where this window's taskbar button is (minimising flies there).
    pub fn set_icon_geometry_of(&self, win: u32, button: &impl IsA<gtk::Widget>) {
        let b = button.as_ref();
        let (x, y) = crate::shellwin::widget_pos(b);
        let a = b.allocation();
        if a.width() > 1 { self.set_icon_geometry(win, x, y, a.width(), a.height()); }
    }
}

thread_local! {
    static LISTENERS: RefCell<Vec<Rc<dyn Fn(&str)>>> = const { RefCell::new(Vec::new()) };
    static STREAM: RefCell<Option<BufReader<std::os::unix::net::UnixStream>>> = const { RefCell::new(None) };
}

/// Run `f` with the name of every event of the compositor ("state", "colorization", "outputs", "keyboard").
pub fn on_event(f: impl Fn(&str) + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Rc::new(f)));
    if STREAM.with(|s| s.borrow().is_some()) { return; }
    let Some(stream) = wmipc::subscribe() else { return };
    use std::os::fd::AsRawFd;
    let fd = stream.as_raw_fd();
    STREAM.with(|s| *s.borrow_mut() = Some(BufReader::new(stream)));
    glib::source::unix_fd_add_local(fd, glib::IOCondition::IN | glib::IOCondition::HUP, |_, cond| {
        if cond.contains(glib::IOCondition::HUP) { return glib::ControlFlow::Break; }
        let mut line = String::new();
        // one line per wakeup: the fd stays readable while more are queued (or buffered, drained below)
        let more = STREAM.with(|s| {
            let mut s = s.borrow_mut();
            let Some(r) = s.as_mut() else { return false };
            if r.read_line(&mut line).unwrap_or(0) == 0 { return false; }
            true
        });
        if !more { return glib::ControlFlow::Break; }
        let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        if let Some(ev) = v.get("ev").and_then(|e| e.as_str()) {
            let list: Vec<Rc<dyn Fn(&str)>> = LISTENERS.with(|l| l.borrow().clone());
            for f in list { f(ev); }
        }
        glib::ControlFlow::Continue
    });
}

/// Call `on_change` (once per burst) whenever the window list changes.
pub fn watch(_wm: Rc<Wm>, on_change: impl Fn() + 'static) {
    let fire = Rc::new(on_change);
    let pending = Rc::new(std::cell::Cell::new(false));
    on_event(move |ev| {
        if ev != "state" || pending.replace(true) { return; }
        let (f, p) = (fire.clone(), pending.clone());
        glib::idle_add_local_once(move || { p.set(false); f(); });
    });
}
