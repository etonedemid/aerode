//! The real thing: DRM/KMS outputs through libseat, libinput for input.
use super::render::El;
use super::{Backend, Wm};
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::Fourcc;
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ImportDma, ImportEgl, ImportMemWl};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{all_gpus, primary_gpu, UdevBackend, UdevEvent};
use smithay::desktop::utils::OutputPresentationFeedback;
use smithay::output::{Mode as WlMode, Output, PhysicalProperties};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, RegistrationToken};
use smithay::reexports::drm::control::{connector, crtc, ModeTypeFlags};
use smithay::reexports::drm::Device as _;
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Transform};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;
use smithay::wayland::drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjState};
use smithay_drm_extras::drm_scanner::{DrmScanEvent, DrmScanner};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

const FORMATS: &[Fourcc] = &[Fourcc::Abgr8888, Fourcc::Argb8888];

type Drm = DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, Option<OutputPresentationFeedback>, DrmDeviceFd>;
type Manager = DrmOutputManager<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, Option<OutputPresentationFeedback>, DrmDeviceFd>;

struct Surf {
    output: Output,
    global: smithay::reexports::wayland_server::backend::GlobalId,
    drm: Drm,
    connector: connector::Info,
    /// a frame is on its way to the screen: wait for its vblank
    queued: bool,
    /// the display is off (DPMS)
    off: bool,
    idle_timer: Option<RegistrationToken>,
}

pub struct UdevData {
    pub session: LibSeatSession,
    pub renderer: Option<GlesRenderer>,
    node: Option<DrmNode>,
    manager: Option<Manager>,
    scanner: DrmScanner,
    surfaces: HashMap<crtc::Handle, Surf>,
    /// connected but switched off in Control Panel
    disabled: Vec<(connector::Info, crtc::Handle)>,
    active: bool,
}

impl UdevData {
    pub fn change_vt(&mut self, n: i32) { let _ = self.session.change_vt(n); }

    pub fn disabled_outputs(&self) -> Vec<crate::wmipc::Output> {
        self.disabled.iter().map(|(c, _)| crate::wmipc::Output {
            name: connector_name(c), make: String::new(), model: String::new(), enabled: false, connected: true,
            modes: c.modes().iter().map(|m| { let w = WlMode::from(*m); (w.size.w, w.size.h, w.refresh) }).collect(), ..Default::default()
        }).collect()
    }
}

fn connector_name(c: &connector::Info) -> String { format!("{}-{}", c.interface().as_str(), c.interface_id()) }

/// Manufacturer code and monitor name from the connector's EDID in sysfs.
fn edid_names(connector: &str) -> (String, String) {
    let edid = std::fs::read_dir("/sys/class/drm").into_iter().flatten().flatten()
        .find(|e| e.file_name().to_string_lossy().ends_with(&format!("-{connector}")))
        .and_then(|e| std::fs::read(e.path().join("edid")).ok()).unwrap_or_default();
    parse_edid(&edid)
}

fn parse_edid(e: &[u8]) -> (String, String) {
    if e.len() < 128 { return ("Unknown".into(), "Unknown".into()); }
    let m = u16::from_be_bytes([e[8], e[9]]);
    let letter = |v: u16| (b'@' + (v & 31) as u8) as char;
    let make: String = [letter(m >> 10), letter(m >> 5), letter(m)].iter().collect();
    // detailed descriptors: tag 0xFC is the monitor name
    let model = (0..4).map(|i| &e[54 + i * 18..72 + i * 18]).find(|d| d[..3] == [0, 0, 0] && d[3] == 0xfc)
        .map(|d| String::from_utf8_lossy(&d[5..18]).trim().to_string()).unwrap_or_else(|| "Unknown".into());
    (make, model)
}

