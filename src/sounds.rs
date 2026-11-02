//! Windows 7 sound scheme: the program events of Control Panel > Sound > Sounds (HKCU\AppEvents in
//! Windows), the event → .wav assignments of the 15 Win7 schemes, and playback.
//!
//! The .wav files come from the user's own Windows\Media folder (`aerode-extract`), the scheme
//! folders (Afternoon, Calligraphy…) included. config.json `"sounds": {"<event key>": "file"}`
//! overrides an event: a name or a path relative to the extracted sounds folder
//! ("Afternoon/Windows Ding.wav"), an absolute path, or "" for no sound. `"sounds_muted"` is the
//! "No Sounds" scheme.
//!
//! Like Windows' PlaySound, every play reads the current assignment: config.json is checked (one
//! stat) on each play, so a scheme applied in Control Panel reaches running programs at once.
//!
//! Playback (one worker thread per process) follows PlaySound:
//! - a process plays one event sound at a time; a new one stops the one playing
//!   ([`Policy::Replace`]), except the shell's own event sounds, which Explorer plays with
//!   SND_NOSTOP: those are dropped while another sound plays ([`Policy::Yield`]);
//! - the sounds are system sounds (SND_SYSTEM): PipeWire/PulseAudio streams with the
//!   Notification/"event" role, so the "System Sounds" volume applies to them.
use crate::{config, util};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// The applications of the Program Events list (HKCU\AppEvents\Schemes\Apps).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum App { Windows, Explorer, Speech }

impl App {
    pub const ALL: [App; 3] = [App::Windows, App::Explorer, App::Speech];
    pub fn key(self) -> &'static str {
        match self { App::Windows => ".Default", App::Explorer => "Explorer", App::Speech => "sapisvr" }
    }
    /// Group label in the Program Events tree.
    pub fn label(self) -> &'static str {
        match self { App::Windows => "Windows", App::Explorer => "Windows Explorer", App::Speech => "Windows Speech Recognition" }
    }
}

/// Every Windows 7 SP1 program event (default user hive of install.wim), in the order the
/// Sounds tab lists them: by application, then alphabetically by label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Event {
    // Windows
    Asterisk, CloseProgram, CriticalBattery, CriticalStop, DefaultBeep, DeviceConnect, DeviceDisconnect,
    DeviceFail, Exclamation, ExitWindows, LowBattery, Maximize, MenuCommand, MenuPopup, Minimize, NewFax,
    NewMail, OpenProgram, PrintComplete, ProgramError, Question, Restore, RestoreUp, Select, ShowToolbarBand,
    SystemNotification, ChangeTheme, Logoff, Logon, UserAccountControl,
    // Windows Explorer
    PopupBlocked, CompleteNavigation, EmptyRecycleBin, FeedDiscovered, InfoBar, MoveMenuItem, StartNavigation,
    // Windows Speech Recognition
    SpeechDisNumbers, SpeechDisPanel, SpeechMisrecognition, SpeechOff, SpeechOn, SpeechSleep,
}

