//! Registry-owned pairing persistence, conditional publication, and key authentication.
//! Restored history asks the domain transition to correlate cancellation and revocation.
mod credential_device;
mod device_verifier;
mod discovery;
pub use discovery::GrantorPeer;
mod enrollment;
mod owner_phone;
mod owner_phone_original;
mod owner_phone_purpose;
mod projection;
pub use device_verifier::DeviceCredentialVerifier;
pub(super) use projection::StoredPairing;

#[cfg(all(test, unix))]
mod tests;

mod person_links;
pub(super) use person_links::owner_principal;
