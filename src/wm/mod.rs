//! aerode-wm: the Wayland compositor (smithay). Draws Windows 7 frames server-side, blurs the glass
//! itself (no external compositor), runs Aero Snap/Shake/Peek, Alt+Tab, the global keys, and XWayland
//! for X11 programs. The shell talks to it through `crate::wmipc`.
pub mod anim;
pub mod decor;
pub mod glass;
pub mod keys;
pub mod switcher;
mod blur;
mod capture;
mod cursor;
mod handlers;
mod input;
mod ipc;
mod render;
mod udev;
mod winit;
mod xwm;

use anyhow::Result;
use smithay::desktop::{layer_map_for_output, PopupManager, Space, Window};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Logical, Rectangle, Size, SERIAL_COUNTER};
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::idle_inhibit::IdleInhibitManagerState;
use smithay::wayland::idle_notify::IdleNotifierState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::PointerConstraintsState;
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::wlr_data_control::DataControlState;
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::wlr_layer::WlrLayerShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xwayland_keyboard_grab::XWaylandKeyboardGrabState;
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::xwayland::X11Wm;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use decor::{BState, Btn, Decor, BORDER, MAX_TOP, TOP};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Normal, Dialog, Utility, Splash, Menu }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool { x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h }
    fn center(&self) -> (i32, i32) { (self.x + self.w / 2, self.y + self.h / 2) }
    pub fn smithay(&self) -> Rectangle<i32, Logical> { Rectangle::new((self.x, self.y).into(), (self.w, self.h).into()) }
    pub fn from_smithay(r: Rectangle<i32, Logical>) -> Rect { Rect { x: r.loc.x, y: r.loc.y, w: r.size.w, h: r.size.h } }
}

/// A managed window.
pub struct Win {
    pub id: u32,
    pub window: Window,
    pub kind: Kind,
    pub title: String,
    pub app_id: String,
    pub pid: u32,
    pub icon: Option<cairo::ImageSurface>,
    /// client area on the global plane
    pub geom: Rect,
    pub restore: Option<Rect>,
    pub maximized: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    pub above: bool,
    pub skip_taskbar: bool,
    pub urgent: bool,
    pub transient_for: u32,
    pub min_size: (i32, i32),
    pub resizable: bool,
    pub can_minimize: bool,
    pub can_maximize: bool,
    pub desktop: u32,
    pub hover: Option<Btn>,
    pub pressed: Option<Btn>,
    pub snapped: Option<i8>, // -1 left, 1 right, 2 vertically maximised (keeps x and width)
    /// server-side frame (false: client-side decorated, or a popup-like X11 window)
    pub framed: bool,
    pub mapped: bool,
    pub glow: anim::Glow,
    pub focus_fade: Option<anim::Tween>,
    /// the four border strips (top, bottom, left, right), None = repaint before the next frame
    pub frame_cache: Option<render::FrameCache>,
    pub glass_ids: render::GlassIds,
    /// window animation in progress (open, minimise, restore...)
    pub anim: Option<render::WinAnim>,
    /// taskbar button: minimising flies there
    pub icon_geometry: Option<Rect>,
    /// a size asked of the client: the position goes with it once the client has answered
    pub pending: Option<Rect>,
}

impl Win {
    pub fn surface(&self) -> Option<WlSurface> { use smithay::wayland::seat::WaylandFocus; self.window.wl_surface().map(|s| (*s).clone()) }
    pub fn is_x11(&self) -> bool { self.window.x11_surface().is_some() }
}

/// Win7's window event sounds (user32 plays them): silent unless the scheme assigns a file, which
/// none of Win7's schemes do - they are there for users who pick one in the Sounds tab.
fn window_sound(ev: crate::sounds::Event) {
    if crate::sounds::file_for(ev).is_some() { crate::sounds::play(ev); }
}

fn is_app(c: &Win) -> bool { matches!(c.kind, Kind::Normal | Kind::Dialog | Kind::Utility) }

#[derive(Clone, Copy, Debug)]
pub enum Drag {
    Move { cx: i32, cy: i32, ox: i32, oy: i32, from_max: bool },
    Resize { edges: u8, start: Rect, px: i32, py: i32 },
    Button { btn: Btn },
}

/// Aero Shake: a title-bar drag that reverses direction (by at least `SHAKE_MIN` px) a number of
/// times (Ease of Access > Make the mouse easier to use > Aero Shake, 8 by default) within
/// `SHAKE_MS_PER_FLIP` per reversal minimises every other window; shaking again brings them back.
const SHAKE_MIN: i32 = 24;
const SHAKE_DEFAULT_FLIPS: usize = 8;
const SHAKE_MS_PER_FLIP: u128 = 250;

#[derive(Default)]
struct Shake { win: u32, flips: Vec<Instant>, last: (i32, i32), dir: (i32, i32) }

impl Shake {
    fn step(&mut self, w: u32, x: i32, y: i32, flips: usize) -> bool {
        if self.win != w { *self = Shake { win: w, last: (x, y), ..Default::default() }; return false; }
        let (dx, dy) = (x - self.last.0, y - self.last.1);
        let mut flipped = false;
        if dx.abs() >= SHAKE_MIN { if dx.signum() == -self.dir.0 { flipped = true; } self.dir.0 = dx.signum(); self.last.0 = x; }
        if dy.abs() >= SHAKE_MIN { if dy.signum() == -self.dir.1 { flipped = true; } self.dir.1 = dy.signum(); self.last.1 = y; }
        if !flipped { return false; }
        let now = Instant::now();
        self.flips.push(now);
        let window = SHAKE_MS_PER_FLIP * flips as u128;
        self.flips.retain(|t| now.duration_since(*t).as_millis() <= window);
        if self.flips.len() >= flips { self.flips.clear(); return true; }
        false
    }
}

pub enum Backend { None, Winit(winit::WinitData), Udev(udev::UdevData) }

