//! System backends: audio, network, battery, brightness, power, lock.
use crate::util::{output, spawn};

// audio (volume, mute, devices, the serialised writer): src/sys/audio.rs
mod audio;
pub use audio::*;

// ---------------------------------------------------------------- network
/// Split an `nmcli -t` line, honouring `\:` escapes.
pub fn nm_split(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

#[derive(Clone, Debug, Default)]
pub struct NetStatus {
    pub kind: String, // wifi | ethernet | none
    pub name: String,
    pub signal: u32,
}

pub fn net_status() -> NetStatus {
    let Some(s) = output(&["nmcli", "-t", "-f", "TYPE,STATE,CONNECTION", "device"]) else {
        // no NetworkManager: guess from /sys
        let up = std::fs::read_dir("/sys/class/net").map(|rd| rd.flatten().any(|e| {
            e.file_name() != "lo" && std::fs::read_to_string(e.path().join("operstate")).map(|s| s.trim() == "up").unwrap_or(false)
        })).unwrap_or(false);
        return NetStatus { kind: if up { "ethernet".into() } else { "none".into() }, name: if up { "Network".into() } else { String::new() }, signal: 100 };
    };
    for l in s.lines() {
        let f = nm_split(l);
        if f.len() >= 3 && f[1] == "connected" && (f[0] == "wifi" || f[0] == "ethernet") {
            let mut st = NetStatus { kind: f[0].clone(), name: f[2].clone(), signal: 100 };
            if st.kind == "wifi" {
                st.signal = wifi_list().iter().find(|w| w.active).map(|w| w.signal).unwrap_or(70);
            }
            return st;
        }
    }
    NetStatus { kind: "none".into(), ..Default::default() }
}

pub fn net_icon(n: &NetStatus) -> &'static str {
    match n.kind.as_str() {
        "wifi" => match n.signal {
            0..=24 => "network-wireless-signal-weak",
            25..=49 => "network-wireless-signal-ok",
            50..=74 => "network-wireless-signal-good",
            _ => "network-wireless-signal-excellent",
        },
        "ethernet" => "network-wired",
        _ => "network-offline",
    }
}

#[derive(Clone, Debug)]
pub struct Wifi {
    pub active: bool,
    pub ssid: String,
    pub signal: u32,
    pub secure: bool,
}

pub fn wifi_list() -> Vec<Wifi> {
    let Some(s) = output(&["nmcli", "-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"]) else { return vec![] };
    let mut v: Vec<Wifi> = s
        .lines()
        .map(nm_split)
        .filter(|f| f.len() >= 4 && !f[1].is_empty())
        .map(|f| Wifi { active: f[0] == "*", ssid: f[1].clone(), signal: f[2].parse().unwrap_or(0), secure: !f[3].is_empty() && f[3] != "--" })
        .collect();
    v.sort_by(|a, b| b.active.cmp(&a.active).then(b.signal.cmp(&a.signal)));
    v.dedup_by(|a, b| a.ssid == b.ssid);
    v
}

pub fn known_connections() -> Vec<String> {
    output(&["nmcli", "-t", "-f", "NAME", "connection", "show"])
        .map(|s| s.lines().map(|l| nm_split(l)[0].clone()).collect())
        .unwrap_or_default()
}

pub fn wifi_enabled() -> bool {
    output(&["nmcli", "radio", "wifi"]).map(|s| s.trim() == "enabled").unwrap_or(false)
}

// ---------------------------------------------------------------- battery
pub struct Battery {
    pub percent: u32,
    pub status: String,
}

/// The computer's battery as Win7 sees it: every system battery pack together (the composite
/// battery), never a wireless mouse's or keyboard's (power_supply scope "Device").
pub fn battery() -> Option<Battery> {
    let rd = std::fs::read_dir("/sys/class/power_supply").ok()?;
    let read = |p: &std::path::Path, f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
    let packs: Vec<Pack> = rd.flatten().filter_map(|e| {
        let p = e.path();
        if read(&p, "type")? != "Battery" { return None; }
        if !is_system_battery(&e.file_name().to_string_lossy(), read(&p, "scope").as_deref(), &device_subsystem(&p)) { return None; }
        if read(&p, "present").as_deref() == Some("0") { return None; }
        let num = |f: &str| read(&p, f).and_then(|v| v.parse::<u64>().ok());
        let (now, full) = match (num("energy_now"), num("energy_full")) {
            (Some(n), Some(f)) => (Some(n), Some(f)),
            _ => (num("charge_now"), num("charge_full")),
        };
        Some(Pack { capacity: num("capacity").map(|c| c as u32), now, full, status: read(&p, "status").unwrap_or_default() })
    }).collect();
    composite(&packs)
}

