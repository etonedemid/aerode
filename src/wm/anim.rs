//! Timed effects of aerode-wm – the Windows 7 DWM animations the window manager draws itself – and
//! the small timer queue of its event loop.
//!
//! picom (≥ 12.3, see `crate::picomconf`) animates the application windows; aerode-wm animates its
//! own surfaces and marks them `_AERODE_NO_ANIM`:
//! - caption-button glow: cross-fades in `GLOW_IN`, out `GLOW_OUT`;
//! - the frame after a focus change: cross-fades between its inactive and active look (`FOCUS_FADE`);
//! - Aero Snap preview: a glass sheet that grows out of the pointer in `SNAP_GROW` and morphs when
//!   the drag moves to another zone, with a ripple of light at the pointer for `RIPPLE`;
//! - Aero Peek outlines: fade in and out with the windows (`PEEK_FADE`);
//! - Alt+Tab: no animation of its own; resting on an item for `SWITCH_PEEK` peeks at that window.
//!
//! Microsoft never published these numbers. The glow follows the theme's own pattern (aero.msstyles
//! TRANSITIONDURATIONS: quick in, slow out) and the snap preview AeroThemePlasma's recreation; all of
//! them are estimates to measure on a real Win7.
use std::time::{Duration, Instant};

use super::decor::Btn;

pub const GLOW_IN: Duration = Duration::from_millis(150);
pub const GLOW_OUT: Duration = Duration::from_millis(500);
/// The frame cross-fading between its active and inactive look after a focus change.
pub const FOCUS_FADE: Duration = Duration::from_millis(200);
pub const SNAP_GROW: Duration = Duration::from_millis(200);
pub const RIPPLE: Duration = Duration::from_millis(600);
pub const PEEK_FADE: Duration = Duration::from_millis(250);
pub const SWITCH_PEEK: Duration = Duration::from_millis(500);
/// How long a minimising window stays mapped so that picom reads its `_AERODE_MIN_SLOT` (and picks
/// the minimise animation) before the unmap: one compositor frame or two.
pub const MIN_UNMAP_DELAY: Duration = Duration::from_millis(40);
/// How long `_AERODE_MIN_SLOT` stays after a restore and `_AERODE_ANIM_GEOM` after a maximise or
/// snap: longer than picom needs to start the animation, shorter than any follow-up drag.
pub const PROP_LINGER: Duration = Duration::from_millis(300);
/// Frame interval of the animations drawn here.
pub const FRAME: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease { Linear, Out, InOut }

impl Ease {
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear => t,
            // fast start, soft landing (cubic)
            Ease::Out => 1.0 - (1.0 - t).powi(3),
            Ease::InOut => if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 },
        }
    }
}

/// A value going from `from` to `to` over `dur`.
#[derive(Clone, Copy, Debug)]
pub struct Tween { pub from: f64, pub to: f64, pub t0: Instant, pub dur: Duration, pub ease: Ease }

impl Tween {
    pub fn new(from: f64, to: f64, dur: Duration, ease: Ease) -> Tween { Tween { from, to, t0: Instant::now(), dur, ease } }
    pub fn progress(&self, now: Instant) -> f64 {
        if self.dur.is_zero() { return 1.0; }
        now.saturating_duration_since(self.t0).as_secs_f64() / self.dur.as_secs_f64()
    }
    pub fn at(&self, now: Instant) -> f64 { self.from + (self.to - self.from) * self.ease.apply(self.progress(now)) }
    pub fn done(&self, now: Instant) -> bool { self.progress(now) >= 1.0 }
}

/// Fade of a level (0…1) towards `to` at the speed of a full fade over `full`: a glow that is
/// half lit fades out in half the time.
pub fn level_tween(cur: f64, to: f64, full: Duration) -> Tween {
    let dist = (to - cur).abs().clamp(0.0, 1.0);
    Tween::new(cur, to, full.mul_f64(dist), Ease::Linear)
}

/// Caption-button glow levels of one window: buttons in the middle of a fade.
#[derive(Default)]
pub struct Glow(pub Vec<(Btn, Tween)>);

impl Glow {
    pub fn level(&self, b: Btn, now: Instant) -> Option<f64> { self.0.iter().find(|x| x.0 == b).map(|x| x.1.at(now)) }
    /// The pointer moved from button `old` to `new` (either may be None).
    pub fn hover(&mut self, old: Option<Btn>, new: Option<Btn>, now: Instant) {
        if let Some(o) = old {
            let cur = self.level(o, now).unwrap_or(1.0);
            self.set(o, level_tween(cur, 0.0, GLOW_OUT));
        }
        if let Some(n) = new {
            let cur = self.level(n, now).unwrap_or(0.0);
            self.set(n, level_tween(cur, 1.0, GLOW_IN));
        }
    }
    fn set(&mut self, b: Btn, t: Tween) {
        self.0.retain(|x| x.0 != b);
        if !t.dur.is_zero() { self.0.push((b, t)); }
    }
    pub fn levels(&self, now: Instant) -> Vec<(Btn, f64)> { self.0.iter().map(|(b, t)| (*b, t.at(now))).collect() }
    /// Drop finished fades; true while one still runs.
    pub fn prune(&mut self, now: Instant) -> bool {
        self.0.retain(|x| !x.1.done(now));
        !self.0.is_empty()
    }
}

