//! aerode-wm: a small reparenting X11 window manager drawing Windows 7 frames.
pub mod decor;
pub mod glass;
pub mod keys;
pub mod switcher;

use anyhow::{Context as _, Result};
use std::collections::HashMap;
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::CURRENT_TIME;

use decor::{BState, Btn, Decor, Params, BORDER, MAX_TOP, TOP};

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        WM_PROTOCOLS, WM_DELETE_WINDOW, WM_TAKE_FOCUS, WM_STATE, WM_CHANGE_STATE, WM_TRANSIENT_FOR,
        UTF8_STRING, _MOTIF_WM_HINTS,
        _NET_SUPPORTED, _NET_SUPPORTING_WM_CHECK, _NET_WM_NAME, _NET_CLIENT_LIST, _NET_CLIENT_LIST_STACKING,
        _NET_ACTIVE_WINDOW, _NET_CLOSE_WINDOW, _NET_WORKAREA, _NET_NUMBER_OF_DESKTOPS, _NET_CURRENT_DESKTOP,
        _NET_DESKTOP_NAMES, _NET_DESKTOP_GEOMETRY, _NET_DESKTOP_VIEWPORT, _NET_SHOWING_DESKTOP,
        _NET_WM_DESKTOP, _NET_WM_STATE, _NET_WM_STATE_MAXIMIZED_VERT, _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_FULLSCREEN, _NET_WM_STATE_HIDDEN, _NET_WM_STATE_ABOVE, _NET_WM_STATE_BELOW,
        _NET_WM_STATE_SKIP_TASKBAR, _NET_WM_STATE_SKIP_PAGER, _NET_WM_STATE_DEMANDS_ATTENTION,
        _NET_WM_STATE_MODAL, _NET_WM_STATE_STICKY, _NET_WM_STATE_FOCUSED,
        _NET_WM_WINDOW_TYPE, _NET_WM_WINDOW_TYPE_DESKTOP, _NET_WM_WINDOW_TYPE_DOCK,
        _NET_WM_WINDOW_TYPE_TOOLBAR, _NET_WM_WINDOW_TYPE_MENU, _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_WINDOW_TYPE_SPLASH, _NET_WM_WINDOW_TYPE_DIALOG, _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_NOTIFICATION, _NET_WM_WINDOW_TYPE_DROPDOWN_MENU, _NET_WM_WINDOW_TYPE_POPUP_MENU,
        _NET_WM_WINDOW_TYPE_TOOLTIP,
        _NET_WM_STRUT, _NET_WM_STRUT_PARTIAL, _NET_WM_ICON, _NET_WM_PID, _NET_FRAME_EXTENTS,
        _NET_REQUEST_FRAME_EXTENTS, _NET_WM_MOVERESIZE, _NET_MOVERESIZE_WINDOW, _NET_WM_ALLOWED_ACTIONS,
        _NET_WM_ACTION_MOVE, _NET_WM_ACTION_RESIZE, _NET_WM_ACTION_MINIMIZE, _NET_WM_ACTION_MAXIMIZE_HORZ,
        _NET_WM_ACTION_MAXIMIZE_VERT, _NET_WM_ACTION_FULLSCREEN, _NET_WM_ACTION_CLOSE, _NET_WM_ACTION_CHANGE_DESKTOP,
        _NET_WM_USER_TIME, _NET_WM_WINDOW_OPACITY, _GTK_FRAME_EXTENTS, _NET_CM_S0, _AERODE_SNAP_PREVIEW, _AERODE_RESTART, _GTK_WORKAREAS, _AERODE_GLASS, _AERODE_COLORIZATION,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Normal, Dialog, Utility, Dock, Desktop, Splash, Notification, Menu }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

impl Rect {
    fn contains(&self, x: i32, y: i32) -> bool { x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h }
    fn center(&self) -> (i32, i32) { (self.x + self.w / 2, self.y + self.h / 2) }
}

pub struct Client {
    pub win: u32,
    pub frame: Option<u32>, // None = undecorated (mapped directly)
    pub kind: Kind,
    pub title: String,
    pub icon: Option<cairo::ImageSurface>,
    pub geom: Rect,        // client geometry (root coords)
    pub restore: Option<Rect>,
    pub maximized: bool,
    pub fullscreen: bool,
    pub minimized: bool,
    pub above: bool,
    pub skip_taskbar: bool,
    pub urgent: bool,
    pub transient_for: u32,
    pub protocols: Vec<u32>,
    pub min_size: (i32, i32),
    pub resizable: bool,
    pub desktop: u32,
    pub hover: Option<Btn>,
    pub pressed: Option<Btn>,
    pub ignore_unmaps: u32,
    pub snapped: Option<i8>, // -1 left, 1 right, 2 vertically maximised (keeps x and width)
    pub mapped_once: bool,
    /// we have unmapped its top-level (minimized / other desktop / show desktop)
    pub wm_hidden: bool,
    /// last _NET_FRAME_EXTENTS written (avoid property spam)
    pub ext: [i32; 4],
    /// _GTK_FRAME_EXTENTS (invisible shadow margins of client-side decorated windows)
    pub csd: [i32; 4],
    /// WM_NORMAL_HINTS win_gravity == StaticGravity
    pub gravity_static: bool,
    /// input-only children of the frame that carry the resize cursors
    pub zones: Vec<u32>,
}

fn is_app(c: &Client) -> bool { matches!(c.kind, Kind::Normal | Kind::Dialog | Kind::Utility) }

#[derive(Clone, Copy, Debug)]
enum Drag {
    Move { cx: i32, cy: i32, ox: i32, oy: i32, from_max: bool },
    Resize { edges: u8, start: Rect, px: i32, py: i32 },
    Button { btn: Btn },
}

/// Aero Shake: a title-bar drag that reverses direction (by at least `SHAKE_MIN` px) `SHAKE_FLIPS`
/// times within `SHAKE_MS` minimises every other window; shaking again brings them back.
const SHAKE_MIN: i32 = 18;
const SHAKE_FLIPS: usize = 4;
const SHAKE_MS: u128 = 1000;

#[derive(Default)]
struct Shake {
    w: u32,
    t: Option<Instant>,
    dir: [i32; 2],
    extreme: [i32; 2],
    flips: Vec<Instant>,
}

impl Shake {
    /// Feed a pointer position of a move drag of `w`; true when the window was shaken.
    fn step(&mut self, w: u32, x: i32, y: i32) -> bool {
        let now = Instant::now();
        let stale = self.t.map(|t| now.duration_since(t).as_millis() > 400).unwrap_or(true);
        if self.w != w || stale { *self = Shake { w, t: Some(now), dir: [0, 0], extreme: [x, y], flips: vec![] }; return false; }
        self.t = Some(now);
        for (k, p) in [x, y].into_iter().enumerate() {
            let d = p - self.extreme[k];
            if self.dir[k] == 0 {
                if d.abs() >= SHAKE_MIN { self.dir[k] = d.signum(); self.extreme[k] = p; }
            } else if d.signum() == self.dir[k] {
                self.extreme[k] = p; // still going the same way
            } else if d.abs() >= SHAKE_MIN {
                self.dir[k] = d.signum();
                self.extreme[k] = p;
                self.flips.push(now);
            }
        }
        self.flips.retain(|t| now.duration_since(*t).as_millis() < SHAKE_MS);
        if self.flips.len() >= SHAKE_FLIPS { self.flips.clear(); self.dir = [0, 0]; self.extreme = [x, y]; return true; }
        false
    }
}

pub struct Wm {
    pub conn: RustConnection,
    pub root: u32,
    pub screen_num: usize,
    pub atoms: Atoms,
    pub clients: HashMap<u32, Client>,
    pub frames: HashMap<u32, u32>, // frame → client
    pub stack: Vec<u32>,           // bottom → top (clients)
    pub active: u32,
    /// the window that was active before `active` (focus returns to it when `active` goes away)
    prev_active: u32,
    pub monitors: Vec<Rect>,
    pub workarea: Rect,
    pub decor: Decor,
    pub argb: Option<(u32, u32)>, // (visual, colormap)
    pub check_win: u32,
    pub showing_desktop: bool,
    pub current_desktop: u32,
    pub num_desktops: u32,
    drag: Option<(u32, Drag)>,
    last_click: (u32, Instant, i32, i32),
    pub keys: keys::Keys,
    pub switcher: Option<switcher::Switcher>,
    pub snap_preview: Option<(u32, i8)>,
    snap_wa: Option<Rect>,
    /// keyboard move/size in progress: (window, geometry to restore on Esc)
    kbd_drag: Option<(u32, Rect)>,
    root_size: (i32, i32),
    last_move_redraw: Instant,
    composited_cache: std::cell::Cell<(Instant, bool)>,
    /// frame glass: colourisation + the wallpaper behind it
    pub glass: glass::Glass,
    /// `_AERODE_COLORIZATION` changed; applied once the event queue is empty
    glass_dirty: bool,
    /// Aero Shake detector for the window being dragged
    shake: Shake,
    /// windows the last Aero Shake / Win+Home minimised (shaking again restores them)
    shaken: Vec<u32>,
}

fn frame_events() -> EventMask {
    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY | EventMask::EXPOSURE | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::ENTER_WINDOW | EventMask::LEAVE_WINDOW
}

impl Wm {
    pub fn new() -> Result<Wm> {
        let (conn, screen_num) = x11rb::connect(None).context("cannot open display")?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE
            | EventMask::BUTTON_PRESS | EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE;
        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(mask))?
            .check().context("another window manager is already running")?;
        let _ = conn.randr_select_input(root, x11rb::protocol::randr::NotifyMask::SCREEN_CHANGE);
        // repaint frames when a compositor starts/stops (glass vs. opaque)
        if conn.xfixes_query_version(5, 0).ok().and_then(|c| c.reply().ok()).is_some() {
            let _ = conn.xfixes_select_selection_input(root, atoms._NET_CM_S0,
                x11rb::protocol::xfixes::SelectionEventMask::SET_SELECTION_OWNER
                    | x11rb::protocol::xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | x11rb::protocol::xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE);
        }
        // ARGB visual for translucent frames
        let scr = &conn.setup().roots[screen_num];
        let argb = scr.allowed_depths.iter().find(|d| d.depth == 32).and_then(|d| {
            d.visuals.iter().find(|v| v.class == VisualClass::TRUE_COLOR).map(|v| v.visual_id)
        }).map(|vis| {
            let cmap = conn.generate_id().unwrap();
            conn.create_colormap(ColormapAlloc::NONE, cmap, root, vis).unwrap();
            (vis, cmap)
        });
        let check_win = conn.generate_id()?;
        conn.create_window(0, check_win, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
        let decor = Decor::load();
        let glass = glass::Glass::new(crate::config::get().colorization.dwm(), decor.theme);
        let mut wm = Wm {
            conn, root, screen_num, atoms, clients: HashMap::new(), frames: HashMap::new(), stack: vec![],
            active: 0, prev_active: 0, monitors: vec![], workarea: Rect::default(), decor, argb, check_win,
            showing_desktop: false, current_desktop: 0, num_desktops: 1, drag: None,
            last_click: (0, Instant::now(), 0, 0), keys: keys::Keys::default(), switcher: None, snap_preview: None,
            snap_wa: None, root_size: (0, 0), kbd_drag: None,
            last_move_redraw: Instant::now(), composited_cache: std::cell::Cell::new((Instant::now() - std::time::Duration::from_secs(10), false)),
            glass, glass_dirty: false, shake: Shake::default(), shaken: vec![],
        };
        {
            let n = wm.prop32(wm.root, wm.atoms._NET_NUMBER_OF_DESKTOPS, AtomEnum::CARDINAL).first().copied().unwrap_or(1);
            let cur = wm.prop32(wm.root, wm.atoms._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL).first().copied().unwrap_or(0);
            wm.num_desktops = n.clamp(1, 16);
            wm.current_desktop = cur.min(wm.num_desktops - 1);
        }
        wm.update_monitors();
        wm.load_wallpaper();
        wm.read_colorization();
        wm.set_supported()?;
        wm.keys.grab(&wm.conn, wm.root)?;
        // the root shows the arrow too (the session's `xsetroot` may be missing): the desktop, docks,
        // override-redirect popups and the snap preview inherit it instead of X's default cross
        if let Some(cur) = keys::cursor(&wm.conn, wm.screen_num, "left_ptr").filter(|c| *c != x11rb::NONE) {
            let _ = wm.conn.change_window_attributes(wm.root, &ChangeWindowAttributesAux::new().cursor(cur));
        }
        wm.manage_existing()?;
        wm.conn.flush()?;
        spawn_compositor_watch(wm.check_win);
        Ok(wm)
    }

    fn load_wallpaper(&mut self) {
        let size = self.screen_size();
        self.glass.set_wallpaper(size);
    }

    /// The live colourisation (root `_AERODE_COLORIZATION`, written by Personalize ▸ Window Color),
    /// else the saved one. True when it changed.
    fn read_colorization(&mut self) -> bool {
        let v = self.prop32(self.root, self.atoms._AERODE_COLORIZATION, AtomEnum::CARDINAL);
        let c = crate::glass::from_prop(&v).unwrap_or_else(|| {
            crate::config::reload();
            crate::config::get().colorization.dwm()
        });
        self.glass.set_colorization(c)
    }

    fn apply_colorization(&mut self) {
        self.glass_dirty = false;
        if !self.read_colorization() { return; }
        let wins: Vec<u32> = self.clients.values().filter(|c| c.frame.is_some()).map(|c| c.win).collect();
        for w in wins { self.redraw(w); }
        if let Some(sw) = &self.switcher { sw.draw(self); }
    }

    pub fn screen_size(&self) -> (i32, i32) {
        if self.root_size.0 > 0 { return self.root_size; }
        let s = &self.conn.setup().roots[self.screen_num];
        (s.width_in_pixels as i32, s.height_in_pixels as i32)
    }

