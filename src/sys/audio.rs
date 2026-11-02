//! Audio backend: the default output device's level and mute, every output device, the programs
//! playing sound (for the Volume Mixer), and one serialised writer.
//!
//! Win7's tray icon, flyout and mixer talk to the endpoint in-process (IAudioEndpointVolume) and
//! never freeze when the audio service is slow. Here every sound-server call is a `pactl` (or
//! `wpctl`) child with a deadline, and the GUI never runs one itself: reads and writes go through
//! [`send`] / [`query_async`] to one worker thread, which applies them in order, drops stale slider
//! positions and answers reads after the writes before them. Short-lived helpers (`aerode
//! volume-step` for the volume keys) use the blocking functions and a lock file, so a key press and
//! a slider drag can never interleave their read-modify-write.
//!
//! The level is Windows' master volume: the loudest channel. Setting it scales every channel by
//! the same factor (pa_cvolume_scale), so the balance set under Sound > Properties > Levels
//! survives the slider, the wheel and the volume keys (`wpctl set-volume` flattens it).
use crate::util::which;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Longest a sound-server command may take: a stalled server (Bluetooth profile switch, resume,
/// a hung driver) costs a caller at most this, never a frozen taskbar.
const DEADLINE: Duration = Duration::from_secs(2);
/// pactl's unit: 0x10000 = 100 % (PA_VOLUME_NORM).
const NORM: u64 = 65536;

/// Run a command with a deadline (killed when it passes). Ok(stdout) on success,
/// Err(true) when it timed out, Err(false) when it failed or is not installed.
fn run(argv: &[&str]) -> Result<String, bool> {
    use std::io::Read;
    if argv.is_empty() || !which(argv[0]) { return Err(false); }
    let mut child = std::process::Command::new(argv[0]).args(&argv[1..])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| false)?;
    let Some(mut out) = child.stdout.take() else { let _ = child.kill(); let _ = child.wait(); return Err(false) };
    // read in a helper thread: a big `list` output must not fill the pipe while we wait
    let reader = std::thread::spawn(move || { let mut v = Vec::new(); let _ = out.read_to_end(&mut v); v });
    let start = Instant::now();
    let mut nap = Duration::from_millis(1);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let v = reader.join().unwrap_or_default();
                return if st.success() { Ok(String::from_utf8_lossy(&v).into_owned()) } else { Err(false) };
            }
            Ok(None) if start.elapsed() < DEADLINE => { std::thread::sleep(nap); nap = (nap * 2).min(Duration::from_millis(15)); }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(true);
            }
        }
    }
}

fn run_ok(argv: &[&str]) -> bool { run(argv).is_ok() }

// ---------------------------------------------------------------- state
/// Why there is no usable output (SndVolSSO.dll.mui strings 2006-2009, shown with icon 125).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AudioState {
    #[default]
    Ok,
    /// 2006 "The Audio Service is not running."
    NoService,
    /// 2007 "No Audio Output Device is installed."
    NotInstalled,
    /// 2009 "No Audio Output Device is enabled." (a sound card whose profile is Off)
    NotEnabled,
    /// 2008 "No speakers or headphones are plugged in." (the active jack reports nothing plugged in)
    Unplugged,
}

impl AudioState {
    pub fn text(self) -> &'static str {
        match self {
            AudioState::Ok => "",
            AudioState::NoService => "The Audio Service is not running.",
            AudioState::NotInstalled => "No Audio Output Device is installed.",
            AudioState::NotEnabled => "No Audio Output Device is enabled.",
            AudioState::Unplugged => "No speakers or headphones are plugged in.",
        }
    }
}

/// An output device (endpoint).
#[derive(Clone, Debug, Default)]
pub struct Sink {
    pub index: u32,
    /// the sound server's id (pactl name)
    pub name: String,
    /// the device's description ("Built-in Audio Analog Stereo")
    pub description: String,
    /// Windows' endpoint name: "Speakers", "Headphones", "Digital Output"…
    pub title: String,
    /// device.bus: "usb", "bluetooth", "pci"…
    pub bus: String,
    /// icon names, best first (resolve with `util::first_icon` on the GTK thread)
    pub icons: &'static [&'static str],
    /// raw channel volumes (65536 = 100 %)
    pub channels: Vec<u32>,
    pub muted: bool,
    /// playing right now (state RUNNING)
    pub running: bool,
    /// its active port says nothing is plugged in
    pub unplugged: bool,
    /// the monitor source (for peak meters)
    pub monitor: String,
}