/// Timers of the event loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    /// unmap a minimised window's top-level now that picom has seen its `_AERODE_MIN_SLOT`
    Unmap(u32),
    /// delete property `atom` of window `win`
    DelProp { win: u32, atom: u32 },
    /// Alt+Tab rested on an item: peek at that window
    SwitchPeek,
    /// an interactive resize held back for the client (_NET_WM_SYNC_REQUEST / frame pacing):
    /// apply the newest size once the client is ready
    ResizeFlush(u32),
}

#[derive(Default)]
pub struct Timers(Vec<(Instant, Timer)>);

impl Timers {
    /// Arm `t` (re-arming it when it is already pending).
    pub fn set(&mut self, after: Duration, t: Timer) {
        self.0.retain(|x| x.1 != t);
        self.0.push((Instant::now() + after, t));
    }
    pub fn cancel(&mut self, f: impl Fn(&Timer) -> bool) -> bool {
        let n = self.0.len();
        self.0.retain(|x| !f(&x.1));
        n != self.0.len()
    }
    pub fn pending(&self, t: &Timer) -> bool { self.0.iter().any(|x| x.1 == *t) }
    pub fn next(&self) -> Option<Instant> { self.0.iter().map(|x| x.0).min() }
    pub fn take_due(&mut self, now: Instant) -> Vec<Timer> {
        let (due, rest): (Vec<_>, Vec<_>) = self.0.drain(..).partition(|x| x.0 <= now);
        self.0 = rest;
        due.into_iter().map(|x| x.1).collect()
    }
}

/// A rectangle between two others.
pub fn lerp_rect(a: super::Rect, b: super::Rect, t: f64) -> super::Rect {
    let l = |x: i32, y: i32| (x as f64 + (y - x) as f64 * t).round() as i32;
    super::Rect { x: l(a.x, b.x), y: l(a.y, b.y), w: l(a.w, b.w).max(1), h: l(a.h, b.h).max(1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases() {
        for e in [Ease::Linear, Ease::Out, Ease::InOut] {
            assert_eq!(e.apply(0.0), 0.0);
            assert!((e.apply(1.0) - 1.0).abs() < 1e-9);
            assert_eq!(e.apply(2.0), 1.0);
        }
        assert!(Ease::Out.apply(0.25) > 0.25);
        assert!(Ease::InOut.apply(0.25) < 0.25 && Ease::InOut.apply(0.75) > 0.75);
    }

    #[test]
    fn glow_fades_both_ways() {
        let now = Instant::now();
        let mut g = Glow::default();
        g.hover(None, Some(Btn::Close), now);
        assert!(g.level(Btn::Close, now).unwrap() < 0.05);
        assert!((g.0[0].1.dur.as_millis() as i64 - GLOW_IN.as_millis() as i64).abs() <= 1);
        // leaving half-way: fades out from where it was, in half of GLOW_OUT
        let half = now + GLOW_IN / 2;
        g.0[0].1.t0 = now;
        g.hover(Some(Btn::Close), Some(Btn::Min), half);
        let out = g.0.iter().find(|x| x.0 == Btn::Close).unwrap().1;
        assert!((out.from - 0.5).abs() < 0.05 && out.to == 0.0);
        assert!((out.dur.as_millis() as i64 - (GLOW_OUT.as_millis() / 2) as i64).abs() <= 30);
        assert!(g.prune(half));
        assert!(!g.prune(half + GLOW_OUT * 2));
    }

    #[test]
    fn timers() {
        let mut t = Timers::default();
        t.set(Duration::from_millis(0), Timer::Unmap(5));
        t.set(Duration::from_secs(60), Timer::SwitchPeek);
        t.set(Duration::from_secs(60), Timer::Unmap(5));
        assert_eq!(t.0.len(), 2);
        assert!(t.pending(&Timer::Unmap(5)));
        assert!(t.take_due(Instant::now()).is_empty());
        assert!(t.cancel(|x| matches!(x, Timer::Unmap(5))));
        assert_eq!(t.take_due(Instant::now() + Duration::from_secs(61)), vec![Timer::SwitchPeek]);
        assert_eq!(t.next(), None);
    }
}