    pub fn update_monitors(&mut self) {
        // the root size changes with RandR; the connection setup data does not
        if let Ok(Ok(g)) = self.conn.get_geometry(self.root).map(|c| c.reply()) { self.root_size = (g.width as i32, g.height as i32); }
        let mut mons = vec![];
        if let Ok(r) = self.conn.randr_get_monitors(self.root, true).map(|c| c.reply()) {
            if let Ok(r) = r {
                for m in r.monitors {
                    mons.push(Rect { x: m.x as i32, y: m.y as i32, w: m.width as i32, h: m.height as i32 });
                }
            }
        }
        // mirrored outputs report the same rectangle twice; a monitor split into virtual ones
        // (xrandr --setmonitor, ultrawides) keeps its whole-output monitor next to the parts
        let mut uniq: Vec<Rect> = vec![];
        for m in mons { if !uniq.contains(&m) { uniq.push(m); } }
        let inside = |a: &Rect, b: &Rect| b.x >= a.x && b.y >= a.y && b.x + b.w <= a.x + a.w && b.y + b.h <= a.y + a.h;
        let mut mons: Vec<Rect> = uniq.iter().copied().filter(|a| uniq.iter().filter(|b| *b != a && inside(a, b)).count() < 2).collect();
        if mons.is_empty() {
            let (w, h) = self.screen_size();
            mons.push(Rect { x: 0, y: 0, w, h });
        }
        self.monitors = mons;
        self.update_workarea();
    }

    fn composited(&self) -> bool {
        let (t, v) = self.composited_cache.get();
        if t.elapsed().as_millis() < 1000 { return v; }
        let v = self.argb.is_some() && self.conn.get_selection_owner(self.atoms._NET_CM_S0).ok()
            .and_then(|c| c.reply().ok()).map(|r| r.owner != 0).unwrap_or(false);
        self.composited_cache.set((Instant::now(), v));
        v
    }

