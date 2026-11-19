//! Unlocking the vault with a fingerprint instead of typing its passphrase.
//!
//! The gateway only knows passphrases, so the host keeps the person's
//! passphrase in the OS credential store and hands it to the vault child only
//! after the fingerprint check passes. The check, not the store, is the gate:
//! [`BiometricStore::release`] shows the system prompt first and reads the
//! secret only after it succeeds, and it refuses a secret saved under a
//! different set of enrolled fingerprints.
//!
//! What may hold the passphrase follows one rule: only an unlock the gateway
//! just accepted saves it, and only a passphrase change the gateway just made
//! replaces it. Anything the gateway refuses as a wrong passphrase is dropped,
//! so a stale copy costs at most one refused attempt.
use super::{execute, Command, Failure, Operation, VaultConnection};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

/// Why the fingerprint path did not produce a passphrase. Every one of them
/// leaves the passphrase form as the way in.
/// Only a real fingerprint reader (macOS) can answer cancelled, changed or
/// missing; elsewhere `NoBiometrics` only ever answers unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum BiometricFailure {
    /// No fingerprint reader, none enrolled, locked out, or the store refused.
    #[serde(rename = "biometric_unavailable")]
    Unavailable,
    /// The person dismissed the prompt or chose to type the passphrase.
    #[serde(rename = "biometric_cancelled")]
    Cancelled,
    /// Fingerprints were added or removed since the passphrase was saved; the
    /// saved copy has been dropped. Only a fingerprint reader can tell.
    #[serde(rename = "biometric_changed")]
    Changed,
    /// Nothing is saved.
    #[serde(rename = "biometric_missing")]
    Missing,
}

/// The OS credential store and fingerprint reader, as one port. Composition
/// supplies the macOS Keychain + LocalAuthentication adapter, or a store that
/// is never available; tests substitute it.
pub trait BiometricStore: Send + Sync {
    /// Whether this device can check a fingerprint now. Never prompts.
    fn available(&self) -> bool;
    /// Whether a passphrase is saved. Never prompts and never reads the secret.
    fn saved(&self) -> bool;
    /// Save (or replace) the passphrase, bound to the fingerprints enrolled now.
    fn save(&self, passphrase: &str) -> Result<(), BiometricFailure>;
    /// Show the fingerprint prompt; only once it passes, read the passphrase.
    /// Blocks until the person answers the prompt.
    fn release(&self) -> Result<Zeroizing<String>, BiometricFailure>;
    /// Drop any saved passphrase. Dropping nothing is not a failure.
    fn forget(&self);
}

/// A device without a fingerprint reader, or a platform without this support.
pub struct NoBiometrics;
impl BiometricStore for NoBiometrics {
    fn available(&self) -> bool {
        false
    }
    fn saved(&self) -> bool {
        false
    }
    fn save(&self, _: &str) -> Result<(), BiometricFailure> {
        Err(BiometricFailure::Unavailable)
    }
    fn release(&self) -> Result<Zeroizing<String>, BiometricFailure> {
        Err(BiometricFailure::Unavailable)
    }
    fn forget(&self) {}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BiometricOperation {
    Status,
    Unlock,
    Forget,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiometricCommand {
    pub operation: BiometricOperation,
}

/// What the window learns: a vault refusal, or why no fingerprint unlock ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum BiometricRefusal {
    Vault(Failure),
    Biometric(BiometricFailure),
}

/// `status` and `forget` answer `{available, saved}`; `unlock` answers the
/// vault's own reply, exactly as `vault_command` would for a typed passphrase.
pub async fn execute_biometric(
    vault: Arc<dyn VaultConnection>,
    store: Arc<dyn BiometricStore>,
    command: BiometricCommand,
    deadline: Duration,
) -> Result<serde_json::Value, BiometricRefusal> {
    match command.operation {
        BiometricOperation::Status => Ok(status(store.as_ref())),
        BiometricOperation::Forget => {
            let pending = store.clone();
            tokio::task::spawn_blocking(move || pending.forget())
                .await
                .map_err(|_| BiometricRefusal::Vault(Failure::VaultUnavailable))?;
            Ok(status(store.as_ref()))
        }
        BiometricOperation::Unlock => {
            if !store.available() {
                return Err(BiometricRefusal::Biometric(BiometricFailure::Unavailable));
            }
            // The prompt waits on the person, so it is outside the vault
            // deadline, which ends the child connection when it expires.
            let pending = store.clone();
            let passphrase = tokio::task::spawn_blocking(move || pending.release())
                .await
                .map_err(|_| BiometricRefusal::Biometric(BiometricFailure::Unavailable))?
                .map_err(BiometricRefusal::Biometric)?;
            let reply = execute(
                vault,
                Command {
                    operation: Operation::Unlock,
                    passphrase: Some(passphrase),
                    remember: false,
                },
                deadline,
            )
            .await
            .map_err(BiometricRefusal::Vault)?;
            if refusal_code(&reply) == Some("vault_wrong_passphrase") {
                // Changed elsewhere (another app session, a lost change
                // reply): the copy can never unlock again.
                let pending = store.clone();
                let _ = tokio::task::spawn_blocking(move || pending.forget()).await;
            }
            Ok(reply)
        }
    }
}

/// After a typed-passphrase command: save on a requested, accepted unlock or
/// setup; replace on a change the gateway made while a copy is saved. A store
/// failure never turns the vault's answer into a failure; the next status
/// read simply says nothing is saved.
pub async fn settle_biometric(
    store: Arc<dyn BiometricStore>,
    operation: &Operation,
    remember: bool,
    passphrase: Option<Zeroizing<String>>,
    reply: &serde_json::Value,
) {
    let Some(passphrase) = passphrase else { return };
    let accepted = reply["ok"] == true && reply["status"]["state"] == "unlocked";
    let changed =
        reply["ok"] == true || refusal_code(reply) == Some("vault_changed_audit_unavailable");
    let save = match operation {
        Operation::Unlock | Operation::Setup => remember && accepted,
        Operation::ChangePassphrase => changed,
        Operation::Status | Operation::Lock => false,
    };
    if !save {
        return;
    }
    let replace_only = matches!(operation, Operation::ChangePassphrase);
    let _ = tokio::task::spawn_blocking(move || {
        if store.available() && (!replace_only || store.saved()) {
            let _ = store.save(&passphrase);
        }
    })
    .await;
}

fn status(store: &dyn BiometricStore) -> serde_json::Value {
    let available = store.available();
    serde_json::json!({ "available": available, "saved": available && store.saved() })
}

fn refusal_code(reply: &serde_json::Value) -> Option<&str> {
    if reply["ok"] == false {
        reply["code"].as_str()
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../../../tests/vault/biometrics.rs"]
mod tests;
