//! Extract the original Windows 7 artwork from the user's own ISO/WIM.
//!
//! Nothing from Microsoft ships with Aerode; this reads YOUR ISO and writes:
//!   ~/.local/share/icons/Aerode/          icon theme (real Win7 icons)
//!   ~/.local/share/icons/Aerode-cursors/  Xcursor theme (aero_* cursors)
//!   ~/.local/share/fonts/aerode/          Segoe UI etc.
//!   ~/.local/share/aerode/                orb, wallpapers, sounds, avatars, aero.msstyles
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::pe::{self, Pe, ResId};

const SIZES: [u32; 5] = [16, 24, 32, 48, 256];

const WANTED: &[&str] = &[
    "Windows/System32/imageres.dll", "Windows/System32/shell32.dll", "Windows/explorer.exe",
    "Windows/System32/cmd.exe", "Windows/System32/notepad.exe", "Windows/System32/calc.exe",
    "Windows/System32/mspaint.exe", "Windows/System32/SndVol.exe", "Windows/System32/taskmgr.exe",
    "Windows/System32/control.exe", "Windows/System32/timedate.cpl", "Windows/System32/pnidui.dll", "Windows/System32/SnippingTool.exe",
    "Windows/System32/DeviceCenter.dll", "Windows/System32/SndVolSSO.dll", "Windows/System32/batmeter.dll", "Windows/System32/ActionCenter.dll", "Windows/System32/wscui.cpl", "Windows/System32/powercpl.dll",
    "Windows/System32/timedate.cpl", "Windows/System32/mmsys.cpl", "Windows/System32/main.cpl",
    "Windows/System32/desk.cpl", "Windows/System32/themecpl.dll", "Windows/System32/sysdm.cpl",
    "Windows/System32/appwiz.cpl", "Windows/System32/inetcpl.cpl", "Windows/System32/netcenter.dll",
    "Windows/System32/ieframe.dll", "Windows/System32/comctl32.dll", "Windows/System32/user32.dll",
    "Windows/System32/zipfldr.dll", "Windows/System32/mstsc.exe", "Windows/System32/sdclt.exe",
    "Windows/System32/StikyNot.exe", "Windows/System32/magnify.exe", "Windows/System32/osk.exe",
    "Windows/System32/wordpad.exe", "Windows/System32/sndrec32.exe", "Windows/System32/SoundRecorder.exe",
    "Windows/System32/xpsrchvw.exe", "Windows/System32/perfmon.exe", "Windows/System32/regedit.exe",
    "Windows/regedit.exe", "Windows/HelpPane.exe", "Windows/notepad.exe",
    "Program Files/Internet Explorer/iexplore.exe", "Program Files/Windows Media Player/wmplayer.exe",
    "Program Files/Windows NT/Accessories/wordpad.exe", "Program Files/Windows Photo Viewer/PhotoViewer.dll",
    "Program Files/Windows Sidebar/sidebar.exe", "Program Files/Windows Journal/Journal.exe",
    "Program Files/Microsoft Games/Solitaire/Solitaire.exe", "Program Files/Microsoft Games/Minesweeper/MineSweeper.exe",
    "Windows/Resources/Themes/aero", "Windows/Resources/Themes/aero.theme",
    "Windows/Web/Wallpaper", "Windows/Web/Screen", "Windows/Media", "Windows/Cursors",
    "Windows/System32/oobe/info/backgrounds", "Windows/System32/oobe/background.bmp",
    "Windows/Fonts/segoeui.ttf", "Windows/Fonts/segoeuib.ttf", "Windows/Fonts/segoeuii.ttf",
    "Windows/Fonts/segoeuiz.ttf", "Windows/Fonts/seguisb.ttf", "Windows/Fonts/segoeuil.ttf",
    "Windows/Fonts/consola.ttf", "Windows/Fonts/consolab.ttf", "Windows/Fonts/calibri.ttf",
    "Windows/Fonts/tahoma.ttf", "Windows/Fonts/lucon.ttf", "Windows/Fonts/segoesc.ttf",
    "Windows/Fonts/seguisym.ttf", "Windows/Fonts/marlett.ttf",
    "Windows/Fonts/vgaoem.fon", "Windows/Fonts/dosapp.fon", "Windows/Fonts/vga850.fon",
    "ProgramData/Microsoft/User Account Pictures",
    "Windows/System32/wmploc.dll", "Windows/System32/sud.dll", "Windows/System32/isoburn.exe", "Windows/System32/msxml3.dll",
    // device, applet and accessory icons (see ICONS)
    "Windows/System32/DDORes.dll", "Windows/System32/mmres.dll", "Windows/System32/wpdshext.dll", "Windows/System32/bthprops.cpl",
    "Windows/System32/ExplorerFrame.dll", "Windows/System32/wucltux.dll", "Windows/System32/intl.cpl", "Windows/System32/telephon.cpl",
    "Windows/System32/charmap.exe", "Windows/System32/winver.exe", "Windows/System32/eventvwr.exe", "Windows/System32/msinfo32.exe",
    "Windows/System32/FirewallControlPanel.dll", "Windows/System32/rstrui.exe", "Windows/System32/WFS.exe", "Windows/System32/fontview.exe",
    "Windows/System32/cleanmgr.exe", "Windows/System32/Narrator.exe", "Windows/System32/dmdskres.dll", "Windows/ehome/ehshell.exe",
    "Program Files/Microsoft Games/Chess/Chess.exe", "Program Files/Microsoft Games/Mahjong/Mahjong.exe",
];

