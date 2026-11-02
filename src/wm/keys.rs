//! Global keyboard shortcuts (Windows 7 set + a few Plasma extras).
use std::collections::HashMap;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Debug)]
pub enum Action {
    Spawn(String),
    Close, Maximize, RestoreOrMinimize, SnapLeft, SnapRight, ShowDesktop, MinimizeOthers,
    /// Win+Shift+Up: stretch to the top and bottom of the screen; Win+Shift+Left/Right: next monitor
    MaximizeVert, MonitorLeft, MonitorRight,
    SwitchNext, SwitchPrev, SwitchCommit, SwitchCancel,
    DesktopLeft, DesktopRight, DesktopAdd, DesktopRemove,
    SysMenu,
}

const SHIFT: u16 = 1;
const CTRL: u16 = 4;
const ALT: u16 = 8;
const SUPER: u16 = 64;
const IGNORED: [u16; 4] = [0, 2, 16, 18]; // CapsLock / NumLock combos

// keysyms
const XK_TAB: u32 = 0xff09;
const XK_ESCAPE: u32 = 0xff1b;
const XK_F4: u32 = 0xffc1;
const XK_SPACE: u32 = 0x20;
const XK_LEFT: u32 = 0xff51;
const XK_UP: u32 = 0xff52;
const XK_RIGHT: u32 = 0xff53;
const XK_DOWN: u32 = 0xff54;
const XK_HOME: u32 = 0xff50;
const XK_PRINT: u32 = 0xff61;
const XK_PAUSE: u32 = 0xff13;
const XK_SUPER_L: u32 = 0xffeb;
const XK_SUPER_R: u32 = 0xffec;
const XK_ALT_L: u32 = 0xffe9;
const XK_DELETE: u32 = 0xffff;

fn bindings() -> Vec<(u16, u32, Action)> {
    use Action::*;
    let s = |c: &str| Spawn(c.to_string());
    let mut v = vec![
        (ALT, XK_TAB, SwitchNext), (ALT | SHIFT, XK_TAB, SwitchPrev),
        (SUPER, XK_TAB, SwitchNext), (SUPER | SHIFT, XK_TAB, SwitchPrev),
        (ALT, XK_F4, Close), (ALT, XK_SPACE, SysMenu),
        (SUPER, XK_UP, Maximize), (SUPER, XK_DOWN, RestoreOrMinimize),
        (SUPER, XK_LEFT, SnapLeft), (SUPER, XK_RIGHT, SnapRight),
        (SUPER | SHIFT, XK_UP, MaximizeVert), (SUPER | SHIFT, XK_DOWN, RestoreOrMinimize),
        (SUPER | SHIFT, XK_LEFT, MonitorLeft), (SUPER | SHIFT, XK_RIGHT, MonitorRight),
        (SUPER, 'd' as u32, ShowDesktop), (SUPER, 'm' as u32, ShowDesktop), (SUPER, XK_HOME, MinimizeOthers),
        (SUPER | CTRL, XK_LEFT, DesktopLeft), (SUPER | CTRL, XK_RIGHT, DesktopRight),
        (SUPER | CTRL, 'd' as u32, DesktopAdd), (SUPER | CTRL, XK_F4, DesktopRemove),
        (SUPER, 'e' as u32, s("aerode-explorer")), (SUPER, 'r' as u32, s("aerode run")),
        (SUPER, 'l' as u32, s("aerode lock")), (SUPER, 'p' as u32, s("aerode-control display")),
        (SUPER, 'f' as u32, s("aerode start")), (SUPER, XK_PAUSE, s("aerode-control system")),
        (SUPER | SHIFT, 's' as u32, s("aerode-snip --clip")), (0, XK_PRINT, s("aerode-snip --full")),
        (CTRL | SHIFT, XK_ESCAPE, s("aerode-taskmgr")), (CTRL | ALT, XK_DELETE, s("aerode-taskmgr")),
        (CTRL, XK_ESCAPE, s("aerode start")),
        // volume keys as Win7's SndVolSSO handles APPCOMMAND_VOLUME_UP/DOWN/MUTE and
        // APPCOMMAND_MICROPHONE_VOLUME_MUTE: +-2 % from the loudest channel (balance kept, a level
        // above 100 % never lowered by Up), unmuting first; serialised with the shell's own changes
        (0, 0x1008ff13, s("aerode volume-step up")),
        (0, 0x1008ff11, s("aerode volume-step down")),
        (0, 0x1008ff12, s("aerode audio-mute")),
        (0, 0x1008ffb2, s("aerode mic-mute")),
        (0, 0x1008ff14, s("playerctl play-pause")), (0, 0x1008ff17, s("playerctl next")), (0, 0x1008ff16, s("playerctl previous")),
        (0, 0x1008ff02, s("brightnessctl set 5%+")), (0, 0x1008ff03, s("brightnessctl set 5%-")),
        (0, 0x1008ff1d, s("aerode-calc")), (0, 0x1008ff5d, s("aerode-explorer")),
    ];
    // Win+1..9 handled by the shell (taskbar launch); register them as commands
    for i in 1..=9u32 { v.push((SUPER, '0' as u32 + i, s(&format!("aerode taskbar-launch {i}")))); }
    v
}