impl Sink {
    /// Master volume in percent: the loudest channel (may exceed 100 when another tool raised it).
    pub fn level(&self) -> u32 { pct_of(self.channels.iter().copied().max().unwrap_or(0) as u64) }
    /// Name with the bus, as in SndVolSSO's tooltips: "Speakers (USB)", "Headphones (Bluetooth)".
    pub fn long_title(&self) -> String {
        match self.bus.as_str() {
            "bluetooth" => format!("{} (Bluetooth)", self.title),
            "usb" => format!("{} (USB)", self.title),
            _ => self.title.clone(),
        }
    }
}

/// A program playing sound (sink input), for the Volume Mixer.
#[derive(Clone, Debug, Default)]
pub struct Stream {
    pub index: u32,
    pub sink: u32,
    pub app: String,
    pub icon: String,
    pub binary: String,
    pub pid: String,
    pub role: String,
    pub channels: Vec<u32>,
    pub muted: bool,
}

impl Stream {
    pub fn level(&self) -> u32 { pct_of(self.channels.iter().copied().max().unwrap_or(0) as u64) }
    /// One of aerode's own event sounds (Win7's "System Sounds" session)?
    pub fn is_system_sounds(&self) -> bool { self.app == SYSTEM_SOUNDS || self.role.eq_ignore_ascii_case("event") }
}

/// What the tray icon, the flyout and the mixer show.
#[derive(Clone, Debug, Default)]
pub struct AudioInfo {
    pub state: AudioState,
    /// pactl name of the default output
    pub default: String,
    pub sinks: Vec<Sink>,
    /// sink inputs (only filled when asked for)
    pub streams: Vec<Stream>,
}

impl AudioInfo {
    pub fn default_sink(&self) -> Option<&Sink> {
        if self.state != AudioState::Ok && self.state != AudioState::Unplugged { return None; }
        self.sinks.iter().find(|s| s.name == self.default)
    }
    /// (level, muted) of the default device, None without a usable one.
    pub fn level(&self) -> Option<(u32, bool)> {
        if self.state != AudioState::Ok { return None; }
        self.default_sink().map(|s| (s.level(), s.muted))
    }
    /// The speaker icon's tooltip (SndVolSSO.dll.mui 2000-2009).
    pub fn tooltip(&self) -> String {
        match (self.state, self.default_sink()) {
            (AudioState::Ok, Some(s)) if s.muted => format!("{}: Muted", s.long_title()),
            (AudioState::Ok, Some(s)) => format!("{}: {}%", s.long_title(), s.level().min(100)),
            (AudioState::Ok, None) => AudioState::NotInstalled.text().into(),
            (st, _) => st.text().into(),
        }
    }
    /// The speaker icon (SndVolSSO.dll icons 120-125).
    pub fn icon(&self) -> &'static str {
        match self.level() {
            Some((l, m)) => volume_icon(Some((l.min(100), m))),
            None => volume_icon(None),
        }
    }
}

fn pct_of(raw: u64) -> u32 { ((raw * 100 + NORM / 2) / NORM) as u32 }
fn raw_of(pct: u32) -> u64 { (pct as u64 * NORM + 50) / 100 }

/// Win7's notification-area icon for a level (SndVolSSO 0x7FF71EC9B68: muted 120, level < 1 → 121,
/// < 33 → 122, < 66 → 123, else 124; no device 125), with fallbacks for themes without them.
pub fn volume_icon(v: Option<(u32, bool)>) -> &'static str {
    use crate::util::first_icon;
    match v {
        None => first_icon(&["audio-volume-error", "audio-volume-muted"]),
        Some((_, true)) => "audio-volume-muted",
        Some((0, _)) => first_icon(&["audio-volume-zero", "audio-volume-off", "audio-volume-muted"]),
        Some((p, _)) if p < 33 => "audio-volume-low",
        Some((p, _)) if p < 66 => "audio-volume-medium",
        _ => "audio-volume-high",
    }
}

/// Icon names for a device, best first (same rules as Control Panel > Sound).
pub fn device_icons(sink: bool, port_type: &str, port: &str, form: &str, bus: &str) -> &'static [&'static str] {
    let p = format!("{port_type} {port} {form}").to_lowercase();
    if sink {
        if p.contains("headphone") || p.contains("headset") || form == "headset" || bus == "bluetooth" { &["audio-headphones", "audio-speakers"] }
        else if p.contains("hdmi") || p.contains("iec958") || p.contains("spdif") || p.contains("displayport") { &["video-display", "audio-card"] }
        else if p.contains("line") { &["audio-card", "audio-speakers"] }
        else { &["audio-speakers", "audio-volume-high"] }
    } else if p.contains("line") { &["audio-card", "audio-input-microphone"] }
    else if bus == "bluetooth" || p.contains("headset") { &["audio-headset", "audio-input-microphone"] }
    else { &["audio-input-microphone"] }
}

