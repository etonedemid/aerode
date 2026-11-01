//! aero7-wm: a small reparenting X11 window manager drawing Windows 7 frames.
pub mod decor;
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
        _NET_WM_USER_TIME, _NET_WM_WINDOW_OPACITY, _GTK_FRAME_EXTENTS, _NET_CM_S0, _AERO7_SNAP_PREVIEW,
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
    pub snapped: Option<i8>, // -1 left, 1 right
    pub mapped_once: bool,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    Move { cx: i32, cy: i32, ox: i32, oy: i32, from_max: bool },
    Resize { edges: u8, start: Rect, px: i32, py: i32 },
    Button { btn: Btn },
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
    pub wallpaper: Option<cairo::ImageSurface>,
    pub color: [f64; 4],
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
        let cfg = crate::config::get();
        let mut wm = Wm {
            conn, root, screen_num, atoms, clients: HashMap::new(), frames: HashMap::new(), stack: vec![],
            active: 0, monitors: vec![], workarea: Rect::default(), decor: Decor::load(), argb, check_win,
            showing_desktop: false, current_desktop: 0, num_desktops: 1, drag: None,
            last_click: (0, Instant::now(), 0, 0), keys: keys::Keys::default(), switcher: None, snap_preview: None,
            wallpaper: None, color: [0.45, 0.72, 0.99, cfg.tint[3].clamp(0.2, 0.8)],
        };
        wm.color = colorization(&cfg.tint);
        wm.load_wallpaper();
        wm.update_monitors();
        wm.set_supported()?;
        wm.keys.grab(&wm.conn, wm.root)?;
        wm.manage_existing()?;
        wm.conn.flush()?;
        spawn_compositor_watch(wm.check_win);
        Ok(wm)
    }

    fn load_wallpaper(&mut self) {
        // blurred wallpaper used as fake glass when no compositor is running
        let path = crate::glass::wallpaper_path();
        self.wallpaper = path.and_then(|p| image::open(p).ok()).map(|img| {
            let s = self.screen_size();
            let img = img.resize_to_fill((s.0 as u32 / 8).max(1), (s.1 as u32 / 8).max(1), image::imageops::FilterType::Triangle).blur(3.0)
                .resize_exact(s.0 as u32, s.1 as u32, image::imageops::FilterType::Triangle);
            decor::rgba_surface(&img.to_rgba8())
        });
    }

    pub fn screen_size(&self) -> (i32, i32) {
        let s = &self.conn.setup().roots[self.screen_num];
        (s.width_in_pixels as i32, s.height_in_pixels as i32)
    }

    pub fn update_monitors(&mut self) {
        let mut mons = vec![];
        if let Ok(r) = self.conn.randr_get_monitors(self.root, true).map(|c| c.reply()) {
            if let Ok(r) = r {
                for m in r.monitors {
                    mons.push(Rect { x: m.x as i32, y: m.y as i32, w: m.width as i32, h: m.height as i32 });
                }
            }
        }
        if mons.is_empty() {
            let (w, h) = self.screen_size();
            mons.push(Rect { x: 0, y: 0, w, h });
        }
        self.monitors = mons;
        self.update_workarea();
    }

    fn composited(&self) -> bool {
        self.argb.is_some() && self.conn.get_selection_owner(self.atoms._NET_CM_S0).ok()
            .and_then(|c| c.reply().ok()).map(|r| r.owner != 0).unwrap_or(false)
    }

    fn set_supported(&self) -> Result<()> {
        let a = &self.atoms;
        let c = &self.conn;
        c.change_property32(PropMode::REPLACE, self.check_win, a._NET_SUPPORTING_WM_CHECK, AtomEnum::WINDOW, &[self.check_win])?;
        c.change_property8(PropMode::REPLACE, self.check_win, a._NET_WM_NAME, a.UTF8_STRING, b"Aero7")?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_SUPPORTING_WM_CHECK, AtomEnum::WINDOW, &[self.check_win])?;
        let sup = [a._NET_SUPPORTED, a._NET_SUPPORTING_WM_CHECK, a._NET_WM_NAME, a._NET_CLIENT_LIST, a._NET_CLIENT_LIST_STACKING,
            a._NET_ACTIVE_WINDOW, a._NET_CLOSE_WINDOW, a._NET_WORKAREA, a._NET_NUMBER_OF_DESKTOPS, a._NET_CURRENT_DESKTOP,
            a._NET_DESKTOP_GEOMETRY, a._NET_SHOWING_DESKTOP, a._NET_WM_DESKTOP, a._NET_WM_STATE, a._NET_WM_STATE_MAXIMIZED_VERT,
            a._NET_WM_STATE_MAXIMIZED_HORZ, a._NET_WM_STATE_FULLSCREEN, a._NET_WM_STATE_HIDDEN, a._NET_WM_STATE_ABOVE,
            a._NET_WM_STATE_SKIP_TASKBAR, a._NET_WM_STATE_DEMANDS_ATTENTION, a._NET_WM_STATE_FOCUSED, a._NET_WM_WINDOW_TYPE,
            a._NET_WM_STRUT, a._NET_WM_STRUT_PARTIAL, a._NET_FRAME_EXTENTS, a._NET_REQUEST_FRAME_EXTENTS, a._NET_WM_MOVERESIZE,
            a._NET_MOVERESIZE_WINDOW, a._NET_WM_ALLOWED_ACTIONS, a._GTK_FRAME_EXTENTS];
        c.change_property32(PropMode::REPLACE, self.root, a._NET_SUPPORTED, AtomEnum::ATOM, &sup)?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_NUMBER_OF_DESKTOPS, AtomEnum::CARDINAL, &[self.num_desktops])?;
        c.change_property32(PropMode::REPLACE, self.root, a._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL, &[0])?;
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
            if attr.override_redirect || attr.map_state != MapState::VIEWABLE { continue; }
            self.manage(w);
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

    fn read_icon(&self, w: u32) -> Option<cairo::ImageSurface> {
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
    pub fn manage(&mut self, w: u32) {
        if self.clients.contains_key(&w) || self.frames.contains_key(&w) { return; }
        let Ok(Ok(g)) = self.conn.get_geometry(w).map(|c| c.reply()) else { return };
        let kind = self.window_kind(w);
        let a = &self.atoms;
        let state = self.prop32(w, a._NET_WM_STATE, AtomEnum::ATOM);
        let transient_for = self.prop32(w, a.WM_TRANSIENT_FOR, AtomEnum::WINDOW).first().copied().unwrap_or(0);
        let protocols = self.prop32(w, a.WM_PROTOCOLS, AtomEnum::ATOM);
        // WM_NORMAL_HINTS: min size / fixed size
        let nh = self.prop32(w, AtomEnum::WM_NORMAL_HINTS.into(), AtomEnum::WM_SIZE_HINTS);
        let (mut min_size, mut resizable) = ((80, 30), true);
        if nh.len() >= 9 {
            if nh[0] & 16 != 0 { min_size = (nh[5] as i32, nh[6] as i32); }
            if nh[0] & 32 != 0 && nh.len() >= 9 && nh[0] & 16 != 0 && nh[7] == nh[5] && nh[8] == nh[6] { resizable = false; }
        }
        let decorated = matches!(kind, Kind::Normal | Kind::Dialog | Kind::Utility) && self.wants_decor(w);
        let mut geom = Rect { x: g.x as i32, y: g.y as i32, w: g.width as i32, h: g.height as i32 };
        let mut c = Client {
            win: w, frame: None, kind, title: self.read_title(w), icon: self.read_icon(w), geom, restore: None,
            maximized: false, fullscreen: state.contains(&a._NET_WM_STATE_FULLSCREEN), minimized: false,
            above: state.contains(&a._NET_WM_STATE_ABOVE) || kind == Kind::Dock, skip_taskbar: state.contains(&a._NET_WM_STATE_SKIP_TASKBAR),
            urgent: false, transient_for, protocols, min_size, resizable, desktop: self.current_desktop,
            hover: None, pressed: None, ignore_unmaps: 0, snapped: None, mapped_once: false,
        };
        let _ = self.conn.change_window_attributes(w, &ChangeWindowAttributesAux::new()
            .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE));
        if decorated {
            // place new windows sensibly (centred on the pointer's monitor) unless the app chose a position
            let wa = self.workarea_for(self.pointer_monitor());
            let user_pos = nh.first().map(|f| f & 1 != 0 || f & 4 != 0).unwrap_or(false);
            if (!user_pos && geom.x == 0 && geom.y == 0) || !self.monitors.iter().any(|m| m.contains(geom.x, geom.y)) {
                if transient_for != 0 {
                    if let Some(p) = self.clients.get(&transient_for) {
                        let (px, py) = p.geom.center();
                        geom.x = px - geom.w / 2;
                        geom.y = py - geom.h / 2;
                    }
                } else {
                    geom.x = wa.x + (wa.w - geom.w - 2 * BORDER) / 2 + BORDER;
                    geom.y = wa.y + (wa.h - geom.h - TOP - BORDER) / 2 + TOP;
                    // cascade if something already sits there
                    while self.clients.values().any(|o| o.frame.is_some() && (o.geom.x - geom.x).abs() < 8 && (o.geom.y - geom.y).abs() < 8) {
                        geom.x += 26; geom.y += 26;
                    }
                }
                geom.x = geom.x.max(wa.x + BORDER);
                geom.y = geom.y.max(wa.y + TOP);
            }
            c.geom = geom;
            let frame = self.conn.generate_id().unwrap();
            let (fx, fy, fw, fh) = (geom.x - BORDER, geom.y - TOP, geom.w + 2 * BORDER, geom.h + TOP + BORDER);
            let aux = CreateWindowAux::new().event_mask(frame_events()).background_pixel(0).border_pixel(0).bit_gravity(Gravity::NORTH_WEST);
            let res = if let Some((vis, cmap)) = self.argb {
                self.conn.create_window(32, frame, self.root, fx as i16, fy as i16, fw as u16, fh as u16, 0, WindowClass::INPUT_OUTPUT, vis, &aux.colormap(cmap))
            } else {
                self.conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, frame, self.root, fx as i16, fy as i16, fw as u16, fh as u16, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            };
            if res.is_err() { return; }
            let _ = self.conn.change_save_set(SetMode::INSERT, w);
            let _ = self.conn.reparent_window(w, frame, BORDER as i16, TOP as i16);
            let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().border_width(0));
            // click-to-focus: passive grab on the client
            let _ = self.conn.grab_button(true, w, EventMask::BUTTON_PRESS, GrabMode::SYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, ButtonIndex::ANY, ModMask::ANY);
            c.ignore_unmaps += 1;
            c.frame = Some(frame);
            self.frames.insert(frame, w);
            self.set_frame_extents(w, [BORDER, BORDER, TOP, BORDER]);
            let _ = self.conn.map_window(w);
            let _ = self.conn.map_window(frame);
        } else {
            if kind == Kind::Dock || kind == Kind::Desktop || kind == Kind::Notification { c.skip_taskbar = true; }
            self.set_frame_extents(w, [0; 4]);
            let _ = self.conn.map_window(w);
        }
        self.set_wm_state(w, 1);
        let _ = self.conn.change_property32(PropMode::REPLACE, w, a._NET_WM_DESKTOP, AtomEnum::CARDINAL, &[if kind == Kind::Dock || kind == Kind::Desktop { 0xffffffff } else { c.desktop }]);
        let allowed = [a._NET_WM_ACTION_MOVE, a._NET_WM_ACTION_RESIZE, a._NET_WM_ACTION_MINIMIZE, a._NET_WM_ACTION_MAXIMIZE_HORZ,
            a._NET_WM_ACTION_MAXIMIZE_VERT, a._NET_WM_ACTION_FULLSCREEN, a._NET_WM_ACTION_CLOSE, a._NET_WM_ACTION_CHANGE_DESKTOP];
        let _ = self.conn.change_property32(PropMode::REPLACE, w, a._NET_WM_ALLOWED_ACTIONS, AtomEnum::ATOM, &allowed);
        let fs = c.fullscreen;
        let want_max = state.contains(&a._NET_WM_STATE_MAXIMIZED_VERT) && state.contains(&a._NET_WM_STATE_MAXIMIZED_HORZ);
        let focusable = !matches!(kind, Kind::Dock | Kind::Desktop | Kind::Notification | Kind::Menu);
        self.clients.insert(w, c);
        self.stack.push(w);
        if kind == Kind::Dock { self.update_workarea(); }
        if fs { self.set_fullscreen(w, true); } else if want_max { self.set_maximized(w, true); }
        self.restack();
        self.update_client_list();
        if focusable { self.focus(w); }
        self.redraw(w);
    }

    fn unmanage(&mut self, w: u32, destroyed: bool) {
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
        if self.active == w {
            self.active = 0;
            // focus the parent of a closed dialog, else the next window down the stack
            let next = if c.transient_for != 0 && self.clients.contains_key(&c.transient_for) { Some(c.transient_for) }
                else { self.stack.iter().rev().copied().find(|x| self.clients.get(x).map(|c| c.frame.is_some() && !c.minimized && c.desktop == self.current_desktop).unwrap_or(false)) };
            if let Some(n) = next { self.focus(n) } else {
                let _ = self.conn.set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME);
                let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW, &[0]);
            }
        }
        self.update_client_list();
    }

    fn set_wm_state(&self, w: u32, st: u32) {
        let _ = self.conn.change_property32(PropMode::REPLACE, w, self.atoms.WM_STATE, self.atoms.WM_STATE, &[st, 0]);
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
            self.set_frame_extents(w, [b, b, t, b]);
        } else {
            let _ = self.conn.configure_window(w, &ConfigureWindowAux::new().x(g.x).y(g.y).width(g.w as u32).height(g.h as u32));
        }
        // ICCCM synthetic ConfigureNotify with root coordinates
        let ev = ConfigureNotifyEvent { response_type: CONFIGURE_NOTIFY_EVENT, sequence: 0, event: w, window: w, above_sibling: x11rb::NONE,
            x: g.x as i16, y: g.y as i16, width: g.w as u16, height: g.h as u16, border_width: 0, override_redirect: false };
        let _ = self.conn.send_event(false, w, EventMask::STRUCTURE_NOTIFY, ev);
        if size_changed || true { self.redraw(w); }
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
        self.monitors.iter().position(|m| m.contains(x, y)).unwrap_or(0)
    }

    /// Work area of a monitor (monitor minus dock struts).
    pub fn workarea_for(&self, mon: usize) -> Rect {
        let m = self.monitors.get(mon).copied().unwrap_or_default();
        let (sw, sh) = self.screen_size();
        let (mut l, mut r, mut t, mut b) = (m.x, m.x + m.w, m.y, m.y + m.h);
        for c in self.clients.values().filter(|c| c.kind == Kind::Dock) {
            let s = self.prop32(c.win, self.atoms._NET_WM_STRUT_PARTIAL, AtomEnum::CARDINAL);
            let s = if s.len() >= 4 { s } else { self.prop32(c.win, self.atoms._NET_WM_STRUT, AtomEnum::CARDINAL) };
            if s.len() < 4 { continue; }
            let (sl, sr, st, sb) = (s[0] as i32, s[1] as i32, s[2] as i32, s[3] as i32);
            if sb > 0 && sh - sb < m.y + m.h && sh - sb > m.y { b = b.min(sh - sb); }
            if st > 0 && st > m.y && st < m.y + m.h { t = t.max(st); }
            if sl > 0 && sl > m.x && sl < m.x + m.w { l = l.max(sl); }
            if sr > 0 && sw - sr < m.x + m.w && sw - sr > m.x { r = r.min(sw - sr); }
        }
        Rect { x: l, y: t, w: r - l, h: b - t }
    }

    fn update_workarea(&mut self) {
        self.workarea = self.workarea_for(0);
        let wa = self.workarea;
        let v: Vec<u32> = (0..self.num_desktops).flat_map(|_| [wa.x as u32, wa.y as u32, wa.w as u32, wa.h as u32]).collect();
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_WORKAREA, AtomEnum::CARDINAL, &v);
        let ids: Vec<u32> = self.clients.iter().filter(|(_, c)| c.maximized).map(|(w, _)| *w).collect();
        for w in ids { self.set_maximized(w, true); }
    }

    // ---------------------------------------------------------- state changes
    pub fn set_maximized(&mut self, w: u32, on: bool) {
        let Some(c) = self.clients.get(&w) else { return };
        if c.frame.is_none() && c.kind != Kind::Normal { return; }
        let mon = self.monitor_of(&c.geom);
        let wa = self.workarea_for(mon);
        let c = self.clients.get_mut(&w).unwrap();
        if on {
            if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
            c.maximized = true;
            c.snapped = None;
            let t = if c.frame.is_some() { MAX_TOP } else { 0 };
            self.configure(w, Rect { x: wa.x, y: wa.y + t, w: wa.w, h: wa.h - t });
        } else if c.maximized || c.snapped.is_some() {
            c.maximized = false;
            c.snapped = None;
            let r = c.restore.take().unwrap_or(Rect { x: wa.x + 60, y: wa.y + 60, w: wa.w * 2 / 3, h: wa.h * 2 / 3 });
            self.configure(w, r);
        }
        self.update_net_state(w);
    }

    /// Aero Snap: -1 left half, 1 right half.
    pub fn snap(&mut self, w: u32, side: i8) {
        let Some(c) = self.clients.get(&w) else { return };
        if c.frame.is_none() || !c.resizable { return; }
        let wa = self.workarea_for(self.monitor_of(&c.geom));
        let c = self.clients.get_mut(&w).unwrap();
        if !c.maximized && c.snapped.is_none() { c.restore = Some(c.geom); }
        c.maximized = false;
        c.snapped = Some(side);
        let half = wa.w / 2;
        let x = if side < 0 { wa.x } else { wa.x + wa.w - half };
        self.configure(w, Rect { x: x + BORDER, y: wa.y + TOP, w: half - 2 * BORDER, h: wa.h - TOP - BORDER });
        self.update_net_state(w);
    }

    pub fn set_fullscreen(&mut self, w: u32, on: bool) {
        let Some(c) = self.clients.get(&w) else { return };
        let m = self.monitors.get(self.monitor_of(&c.geom)).copied().unwrap_or_default();
        let c = self.clients.get_mut(&w).unwrap();
        if on && !c.fullscreen {
            if !c.maximized { c.restore = Some(c.geom); }
            c.fullscreen = true;
            self.configure(w, m);
        } else if !on && c.fullscreen {
            c.fullscreen = false;
            let r = c.restore.take().unwrap_or(Rect { x: m.x + 50, y: m.y + 80, w: m.w / 2, h: m.h / 2 });
            if c.maximized { self.set_maximized(w, true) } else { self.configure(w, r) }
        }
        self.restack();
        self.update_net_state(w);
    }

    pub fn minimize(&mut self, w: u32) {
        let Some(c) = self.clients.get_mut(&w) else { return };
        if c.minimized { return; }
        c.minimized = true;
        c.ignore_unmaps += if c.frame.is_some() { 1 } else { 1 };
        let target = c.frame.unwrap_or(w);
        if c.frame.is_some() { let _ = self.conn.unmap_window(w); }
        let _ = self.conn.unmap_window(target);
        self.set_wm_state(w, 3);
        self.update_net_state(w);
        if self.active == w {
            self.active = 0;
            let next = self.stack.iter().rev().copied().find(|x| *x != w && self.clients.get(x).map(|c| c.frame.is_some() && !c.minimized && c.desktop == self.current_desktop).unwrap_or(false));
            if let Some(n) = next { self.focus(n) } else { let _ = self.conn.set_input_focus(InputFocus::POINTER_ROOT, self.root, CURRENT_TIME); let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW, &[0]); }
        }
    }

    pub fn unminimize(&mut self, w: u32) {
        let Some(c) = self.clients.get_mut(&w) else { return };
        if !c.minimized { return; }
        c.minimized = false;
        if let Some(f) = c.frame { let _ = self.conn.map_window(w); let _ = self.conn.map_window(f); } else { let _ = self.conn.map_window(w); }
        self.set_wm_state(w, 1);
        self.update_net_state(w);
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
        if self.showing_desktop { self.set_showing_desktop(false); }
        let c = &self.clients[&w];
        if c.desktop != self.current_desktop && c.desktop != 0xffffffff { self.set_desktop(c.desktop); }
        let old = self.active;
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
        self.update_client_list();
    }

    pub fn set_showing_desktop(&mut self, on: bool) {
        if on == self.showing_desktop { return; }
        self.showing_desktop = on;
        let ids: Vec<u32> = self.clients.values().filter(|c| matches!(c.kind, Kind::Normal | Kind::Dialog | Kind::Utility)).map(|c| c.win).collect();
        for w in ids {
            let c = self.clients.get_mut(&w).unwrap();
            if c.minimized { continue; }
            c.ignore_unmaps += 1;
            let t = c.frame.unwrap_or(w);
            if on { let _ = self.conn.unmap_window(t); } else { c.ignore_unmaps -= 1; let _ = self.conn.map_window(t); }
        }
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_SHOWING_DESKTOP, AtomEnum::CARDINAL, &[on as u32]);
    }

    pub fn set_desktop(&mut self, d: u32) {
        let d = d.min(self.num_desktops.saturating_sub(1));
        if d == self.current_desktop { return; }
        self.current_desktop = d;
        let ids: Vec<u32> = self.clients.keys().copied().collect();
        for w in ids {
            let c = self.clients.get_mut(&w).unwrap();
            if c.desktop == 0xffffffff || c.minimized || matches!(c.kind, Kind::Dock | Kind::Desktop) { continue; }
            let t = c.frame.unwrap_or(w);
            if c.desktop == d { let _ = self.conn.map_window(t); } else { c.ignore_unmaps += 1; let _ = self.conn.unmap_window(t); }
        }
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL, &[d]);
    }

    pub fn set_num_desktops(&mut self, n: u32) {
        self.num_desktops = n.clamp(1, 16);
        let _ = self.conn.change_property32(PropMode::REPLACE, self.root, self.atoms._NET_NUMBER_OF_DESKTOPS, AtomEnum::CARDINAL, &[self.num_desktops]);
        let names: String = (1..=self.num_desktops).map(|i| format!("Desktop {i}\0")).collect();
        let _ = self.conn.change_property8(PropMode::REPLACE, self.root, self.atoms._NET_DESKTOP_NAMES, self.atoms.UTF8_STRING, names.as_bytes());
        if self.current_desktop >= self.num_desktops { self.set_desktop(self.num_desktops - 1); }
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
        if c.fullscreen || c.minimized { return; }
        let fr = self.frame_rect(c);
        let (fw, fh) = (fr.w, fr.h);
        if fw <= 0 || fh <= 0 { return; }
        let Ok(mut surf) = cairo::ImageSurface::create(cairo::Format::ARgb32, fw, fh) else { return };
        let composited = self.composited();
        {
            let cr = cairo::Context::new(&surf).unwrap();
            let btns = self.buttons_for(c);
            let wp = self.wallpaper.clone();
            let backdrop = move |cr: &cairo::Context, sx: f64, sy: f64| {
                if let Some(wp) = &wp { cr.set_source_surface(wp, -sx, -sy).ok(); cr.paint().ok(); }
                else { cr.set_source_rgb(0.35, 0.55, 0.75); cr.paint().ok(); }
            };
            self.decor.render(&cr, &Params {
                w: fw, h: fh, title: &c.title, icon: c.icon.as_ref(), active: self.active == w, maximized: c.maximized,
                buttons: &btns, composited, color: self.color, backdrop: Some(&backdrop), screen_pos: (fr.x, fr.y),
            });
        }
        surf.flush();
        let stride = surf.stride() as usize;
        let data = surf.data().unwrap().to_vec();
        let depth = if self.argb.is_some() { 32 } else { self.conn.setup().roots[self.screen_num].root_depth };
        let (b, t) = (self.border_of(c), self.top_of(c));
        let gc = self.conn.generate_id().unwrap();
        if self.conn.create_gc(gc, frame, &CreateGCAux::new().graphics_exposures(0)).is_err() { return; }
        // only upload the border strips; the client covers the rest
        let strips = [(0, 0, fw, t), (0, fh - b, fw, b), (0, t, b, fh - t - b), (fw - b, t, b, fh - t - b)];
        let max_rows = (200_000 / (fw as usize * 4).max(1)).max(1) as i32;
        for (x, y, sw, sh) in strips {
            if sw <= 0 || sh <= 0 { continue; }
            let mut row = y;
            while row < y + sh {
                let n = max_rows.min(y + sh - row);
                let mut buf = Vec::with_capacity((sw * n * 4) as usize);
                for r in row..row + n {
                    let o = r as usize * stride + x as usize * 4;
                    buf.extend_from_slice(&data[o..o + sw as usize * 4]);
                }
                let _ = self.conn.put_image(ImageFormat::Z_PIXMAP, frame, gc, sw as u16, n as u16, x as i16, row as i16, 0, depth, &buf);
                row += n;
            }
        }
        let _ = self.conn.free_gc(gc);
        self.shape_frame(frame, fw, fh, if c.maximized { 0 } else { decor::RADIUS as i32 });
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
        for (b, r) in self.decor.button_rects(fr.w, c.maximized, &self.buttons_for(c)) {
            if x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3] { return Hit::Button(b); }
        }
        if c.maximized || !c.resizable { return Hit::Caption; }
        let corner = 16;
        let mut e = 0u8;
        if x < BORDER || (x < corner && (y < BORDER || y > fr.h - BORDER)) { e |= 1; }
        if x >= fr.w - BORDER || (x >= fr.w - corner && (y < BORDER || y > fr.h - BORDER)) { e |= 2; }
        if y < 4 || (y < corner && (x < BORDER || x >= fr.w - BORDER)) { e |= 4; }
        if y >= fr.h - BORDER || (y >= fr.h - corner && (x < BORDER || x >= fr.w - BORDER)) { e |= 8; }
        if e != 0 { Hit::Edge(e) } else { Hit::Caption }
    }

    // ---------------------------------------------------------- event loop
    pub fn run(&mut self) -> Result<()> {
        loop {
            self.conn.flush()?;
            let ev = self.conn.wait_for_event()?;
            if let Err(e) = self.handle(ev) { eprintln!("aero7-wm: {e}"); }
        }
    }

    fn handle(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::MapRequest(e) => {
                if let Some(c) = self.clients.get(&e.window) {
                    if c.minimized { let w = c.win; self.unminimize(w); self.focus(w); }
                    else if c.frame.is_none() { self.conn.map_window(e.window)?; }
                } else { self.manage(e.window); }
            }
            Event::UnmapNotify(e) => {
                if let Some(c) = self.clients.get_mut(&e.window) {
                    if c.ignore_unmaps > 0 { c.ignore_unmaps -= 1; } else if e.event != self.root || c.frame.is_none() { self.unmanage(e.window, false); }
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
                eprintln!("aero7-wm: compositor change, composited={} argb={}", self.composited(), self.argb.is_some());
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
            if c.maximized || c.fullscreen {
                // keep maximised windows where they are, but confirm geometry
                let g = c.geom;
                self.configure(e.window, g);
                return;
            }
            let mut g = c.geom;
            let m = e.value_mask;
            if m.contains(ConfigWindow::X) { g.x = e.x as i32; }
            if m.contains(ConfigWindow::Y) { g.y = e.y as i32; }
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
        if e.window == self.root { return; }
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
        } else if e.atom == a.WM_PROTOCOLS {
            let p = self.prop32(e.window, a.WM_PROTOCOLS, AtomEnum::ATOM);
            self.clients.get_mut(&e.window).unwrap().protocols = p;
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
            for prop in [d[1], d[2]] {
                if prop == 0 { continue; }
                let Some(c) = self.clients.get(&w) else { return };
                let cur = if prop == a._NET_WM_STATE_FULLSCREEN { c.fullscreen }
                    else if prop == a._NET_WM_STATE_MAXIMIZED_VERT || prop == a._NET_WM_STATE_MAXIMIZED_HORZ { c.maximized }
                    else if prop == a._NET_WM_STATE_ABOVE { c.above }
                    else if prop == a._NET_WM_STATE_HIDDEN { c.minimized }
                    else if prop == a._NET_WM_STATE_DEMANDS_ATTENTION { c.urgent }
                    else if prop == a._NET_WM_STATE_SKIP_TASKBAR { c.skip_taskbar } else { continue };
                let on = match d[0] { 0 => false, 1 => true, _ => !cur };
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
                c.desktop = d[0];
                let _ = self.conn.change_property32(PropMode::REPLACE, w, a._NET_WM_DESKTOP, AtomEnum::CARDINAL, &[d[0]]);
                let t = c.frame.unwrap_or(w);
                if d[0] != self.current_desktop && d[0] != 0xffffffff { c.ignore_unmaps += 1; let _ = self.conn.unmap_window(t); }
            }
        } else if e.type_ == a._AERO7_SNAP_PREVIEW && w == self.check_win {
            // compositor started/stopped: repaint frames (glass vs. opaque)
            let ids: Vec<u32> = self.clients.keys().copied().collect();
            for w in ids { self.redraw(w); }
        } else if e.type_ == a._NET_REQUEST_FRAME_EXTENTS {
            self.set_frame_extents(w, [BORDER, BORDER, TOP, BORDER]);
        } else if e.type_ == a._NET_MOVERESIZE_WINDOW {
            if let Some(c) = self.clients.get(&w) {
                let mut g = c.geom;
                let f = d[0] >> 8;
                if f & 1 != 0 { g.x = d[1] as i32; }
                if f & 2 != 0 { g.y = d[2] as i32; }
                if f & 4 != 0 { g.w = d[3] as i32; }
                if f & 8 != 0 { g.h = d[4] as i32; }
                self.configure(w, g);
            }
        } else if e.type_ == a._NET_WM_MOVERESIZE {
            // client-side-decorated apps asking us to move/resize them
            let Some(c) = self.clients.get(&w) else { return };
            let (px, py) = (d[0] as i32, d[1] as i32);
            let dir = d[2];
            if dir == 11 { self.drag = None; let _ = self.conn.ungrab_pointer(CURRENT_TIME); return; }
            let drag = if dir == 8 {
                Drag::Move { cx: px, cy: py, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized }
            } else if dir <= 7 {
                let edges = [5u8, 4, 6, 2, 10, 8, 9, 1][dir as usize];
                Drag::Resize { edges, start: c.geom, px, py }
            } else { return };
            self.drag = Some((w, drag));
            let _ = self.conn.grab_pointer(false, self.root, EventMask::POINTER_MOTION | EventMask::BUTTON_RELEASE, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, CURRENT_TIME);
        }
    }

    fn button_press(&mut self, e: ButtonPressEvent) -> Result<()> {
        // click on a client window (passive grab): focus, then replay
        if let Some(c) = self.clients.get(&e.event) {
            let w = c.win;
            if self.active != w { self.focus(w); }
            // Alt+drag moves windows (like Win7 utilities/Plasma)
            if u16::from(e.state) & u16::from(ModMask::M1) != 0 && e.detail == 1 {
                let c = &self.clients[&w];
                self.drag = Some((w, Drag::Move { cx: e.root_x as i32, cy: e.root_y as i32, ox: c.geom.x, oy: c.geom.y, from_max: c.maximized }));
                self.conn.allow_events(Allow::ASYNC_POINTER, e.time)?;
                self.conn.grab_pointer(false, self.root, EventMask::POINTER_MOTION | EventMask::BUTTON_RELEASE, GrabMode::ASYNC, GrabMode::ASYNC, x11rb::NONE, x11rb::NONE, e.time)?;
                return Ok(());
            }
            self.conn.allow_events(Allow::REPLAY_POINTER, e.time)?;
            return Ok(());
        }
        let Some(w) = self.frames.get(&e.event).copied() else {
            if e.event == self.root { self.switcher_cancel(); }
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
            }
            (1, Hit::Edge(edges)) => {
                self.drag = Some((w, Drag::Resize { edges, start: c.geom, px: e.root_x as i32, py: e.root_y as i32 }));
            }
            (3, Hit::Caption) | (3, Hit::Button(_)) => {
                crate::util::spawn(&["aero7", "sysmenu", &w.to_string(), &e.root_x.to_string(), &e.root_y.to_string()]);
            }
            (2, Hit::Caption) => { self.stack.retain(|x| *x != w); self.stack.insert(0, w); self.restack(); }
            _ => {}
        }
        Ok(())
    }

    fn button_release(&mut self, e: ButtonReleaseEvent) {
        let Some((w, drag)) = self.drag.take() else { return };
        let _ = self.conn.ungrab_pointer(e.time);
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
                if let Some((_, side)) = self.snap_preview.take() {
                    self.hide_snap_preview();
                    match side { 0 => self.set_maximized(w, true), s => self.snap(w, s) }
                }
            }
            _ => {}
        }
    }

    fn motion(&mut self, e: MotionNotifyEvent) {
        let (rx, ry) = (e.root_x as i32, e.root_y as i32);
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
                    let g = self.clients[&w].geom;
                    self.drag = Some((w, Drag::Move { cx: rx, cy: ry, ox: g.x, oy: g.y, from_max: false }));
                    return;
                }
                let g = Rect { x: ox + rx - cx, y: (oy + ry - cy).max(self.monitors.iter().map(|m| m.y).min().unwrap_or(0) + TOP / 2), ..c.geom };
                self.configure(w, g);
                // Aero Snap preview when touching a screen edge
                let m = self.monitors.get(self.monitors.iter().position(|m| m.contains(rx, ry)).unwrap_or(0)).copied().unwrap_or_default();
                let side = if rx <= m.x + 1 { Some(-1) } else if rx >= m.x + m.w - 2 { Some(1) } else if ry <= m.y + 1 { Some(0) } else { None };
                if side != self.snap_preview.map(|s| s.1) {
                    self.hide_snap_preview();
                    if let Some(s) = side { if self.clients[&w].resizable { self.show_snap_preview(w, s); } }
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
            }
            Some((_, Drag::Button { .. })) => {}
            None => {
                // hover highlight on caption buttons + resize cursors
                let Some(w) = self.frames.get(&e.event).copied() else { return };
                let c = &self.clients[&w];
                let hit = self.hit(c, e.event_x as i32, e.event_y as i32);
                let hb = if let Hit::Button(b) = hit { Some(b) } else { None };
                if hb != c.hover { self.clients.get_mut(&w).unwrap().hover = hb; self.redraw(w); }
                self.set_cursor(e.event, &hit);
            }
        }
    }

    fn set_cursor(&self, win: u32, hit: &Hit) {
        let name = match hit {
            Hit::Edge(5) | Hit::Edge(10) => "top_left_corner",
            Hit::Edge(6) | Hit::Edge(9) => "top_right_corner",
            Hit::Edge(1) | Hit::Edge(2) => "sb_h_double_arrow",
            Hit::Edge(4) | Hit::Edge(8) => "sb_v_double_arrow",
            _ => "left_ptr",
        };
        if let Some(cur) = keys::cursor(&self.conn, self.screen_num, name) {
            let _ = self.conn.change_window_attributes(win, &ChangeWindowAttributesAux::new().cursor(cur));
        }
    }

    fn show_snap_preview(&mut self, w: u32, side: i8) {
        let Some(c) = self.clients.get(&w) else { return };
        let wa = self.workarea_for(self.monitor_of(&Rect { x: self.pointer().0, y: self.pointer().1, w: 1, h: 1 }));
        let _ = c;
        let r = match side { -1 => Rect { w: wa.w / 2, ..wa }, 1 => Rect { x: wa.x + wa.w - wa.w / 2, w: wa.w / 2, ..wa }, _ => wa };
        let win = self.conn.generate_id().unwrap();
        let composited = self.composited();
        let (depth, vis, cmap) = match (composited, self.argb) { (true, Some((v, c))) => (32, v, c), _ => (x11rb::COPY_DEPTH_FROM_PARENT, 0, 0) };
        let mut aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).border_pixel(0);
        if cmap != 0 { aux = aux.colormap(cmap); }
        let r2 = Rect { x: r.x + 8, y: r.y + 8, w: r.w - 16, h: r.h - 16 };
        if self.conn.create_window(depth, win, self.root, r2.x as i16, r2.y as i16, r2.w as u16, r2.h as u16, 0, WindowClass::INPUT_OUTPUT, vis, &aux).is_err() { return; }
        let _ = self.conn.map_window(win);
        // paint: glass rectangle with white edge (Win7 "peek" outline)
        if let Ok(mut s) = cairo::ImageSurface::create(cairo::Format::ARgb32, r2.w, r2.h) {
            {
                let cr = cairo::Context::new(&s).unwrap();
                if !composited { if let Some(wp) = &self.wallpaper { cr.set_source_surface(wp, -(r2.x as f64), -(r2.y as f64)).ok(); cr.paint().ok(); } }
                crate::glass::rounded(&cr, 0.5, 0.5, r2.w as f64 - 1.0, r2.h as f64 - 1.0, 6.0);
                cr.set_source_rgba(self.color[0], self.color[1], self.color[2], 0.35);
                cr.fill_preserve().ok();
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
        if let Some((win, _)) = self.snap_preview.take() { let _ = self.conn.destroy_window(win); }
    }

    fn switcher_cancel(&mut self) {
        if let Some(s) = self.switcher.take() { s.destroy(&self.conn); }
    }

    // ---------------------------------------------------------- keys
    fn key(&mut self, e: KeyPressEvent, press: bool) {
        let Some(action) = self.keys.lookup(e.detail, u16::from(e.state), press, &self.conn) else { return };
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
            MinimizeOthers => {
                let ids: Vec<u32> = self.clients.values().filter(|c| c.win != act && c.frame.is_some() && !c.minimized).map(|c| c.win).collect();
                for w in ids { self.minimize(w); }
            }
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
            }
            SwitchCommit => {
                if let Some(s) = self.switcher.take() {
                    let target = s.selected();
                    s.destroy(&self.conn);
                    let _ = self.conn.ungrab_keyboard(e.time);
                    if let Some(w) = target { self.focus(w); }
                }
            }
            SwitchCancel => { self.switcher_cancel(); let _ = self.conn.ungrab_keyboard(e.time); }
            DesktopLeft => { let d = self.current_desktop.saturating_sub(1); self.set_desktop(d); }
            DesktopRight => { let d = (self.current_desktop + 1).min(self.num_desktops - 1); self.set_desktop(d); }
            DesktopAdd => { let n = self.num_desktops + 1; self.set_num_desktops(n); self.set_desktop(n - 1); }
            DesktopRemove => { let n = self.num_desktops.saturating_sub(1).max(1); self.set_num_desktops(n); }
        }
    }

    fn switch_order(&self) -> Vec<u32> {
        // most recently used first = top of stack first
        let mut v: Vec<u32> = self.stack.iter().rev().copied().filter(|w| self.clients.get(w).map(|c| matches!(c.kind, Kind::Normal | Kind::Dialog) && !c.skip_taskbar && c.frame.is_some()).unwrap_or(false)).collect();
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
                let ev = ClientMessageEvent::new(32, check_win, a._AERO7_SNAP_PREVIEW, [0, 0, 0, 0, 0]);
                let _ = conn.send_event(false, check_win, EventMask::NO_EVENT, ev);
                let _ = conn.flush();
            }
            last = owner;
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

fn c_kind_focusable(c: &Client) -> bool { !matches!(c.kind, Kind::Dock | Kind::Desktop) }

/// Win7 DWM colourisation (0xRRGGBB with alpha) derived from the Personalize tint.
pub fn colorization(t: &[f64; 4]) -> [f64; 4] {
    // default "Sky" = 0x6B74B8FC; scale other tints to similar lightness
    let l = 0.299 * t[0] + 0.587 * t[1] + 0.114 * t[2];
    let k = if l > 0.0 { 0.62 / l } else { 1.0 };
    let (r, g, b) = ((t[0] * k).min(1.0), (t[1] * k).min(1.0), (t[2] * k).min(1.0));
    if (t[0] - 0.10).abs() < 0.01 && (t[1] - 0.22).abs() < 0.01 { return [0.455, 0.722, 0.988, 0.42]; }
    [r, g, b, 0.30 + t[3] * 0.25]
}

enum Hit { Button(Btn), Caption, Edge(u8) }