impl Event {
    /// The registry key name (HKCU\AppEvents\Schemes\Apps\<app>\<key>), also the config.json key.
    pub fn key(self) -> &'static str {
        use Event::*;
        match self {
            Asterisk => "SystemAsterisk", CloseProgram => "Close", CriticalBattery => "CriticalBatteryAlarm",
            CriticalStop => "SystemHand", DefaultBeep => ".Default", DeviceConnect => "DeviceConnect",
            DeviceDisconnect => "DeviceDisconnect", DeviceFail => "DeviceFail", Exclamation => "SystemExclamation",
            ExitWindows => "SystemExit", LowBattery => "LowBatteryAlarm", Maximize => "Maximize",
            MenuCommand => "MenuCommand", MenuPopup => "MenuPopup", Minimize => "Minimize", NewFax => "FaxBeep",
            NewMail => "MailBeep", OpenProgram => "Open", PrintComplete => "PrintComplete", ProgramError => "AppGPFault",
            Question => "SystemQuestion", Restore => "RestoreDown", RestoreUp => "RestoreUp", Select => "CCSelect",
            ShowToolbarBand => "ShowBand", SystemNotification => "SystemNotification", ChangeTheme => "ChangeTheme",
            Logoff => "WindowsLogoff", Logon => "WindowsLogon", UserAccountControl => "WindowsUAC",
            PopupBlocked => "BlockedPopup", CompleteNavigation => "ActivatingDocument", EmptyRecycleBin => "EmptyRecycleBin",
            FeedDiscovered => "FeedDiscovered", InfoBar => "SecurityBand", MoveMenuItem => "MoveMenuItem",
            StartNavigation => "Navigating",
            SpeechDisNumbers => "DisNumbersSound", SpeechDisPanel => "PanelSound", SpeechMisrecognition => "MisrecoSound",
            SpeechOff => "HubOffSound", SpeechOn => "HubOnSound", SpeechSleep => "HubSleepSound",
        }
    }

    pub fn from_key(key: &str) -> Option<Event> {
        Event::ALL.iter().copied().find(|e| e.key().eq_ignore_ascii_case(key))
    }

    pub fn app(self) -> App {
        use Event::*;
        match self {
            PopupBlocked | CompleteNavigation | EmptyRecycleBin | FeedDiscovered | InfoBar | MoveMenuItem | StartNavigation => App::Explorer,
            SpeechDisNumbers | SpeechDisPanel | SpeechMisrecognition | SpeechOff | SpeechOn | SpeechSleep => App::Speech,
            _ => App::Windows,
        }
    }

    /// Label shown in Control Panel → Sound (mmres.dll, ieframe.dll and sapi.cpl strings).
    pub fn label(self) -> &'static str {
        use Event::*;
        match self {
            Asterisk => "Asterisk", CloseProgram => "Close Program", CriticalBattery => "Critical Battery Alarm",
            CriticalStop => "Critical Stop", DefaultBeep => "Default Beep", DeviceConnect => "Device Connect",
            DeviceDisconnect => "Device Disconnect", DeviceFail => "Device Failed to Connect", Exclamation => "Exclamation",
            ExitWindows => "Exit Windows", LowBattery => "Low Battery Alarm", Maximize => "Maximize",
            MenuCommand => "Menu Command", MenuPopup => "Menu Pop-up", Minimize => "Minimize", NewFax => "New Fax Notification",
            NewMail => "New Mail Notification", OpenProgram => "Open Program", PrintComplete => "Print Complete",
            ProgramError => "Program Error", Question => "Question", Restore => "Restore Down", RestoreUp => "Restore Up",
            Select => "Select", ShowToolbarBand => "Show Toolbar Band", SystemNotification => "System Notification",
            ChangeTheme => "Windows Change Theme", Logoff => "Windows Logoff", Logon => "Windows Logon",
            UserAccountControl => "Windows User Account Control",
            // SP1 ships IE8, which calls it "Information Bar" (IE9 renamed it "Notification bar")
            PopupBlocked => "Blocked Pop-up Window", CompleteNavigation => "Complete Navigation",
            EmptyRecycleBin => "Empty Recycle Bin", FeedDiscovered => "Feed Discovered", InfoBar => "Information Bar",
            MoveMenuItem => "Move Menu Item", StartNavigation => "Start Navigation",
            SpeechDisNumbers => "Disambiguation Numbers", SpeechDisPanel => "Disambiguation Panel",
            SpeechMisrecognition => "Misrecognition", SpeechOff => "Off", SpeechOn => "On", SpeechSleep => "Sleep",
        }
    }

    /// Assignment in the "Windows Default" scheme ("" = no sound).
    pub fn default_file(self) -> &'static str {
        use Event::*;
        match self {
            Asterisk => "Windows Error.wav", CriticalBattery => "Windows Battery Critical.wav",
            CriticalStop => "Windows Critical Stop.wav", DefaultBeep => "Windows Ding.wav",
            DeviceConnect => "Windows Hardware Insert.wav", DeviceDisconnect => "Windows Hardware Remove.wav",
            DeviceFail => "Windows Hardware Fail.wav", Exclamation => "Windows Exclamation.wav",
            ExitWindows => "Windows Shutdown.wav", LowBattery => "Windows Battery Low.wav", NewFax => "Windows Notify.wav",
            NewMail => "Windows Notify.wav", SystemNotification => "Windows Balloon.wav",
            ChangeTheme => "Windows Logon Sound.wav", Logoff => "Windows Logoff Sound.wav", Logon => "Windows Logon Sound.wav",
            UserAccountControl => "Windows User Account Control.wav",
            PopupBlocked => "Windows Pop-up Blocked.wav", EmptyRecycleBin => "Windows Recycle.wav",
            FeedDiscovered => "Windows Feed Discovered.wav", InfoBar => "Windows Information Bar.wav",
            StartNavigation => "Windows Navigation Start.wav",
            SpeechDisNumbers | SpeechDisPanel => "Speech Disambiguation.wav", SpeechMisrecognition => "Speech Misrecognition.wav",
            SpeechOff => "Speech Off.wav", SpeechOn => "Speech On.wav", SpeechSleep => "Speech Sleep.wav",
            CloseProgram | Maximize | MenuCommand | MenuPopup | Minimize | OpenProgram | PrintComplete | ProgramError
            | Question | Restore | RestoreUp | Select | ShowToolbarBand | CompleteNavigation | MoveMenuItem => "",
        }
    }

    pub const ALL: [Event; 43] = {
        use Event::*;
        [Asterisk, CloseProgram, CriticalBattery, CriticalStop, DefaultBeep, DeviceConnect, DeviceDisconnect,
         DeviceFail, Exclamation, ExitWindows, LowBattery, Maximize, MenuCommand, MenuPopup, Minimize, NewFax,
         NewMail, OpenProgram, PrintComplete, ProgramError, Question, Restore, RestoreUp, Select, ShowToolbarBand,
         SystemNotification, ChangeTheme, Logoff, Logon, UserAccountControl,
         PopupBlocked, CompleteNavigation, EmptyRecycleBin, FeedDiscovered, InfoBar, MoveMenuItem, StartNavigation,
         SpeechDisNumbers, SpeechDisPanel, SpeechMisrecognition, SpeechOff, SpeechOn, SpeechSleep]
    };
}

// ------------------------------------------------------------------ schemes

/// The schemes of the Sound scheme list (HKCU\AppEvents\Schemes\Names), in Win7's order.
/// ".Default" is "Windows Default", ".None" is "No Sounds"; the others use Windows\Media\<name>.
pub const SCHEMES: [&str; 15] = [".Default", ".None", "Afternoon", "Calligraphy", "Characters", "Cityscape", "Delta",
    "Festival", "Garden", "Heritage", "Landscape", "Quirky", "Raga", "Savanna", "Sonata"];

pub fn scheme_label(id: &str) -> &str {
    match id { ".Default" => "Windows Default", ".None" => "No Sounds", s => s }
}

