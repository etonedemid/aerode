//! System and Security / administrative applets.
use std::rc::Rc;
use super::super::Cp;

pub fn page_actioncenter(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Action Center", "Review recent messages and resolve problems.") }
pub fn page_firewall(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Windows Firewall", "Help protect your computer with Windows Firewall.") }
pub fn page_update(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Windows Update", "Check for updates.") }
pub fn page_backup(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Backup and Restore", "Back up or restore your files.") }
pub fn page_bitlocker(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "BitLocker Drive Encryption", "Encrypted drives.") }
pub fn page_admintools(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Administrative Tools", "Tools for system administrators.") }
pub fn page_eventvwr(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Event Viewer", "System logs.") }
pub fn page_taskschd(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Task Scheduler", "Scheduled tasks.") }
pub fn page_diskmgmt(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Disk Management", "Disks and volumes.") }
pub fn page_devmgr(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Device Manager", "Hardware devices.") }
pub fn page_sysinfo(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "System Information", "Hardware and software summary.") }
pub fn page_perfinfo(cp: &Rc<Cp>) -> gtk::Widget { super::todo(cp, "Performance Information and Tools", "Rate and improve your computer's performance.") }