pub struct Wm {
    pub dh: DisplayHandle,
    pub handle: LoopHandle<'static, Wm>,
    pub signal: LoopSignal,
    pub socket_name: String,
    pub backend: Backend,
    pub start: Instant,

    pub space: Space<Window>,
    pub popups: PopupManager,
    pub seat: Seat<Wm>,
    pub seat_state: SeatState<Wm>,

    pub compositor_state: CompositorState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<smithay::wayland::dmabuf::DmabufGlobal>,
    pub syncobj_state: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub kde_decoration_state: KdeDecorationState,
    pub output_manager_state: OutputManagerState,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    pub viewporter_state: ViewporterState,
    pub presentation_state: PresentationState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub xdg_activation_state: XdgActivationState,
    pub idle_notifier: IdleNotifierState<Wm>,
    pub idle_inhibit_state: IdleInhibitManagerState,
    pub pointer_gestures_state: PointerGesturesState,
    pub relative_pointer_state: RelativePointerManagerState,
    pub pointer_constraints_state: PointerConstraintsState,
    pub cursor_shape_state: CursorShapeManagerState,
    pub text_input_state: TextInputManagerState,
    pub virtual_keyboard_state: VirtualKeyboardManagerState,
    pub single_pixel_state: SinglePixelBufferState,
    pub xwayland_shell_state: XWaylandShellState,
    pub xwayland_grab_state: XWaylandKeyboardGrabState,
    pub xwm: Option<X11Wm>,
    pub capture: capture::Capture,

    // --- window management
    pub wins: HashMap<u32, Win>,
    /// bottom to top
    pub stack: Vec<u32>,
    pub next_id: u32,
    pub cascade: u32,
    pub active: u32,
    pub current_desktop: u32,
    pub num_desktops: u32,
    pub showing_desktop: bool,
    pub drag: Option<(u32, Drag)>,
    /// arrow keys move/resize `drag` until Enter/Escape (Alt+Space > Move/Size)
    pub kbd_drag: Option<(i32, i32)>,
    shake: Shake,
    shaken: Vec<u32>,
    pub snap_preview: Option<(u32, i8, render::SnapSheet)>,
    snap_wa: Option<Rect>,
    /// (window that stays, other windows' fade 1 -> 0)
    pub peek: Option<(u32, anim::Tween)>,
    pub switcher: Option<switcher::Switcher>,
    switcher_start_active: u32,
    pub ripple: Option<(i32, i32, anim::Tween)>,
    pub keys: keys::Keys,
    pub decor: Decor,
    pub glass: glass::Glass,
    pub live_colorization: Option<[u32; 7]>,
    pub cursors: cursor::Cursors,
    pub cursor_status: smithay::input::pointer::CursorImageStatus,
    /// compositor's own cursor over a frame (resize arrows), wins over what the last client set
    pub frame_cursor: Option<&'static str>,
    pub dnd_icon: Option<WlSurface>,
    pub needs_redraw: bool,
    pub subscribers: Vec<std::os::unix::net::UnixStream>,
    pub state_changed: bool,
    pub ipc_path: std::path::PathBuf,
    config_mtime: std::cell::Cell<Option<std::time::SystemTime>>,

    // --- session
    pub last_input: Instant,
    pub dpms_timeout: u32,
    pub dpms_off: bool,
    pub saver_timeout: u32,
    pub saver_cmd: String,
    saver_running: bool,
    /// the lock: everything but the lock program's surfaces is hidden from sight and input
    pub lock: Option<input::Lock>,
    pub inhibitors: Vec<WlSurface>,
    pub input_config: input::InputConfig,
    pub devices: Vec<smithay::reexports::input::Device>,
    pub kb_layouts: Vec<String>,
    pub kb_config: (String, String, String),
    pub pointer_cons_active: bool,
    pub suppressed: Vec<u32>,
    pub last_click: (u32, Instant, i32, i32),
    pub resize_next: Option<(u32, Rect)>,
    pub kbd_drag_orig: Option<Rect>,
    pub switcher_id: smithay::backend::renderer::element::Id,
    pub peek_sheets: HashMap<u32, smithay::backend::renderer::element::memory::MemoryRenderBuffer>,
    pub peek_ids: std::cell::RefCell<HashMap<u32, smithay::backend::renderer::element::Id>>,
    pub snap_seq: u64,
    pub last_group: u32,
    pub startup_cmd: Option<String>,
}

pub struct ClientState {
    pub compositor_state: smithay::wayland::compositor::CompositorClientState,
    pub pid: Option<i32>,
}

impl smithay::reexports::wayland_server::backend::ClientData for ClientState {
    fn initialized(&self, _: smithay::reexports::wayland_server::backend::ClientId) {}
    fn disconnected(&self, _: smithay::reexports::wayland_server::backend::ClientId, _: smithay::reexports::wayland_server::backend::DisconnectReason) {}
}

/// The process on the other end of a client's socket.
fn peer_pid(s: &std::os::unix::net::UnixStream) -> Option<i32> {
    use std::os::fd::AsRawFd;
    let mut c = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut n = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: c and n are valid for the duration of the call
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut c as *mut _ as *mut _, &mut n) };
    (r == 0).then_some(c.pid)
}