    fn set_supported(&self) -> Result<()> {
        let a = &self.atoms;
        let c = &self.conn;
        c.change_property32(PropMode::REPLACE, self.check_win, a._NET_SUPPORTING_WM_CHECK, AtomEnum::WINDOW, &[self.check_win])?;
        c.change_property8(PropMode::REPLACE, self.check_win, a._NET_WM_NAME, a.UTF8_STRING, b"Aerode")?;
        // our pid on the check window also tells `aerode restart` that this build can re-exec a binary that
        // ./install.sh replaced (older builds tear every frame down and then fail to exec "… (deleted)")
        c.change_property32(PropMode::REPLACE, self.check_win, a._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()])?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_SUPPORTING_WM_CHECK, AtomEnum::WINDOW, &[self.check_win])?;
        let sup = [a._NET_SUPPORTED, a._NET_SUPPORTING_WM_CHECK, a._NET_WM_NAME, a._NET_CLIENT_LIST, a._NET_CLIENT_LIST_STACKING,
            a._NET_ACTIVE_WINDOW, a._NET_CLOSE_WINDOW, a._NET_WORKAREA, a._NET_NUMBER_OF_DESKTOPS, a._NET_CURRENT_DESKTOP,
            a._NET_DESKTOP_GEOMETRY, a._NET_SHOWING_DESKTOP, a._NET_WM_DESKTOP, a._NET_WM_STATE, a._NET_WM_STATE_MAXIMIZED_VERT,
            a._NET_WM_STATE_MAXIMIZED_HORZ, a._NET_WM_STATE_FULLSCREEN, a._NET_WM_STATE_HIDDEN, a._NET_WM_STATE_ABOVE,
            a._NET_WM_STATE_SKIP_TASKBAR, a._NET_WM_STATE_DEMANDS_ATTENTION, a._NET_WM_STATE_FOCUSED, a._NET_WM_WINDOW_TYPE,
            a._NET_WM_STRUT, a._NET_WM_STRUT_PARTIAL, a._NET_FRAME_EXTENTS, a._NET_REQUEST_FRAME_EXTENTS, a._NET_WM_MOVERESIZE,
            a._NET_MOVERESIZE_WINDOW, a._NET_WM_ALLOWED_ACTIONS, a._GTK_FRAME_EXTENTS, a._GTK_WORKAREAS];
        c.change_property32(PropMode::REPLACE, self.root, a._NET_SUPPORTED, AtomEnum::ATOM, &sup)?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_NUMBER_OF_DESKTOPS, AtomEnum::CARDINAL, &[self.num_desktops])?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL, &[self.current_desktop])?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_SHOWING_DESKTOP, AtomEnum::CARDINAL, &[0])?;
        let (w, h) = self.screen_size();
        c.change_property32(PropMode::REPLACE, self.root, a._NET_DESKTOP_GEOMETRY, AtomEnum::CARDINAL, &[w as u32, h as u32])?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_DESKTOP_VIEWPORT, AtomEnum::CARDINAL, &[0, 0])?;
        c.change_property8(PropMode::REPLACE, self.root, a._NET_DESKTOP_NAMES, a.UTF8_STRING, b"Desktop 1\0")?;
        Ok(())
    }

    fn manage_existing(&mut self) -> Result<()> {
        let tree = self.conn.query_tree(self.root)?.reply()?;
        for w in tree.children {
            if w == self.check_win { continue; }
            let Ok(attr) = self.conn.get_window_attributes(w)?.reply() else { continue };
            if attr.override_redirect { continue; }
            let iconic = self.prop32(w, self.atoms.WM_STATE, self.atoms.WM_STATE).first() == Some(&3);
            if attr.map_state == MapState::VIEWABLE { self.manage_ex(w, false); } else if iconic { self.manage_ex(w, true); }
        }
        Ok(())
    }

    // ---------------------------------------------------------- properties
    fn prop32(&self, w: u32, atom: u32, ty: impl Into<u32>) -> Vec<u32> {
        self.conn.get_property(false, w, atom, ty.into(), 0, 1 << 20).ok().and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect())).unwrap_or_default()
    }

    fn read_title(&self, w: u32) -> String {
        let a = &self.atoms;
        let r = self.conn.get_property(false, w, a._NET_WM_NAME, a.UTF8_STRING, 0, 1024).ok().and_then(|c| c.reply().ok());
        if let Some(r) = r.filter(|r| !r.value.is_empty()) { return String::from_utf8_lossy(&r.value).into_owned(); }
        self.conn.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 1024).ok().and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.value).into_owned()).unwrap_or_default()
    }

    /// The window's icon: _NET_WM_ICON, else the icon named after its WM_CLASS in the icon themes,
    /// else the Win7 generic program icon (like Windows shows for programs without one).
    fn read_icon(&self, w: u32) -> Option<cairo::ImageSurface> {
        self.read_net_icon(w).or_else(|| self.theme_icon(w))
    }

    fn theme_icon(&self, w: u32) -> Option<cairo::ImageSurface> {
        let cls = self.conn.get_property(false, w, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256).ok()?.reply().ok()
            .map(|r| r.value).unwrap_or_default();
        let mut names: Vec<String> = cls.split(|b| *b == 0).filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).to_lowercase()).collect();
        names.push("application-x-executable".into());
        let data = gtk::glib::user_data_dir();
        let dirs = [data.join("icons/Aerode/48x48/apps"), data.join("icons/Aerode/32x32/apps"), data.join("icons/hicolor/48x48/apps"),
            "/usr/share/icons/hicolor/48x48/apps".into(), "/usr/share/icons/hicolor/64x64/apps".into(), "/usr/share/pixmaps".into()];
        for n in &names {
            for d in &dirs {
                let p = d.join(format!("{n}.png"));
                if let Ok(mut f) = std::fs::File::open(&p) {
                    if let Ok(img) = cairo::ImageSurface::create_from_png(&mut f) { return Some(img); }
                }
            }
        }
        None
    }

    fn read_net_icon(&self, w: u32) -> Option<cairo::ImageSurface> {
        let v = self.prop32(w, self.atoms._NET_WM_ICON, AtomEnum::CARDINAL);
        let (mut i, mut best): (usize, Option<(u32, u32, usize)>) = (0, None);
        while i + 2 <= v.len() {
            let (iw, ih) = (v[i], v[i + 1]);
            let n = (iw * ih) as usize;
            if iw == 0 || i + 2 + n > v.len() { break; }
            let good = |bw: u32| (iw >= 16 && (bw < 16 || iw < bw)) || (bw < 16 && iw > bw);
            if best.map(|b| good(b.0)).unwrap_or(true) { best = Some((iw, ih, i + 2)); }
            i += 2 + n;
        }
        let (iw, ih, off) = best?;
        let mut s = cairo::ImageSurface::create(cairo::Format::ARgb32, iw as i32, ih as i32).ok()?;
        let stride = s.stride() as usize;
        {
            let mut d = s.data().ok()?;
            for y in 0..ih as usize {
                for x in 0..iw as usize {
                    let px = v[off + y * iw as usize + x];
                    let a = px >> 24;
                    let pm = |c: u32| (c * a + 127) / 255;
                    let val = (a << 24) | (pm((px >> 16) & 255) << 16) | (pm((px >> 8) & 255) << 8) | pm(px & 255);
                    d[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&val.to_ne_bytes());
                }
            }
        }
        Some(s)
    }

    fn window_kind(&self, w: u32) -> Kind {
        let a = &self.atoms;
        for t in self.prop32(w, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM) {
            return match t {
                t if t == a._NET_WM_WINDOW_TYPE_DOCK => Kind::Dock,
                t if t == a._NET_WM_WINDOW_TYPE_DESKTOP => Kind::Desktop,
                t if t == a._NET_WM_WINDOW_TYPE_SPLASH => Kind::Splash,
                t if t == a._NET_WM_WINDOW_TYPE_NOTIFICATION => Kind::Notification,
                t if t == a._NET_WM_WINDOW_TYPE_DIALOG => Kind::Dialog,
                t if t == a._NET_WM_WINDOW_TYPE_UTILITY || t == a._NET_WM_WINDOW_TYPE_TOOLBAR => Kind::Utility,
                t if t == a._NET_WM_WINDOW_TYPE_MENU || t == a._NET_WM_WINDOW_TYPE_POPUP_MENU
                    || t == a._NET_WM_WINDOW_TYPE_DROPDOWN_MENU || t == a._NET_WM_WINDOW_TYPE_TOOLTIP => Kind::Menu,
                t if t == a._NET_WM_WINDOW_TYPE_NORMAL => Kind::Normal,
                _ => continue,
            };
        }
        if self.prop32(w, a.WM_TRANSIENT_FOR, AtomEnum::WINDOW).first().copied().unwrap_or(0) != 0 { Kind::Dialog } else { Kind::Normal }
    }

    fn wants_decor(&self, w: u32) -> bool {
        // _MOTIF_WM_HINTS: flags, functions, decorations, ...
        let m = self.prop32(w, self.atoms._MOTIF_WM_HINTS, self.atoms._MOTIF_WM_HINTS);
        !(m.len() >= 3 && m[0] & 2 != 0 && m[2] == 0)
    }

    // ---------------------------------------------------------- managing
    pub fn manage(&mut self, w: u32) { self.manage_ex(w, false) }

    fn read_csd(&self, w: u32) -> [i32; 4] {
        let v = self.prop32(w, self.atoms._GTK_FRAME_EXTENTS, AtomEnum::CARDINAL);
        if v.len() >= 4 { [v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32] } else { [0; 4] }
    }

    /// Passive grab so clicking an inactive window focuses + raises it (then replays the click).
    fn grab_click(&self, w: u32) {
        for b in [ButtonIndex::M1, ButtonIndex::M2, ButtonIndex::M3] {
            let _ = self.conn.grab_button(true, w, EventMask::BUTTON_PRESS, GrabMode::SYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, b, ModMask::ANY);
        }
    }

    /// Manage a new top-level. `start_minimized` adopts an iconic window (after a restart).
    pub fn manage_ex(&mut self, w: u32, start_minimized: bool) {
        if self.clients.contains_key(&w) || self.frames.contains_key(&w) || w == self.check_win { return; }
        let Ok(Ok(attr)) = self.conn.get_window_attributes(w).map(|c| c.reply()) else { return };
        if attr.override_redirect { let _ = self.conn.map_window(w); return; }
        let Ok(Ok(g)) = self.conn.get_geometry(w).map(|c| c.reply()) else { return };
        let kind = self.window_kind(w);
        let a = self.atoms;
        let state = self.prop32(w, a._NET_WM_STATE, AtomEnum::ATOM);
        let transient_for = self.prop32(w, a.WM_TRANSIENT_FOR, AtomEnum::WINDOW).first().copied().unwrap_or(0);
        let protocols = self.prop32(w, a.WM_PROTOCOLS, AtomEnum::ATOM);
        // WM_NORMAL_HINTS: flags, (pad x4), min w/h, max w/h, ..., win_gravity at [17]
        let nh = self.prop32(w, AtomEnum::WM_NORMAL_HINTS.into(), AtomEnum::WM_SIZE_HINTS);
        let (mut min_size, mut resizable) = ((80, 30), true);
        if nh.len() >= 9 {
            if nh[0] & 16 != 0 { min_size = (nh[5] as i32, nh[6] as i32); }
            if nh[0] & 32 != 0 && nh[0] & 16 != 0 && nh[7] == nh[5] && nh[8] == nh[6] { resizable = false; }
        }
        let gravity_static = nh.len() >= 18 && nh[0] & 512 != 0 && nh[17] == 10;
        let user_pos = nh.first().map(|f| f & 1 != 0 || f & 4 != 0).unwrap_or(false);
        let app = matches!(kind, Kind::Normal | Kind::Dialog | Kind::Utility);
        let decorated = app && self.wants_decor(w);
        let csd = if decorated { [0; 4] } else { self.read_csd(w) };
        let mut geom = Rect { x: g.x as i32, y: g.y as i32, w: g.width as i32, h: g.height as i32 };
        let (b, t) = if decorated { (BORDER, TOP) } else { (0, 0) };
        // ICCCM NorthWest gravity: a requested position is where the *frame* goes
        if decorated && user_pos && !gravity_static && !(geom.x == 0 && geom.y == 0) { geom.x += b; geom.y += t; }
        let on_screen = self.monitors.iter().any(|m| m.contains(geom.x + geom.w / 2, geom.y + geom.h / 2) || m.contains(geom.x, geom.y));
        if app && ((!user_pos && geom.x == 0 && geom.y == 0) || !on_screen) {
            // place new windows like Windows does: centred on the active monitor (dialogs on their owner)
            let wa = self.workarea_for(self.pointer_monitor());
            if let Some(p) = self.clients.get(&transient_for) {
                let (px, py) = self.frame_rect(p).center();
                geom.x = px - geom.w / 2;
                geom.y = py - geom.h / 2;
            } else {
                geom.x = wa.x + (wa.w - geom.w - 2 * b + csd[0] + csd[1]) / 2 + b - csd[0];
                geom.y = wa.y + (wa.h - geom.h - t - b + csd[2] + csd[3]) / 2 + t - csd[2];
                // cascade if another window already sits there
                let mut guard = 0;
                while guard < 20 && self.clients.values().any(|o| is_app(o) && (o.geom.x - geom.x).abs() < 8 && (o.geom.y - geom.y).abs() < 8) {
                    geom.x += 26; geom.y += 26; guard += 1;
                }
            }
            geom.x = geom.x.max(wa.x + b - csd[0]);
            geom.y = geom.y.max(wa.y + t - csd[2]);
        }
        // never put a title bar above the top of the screen
        if decorated {
            let top_min = self.monitors.iter().map(|m| m.y).min().unwrap_or(0);
            if geom.y - t < top_min { geom.y = top_min + t; }
        }
        let wm_desktop = self.prop32(w, a._NET_WM_DESKTOP, AtomEnum::CARDINAL).first().copied();
        let desktop = match wm_desktop { Some(d) if d == 0xffffffff || d < self.num_desktops => d, _ => self.current_desktop };
        let mut c = Client {
            win: w, frame: None, kind, title: self.read_title(w), icon: self.read_icon(w), geom, restore: None,
            maximized: false, fullscreen: state.contains(&a._NET_WM_STATE_FULLSCREEN), minimized: start_minimized,
            above: state.contains(&a._NET_WM_STATE_ABOVE) || kind == Kind::Dock, skip_taskbar: state.contains(&a._NET_WM_STATE_SKIP_TASKBAR),
            urgent: false, transient_for, protocols, min_size, resizable, desktop,
            hover: None, pressed: None, ignore_unmaps: 0, snapped: None, mapped_once: false,
            wm_hidden: false, ext: [-1; 4], csd, gravity_static, zones: vec![],
        };
        let _ = self.conn.change_window_attributes(w, &ChangeWindowAttributesAux::new()
            .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE));
        if decorated {
            let frame = self.conn.generate_id().unwrap();
            let (fx, fy, fw, fh) = (geom.x - BORDER, geom.y - TOP, geom.w + 2 * BORDER, geom.h + TOP + BORDER);
            let aux = CreateWindowAux::new().event_mask(frame_events()).background_pixel(0).border_pixel(0).bit_gravity(Gravity::NORTH_WEST);
            let res = if let Some((vis, cmap)) = self.argb {
                self.conn.create_window(32, frame, self.root, fx as i16, fy as i16, fw.max(1) as u16, fh.max(1) as u16, 0, WindowClass::INPUT_OUTPUT, vis, &aux.colormap(cmap))
            } else {
                self.conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, frame, self.root, fx as i16, fy as i16, fw.max(1) as u16, fh.max(1) as u16, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            };
            if res.is_err() { return; }
            // picom blurs only windows tagged as aerode glass (not the shadow margins of client-drawn apps)
            let _ = self.conn.change_property32(PropMode::REPLACE, frame, self.atoms._AERODE_GLASS, AtomEnum::CARDINAL, &[1]);
            let _ = self.conn.change_save_set(SetMode::INSERT, w);
            let _ = self.conn.reparent_window(w, frame, BORDER as i16, TOP as i16);
            let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().border_width(0));
            c.frame = Some(frame);
            self.frames.insert(frame, w);
            // the frame always shows the normal arrow: client windows inherit their parent's cursor,
            // so a resize cursor set on the frame would leak into the application
            if let Some(cur) = keys::cursor(&self.conn, self.screen_num, "left_ptr") {
                let _ = self.conn.change_window_attributes(frame, &ChangeWindowAttributesAux::new().cursor(cur));
            }
            const ZONE_CURSORS: [&str; 12] = ["top_side", "bottom_side", "left_side", "right_side",
                "top_left_corner", "top_left_corner", "top_right_corner", "top_right_corner",
                "bottom_left_corner", "bottom_left_corner", "bottom_right_corner", "bottom_right_corner"];
            for name in ZONE_CURSORS {
                let z = self.conn.generate_id().unwrap();
                let mut aux = CreateWindowAux::new();
                if let Some(cur) = keys::cursor(&self.conn, self.screen_num, name) { aux = aux.cursor(cur); }
                if self.conn.create_window(0, z, frame, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &aux).is_ok() { c.zones.push(z); }
            }
            let _ = self.conn.map_window(w);
        } else if kind == Kind::Dock || kind == Kind::Desktop || kind == Kind::Notification {
            c.skip_taskbar = true;
        }
        if app { self.grab_click(w); }
        // the top-level is mapped by sync_visibility below (unless minimized / on another desktop)
        c.wm_hidden = true;
        if c.frame.is_none() {
            // a frameless window that is already mapped (adopted at startup) counts as shown
            if attr.map_state == MapState::VIEWABLE { c.wm_hidden = false; }
        }
        self.set_wm_state(w, if start_minimized { 3 } else { 1 });
        let _ = self.conn.change_property32(PropMode::REPLACE, w, a._NET_WM_DESKTOP, AtomEnum::CARDINAL, &[if kind == Kind::Dock || kind == Kind::Desktop { 0xffffffff } else { c.desktop }]);
        let mut allowed = vec![a._NET_WM_ACTION_MOVE, a._NET_WM_ACTION_MINIMIZE, a._NET_WM_ACTION_FULLSCREEN, a._NET_WM_ACTION_CLOSE, a._NET_WM_ACTION_CHANGE_DESKTOP];
        if c.resizable { allowed.extend([a._NET_WM_ACTION_RESIZE, a._NET_WM_ACTION_MAXIMIZE_HORZ, a._NET_WM_ACTION_MAXIMIZE_VERT]); }
        let _ = self.conn.change_property32(PropMode::REPLACE, w, a._NET_WM_ALLOWED_ACTIONS, AtomEnum::ATOM, &allowed);
        let fs = c.fullscreen;
        c.fullscreen = false;
        let want_max = state.contains(&a._NET_WM_STATE_MAXIMIZED_VERT) && state.contains(&a._NET_WM_STATE_MAXIMIZED_HORZ);
        let focusable = !matches!(kind, Kind::Dock | Kind::Desktop | Kind::Notification | Kind::Menu);
        self.clients.insert(w, c);
        self.stack.push(w);
        self.write_extents(w);
        if fs { self.set_fullscreen(w, true); } else if want_max { self.set_maximized(w, true); }
        self.sync_visibility(w);
        // docks reserve space only once they are actually shown
        if kind == Kind::Dock { self.update_workarea(); }
        self.restack();
        self.update_client_list();
        self.layout_zones(w);
        let visible = self.clients.get(&w).map(|c| !c.wm_hidden).unwrap_or(false);
        if focusable && visible { self.focus(w); }
        self.redraw(w);
        self.update_net_state(w);
    }

    fn unmanage(&mut self, w: u32, destroyed: bool) {
        if self.drag.map(|d| d.0 == w).unwrap_or(false) { self.end_drag(); }
        let Some(c) = self.clients.remove(&w) else { return };
        if let Some(f) = c.frame {
            self.frames.remove(&f);
            if !destroyed {
                let _ = self.conn.reparent_window(w, self.root, c.geom.x as i16, c.geom.y as i16);
                let _ = self.conn.change_save_set(SetMode::DELETE, w);
                self.set_wm_state(w, 0);
            }
            let _ = self.conn.destroy_window(f);
        }
        self.stack.retain(|x| *x != w);
        if c.kind == Kind::Dock { self.update_workarea(); }
        if self.prev_active == w { self.prev_active = 0; }
        if self.active == w {
            self.active = 0;
            // focus the owner of a closed dialog, else the window that was active before this one (the
            // desktop too: closing Start or a snip must not activate and raise some other window, like
            // Win7), else the next window down the stack
            let owner_ok = self.clients.get(&c.transient_for).map(|o| self.should_show(o)).unwrap_or(false);
            let prev = self.prev_active;
            let prev_ok = self.clients.get(&prev).map(|p| self.should_show(p) && !p.wm_hidden
                && matches!(p.kind, Kind::Normal | Kind::Dialog | Kind::Utility | Kind::Desktop)).unwrap_or(false);
            if owner_ok { self.focus(c.transient_for) } else if prev_ok { self.focus(prev) } else { self.focus_fallback(w) }
        }
        self.update_client_list();
    }

    fn set_wm_state(&self, w: u32, st: u32) {
        let _ = self.conn.change_property32(PropMode::REPLACE, w, self.atoms.WM_STATE, self.atoms.WM_STATE, &[st, 0]);
    }

    fn write_extents(&mut self, w: u32) {
        let Some(c) = self.clients.get(&w) else { return };
        let (b, t) = (self.border_of(c), self.top_of(c));
        let e = [b, b, t, b];
        if c.ext == e { return; }
        self.clients.get_mut(&w).unwrap().ext = e;
        self.set_frame_extents(w, e);
    }

    fn set_frame_extents(&self, w: u32, e: [i32; 4]) {
        let v = e.map(|x| x as u32);
        let _ = self.conn.change_property32(PropMode::REPLACE, w, self.atoms._NET_FRAME_EXTENTS, AtomEnum::CARDINAL, &v);
    }

    fn update_client_list(&self) {
        let mut order: Vec<u32> = self.stack.iter().copied().filter(|w| self.clients.get(w).map(|c| c.kind != Kind::Desktop).unwrap_or(false)).collect();
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_CLIENT_LIST_STACKING, AtomEnum::WINDOW, &order);
        // _NET_CLIENT_LIST is in mapping order: keep stack order but stable enough for taskbars
        order.sort_by_key(|w| *w);
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_CLIENT_LIST, AtomEnum::WINDOW, &order);
    }

    fn update_net_state(&self, w: u32) {
        let Some(c) = self.clients.get(&w) else { return };
        let a = &self.atoms;
        let mut s = vec![];
        if c.maximized { s.push(a._NET_WM_STATE_MAXIMIZED_VERT); s.push(a._NET_WM_STATE_MAXIMIZED_HORZ); }
        if c.fullscreen { s.push(a._NET_WM_STATE_FULLSCREEN); }
        if c.minimized { s.push(a._NET_WM_STATE_HIDDEN); }
        if c.above && c.kind != Kind::Dock { s.push(a._NET_WM_STATE_ABOVE); }
        if c.skip_taskbar { s.push(a._NET_WM_STATE_SKIP_TASKBAR); }
        if c.urgent { s.push(a._NET_WM_STATE_DEMANDS_ATTENTION); }
        if self.active == w { s.push(a._NET_WM_STATE_FOCUSED); }
        let _ = self.conn.change_property32(PropMode::REPLACE, w, a._NET_WM_STATE, AtomEnum::ATOM, &s);
    }

    // ---------------------------------------------------------- geometry
    fn top_of(&self, c: &Client) -> i32 { if c.frame.is_none() || c.fullscreen { 0 } else if c.maximized { MAX_TOP } else { TOP } }
    fn border_of(&self, c: &Client) -> i32 { if c.frame.is_none() || c.fullscreen || c.maximized { 0 } else { BORDER } }

    pub fn frame_rect(&self, c: &Client) -> Rect {
        let (b, t) = (self.border_of(c), self.top_of(c));
        Rect { x: c.geom.x - b, y: c.geom.y - t, w: c.geom.w + 2 * b, h: c.geom.h + t + b }
    }

    /// Move/resize a client so that its *client* area is `g`.
    pub fn configure(&mut self, w: u32, g: Rect) {
        let Some(c) = self.clients.get_mut(&w) else { return };
        let g = Rect { w: g.w.max(c.min_size.0.max(1)), h: g.h.max(c.min_size.1.max(1)), ..g };
        let size_changed = g.w != c.geom.w || g.h != c.geom.h;
        c.geom = g;
        let c = &self.clients[&w];
        let (b, t) = (self.border_of(c), self.top_of(c));
        if let Some(f) = c.frame {
            let fr = self.frame_rect(c);
            let _ = self.conn.configure_window(f, &ConfigureWindowAux::new().x(fr.x).y(fr.y).width(fr.w as u32).height(fr.h as u32));
            let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().x(b).y(t).width(g.w as u32).height(g.h as u32));
            self.write_extents(w);
        } else {
            let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().x(g.x).y(g.y).width(g.w as u32).height(g.h as u32));
        }
        // ICCCM synthetic ConfigureNotify with root coordinates
        let ev = ConfigureNotifyEvent { response_type: CONFIGURE_NOTIFY_EVENT, sequence: 0, event: w, window: w, above_sibling: x11rb::NONE,
            x: g.x as i16, y: g.y as i16, width: g.w as u16, height: g.h as u16, border_width: 0, override_redirect: false };
        let _ = self.conn.send_event(false, w, EventMask::STRUCTURE_NOTIFY, ev);
        if size_changed {
            self.layout_zones(w);
            self.redraw(w);
        } else if !self.composited() && self.last_move_redraw.elapsed().as_millis() > 60 {
            // opaque fake glass shows the wallpaper behind: refresh it, but not on every pixel
            self.last_move_redraw = Instant::now();
            self.redraw(w);
        }
    }

    pub fn pointer(&self) -> (i32, i32) {
        self.conn.query_pointer(self.root).ok().and_then(|c| c.reply().ok()).map(|r| (r.root_x as i32, r.root_y as i32)).unwrap_or((0, 0))
    }

    pub fn pointer_monitor(&self) -> usize {
        let (x, y) = self.pointer();
        self.monitors.iter().position(|m| m.contains(x, y)).unwrap_or(0)
    }

    fn monitor_of(&self, r: &Rect) -> usize {
        let (x, y) = r.center();
        if let Some(i) = self.monitors.iter().position(|m| m.contains(x, y)) { return i; }
        // off-screen centre: the monitor with the largest overlap, else the nearest
        let overlap = |m: &Rect| { let w = (r.x + r.w).min(m.x + m.w) - r.x.max(m.x); let h = (r.y + r.h).min(m.y + m.h) - r.y.max(m.y); w.max(0) as i64 * h.max(0) as i64 };
        self.monitors.iter().enumerate().max_by_key(|(_, m)| (overlap(m), -(((m.center().0 - x) as i64).abs() + ((m.center().1 - y) as i64).abs()))).map(|(i, _)| i).unwrap_or(0)
    }

    /// Work area of a monitor: the monitor minus the docks' struts that actually touch it
    /// (EWMH struts are measured from the root edges; _NET_WM_STRUT_PARTIAL limits their span).
    pub fn workarea_for(&self, mon: usize) -> Rect {
        let m = self.monitors.get(mon).copied().unwrap_or_default();
        let (sw, sh) = self.screen_size();
        let (mut l, mut r, mut t, mut b) = (m.x, m.x + m.w, m.y, m.y + m.h);
        let overlaps = |a0: i32, a1: i32, b0: i32, b1: i32| a0 <= b1 && b0 <= a1;
        for c in self.clients.values().filter(|c| c.kind == Kind::Dock && !c.wm_hidden) {
            let mut s = self.prop32(c.win, self.atoms._NET_WM_STRUT_PARTIAL, AtomEnum::CARDINAL);
            let partial = s.len() >= 12;
            if !partial { s = self.prop32(c.win, self.atoms._NET_WM_STRUT, AtomEnum::CARDINAL); }
            if s.len() < 4 { continue; }
            let g = |i: usize, d: i32| if partial { s.get(i).map(|v| *v as i32).unwrap_or(d) } else { d };
            let (sl, sr, st, sb) = (s[0] as i32, s[1] as i32, s[2] as i32, s[3] as i32);
            let (ly0, ly1, ry0, ry1) = (g(4, 0), g(5, sh), g(6, 0), g(7, sh));
            let (tx0, tx1, bx0, bx1) = (g(8, 0), g(9, sw), g(10, 0), g(11, sw));
            if sb > 0 && overlaps(bx0, bx1, m.x, m.x + m.w - 1) && sh - sb < m.y + m.h && sh > m.y { b = b.min(sh - sb); }
            if st > 0 && overlaps(tx0, tx1, m.x, m.x + m.w - 1) && st > m.y { t = t.max(st); }
            if sl > 0 && overlaps(ly0, ly1, m.y, m.y + m.h - 1) && sl > m.x { l = l.max(sl); }
            if sr > 0 && overlaps(ry0, ry1, m.y, m.y + m.h - 1) && sw - sr < m.x + m.w { r = r.min(sw - sr); }
        }
        Rect { x: l, y: t, w: (r - l).max(1), h: (b - t).max(1) }
    }

    fn update_workarea(&mut self) {
        let was: Vec<Rect> = (0..self.monitors.len()).map(|i| self.workarea_for(i)).collect();
        self.workarea = was.first().copied().unwrap_or_default();
        // _NET_WORKAREA (one rect for the whole screen): bounding box of the monitor work areas
        let (x0, y0) = (was.iter().map(|r| r.x).min().unwrap_or(0), was.iter().map(|r| r.y).min().unwrap_or(0));
        let (x1, y1) = (was.iter().map(|r| r.x + r.w).max().unwrap_or(1), was.iter().map(|r| r.y + r.h).max().unwrap_or(1));
        let v: Vec<u32> = (0..self.num_desktops).flat_map(|_| [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]).collect();
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_WORKAREA, AtomEnum::CARDINAL, &v);
        // GTK reads exact per-monitor work areas from _GTK_WORKAREAS_D<n>; without this its menus
        // get squeezed (scroll arrows) on monitors that _NET_WORKAREA doesn't describe well
        let per: Vec<u32> = was.iter().flat_map(|r| [r.x as u32, r.y as u32, r.w as u32, r.h as u32]).collect();
        for d in 0..self.num_desktops {
            if let Ok(Ok(atom)) = self.conn.intern_atom(false, format!("_GTK_WORKAREAS_D{d}").as_bytes()).map(|c| c.reply()) {
                let _ = self.conn.change_property32(PropMode::REPLACE, self.root, atom.atom, AtomEnum::CARDINAL, &per);
            }
        }
        // keep maximised/snapped/fullscreen windows fitted to their monitor
        let ids: Vec<u32> = self.clients.keys().copied().collect();
        for w in ids {
            let Some(c) = self.clients.get(&w) else { continue };
            let mon = self.monitor_of(&self.frame_rect(c));
            if c.fullscreen { let m = self.monitors.get(mon).copied().unwrap_or_default(); self.configure(w, m); }
            else if c.maximized { let wa = self.workarea_for(mon); self.maximize_in(w, wa); }
            else if let Some(side) = c.snapped { let wa = self.workarea_for(mon); self.snap_in(w, side, wa); }
        }
    }

    /// Client geometry that makes a window's *visible* outer edge fill `outer`
    /// (framed: subtract our frame; client-side decorated: add back its invisible shadow).
    fn fit_rect(&self, c: &Client, outer: Rect, maximized: bool) -> Rect {
        if c.frame.is_some() {
            let (b, t) = if maximized { (0, MAX_TOP) } else { (BORDER, TOP) };
            Rect { x: outer.x + b, y: outer.y + t, w: outer.w - 2 * b, h: outer.h - t - b }
        } else {
            let [l, r, t, bt] = c.csd;
            Rect { x: outer.x - l, y: outer.y - t, w: outer.w + l + r, h: outer.h + t + bt }
        }
    }

    // ---------------------------------------------------------- state changes
    fn maximize_in(&mut self, w: u32, wa: Rect) {
        let Some(c) = self.clients.get_mut(&w) else { return };
        if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
        c.maximized = true;
        c.snapped = None;
        if !c.fullscreen {
            let r = { let c = &self.clients[&w]; self.fit_rect(c, wa, true) };
            self.configure(w, r);
        }
        self.update_net_state(w);
        self.layout_zones(w);
        self.redraw(w);
    }

    pub fn set_maximized(&mut self, w: u32, on: bool) {
        let Some(c) = self.clients.get(&w) else { return };
        if !is_app(c) || (on && !c.resizable) { return; }
        let wa = self.workarea_for(self.monitor_of(&self.frame_rect(c)));
        if on { self.maximize_in(w, wa); return; }
        let c = self.clients.get_mut(&w).unwrap();
        if !(c.maximized || c.snapped.is_some()) { return; }
        c.maximized = false;
        c.snapped = None;
        let r = c.restore.take().unwrap_or(Rect { x: wa.x + 60, y: wa.y + 60, w: wa.w * 2 / 3, h: wa.h * 2 / 3 });
        if !c.fullscreen { self.configure(w, r); }
        self.update_net_state(w);
        self.layout_zones(w);
        self.redraw(w);
    }

    /// Aero Snap: -1 left half, 1 right half of the given work area.
    fn snap_in(&mut self, w: u32, side: i8, wa: Rect) {
        let Some(c) = self.clients.get(&w) else { return };
        // side 2 (vertical maximise) keeps the frame's x and width; a maximised window has none of
        // its own, so it uses the size it would restore to
        let fr = if c.maximized { c.restore.map(|r| Rect { x: r.x - BORDER, w: r.w + 2 * BORDER, ..r }).unwrap_or_else(|| self.frame_rect(c)) } else { self.frame_rect(c) };
        let c = self.clients.get_mut(&w).unwrap();
        if !c.resizable || c.fullscreen { return; }
        if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
        c.maximized = false;
        c.snapped = Some(side);
        let half = wa.w / 2;
        let outer = match side {
            2 => Rect { x: fr.x, y: wa.y, w: fr.w, h: wa.h },
            s if s < 0 => Rect { x: wa.x, y: wa.y, w: half, h: wa.h },
            _ => Rect { x: wa.x + wa.w - half, y: wa.y, w: half, h: wa.h },
        };
        let r = { let c = &self.clients[&w]; self.fit_rect(c, outer, false) };
        self.configure(w, r);
        self.update_net_state(w);
        self.redraw(w);
    }

    pub fn snap(&mut self, w: u32, side: i8) {
        let Some(c) = self.clients.get(&w) else { return };
        if !is_app(c) { return; }
        let wa = self.workarea_for(self.monitor_of(&self.frame_rect(c)));
        self.snap_in(w, side, wa);
    }

    pub fn set_fullscreen(&mut self, w: u32, on: bool) {
        if self.drag.map(|d| d.0 == w).unwrap_or(false) { self.end_drag(); }
        let Some(c) = self.clients.get(&w) else { return };
        let mon = self.monitor_of(&self.frame_rect(c));
        let m = self.monitors.get(mon).copied().unwrap_or_default();
        let c = self.clients.get_mut(&w).unwrap();
        if on && !c.fullscreen {
            if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
            c.fullscreen = true;
            self.configure(w, m);
        } else if !on && c.fullscreen {
            c.fullscreen = false;
            let (maxed, snapped) = (c.maximized, c.snapped);
            let wa = self.workarea_for(mon);
            if maxed { self.clients.get_mut(&w).unwrap().maximized = false; self.maximize_in(w, wa); }
            else if let Some(sd) = snapped { self.clients.get_mut(&w).unwrap().snapped = None; self.snap_in(w, sd, wa); }
            else {
                let r = self.clients.get_mut(&w).unwrap().restore.take().unwrap_or(Rect { x: m.x + 50, y: m.y + 80, w: m.w / 2, h: m.h / 2 });
                self.configure(w, r);
            }
        }
        self.write_extents(w);
        self.restack();
        self.update_net_state(w);
        self.layout_zones(w);
        self.redraw(w);
    }

    /// Should this window be on screen right now?
    fn should_show(&self, c: &Client) -> bool {
        if c.minimized { return false; }
        if !is_app(c) { return true; }
        // Show desktop hides the windows that have taskbar buttons; Start, tray flyouts and other
        // skip-taskbar helpers stay usable on top of the desktop (Win7)
        if self.showing_desktop && !c.skip_taskbar { return false; }
        c.desktop == 0xffffffff || c.desktop == self.current_desktop
    }

    /// Map/unmap a window's top-level to match should_show (the single place that hides windows).
    fn sync_visibility(&mut self, w: u32) {
        let Some(c) = self.clients.get(&w) else { return };
        let want = self.should_show(c);
        let top = c.frame.unwrap_or(w);
        let framed = c.frame.is_some();
        let c = self.clients.get_mut(&w).unwrap();
        if want && c.wm_hidden {
            c.wm_hidden = false;
            let _ = self.conn.map_window(top);
        } else if !want && !c.wm_hidden {
            c.wm_hidden = true;
            // only frameless windows report our unmap as an UnmapNotify we must ignore
            if !framed { c.ignore_unmaps += 1; }
            let _ = self.conn.unmap_window(top);
        }
    }

    fn sync_all(&mut self) {
        let ids: Vec<u32> = self.clients.keys().copied().collect();
        for w in ids { self.sync_visibility(w); }
    }

    fn clear_focus(&mut self) {
        let old = self.active;
        self.active = 0;
        let _ = self.conn.set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME);
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW, &[0]);
        if old != 0 { self.redraw(old); self.update_net_state(old); }
    }

    /// Focus the topmost visible app window other than `except`, or nothing.
    fn focus_fallback(&mut self, except: u32) {
        let next = self.stack.iter().rev().copied().find(|x| *x != except && self.clients.get(x).map(|c| is_app(c) && self.should_show(c)).unwrap_or(false));
        match next { Some(n) => self.focus(n), None => self.clear_focus() }
    }

    pub fn minimize(&mut self, w: u32) {
        let Some(c) = self.clients.get_mut(&w) else { return };
        if c.minimized { return; }
        c.minimized = true;
        self.sync_visibility(w);
        self.set_wm_state(w, 3);
        self.update_net_state(w);
        if self.active == w { self.focus_fallback(w); }
    }

    pub fn unminimize(&mut self, w: u32) {
        let Some(c) = self.clients.get_mut(&w) else { return };
        if !c.minimized { return; }
        c.minimized = false;
        self.sync_visibility(w);
        self.set_wm_state(w, 1);
        self.update_net_state(w);
        self.redraw(w);
    }

    pub fn close(&self, w: u32) {
        let Some(c) = self.clients.get(&w) else { return };
        if c.protocols.contains(&self.atoms.WM_DELETE_WINDOW) {
            let ev = ClientMessageEvent::new(32, w, self.atoms.WM_PROTOCOLS, [self.atoms.WM_DELETE_WINDOW, CURRENT_TIME, 0, 0, 0]);
            let _ = self.conn.send_event(false, w, EventMask::NO_EVENT, ev);
        } else {
            let _ = self.conn.kill_client(w);
        }
    }

    pub fn focus(&mut self, w: u32) {
        let Some(c) = self.clients.get(&w) else { return };
        if c.minimized { self.unminimize(w); }
        // only activating a window that Show desktop hid ends it - not the desktop itself (clicking the
        // empty desktop) or Start/flyouts
        let hid_by_show_desktop = self.clients.get(&w).map(|c| is_app(c) && !c.skip_taskbar).unwrap_or(false);
        if self.showing_desktop && hid_by_show_desktop { self.showing_desktop_set(false, false); }
        let c = &self.clients[&w];
        if c.desktop != self.current_desktop && c.desktop != 0xffffffff { let d = c.desktop; self.desktop_set(d, false); }
        let old = self.active;
        // also when nothing was active (0): a stale older window must not be "the one before" the new one
        if old != w { self.prev_active = old; }
        self.active = w;
        let c = &self.clients[&w];
        if c.protocols.contains(&self.atoms.WM_TAKE_FOCUS) {
            let ev = ClientMessageEvent::new(32, w, self.atoms.WM_PROTOCOLS, [self.atoms.WM_TAKE_FOCUS, CURRENT_TIME, 0, 0, 0]);
            let _ = self.conn.send_event(false, w, EventMask::NO_EVENT, ev);
        }
        let _ = self.conn.set_input_focus(InputFocus::POINTER_ROOT, w, CURRENT_TIME);
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW, &[w]);
        if let Some(c) = self.clients.get_mut(&w) { c.urgent = false; }
        self.raise(w);
        if old != w && old != 0 { self.redraw(old); self.update_net_state(old); }
        self.redraw(w);
        self.update_net_state(w);
    }

    pub fn raise(&mut self, w: u32) {
        // raise the window plus its transients
        self.stack.retain(|x| *x != w);
        self.stack.push(w);
        let trans: Vec<u32> = self.clients.values().filter(|c| c.transient_for == w).map(|c| c.win).collect();
        for t in trans { self.stack.retain(|x| *x != t); self.stack.push(t); }
        self.restack();
    }

    /// Apply layers: desktop < normal < above < dock < fullscreen-active.
    pub fn restack(&mut self) {
        let layer = |c: &Client, active: u32| -> i32 {
            match c.kind {
                Kind::Desktop => 0,
                Kind::Dock => 3,
                Kind::Notification | Kind::Menu | Kind::Splash => 5,
                _ if c.fullscreen && c.win == active => 4,
                _ if c.above => 2,
                _ => 1,
            }
        };
        let active = self.active;
        let mut order = self.stack.clone();
        order.sort_by_key(|w| self.clients.get(w).map(|c| layer(c, active)).unwrap_or(1));
        self.stack = order.clone();
        let mut below: Option<u32> = None;
        for w in order {
            let Some(c) = self.clients.get(&w) else { continue };
            let top = c.frame.unwrap_or(w);
            let aux = match below { Some(b) => ConfigureWindowAux::new().sibling(b).stack_mode(StackMode::ABOVE), None => ConfigureWindowAux::new().stack_mode(StackMode::BELOW) };
            let _ = self.conn.configure_window(top, &aux);
            below = Some(top);
        }
        if let Some(s) = &self.switcher { let _ = self.conn.configure_window(s.win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)); }
        if let Some((p, _)) = self.snap_preview { let _ = self.conn.configure_window(p, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE)); }
        self.update_client_list();
    }

    pub fn set_showing_desktop(&mut self, on: bool) { self.showing_desktop_set(on, true) }

    fn showing_desktop_set(&mut self, on: bool, refocus: bool) {
        if on == self.showing_desktop { return; }
        self.showing_desktop = on;
        self.sync_all();
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_SHOWING_DESKTOP, AtomEnum::CARDINAL, &[on as u32]);
        // the hidden window must not stay "active" (the taskbar would minimise it on click)
        if on { self.clear_focus(); } else if refocus && self.active == 0 { self.focus_fallback(0); }
    }

    pub fn set_desktop(&mut self, d: u32) { self.desktop_set(d, true) }

    fn desktop_set(&mut self, d: u32, refocus: bool) {
        let d = d.min(self.num_desktops.saturating_sub(1));
        if d == self.current_desktop { return; }
        self.current_desktop = d;
        self.sync_all();
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL, &[d]);
        let active_visible = self.clients.get(&self.active).map(|c| !c.wm_hidden).unwrap_or(false);
        if refocus && !active_visible { self.focus_fallback(0); }
    }

    pub fn set_num_desktops(&mut self, n: u32) {
        self.num_desktops = n.clamp(1, 16);
        let last = self.num_desktops - 1;
        // windows on removed desktops move to the last remaining one (instead of vanishing)
        let moved: Vec<u32> = self.clients.values().filter(|c| c.desktop != 0xffffffff && c.desktop > last).map(|c| c.win).collect();
        for w in moved {
            self.clients.get_mut(&w).unwrap().desktop = last;
            let _ = self.conn.change_property32(PropMode::REPLACE, w, self.atoms._NET_WM_DESKTOP, AtomEnum::CARDINAL, &[last]);
        }
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_NUMBER_OF_DESKTOPS, AtomEnum::CARDINAL, &[self.num_desktops]);
        let names: String = (1..=self.num_desktops).map(|i| format!("Desktop {i}\0")).collect();
        let _ = self.conn.change_property8(PropMode::REPLACE, self.root, self.atoms._NET_DESKTOP_NAMES, self.atoms.UTF8_STRING, names.as_bytes());
        if self.current_desktop > last { self.set_desktop(last); }
        self.sync_all();
        self.update_workarea();
    }

    // ---------------------------------------------------------- drawing
    fn buttons_for(&self, c: &Client) -> Vec<(Btn, BState)> {
        let st = |b: Btn| if c.pressed == Some(b) { BState::Pressed } else if c.hover == Some(b) { BState::Hot } else { BState::Normal };
        match c.kind {
            Kind::Dialog | Kind::Utility if !c.resizable => vec![(Btn::Close, st(Btn::Close))],
            Kind::Utility => vec![(Btn::Close, st(Btn::Close))],
            _ => {
                let mut v = vec![(Btn::Min, st(Btn::Min)), (Btn::Max, if c.resizable { st(Btn::Max) } else { BState::Disabled }), (Btn::Close, st(Btn::Close))];
                if c.kind == Kind::Dialog && c.transient_for != 0 { v.remove(0); v.remove(0); }
                v
            }
        }
    }

    pub fn redraw(&mut self, w: u32) {
        let Some(c) = self.clients.get(&w) else { return };
        let Some(frame) = c.frame else { return };
        if c.minimized || c.wm_hidden { return; }
        if c.fullscreen {
            // no decorations: drop the rounded-corner shape so the whole window shows
            let _ = self.conn.shape_mask(x11rb::protocol::shape::SO::SET, x11rb::protocol::shape::SK::BOUNDING, frame, 0, 0, x11rb::NONE);
            return;
        }
        let fr = self.frame_rect(c);
        let (fw, fh) = (fr.w, fr.h);
        if fw <= 0 || fh <= 0 { return; }
        let composited = self.composited();
        let btns = self.buttons_for(c);
        let (active, maximized) = (self.active == w, c.maximized);
        let glass = |cr: &cairo::Context| self.glass.fill(cr, fr.x as f64, fr.y as f64, active, maximized, composited);
        let params = Params {
            w: fw, h: fh, title: &c.title, icon: c.icon.as_ref(), active, maximized, small: is_small(c),
            buttons: &btns, composited, glass: &glass, screen_pos: (fr.x, fr.y), screen_size: self.screen_size(),
        };
        let depth = if self.argb.is_some() { 32 } else { self.conn.setup().roots[self.screen_num].root_depth };
        let (b, t) = (self.border_of(c), self.top_of(c));
        let gc = self.conn.generate_id().unwrap();
        if self.conn.create_gc(gc, frame, &CreateGCAux::new().graphics_exposures(0)).is_err() { return; }
        // render + upload only the four border strips (the client covers the rest):
        // a maximised 4K window no longer allocates/paints a full-screen surface per redraw
        let strips = [(0, 0, fw, t), (0, fh - b, fw, b), (0, t, b, fh - t - b), (fw - b, t, b, fh - t - b)];
        for (x, y, sw, sh) in strips {
            if sw <= 0 || sh <= 0 { continue; }
            let Ok(mut surf) = cairo::ImageSurface::create(cairo::Format::ARgb32, sw, sh) else { continue };
            {
                let Ok(cr) = cairo::Context::new(&surf) else { continue };
                cr.translate(-x as f64, -y as f64);
                cr.rectangle(x as f64, y as f64, sw as f64, sh as f64);
                cr.clip();
                self.decor.render(&cr, &params);
            }
            surf.flush();
            let stride = surf.stride() as usize;
            let Ok(data) = surf.data() else { continue };
            let row_bytes = sw as usize * 4;
            let rows_per = (200_000 / row_bytes.max(1)).max(1);
            let mut row = 0usize;
            while row < sh as usize {
                let n = rows_per.min(sh as usize - row);
                let mut buf = Vec::with_capacity(row_bytes * n);
                for r in row..row + n { buf.extend_from_slice(&data[r * stride..r * stride + row_bytes]); }
                let _ = self.conn.put_image(ImageFormat::Z_PIXMAP, frame, gc, sw as u16, n as u16, x as i16, (y as usize + row) as i16, 0, depth, &buf);
                row += n;
            }
        }
        let _ = self.conn.free_gc(gc);
        self.shape_frame(frame, fw, fh, if c.maximized || is_small(c) { 0 } else { decor::RADIUS as i32 });
    }

    /// Place the resize zones along the frame edges (hidden when the window can't be resized).
    fn layout_zones(&self, w: u32) {
        let Some(c) = self.clients.get(&w) else { return };
        if c.zones.len() != 12 { return; }
        if c.maximized || c.fullscreen || !c.resizable {
            for z in &c.zones { let _ = self.conn.unmap_window(*z); }
            return;
        }
        let fr = self.frame_rect(c);
        for (z, (x, y, zw, zh)) in c.zones.iter().zip(zone_rects(fr.w, fr.h)) {
            let _ = self.conn.configure_window(*z, &ConfigureWindowAux::new().x(x).y(y).width(zw.max(1) as u32).height(zh.max(1) as u32).stack_mode(StackMode::ABOVE));
            let _ = self.conn.map_window(*z);
        }
    }

    /// Round the top corners with the Shape extension (otherwise the transparent
    /// corner pixels show up black when no compositor is running).
    fn shape_frame(&self, frame: u32, w: i32, h: i32, r: i32) {
        let mut rects = vec![];
        for y in 0..r {
            let dy = r as f64 - y as f64 - 0.5;
            let inset = (r as f64 - (r as f64 * r as f64 - dy * dy).max(0.0).sqrt()).round() as i32;
            rects.push(Rectangle { x: inset as i16, y: y as i16, width: (w - 2 * inset).max(0) as u16, height: 1 });
        }
        rects.push(Rectangle { x: 0, y: r as i16, width: w as u16, height: (h - r).max(0) as u16 });
        let _ = self.conn.shape_rectangles(x11rb::protocol::shape::SO::SET, x11rb::protocol::shape::SK::BOUNDING,
            ClipOrdering::UNSORTED, frame, 0, 0, &rects);
    }

    // ---------------------------------------------------------- hit testing
    fn hit(&self, c: &Client, x: i32, y: i32) -> Hit {
        let fr = self.frame_rect(c);
        for (b, r) in self.decor.button_rects(fr.w, c.maximized, is_small(c), &self.buttons_for(c)) {
            if x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3] { return Hit::Button(b); }
        }
        if c.maximized || c.fullscreen || !c.resizable { return Hit::Caption; }
        // exactly the zones that carry the resize cursors: a click resizes only where the cursor says so
        for ((zx, zy, zw, zh), e) in zone_rects(fr.w, fr.h).into_iter().zip(ZONE_EDGES) {
            if x >= zx && y >= zy && x < zx + zw && y < zy + zh { return Hit::Edge(e); }
        }
        Hit::Caption
    }

    /// Cursor held for the whole of a drag: the matching sizing arrow, or the plain arrow while moving
    /// (Win7). A grab without a cursor shows whatever the pointer happens to cross while the frame lags
    /// behind it – the app's cursor, the 8 px side zones (horizontal arrows), the window underneath.
    fn drag_cursor(&self, d: &Drag) -> u32 {
        let name = match *d {
            Drag::Resize { edges, .. } => match edges {
                1 => "left_side", 2 => "right_side", 4 => "top_side", 8 => "bottom_side",
                5 => "top_left_corner", 6 => "top_right_corner", 9 => "bottom_left_corner", _ => "bottom_right_corner",
            },
            _ => "left_ptr",
        };
        keys::cursor(&self.conn, self.screen_num, name).unwrap_or(x11rb::NONE)
    }

    /// Put the drag cursor on the automatic grab of a frame click.
    fn pin_drag_cursor(&self, time: u32) {
        let Some((_, d)) = self.drag else { return };
        let _ = self.conn.change_active_pointer_grab(self.drag_cursor(&d), time,
            EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::ENTER_WINDOW | EventMask::LEAVE_WINDOW);
    }

    // ---------------------------------------------------------- event loop
    pub fn run(&mut self) -> Result<()> {
        let mut pending: std::collections::VecDeque<Event> = std::collections::VecDeque::new();
        let mut panics: Vec<Instant> = vec![];
        loop {
            self.conn.flush()?;
            let ev = match pending.pop_front() {
                Some(e) => e,
                None => match self.conn.poll_for_event()? {
                    Some(e) => e,
                    None if self.glass_dirty => { self.apply_colorization(); continue; }
                    None => self.conn.wait_for_event()?,
                },
            };
            // Motion compression: high-rate mice/high-refresh screens produce far more motion
            // events than we need – only act on the newest queued position.
            let ev = if let Event::MotionNotify(mut m) = ev {
                while let Ok(Some(next)) = self.conn.poll_for_event() {
                    match next {
                        Event::MotionNotify(n) if n.event == m.event => m = n,
                        other => { pending.push_back(other); break; }
                    }
                }
                Event::MotionNotify(m)
            } else { ev };
            // a bug in one handler must not take the session down with it (the WM is the session
            // leader): log it and carry on; if it keeps happening, start afresh with the windows kept
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(ev))) {
                Ok(Err(e)) => eprintln!("aerode-wm: {e}"),
                Ok(Ok(())) => {}
                Err(_) => {
                    let now = Instant::now();
                    panics.retain(|t: &Instant| now.duration_since(*t).as_secs() < 60);
                    panics.push(now);
                    // drags and grabs may be half-done: let go of them
                    self.drag = None;
                    self.kbd_drag = None;
                    let _ = self.conn.ungrab_pointer(CURRENT_TIME);
                    let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
                    if panics.len() >= 5 { eprintln!("aerode-wm: repeated errors, restarting"); self.restart(); }
                }
            }
        }
    }

    fn handle(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::MapRequest(e) => {
                if let Some(c) = self.clients.get(&e.window) {
                    let w = c.win;
                    if c.minimized { self.unminimize(w); self.focus(w); }
                    else if c.frame.is_none() { if self.should_show(c) { self.clients.get_mut(&w).unwrap().wm_hidden = false; self.conn.map_window(w)?; } }
                } else { self.manage(e.window); }
            }
            Event::UnmapNotify(e) => {
                if let Some(c) = self.clients.get_mut(&e.window) {
                    match c.frame {
                        // the client withdrew itself (reported via SubstructureNotify on its frame)
                        Some(f) => if e.event == f { if c.ignore_unmaps > 0 { c.ignore_unmaps -= 1; } else { self.unmanage(e.window, false); } },
                        // undecorated windows: we watch them through the root
                        None => if e.event == self.root { if c.ignore_unmaps > 0 { c.ignore_unmaps -= 1; } else { self.unmanage(e.window, false); } },
                    }
                }
            }
            Event::DestroyNotify(e) => { if self.clients.contains_key(&e.window) { self.unmanage(e.window, true); } }
            Event::ConfigureRequest(e) => self.configure_request(e),
            Event::PropertyNotify(e) => self.property(e),
            Event::ClientMessage(e) => self.client_message(e),
            Event::Expose(e) if e.count == 0 => { if let Some(w) = self.frames.get(&e.window).copied() { self.redraw(w); } }
            Event::ButtonPress(e) => self.button_press(e)?,
            Event::ButtonRelease(e) => self.button_release(e),
            Event::MotionNotify(e) => self.motion(e),
            Event::LeaveNotify(e) => {
                if let Some(w) = self.frames.get(&e.event).copied() {
                    if self.clients.get(&w).map(|c| c.hover.is_some()).unwrap_or(false) && self.drag.is_none() {
                        self.clients.get_mut(&w).unwrap().hover = None;
                        self.redraw(w);
                    }
                }
            }
            Event::KeyPress(e) => self.key(e, true),
            Event::KeyRelease(e) => self.key(e, false),
            Event::MappingNotify(_) => { self.keys.grab(&self.conn, self.root)?; }
            Event::XfixesSelectionNotify(_) => {
                eprintln!("aerode-wm: compositor change, composited={} argb={}", self.composited(), self.argb.is_some());
                let ids: Vec<u32> = self.clients.keys().copied().collect();
                for w in ids { self.redraw(w); }
            }
            Event::RandrScreenChangeNotify(_) => { self.update_monitors(); self.load_wallpaper(); }
            _ => {}
        }
        Ok(())
    }

    fn configure_request(&mut self, e: ConfigureRequestEvent) {
        if let Some(c) = self.clients.get(&e.window) {
            if c.maximized || c.fullscreen || c.snapped.is_some() {
                // keep maximised/snapped windows where they are, but confirm geometry
                let g = c.geom;
                self.configure(e.window, g);
                return;
            }
            let mut g = c.geom;
            let m = e.value_mask;
            // NorthWest gravity: the requested position is the frame's, the client sits inside it
            let (ox, oy) = if c.frame.is_some() && !c.gravity_static { (self.border_of(c), self.top_of(c)) } else { (0, 0) };
            if m.contains(ConfigWindow::X) { g.x = e.x as i32 + ox; }
            if m.contains(ConfigWindow::Y) { g.y = e.y as i32 + oy; }
            if m.contains(ConfigWindow::WIDTH) { g.w = e.width as i32; }
            if m.contains(ConfigWindow::HEIGHT) { g.h = e.height as i32; }
            self.configure(e.window, g);
            if m.contains(ConfigWindow::STACK_MODE) && e.stack_mode == StackMode::ABOVE && c_kind_focusable(&self.clients[&e.window]) {
                self.raise(e.window);
            }
        } else {
            let aux = ConfigureWindowAux::from_configure_request(&e);
            let _ = self.conn.configure_window(e.window, &aux);
        }
    }

    fn property(&mut self, e: PropertyNotifyEvent) {
        let a = self.atoms;
        if e.window == self.root {
            // a dragged intensity slider writes many values: repaint once the queue is drained
            if e.atom == a._AERODE_COLORIZATION { self.glass_dirty = true; }
            return;
        }
        if !self.clients.contains_key(&e.window) { return; }
        if e.atom == a._NET_WM_NAME || e.atom == u32::from(AtomEnum::WM_NAME) {
            let t = self.read_title(e.window);
            self.clients.get_mut(&e.window).unwrap().title = t;
            self.redraw(e.window);
        } else if e.atom == a._NET_WM_ICON {
            let i = self.read_icon(e.window);
            self.clients.get_mut(&e.window).unwrap().icon = i;
            self.redraw(e.window);
        } else if e.atom == u32::from(AtomEnum::WM_HINTS) {
            let h = self.prop32(e.window, AtomEnum::WM_HINTS.into(), AtomEnum::WM_HINTS);
            let urgent = h.first().map(|f| f & 256 != 0).unwrap_or(false);
            if urgent && self.active != e.window { self.clients.get_mut(&e.window).unwrap().urgent = true; self.update_net_state(e.window); }
        } else if e.atom == a._NET_WM_STRUT_PARTIAL || e.atom == a._NET_WM_STRUT {
            self.update_workarea();
        } else if e.atom == a._GTK_FRAME_EXTENTS {
            let csd = self.read_csd(e.window);
            let c = self.clients.get_mut(&e.window).unwrap();
            if c.frame.is_none() { c.csd = csd; }
        } else if e.atom == a.WM_PROTOCOLS {
            let p = self.prop32(e.window, a.WM_PROTOCOLS, AtomEnum::ATOM);
            self.clients.get_mut(&e.window).unwrap().protocols = p;
        } else if e.atom == a._NET_WM_WINDOW_TYPE || e.atom == a.WM_TRANSIENT_FOR {
            // some apps (Electron) map a window first and set its type/owner afterwards
            let kind = self.window_kind(e.window);
            let tf = self.prop32(e.window, a.WM_TRANSIENT_FOR, AtomEnum::WINDOW).first().copied().unwrap_or(0);
            let c = self.clients.get_mut(&e.window).unwrap();
            // only re-type between ordinary window kinds; docks/desktops are set up at map time
            let ordinary = |k: Kind| matches!(k, Kind::Normal | Kind::Dialog | Kind::Utility | Kind::Splash);
            if ordinary(c.kind) && ordinary(kind) { c.kind = kind; }
            c.transient_for = tf;
            self.restack();
        }
    }

    fn client_message(&mut self, e: ClientMessageEvent) {
        let a = self.atoms;
        let d = e.data.as_data32();
        let w = e.window;
        if e.type_ == a._NET_ACTIVE_WINDOW {
            if self.clients.contains_key(&w) { self.focus(w); }
        } else if e.type_ == a._NET_CLOSE_WINDOW {
            self.close(w);
        } else if e.type_ == a.WM_CHANGE_STATE && d[0] == 3 {
            self.minimize(w);
        } else if e.type_ == a._NET_WM_STATE {
            let mut decided: Option<bool> = None;
            for prop in [d[1], d[2]] {
                if prop == 0 { continue; }
                let Some(c) = self.clients.get(&w) else { return };
                let cur = if prop == a._NET_WM_STATE_FULLSCREEN { c.fullscreen }
                    else if prop == a._NET_WM_STATE_MAXIMIZED_VERT || prop == a._NET_WM_STATE_MAXIMIZED_HORZ { c.maximized }
                    else if prop == a._NET_WM_STATE_ABOVE { c.above }
                    else if prop == a._NET_WM_STATE_HIDDEN { c.minimized }
                    else if prop == a._NET_WM_STATE_DEMANDS_ATTENTION { c.urgent }
                    else if prop == a._NET_WM_STATE_SKIP_TASKBAR { c.skip_taskbar } else { continue };
                // a toggle of MAXIMIZED_VERT+HORZ must not flip back on the second atom
                let on = match d[0] { 0 => false, 1 => true, _ => *decided.get_or_insert(!cur) };
                if prop == a._NET_WM_STATE_FULLSCREEN { self.set_fullscreen(w, on); }
                else if prop == a._NET_WM_STATE_MAXIMIZED_VERT || prop == a._NET_WM_STATE_MAXIMIZED_HORZ { if on != cur { self.set_maximized(w, on); } }
                else if prop == a._NET_WM_STATE_ABOVE { self.clients.get_mut(&w).unwrap().above = on; self.restack(); }
                else if prop == a._NET_WM_STATE_HIDDEN { if on { self.minimize(w) } else { self.focus(w) } }
                else if prop == a._NET_WM_STATE_DEMANDS_ATTENTION { self.clients.get_mut(&w).unwrap().urgent = on; }
                else if prop == a._NET_WM_STATE_SKIP_TASKBAR { self.clients.get_mut(&w).unwrap().skip_taskbar = on; }
                if d[2] == d[1] { break; }
            }
            self.update_net_state(w);
        } else if e.type_ == a._NET_SHOWING_DESKTOP {
            self.set_showing_desktop(d[0] != 0);
        } else if e.type_ == a._NET_CURRENT_DESKTOP {
            self.set_desktop(d[0]);
        } else if e.type_ == a._NET_NUMBER_OF_DESKTOPS {
            self.set_num_desktops(d[0]);
        } else if e.type_ == a._NET_WM_DESKTOP {
            if let Some(c) = self.clients.get_mut(&w) {
                c.desktop = if d[0] == 0xffffffff { d[0] } else { d[0].min(self.num_desktops - 1) };
                let v = c.desktop;
                let _ = self.conn.change_property32(PropMode::REPLACE, w, a._NET_WM_DESKTOP, AtomEnum::CARDINAL, &[v]);
                self.sync_visibility(w);
                if self.active == w && self.clients[&w].wm_hidden { self.focus_fallback(w); }
            }
        } else if cfg!(debug_assertions) && e.type_ == a._AERODE_RESTART && w == self.check_win && d[0] == 0xDEAD {
            // debug builds only: exercise the crash recovery in run()
            panic!("test panic requested through _AERODE_RESTART");
        } else if e.type_ == a._AERODE_RESTART && w == self.check_win {
            self.restart();
        } else if e.type_ == a._AERODE_SNAP_PREVIEW && w == self.check_win {
            // compositor started/stopped: repaint frames (glass vs. opaque)
            self.composited_cache.set((Instant::now() - std::time::Duration::from_secs(10), false));
            let ids: Vec<u32> = self.clients.keys().copied().collect();
            for w in ids { self.redraw(w); }
        } else if e.type_ == a._NET_REQUEST_FRAME_EXTENTS {
            let decorated = self.clients.get(&w).map(|c| c.frame.is_some()).unwrap_or_else(|| self.wants_decor(w) && matches!(self.window_kind(w), Kind::Normal | Kind::Dialog | Kind::Utility));
            self.set_frame_extents(w, if decorated { [BORDER, BORDER, TOP, BORDER] } else { [0; 4] });
        } else if e.type_ == a._NET_MOVERESIZE_WINDOW {
            if let Some(c) = self.clients.get(&w) {
                let mut g = c.geom;
                let f = d[0] >> 8;
                let grav = d[0] & 0xff;
                let static_g = grav == 10 || (grav == 0 && c.gravity_static);
                let (ox, oy) = if c.frame.is_some() && !static_g { (self.border_of(c), self.top_of(c)) } else { (0, 0) };
                if f & 1 != 0 { g.x = d[1] as i32 + ox; }
                if f & 2 != 0 { g.y = d[2] as i32 + oy; }
                if f & 4 != 0 { g.w = d[3] as i32; }
                if f & 8 != 0 { g.h = d[4] as i32; }
                self.configure(w, g);
            }
        } else if e.type_ == a._NET_WM_MOVERESIZE {
            // client-side-decorated apps asking us to move/resize them
            let Some(c) = self.clients.get(&w) else { return };
            let (px, py) = (d[0] as i32, d[1] as i32);
            let dir = d[2];
            if dir == 11 { self.end_drag(); return; }
            if dir == 9 || dir == 10 { self.keyboard_moveresize(w, dir == 9); return; }
            // the button may already be up (the app sent this late): then do nothing
            let held = self.conn.query_pointer(self.root).ok().and_then(|c| c.reply().ok()).map(|r| u16::from(r.mask) & (u16::from(KeyButMask::BUTTON1) | u16::from(KeyButMask::BUTTON2) | u16::from(KeyButMask::BUTTON3)) != 0).unwrap_or(false);
            if !held { return; }
            let drag = if dir == 8 {
                Drag::Move { cx: px, cy: py, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized }
            } else if dir <= 7 {
                let edges = [5u8, 4, 6, 2, 10, 8, 9, 1][dir as usize];
                Drag::Resize { edges, start: c.geom, px, py }
            } else { return };
            let cur = self.drag_cursor(&drag);
            let ok = self.conn.grab_pointer(false, self.root, EventMask::POINTER_MOTION | EventMask::BUTTON_RELEASE, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, cur, CURRENT_TIME)
                .ok().and_then(|c| c.reply().ok()).map(|r| r.status == GrabStatus::SUCCESS).unwrap_or(false);
            if ok { self.drag = Some((w, drag)); }
        }
    }

    fn button_press(&mut self, e: ButtonPressEvent) -> Result<()> {
        if let Some((w, _)) = self.kbd_drag { self.end_drag(); self.redraw(w); return Ok(()); }
        // click on a client window (passive grab): focus, then replay
        if let Some(c) = self.clients.get(&e.event) {
            let w = c.win;
            if self.active != w { self.focus(w); }
            // Super+drag moves windows (Plasma-style extra; Alt+click stays with the app like on Windows)
            if u16::from(e.state) & u16::from(ModMask::M4) != 0 && e.detail == 1 {
                let c = &self.clients[&w];
                self.drag = Some((w, Drag::Move { cx: e.root_x as i32, cy: e.root_y as i32, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized }));
                self.conn.allow_events(Allow::ASYNC_POINTER, e.time)?;
                let cur = self.drag.map(|(_, d)| self.drag_cursor(&d)).unwrap_or(x11rb::NONE);
                self.conn.grab_pointer(false, self.root, EventMask::POINTER_MOTION | EventMask::BUTTON_RELEASE, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, cur, e.time)?;
                return Ok(());
            }
            self.conn.allow_events(Allow::REPLAY_POINTER, e.time)?;
            return Ok(());
        }
        let Some(w) = self.frames.get(&e.event).copied() else {
            if e.event == self.root { self.switcher_cancel(); let _ = self.conn.ungrab_keyboard(e.time); }
            return Ok(());
        };
        if self.active != w { self.focus(w); } else { self.raise(w); }
        let c = &self.clients[&w];
        let (x, y) = (e.event_x as i32, e.event_y as i32);
        match (e.detail, self.hit(c, x, y)) {
            (1, Hit::Button(b)) => {
                self.clients.get_mut(&w).unwrap().pressed = Some(b);
                self.drag = Some((w, Drag::Button { btn: b }));
                self.redraw(w);
            }
            (1, Hit::Caption) => {
                let now = Instant::now();
                let (lw, lt, lx, ly) = self.last_click;
                if lw == w && now.duration_since(lt).as_millis() < 450 && (lx - x).abs() < 5 && (ly - y).abs() < 5 {
                    let m = c.maximized || c.snapped.is_some();
                    if c.resizable { self.set_maximized(w, !m); }
                    self.last_click = (0, now, 0, 0);
                    return Ok(());
                }
                self.last_click = (w, now, x, y);
                self.drag = Some((w, Drag::Move { cx: e.root_x as i32, cy: e.root_y as i32, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized || c.snapped.is_some() }));
                self.pin_drag_cursor(e.time);
            }
            (1, Hit::Edge(edges)) => {
                // double-click on the top or bottom border: maximise vertically / restore (Win7)
                let now = Instant::now();
                let (lw, lt, lx, ly) = self.last_click;
                if (edges == 4 || edges == 8) && lw == w && now.duration_since(lt).as_millis() < 450 && (lx - x).abs() < 5 && (ly - y).abs() < 5 {
                    self.last_click = (0, now, 0, 0);
                    if c.snapped == Some(2) { self.set_maximized(w, false); } else if c.resizable && is_app(c) { self.snap(w, 2); }
                    return Ok(());
                }
                self.last_click = (w, now, x, y);
                self.drag = Some((w, Drag::Resize { edges, start: c.geom, px: e.root_x as i32, py: e.root_y as i32 }));
                self.pin_drag_cursor(e.time);
            }
            (3, Hit::Caption) | (3, Hit::Button(_)) => {
                crate::util::spawn(&["aerode", "sysmenu", &w.to_string(), &e.root_x.to_string(), &e.root_y.to_string()]);
            }
            (2, Hit::Caption) => { self.stack.retain(|x| *x != w); self.stack.insert(0, w); self.restack(); }
            _ => {}
        }
        Ok(())
    }

    /// Abort any drag in progress (always removes the snap preview).
    fn end_drag(&mut self) {
        if let Some((w, Drag::Button { .. })) = self.drag {
            if let Some(c) = self.clients.get_mut(&w) { c.pressed = None; }
            self.redraw(w);
        }
        self.drag = None;
        self.hide_snap_preview();
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        if self.kbd_drag.take().is_some() { let _ = self.conn.ungrab_keyboard(CURRENT_TIME); }
    }

    /// Win7 "Move"/"Size" from the system menu (Alt+Space, M / S): the pointer jumps to the title bar or the
    /// bottom-right corner, the mouse or the arrow keys move/resize, a click or Enter commits, Esc cancels.
    fn keyboard_moveresize(&mut self, w: u32, size: bool) {
        let Some(c) = self.clients.get(&w) else { return };
        if c.maximized || c.fullscreen || (size && !c.resizable) { return; }
        self.end_drag();
        let g = self.clients[&w].geom;
        let c = &self.clients[&w];
        let (px, py) = if size { (g.x + g.w - 1, g.y + g.h - 1) }
            else if c.frame.is_some() { (g.x + g.w / 2, g.y + TOP / 2) } else { (g.x + g.w / 2, g.y + c.csd[2] + 12) };
        let _ = self.conn.warp_pointer(x11rb::NONE, self.root, 0, 0, 0, 0, px as i16, py as i16);
        let cursor = keys::cursor(&self.conn, self.screen_num, if size { "bottom_right_corner" } else { "fleur" }).unwrap_or(x11rb::NONE);
        let ok = self.conn.grab_pointer(false, self.root, EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE,
            GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, cursor, CURRENT_TIME).ok().and_then(|c| c.reply().ok()).map(|r| r.status == GrabStatus::SUCCESS).unwrap_or(false);
        if !ok { return; }
        let _ = self.conn.grab_keyboard(false, self.root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC);
        self.drag = Some((w, if size { Drag::Resize { edges: 10, start: g, px, py } } else { Drag::Move { cx: px, cy: py, ox: g.x, oy: g.y, from_max: false } }));
        self.kbd_drag = Some((w, g));
        self.focus(w);
    }

    /// Keys while a keyboard move/size runs. Returns true when consumed.
    fn kbd_drag_key(&mut self, code: u8, press: bool) -> bool {
        let Some((w, orig)) = self.kbd_drag else { return false };
        if !press { return true; }
        match self.keys.sym(code) {
            0xff0d | 0xff8d => { self.end_drag(); self.redraw(w); }               // Return / KP_Enter: done
            0xff1b => { self.end_drag(); self.configure(w, orig); self.redraw(w); } // Escape: put it back
            s @ (0xff51..=0xff54) => {
                let (dx, dy) = match s { 0xff51 => (-10, 0), 0xff52 => (0, -10), 0xff53 => (10, 0), _ => (0, 10) };
                let _ = self.conn.warp_pointer(x11rb::NONE, x11rb::NONE, 0, 0, 0, 0, dx, dy);
            }
            _ => {}
        }
        true
    }

    fn button_release(&mut self, e: ButtonReleaseEvent) {
        let Some((w, drag)) = self.drag.take() else { self.hide_snap_preview(); return };
        let _ = self.conn.ungrab_pointer(e.time);
        if matches!(drag, Drag::Resize { .. }) || matches!(drag, Drag::Move { .. }) { self.redraw(w); }
        match drag {
            Drag::Button { btn } => {
                let Some(c) = self.clients.get_mut(&w) else { return };
                c.pressed = None;
                let inside = self.frames.get(&e.event) == Some(&w) && {
                    let c = &self.clients[&w];
                    matches!(self.hit(c, e.event_x as i32, e.event_y as i32), Hit::Button(b) if b == btn)
                };
                self.redraw(w);
                if inside {
                    match btn {
                        Btn::Close => self.close(w),
                        Btn::Min => self.minimize(w),
                        Btn::Max => { let m = self.clients[&w].maximized || self.clients[&w].snapped.is_some(); self.set_maximized(w, !m); }
                        Btn::Help => {}
                    }
                }
            }
            Drag::Move { .. } => {
                if let Some((win, side)) = self.snap_preview.take() {
                    let _ = self.conn.destroy_window(win);
                    let wa = self.snap_wa.take();
                    if self.clients.contains_key(&w) {
                        match (side, wa) {
                            (0, Some(wa)) => self.maximize_in(w, wa),
                            (s, Some(wa)) => self.snap_in(w, s, wa),
                            (0, None) => self.set_maximized(w, true),
                            (s, None) => self.snap(w, s),
                        }
                    }
                }
            }
            Drag::Resize { start, .. } => {
                // the top/bottom border dragged to the screen edge: vertical maximise; restoring
                // returns to the size from before the drag
                if let Some((win, 2)) = self.snap_preview {
                    self.snap_preview = None;
                    self.snap_wa = None;
                    let _ = self.conn.destroy_window(win);
                    if let Some(c) = self.clients.get_mut(&w) { c.snapped = None; c.maximized = false; }
                    self.snap(w, 2);
                    if let Some(c) = self.clients.get_mut(&w) { if c.snapped == Some(2) { c.restore = Some(start); } }
                }
            }
            _ => {}
        }
    }

    fn motion(&mut self, e: MotionNotifyEvent) {
        let (rx, ry) = (e.root_x as i32, e.root_y as i32);
        let buttons = u16::from(e.state) & (u16::from(KeyButMask::BUTTON1) | u16::from(KeyButMask::BUTTON2) | u16::from(KeyButMask::BUTTON3));
        if self.drag.is_some() && buttons == 0 && self.kbd_drag.is_none() {
            // we missed the release (e.g. a _NET_WM_MOVERESIZE that started after the button went up)
            let w = self.drag.map(|d| d.0).unwrap_or(0);
            let snap = self.snap_preview.map(|s| s.1);
            let is_move = matches!(self.drag, Some((_, Drag::Move { .. })));
            self.end_drag();
            if is_move { if let Some(side) = snap { match side { 0 => self.set_maximized(w, true), s => self.snap(w, s) } } }
            self.snap_wa = None;
            self.redraw(w);
            return;
        }
        match self.drag {
            Some((w, Drag::Move { cx, cy, ox, oy, from_max })) => {
                let Some(c) = self.clients.get(&w) else { return };
                if (rx - cx).abs() < 4 && (ry - cy).abs() < 4 && c.geom.x == ox { return; }
                if from_max {
                    // dragging a maximised/snapped window restores it under the pointer
                    let r = c.restore.unwrap_or(Rect { w: c.geom.w * 2 / 3, h: c.geom.h * 2 / 3, ..c.geom });
                    let frac = (cx - c.geom.x) as f64 / c.geom.w.max(1) as f64;
                    let nx = rx - (r.w as f64 * frac) as i32;
                    let c = self.clients.get_mut(&w).unwrap();
                    c.maximized = false; c.snapped = None; c.restore = None;
                    self.configure(w, Rect { x: nx, y: ry + 10, ..r });
                    self.update_net_state(w);
                    self.layout_zones(w);
                    let g = self.clients[&w].geom;
                    self.drag = Some((w, Drag::Move { cx: rx, cy: ry, ox: g.x, oy: g.y, from_max: false }));
                    return;
                }
                let min_y = self.monitors.iter().map(|m| m.y).min().unwrap_or(0);
                let floor = if c.frame.is_some() { min_y + TOP / 2 } else { min_y - c.csd[2] };
                let g = Rect { x: ox + rx - cx, y: (oy + ry - cy).max(floor), ..c.geom };
                self.configure(w, g);
                if self.kbd_drag.is_none() && self.shake.step(w, rx, ry) { self.aero_shake(w); }
                // Aero Snap preview when touching a screen edge
                let m = self.monitors.get(self.monitors.iter().position(|m| m.contains(rx, ry)).unwrap_or(0)).copied().unwrap_or_default();
                let side = if self.kbd_drag.is_some() { None } else if rx <= m.x + 1 { Some(-1) } else if rx >= m.x + m.w - 2 { Some(1) } else if ry <= m.y + 1 { Some(0) } else { None };
                if side != self.snap_preview.map(|s| s.1) {
                    self.hide_snap_preview();
                    if let Some(s) = side { if self.clients[&w].resizable && is_app(&self.clients[&w]) { self.show_snap_preview(w, s); } }
                }
            }
            Some((w, Drag::Resize { edges, start, px, py })) => {
                let (dx, dy) = (rx - px, ry - py);
                let mut g = start;
                if edges & 1 != 0 { g.x = start.x + dx; g.w = start.w - dx; }
                if edges & 2 != 0 { g.w = start.w + dx; }
                if edges & 4 != 0 { g.y = start.y + dy; g.h = start.h - dy; }
                if edges & 8 != 0 { g.h = start.h + dy; }
                let min = self.clients.get(&w).map(|c| c.min_size).unwrap_or((80, 30));
                if g.w < min.0.max(100) { if edges & 1 != 0 { g.x = start.x + start.w - min.0.max(100); } g.w = min.0.max(100); }
                if g.h < min.1.max(30) { if edges & 4 != 0 { g.y = start.y + start.h - min.1.max(30); } g.h = min.1.max(30); }
                if let Some(c) = self.clients.get_mut(&w) { c.snapped = None; c.maximized = false; }
                self.configure(w, g);
                // top or bottom border at the screen edge: preview of the vertical maximise
                let m = self.monitors.iter().copied().find(|m| m.contains(rx, ry)).unwrap_or_default();
                let vert = self.kbd_drag.is_none() && ((edges == 4 && ry <= m.y + 1) || (edges == 8 && ry >= m.y + m.h - 2));
                if vert != (self.snap_preview.map(|s| s.1) == Some(2)) {
                    self.hide_snap_preview();
                    if vert && self.clients.get(&w).map(|c| c.resizable && is_app(c)).unwrap_or(false) { self.show_snap_preview(w, 2); }
                }
            }
            Some((_, Drag::Button { .. })) => {}
            None => {
                // hover highlight on caption buttons + resize cursors
                let Some(w) = self.frames.get(&e.event).copied() else { return };
                let c = &self.clients[&w];
                let hit = self.hit(c, e.event_x as i32, e.event_y as i32);
                let hb = if let Hit::Button(b) = hit { Some(b) } else { None };
                if hb != c.hover { self.clients.get_mut(&w).unwrap().hover = hb; self.redraw(w); }
                let _ = hit;
            }
        }
    }

    /// Aero Shake (and Win+Home): minimise every other window; doing it again restores the
    /// windows the last shake minimised that are still minimised.
    fn aero_shake(&mut self, keep: u32) {
        let prev = std::mem::take(&mut self.shaken);
        let back: Vec<u32> = prev.into_iter().filter(|x| *x != keep && self.clients.get(x).map(|c| c.minimized).unwrap_or(false)).collect();
        if !back.is_empty() {
            // bring them back bottom-most first so the stacking order is kept
            let mut back = back;
            back.sort_by_key(|x| std::cmp::Reverse(self.stack.iter().position(|s| s == x).unwrap_or(0)));
            for x in back { self.unminimize(x); }
            self.focus(keep);
            return;
        }
        let ids: Vec<u32> = self.clients.values()
            .filter(|c| c.win != keep && is_app(c) && !c.minimized && !c.wm_hidden && !c.skip_taskbar && c.transient_for != keep)
            .map(|c| c.win).collect();
        for x in &ids { self.minimize(*x); }
        self.shaken = ids;
        self.focus(keep);
    }

    /// Win+Shift+Left/Right: move a window to the neighbouring monitor (wrapping around), keeping
    /// its maximised/snapped state and its relative position.
    fn move_to_monitor(&mut self, w: u32, dir: i32) {
        if self.monitors.len() < 2 { return; }
        let Some(c) = self.clients.get(&w) else { return };
        if !is_app(c) { return; }
        let cur = self.monitor_of(&self.frame_rect(c));
        let mut order: Vec<usize> = (0..self.monitors.len()).collect();
        order.sort_by_key(|&i| (self.monitors[i].x, self.monitors[i].y));
        let pos = order.iter().position(|&i| i == cur).unwrap_or(0) as i32;
        let to = order[(pos + dir).rem_euclid(order.len() as i32) as usize];
        let (a, b) = (self.workarea_for(cur), self.workarea_for(to));
        let shift = |r: Rect| -> Rect {
            let (w, h) = (r.w.min(b.w), r.h.min(b.h));
            let fx = (r.x - a.x) as f64 / (a.w - r.w).max(1) as f64;
            let fy = (r.y - a.y) as f64 / (a.h - r.h).max(1) as f64;
            Rect { x: b.x + ((b.w - w).max(0) as f64 * fx.clamp(0.0, 1.0)) as i32, y: b.y + ((b.h - h).max(0) as f64 * fy.clamp(0.0, 1.0)) as i32, w, h }
        };
        let (maxed, snapped, full, geom) = (c.maximized, c.snapped, c.fullscreen, c.geom);
        if let Some(r) = c.restore { self.clients.get_mut(&w).unwrap().restore = Some(shift(r)); }
        if full {
            let m = self.monitors[to];
            self.configure(w, m);
        } else if maxed {
            self.maximize_in(w, b);
        } else if let Some(side) = snapped {
            if side == 2 { self.configure(w, shift(geom)); }
            self.snap_in(w, side, b);
        } else {
            self.configure(w, shift(geom));
        }
        self.update_net_state(w);
        self.layout_zones(w);
        self.redraw(w);
    }

    fn show_snap_preview(&mut self, w: u32, side: i8) {
        let Some(c) = self.clients.get(&w) else { return };
        let wa = self.workarea_for(self.monitor_of(&Rect { x: self.pointer().0, y: self.pointer().1, w: 1, h: 1 }));
        let _ = c;
        let r = match side {
            -1 => Rect { w: wa.w / 2, ..wa },
            1 => Rect { x: wa.x + wa.w - wa.w / 2, w: wa.w / 2, ..wa },
            2 => { let fr = self.frame_rect(c); Rect { x: fr.x, w: fr.w, ..wa } }
            _ => wa,
        };
        self.snap_wa = Some(wa);
        let win = self.conn.generate_id().unwrap();
        let composited = self.composited();
        let (depth, vis, cmap) = match (composited, self.argb) { (true, Some((v, c))) => (32, v, c), _ => (x11rb::COPY_DEPTH_FROM_PARENT, 0, 0) };
        let mut aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).border_pixel(0);
        if cmap != 0 { aux = aux.colormap(cmap); }
        let r2 = Rect { x: r.x + 8, y: r.y + 8, w: r.w - 16, h: r.h - 16 };
        if self.conn.create_window(depth, win, self.root, r2.x as i16, r2.y as i16, r2.w as u16, r2.h as u16, 0, WindowClass::INPUT_OUTPUT, vis, &aux).is_err() { return; }
        let _ = self.conn.change_property32(PropMode::REPLACE, win, self.atoms._AERODE_GLASS, AtomEnum::CARDINAL, &[1]);
        let _ = self.conn.map_window(win);
        // paint: glass rectangle with white edge (Win7 "peek" outline)
        if let Ok(mut s) = cairo::ImageSurface::create(cairo::Format::ARgb32, r2.w, r2.h) {
            {
                let cr = cairo::Context::new(&s).unwrap();
                crate::glass::rounded(&cr, 0.5, 0.5, r2.w as f64 - 1.0, r2.h as f64 - 1.0, 6.0);
                cr.save().ok();
                cr.clip_preserve();
                self.glass.fill(&cr, r2.x as f64, r2.y as f64, true, false, composited);
                cr.restore().ok();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.8);
                cr.set_line_width(2.0);
                cr.stroke().ok();
            }
            s.flush();
            let stride = s.stride() as usize;
            let data = s.data().unwrap().to_vec();
            let gc = self.conn.generate_id().unwrap();
            let _ = self.conn.create_gc(gc, win, &CreateGCAux::new());
            let depth = if depth == 32 { 32 } else { self.conn.setup().roots[self.screen_num].root_depth };
            let rows = (200_000 / stride).max(1);
            for y in (0..r2.h as usize).step_by(rows) {
                let n = rows.min(r2.h as usize - y);
                let _ = self.conn.put_image(ImageFormat::Z_PIXMAP, win, gc, r2.w as u16, n as u16, 0, y as i16, 0, depth, &data[y * stride..(y + n) * stride]);
            }
            let _ = self.conn.free_gc(gc);
        }
        // keep the dragged window above the preview
        if let Some(f) = self.clients.get(&w).and_then(|c| c.frame) {
            let _ = self.conn.configure_window(f, &ConfigureWindowAux::new().sibling(win).stack_mode(StackMode::ABOVE));
        }
        self.snap_preview = Some((win, side));
    }

    fn hide_snap_preview(&mut self) {
        self.snap_wa = None;
        if let Some((win, _)) = self.snap_preview.take() { let _ = self.conn.destroy_window(win); }
    }

    fn switcher_cancel(&mut self) {
        if let Some(s) = self.switcher.take() { s.destroy(&self.conn); }
    }

    // ---------------------------------------------------------- keys
    fn key(&mut self, e: KeyPressEvent, press: bool) {
        if self.kbd_drag_key(e.detail, press) { return; }
        let switching = self.switcher.is_some();
        let Some(action) = self.keys.lookup(e.detail, u16::from(e.state), press, e.time, switching, &self.conn) else { return };
        let act = self.active;
        use keys::Action::*;
        match action {
            Spawn(cmd) => { crate::util::spawn(&["sh", "-c", &cmd]); }
            Close => { if act != 0 { self.close(act) } }
            Maximize => { if act != 0 { let m = self.clients.get(&act).map(|c| c.maximized).unwrap_or(false); if !m { self.set_maximized(act, true) } } }
            RestoreOrMinimize => {
                if let Some(c) = self.clients.get(&act) {
                    if c.maximized || c.snapped.is_some() { self.set_maximized(act, false) } else { self.minimize(act) }
                }
            }
            SnapLeft | SnapRight => {
                let side = if matches!(action, SnapLeft) { -1 } else { 1 };
                if let Some(c) = self.clients.get(&act) {
                    // pressing again from the opposite snap restores (Win7 behaviour)
                    if c.snapped == Some(-side) || (c.maximized && false) { self.set_maximized(act, false) } else { self.snap(act, side) }
                }
            }
            ShowDesktop => { let s = !self.showing_desktop; self.set_showing_desktop(s); }
            // Win+Home works like Aero Shake: minimise the others, press again to bring them back
            MinimizeOthers => { if act != 0 { self.aero_shake(act); } }
            MaximizeVert => {
                if let Some(c) = self.clients.get(&act) {
                    if is_app(c) && c.resizable && c.snapped != Some(2) { self.snap(act, 2); }
                }
            }
            MonitorLeft | MonitorRight => { if act != 0 { self.move_to_monitor(act, if matches!(action, MonitorLeft) { -1 } else { 1 }); } }
            SwitchNext | SwitchPrev => {
                if self.switcher.is_none() {
                    let wins = self.switch_order();
                    if wins.len() < 1 { return; }
                    let held = e.state;
                    self.switcher = switcher::Switcher::new(self, wins, u16::from(held));
                    let _ = self.conn.grab_keyboard(false, self.root, e.time, GrabMode::ASYNC, GrabMode::ASYNC);
                }
                let fwd = matches!(action, SwitchNext);
                if let Some(mut s) = self.switcher.take() { s.step(fwd); s.draw(self); self.switcher = Some(s); }
                // a very quick Alt+Tab tap can release Alt before our grab took effect
                let mods = self.conn.query_pointer(self.root).ok().and_then(|c| c.reply().ok()).map(|r| u16::from(r.mask)).unwrap_or(0xffff);
                if mods & (u16::from(KeyButMask::MOD1) | u16::from(KeyButMask::MOD4)) == 0 { self.switch_commit(e.time); }
            }
            SwitchCommit => self.switch_commit(e.time),
            SwitchCancel => { self.switcher_cancel(); let _ = self.conn.ungrab_keyboard(e.time); }
            SysMenu => {
                if let Some(c) = self.clients.get(&act) {
                    let (x, y) = (c.geom.x + if c.frame.is_some() { BORDER } else { c.csd[0] }, c.geom.y + if c.frame.is_some() { TOP } else { c.csd[2] + 30 });
                    crate::util::spawn(&["aerode", "sysmenu", &act.to_string(), &x.to_string(), &y.to_string()]);
                }
            }
            DesktopLeft => { let d = self.current_desktop.saturating_sub(1); self.set_desktop(d); }
            DesktopRight => { let d = (self.current_desktop + 1).min(self.num_desktops - 1); self.set_desktop(d); }
            DesktopAdd => { let n = self.num_desktops + 1; self.set_num_desktops(n); self.set_desktop(n - 1); }
            DesktopRemove => { let n = self.num_desktops.saturating_sub(1).max(1); self.set_num_desktops(n); }
        }
    }

    /// Hand every window back to the root (keeping it where it is) and re-exec ourselves.
    pub fn restart(&mut self) {
        self.end_drag();
        self.switcher_cancel();
        let ids: Vec<u32> = self.stack.clone();
        for w in ids {
            let Some(c) = self.clients.get(&w) else { continue };
            // minimised windows stay unmapped + WM_STATE Iconic: the new instance adopts them minimised;
            // windows hidden for other reasons (other desktop, show desktop) are mapped again
            if let Some(f) = c.frame {
                let _ = self.conn.reparent_window(w, self.root, c.geom.x as i16, c.geom.y as i16);
                let _ = self.conn.change_save_set(SetMode::DELETE, w);
                if c.minimized { let _ = self.conn.unmap_window(w); } else { let _ = self.conn.map_window(w); }
                let _ = self.conn.destroy_window(f);
            } else if !c.minimized && c.wm_hidden { let _ = self.conn.map_window(w); }
        }
        let _ = self.conn.flush();
        // after ./install.sh replaced the binary, /proc/self/exe reads "/usr/bin/aerode-wm (deleted)":
        // exec'ing that fails and the old code would keep running with every frame already gone
        let exe = std::env::current_exe().ok()
            .map(|p| std::path::PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)")))
            .filter(|p| p.exists())
            .unwrap_or_else(|| "aerode-wm".into());
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(exe).args(std::env::args().skip(1)).exec();
        eprintln!("aerode-wm: restart failed: {err}");
        // keep running: adopt the windows we just released again
        self.clients.clear();
        self.frames.clear();
        self.stack.clear();
        self.active = 0;
        let _ = self.manage_existing();
    }

    fn switch_commit(&mut self, time: u32) {
        let _ = self.conn.ungrab_keyboard(time);
        if let Some(s) = self.switcher.take() {
            let target = s.selected();
            s.destroy(&self.conn);
            if let Some(w) = target { self.focus(w); }
        }
    }

    fn switch_order(&self) -> Vec<u32> {
        // most recently used first = top of stack first (minimised windows too, like Win7)
        // the windows that have a taskbar button (like Win7): not docks/desktop/menus/notifications, not
        // skip-taskbar, not owned tool windows. Electron apps sometimes type their main window oddly.
        let listed = |c: &Client| !c.skip_taskbar && match c.kind {
            Kind::Normal | Kind::Dialog | Kind::Splash => true,
            Kind::Utility => c.transient_for == 0,
            _ => false,
        };
        let mut v: Vec<u32> = self.stack.iter().rev().copied().filter(|w| self.clients.get(w).map(listed).unwrap_or(false)).collect();
        if let Some(i) = v.iter().position(|w| *w == self.active) { let a = v.remove(i); v.insert(0, a); }
        v
    }
}