/// Windows' endpoint name for an output: its active port's name ("Headphones", "HDMI / DisplayPort"),
/// except the generic analog ones, which are "Speakers" (same rule as Control Panel > Sound).
pub fn endpoint_title(port_desc: &str) -> String {
    if !port_desc.is_empty() && !port_desc.contains("Analog") { port_desc.to_string() } else { "Speakers".into() }
}

fn channels_of(v: &serde_json::Value) -> Vec<u32> {
    v.get("volume").and_then(|v| v.as_object())
        .map(|o| o.values().filter_map(|c| c.get("value").and_then(|x| x.as_u64())).map(|x| x as u32).collect())
        .unwrap_or_default()
}

/// Parse `pactl -f json list sinks`.
pub fn parse_sinks(json: &str) -> Vec<Sink> {
    use serde_json::Value;
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(json) else { return vec![] };
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    a.iter().map(|d| {
        let props = d.get("properties").cloned().unwrap_or(Value::Null);
        let p = |k: &str| props.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let active = s(d, "active_port");
        let ap = d.get("ports").and_then(|x| x.as_array()).and_then(|a| a.iter().find(|x| s(x, "name") == active)).cloned().unwrap_or(Value::Null);
        Sink {
            index: d.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            name: s(d, "name"),
            description: s(d, "description"),
            title: endpoint_title(&s(&ap, "description")),
            bus: p("device.bus"),
            icons: device_icons(true, &s(&ap, "type"), &active, &p("device.form_factor"), &p("device.bus")),
            channels: channels_of(d),
            muted: d.get("mute").and_then(|x| x.as_bool()).unwrap_or(false),
            running: s(d, "state") == "RUNNING",
            unplugged: s(&ap, "availability") == "not available",
            monitor: s(d, "monitor_source"),
        }
    }).collect()
}

/// Parse `pactl -f json list sink-inputs`.
pub fn parse_streams(json: &str) -> Vec<Stream> {
    use serde_json::Value;
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(json) else { return vec![] };
    a.iter().map(|d| {
        let props = d.get("properties").cloned().unwrap_or(Value::Null);
        let p = |k: &str| props.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let app = [p("application.name"), p("media.name"), p("node.name")].into_iter().find(|x| !x.is_empty()).unwrap_or_default();
        Stream {
            index: d.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            sink: d.get("sink").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            app, icon: p("application.icon_name"), binary: p("application.process.binary"), pid: p("application.process.id"),
            role: p("media.role"), channels: channels_of(d), muted: d.get("mute").and_then(|x| x.as_bool()).unwrap_or(false),
        }
    }).collect()
}

/// Cards that could play but whose profile is Off (Windows: a disabled device).
fn disabled_output_cards(json: &str) -> bool {
    use serde_json::Value;
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(json) else { return false };
    a.iter().any(|c| c.get("active_profile").and_then(|x| x.as_str()) == Some("off")
        && c.get("profiles").and_then(|p| p.as_object()).map(|o| o.values().any(|p| p.get("sinks").and_then(|x| x.as_u64()).unwrap_or(0) > 0)).unwrap_or(false))
}