/// Run the compositor until logoff.
pub fn run() -> Result<()> {
    let mut event_loop: EventLoop<'static, Wm> = EventLoop::try_new()?;
    let display: Display<Wm> = Display::new()?;
    let dh = display.handle();

    let source = ListeningSocketSource::new_auto()?;
    let socket_name = source.socket_name().to_string_lossy().into_owned();
    event_loop.handle().insert_source(source, |stream, _, wm: &mut Wm| {
        let pid = peer_pid(&stream);
        if let Err(e) = wm.dh.insert_client(stream, Arc::new(ClientState { compositor_state: Default::default(), pid })) {
            eprintln!("aerode-wm: client refused: {e}");
        }
    })?;
    event_loop.handle().insert_source(Generic::new(display, Interest::READ, Mode::Level), |_, display, wm: &mut Wm| {
        // Safety: the display lives as long as the source
        unsafe { display.get_mut().dispatch_clients(wm).unwrap(); }
        Ok(PostAction::Continue)
    })?;
    let mut seat_state = SeatState::new();
    let mut seat: Seat<Wm> = seat_state.new_wl_seat(&dh, "seat0");
    let input_config = input::InputConfig::load();
    let (kb_config, kb_layouts) = input::keyboard_from_config();
    seat.add_keyboard(input::xkb_config(&kb_config), input_config.repeat_delay, input_config.repeat_rate)
        .or_else(|_| seat.add_keyboard(Default::default(), 500, 30))?;
    seat.add_pointer();

    crate::config::reload();
    let cfg = crate::config::get();
    let decor = Decor::load();
    let theme_glass = crate::dwmcolor::ThemeGlass::default();
    let glass = glass::Glass::new(cfg.colorization.dwm(), theme_glass);

    let mut wm = Wm {
        capture: capture::Capture::new(&dh),
        compositor_state: CompositorState::new::<Wm>(&dh),
        shm_state: ShmState::new::<Wm>(&dh, vec![]),
        dmabuf_state: DmabufState::new(),
        dmabuf_global: None,
        syncobj_state: None,
        xdg_shell_state: XdgShellState::new::<Wm>(&dh),
        layer_shell_state: WlrLayerShellState::new::<Wm>(&dh),
        xdg_decoration_state: XdgDecorationState::new::<Wm>(&dh),
        kde_decoration_state: KdeDecorationState::new::<Wm>(&dh, smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode::Server),
        output_manager_state: OutputManagerState::new_with_xdg_output::<Wm>(&dh),
        data_device_state: DataDeviceState::new::<Wm>(&dh),
        primary_selection_state: PrimarySelectionState::new::<Wm>(&dh),
        data_control_state: DataControlState::new::<Wm, _>(&dh, Some(&PrimarySelectionState::new::<Wm>(&dh)), |_| true),
        viewporter_state: ViewporterState::new::<Wm>(&dh),
        presentation_state: PresentationState::new::<Wm>(&dh, libc::CLOCK_MONOTONIC as u32),
        fractional_scale_state: FractionalScaleManagerState::new::<Wm>(&dh),
        xdg_activation_state: XdgActivationState::new::<Wm>(&dh),
        idle_notifier: IdleNotifierState::new(&dh, event_loop.handle()),
        idle_inhibit_state: IdleInhibitManagerState::new::<Wm>(&dh),
        pointer_gestures_state: PointerGesturesState::new::<Wm>(&dh),
        relative_pointer_state: RelativePointerManagerState::new::<Wm>(&dh),
        pointer_constraints_state: PointerConstraintsState::new::<Wm>(&dh),
        cursor_shape_state: CursorShapeManagerState::new::<Wm>(&dh),
        text_input_state: TextInputManagerState::new::<Wm>(&dh),
        virtual_keyboard_state: VirtualKeyboardManagerState::new::<Wm, _>(&dh, |_| true),
        single_pixel_state: SinglePixelBufferState::new::<Wm>(&dh),
        xwayland_shell_state: XWaylandShellState::new::<Wm>(&dh),
        xwayland_grab_state: XWaylandKeyboardGrabState::new::<Wm>(&dh),
        xwm: None,
        dh: dh.clone(),
        handle: event_loop.handle(),
        signal: event_loop.get_signal(),
        socket_name,
        backend: Backend::None,
        start: Instant::now(),
        space: Space::default(),
        popups: PopupManager::default(),
        seat,
        seat_state,
        wins: HashMap::new(),
        stack: Vec::new(),
        next_id: 1,
        cascade: 0,
        active: 0,
        current_desktop: 0,
        num_desktops: 1,
        showing_desktop: false,
        drag: None,
        kbd_drag: None,
        shake: Shake::default(),
        shaken: Vec::new(),
        snap_preview: None,
        snap_wa: None,
        peek: None,
        switcher: None,
        switcher_start_active: 0,
        ripple: None,
        keys: keys::Keys::default(),
        decor,
        glass,
        live_colorization: None,
        cursors: cursor::Cursors::load(),
        cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
        frame_cursor: None,
        dnd_icon: None,
        needs_redraw: true,
        subscribers: Vec::new(),
        state_changed: false,
        ipc_path: std::path::PathBuf::new(),
        config_mtime: std::cell::Cell::new(None),
        last_input: Instant::now(),
        dpms_timeout: 0,
        dpms_off: false,
        saver_timeout: 0,
        saver_cmd: String::new(),
        saver_running: false,
        lock: None,
        inhibitors: Vec::new(),
        input_config,
        devices: Vec::new(),
        kb_layouts,
        kb_config,
        pointer_cons_active: false,
        suppressed: Vec::new(),
        last_click: (0, Instant::now(), 0, 0),
        resize_next: None,
        kbd_drag_orig: None,
        switcher_id: smithay::backend::renderer::element::Id::new(),
        peek_sheets: HashMap::new(),
        peek_ids: Default::default(),
        snap_seq: 0,
        last_group: 0,
        startup_cmd: None,
    };
    wm.load_session_settings();
    ipc::listen(&mut wm)?;

    // Nested inside another desktop for testing, on the DRM device otherwise.
    let nested = std::env::var_os("WAYLAND_DISPLAY_PARENT").is_some() || std::env::var_os("AERODE_NESTED").is_some();
    if nested { winit::init(&mut wm, &event_loop)?; } else { udev::init(&mut wm, &event_loop)?; }
    wm.glass.set_wallpaper(wm.screen_size());
    wm.update_workarea();
    // SAFETY: single-threaded, nothing has spawned threads reading the environment yet
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &wm.socket_name);
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
        std::env::remove_var("DISPLAY");
        std::env::set_var("GDK_BACKEND", "wayland,x11");
    }


    eprintln!("aerode-wm: WAYLAND_DISPLAY={}", wm.socket_name);
    // the session's programs start once XWayland is up, so they inherit DISPLAY
    wm.startup_cmd = std::env::args().skip_while(|a| a != "--").nth(1);
    xwm::start(&mut wm);

    // animation timers and the session's idle clock
    let timer = smithay::reexports::calloop::timer::Timer::from_duration(anim::FRAME);
    event_loop.handle().insert_source(timer, |_, _, wm: &mut Wm| {
        wm.tick();
        smithay::reexports::calloop::timer::TimeoutAction::ToDuration(if wm.animating() { anim::FRAME } else { Duration::from_millis(100) })
    }).map_err(|e| anyhow::anyhow!("timer: {e}"))?;

    event_loop.run(None, &mut wm, |wm| {
        wm.after_dispatch();
    })?;
    let _ = std::fs::remove_file(&wm.ipc_path);
    Ok(())
}

