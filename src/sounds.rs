//! Windows 7 default sound scheme ("Windows Default"), event → .wav from the
//! user's own extracted Windows\Media folder.  Overridable per event in
//! config.json ("sounds": {"DeviceConnect": "/path.wav" | ""}).
use crate::{config, util};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    StartWindows, Logon, Logoff, ExitWindows,
    DefaultBeep, Asterisk, CriticalStop, Exclamation, Question,
    DeviceConnect, DeviceDisconnect, DeviceFail,
    LowBattery, CriticalBattery,
    SystemNotification, NewMail, PrintComplete, UserAccountControl,
    EmptyRecycleBin, StartNavigation, PopupBlocked, InfoBar, FeedDiscovered,
    Minimize, Restore, MenuCommand,
}

impl Event {
    pub fn key(self) -> &'static str {
        use Event::*;
        match self {
            StartWindows => "SystemStart", Logon => "WindowsLogon", Logoff => "WindowsLogoff", ExitWindows => "SystemExit",
            DefaultBeep => ".Default", Asterisk => "SystemAsterisk", CriticalStop => "SystemHand", Exclamation => "SystemExclamation",
            Question => "SystemQuestion", DeviceConnect => "DeviceConnect", DeviceDisconnect => "DeviceDisconnect",
            DeviceFail => "DeviceFail", LowBattery => "LowBatteryAlarm", CriticalBattery => "CriticalBatteryAlarm",
            SystemNotification => "SystemNotification", NewMail => "MailBeep", PrintComplete => "PrintComplete",
            UserAccountControl => "WindowsUAC", EmptyRecycleBin => "EmptyRecycleBin", StartNavigation => "Navigating",
            PopupBlocked => "BlockedPopup", InfoBar => "SecurityBand", FeedDiscovered => "FeedDiscovered",
            Minimize => "Minimize", Restore => "RestoreDown", MenuCommand => "MenuCommand",
        }
    }

    /// Label shown in Control Panel → Sound.
    pub fn label(self) -> &'static str {
        use Event::*;
        match self {
            StartWindows => "Start Windows", Logon => "Windows Logon", Logoff => "Windows Logoff", ExitWindows => "Exit Windows",
            DefaultBeep => "Default Beep", Asterisk => "Asterisk", CriticalStop => "Critical Stop", Exclamation => "Exclamation",
            Question => "Question", DeviceConnect => "Device Connect", DeviceDisconnect => "Device Disconnect",
            DeviceFail => "Device Failed to Connect", LowBattery => "Low Battery Alarm", CriticalBattery => "Critical Battery Alarm",
            SystemNotification => "System Notification", NewMail => "New Mail Notification", PrintComplete => "Print Complete",
            UserAccountControl => "Windows User Account Control", EmptyRecycleBin => "Empty Recycle Bin",
            StartNavigation => "Start Navigation", PopupBlocked => "Blocked Pop-up Window", InfoBar => "Notification bar",
            FeedDiscovered => "Feed Discovered", Minimize => "Minimize", Restore => "Restore Down", MenuCommand => "Menu Command",
        }
    }

    /// Default assignment in the Windows 7 scheme ("" = no sound).
    pub fn default_file(self) -> &'static str {
        use Event::*;
        match self {
            StartWindows => "Windows Startup.wav", Logon => "Windows Logon Sound.wav", Logoff => "Windows Logoff Sound.wav",
            ExitWindows => "Windows Shutdown.wav", DefaultBeep => "Windows Default.wav", Asterisk => "Windows Ding.wav",
            CriticalStop => "Windows Critical Stop.wav", Exclamation => "Windows Exclamation.wav", Question => "",
            DeviceConnect => "Windows Hardware Insert.wav", DeviceDisconnect => "Windows Hardware Remove.wav",
            DeviceFail => "Windows Hardware Fail.wav", LowBattery => "Windows Battery Low.wav",
            CriticalBattery => "Windows Battery Critical.wav", SystemNotification => "Windows Balloon.wav",
            NewMail => "Windows Notify.wav", PrintComplete => "", UserAccountControl => "Windows User Account Control.wav",
            EmptyRecycleBin => "Windows Recycle.wav", StartNavigation => "Windows Navigation Start.wav",
            PopupBlocked => "Windows Pop-up Blocked.wav", InfoBar => "Windows Information Bar.wav",
            FeedDiscovered => "Windows Feed Discovered.wav", Minimize => "", Restore => "", MenuCommand => "",
        }
    }

    pub const ALL: [Event; 26] = {
        use Event::*;
        [StartWindows, Logon, Logoff, ExitWindows, DefaultBeep, Asterisk, CriticalStop, Exclamation, Question,
         DeviceConnect, DeviceDisconnect, DeviceFail, LowBattery, CriticalBattery, SystemNotification, NewMail,
         PrintComplete, UserAccountControl, EmptyRecycleBin, StartNavigation, PopupBlocked, InfoBar, FeedDiscovered,
         Minimize, Restore, MenuCommand]
    };
}

/// Resolve the file currently assigned to an event (None = silent).
pub fn file_for(ev: Event) -> Option<std::path::PathBuf> {
    let cfg = config::get();
    if cfg.sounds_muted { return None; }
    let assigned = cfg.sounds.get(ev.key()).cloned().unwrap_or_else(|| ev.default_file().to_string());
    if assigned.is_empty() { return None; }
    let p = std::path::PathBuf::from(&assigned);
    if p.is_absolute() { return p.exists().then_some(p); }
    let dir = config::data_dir().join("sounds");
    std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path())
        .find(|f| f.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(&assigned)).unwrap_or(false))
}

pub fn play(ev: Event) {
    if let Some(f) = file_for(ev) { play_file(&f); }
}

pub fn play_file(f: &std::path::Path) {
    let p = f.to_string_lossy();
    for pl in [&["pw-play"][..], &["paplay"][..], &["aplay", "-q"][..]] {
        let mut a: Vec<&str> = pl.to_vec();
        a.push(&p);
        if util::spawn(&a) { return; }
    }
}

/// Background watchers: USB device plug/unplug and battery alarms (as Win7 does).
pub fn start_watchers() {
    use std::collections::HashSet;
    fn usb() -> HashSet<String> {
        std::fs::read_dir("/sys/bus/usb/devices").map(|r| r.flatten()
            .filter(|e| !e.file_name().to_string_lossy().contains(':'))
            .map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    }
    let mut known = usb();
    let mut warned = (false, false);
    gtk::glib::timeout_add_seconds_local(2, move || {
        let now = usb();
        if now.difference(&known).next().is_some() { play(Event::DeviceConnect); }
        else if known.difference(&now).next().is_some() { play(Event::DeviceDisconnect); }
        known = now;
        if let Some(b) = crate::sys::battery() {
            let discharging = b.status == "Discharging";
            if discharging && b.percent <= 5 && !warned.1 { warned.1 = true; play(Event::CriticalBattery); }
            else if discharging && b.percent <= 10 && !warned.0 { warned.0 = true; play(Event::LowBattery); }
            if !discharging { warned = (false, false); }
        }
        gtk::glib::ControlFlow::Continue
    });
}