/// (freedesktop icon name, PE file, group id or None = first group, aliases)
const ICONS: &[(&str, &str, Option<u16>, &[&str])] = &[
    ("computer", "Windows/System32/imageres.dll", Some(109), &["user-desktop-computer", "computer-symbolic"]),
    ("user-trash", "Windows/System32/imageres.dll", Some(55), &["user-trash-symbolic"]),
    ("user-trash-full", "Windows/System32/imageres.dll", Some(54), &["user-trash-full-symbolic"]),
    ("user-home", "Windows/System32/imageres.dll", Some(123), &["folder-home", "user-home-symbolic"]),
    // "-symbolic" aliases: the navigation pane of Win7 (and so of GTK file dialogs here) uses full-colour icons
    ("folder", "Windows/System32/imageres.dll", Some(3), &["inode-directory", "folder-templates", "folder-symbolic",
        // menu categories: every program group in Win7's All Programs is a plain folder
        "applications-accessories", "applications-development", "applications-engineering", "applications-internet", "applications-multimedia",
        "applications-office", "applications-other", "applications-science", "applications-utilities"]),
    ("folder-open", "Windows/System32/imageres.dll", Some(4), &["folder-drag-accept", "folder-visiting"]),
    ("folder-documents", "Windows/System32/imageres.dll", Some(112), &["folder-documents-symbolic"]),
    ("folder-pictures", "Windows/System32/imageres.dll", Some(113), &["folder-pictures-symbolic"]),
    ("folder-music", "Windows/System32/imageres.dll", Some(108), &["folder-music-symbolic"]),
    ("folder-videos", "Windows/System32/imageres.dll", Some(189), &["folder-videos-symbolic"]),
    ("folder-download", "Windows/System32/imageres.dll", Some(184), &["folder-downloads", "folder-download-symbolic"]),
    ("user-desktop", "Windows/System32/imageres.dll", Some(183), &["desktop", "user-desktop-symbolic"]),
    ("folder-recent", "Windows/System32/imageres.dll", Some(117), &["document-open-recent", "document-open-recent-symbolic", "folder-recent-symbolic"]),
    ("starred", "Windows/System32/imageres.dll", Some(1024), &["emblem-favorite", "user-bookmarks", "folder-favorites", "bookmark-new", "starred-symbolic", "user-bookmarks-symbolic"]),
    ("folder-saved-search", "Windows/System32/imageres.dll", Some(18), &["folder-saved-search-symbolic"]),
    ("folder-games", "Windows/System32/imageres.dll", Some(186), &[]),
    ("folder-publicshare", "Windows/System32/imageres.dll", Some(178), &["folder-publicshare-symbolic"]),
    ("folder-new", "Windows/System32/shell32.dll", Some(16784), &["folder-new-symbolic"]),
    ("network-workgroup", "Windows/System32/imageres.dll", Some(25), &["network", "folder-remote", "network-server", "folder-network", "network-workgroup-symbolic", "network-server-symbolic", "folder-remote-symbolic"]),
    ("drive-harddisk", "Windows/System32/imageres.dll", Some(32), &["drive-multidisk", "drive-harddisk-symbolic", "drive-multidisk-symbolic"]),
    ("drive-harddisk-system", "Windows/System32/imageres.dll", Some(36), &["drive-harddisk-system-symbolic"]),
    ("drive-removable-media", "Windows/System32/imageres.dll", Some(35), &["drive-removable-media-usb", "drive-removable-media-usb-pendrive", "media-removable", "drive-removable-media-symbolic", "media-removable-symbolic", "drive-removable-media-usb-symbolic"]),
    ("media-flash", "Windows/System32/imageres.dll", Some(60), &["media-flash-sd", "media-flash-symbolic"]),
    ("media-floppy", "Windows/System32/imageres.dll", Some(28), &["media-floppy-symbolic"]),
    ("media-tape", "Windows/System32/imageres.dll", Some(43), &["media-tape-symbolic"]),
    ("drive-optical", "Windows/System32/imageres.dll", Some(30), &["drive-optical-symbolic"]),
    ("media-optical", "Windows/System32/imageres.dll", Some(61), &["media-optical-cd", "media-optical-dvd", "media-optical-symbolic"]),
    ("camera-photo", "Windows/System32/imageres.dll", Some(57), &["camera", "camera-photo-symbolic"]),
    ("camera-video", "Windows/System32/imageres.dll", Some(46), &[]),
    ("camera-web", "Windows/System32/DDORes.dll", Some(2555), &["cheese", "org.gnome.Cheese", "kamoso", "org.kde.kamoso", "guvcview"]),
    ("phone", "Windows/System32/imageres.dll", Some(47), &["phone-symbolic"]),
    ("pda", "Windows/System32/imageres.dll", Some(93), &[]),
    ("modem", "Windows/System32/telephon.cpl", Some(100), &[]),
    ("input-keyboard", "Windows/System32/imageres.dll", Some(182), &[]),
    ("input-gaming", "Windows/System32/DDORes.dll", Some(2207), &["preferences-desktop-gaming", "jstest-gtk"]),
    ("input-tablet", "Windows/System32/DDORes.dll", Some(2202), &["preferences-desktop-tablet"]),
    ("scanner", "Windows/System32/imageres.dll", Some(95), &[]),
    ("computer-laptop", "Windows/System32/imageres.dll", Some(1008), &[]),
    // portable media players / MTP phones (gvfs mounts); the media player *app* is multimedia-video-player
    ("multimedia-player", "Windows/System32/wpdshext.dll", Some(701), &["multimedia-player-symbolic"]),
    // audio devices as the Sound control panel shows them
    ("audio-speakers", "Windows/System32/mmres.dll", Some(3010), &[]),
    ("audio-headphones", "Windows/System32/mmres.dll", Some(3011), &[]),
    ("audio-headset", "Windows/System32/mmres.dll", Some(3015), &["blueman-headset"]),
    ("audio-input-microphone", "Windows/System32/mmres.dll", Some(3014), &[]),
    ("audio-card", "Windows/System32/mmres.dll", Some(3018), &[]),
    ("bluetooth", "Windows/System32/bthprops.cpl", Some(151), &["preferences-system-bluetooth", "bluetooth-active", "network-bluetooth", "blueman", "blueman-tray", "blueman-active", "blueberry"]),
    // file types – the icons Win7 itself shows for them
    ("unknown", "Windows/System32/imageres.dll", Some(2), &["application-octet-stream", "application-x-generic", "application-pdf", "x-office-spreadsheet", "x-office-presentation", "x-office-drawing", "application-x-zerosize",
        "x-office-spreadsheet-template", "x-office-presentation-template", "x-office-drawing-template"]),
    ("text-x-generic", "Windows/System32/imageres.dll", Some(102), &["text-plain", "text-x-generic-template"]),
    ("text-x-script", "Windows/System32/imageres.dll", Some(68), &["application-x-shellscript", "text-x-python", "text-x-python3", "application-x-perl", "application-x-ruby", "application-x-executable-script"]),
    ("text-html", "Windows/System32/ieframe.dll", Some(110), &["application-xhtml+xml"]),
    ("text-xml", "Windows/System32/msxml3.dll", Some(128), &["application-xml"]),
    ("image-x-generic", "Program Files/Windows Photo Viewer/PhotoViewer.dll", Some(3005), &["image-png", "image-jpeg", "image-bmp", "image-gif", "image-tiff", "image-webp", "image-x-icon"]),
    ("audio-x-generic", "Windows/System32/imageres.dll", Some(131), &["audio-flac", "audio-x-flac", "audio-ogg", "audio-x-vorbis+ogg", "audio-x-opus+ogg"]),
    ("audio-mpeg", "Windows/System32/wmploc.dll", Some(732), &["audio-x-mp3", "audio-mp3", "audio-x-mpeg"]),
    ("audio-x-wav", "Windows/System32/wmploc.dll", Some(734), &["audio-wav", "audio-x-wave"]),
    ("audio-x-ms-wma", "Windows/System32/wmploc.dll", Some(735), &[]),
    ("audio-mp4", "Windows/System32/wmploc.dll", Some(738), &["audio-x-m4a", "audio-m4a", "audio-aac"]),
    ("video-x-generic", "Windows/System32/imageres.dll", Some(133), &["video-x-matroska", "video-webm"]),
    ("video-mp4", "Windows/System32/wmploc.dll", Some(739), &[]),
    ("video-x-m4v", "Windows/System32/wmploc.dll", Some(737), &[]),
    ("video-x-msvideo", "Windows/System32/wmploc.dll", Some(731), &["video-avi"]),
    ("video-mpeg", "Windows/System32/wmploc.dll", Some(733), &["video-x-mpeg"]),
    ("video-x-ms-wmv", "Windows/System32/wmploc.dll", Some(736), &["video-x-ms-asf"]),
    ("x-office-document", "Program Files/Windows NT/Accessories/wordpad.exe", Some(154), &["application-rtf", "text-rtf", "application-msword", "x-office-document-template"]),
    ("application-vnd.openxmlformats-officedocument.wordprocessingml.document", "Program Files/Windows NT/Accessories/wordpad.exe", Some(156), &[]),
    ("application-vnd.oasis.opendocument.text", "Program Files/Windows NT/Accessories/wordpad.exe", Some(157), &[]),
    ("font-x-generic", "Windows/System32/imageres.dll", Some(125), &["application-x-font-ttf", "font-ttf"]),
    ("application-x-font-otf", "Windows/System32/imageres.dll", Some(127), &["font-otf"]),
    ("application-x-cd-image", "Windows/System32/isoburn.exe", None, &["application-x-iso9660-image", "application-x-raw-disk-image", "k3b", "org.kde.k3b", "brasero", "org.gnome.Brasero", "xfburn", "org.xfce.xfburn"]),
    ("mail-message", "Windows/System32/imageres.dll", Some(20), &["internet-mail", "mail-send", "message-rfc822", "mail-unread", "mail-read"]),
    ("x-office-calendar", "Windows/System32/shell32.dll", Some(326), &["office-calendar"]),
    ("x-office-address-book", "Windows/System32/imageres.dll", Some(181), &["contact-new", "folder-contacts"]),
    ("application-x-executable", "Windows/System32/imageres.dll", Some(15), &["application-x-sharedlib", "application-x-pie-executable", "application-x-msdownload", "application-x-ms-dos-executable"]),
    ("system-run", "Windows/System32/imageres.dll", Some(100), &[]),
    // dialogs, session, common actions
    ("dialog-information", "Windows/System32/imageres.dll", Some(81), &["gtk-dialog-info", "help-about"]),
    ("dialog-warning", "Windows/System32/imageres.dll", Some(84), &["gtk-dialog-warning"]),
    ("dialog-error", "Windows/System32/imageres.dll", Some(98), &["gtk-dialog-error"]),
    ("dialog-question", "Windows/System32/imageres.dll", Some(99), &["gtk-dialog-question"]),
    ("dialog-password", "Windows/System32/imageres.dll", Some(82), &["gtk-dialog-authentication", "seahorse", "org.gnome.seahorse.Application", "kwalletmanager", "kwalletmanager2", "org.kde.kwalletmanager5"]),
    // the UAC shield (aerode-polkit asks for aerode-uac-shield first)
    ("security-medium", "Windows/System32/imageres.dll", Some(78), &["aerode-uac-shield"]),
    ("security-low", "Windows/System32/imageres.dll", Some(105), &[]),
    ("image-missing", "Windows/System32/imageres.dll", Some(5102), &[]),
    ("action-unavailable", "Windows/System32/imageres.dll", Some(1027), &[]),
    ("system-users", "Windows/System32/imageres.dll", Some(88), &["preferences-system-users", "user-info", "preferences-desktop-user"]),
    ("avatar-default", "Windows/System32/imageres.dll", Some(1029), &[]),
    ("system-lock-screen", "Windows/System32/imageres.dll", Some(59), &["changes-prevent", "channel-secure-symbolic", "channel-secure"]),
    ("system-log-out", "Windows/System32/shell32.dll", Some(27), &[]),
    ("system-shutdown", "Windows/System32/shell32.dll", Some(28), &[]),
    ("system-suspend", "Windows/System32/shell32.dll", Some(221), &["system-hibernate"]),
    ("document-new", "Windows/System32/shell32.dll", Some(319), &[]),
    ("document-save", "Windows/System32/shell32.dll", Some(16761), &["document-save-as"]),
    ("edit-cut", "Windows/System32/shell32.dll", Some(16762), &[]),
    ("edit-copy", "Windows/System32/shell32.dll", Some(243), &[]),
    ("edit-paste", "Windows/System32/shell32.dll", Some(16763), &[]),
    ("edit-delete", "Windows/System32/shell32.dll", Some(240), &[]),
    ("edit-find", "Windows/System32/shell32.dll", Some(23), &[]),
    ("view-refresh", "Windows/System32/shell32.dll", Some(16739), &["view-refresh-symbolic"]),
    ("view-fullscreen", "Windows/System32/shell32.dll", Some(268), &[]),
    ("document-properties", "Windows/System32/shell32.dll", Some(16775), &[]),
    ("media-eject", "Windows/System32/shell32.dll", Some(331), &[]),
    ("go-up", "Windows/System32/xpsrchvw.exe", Some(116), &[]),
    ("go-down", "Windows/System32/xpsrchvw.exe", Some(118), &[]),
    ("go-home", "Windows/System32/ExplorerFrame.dll", Some(100), &[]),
    // Explorer command bar / search box glyphs (the explorer, start menu and control panel ask for these)
    ("system-search-symbolic", "Windows/System32/ExplorerFrame.dll", Some(20801), &["edit-find-symbolic"]),
    ("view-grid-symbolic", "Windows/System32/shell32.dll", Some(63009), &[]),
    ("view-dual-symbolic", "Windows/System32/shell32.dll", Some(16757), &[]),
    ("preferences-desktop-font", "Windows/System32/imageres.dll", Some(77), &[]),
    ("gnome-font-viewer", "Windows/System32/fontview.exe", Some(150), &["org.gnome.font-viewer", "kfontview", "org.kde.kfontview"]),
    ("preferences-desktop-default-applications", "Windows/System32/sud.dll", None, &[]),
    ("aerode-gadgets", "Program Files/Windows Sidebar/sidebar.exe", None, &["preferences-desktop-gadgets"]),
    ("system-search", "Windows/System32/imageres.dll", Some(8), &[]),
    ("printer", "Windows/System32/DeviceCenter.dll", None, &["preferences-devices-printer", "document-print", "system-config-printer", "printer-network", "cups"]),
    // the Start menu's Getting Started (the Welcome Center shortcut's icon: shellbrd.dll,-1075)
    ("aerode-gettingstarted", "Windows/Branding/ShellBrd/shellbrd.dll", Some(1075), &[]),
    ("help-browser", "Windows/HelpPane.exe", None, &["help-contents", "help-faq", "system-help", "yelp", "org.gnome.Yelp", "khelpcenter", "org.kde.khelpcenter"]),
    ("preferences-system", "Windows/System32/control.exe", None, &["preferences-desktop", "preferences-other", "gnome-control-center", "org.gnome.Settings", "systemsettings", "xfce4-settings-manager", "org.xfce.settings.manager", "aerode-control"]),
    ("system-file-manager", "Windows/explorer.exe", None, &["org.gnome.Nautilus", "nautilus", "thunar", "Thunar", "org.xfce.thunar", "pcmanfm", "pcmanfm-qt", "nemo", "caja", "dolphin", "org.kde.dolphin", "spacefm", "aerode-explorer"]),
    ("web-browser", "Program Files/Internet Explorer/iexplore.exe", None, &["firefox", "firefox-esr", "chromium", "google-chrome", "internet-web-browser", "org.mozilla.firefox", "brave-browser", "librewolf",
        "falkon", "org.kde.falkon", "konqueror", "org.kde.konqueror", "epiphany", "org.gnome.Epiphany", "vivaldi", "opera"]),
    ("utilities-terminal", "Windows/System32/cmd.exe", None, &["org.gnome.Terminal", "org.gnome.Console", "gnome-terminal", "xfce4-terminal", "org.xfce.terminal", "xterm", "lxterminal", "Alacritty", "kitty", "konsole", "org.kde.konsole",
        "yakuake", "org.kde.yakuake", "terminator", "com.gexperts.Tilix", "foot", "org.wezfurlong.wezterm", "com.mitchellh.ghostty", "guake", "terminal", "qterminal"]),
    ("accessories-text-editor", "Windows/System32/notepad.exe", None, &["org.gnome.gedit", "gedit", "org.gnome.TextEditor", "mousepad", "org.xfce.mousepad", "kate", "org.kde.kate", "kwrite", "org.kde.kwrite",
        "featherpad", "xed", "pluma", "leafpad", "l3afpad", "aerode-notepad", "text-editor"]),
    ("accessories-calculator", "Windows/System32/calc.exe", None, &["org.gnome.Calculator", "gnome-calculator", "galculator", "kcalc", "org.kde.kcalc", "org.kde.kalk", "qalculate", "qalculate-gtk", "qalculate-qt", "speedcrunch", "aerode-calc"]),
    ("accessories-character-map", "Windows/System32/charmap.exe", Some(111), &["gucharmap", "gnome-character-map", "org.gnome.Characters", "gnome-characters", "kcharselect", "org.kde.kcharselect"]),
    ("applications-graphics", "Windows/System32/mspaint.exe", None, &["kolourpaint", "org.kde.kolourpaint", "pinta", "drawing", "aerode-mspaint"]),
    ("multimedia-video-player", "Program Files/Windows Media Player/wmplayer.exe", None, &["vlc", "totem", "org.gnome.Totem", "mpv", "celluloid", "io.github.celluloid_player.Celluloid", "smplayer", "haruna", "org.kde.haruna",
        "dragonplayer", "org.kde.dragonplayer", "parole", "org.xfce.parole", "kaffeine", "multimedia-audio-player", "elisa", "org.kde.elisa", "rhythmbox", "org.gnome.Rhythmbox3",
        "org.gnome.Music", "audacious", "strawberry", "clementine"]),
    ("kodi", "Windows/ehome/ehshell.exe", None, &["tv.kodi.Kodi"]),
    ("accessories-word-processor", "Program Files/Windows NT/Accessories/wordpad.exe", None, &["libreoffice-writer", "abiword"]),
    ("document-viewer", "Windows/System32/xpsrchvw.exe", Some(100), &["evince", "org.gnome.Evince", "org.gnome.Papers", "okular", "org.kde.okular", "atril", "zathura", "org.pwmt.zathura", "qpdfview"]),
    ("xournalpp", "Program Files/Windows Journal/Journal.exe", Some(104), &["com.github.xournalpp.xournalpp", "xournal", "rnote", "com.github.flxzt.rnote"]),
    ("utilities-system-monitor", "Windows/System32/taskmgr.exe", None, &["gnome-system-monitor", "org.gnome.SystemMonitor", "lxtask", "xfce4-taskmanager", "org.xfce.taskmanager", "htop", "btop", "ksysguard",
        "plasma-systemmonitor", "org.kde.plasma-systemmonitor", "io.missioncenter.MissionCenter", "net.nokyan.Resources", "aerode-taskmgr"]),
    ("utilities-log-viewer", "Windows/System32/eventvwr.exe", Some(101), &["gnome-logs", "org.gnome.Logs", "ksystemlog", "org.kde.ksystemlog", "qjournalctl"]),
    ("hwinfo", "Windows/System32/msinfo32.exe", Some(10002), &["kinfocenter", "org.kde.kinfocenter", "hardinfo", "hardinfo2"]),
    ("multimedia-volume-control", "Windows/System32/SndVol.exe", None, &["pavucontrol", "org.pulseaudio.pavucontrol"]),
    // the Volume Mixer's / speaker flyout's mute buttons (SndVol.exe icon groups 110 and 111)
    ("aerode-sndvol-unmuted", "Windows/System32/SndVol.exe", Some(110), &[]),
    ("aerode-sndvol-muted", "Windows/System32/SndVol.exe", Some(111), &[]),
    ("preferences-desktop-sound", "Windows/System32/mmsys.cpl", Some(100), &["preferences-desktop-multimedia"]),
    // notification-area icons (the real Win7 tray art)
    ("audio-volume-muted", "Windows/System32/SndVolSSO.dll", Some(120), &["audio-volume-muted-blocking"]),
    ("audio-volume-zero", "Windows/System32/SndVolSSO.dll", Some(121), &[]),
    ("audio-volume-low", "Windows/System32/SndVolSSO.dll", Some(122), &[]),
    ("audio-volume-medium", "Windows/System32/SndVolSSO.dll", Some(123), &[]),
    ("audio-volume-high", "Windows/System32/SndVolSSO.dll", Some(124), &[]),
    ("audio-volume-error", "Windows/System32/SndVolSSO.dll", Some(125), &[]),
    ("network-wired", "Windows/System32/pnidui.dll", Some(3048), &["network-transmit-receive", "network-idle", "network-wired-symbolic"]),
    ("network-offline", "Windows/System32/pnidui.dll", Some(3020), &["network-wired-disconnected", "network-wireless-disconnected"]),
    ("network-error", "Windows/System32/pnidui.dll", Some(3051), &["network-wired-no-route", "network-wireless-no-route"]),
    ("network-wireless-signal-none", "Windows/System32/pnidui.dll", Some(3021), &[]),
    ("network-wireless-signal-weak", "Windows/System32/pnidui.dll", Some(3022), &[]),
    ("network-wireless-signal-ok", "Windows/System32/pnidui.dll", Some(3023), &[]),
    ("network-wireless-signal-good", "Windows/System32/pnidui.dll", Some(3024), &[]),
    ("network-wireless-signal-excellent", "Windows/System32/pnidui.dll", Some(3026), &["network-wireless"]),
    ("display-brightness", "Windows/System32/powercpl.dll", Some(515), &["display-brightness-symbolic"]),
    // Network and Sharing Center
    ("preferences-system-network", "Windows/System32/netcenter.dll", Some(1), &["network-workgroup-sharing"]),
    ("accessories-screenshot", "Windows/System32/SnippingTool.exe", None, &["org.gnome.Screenshot", "gnome-screenshot", "xfce4-screenshooter", "org.xfce.screenshooter", "flameshot", "org.flameshot.Flameshot",
        "ksnip", "spectacle", "org.kde.spectacle", "aerode-snip"]),
    ("accessories-notes", "Windows/System32/StikyNot.exe", None, &["xpad", "knotes", "org.kde.knotes", "indicator-stickynotes"]),
    // Ease of Access Center
    ("preferences-desktop-accessibility", "Windows/System32/imageres.dll", Some(86), &[]),
    ("kmag", "Windows/System32/magnify.exe", None, &["org.kde.kmag", "magnus"]),
    ("orca", "Windows/System32/Narrator.exe", None, &["org.gnome.Orca"]),
    ("preferences-desktop-keyboard", "Windows/System32/main.cpl", Some(200), &[]),
    ("input-keyboard-virtual", "Windows/System32/osk.exe", None, &["onboard", "onboard-panel", "florence"]),
    ("preferences-desktop-locale", "Windows/System32/intl.cpl", Some(200), &[]),
    ("image-viewer", "Program Files/Windows Photo Viewer/PhotoViewer.dll", None, &["eog", "org.gnome.eog", "org.gnome.Loupe", "gwenview", "org.kde.gwenview", "ristretto", "org.xfce.ristretto", "gpicview",
        "nomacs", "org.nomacs.ImageLounge", "qimgv", "feh", "viewnior", "lximage-qt", "gthumb", "org.gnome.gThumb"]),
    ("applications-games", "Program Files/Microsoft Games/Solitaire/Solitaire.exe", None, &["aisleriot", "sol", "kpat", "org.kde.kpat"]),
    ("gnome-mines", "Program Files/Microsoft Games/Minesweeper/MineSweeper.exe", Some(110), &["org.gnome.Mines", "kmines", "org.kde.kmines"]),
    ("gnome-chess", "Program Files/Microsoft Games/Chess/Chess.exe", None, &["org.gnome.Chess", "knights", "org.kde.knights", "pychess"]),
    ("gnome-mahjongg", "Program Files/Microsoft Games/Mahjong/Mahjong.exe", None, &["org.gnome.Mahjongg", "kmahjongg", "org.kde.kmahjongg"]),
    ("preferences-desktop-wallpaper", "Windows/System32/themecpl.dll", None, &["preferences-desktop-theme", "preferences-desktop-personal", "preferences-desktop-theme-global",
        "lxappearance", "nwg-look", "org.xfce.settings.appearance"]),
    ("preferences-system-time", "Windows/System32/timedate.cpl", None, &["preferences-system-date"]),
    // the Screen Saver Settings dialog is part of desk.cpl and carries its icon
    ("preferences-desktop-display", "Windows/System32/desk.cpl", None, &["video-display", "preferences-desktop-screensaver", "xscreensaver", "arandr", "lxrandr"]),
    ("input-mouse", "Windows/System32/main.cpl", None, &["preferences-desktop-peripherals", "preferences-desktop-mouse"]),
    ("system-software-install", "Windows/System32/appwiz.cpl", None, &["pamac", "org.manjaro.pamac.manager", "octopi", "plasmadiscover", "org.kde.discover", "gnome-software", "org.gnome.Software", "bauh"]),
    // Windows Update
    ("system-software-update", "Windows/System32/wucltux.dll", Some(1), &["software-update-available", "software-update-urgent", "pamac-tray-update", "update-notifier"]),
    ("preferences-system-power", "Windows/System32/powercpl.dll", None, &["battery", "xfce4-power-manager-settings", "org.xfce.powermanager", "preferences-system-power-management"]),
    ("security-high", "Windows/System32/wscui.cpl", None, &["preferences-system-privacy"]),
    ("firewall-config", "Windows/System32/FirewallControlPanel.dll", Some(1), &["preferences-security-firewall", "preferences-system-firewall", "gufw", "firewall-applet"]),
    ("regedit", "Windows/regedit.exe", None, &["dconf-editor", "ca.desrt.dconf-editor"]),
    // System Restore / Backup and Restore
    ("timeshift", "Windows/System32/rstrui.exe", None, &[]),
    ("backup", "Windows/System32/sdclt.exe", None, &["deja-dup", "org.gnome.DejaDup", "backintime", "kbackup", "org.gnome.World.PikaBackup", "com.borgbase.Vorta"]),
    ("gparted", "Windows/System32/dmdskres.dll", Some(344), &["partitionmanager", "org.kde.partitionmanager", "gnome-disks", "org.gnome.DiskUtility", "disk-utility"]),
    ("bleachbit", "Windows/System32/cleanmgr.exe", Some(104), &["org.bleachbit.BleachBit"]),
    ("baobab", "Windows/System32/shell32.dll", Some(275), &["org.gnome.baobab", "disk-usage-analyzer", "filelight", "org.kde.filelight", "qdirstat", "k4dirstat"]),
    // Windows Fax and Scan
    ("simple-scan", "Windows/System32/WFS.exe", None, &["org.gnome.SimpleScan", "skanlite", "org.kde.skanlite", "skanpage", "org.kde.skanpage", "xsane"]),
    ("start-here", "Windows/System32/winver.exe", None, &[]),
    ("package-x-generic", "Windows/System32/zipfldr.dll", None, &["application-zip", "file-roller", "org.gnome.FileRoller", "ark", "org.kde.ark", "xarchiver", "engrampa", "peazip",
        "application-x-archive", "application-x-tar", "application-x-compressed-tar", "application-x-7z-compressed", "application-x-rar", "application-vnd.rar",
        "application-gzip", "application-x-gzip", "application-x-bzip", "application-x-bzip2", "application-x-xz", "application-zstd", "application-x-zstd-compressed-tar",
        "application-x-xz-compressed-tar", "application-x-bzip-compressed-tar", "application-x-lzma"]),
    ("preferences-desktop-remote-desktop", "Windows/System32/mstsc.exe", None, &["remmina", "org.remmina.Remmina", "krdc", "org.kde.krdc", "org.gnome.Connections", "vinagre"]),
    ("utilities-system-monitor-perf", "Windows/System32/perfmon.exe", None, &[]),
    ("applications-system", "Windows/System32/sysdm.cpl", None, &[]),
    ("audio-recorder", "Windows/System32/SoundRecorder.exe", None, &["gnome-sound-recorder", "org.gnome.SoundRecorder"]),
    // Control Panel ▸ System and Security / administrative tools (pages fall back to the
    // freedesktop names when an older extraction lacks these)
    // the Action Center flag in the notification area (white) and the applet's blue flag
    ("aerode-ac-flag", "Windows/System32/ActionCenter.dll", Some(5), &[]),
    ("aerode-ac-flag-warning", "Windows/System32/ActionCenter.dll", Some(6), &[]),
    ("aerode-ac-flag-error", "Windows/System32/ActionCenter.dll", Some(7), &[]),
    ("aerode-ac-flag-clock", "Windows/System32/ActionCenter.dll", Some(8), &[]),
    ("aerode-actioncenter", "Windows/System32/ActionCenter.dll", Some(1), &[]),
    ("aerode-shield-ok", "Windows/System32/imageres.dll", Some(106), &[]),
    ("aerode-shield-error", "Windows/System32/imageres.dll", Some(105), &["security-low"]),
    ("aerode-shield-warning", "Windows/System32/imageres.dll", Some(107), &[]),
    ("aerode-shield-unknown", "Windows/System32/imageres.dll", Some(104), &[]),
    ("aerode-admintools", "Windows/System32/imageres.dll", Some(114), &[]),
    ("aerode-wu", "Windows/System32/wucltux.dll", Some(1), &[]),
    ("aerode-firewall", "Windows/System32/FirewallControlPanel.dll", Some(1), &[]),
    ("aerode-backup", "Windows/System32/sdcpl.dll", Some(1), &[]),
    ("aerode-bitlocker", "Windows/System32/fvecpl.dll", Some(1), &["aerode-bitlocker-unlocked"]),
    ("aerode-bitlocker-locked", "Windows/System32/fveui.dll", Some(1), &[]),
    ("aerode-perfinfo", "Windows/System32/wdc.dll", Some(108), &[]),
    ("aerode-compmgmt", "Windows/System32/mycomput.dll", Some(203), &[]),
    ("aerode-systools", "Windows/System32/mycomput.dll", Some(201), &[]),
    ("aerode-eventvwr", "Windows/System32/eventvwr.exe", Some(101), &[]),
    ("aerode-ev-log", "Windows/System32/els.dll", Some(100), &[]),
    ("aerode-ev-filter", "Windows/System32/els.dll", Some(308), &[]),
    ("aerode-taskschd", "Windows/System32/miguiresource.dll", Some(600), &["aerode-task"]),
    ("aerode-devmgr", "Windows/System32/devmgr.dll", Some(201), &[]),
    ("aerode-diskmgmt", "Windows/System32/dmdskres.dll", Some(344), &[]),
    ("aerode-dm-disk", "Windows/System32/dmdskres.dll", Some(362), &[]),
    ("aerode-dm-removable", "Windows/System32/dmdskres.dll", Some(369), &[]),
    ("aerode-dm-cdrom", "Windows/System32/dmdskres.dll", Some(370), &[]),
    ("aerode-msinfo", "Windows/System32/msinfo32.exe", Some(10002), &[]),
    ("aerode-msconfig", "Windows/System32/msconfig.exe", Some(128), &[]),
    ("aerode-services", "Windows/System32/filemgmt.dll", Some(236), &[]),
    ("aerode-cleanmgr", "Windows/System32/cleanmgr.exe", Some(104), &[]),
    ("aerode-mdsched", "Windows/System32/MdSched.exe", Some(5000), &[]),
    // Device Manager classes (setupapi.dll class icons)
    ("aerode-dev-display", "Windows/System32/setupapi.dll", Some(1), &[]),
    ("aerode-dev-mouse", "Windows/System32/setupapi.dll", Some(2), &[]),
    ("aerode-dev-keyboard", "Windows/System32/setupapi.dll", Some(3), &[]),
    ("aerode-dev-network", "Windows/System32/setupapi.dll", Some(5), &[]),
    ("aerode-dev-computer", "Windows/System32/setupapi.dll", Some(8), &[]),
    ("aerode-dev-ide", "Windows/System32/setupapi.dll", Some(9), &["aerode-dev-storage"]),
    ("aerode-dev-ports", "Windows/System32/setupapi.dll", Some(10), &[]),
    ("aerode-dev-unknown", "Windows/System32/setupapi.dll", Some(18), &[]),
    ("aerode-dev-usb", "Windows/System32/setupapi.dll", Some(20), &[]),
    ("aerode-dev-hid", "Windows/System32/setupapi.dll", Some(24), &[]),
    ("aerode-dev-system", "Windows/System32/setupapi.dll", Some(27), &[]),
    ("aerode-dev-cpu", "Windows/System32/setupapi.dll", Some(28), &[]),
    ("aerode-dev-memory", "Windows/System32/setupapi.dll", Some(32), &[]),
    ("aerode-dev-monitor", "Windows/System32/setupapi.dll", Some(35), &[]),
    ("aerode-dev-cdrom", "Windows/System32/setupapi.dll", Some(51), &[]),
    ("aerode-dev-disk", "Windows/System32/setupapi.dll", Some(53), &[]),
];

