//! Wayland protocol glue: what each global does to the window list.
use super::{ClientState, Kind, Rect, Win, Wm};
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::utils::{on_commit_buffer_handler, RendererSurfaceStateUserData};
use smithay::desktop::{find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface, PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window, WindowSurfaceType};
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::Interest;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as XdgMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{Mode as KdeMode, OrgKdeKwinServerDecoration};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_output, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::{Logical, Point, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states, BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes};
use smithay::wayland::dmabuf::{get_dmabuf, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::drm_syncobj::{DrmSyncobjCachedState, DrmSyncobjHandler, DrmSyncobjState};
use smithay::wayland::fractional_scale::FractionalScaleHandler;
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraintsHandler};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::wlr_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::selection::data_device::{set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler};
use smithay::wayland::selection::primary_selection::{set_primary_focus, PrimarySelectionHandler, PrimarySelectionState};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};
use smithay::wayland::shell::wlr_layer::{Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::tablet_manager::TabletSeatHandler;
use smithay::wayland::xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData};
use smithay::{delegate_compositor, delegate_cursor_shape, delegate_data_control, delegate_data_device, delegate_dmabuf, delegate_drm_syncobj, delegate_fractional_scale, delegate_idle_inhibit, delegate_idle_notify, delegate_kde_decoration, delegate_layer_shell, delegate_output, delegate_pointer_constraints, delegate_pointer_gestures, delegate_presentation, delegate_primary_selection, delegate_relative_pointer, delegate_seat, delegate_shm, delegate_single_pixel_buffer, delegate_tablet_manager, delegate_text_input_manager, delegate_viewporter, delegate_virtual_keyboard_manager, delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell};
use std::os::fd::OwnedFd;

use super::input::KFocus;

impl CompositorHandler for Wm {
    fn compositor_state(&mut self) -> &mut CompositorState { &mut self.compositor_state }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(s) = client.get_data::<smithay::xwayland::XWaylandClientData>() { return &s.compositor_state; }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    /// Hold a commit back until its buffer is ready to read (explicit sync point, or implicit fence).
    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, |wm, _dh, surface| {
            let mut acquire = None;
            let dmabuf = with_states(surface, |s| {
                acquire.clone_from(&s.cached_state.get::<DrmSyncobjCachedState>().pending().acquire_point);
                s.cached_state.get::<SurfaceAttributes>().pending().buffer.as_ref().and_then(|b| match b {
                    BufferAssignment::NewBuffer(b) => get_dmabuf(b).cloned().ok(),
                    _ => None,
                })
            });
            let Some(dmabuf) = dmabuf else { return };
            let Some(client) = surface.client() else { return };
            if let Some(point) = acquire {
                if let Ok((blocker, source)) = point.generate_blocker() {
                    let c = client.clone();
                    if wm.handle.insert_source(source, move |_, _, wm: &mut Wm| {
                        let dh = wm.dh.clone();
                        wm.client_compositor_state(&c).blocker_cleared(wm, &dh);
                        Ok(())
                    }).is_ok() { add_blocker(surface, blocker); return; }
                }
            }
            if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
                if wm.handle.insert_source(source, move |_, _, wm: &mut Wm| {
                    let dh = wm.dh.clone();
                    wm.client_compositor_state(&client).blocker_cleared(wm, &dh);
                    Ok(())
                }).is_ok() { add_blocker(surface, blocker); }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.needs_redraw = true;
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(p) = get_parent(&root) { root = p; }
            if let Some(id) = self.win_of_surface(&root) {
                self.wins[&id].window.on_commit();
                if &root == surface { self.window_committed(id); }
            }
        }
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(p)) = self.popups.find_popup(surface) {
            if !p.is_initial_configure_sent() { let _ = p.send_configure(); }
        }
        self.layer_committed(surface);
        self.lock_committed(surface);
    }
}
delegate_compositor!(Wm);

fn c_pending_clear(c: &Win) -> bool { c.pending.is_none() }

pub fn has_buffer(surface: &WlSurface) -> bool {
    with_states(surface, |s| s.data_map.get::<RendererSurfaceStateUserData>().is_some_and(|d| d.lock().unwrap().buffer().is_some()))
}

impl Wm {
    pub fn win_of_surface(&self, root: &WlSurface) -> Option<u32> {
        self.wins.values().find(|c| c.window.wl_surface().is_some_and(|s| *s == *root)).map(|c| c.id)
    }

