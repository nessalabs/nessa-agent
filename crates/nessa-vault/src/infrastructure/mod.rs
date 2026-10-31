//! Private-file persistence, reviewed cryptographic primitives and metadata audit.
mod encrypted_file;
pub use encrypted_file::{EncryptedFileVault, FilePublication, NativeFilePublication, OsEntropy};

mod audit;
pub use audit::FileAudit;
