//! Typed commands consumed by the host; secret input has no Debug implementation.
pub(crate) mod contract;
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Status,
    Setup,
    Unlock,
    Lock,
    ChangePassphrase,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub operation: Operation,
    pub passphrase: Option<zeroize::Zeroizing<String>>,
}
impl Command {
    pub fn operation(&self) -> &'static str {
        match self.operation {
            Operation::Status => "status",
            Operation::Setup => "setup",
            Operation::Unlock => "unlock",
            Operation::Lock => "lock",
            Operation::ChangePassphrase => "changePassphrase",
        }
    }
    /// Losing the answer after dispatch may hide a durable passphrase change.
    /// The native timeout and blocking-panic tests enforce this distinction.
    pub fn unconfirmed_failure(&self) -> Failure {
        match self.operation {
            Operation::ChangePassphrase => Failure::VaultChangeNotConfirmed,
            _ => Failure::VaultUnavailable,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    /// No delivery was attempted for change, or an ordinary read/open failed.
    VaultUnavailable,
    VaultBusy,
    /// A change may have reached storage; the caller must not replay it.
    VaultChangeNotConfirmed,
    Forbidden,
}
/// Composition supplies the sole native process/pipe connection. Tests substitute it.
pub trait VaultConnection: Send + Sync {
    fn next_request(&self) -> u64;
    /// Preserve the exact gateway receipt when available. For change, an
    /// unavailable error is pre-dispatch only; missing answers after any bytes
    /// may have been sent return VaultChangeNotConfirmed and retire the child.
    fn command(&self, request: Command, request_id: u64) -> Result<serde_json::Value, Failure>;
    fn interrupt(&self, request_id: u64);
    fn close(&self);
}

mod execute;
pub use execute::execute;
