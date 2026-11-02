//! Windows 7's DWM animations for picom ≥ 12.3.
//!
//! The shipped `picom.conf` (and the user's copy that Control Panel saves) holds only options that
//! every picom since v10 understands. picom 12 can also animate windows, through window `rules`
//! with animation scripts – but once `rules` is set it ignores the old-style rule options
//! (shadow-exclude, blur-background-exclude, wintypes…). So picom ≥ 12.3 runs a generated file
//! instead (`~/.cache/aerode/picom.conf`): it `@include`s the real config, repeats its old-style rules
//! as window rules and adds the animations. picom 10/11 keep running the real config with their
//! plain fades.
//!
//! The generated file is rewritten at session start (`aerode picom-config`), by `aerode restart`, by
//! `aerode reload` (Visual Effects switches) and by aerode-wm when the taskbar or the screens change;
//! picom 12 watches its config files and reloads itself when one changes.
//!
//! Window properties aerode-wm sets for the rules (see `src/wm`):
//! - `_AERODE_ANIM` on the top-level of every application window (frame, or the window itself when
//!   it draws its own title bar): open/close/show/hide and the Aero Peek fade;
//! - `_AERODE_MIN_SLOT` while a window minimises or restores: 1 + its taskbar button's centre along
//!   the taskbar / 16 (0 when the taskbar didn't say where the button is). One rule per 16 px slot
//!   aims the minimise animation at the button – picom scripts can't read properties or other
//!   windows, so the target has to be a constant of the rule;
//! - `_AERODE_ANIM_GEOM` around maximise, restore and Aero Snap: the window morphs to its new size
//!   (never while it is dragged);
//! - `_AERODE_NO_ANIM` on surfaces that animate themselves (snap preview, Peek outlines, Alt+Tab,
//!   flyouts, balloons).
//!
//! Timings: spec in the animations agent's notes; Microsoft never published DWM's keyframes. Open
//! and close follow AeroThemePlasma's Win7 recreation (250 ms; open swings in from 95 % with an
//! ease-out, close shrinks to 90 % with InQuad), minimise is 275 ms linear into the button and
//! restore 250 ms, menus fade in 175 ms and vanish at once, combo lists slide down in 165 ms
//! (CMS_QANIMATION), tooltips fade 150/200 ms.
use std::path::{Path, PathBuf};

pub const SHIPPED: &str = "/usr/share/aerode/picom.conf";
/// First line of a current shipped config; a user copy without it is rebuilt from the shipped one.
pub const MARKER: &str = "# aerode-picom 3";
/// Width of one taskbar slot of `_AERODE_MIN_SLOT`.
pub const SLOT: i32 = 16;
const HEADER: &str = "# aerode-picom-run 1";

/// The taskbar's rectangle in root coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }

impl Bar {
    pub fn horizontal(&self) -> bool { self.w >= self.h }
    /// `_AERODE_MIN_SLOT` for a taskbar button whose centre is at (x, y).
    pub fn slot_of(&self, x: i32, y: i32) -> u32 {
        let along = if self.horizontal() { x } else { y };
        1 + (along.max(0) / SLOT) as u32
    }
}

/// The Visual Effects switches (Performance Options) that gate picom's animations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects { pub minmax: bool, pub peek: bool, pub menu: bool, pub combo: bool, pub tooltip: bool }

impl Effects {
    pub fn all() -> Effects { Effects { minmax: true, peek: true, menu: true, combo: true, tooltip: true } }
    pub fn from_config(c: &crate::config::Config) -> Effects {
        Effects {
            minmax: c.hw.effect("animate_minmax"),
            peek: c.taskbar.aero_peek && c.hw.effect("aero_peek"),
            menu: c.hw.effect("menu_fade"),
            combo: c.hw.effect("combo_slide"),
            tooltip: c.hw.effect("tooltip_fade"),
        }
    }
}

/// `picom --version` as (major, minor); git builds ("vgit-…") count as current.
pub fn picom_version() -> Option<(u32, u32)> {
    let out = std::process::Command::new("picom").arg("--version").stderr(std::process::Stdio::null()).output().ok()?;
    parse_version(&String::from_utf8_lossy(&out.stdout))
}

