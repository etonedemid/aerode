//! User configuration (~/.config/aerode/config.json) and well-known paths.
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub wallpaper: Option<String>,
    /// fill | fit | stretch | center
    pub wallpaper_mode: String,
    pub desktop_icons: Vec<String>,
    pub icon_size: i32,
    pub pinned: Option<Vec<String>>,
    pub usage: HashMap<String, u32>,
    pub clock_24h: bool,
    /// Aero glass colourisation (Personalize > Window Color): Win7's HKCU\Software\Microsoft\Windows\DWM
    /// values. Replaces the old `"tint": [r, g, b, intensity]` (migrated in `load`). While a session runs
    /// the live value is the root property `_AERODE_COLORIZATION` (see `glass::colorization`).
    pub colorization: Colorization,
    pub taskbar_height: i32,
    /// sound scheme overrides: event key -> file name/path ("" = none)
    pub sounds: HashMap<String, String>,
    pub sounds_muted: bool,
    /// desktop "Sort by": name | size | type | date
    pub desktop_sort: String,
    pub desktop_sort_desc: bool,
    /// keyboard layouts ("us", "de(nodeadkeys)"…); empty = whatever the system set up
    pub kbd_layouts: Vec<String>,
    /// XKB option switching input languages; None = Alt+Shift (Windows default), Some("") = none
    pub kbd_switch: Option<String>,
    /// notification-area icon behaviour per program: "show" (Show icon and notifications) or
    /// "hide" (Hide icon and notifications); absent = "notify" (Only show notifications, Win7's default)
    pub tray_icons: HashMap<String, String>,
    /// "Always show all icons and notifications on the taskbar"
    pub tray_show_all: bool,
    /// Control Panel ▸ System and Security: Windows Update, Action Center, Backup, Experience Index
    pub maintenance: Maintenance,
    /// Taskbar and Start Menu Properties (Control Panel)
    pub taskbar: TaskbarOptions,
    /// Folder Options (Explorer and the desktop)
    pub folder: FolderOptions,
    /// Screen Saver Settings
    pub screensaver: ScreenSaverOptions,
    /// Fonts: rendering (Xft/xsettings), hidden families
    pub fonts: FontOptions,
    /// Desktop Icon Settings > Change Icon: item ("computer", "home", "network", "trash", "trash-full", "control") -> icon name or file
    pub desktop_icon_art: HashMap<String, String>,
    /// AutoPlay: "Use AutoPlay for all media and devices"
    pub autoplay: bool,
    /// AutoPlay action per media type (key from autoplay::MEDIA): "ask" | "none" | "open" | "run" | "app:<desktop id>"
    pub autoplay_actions: HashMap<String, String>,
    /// Control Panel hardware / system applets: System Properties, Sound, Date and Time, Power
    /// Options, Mouse, Keyboard, Display text size
    pub hw: HwOptions,
    /// User Account Control Settings slider: 3 Always notify, 2 Default, 1 Notify without dimming
    /// the desktop, 0 Never notify (None = 2, the default)
    pub uac_level: Option<u32>,
    /// Region and Language > Location (ISO 3166 country code; "" = the one the locale names)
    pub location: String,
    /// Ease of Access settings that start each time you log on ("stickykeys" -> "1", "cursor" -> "large"…)
    pub ease_of_access: HashMap<String, String>,
}