/// Windows path inside an extracted tree, case-insensitively.
pub fn find_ci(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for part in rel.split('/') {
        let hit = std::fs::read_dir(&cur).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        cur = hit.path();
    }
    Some(cur)
}

fn have(cmd: &str) -> bool { crate::util::which(cmd) }

fn locate_wim(src: &Path, tmp: &Path) -> Result<PathBuf> {
    let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "wim" || ext == "esd" { return Ok(src.to_path_buf()); }
    if !have("7z") { bail!("7z not found – install it:  sudo pacman -S 7zip"); }
    println!("· reading ISO …");
    let l = Command::new("7z").args(["l", "-ba"]).arg(src).output()?;
    let listing = String::from_utf8_lossy(&l.stdout);
    let name = listing.lines().filter_map(|l| l.split_whitespace().last())
        .find(|n| { let n = n.to_lowercase().replace('\\', "/"); n == "sources/install.wim" || n == "sources/install.esd" })
        .map(String::from).context("no sources/install.wim or install.esd in the ISO")?;
    let st = Command::new("7z").args(["e", "-y"]).arg(format!("-o{}", tmp.display())).arg(src).arg(&name).stdout(std::process::Stdio::null()).status()?;
    if !st.success() { bail!("7z failed to extract {name}"); }
    Ok(tmp.join(Path::new(&name.replace('\\', "/")).file_name().unwrap()))
}

