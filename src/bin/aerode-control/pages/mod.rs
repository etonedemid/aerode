//! Control Panel applets that live outside main.rs, one file per group so they can grow independently.
pub mod admin;
pub mod core;
pub mod hw;
pub mod desktop;
pub mod extras;

use std::rc::Rc;
use super::{text, title, Cp};
use gtk::prelude::*;

/// Placeholder for an applet that is not implemented yet.
#[allow(dead_code)]
pub fn todo(_cp: &Rc<Cp>, name: &str, what: &str) -> gtk::Widget {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 8);
    v.pack_start(&title(name), false, false, 0);
    v.pack_start(&text(what), false, false, 0);
    v.upcast()
}
