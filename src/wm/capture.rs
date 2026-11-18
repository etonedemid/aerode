//! Screen sharing: ext-foreign-toplevel-list (the windows a client may pick) and ext-image-copy-capture of
//! outputs and windows into shm buffers. xdg-desktop-portal-wlr turns these into PipeWire streams for
//! browsers, OBS, Discord...; its chooser is `aerode-sharepicker`.
//! ponytail: shm + CPU readback only, add dmabuf when the readback shows up in profiles.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use smithay::output::Output;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::{
    ext_foreign_toplevel_image_capture_source_manager_v1::{self as tl_mgr, ExtForeignToplevelImageCaptureSourceManagerV1},
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::{self as out_mgr, ExtOutputImageCaptureSourceManagerV1},
};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::{
    ext_image_copy_capture_cursor_session_v1::{self as cur, ExtImageCopyCaptureCursorSessionV1},
    ext_image_copy_capture_frame_v1::{self as frm, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{self as mgr, ExtImageCopyCaptureManagerV1},
    ext_image_copy_capture_session_v1::{self as sess, ExtImageCopyCaptureSessionV1},
};
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_output::Transform, wl_shm};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListHandler, ForeignToplevelListState};

use super::Wm;

#[derive(Clone)]
pub enum Src { Output(Output), Win(u32), None }

pub struct SessionData {
    src: Src,
    cursors: bool,
    /// size last announced with buffer_size
    size: Mutex<(i32, i32)>,
    /// `capture_gen` of the last frame sent; frames wait until the screen changed
    seen: Mutex<u64>,
}

pub struct FrameData {
    session: ExtImageCopyCaptureSessionV1,
    buffer: Mutex<Option<WlBuffer>>,
    captured: Mutex<bool>,
}

pub struct Capture {
    pub list: ForeignToplevelListState,
    /// window id -> its handle in the toplevel list
    pub handles: HashMap<u32, ForeignToplevelHandle>,
    sessions: Vec<ExtImageCopyCaptureSessionV1>,
    pending: Vec<ExtImageCopyCaptureFrameV1>,
    timer: bool,
    /// bumped on every redraw: something on screen changed
    pub generation: u64,
}

impl Capture {
    pub fn new(dh: &DisplayHandle) -> Self {
        dh.create_global::<Wm, ExtImageCopyCaptureManagerV1, ()>(1, ());
        dh.create_global::<Wm, ExtOutputImageCaptureSourceManagerV1, ()>(1, ());
        dh.create_global::<Wm, ExtForeignToplevelImageCaptureSourceManagerV1, ()>(1, ());
        Capture { list: ForeignToplevelListState::new::<Wm>(dh), handles: HashMap::new(), sessions: vec![], pending: vec![], timer: false, generation: 0 }
    }
}

impl ForeignToplevelListHandler for Wm {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState { &mut self.capture.list }
}
smithay::delegate_foreign_toplevel_list!(Wm);

impl Wm {
    /// Bring the toplevel list in line with the taskbar's windows; sessions of closed windows stop.
    pub fn sync_toplevels(&mut self) {
        let shown: Vec<u32> = self.wins.values().filter(|c| c.mapped && !c.skip_taskbar && !matches!(c.kind, super::Kind::Menu | super::Kind::Splash)).map(|c| c.id).collect();
        let gone: Vec<u32> = self.capture.handles.keys().filter(|id| !shown.contains(id)).copied().collect();
        for id in gone {
            if let Some(h) = self.capture.handles.remove(&id) { self.capture.list.remove_toplevel(&h); }
            for s in &self.capture.sessions {
                if matches!(s.data::<SessionData>().map(|d| &d.src), Some(Src::Win(w)) if *w == id) { s.stopped(); }
            }
        }
        for id in shown {
            let c = &self.wins[&id];
            match self.capture.handles.get(&id) {
                Some(h) => {
                    if h.title() != c.title || h.app_id() != c.app_id {
                        h.send_title(&c.title);
                        h.send_app_id(&c.app_id);
                        h.send_done();
                    }
                }
                None => { let h = self.capture.list.new_toplevel::<Wm>(c.title.clone(), c.app_id.clone()); self.capture.handles.insert(id, h); }
            }
        }
    }

    /// The identifier the portal's chooser sees for a window ("Window: TITLE (IDENT)").
    pub fn share_id(&self, id: u32) -> String { self.capture.handles.get(&id).map(|h| h.identifier()).unwrap_or_default() }

