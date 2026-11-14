//! The server side of `crate::wmipc`.
use super::{Backend, Rect, Wm};
use crate::wmipc::{Output as IOutput, Req, State, Win as IWin};
use anyhow::Result;
use serde_json::{json, Value};
use smithay::reexports::calloop::{generic::Generic, Interest, Mode, PostAction};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};

pub fn listen(wm: &mut Wm) -> Result<()> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| anyhow::anyhow!("no XDG_RUNTIME_DIR"))?;
    let path = std::path::PathBuf::from(dir).join(format!("aerode-{}.sock", wm.socket_name));
    let _ = std::fs::remove_file(&path);
    let l = UnixListener::bind(&path)?;
    l.set_nonblocking(true)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    wm.ipc_path = path;
    wm.handle.insert_source(Generic::new(l, Interest::READ, Mode::Level), |_, l, wm: &mut Wm| {
        // Safety: the listener is not dropped
        while let Ok((s, _)) = unsafe { l.get_mut() }.accept() {
            let _ = s.set_nonblocking(true);
            let mut buf = Vec::new();
            let source = Generic::new(s, Interest::READ, Mode::Level);
            let _ = wm.handle.insert_source(source, move |_, s, wm: &mut Wm| {
                let stream = unsafe { s.get_mut() };
                Ok(read_requests(wm, stream, &mut buf))
            });
        }
        Ok(PostAction::Continue)
    }).map_err(|e| anyhow::anyhow!("ipc source: {e}"))?;
    Ok(())
}

fn read_requests(wm: &mut Wm, s: &mut UnixStream, buf: &mut Vec<u8>) -> PostAction {
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => return PostAction::Remove,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(_) => return PostAction::Remove,
        }
    }
    while let Some(i) = buf.iter().position(|b| *b == b'\n') {
        let line: Vec<u8> = buf.drain(..=i).collect();
        let Ok(req) = serde_json::from_slice::<Req>(&line) else {
            let _ = s.write_all(b"null\n");
            continue;
        };
        if matches!(req, Req::Subscribe) {
            if let Ok(c) = s.try_clone() { wm.subscribers.push(c); }
            continue;
        }
        let reply = wm.handle_req(req);
        let mut out = reply.to_string();
        out.push('\n');
        let _ = s.set_nonblocking(false);
        let _ = s.write_all(out.as_bytes());
        let _ = s.set_nonblocking(true);
    }
    PostAction::Continue
}

impl Wm {
    pub fn state_snapshot(&self) -> State {
        let windows = self.stack.iter().filter_map(|id| self.wins.get(id)).map(|c| {
            let f = self.frame_rect(c);
            let g = c.geom;
            IWin {
                id: c.id, title: c.title.clone(), app_id: c.app_id.clone(), pid: c.pid, minimized: c.minimized, urgent: c.urgent,
                desktop: c.desktop, maximized: c.maximized, fullscreen: c.fullscreen, resizable: c.resizable,
                can_minimize: c.can_minimize, can_maximize: c.can_maximize, above: c.above,
                taskbar: !c.skip_taskbar && !matches!(c.kind, super::Kind::Menu | super::Kind::Splash),
                frame: [f.x, f.y, f.w, f.h], geom: [g.x, g.y, g.w, g.h], dialog: c.kind == super::Kind::Dialog, transient_for: c.transient_for,
            }
        }).collect();
        State { windows, active: self.active, showing_desktop: self.showing_desktop, current_desktop: self.current_desktop, num_desktops: self.num_desktops }
    }

