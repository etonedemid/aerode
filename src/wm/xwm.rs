//! XWayland: X11 programs keep running, framed and listed like the others.
use super::{Kind, Rect, Wm};
use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::selection::data_device::{clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection, set_data_device_selection};
use smithay::wayland::selection::primary_selection::{clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection, set_primary_selection};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabHandler;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, WmWindowType, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};
use smithay::{delegate_xwayland_keyboard_grab, delegate_xwayland_shell};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use std::os::fd::OwnedFd;
use std::process::Stdio;

use super::input::KFocus;

/// Start Xwayland; `DISPLAY` is exported for children once it is ready.
pub fn start(wm: &mut Wm) {
    let spawned = XWayland::spawn(&wm.dh, None, std::iter::empty::<(String, String)>(), true, Stdio::null(), Stdio::null(), |_| ());
    let (xwayland, client) = match spawned {
        Ok(x) => x,
        Err(e) => { eprintln!("aerode-wm: no XWayland, X11 programs won't run: {e}"); wm.run_startup_cmd(); return; }
    };
    let r = wm.handle.insert_source(xwayland, move |event, _, wm: &mut Wm| match event {
        XWaylandEvent::Ready { x11_socket, display_number } => {
            let mut x = match X11Wm::start_wm(wm.handle.clone(), x11_socket, client.clone()) {
                Ok(x) => x,
                Err(e) => { eprintln!("aerode-wm: X11 window manager failed: {e}"); wm.run_startup_cmd(); return; }
            };
            if let Some((px, (w, h), (hx, hy))) = wm.cursors.rgba("left_ptr") {
                let _ = x.set_cursor(&px, (w as u16, h as u16).into(), (hx as u16, hy as u16).into());
            }
            wm.xwm = Some(x);
            // SAFETY: the compositor's env is only read by spawn, on this thread
            unsafe { std::env::set_var("DISPLAY", format!(":{display_number}")); }
            crate::util::spawn(&["dbus-update-activation-environment", "--systemd", &format!("DISPLAY=:{display_number}")]);
            wm.publish_env();
            wm.run_startup_cmd();
        }
        XWaylandEvent::Error => { eprintln!("aerode-wm: XWayland crashed on startup"); wm.run_startup_cmd(); }
    });
    if let Err(e) = r { eprintln!("aerode-wm: XWayland source: {e}"); wm.run_startup_cmd(); }
}

impl Wm {
    fn win_of_x11(&self, x: &X11Surface) -> Option<u32> {
        self.wins.values().find(|c| c.window.x11_surface() == Some(x)).map(|c| c.id)
    }

    /// EWMH type -> our kinds.
    fn x11_kind(x: &X11Surface) -> Kind {
        match x.window_type() {
            Some(WmWindowType::Dialog) => Kind::Dialog,
            Some(WmWindowType::Utility | WmWindowType::Toolbar) => Kind::Utility,
            Some(WmWindowType::Splash) => Kind::Splash,
            Some(WmWindowType::DropdownMenu | WmWindowType::PopupMenu | WmWindowType::Menu | WmWindowType::Tooltip | WmWindowType::Notification) => Kind::Menu,
            _ => if x.is_transient_for().is_some() { Kind::Dialog } else { Kind::Normal },
        }
    }

    fn x11_props(&mut self, id: u32, x: &X11Surface) {
        let transient = x.is_transient_for().and_then(|t| self.wins.values().find(|c| c.window.x11_surface().is_some_and(|s| s.window_id() == t)).map(|c| c.id)).unwrap_or(0);
        let kind = Self::x11_kind(x);
        let (minsz, maxsz) = (x.min_size(), x.max_size());
        let fixed = matches!((minsz, maxsz), (Some(a), Some(b)) if a == b && a.w > 0);
        let c = self.wins.get_mut(&id).unwrap();
        c.transient_for = transient;
        c.kind = kind;
        c.min_size = minsz.map(|s| (s.w, s.h)).unwrap_or((0, 0));
        c.resizable = !fixed;
        // is_decorated: the client draws its own decorations
        c.framed = !x.is_decorated() && !matches!(kind, Kind::Menu | Kind::Splash);
        c.skip_taskbar = matches!(kind, Kind::Menu);
        c.frame_cache = None;
    }
}