impl Wm {
    /// Run what came after `--` on the command line, once.
    pub fn run_startup_cmd(&mut self) {
        if let Some(cmd) = self.startup_cmd.take() { crate::util::spawn(&["sh", "-c", &cmd]); }
    }

    fn after_dispatch(&mut self) {
        self.space.refresh();
        self.popups.cleanup();
        for o in self.space.outputs().cloned().collect::<Vec<_>>() { layer_map_for_output(&o).cleanup(); }
        if std::mem::take(&mut self.state_changed) { self.sync_toplevels(); self.publish_state(); }
        if self.needs_redraw { self.redraw_outputs(); }
        let _ = self.dh.flush_clients();
    }

    pub fn outputs(&self) -> Vec<Output> { self.space.outputs().cloned().collect() }

    pub fn monitors(&self) -> Vec<Rect> {
        let mut v: Vec<(Rect, String)> = self.space.outputs().filter_map(|o| Some((Rect::from_smithay(self.space.output_geometry(o)?), o.name()))).collect();
        v.sort_by_key(|(r, _)| (r.x, r.y));
        v.into_iter().map(|x| x.0).collect()
    }

    pub fn screen_size(&self) -> (i32, i32) {
        let m = self.monitors();
        (m.iter().map(|r| r.x + r.w).max().unwrap_or(1), m.iter().map(|r| r.y + r.h).max().unwrap_or(1))
    }

    fn pointer_pos(&self) -> (i32, i32) {
        let p = self.seat.get_pointer().unwrap().current_location();
        (p.x as i32, p.y as i32)
    }

    pub fn pointer_monitor(&self) -> usize {
        let (x, y) = self.pointer_pos();
        self.monitors().iter().position(|m| m.contains(x, y)).unwrap_or(0)
    }

    fn monitor_of(&self, r: &Rect) -> usize {
        let (cx, cy) = r.center();
        let mons = self.monitors();
        mons.iter().position(|m| m.contains(cx, cy)).or_else(|| {
            let overlap = |m: &Rect| { let w = (r.x + r.w).min(m.x + m.w) - r.x.max(m.x); let h = (r.y + r.h).min(m.y + m.h) - r.y.max(m.y); w.max(0) * h.max(0) };
            mons.iter().enumerate().max_by_key(|(_, m)| overlap(m)).map(|(i, _)| i)
        }).unwrap_or(0)
    }

    pub fn top_of(&self, c: &Win) -> i32 { if !c.framed || c.fullscreen { 0 } else if c.maximized { MAX_TOP } else { TOP } }
    pub fn border_of(&self, c: &Win) -> i32 { if !c.framed || c.fullscreen || c.maximized { 0 } else { BORDER } }

    pub fn frame_rect(&self, c: &Win) -> Rect {
        let (b, t) = (self.border_of(c), self.top_of(c));
        Rect { x: c.geom.x - b, y: c.geom.y - t, w: c.geom.w + 2 * b, h: c.geom.h + t + b }
    }

    /// The monitor minus what panels reserve (the taskbar's exclusive zone).
    pub fn workarea_for(&self, mon: usize) -> Rect {
        let m = self.monitors().get(mon).copied().unwrap_or_default();
        for o in self.space.outputs() {
            if self.space.output_geometry(o).map(Rect::from_smithay) == Some(m) {
                let z = layer_map_for_output(o).non_exclusive_zone();
                return Rect { x: m.x + z.loc.x, y: m.y + z.loc.y, w: z.size.w.max(1), h: z.size.h.max(1) };
            }
        }
        m
    }

    /// Panels or monitors changed: keep maximised/snapped/fullscreen windows fitted.
    pub fn update_workarea(&mut self) {
        for o in self.outputs() { layer_map_for_output(&o).arrange(); }
        let ids: Vec<u32> = self.wins.keys().copied().collect();
        for w in ids { self.refit(w); }
    }

    fn refit(&mut self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        let mon = self.monitor_of(&self.frame_rect(c));
        if c.fullscreen { let m = self.monitors().get(mon).copied().unwrap_or_default(); self.configure(w, m); }
        else if c.maximized { let wa = self.workarea_for(mon); self.maximize_in(w, wa); }
        else if let Some(side) = c.snapped { let wa = self.workarea_for(mon); self.snap_in(w, side, wa); }
    }

    /// Client geometry that makes a window's *visible* outer edge fill `outer`.
    fn fit_rect(&self, c: &Win, outer: Rect, maximized: bool) -> Rect {
        if c.framed {
            let (b, t) = if maximized { (0, MAX_TOP) } else { (BORDER, TOP) };
            Rect { x: outer.x + b, y: outer.y + t, w: outer.w - 2 * b, h: outer.h - t - b }
        } else { outer }
    }

