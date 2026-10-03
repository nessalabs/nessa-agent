//! Native pairing cryptography and strict wire representations.
//!
//! ```text
//! local entropy -> one-use code -> OPAQUE registration (gateway only)
//! complete native TLS -> local exporter -> OPAQUE context -> confirmed claim
//! ```
#![deny(missing_docs)]
mod code;
mod context;
mod entropy;
pub use entropy::OsEntropy;
mod opaque;
mod private_state;
mod tls;
// The adapter's existing generic RNG inputs use this exact upstream interface.
pub use code::ManualCode;
pub use context::PairingContext;
pub use opaque::{
    credential_request_fingerprint, ClientAttempt, ConfirmedAttempt, PairingCryptoError,
    ServerAttempt, ServerInvitation,
};
pub use opaque_ke::rand;
pub use opaque_ke::rand::{CryptoRng, RngCore};
pub use tls::{GatewayTrust, NativeIdentity, NativeTransport};
/// Maximum raw enrollment cryptographic message consumed by these adapters.
/// Native framing independently validates the encoded envelope, including JSON
/// and base64 overhead. Protected product framing has separate directional bounds.
pub const MAX_ENROLLMENT_MESSAGE_BYTES: usize = 4096;

#[cfg(test)]
pub(crate) mod tests;

pub use private_state::FilePairingState;
