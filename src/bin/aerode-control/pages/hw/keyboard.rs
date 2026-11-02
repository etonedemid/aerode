//! Keyboard Properties: Speed (repeat delay and rate through `xset r rate`, the test box, cursor
//! blink rate for GTK through xsettings) and Hardware. Saved in config, re-applied at logon by
//! ~/.config/aerode/input.sh.
use super::*;
use super::super::admin::img;
use super::super::desktop::{group, write_xsettings, Sheet};
use std::cell::Cell;

fn vbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Vertical, s) }
fn hbox(s: i32) -> gtk::Box { gtk::Box::new(gtk::Orientation::Horizontal, s) }

/// Win7's repeat delay positions 0 (short, 250 ms) … 3 (long, 1000 ms).
fn delay_pos(ms: u32) -> f64 { ((ms.clamp(250, 1000) as f64 - 250.0) / 250.0).round() }
/// Win7's repeat rate 0 … 31 ≈ 2.5 … 30 characters a second.
fn rate_pos(cps: u32) -> f64 { ((cps as f64 - 2.5) * 31.0 / 27.5).round().clamp(0.0, 31.0) }
fn rate_cps(pos: f64) -> u32 { (2.5 + pos * 27.5 / 31.0).round().max(1.0) as u32 }
/// Cursor blink slider: 0 = None, else 1300 - value ms (1200 slow … 200 fast).
fn blink_pos(ms: u32) -> f64 { if ms == 0 { 0.0 } else { 1300.0 - ms.clamp(100, 1200) as f64 } }
fn blink_ms(pos: f64) -> u32 { if pos < 50.0 { 0 } else { (1300.0 - pos).round() as u32 } }

/// (delay ms, rate cps) the X server uses now.
fn xset_rate() -> Option<(u32, u32)> {
    let s = util::output(&["xset", "q"])?;
    let l = s.lines().find(|l| l.contains("auto repeat delay"))?;
    let w: Vec<&str> = l.split_whitespace().collect();
    let d = w.iter().position(|x| *x == "delay:").and_then(|i| w.get(i + 1)?.parse().ok())?;
    let r = w.iter().position(|x| *x == "rate:").and_then(|i| w.get(i + 1)?.parse().ok())?;
    Some((d, r))
}

pub fn page(cp: &Rc<Cp>) -> gtk::Widget { page_tab(cp, 0) }

