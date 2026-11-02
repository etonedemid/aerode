//! Power Options backend shared by Control Panel and the shell: the Windows 7 power plans (mapped
//! onto power-profiles-daemon when it is installed), the per-plan "Turn off the display" and
//! "Put the computer to sleep" timers on battery / plugged in, and the shell's watcher that applies
//! them (`start`): DPMS for the display, the X idle time + logind inhibitors for sleep.
use crate::config::{self, PowerOptions, PowerPlan};
use crate::{sys, util};

/// The built-in plans: (id = power profile, name, description).
pub const PLANS: [(&str, &str, &str); 3] = [
    ("balanced", "Balanced", "Automatically balances performance with energy consumption on capable hardware."),
    ("power-saver", "Power saver", "Saves energy by reducing your computer's performance where possible."),
    ("performance", "High performance", "Favors performance, but may use more energy."),
];

/// Win7's choices for the display and sleep timers: (minutes, label), 0 = Never.
pub const TIMEOUTS: &[(u32, &str)] = &[
    (1, "1 minute"), (2, "2 minutes"), (3, "3 minutes"), (5, "5 minutes"), (10, "10 minutes"), (15, "15 minutes"),
    (20, "20 minutes"), (25, "25 minutes"), (30, "30 minutes"), (45, "45 minutes"), (60, "1 hour"), (120, "2 hours"),
    (180, "3 hours"), (240, "4 hours"), (300, "5 hours"), (0, "Never"),
];

/// The Windows 7 defaults of a built-in plan (Balanced 5/10 min display, 15/30 min sleep …).
pub fn defaults(base: &str) -> PowerPlan {
    match base {
        "power-saver" => PowerPlan { display_dc: 2, display_ac: 5, sleep_dc: 10, sleep_ac: 15, bright_dc: 40, bright_ac: 75 },
        "performance" => PowerPlan { display_dc: 10, display_ac: 15, sleep_dc: 0, sleep_ac: 0, bright_dc: 75, bright_ac: 100 },
        _ => PowerPlan { display_dc: 5, display_ac: 10, sleep_dc: 15, sleep_ac: 30, bright_dc: 40, bright_ac: 100 },
    }
}

/// The power profile a plan stands for (custom plans: the plan they were based on).
pub fn base_of(o: &PowerOptions, id: &str) -> String {
    if PLANS.iter().any(|p| p.0 == id) { return id.to_string(); }
    o.custom.iter().find(|c| c.id == id).map(|c| c.base.clone()).unwrap_or_else(|| "balanced".into())
}

pub fn plan_name(o: &PowerOptions, id: &str) -> String {
    PLANS.iter().find(|p| p.0 == id).map(|p| p.1.to_string())
        .or_else(|| o.custom.iter().find(|c| c.id == id).map(|c| c.name.clone()))
        .unwrap_or_else(|| "Balanced".into())
}

/// A plan's settings (the user's changes, else the defaults of the plan it is based on).
pub fn settings(o: &PowerOptions, id: &str) -> PowerPlan {
    o.plans.get(id).copied().unwrap_or_else(|| defaults(&base_of(o, id)))
}

/// The active power profile of power-profiles-daemon (None when it is not running).
pub fn profile() -> Option<String> {
    for (name, path) in [("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"), ("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles")] {
        if let Some(s) = util::output(&["busctl", "--system", "get-property", name, path, name, "ActiveProfile"]) {
            // s "balanced"
            if let Some(p) = s.split('"').nth(1) { return Some(p.to_string()); }
        }
    }
    None
}

/// The active plan: the saved one, unless the power profile was switched elsewhere (battery
/// flyout, another tool) to one the saved plan isn't based on. Blocking (asks D-Bus).
pub fn active_plan(o: &PowerOptions) -> String {
    match profile() {
        Some(p) if base_of(o, &o.plan) != p && PLANS.iter().any(|x| x.0 == p) => p,
        _ => o.plan.clone(),
    }
}

/// Make a plan active: remember it and switch the power profile.
pub fn set_plan(id: &str) {
    config::update(|c| c.hw.power.plan = id.to_string());
    let base = base_of(&config::get().hw.power, id);
    if util::which("powerprofilesctl") { sys::set_power_profile(&base); }
    apply_now();
}

pub fn has_battery() -> bool { sys::battery().is_some() }

/// Running on battery: a battery exists and no mains/USB supply is online (without such a
/// supply in sysfs, the battery discharging).
pub fn on_battery() -> bool {
    let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else { return false };
    let (mut battery, mut mains, mut online, mut discharging) = (false, false, false, false);
    for e in rd.flatten() {
        let p = e.path();
        let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        match ty.trim() {
            "Battery" => {
                if std::fs::read_to_string(p.join("scope")).map(|s| s.trim() == "Device").unwrap_or(false) { continue; } // mice, headsets
                battery = true;
                discharging |= std::fs::read_to_string(p.join("status")).map(|s| s.trim() == "Discharging").unwrap_or(false);
            }
            "Mains" | "USB" | "USB_C" | "USB_PD" => {
                mains = true;
                online |= std::fs::read_to_string(p.join("online")).map(|s| s.trim() == "1").unwrap_or(false);
            }
            _ => {}
        }
    }
    battery && if mains { !online } else { discharging }
}