/// Is the scheme's sound folder there (the named schemes need a newer `aerode-extract`)?
pub fn scheme_available(id: &str) -> bool {
    matches!(id, ".Default" | ".None") || resolve_ci(&sounds_dir(), id).map(|p| p.is_dir()).unwrap_or(false)
}

/// What a scheme assigns to an event, as stored in config.json ("" = no sound).
/// The named schemes use their own folder, except Exit Windows, Empty Recycle Bin and the speech
/// events, which they leave on the root Media files (Win7 hive).
pub fn scheme_file(id: &str, ev: Event) -> String {
    use Event::*;
    match id {
        ".Default" => ev.default_file().to_string(),
        ".None" => String::new(),
        s => match ev {
            ExitWindows | EmptyRecycleBin => ev.default_file().to_string(),
            _ if ev.app() == App::Speech => ev.default_file().to_string(),
            PrintComplete => format!("{s}/Windows Print complete.wav"),
            DefaultBeep | ChangeTheme | CriticalBattery | DeviceConnect | DeviceDisconnect | DeviceFail | NewFax
            | LowBattery | NewMail | Asterisk | Exclamation | CriticalStop | SystemNotification | Logoff | Logon
            | UserAccountControl | PopupBlocked | FeedDiscovered | StartNavigation | InfoBar => format!("{s}/{}", ev.default_file()),
            _ => String::new(),
        },
    }
}

/// The complete `sounds` map of config.json for a scheme (every event spelled out, like Windows
/// copies the scheme into .Current).
pub fn scheme_map(id: &str) -> HashMap<String, String> {
    Event::ALL.iter().map(|e| (e.key().to_string(), scheme_file(id, *e))).collect()
}

// ------------------------------------------------------------------ current assignments

pub fn sounds_dir() -> PathBuf { config::data_dir().join("sounds") }

#[derive(Default)]
struct Scheme {
    stamp: Option<(SystemTime, u64, u64)>,
    map: HashMap<String, String>,
    muted: bool,
    version: u64,
}

static SCHEME: Mutex<Option<Scheme>> = Mutex::new(None);

fn parse_scheme(json: &str) -> (HashMap<String, String>, bool) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let map = v.get("sounds").and_then(|s| s.as_object()).map(|o| o.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default();
    (map, v.get("sounds_muted").and_then(|m| m.as_bool()).unwrap_or(false))
}

/// Run `f` on the scheme as config.json has it now (re-read only when the file changed).
fn with_scheme<R>(f: impl FnOnce(&Scheme) -> R) -> R {
    use std::os::unix::fs::MetadataExt;
    let path = config::conf_dir().join("config.json");
    let stamp = std::fs::metadata(&path).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len(), m.ino()));
    let mut g = SCHEME.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(Scheme::default);
    if s.version == 0 || s.stamp != stamp {
        let (map, muted) = std::fs::read_to_string(&path).map(|j| parse_scheme(&j)).unwrap_or_default();
        *s = Scheme { stamp, map, muted, version: s.version + 1 };
    }
    f(s)
}

/// "No Sounds" scheme chosen.
pub fn muted() -> bool { with_scheme(|s| s.muted) }

/// Changes whenever the sound settings in config.json may have changed.
pub fn scheme_version() -> u64 { with_scheme(|s| s.version) }

/// The current assignment of an event as config.json stores it ("" = none), ignoring "No Sounds".
pub fn assigned(ev: Event) -> String {
    with_scheme(|s| s.map.get(ev.key()).cloned().unwrap_or_else(|| ev.default_file().to_string()))
}

/// Find `rel` ("Afternoon/Windows Ding.wav") under `dir`, ignoring case like Windows.
fn resolve_ci(dir: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = dir.to_path_buf();
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        let exact = p.join(part);
        p = if exact.exists() { exact } else {
            std::fs::read_dir(&p).ok()?.flatten().map(|e| e.path())
                .find(|f| f.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(part)).unwrap_or(false))?
        };
    }
    Some(p)
}

/// The file an assignment names, if it exists.
pub fn resolve(assigned: &str) -> Option<PathBuf> {
    if assigned.is_empty() { return None; }
    let p = PathBuf::from(assigned);
    if p.is_absolute() { return p.is_file().then_some(p); }
    resolve_ci(&sounds_dir(), assigned).filter(|p| p.is_file())
}

/// Resolve the file currently assigned to an event (None = silent).
pub fn file_for(ev: Event) -> Option<PathBuf> {
    let (assigned, muted) = with_scheme(|s| (s.map.get(ev.key()).cloned().unwrap_or_else(|| ev.default_file().to_string()), s.muted));
    if muted { return None; }
    resolve(&assigned)
}

// ------------------------------------------------------------------ playback

/// What a new sound does to one this process is still playing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// PlaySound / MessageBeep: stop the sound playing and play this one.
    Replace,
    /// SND_NOSTOP (Explorer's own event sounds): drop this one while another sound plays.
    Yield,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Player { PwPlay, Paplay, Aplay }

struct Req {
    file: PathBuf,
    /// stream name (the event's label) and event.id (its key)
    name: String,
    id: String,
    policy: Policy,
    /// keep retrying a player that fails right away (no output device yet) until then
    retry_until: Option<Instant>,
}

/// A player that exits with an error this soon did not play anything.
const FAST_FAIL: Duration = Duration::from_millis(400);
/// The same file asked for again this soon is one sound (bell storms, key repeat).
const DEDUP: Duration = Duration::from_millis(100);