fn extract_wim(wim: &Path, dest: &Path) -> Result<()> {
    if !have("wimlib-imagex") { bail!("wimlib-imagex not found – install it:  sudo pacman -S wimlib"); }
    println!("· extracting system files from {} …", wim.file_name().unwrap().to_string_lossy());
    let list = dest.with_extension("lst");
    std::fs::write(&list, WANTED.iter().map(|p| format!("/{p}\n")).collect::<String>())?;
    // use the last (most complete, usually Ultimate) edition
    let info = Command::new("wimlib-imagex").arg("info").arg(wim).output()?;
    let count = String::from_utf8_lossy(&info.stdout).lines()
        .find_map(|l| l.strip_prefix("Image Count:").and_then(|v| v.trim().parse::<u32>().ok())).unwrap_or(1);
    // only Windows 7 (NT 6.1): another Windows' icons/cursors/sounds would silently replace the Win7 ones
    let detail = Command::new("wimlib-imagex").arg("info").arg(wim).arg(count.to_string()).output()?;
    let detail = String::from_utf8_lossy(&detail.stdout).to_string();
    let field = |k: &str| detail.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string())).unwrap_or_default();
    let (major, minor, name) = (field("Major Version:"), field("Minor Version:"), field("Name:"));
    if !major.is_empty() && (major != "6" || minor != "1") {
        bail!("{} is not a Windows 7 image ({name}, version {major}.{minor}) – aerode needs a Windows 7 ISO", wim.display());
    }
    let st = Command::new("wimlib-imagex").arg("extract").arg(wim).arg(count.to_string()).arg(format!("@{}", list.display()))
        .arg(format!("--dest-dir={}", dest.display())).args(["--no-acls", "--preserve-dir-structure", "--nullglob"])
        .env("WIMLIB_IMAGEX_IGNORE_CASE", "1").stdout(std::process::Stdio::null()).status()?;
    if !st.success() { eprintln!("  (wimlib reported errors; continuing with what was extracted)"); }
    Ok(())
}