    fn win_of_toplevel(&self, t: &ToplevelSurface) -> Option<u32> { self.win_of_surface(t.wl_surface()) }

    /// A new window record, not on screen until its first buffer.
    pub fn add_window(&mut self, window: Window, kind: Kind, framed: bool) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.wins.insert(id, Win {
            id, window, kind, title: String::new(), app_id: String::new(), pid: 0, icon: None, geom: Rect::default(), restore: None,
            maximized: false, fullscreen: false, minimized: false, above: false, skip_taskbar: false, urgent: false, transient_for: 0,
            min_size: (0, 0), resizable: true, can_minimize: true, can_maximize: true, desktop: self.current_desktop,
            hover: None, pressed: None, snapped: None, framed, mapped: false, glow: Default::default(), focus_fade: None,
            frame_cache: None, glass_ids: Default::default(), anim: None, icon_geometry: None, pending: None,
        });
        id
    }

    /// A client's root surface committed: map it the first time, follow the size it chose.
    fn window_committed(&mut self, id: u32) {
        let c = &self.wins[&id];
        let Some(t) = c.window.toplevel().cloned() else {
            // X11: the buffer shows up after map; geometry comes through configure_notify
            if !c.mapped && c.window.wl_surface().is_some_and(|s| has_buffer(&s)) { self.first_buffer(id); }
            return;
        };
        let initial_sent = with_states(t.wl_surface(), |s| s.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().initial_configure_sent);
        if !initial_sent {
            // server-side frames from the first configure
            let framed = c.framed;
            t.with_pending_state(|s| s.decoration_mode = Some(if framed { XdgMode::ServerSide } else { XdgMode::ClientSide }));
            t.send_configure();
            return;
        }
        if !has_buffer(t.wl_surface()) {
            if c.mapped { self.unmap_window(id); }
            return;
        }
        self.read_toplevel_props(id);
        let c = &self.wins[&id];
        if !c.mapped { self.first_buffer(id); return; }
        let size = c.window.geometry().size;
        let c = self.wins.get_mut(&id).unwrap();
        // a configure the client has now answered: take the position that goes with it
        if let Some(p) = c.pending {
            if t.current_state().size == Some((p.w, p.h).into()) {
                c.pending = None;
                // the far edge stays put if the client didn't take exactly the size offered
                let (x, y) = (if p.x != c.geom.x { p.x + p.w - size.w } else { p.x }, if p.y != c.geom.y { p.y + p.h - size.h } else { p.y });
                c.geom.x = x;
                c.geom.y = y;
            }
        }
        if (size.w, size.h) != (c.geom.w, c.geom.h) && size.w > 0 && size.h > 0 {
            c.geom.w = size.w;
            c.geom.h = size.h;
            c.frame_cache = None;
            self.state_changed = true;
        }
        self.sync_mapped(id);
        if c_pending_clear(&self.wins[&id]) {
            if let Some((w, g)) = self.resize_next.take_if(|r| r.0 == id) { self.configure(w, g); }
        }
    }

    fn read_toplevel_props(&mut self, id: u32) {
        let c = self.wins.get_mut(&id).unwrap();
        let Some(t) = c.window.toplevel().cloned() else { return };
        let (min, max) = with_states(t.wl_surface(), |s| {
            let mut g = s.cached_state.get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
            let cur = g.current();
            (cur.min_size, cur.max_size)
        });
        c.min_size = (min.w, min.h);
        let fixed = min.w > 0 && min.w == max.w && min.h == max.h;
        if c.resizable == fixed { c.resizable = !fixed; c.frame_cache = None; self.state_changed = true; }
    }

    /// The window has something to show: place it and put it on screen.
    pub fn first_buffer(&mut self, id: u32) {
        let (x11_geom, size) = {
            let c = &self.wins[&id];
            (c.window.x11_surface().map(|x| x.geometry()), c.window.geometry().size)
        };
        self.read_toplevel_props(id);
        self.refresh_title(id);
        let parent = self.wins[&id].transient_for;
        let (w, h) = (size.w.max(1), size.h.max(1));
        let g = match x11_geom {
            // an X11 program that asked for a position, or an override-redirect window
            Some(g) if g.loc.x != 0 || g.loc.y != 0 || self.wins[&id].kind == Kind::Menu => Rect::from_smithay(g),
            _ => self.place_new(id, parent, w, h),
        };
        let kind = self.wins[&id].kind;
        let c = self.wins.get_mut(&id).unwrap();
        c.geom = g;
        c.mapped = true;
        c.frame_cache = None;
        if let Some(x) = c.window.x11_surface() { let _ = x.configure(g.smithay()); }
        self.stack.push(id);
        self.sync_mapped(id);
        if kind != Kind::Menu {
            if self.window_anims() { self.begin_open_anim(id); }
            self.focus(id);
        } else { self.restack(); }
        self.state_changed = true;
    }

    /// Centered on its owner, otherwise cascading across the work area under the pointer.
    fn place_new(&mut self, id: u32, parent: u32, w: i32, h: i32) -> Rect {
        let mon = self.pointer_monitor();
        let wa = self.workarea_for(mon);
        let c = &self.wins[&id];
        let (b, t) = (self.border_of(c), self.top_of(c));
        if let Some(p) = self.wins.get(&parent) {
            let fr = self.frame_rect(p);
            return Rect { x: fr.x + (fr.w - w) / 2, y: (fr.y + (fr.h - h) / 2).max(wa.y + t), w, h };
        }
        let n = (self.cascade % 8) as i32;
        self.cascade += 1;
        let x = wa.x + b + 26 + n * 26;
        let y = wa.y + t + 26 + n * 26;
        let x = if x + w + b > wa.x + wa.w { (wa.x + (wa.w - w) / 2).max(wa.x + b) } else { x };
        let y = if y + h + b > wa.y + wa.h { (wa.y + t).max(wa.y + (wa.h - h) / 2) } else { y };
        Rect { x, y, w, h }
    }

    pub fn unmap_window(&mut self, id: u32) {
        if self.drag.is_some_and(|d| d.0 == id) { self.end_drag(); }
        let Some(c) = self.wins.get_mut(&id) else { return };
        c.mapped = false;
        c.frame_cache = None;
        let win = c.window.clone();
        self.space.unmap_elem(&win);
        self.stack.retain(|x| *x != id);
        if self.active == id { self.active = 0; self.focus_fallback(id); }
        self.state_changed = true;
        self.needs_redraw = true;
    }

    pub fn remove_window(&mut self, id: u32) {
        self.unmap_window(id);
        self.wins.remove(&id);
        for c in self.wins.values_mut() { if c.transient_for == id { c.transient_for = 0; } }
    }

    pub fn refresh_title(&mut self, id: u32) {
        let c = self.wins.get_mut(&id).unwrap();
        let old = (c.title.clone(), c.app_id.clone());
        if let Some(t) = c.window.toplevel() {
            let (title, app_id) = with_states(t.wl_surface(), |s| {
                let d = s.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap();
                (d.title.clone().unwrap_or_default(), d.app_id.clone().unwrap_or_default())
            });
            c.title = title;
            c.app_id = app_id;
        }
        if let Some(x) = c.window.x11_surface() {
            c.title = x.title();
            c.app_id = x.class();
            c.pid = x.pid().unwrap_or(0);
        }
        if c.pid == 0 {
            if let Some(s) = c.window.wl_surface() {
                if let Ok(cl) = self.dh.get_client(s.id()) {
                    c.pid = cl.get_data::<ClientState>().and_then(|d| d.pid).unwrap_or(0) as u32;
                }
            }
        }
        if (c.title.clone(), c.app_id.clone()) != old {
            c.frame_cache = None;
            if c.app_id != old.1 || c.icon.is_none() { c.icon = super::render::theme_icon(&c.app_id); }
            self.state_changed = true;
            self.needs_redraw = true;
        }
    }

    fn layer_committed(&mut self, surface: &WlSurface) {
        let Some(o) = self.outputs().into_iter().find(|o| layer_map_for_output(o).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).is_some()) else { return };
        let (changed, exclusive) = {
            let mut map = layer_map_for_output(&o);
            let before = map.non_exclusive_zone();
            map.arrange();
            let initial = with_states(surface, |s| s.data_map.get::<LayerSurfaceData>().unwrap().lock().unwrap().initial_configure_sent);
            let layer = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned();
            if !initial { if let Some(l) = &layer { l.layer_surface().send_configure(); } }
            let wants = layer.as_ref().is_some_and(|l| {
                use smithay::wayland::shell::wlr_layer::KeyboardInteractivity as K;
                l.cached_state().keyboard_interactivity == K::Exclusive && has_buffer(surface)
            });
            (before != map.non_exclusive_zone(), wants)
        };
        if changed { self.update_workarea(); }
        // a menu or flyout that wants the keyboard gets it when it appears
        if exclusive && self.lock.is_none() {
            let kb = self.seat.get_keyboard().unwrap();
            if kb.current_focus().and_then(|f| f.wl_surface().map(|s| s.into_owned())).as_ref() != Some(surface) {
                kb.set_focus(self, Some(KFocus::Surface(surface.clone())), smithay::utils::SERIAL_COUNTER.next_serial());
            }
        }
    }

    /// Where a popup's root surface sits on the global plane.
    fn root_location(&self, root: &WlSurface) -> Option<(Point<i32, Logical>, Output)> {
        if let Some(id) = self.win_of_surface(root) {
            let c = &self.wins[&id];
            let o = self.output_for_rect(&c.geom)?;
            return Some(((c.geom.x, c.geom.y).into(), o));
        }
        for o in self.outputs() {
            let map = layer_map_for_output(&o);
            if let Some(l) = map.layer_for_surface(root, WindowSurfaceType::TOPLEVEL) {
                let g = map.layer_geometry(l)?;
                let ol = self.space.output_geometry(&o)?.loc;
                return Some((ol + g.loc, o.clone()));
            }
        }
        None
    }

    pub fn output_for_rect(&self, r: &Rect) -> Option<Output> {
        let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
        self.space.outputs().find(|o| self.space.output_geometry(o).is_some_and(|g| Rect::from_smithay(g).contains(cx, cy)))
            .or_else(|| self.space.outputs().next()).cloned()
    }

    /// Keep menus in the monitor's work area, off the taskbar, like Windows (GTK's menus flip and
    /// slide as the positioner asks).
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else { return };
        let Some((loc, o)) = self.root_location(&root) else { return };
        let Some(og) = self.space.output_geometry(&o) else { return };
        let mut target = layer_map_for_output(&o).non_exclusive_zone();
        target.loc += og.loc;
        // the taskbar's own menus open over it, as in Win7
        if target.size.w <= 0 || target.size.h <= 0 || !target.contains(loc) { target = og; }
        target.loc -= loc;
        target.loc -= get_popup_toplevel_coords(&kind);
        popup.with_pending_state(|s| s.geometry = s.positioner.get_unconstrained_geometry(target));
    }

    /// Output a layer surface was created on.
    fn layer_output(&self, wl_output: Option<&wl_output::WlOutput>) -> Option<Output> {
        wl_output.and_then(Output::from_resource).or_else(|| {
            let mons = self.monitors();
            let m = mons.get(self.pointer_monitor())?;
            self.output_for_rect(m)
        })
    }
}

