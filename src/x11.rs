//! Minimal EWMH client used by the taskbar (task list, activate, minimise…).
use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::glib;
use std::os::fd::AsRawFd;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        _NET_CLIENT_LIST, _NET_ACTIVE_WINDOW, _NET_WM_NAME, UTF8_STRING, _NET_WM_ICON,
        _NET_WM_STATE, _NET_WM_STATE_SKIP_TASKBAR, _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_DEMANDS_ATTENTION, _NET_WM_WINDOW_TYPE, _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_DIALOG, _NET_WM_WINDOW_TYPE_DOCK, _NET_WM_WINDOW_TYPE_DESKTOP,
        _NET_CLOSE_WINDOW, _NET_SHOWING_DESKTOP, WM_CHANGE_STATE, _NET_WM_STRUT,
        _NET_WM_STRUT_PARTIAL, _NET_WM_PID, _NET_CURRENT_DESKTOP, _NET_WM_DESKTOP,
        _NET_NUMBER_OF_DESKTOPS, _NET_DESKTOP_NAMES, _NET_MOVERESIZE_WINDOW, _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ, _NET_WM_STATE_ABOVE, WM_STATE,
        _NET_WM_ALLOWED_ACTIONS, _NET_WM_ACTION_RESIZE, _NET_WM_ACTION_MINIMIZE, _NET_WM_ACTION_MAXIMIZE_VERT, _NET_WM_MOVERESIZE,
    }
}

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

pub struct Wm {
    pub conn: RustConnection,
    pub root: u32,
    pub atoms: Atoms,
}