pub fn page_tab(cp: &Rc<Cp>, tab: u32) -> gtk::Widget {
    css();
    let sheet = Sheet::new(cp);
    let k = config::get().hw.keyboard;
    // what the server does now (other tools may have changed it) wins over the saved values
    let (d0, r0) = xset_rate().unwrap_or((k.delay, k.rate));
    let t = vbox(10);
    let cr = vbox(8);
    let r1 = hbox(12);
    r1.pack_start(&img(&["input-keyboard", "preferences-desktop-keyboard"], 32), false, false, 0);
    let c1 = vbox(4);
    c1.pack_start(&lbl("Repeat _delay:"), false, false, 0);
    let (drow, delay) = slider(0.0, 3.0, 1.0, 3.0 - delay_pos(d0), "Long", "Short");
    c1.pack_start(&drow, false, false, 0);
    r1.pack_start(&c1, true, true, 0);
    cr.pack_start(&r1, false, false, 0);
    let r2 = hbox(12);
    r2.pack_start(&img(&["input-keyboard", "preferences-desktop-keyboard"], 32), false, false, 0);
    let c2 = vbox(4);
    c2.pack_start(&lbl("_Repeat rate:"), false, false, 0);
    let (rrow, rate) = slider(0.0, 31.0, 1.0, rate_pos(r0), "Slow", "Fast");
    c2.pack_start(&rrow, false, false, 0);
    r2.pack_start(&c2, true, true, 0);
    cr.pack_start(&r2, false, false, 0);
    cr.pack_start(&lbl("Click here and hold down a key to _test repeat rate:"), false, false, 0);
    let test = gtk::Entry::new();
    cr.pack_start(&test, false, false, 0);
    t.pack_start(&group("Character repeat", &cr), false, false, 0);
    let cb = hbox(12);
    let blink = Rc::new(Cell::new(k.blink));
    let caret = gtk::DrawingArea::new();
    caret.set_size_request(32, 32);
    let on = Rc::new(Cell::new(true));
    { let on = on.clone(); caret.connect_draw(move |w, cr| {
        if on.get() {
            cr.set_source_rgb(0.0, 0.0, 0.0);
            cr.rectangle((w.allocated_width() / 2) as f64, 6.0, 1.0, w.allocated_height() as f64 - 12.0);
            cr.fill().ok();
        }
        glib::Propagation::Stop
    }); }
    // blink the preview at the chosen rate
    { let (c, on, b) = (caret.clone(), on.clone(), blink.clone()); let next = Rc::new(Cell::new(std::time::Instant::now()));
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            if c.parent().is_none() { return glib::ControlFlow::Break; }
            let ms = b.get();
            if ms == 0 { if !on.get() { on.set(true); c.queue_draw(); } return glib::ControlFlow::Continue; }
            if next.get().elapsed().as_millis() as u32 >= ms { on.set(!on.get()); next.set(std::time::Instant::now()); c.queue_draw(); }
            glib::ControlFlow::Continue
        }); }
    let caret_frame = gtk::Frame::new(None);
    caret_frame.add(&caret);
    caret_frame.set_valign(gtk::Align::Center);
    cb.pack_start(&caret_frame, false, false, 0);
    let (brow, bs) = slider(0.0, 1100.0, 10.0, blink_pos(k.blink), "None", "Fast");
    cb.pack_start(&brow, true, true, 0);
    t.pack_start(&group("Cursor blink rate", &cb), false, false, 0);
    { let (b, d) = (blink.clone(), sheet.dirty()); bs.connect_value_changed(move |s| { b.set(blink_ms(s.value())); d(); }); }
    // the test box follows the sliders at once (Cancel puts the old values back)
    let live = { let (delay, rate) = (delay.clone(), rate.clone()); move || {
        let d = ((3.0 - delay.value()) * 250.0 + 250.0) as u32;
        util::spawn(&["xset", "r", "rate", &d.to_string(), &rate_cps(rate.value()).to_string()]);
    } };
    { let (l, d) = (live.clone(), sheet.dirty()); delay.connect_value_changed(move |_| { l(); d(); }); }
    { let (l, d) = (live.clone(), sheet.dirty()); rate.connect_value_changed(move |_| { l(); d(); }); }
    {
        // leaving without Apply/OK: back to the old repeat settings
        let applied = Rc::new(Cell::new(false));
        let a2 = applied.clone();
        let (delay2, rate2, blink2) = (delay.clone(), rate.clone(), blink.clone());
        sheet.on_apply(move || {
            let d = ((3.0 - delay2.value()) * 250.0 + 250.0) as u32;
            let r = rate_cps(rate2.value());
            let b = blink2.get();
            config::update(|c| { c.hw.keyboard = aerode::config::KeyboardOptions { delay: d, rate: r, blink: b }; });
            write_input_script();
            write_xsettings();
            a2.set(true);
            true
        });
        t.connect_destroy(move |_| {
            if !applied.get() && d0 > 0 && r0 > 0 { util::spawn(&["xset", "r", "rate", &d0.to_string(), &r0.to_string()]); }
        });
    }
    sheet.tab("Speed", &t);
    sheet.tab("Hardware", &super::mouse::hardware_tab(true));
    let w = sheet.widget();
    { let nb = sheet.nb.clone(); glib::idle_add_local_once(move || nb.set_current_page(Some(tab))); }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positions() {
        assert_eq!(delay_pos(500), 1.0);
        assert_eq!(rate_cps(31.0), 30);
        assert_eq!(rate_cps(rate_pos(25)) , 25);
        assert_eq!(blink_ms(blink_pos(530)), 530);
        assert_eq!(blink_ms(0.0), 0);
    }
}