impl BufferHandler for Wm { fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {} }
impl ShmHandler for Wm { fn shm_state(&self) -> &ShmState { &self.shm_state } }
delegate_shm!(Wm);

impl DmabufHandler for Wm {
    fn dmabuf_state(&mut self) -> &mut DmabufState { &mut self.dmabuf_state }
    fn dmabuf_imported(&mut self, _g: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        if self.import_dmabuf(&dmabuf) { let _ = notifier.successful::<Wm>(); } else { notifier.failed(); }
    }
}
delegate_dmabuf!(Wm);

impl DrmSyncobjHandler for Wm {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> { self.syncobj_state.as_mut() }
}
delegate_drm_syncobj!(Wm);

impl SeatHandler for Wm {
    type KeyboardFocus = KFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Wm> { &mut self.seat_state }
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&KFocus>) {
        let client = focused.and_then(|f| f.wl_surface()).and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh, seat, client.clone());
        set_primary_focus(&self.dh, seat, client);
    }
    fn cursor_image(&mut self, _: &Seat<Self>, image: CursorImageStatus) { self.cursor_status = image; self.needs_redraw = true; }
}
delegate_seat!(Wm);

impl TabletSeatHandler for Wm {}
delegate_tablet_manager!(Wm);
delegate_cursor_shape!(Wm);
delegate_viewporter!(Wm);
delegate_presentation!(Wm);
delegate_pointer_gestures!(Wm);
delegate_relative_pointer!(Wm);
delegate_text_input_manager!(Wm);
delegate_virtual_keyboard_manager!(Wm);
delegate_single_pixel_buffer!(Wm);