    /// Move/resize so the *client* area is `g`: tells the client the size and moves the window.
    pub fn configure(&mut self, w: u32, g: Rect) {
        let Some(c) = self.wins.get_mut(&w) else { return };
        let g = Rect { w: g.w.max(c.min_size.0.max(1)), h: g.h.max(c.min_size.1.max(1)), ..g };
        let size_changed = (g.w, g.h) != (c.geom.w, c.geom.h);
        if let Some(x) = c.window.x11_surface() {
            let _ = x.configure(Rectangle::new((g.x, g.y).into(), (g.w, g.h).into()));
            c.geom = g;
            if size_changed { c.frame_cache = None; }
        } else if size_changed {
            // frame and buffer change together: the window keeps its place and size until the client answers
            c.pending = Some(g);
            if let Some(t) = c.window.toplevel().cloned() {
                t.with_pending_state(|s| s.size = Some(Size::from((g.w, g.h))));
                // the states go in the same configure: Chromium takes a screen-sized configure
                // without Fullscreen as its fullscreen request refused (the video drops out of
                // full screen, then the next configure makes the whole page full screen)
                self.pending_states(w);
                t.send_pending_configure();
            }
        } else {
            c.geom = g;
            c.pending = None;
        }
        self.sync_mapped(w);
        self.needs_redraw = true;
        self.state_changed = true;
    }

    /// Tell an xdg toplevel its states (maximized/fullscreen/tiled look).
    pub fn send_states(&mut self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        let active = self.active == w;
        if let Some(t) = c.window.toplevel() {
            self.pending_states(w);
            t.send_pending_configure();
        }
        if let Some(x) = c.window.x11_surface() {
            let _ = x.set_maximized(c.maximized);
            let _ = x.set_fullscreen(c.fullscreen);
            let _ = x.set_activated(active);
        }
        self.state_changed = true;
    }

    /// An xdg toplevel's states into its next configure, without sending it.
    fn pending_states(&self, w: u32) {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as S;
        let Some(c) = self.wins.get(&w) else { return };
        let active = self.active == w;
        if let Some(t) = c.window.toplevel() {
            let (maxed, full, snapped) = (c.maximized, c.fullscreen, c.snapped);
            t.with_pending_state(|s| {
                let mut set = |st: S, on: bool| { if on { s.states.set(st); } else { s.states.unset(st); } };
                set(S::Maximized, maxed);
                set(S::Fullscreen, full);
                set(S::Activated, active);
                set(S::TiledLeft, snapped == Some(-1));
                set(S::TiledRight, snapped == Some(1));
                set(S::TiledTop, snapped.is_some());
                set(S::TiledBottom, snapped.is_some());
            });
        }
    }

    // ---------------------------------------------------------- state changes
    fn maximize_in(&mut self, w: u32, wa: Rect) {
        let Some(c) = self.wins.get_mut(&w) else { return };
        if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
        c.maximized = true;
        c.snapped = None;
        c.frame_cache = None;
        if !c.fullscreen {
            let r = { let c = &self.wins[&w]; self.fit_rect(c, wa, true) };
            self.configure(w, r);
        }
        self.send_states(w);
    }

    pub fn set_maximized(&mut self, w: u32, on: bool) {
        let Some(c) = self.wins.get(&w) else { return };
        if !is_app(c) || (on && (!c.resizable || !c.can_maximize)) { return; }
        let wa = self.workarea_for(self.monitor_of(&self.frame_rect(c)));
        if on {
            if !c.maximized { window_sound(crate::sounds::Event::Maximize); }
            self.begin_geom_anim(w);
            self.maximize_in(w, wa);
            return;
        }
        if !(c.maximized || c.snapped.is_some()) { return; }
        if c.maximized { window_sound(crate::sounds::Event::Restore); }
        self.begin_geom_anim(w);
        let c = self.wins.get_mut(&w).unwrap();
        c.maximized = false;
        c.snapped = None;
        c.frame_cache = None;
        let r = c.restore.take().unwrap_or(Rect { x: wa.x + 60, y: wa.y + 60, w: wa.w * 2 / 3, h: wa.h * 2 / 3 });
        if !c.fullscreen { self.configure(w, r); }
        self.send_states(w);
    }

    /// Aero Snap: -1 left half, 1 right half of the given work area, 2 vertical maximise.
    fn snap_in(&mut self, w: u32, side: i8, wa: Rect) {
        let Some(c) = self.wins.get(&w) else { return };
        let fr = if c.maximized { c.restore.map(|r| Rect { x: r.x - BORDER, w: r.w + 2 * BORDER, ..r }).unwrap_or_else(|| self.frame_rect(c)) } else { self.frame_rect(c) };
        let c = self.wins.get_mut(&w).unwrap();
        if !c.resizable || c.fullscreen { return; }
        if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
        c.maximized = false;
        c.snapped = Some(side);
        c.frame_cache = None;
        let half = wa.w / 2;
        let outer = match side {
            2 => Rect { x: fr.x, y: wa.y, w: fr.w, h: wa.h },
            s if s < 0 => Rect { x: wa.x, y: wa.y, w: half, h: wa.h },
            _ => Rect { x: wa.x + wa.w - half, y: wa.y, w: half, h: wa.h },
        };
        let r = { let c = &self.wins[&w]; self.fit_rect(c, outer, false) };
        self.configure(w, r);
        self.send_states(w);
    }

    pub fn snap(&mut self, w: u32, side: i8) {
        let Some(c) = self.wins.get(&w) else { return };
        if !is_app(c) { return; }
        let wa = self.workarea_for(self.monitor_of(&self.frame_rect(c)));
        self.begin_geom_anim(w);
        self.snap_in(w, side, wa);
    }

