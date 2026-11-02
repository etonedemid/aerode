//! Every Control Panel applet in one table: routing, the category pages, "All Control Panel Items"
//! (View by: Large/Small icons) and search are all driven from here. To add an applet: write its
//! page function (in pages/) and add one row.
use std::rc::Rc;

use super::pages::{admin, desktop, extras};
use super::Cp;

pub type Build = fn(&Rc<Cp>) -> gtk::Widget;

pub struct Applet {
    pub id: &'static str,
    /// the Windows 7 name
    pub name: &'static str,
    /// system | network | hardware | programs | users | appearance | clock | access
    pub cat: &'static str,
    pub icon: &'static str,
    /// task links shown under the applet on its category page: (label, page id)
    pub tasks: &'static [(&'static str, &'static str)],
    pub keywords: &'static str,
    pub build: Build,
}

pub fn category_name(cat: &str) -> &'static str {
    match cat {
        "system" => "System and Security",
        "network" => "Network and Internet",
        "hardware" => "Hardware and Sound",
        "programs" => "Programs",
        "users" => "User Accounts and Family Safety",
        "appearance" => "Appearance and Personalization",
        "clock" => "Clock, Language, and Region",
        _ => "Ease of Access",
    }
}

macro_rules! applet {
    ($id:expr, $name:expr, $cat:expr, $icon:expr, [$(($tl:expr, $tp:expr)),* $(,)?], $kw:expr, $build:expr) => {
        Applet { id: $id, name: $name, cat: $cat, icon: $icon, tasks: &[$(($tl, $tp)),*], keywords: $kw, build: $build }
    };
}