impl XWaylandShellHandler for Wm { fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState { &mut self.xwayland_shell_state } }
delegate_xwayland_shell!(Wm);

impl XWaylandKeyboardGrabHandler for Wm {
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<KFocus> {
        let id = self.win_of_surface(surface)?;
        KFocus::of_window(&self.wins[&id].window)
    }
}
delegate_xwayland_keyboard_grab!(Wm);

impl XwmHandler for Wm {
    fn xwm_state(&mut self, _: XwmId) -> &mut X11Wm { self.xwm.as_mut().unwrap() }
    fn new_window(&mut self, _: XwmId, _: X11Surface) {}
    fn new_override_redirect_window(&mut self, _: XwmId, _: X11Surface) {}

    fn map_window_request(&mut self, _: XwmId, x: X11Surface) {
        let _ = x.set_mapped(true);
        let id = match self.win_of_x11(&x) {
            Some(id) => id,
            None => self.add_window(Window::new_x11_window(x.clone()), Kind::Normal, true),
        };
        self.x11_props(id, &x);
        self.refresh_title(id);
        // tell it where it will be: the buffer arrives after this
        let c = self.wins.get_mut(&id).unwrap();
        c.geom = Rect::from_smithay(x.geometry());
    }

    fn mapped_override_redirect_window(&mut self, _: XwmId, x: X11Surface) {
        let id = self.add_window(Window::new_x11_window(x.clone()), Kind::Menu, false);
        let c = self.wins.get_mut(&id).unwrap();
        c.skip_taskbar = true;
        c.above = true;
        c.geom = Rect::from_smithay(x.geometry());
    }

    fn unmapped_window(&mut self, _: XwmId, x: X11Surface) {
        if let Some(id) = self.win_of_x11(&x) { self.remove_window(id); }
        if !x.is_override_redirect() { let _ = x.set_mapped(false); }
    }

    fn destroyed_window(&mut self, _: XwmId, x: X11Surface) {
        if let Some(id) = self.win_of_x11(&x) { self.remove_window(id); }
    }

    fn configure_request(&mut self, _: XwmId, x: X11Surface, nx: Option<i32>, ny: Option<i32>, w: Option<u32>, h: Option<u32>, _r: Option<Reorder>) {
        let mut g = x.geometry();
        if let Some(n) = nx { g.loc.x = n; }
        if let Some(n) = ny { g.loc.y = n; }
        if let Some(n) = w { g.size.w = n as i32; }
        if let Some(n) = h { g.size.h = n as i32; }
        let _ = x.configure(g);
        if let Some(id) = self.win_of_x11(&x) {
            let c = self.wins.get_mut(&id).unwrap();
            if c.mapped { c.geom = Rect::from_smithay(g); c.frame_cache = None; self.sync_mapped(id); }
        }
    }

    fn configure_notify(&mut self, _: XwmId, x: X11Surface, g: Rectangle<i32, Logical>, _above: Option<u32>) {
        let Some(id) = self.win_of_x11(&x) else { return };
        let c = self.wins.get_mut(&id).unwrap();
        let g = Rect::from_smithay(g);
        if c.geom != g {
            if (c.geom.w, c.geom.h) != (g.w, g.h) { c.frame_cache = None; }
            c.geom = g;
            if c.mapped { self.sync_mapped(id); }
            self.needs_redraw = true;
        }
    }