/// `outputs.json`: what Control Panel > Display saved, by connector name.
fn saved_outputs() -> serde_json::Value {
    std::fs::read_to_string(crate::config::conf_dir().join("outputs.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_output_config(wm: &mut Wm) {
    let mut v = saved_outputs();
    if !v.is_object() { v = serde_json::json!({}); }
    for o in wm.outputs() {
        let g = wm.space.output_geometry(&o).unwrap_or_default();
        let m = o.current_mode();
        v[o.name()] = serde_json::json!({"enabled": true, "w": m.map(|m| m.size.w), "h": m.map(|m| m.size.h), "refresh": m.map(|m| m.refresh),
            "x": g.loc.x, "y": g.loc.y, "scale": o.current_scale().fractional_scale(), "transform": super::ipc::transform_code(o.current_transform())});
    }
    if let Backend::Udev(u) = &wm.backend {
        for (c, _) in &u.disabled { v[connector_name(c)]["enabled"] = serde_json::json!(false); }
    }
    let _ = std::fs::write(crate::config::conf_dir().join("outputs.json"), v.to_string());
}

pub fn init(wm: &mut Wm, event_loop: &EventLoop<'static, Wm>) -> anyhow::Result<()> {
    let (session, notifier) = LibSeatSession::new().map_err(|e| anyhow::anyhow!("no seat session (run from a display manager or a VT): {e}"))?;
    let seat_name = session.seat();

    // libinput
    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput.udev_assign_seat(&seat_name).map_err(|_| anyhow::anyhow!("libinput seat"))?;
    let input = LibinputInputBackend::new(libinput.clone());
    event_loop.handle().insert_source(input, |ev, _, wm: &mut Wm| {
        if let InputEvent::DeviceRemoved { device } = &ev { wm.devices.retain(|d| d != device); }
        wm.process_input(ev);
    }).map_err(|e| anyhow::anyhow!("libinput source: {e}"))?;

    // VT switches
    event_loop.handle().insert_source(notifier, move |ev, _, wm: &mut Wm| match ev {
        SessionEvent::PauseSession => {
            libinput.suspend();
            let Backend::Udev(u) = &mut wm.backend else { return };
            u.active = false;
            if let Some(m) = &mut u.manager { m.pause(); }
        }
        SessionEvent::ActivateSession => {
            let _ = libinput.resume();
            let Backend::Udev(u) = &mut wm.backend else { return };
            u.active = true;
            if let Some(m) = &mut u.manager { let _ = m.activate(false); }
            for s in u.surfaces.values_mut() { s.queued = false; s.drm.reset_buffers(); }
            wm.needs_redraw = true;
        }
    }).map_err(|e| anyhow::anyhow!("session source: {e}"))?;

    wm.backend = Backend::Udev(UdevData { session, renderer: None, node: None, manager: None, scanner: DrmScanner::new(), surfaces: HashMap::new(), disabled: Vec::new(), active: true });

    // the GPU the boot display is on; the first one otherwise
    let udev = UdevBackend::new(&seat_name)?;
    let primary = std::env::var("AERODE_DRM_DEVICE").ok().map(std::path::PathBuf::from)
        .or_else(|| primary_gpu(&seat_name).ok().flatten())
        .or_else(|| all_gpus(&seat_name).ok()?.into_iter().next())
        .ok_or_else(|| anyhow::anyhow!("no GPU"))?;
    let node = DrmNode::from_path(&primary)?;
    let path = node.node_with_type(NodeType::Primary).and_then(|n| n.ok()).and_then(|n| n.dev_path()).unwrap_or(primary);
    device_added(wm, event_loop, node, &path)?;

    event_loop.handle().insert_source(udev, |ev, _, wm: &mut Wm| match ev {
        UdevEvent::Changed { device_id } => {
            let same = matches!(&wm.backend, Backend::Udev(u) if u.node.is_some_and(|n| n.dev_id() == device_id));
            if same { scan(wm); }
        }
        // ponytail: one GPU drives everything; hot-plugged GPUs are ignored until the next logon
        UdevEvent::Added { .. } | UdevEvent::Removed { .. } => {}
    }).map_err(|e| anyhow::anyhow!("udev source: {e}"))?;
    Ok(())
}

fn device_added(wm: &mut Wm, event_loop: &EventLoop<'static, Wm>, node: DrmNode, path: &Path) -> anyhow::Result<()> {
    let Backend::Udev(u) = &mut wm.backend else { unreachable!() };
    let fd = u.session.open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK).map_err(|e| anyhow::anyhow!("open {}: {e}", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd.clone())?;

    event_loop.handle().insert_source(notifier, |ev, meta, wm: &mut Wm| match ev {
        DrmEvent::VBlank(crtc) => frame_finish(wm, crtc, meta),
        DrmEvent::Error(e) => eprintln!("aerode-wm: drm: {e:?}"),
    }).map_err(|e| anyhow::anyhow!("drm source: {e}"))?;

    // SAFETY: the gbm device outlives the display, both are kept for the whole session
    let display = unsafe { EGLDisplay::new(gbm.clone())? };
    let egl_device = EGLDevice::device_for_display(&display)?;
    let render_node = egl_device.try_get_render_node().ok().flatten().unwrap_or(node);
    let context = EGLContext::new_with_priority(&display, ContextPriority::High)?;
    // SAFETY: the context is current only on this thread
    let mut renderer = unsafe { GlesRenderer::new(context)? };
    wm.shm_state.update_formats(renderer.shm_formats().collect::<Vec<_>>());
    let _ = renderer.bind_wl_display(&wm.dh);

    // clients' buffers: dmabuf with the render node as main device, explicit sync where the kernel can
    let formats = renderer.dmabuf_formats();
    let feedback = DmabufFeedbackBuilder::new(render_node.dev_id(), formats.clone()).build()?;
    wm.dmabuf_global = Some(wm.dmabuf_state.create_global_with_default_feedback::<Wm>(&wm.dh, &feedback));
    if supports_syncobj_eventfd(&fd) {
        wm.syncobj_state = Some(DrmSyncobjState::new::<Wm>(&wm.dh, fd.clone()));
    }

    let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
    let exporter = GbmFramebufferExporter::new(gbm.clone(), Some(render_node).into());
    let render_formats: FormatSet = renderer.egl_context().dmabuf_render_formats().iter().copied().collect();
    let manager = DrmOutputManager::new(drm, allocator, exporter, Some(gbm), FORMATS.iter().copied(), render_formats);

    let Backend::Udev(u) = &mut wm.backend else { unreachable!() };
    u.node = Some(node);
    u.renderer = Some(renderer);
    u.manager = Some(manager);
    scan(wm);
    Ok(())
}

/// Monitors plugged in or out.
fn scan(wm: &mut Wm) {
    let Backend::Udev(u) = &mut wm.backend else { return };
    let Some(m) = &u.manager else { return };
    let Ok(result) = u.scanner.scan_connectors(m.device()) else { return };
    for ev in result {
        match ev {
            DrmScanEvent::Connected { connector, crtc: Some(crtc) } => connected(wm, connector, crtc),
            DrmScanEvent::Disconnected { connector, crtc: Some(crtc) } => disconnected(wm, connector, crtc),
            _ => {}
        }
    }
    wm.glass.set_wallpaper(wm.screen_size());
    wm.update_workarea();
    wm.needs_redraw = true;
    wm.publish_event("outputs");
}

fn connected(wm: &mut Wm, c: connector::Info, crtc: crtc::Handle) {
    let name = connector_name(&c);
    let saved = saved_outputs();
    let saved = saved.get(&name).cloned().unwrap_or_default();
    if saved["enabled"] == serde_json::json!(false) {
        if let Backend::Udev(u) = &mut wm.backend { u.disabled.push((c, crtc)); }
        return;
    }
    let want = (saved["w"].as_i64(), saved["h"].as_i64(), saved["refresh"].as_i64());
    let modes = c.modes();
    let mode_id = modes.iter().position(|m| {
        let w = WlMode::from(*m);
        Some(w.size.w as i64) == want.0 && Some(w.size.h as i64) == want.1 && Some(w.refresh as i64) == want.2
    }).or_else(|| {
        // the preferred resolution at its fastest refresh: monitors mark a 60 Hz mode preferred even when they do 165
        let p = WlMode::from(*modes.iter().find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED)).or(modes.first())?);
        (0..modes.len()).filter(|&i| WlMode::from(modes[i]).size == p.size).max_by_key(|&i| WlMode::from(modes[i]).refresh)
    }).unwrap_or(0);
    let Some(&drm_mode) = modes.get(mode_id) else { return };
    let wl_mode = WlMode::from(drm_mode);

    let x_next = wm.space.outputs().fold(0, |acc, o| acc + wm.space.output_geometry(o).map(|g| g.size.w).unwrap_or(0));
    let pos = (saved["x"].as_i64().map(|v| v as i32).unwrap_or(x_next), saved["y"].as_i64().unwrap_or(0) as i32);
    let scale = saved["scale"].as_f64().unwrap_or(1.0);
    let tf = saved["transform"].as_u64().map(|v| super::ipc::transform_from_code(v as u32)).unwrap_or(Transform::Normal);

    let dh = wm.dh.clone();
    let Backend::Udev(u) = &mut wm.backend else { return };
    let (Some(manager), Some(renderer)) = (u.manager.as_mut(), u.renderer.as_mut()) else { return };
    let (make, model) = edid_names(&name);
    let (pw, ph) = c.size().unwrap_or((0, 0));
    let output = Output::new(name.clone(), PhysicalProperties { size: (pw as i32, ph as i32).into(), subpixel: c.subpixel().into(), make, model });
    for m in modes { output.add_mode(WlMode::from(*m)); }
    let global = output.create_global::<Wm>(&dh);
    output.set_preferred(wl_mode);
    output.change_current_state(Some(wl_mode), Some(tf), Some(smithay::output::Scale::Fractional(scale)), Some(pos.into()));

    // overlay planes break on NVIDIA's driver
    let device = manager.device();
    let nvidia = device.get_driver().map(|d| d.name().to_string_lossy().to_lowercase().contains("nvidia")).unwrap_or(false);
    let Ok(mut planes) = device.planes(&crtc) else { return };
    if nvidia { planes.overlay = vec![]; }

    let drm = match manager.initialize_output::<_, El>(crtc, drm_mode, &[c.handle()], &output, Some(planes), renderer, &DrmOutputRenderElements::default()) {
        Ok(d) => d,
        Err(e) => { eprintln!("aerode-wm: output {name}: {e}"); return; }
    };
    u.surfaces.insert(crtc, Surf { output: output.clone(), global, drm, connector: c, queued: false, off: false, idle_timer: None });
    wm.space.map_output(&output, pos);
    wm.needs_redraw = true;
}

