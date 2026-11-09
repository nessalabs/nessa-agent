//! App-lifetime bridge to the gateway's vault. The host retains one child/socket
//! across windows; app quit or pipe EOF releases that unique unlock owner.
pub mod application;
pub mod infrastructure;