impl FractionalScaleHandler for Wm {}
delegate_fractional_scale!(Wm);

impl PointerConstraintsHandler for Wm {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &smithay::input::pointer::PointerHandle<Self>) {
        if pointer.current_focus().as_ref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |c| { if let Some(c) = c { c.activate(); } });
        }
    }
    fn cursor_position_hint(&mut self, _: &WlSurface, _: &smithay::input::pointer::PointerHandle<Self>, _: Point<f64, Logical>) {}
}
delegate_pointer_constraints!(Wm);

impl IdleNotifierHandler for Wm { fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> { &mut self.idle_notifier } }
delegate_idle_notify!(Wm);

impl IdleInhibitHandler for Wm {
    fn inhibit(&mut self, s: WlSurface) { self.inhibitors.push(s); }
    fn uninhibit(&mut self, s: WlSurface) { self.inhibitors.retain(|x| *x != s); }
}
delegate_idle_inhibit!(Wm);

impl XdgActivationHandler for Wm {
    fn activation_state(&mut self) -> &mut XdgActivationState { &mut self.xdg_activation_state }
    fn request_activation(&mut self, _t: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        let Some(id) = self.win_of_surface(&surface) else { return };
        if data.timestamp.elapsed().as_secs() < 10 { self.activate(id); }
        else if self.active != id { self.wins.get_mut(&id).unwrap().urgent = true; self.state_changed = true; }
    }
}
delegate_xdg_activation!(Wm);