/// The Win7 Visual Effects list (System Properties > Advanced > Performance Settings), in its order:
/// (key in `HwOptions::effects_off`, label).
pub const VISUAL_EFFECTS: &[(&str, &str)] = &[
    ("animate_controls", "Animate controls and elements inside windows"),
    ("animate_minmax", "Animate windows when minimizing and maximizing"),
    ("animate_taskbar", "Animations in the taskbar and Start Menu"),
    ("aero_peek", "Enable Aero Peek"),
    ("composition", "Enable desktop composition"),
    ("glass", "Enable transparent glass"),
    ("menu_fade", "Fade or slide menus into view"),
    ("tooltip_fade", "Fade or slide ToolTips into view"),
    ("menu_item_fade", "Fade out menu items after clicking"),
    ("thumb_cache", "Save taskbar thumbnail previews"),
    ("pointer_shadow", "Show shadows under mouse pointer"),
    ("window_shadow", "Show shadows under windows"),
    ("thumbnails", "Show thumbnails instead of icons"),
    ("selection_rect", "Show translucent selection rectangle"),
    ("drag_full", "Show window contents while dragging"),
    ("combo_slide", "Slide open combo boxes"),
    ("font_smoothing", "Smooth edges of screen fonts"),
    ("list_smooth", "Smooth-scroll list boxes"),
    ("icon_shadow", "Use drop shadows for icon labels on the desktop"),
    ("visual_styles", "Use visual styles on windows and buttons"),
];

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct HwOptions {
    /// Performance Options > Visual Effects: auto | appearance | performance | custom
    pub visual_effects: String,
    /// visual effects turned off (keys of `VISUAL_EFFECTS`); see `HwOptions::effect`
    pub effects_off: Vec<String>,
    /// Performance Options > Advanced > Processor scheduling: programs | services
    pub scheduling: String,
    /// System Properties > Hardware > Device Installation Settings: auto | ask | never
    pub driver_downloads: String,
    /// System Properties > Remote: Remote Assistance (x11vnc on localhost, started at logon)
    pub remote_assistance: bool,
    /// Sound > Communications: mute | reduce80 | reduce50 | none
    pub comms_action: String,
    /// Sound: the Default Communication Device (PulseAudio/PipeWire name, "" = the default device)
    pub comms_sink: String,
    pub comms_source: String,
    /// Date and Time > Additional Clocks (Win7 has two)
    pub clocks: Vec<ExtraClock>,
    /// Date and Time: "Notify me when the clock changes" (daylight saving time)
    pub dst_notify: bool,
    /// Display: text size in dots per inch (96 = 100 %, 120 = 125 %, 144 = 150 %)
    pub dpi: u32,
    pub power: PowerOptions,
    pub mouse: MouseOptions,
    pub keyboard: KeyboardOptions,
}

impl Default for HwOptions {
    fn default() -> Self {
        HwOptions { visual_effects: "auto".into(), effects_off: vec![], scheduling: "programs".into(), driver_downloads: "auto".into(),
            remote_assistance: false, comms_action: "reduce80".into(), comms_sink: String::new(), comms_source: String::new(),
            clocks: vec![], dst_notify: true, dpi: 96, power: PowerOptions::default(), mouse: MouseOptions::default(),
            keyboard: KeyboardOptions::default() }
    }
}

