//! Test client for aerode-wm's screen sharing: `capture` lists the windows (ext-foreign-toplevel-list);
//! `capture out.png [N]` captures the first output, `capture out.png N TITLE` the window whose title
//! contains TITLE, N frames in a row (ext-image-copy-capture into shm), reporting the frame rate.
use std::os::fd::AsFd;
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{ext_foreign_toplevel_handle_v1 as th, ext_foreign_toplevel_list_v1 as tl};
use wayland_protocols::ext::image_capture_source::v1::client::{ext_foreign_toplevel_image_capture_source_manager_v1 as tsm, ext_image_capture_source_v1 as src, ext_output_image_capture_source_manager_v1 as osm};
use wayland_protocols::ext::image_copy_capture::v1::client::{ext_image_copy_capture_frame_v1 as frm, ext_image_copy_capture_manager_v1 as mgr, ext_image_copy_capture_session_v1 as sess};

#[derive(Default)]
struct S {
    output: Option<wl_output::WlOutput>, shm: Option<wl_shm::WlShm>, mgr: Option<mgr::ExtImageCopyCaptureManagerV1>,
    osm: Option<osm::ExtOutputImageCaptureSourceManagerV1>, tsm: Option<tsm::ExtForeignToplevelImageCaptureSourceManagerV1>,
    toplevels: Vec<(th::ExtForeignToplevelHandleV1, String, String)>,
    size: (u32, u32), formats: Vec<wl_shm::Format>, session_done: bool, stopped: bool,
    ready: bool, failed: Option<String>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for S {
    fn event(s: &mut S, r: &wl_registry::WlRegistry, e: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<S>) {
        if let wl_registry::Event::Global { name, interface, .. } = e {
            match interface.as_str() {
                "wl_output" if s.output.is_none() => s.output = Some(r.bind(name, 1, qh, ())),
                "wl_shm" => s.shm = Some(r.bind(name, 1, qh, ())),
                "ext_image_copy_capture_manager_v1" => s.mgr = Some(r.bind(name, 1, qh, ())),
                "ext_output_image_capture_source_manager_v1" => s.osm = Some(r.bind(name, 1, qh, ())),
                "ext_foreign_toplevel_image_capture_source_manager_v1" => s.tsm = Some(r.bind(name, 1, qh, ())),
                "ext_foreign_toplevel_list_v1" => { let _: tl::ExtForeignToplevelListV1 = r.bind(name, 1, qh, ()); }
                _ => {}
            }
        }
    }
}
impl Dispatch<tl::ExtForeignToplevelListV1, ()> for S {
    fn event(s: &mut S, _: &tl::ExtForeignToplevelListV1, e: tl::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {
        if let tl::Event::Toplevel { toplevel } = e { s.toplevels.push((toplevel, String::new(), String::new())); }
    }
    wayland_client::event_created_child!(S, tl::ExtForeignToplevelListV1, [tl::EVT_TOPLEVEL_OPCODE => (th::ExtForeignToplevelHandleV1, ())]);
}
impl Dispatch<th::ExtForeignToplevelHandleV1, ()> for S {
    fn event(s: &mut S, h: &th::ExtForeignToplevelHandleV1, e: th::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {
        let Some(t) = s.toplevels.iter_mut().find(|t| &t.0 == h) else { return };
        match e { th::Event::Title { title } => t.1 = title, th::Event::Identifier { identifier } => t.2 = identifier, _ => {} }
    }
}
impl Dispatch<sess::ExtImageCopyCaptureSessionV1, ()> for S {
    fn event(s: &mut S, _: &sess::ExtImageCopyCaptureSessionV1, e: sess::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {
        match e {
            sess::Event::BufferSize { width, height } => s.size = (width, height),
            sess::Event::ShmFormat { format } => if let wayland_client::WEnum::Value(f) = format { s.formats.push(f) },
            sess::Event::Done => s.session_done = true,
            sess::Event::Stopped => s.stopped = true,
            _ => {}
        }
    }
}
impl Dispatch<frm::ExtImageCopyCaptureFrameV1, ()> for S {
    fn event(s: &mut S, _: &frm::ExtImageCopyCaptureFrameV1, e: frm::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {
        match e { frm::Event::Ready => s.ready = true, frm::Event::Failed { reason } => s.failed = Some(format!("{reason:?}")), _ => {} }
    }
}
macro_rules! ignore { ($($t:ty),*) => { $(impl Dispatch<$t, ()> for S { fn event(_: &mut S, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<S>) {} })* } }
ignore!(wl_output::WlOutput, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer, mgr::ExtImageCopyCaptureManagerV1, osm::ExtOutputImageCaptureSourceManagerV1, tsm::ExtForeignToplevelImageCaptureSourceManagerV1, src::ExtImageCaptureSourceV1);

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let conn = Connection::connect_to_env().expect("no wayland display");
    let mut q = conn.new_event_queue();
    let qh = q.handle();
    conn.display().get_registry(&qh, ());
    let mut s = S::default();
    q.roundtrip(&mut s).unwrap();
    q.roundtrip(&mut s).unwrap();
    let Some(out_path) = a.get(1) else {
        for t in &s.toplevels { println!("{} ({})", t.1, t.2); }
        return;
    };
    let n: usize = a.get(2).and_then(|x| x.parse().ok()).unwrap_or(1);
    let source = match a.get(3) {
        Some(title) => {
            let t = s.toplevels.iter().find(|t| t.1.contains(title.as_str())).expect("no such window");
            s.tsm.as_ref().expect("no toplevel source manager").create_source(&t.0, &qh, ())
        }
        None => s.osm.as_ref().expect("no output source manager").create_source(s.output.as_ref().unwrap(), &qh, ()),
    };
    let session = s.mgr.as_ref().expect("no capture manager").create_session(&source, mgr::Options::PaintCursors, &qh, ());
    while !s.session_done && !s.stopped { q.blocking_dispatch(&mut s).unwrap(); }
    assert!(!s.stopped, "session stopped");
    assert!(s.formats.contains(&wl_shm::Format::Argb8888), "no argb8888");
    let (w, h) = (s.size.0 as i32, s.size.1 as i32);
    let file = std::fs::File::from(rustix_memfd(w as usize * h as usize * 4));
    let pool = s.shm.as_ref().unwrap().create_pool(file.as_fd(), w * h * 4, &qh, ());
    let buf = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &qh, ());
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        s.ready = false;
        let f = session.create_frame(&qh, ());
        f.attach_buffer(&buf);
        f.damage_buffer(0, 0, w, h);
        f.capture();
        while !s.ready && s.failed.is_none() { q.blocking_dispatch(&mut s).unwrap(); }
        if let Some(r) = s.failed.take() { panic!("frame failed: {r}"); }
        f.destroy();
    }
    let secs = t0.elapsed().as_secs_f64();
    println!("{n} frames of {w}x{h} in {secs:.2}s ({:.1} fps)", n as f64 / secs);
    let data = std::fs::read(format!("/proc/self/fd/{}", std::os::fd::AsRawFd::as_raw_fd(&file))).unwrap();
    let img = cairo::ImageSurface::create_for_data(data, cairo::Format::ARgb32, w, h, w * 4).unwrap();
    img.write_to_png(&mut std::fs::File::create(out_path).unwrap()).unwrap();
}

fn rustix_memfd(len: usize) -> std::os::fd::OwnedFd {
    use std::os::fd::FromRawFd;
    let fd = unsafe { libc::memfd_create(c"capture".as_ptr(), 0) };
    assert!(fd >= 0);
    assert_eq!(unsafe { libc::ftruncate(fd, len as i64) }, 0);
    unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }
}