    fn handle_req(&mut self, req: Req) -> Value {
        match req {
            Req::Subscribe => {}
            Req::State => return serde_json::to_value(self.state_snapshot()).unwrap_or(Value::Null),
            Req::Activate { id } => self.activate(id),
            Req::Minimize { id } => self.minimize(id),
            Req::Close { id } => self.close(id),
            Req::SetMaximized { id, on } => self.set_maximized(id, on),
            Req::KeyboardMoveResize { id, size } => self.keyboard_moveresize(id, size),
            Req::Place { id, x, y, w, h, outer } => {
                if let Some(c) = self.wins.get(&id) {
                    let r = Rect { x, y, w, h };
                    let r = if outer { self.fit_rect(c, r, c.maximized) } else { r };
                    self.configure(id, r);
                }
            }
            Req::SetShowingDesktop { on } => self.set_showing_desktop(on),
            Req::SetDesktop { n } => self.set_desktop(n),
            Req::SetNumDesktops { n } => self.set_num_desktops(n),
            Req::IconGeometry { id, x, y, w, h } => if let Some(c) = self.wins.get_mut(&id) { c.icon_geometry = Some(Rect { x, y, w, h }); },
            Req::Peek { id, on } => self.start_peek(id, on),
            Req::Thumb { id, w, h } => return self.thumbnail(id, w, h).unwrap_or(Value::Null),
            Req::Screenshot => return self.screenshot().unwrap_or(Value::Null),
            Req::Pointer => { let (x, y) = self.pointer_pos(); return json!({"x": x, "y": y}); }
            Req::Idle => return json!({"ms": self.last_input.elapsed().as_millis() as u64, "dpms": self.dpms_timeout, "saver": self.saver_timeout, "repeat_delay": self.input_config.repeat_delay, "repeat_rate": self.input_config.repeat_rate}),
            Req::SetDpmsTimeout { secs } => self.dpms_timeout = secs,
            Req::DpmsOff => self.set_dpms(false),
            Req::SetSaverTimeout { secs, cmd } => { self.saver_timeout = secs; self.saver_cmd = cmd; }
            Req::Lock { cmd } => self.lock_session(cmd),
            Req::Locked => return json!({"locked": self.lock.is_some()}),
            Req::SetKeyboard { layout, variant, options } => self.set_keyboard(layout, variant, options),
            Req::KeyboardState => {
                let kb = self.seat.get_keyboard().unwrap();
                let group = kb.with_xkb_state(self, |ctx| ctx.xkb().lock().unwrap().active_layout().0);
                return json!({"group": group, "layouts": self.kb_layouts, "layout": self.kb_config.0, "variant": self.kb_config.1, "options": self.kb_config.2});
            }
            Req::SetGroup { group } => {
                let kb = self.seat.get_keyboard().unwrap();
                kb.with_xkb_state(self, |mut ctx| ctx.set_layout(smithay::input::keyboard::Layout(group)));
                self.publish_event("keyboard");
            }
            Req::SetInput { repeat_delay, repeat_rate, pointer_speed, left_handed, natural_scroll } => {
                let c = &mut self.input_config;
                if let Some(v) = repeat_delay { c.repeat_delay = v; }
                if let Some(v) = repeat_rate { c.repeat_rate = v; }
                if let Some(v) = pointer_speed { c.pointer_speed = v; }
                if let Some(v) = left_handed { c.left_handed = v; }
                if let Some(v) = natural_scroll { c.natural_scroll = v; }
                c.save();
                let (rd, rr) = (c.repeat_delay, c.repeat_rate);
                self.seat.get_keyboard().unwrap().change_repeat_info(rr, rd);
                let mut devs = std::mem::take(&mut self.devices);
                for d in &mut devs { self.configure_pointer(d); }
                self.devices = devs;
            }
            Req::SetAccessKeys { .. } => {}
            Req::Outputs => return json!({"outputs": self.output_list()}),
            Req::SetOutput { name, enabled, w, h, refresh_mhz, x, y, scale, transform } => self.set_output(&name, enabled, w, h, refresh_mhz, x, y, scale, transform),
            Req::SetColorization { value } => {
                self.live_colorization = value;
                let c = value.as_ref().and_then(|v| crate::glass::from_prop(v)).unwrap_or_else(|| { crate::config::reload(); crate::config::get().colorization.dwm() });
                if self.glass.set_colorization(c) {
                    for w in self.wins.values_mut() { w.frame_cache = None; }
                    self.needs_redraw = true;
                }
                self.publish_event("colorization");
            }
            Req::GetColorization => return json!({"value": self.live_colorization}),
            Req::ReloadConfig => {
                crate::config::reload();
                self.glass.set_wallpaper(self.screen_size());
                self.load_session_settings();
                for w in self.wins.values_mut() { w.frame_cache = None; }
                self.needs_redraw = true;
            }
            Req::SetCursor { theme, size } => {
                self.cursors.set_theme(&theme, size);
                // SAFETY: single thread; children inherit it
                unsafe { std::env::set_var("XCURSOR_THEME", &theme); std::env::set_var("XCURSOR_SIZE", size.to_string()); }
                self.needs_redraw = true;
            }
            Req::Spawn { cmd } => { crate::util::spawn(&["sh", "-c", &cmd]); }
            Req::Quit => self.signal.stop(),
        }
        json!({"ok": true})
    }

