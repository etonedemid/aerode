//! Keyboard, pointer and frame clicks.
use super::decor::Btn;
use super::keys::Action;
use super::{decor::TOP, is_app, Backend, Drag, Rect, Win, Wm};
use smithay::backend::input::{AbsolutePositionEvent, Axis, AxisSource, ButtonState, DeviceCapability, Event, GestureBeginEvent, GestureEndEvent, GestureSwipeUpdateEvent as _, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent};
use smithay::desktop::{layer_map_for_output, Window, WindowSurfaceType};
use smithay::input::keyboard::{FilterResult, KeyboardTarget, KeysymHandle, ModifiersState, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent, RelativeMotionEvent};
use smithay::input::Seat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{IsAlive, Logical, Point, Serial, SERIAL_COUNTER};
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, Layer};
use smithay::xwayland::X11Surface;
use std::borrow::Cow;
use std::time::Instant;

/// Who has the keyboard. An X11 window has to hear about it through the X connection as well.
#[derive(Clone, Debug, PartialEq)]
pub enum KFocus { Surface(WlSurface), X11(X11Surface) }

impl KFocus {
    pub fn of_window(w: &Window) -> Option<KFocus> {
        if let Some(x) = w.x11_surface() { return Some(KFocus::X11(x.clone())); }
        w.wl_surface().map(|s| KFocus::Surface((*s).clone()))
    }
}

impl IsAlive for KFocus {
    fn alive(&self) -> bool { match self { KFocus::Surface(s) => s.alive(), KFocus::X11(x) => x.alive() } }
}

impl WaylandFocus for KFocus {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self { KFocus::Surface(s) => Some(Cow::Borrowed(s)), KFocus::X11(x) => x.wl_surface().map(Cow::Owned) }
    }
}

impl From<smithay::desktop::PopupKind> for KFocus {
    fn from(p: smithay::desktop::PopupKind) -> KFocus { KFocus::Surface(p.wl_surface().clone()) }
}

impl From<KFocus> for WlSurface {
    fn from(f: KFocus) -> WlSurface {
        // a dead X11 surface has nothing to point at; popup grabs only use it while alive
        match f { KFocus::Surface(s) => s, KFocus::X11(x) => x.wl_surface().expect("focused X11 window has a surface") }
    }
}

impl KeyboardTarget<Wm> for KFocus {
    fn enter(&self, seat: &Seat<Wm>, data: &mut Wm, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        match self { KFocus::Surface(s) => KeyboardTarget::enter(s, seat, data, keys, serial), KFocus::X11(x) => KeyboardTarget::enter(x, seat, data, keys, serial) }
    }
    fn leave(&self, seat: &Seat<Wm>, data: &mut Wm, serial: Serial) {
        match self { KFocus::Surface(s) => KeyboardTarget::leave(s, seat, data, serial), KFocus::X11(x) => KeyboardTarget::leave(x, seat, data, serial) }
    }
    fn key(&self, seat: &Seat<Wm>, data: &mut Wm, key: KeysymHandle<'_>, state: KeyState, serial: Serial, time: u32) {
        match self { KFocus::Surface(s) => KeyboardTarget::key(s, seat, data, key, state, serial, time), KFocus::X11(x) => KeyboardTarget::key(x, seat, data, key, state, serial, time) }
    }
    fn modifiers(&self, seat: &Seat<Wm>, data: &mut Wm, m: ModifiersState, serial: Serial) {
        match self { KFocus::Surface(s) => KeyboardTarget::modifiers(s, seat, data, m, serial), KFocus::X11(x) => KeyboardTarget::modifiers(x, seat, data, m, serial) }
    }
}

/// The lock program runs; only its surfaces show or get input until it exits with status 0.
pub struct Lock { pub child: Option<std::process::Child>, pub cmd: String }

#[derive(Clone, Debug)]
pub struct InputConfig { pub repeat_delay: i32, pub repeat_rate: i32, pub pointer_speed: f64, pub left_handed: bool, pub natural_scroll: bool }

