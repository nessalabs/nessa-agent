//! Where serving reads what this host's owner grants each gateway: the
//! [`PeerGrants`] port, read again at every grant, so a change applies from
//! the next lease while the leases already running go on.
//!
//! ```text
//! Grant / GrantCommand ──▶ grant_for(peer) ──▶ PeerGrants::entry(peer)
//!   entry, or none ──▶ granted_to(peer, entry) (the domain's defaults)
//!   unreadable ──▶ logged, and nothing granted
//! ```
//!
//! Arrows are calls, in order. A grant that cannot be read grants nothing,
//! to SSH as to a dialed gateway: a file somebody broke is not taken as no
//! file.
use crate::env_serve::domain::{granted_to, Peer, PeerGrant};

/// What this host's owner grants each peer, as they keep it.
pub(crate) trait PeerGrants: Send + Sync {
    /// `peer`'s entry, or `None` when it has none or there are no grants
    /// at all.
    ///
    /// # Errors
    /// The grants could not be read, or are not ones this build reads; the
    /// message says which.
    fn entry(&self, peer: &Peer) -> Result<Option<PeerGrant>, String>;
}

/// What `peer` is granted now: its entry with the domain's defaults, or
/// nothing when the grants could not be read.
pub(crate) fn grant_for(grants: &dyn PeerGrants, peer: &Peer) -> PeerGrant {
    match grants.entry(peer) {
        Ok(entry) => granted_to(peer, entry),
        Err(error) => {
            let peer = match peer {
                Peer::Ssh => "ssh",
                Peer::Gateway { .. } => "a dialed gateway",
            };
            tracing::error!(peer, %error, "this host's grants could not be read; nothing is granted");
            PeerGrant::nothing()
        }
    }
}
