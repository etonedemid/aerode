//! Global keyboard shortcuts (Windows 7 set + a few Plasma extras), checked ahead of the clients.
use smithay::input::keyboard::{keysyms as ks, ModifiersState};

#[derive(Clone, Debug)]
pub enum Action {
    Spawn(String),
    Close, Maximize, RestoreOrMinimize, SnapLeft, SnapRight, ShowDesktop, MinimizeOthers,
    /// Win+Shift+Up: stretch to the top and bottom of the screen; Win+Shift+Left/Right: next monitor
    MaximizeVert, MonitorLeft, MonitorRight,
    SwitchNext, SwitchPrev, SwitchCommit, SwitchCancel,
    DesktopLeft, DesktopRight, DesktopAdd, DesktopRemove,
    SysMenu,
    /// Ctrl+Alt+Fn: go to another virtual terminal
    Vt(i32),
}

const SHIFT: u8 = 1;
const CTRL: u8 = 2;
const ALT: u8 = 4;
const SUPER: u8 = 8;

fn bindings() -> Vec<(u8, u32, Action)> {
    use Action::*;
    let s = |c: &str| Spawn(c.to_string());
    let mut v = vec![
        (ALT, ks::KEY_Tab, SwitchNext), (ALT | SHIFT, ks::KEY_Tab, SwitchPrev),
        (ALT | SHIFT, ks::KEY_ISO_Left_Tab, SwitchPrev),
        (SUPER, ks::KEY_Tab, SwitchNext), (SUPER | SHIFT, ks::KEY_Tab, SwitchPrev),
        (SUPER | SHIFT, ks::KEY_ISO_Left_Tab, SwitchPrev),
        (ALT, ks::KEY_F4, Close), (ALT, ks::KEY_space, SysMenu),
        (SUPER, ks::KEY_Up, Maximize), (SUPER, ks::KEY_Down, RestoreOrMinimize),
        (SUPER, ks::KEY_Left, SnapLeft), (SUPER, ks::KEY_Right, SnapRight),
        (SUPER | SHIFT, ks::KEY_Up, MaximizeVert), (SUPER | SHIFT, ks::KEY_Down, RestoreOrMinimize),
        (SUPER | SHIFT, ks::KEY_Left, MonitorLeft), (SUPER | SHIFT, ks::KEY_Right, MonitorRight),
        (SUPER, ks::KEY_d, ShowDesktop), (SUPER, ks::KEY_m, ShowDesktop), (SUPER, ks::KEY_Home, MinimizeOthers),
        (SUPER | CTRL, ks::KEY_Left, DesktopLeft), (SUPER | CTRL, ks::KEY_Right, DesktopRight),
        (SUPER | CTRL, ks::KEY_d, DesktopAdd), (SUPER | CTRL, ks::KEY_F4, DesktopRemove),
        (SUPER, ks::KEY_e, s("aerode-explorer")), (SUPER, ks::KEY_r, s("aerode run")),
        (SUPER, ks::KEY_l, s("aerode lock")), (SUPER, ks::KEY_p, s("aerode-control display")),
        (SUPER, ks::KEY_f, s("aerode start")), (SUPER, ks::KEY_Pause, s("aerode-control system")),
        (SUPER | SHIFT, ks::KEY_s, s("aerode-snip")), (0, ks::KEY_Print, s("aerode-snip --full")),
        (CTRL | SHIFT, ks::KEY_Escape, s("aerode-taskmgr")), (CTRL | ALT, ks::KEY_Delete, s("aerode-taskmgr")),
        (CTRL, ks::KEY_Escape, s("aerode start")),
        // volume keys as Win7's SndVolSSO handles APPCOMMAND_VOLUME_UP/DOWN/MUTE and
        // APPCOMMAND_MICROPHONE_VOLUME_MUTE: +-2 % from the loudest channel (balance kept, a level
        // above 100 % never lowered by Up), unmuting first; serialised with the shell's own changes
        (0, ks::KEY_XF86AudioRaiseVolume, s("aerode volume-step up")),
        (0, ks::KEY_XF86AudioLowerVolume, s("aerode volume-step down")),
        (0, ks::KEY_XF86AudioMute, s("aerode audio-mute")),
        (0, ks::KEY_XF86AudioMicMute, s("aerode mic-mute")),
        (0, ks::KEY_XF86AudioPlay, s("playerctl play-pause")), (0, ks::KEY_XF86AudioNext, s("playerctl next")), (0, ks::KEY_XF86AudioPrev, s("playerctl previous")),
        (0, ks::KEY_XF86MonBrightnessUp, s("brightnessctl set 5%+")), (0, ks::KEY_XF86MonBrightnessDown, s("brightnessctl set 5%-")),
        (0, ks::KEY_XF86Calculator, s("aerode-calc")), (0, ks::KEY_XF86Explorer, s("aerode-explorer")),
    ];
    // Win+1..9 handled by the shell (taskbar launch); register them as commands
    for i in 1..=9u32 { v.push((SUPER, ks::KEY_0 + i, s(&format!("aerode taskbar-launch {i}")))); }
    v
}

