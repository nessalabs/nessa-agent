use crate::domain::{ChangePublication, EntryName, Failure, Passphrase, Secret, Status, VaultKey};
/// The authenticated initiator; fields contain identifiers, never credentials.
#[derive(Clone, Debug)]
pub struct Caller {
    pub principal: String,
    pub credential: String,
}
/// Every consequential attempt names the actual before/after state and cause.
#[derive(Clone, Debug)]
pub struct AuditRecord {
    pub target: Option<EntryName>,
    pub operation: u64,
    pub lease: String,
    pub caller: Caller,
    pub action: &'static str,
    pub before: Status,
    pub after: Status,
    pub cause: &'static str,
    pub result: &'static str,
}
/// A bounded durable metadata journal that is writable while the vault is locked.
pub trait Audit: Send + Sync {
    fn record(&self, record: &AuditRecord) -> Result<(), Failure>;
}
/// Monotonic elapsed seconds for bounded failed-attempt cooldowns.
pub trait Clock: Send + Sync {
    fn seconds(&self) -> u64;
}
/// Injected cryptographic entropy; production uses the operating system source.
pub trait Entropy: Send + Sync {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), Failure>;
}
/// One encrypted repository; adapters own bounded IO and cryptographic formats.
pub trait Repository: Send + Sync {
    fn configured(&self) -> Result<bool, Failure>;
    fn create(
        &self,
        passphrase: &Passphrase,
        entries: &[(EntryName, Secret)],
    ) -> Result<VaultKey, Failure>;
    fn unlock(&self, passphrase: &Passphrase) -> Result<VaultKey, Failure>;
    /// Replace only the passphrase encryption, preserving every entry. The caller
    /// seals shared key state before dispatch and never automatically retries.
    fn change_passphrase(
        &self,
        current_key: &VaultKey,
        new_passphrase: &Passphrase,
    ) -> ChangePublication;
    fn read(&self, key: &VaultKey, name: &EntryName) -> Result<Option<Secret>, Failure>;
    fn write(&self, key: &VaultKey, name: &EntryName, value: &[u8]) -> Result<(), Failure>;
    fn delete(&self, key: &VaultKey, name: &EntryName) -> Result<(), Failure>;
}

/// UTC observation time assigned by the durable audit adapter, not external-effect time.
pub trait ObservationClock: Send + Sync {
    fn unix_millis(&self) -> Result<u64, Failure>;
}
