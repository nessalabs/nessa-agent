//! Pure pairing decisions: no crypto, network, filesystem or async runtime.
#[path = "domain/pairing/invitation.rs"]
mod invitation;

#[path = "domain/pairing/disclosure.rs"]
mod disclosure;

#[path = "domain/pairing/peer.rs"]
mod peer;