/// Read the sound server (blocking, at most a few deadlines long). None = the server did not answer
/// in time (keep showing what was there).
pub fn audio_info(streams: bool) -> Option<AudioInfo> {
    let info = match run(&["pactl", "-f", "json", "info"]) {
        Ok(s) => s,
        Err(true) => return None,
        // pipewire-pulse (or PulseAudio) is the audio service programs play through
        Err(false) if which("pactl") => return Some(AudioInfo { state: AudioState::NoService, ..Default::default() }),
        Err(false) => return wpctl_info(),
    };
    let default = serde_json::from_str::<serde_json::Value>(&info).ok()
        .and_then(|v| v.get("default_sink_name").and_then(|x| x.as_str()).map(str::to_string)).unwrap_or_default();
    let sinks = match run(&["pactl", "-f", "json", "list", "sinks"]) { Ok(s) => parse_sinks(&s), Err(true) => return None, Err(false) => vec![] };
    // pipewire-pulse's module-always-sink adds "Dummy Output" (auto_null) when there is no real
    // device; user-made null sinks (OBS…) do count as devices
    let real: Vec<&Sink> = sinks.iter().filter(|s| s.name != "auto_null").collect();
    let state = if real.is_empty() || default == "auto_null" || !sinks.iter().any(|s| s.name == default) {
        if real.is_empty() {
            let cards = run(&["pactl", "-f", "json", "list", "cards"]).unwrap_or_default();
            if disabled_output_cards(&cards) { AudioState::NotEnabled } else { AudioState::NotInstalled }
        } else { AudioState::NotInstalled }
    } else if sinks.iter().find(|s| s.name == default).map(|s| s.unplugged).unwrap_or(false) {
        AudioState::Unplugged
    } else { AudioState::Ok };
    let streams = if streams { run(&["pactl", "-f", "json", "list", "sink-inputs"]).map(|s| parse_streams(&s)).unwrap_or_default() } else { vec![] };
    Some(AudioInfo { state, default, sinks: sinks.into_iter().filter(|s| s.name != "auto_null").collect(), streams })
}

/// Without pactl: the default device through WirePlumber, else "not running".
fn wpctl_info() -> Option<AudioInfo> {
    match run(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"]) {
        // without any output wpctl prints "Translate ID error…" and still succeeds
        Ok(s) if !s.trim_start().starts_with("Volume:") => Some(AudioInfo { state: AudioState::NotInstalled, ..Default::default() }),
        Ok(s) => {
            let v: f64 = s.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0.0);
            let raw = (v * NORM as f64).round() as u32;
            let sink = Sink { name: "@DEFAULT_AUDIO_SINK@".into(), title: "Speakers".into(), icons: &["audio-speakers", "audio-volume-high"],
                channels: vec![raw], muted: s.contains("MUTED"), ..Default::default() };
            Some(AudioInfo { state: AudioState::Ok, default: sink.name.clone(), sinks: vec![sink], streams: vec![] })
        }
        Err(true) => None,
        Err(false) => Some(AudioInfo { state: AudioState::NoService, ..Default::default() }),
    }
}

// ---------------------------------------------------------------- compatibility helpers
/// (level, muted) of the default output, blocking (at most a few seconds). The shell never calls
/// this on its GTK thread; prefer [`query_async`].
pub fn volume() -> Option<(u32, bool)> { audio_info(false)?.level() }

/// Set the default output's level (keeps the balance; queued, returns at once).
pub fn set_volume(pct: u32) { send(AudioCmd::Level { sink: None, pct: pct.min(100) }); }

/// Toggle the default output's mute (queued, returns at once).
pub fn toggle_mute() { send(AudioCmd::Mute { sink: None, mute: None }); }

/// Output devices: (id, description, is_default)
pub fn sinks() -> Vec<(String, String, bool)> {
    let Some(i) = audio_info(false) else { return vec![] };
    i.sinks.iter().map(|s| (s.name.clone(), s.description.clone(), s.name == i.default)).collect()
}

pub fn set_default_sink(name: &str) { crate::util::spawn(&["pactl", "set-default-sink", name]); }

// ---------------------------------------------------------------- level maths
/// Scale channel volumes so the loudest becomes `target` (raw), keeping their ratios
/// (pa_cvolume_scale). All channels equal when they are all 0 and no earlier ratio is known.
pub fn scale_channels(ch: &[u32], target: u64, ratio: Option<&[u32]>) -> Vec<u32> {
    let base: Vec<u32> = match ch.iter().copied().max() {
        Some(m) if m > 0 => ch.to_vec(),
        _ => match ratio { Some(r) if r.len() == ch.len() && r.iter().any(|&x| x > 0) => r.to_vec(), _ => vec![1; ch.len().max(1)] },
    };
    let max = base.iter().copied().max().unwrap_or(1).max(1) as u64;
    base.iter().map(|&c| ((c as u64 * target + max / 2) / max) as u32).collect()
}

/// The level after `steps` presses of Volume Up (+) or Down (−), 2 % each like Win7's
/// VolumeStepUp/Down, never above 100 – but a level another tool put above 100 % is left alone by
/// Up (up must never make it quieter) and goes straight to ≤ 100 % on Down.
pub fn stepped(cur: u32, steps: i32) -> u32 {
    let mut v = cur as i64;
    for _ in 0..steps.unsigned_abs() {
        v = if steps > 0 { if v > 100 { v } else { (v + 2).min(100) } } else { (v - 2).clamp(0, 100) };
    }
    v as u32
}