impl InputConfig {
    /// `input.json`, written by Mouse and Keyboard Properties.
    pub fn load() -> InputConfig {
        let v: serde_json::Value = std::fs::read_to_string(crate::config::conf_dir().join("input.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        InputConfig {
            repeat_delay: v["repeat_delay"].as_i64().unwrap_or(500) as i32,
            repeat_rate: v["repeat_rate"].as_i64().unwrap_or(30) as i32,
            pointer_speed: v["pointer_speed"].as_f64().unwrap_or(0.0),
            left_handed: v["left_handed"].as_bool().unwrap_or(false),
            natural_scroll: v["natural_scroll"].as_bool().unwrap_or(false),
        }
    }
    pub fn save(&self) {
        let v = serde_json::json!({"repeat_delay": self.repeat_delay, "repeat_rate": self.repeat_rate, "pointer_speed": self.pointer_speed, "left_handed": self.left_handed, "natural_scroll": self.natural_scroll});
        let _ = std::fs::write(crate::config::conf_dir().join("input.json"), v.to_string());
    }
}

/// `keyboard.json`, written by Region and Language: (layout, variant, options), and the layout list.
pub fn keyboard_from_config() -> ((String, String, String), Vec<String>) {
    let v: serde_json::Value = std::fs::read_to_string(crate::config::conf_dir().join("keyboard.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let get = |k: &str, d: &str| v[k].as_str().unwrap_or(d).to_string();
    let c = (get("layout", "us"), get("variant", ""), get("options", ""));
    let list = c.0.split(',').map(str::to_string).collect();
    (c, list)
}

pub fn xkb_config(c: &(String, String, String)) -> XkbConfig<'_> {
    XkbConfig { layout: &c.0, variant: &c.1, options: if c.2.is_empty() { None } else { Some(c.2.clone()) }, ..Default::default() }
}

/// What is under a global point, topmost first.
pub enum Under {
    /// a client surface; `Some(window)` when it belongs to a managed window
    Surface(WlSurface, Point<f64, Logical>, Option<u32>),
    LayerSurface(WlSurface, Point<f64, Logical>, bool),
    Frame(u32),
    Nothing,
}

enum Hit { Button(Btn), Caption, Edge(u8) }

/// Resize zones around a frame: (x, y, w, h), 12 of them, wide grips at the corners.
fn zone_rects(fw: i32, fh: i32) -> [(i32, i32, i32, i32); 12] {
    let (b, t, k) = (8, 8, 16);
    [
        (k, 0, fw - 2 * k, t), (k, fh - b, fw - 2 * k, b), (0, k, b, fh - 2 * k), (fw - b, k, b, fh - 2 * k),
        (0, 0, k, t), (0, 0, b, k), (fw - k, 0, k, t), (fw - b, 0, b, k),
        (0, fh - b, k, b), (0, fh - k, b, k), (fw - k, fh - b, k, b), (fw - b, fh - k, b, k),
    ]
}

/// Resize edges (1 left, 2 right, 4 top, 8 bottom) of each zone in `zone_rects`.
const ZONE_EDGES: [u8; 12] = [4, 8, 1, 2, 5, 5, 6, 6, 9, 9, 10, 10];

fn edge_cursor(edges: u8) -> &'static str {
    match edges {
        1 => "left_side", 2 => "right_side", 4 => "top_side", 8 => "bottom_side",
        5 => "top_left_corner", 6 => "top_right_corner", 9 => "bottom_left_corner", _ => "bottom_right_corner",
    }
}

impl Wm {
    fn hit(&self, c: &Win, x: i32, y: i32) -> Hit {
        let fr = self.frame_rect(c);
        for (b, r) in self.decor.button_rects(fr.w, c.maximized, c.kind == super::Kind::Utility, &self.buttons_for(c)) {
            if x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3] { return Hit::Button(b); }
        }
        if c.maximized || c.fullscreen || !c.resizable { return Hit::Caption; }
        // exactly the zones that carry the resize cursors: a click resizes only where the cursor says so
        for ((zx, zy, zw, zh), e) in zone_rects(fr.w, fr.h).into_iter().zip(ZONE_EDGES) {
            if x >= zx && y >= zy && x < zx + zw && y < zy + zh { return Hit::Edge(e); }
        }
        Hit::Caption
    }

    fn lock_allows(&self, s: &WlSurface) -> bool {
        let Some(l) = &self.lock else { return true };
        let pid = l.child.as_ref().map(|c| c.id() as i32);
        self.dh.get_client(s.id()).ok().and_then(|c| c.get_data::<super::ClientState>().and_then(|d| d.pid)) == pid && pid.is_some()
    }

    pub fn lock_surface_allowed(&self, s: &WlSurface) -> bool { self.lock_allows(s) }

    pub fn under(&self, p: Point<f64, Logical>) -> Under {
        let (ix, iy) = (p.x as i32, p.y as i32);
        let output = self.space.output_under(p).next().or_else(|| self.space.outputs().next());
        let layer_hit = |layers: &[Layer]| -> Option<Under> {
            let o = output?;
            let ol = self.space.output_geometry(o)?.loc;
            let map = layer_map_for_output(o);
            for &ly in layers {
                for l in map.layers_on(ly).rev() {
                    let g = map.layer_geometry(l)?;
                    let at = (ol + g.loc).to_f64();
                    if let Some((s, sp)) = l.surface_under(p - at, WindowSurfaceType::ALL) {
                        if !self.lock_allows(&s) { continue; }
                        let wants = l.cached_state().keyboard_interactivity != KeyboardInteractivity::None;
                        return Some(Under::LayerSurface(s, (sp + ol + g.loc).to_f64(), wants));
                    }
                }
            }
            None
        };
        if let Some(u) = layer_hit(&[Layer::Overlay]) { return u; }
        if self.lock.is_some() { return Under::Nothing; }
        let fullscreen_on_top = self.stack.iter().rev().filter_map(|w| self.wins.get(w)).find(|c| self.should_show(c)).is_some_and(|c| c.fullscreen);
        if !fullscreen_on_top { if let Some(u) = layer_hit(&[Layer::Top]) { return u; } }
        for id in self.stack.iter().rev() {
            let Some(c) = self.wins.get(id) else { continue };
            if !self.should_show(c) { continue; }
            let at = Point::<i32, Logical>::from((c.geom.x, c.geom.y)) - c.window.geometry().loc;
            if let Some((s, sp)) = c.window.surface_under(p - at.to_f64(), WindowSurfaceType::ALL) {
                return Under::Surface(s, (sp + at).to_f64(), Some(*id));
            }
            if c.framed && !c.fullscreen && self.frame_rect(c).contains(ix, iy) { return Under::Frame(*id); }
        }
        if let Some(u) = layer_hit(&[Layer::Bottom, Layer::Background]) { return u; }
        Under::Nothing
    }

