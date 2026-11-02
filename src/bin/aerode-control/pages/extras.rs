//! Network, hardware and other applets: Recovery, Troubleshooting, HomeGroup, Internet Options,
//! AutoPlay, Windows Mobility Center, Color Management and Credential Manager (one file each in
//! extras/, shared helpers in extras/common.rs).
pub(crate) mod common;
mod autoplay;
mod colormgmt;
mod credentials;
mod homegroup;
mod internet;
mod mobility;
mod recovery;
mod troubleshoot;

pub use autoplay::page_autoplay;
pub use colormgmt::page_colormgmt;
pub use credentials::page_credentials;
pub use homegroup::page_homegroup;
pub use internet::page_internet;
pub use mobility::page_mobility;
pub use recovery::page_recovery;
pub use troubleshoot::page_troubleshooting;
