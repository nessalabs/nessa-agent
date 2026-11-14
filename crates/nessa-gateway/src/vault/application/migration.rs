use nessa_vault::{
    application::Vault,
    domain::{EntryName, Failure, Secret},
};
/// Captures old secrets behind a source-owned, cross-process migration lease.
pub trait Migration: Send + Sync {
    fn capture(&self) -> Result<Box<dyn MigrationSnapshot>, Failure>;
}
/// The source lease outlives destination publication and retirement verification.
pub trait MigrationSnapshot: Send {
    fn entries(&self) -> &[(EntryName, Secret)];
    fn required(&self) -> &[EntryName];
    fn retire(&self) -> Result<(), Failure>;
}
/// Every required ciphertext key and source value has a verified encrypted receipt
/// before the adapter may delete any source. A failed retirement can be retried.
pub fn verify_and_retire(vault: &Vault, snapshot: &dyn MigrationSnapshot) -> Result<(), Failure> {
    for name in snapshot.required() {
        if vault.read(name)?.is_none() {
            return Err(Failure::MigrationUnavailable);
        }
    }
    for (name, expected) in snapshot.entries() {
        if vault.read(name)?.as_ref().map(|value| value.as_slice()) != Some(expected.as_slice()) {
            return Err(Failure::MigrationUnavailable);
        }
    }
    snapshot.retire()
}