#[derive(Default)]
pub struct Keys {
    map: HashMap<(u8, u16), Action>,
    sym_of: HashMap<u8, u32>,
    super_codes: Vec<u8>,
    super_down_alone: bool,
    alt_codes: Vec<u8>,
    /// last key release (code, time): X autorepeat sends release+press with the same timestamp
    last_release: (u8, u32),
}

impl Keys {
    pub fn grab(&mut self, conn: &RustConnection, root: u32) -> anyhow::Result<()> {
        let setup = conn.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let km = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        let per = km.keysyms_per_keycode as usize;
        let mut code_of: HashMap<u32, u8> = HashMap::new();
        self.sym_of.clear();
        for (i, chunk) in km.keysyms.chunks(per.max(1)).enumerate() {
            let code = min + i as u8;
            if let Some(&sym) = chunk.first() {
                if sym != 0 { code_of.entry(sym).or_insert(code); self.sym_of.insert(code, sym); }
            }
        }
        // with a non-Latin first layout (e.g. "ru,us") the letters of Win+E / Win+R / Win+D… only exist in
        // a later XKB group: the core map lists every group's levels after the first, so look there too
        for (i, chunk) in km.keysyms.chunks(per.max(1)).enumerate() {
            for &sym in chunk.iter().skip(1) { if sym != 0 { code_of.entry(sym).or_insert(min + i as u8); } }
        }
        let _ = conn.ungrab_key(Grab::ANY, root, ModMask::ANY);
        self.map.clear();
        for (mods, sym, act) in bindings() {
            let Some(&code) = code_of.get(&sym) else { continue };
            self.map.insert((code, mods), act);
            for ig in IGNORED {
                let _ = conn.grab_key(true, root, ModMask::from(mods | ig), code, GrabMode::ASYNC, GrabMode::ASYNC);
            }
        }
        // the Windows key on its own opens Start (on release, if nothing else was pressed)
        self.super_codes = [XK_SUPER_L, XK_SUPER_R].iter().filter_map(|s| code_of.get(s).copied()).collect();
        for &c in &self.super_codes {
            for ig in IGNORED { let _ = conn.grab_key(true, root, ModMask::from(ig), c, GrabMode::ASYNC, GrabMode::SYNC); }
        }
        self.alt_codes = code_of.get(&XK_ALT_L).copied().into_iter().collect();
        Ok(())
    }

    pub fn sym(&self, code: u8) -> u32 { self.sym_of.get(&code).copied().unwrap_or(0) }