/// Channel ratios of each device the last time it was not silent (so 0 % keeps the balance).
static RATIOS: Mutex<Vec<(String, Vec<u32>)>> = Mutex::new(Vec::new());

fn remember_ratio(name: &str, ch: &[u32]) {
    if ch.iter().all(|&c| c == 0) { return; }
    let mut r = RATIOS.lock().unwrap_or_else(|e| e.into_inner());
    r.retain(|(n, _)| n != name);
    r.push((name.to_string(), ch.to_vec()));
}

fn ratio_of(name: &str) -> Option<Vec<u32>> {
    RATIOS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(n, _)| n == name).map(|(_, c)| c.clone())
}

/// The device a write is for: the named one, else the default.
fn target_sink(name: Option<&str>) -> Option<Sink> {
    let def = if name.is_none() { run(&["pactl", "get-default-sink"]).ok().map(|s| s.trim().to_string()) } else { None };
    let want = name.map(str::to_string).or(def)?;
    let sinks = parse_sinks(&run(&["pactl", "-f", "json", "list", "sinks"]).ok()?);
    sinks.into_iter().find(|s| s.name == want)
}

fn write_channels(s: &Sink, ch: &[u32]) -> bool {
    let vals: Vec<String> = ch.iter().map(|c| c.to_string()).collect();
    let mut a = vec!["pactl", "set-sink-volume", s.name.as_str()];
    a.extend(vals.iter().map(|v| v.as_str()));
    run_ok(&a)
}

/// Lock shared by every process that changes the volume (shell, volume keys, Control Panel).
struct VolLock(Option<std::fs::File>);
impl VolLock {
    fn take() -> VolLock {
        use std::os::unix::io::AsRawFd;
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("aerode-volume.lock")) else { return VolLock(None) };
        let start = Instant::now();
        // holders only run commands with deadlines; never wait forever on a stuck one
        while unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            if start.elapsed() > DEADLINE * 2 { return VolLock(None); }
            std::thread::sleep(Duration::from_millis(3));
        }
        VolLock(Some(f))
    }
}
impl Drop for VolLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        if let Some(f) = &self.0 { unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN); } }
    }
}