    pub fn set_fullscreen(&mut self, w: u32, on: bool) {
        if self.drag.map(|d| d.0 == w).unwrap_or(false) { self.end_drag(); }
        let Some(c) = self.wins.get(&w) else { return };
        let mon = self.monitor_of(&self.frame_rect(c));
        let m = self.monitors().get(mon).copied().unwrap_or_default();
        let c = self.wins.get_mut(&w).unwrap();
        c.frame_cache = None;
        if on && !c.fullscreen {
            if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
            c.fullscreen = true;
            self.configure(w, m);
        } else if !on && c.fullscreen {
            c.fullscreen = false;
            let (maxed, snapped) = (c.maximized, c.snapped);
            let wa = self.workarea_for(mon);
            if maxed { self.wins.get_mut(&w).unwrap().maximized = false; self.maximize_in(w, wa); }
            else if let Some(sd) = snapped { self.wins.get_mut(&w).unwrap().snapped = None; self.snap_in(w, sd, wa); }
            else {
                let r = self.wins.get_mut(&w).unwrap().restore.take().unwrap_or(Rect { x: m.x + 50, y: m.y + 80, w: m.w / 2, h: m.h / 2 });
                self.configure(w, r);
            }
        }
        self.restack();
        self.send_states(w);
    }

    /// Should this window be on screen right now?
    fn should_show(&self, c: &Win) -> bool {
        if c.minimized || !c.mapped { return false; }
        if self.showing_desktop && is_app(c) { return false; }
        c.desktop == self.current_desktop || c.desktop == u32::MAX
    }

    /// Put the window in the smithay space (or take it out), at the top-left of its client area.
    pub fn sync_mapped(&mut self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        let show = self.should_show(c);
        let (win, loc) = (c.window.clone(), (c.geom.x, c.geom.y));
        if show { self.space.map_element(win, loc, false); } else { self.space.unmap_elem(&win); }
    }

    fn sync_all(&mut self) {
        let ids: Vec<u32> = self.wins.keys().copied().collect();
        for w in ids { self.sync_mapped(w); }
        self.needs_redraw = true;
        self.state_changed = true;
    }

    fn effect(&self, key: &str) -> bool {
        self.reload_config_if_changed();
        crate::config::get().hw.effect(key)
    }

    fn reload_config_if_changed(&self) {
        let m = std::fs::metadata(crate::config::conf_dir().join("config.json")).and_then(|m| m.modified()).ok();
        if m != self.config_mtime.get() { self.config_mtime.set(m); crate::config::reload(); }
    }

    pub fn window_anims(&self) -> bool { self.effect("animate_minmax") }

    fn arrange_options(&self) -> (bool, usize) {
        self.reload_config_if_changed();
        let c = crate::config::get();
        if c.ease_of_access.get("noarrange").map(|v| v == "1").unwrap_or(false) { return (false, 0); }
        let flips = c.ease_of_access.get("shake").and_then(|v| v.parse().ok()).unwrap_or(SHAKE_DEFAULT_FLIPS);
        (true, flips)
    }

    fn focus_fallback(&mut self, except: u32) {
        let next = self.stack.iter().rev().copied().find(|x| *x != except && self.wins.get(x).map(|c| self.should_show(c) && is_app(c)).unwrap_or(false));
        match next { Some(n) => self.focus(n), None => self.clear_focus() }
    }

    fn clear_focus(&mut self) {
        let was = std::mem::take(&mut self.active);
        if was != 0 { self.send_states(was); self.fade_frame(was, false); }
        let kb = self.seat.get_keyboard().unwrap();
        kb.set_focus(self, None, SERIAL_COUNTER.next_serial());
        self.state_changed = true;
    }

    pub fn minimize(&mut self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        if !is_app(c) || c.minimized || !c.can_minimize { return; }
        window_sound(crate::sounds::Event::Minimize);
        if self.window_anims() { self.begin_minimize_anim(w, false); }
        let c = self.wins.get_mut(&w).unwrap();
        c.minimized = true;
        if c.anim.is_none() { self.sync_mapped(w); }
        if self.active == w { self.focus_fallback(w); }
        self.state_changed = true;
        self.needs_redraw = true;
    }

    pub fn unminimize(&mut self, w: u32) {
        let Some(c) = self.wins.get_mut(&w) else { return };
        if !c.minimized { return; }
        c.minimized = false;
        window_sound(crate::sounds::Event::Restore);
        if self.window_anims() { self.begin_minimize_anim(w, true); }
        self.sync_mapped(w);
        self.needs_redraw = true;
        self.state_changed = true;
    }

    pub fn close(&self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        if let Some(t) = c.window.toplevel() { t.send_close(); }
        if let Some(x) = c.window.x11_surface() { let _ = x.close(); }
    }

    /// Activate: unminimise, go to its desktop, raise, give it the keyboard.
    pub fn activate(&mut self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        let d = c.desktop;
        if self.showing_desktop { self.showing_desktop_set(false, false); }
        if d != u32::MAX && d != self.current_desktop { self.desktop_set(d, false); }
        self.unminimize(w);
        self.focus(w);
    }

    pub fn focus(&mut self, w: u32) {
        let Some(c) = self.wins.get(&w) else { return };
        if !self.should_show(c) { return; }
        let target = input::KFocus::of_window(&c.window);
        let urgent = c.urgent;
        if urgent { self.wins.get_mut(&w).unwrap().urgent = false; }
        let was = self.active;
        self.active = w;
        if was != w {
            if was != 0 { self.send_states(was); self.fade_frame(was, false); }
            self.send_states(w);
            self.fade_frame(w, true);
        }
        if let Some(t) = target {
            if self.lock.is_none() {
                let kb = self.seat.get_keyboard().unwrap();
                kb.set_focus(self, Some(t), SERIAL_COUNTER.next_serial());
            }
        }
        self.raise(w);
        self.state_changed = true;
    }