/// Run a short command with a deadline; stdout on success.
fn output_timeout(argv: &[&str], limit: Duration) -> Option<String> {
    use std::io::Read;
    if !util::which(argv[0]) { return None; }
    let mut c = Command::new(argv[0]).args(&argv[1..]).env("LC_ALL", "C")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let end = Instant::now() + limit;
    loop {
        match c.try_wait() {
            Ok(Some(st)) => {
                let mut s = String::new();
                c.stdout.take()?.read_to_string(&mut s).ok()?;
                return st.success().then_some(s);
            }
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            _ => { let _ = c.kill(); let _ = c.wait(); return None; }
        }
    }
}

/// The players to try, best first. pw-play on PipeWire (also behind pipewire-pulse); paplay on a
/// real PulseAudio server, where pw-play would socket-activate a second sound server (Arch enables
/// pipewire.socket for every user); aplay last.
fn players() -> &'static [Player] {
    static CHAIN: OnceLock<Vec<Player>> = OnceLock::new();
    CHAIN.get_or_init(|| {
        let pulse = output_timeout(&["pactl", "info"], Duration::from_secs(2))
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Server Name:").map(|v| v.trim().to_string())));
        let order: &[Player] = match &pulse {
            Some(name) if !name.contains("PipeWire") => &[Player::Paplay, Player::Aplay],
            _ => &[Player::PwPlay, Player::Paplay, Player::Aplay],
        };
        order.iter().copied().filter(|p| util::which(match p { Player::PwPlay => "pw-play", Player::Paplay => "paplay", Player::Aplay => "aplay" })).collect()
    })
}

fn spawn_player(p: Player, file: &Path, name: &str, id: &str) -> Option<Child> {
    let clean = |s: &str| s.replace(['"', '\\', '\n'], " ");
    let mut c = match p {
        Player::PwPlay => {
            // role Notification = pulse "event" = the "System Sounds" volume (pavucontrol, Plasma)
            let mut c = Command::new("pw-play");
            c.arg("--media-role").arg("Notification").arg("-P").arg(format!(
                "{{ application.name = \"System Sounds\" application.id = \"aerode.system-sounds\" media.name = \"{}\" event.id = \"{}\" }}",
                clean(name), clean(id)));
            c
        }
        Player::Paplay => {
            let mut c = Command::new("paplay");
            c.arg("--client-name=System Sounds").arg(format!("--stream-name={name}"))
                .arg("--property=media.role=event").arg(format!("--property=event.id={id}"));
            c
        }
        Player::Aplay => { let mut c = Command::new("aplay"); c.arg("-q"); c }
    };
    c.arg(file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()
}

struct Playing { child: Child, req: Req, player: usize, started: Instant }

fn player_thread(rx: mpsc::Receiver<Req>) {
    let mut pref = 0usize;
    let mut cur: Option<Playing> = None;
    let mut last: Option<(PathBuf, Instant)> = None;
    let mut retries: Vec<(Instant, Req, usize)> = vec![];
    // start `req` with the first player from `from` on that can be spawned
    let start = |req: Req, from: usize| -> Option<Playing> {
        let chain = players();
        (from..chain.len()).find_map(|i| spawn_player(chain[i], &req.file, &req.name, &req.id).map(|c| (c, i)))
            .map(|(child, player)| Playing { child, req, player, started: Instant::now() })
    };
    loop {
        let msg = if cur.is_some() || !retries.is_empty() {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(r) => Some(r),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() { Ok(r) => Some(r), Err(_) => return }
        };
        // reap the sound that ended; one that failed at once gets another try
        if let Some(Ok(Some(st))) = cur.as_mut().map(|p| p.child.try_wait()) {
            let p = cur.take().unwrap();
            if !st.success() && p.started.elapsed() < FAST_FAIL {
                match p.req.retry_until {
                    Some(t) if Instant::now() < t => retries.push((Instant::now() + Duration::from_millis(250), p.req, p.player)),
                    _ => { cur = start(p.req, p.player + 1); if let Some(c) = &cur { pref = c.player; } }
                }
            }
        }
        if cur.is_none() {
            if let Some(i) = retries.iter().position(|r| Instant::now() >= r.0) {
                let (_, req, player) = retries.remove(i);
                cur = start(req, player);
            }
        }
        let Some(req) = msg else { continue };
        if last.as_ref().map(|(f, t)| *f == req.file && t.elapsed() < DEDUP).unwrap_or(false) { continue; }
        last = Some((req.file.clone(), Instant::now()));
        if let Some(p) = cur.as_mut() {
            if matches!(p.child.try_wait(), Ok(None)) {
                if req.policy == Policy::Yield { continue; }
                let _ = p.child.kill();
            }
            let _ = p.child.wait();
        }
        cur = start(req, pref);
    }
}

fn submit(req: Req) {
    static TX: OnceLock<Mutex<mpsc::Sender<Req>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("aerode-sound".into()).spawn(move || player_thread(rx));
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(req);
}

/// Play an event of the current scheme (PlaySound semantics: replaces this process's sound).
pub fn play(ev: Event) { play_with(ev, Policy::Replace) }

pub fn play_with(ev: Event, policy: Policy) {
    if let Some(f) = file_for(ev) {
        submit(Req { file: f, name: ev.label().into(), id: ev.key().into(), policy, retry_until: None });
    }
}

/// Play a file as a system sound (not subject to "No Sounds": callers decide).
pub fn play_file(f: &Path) { play_file_with(f, Policy::Replace) }

pub fn play_file_with(f: &Path, policy: Policy) {
    let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    submit(Req { file: f.to_path_buf(), name, id: String::new(), policy, retry_until: None });
}

/// Length of a PCM .wav from its RIFF header.
pub fn wav_duration(data: &[u8]) -> Option<Duration> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" { return None; }
    let (mut o, mut rate, mut size) = (12usize, None, None);
    while o + 8 <= data.len() {
        let len = u32::from_le_bytes(data[o + 4..o + 8].try_into().ok()?) as usize;
        match &data[o..o + 4] {
            b"fmt " if o + 16 <= data.len() => rate = Some(u32::from_le_bytes(data[o + 16..o + 20].try_into().ok()?)),
            b"data" => { size = Some(len.min(data.len().saturating_sub(o + 8))); break; }
            _ => {}
        }
        o += 8 + len + (len & 1);
    }
    let rate = rate.filter(|r| *r > 0)?;
    Some(Duration::from_secs_f64(size? as f64 / rate as f64))
}