impl Wm {
    pub fn connect() -> anyhow::Result<Wm> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE | EventMask::SUBSTRUCTURE_NOTIFY),
        )?;
        conn.flush()?;
        Ok(Wm { conn, root, atoms })
    }

    pub fn fd(&self) -> i32 {
        self.conn.stream().as_raw_fd()
    }

    fn prop32(&self, w: u32, atom: u32, ty: impl Into<u32>) -> Vec<u32> {
        self.conn
            .get_property(false, w, atom, ty.into(), 0, u32::MAX / 4)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|i| i.collect()))
            .unwrap_or_default()
    }

    fn prop_bytes(&self, w: u32, atom: u32, ty: impl Into<u32>) -> Vec<u8> {
        self.conn
            .get_property(false, w, atom, ty.into(), 0, 4096)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.value)
            .unwrap_or_default()
    }

    pub fn client_ids(&self) -> Vec<u32> {
        self.prop32(self.root, self.atoms._NET_CLIENT_LIST, AtomEnum::WINDOW)
    }

    pub fn active(&self) -> u32 {
        self.prop32(self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW).first().copied().unwrap_or(0)
    }

    pub fn showing_desktop(&self) -> bool {
        self.prop32(self.root, self.atoms._NET_SHOWING_DESKTOP, AtomEnum::CARDINAL).first() == Some(&1)
    }

    pub fn current_desktop(&self) -> u32 {
        self.prop32(self.root, self.atoms._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL).first().copied().unwrap_or(0)
    }

    pub fn num_desktops(&self) -> u32 {
        self.prop32(self.root, self.atoms._NET_NUMBER_OF_DESKTOPS, AtomEnum::CARDINAL).first().copied().unwrap_or(1)
    }

    /// Clients that belong on the taskbar.
    pub fn clients(&self) -> Vec<Client> {
        let a = &self.atoms;
        let mut out = Vec::new();
        for id in self.client_ids() {
            let types = self.prop32(id, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM);
            if types.iter().any(|t| *t == a._NET_WM_WINDOW_TYPE_DOCK || *t == a._NET_WM_WINDOW_TYPE_DESKTOP) {
                continue;
            }
            let state = self.prop32(id, a._NET_WM_STATE, AtomEnum::ATOM);
            if state.contains(&a._NET_WM_STATE_SKIP_TASKBAR) {
                continue;
            }
            let _ = self.conn.change_window_attributes(id, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE));
            let mut title = String::from_utf8_lossy(&self.prop_bytes(id, a._NET_WM_NAME, a.UTF8_STRING)).into_owned();
            if title.is_empty() {
                title = String::from_utf8_lossy(&self.prop_bytes(id, AtomEnum::WM_NAME.into(), AtomEnum::STRING)).into_owned();
            }
            let cls = self.prop_bytes(id, AtomEnum::WM_CLASS.into(), AtomEnum::STRING);
            let mut parts = cls.split(|b| *b == 0).map(|s| String::from_utf8_lossy(s).into_owned());
            let instance = parts.next().unwrap_or_default();
            let class = parts.next().unwrap_or_default();
            let wm_state = self.prop32(id, a.WM_STATE, a.WM_STATE);
            out.push(Client {
                id,
                title,
                instance,
                class,
                pid: self.prop32(id, a._NET_WM_PID, AtomEnum::CARDINAL).first().copied().unwrap_or(0),
                minimized: state.contains(&a._NET_WM_STATE_HIDDEN) || wm_state.first() == Some(&3),
                urgent: state.contains(&a._NET_WM_STATE_DEMANDS_ATTENTION),
                desktop: self.prop32(id, a._NET_WM_DESKTOP, AtomEnum::CARDINAL).first().copied().unwrap_or(0),
            });
        }
        let _ = self.conn.flush();
        out
    }

    /// Best _NET_WM_ICON for `size`.
    pub fn icon(&self, w: u32, size: u32) -> Option<Pixbuf> {
        let data = self
            .conn
            .get_property(false, w, self.atoms._NET_WM_ICON, AtomEnum::CARDINAL, 0, u32::MAX / 4)
            .ok()?
            .reply()
            .ok()?;
        let v: Vec<u32> = data.value32()?.collect();
        let mut i = 0;
        let mut best: Option<(u32, u32, usize)> = None;
        while i + 2 <= v.len() {
            let (iw, ih) = (v[i], v[i + 1]);
            let n = (iw * ih) as usize;
            if iw == 0 || i + 2 + n > v.len() {
                break;
            }
            let better = match best {
                None => true,
                Some((bw, _, _)) => (iw >= size && (bw < size || iw < bw)) || (bw < size && iw > bw),
            };
            if better {
                best = Some((iw, ih, i + 2));
            }
            i += 2 + n;
        }
        let (iw, ih, off) = best?;
        let mut rgba = Vec::with_capacity((iw * ih * 4) as usize);
        for px in &v[off..off + (iw * ih) as usize] {
            rgba.extend_from_slice(&[(px >> 16) as u8, (px >> 8) as u8, *px as u8, (px >> 24) as u8]);
        }
        Some(Pixbuf::from_bytes(&glib::Bytes::from_owned(rgba), Colorspace::Rgb, true, 8, iw as i32, ih as i32, iw as i32 * 4))
    }

    fn client_msg(&self, w: u32, ty: u32, data: [u32; 5]) {
        let ev = ClientMessageEvent::new(32, w, ty, data);
        let _ = self.conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ev,
        );
        // a round trip, not just a flush: short-lived callers (aerode sysmenu) exit right after,
        // and a connection that closes with the request in flight can lose it
        let _ = self.conn.get_input_focus().map(|c| c.reply());
    }

    /// (maximized, minimized, resizable, can minimize) for the system menu.
    pub fn window_state(&self, w: u32) -> (bool, bool, bool, bool) {
        let a = &self.atoms;
        let st = self.prop32(w, a._NET_WM_STATE, AtomEnum::ATOM);
        let allowed = self.prop32(w, a._NET_WM_ALLOWED_ACTIONS, AtomEnum::ATOM);
        let can = |x: u32| allowed.is_empty() || allowed.contains(&x);
        (st.contains(&a._NET_WM_STATE_MAXIMIZED_VERT) && st.contains(&a._NET_WM_STATE_MAXIMIZED_HORZ),
         st.contains(&a._NET_WM_STATE_HIDDEN), can(a._NET_WM_ACTION_RESIZE), can(a._NET_WM_ACTION_MINIMIZE))
    }

    /// Keyboard move (`size` = false) or size, as from the system menu.
    pub fn keyboard_moveresize(&self, w: u32, size: bool) {
        self.client_msg(w, self.atoms._NET_WM_MOVERESIZE, [0, 0, if size { 9 } else { 10 }, 0, 2]);
    }

    pub fn activate(&self, w: u32) {
        self.client_msg(w, self.atoms._NET_ACTIVE_WINDOW, [2, 0, 0, 0, 0]);
    }
    pub fn minimize(&self, w: u32) {
        self.client_msg(w, self.atoms.WM_CHANGE_STATE, [3, 0, 0, 0, 0]);
    }
    pub fn close(&self, w: u32) {
        self.client_msg(w, self.atoms._NET_CLOSE_WINDOW, [0, 2, 0, 0, 0]);
    }
    pub fn set_maximized(&self, w: u32, on: bool) {
        let a = &self.atoms;
        self.client_msg(w, a._NET_WM_STATE, [on as u32, a._NET_WM_STATE_MAXIMIZED_VERT, a._NET_WM_STATE_MAXIMIZED_HORZ, 2, 0]);
    }
    pub fn set_showing_desktop(&self, on: bool) {
        self.client_msg(self.root, self.atoms._NET_SHOWING_DESKTOP, [on as u32, 0, 0, 0, 0]);
    }
    pub fn set_desktop(&self, d: u32) {
        self.client_msg(self.root, self.atoms._NET_CURRENT_DESKTOP, [d, 0, 0, 0, 0]);
    }
    pub fn move_resize(&self, w: u32, x: i32, y: i32, width: i32, height: i32) {
        // gravity static + x,y,w,h flags, source = pager
        self.set_maximized(w, false);
        self.client_msg(w, self.atoms._NET_MOVERESIZE_WINDOW, [(0xf << 8) | (2 << 12) | 10, x as u32, y as u32, width as u32, height as u32]);
    }

    pub fn set_strut(&self, xid: u32, bottom: u32, x0: u32, x1: u32) {
        let a = &self.atoms;
        let _ = self.conn.change_property32(PropMode::REPLACE, xid, a._NET_WM_STRUT_PARTIAL, AtomEnum::CARDINAL,
            &[0, 0, 0, bottom, 0, 0, 0, 0, 0, 0, x0, x1]);
        let _ = self.conn.change_property32(PropMode::REPLACE, xid, a._NET_WM_STRUT, AtomEnum::CARDINAL, &[0, 0, 0, bottom]);
        let _ = self.conn.flush();
    }

    /// EWMH `_NET_WM_ICON_GEOMETRY`: where `win`'s taskbar button is (root coordinates). aerode-wm
    /// aims the Win7 minimise and restore animations at it; for grouped windows pass the group's
    /// button. Writes only when the rectangle changed.
    pub fn set_icon_geometry(&self, win: u32, x: i32, y: i32, w: i32, h: i32) {
        thread_local! {
            static ATOM: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
            static LAST: std::cell::RefCell<std::collections::HashMap<u32, [u32; 4]>> = std::cell::RefCell::new(Default::default());
        }
        let v = [x.max(0) as u32, y.max(0) as u32, w.max(0) as u32, h.max(0) as u32];
        if LAST.with(|l| l.borrow().get(&win) == Some(&v)) { return; }
        let mut atom = ATOM.with(|a| a.get());
        if atom == 0 {
            atom = self.conn.intern_atom(false, b"_NET_WM_ICON_GEOMETRY").ok().and_then(|c| c.reply().ok()).map(|r| r.atom).unwrap_or(0);
            if atom == 0 { return; }
            ATOM.with(|a| a.set(atom));
        }
        if self.conn.change_property32(PropMode::REPLACE, win, atom, AtomEnum::CARDINAL, &v).is_ok() {
            LAST.with(|l| { let mut l = l.borrow_mut(); if l.len() > 512 { l.clear(); } l.insert(win, v); });
        }
        let _ = self.conn.flush();
    }

    /// `set_icon_geometry` for the taskbar button widget `button` (its allocation on screen).
    pub fn set_icon_geometry_of(&self, win: u32, button: &impl gtk::prelude::IsA<gtk::Widget>) {
        use gtk::prelude::*;
        let b = button.upcast_ref::<gtk::Widget>();
        let Some(top) = b.toplevel().and_then(|t| t.window()) else { return };
        if !b.is_mapped() { return; }
        let (_, ox, oy) = top.origin();
        let Some((bx, by)) = b.translate_coordinates(&b.toplevel().unwrap(), 0, 0) else { return };
        let a = b.allocation();
        self.set_icon_geometry(win, ox + bx, oy + by, a.width(), a.height());
    }

    /// Drain pending events; true if anything relevant to the taskbar changed.
    pub fn drain(&self) -> bool {
        let mut changed = false;
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            match ev {
                Event::PropertyNotify(p) => {
                    let a = &self.atoms;
                    let relevant = [a._NET_CLIENT_LIST, a._NET_ACTIVE_WINDOW, a._NET_WM_NAME, u32::from(AtomEnum::WM_NAME), a._NET_WM_STATE,
                        a.WM_STATE, a._NET_WM_ICON, a._NET_WM_DESKTOP, a._NET_CURRENT_DESKTOP, u32::from(AtomEnum::WM_HINTS), a._NET_SHOWING_DESKTOP];
                    if relevant.contains(&p.atom) { changed = true; }
                }
                Event::DestroyNotify(_) | Event::UnmapNotify(_) | Event::MapNotify(_) => changed = true,
                _ => {}
            }
        }
        changed
    }
}

/// Watch the X connection from the GLib main loop.
pub fn watch(wm: std::rc::Rc<Wm>, on_change: impl Fn() + 'static) {
    let w2 = wm.clone();
    let fire = std::rc::Rc::new(on_change);
    let pending = std::rc::Rc::new(std::cell::Cell::new(false));
    let schedule = {
        let fire = fire.clone();
        let pending = pending.clone();
        move || {
            if !pending.replace(true) {
                let fire = fire.clone();
                let pending = pending.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
                    pending.set(false);
                    fire();
                });
            }
        }
    };
    let s2 = schedule.clone();
    glib::source::unix_fd_add_local(wm.fd(), glib::IOCondition::IN, move |_, _| {
        if w2.drain() {
            s2();
        }
        glib::ControlFlow::Continue
    });
    // replies can swallow events into the queue; poll occasionally too
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        if wm.drain() {
            schedule();
        }
        glib::ControlFlow::Continue
    });
}