    fn pointer_focus(&self, p: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.drag.is_some() { return None; }
        match self.under(p) {
            Under::Surface(s, l, _) | Under::LayerSurface(s, l, _) => Some((s, l)),
            _ => None,
        }
    }

    fn clamp_to_outputs(&self, p: Point<f64, Logical>) -> Point<f64, Logical> {
        let mons = self.monitors();
        if mons.iter().any(|m| m.contains(p.x as i32, p.y as i32)) || mons.is_empty() { return p; }
        // to the nearest monitor
        let best = mons.iter().map(|m| {
            let x = p.x.clamp(m.x as f64, (m.x + m.w - 1) as f64);
            let y = p.y.clamp(m.y as f64, (m.y + m.h - 1) as f64);
            ((x - p.x).powi(2) + (y - p.y).powi(2), x, y)
        }).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        (best.1, best.2).into()
    }

    /// Any input: wakes the display and restarts the idle clock.
    fn activity(&mut self) {
        self.last_input = Instant::now();
        if self.dpms_off { self.set_dpms(true); }
        let seat = self.seat.clone();
        self.idle_notifier.notify_activity(&seat);
    }

    pub fn process_input<I: InputBackend>(&mut self, event: InputEvent<I>) where I::Device: 'static {
        match event {
            InputEvent::Keyboard { event } => self.on_key::<I>(event),
            InputEvent::PointerMotion { event } => self.on_relative_motion::<I>(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.activity();
                let Some(o) = self.space.outputs().next() else { return };
                let Some(g) = self.space.output_geometry(o) else { return };
                let pos = event.position_transformed(g.size) + g.loc.to_f64();
                self.on_motion(pos, event.time_msec());
            }
            InputEvent::PointerButton { event } => self.on_button::<I>(event),
            InputEvent::PointerAxis { event } => self.on_axis::<I>(event),
            InputEvent::GestureSwipeBegin { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_swipe_begin(self, &GestureSwipeBeginEvent { serial: SERIAL_COUNTER.next_serial(), time: event.time_msec(), fingers: event.fingers() });
            }
            InputEvent::GestureSwipeUpdate { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_swipe_update(self, &GestureSwipeUpdateEvent { time: event.time_msec(), delta: event.delta() });
            }
            InputEvent::GestureSwipeEnd { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_swipe_end(self, &GestureSwipeEndEvent { serial: SERIAL_COUNTER.next_serial(), time: event.time_msec(), cancelled: event.cancelled() });
            }
            InputEvent::DeviceAdded { device } => self.device_added::<I>(device),
            _ => {}
        }
    }

    fn device_added<I: InputBackend>(&mut self, device: I::Device) where I::Device: 'static {
        // only libinput devices can be configured
        let any: Box<dyn std::any::Any> = Box::new(device);
        if let Ok(d) = any.downcast::<smithay::reexports::input::Device>() {
            let mut d = *d;
            if d.has_capability(smithay::reexports::input::DeviceCapability::Pointer) { self.configure_pointer(&mut d); self.devices.push(d); }
        }
        let _ = DeviceCapability::Pointer;
    }

    pub fn configure_pointer(&self, d: &mut smithay::reexports::input::Device) {
        let c = &self.input_config;
        let _ = d.config_accel_set_speed(c.pointer_speed);
        let _ = d.config_left_handed_set(c.left_handed);
        let _ = d.config_scroll_set_natural_scroll_enabled(c.natural_scroll);
        let _ = d.config_tap_set_enabled(true);
    }

    fn on_relative_motion<I: InputBackend>(&mut self, event: I::PointerMotionEvent) {
        self.activity();
        let pointer = self.seat.get_pointer().unwrap();
        let cur = pointer.current_location();
        let time = event.time_msec();
        let (delta, delta_u) = (event.delta(), event.delta_unaccel());
        let utime = event.time();
        // a locked or confined pointer
        let mut locked = false;
        let focus = pointer.current_focus();
        if let Some(f) = &focus {
            with_pointer_constraint(f, &pointer, |c| {
                if let Some(c) = c { if c.is_active() && matches!(&*c, PointerConstraint::Locked(_)) { locked = true; } }
            });
        }
        let serial_focus = self.pointer_focus(cur);
        pointer.relative_motion(self, serial_focus, &RelativeMotionEvent { delta, delta_unaccel: delta_u, utime });
        if locked { pointer.frame(self); return; }
        let pos = self.clamp_to_outputs(cur + delta);
        self.on_motion(pos, time);
    }

    fn on_motion(&mut self, pos: Point<f64, Logical>, time: u32) {
        self.needs_redraw = true; // the cursor is drawn with the scene
        let (rx, ry) = (pos.x as i32, pos.y as i32);
        if self.drag.is_some() { self.drag_motion(rx, ry); }
        else { self.hover_frames(pos); }
        let focus = self.pointer_focus(pos);
        let pointer = self.seat.get_pointer().unwrap();
        if let Some((s, _)) = &focus {
            with_pointer_constraint(s, &pointer, |c| { if let Some(c) = c { if !c.is_active() { c.activate(); } } });
        }
        pointer.motion(self, focus, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
        pointer.frame(self);
    }

    /// Hover highlight on caption buttons and the resize cursors.
    fn hover_frames(&mut self, pos: Point<f64, Logical>) {
        let under = self.under(pos);
        let (rx, ry) = (pos.x as i32, pos.y as i32);
        let mut hover_on: Option<(u32, Option<Btn>)> = None;
        self.frame_cursor = None;
        if let Under::Frame(id) = under {
            let c = &self.wins[&id];
            let fr = self.frame_rect(c);
            match self.hit(c, rx - fr.x, ry - fr.y) {
                Hit::Button(b) => { hover_on = Some((id, Some(b))); self.frame_cursor = Some("left_ptr"); }
                Hit::Edge(e) => { hover_on = Some((id, None)); self.frame_cursor = Some(edge_cursor(e)); }
                Hit::Caption => { hover_on = Some((id, None)); self.frame_cursor = Some("left_ptr"); }
            }
        }
        let now = Instant::now();
        let target = hover_on.and_then(|h| h.1.map(|b| (h.0, b)));
        let ids: Vec<u32> = self.wins.iter().filter(|(_, c)| c.hover.is_some()).map(|(i, _)| *i).collect();
        for id in ids {
            if target.map(|t| t.0) != Some(id) {
                let c = self.wins.get_mut(&id).unwrap();
                let old = c.hover.take();
                c.glow.hover(old, None, now);
                c.frame_cache = None;
                self.needs_redraw = true;
            }
        }
        if let Some((id, b)) = target {
            let c = self.wins.get_mut(&id).unwrap();
            if c.hover != Some(b) {
                let old = c.hover.replace(b);
                c.glow.hover(old, Some(b), now);
                c.frame_cache = None;
                self.needs_redraw = true;
            }
        }
    }

    fn drag_motion(&mut self, rx: i32, ry: i32) {
        match self.drag {
            Some((w, Drag::Move { cx, cy, ox, oy, from_max })) => {
                let Some(c) = self.wins.get(&w) else { return };
                if (rx - cx).abs() < 4 && (ry - cy).abs() < 4 && c.geom.x == ox { return; }
                self.wins.get_mut(&w).unwrap().anim = None;
                let c = &self.wins[&w];
                if from_max {
                    // dragging a maximised/snapped window restores it under the pointer
                    let r = c.restore.unwrap_or(Rect { w: c.geom.w * 2 / 3, h: c.geom.h * 2 / 3, ..c.geom });
                    let frac = (cx - c.geom.x) as f64 / c.geom.w.max(1) as f64;
                    let nx = rx - (r.w as f64 * frac) as i32;
                    let c = self.wins.get_mut(&w).unwrap();
                    c.maximized = false; c.snapped = None; c.restore = None; c.frame_cache = None;
                    let g = Rect { x: nx, y: ry + 10, ..r };
                    self.configure(w, g);
                    self.send_states(w);
                    self.drag = Some((w, Drag::Move { cx: rx, cy: ry, ox: g.x, oy: g.y, from_max: false }));
                    return;
                }
                let min_y = self.monitors().iter().map(|m| m.y).min().unwrap_or(0);
                let floor = if c.framed { min_y + TOP / 2 } else { min_y };
                let g = Rect { x: ox + rx - cx, y: (oy + ry - cy).max(floor), ..c.geom };
                self.configure(w, g);
                let (arrange, flips) = self.arrange_options();
                if self.kbd_drag.is_none() && flips > 0 && self.shake.step(w, rx, ry, flips) { self.aero_shake(w); }
                // Aero Snap preview when touching a screen edge
                let mons = self.monitors();
                let m = mons.iter().copied().find(|m| m.contains(rx, ry)).unwrap_or_default();
                let side = if self.kbd_drag.is_some() || !arrange { None }
                    else if rx <= m.x + 1 { Some(-1) } else if rx >= m.x + m.w - 2 { Some(1) } else if ry <= m.y + 1 { Some(0) } else { None };
                if side != self.snap_preview.as_ref().map(|s| s.1) {
                    match side {
                        Some(s) if self.wins[&w].resizable && is_app(&self.wins[&w]) => self.show_snap_preview(w, s, rx, ry),
                        _ => self.hide_snap_preview(),
                    }
                }
            }
            Some((w, Drag::Resize { edges, start, px, py })) => {
                self.wins.get_mut(&w).map(|c| c.anim = None);
                let (dx, dy) = (rx - px, ry - py);
                let mut g = start;
                if edges & 1 != 0 { g.x = start.x + dx; g.w = start.w - dx; }
                if edges & 2 != 0 { g.w = start.w + dx; }
                if edges & 4 != 0 { g.y = start.y + dy; g.h = start.h - dy; }
                if edges & 8 != 0 { g.h = start.h + dy; }
                let min = self.wins.get(&w).map(|c| c.min_size).unwrap_or((80, 30));
                if g.w < min.0.max(100) { if edges & 1 != 0 { g.x = start.x + start.w - min.0.max(100); } g.w = min.0.max(100); }
                if g.h < min.1.max(30) { if edges & 4 != 0 { g.y = start.y + start.h - min.1.max(30); } g.h = min.1.max(30); }
                if let Some(c) = self.wins.get_mut(&w) { c.snapped = None; c.maximized = false; }
                // Wayland paces this by itself: only the newest size goes out, once the client has answered
                let waiting = self.wins.get(&w).is_some_and(|c| c.pending.is_some() && c.window.toplevel().is_some_and(|t| t.current_state().size != c.pending.map(|p| (p.w, p.h).into())));
                if waiting { self.resize_next = Some((w, g)); } else { self.configure(w, g); }
                // top or bottom border at the screen edge: preview of the vertical maximise
                let m = self.monitors().iter().copied().find(|m| m.contains(rx, ry)).unwrap_or_default();
                let vert = self.kbd_drag.is_none() && self.arrange_options().0 && ((edges == 4 && ry <= m.y + 1) || (edges == 8 && ry >= m.y + m.h - 2));
                if vert != (self.snap_preview.as_ref().map(|s| s.1) == Some(2)) {
                    self.hide_snap_preview();
                    if vert && self.wins.get(&w).map(|c| c.resizable && is_app(c)).unwrap_or(false) { self.show_snap_preview(w, 2, rx, ry); }
                }
            }
            _ => {}
        }
    }

    fn on_button<I: InputBackend>(&mut self, event: I::PointerButtonEvent) {
        self.activity();
        let pointer = self.seat.get_pointer().unwrap();
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let state = event.state();
        let pos = pointer.current_location();
        if state == ButtonState::Pressed { if self.press(button, pos) { return; } }
        else if self.drag.is_some() {
            self.release(pos);
            pointer.frame(self);
            let focus = self.pointer_focus(pos);
            pointer.motion(self, focus, &MotionEvent { location: pos, serial, time: event.time_msec() });
            pointer.frame(self);
            return;
        }
        pointer.button(self, &ButtonEvent { button, state, serial, time: event.time_msec() });
        pointer.frame(self);
    }

    /// A pointer press: focus and frame clicks. True when the client must not see it.
    fn press(&mut self, button: u32, pos: Point<f64, Logical>) -> bool {
        const LEFT: u32 = 0x110; const RIGHT: u32 = 0x111; const MIDDLE: u32 = 0x112;
        if self.lock.is_some() { return false; }
        if self.kbd_drag.is_some() { self.end_drag(); return true; }
        let (rx, ry) = (pos.x as i32, pos.y as i32);
        match self.under(pos) {
            Under::Surface(_, _, Some(id)) => {
                // also when already active: the keyboard may be with Start or a flyout, which then closes
                self.focus(id);
                // Super+drag moves windows (Plasma-style extra; Alt+click stays with the app like on Windows)
                let logo = self.seat.get_keyboard().unwrap().modifier_state().logo;
                if logo && button == LEFT {
                    let c = &self.wins[&id];
                    self.drag = Some((id, Drag::Move { cx: rx, cy: ry, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized }));
                    return true;
                }
                false
            }
            Under::LayerSurface(s, _, wants) => {
                let kb = self.seat.get_keyboard().unwrap();
                if self.is_panel(&s) {
                    // the taskbar never takes the keyboard from the active window; a click on it does close Start or a flyout
                    let target = self.wins.get(&self.active).and_then(|c| KFocus::of_window(&c.window));
                    if kb.current_focus() != target { kb.set_focus(self, target, SERIAL_COUNTER.next_serial()); }
                } else if self.is_desktop(&s) {
                    self.clear_focus();
                    kb.set_focus(self, Some(KFocus::Surface(s)), SERIAL_COUNTER.next_serial());
                } else if wants {
                    kb.set_focus(self, Some(KFocus::Surface(s)), SERIAL_COUNTER.next_serial());
                }
                false
            }
            Under::Frame(id) => {
                self.focus(id);
                let c = &self.wins[&id];
                let fr = self.frame_rect(c);
                let (x, y) = (rx - fr.x, ry - fr.y);
                let last = self.last_click;
                let double = last.0 == id && last.1.elapsed().as_millis() < 450 && (last.2 - x).abs() < 5 && (last.3 - y).abs() < 5;
                match (button, self.hit(c, x, y)) {
                    (LEFT, Hit::Button(b)) => {
                        let c = self.wins.get_mut(&id).unwrap();
                        c.pressed = Some(b);
                        c.frame_cache = None;
                        self.drag = Some((id, Drag::Button { btn: b }));
                        self.needs_redraw = true;
                    }
                    (LEFT, Hit::Caption) => {
                        if double {
                            let m = c.maximized || c.snapped.is_some();
                            if c.resizable && c.can_maximize { self.set_maximized(id, !m); }
                            self.last_click.0 = 0;
                            return true;
                        }
                        let d = Drag::Move { cx: rx, cy: ry, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized || c.snapped.is_some() };
                        self.last_click = (id, Instant::now(), x, y);
                        self.drag = Some((id, d));
                        self.frame_cursor = Some("left_ptr");
                    }
                    (LEFT, Hit::Edge(edges)) => {
                        // double-click on the top or bottom border: maximise vertically / restore (Win7)
                        if (edges == 4 || edges == 8) && double {
                            let snapped2 = c.snapped == Some(2);
                            let can = c.resizable && is_app(c);
                            self.last_click.0 = 0;
                            if snapped2 { self.set_maximized(id, false); } else if can { self.snap(id, 2); }
                            return true;
                        }
                        let d = Drag::Resize { edges, start: c.geom, px: rx, py: ry };
                        self.last_click = (id, Instant::now(), x, y);
                        self.begin_resize(id);
                        self.drag = Some((id, d));
                        self.frame_cursor = Some(edge_cursor(edges));
                    }
                    (RIGHT, Hit::Caption) | (RIGHT, Hit::Button(_)) => self.spawn_sysmenu(id, rx, ry),
                    (MIDDLE, Hit::Caption) => { self.stack.retain(|x| *x != id); self.stack.insert(0, id); self.restack(); }
                    _ => {}
                }
                true
            }
            Under::Surface(_, _, None) | Under::Nothing => {
                self.switcher_cancel();
                false
            }
        }
    }

    fn layer_namespace_is(&self, s: &WlSurface, ns: &str) -> bool {
        self.outputs().iter().any(|o| {
            let map = layer_map_for_output(o);
            let hit = map.layer_for_surface(s, WindowSurfaceType::ALL).is_some_and(|l| l.namespace() == ns);
            hit
        })
    }
    fn is_panel(&self, s: &WlSurface) -> bool { self.layer_namespace_is(s, "aerode-glass-panel") }
    fn is_desktop(&self, s: &WlSurface) -> bool { self.layer_namespace_is(s, "aerode-desktop") }

    fn begin_resize(&mut self, id: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as S;
        if let Some(t) = self.wins.get(&id).and_then(|c| c.window.toplevel().cloned()) {
            t.with_pending_state(|s| { s.states.set(S::Resizing); });
            t.send_pending_configure();
        }
    }

    fn release(&mut self, pos: Point<f64, Logical>) {
        let Some((w, drag)) = self.drag else { return };
        let snap = self.snap_preview.as_ref().map(|s| s.1);
        let wa = self.snap_wa;
        self.end_drag();
        self.drop_snap_preview();
        match drag {
            Drag::Button { btn } => {
                let Some(c) = self.wins.get_mut(&w) else { return };
                c.pressed = None;
                c.frame_cache = None;
                let c = &self.wins[&w];
                let fr = self.frame_rect(c);
                let inside = fr.contains(pos.x as i32, pos.y as i32) && matches!(self.hit(c, pos.x as i32 - fr.x, pos.y as i32 - fr.y), Hit::Button(b) if b == btn);
                if inside {
                    match btn {
                        Btn::Close => self.close(w),
                        Btn::Min => if c.can_minimize { self.minimize(w) },
                        Btn::Max => { let m = c.maximized || c.snapped.is_some(); self.set_maximized(w, !m); }
                        Btn::Help => {}
                    }
                }
            }
            Drag::Move { .. } => {
                if let Some(side) = snap {
                    if self.wins.contains_key(&w) {
                        match (side, wa) {
                            (0, Some(wa)) => { self.begin_geom_anim(w); self.maximize_in(w, wa) }
                            (s, Some(wa)) => { self.begin_geom_anim(w); self.snap_in(w, s, wa) }
                            (0, None) => self.set_maximized(w, true),
                            (s, None) => self.snap(w, s),
                        }
                    }
                }
            }
            Drag::Resize { start, .. } => {
                if snap == Some(2) {
                    if let Some(c) = self.wins.get_mut(&w) { c.snapped = None; c.maximized = false; }
                    self.snap(w, 2);
                    if let Some(c) = self.wins.get_mut(&w) { if c.snapped == Some(2) { c.restore = Some(start); } }
                }
            }
        }
    }

    fn on_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        self.activity();
        let source = event.source();
        let amount = |a: Axis| event.amount(a).unwrap_or_else(|| event.amount_v120(a).unwrap_or(0.0) * 15.0 / 120.0);
        let mut frame = AxisFrame::new(event.time_msec()).source(source);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let v = amount(axis);
            if v != 0.0 {
                frame = frame.value(axis, v);
                if let Some(d) = event.amount_v120(axis) { frame = frame.v120(axis, d as i32); }
            }
            if source == AxisSource::Finger && event.amount(axis) == Some(0.0) { frame = frame.stop(axis); }
        }
        let p = self.seat.get_pointer().unwrap();
        p.axis(self, frame);
        p.frame(self);
    }

    /// A client asked to be moved or resized (client-side title bar, _NET_WM_MOVERESIZE).
    pub fn client_drag(&mut self, id: u32, edges: Option<u8>, serial: Option<Serial>) {
        let pointer = self.seat.get_pointer().unwrap();
        if let Some(s) = serial { if !pointer.has_grab(s) { return; } }
        let p = pointer.current_location();
        let (rx, ry) = (p.x as i32, p.y as i32);
        let c = &self.wins[&id];
        let d = match edges {
            None => Drag::Move { cx: rx, cy: ry, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized || c.snapped.is_some() },
            Some(e) => Drag::Resize { edges: e, start: c.geom, px: rx, py: ry },
        };
        if edges.is_some() { self.begin_resize(id); }
        self.frame_cursor = Some(edges.map(edge_cursor).unwrap_or("left_ptr"));
        self.drag = Some((id, d));
        // the press belongs to us now; the client sees the pointer leave
        pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), 0);
        let time = self.start.elapsed().as_millis() as u32;
        pointer.motion(self, None, &MotionEvent { location: p, serial: SERIAL_COUNTER.next_serial(), time });
        pointer.frame(self);
    }

    /// Alt+Space > Move/Size: the arrow keys drive the drag, Enter ends it, Esc puts it back.
    pub fn keyboard_moveresize(&mut self, w: u32, size: bool) {
        self.end_drag();
        let Some(c) = self.wins.get(&w) else { return };
        let g = c.geom;
        let (px, py) = if size { (g.x + g.w - 1, g.y + g.h - 1) } else { (g.x + g.w / 2, g.y - if c.framed { TOP / 2 } else { -12 }) };
        self.warp((px as f64, py as f64).into());
        if size { self.begin_resize(w); }
        self.drag = Some((w, if size { Drag::Resize { edges: 10, start: g, px, py } } else { Drag::Move { cx: px, cy: py, ox: g.x, oy: g.y, from_max: false } }));
        self.frame_cursor = Some(if size { "bottom_right_corner" } else { "fleur" });
        self.kbd_drag = Some((g.x, g.y));
        self.kbd_drag_orig = Some(g);
        self.focus(w);
    }

    pub fn warp(&mut self, p: Point<f64, Logical>) {
        let pointer = self.seat.get_pointer().unwrap();
        let time = self.start.elapsed().as_millis() as u32;
        let focus = self.pointer_focus(p);
        pointer.motion(self, focus, &MotionEvent { location: p, serial: SERIAL_COUNTER.next_serial(), time });
        pointer.frame(self);
        self.needs_redraw = true;
    }

    fn kbd_drag_key(&mut self, sym: u32) {
        use smithay::input::keyboard::keysyms as ks;
        let Some((w, _)) = self.drag else { return };
        match sym {
            ks::KEY_Return | ks::KEY_KP_Enter => self.end_drag(),
            ks::KEY_Escape => { let o = self.kbd_drag_orig; self.end_drag(); if let Some(o) = o { self.configure(w, o); } }
            ks::KEY_Left | ks::KEY_Up | ks::KEY_Right | ks::KEY_Down => {
                let (dx, dy) = match sym { ks::KEY_Left => (-10.0, 0.0), ks::KEY_Up => (0.0, -10.0), ks::KEY_Right => (10.0, 0.0), _ => (0.0, 10.0) };
                let p = self.seat.get_pointer().unwrap().current_location() + Point::from((dx, dy));
                self.warp(p);
                self.drag_motion(p.x as i32, p.y as i32);
            }
            _ => {}
        }
    }

    fn on_key<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        self.activity();
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&event);
        let code = event.key_code();
        let state = event.state();
        let press = state == KeyState::Pressed;
        let keyboard = self.seat.get_keyboard().unwrap();
        let locked = self.lock.is_some();
        let action = keyboard.input::<(Option<Action>, Option<u32>), _>(self, code, state, serial, time, |wm, m, h| {
            let sym = h.raw_latin_sym_or_raw_current_sym().map(|k| k.raw()).unwrap_or_else(|| h.modified_sym().raw());
            let modified = h.modified_sym().raw();
            // a key held from before a lock or a chord: its release was filtered with the press
            if wm.suppressed.contains(&code.raw()) {
                if !press { wm.suppressed.retain(|c| *c != code.raw()); }
                return FilterResult::Intercept((None, None));
            }
            if locked { return FilterResult::Forward; }
            if wm.kbd_drag.is_some() {
                if press { return FilterResult::Intercept((None, Some(modified))); }
                return FilterResult::Forward;
            }
            let switching = wm.switcher.is_some();
            let repeat = false;
            match wm.keys.lookup(sym, modified, m, press, repeat, switching) {
                Some(a) => {
                    if press { wm.suppressed.push(code.raw()); }
                    FilterResult::Intercept((Some(a), None))
                }
                None => {
                    // inside Alt+Tab no key reaches the clients
                    if switching && press { wm.suppressed.push(code.raw()); return FilterResult::Intercept((None, None)); }
                    FilterResult::Forward
                }
            }
        });
        // a group-switch key (grp:...) changed the language: tell the indicator
        let group = keyboard.with_xkb_state(self, |ctx| ctx.xkb().lock().unwrap().active_layout().0);
        if group != self.last_group { self.last_group = group; self.publish_event("keyboard"); }
        if let Some((a, kbd)) = action {
            if let Some(sym) = kbd { self.kbd_drag_key(sym); }
            if let Some(a) = a { self.run_action(a); }
        }
    }

    fn run_action(&mut self, action: Action) {
        use Action::*;
        let act = self.active;
        match action {
            Spawn(cmd) => { crate::util::spawn(&["sh", "-c", &cmd]); }
            Close => if act != 0 { self.close(act) },
            Maximize => if act != 0 { let m = self.wins.get(&act).is_some_and(|c| c.maximized); if !m { self.set_maximized(act, true); } },
            RestoreOrMinimize => if let Some(c) = self.wins.get(&act) {
                if c.maximized || c.snapped.is_some() { self.set_maximized(act, false) } else { self.minimize(act) }
            },
            SnapLeft | SnapRight => {
                let side = if matches!(action, SnapLeft) { -1 } else { 1 };
                if let Some(c) = self.wins.get(&act) {
                    // pressing again from the opposite snap restores (Win7 behaviour)
                    if c.snapped == Some(-side) { self.set_maximized(act, false) } else { self.snap(act, side) }
                }
            }
            ShowDesktop => { let s = !self.showing_desktop; self.set_showing_desktop(s); }
            MinimizeOthers => if act != 0 { self.aero_shake(act); },
            MaximizeVert => if let Some(c) = self.wins.get(&act) { if is_app(c) && c.resizable && c.snapped != Some(2) { self.snap(act, 2); } },
            MonitorLeft | MonitorRight => if act != 0 { self.move_to_monitor(act, if matches!(action, MonitorLeft) { -1 } else { 1 }); },
            SwitchNext | SwitchPrev => self.switcher_step(matches!(action, SwitchNext)),
            SwitchCommit => self.switch_commit(),
            SwitchCancel => self.switcher_cancel(),
            SysMenu => if let Some(c) = self.wins.get(&act) { let (x, y) = (c.geom.x, c.geom.y); self.spawn_sysmenu(act, x, y); },
            DesktopLeft => { let d = self.current_desktop.saturating_sub(1); self.set_desktop(d); }
            DesktopRight => { let d = (self.current_desktop + 1).min(self.num_desktops - 1); self.set_desktop(d); }
            DesktopAdd => { let n = self.num_desktops + 1; self.set_num_desktops(n); self.set_desktop(n - 1); }
            DesktopRemove => { let n = self.num_desktops.saturating_sub(1).max(1); self.set_num_desktops(n); }
            Vt(n) => if let Backend::Udev(u) = &mut self.backend { u.change_vt(n); },
        }
    }

    pub fn spawn_sysmenu(&self, id: u32, x: i32, y: i32) {
        crate::util::spawn(&["aerode", "sysmenu", &id.to_string(), &x.to_string(), &y.to_string()]);
    }

    /// The keyboard's surface went away: back to the active window.
    pub fn restore_focus_if_lost(&mut self) {
        let kb = self.seat.get_keyboard().unwrap();
        let lost = kb.current_focus().is_none_or(|f| !f.alive());
        if !lost { return; }
        if self.lock.is_some() { return; }
        let target = self.wins.get(&self.active).and_then(|c| KFocus::of_window(&c.window));
        kb.set_focus(self, target, SERIAL_COUNTER.next_serial());
    }

    /// The lock program's overlay wants the keyboard the moment it shows.
    pub fn lock_committed(&mut self, surface: &WlSurface) {
        if self.lock.is_none() || !self.lock_allows(surface) { return; }
        let kb = self.seat.get_keyboard().unwrap();
        if kb.current_focus().is_none_or(|f| !f.alive() || !f.wl_surface().is_some_and(|s| self.lock_allows(&s))) {
            kb.set_focus(self, Some(KFocus::Surface(surface.clone())), SERIAL_COUNTER.next_serial());
        }
    }

    pub fn lock_session(&mut self, cmd: String) {
        if self.lock.is_some() { return; }
        self.end_drag();
        self.switcher_cancel();
        let kb = self.seat.get_keyboard().unwrap();
        kb.set_focus(self, None, SERIAL_COUNTER.next_serial());
        let child = std::process::Command::new("sh").args(["-c", &format!("exec {cmd}")]).spawn().ok();
        self.lock = Some(Lock { child, cmd });
        self.needs_redraw = true;
    }

    /// The lock program ended: 0 unlocks; anything else (crash, kill) starts it again, still locked.
    pub fn lock_tick(&mut self) {
        let Some(l) = self.lock.as_mut() else { return };
        let status = l.child.as_mut().and_then(|c| c.try_wait().ok().flatten());
        let dead = l.child.is_none() || status.is_some();
        if !dead { return; }
        if status.is_some_and(|s| s.success()) {
            self.lock = None;
            let target = self.wins.get(&self.active).and_then(|c| KFocus::of_window(&c.window));
            let kb = self.seat.get_keyboard().unwrap();
            kb.set_focus(self, target, SERIAL_COUNTER.next_serial());
        } else {
            let cmd = l.cmd.clone();
            l.child = std::process::Command::new("sh").args(["-c", &format!("exec {cmd}")]).spawn().ok();
        }
        self.needs_redraw = true;
    }
}

