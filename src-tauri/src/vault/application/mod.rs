//! Typed commands consumed by the host; secret input has no Debug implementation.
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    Status,
    Setup,
    Unlock,
    Lock,
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
        }
    }
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    VaultUnavailable,
    VaultBusy,
    Forbidden,
}
/// Composition supplies the sole native process/pipe connection. Tests substitute it.
pub trait VaultConnection: Send + Sync {
    fn next_request(&self) -> u64;
    fn command(&self, request: Command, request_id: u64) -> Result<serde_json::Value, Failure>;
    fn interrupt(&self, request_id: u64);
    fn close(&self);
}

mod execute;
pub use execute::execute;
