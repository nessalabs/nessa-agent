//! Fixed identities, public key bytes, immutable read consent and finite policy.
//! wire-values.json publishes widths; the product generator compiles wire_values.rs.
mod wire_values;
pub(crate) use wire_values::MANUAL_CODE_BYTES;
mod values;
pub use values::{
    peer_principal, AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, DeviceKey,
    InvitationId, PairingError, PairingInitiator, PairingPolicy, PEER_PRINCIPAL_PREFIX,
};

mod public_intent;
pub use public_intent::PublicIntent;

mod disclosed_consent;
pub use disclosed_consent::DisclosedConsent;