impl XdgDecorationHandler for Wm {
    fn new_decoration(&mut self, t: ToplevelSurface) { self.set_decoration(&t, true); }
    fn request_mode(&mut self, t: ToplevelSurface, mode: XdgMode) { self.set_decoration(&t, mode == XdgMode::ServerSide); }
    fn unset_mode(&mut self, t: ToplevelSurface) { self.set_decoration(&t, true); }
}
delegate_xdg_decoration!(Wm);

impl KdeDecorationHandler for Wm {
    fn kde_decoration_state(&self) -> &KdeDecorationState { &self.kde_decoration_state }
    fn request_mode(&mut self, surface: &WlSurface, deco: &OrgKdeKwinServerDecoration, mode: smithay::reexports::wayland_server::WEnum<KdeMode>) {
        let Ok(mode) = mode.into_result() else { return };
        deco.mode(mode);
        if let Some(id) = self.win_of_surface(surface) {
            let c = self.wins.get_mut(&id).unwrap();
            c.framed = mode == KdeMode::Server;
            c.frame_cache = None;
            self.needs_redraw = true;
        }
    }
}
delegate_kde_decoration!(Wm);

impl Wm {
    fn set_decoration(&mut self, t: &ToplevelSurface, server: bool) {
        let Some(id) = self.win_of_toplevel(t) else { return };
        let was_sent = with_states(t.wl_surface(), |s| s.data_map.get::<XdgToplevelSurfaceData>().unwrap().lock().unwrap().initial_configure_sent);
        let c = self.wins.get_mut(&id).unwrap();
        c.framed = server;
        c.frame_cache = None;
        t.with_pending_state(|s| s.decoration_mode = Some(if server { XdgMode::ServerSide } else { XdgMode::ClientSide }));
        if was_sent { t.send_pending_configure(); }
        self.needs_redraw = true;
    }
}