fn save_png(img: &image::RgbaImage, path: &Path) -> Result<()> {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    // an older theme may have this name as a symlink to another icon – never write through it
    let _ = std::fs::remove_file(path);
    img.save(path)?;
    Ok(())
}

fn best_for(imgs: &[image::RgbaImage], size: u32) -> Option<image::RgbaImage> {
    let exact = imgs.iter().filter(|i| i.width() == size).max_by_key(|i| i.pixels().filter(|p| p.0[3] > 0 && p.0[3] < 255).count());
    if let Some(e) = exact { return Some(e.clone()); }
    let src = imgs.iter().filter(|i| i.width() > size).min_by_key(|i| i.width()).or_else(|| imgs.iter().max_by_key(|i| i.width()))?;
    Some(image::imageops::resize(src, size, size, image::imageops::FilterType::Lanczos3))
}

fn build_icons(root: &Path, out: &Path) -> Result<usize> {
    println!("· building icon theme …");
    let mut n = 0;
    let mut cache: std::collections::HashMap<String, Option<Pe>> = Default::default();
    for (name, file, id, aliases) in ICONS {
        let pe = cache.entry(file.to_string()).or_insert_with(|| find_ci(root, file).and_then(|p| Pe::open(&p).ok()));
        let Some(pe) = pe else { continue };
        let imgs = pe.group_icon(id.map(ResId::Id));
        if imgs.is_empty() { eprintln!("  ! {name}: no icon {id:?} in {file}"); continue; }
        for s in SIZES {
            if let Some(img) = best_for(&imgs, s) {
                let dir = out.join(format!("{s}x{s}/apps"));
                save_png(&img, &dir.join(format!("{name}.png")))?;
                for a in *aliases {
                    let l = dir.join(format!("{a}.png"));
                    let _ = std::fs::remove_file(&l);
                    let _ = std::os::unix::fs::symlink(format!("{name}.png"), l);
                }
            }
        }
        n += 1;
    }
    n += build_battery(root, out);
    n += build_snip_tools(root, out);
    let dirs: Vec<String> = SIZES.iter().map(|s| format!("{s}x{s}/apps")).collect();
    let mut idx = format!("[Icon Theme]\nName=Aerode\nComment=Windows 7 icons (extracted locally)\nInherits=Papirus,breeze,Adwaita,hicolor\nDirectories={}\n\n", dirs.join(","));
    for s in SIZES {
        idx += &format!("[{s}x{s}/apps]\nSize={s}\nContext=Applications\nType={}\n\n", if s == 256 { "Scalable\nMinSize=64\nMaxSize=512" } else { "Fixed" });
    }
    std::fs::write(out.join("index.theme"), idx)?;
    let _ = Command::new("gtk-update-icon-cache").args(["-f", "-t"]).arg(out).output();
    println!("  {n} icons");
    Ok(n)
}