    fn property_notify(&mut self, _: XwmId, x: X11Surface, p: WmWindowProperty) {
        let Some(id) = self.win_of_x11(&x) else { return };
        match p {
            WmWindowProperty::Title | WmWindowProperty::Class => self.refresh_title(id),
            WmWindowProperty::Hints => {
                let urgent = x.hints().is_some_and(|h| h.urgent);
                let c = self.wins.get_mut(&id).unwrap();
                if c.urgent != urgent && self.active != id { c.urgent = urgent; self.state_changed = true; }
            }
            WmWindowProperty::NormalHints | WmWindowProperty::WindowType | WmWindowProperty::TransientFor | WmWindowProperty::MotifHints => self.x11_props(id, &x),
            _ => {}
        }
    }

    fn maximize_request(&mut self, _: XwmId, x: X11Surface) { if let Some(id) = self.win_of_x11(&x) { self.set_maximized(id, true); } }
    fn unmaximize_request(&mut self, _: XwmId, x: X11Surface) { if let Some(id) = self.win_of_x11(&x) { self.set_maximized(id, false); } }
    fn fullscreen_request(&mut self, _: XwmId, x: X11Surface) { if let Some(id) = self.win_of_x11(&x) { self.set_fullscreen(id, true); } }
    fn unfullscreen_request(&mut self, _: XwmId, x: X11Surface) { if let Some(id) = self.win_of_x11(&x) { self.set_fullscreen(id, false); } }
    fn minimize_request(&mut self, _: XwmId, x: X11Surface) { if let Some(id) = self.win_of_x11(&x) { self.minimize(id); } }
    fn unminimize_request(&mut self, _: XwmId, x: X11Surface) { if let Some(id) = self.win_of_x11(&x) { self.unminimize(id); } }

    fn resize_request(&mut self, _: XwmId, x: X11Surface, _b: u32, edge: ResizeEdge) {
        let edges = match edge {
            ResizeEdge::Left => 1, ResizeEdge::Right => 2, ResizeEdge::Top => 4, ResizeEdge::Bottom => 8,
            ResizeEdge::TopLeft => 5, ResizeEdge::TopRight => 6, ResizeEdge::BottomLeft => 9, ResizeEdge::BottomRight => 10,
        };
        if let Some(id) = self.win_of_x11(&x) { self.client_drag(id, Some(edges), None); }
    }

    fn move_request(&mut self, _: XwmId, x: X11Surface, _b: u32) {
        if let Some(id) = self.win_of_x11(&x) { self.client_drag(id, None, None); }
    }

    fn allow_selection_access(&mut self, _: XwmId, _: SelectionTarget) -> bool {
        // only the program with the keyboard reads the clipboard
        let kb = self.seat.get_keyboard().unwrap();
        matches!(kb.current_focus(), Some(KFocus::X11(_)))
    }

    fn send_selection(&mut self, _: XwmId, sel: SelectionTarget, mime: String, fd: OwnedFd) {
        match sel {
            SelectionTarget::Clipboard => { let _ = request_data_device_client_selection(&self.seat, mime, fd); }
            SelectionTarget::Primary => { let _ = request_primary_client_selection(&self.seat, mime, fd); }
        }
    }

    fn new_selection(&mut self, _: XwmId, sel: SelectionTarget, mimes: Vec<String>) {
        match sel {
            SelectionTarget::Clipboard => set_data_device_selection(&self.dh, &self.seat, mimes, ()),
            SelectionTarget::Primary => set_primary_selection(&self.dh, &self.seat, mimes, ()),
        }
    }

    fn cleared_selection(&mut self, _: XwmId, sel: SelectionTarget) {
        match sel {
            SelectionTarget::Clipboard => if current_data_device_selection_userdata(&self.seat).is_some() { clear_data_device_selection(&self.dh, &self.seat) },
            SelectionTarget::Primary => if current_primary_selection_userdata(&self.seat).is_some() { clear_primary_selection(&self.dh, &self.seat) },
        }
    }
}