    fn start_peek(&mut self, id: u32, on: bool) {
        if !on { self.peek = None; self.peek_sheets.clear(); self.needs_redraw = true; return; }
        let rects: Vec<(u32, Rect)> = self.wins.values().filter(|c| c.id != id && super::is_app(c) && self.should_show(c)).map(|c| (c.id, self.frame_rect(c))).collect();
        self.peek_sheets = rects.into_iter().filter_map(|(w, r)| Some((w, self.paint_sheet(r)?))).collect();
        self.peek(id, true);
    }

    pub fn publish_event(&mut self, ev: &str) {
        let line = format!("{{\"ev\":\"{ev}\"}}\n");
        self.subscribers.retain_mut(|s| s.write_all(line.as_bytes()).is_ok());
    }

    /// Tell the shell the X display for X11 programs it launches.
    pub fn publish_env(&mut self) { self.publish_event("env"); }

    fn set_keyboard(&mut self, layout: String, variant: String, options: String) {
        let c = (layout, variant, options);
        let kb = self.seat.get_keyboard().unwrap();
        if kb.set_xkb_config(self, super::input::xkb_config(&c)).is_err() { return; }
        self.kb_layouts = c.0.split(',').map(str::to_string).collect();
        let v = json!({"layout": c.0, "variant": c.1, "options": c.2});
        let _ = std::fs::write(crate::config::conf_dir().join("keyboard.json"), v.to_string());
        self.kb_config = c;
        self.publish_event("keyboard");
    }

    fn output_list(&self) -> Vec<IOutput> {
        let mut list: Vec<IOutput> = self.space.outputs().map(|o| {
            let g = self.space.output_geometry(o).unwrap_or_default();
            let mode = o.current_mode();
            let phys = o.physical_properties();
            IOutput {
                name: o.name(), make: phys.make.clone(), model: phys.model.clone(), enabled: true, connected: true,
                x: g.loc.x, y: g.loc.y, w: mode.map(|m| m.size.w).unwrap_or(0), h: mode.map(|m| m.size.h).unwrap_or(0),
                refresh_mhz: mode.map(|m| m.refresh).unwrap_or(0), scale: o.current_scale().fractional_scale(),
                transform: transform_code(o.current_transform()), modes: o.modes().iter().map(|m| (m.size.w, m.size.h, m.refresh)).collect(),
                phys_mm: (phys.size.w, phys.size.h),
            }
        }).collect();
        if let Backend::Udev(u) = &self.backend { list.extend(u.disabled_outputs()); }
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    #[allow(clippy::too_many_arguments)]
    fn set_output(&mut self, name: &str, enabled: Option<bool>, w: Option<i32>, h: Option<i32>, refresh: Option<i32>, x: Option<i32>, y: Option<i32>, scale: Option<f64>, transform: Option<u32>) {
        if let Some(o) = self.outputs().into_iter().find(|o| o.name() == name) {
            let tf = transform.map(transform_from_code);
            let sc = scale.map(smithay::output::Scale::Fractional);
            let pos = match (x, y) { (Some(x), Some(y)) => Some((x, y)), _ => None };
            o.change_current_state(None, tf, sc, pos.map(Into::into));
            if let Some(p) = pos { self.space.map_output(&o, p); }
        }
        if let Backend::Udev(_) = self.backend { super::udev::apply_output(self, name, enabled, w, h, refresh); }
        else if let (Some(w), Some(h)) = (w, h) { let _ = (w, h); }
        // what Control Panel saved is reapplied at the next logon
        super::udev::save_output_config(self);
        self.glass.set_wallpaper(self.screen_size());
        self.update_workarea();
        for c in self.wins.values_mut() { c.frame_cache = None; }
        self.needs_redraw = true;
        self.publish_event("outputs");
    }
}

pub fn transform_code(t: smithay::utils::Transform) -> u32 {
    use smithay::utils::Transform::*;
    match t { Normal => 0, _90 => 1, _180 => 2, _270 => 3, Flipped => 4, Flipped90 => 5, Flipped180 => 6, Flipped270 => 7 }
}

pub fn transform_from_code(c: u32) -> smithay::utils::Transform {
    use smithay::utils::Transform::*;
    match c { 1 => _90, 2 => _180, 3 => _270, 4 => Flipped, 5 => Flipped90, 6 => Flipped180, 7 => Flipped270, _ => Normal }
}