fn disconnected(wm: &mut Wm, c: connector::Info, crtc: crtc::Handle) {
    let Backend::Udev(u) = &mut wm.backend else { return };
    u.disabled.retain(|(d, _)| d.handle() != c.handle());
    if let Some(s) = u.surfaces.remove(&crtc) {
        wm.dh.remove_global::<Wm>(s.global);
        wm.space.unmap_output(&s.output);
    }
}

/// Control Panel > Display: resolution, refresh rate, on/off.
pub fn apply_output(wm: &mut Wm, name: &str, enabled: Option<bool>, w: Option<i32>, h: Option<i32>, refresh: Option<i32>) {
    let Backend::Udev(u) = &mut wm.backend else { return };
    // switched off -> on
    if enabled == Some(true) {
        if let Some(i) = u.disabled.iter().position(|(c, _)| connector_name(c) == name) {
            let (c, crtc) = u.disabled.remove(i);
            let mut v = saved_outputs();
            if v.get(name).is_some() { v[name]["enabled"] = serde_json::json!(true); }
            let _ = std::fs::write(crate::config::conf_dir().join("outputs.json"), v.to_string());
            connected(wm, c, crtc);
            return;
        }
    }
    let Some((&crtc, _)) = u.surfaces.iter().find(|(_, s)| s.output.name() == name) else { return };
    if enabled == Some(false) {
        // the last monitor stays on
        if u.surfaces.len() < 2 { return; }
        let s = u.surfaces.remove(&crtc).unwrap();
        wm.dh.remove_global::<Wm>(s.global);
        wm.space.unmap_output(&s.output);
        let Backend::Udev(u) = &mut wm.backend else { return };
        u.disabled.push((s.connector, crtc));
        return;
    }
    let (Some(w), Some(h)) = (w, h) else { return };
    let (Some(manager), Some(renderer)) = (u.manager.as_mut(), u.renderer.as_mut()) else { return };
    let s = u.surfaces.get_mut(&crtc).unwrap();
    let mode = s.connector.modes().iter().copied().filter(|m| {
        let wm = WlMode::from(*m);
        wm.size.w == w && wm.size.h == h && refresh.is_none_or(|r| wm.refresh == r)
    }).max_by_key(|m| WlMode::from(*m).refresh);
    let Some(mode) = mode else { return };
    let r = manager.use_mode::<_, El>(&crtc, mode, renderer, &DrmOutputRenderElements::default());
    if let Err(e) = r { eprintln!("aerode-wm: mode change on {name}: {e}"); return; }
    s.output.change_current_state(Some(WlMode::from(mode)), None, None, None);
    s.queued = false;
    let o = s.output.clone();
    let loc = wm.space.output_geometry(&o).map(|g| g.loc).unwrap_or_default();
    wm.space.map_output(&o, loc);
}

