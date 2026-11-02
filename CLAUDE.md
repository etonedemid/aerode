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
| `aerode-wm` | `src/wm/` | reparenting X11 window manager; Win7 frames drawn from the aero.msstyles DWM atlas (`wm/decor.rs`) with the DWM glass formula (`wm/glass.rs`, live `_AERODE_COLORIZATION`), Aero Snap/Shake/Peek, vertical maximise, Alt+Tab (`wm/switcher.rs`), global keys (`wm/keys.rs`), EWMH/ICCCM |
| `aerode-extract` | `src/extract.rs`, `src/pe.rs` | pulls icons/cursors/fonts/sounds/wallpapers/msstyles out of a Win7 ISO/WIM |
| `aerode-polkit` | `src/bin/aerode-polkit.rs` | UAC-style polkit agent (`--demo` to preview) |
| `aerode-cmd` | `src/console/`, `src/cmdshell/` | Win7 console host (PTY + VT parser, raster .fon fonts) and the cmd.exe interpreter (`--shell`) |
| `aerode-explorer` | `src/explorer/` | Windows Explorer |
| `aerode-control` | `src/bin/aerode-control/` | Control Panel (applet table in `registry.rs`, pages in `pages/`); `--action-center-summary` feeds the tray flag |
| `aerode-lockui` | `src/bin/aerode-lockui.rs` | Win7 locked screen as xsecurelock modules (`/usr/lib/aerode/{auth,saver}_aerode` links) |
| `aerode-taskmgr`, `aerode-notepad`, `aerode-calc`, `aerode-snip` | `src/bin/` | the Win7 apps |

Shared: `src/glass.rs` (wallpaper + fake/real Aero glass), `src/msstyles.rs` (aero.msstyles parser),
`src/dwmcolor.rs` (Win7 colourisation maths), `src/theme.rs` (CSS), `src/sounds.rs` (Win7 sound scheme),
`src/sys.rs` (audio/network/battery/brightness incl. DDC/CI via ddcutil), `src/x11.rs` (EWMH client used
by the taskbar), `src/thumbs.rs` (taskbar thumbnails + Aero Peek requests), `src/actioncenter.rs`
(tray flag), `src/autoplay.rs`, `src/notify.rs`, `src/tray/`, `src/kbdlayout.rs`,
`src/power.rs` (Win7 power plans on power-profiles-daemon + the shell's display-off/sleep watcher;
auto-sleep only in a real logind session), `src/tzinfo.rs` (TZif/POSIX-TZ reader for other zones' times).
Data: `data/` (session script, picom.conf, GTK css, .desktop files). Installer: `install.sh`.

## Build / test / run
```bash
cargo build                      # debug build of everything
cargo test --lib                 # unit tests (terminal emulator, cmd parser, PE/DIB decoding…)
./install.sh [win7.iso]          # release build + install to /usr + extract assets
aerode restart                   # restart WM in place (windows kept), shell and picom – no logout
```
Logs: start pieces from a terminal to see stderr, e.g. `aerode-wm --replace` isn't supported –
use `aerode restart`; for the shell `aerode quit; aerode shell`. Panics of the WM and the shell are
logged with a backtrace to `~/.cache/aerode/<name>.log`; the WM survives them (catch per event,
in-place restart after 5/min) and the session script restarts a crashed WM or shell.

Testing without real hardware: Xvfb + `dbus-daemon --session --fork --print-address=1` (dbus-launch
is often missing), `picom --backend xrender` (no blur there). `cargo run --example wintype -- utility
Title x y w h` opens windows of any EWMH type; `cargo run --example wmmsg -- ATOM data…` sends client
messages to the WM (debug builds: `_AERODE_RESTART 0xDEAD` tests the crash recovery). Short-lived X
clients must end with a round trip, not just a flush, or the server can drop their last request.
Copies of this crate in other directories (git worktrees) get the same artifact names from cargo,
so builds sharing one `target/` overwrite each other's `libaerode` and binaries – use a separate
`CARGO_TARGET_DIR` per checkout when you need reliable builds, and copy the binaries you test.

Config lives in `~/.config/aerode/` (config.json, console.json, picom.conf override, displays.sh…),
extracted assets in `~/.local/share/aerode/`, icon theme `~/.local/share/icons/Aerode`.

## Known issues / next steps (as of handoff)
- User reported "very buggy" on real hardware; fixed so far: stuck Aero Snap preview after
  fullscreening an Electron app, unmap handling dropping minimized windows, 60 Hz cap sources
  (xrandr without --rate, motion events not coalesced, console polling).
- Verify on real hardware: real glass under picom, polkit agent with real polkitd, DDC/CI brightness,
  fish inside aerode-cmd, multi-monitor/high-refresh behaviour.
- Not done yet (sound follow-ups from the sound audit, APIs exist in `src/sounds.rs`): lock/unlock and
  log-off sounds off the GTK thread (`sounds::play_sync` in `sys.rs`/`lock.rs`), X bell → Default Beep
  in the WM, Open/Close/Minimize/Maximize/Menu event callers, the Sound applet's startup-sound checkbox
  via `sounds::set_startup_sound`, `pactl subscribe` children outliving the shell (PR_SET_PDEATHSIG in
  `util::watch_lines`), the WM ignoring `_MOTIF_WM_HINTS` functions.
- Done since the handoff (verify on real hardware): taskbar/Start menu from msstyles (`src/shellstyle.rs`)
  with colour hot-track, Win7 animations (`src/anim.rs`, `src/wm/anim.rs`, picom 12 config from
  `src/picomconf.rs`), desktop icon drag and drop, taskbar/tray context menus, Win7 event sound scheme +
  startup sound, SndVol flyout and Volume Mixer (`src/flyouts/volume.rs`, `src/mixer.rs`,
  `src/sys/audio.rs`), Snipping Tool / Command Prompt parity.
- Done since: notification daemon (`src/notify.rs`, Win7 balloons), notification area with SNI +
  XEmbed icons and the Show hidden icons overflow (`src/tray/`), window system menu, keyboard layouts
  (`src/kbdlayout.rs`), Control Panel applet registry (`src/bin/aerode-control/`), Window Color with
  live frame tint, live taskbar thumbnails + Aero Peek, Aero Shake, Action Center flag, the xsecurelock
  lock screen, event-driven tray (pactl subscribe / nmcli monitor).
- The WM looked for `_NET_CM_S0` instead of `_NET_WM_CM_S0` until 2026-11-02, so composited frames
  (translucent glass under picom) are new: verify them on real hardware.
- Paint is intentionally not part of the project.