fn parse_version(s: &str) -> Option<(u32, u32)> {
    let v = s.trim().split_whitespace().next()?.trim_start_matches('v');
    if v.starts_with("git") { return Some((99, 0)); }
    let mut it = v.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0)))
}

/// picom 12.0–12.2 corrupted memory when a property a rule watches changed (picom #1350).
pub fn animations_supported(v: Option<(u32, u32)>) -> bool { matches!(v, Some((maj, min)) if maj > 12 || (maj == 12 && min >= 3)) }

/// The config picom runs with when no animations are added: the user's copy, else the shipped one.
pub fn base_config() -> PathBuf {
    let user = crate::config::conf_dir().join("picom.conf");
    if user.exists() { user } else { PathBuf::from(SHIPPED) }
}

/// A user copy saved by an older Control Panel lacks newer rules: rebuild it from the shipped one,
/// keeping its VSync choice (and its shadow switch).
pub fn migrate_user_config() {
    let user = crate::config::conf_dir().join("picom.conf");
    let Ok(old) = std::fs::read_to_string(&user) else { return };
    if old.contains(MARKER) { return; }
    let Ok(base) = std::fs::read_to_string(SHIPPED) else { return };
    let vsync_off = old.contains("vsync = false");
    let shadow_off = old.lines().any(|l| l.trim() == "shadow = false;");
    if !vsync_off && !shadow_off { let _ = std::fs::remove_file(&user); return; }
    let mut txt = base;
    if vsync_off { txt = txt.replace("vsync = true;", "vsync = false;"); }
    if shadow_off { txt = txt.replacen("shadow = true;", "shadow = false;", 1); }
    let _ = std::fs::write(&user, txt);
}

/// Where the generated config lives.
pub fn runtime_path() -> PathBuf { gtk::glib::user_cache_dir().join("aerode").join("picom.conf") }

/// The config file to start picom with: the generated one for picom ≥ 12.3 (written now), else `base`.
pub fn config_for_picom(base: &Path) -> PathBuf {
    if !animations_supported(picom_version()) { return base.to_path_buf(); }
    write_runtime(base, None).unwrap_or_else(|| base.to_path_buf())
}

/// Rewrite the generated config if it is in use (exists), e.g. for a new taskbar rectangle or
/// changed Visual Effects. picom 12 notices the change and reloads.
pub fn refresh(bar: Option<Bar>) {
    let Ok(cur) = std::fs::read_to_string(runtime_path()) else { return };
    let Some(base) = cur.lines().find_map(|l| l.strip_prefix("@include \"").and_then(|r| r.strip_suffix('"'))) else { return };
    let base = PathBuf::from(base);
    // the user's copy appeared or went away since (Control Panel saved/reset it)
    let base = if base.exists() { base } else { base_config() };
    write_runtime(&base, bar);
}

/// Write the generated config for `base` (keeping the last taskbar rectangle when `bar` is None).
/// None when `base` brings its own `rules` or `animations` (then it is used as it is).
pub fn write_runtime(base: &Path, bar: Option<Bar>) -> Option<PathBuf> {
    let txt = std::fs::read_to_string(base).ok()?;
    if has_own_rules(&txt) { return None; }
    let path = runtime_path();
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let bar = bar.or_else(|| read_bar(&old)).unwrap_or_else(default_bar);
    let fx = Effects::from_config(&crate::config::get());
    let new = generate(base, bar, fx);
    // an unchanged file must not be touched: picom 12 resets itself on every write
    if new != old {
        let _ = std::fs::create_dir_all(path.parent()?);
        std::fs::write(&path, new).ok()?;
    }
    Some(path)
}

fn has_own_rules(txt: &str) -> bool {
    txt.lines().map(|l| l.trim_start()).filter(|l| !l.starts_with('#')).any(|l| {
        let key: String = l.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
        (key == "rules" || key == "animations") && l[key.len()..].trim_start().starts_with(['=', ':'])
    })
}