/// Play an event and wait until it ends — for the Log off / Shut down / Restart sounds, which
/// Windows plays before the session ends. Call it from a worker thread, never the GTK thread.
/// Like Explorer it skips files of 4 MB or more, and it never waits longer than the sound plus
/// a second (at most 6 s; Windows' hung-app timeout ends a stuck one similarly).
pub fn play_sync(ev: Event) {
    let Some(f) = file_for(ev) else { return };
    let Ok(data) = std::fs::read(&f) else { return };
    if data.len() >= 0x40_0000 { return; }
    let limit = (wav_duration(&data).unwrap_or(Duration::from_secs(5)) + Duration::from_secs(1)).min(Duration::from_secs(6));
    let end = Instant::now() + limit;
    for p in players() {
        let Some(mut c) = spawn_player(*p, &f, ev.label(), ev.key()) else { continue };
        let t0 = Instant::now();
        loop {
            match c.try_wait() {
                Ok(Some(st)) if !st.success() && t0.elapsed() < FAST_FAIL && Instant::now() < end => break,
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
                _ => { let _ = c.kill(); let _ = c.wait(); return; }
            }
        }
    }
}

// ------------------------------------------------------------------ console beep

/// A sine tone as a 16-bit stereo .wav (with short fades so it doesn't click).
pub fn tone_wav(freq: f64, ms: u32, rate: u32, amplitude: f64) -> Vec<u8> {
    let n = (rate as u64 * ms as u64 / 1000) as usize;
    let fade = (rate as usize / 200).min(n / 2).max(1);
    let mut pcm = Vec::with_capacity(n * 4);
    for i in 0..n {
        let env = (i.min(n - 1 - i) as f64 / fade as f64).min(1.0);
        let v = (amplitude * env * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() * 32767.0) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    w
}

/// kernel32!Beep(800, 200) — what the Windows 7 console does for BEL. Not a scheme event ("No
/// Sounds" doesn't silence it). Beeps queue up and play one after another like the synchronous
/// Beep(); a few at most wait, so `cat` of a binary file doesn't beep for minutes.
pub fn console_beep() {
    static TX: OnceLock<Mutex<mpsc::SyncSender<()>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<()>(3);
        let _ = std::thread::Builder::new().name("aerode-beep".into()).spawn(move || {
            let dir = gtk::glib::user_cache_dir().join("aerode");
            let f = dir.join("beep800.wav");
            if !f.is_file() {
                let _ = std::fs::create_dir_all(&dir);
                let tmp = dir.join(format!("beep800.{}.tmp", std::process::id()));
                if std::fs::write(&tmp, tone_wav(800.0, 200, 48000, 0.5)).is_ok() { let _ = std::fs::rename(&tmp, &f); }
            }
            while rx.recv().is_ok() {
                for p in players() {
                    let Some(mut c) = spawn_player(*p, &f, "Beep", "") else { continue };
                    let t0 = Instant::now();
                    let st = loop {
                        match c.try_wait() {
                            Ok(Some(st)) => break Some(st),
                            Ok(None) if t0.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(10)),
                            _ => { let _ = c.kill(); let _ = c.wait(); break None; }
                        }
                    };
                    if !matches!(st, Some(s) if !s.success() && t0.elapsed() < FAST_FAIL) { break; }
                }
            }
        });
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).try_send(());
}

// ------------------------------------------------------------------ startup and logon

/// The Windows 7 startup tune (WAVE resource 5080 of imageres.dll), saved by aerode-extract.
pub const STARTUP_FILE: &str = "Windows 7 Startup.wav";

/// Save the startup tune from an imageres.dll.
pub fn extract_startup(imageres: &Path, out: &Path) -> bool {
    let Ok(pe) = crate::pe::Pe::open(imageres) else { return false };
    let Some(wav) = pe.get("WAVE", 5080u16).filter(|w| w.starts_with(b"RIFF")) else { return false };
    let tmp = out.with_extension("tmp");
    std::fs::write(&tmp, wav).is_ok() && std::fs::rename(&tmp, out).is_ok()
}

/// The startup tune; taken from the imageres.dll kept by an older extraction when needed.
pub fn startup_file() -> Option<PathBuf> {
    let dst = sounds_dir().join(STARTUP_FILE);
    if dst.is_file() { return Some(dst); }
    let dll = resolve_ci(&config::data_dir().join("sysfiles"), "Windows/System32/imageres.dll")?;
    extract_startup(&dll, &dst).then_some(dst)
}

