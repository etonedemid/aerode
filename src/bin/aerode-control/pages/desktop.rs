//! Appearance and desktop applets.
use std::rc::Rc;
use super::super::Cp;

pub fn page_gadgets(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Desktop Gadgets", "Add gadgets to the desktop.") }
pub fn page_screensaver(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Screen Saver Settings", "Screen saver and lock settings.") }
pub fn page_desktopicons(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Desktop Icon Settings", "Desktop icons.") }
pub fn page_taskbar(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Taskbar and Start Menu Properties", "Taskbar and Start menu settings.") }
pub fn page_folderoptions(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Folder Options", "How folders and files are shown.") }
pub fn page_fonts(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Fonts", "Preview, delete, or show and hide the fonts installed on your computer.") }
pub fn page_gettingstarted(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Getting Started", "Learn about aerode.") }