pub struct Keys {
    map: Vec<(u8, u32, Action)>,
    super_down_alone: bool,
}

impl Default for Keys { fn default() -> Keys { Keys { map: bindings(), super_down_alone: false } } }

fn is_super(s: u32) -> bool { s == ks::KEY_Super_L || s == ks::KEY_Super_R }
fn is_alt(s: u32) -> bool { s == ks::KEY_Alt_L || s == ks::KEY_Alt_R }

impl Keys {
    /// What a key does. `sym` is the Latin keysym (so Win+E works on a Russian layout), `modified` the
    /// keysym of the current group. An action returned on press means the client must not see the key.
    pub fn lookup(&mut self, sym: u32, modified: u32, m: &ModifiersState, press: bool, repeat: bool, switching: bool) -> Option<Action> {
        let mods = (m.shift as u8 * SHIFT) | (m.ctrl as u8 * CTRL) | (m.alt as u8 * ALT) | (m.logo as u8 * SUPER);
        // XF86Switch_VT_n comes from the Ctrl+Alt+Fn level of the keymap
        if press && (ks::KEY_XF86Switch_VT_1..=ks::KEY_XF86Switch_VT_12).contains(&modified) {
            return Some(Action::Vt((modified - ks::KEY_XF86Switch_VT_1 + 1) as i32));
        }
        if is_super(sym) {
            if press { self.super_down_alone = !switching; return None; }
            if switching { self.super_down_alone = false; return Some(Action::SwitchCommit); }
            let fire = self.super_down_alone && mods & !SUPER == 0;
            self.super_down_alone = false;
            return fire.then(|| Action::Spawn("aerode start".into()));
        }
        if !press {
            // releasing Alt ends the window switcher
            return (switching && is_alt(sym)).then_some(Action::SwitchCommit);
        }
        self.super_down_alone = false;
        if sym == ks::KEY_Escape && switching { return Some(Action::SwitchCancel); }
        if switching && (sym == ks::KEY_Left || sym == ks::KEY_Right) {
            return Some(if sym == ks::KEY_Left { Action::SwitchPrev } else { Action::SwitchNext });
        }
        let act = self.map.iter().find(|b| b.0 == mods && (b.1 == sym || b.1 == modified)).map(|b| b.2.clone())?;
        // holding a window-management chord must not run it again on whatever window got focus next
        // (Win+Down held would minimize window after window); switching, desktops and media keys repeat
        if repeat && !matches!(act, Action::SwitchNext | Action::SwitchPrev | Action::DesktopLeft | Action::DesktopRight)
            && !matches!(&act, Action::Spawn(c) if c.contains("volume") || c.contains("brightness")) { return None; }
        Some(act)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(alt: bool, logo: bool) -> ModifiersState { ModifiersState { alt, logo, ..Default::default() } }

    #[test]
    fn super_alone_opens_start_but_not_after_a_chord() {
        let mut k = Keys::default();
        assert!(k.lookup(ks::KEY_Super_L, ks::KEY_Super_L, &m(false, true), true, false, false).is_none());
        assert!(matches!(k.lookup(ks::KEY_Super_L, ks::KEY_Super_L, &m(false, true), false, false, false), Some(Action::Spawn(c)) if c == "aerode start"));
        k.lookup(ks::KEY_Super_L, ks::KEY_Super_L, &m(false, true), true, false, false);
        assert!(matches!(k.lookup(ks::KEY_e, ks::KEY_e, &m(false, true), true, false, false), Some(Action::Spawn(_))));
        assert!(k.lookup(ks::KEY_Super_L, ks::KEY_Super_L, &m(false, true), false, false, false).is_none());
    }

    #[test]
    fn repeat_only_for_switching_and_volume() {
        let mut k = Keys::default();
        assert!(k.lookup(ks::KEY_Down, ks::KEY_Down, &m(false, true), true, true, false).is_none());
        assert!(matches!(k.lookup(ks::KEY_Tab, ks::KEY_Tab, &m(true, false), true, true, false), Some(Action::SwitchNext)));
    }
}