/// Sound > Sounds > "Play Windows Startup sound" (Win7: LogonUI\BootAnimation\DisableStartupSound).
/// Kept apart from the scheme, like in Windows.
pub fn startup_sound_enabled() -> bool { !config::conf_dir().join("no-startup-sound").exists() }

pub fn set_startup_sound(on: bool) {
    let f = config::conf_dir().join("no-startup-sound");
    if on { let _ = std::fs::remove_file(f); } else { let _ = std::fs::create_dir_all(config::conf_dir()); let _ = std::fs::write(f, ""); }
}

/// Is this the first shell of this logon session? Explorer keeps a volatile session key
/// ("LogonSoundHasBeenPlayed", StartupHasBeenRun) so restarting it replays neither the logon sound
/// nor the Startup programs. Here it is a root window property, which lives exactly as long as the
/// X session; its value tells a later logon on the same X server apart (AERODE_SESSION is set by
/// the session script, so `aerode restart` and crash restarts inherit it).
pub fn first_shell_of_session() -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let token = std::env::var("AERODE_SESSION").ok().filter(|s| !s.is_empty())
        .or_else(|| std::env::var("XDG_SESSION_ID").ok()).unwrap_or_default();
    let Ok((conn, screen)) = x11rb::connect(None) else { return true };
    let root = conn.setup().roots[screen].root;
    let Some(atom) = conn.intern_atom(false, b"_AERODE_SESSION_STARTED").ok().and_then(|c| c.reply().ok()).map(|r| r.atom) else { return true };
    // check and set under a server grab: two shells started together still play it once
    let _ = conn.grab_server();
    let old = conn.get_property(false, root, atom, AtomEnum::STRING, 0, 256).ok().and_then(|c| c.reply().ok())
        .filter(|r| r.type_ != u32::from(AtomEnum::NONE)).map(|r| r.value);
    let fresh = match old { None => true, Some(v) => !token.is_empty() && v != token.as_bytes() };
    if fresh { let _ = conn.change_property8(PropMode::REPLACE, root, atom, AtomEnum::STRING, token.as_bytes()); }
    let _ = conn.ungrab_server();
    let _ = conn.get_input_focus().ok().and_then(|c| c.reply().ok());
    fresh
}

/// First logon since boot? Windows plays the startup tune once per boot for the whole machine
/// (HKLM\…\LogonUI\LogonSoundPlayed), so the marker is machine-wide too.
fn first_logon_of_boot() -> bool {
    let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    let id = id.trim();
    if id.is_empty() { return false; }
    std::fs::OpenOptions::new().write(true).create_new(true)
        .open(std::env::temp_dir().join(format!("aerode-startup-sound-{id}"))).is_ok()
}

/// Is there a real output device yet? At login PipeWire/WirePlumber (socket-activated by the
/// shell's first audio client) set the sound card up a moment after the shell starts; a sound
/// started before that goes to the dummy output or fails.
fn output_ready() -> bool {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_default();
    let server = std::env::var_os("PULSE_SERVER").is_some() || rt.join("pulse/native").exists() || rt.join("pipewire-0").exists();
    if !server || !util::which("pactl") { return true; }
    let t = Duration::from_secs(2);
    let Some(def) = output_timeout(&["pactl", "get-default-sink"], t) else { return false };
    let def = def.trim();
    if def.is_empty() || def == "auto_null" { return false; }
    output_timeout(&["pactl", "list", "short", "sinks"], t)
        .map(|s| s.lines().any(|l| l.split('\t').nth(1) == Some(def))).unwrap_or(false)
}

/// The sound of a new logon session (call once, when [`first_shell_of_session`] says so).
/// Win7: on the first logon after boot AuthUI plays the startup tune *instead of* Windows Logon,
/// and nothing at all when "Play Windows Startup sound" is off; later logons play Windows Logon.
pub fn logon() {
    let first_boot = first_logon_of_boot();
    if first_boot && !startup_sound_enabled() { return; }
    let _ = std::thread::Builder::new().name("aerode-logon-sound".into()).spawn(move || {
        let (file, ev) = if first_boot {
            match startup_file() { Some(f) => (Some(f), None), None => (file_for(Event::Logon), Some(Event::Logon)) }
        } else { (file_for(Event::Logon), Some(Event::Logon)) };
        let Some(file) = file else { return };
        // wait for the sound card (a few seconds at most: a logon sound long after the desktop
        // appeared would be worse than none)
        let deadline = Instant::now() + Duration::from_secs(8);
        while !output_ready() && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(250)); }
        let (name, id) = ev.map(|e| (e.label(), e.key())).unwrap_or(("Windows Startup", ""));
        submit(Req { file, name: name.into(), id: id.into(), policy: Policy::Yield, retry_until: Some(deadline) });
    });
}

// ------------------------------------------------------------------ XDG sound theme