/// The subsystem of a power supply's device ("acpi", "hid", "usb", "bluetooth", …).
fn device_subsystem(p: &std::path::Path) -> String {
    std::fs::read_link(p.join("device/subsystem")).ok()
        .and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default()
}

/// Does this power supply power the computer? Only system batteries count (a laptop's ACPI packs);
/// a wireless mouse, keyboard, headset or game controller reports a "Battery" too, often without
/// scope=Device (that depends on the driver), so their bus decides.
pub fn is_system_battery(name: &str, scope: Option<&str>, subsystem: &str) -> bool {
    if scope == Some("Device") { return false; }
    if scope == Some("System") { return true; }
    if matches!(subsystem, "hid" | "usb" | "bluetooth" | "input" | "hidraw" | "i2c" | "serio") { return false; }
    let n = name.to_ascii_lowercase();
    if ["hid", "hidpp", "ps-controller", "sony_controller", "wacom", "xpadneo", "gip", "nintendo", "steam", "logitech", "apple-", "magicmouse"]
        .iter().any(|p| n.starts_with(p)) { return false; }
    true
}

#[derive(Debug, Clone, Default)]
pub struct Pack { pub capacity: Option<u32>, pub now: Option<u64>, pub full: Option<u64>, pub status: String }

/// Several packs as one battery: charge summed over capacity (average capacity when a pack has no
/// energy/charge readings); discharging when any pack is, charging when any is, full when all are.
pub fn composite(packs: &[Pack]) -> Option<Battery> {
    if packs.is_empty() { return None; }
    let all_measured = packs.iter().all(|p| p.now.is_some() && p.full.map(|f| f > 0).unwrap_or(false));
    let percent = if all_measured {
        let (n, f) = packs.iter().fold((0u64, 0u64), |a, p| (a.0 + p.now.unwrap(), a.1 + p.full.unwrap()));
        ((n * 100 + f / 2) / f).min(100) as u32
    } else {
        let caps: Vec<u32> = packs.iter().filter_map(|p| p.capacity).collect();
        if caps.is_empty() { return None; }
        caps.iter().sum::<u32>() / caps.len() as u32
    };
    let any = |s: &str| packs.iter().any(|p| p.status == s);
    let status = if any("Discharging") { "Discharging" } else if any("Charging") { "Charging" }
        else if packs.iter().all(|p| p.status == "Full") { "Full" } else { packs[0].status.as_str() };
    Some(Battery { percent, status: status.to_string() })
}

#[cfg(test)]
mod battery_tests {
    use super::*;
    fn pack(now: u64, full: u64, status: &str) -> Pack { Pack { capacity: Some((now * 100 / full) as u32), now: Some(now), full: Some(full), status: status.into() } }
    #[test]
    fn two_packs_are_one_battery() {
        let b = composite(&[pack(10, 50, "Discharging"), pack(40, 50, "Unknown")]).unwrap();
        assert_eq!((b.percent, b.status.as_str()), (50, "Discharging"));
    }
    #[test]
    fn capacity_only() {
        let b = composite(&[Pack { capacity: Some(30), status: "Charging".into(), ..Default::default() }]).unwrap();
        assert_eq!((b.percent, b.status.as_str()), (30, "Charging"));
    }
    #[test]
    fn none_without_packs() { assert!(composite(&[]).is_none()); }
    #[test]
    fn peripherals_are_not_the_computers_battery() {
        assert!(is_system_battery("BAT0", None, "acpi"));
        assert!(is_system_battery("BAT1", Some("System"), ""));
        assert!(!is_system_battery("hidpp_battery_0", None, "hid"));
        assert!(!is_system_battery("ps-controller-battery-aa:bb", None, ""));
        assert!(!is_system_battery("hid-00:11:22:33:44:55-battery", None, ""));
        assert!(!is_system_battery("BAT0", Some("Device"), "acpi"));
    }
}

/// Icon names for the battery, best first: the 10 %-step Win7 icons (built from batmeter.dll by
/// aerode-extract), then the classic freedesktop names other themes have.
pub fn battery_icons(b: &Battery) -> [String; 2] {
    let charging = b.status == "Charging" || b.status == "Full";
    let suffix = if charging { "-charging" } else { "" };
    let step = ((b.percent.min(100) + 5) / 10 * 10).max(if b.percent > 0 { 10 } else { 0 });
    let lvl = match b.percent { 0..=10 => "caution", 11..=30 => "low", 31..=80 => "good", _ => "full" };
    [format!("battery-level-{step}{suffix}"), format!("battery-{lvl}{suffix}")]
}

pub fn battery_icon(b: &Battery) -> String {
    use gtk::prelude::IconThemeExt;
    let [a, c] = battery_icons(b);
    if gtk::IconTheme::default().map(|t| t.has_icon(&a)).unwrap_or(false) { a } else { c }
}