impl XdgShellHandler for Wm {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState { &mut self.xdg_shell_state }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // GTK3 asks for server-side frames with the KDE protocol after this, others with xdg-decoration
        let id = self.add_window(Window::new_wayland_window(surface), Kind::Normal, false);
        self.refresh_title(id);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.remove_window(id); }
    }

    fn new_popup(&mut self, surface: PopupSurface, _p: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|s| { s.geometry = positioner.get_geometry(); s.positioner = positioner; });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Wm>::from_resource(&seat) else { return };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else { return };
        let root = self.win_of_surface(&root).and_then(|id| KFocus::of_window(&self.wins[&id].window)).unwrap_or(KFocus::Surface(root));
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else { return };
        if let Some(kb) = seat.get_keyboard() {
            if kb.is_grabbed() && !(kb.has_grab(serial) || kb.has_grab(grab.previous_serial().unwrap_or(serial))) {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            kb.set_focus(self, grab.current_grab(), serial);
            kb.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(p) = seat.get_pointer() {
            if p.is_grabbed() && !(p.has_grab(serial) || p.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial()))) {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            p.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.client_drag(id, None, Some(serial)); }
    }

    fn resize_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial, edges: xdg_toplevel::ResizeEdge) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.client_drag(id, Some(edges as u32 as u8), Some(serial)); }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        match self.win_of_toplevel(&surface) {
            Some(id) if self.wins[&id].mapped => self.set_maximized(id, true),
            // before the first buffer: size it with the first configure
            Some(id) => { self.wins.get_mut(&id).unwrap().maximized = false; surface.send_configure(); }
            None => {}
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.set_maximized(id, false); }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _o: Option<wl_output::WlOutput>) {
        if let Some(id) = self.win_of_toplevel(&surface) {
            if self.wins[&id].mapped { self.set_fullscreen(id, true); } else { surface.send_configure(); }
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.set_fullscreen(id, false); }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.minimize(id); }
    }

    fn show_window_menu(&mut self, surface: ToplevelSurface, _s: wl_seat::WlSeat, _serial: Serial, loc: Point<i32, Logical>) {
        if let Some(id) = self.win_of_toplevel(&surface) {
            let g = self.wins[&id].geom;
            self.spawn_sysmenu(id, g.x + loc.x, g.y + loc.y);
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.refresh_title(id); }
    }
    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = self.win_of_toplevel(&surface) { self.refresh_title(id); }
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        let Some(id) = self.win_of_toplevel(&surface) else { return };
        let parent = surface.parent().and_then(|p| self.win_of_surface(&p)).unwrap_or(0);
        let c = self.wins.get_mut(&id).unwrap();
        c.transient_for = parent;
        c.kind = if parent != 0 { Kind::Dialog } else { Kind::Normal };
        c.frame_cache = None;
        self.restack();
    }

    fn popup_destroyed(&mut self, _: PopupSurface) { self.needs_redraw = true; }
}
delegate_xdg_shell!(Wm);

impl WlrLayerShellHandler for Wm {
    fn shell_state(&mut self) -> &mut WlrLayerShellState { &mut self.layer_shell_state }
    fn new_layer_surface(&mut self, surface: WlrLayerSurface, output: Option<wl_output::WlOutput>, _l: Layer, namespace: String) {
        let Some(o) = self.layer_output(output.as_ref()) else { surface.send_close(); return };
        let _ = layer_map_for_output(&o).map_layer(&LayerSurface::new(surface, namespace));
    }
    /// A shell surface's menu (desktop, taskbar, tray) gets its parent only now: xdg_shell's
    /// new_popup could not place it yet.
    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
    }
    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let hit = self.outputs().into_iter().find_map(|o| {
            let map = layer_map_for_output(&o);
            let l = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            drop(map);
            l.map(|l| (o, l))
        });
        if let Some((o, l)) = hit {
            layer_map_for_output(&o).unmap_layer(&l);
            self.update_workarea();
        }
        self.restore_focus_if_lost();
        self.needs_redraw = true;
    }
}
delegate_layer_shell!(Wm);

impl OutputHandler for Wm {}
delegate_output!(Wm);

impl SelectionHandler for Wm {
    type SelectionUserData = ();
    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _s: Seat<Self>) {
        if let Some(x) = self.xwm.as_mut() { let _ = x.new_selection(ty, source.map(|s| s.mime_types())); }
    }
    fn send_selection(&mut self, ty: SelectionTarget, mime: String, fd: OwnedFd, _s: Seat<Self>, _u: &()) {
        let h = self.handle.clone();
        if let Some(x) = self.xwm.as_mut() { let _ = x.send_selection(ty, mime, fd, h); }
    }
}
impl DataDeviceHandler for Wm { fn data_device_state(&self) -> &DataDeviceState { &self.data_device_state } }
impl ClientDndGrabHandler for Wm {
    fn started(&mut self, _src: Option<smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource>, icon: Option<WlSurface>, _s: Seat<Self>) { self.dnd_icon = icon; }
    fn dropped(&mut self, _t: Option<WlSurface>, _v: bool, _s: Seat<Self>) { self.dnd_icon = None; self.needs_redraw = true; }
}
impl ServerDndGrabHandler for Wm {}
delegate_data_device!(Wm);
impl PrimarySelectionHandler for Wm { fn primary_selection_state(&self) -> &PrimarySelectionState { &self.primary_selection_state } }
delegate_primary_selection!(Wm);
impl DataControlHandler for Wm { fn data_control_state(&self) -> &DataControlState { &self.data_control_state } }
delegate_data_control!(Wm);