/// freedesktop sound names (as libcanberra programs use them: GTK dialogs, Firefox, Thunderbird…)
/// → the Win7 event that plays for them.
pub const THEME_NAMES: &[(&str, Event)] = &[
    ("bell", Event::DefaultBeep), ("bell-window-system", Event::DefaultBeep), ("bell-terminal", Event::DefaultBeep),
    ("audio-volume-change", Event::DefaultBeep),
    ("dialog-error", Event::CriticalStop), ("dialog-warning", Event::Exclamation),
    ("dialog-information", Event::Asterisk), ("dialog-question", Event::Question),
    ("message-new-email", Event::NewMail), ("message-new-instant", Event::NewMail),
    ("device-added", Event::DeviceConnect), ("device-removed", Event::DeviceDisconnect),
    ("battery-low", Event::LowBattery), ("battery-caution", Event::CriticalBattery),
    ("trash-empty", Event::EmptyRecycleBin),
    ("desktop-login", Event::Logon), ("desktop-logout", Event::Logoff),
    ("window-new", Event::OpenProgram), ("window-close", Event::CloseProgram),
    ("window-maximized", Event::Maximize), ("window-unmaximized", Event::Restore),
    ("window-minimized", Event::Minimize), ("window-unminimized", Event::RestoreUp),
    ("menu-popup", Event::MenuPopup), ("menu-click", Event::MenuCommand),
];

/// freedesktop sounds Windows has no event for but that do a job (alarms, test tones, shutter)
fn theme_keeps(name: &str) -> bool {
    ["alarm-clock", "audio-channel-", "audio-test", "camera-shutter", "screen-capture", "phone-"].iter().any(|p| name.starts_with(p))
}

/// Write ~/.local/share/sounds/Aerode from the current scheme: links to the scheme's files, and
/// "<name>.disabled" for silent events and for freedesktop sounds Windows doesn't make, so
/// libcanberra doesn't fall back to the freedesktop theme for them.
pub fn write_sound_theme() -> std::io::Result<()> {
    let dir = gtk::glib::user_data_dir().join("sounds/Aerode");
    let stereo = dir.join("stereo");
    std::fs::create_dir_all(&stereo)?;
    std::fs::write(dir.join("index.theme"), "[Sound Theme]\nName=Aerode\nComment=Windows 7 sound scheme (written by aerode)\n\
        Inherits=freedesktop\nDirectories=stereo\n\n[stereo]\nOutputProfile=stereo\n")?;
    for e in std::fs::read_dir(&stereo)?.flatten() { let _ = std::fs::remove_file(e.path()); }
    let mut done = std::collections::HashSet::new();
    for (name, ev) in THEME_NAMES {
        match file_for(*ev) {
            Some(f) => std::os::unix::fs::symlink(&f, stereo.join(format!("{name}.wav")))?,
            None => std::fs::write(stereo.join(format!("{name}.disabled")), "")?,
        }
        done.insert(name.to_string());
    }
    let mut dirs = vec![gtk::glib::user_data_dir()];
    dirs.extend(gtk::glib::system_data_dirs());
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d.join("sounds/freedesktop/stereo")) else { continue };
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            let Some(name) = f.rsplit_once('.').map(|(n, _)| n.to_string()) else { continue };
            if theme_keeps(&name) || !done.insert(name.clone()) { continue; }
            std::fs::write(stereo.join(format!("{name}.disabled")), "")?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ shell watchers