/// Battery icons from batmeter.dll's bitmap strips: the white notification-area battery (16/24 px)
/// and the coloured flyout battery (32/48 px), as `battery-level-N[-charging]` plus the classic names.
fn build_battery(root: &Path, out: &Path) -> usize {
    let Some(pe) = find_ci(root, "Windows/System32/batmeter.dll").and_then(|p| Pe::open(&p).ok()) else { return 0 };
    let strip = |id: u16| -> Option<Vec<image::RgbaImage>> {
        let img = pe::decode_dib(pe.get(pe::RT_BITMAP, ResId::Id(id))?, false).ok()?;
        let h = img.height();
        if h == 0 || img.width() % h != 0 { return None; }
        Some((0..img.width() / h).map(|i| image::imageops::crop_imm(&img, i * h, 0, h, h).to_image()).collect())
    };
    // 303 = 16px tray strip, 305 = 24px; 301 = 32px flyout strip, 302 = 40px
    let (Some(t16), Some(f32_)) = (strip(303), strip(301)) else { eprintln!("  ! batmeter.dll: no battery strips"); return 0 };
    let t24 = strip(305).unwrap_or_else(|| t16.clone());
    let f40 = strip(302).unwrap_or_else(|| f32_.clone());
    if t16.len() < 43 || f32_.len() < 31 { return 0; }
    let over = |base: &image::RgbaImage, top: &image::RgbaImage| { let mut b = base.clone(); image::imageops::overlay(&mut b, top, 0, 0); b };
    // tray: 0..=8 discharge levels, 18 = low (yellow !), 19 = critical (red x), 32..=42 charging levels, 30 = no battery
    let tray = |fr: &[image::RgbaImage], lvl: u32, charging: bool| -> image::RgbaImage {
        if charging { return fr[32 + (lvl / 10) as usize].clone(); }
        let b = fr[((lvl * 8 + 50) / 100) as usize].clone();
        match lvl { 0 => over(&b, &fr[19]), 10 => over(&b, &fr[18]), _ => b }
    };
    // flyout: 0..=9 green 10–100 %, 10..=18 yellow, 19..=25 red, 27 = missing, 30 = plug
    let flyout = |fr: &[image::RgbaImage], lvl: u32, charging: bool| -> image::RgbaImage {
        let b = if lvl >= 30 { fr[(lvl / 10 - 1) as usize].clone() } else if lvl >= 10 { fr[(10 + lvl / 10 - 1) as usize].clone() } else { fr[19].clone() };
        if charging { over(&b, &fr[30]) } else { b }
    };
    let classic = |lvl: u32| match lvl { 0 => Some("empty"), 10 => Some("caution"), 20 => Some("low"), 60 => Some("good"), 100 => Some("full"), _ => None };
    let mut n = 0;
    for (size, src, is_tray) in [(16u32, &t16, true), (24, &t24, true), (32, &f32_, false), (48, &f40, false)] {
        let dir = out.join(format!("{size}x{size}/apps"));
        let fit = |img: image::RgbaImage| if img.width() == size { img } else { image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3) };
        for lvl in (0..=100).step_by(10) {
            for charging in [false, true] {
                let img = fit(if is_tray { tray(src, lvl, charging) } else { flyout(src, lvl, charging) });
                let suffix = if charging { "-charging" } else { "" };
                let name = format!("battery-level-{lvl}{suffix}");
                if save_png(&img, &dir.join(format!("{name}.png"))).is_err() { continue; }
                let mut aliases = vec![];
                if let Some(c) = classic(lvl) { aliases.push(format!("battery-{c}{suffix}")); }
                if lvl == 100 && charging { aliases.push("battery-full-charged".into()); aliases.push("battery-level-100-charged".into()); }
                for a in aliases {
                    let l = dir.join(format!("{a}.png"));
                    let _ = std::fs::remove_file(&l);
                    let _ = std::os::unix::fs::symlink(format!("{name}.png"), l);
                }
                n += 1;
            }
        }
        let _ = save_png(&fit(if is_tray { src[30].clone() } else { src[27].clone() }), &dir.join("battery-missing.png"));
    }
    n / 4
}

/// Pen, highlighter and eraser from Snipping Tool's own toolbar strips (RT_BITMAP, 7 frames: new snip,
/// save, copy, send, pen, highlighter, eraser; 134 = 24 px, 226 = 30 px, 227 = 36 px).
fn build_snip_tools(root: &Path, out: &Path) -> usize {
    let Some(pe) = find_ci(root, "Windows/System32/SnippingTool.exe").and_then(|p| Pe::open(&p).ok()) else { return 0 };
    let strips: Vec<Vec<image::RgbaImage>> = [134u16, 226, 227].into_iter().filter_map(|id| {
        let img = pe::decode_dib(pe.get(pe::RT_BITMAP, ResId::Id(id))?, false).ok()?;
        let h = img.height();
        (h > 0 && img.width() == 7 * h).then(|| (0..7).map(|i| image::imageops::crop_imm(&img, i * h, 0, h, h).to_image()).collect::<Vec<_>>())
    }).collect();
    if strips.is_empty() { eprintln!("  ! SnippingTool.exe: no toolbar strips"); return 0; }
    let mut n = 0;
    for (frame, name) in [(4usize, "draw-freehand"), (5, "format-text-highlight"), (6, "draw-eraser")] {
        let imgs: Vec<image::RgbaImage> = strips.iter().map(|s| s[frame].clone()).collect();
        for s in [16u32, 24, 32, 48] {
            if let Some(img) = best_for(&imgs, s) { let _ = save_png(&img, &out.join(format!("{s}x{s}/apps/{name}.png"))); }
        }
        n += 1;
    }
    n
}

/// Dump every icon of the icon-bearing system files into a browsable sheet (to tune ICONS).
fn icon_sheet(root: &Path, out: &Path) -> Result<()> {
    let mut html = String::from("<html><body style='font:12px sans-serif;background:#eee'>");
    let mut files: Vec<String> = vec!["Windows/System32/imageres.dll".into(), "Windows/System32/shell32.dll".into()];
    if let Some(sys) = find_ci(root, "Windows/System32") {
        let mut more: Vec<String> = std::fs::read_dir(sys).into_iter().flatten().flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| { let l = n.to_lowercase(); (l.ends_with(".dll") || l.ends_with(".exe") || l.ends_with(".cpl")) && l != "imageres.dll" && l != "shell32.dll" })
            .map(|n| format!("Windows/System32/{n}")).collect();
        more.sort();
        files.extend(more);
    }
    for f in &files {
        let Some(pe) = find_ci(root, f).and_then(|p| Pe::open(&p).ok()) else { continue };
        html += &format!("<h2>{f}</h2><div style='display:flex;flex-wrap:wrap'>");
        for id in pe.names(pe::RT_GROUP_ICON) {
            let label = match &id { ResId::Id(i) => i.to_string(), ResId::Name(s) => s.clone() };
            if let Some(img) = best_for(&pe.group_icon(Some(id)), 48) {
                let file = format!("{}_{label}.png", Path::new(f).file_stem().unwrap().to_string_lossy());
                save_png(&img, &out.join(&file))?;
                html += &format!("<div style='width:80px;text-align:center;margin:4px'><img src='{file}'><br>{label}</div>");
            }
        }
        html += "</div>";
    }
    std::fs::write(out.join("index.html"), html)?;
    println!("· icon contact sheet: {}", out.join("index.html").display());
    Ok(())
}