fn read_bar(txt: &str) -> Option<Bar> {
    let l = txt.lines().find_map(|l| l.strip_prefix("# aerode-bar "))?;
    let v: Vec<i32> = l.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    (v.len() == 4 && v[2] > 0 && v[3] > 0).then(|| Bar { x: v[0], y: v[1], w: v[2], h: v[3] })
}

/// Where the shell puts the taskbar: along the bottom of the primary monitor.
fn default_bar() -> Bar {
    let h = crate::config::get().taskbar_height.max(1);
    let m = primary_monitor().unwrap_or((0, 0, 1920, 1080));
    Bar { x: m.0, y: m.1 + m.3 - h, w: m.2, h }
}

fn primary_monitor() -> Option<(i32, i32, i32, i32)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::randr::ConnectionExt as _;
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen].root;
    let (rw, rh) = (conn.setup().roots[screen].width_in_pixels as i32, conn.setup().roots[screen].height_in_pixels as i32);
    let mons = conn.randr_get_monitors(root, true).ok().and_then(|c| c.reply().ok()).map(|r| r.monitors).unwrap_or_default();
    let m = mons.iter().find(|m| m.primary).or(mons.first());
    Some(m.map(|m| (m.x as i32, m.y as i32, m.width as i32, m.height as i32)).unwrap_or((0, 0, rw, rh)))
}

// ------------------------------------------------------------------ the generated config

/// Win7 open: ease-out (AeroThemePlasma's Qt OutCurve, fitted).
const EASE_OPEN: &str = "cubic-bezier(0.52, 0.45, 0.6, 0.8)";
/// Win7 close: InQuad.
const EASE_CLOSE: &str = "cubic-bezier(0.41, 0, 0.75, 0.5)";
/// Maximise / snap morph and the Peek fade: fast start, soft landing (OutCubic) / ease-in-out.
const EASE_MORPH: &str = "cubic-bezier(0.215, 0.61, 0.355, 1)";
const EASE_PEEK: &str = "cubic-bezier(0.42, 0, 0.58, 1)";
const SHADOW_FOLLOWS: &str = "shadow-scale-x = \"scale-x\"; shadow-scale-y = \"scale-y\"; shadow-offset-x = \"offset-x\"; shadow-offset-y = \"offset-y\";";
const FADE_OUTPUTS: &str = "blur-opacity = \"opacity\"; shadow-opacity = \"opacity\";";

fn group(match_: &str, body: &str) -> String { format!("  {{ match = \"{match_}\";\n    {body}\n  }}") }

fn animations(scripts: &[String]) -> String { format!("animations = (\n{}\n    );", scripts.join(",\n")) }

fn none(triggers: &[&str]) -> String { format!("      {{ triggers = [ {} ]; }}", quoted(triggers)) }

