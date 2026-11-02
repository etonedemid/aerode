//! Network, hardware and other applets.
use std::rc::Rc;
use super::super::Cp;

pub fn page_recovery(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Recovery", "Restore this computer to an earlier time.") }
pub fn page_troubleshooting(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Troubleshooting", "Find and fix problems.") }
pub fn page_homegroup(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "HomeGroup", "Share files and printers.") }
pub fn page_internet(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Internet Options", "Browser and connection settings.") }
pub fn page_autoplay(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "AutoPlay", "What happens when you insert media or a device.") }
pub fn page_mobility(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Windows Mobility Center", "Brightness, volume, battery, display.") }
pub fn page_colormgmt(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Color Management", "Color profiles.") }
pub fn page_credentials(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Credential Manager", "Saved passwords.") }
