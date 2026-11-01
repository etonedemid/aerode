//! System backends: audio, network, battery, brightness, power, lock.
use crate::util::{output, spawn, which};

// ---------------------------------------------------------------- audio
pub fn volume() -> Option<(u32, bool)> {
    if let Some(s) = output(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"]) {
        let v: f64 = s.split_whitespace().nth(1)?.parse().ok()?;
        return Some(((v * 100.0).round() as u32, s.contains("MUTED")));
    }
    let s = output(&["pactl", "get-sink-volume", "@DEFAULT_SINK@"])?;
    let pct = s.split('/').nth(1)?.trim().trim_end_matches('%').parse().ok()?;
    let m = output(&["pactl", "get-sink-mute", "@DEFAULT_SINK@"]).map(|m| m.contains("yes")).unwrap_or(false);
    Some((pct, m))
}

pub fn set_volume(pct: u32) {
    let pct = pct.min(100);
    if which("wpctl") {
        spawn(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", pct as f64 / 100.0)]);
    } else {
        spawn(&["pactl", "set-sink-volume", "@DEFAULT_SINK@", &format!("{pct}%")]);
    }
}

pub fn toggle_mute() {
    if which("wpctl") {
        spawn(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
    } else {
        spawn(&["pactl", "set-sink-mute", "@DEFAULT_SINK@", "toggle"]);
    }
}

pub fn volume_icon(v: Option<(u32, bool)>) -> &'static str {
    match v {
        None => "audio-volume-muted",
        Some((_, true)) | Some((0, _)) => "audio-volume-muted",
        Some((p, _)) if p < 34 => "audio-volume-low",
        Some((p, _)) if p < 67 => "audio-volume-medium",
        _ => "audio-volume-high",
    }
}

/// Output devices: (id, description, is_default)
pub fn sinks() -> Vec<(String, String, bool)> {
    let def = output(&["pactl", "get-default-sink"]).unwrap_or_default().trim().to_string();
    let Some(s) = output(&["pactl", "list", "sinks"]) else { return vec![] };
    let mut out = vec![];
    let mut name = String::new();
    for l in s.lines() {
        let l = l.trim();
        if let Some(n) = l.strip_prefix("Name: ") {
            name = n.to_string();
        } else if let Some(d) = l.strip_prefix("Description: ") {
            out.push((name.clone(), d.to_string(), name == def));
        }
    }
    out
}

pub fn set_default_sink(name: &str) {
    spawn(&["pactl", "set-default-sink", name]);
}

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

pub fn battery() -> Option<Battery> {
    let rd = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for e in rd.flatten() {
        let p = e.path();
        let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        if ty.trim() == "Battery" {
            let percent = std::fs::read_to_string(p.join("capacity")).ok()?.trim().parse().ok()?;
            let status = std::fs::read_to_string(p.join("status")).unwrap_or_default().trim().to_string();
            return Some(Battery { percent, status });
        }
    }
    None
}

pub fn battery_icon(b: &Battery) -> String {
    let lvl = match b.percent { 0..=10 => "caution", 11..=30 => "low", 31..=80 => "good", _ => "full" };
    if b.status == "Charging" || b.status == "Full" { format!("battery-{lvl}-charging") } else { format!("battery-{lvl}") }
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
/// Play an event sound to completion (used before the session goes away).
fn play_blocking(ev: crate::sounds::Event) {
    if let Some(f) = crate::sounds::file_for(ev) {
        for pl in ["pw-play", "paplay", "aplay"] {
            if which(pl) { let _ = std::process::Command::new(pl).arg(&f).status(); return; }
        }
    }
}
pub fn shutdown() { play_blocking(crate::sounds::Event::ExitWindows); spawn(&["systemctl", "poweroff"]); }
pub fn restart() { play_blocking(crate::sounds::Event::ExitWindows); spawn(&["systemctl", "reboot"]); }
pub fn sleep() { lock(); spawn(&["systemctl", "suspend"]); }
pub fn hibernate() { lock(); spawn(&["systemctl", "hibernate"]); }
pub fn log_off() {
    play_blocking(crate::sounds::Event::Logoff);
    // ending the window manager ends the session (it is the session leader)
    if !spawn(&["pkill", "-TERM", "-x", "aerode-wm"]) {
        spawn(&["loginctl", "terminate-session", &std::env::var("XDG_SESSION_ID").unwrap_or_default()]);
    }
}
pub fn lock() {
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "aerode".into());
    spawn(&[&exe, "lock"]);
}