    fn source_size(&self, src: &Src) -> Option<(i32, i32)> {
        match src {
            Src::Output(o) => o.current_mode().map(|m| (m.size.w, m.size.h)),
            Src::Win(id) => self.wins.get(id).map(|c| { let f = self.frame_rect(c); (f.w.max(1), f.h.max(1)) }),
            Src::None => None,
        }
    }

    fn announce(&self, s: &ExtImageCopyCaptureSessionV1, d: &SessionData) -> bool {
        let Some((w, h)) = self.source_size(&d.src) else { s.stopped(); return false };
        *d.size.lock().unwrap() = (w, h);
        s.buffer_size(w as u32, h as u32);
        s.shm_format(wl_shm::Format::Argb8888);
        s.shm_format(wl_shm::Format::Xrgb8888);
        s.done();
        true
    }

    fn start_capture_timer(&mut self) {
        if std::mem::replace(&mut self.capture.timer, true) { return; }
        let _ = self.handle.insert_source(Timer::from_duration(Duration::from_millis(16)), |_, _, wm: &mut Wm| {
            wm.capture.sessions.retain(|s| s.is_alive());
            wm.capture.pending.retain(|f| f.is_alive());
            if wm.capture.sessions.is_empty() { wm.capture.timer = false; return TimeoutAction::Drop; }
            wm.serve_frames();
            TimeoutAction::ToDuration(Duration::from_millis(16))
        });
    }

    /// Fill the frames waiting for a changed screen.
    fn serve_frames(&mut self) {
        let generation = self.capture.generation;
        let pending = std::mem::take(&mut self.capture.pending);
        for f in pending {
            let Some(fd) = f.data::<FrameData>() else { continue };
            let Some(sd) = fd.session.data::<SessionData>() else { f.failed(frm::FailureReason::Stopped); continue };
            if *sd.seen.lock().unwrap() == generation { self.capture.pending.push(f); continue; }
            let Some(size) = self.source_size(&sd.src) else { f.failed(frm::FailureReason::Stopped); continue };
            if size != *sd.size.lock().unwrap() {
                // the window was resized: tell the client the new size, it allocates new buffers
                self.announce(&fd.session, sd);
                f.failed(frm::FailureReason::BufferConstraints);
                continue;
            }
            let pixels = match &sd.src {
                Src::Output(o) => self.output_pixels(o, sd.cursors),
                Src::Win(id) => self.window_pixels(*id),
                Src::None => None,
            };
            let Some(pixels) = pixels else { f.failed(frm::FailureReason::Unknown); continue };
            let Some(buf) = fd.buffer.lock().unwrap().clone() else { f.failed(frm::FailureReason::BufferConstraints); continue };
            let (w, h) = size;
            let ok = smithay::wayland::shm::with_buffer_contents_mut(&buf, |ptr, len, d| {
                if d.width != w || d.height != h || !matches!(d.format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888) { return false; }
                if d.offset as usize + d.stride as usize * h as usize > len || (d.stride as usize) < w as usize * 4 { return false; }
                for y in 0..h as usize {
                    let src = &pixels[y * w as usize * 4..(y + 1) * w as usize * 4];
                    // SAFETY: bounds checked against the pool length above
                    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), ptr.add(d.offset as usize + y * d.stride as usize), src.len()); }
                }
                true
            }).unwrap_or(false);
            if !ok { f.failed(frm::FailureReason::BufferConstraints); continue; }
            *sd.seen.lock().unwrap() = generation;
            let t = smithay::utils::Clock::<smithay::utils::Monotonic>::new().now();
            let t: Duration = t.into();
            f.transform(Transform::Normal);
            f.damage(0, 0, w, h);
            f.presentation_time((t.as_secs() >> 32) as u32, t.as_secs() as u32, t.subsec_nanos());
            f.ready();
        }
    }
}

