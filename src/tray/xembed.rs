//! The legacy XEmbed system tray (freedesktop System Tray Protocol): Wine programs and older
//! GTK/Qt applications dock their icon windows into `_NET_SYSTEM_TRAY_S<screen>`.
//!
//! Icons are not shown as embedded X windows. Each one is reparented into a small container of
//! ours and redirected off-screen with Composite; its picture is read back whenever Damage reports
//! a change and drawn like every other notification-area icon, so it can sit on the glass taskbar
//! or in the overflow flyout with the same hover look (a raw child window could do neither: moving
//! a GtkSocket between windows would destroy the icon, and an ARGB child punches a hole into the
//! glass). Clicks are replayed on the real icon with XTest after moving its container under the
//! pointer; the container is invisible (an empty Shape) except for the instant of the click.
use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::composite::{self, ConnectionExt as _};
use x11rb::protocol::damage::{self, ConnectionExt as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest::{self, ConnectionExt as _};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::{Backend, Status};

const SIZE: u16 = 16;
const REQUEST_DOCK: u32 = 0;
const XEMBED_EMBEDDED_NOTIFY: u32 = 0;
const XEMBED_MAPPED: u32 = 1;
/// Background behind icons without an alpha channel; read back as transparent.
const KEY: (u8, u8, u8) = (0x28, 0x4e, 0x74);

struct Atoms {
    selection: u32,
    opcode: u32,
    manager: u32,
    orientation: u32,
    visual: u32,
    icon_size: u32,
    xembed: u32,
    xembed_info: u32,
    net_wm_name: u32,
    utf8: u32,
    opacity: u32,
    pid: u32,
}

struct Icon {
    container: u32,
    damage: u32,
    depth: u8,
    resized: Cell<u8>,
}

struct Tray {
    conn: RustConnection,
    root: u32,
    root_depth: u8,
    owner: u32,
    atoms: Atoms,
    xtest: bool,
    lsb: bool,
    icons: RefCell<HashMap<u32, Icon>>,
    dirty: RefCell<Vec<u32>>,
    capture_queued: Cell<bool>,
    clicks: Cell<u32>,
}

thread_local! {
    static TRAY: RefCell<Option<Rc<Tray>>> = const { RefCell::new(None) };
}

fn tray() -> Option<Rc<Tray>> {
    TRAY.with(|t| t.borrow().clone())
}

fn id_of(win: u32) -> String {
    format!("xembed:{win}")
}

/// Become the system tray of the default screen (unless another one already is).
pub fn start() {
    match Tray::new() {
        Ok(Some(t)) => {
            use std::os::fd::AsRawFd;
            let fd = t.conn.stream().as_raw_fd();
            let t2 = t.clone();
            glib::source::unix_fd_add_local(fd, glib::IOCondition::IN, move |_, _| { t2.drain(); glib::ControlFlow::Continue });
            TRAY.with(|x| *x.borrow_mut() = Some(t.clone()));
            t.drain();
        }
        Ok(None) => {}
        Err(e) => eprintln!("aerode: notification area: no XEmbed system tray: {e}"),
    }
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

impl Tray {
    fn new() -> Res<Option<Rc<Tray>>> {
        let (conn, screen) = x11rb::connect(None)?;
        let scr = conn.setup().roots[screen].clone();
        let atom = |n: &str| -> Res<u32> { Ok(conn.intern_atom(false, n.as_bytes())?.reply()?.atom) };
        let atoms = Atoms {
            selection: atom(&format!("_NET_SYSTEM_TRAY_S{screen}"))?,
            opcode: atom("_NET_SYSTEM_TRAY_OPCODE")?,
            manager: atom("MANAGER")?,
            orientation: atom("_NET_SYSTEM_TRAY_ORIENTATION")?,
            visual: atom("_NET_SYSTEM_TRAY_VISUAL")?,
            icon_size: atom("_NET_SYSTEM_TRAY_ICON_SIZE")?,
            xembed: atom("_XEMBED")?,
            xembed_info: atom("_XEMBED_INFO")?,
            net_wm_name: atom("_NET_WM_NAME")?,
            utf8: atom("UTF8_STRING")?,
            opacity: atom("_NET_WM_WINDOW_OPACITY")?,
            pid: atom("_NET_WM_PID")?,
        };
        if conn.get_selection_owner(atoms.selection)?.reply()?.owner != x11rb::NONE {
            eprintln!("aerode: notification area: another program is the system tray, legacy icons stay there");
            return Ok(None);
        }
        if conn.extension_information(composite::X11_EXTENSION_NAME)?.is_none() || conn.extension_information(damage::X11_EXTENSION_NAME)?.is_none() {
            return Err("the X server lacks Composite/Damage".into());
        }
        conn.composite_query_version(0, 4)?.reply()?;
        conn.damage_query_version(1, 1)?.reply()?;
        let xtest = conn.extension_information(xtest::X11_EXTENSION_NAME)?.is_some() && conn.xtest_get_version(2, 2)?.reply().is_ok();
        let owner = conn.generate_id()?;
        conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, owner, scr.root, -1, -1, 1, 1, 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().override_redirect(1).event_mask(EventMask::STRUCTURE_NOTIFY))?;
        conn.change_property32(PropMode::REPLACE, owner, atoms.orientation, AtomEnum::CARDINAL, &[0])?;
        conn.change_property32(PropMode::REPLACE, owner, atoms.icon_size, AtomEnum::CARDINAL, &[SIZE as u32])?;
        // icons may draw with alpha: offer the 32-bit visual
        let argb = scr.allowed_depths.iter().filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter()).find(|v| v.class == VisualClass::TRUE_COLOR).map(|v| v.visual_id);
        if let Some(v) = argb {
            conn.change_property32(PropMode::REPLACE, owner, atoms.visual, AtomEnum::VISUALID, &[v])?;
        }
        conn.set_selection_owner(owner, atoms.selection, x11rb::CURRENT_TIME)?;
        if conn.get_selection_owner(atoms.selection)?.reply()?.owner != owner {
            conn.destroy_window(owner)?;
            return Ok(None);
        }
        let ev = ClientMessageEvent::new(32, scr.root, atoms.manager, [x11rb::CURRENT_TIME, atoms.selection, owner, 0, 0]);
        conn.send_event(false, scr.root, EventMask::STRUCTURE_NOTIFY, ev)?;
        conn.flush()?;
        let lsb = conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        Ok(Some(Rc::new(Tray {
            conn, root: scr.root, root_depth: scr.root_depth, owner, atoms, xtest, lsb,
            icons: RefCell::new(HashMap::new()), dirty: RefCell::new(vec![]), capture_queued: Cell::new(false), clicks: Cell::new(0),
        })))
    }

    fn drain(self: &Rc<Self>) {
        while let Ok(Some(ev)) = self.conn.poll_for_event() {
            match ev {
                Event::ClientMessage(e) if e.window == self.owner && e.type_ == self.atoms.opcode => {
                    let d = e.data.as_data32();
                    // balloon messages (BEGIN/CANCEL_MESSAGE) are not shown
                    if d[1] == REQUEST_DOCK { self.dock(d[2]); }
                }
                Event::SelectionClear(e) if e.selection == self.atoms.selection => {
                    self.release_all();
                    return;
                }
                Event::DestroyNotify(e) => self.undock(e.window, false),
                Event::ReparentNotify(e) => {
                    let inside = self.icons.borrow().get(&e.window).map(|i| i.container == e.parent);
                    if inside == Some(false) { self.undock(e.window, false); }
                }
                Event::PropertyNotify(e) if self.icons.borrow().contains_key(&e.window) => {
                    if e.atom == self.atoms.xembed_info {
                        let shown = self.wants_map(e.window);
                        if shown { let _ = self.conn.map_window(e.window); } else { let _ = self.conn.unmap_window(e.window); }
                        super::update(&id_of(e.window), |it| it.shown = shown);
                        self.queue_capture(e.window);
                    } else if e.atom == self.atoms.net_wm_name || e.atom == u32::from(AtomEnum::WM_NAME) {
                        let t = self.title(e.window);
                        if !t.is_empty() { super::update(&id_of(e.window), |it| { it.tooltip = t.clone(); it.title = t; }); }
                    }
                }
                Event::ConfigureNotify(e) if self.icons.borrow().contains_key(&e.window) => {
                    // keep icons at 16×16 (a few apps resize themselves; give up after a few tries)
                    if (e.width, e.height) != (SIZE, SIZE) {
                        let again = self.icons.borrow().get(&e.window).map(|i| { i.resized.set(i.resized.get() + 1); i.resized.get() < 4 }).unwrap_or(false);
                        if again {
                            let _ = self.conn.configure_window(e.window, &ConfigureWindowAux::new().x(0).y(0).width(SIZE as u32).height(SIZE as u32));
                        }
                    }
                    self.queue_capture(e.window);
                }
                Event::DamageNotify(e) => {
                    let _ = self.conn.damage_subtract(e.damage, x11rb::NONE, x11rb::NONE);
                    let win = self.icons.borrow().iter().find(|(_, i)| i.damage == e.damage).map(|(w, _)| *w);
                    if let Some(w) = win { self.queue_capture(w); }
                }
                _ => {}
            }
        }
        let _ = self.conn.flush();
    }

    fn prop(&self, win: u32, atom: u32, ty: impl Into<u32>, len: u32) -> Option<GetPropertyReply> {
        self.conn.get_property(false, win, atom, ty, 0, len).ok()?.reply().ok().filter(|r| r.format != 0)
    }

    /// `_XEMBED_INFO` flags: does the client want its icon shown?
    fn wants_map(&self, win: u32) -> bool {
        self.prop(win, self.atoms.xembed_info, self.atoms.xembed_info, 2)
            .and_then(|r| r.value32().map(|v| v.collect::<Vec<u32>>()))
            .and_then(|v| v.get(1).copied())
            .map(|flags| flags & XEMBED_MAPPED != 0)
            .unwrap_or(true)
    }

    fn title(&self, win: u32) -> String {
        if let Some(r) = self.prop(win, self.atoms.net_wm_name, self.atoms.utf8, 256) {
            let s = String::from_utf8_lossy(&r.value).trim().to_string();
            if !s.is_empty() { return s; }
        }
        self.prop(win, AtomEnum::WM_NAME.into(), AtomEnum::ANY, 256)
            .map(|r| r.value.iter().map(|b| *b as char).collect::<String>().trim().to_string())
            .unwrap_or_default()
    }

    /// (key, fallback name): WM_CLASS instance, else the process name.
    fn program(&self, win: u32) -> (String, String) {
        if let Some(r) = self.prop(win, AtomEnum::WM_CLASS.into(), AtomEnum::STRING, 256) {
            let parts: Vec<String> = r.value.split(|b| *b == 0).map(|s| String::from_utf8_lossy(s).into_owned()).filter(|s| !s.is_empty()).collect();
            if let Some(inst) = parts.first() {
                let name = parts.get(1).filter(|c| !c.eq_ignore_ascii_case("wine")).unwrap_or(inst).clone();
                return (inst.to_lowercase(), name);
            }
        }
        let pid = self.prop(win, self.atoms.pid, AtomEnum::CARDINAL, 1).and_then(|r| r.value32().and_then(|mut v| v.next()));
        let exe = pid.and_then(|p| std::fs::read_to_string(format!("/proc/{p}/comm")).ok()).map(|s| s.trim().to_string()).unwrap_or_default();
        if exe.is_empty() { (format!("xembed-{win}"), "Notification icon".into()) } else { (exe.to_lowercase(), exe) }
    }

    fn dock(self: &Rc<Self>, win: u32) {
        if win == 0 || self.icons.borrow().contains_key(&win) { return; }
        match self.embed(win) {
            Ok(icon) => {
                self.icons.borrow_mut().insert(win, icon);
                let (key, prog) = self.program(win);
                let t = self.title(win);
                let title = if t.is_empty() { prog } else { t };
                super::upsert(super::Item {
                    id: id_of(win),
                    key,
                    tooltip: title.clone(),
                    title,
                    icon: None,
                    status: Status::Active,
                    shown: self.wants_map(win),
                    backend: Backend::XEmbed(win),
                });
                // the first frames often come without Damage
                for ms in [60u64, 400, 1500] {
                    let me = self.clone();
                    glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || me.queue_capture(win));
                }
            }
            Err(e) => {
                super::sni::debug(&format!("cannot dock 0x{win:x}: {e}"));
            }
        }
        let _ = self.conn.flush();
    }

    fn embed(&self, win: u32) -> Res<Icon> {
        let c = &self.conn;
        let attr = c.get_window_attributes(win)?.reply()?;
        let depth = c.get_geometry(win)?.reply()?.depth;
        // the container shares the icon's visual (no background: it never shows anything itself)
        let container = c.generate_id()?;
        let mut aux = CreateWindowAux::new().override_redirect(1).event_mask(EventMask::SUBSTRUCTURE_NOTIFY);
        let (cdepth, cvisual) = if depth == self.root_depth {
            (x11rb::COPY_DEPTH_FROM_PARENT, x11rb::COPY_FROM_PARENT)
        } else {
            let cmap = c.generate_id()?;
            c.create_colormap(ColormapAlloc::NONE, cmap, self.root, attr.visual)?;
            aux = aux.colormap(cmap).border_pixel(0);
            (depth, attr.visual)
        };
        c.create_window(cdepth, container, self.root, -100, -100, SIZE, SIZE, 0, WindowClass::INPUT_OUTPUT, cvisual, &aux)?;
        // invisible and click-through: an empty bounding shape (the redirected icon still renders)
        c.shape_rectangles(shape::SO::SET, shape::SK::BOUNDING, ClipOrdering::UNSORTED, container, 0, 0, &[])?;
        c.change_property32(PropMode::REPLACE, container, self.atoms.opacity, AtomEnum::CARDINAL, &[0])?;
        c.change_property8(PropMode::REPLACE, container, AtomEnum::WM_NAME, AtomEnum::STRING, b"aerode tray icon")?;
        // back to the root window if the shell goes away
        c.change_save_set(SetMode::INSERT, win)?;
        c.change_window_attributes(win, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE))?;
        c.reparent_window(win, container, 0, 0)?;
        if depth == self.root_depth {
            // no alpha channel: paint the icon on a key colour that reads back as transparent
            // (instead of ParentRelative, which would show whatever is behind the container)
            c.change_window_attributes(win, &ChangeWindowAttributesAux::new().background_pixel(((KEY.0 as u32) << 16) | ((KEY.1 as u32) << 8) | KEY.2 as u32))?;
            c.clear_area(true, win, 0, 0, 0, 0)?;
        }
        c.configure_window(win, &ConfigureWindowAux::new().x(0).y(0).width(SIZE as u32).height(SIZE as u32))?;
        c.composite_redirect_window(win, composite::Redirect::MANUAL)?;
        let dmg = c.generate_id()?;
        c.damage_create(dmg, win, damage::ReportLevel::NON_EMPTY)?;
        c.map_window(container)?;
        if self.wants_map(win) { c.map_window(win)?; }
        let ev = ClientMessageEvent::new(32, win, self.atoms.xembed, [x11rb::CURRENT_TIME, XEMBED_EMBEDDED_NOTIFY, 0, container, 0]);
        c.send_event(false, win, EventMask::NO_EVENT, ev)?;
        // make the parent switch take effect before anything else talks to the window
        c.get_input_focus()?.reply()?;
        Ok(Icon { container, damage: dmg, depth, resized: Cell::new(0) })
    }

    fn undock(&self, win: u32, give_back: bool) {
        let Some(icon) = self.icons.borrow_mut().remove(&win) else { return };
        if give_back {
            let _ = self.conn.unmap_window(win);
            let _ = self.conn.reparent_window(win, self.root, 0, 0);
            let _ = self.conn.change_save_set(SetMode::DELETE, win);
            let _ = self.conn.damage_destroy(icon.damage);
        }
        let _ = self.conn.destroy_window(icon.container);
        let _ = self.conn.flush();
        super::remove(&id_of(win));
    }

    fn release_all(&self) {
        let wins: Vec<u32> = self.icons.borrow().keys().copied().collect();
        for w in wins { self.undock(w, true); }
        let _ = self.conn.destroy_window(self.owner);
        let _ = self.conn.flush();
        TRAY.with(|t| *t.borrow_mut() = None);
    }

    fn queue_capture(self: &Rc<Self>, win: u32) {
        { let mut d = self.dirty.borrow_mut(); if !d.contains(&win) { d.push(win); } }
        if self.capture_queued.replace(true) { return; }
        let me = self.clone();
        // coalesce bursts of damage
        glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
            me.capture_queued.set(false);
            let wins: Vec<u32> = me.dirty.borrow_mut().drain(..).collect();
            for w in wins { me.capture(w); }
            me.drain();
        });
    }

    /// Read the icon's picture back from its off-screen pixmap.
    fn capture(&self, win: u32) {
        let Some(depth) = self.icons.borrow().get(&win).map(|i| i.depth) else { return };
        let Ok(Ok(g)) = self.conn.get_geometry(win).map(|c| c.reply()) else { return };
        let (w, h) = (g.width.max(1), g.height.max(1));
        let Ok(Ok(img)) = self.conn.get_image(ImageFormat::Z_PIXMAP, win, 0, 0, w, h, !0).map(|c| c.reply()) else { return };
        let n = w as usize * h as usize;
        if img.data.len() < n * 4 { return; }
        let mut rgba = Vec::with_capacity(n * 4);
        for p in img.data.chunks_exact(4).take(n) {
            let (b, g, r, a) = if self.lsb { (p[0], p[1], p[2], p[3]) } else { (p[3], p[2], p[1], p[0]) };
            if depth == 32 {
                // premultiplied → straight alpha
                let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
                rgba.extend_from_slice(&[un(r), un(g), un(b), a]);
            } else if (r, g, b) == KEY {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        let pb = Pixbuf::from_bytes(&glib::Bytes::from_owned(rgba), Colorspace::Rgb, true, 8, w as i32, h as i32, w as i32 * 4);
        let pb = crate::util::scale(&pb, SIZE as i32);
        super::update(&id_of(win), |it| it.icon = Some(pb));
    }

    fn click(&self, win: u32, button: u8, x: i32, y: i32) {
        let Some(container) = self.icons.borrow().get(&win).map(|i| i.container) else { return };
        let c = &self.conn;
        // the icon has to be under the pointer and on top for the click (toolkits check, and
        // place their menus by it); right after, it turns invisible again but stays there
        let half = (SIZE / 2) as i32;
        let _ = c.shape_mask(shape::SO::SET, shape::SK::BOUNDING, container, 0, 0, x11rb::NONE);
        let _ = c.configure_window(container, &ConfigureWindowAux::new().x(x - half).y(y - half).stack_mode(StackMode::ABOVE));
        if self.xtest {
            let _ = c.xtest_fake_input(BUTTON_PRESS_EVENT, button, x11rb::CURRENT_TIME, self.root, 0, 0, 0);
            let _ = c.xtest_fake_input(BUTTON_RELEASE_EVENT, button, x11rb::CURRENT_TIME, self.root, 0, 0, 0);
        } else {
            for (kind, state) in [(BUTTON_PRESS_EVENT, 0u16), (BUTTON_RELEASE_EVENT, 0x80u16 << button.min(5))] {
                let ev = ButtonPressEvent {
                    response_type: kind, detail: button, sequence: 0, time: x11rb::CURRENT_TIME, root: self.root, event: win, child: x11rb::NONE,
                    root_x: x as i16, root_y: y as i16, event_x: half as i16, event_y: half as i16, state: state.into(), same_screen: true,
                };
                let mask = if kind == BUTTON_PRESS_EVENT { EventMask::BUTTON_PRESS } else { EventMask::BUTTON_RELEASE };
                let _ = c.send_event(false, win, mask, ev);
            }
        }
        let _ = c.shape_rectangles(shape::SO::SET, shape::SK::BOUNDING, ClipOrdering::UNSORTED, container, 0, 0, &[]);
        let _ = c.flush();
    }
}

/// Replay a click (X button number) on a docked icon at the pointer position.
pub fn click(win: u32, button: u8, x: i32, y: i32) {
    let Some(t) = tray() else { return };
    t.click(win, button, x, y);
    let n = t.clicks.get().wrapping_add(1);
    t.clicks.set(n);
    // once the program has placed its menu, park the container off-screen again (where the icon
    // would land if the shell went away)
    glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
        if t.clicks.get() != n { return; }
        if let Some(c) = t.icons.borrow().get(&win).map(|i| i.container) {
            let _ = t.conn.configure_window(c, &ConfigureWindowAux::new().x(-100).y(-100));
            let _ = t.conn.flush();
        }
    });
}
