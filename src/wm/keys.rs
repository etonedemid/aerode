//! Global keyboard shortcuts (Windows 7 set + a few Plasma extras).
use std::collections::HashMap;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

#[derive(Clone, Debug)]
pub enum Action {
    Spawn(String),
    Close, Maximize, RestoreOrMinimize, SnapLeft, SnapRight, ShowDesktop, MinimizeOthers,
    SwitchNext, SwitchPrev, SwitchCommit, SwitchCancel,
    DesktopLeft, DesktopRight, DesktopAdd, DesktopRemove,
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
        (ALT, XK_F4, Close),
        (SUPER, XK_UP, Maximize), (SUPER, XK_DOWN, RestoreOrMinimize),
        (SUPER, XK_LEFT, SnapLeft), (SUPER, XK_RIGHT, SnapRight),
        (SUPER, 'd' as u32, ShowDesktop), (SUPER, 'm' as u32, ShowDesktop), (SUPER, XK_HOME, MinimizeOthers),
        (SUPER | CTRL, XK_LEFT, DesktopLeft), (SUPER | CTRL, XK_RIGHT, DesktopRight),
        (SUPER | CTRL, 'd' as u32, DesktopAdd), (SUPER | CTRL, XK_F4, DesktopRemove),
        (SUPER, 'e' as u32, s("aero7-explorer")), (SUPER, 'r' as u32, s("aero7 run")),
        (SUPER, 'l' as u32, s("aero7 lock")), (SUPER, 'p' as u32, s("aero7-control display")),
        (SUPER, 'f' as u32, s("aero7 start")), (SUPER, XK_PAUSE, s("aero7-control system")),
        (SUPER | SHIFT, 's' as u32, s("aero7-snip")), (0, XK_PRINT, s("aero7-snip --full")),
        (CTRL | SHIFT, XK_ESCAPE, s("aero7-taskmgr")), (CTRL | ALT, XK_DELETE, s("aero7-taskmgr")),
        (CTRL, XK_ESCAPE, s("aero7 start")),
        (0, 0x1008ff13, s("wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 2%+ || pactl set-sink-volume @DEFAULT_SINK@ +2%")),
        (0, 0x1008ff11, s("wpctl set-volume @DEFAULT_AUDIO_SINK@ 2%- || pactl set-sink-volume @DEFAULT_SINK@ -2%")),
        (0, 0x1008ff12, s("wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle || pactl set-sink-mute @DEFAULT_SINK@ toggle")),
        (0, 0x1008ff14, s("playerctl play-pause")), (0, 0x1008ff17, s("playerctl next")), (0, 0x1008ff16, s("playerctl previous")),
        (0, 0x1008ff02, s("brightnessctl set 5%+")), (0, 0x1008ff03, s("brightnessctl set 5%-")),
        (0, 0x1008ff1d, s("aero7-calc")), (0, 0x1008ff5d, s("aero7-explorer")),
    ];
    // Win+1..9 handled by the shell (taskbar launch); register them as commands
    for i in 1..=9u32 { v.push((SUPER, '0' as u32 + i, s(&format!("aero7 taskbar-launch {i}")))); }
    v
}

#[derive(Default)]
pub struct Keys {
    map: HashMap<(u8, u16), Action>,
    sym_of: HashMap<u8, u32>,
    super_codes: Vec<u8>,
    super_down_alone: bool,
    alt_codes: Vec<u8>,
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

    pub fn lookup(&mut self, code: u8, state: u16, press: bool, conn: &RustConnection) -> Option<Action> {
        let mods = state & (SHIFT | CTRL | ALT | SUPER);
        if self.super_codes.contains(&code) {
            if press { self.super_down_alone = true; let _ = conn.allow_events(Allow::ASYNC_KEYBOARD, x11rb::CURRENT_TIME); return None; }
            let fire = self.super_down_alone && mods & !SUPER == 0;
            self.super_down_alone = false;
            return fire.then(|| Action::Spawn("aero7 start".into()));
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
        self.map.get(&(code, mods)).cloned()
    }
}

/// Load a named cursor via the cursor font fallback (left_ptr etc).
pub fn cursor(conn: &RustConnection, screen: usize, name: &str) -> Option<u32> {
    thread_local! { static CACHE: std::cell::RefCell<HashMap<String, u32>> = Default::default(); }
    if let Some(c) = CACHE.with(|c| c.borrow().get(name).copied()) { return Some(c); }
    let handle = x11rb::cursor::Handle::new(conn, screen, &x11rb::resource_manager::new_from_default(conn).ok()?).ok()?.reply().ok()?;
    let c = handle.load_cursor(conn, name).ok()?;
    CACHE.with(|cc| cc.borrow_mut().insert(name.to_string(), c));
    Some(c)
}
