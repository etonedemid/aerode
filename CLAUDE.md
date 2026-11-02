# Aerode – Windows 7 desktop environment for Linux (X11, Rust)

Personal project for the owner's EndeavourOS (Arch) machine. Goal: look and behave like
Windows 7 as closely as possible, while doing everything a modern Plasma desktop does.

## Rules
- Commits: author `Demid <etonedemid@gmail.com>`. **Never** add `Co-Authored-By` or any Claude
  attribution/trailer lines to commits or PRs.
- Push to `origin main` (github.com/etonedemid/aerode, private).
- Never commit Microsoft files. All Windows 7 artwork/fonts/sounds are extracted at install time
  from the user's own ISO by `aerode-extract` into the home directory.
- Wayland port is planned for later; for now everything is X11.

## Layout (one crate, lib + many binaries)
| Binary | Source | What |
|---|---|---|
| `aerode` | `src/main.rs` | shell: desktop, taskbar, start menu, tray flyouts; also `aerode restart`, `aerode lock`, `aerode run`… |
| `aerode-wm` | `src/wm/` | reparenting X11 window manager; Win7 frames drawn from the aero.msstyles DWM atlas (`wm/decor.rs`), Aero Snap, Alt+Tab (`wm/switcher.rs`), global keys (`wm/keys.rs`), EWMH/ICCCM |
| `aerode-extract` | `src/extract.rs`, `src/pe.rs` | pulls icons/cursors/fonts/sounds/wallpapers/msstyles out of a Win7 ISO/WIM |
| `aerode-polkit` | `src/bin/aerode-polkit.rs` | UAC-style polkit agent (`--demo` to preview) |
| `aerode-cmd` | `src/console/`, `src/cmdshell/` | Win7 console host (PTY + VT parser, raster .fon fonts) and the cmd.exe interpreter (`--shell`) |
| `aerode-explorer` | `src/explorer/` | Windows Explorer |
| `aerode-control` | `src/bin/aerode-control.rs` | Control Panel |
| `aerode-taskmgr`, `aerode-notepad`, `aerode-calc`, `aerode-snip` | `src/bin/` | the Win7 apps |

Shared: `src/glass.rs` (wallpaper + fake/real Aero glass), `src/msstyles.rs` (aero.msstyles parser),
`src/theme.rs` (CSS), `src/sounds.rs` (Win7 sound scheme), `src/sys.rs` (audio/network/battery/
brightness incl. DDC/CI via ddcutil), `src/x11.rs` (EWMH client used by the taskbar).
Data: `data/` (session script, picom.conf, GTK css, .desktop files). Installer: `install.sh`.

## Build / test / run
```bash
cargo build                      # debug build of everything
cargo test --lib                 # unit tests (terminal emulator, cmd parser, PE/DIB decoding…)
./install.sh [win7.iso]          # release build + install to /usr + extract assets
aerode restart                   # restart WM in place (windows kept), shell and picom – no logout
```
Logs: start pieces from a terminal to see stderr, e.g. `aerode-wm --replace` isn't supported –
use `aerode restart`; for the shell `aerode quit; aerode shell`.

Config lives in `~/.config/aerode/` (config.json, console.json, picom.conf override, displays.sh…),
extracted assets in `~/.local/share/aerode/`, icon theme `~/.local/share/icons/Aerode`.

## Known issues / next steps (as of handoff)
- User reported "very buggy" on real hardware; fixed so far: stuck Aero Snap preview after
  fullscreening an Electron app, unmap handling dropping minimized windows, 60 Hz cap sources
  (xrandr without --rate, motion events not coalesced, console polling).
- Verify on real hardware: real glass under picom, polkit agent with real polkitd, DDC/CI brightness,
  fish inside aerode-cmd, multi-monitor/high-refresh behaviour.
- Not done yet: taskbar/start menu drawn from the real msstyles bitmaps (TaskBand2/StartPanel classes),
  colour hot-track glow on taskbar buttons, live window thumbnails, notification daemon + SNI tray,
  window title-bar right-click (WM spawns `aerode sysmenu <win> <x> <y>`, shell side missing),
  desktop gadgets.
- Paint is intentionally not part of the project.
