//! Registry-owned pairing persistence, conditional publication, and key authentication.
//! Restored history asks the domain transition to correlate cancellation and revocation.
mod device_verifier;
mod enrollment;
mod projection;
pub use device_verifier::DeviceCredentialVerifier;
pub(super) use projection::StoredPairing;

#[cfg(all(test, unix))]
mod tests;