pub fn set_dpms(wm: &mut Wm, on: bool) {
    let Backend::Udev(u) = &mut wm.backend else { return };
    for s in u.surfaces.values_mut() {
        s.off = !on;
        if on { s.queued = false; s.drm.reset_buffers(); }
        else if let Err(e) = s.drm.with_compositor(|c| c.clear()) { eprintln!("aerode-wm: display off: {e}"); }
    }
    wm.needs_redraw = on;
}

fn frame_finish(wm: &mut Wm, crtc: crtc::Handle, meta: &mut Option<smithay::backend::drm::DrmEventMetadata>) {
    let Backend::Udev(u) = &mut wm.backend else { return };
    let Some(s) = u.surfaces.get_mut(&crtc) else { return };
    s.queued = false;
    let output = s.output.clone();
    match s.drm.frame_submitted() {
        Ok(Some(Some(mut fb))) => {
            use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind as K;
            let refresh = Duration::from_secs_f64(1000.0 / output.current_mode().map(|m| m.refresh.max(1)).unwrap_or(60_000) as f64);
            let hw = meta.as_ref().and_then(|m| match m.time { smithay::backend::drm::DrmEventTime::Monotonic(t) if !t.is_zero() => Some(t), _ => None });
            let seq = meta.as_ref().map(|m| m.sequence as u64).unwrap_or(0);
            let (time, flags) = match hw {
                Some(t) => (t.into(), K::Vsync | K::HwClock | K::HwCompletion),
                None => (smithay::utils::Clock::<smithay::utils::Monotonic>::new().now(), K::Vsync),
            };
            fb.presented(time, smithay::wayland::presentation::Refresh::fixed(refresh), seq, flags);
        }
        Ok(_) => {}
        Err(e) => eprintln!("aerode-wm: frame: {e:?}"),
    }
    wm.send_frames(&output);
    let _ = wm.dh.flush_clients();
    wm.needs_redraw |= wm.animating();
    // the next frame, if anything changed while this one was on its way
    if wm.needs_redraw { redraw_one(wm, crtc); }
}

