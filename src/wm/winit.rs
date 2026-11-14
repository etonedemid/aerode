//! Nested backend for testing: a window inside X11 or another Wayland desktop (`AERODE_NESTED=1`).
use super::render::El;
use super::{Backend, Wm};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::{gles::GlesRenderer, ImportEgl};
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::winit::dpi::LogicalSize;
use smithay::reexports::winit::window::Window as WinitWindow;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

pub struct WinitData { pub backend: WinitGraphicsBackend<GlesRenderer>, pub output: Output, pub damage: OutputDamageTracker }

pub fn init(wm: &mut Wm, event_loop: &EventLoop<'static, Wm>) -> anyhow::Result<()> {
    let (w, h) = std::env::var("AERODE_NESTED_SIZE").ok().and_then(|s| { let (a, b) = s.split_once('x')?; Some((a.parse().ok()?, b.parse().ok()?)) }).unwrap_or((1280, 800));
    let attrs = WinitWindow::default_attributes().with_title("aerode").with_inner_size(LogicalSize::new(w as f64, h as f64));
    let (mut backend, source) = winit::init_from_attributes::<GlesRenderer>(attrs).map_err(|e| anyhow::anyhow!("winit: {e:?}"))?;
    let mode = Mode { size: backend.window_size(), refresh: 60_000 };
    let output = Output::new("winit".into(), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Aerode".into(), model: "Nested".into() });
    let _global = output.create_global::<Wm>(&wm.dh);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);
    wm.space.map_output(&output, (0, 0));
    let damage = OutputDamageTracker::from_output(&output);
    // SHM is enough for nested runs; dmabuf when the host GL offers it
    let formats = smithay::backend::renderer::ImportMemWl::shm_formats(backend.renderer()).collect::<Vec<_>>();
    wm.shm_state.update_formats(formats);
    let dmabuf_formats: Vec<_> = smithay::backend::renderer::ImportDma::dmabuf_formats(backend.renderer()).into_iter().collect();
    if !dmabuf_formats.is_empty() {
        if let Ok(fb) = DmabufFeedbackBuilder::new(libc::makedev(0, 0), dmabuf_formats.clone()).build() {
            wm.dmabuf_global = Some(wm.dmabuf_state.create_global_with_default_feedback::<Wm>(&wm.dh, &fb));
        }
    }
    let _ = ImportEgl::bind_wl_display(backend.renderer(), &wm.dh);
    wm.backend = Backend::Winit(WinitData { backend, output: output.clone(), damage });
    event_loop.handle().insert_source(source, move |event, _, wm: &mut Wm| match event {
        WinitEvent::Resized { size, .. } => {
            let Backend::Winit(d) = &mut wm.backend else { return };
            d.output.change_current_state(Some(Mode { size, refresh: 60_000 }), None, None, None);
            wm.update_workarea();
            wm.needs_redraw = true;
        }
        WinitEvent::Input(ev) => wm.process_input(ev),
        WinitEvent::Redraw => { wm.needs_redraw = true; }
        WinitEvent::CloseRequested => wm.signal.stop(),
        _ => {}
    }).map_err(|e| anyhow::anyhow!("winit source: {e}"))?;
    Ok(())
}

pub fn redraw(wm: &mut Wm) {
    wm.needs_redraw = false;
    wm.prepare_frames();
    let Backend::Winit(d) = &mut wm.backend else { return };
    let output = d.output.clone();
    let renderer_ptr: *mut GlesRenderer = d.backend.renderer();
    // SAFETY: `elements` never touches the backend, only window state
    let elems: Vec<El> = wm.elements(unsafe { &mut *renderer_ptr }, &output);
    let Backend::Winit(d) = &mut wm.backend else { return };
    let size = d.backend.window_size();
    let age = d.backend.buffer_age().unwrap_or(0);
    let (damage, states) = {
        let Ok((renderer, mut fb)) = d.backend.bind() else { return };
        match d.damage.render_output(renderer, &mut fb, age, &elems, [0.0, 0.0, 0.0, 1.0]) {
            Ok(r) => (r.damage.cloned(), r.states),
            Err(e) => { eprintln!("aerode-wm: render: {e:?}"); return; }
        }
    };
    let _ = d.backend.submit(damage.as_deref().or(Some(&[Rectangle::from_size(size)])));
    {
        use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind as K;
        let mut fb = wm.take_feedback(&output, &states);
        fb.presented(smithay::utils::Clock::<smithay::utils::Monotonic>::new().now(), smithay::wayland::presentation::Refresh::Unknown, 0, K::empty());
    }
    wm.send_frames(&output);
}