    pub fn raise(&mut self, w: u32) {
        self.stack.retain(|x| *x != w);
        self.stack.push(w);
        // X11 needs to hear about it too: override-redirect menus stack against this
        if let Some(c) = self.wins.get(&w) {
            if let (Some(x), Some(xwm)) = (c.window.x11_surface().cloned(), self.xwm.as_mut()) { let _ = xwm.raise_window(&x); }
        }
        self.restack();
    }

    /// Order: normal < always-on-top < fullscreen; a window's transients stay above it.
    pub fn restack(&mut self) {
        let mut order: Vec<u32> = Vec::with_capacity(self.stack.len());
        let rank = |c: &Win| if c.fullscreen { 2 } else if c.above { 1 } else { 0 };
        let mut ranked: Vec<(i32, u32)> = self.stack.iter().filter_map(|x| self.wins.get(x).map(|c| (rank(c), *x))).collect();
        ranked.sort_by_key(|x| x.0);
        for (_, x) in &ranked {
            let t = self.wins[x].transient_for;
            if t != 0 && self.wins.contains_key(&t) { continue; }
            order.push(*x);
        }
        for (_, x) in &ranked {
            let t = self.wins[x].transient_for;
            if t != 0 && self.wins.contains_key(&t) {
                let at = order.iter().rposition(|o| *o == t || self.wins[o].transient_for == t).map(|i| i + 1).unwrap_or(order.len());
                order.insert(at, *x);
            }
        }
        self.stack = order;
        self.needs_redraw = true;
        self.state_changed = true;
    }

    pub fn set_showing_desktop(&mut self, on: bool) { self.showing_desktop_set(on, true) }

    fn showing_desktop_set(&mut self, on: bool, refocus: bool) {
        self.showing_desktop = on;
        self.sync_all();
        if refocus { if on { self.clear_focus(); } else { self.focus_fallback(0); } }
    }

    pub fn set_desktop(&mut self, d: u32) { self.desktop_set(d, true) }

    fn desktop_set(&mut self, d: u32, refocus: bool) {
        let d = d.min(self.num_desktops.saturating_sub(1));
        if d == self.current_desktop { return; }
        self.current_desktop = d;
        self.sync_all();
        if refocus { self.focus_fallback(0); }
    }

    pub fn set_num_desktops(&mut self, n: u32) {
        let n = n.max(1);
        self.num_desktops = n;
        for c in self.wins.values_mut() { if c.desktop != u32::MAX && c.desktop >= n { c.desktop = n - 1; } }
        if self.current_desktop >= n { self.current_desktop = n - 1; }
        self.sync_all();
    }

    /// Caption buttons of this window with their states.
    pub fn buttons_for(&self, c: &Win) -> Vec<(Btn, BState)> {
        let st = |b: Btn| if c.pressed == Some(b) { BState::Pressed } else if c.hover == Some(b) { BState::Hot } else { BState::Normal };
        match c.kind {
            Kind::Dialog | Kind::Utility if !c.resizable => vec![(Btn::Close, st(Btn::Close))],
            Kind::Utility => vec![(Btn::Close, st(Btn::Close))],
            _ => {
                // Win7: no minimize and no maximize box -> Close alone; one of them -> both, the
                // missing one greyed
                let (can_min, can_max) = (c.can_minimize, c.can_maximize && c.resizable);
                if !c.can_minimize && !c.can_maximize { return vec![(Btn::Close, st(Btn::Close))]; }
                let mut v = vec![(Btn::Min, if can_min { st(Btn::Min) } else { BState::Disabled }), (Btn::Max, if can_max { st(Btn::Max) } else { BState::Disabled }), (Btn::Close, st(Btn::Close))];
                if c.kind == Kind::Dialog && c.transient_for != 0 { v.remove(0); v.remove(0); }
                v
            }
        }
    }

    // ---------------------------------------------------------- Aero Shake, monitors, peek
    /// Aero Shake (and Win+Home): minimise every other window; doing it again restores the
    /// windows the last shake minimised that are still minimised.
    pub fn aero_shake(&mut self, keep: u32) {
        let prev = std::mem::take(&mut self.shaken);
        let back: Vec<u32> = prev.into_iter().filter(|x| *x != keep && self.wins.get(x).map(|c| c.minimized).unwrap_or(false)).collect();
        if !back.is_empty() {
            let mut back = back;
            back.sort_by_key(|x| std::cmp::Reverse(self.stack.iter().position(|s| s == x).unwrap_or(0)));
            for x in back { self.unminimize(x); }
            self.focus(keep);
            return;
        }
        let ids: Vec<u32> = self.wins.values()
            .filter(|c| c.id != keep && is_app(c) && !c.minimized && self.should_show(c) && !c.skip_taskbar && c.transient_for != keep)
            .map(|c| c.id).collect();
        for x in &ids { self.minimize(*x); }
        self.shaken = ids;
        self.focus(keep);
    }

    /// Win+Shift+Left/Right: move a window to the neighbouring monitor (wrapping around), keeping
    /// its maximised/snapped state and its relative position.
    pub fn move_to_monitor(&mut self, w: u32, dir: i32) {
        let mons = self.monitors();
        if mons.len() < 2 { return; }
        let Some(c) = self.wins.get(&w) else { return };
        if !is_app(c) { return; }
        let cur = self.monitor_of(&self.frame_rect(c));
        let to = (cur as i32 + dir).rem_euclid(mons.len() as i32) as usize;
        let (ma, mb) = (mons[cur], mons[to]);
        let g = c.geom;
        let (maxed, snapped, full) = (c.maximized, c.snapped, c.fullscreen);
        let nx = mb.x + (g.x - ma.x) * mb.w / ma.w.max(1);
        let ny = mb.y + (g.y - ma.y) * mb.h / ma.h.max(1);
        self.configure(w, Rect { x: nx, y: ny, ..g });
        let wa = self.workarea_for(to);
        if full { self.configure(w, mb); }
        else if maxed { self.maximize_in(w, wa); }
        else if let Some(s) = snapped { self.snap_in(w, s, wa); }
    }

