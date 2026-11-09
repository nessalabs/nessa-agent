//! Pure constrained secrets and the vault's observable lifecycle.
mod passphrase;
mod passphrase_change;
mod values;
pub use passphrase::Passphrase;
pub use passphrase_change::ChangePublication;
pub use values::{EntryName, Failure, Secret, Status, VaultKey};
