//! The private channel between the compositor (`aerode-wm`) and the shell, apps and Control Panel:
//! one JSON line per request, one JSON line back, on `$XDG_RUNTIME_DIR/aerode-$WAYLAND_DISPLAY.sock`.
//! Same-user only (the socket is 0600 inside the runtime dir), like root window properties were.
//! `Req::Subscribe` turns a connection into a stream of `{"ev":...}` lines.
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Req {
    Subscribe,
    /// -> `State`
    State,
    Activate { id: u32 },
    Minimize { id: u32 },
    Close { id: u32 },
    SetMaximized { id: u32, on: bool },
    KeyboardMoveResize { id: u32, size: bool },
    /// the window's visible outer rectangle (frame included); `outer: false` means the client area
    Place { id: u32, x: i32, y: i32, w: i32, h: i32, outer: bool },
    SetShowingDesktop { on: bool },
    SetDesktop { n: u32 },
    SetNumDesktops { n: u32 },
    /// where the taskbar shows this window: minimising flies there
    IconGeometry { id: u32, x: i32, y: i32, w: i32, h: i32 },
    /// Aero Peek: `id` stays, the others turn to glass outlines; `on: false` ends it
    Peek { id: u32, on: bool },
    /// -> `{"path":..,"w":..,"h":..}`: premultiplied BGRA (cairo ARGB32) in a file the caller deletes
    Thumb { id: u32, w: i32, h: i32 },
    /// -> `{"path":..,"w":..,"h":..}`: same format, the whole virtual screen
    Screenshot,
    /// -> `{"x":..,"y":..}`
    Pointer,
    /// -> `{"ms":..}` since the last input
    Idle,
    /// display power: seconds of idle before turning off (0 = never), or now
    SetDpmsTimeout { secs: u32 },
    DpmsOff,
    /// seconds of idle before running the screen saver command (0 = never)
    SetSaverTimeout { secs: u32, cmd: String },
    /// lock the session: only `cmd`'s windows show until it exits with 0
    Lock { cmd: String },
    /// -> `{"locked":..}`
    Locked,
    /// xkb names; a comma-separated layout list switches groups with `group`
    SetKeyboard { layout: String, variant: String, options: String },
    /// -> `{"group":..,"layouts":[..]}`
    KeyboardState,
    SetGroup { group: u32 },
    /// key repeat and pointer settings (Mouse and Keyboard Properties)
    SetInput { repeat_delay: Option<i32>, repeat_rate: Option<i32>, pointer_speed: Option<f64>, left_handed: Option<bool>, natural_scroll: Option<bool> },
    /// xkb accessibility controls (Ease of Access): sticky/slow/bounce keys
    SetAccessKeys { sticky: bool },
    /// -> `{"outputs":[Output..]}`
    Outputs,
    /// null fields keep the current value; `enabled: false` turns the output off
    SetOutput { name: String, enabled: Option<bool>, w: Option<i32>, h: Option<i32>, refresh_mhz: Option<i32>, x: Option<i32>, y: Option<i32>, scale: Option<f64>, transform: Option<u32> },
    /// live colourisation, None = back to the saved one
    SetColorization { value: Option<[u32; 7]> },
    /// -> `{"value":[..]|null}`
    GetColorization,
    /// glass surfaces of this client: the blurred rectangles of a surface by its title/namespace
    /// (shell windows use layer-shell namespaces; see `glass_ns`)
    ReloadConfig,
    /// the pointer theme chosen in Control Panel
    SetCursor { theme: String, size: u32 },
    /// a window's system menu items (written by the app) and picks (read by it)
    Spawn { cmd: String },
    /// end the session
    Quit,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Win {
    pub id: u32,
    pub title: String,
    pub app_id: String,
    pub pid: u32,
    pub minimized: bool,
    pub urgent: bool,
    pub desktop: u32,
    pub maximized: bool,
    pub fullscreen: bool,
    pub resizable: bool,
    pub can_minimize: bool,
    pub can_maximize: bool,
    pub above: bool,
    pub taskbar: bool,
    /// outer rectangle, frame included
    pub frame: [i32; 4],
    /// client area
    pub geom: [i32; 4],
    pub dialog: bool,
    pub transient_for: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct State {
    /// bottom to top
    pub windows: Vec<Win>,
    pub active: u32,
    pub showing_desktop: bool,
    pub current_desktop: u32,
    pub num_desktops: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Output {
    pub name: String,
    pub make: String,
    pub model: String,
    pub enabled: bool,
    pub x: i32, pub y: i32, pub w: i32, pub h: i32,
    pub refresh_mhz: i32,
    pub scale: f64,
    pub transform: u32,
    /// (w, h, refresh in mHz)
    pub modes: Vec<(i32, i32, i32)>,
    pub phys_mm: (i32, i32),
    pub connected: bool,
}

/// The socket of the session this process belongs to.
pub fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(PathBuf::from(dir).join(format!("aerode-{display}.sock")))
}

/// One request on a fresh connection. Blocks for the reply, which the compositor gives right away.
pub fn call(req: &Req) -> Option<serde_json::Value> {
    let mut s = UnixStream::connect(socket_path()?).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_secs(3))).ok()?;
    let mut line = serde_json::to_string(req).ok()?;
    line.push('\n');
    s.write_all(line.as_bytes()).ok()?;
    let mut back = String::new();
    BufReader::new(s).read_line(&mut back).ok()?;
    serde_json::from_str(&back).ok()
}

/// Fire and forget.
pub fn send(req: Req) { let _ = call(&req); }

/// A connection that receives `{"ev":..}` lines; poll its fd and read lines.
pub fn subscribe() -> Option<UnixStream> {
    let mut s = UnixStream::connect(socket_path()?).ok()?;
    let mut line = serde_json::to_string(&Req::Subscribe).ok()?;
    line.push('\n');
    s.write_all(line.as_bytes()).ok()?;
    Some(s)
}

pub fn state() -> State {
    call(&Req::State).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default()
}

/// Read a `Thumb`/`Screenshot` reply into a cairo surface, removing the file.
pub fn read_image(v: &serde_json::Value) -> Option<cairo::ImageSurface> {
    let path = v.get("path")?.as_str()?;
    let (w, h) = (v.get("w")?.as_i64()? as i32, v.get("h")?.as_i64()? as i32);
    let data = std::fs::read(path).ok();
    let _ = std::fs::remove_file(path);
    cairo::ImageSurface::create_for_data(data?, cairo::Format::ARgb32, w, h, w * 4).ok()
}