    pub fn lookup(&mut self, code: u8, state: u16, press: bool, time: u32, switching: bool, conn: &RustConnection) -> Option<Action> {
        let mods = state & (SHIFT | CTRL | ALT | SUPER);
        let repeat = press && self.last_release == (code, time);
        if !press { self.last_release = (code, time); }
        if self.super_codes.contains(&code) {
            if press { self.super_down_alone = !switching; let _ = conn.allow_events(Allow::ASYNC_KEYBOARD, x11rb::CURRENT_TIME); return None; }
            if switching { self.super_down_alone = false; return Some(Action::SwitchCommit); }
            let fire = self.super_down_alone && mods & !SUPER == 0;
            self.super_down_alone = false;
            return fire.then(|| Action::Spawn("aerode start".into()));
        }
        self.super_down_alone = false;
        let sym = self.sym_of.get(&code).copied().unwrap_or(0);
        if !press {
            // releasing Alt/Super ends the window switcher
            if self.alt_codes.contains(&code) || self.super_codes.contains(&code) { return Some(Action::SwitchCommit); }
            if sym == 0xffe9 || sym == 0xffea || sym == XK_SUPER_L || sym == XK_SUPER_R { return Some(Action::SwitchCommit); }
            return None;
        }
        if sym == XK_ESCAPE && mods & ALT != 0 { return Some(Action::SwitchCancel); }
        if (sym == XK_LEFT || sym == XK_RIGHT) && mods & ALT != 0 && mods & SUPER == 0 && mods & CTRL == 0 {
            return Some(if sym == XK_LEFT { Action::SwitchPrev } else { Action::SwitchNext });
        }
        let act = self.map.get(&(code, mods)).cloned()?;
        // holding a window-management chord must not run it again on whatever window got focus next
        // (Win+Down held would minimize window after window); switching, desktops and media keys repeat
        if repeat && !matches!(act, Action::SwitchNext | Action::SwitchPrev | Action::DesktopLeft | Action::DesktopRight)
            && !matches!(&act, Action::Spawn(c) if c.contains("volume") || c.contains("brightness")) { return None; }
        Some(act)
    }
}

/// Load a named cursor from the cursor theme, falling back to the X cursor font (left_ptr etc).
pub fn cursor(conn: &RustConnection, screen: usize, name: &str) -> Option<u32> {
    thread_local! { static CACHE: std::cell::RefCell<HashMap<String, u32>> = Default::default(); }
    if let Some(c) = CACHE.with(|c| c.borrow().get(name).copied()) { return Some(c); }
    let handle = x11rb::cursor::Handle::new(conn, screen, &x11rb::resource_manager::new_from_default(conn).ok()?).ok()?.reply().ok()?;
    let mut c = handle.load_cursor(conn, name).ok()?;
    // x11rb returns None (0) when no theme has the name – unlike libXcursor it has no cursor-font fallback
    // (its own "core" theme path also passes the glyph index without the *2), so do it here
    if c == x11rb::NONE { c = font_cursor(conn, name).unwrap_or(x11rb::NONE); }
    CACHE.with(|cc| cc.borrow_mut().insert(name.to_string(), c));
    Some(c)
}

/// Glyph cursor from the core "cursor" font (indices from <X11/cursorfont.h>).
fn font_cursor(conn: &RustConnection, name: &str) -> Option<u32> {
    let glyph: u16 = match name {
        "left_ptr" => 68, "fleur" => 52, "top_side" => 138, "bottom_side" => 16, "left_side" => 70, "right_side" => 96,
        "top_left_corner" => 134, "top_right_corner" => 136, "bottom_left_corner" => 12, "bottom_right_corner" => 14,
        _ => return None,
    };
    let font = conn.generate_id().ok()?;
    conn.open_font(font, b"cursor").ok()?;
    let c = conn.generate_id().ok()?;
    let _ = conn.create_glyph_cursor(c, font, font, glyph, glyph + 1, 0, 0, 0, 0xffff, 0xffff, 0xffff);
    let _ = conn.close_font(font);
    Some(c)
}