pub fn redraw(wm: &mut Wm) {
    let Backend::Udev(u) = &wm.backend else { return };
    if !u.active { return; }
    let crtcs: Vec<crtc::Handle> = u.surfaces.keys().copied().collect();
    let mut all_done = true;
    for c in crtcs { all_done &= redraw_one(wm, c); }
    // an output with a frame still queued catches up at its vblank
    wm.needs_redraw = !all_done;
}

/// False when the output is still waiting for a vblank.
fn redraw_one(wm: &mut Wm, crtc: crtc::Handle) -> bool {
    let Backend::Udev(u) = &wm.backend else { return true };
    let Some(s) = u.surfaces.get(&crtc) else { return true };
    if s.off { return true; }
    if s.queued { return false; }
    let output = s.output.clone();
    wm.prepare_frames();
    let Backend::Udev(u) = &mut wm.backend else { return true };
    let Some(renderer_ptr) = u.renderer.as_mut().map(|r| r as *mut GlesRenderer) else { return true };
    // SAFETY: `elements` never touches the backend, only window state
    let elems: Vec<El> = wm.elements(unsafe { &mut *renderer_ptr }, &output);
    let Backend::Udev(u) = &mut wm.backend else { return true };
    let Some(s) = u.surfaces.get_mut(&crtc) else { return true };
    let renderer = u.renderer.as_mut().unwrap();
    let r = s.drm.render_frame(renderer, &elems, [0.0, 0.0, 0.0, 1.0], FrameFlags::DEFAULT);
    let mut queued = false;
    let rendered = match r {
        Ok(res) if !res.is_empty => Some(res.states),
        Ok(_) => None,
        Err(e) => { eprintln!("aerode-wm: render: {e:?}"); None }
    };
    if let Some(states) = rendered {
        let fb = wm.take_feedback(&output, &states);
        let Backend::Udev(u) = &mut wm.backend else { return true };
        let Some(s) = u.surfaces.get_mut(&crtc) else { return true };
        match s.drm.queue_frame(Some(fb)) {
            Ok(()) => { s.queued = true; queued = true; }
            Err(e) => eprintln!("aerode-wm: queue frame: {e:?}"),
        }
    }
    let Backend::Udev(u) = &mut wm.backend else { return true };
    let Some(s) = u.surfaces.get_mut(&crtc) else { return true };
    if !queued {
        // nothing changed on screen: clients still get their frame callback, at the refresh rate
        let refresh = output.current_mode().map(|m| m.refresh.max(1)).unwrap_or(60_000);
        let dur = Duration::from_secs_f64(1000.0 / refresh as f64);
        if s.idle_timer.is_none() {
            let token = wm.handle.insert_source(Timer::from_duration(dur), move |_, _, wm: &mut Wm| {
                if let Backend::Udev(u) = &mut wm.backend {
                    if let Some(s) = u.surfaces.get_mut(&crtc) {
                        s.idle_timer = None;
                        let o = s.output.clone();
                        wm.send_frames(&o);
                    }
                }
                TimeoutAction::Drop
            }).ok();
            let Backend::Udev(u) = &mut wm.backend else { return true };
            if let Some(s) = u.surfaces.get_mut(&crtc) { s.idle_timer = token; }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn edid_names() {
        let mut e = vec![0u8; 128];
        // "DEL" = 00100 00101 01100
        e[8] = 0b0001_0000; e[9] = 0b1010_1100;
        e[54 + 3] = 0xfc;
        e[54 + 5..54 + 18].copy_from_slice(b"U2720Q\n      ");
        assert_eq!(super::parse_edid(&e), ("DEL".to_string(), "U2720Q".to_string()));
        assert_eq!(super::parse_edid(&[]).0, "Unknown");
    }
}