fn extract_orb(root: &Path, data: &Path) {
    let Some(pe) = find_ci(root, "Windows/explorer.exe").and_then(|p| Pe::open(&p).ok()) else { return };
    for (id, name) in [(6801u16, "orb.png"), (6805, "orb-120.png"), (6809, "orb-144.png")] {
        if let Some(img) = pe.get(pe::RT_BITMAP, id).and_then(|d| pe::decode_dib(d, false).ok()) {
            let _ = save_png(&img, &data.join(name));
        }
    }
    // clock flyout faces (timedate.cpl PNGFILE 5000-5011)
    if let Some(td) = find_ci(root, "Windows/System32/timedate.cpl").and_then(|p| Pe::open(&p).ok()) {
        let dir = data.join("clock");
        let _ = std::fs::create_dir_all(&dir);
        for n in td.names("PNGFILE") {
            if let ResId::Id(i) = n { if let Some(d) = td.get("PNGFILE", i) { let _ = std::fs::write(dir.join(format!("{i}.png")), d); } }
        }
    }
    // dump every bitmap in explorer.exe too (taskbar/start menu pieces) for the renderer
    let dir = data.join("explorer");
    for id in pe.names(pe::RT_BITMAP) {
        if let ResId::Id(i) = id {
            if let Some(img) = pe.get(pe::RT_BITMAP, i).and_then(|d| pe::decode_dib(d, false).ok()) {
                let _ = save_png(&img, &dir.join(format!("{i}.png")));
            }
        }
    }
    if data.join("orb.png").exists() { println!("· start orb extracted"); }
}

// ---------------------------------------------------------------- cursors
fn ani_frames(d: &[u8]) -> Vec<(Vec<u8>, u32)> {
    fn u32at(d: &[u8], o: usize) -> u32 { u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) }
    let mut frames = vec![];
    let mut rates = vec![];
    let mut def = 6;
    fn walk(d: &[u8], mut p: usize, end: usize, frames: &mut Vec<Vec<u8>>, rates: &mut Vec<u32>, def: &mut u32) {
        while p + 8 <= end.min(d.len()) {
            let id = &d[p..p + 4];
            let size = u32at(d, p + 4) as usize;
            let body = p + 8;
            if body + size > d.len() { break; }
            match id {
                b"anih" if size >= 32 => *def = u32at(d, body + 28).max(1),
                b"rate" => rates.extend((0..size / 4).map(|i| u32at(d, body + i * 4))),
                b"LIST" => walk(d, body + 4, body + size, frames, rates, def),
                b"icon" => frames.push(d[body..body + size].to_vec()),
                _ => {}
            }
            p = body + size + (size & 1);
        }
    }
    let mut fr = vec![];
    if d.len() > 12 { walk(d, 12, d.len(), &mut fr, &mut rates, &mut def); }
    for (i, f) in fr.into_iter().enumerate() {
        frames.push((f, rates.get(i).copied().unwrap_or(def)));
    }
    frames
}

/// Write an Xcursor file directly (no xcursorgen needed).
fn write_xcursor(path: &Path, frames: &[(image::RgbaImage, u16, u16, u32)]) -> Result<()> {
    let mut chunks: Vec<Vec<u8>> = vec![];
    for (img, hx, hy, delay_ms) in frames {
        let mut c = vec![];
        for v in [36u32, 0xfffd0002, img.width(), 1, img.width(), img.height(), *hx as u32, *hy as u32, *delay_ms] {
            c.extend_from_slice(&v.to_le_bytes());
        }
        for p in img.pixels() {
            let [r, g, b, a] = p.0;
            let pm = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
            c.extend_from_slice(&u32::from_le_bytes([pm(b), pm(g), pm(r), a]).to_le_bytes());
        }
        chunks.push(c);
    }
    let mut out = vec![];
    out.extend_from_slice(b"Xcur");
    for v in [16u32, 0x10000, chunks.len() as u32] { out.extend_from_slice(&v.to_le_bytes()); }
    let mut pos = 16 + 12 * chunks.len() as u32;
    for (i, c) in chunks.iter().enumerate() {
        for v in [0xfffd0002u32, frames[i].0.width(), pos] { out.extend_from_slice(&v.to_le_bytes()); }
        pos += c.len() as u32;
    }
    for c in chunks { out.extend(c); }
    std::fs::write(path, out)?;
    Ok(())
}

fn build_cursors(root: &Path, out: &Path) -> Result<()> {
    let Some(cdir) = find_ci(root, "Windows/Cursors") else { return Ok(()) };
    let table: &[(&str, &[&str])] = &[
        ("aero_arrow", &["left_ptr", "default", "arrow", "top_left_arrow", "context-menu"]),
        ("aero_link", &["hand2", "pointer", "hand1", "pointing_hand", "9d800788f1b08800ae810202380a0822", "e29285e634086352946a0e7090d73106"]),
        ("aero_helpsel", &["help", "question_arrow", "whats_this", "left_ptr_help", "d9ce0ab605698f320427677b458ad60b", "5c6cd98b3f3ebcb1f9c7f1c204630408"]),
        ("aero_unavail", &["not-allowed", "crossed_circle", "circle", "no-drop", "forbidden", "03b6e0fcb3499374a867c041f52298f0"]),
        ("aero_ns", &["sb_v_double_arrow", "ns-resize", "n-resize", "s-resize", "top_side", "bottom_side", "row-resize", "v_double_arrow", "size_ver", "00008160000006810000408080010102"]),
        ("aero_ew", &["sb_h_double_arrow", "ew-resize", "e-resize", "w-resize", "left_side", "right_side", "col-resize", "h_double_arrow", "size_hor", "028006030e0e7ebffc7f7070c0600140"]),
        ("aero_nwse", &["bottom_right_corner", "top_left_corner", "nwse-resize", "se-resize", "nw-resize", "size_fdiag", "c7088f0f3e6c8088236ef8e1e3e70000"]),
        ("aero_nesw", &["bottom_left_corner", "top_right_corner", "nesw-resize", "ne-resize", "sw-resize", "size_bdiag", "fcf1c3c7cd4491d801f1e1c78f100000"]),
        ("aero_move", &["fleur", "move", "all-scroll", "size_all", "grabbing", "dnd-move", "4498f0e0c1937ffe01fd06f973665830"]),
        ("aero_pen", &["pencil", "draft"]),
        ("aero_up", &["up-arrow", "center_ptr", "sb_up_arrow"]),
        ("aero_busy", &["watch", "wait", "clock"]),
        ("aero_working", &["left_ptr_watch", "progress", "half-busy", "08e8e1c95fe2fc01f976f1e063a24ccd", "3ecb610c1bf2410f44200f48c40d3599"]),
        ("beam_i", &["xterm", "text", "ibeam", "vertical-text"]),
        ("cross_i", &["crosshair", "cross", "tcross", "cell", "plus"]),
    ];
    let dir = out.join("cursors");
    std::fs::create_dir_all(&dir)?;
    let mut n = 0;
    for (base, names) in table {
        let Some(f) = std::fs::read_dir(&cdir)?.flatten().map(|e| e.path()).find(|p| {
            p.file_stem().map(|s| s.to_string_lossy().eq_ignore_ascii_case(base)).unwrap_or(false)
        }) else { continue };
        let d = std::fs::read(&f)?;
        let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let frames: Vec<_> = if ext == "ani" {
            ani_frames(&d).into_iter().filter_map(|(cur, jif)| {
                pe::decode_ico_file(&cur).into_iter().max_by_key(|x| x.0.width()).map(|(img, hx, hy)| (img, hx, hy, jif * 1000 / 60))
            }).collect()
        } else {
            pe::decode_ico_file(&d).into_iter().map(|(i, x, y)| (i, x, y, 0)).collect()
        };
        if frames.is_empty() { continue; }
        write_xcursor(&dir.join(names[0]), &frames)?;
        for a in &names[1..] {
            let l = dir.join(a);
            let _ = std::fs::remove_file(&l);
            let _ = std::os::unix::fs::symlink(names[0], l);
        }
        n += 1;
    }
    std::fs::write(out.join("index.theme"), "[Icon Theme]\nName=Aerode-cursors\nComment=Windows 7 Aero cursors\nInherits=Adwaita\n")?;
    println!("· {n} cursors");
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) {
    let _ = std::fs::create_dir_all(dst);
    if let Ok(rd) = std::fs::read_dir(src) {
        for e in rd.flatten() {
            let p = e.path();
            let t = dst.join(e.file_name());
            if p.is_dir() { copy_tree(&p, &t) } else { let _ = std::fs::copy(&p, &t); }
        }
    }
}

/// Windows\Media (the "Windows Default" scheme) with its scheme folders (Afternoon, Calligraphy…:
/// Control Panel > Sound > Sound Scheme), and the startup tune, which is a resource of imageres.dll.
fn copy_sounds(root: &Path, data: &Path) -> Result<()> {
    let wav = |p: &Path| p.extension().map(|x| x.eq_ignore_ascii_case("wav")).unwrap_or(false);
    let dst = data.join("sounds");
    if let Some(m) = find_ci(root, "Windows/Media") {
        std::fs::create_dir_all(&dst)?;
        let mut schemes = 0;
        for e in std::fs::read_dir(m)?.flatten() {
            let p = e.path();
            if p.is_dir() {
                let sub = dst.join(e.file_name());
                let mut any = false;
                for f in std::fs::read_dir(&p)?.flatten().filter(|f| wav(&f.path())) {
                    std::fs::create_dir_all(&sub)?;
                    std::fs::copy(f.path(), sub.join(f.file_name()))?;
                    any = true;
                }
                if any { schemes += 1; }
            } else if wav(&p) {
                std::fs::copy(&p, dst.join(e.file_name()))?;
            }
        }
        println!("· sounds copied ({schemes} sound schemes)");
    }
    if let Some(dll) = find_ci(root, "Windows/System32/imageres.dll") {
        std::fs::create_dir_all(&dst)?;
        if crate::sounds::extract_startup(&dll, &dst.join(crate::sounds::STARTUP_FILE)) { println!("· startup sound saved"); }
    }
    Ok(())
}

