//! Private-file persistence, reviewed cryptographic primitives and metadata audit.
mod encrypted_file;
pub use encrypted_file::{EncryptedFileVault, OsEntropy};

mod audit;
pub use audit::FileAudit;