/// Watch the _NET_CM_S0 owner on a separate connection and poke the WM when it changes.
fn spawn_compositor_watch(check_win: u32) {
    std::thread::spawn(move || {
        let Ok((conn, _)) = x11rb::connect(None) else { return };
        let Ok(Ok(a)) = Atoms::new(&conn).map(|c| c.reply()) else { return };
        let mut last = u32::MAX;
        loop {
            let owner = conn.get_selection_owner(a._NET_CM_S0).ok().and_then(|c| c.reply().ok()).map(|r| r.owner).unwrap_or(0);
            if owner != last && last != u32::MAX {
                let ev = ClientMessageEvent::new(32, check_win, a._AERODE_SNAP_PREVIEW, [0, 0, 0, 0, 0]);
                let _ = conn.send_event(false, check_win, EventMask::NO_EVENT, ev);
                let _ = conn.flush();
            }
            last = owner;
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

fn c_kind_focusable(c: &Client) -> bool { !matches!(c.kind, Kind::Dock | Kind::Desktop) }

/// The 12 resize zones of a frame (frame coordinates), in the order of `ZONE_CURSORS`:
/// four sides, then two arms per corner.
fn zone_rects(fw: i32, fh: i32) -> [(i32, i32, i32, i32); 12] {
    let (b, k, t) = (BORDER, 16, 4); // border, corner length, top grab height (caption above)
    [
        (k, 0, fw - 2 * k, t), (k, fh - b, fw - 2 * k, b), (0, k, b, fh - 2 * k), (fw - b, k, b, fh - 2 * k),
        (0, 0, k, t), (0, 0, b, k), (fw - k, 0, k, t), (fw - b, 0, b, k),
        (0, fh - b, k, b), (0, fh - k, b, k), (fw - k, fh - b, k, b), (fw - b, fh - k, b, k),
    ]
}

/// Resize edges (1 left, 2 right, 4 top, 8 bottom) of each zone in `zone_rects`.
const ZONE_EDGES: [u8; 12] = [4, 8, 1, 2, 5, 5, 6, 6, 9, 9, 10, 10];

enum Hit { Button(Btn), Caption, Edge(u8) }

/// Tool windows get Win7's small frame (SMALL* DWM parts, square corners, small close button).
fn is_small(c: &Client) -> bool { c.kind == Kind::Utility }