fn copy_misc(root: &Path, data: &Path, fonts: &Path) -> Result<()> {
    if let Some(fd) = find_ci(root, "Windows/Fonts") {
        std::fs::create_dir_all(fonts)?;
        for e in std::fs::read_dir(fd)?.flatten() {
            std::fs::copy(e.path(), fonts.join(e.file_name().to_string_lossy().to_lowercase()))?;
        }
        let _ = Command::new("fc-cache").arg("-f").arg(fonts).output();
        println!("· fonts installed (Segoe UI, Consolas, …)");
    }
    if let Some(wp) = find_ci(root, "Windows/Web/Wallpaper") {
        copy_tree(&wp, &data.join("wallpapers"));
        if let Some(img0) = find_ci(&data.join("wallpapers"), "Windows/img0.jpg") { std::fs::copy(img0, data.join("wallpaper.jpg"))?; }
        println!("· wallpapers copied");
    }
    if let Some(bg) = find_ci(root, "Windows/System32/oobe/background.bmp").or_else(|| find_ci(root, "Windows/Web/Screen/img100.jpg")) {
        if let Ok(img) = image::open(&bg) { let _ = img.save(data.join("lockscreen.png")); }
    }
    copy_sounds(root, data)?;
    if let Some(t) = find_ci(root, "Windows/Resources/Themes/aero") {
        copy_tree(&t, &data.join("theme"));
        println!("· aero.msstyles copied");
    }
    if let Some(pics) = find_ci(root, "ProgramData/Microsoft/User Account Pictures") {
        let mut all = vec![];
        fn walk(p: &Path, v: &mut Vec<PathBuf>) { if let Ok(rd) = std::fs::read_dir(p) { for e in rd.flatten() { let q = e.path(); if q.is_dir() { walk(&q, v) } else { v.push(q) } } } }
        walk(&pics, &mut all);
        all.sort();
        let av = data.join("avatars");
        std::fs::create_dir_all(&av)?;
        for p in &all {
            if let Ok(img) = image::open(p) { let _ = img.save(av.join(format!("{}.png", p.file_stem().unwrap().to_string_lossy()))); }
        }
        let pick = all.iter().find(|p| p.to_string_lossy().to_lowercase().contains("usertile")).or_else(|| all.first());
        if let Some(p) = pick { if let Ok(img) = image::open(p) { let _ = img.save(data.join("avatar.png")); } }
    }
    Ok(())
}

/// Keep a copy of the files the icon theme is built from (in your home, like all extracted art), so a
/// later aerode update can rebuild the theme with new mappings without asking for the ISO again.
fn cache_sysfiles(root: &Path, cache: &Path) {
    let mut files: Vec<&str> = ICONS.iter().map(|i| i.1).collect();
    files.push("Windows/System32/batmeter.dll");
    files.sort();
    files.dedup();
    for f in files.iter().copied() {
        let Some(src) = find_ci(root, f) else { continue };
        let dst = cache.join(f);
        if let Some(p) = dst.parent() { let _ = std::fs::create_dir_all(p); }
        let _ = std::fs::copy(&src, &dst);
    }
    // what this extraction looked for (found or not), so `--icons` can tell when ICONS needs more files
    let _ = std::fs::write(cache.join("files.lst"), files.join("\n"));
}

/// `aerode-extract --icons`: rebuild only the icon theme, from the cached system files or the
/// remembered ISO.
fn rebuild_icons() -> Result<()> {
    let data = gtk::glib::user_data_dir().join("aerode");
    let cache = data.join("sysfiles");
    let iso = std::fs::read_to_string(data.join("source")).unwrap_or_default();
    let iso = iso.trim();
    let have_iso = !iso.is_empty() && Path::new(iso).exists();
    if find_ci(&cache, "Windows/System32/imageres.dll").is_some() {
        // a newer ICONS table can map files the last extraction did not keep: read them from the ISO once
        let looked = std::fs::read_to_string(cache.join("files.lst")).unwrap_or_default();
        let new_files = ICONS.iter().any(|i| !looked.lines().any(|l| l == i.1));
        if new_files && have_iso {
            println!("· new icon mappings – re-reading {iso}");
            match run(&[iso.to_string()]) {
                Ok(()) => return Ok(()),
                Err(e) => eprintln!("  ! {e:#} – rebuilding from the kept files"),
            }
        } else if new_files {
            println!("  (some icons need files from the Windows 7 ISO – run: aerode-extract /path/to/windows7.iso)");
        }
        build_icons(&cache, &gtk::glib::user_data_dir().join("icons/Aerode"))?;
        return Ok(());
    }
    if have_iso {
        println!("· no cached system files yet – re-reading {iso}");
        return run(&[iso.to_string()]);
    }
    bail!("no cached Windows 7 files – run: aerode-extract /path/to/windows7.iso")
}

pub fn run(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--icons") && !args.iter().any(|a| !a.starts_with("--")) { return rebuild_icons(); }
    let Some(src) = args.iter().find(|a| !a.starts_with("--")) else {
        bail!("usage: aerode-extract /path/to/windows7.iso [--sheet]\n       aerode-extract --icons   (rebuild the icon theme from the files kept at the last extraction)\n       (also accepts install.wim / install.esd or an extracted/mounted Windows folder)");
    };
    let src = std::fs::canonicalize(crate::util::shellexpand(src)).context("ISO not found")?;
    // developer aid: aerode-extract <extracted dir> --sheet-only <out dir>
    if let Some(i) = args.iter().position(|a| a == "--sheet-only") {
        let out = args.get(i + 1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("iconsheet"));
        return icon_sheet(&src, &out);
    }
    // developer aid: aerode-extract <aero.msstyles> --msstyles-dump CLASS <out dir>
    if let Some(i) = args.iter().position(|a| a == "--msstyles-dump") {
        let class = args.get(i + 1).cloned().unwrap_or_default();
        let out = args.get(i + 2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("msdump"));
        std::fs::create_dir_all(&out)?;
        let th = crate::msstyles::Theme::open(&src)?;
        for p in th.props.iter().filter(|p| p.class.eq_ignore_ascii_case(&class)) {
            println!("part {:>2} state {:>2} prop {:>5} type {:>3} = {:?}", p.part, p.state, p.name, p.ty, p.value);
            if p.name == crate::msstyles::IMAGEFILE || p.name == crate::msstyles::IMAGEFILE1 {
                if let crate::msstyles::Value::Image(id) = p.value {
                    if let Some(img) = th.image_res(id) { save_png(&img, &out.join(format!("{class}_{}_{}_{id}.png", p.part, p.state)))?; }
                }
            }
        }
        return Ok(());
    }
    // developer aid: aerode-extract <extracted dir> --sounds-only <data dir>
    if let Some(i) = args.iter().position(|a| a == "--sounds-only") {
        return copy_sounds(&src, &args.get(i + 1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("aerode")));
    }
    // developer aid: aerode-extract <extracted dir> --icons-only <out dir>
    if let Some(i) = args.iter().position(|a| a == "--icons-only") {
        let out = args.get(i + 1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("Aerode"));
        build_icons(&src, &out)?;
        return Ok(());
    }
    let share = gtk::glib::user_data_dir();
    let data = share.join("aerode");
    std::fs::create_dir_all(&data)?;
    let tmp = std::env::temp_dir().join(format!("aerode-extract-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let result = (|| -> Result<()> {
        let root = if src.is_dir() { src.clone() } else {
            let wim = locate_wim(&src, &tmp)?;
            let root = tmp.join("root");
            std::fs::create_dir_all(&root)?;
            extract_wim(&wim, &root)?;
            root
        };
        if find_ci(&root, "Windows/System32/imageres.dll").is_none() { bail!("imageres.dll not found – is this a Windows 7 image?"); }
        cache_sysfiles(&root, &data.join("sysfiles"));
        let _ = std::fs::write(data.join("source"), src.to_string_lossy().as_bytes());
        build_icons(&root, &share.join("icons/Aerode"))?;
        extract_orb(&root, &data);
        build_cursors(&root, &share.join("icons/Aerode-cursors"))?;
        copy_misc(&root, &data, &share.join("fonts/aerode"))?;
        if args.iter().any(|a| a == "--sheet") { icon_sheet(&root, &data.join("iconsheet"))?; }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result?;
    println!("done. Log out and choose the \"Aerode\" session.");
    Ok(())
}