pub fn power_profile() -> Option<String> {
    output(&["powerprofilesctl", "get"]).map(|s| s.trim().to_string())
}

pub fn set_power_profile(p: &str) {
    spawn(&["powerprofilesctl", "set", p]);
}

// ---------------------------------------------------------------- brightness
/// Laptop panel backlight (brightnessctl), as a percentage.
pub fn backlight() -> Option<u32> {
    if !std::path::Path::new("/sys/class/backlight").read_dir().map(|mut r| r.next().is_some()).unwrap_or(false) { return None; }
    let cur: f64 = output(&["brightnessctl", "get"])?.trim().parse().ok()?;
    let max: f64 = output(&["brightnessctl", "max"])?.trim().parse().ok()?;
    Some((cur * 100.0 / max).round() as u32)
}

/// External monitors that answer DDC/CI: (display number, model name).
pub fn ddc_displays() -> Vec<(u32, String)> {
    let Some(s) = output(&["ddcutil", "detect", "--brief"]) else { return vec![] };
    let mut v = vec![];
    let mut cur: Option<u32> = None;
    for l in s.lines() {
        let t = l.trim();
        if let Some(n) = t.strip_prefix("Display ") { cur = n.trim().parse().ok(); }
        else if t.starts_with("Invalid display") { cur = None; }
        else if let (Some(n), Some(m)) = (cur, t.strip_prefix("Monitor:")) {
            // "Monitor: DEL:DELL U2415:XXXX" → "DELL U2415"
            let model = m.trim().split(':').nth(1).unwrap_or(m.trim()).trim().to_string();
            v.push((n, model));
            cur = None;
        }
    }
    v
}

/// Brightness (VCP feature 0x10) of a DDC display: (current, max).
pub fn ddc_get(display: u32) -> Option<(u32, u32)> {
    let s = output(&["ddcutil", "--display", &display.to_string(), "getvcp", "10", "--brief"])?;
    let f: Vec<&str> = s.split_whitespace().collect();
    // "VCP 10 C <cur> <max>"
    Some((f.get(3)?.parse().ok()?, f.get(4)?.parse().ok()?))
}

pub fn ddc_set(display: u32, pct: u32) {
    let d = display.to_string();
    let v = pct.min(100).to_string();
    std::thread::spawn(move || { let _ = std::process::Command::new("ddcutil").args(["--display", &d, "--noverify", "setvcp", "10", &v]).status(); });
}

/// Brightness of the "main" screen: laptop backlight, else the first DDC monitor.
pub fn brightness() -> Option<u32> {
    backlight().or_else(|| {
        let (n, _) = ddc_displays().into_iter().next()?;
        let (cur, max) = ddc_get(n)?;
        Some(cur * 100 / max.max(1))
    })
}

/// Set every screen we can control.
pub fn set_brightness(pct: u32) {
    if backlight().is_some() { spawn(&["brightnessctl", "set", &format!("{}%", pct.clamp(1, 100))]); }
    for (n, _) in ddc_displays() { ddc_set(n, pct); }
}

// ---------------------------------------------------------------- power
/// End the session the Win7 way: the Log off / Exit Windows sound plays to its end (bounded by
/// `sounds::play_sync`), then the session goes away. On a worker thread, so the caller's UI keeps
/// painting meanwhile.
fn end_session(ev: crate::sounds::Event, then: fn()) {
    std::thread::spawn(move || { crate::sounds::play_sync(ev); then(); });
}
pub fn shutdown() { end_session(crate::sounds::Event::ExitWindows, || { spawn(&["systemctl", "poweroff"]); }); }
pub fn restart() { end_session(crate::sounds::Event::ExitWindows, || { spawn(&["systemctl", "reboot"]); }); }
/// Power Options > "Require a password on wakeup": lock before sleeping / hibernating (silently:
/// Win7 skips the lock sound when the lock comes from suspending).
fn lock_for_wakeup() {
    if crate::config::get().hw.power.require_password {
        let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aerode".into());
        spawn(&[&exe, "lock", "--sleep"]);
    }
}
pub fn sleep() { lock_for_wakeup(); spawn(&["systemctl", "suspend"]); }
pub fn hibernate() { lock_for_wakeup(); spawn(&["systemctl", "hibernate"]); }
pub fn log_off() {
    end_session(crate::sounds::Event::Logoff, || {
        // ending the window manager ends the session (it is the session leader)
        if !crate::util::signal_on_display("aerode-wm", None, libc::SIGTERM) {
            spawn(&["loginctl", "terminate-session", &std::env::var("XDG_SESSION_ID").unwrap_or_default()]);
        }
    });
}
pub fn lock() {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aerode".into());
    spawn(&[&exe, "lock"]);
}