impl HwOptions {
    /// Is this visual effect (a key of `VISUAL_EFFECTS`) turned on?
    pub fn effect(&self, key: &str) -> bool { !self.effects_off.iter().any(|k| k == key) }
}

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
pub struct ExtraClock {
    pub show: bool,
    /// IANA time zone ("Europe/Berlin")
    pub tz: String,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct PowerOptions {
    /// the active plan: balanced | power-saver | performance | an id of `custom`
    pub plan: String,
    /// per-plan settings changed in Control Panel (missing = the Win7 defaults, see `power::defaults`)
    pub plans: HashMap<String, PowerPlan>,
    /// plans made with "Create a power plan"
    pub custom: Vec<CustomPlan>,
    /// "Require a password on wakeup"
    pub require_password: bool,
    /// "Show additional plans" expanded
    pub show_more: bool,
}

impl Default for PowerOptions {
    fn default() -> Self {
        PowerOptions { plan: "balanced".into(), plans: HashMap::new(), custom: vec![], require_password: true, show_more: false }
    }
}

/// Timers in minutes (0 = never), brightness in percent; `_dc` = on battery, `_ac` = plugged in.
#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Debug)]
#[serde(default)]
pub struct PowerPlan {
    pub display_dc: u32,
    pub display_ac: u32,
    pub sleep_dc: u32,
    pub sleep_ac: u32,
    pub bright_dc: u32,
    pub bright_ac: u32,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct CustomPlan {
    pub id: String,
    pub name: String,
    /// the plan it was based on (its power profile): balanced | power-saver | performance
    pub base: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct MouseOptions {
    /// Buttons: switch primary and secondary buttons
    pub left_handed: bool,
    /// double-click time in ms (Win7 default 500)
    pub double_click: u32,
    /// ClickLock (libinput tap-and-drag lock) and how long to hold before it locks (ms)
    pub click_lock: bool,
    pub click_lock_ms: u32,
    /// Pointer Options: speed -1..1 (libinput Accel Speed), Enhance pointer precision (adaptive profile)
    pub speed: f64,
    pub precision: bool,
    /// Hide pointer while typing (unclutter/xbanish when installed)
    pub hide_typing: bool,
    /// Wheel: lines per notch, or one screen at a time; characters per tilt
    pub wheel_lines: u32,
    pub wheel_page: bool,
    pub wheel_chars: u32,
    /// Pointers: cursor theme ("" = the session default)
    pub cursor_theme: String,
    pub cursor_shadow: bool,
}

impl Default for MouseOptions {
    fn default() -> Self {
        MouseOptions { left_handed: false, double_click: 500, click_lock: false, click_lock_ms: 1200, speed: 0.0, precision: true,
            hide_typing: true, wheel_lines: 3, wheel_page: false, wheel_chars: 3, cursor_theme: String::new(), cursor_shadow: true }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct KeyboardOptions {
    /// repeat delay in ms (Win7: 250 / 500 / 750 / 1000) and repeat rate in characters per second;
    /// 0 = never set here (the X server's own values stay)
    pub delay: u32,
    pub rate: u32,
    /// cursor blink time in ms (0 = no blinking)
    pub blink: u32,
}

impl Default for KeyboardOptions {
    fn default() -> Self { KeyboardOptions { delay: 0, rate: 0, blink: 530 } }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct TaskbarOptions {
    pub locked: bool,
    pub autohide: bool,
    pub small_icons: bool,
    /// bottom | left | right | top
    pub location: String,
    /// always | full | never
    pub buttons: String,
    pub aero_peek: bool,
    /// shutdown | switchuser | logoff | lock | restart | sleep | hibernate
    pub power_button: String,
    /// Privacy: store and display recently opened programs / items
    pub track_programs: bool,
    pub track_items: bool,
    /// Customize Start Menu: number of recent programs / Jump List items
    pub recent_programs: u32,
    pub jump_items: u32,
    /// Customize Start Menu items: key -> link | menu | hide | on | off (missing = default)
    pub start_items: HashMap<String, String>,
    /// taskbar toolbars: address | links | tabletpc | desktop
    pub toolbars: Vec<String>,
}

impl Default for TaskbarOptions {
    fn default() -> Self {
        TaskbarOptions { locked: true, autohide: false, small_icons: false, location: "bottom".into(), buttons: "always".into(), aero_peek: true,
            power_button: "shutdown".into(), track_programs: true, track_items: true, recent_programs: 10, jump_items: 10,
            start_items: HashMap::new(), toolbars: vec![] }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct FolderOptions {
    pub own_window: bool,
    pub single_click: bool,
    /// single-click mode: underline titles only when pointed at (else always, "consistent with my browser")
    pub underline_hover: bool,
    pub nav_all_folders: bool,
    pub nav_expand: bool,
    pub icons_only: bool,
    pub always_menus: bool,
    pub file_icon_on_thumbs: bool,
    pub size_in_tips: bool,
    pub full_path_title: bool,
    pub show_hidden: bool,
    pub hide_empty_drives: bool,
    pub hide_ext: bool,
    pub hide_protected: bool,
    pub separate_process: bool,
    pub restore_windows: bool,
    pub drive_letters: bool,
    pub color_compressed: bool,
    pub popup_desc: bool,
    pub preview_handlers: bool,
    pub checkboxes: bool,
    pub sharing_wizard: bool,
    /// when typing into list view: true = type into the search box, false = select the typed item
    pub type_to_search: bool,
    /// Search tab
    pub search_contents: bool,
    pub search_subfolders: bool,
    pub search_partial: bool,
    pub search_natural: bool,
    pub search_no_index: bool,
    pub search_system_dirs: bool,
    pub search_compressed: bool,
}

impl Default for FolderOptions {
    fn default() -> Self {
        FolderOptions { own_window: false, single_click: false, underline_hover: false, nav_all_folders: false, nav_expand: false,
            icons_only: false, always_menus: false, file_icon_on_thumbs: true, size_in_tips: true, full_path_title: false, show_hidden: false,
            hide_empty_drives: true, hide_ext: true, hide_protected: true, separate_process: false, restore_windows: false, drive_letters: true,
            color_compressed: false, popup_desc: true, preview_handlers: true, checkboxes: false, sharing_wizard: true, type_to_search: false,
            search_contents: false, search_subfolders: true, search_partial: true, search_natural: false, search_no_index: false,
            search_system_dirs: true, search_compressed: false }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct ScreenSaverOptions {
    /// "" = (None) | blank | bubbles | mystify | photos | ribbons
    pub name: String,
    /// minutes of inactivity
    pub wait: u32,
    /// On resume, display logon screen
    pub lock: bool,
    /// Photos: folder ("" = Pictures), slow | medium | fast, shuffle
    pub photos_dir: String,
    pub photos_speed: String,
    pub photos_shuffle: bool,
}

impl Default for ScreenSaverOptions {
    fn default() -> Self {
        ScreenSaverOptions { name: String::new(), wait: 10, lock: true, photos_dir: String::new(), photos_speed: "medium".into(), photos_shuffle: false }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct FontOptions {
    pub antialias: bool,
    pub hinting: bool,
    /// hintnone | hintslight | hintmedium | hintfull
    pub hintstyle: String,
    /// rgb | bgr | vrgb | vbgr | none (ClearType off = grayscale)
    pub rgba: String,
    /// families hidden with Fonts > Hide (rejected through fontconfig)
    pub hidden: Vec<String>,
    pub hide_by_language: bool,
    pub install_as_shortcut: bool,
}

impl Default for FontOptions {
    fn default() -> Self {
        FontOptions { antialias: true, hinting: true, hintstyle: "hintfull".into(), rgba: "rgb".into(), hidden: vec![], hide_by_language: true, install_as_shortcut: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Maintenance {
    /// Windows Update: "" / "check" = look for updates once a day, "never"
    pub update_mode: String,
    /// Windows Update: leave out AUR updates (yay/paru)
    pub update_no_aur: bool,
    /// Action Center message kinds that are turned off ("firewall", "updates", "backup", …)
    pub ac_off: Vec<String>,
    /// Backup and Restore: the home-folder backup set up there
    pub backup: Option<Backup>,
    /// Performance Information and Tools: the last Windows Experience Index assessment
    pub wei: Option<Wei>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Backup {
    /// folder on the backup drive that holds the copy
    pub dest: String,
    /// folders under home to back up; empty = the whole home folder
    pub folders: Vec<String>,
    /// "daily" | "weekly" | "monthly" | "" (no schedule)
    pub schedule: String,
    /// weekday (0 = Sunday) or day of the month
    pub day: u32,
    pub hour: u32,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Wei {
    pub cpu: f64,
    pub mem: f64,
    pub gfx: f64,
    pub game: f64,
    pub disk: f64,
    /// unix time of the assessment
    pub when: i64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            wallpaper: None,
            wallpaper_mode: "fill".into(),
            desktop_icons: vec!["computer".into(), "home".into(), "trash".into()],
            icon_size: 48,
            pinned: None,
            usage: HashMap::new(),
            clock_24h: false,
            colorization: Colorization::default(),
            taskbar_height: 40,
            sounds: HashMap::new(),
            sounds_muted: false,
            desktop_sort: "name".into(),
            desktop_sort_desc: false,
            kbd_layouts: vec![],
            kbd_switch: None,
            tray_icons: HashMap::new(),
            tray_show_all: false,
            maintenance: Maintenance::default(),
            taskbar: TaskbarOptions::default(),
            folder: FolderOptions::default(),
            screensaver: ScreenSaverOptions::default(),
            fonts: FontOptions::default(),
            desktop_icon_art: HashMap::new(),
            autoplay: true,
            autoplay_actions: HashMap::new(),
            hw: HwOptions::default(),
            uac_level: None,
            location: String::new(),
            ease_of_access: HashMap::new(),
        }
    }
}

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(load());
}

pub fn data_dir() -> PathBuf {
    gtk::glib::user_data_dir().join("aerode")
}
pub fn conf_dir() -> PathBuf {
    gtk::glib::user_config_dir().join("aerode")
}
pub fn home() -> PathBuf {
    gtk::glib::home_dir()
}
pub fn data_file(name: &str) -> Option<PathBuf> {
    let p = data_dir().join(name);
    p.exists().then_some(p)
}

fn load() -> Config {
    std::fs::read_to_string(conf_dir().join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|mut v| {
            migrate(&mut v);
            serde_json::from_value(v).unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Older configs kept the glass colour as `"tint": [r, g, b, intensity]`: turn it into `"colorization"`.
fn migrate(v: &mut serde_json::Value) {
    let Some(o) = v.as_object_mut() else { return };
    let Some(tint) = o.remove("tint") else { return };
    if o.contains_key("colorization") { return; }
    let t: Option<[f64; 4]> = serde_json::from_value(tint).ok();
    if let Some(c) = t.map(Colorization::from_tint).and_then(|c| serde_json::to_value(c).ok()) {
        o.insert("colorization".into(), c);
    }
}

/// `config.json`'s `"colorization"`: what Win7 keeps under HKCU\Software\Microsoft\Windows\DWM,
/// colours as "#AARRGGBB".
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default)]
pub struct Colorization {
    #[serde(with = "argb_hex")]
    pub color: u32,
    #[serde(with = "argb_hex")]
    pub afterglow: u32,
    pub color_balance: u32,
    pub afterglow_balance: u32,
    pub blur_balance: u32,
    pub reflection_intensity: u32,
    /// "Enable transparency" (Win7's ColorizationOpaqueBlend = !transparency)
    pub transparency: bool,
}

impl Default for Colorization {
    fn default() -> Self { crate::dwmcolor::Colorization::default().into() }
}

impl From<crate::dwmcolor::Colorization> for Colorization {
    fn from(c: crate::dwmcolor::Colorization) -> Self {
        Colorization { color: c.color, afterglow: c.afterglow, color_balance: c.color_balance,
            afterglow_balance: c.afterglow_balance, blur_balance: c.blur_balance,
            reflection_intensity: c.reflection_intensity, transparency: !c.opaque }
    }
}

impl From<Colorization> for crate::dwmcolor::Colorization {
    fn from(c: Colorization) -> Self {
        crate::dwmcolor::Colorization { color: c.color, afterglow: c.afterglow, color_balance: c.color_balance.min(100),
            afterglow_balance: c.afterglow_balance.min(100), blur_balance: c.blur_balance.min(100),
            reflection_intensity: c.reflection_intensity.min(100), opaque: !c.transparency }
    }
}

/// The swatches of the old Personalize page (`tint` rgb), in the order of `dwmcolor::SWATCHES`.
const OLD_SWATCHES: [[f64; 3]; 16] = [
    [0.10, 0.22, 0.36], [0.05, 0.13, 0.35], [0.03, 0.33, 0.38], [0.13, 0.35, 0.10], [0.40, 0.52, 0.05], [0.62, 0.55, 0.05],
    [0.62, 0.32, 0.04], [0.50, 0.04, 0.06], [0.52, 0.05, 0.40], [0.62, 0.35, 0.48], [0.30, 0.10, 0.50], [0.48, 0.45, 0.62],
    [0.42, 0.37, 0.28], [0.24, 0.12, 0.05], [0.25, 0.27, 0.30], [0.82, 0.86, 0.90],
];

impl Colorization {
    /// Migration of the old tint: its default and the old swatches become that Win7 swatch;
    /// a custom colour keeps its rgb with the intensity as alpha.
    pub fn from_tint(t: [f64; 4]) -> Colorization {
        use crate::dwmcolor::{intensity_to_alpha, SWATCHES};
        let same = |a: &[f64; 3]| (0..3).all(|k| (a[k] - t[k]).abs() < 1e-6);
        if let Some(i) = OLD_SWATCHES.iter().position(same) {
            return crate::dwmcolor::Colorization::from_cpl(SWATCHES[i].1, false).into();
        }
        let b = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        let alpha = intensity_to_alpha((t[3] * 100.0).round().max(0.0) as u32);
        crate::dwmcolor::Colorization::from_cpl(alpha << 24 | b(t[0]) << 16 | b(t[1]) << 8 | b(t[2]), false).into()
    }

    pub fn dwm(&self) -> crate::dwmcolor::Colorization { (*self).into() }
}

impl Config {
    /// Compatibility for code that still reads the old `tint` ([r, g, b, intensity] 0..1), derived
    /// from the saved colourisation.
    pub fn tint(&self) -> [f64; 4] {
        let c = self.colorization.color;
        if c == crate::dwmcolor::SWATCHES[0].1 { return [0.10, 0.22, 0.36, 0.52]; } // Sky, the old default
        let ch = |s: u32| ((c >> s) & 0xFF) as f64 / 255.0;
        [ch(16), ch(8), ch(0), (((c >> 24) as f64 / 255.0 - 0.30) / 0.25).clamp(0.0, 1.0)]
    }
}

mod argb_hex {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u32, s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&format!("#{v:08X}")) }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        let s = String::deserialize(d)?;
        let h = s.trim().trim_start_matches('#');
        let v = u32::from_str_radix(h, 16).map_err(serde::de::Error::custom)?;
        // "#RRGGBB" = opaque
        Ok(if h.len() <= 6 { 0xFF00_0000 | v } else { v })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_migration() {
        let sky = crate::dwmcolor::Colorization::default();
        let mut v = serde_json::json!({"tint": [0.10, 0.22, 0.36, 0.52], "clock_24h": true});
        migrate(&mut v);
        let c: Config = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(c.colorization.dwm(), sky);
        assert!(c.clock_24h && v.get("tint").is_none());
        // an old swatch at any intensity -> that Win7 swatch
        assert_eq!(Colorization::from_tint([0.50, 0.04, 0.06, 0.3]).color, 0xA8CE0F0F);
        // custom colour: rgb kept, intensity -> alpha
        let c = Colorization::from_tint([0.2, 0.4, 0.6, 0.5]);
        assert_eq!(c.color, 0x8033_6699);
        assert!(c.transparency);
        // round trip through config.json
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"color\":\"#80336699\""));
        assert_eq!(serde_json::from_str::<Colorization>(&s).unwrap(), c);
        // a config that already has "colorization" ignores a stale tint
        let mut v = serde_json::json!({"tint": [0.2, 0.4, 0.6, 0.5], "colorization": {"color": "#A8CE0F0F"}});
        migrate(&mut v);
        let c: Config = serde_json::from_value(v).unwrap();
        assert_eq!(c.colorization.color, 0xA8CE0F0F);
    }
}

pub fn reload() {
    CONFIG.with(|c| *c.borrow_mut() = load());
}

pub fn get() -> Config {
    CONFIG.with(|c| c.borrow().clone())
}

pub fn update(f: impl FnOnce(&mut Config)) {
    // read-modify-write against the file: several processes (shell, Control Panel, Explorer) share
    // it, and saving a stale in-memory copy would undo what another one just changed
    CONFIG.with(|c| { let mut c = c.borrow_mut(); if conf_dir().join("config.json").exists() { *c = load(); } f(&mut c); });
    save();
}

pub fn save() {
    let dir = conf_dir();
    let _ = std::fs::create_dir_all(&dir);
    let json = CONFIG.with(|c| serde_json::to_string_pretty(&*c.borrow()).unwrap());
    let tmp = dir.join("config.json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(tmp, dir.join("config.json"));
    }
}