/// Set a device's level (blocking): every channel scaled, the balance kept.
pub fn apply_level(sink: Option<&str>, pct: u32) {
    if !which("pactl") {
        run_ok(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", pct.min(150) as f64 / 100.0)]);
        return;
    }
    let _l = VolLock::take();
    let Some(s) = target_sink(sink) else { return };
    remember_ratio(&s.name, &s.channels);
    let ch = scale_channels(&s.channels, raw_of(pct), ratio_of(&s.name).as_deref());
    write_channels(&s, &ch);
}

/// Volume Up/Down on the default device (blocking): unmutes first, like Win7's volume keys.
pub fn apply_step(steps: i32) {
    if !which("pactl") {
        run_ok(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "0"]);
        let cur = run(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"]).ok()
            .and_then(|s| s.split_whitespace().nth(1).and_then(|x| x.parse::<f64>().ok())).map(|v| (v * 100.0).round() as u32);
        if let Some(cur) = cur {
            let v = stepped(cur, steps);
            if v != cur { run_ok(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", v as f64 / 100.0)]); }
        }
        return;
    }
    let _l = VolLock::take();
    let Some(s) = target_sink(None) else { return };
    if s.muted { run_ok(&["pactl", "set-sink-mute", &s.name, "0"]); }
    let (cur, v) = (s.level(), stepped(s.level(), steps));
    if v == cur { return; }
    remember_ratio(&s.name, &s.channels);
    let ch = scale_channels(&s.channels, raw_of(v), ratio_of(&s.name).as_deref());
    write_channels(&s, &ch);
}

/// Mute or unmute a device; None = the default one (blocking).
pub fn apply_mute(sink: Option<&str>, mute: Option<bool>) {
    let m = |b: bool| if b { "1" } else { "0" };
    if !which("pactl") {
        run_ok(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", mute.map(m).unwrap_or("toggle")]);
        return;
    }
    let _l = VolLock::take();
    let name = sink.unwrap_or("@DEFAULT_SINK@");
    run_ok(&["pactl", "set-sink-mute", name, mute.map(m).unwrap_or("toggle")]);
}

/// The microphone mute key (APPCOMMAND_MICROPHONE_VOLUME_MUTE).
pub fn apply_mic_mute_toggle() {
    if !run_ok(&["pactl", "set-source-mute", "@DEFAULT_SOURCE@", "toggle"]) {
        run_ok(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"]);
    }
}

fn apply_stream_level(index: u32, pct: u32) {
    let Ok(s) = run(&["pactl", "-f", "json", "list", "sink-inputs"]) else { return };
    let Some(st) = parse_streams(&s).into_iter().find(|x| x.index == index) else { return };
    let ch = scale_channels(&st.channels, raw_of(pct), None);
    let vals: Vec<String> = ch.iter().map(|c| c.to_string()).collect();
    let idx = index.to_string();
    let mut a = vec!["pactl", "set-sink-input-volume", idx.as_str()];
    a.extend(vals.iter().map(|v| v.as_str()));
    run_ok(&a);
}

/// Set the System Sounds level. WirePlumber's restore-stream (and PulseAudio's module-stream-restore)
/// give every new Event-role stream the level its last one was set to, overriding the player's own
/// volume; so the level lives there: set it on a live System Sounds stream, or on a moment of
/// silence played as one.
fn apply_event_level(pct: u32) {
    let live = |s: &str| parse_streams(s).into_iter().filter(|x| x.is_system_sounds()).map(|x| x.index).collect::<Vec<u32>>();
    if let Ok(s) = run(&["pactl", "-f", "json", "list", "sink-inputs"]) {
        let l = live(&s);
        if !l.is_empty() { for i in l { apply_stream_level(i, pct); } return; }
    }
    let Some(f) = silence_file() else { return };
    let argv: Vec<String> = if which("pw-play") {
        vec!["pw-play".into(), "--media-role=Event".into(), "-P".into(), format!("{{ application.name = \"{SYSTEM_SOUNDS}\" }}"), f.to_string_lossy().into_owned()]
    } else if which("paplay") {
        vec!["paplay".into(), format!("--client-name={SYSTEM_SOUNDS}"), "--property=media.role=event".into(), f.to_string_lossy().into_owned()]
    } else { return };
    let Ok(mut child) = std::process::Command::new(&argv[0]).args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return };
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        std::thread::sleep(Duration::from_millis(30));
        let Ok(s) = run(&["pactl", "-f", "json", "list", "sink-inputs"]) else { break };
        let l = live(&s);
        if !l.is_empty() { for i in l { apply_stream_level(i, pct); } std::thread::sleep(Duration::from_millis(60)); break; }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Two seconds of silence (8 kHz mono WAV) in the runtime dir.
fn silence_file() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let f = dir.join("aerode-silence.wav");
    if f.exists() { return Some(f); }
    let n: u32 = 16000 * 2;
    let mut w = Vec::with_capacity(44 + n as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + n).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&8000u32.to_le_bytes());
    w.extend_from_slice(&16000u32.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&n.to_le_bytes());
    w.resize(44 + n as usize, 0);
    std::fs::write(&f, w).ok()?;
    Some(f)
}

// ---------------------------------------------------------------- the writer
/// A change (or read) for the audio worker.
pub enum AudioCmd {
    /// absolute level of a device (None = default), balance kept
    Level { sink: Option<String>, pct: u32 },
    /// Volume Up (+n) / Down (−n) presses on the default device; unmutes
    Step(i32),
    /// mute (Some) or toggle (None)
    Mute { sink: Option<String>, mute: Option<bool> },
    MicMuteToggle,
    /// a program's level / mute (Volume Mixer)
    StreamLevel { index: u32, pct: u32 },
    StreamMute { index: u32, mute: bool },
    /// the System Sounds level (Volume Mixer): kept by the sound server for the Event role
    EventLevel(u32),
    /// play an event sound after the changes queued before it, cutting off the previous one
    /// (SndVol stops any sound, then plays .Default, when a slider is let go)
    Play(PathBuf),
    /// read the state after everything queued before it
    Query { streams: bool, reply: async_channel::Sender<Option<AudioInfo>> },
}

static WORKER: Mutex<Option<std::sync::mpsc::Sender<AudioCmd>>> = Mutex::new(None);

/// Queue a change for the audio worker (started on first use). Never blocks.
pub fn send(cmd: AudioCmd) {
    let mut w = WORKER.lock().unwrap_or_else(|e| e.into_inner());
    if w.is_none() {
        let (tx, rx) = std::sync::mpsc::channel::<AudioCmd>();
        std::thread::Builder::new().name("audio".into()).spawn(move || worker(rx)).ok();
        *w = Some(tx);
    }
    if let Some(tx) = w.as_ref() { let _ = tx.send(cmd); }
}

/// Read the audio state off-thread and hand it to `cb` on the GTK main loop
/// (None when the server did not answer in time). Several reads queued while one runs are answered
/// together.
pub fn query_async(streams: bool, cb: impl FnOnce(Option<AudioInfo>) + 'static) {
    let (tx, rx) = async_channel::bounded(1);
    send(AudioCmd::Query { streams, reply: tx });
    gtk::glib::MainContext::default().spawn_local(async move {
        if let Ok(v) = rx.recv().await { cb(v); }
    });
}

fn worker(rx: std::sync::mpsc::Receiver<AudioCmd>) {
    let mut player: Option<std::process::Child> = None;
    while let Ok(first) = rx.recv() {
        // everything queued meanwhile, in order; stale slider positions are dropped
        let mut batch = vec![first];
        while let Ok(c) = rx.try_recv() { batch.push(c); }
        let mut ops: Vec<AudioCmd> = Vec::new();
        let mut replies: Vec<async_channel::Sender<Option<AudioInfo>>> = Vec::new();
        let mut want_streams = false;
        for c in batch {
            match c {
                AudioCmd::Query { streams, reply } => { want_streams |= streams; replies.push(reply); }
                AudioCmd::Level { sink, pct } => {
                    if let Some(AudioCmd::Level { sink: s2, pct: p2 }) = ops.last_mut() { if *s2 == sink { *p2 = pct; continue; } }
                    ops.push(AudioCmd::Level { sink, pct });
                }
                AudioCmd::Step(n) => {
                    if let Some(AudioCmd::Step(m)) = ops.last_mut() { *m += n; continue; }
                    ops.push(AudioCmd::Step(n));
                }
                AudioCmd::StreamLevel { index, pct } => {
                    if let Some(AudioCmd::StreamLevel { index: i2, pct: p2 }) = ops.last_mut() { if *i2 == index { *p2 = pct; continue; } }
                    ops.push(AudioCmd::StreamLevel { index, pct });
                }
                AudioCmd::EventLevel(pct) => {
                    if let Some(AudioCmd::EventLevel(p2)) = ops.last_mut() { *p2 = pct; continue; }
                    ops.push(AudioCmd::EventLevel(pct));
                }
                c => ops.push(c),
            }
        }
        for op in ops {
            match op {
                AudioCmd::Level { sink, pct } => apply_level(sink.as_deref(), pct),
                AudioCmd::Step(n) => apply_step(n),
                AudioCmd::Mute { sink, mute } => apply_mute(sink.as_deref(), mute),
                AudioCmd::MicMuteToggle => apply_mic_mute_toggle(),
                AudioCmd::StreamLevel { index, pct } => apply_stream_level(index, pct),
                AudioCmd::StreamMute { index, mute } => { run_ok(&["pactl", "set-sink-input-mute", &index.to_string(), if mute { "1" } else { "0" }]); }
                AudioCmd::EventLevel(pct) => apply_event_level(pct),
                AudioCmd::Play(f) => {
                    if let Some(mut c) = player.take() { let _ = c.kill(); let _ = c.wait(); }
                    player = event_player(&f).and_then(|a| std::process::Command::new(&a[0]).args(&a[1..])
                        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok());
                }
                AudioCmd::Query { .. } => {}
            }
        }
        // reap a finished sound
        if let Some(c) = player.as_mut() { if matches!(c.try_wait(), Ok(Some(_))) { player = None; } }
        if !replies.is_empty() {
            let info = audio_info(want_streams);
            for r in replies { let _ = r.try_send(info.clone()); }
        }
    }
}

// ---------------------------------------------------------------- System Sounds
/// The application name of aerode's event sounds: the Volume Mixer's "System Sounds".
pub const SYSTEM_SOUNDS: &str = "System Sounds";

/// Volume Mixer and Volume Control Options settings (~/.config/aerode/volume.json).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VolumePrefs {
    /// System Sounds level, % of the master volume
    pub system_sounds: u32,
    pub system_sounds_muted: bool,
    /// Volume Control Options: "All devices currently playing sound"
    pub all_playing: bool,
    /// "The default communication device"
    pub communication: bool,
    /// "Sound devices:" ticked (pactl names)
    pub devices: Vec<String>,
}

impl Default for VolumePrefs {
    fn default() -> Self { VolumePrefs { system_sounds: 100, system_sounds_muted: false, all_playing: false, communication: false, devices: vec![] } }
}

fn prefs_path() -> PathBuf { crate::config::conf_dir().join("volume.json") }

pub fn volume_prefs() -> VolumePrefs {
    std::fs::read_to_string(prefs_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_volume_prefs(p: &VolumePrefs) {
    let path = prefs_path();
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    if let Ok(s) = serde_json::to_string_pretty(p) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, s).is_ok() { let _ = std::fs::rename(&tmp, &path); }
    }
}

/// The command line that plays an event sound as Win7's "System Sounds" session: its own name and
/// the Event role (so the Volume Mixer can show and control it) at the System Sounds level.
/// None when System Sounds is muted in the Volume Mixer.
pub fn event_player(file: &Path) -> Option<Vec<String>> {
    let p = volume_prefs();
    if p.system_sounds_muted { return None; }
    let f = file.to_string_lossy().into_owned();
    let vol = p.system_sounds.min(100);
    if which("pw-play") {
        Some(vec!["pw-play".into(), "--media-role=Event".into(), format!("--volume={:.2}", vol as f64 / 100.0),
            "-P".into(), format!("{{ application.name = \"{SYSTEM_SOUNDS}\" application.icon_name = \"preferences-desktop-sound\" }}"), f])
    } else if which("paplay") {
        Some(vec!["paplay".into(), format!("--client-name={SYSTEM_SOUNDS}"), "--property=media.role=event".into(),
            format!("--volume={}", raw_of(vol)), f])
    } else if which("aplay") {
        Some(vec!["aplay".into(), "-q".into(), f])
    } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_keeps_the_balance() {
        // L30/R60 → master 50 %: 25/50
        assert_eq!(scale_channels(&[19661, 39322], raw_of(50), None), vec![16384, 32768]);
        // silent: the remembered ratio, else equal channels
        assert_eq!(scale_channels(&[0, 0], raw_of(60), Some(&[1, 2])), vec![19661, 39322]);
        assert_eq!(scale_channels(&[0, 0], raw_of(10), None), vec![6554, 6554]);
        assert_eq!(pct_of(39322), 60);
    }

    #[test]
    fn disabled_card_is_detected() {
        let off = r#"[{"name":"alsa_card.pci","active_profile":"off","profiles":{"off":{"sinks":0},"output:analog-stereo":{"sinks":1,"sources":0}}}]"#;
        let on = r#"[{"name":"alsa_card.pci","active_profile":"output:analog-stereo","profiles":{"output:analog-stereo":{"sinks":1}}}]"#;
        assert!(disabled_output_cards(off));
        assert!(!disabled_output_cards(on));
    }

    #[test]
    fn steps_follow_win7() {
        assert_eq!(stepped(40, 1), 42);
        assert_eq!(stepped(99, 1), 100);
        assert_eq!(stepped(100, 3), 100);
        assert_eq!(stepped(150, 1), 150); // up never lowers
        assert_eq!(stepped(150, -1), 100);
        assert_eq!(stepped(1, -1), 0);
        assert_eq!(stepped(10, -3), 4);
    }

    #[test]
    fn parses_pactl_json() {
        let j = r#"[{"index":48,"state":"RUNNING","name":"alsa_output.usb","description":"USB Audio","mute":true,
            "volume":{"front-left":{"value":19661},"front-right":{"value":39322}},"monitor_source":"alsa_output.usb.monitor",
            "properties":{"device.bus":"usb"},"ports":[{"name":"analog-output-headphones","description":"Headphones","type":"Headphones","availability":"available"}],
            "active_port":"analog-output-headphones"},
            {"index":49,"state":"SUSPENDED","name":"alsa_output.pci","description":"Built-in","mute":false,"volume":{"mono":{"value":65536}},
            "properties":{},"ports":[{"name":"analog-output-speaker","description":"Analog Output","availability":"not available"}],"active_port":"analog-output-speaker"}]"#;
        let s = parse_sinks(j);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].level(), s[0].muted, s[0].running), (60, true, true));
        assert_eq!(s[0].long_title(), "Headphones (USB)");
        assert_eq!(s[1].title, "Speakers");
        assert!(s[1].unplugged);
        let i = AudioInfo { state: AudioState::Ok, default: "alsa_output.usb".into(), sinks: s, streams: vec![] };
        assert_eq!(i.tooltip(), "Headphones (USB): Muted");
        let i = AudioInfo { state: AudioState::NotInstalled, ..Default::default() };
        assert_eq!(i.tooltip(), "No Audio Output Device is installed.");
    }
}