/// Background watchers in the shell: USB device plug/unplug, battery alarms (as Win7 does) and
/// the XDG sound theme following the scheme.
pub fn start_watchers() {
    use std::collections::HashSet;
    use std::{cell::RefCell, rc::Rc};
    fn usb() -> HashSet<String> {
        std::fs::read_dir("/sys/bus/usb/devices").map(|r| r.flatten()
            .filter(|e| !e.file_name().to_string_lossy().contains(':'))
            .map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    }
    // plug/unplug: right away from udev's events (Win7 plays them on arrival), the poll below
    // only catches what a missing udevadm would miss
    let known = Rc::new(RefCell::new(usb()));
    let check = { let known = known.clone(); Rc::new(move || {
        let now = usb();
        let mut k = known.borrow_mut();
        if now.difference(&k).next().is_some() { play_with(Event::DeviceConnect, Policy::Yield); }
        else if k.difference(&now).next().is_some() { play_with(Event::DeviceDisconnect, Policy::Yield); }
        *k = now;
    }) };
    let live = { let c = check.clone(); crate::util::watch_lines(&["udevadm", "monitor", "--kernel", "--subsystem-match=usb"],
        |l| l.contains(" add ") || l.contains(" remove "), std::time::Duration::from_millis(150), move || c()) };
    let mut warned = (false, false);
    let mut theme_for = 0u64;
    let mut ticks = 0u32;
    let mut tick = move || {
        ticks = ticks.wrapping_add(1);
        if !live || ticks % 5 == 0 { check(); }
        if let Some(b) = crate::sys::battery() {
            let (w, ev) = battery_alarm(warned, b.status == "Discharging", b.percent as i64);
            warned = w;
            if let Some(ev) = ev {
                play_with(ev, Policy::Yield);
                // stobject.dll's balloon (strings 421 / 407), then the alarm's sound
                crate::util::notify("Battery Meter", "battery-caution", "Plug in or find another power source",
                    &format!("Your battery is low ({}%). If you need to continue using your computer, either plug in your computer, or shut it down and then change the battery.", b.percent), true);
            }
        }
        let v = scheme_version();
        if v != theme_for {
            theme_for = v;
            if let Err(e) = write_sound_theme() { eprintln!("aerode: sound theme: {e}"); }
        }
    };
    tick();
    gtk::glib::timeout_add_seconds_local(2, move || { tick(); gtk::glib::ControlFlow::Continue });
}

/// Low (10 %) and critical (5 %) battery alarms: each once per discharge, and the low one never
/// after the critical one (starting or resuming at 4 % plays only the critical alarm).
fn battery_alarm(warned: (bool, bool), discharging: bool, percent: i64) -> ((bool, bool), Option<Event>) {
    if !discharging { return ((false, false), None); }
    if percent <= 5 && !warned.1 { return ((true, true), Some(Event::CriticalBattery)); }
    if percent <= 10 && !warned.0 { return ((true, warned.1), Some(Event::LowBattery)); }
    (warned, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win7_default_scheme() {
        assert_eq!(Event::DefaultBeep.default_file(), "Windows Ding.wav");
        assert_eq!(Event::Asterisk.default_file(), "Windows Error.wav");
        assert_eq!(Event::ChangeTheme.default_file(), "Windows Logon Sound.wav");
        assert_eq!(Event::NewFax.default_file(), "Windows Notify.wav");
        assert_eq!(Event::Question.default_file(), "");
        assert_eq!(Event::InfoBar.label(), "Information Bar");
        // keys are unique and round-trip
        for e in Event::ALL { assert_eq!(Event::from_key(e.key()), Some(e)); }
        assert_eq!(Event::ALL.iter().filter(|e| e.app() == App::Explorer).count(), 7);
        assert_eq!(Event::ALL.iter().filter(|e| e.app() == App::Speech).count(), 6);
        // the Sounds tab lists each application's events alphabetically
        for app in App::ALL {
            let l: Vec<&str> = Event::ALL.iter().filter(|e| e.app() == app).map(|e| e.label()).collect();
            let mut s = l.clone();
            s.sort();
            assert_eq!(l, s);
        }
    }

    #[test]
    fn named_schemes() {
        assert_eq!(scheme_file("Afternoon", Event::DefaultBeep), "Afternoon/Windows Ding.wav");
        assert_eq!(scheme_file("Raga", Event::PrintComplete), "Raga/Windows Print complete.wav");
        assert_eq!(scheme_file("Raga", Event::ExitWindows), "Windows Shutdown.wav");
        assert_eq!(scheme_file("Raga", Event::EmptyRecycleBin), "Windows Recycle.wav");
        assert_eq!(scheme_file("Raga", Event::SpeechOn), "Speech On.wav");
        assert_eq!(scheme_file("Raga", Event::Question), "");
        assert_eq!(scheme_file("Raga", Event::Minimize), "");
        assert_eq!(scheme_file(".Default", Event::PrintComplete), "");
        assert!(scheme_map(".None").values().all(|v| v.is_empty()));
        assert_eq!(scheme_map(".Default").len(), Event::ALL.len());
        // the alternate schemes assign 21 files of their own folder (Win7 hive)
        assert_eq!(Event::ALL.iter().filter(|e| scheme_file("Delta", **e).starts_with("Delta/")).count(), 21);
    }

    #[test]
    fn scheme_json() {
        let (m, muted) = parse_scheme(r#"{"sounds": {"WindowsLogon": "", "SystemHand": "x.wav", "bad": 3}, "sounds_muted": true, "clock_24h": false}"#);
        assert!(muted);
        assert_eq!(m.get("WindowsLogon").map(|s| s.as_str()), Some(""));
        assert_eq!(m.get("SystemHand").map(|s| s.as_str()), Some("x.wav"));
        assert!(!m.contains_key("bad"));
        assert_eq!(parse_scheme("not json"), (HashMap::new(), false));
    }

    #[test]
    fn tone_and_duration() {
        let w = tone_wav(800.0, 200, 48000, 0.5);
        assert_eq!(w.len(), 44 + 48000 / 5 * 4);
        let d = wav_duration(&w).unwrap();
        assert!((d.as_secs_f64() - 0.2).abs() < 1e-6, "{d:?}");
        assert_eq!(&w[0..4], b"RIFF");
        // starts and ends silent (fades), loud in the middle
        let s = |i: usize| i16::from_le_bytes([w[44 + i * 4], w[45 + i * 4]]);
        assert_eq!(s(0), 0);
        assert!(s(9599).abs() < 200);
        assert!((4000..5000).map(s).map(i16::abs).max().unwrap() > 16000);
        assert_eq!(wav_duration(b"RIFF\0\0\0\0WAVE"), None);
    }

    #[test]
    fn battery_alarms() {
        // resuming at 4 %: critical only, never low afterwards
        let (w, e) = battery_alarm((false, false), true, 4);
        assert_eq!(e, Some(Event::CriticalBattery));
        assert_eq!(battery_alarm(w, true, 4).1, None);
        // going down: low at 10 %, critical at 5 %, each once
        let (w, e) = battery_alarm((false, false), true, 10);
        assert_eq!(e, Some(Event::LowBattery));
        assert_eq!(battery_alarm(w, true, 9).1, None);
        assert_eq!(battery_alarm(w, true, 5).1, Some(Event::CriticalBattery));
        // plugging in re-arms both
        assert_eq!(battery_alarm(w, false, 9), ((false, false), None));
    }

    #[test]
    fn case_insensitive_lookup() {
        let d = std::env::temp_dir().join(format!("aerode-sounds-test-{}", std::process::id()));
        std::fs::create_dir_all(d.join("Afternoon")).unwrap();
        std::fs::write(d.join("Afternoon/Windows Ding.wav"), b"x").unwrap();
        assert_eq!(resolve_ci(&d, "afternoon/windows ding.WAV"), Some(d.join("Afternoon/Windows Ding.wav")));
        assert_eq!(resolve_ci(&d, "Afternoon\\Windows Ding.wav"), Some(d.join("Afternoon/Windows Ding.wav")));
        assert_eq!(resolve_ci(&d, "Garden/Windows Ding.wav"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