/// (display off, sleep) in minutes for the active plan and power source.
pub fn timers(o: &PowerOptions, plan: &str, battery: bool) -> (u32, u32) {
    let p = settings(o, plan);
    if battery { (p.display_dc, p.sleep_dc) } else { (p.display_ac, p.sleep_ac) }
}

/// Turn the display off after `minutes` of inactivity (0 = never) and remember it for the next
/// logon and for the screen saver (~/.config/aerode/display-off, seconds).
pub fn apply_display(minutes: u32) {
    let secs = (minutes * 60).to_string();
    if minutes == 0 { util::spawn(&["xset", "-dpms"]); } else { util::spawn(&["xset", "+dpms", "dpms", "0", "0", &secs]); }
    let _ = std::fs::create_dir_all(config::conf_dir());
    let _ = std::fs::write(config::conf_dir().join("display-off"), &secs);
}

/// Apply the display timer of the saved plan straight away (Control Panel after Save changes).
pub fn apply_now() {
    let o = config::get().hw.power;
    let (d, _) = timers(&o, &o.plan, on_battery());
    apply_display(d);
}

/// Is going to sleep blocked by a program (logind "idle" or "sleep" inhibitor in block mode)?
fn inhibited() -> bool {
    let Some(s) = util::output(&["busctl", "--system", "call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "ListInhibitors"]) else { return false };
    // a(ssssuu) N "what" "who" "why" "mode" uid pid …: the quoted strings come in fours
    let strs: Vec<&str> = s.split('"').skip(1).step_by(2).collect();
    strs.chunks(4).any(|c| c.len() == 4 && c[3] == "block" && (c[0].contains("idle") || c[0].contains("sleep")))
}

/// Only a real logon session (started by the display manager, with logind allowing it) puts the
/// computer to sleep by itself – never a nested or test display (Xephyr, Xvfb) inside another session.
fn may_sleep() -> bool {
    std::env::var_os("XDG_SESSION_ID").is_some()
        && util::output(&["busctl", "--system", "call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "CanSuspend"])
            .map(|s| s.contains("\"yes\"")).unwrap_or(false)
}

/// Milliseconds since the last keyboard or mouse input (MIT-SCREEN-SAVER).
fn idle_ms(conn: &impl x11rb::connection::Connection, root: u32) -> Option<u32> {
    use x11rb::protocol::screensaver::ConnectionExt;
    conn.screensaver_query_info(root).ok()?.reply().ok().map(|r| r.ms_since_user_input)
}

/// The shell's power watcher: follows plan and power-source changes (display timer, plan
/// brightness) and puts the computer to sleep after the plan's idle time.
pub fn start() {
    std::thread::spawn(|| {
        use x11rb::connection::Connection;
        let x = x11rb::connect(None).ok().filter(|_| may_sleep());
        let mut last_display: Option<u32> = None;
        let mut last_state = String::new();
        // set when we sent the computer to sleep; cleared by new input (the idle time keeps counting
        // across a suspend, so without this it would go straight back to sleep after waking up)
        let mut slept = false;
        loop {
            config::reload();
            let o = config::get().hw.power;
            let battery = on_battery();
            let plan = active_plan(&o);
            let (display, sleep) = timers(&o, &plan, battery);
            if last_display != Some(display) {
                apply_display(display);
                last_display = Some(display);
            }
            // Win7 applies the plan's brightness when the plan or the power source changes
            let state = format!("{plan}/{battery}");
            if state != last_state {
                if !last_state.is_empty() && sys::backlight().is_some() {
                    let p = settings(&o, &plan);
                    let b = if battery { p.bright_dc } else { p.bright_ac };
                    if b > 0 { util::spawn(&["brightnessctl", "set", &format!("{}%", b.clamp(1, 100))]); }
                }
                last_state = state;
            }
            if let Some((conn, screen)) = x.as_ref() {
                let root = conn.setup().roots[*screen].root;
                if let Some(idle) = idle_ms(conn, root) {
                    if sleep > 0 && idle as u64 >= sleep as u64 * 60_000 {
                        if !slept && !inhibited() {
                            slept = true;
                            sys::sleep();
                        }
                    } else if idle < 60_000 {
                        slept = false;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(15));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_defaults_and_custom() {
        let mut o = PowerOptions::default();
        assert_eq!(timers(&o, "balanced", true), (5, 15));
        assert_eq!(timers(&o, "balanced", false), (10, 30));
        assert_eq!(timers(&o, "performance", false), (15, 0));
        o.custom.push(config::CustomPlan { id: "custom-1".into(), name: "My Custom Plan 1".into(), base: "power-saver".into() });
        assert_eq!(base_of(&o, "custom-1"), "power-saver");
        assert_eq!(timers(&o, "custom-1", true), (2, 10));
        assert_eq!(plan_name(&o, "custom-1"), "My Custom Plan 1");
        o.plans.insert("custom-1".into(), PowerPlan { display_dc: 1, ..defaults("power-saver") });
        assert_eq!(timers(&o, "custom-1", true), (1, 10));
    }
}