// ------------------------------------------------------------------ sources
impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Wm {
    fn bind(_: &mut Wm, _: &DisplayHandle, _: &Client, r: New<ExtOutputImageCaptureSourceManagerV1>, _: &(), di: &mut DataInit<'_, Wm>) { di.init(r, ()); }
}
impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for Wm {
    fn request(_: &mut Wm, _: &Client, _: &ExtOutputImageCaptureSourceManagerV1, req: out_mgr::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, Wm>) {
        if let out_mgr::Request::CreateSource { source, output } = req {
            di.init(source, Output::from_resource(&output).map(Src::Output).unwrap_or(Src::None));
        }
    }
}
impl GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Wm {
    fn bind(_: &mut Wm, _: &DisplayHandle, _: &Client, r: New<ExtForeignToplevelImageCaptureSourceManagerV1>, _: &(), di: &mut DataInit<'_, Wm>) { di.init(r, ()); }
}
impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for Wm {
    fn request(wm: &mut Wm, _: &Client, _: &ExtForeignToplevelImageCaptureSourceManagerV1, req: tl_mgr::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, Wm>) {
        if let tl_mgr::Request::CreateSource { source, toplevel_handle } = req {
            let ident = ForeignToplevelHandle::from_resource(&toplevel_handle).map(|h| h.identifier());
            let id = wm.capture.handles.iter().find(|(_, h)| Some(h.identifier()) == ident).map(|(i, _)| *i);
            di.init(source, id.map(Src::Win).unwrap_or(Src::None));
        }
    }
}
impl Dispatch<ExtImageCaptureSourceV1, Src> for Wm {
    fn request(_: &mut Wm, _: &Client, _: &ExtImageCaptureSourceV1, _: <ExtImageCaptureSourceV1 as Resource>::Request, _: &Src, _: &DisplayHandle, _: &mut DataInit<'_, Wm>) {}
}

// ------------------------------------------------------------------ capture
impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for Wm {
    fn bind(_: &mut Wm, _: &DisplayHandle, _: &Client, r: New<ExtImageCopyCaptureManagerV1>, _: &(), di: &mut DataInit<'_, Wm>) { di.init(r, ()); }
}
impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for Wm {
    fn request(wm: &mut Wm, _: &Client, _: &ExtImageCopyCaptureManagerV1, req: mgr::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, Wm>) {
        match req {
            mgr::Request::CreateSession { session, source, options } => {
                let src = source.data::<Src>().cloned().unwrap_or(Src::None);
                let cursors = options.into_result().map(|o| o.contains(mgr::Options::PaintCursors)).unwrap_or(false);
                let s = di.init(session, SessionData { src, cursors, size: Mutex::new((0, 0)), seen: Mutex::new(u64::MAX) });
                if wm.announce(&s, s.data::<SessionData>().unwrap()) {
                    wm.capture.sessions.push(s);
                    wm.start_capture_timer();
                }
            }
            // the pointer is painted into the frames instead (paint_cursors)
            mgr::Request::CreatePointerCursorSession { session, .. } => { di.init(session, ()); }
            _ => {}
        }
    }
}
impl Dispatch<ExtImageCopyCaptureCursorSessionV1, ()> for Wm {
    fn request(_: &mut Wm, _: &Client, _: &ExtImageCopyCaptureCursorSessionV1, req: cur::Request, _: &(), _: &DisplayHandle, di: &mut DataInit<'_, Wm>) {
        if let cur::Request::GetCaptureSession { session } = req {
            let s = di.init(session, SessionData { src: Src::None, cursors: false, size: Mutex::new((0, 0)), seen: Mutex::new(0) });
            s.stopped();
        }
    }
}
impl Dispatch<ExtImageCopyCaptureSessionV1, SessionData> for Wm {
    fn request(_: &mut Wm, _: &Client, s: &ExtImageCopyCaptureSessionV1, req: sess::Request, _: &SessionData, _: &DisplayHandle, di: &mut DataInit<'_, Wm>) {
        if let sess::Request::CreateFrame { frame } = req {
            di.init(frame, FrameData { session: s.clone(), buffer: Mutex::new(None), captured: Mutex::new(false) });
        }
    }
}
impl Dispatch<ExtImageCopyCaptureFrameV1, FrameData> for Wm {
    fn request(wm: &mut Wm, _: &Client, f: &ExtImageCopyCaptureFrameV1, req: frm::Request, d: &FrameData, _: &DisplayHandle, _: &mut DataInit<'_, Wm>) {
        match req {
            frm::Request::AttachBuffer { buffer } => *d.buffer.lock().unwrap() = Some(buffer),
            frm::Request::Capture => {
                if std::mem::replace(&mut *d.captured.lock().unwrap(), true) { f.post_error(frm::Error::AlreadyCaptured, "capture sent twice"); return; }
                if d.buffer.lock().unwrap().is_none() { f.post_error(frm::Error::NoBuffer, "capture without a buffer"); return; }
                wm.capture.pending.push(f.clone());
            }
            _ => {}
        }
    }
}