pub fn applets() -> Vec<Applet> {
    vec![
        // ------------------------------------------------ System and Security
        applet!("actioncenter", "Action Center", "system", "security-high", [("Review your computer's status and resolve issues", "actioncenter"), ("Troubleshoot common computer problems", "troubleshooting")], "action center security maintenance status messages problems", admin::page_actioncenter),
        applet!("firewall", "Windows Firewall", "system", "security-medium", [("Check firewall status", "firewall"), ("Allow a program through Windows Firewall", "firewall")], "firewall ufw firewalld ports allow block", admin::page_firewall),
        applet!("system", "System", "system", "computer", [("View amount of RAM and processor speed", "system"), ("See the name of this computer", "system")], "system computer name ram processor memory cpu hostname", |c| c.page_system()),
        applet!("update", "Windows Update", "system", "system-software-update", [("Turn automatic updating on or off", "update"), ("Check for updates", "update"), ("View installed updates", "update")], "update upgrade pacman packages patches", admin::page_update),
        applet!("power", "Power Options", "system", "preferences-system-power", [("Change battery settings", "power"), ("Change what the power buttons do", "power"), ("Change when the computer sleeps", "power")], "power battery sleep plan lid hibernate", |c| c.page_power()),
        applet!("backup", "Backup and Restore", "system", "drive-harddisk", [("Back up your computer", "backup"), ("Restore files from backup", "backup")], "backup restore snapshot timeshift snapper", admin::page_backup),
        applet!("bitlocker", "BitLocker Drive Encryption", "system", "drive-harddisk-system", [("Protect your computer by encrypting data on your disk", "bitlocker")], "bitlocker encryption luks crypt", admin::page_bitlocker),
        applet!("admintools", "Administrative Tools", "system", "applications-system", [("Free up disk space", "diskmgmt"), ("Create and format hard disk partitions", "diskmgmt"), ("View event logs", "eventvwr"), ("Schedule tasks", "taskschd")], "administrative tools admin management", admin::page_admintools),
        applet!("services", "Services", "system", "applications-system", [("View local services", "services")], "services daemons systemd units", |c| c.page_services()),
        applet!("startup", "Startup Programs", "system", "applications-system", [("Startup programs", "startup")], "startup autostart login programs", |c| c.page_startup()),
        applet!("eventvwr", "Event Viewer", "system", "utilities-system-monitor", [("View event logs", "eventvwr")], "event viewer logs journal errors warnings", admin::page_eventvwr),
        applet!("taskschd", "Task Scheduler", "system", "x-office-calendar", [("Schedule tasks", "taskschd")], "task scheduler timers cron schedule", admin::page_taskschd),
        applet!("diskmgmt", "Disk Management", "system", "drive-harddisk", [("Create and format hard disk partitions", "diskmgmt")], "disk management partitions volumes mount format drives", admin::page_diskmgmt),
        applet!("devmgr", "Device Manager", "hardware", "computer", [("Device Manager", "devmgr")], "device manager hardware drivers pci usb", admin::page_devmgr),
        applet!("sysinfo", "System Information", "system", "dialog-information", [("System Information", "sysinfo")], "system information msinfo hardware summary bios", admin::page_sysinfo),
        applet!("perfinfo", "Performance Information and Tools", "system", "utilities-system-monitor", [("Check the Windows Experience Index", "perfinfo")], "performance experience index benchmark", admin::page_perfinfo),
        applet!("recovery", "Recovery", "system", "drive-harddisk-system", [("Restore your computer to an earlier time", "recovery")], "recovery restore snapshot rollback", extras::page_recovery),
        applet!("troubleshooting", "Troubleshooting", "system", "help-browser", [("Find and fix problems", "troubleshooting")], "troubleshooting fix problems repair", extras::page_troubleshooting),
        // ------------------------------------------------ Network and Internet
        applet!("network", "Network and Sharing Center", "network", "network-workgroup", [("View network status and tasks", "network"), ("Connect to a network", "network")], "network internet wifi ethernet sharing", |c| c.page_network()),
        applet!("homegroup", "HomeGroup", "network", "folder-publicshare", [("Choose homegroup and sharing options", "homegroup")], "homegroup sharing samba share folders", extras::page_homegroup),
        applet!("internet", "Internet Options", "network", "web-browser", [("Change your homepage", "internet"), ("Manage browser add-ons", "internet")], "internet options browser proxy homepage", extras::page_internet),
        // ------------------------------------------------ Hardware and Sound
        applet!("devices", "Devices and Printers", "hardware", "printer", [("Add a device", "devices"), ("Mouse", "mouse"), ("Keyboard", "keyboard"), ("Device Manager", "devmgr")], "devices printers bluetooth", |c| c.page_devices()),
        applet!("autoplay", "AutoPlay", "hardware", "media-optical", [("Change default settings for media or devices", "autoplay"), ("Play CDs or other media automatically", "autoplay")], "autoplay automount media cd usb", extras::page_autoplay),
        applet!("sound", "Sound", "hardware", "audio-volume-high", [("Adjust system volume", "sound"), ("Change system sounds", "sound"), ("Manage audio devices", "sound")], "sound audio volume speakers microphone", |c| c.page_sound()),
        applet!("mobility", "Windows Mobility Center", "hardware", "computer-laptop", [("Adjust commonly used mobility settings", "mobility")], "mobility center laptop brightness presentation", extras::page_mobility),
        applet!("display", "Display", "appearance", "preferences-desktop-display", [("Adjust screen resolution", "display"), ("Connect to an external display", "display")], "display resolution screen monitor refresh", |c| c.page_display()),
        applet!("mouse", "Mouse", "hardware", "input-mouse", [("Change mouse settings", "mouse")], "mouse pointer cursor speed double click", |c| c.page_mouse()),
        applet!("keyboard", "Keyboard", "hardware", "preferences-desktop-keyboard", [("Change keyboard settings", "keyboard")], "keyboard repeat", |c| c.page_keyboard()),
        applet!("colormgmt", "Color Management", "hardware", "preferences-desktop-display", [("Change advanced color management settings", "colormgmt")], "color management profile icc calibration", extras::page_colormgmt),
        // ------------------------------------------------ Programs
        applet!("programs", "Programs and Features", "programs", "system-software-install", [("Uninstall a program", "programs"), ("View installed updates", "update")], "programs uninstall software features packages", |c| c.page_programs()),
        applet!("defaults", "Default Programs", "programs", "preferences-desktop-default-applications", [("Set your default programs", "defaults"), ("Make a file type always open in a specific program", "defaults")], "default programs browser file types", |c| c.page_defaults()),
        applet!("gadgets", "Desktop Gadgets", "programs", "aerode-gadgets", [("Add gadgets to the desktop", "gadgets")], "gadgets desktop clock calendar", desktop::page_gadgets),
        // ------------------------------------------------ User Accounts
        applet!("users", "User Accounts", "users", "system-users", [("Change account picture", "users"), ("Change your password", "users")], "user account password picture", |c| c.page_users()),
        applet!("credentials", "Credential Manager", "users", "dialog-password", [("Manage Windows Credentials", "credentials")], "credential manager passwords keyring wallet secrets", extras::page_credentials),
        // ------------------------------------------------ Appearance and Personalization
        applet!("personalization", "Personalization", "appearance", "preferences-desktop-wallpaper", [("Change the theme", "personalization"), ("Change desktop background", "personalization"), ("Change sound effects", "sound"), ("Change screen saver", "screensaver")], "personalization wallpaper background theme color", |c| c.page_category("appearance")),
        applet!("screensaver", "Screen Saver", "appearance", "preferences-desktop-display", [("Change screen saver", "screensaver")], "screen saver lock timeout blank", desktop::page_screensaver),
        applet!("desktopicons", "Desktop Icon Settings", "appearance", "user-desktop", [("Show or hide common icons on the desktop", "desktopicons")], "desktop icons computer recycle bin", desktop::page_desktopicons),
        applet!("taskbar", "Taskbar and Start Menu", "appearance", "preferences-system", [("Customize the Start menu", "taskbar"), ("Customize icons on the taskbar", "taskbar")], "taskbar start menu auto-hide small icons", desktop::page_taskbar),
        applet!("folderoptions", "Folder Options", "appearance", "folder", [("Show hidden files and folders", "folderoptions"), ("Specify single- or double-click to open", "folderoptions")], "folder options hidden files extensions click", desktop::page_folderoptions),
        applet!("fonts", "Fonts", "appearance", "preferences-desktop-font", [("Preview, delete, or show and hide fonts", "fonts"), ("Change font settings", "fonts")], "fonts typeface install cleartype", desktop::page_fonts),
        applet!("gettingstarted", "Getting Started", "system", "help-browser", [("Get started with aerode", "gettingstarted")], "getting started welcome help", desktop::page_gettingstarted),
        // ------------------------------------------------ Clock, Language, and Region
        applet!("datetime", "Date and Time", "clock", "preferences-system-time", [("Set the time and date", "datetime"), ("Change the time zone", "datetime")], "date time clock zone ntp", |c| c.page_datetime()),
        applet!("region", "Region and Language", "clock", "preferences-desktop-keyboard", [("Change keyboards or other input methods", "region"), ("Change location", "region")], "region language keyboard layout locale format", |c| c.page_region()),
        // ------------------------------------------------ Ease of Access
        applet!("access", "Ease of Access Center", "access", "preferences-desktop-accessibility", [("Let Windows suggest settings", "access"), ("Optimize visual display", "access"), ("Change how your keyboard works", "access")], "ease access magnifier narrator on-screen keyboard contrast", |c| c.page_access()),
    ]
}