    pub fn peek(&mut self, target: u32, on: bool) {
        self.peek = on.then(|| (target, anim::Tween::new(1.0, 0.0, anim::PEEK_FADE, anim::Ease::Out)));
        if !on && self.peek.is_none() { self.needs_redraw = true; }
        self.needs_redraw = true;
    }

    /// Current Alt+Tab order: most recent first, as Win7 lists them.
    pub fn switch_order(&self) -> Vec<u32> {
        let listed = |c: &Win| !c.skip_taskbar && match c.kind {
            Kind::Normal | Kind::Dialog | Kind::Splash => true,
            Kind::Utility => c.transient_for == 0,
            _ => false,
        };
        let mut v: Vec<u32> = self.stack.iter().rev().copied().filter(|w| self.wins.get(w).map(listed).unwrap_or(false)).collect();
        if let Some(i) = v.iter().position(|w| *w == self.active) { let a = v.remove(i); v.insert(0, a); }
        v
    }

    pub fn switcher_step(&mut self, fwd: bool) {
        if self.switcher.is_none() {
            let wins = self.switch_order();
            if wins.is_empty() { return; }
            self.switcher_start_active = self.active;
            self.switcher = switcher::Switcher::new(self, wins);
        }
        if let Some(mut s) = self.switcher.take() { s.step(fwd); s.draw(self); self.switcher = Some(s); }
        self.needs_redraw = true;
    }

    pub fn switch_commit(&mut self) {
        if let Some(s) = self.switcher.take() { if let Some(w) = s.selected() { self.activate(w); } }
        self.needs_redraw = true;
    }

    pub fn switcher_cancel(&mut self) {
        self.switcher = None;
        self.needs_redraw = true;
    }

    // ---------------------------------------------------------- drags
    pub fn end_drag(&mut self) {
        if let Some((w, Drag::Resize { .. })) = self.drag {
            use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as S;
            if let Some(t) = self.wins.get(&w).and_then(|c| c.window.toplevel().cloned()) {
                t.with_pending_state(|s| { s.states.unset(S::Resizing); });
                t.send_pending_configure();
            }
        }
        self.drag = None;
        self.kbd_drag = None;
        self.frame_cursor = None;
        self.snap_wa = None;
        self.shake = Shake::default();
        self.needs_redraw = true;
    }

    /// Advance timers; called every frame while animating and ten times a second otherwise.
    pub fn peek_id(&self, w: u32) -> smithay::backend::renderer::element::Id { self.peek_ids.borrow_mut().entry(w).or_insert_with(smithay::backend::renderer::element::Id::new).clone() }

    fn tick(&mut self) {
        self.lock_tick();
        self.restore_focus_if_lost();
        let now = Instant::now();
        let mut any = false;
        let ids: Vec<u32> = self.wins.keys().copied().collect();
        for w in ids {
            let c = self.wins.get_mut(&w).unwrap();
            let glowing = c.glow.prune(now);
            let fading = c.focus_fade.is_some_and(|t| !t.done(now));
            if !fading { c.focus_fade = None; }
            if glowing || fading { c.frame_cache = None; any = true; }
            if let Some(a) = c.anim.as_ref() {
                any = true;
                if a.done(now) {
                    c.anim = None;
                    if c.minimized { self.sync_mapped(w); }
                }
            }
        }
        if self.peek.as_ref().is_some_and(|p| !p.1.done(now)) { any = true; }
        if self.snap_preview.as_ref().is_some_and(|s| s.2.animating(now)) { any = true; }
        if let Some(r) = &self.ripple { if r.2.done(now) { self.ripple = None; } any = true; }
        if any { self.needs_redraw = true; }
        self.session_tick();
    }

    fn animating(&self) -> bool {
        let now = Instant::now();
        self.ripple.is_some() || self.peek.as_ref().is_some_and(|p| !p.1.done(now))
            || self.snap_preview.as_ref().is_some_and(|s| s.2.animating(now))
            || self.wins.values().any(|c| c.anim.is_some() || c.focus_fade.is_some() || !c.glow.0.is_empty())
    }

    /// Display power and screen saver timeouts.
    fn session_tick(&mut self) {
        if !self.inhibitors.is_empty() { self.last_input = Instant::now(); }
        let idle = self.last_input.elapsed().as_secs() as u32;
        if self.dpms_timeout > 0 && idle >= self.dpms_timeout && !self.dpms_off { self.set_dpms(false); }
        if self.saver_timeout > 0 && idle >= self.saver_timeout && !self.saver_running && !self.saver_cmd.is_empty() && self.lock.is_none() {
            self.saver_running = true;
            crate::util::spawn(&["sh", "-c", &self.saver_cmd.clone()]);
        }
        if idle < self.saver_timeout.max(1) { self.saver_running = false; }
    }

    /// The Control Panel's saved timeouts: `display-off`, `screensaver-wait`.
    fn load_session_settings(&mut self) {
        let conf = crate::config::conf_dir();
        let read = |n: &str| std::fs::read_to_string(conf.join(n)).ok().and_then(|s| s.trim().parse::<u32>().ok());
        self.dpms_timeout = read("display-off").unwrap_or(0);
        self.saver_timeout = read("screensaver-wait").unwrap_or(0);
        if self.saver_timeout > 0 { self.saver_cmd = "aerode screensaver".into(); }
    }

    pub fn set_dpms(&mut self, on: bool) {
        self.dpms_off = !on;
        if let Backend::Udev(_) = self.backend { udev::set_dpms(self, on); }
        if on { self.needs_redraw = true; }
    }

    fn publish_state(&mut self) {
        use std::io::Write;
        self.subscribers.retain_mut(|s| s.write_all(b"{\"ev\":\"state\"}\n").is_ok());
    }
}
