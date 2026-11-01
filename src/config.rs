//! User configuration (~/.config/aero7/config.json) and well-known paths.
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
    /// Aero glass colour (r, g, b, intensity)
    pub tint: [f64; 4],
    pub taskbar_height: i32,
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
            tint: [0.10, 0.22, 0.36, 0.52],
            taskbar_height: 40,
        }
    }
}

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(load());
}

pub fn data_dir() -> PathBuf {
    gtk::glib::user_data_dir().join("aero7")
}
pub fn conf_dir() -> PathBuf {
    gtk::glib::user_config_dir().join("aero7")
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
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn reload() {
    CONFIG.with(|c| *c.borrow_mut() = load());
}

pub fn get() -> Config {
    CONFIG.with(|c| c.borrow().clone())
}

pub fn update(f: impl FnOnce(&mut Config)) {
    CONFIG.with(|c| f(&mut c.borrow_mut()));
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
