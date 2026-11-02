//! Fades of the shell's own windows (tray flyouts, notification balloons).
//!
//! Win7 fades these in (and balloons out again) itself. The fade runs on the window's opacity, so it
//! needs a compositor: GTK paints the alpha itself for windows with an RGBA visual (all of aerode's
//! glass) and sets `_NET_WM_WINDOW_OPACITY` for the others. The window is marked `_AERODE_NO_ANIM`
//! so that the compositor adds no fade of its own on top (picom 10/11: `fade-exclude`, picom 12: the
//! rule in `crate::picomconf`).
//!
//! Timings (estimates; Microsoft published none): flyouts fade in over 1/6 s – the UX guide's
//! preferred short duration – and close at once; balloons fade in and out over 200 ms.
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

pub const FLYOUT_IN: Duration = Duration::from_millis(167);
pub const BALLOON_IN: Duration = Duration::from_millis(200);
pub const BALLOON_OUT: Duration = Duration::from_millis(200);

/// Is a Visual Effects switch (Control Panel > System > Performance Options) on?
pub fn effect(key: &str) -> bool { crate::config::get().hw.effect(key) }

/// Tell the compositor that `w` animates itself.
pub fn self_animated(w: &impl IsA<gtk::Window>) {
    fn tag(w: &gtk::Window) {
        if let Some(gw) = w.window() {
            gdk::property_change(&gw, &gdk::Atom::intern("_AERODE_NO_ANIM"), &gdk::Atom::intern("CARDINAL"), 32,
                gdk::PropMode::Replace, gdk::ChangeData::ULongs(&[1]));
        }
    }
    let w = w.upcast_ref::<gtk::Window>();
    if w.is_realized() { tag(w); }
    w.connect_realize(tag);
}

fn composited(w: &gtk::Window) -> bool { WidgetExt::screen(w).map(|s| s.is_composited()).unwrap_or(false) }

/// Fade `w` from its current opacity to `to` over `dur` (fast start, soft landing when it appears),
/// then call `done`. Jumps at once when `enabled` is false or nothing composites. A newer fade of
/// the same window takes over and the older one's `done` never runs.
pub fn fade(w: &impl IsA<gtk::Window>, to: f64, dur: Duration, enabled: bool, done: impl FnOnce(&gtk::Window) + 'static) {
    let w = w.upcast_ref::<gtk::Window>();
    let generation = next_generation(w);
    if !enabled || dur.is_zero() || !composited(w) || !w.is_visible() {
        w.set_opacity(to);
        done(w);
        return;
    }
    let from = w.opacity();
    let t0 = Instant::now();
    let done = RefCell::new(Some(done));
    let stopped = Cell::new(false);
    w.add_tick_callback(move |w, _| {
        if stopped.get() || current_generation(w) != generation { return glib::ControlFlow::Break; }
        let t = (t0.elapsed().as_secs_f64() / dur.as_secs_f64()).min(1.0);
        let e = if to > from { 1.0 - (1.0 - t).powi(3) } else { t };
        w.set_opacity(from + (to - from) * e);
        if t < 1.0 { return glib::ControlFlow::Continue; }
        stopped.set(true);
        if let Some(d) = done.borrow_mut().take() { d(w); }
        glib::ControlFlow::Break
    });
}

/// Show `w` fading in from transparent.
pub fn show_faded(w: &impl IsA<gtk::Window>, dur: Duration, enabled: bool) {
    let w = w.upcast_ref::<gtk::Window>();
    let animate = enabled && composited(w);
    next_generation(w);
    w.set_opacity(if animate { 0.0 } else { 1.0 });
    w.show();
    if animate { fade(w, 1.0, dur, true, |_| {}); }
}

const KEY: &str = "aerode-fade-generation";

fn current_generation(w: &gtk::Window) -> u64 {
    // SAFETY: KEY is only ever set to a u64, by next_generation below
    unsafe { w.data::<u64>(KEY).map(|p| *p.as_ref()).unwrap_or(0) }
}

fn next_generation(w: &gtk::Window) -> u64 {
    let g = current_generation(w) + 1;
    // SAFETY: see current_generation
    unsafe { w.set_data::<u64>(KEY, g) };
    g
}
