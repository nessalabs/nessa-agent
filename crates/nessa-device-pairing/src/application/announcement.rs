//! Telling phones on the local network that this gateway is taking pairings.
//!
//! The desktop's pairing code and its signal orb carry only the eight code
//! symbols, never where the gateway is. While native linking listens on the
//! network, the gateway announces itself (`_nessa-link._tcp`, DNS-SD) so a
//! phone that scanned the code finds the gateway to enroll with. The
//! announcement says only what the listener already answers to anyone who
//! connects: its address, the gateway's public id and the machine's name. It
//! grants nothing; the code and the owner's approval still decide.
use std::fmt;
use std::net::SocketAddr;

/// What a gateway announces while its native listener takes connections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkingAnnouncement {
    /// The address the native listener serves: its configured IP (the
    /// unspecified one for every interface of that family) and its port.
    /// Only addresses the listener answers on are announced.
    pub address: SocketAddr,
    /// The gateway's public id, which a phone shows and later pins.
    pub gateway: String,
}

/// An announcement that could not be made. Pairing still works by address;
/// only finding the gateway without one does not.
#[derive(Debug)]
pub struct AnnouncementUnavailable(pub String);

impl fmt::Display for AnnouncementUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Makes and withdraws the announcement. Behind a port so the network side
/// is injected, and tests see what would have been said. The adapter names
/// the gateway as a phone lists it (the machine's name).
pub trait LinkingAnnouncer: Send + Sync {
    fn announce(
        &self,
        announcement: &LinkingAnnouncement,
    ) -> Result<Box<dyn Announced>, AnnouncementUnavailable>;
}

/// A live announcement. Withdrawn when the listener stops; dropping it
/// withdraws it too.
pub trait Announced: Send + Sync {
    fn withdraw(self: Box<Self>);
}