fn quoted(t: &[&str]) -> String { t.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ") }

/// Open (and show after a non-minimise hide): from 95 % pinned near the top edge, 40 % → full opacity.
fn open_script(triggers: &[&str]) -> String {
    format!("      {{ triggers = [ {} ];
        v-s = {{ curve = \"{EASE_OPEN}\"; duration = 0.25; start = 0.95; end = 1; }};
        opacity = {{ curve = \"{EASE_OPEN}\"; duration = 0.25; start = \"window-raw-opacity * 0.4\"; end = \"window-raw-opacity\"; }};
        {FADE_OUTPUTS}
        scale-x = \"v-s\"; scale-y = \"v-s\";
        offset-x = \"(1 - v-s) / 2 * window-width\"; offset-y = \"(1 - v-s) * 0.15 * window-height\";
        {SHADOW_FOLLOWS}
      }}", quoted(triggers))
}

/// Close (and a hide that isn't a minimise): shrinks to 90 % pinned near the bottom edge, fading out.
fn close_script(triggers: &[&str]) -> String {
    format!("      {{ triggers = [ {} ];
        v-s = {{ curve = \"{EASE_CLOSE}\"; duration = 0.25; start = 1; end = 0.9; }};
        opacity = {{ curve = \"{EASE_CLOSE}\"; duration = 0.25; start = \"window-raw-opacity-before\"; end = 0; }};
        {FADE_OUTPUTS}
        scale-x = \"v-s\"; scale-y = \"v-s\";
        offset-x = \"(1 - v-s) / 2 * window-width\"; offset-y = \"(1 - v-s) * 0.85 * window-height\";
        {SHADOW_FOLLOWS}
      }}", quoted(triggers))
}

/// Minimise (hide) into / restore (show) from the rectangle centred on (tx, ty) of size tw × th
/// (None: the window's own centre on that axis). The window's centre travels in a straight line
/// while it scales about its top-left corner.
fn minimize_scripts(tx: Option<i32>, ty: Option<i32>, tw: i32, th: i32) -> [String; 2] {
    let travel = |t: Option<i32>, pos: &str, size: &str| match t {
        Some(t) => format!("({t} - {pos} - {size} / 2) * v-t + "),
        None => String::new(),
    };
    let (mx, my) = (travel(tx, "window-x", "window-width"), travel(ty, "window-y", "window-height"));
    let geo = format!("scale-x = \"1 + ({tw} / window-width - 1) * v-t\"; scale-y = \"1 + ({th} / window-height - 1) * v-t\";
        offset-x = \"{mx}window-width * (1 - scale-x) / 2\";
        offset-y = \"{my}window-height * (1 - scale-y) / 2\";
        {FADE_OUTPUTS}
        {SHADOW_FOLLOWS}");
    [
        format!("      {{ triggers = [ \"hide\" ];
        v-t = {{ duration = 0.275; start = 0; end = 1; }};
        opacity = {{ duration = 0.275; start = \"window-raw-opacity-before * 0.9\"; end = 0; }};
        {geo}
      }}"),
        format!("      {{ triggers = [ \"show\" ];
        v-t = {{ duration = 0.25; start = 1; end = 0; }};
        opacity = {{ duration = 0.25; start = 0; end = \"window-raw-opacity\"; }};
        {geo}
      }}"),
    ]
}

fn fade_script(triggers: &[&str], secs: f64, appear: bool) -> String {
    let (start, end) = if appear { ("0", "window-raw-opacity") } else { ("window-raw-opacity-before", "0") };
    format!("      {{ triggers = [ {} ];
        opacity = {{ duration = {secs}; start = \"{start}\"; end = \"{end}\"; }};
        {FADE_OUTPUTS}
      }}", quoted(triggers))
}

/// The generated config: `base` included, its old-style rules as window rules, then the animations.
pub fn generate(base: &Path, bar: Bar, fx: Effects) -> String {
    let base_txt = std::fs::read_to_string(base).unwrap_or_default();
    let mut rules: Vec<String> = vec![];
    // ---- the old-style rules of the shipped config, which picom 12 ignores once `rules` exists
    rules.push(group("window_type = 'tooltip'", "opacity = 0.97;"));
    rules.push(group("window_type = 'popup_menu' || window_type = 'dropdown_menu'", "shadow = true;"));
    rules.push(group("window_type = 'desktop' || window_type = 'dock' || window_type = 'dnd' || _GTK_FRAME_EXTENTS@ || class_g = 'i3lock'", "shadow = false;"));
    rules.push(group("!_AERODE_GLASS@ || window_type = 'desktop' || window_type = 'tooltip' || class_g = 'i3lock'", "blur-background = false;"));
    // the user's own exclusions (picom 12 would drop them silently)
    if let Some(extra) = extra_conditions(&base_txt, "shadow-exclude") { rules.push(group(&extra, "shadow = false;")); }
    if let Some(extra) = extra_conditions(&base_txt, "fade-exclude") { rules.push(group(&extra, "fade = false;")); }
    if let Some(extra) = extra_conditions(&base_txt, "blur-background-exclude") { rules.push(group(&extra, "blur-background = false;")); }

    // ---- application windows (aerode-wm tags them)
    let app = if fx.minmax {
        let mut s = vec![open_script(&["open", "show"]), close_script(&["close", "hide"])];
        if fx.peek { s.push(peek_script()); }
        s
    } else {
        // Win7 with "Animate windows when minimizing and maximizing" off: windows simply appear
        let mut s = vec![none(&["open", "show", "close", "hide"])];
        if fx.peek { s.push(peek_script()); }
        s
    };
    rules.push(group("_AERODE_ANIM", &animations(&app)));
    if fx.minmax {
        // minimise without a known taskbar button: drop into the taskbar below/beside the window
        let (cx, cy) = (bar.x + bar.w / 2, bar.y + bar.h / 2);
        let thick = if bar.horizontal() { bar.h } else { bar.w };
        let (tw, th) = if bar.horizontal() { (thick * 3 / 2, thick) } else { (thick, thick * 3 / 4) };
        let (tx, ty) = if bar.horizontal() { (None, Some(cy)) } else { (Some(cx), None) };
        rules.push(group("_AERODE_MIN_SLOT = 0", &animations(&minimize_scripts(tx, ty, tw, th))));
        // one rule per taskbar slot: minimise into the button, restore out of it
        let (start, len) = if bar.horizontal() { (bar.x, bar.w) } else { (bar.y, bar.h) };
        let first = 1 + start.max(0) / SLOT;
        let last = 1 + (start + len - 1).max(0) / SLOT;
        for s in first..=last {
            let along = (s - 1) * SLOT + SLOT / 2;
            let (tx, ty) = if bar.horizontal() { (along, cy) } else { (cx, along) };
            rules.push(group(&format!("_AERODE_MIN_SLOT = {s}"), &animations(&minimize_scripts(Some(tx), Some(ty), tw, th))));
        }
        // maximise, restore and Aero Snap: the old image morphs into the new rectangle
        rules.push(group("_AERODE_ANIM_GEOM", &animations(&[format!("      {{ triggers = [ \"geometry\" ];
        scale-x = {{ curve = \"{EASE_MORPH}\"; duration = 0.2; start = \"window-width-before / window-width\"; end = 1; }};
        scale-y = {{ curve = \"{EASE_MORPH}\"; duration = 0.2; start = \"window-height-before / window-height\"; end = 1; }};
        offset-x = {{ curve = \"{EASE_MORPH}\"; duration = 0.2; start = \"window-x-before - window-x\"; end = 0; }};
        offset-y = {{ curve = \"{EASE_MORPH}\"; duration = 0.2; start = \"window-y-before - window-y\"; end = 0; }};
        saved-image-blend = {{ duration = 0.2; start = 1; end = 0; }};
        {SHADOW_FOLLOWS}
      }}")])));
    }

    // ---- menus fade in (SPI_GETMENUFADE) and close at once; combo lists slide down
    let menu = if fx.menu { vec![fade_script(&["open", "show"], 0.175, true), none(&["close", "hide"])] } else { vec![none(&["open", "show", "close", "hide"])] };
    rules.push(group("window_type = 'popup_menu' || window_type = 'dropdown_menu'", &animations(&menu)));
    let combo = if fx.combo {
        vec![format!("      {{ triggers = [ \"open\", \"show\" ];
        v-y = {{ duration = 0.165; start = \"0 - window-height\"; end = 0; }};
        offset-y = \"v-y\"; shadow-offset-y = \"offset-y\";
        crop-x = \"window-x\"; crop-y = \"window-y\"; crop-width = \"window-width\"; crop-height = \"window-height\";
      }}"), none(&["close", "hide"])]
    } else { vec![none(&["open", "show", "close", "hide"])] };
    rules.push(group("window_type = 'combo'", &animations(&combo)));
    // ---- tooltips (and other programs' notification popups) fade in and out
    let tip = if fx.tooltip { vec![fade_script(&["open", "show"], 0.15, true), fade_script(&["close", "hide"], 0.2, false)] } else { vec![none(&["open", "show", "close", "hide"])] };
    rules.push(group("window_type = 'tooltip' || window_type = 'notification'", &animations(&tip)));
    // ---- the taskbar, the desktop and the surfaces that animate themselves
    rules.push(group("window_type = 'dock' || window_type = 'desktop' || _AERODE_NO_ANIM",
        &animations(&[none(&["open", "close", "show", "hide", "increase-opacity", "decrease-opacity", "geometry"])])));

    format!("{HEADER} – generated by aerode for picom >= 12.3. Do not edit: it is rewritten at logon, by
# `aerode restart` / `aerode reload` and when the taskbar or the screens change. Settings go in the
# included file; Visual Effects (Control Panel > System > Performance Options) switch the animations.
# aerode-bar {} {} {} {}
@include \"{}\"

rules = (
{}
);
", bar.x, bar.y, bar.w, bar.h, base.display(), rules.join(",\n"))
}

fn peek_script() -> String {
    // Aero Peek: aerode-wm sets _NET_WM_WINDOW_OPACITY 0 on the other windows ("fade into glass sheets")
    format!("      {{ triggers = [ \"decrease-opacity\", \"increase-opacity\" ];
        opacity = {{ curve = \"{EASE_PEEK}\"; duration = 0.25; start = \"window-raw-opacity-before\"; end = \"window-raw-opacity\"; }};
        {FADE_OUTPUTS}
      }}")
}

/// Conditions of an old-style list option (`name = [ "…", "…" ];`) beyond the ones the shipped
/// config has (those are translated above), joined with `||`.
fn extra_conditions(txt: &str, name: &str) -> Option<String> {
    const SHIPPED_ONES: &[&str] = &["window_type = 'desktop'", "window_type = 'dnd'", "window_type = 'tooltip'", "_GTK_FRAME_EXTENTS@:c",
        "class_g = 'i3lock'", "!_AERODE_GLASS@:c", "_AERODE_NO_ANIM:c"];
    let start = txt.lines().position(|l| { let t = l.trim_start(); !t.starts_with('#') && t.starts_with(name) && t[name.len()..].trim_start().starts_with(['=', ':']) })?;
    let mut body = String::new();
    for l in txt.lines().skip(start) {
        let l = l.split('#').next().unwrap_or("");
        body.push_str(l);
        body.push('\n');
        if l.contains(']') || (l.contains(';') && !l.contains('[')) { break; }
    }
    let inner = body.split_once('[').map(|(_, r)| r.split(']').next().unwrap_or("")).unwrap_or_else(|| body.split_once(['=', ':']).map(|(_, r)| r).unwrap_or(""));
    let conds: Vec<String> = inner.split(',').map(|c| c.trim().trim_end_matches(';').trim()).filter(|c| c.len() >= 2)
        .filter_map(|c| c.strip_prefix('"').and_then(|c| c.strip_suffix('"')))
        .filter(|c| !SHIPPED_ONES.contains(c))
        // picom 12 deprecates the ":type" suffix of the old syntax
        .map(|c| strip_type_suffix(c))
        .map(|c| format!("({c})")).collect();
    (!conds.is_empty()).then(|| conds.join(" || ").replace('"', "'"))
}

/// `_FOO@:c = 1` → `_FOO@ = 1` (picom 12 warns about type specifiers and ignores them).
fn strip_type_suffix(c: &str) -> String {
    let mut out = String::new();
    let mut chars = c.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ':' {
            let mut look = chars.clone();
            let mut spec = String::new();
            while let Some(n) = look.peek().copied() { if n.is_ascii_alphanumeric() { spec.push(n); look.next(); } else { break; } }
            let after = look.peek().copied();
            let is_type = !spec.is_empty() && spec.len() <= 3 && spec.chars().all(|x| x.is_ascii_digit() || "sacwbd".contains(x))
                && matches!(after, None | Some(' ') | Some('=') | Some('!') | Some('<') | Some('>') | Some('*') | Some('^') | Some('%') | Some('~') | Some('?') | Some(')') | Some('&') | Some('|'));
            if is_type { chars = look; continue; }
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v10\n"), Some((10, 0)));
        assert_eq!(parse_version("v12.5 (revision a456d43)"), Some((12, 5)));
        assert_eq!(parse_version("vgit-1a2b3"), Some((99, 0)));
        assert!(!animations_supported(Some((11, 2))));
        assert!(!animations_supported(Some((12, 2))));
        assert!(animations_supported(Some((12, 3))));
        assert!(animations_supported(Some((13, 0))));
        assert!(!animations_supported(None));
    }

    #[test]
    fn own_rules_detected() {
        assert!(has_own_rules("backend = \"glx\";\nrules = (\n);"));
        assert!(has_own_rules("animations: ( );"));
        assert!(!has_own_rules("# rules = ()\nshadow-exclude = [];\nrules-foo = 1;"));
    }

    #[test]
    fn slots() {
        let b = Bar { x: 0, y: 1040, w: 1920, h: 40 };
        assert_eq!(b.slot_of(600, 1060), 38);
        assert_eq!(b.slot_of(0, 1060), 1);
        let v = Bar { x: 0, y: 0, w: 62, h: 1080 };
        assert_eq!(v.slot_of(31, 300), 19);
    }

    #[test]
    fn generated_config_shape() {
        let dir = std::env::temp_dir().join(format!("aerode-picomconf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("picom.conf");
        std::fs::write(&base, "backend = \"glx\";\nshadow-exclude = [\n  \"window_type = 'desktop'\",\n  \"class_g = 'Conky'\",\n  \"_FOO@:c\"\n];\n").unwrap();
        let bar = Bar { x: 0, y: 1040, w: 1920, h: 40 };
        let txt = generate(&base, bar, Effects::all());
        // AERODE_DUMP_PICOM=file: keep a copy to try with a real picom
        if let Ok(p) = std::env::var("AERODE_DUMP_PICOM") { let _ = std::fs::write(p, &txt); }
        assert!(txt.contains(&format!("@include \"{}\"", base.display())));
        assert!(txt.contains("# aerode-bar 0 1040 1920 40"));
        assert_eq!(read_bar(&txt), Some(bar));
        // the user's extra exclusions come along, without the deprecated type suffix
        assert!(txt.contains("match = \"(class_g = 'Conky') || (_FOO@)\";\n    shadow = false;"));
        // 120 slots of 16 px along a 1920 px taskbar; slot 38 aims at x = 37 * 16 + 8 = 600, y = the bar's middle
        assert!(txt.contains("_AERODE_MIN_SLOT = 1\""));
        assert!(txt.contains("_AERODE_MIN_SLOT = 120\""));
        assert!(!txt.contains("_AERODE_MIN_SLOT = 121\""));
        assert!(txt.contains("(600 - window-x - window-width / 2)"));
        assert!(txt.contains("(1060 - window-y - window-height / 2)"));
        // libconfig: balanced brackets, no list ending in a comma
        for (o, c) in [('(', ')'), ('{', '}'), ('[', ']')] {
            assert_eq!(txt.matches(o).count(), txt.matches(c).count(), "{o}{c}");
        }
        let squashed: String = txt.split_whitespace().collect();
        assert!(!squashed.contains(",)") && !squashed.contains(",]") && !squashed.contains(";;"));
        // a minus inside an expression always has spaces around it (picom reads `a-b` as one name)
        for expr in txt.split('"').skip(1).step_by(2) {
            if expr.contains("window-") {
                for (i, ch) in expr.char_indices() {
                    if ch == '-' && i > 0 {
                        let prev = expr[..i].chars().last().unwrap();
                        let next = expr[i + 1..].chars().next().unwrap_or(' ');
                        assert!((prev == ' ' && next == ' ') || (prev.is_ascii_alphabetic() && next.is_ascii_alphabetic()), "{expr}");
                    }
                }
            }
        }
        // switched off: no slot rules, no morph, windows just appear
        let off = generate(&base, bar, Effects { minmax: false, peek: false, menu: false, combo: false, tooltip: false });
        assert!(!off.contains("_AERODE_MIN_SLOT") && !off.contains("_AERODE_ANIM_GEOM") && !off.contains("v-s"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
