//! The gateway's voice key and the calls it starts: the one owner of both
//! (`docs/design/voice-chat.md`, rows K1–K9). Every change is recorded
//! before it is made, and a record that cannot be kept refuses the change;
//! what then happened (saved, forgotten, minted, or why not) is recorded
//! after, so a request is never mistaken for its effect.
use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use super::domain::{CallGrant, VoiceKey, MODEL};
use crate::secrets::{SecretStore, SecretUnavailable};

/// Who asked, as every record names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceCaller {
    pub organization_id: String,
    pub principal_id: String,
    pub credential_id: String,
}

/// Why a provider minted no call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintFailure {
    /// The provider refused the key.
    Rejected,
    /// Unreachable, timed out, or answered something unreadable.
    Unreachable,
}

/// Where calls are minted: OpenAI, or the scripted provider the gateway was
/// started with. Adapters live in `infrastructure`.
#[async_trait]
pub trait RealtimeProvider: Send + Sync {
    async fn mint(&self, key: &VoiceKey) -> Result<CallGrant, MintFailure>;
}

/// The record could not be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditUnavailable;

/// What a record says happened. Never the key or a client secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    /// A key is about to be saved; `replacing` when one already was.
    KeySaveRequested { replacing: bool },
    /// The save did not complete; whatever was saved still is.
    KeySaveFailed,
    /// The key is saved.
    KeySaved,
    /// The saved key is about to be deleted.
    KeyForgetRequested,
    /// The delete did not complete; the key is still saved.
    KeyForgetFailed,
    /// No key is saved now.
    KeyForgotten,
    /// A call is about to be minted for this conversation.
    CallRequested { conversation_id: String },
    /// The provider minted one: a secret went to the caller.
    CallMinted { conversation_id: String },
    /// The provider minted none.
    CallRefused {
        conversation_id: String,
        reason: MintFailure,
    },
}

/// One record. `operation_id` joins a request to its failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceRecord {
    pub operation_id: String,
    pub caller: VoiceCaller,
    pub event: VoiceEvent,
}

pub trait VoiceAudit: Send + Sync {
    fn record(&self, record: &VoiceRecord) -> Result<(), AuditUnavailable>;
}

/// Why a voice method was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceError {
    VaultLocked,
    KeyMissing,
    KeyRejected,
    Unreachable,
    StorageUnavailable,
    AuditUnavailable,
}

/// Whether a key is saved, and the model a call runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceStatus {
    pub key_saved: bool,
    pub model: &'static str,
}

pub struct VoiceCalls {
    key: Arc<dyn SecretStore>,
    /// Shared with the detached task each mint runs in.
    provider: Arc<dyn RealtimeProvider>,
    audit: Arc<dyn VoiceAudit>,
    /// Key changes one at a time, so each sees what the last one left.
    changes: tokio::sync::Mutex<()>,
}

impl VoiceCalls {
    pub fn new(
        key: Arc<dyn SecretStore>,
        provider: Arc<dyn RealtimeProvider>,
        audit: Arc<dyn VoiceAudit>,
    ) -> Self {
        Self {
            key,
            provider,
            audit,
            changes: tokio::sync::Mutex::new(()),
        }
    }

    fn saved(&self) -> Result<Option<VoiceKey>, VoiceError> {
        match self.key.read() {
            // An entry that is not a key's shape is not sent anywhere.
            Ok(bytes) => Ok(bytes.and_then(|bytes| VoiceKey::parse(&bytes))),
            Err(SecretUnavailable::Locked) => Err(VoiceError::VaultLocked),
            Err(SecretUnavailable::Unavailable) => Err(VoiceError::StorageUnavailable),
        }
    }

    fn record(
        &self,
        operation_id: &str,
        caller: &VoiceCaller,
        event: VoiceEvent,
    ) -> Result<(), VoiceError> {
        record_in(self.audit.as_ref(), operation_id, caller, event)
    }

    /// A failure's record, once the change was refused. The failure is what
    /// the caller hears; a record of it that cannot be kept does not hide
    /// it, and is said in the log.
    fn record_failure(&self, operation_id: &str, caller: &VoiceCaller, event: VoiceEvent) {
        record_failure_in(self.audit.as_ref(), operation_id, caller, event)
    }

    fn status_with(&self, key_saved: bool) -> VoiceStatus {
        VoiceStatus {
            key_saved,
            model: MODEL,
        }
    }

    /// K1: whether a key is saved.
    pub fn status(&self) -> Result<VoiceStatus, VoiceError> {
        Ok(self.status_with(self.saved()?.is_some()))
    }

    /// K3: save `key`, replacing any saved one.
    pub async fn save_key(
        &self,
        caller: VoiceCaller,
        key: VoiceKey,
    ) -> Result<VoiceStatus, VoiceError> {
        let _change = self.changes.lock().await;
        let replacing = self.saved()?.is_some();
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            &caller,
            VoiceEvent::KeySaveRequested { replacing },
        )?;
        if let Err(problem) = self.key.write(key.as_bytes()) {
            self.record_failure(&operation_id, &caller, VoiceEvent::KeySaveFailed);
            return Err(match problem {
                SecretUnavailable::Locked => VoiceError::VaultLocked,
                SecretUnavailable::Unavailable => VoiceError::StorageUnavailable,
            });
        }
        // A change made but not recorded is never answered as a success: the
        // key stays saved, and the caller hears that the audit failed.
        self.record(&operation_id, &caller, VoiceEvent::KeySaved)?;
        Ok(self.status_with(true))
    }

    /// K4, K5: delete the saved key.
    pub async fn forget_key(&self, caller: VoiceCaller) -> Result<VoiceStatus, VoiceError> {
        let _change = self.changes.lock().await;
        // A malformed entry is still an entry to delete.
        let present = match self.key.read() {
            Ok(bytes) => bytes.is_some(),
            Err(SecretUnavailable::Locked) => return Err(VoiceError::VaultLocked),
            Err(SecretUnavailable::Unavailable) => return Err(VoiceError::StorageUnavailable),
        };
        if !present {
            return Ok(self.status_with(false));
        }
        let operation_id = Uuid::new_v4().to_string();
        self.record(&operation_id, &caller, VoiceEvent::KeyForgetRequested)?;
        if let Err(problem) = self.key.delete() {
            self.record_failure(&operation_id, &caller, VoiceEvent::KeyForgetFailed);
            return Err(match problem {
                SecretUnavailable::Locked => VoiceError::VaultLocked,
                SecretUnavailable::Unavailable => VoiceError::StorageUnavailable,
            });
        }
        self.record(&operation_id, &caller, VoiceEvent::KeyForgotten)?;
        Ok(self.status_with(false))
    }

    /// K6–K9: mint one call about `conversation_id` with the saved key.
    /// Calls are not serialized behind key changes: each reads the key as
    /// it stands when it starts.
    pub async fn start_call(
        &self,
        caller: VoiceCaller,
        conversation_id: String,
    ) -> Result<CallGrant, VoiceError> {
        let Some(key) = self.saved()? else {
            return Err(VoiceError::KeyMissing);
        };
        let operation_id = Uuid::new_v4().to_string();
        self.record(
            &operation_id,
            &caller,
            VoiceEvent::CallRequested {
                conversation_id: conversation_id.clone(),
            },
        )?;
        // The mint and its outcome's record run on their own: a caller that
        // goes away mid-mint (a dropped socket) leaves a record of what the
        // provider did, never a request with no end.
        let provider = self.provider.clone();
        let audit = self.audit.clone();
        let mint = tokio::spawn(async move {
            let outcome = provider.mint(&key).await;
            let event = match &outcome {
                Ok(_) => VoiceEvent::CallMinted { conversation_id },
                Err(reason) => VoiceEvent::CallRefused {
                    conversation_id,
                    reason: *reason,
                },
            };
            match outcome {
                // A grant whose minting is not on record is not handed out.
                Ok(grant) => {
                    record_in(audit.as_ref(), &operation_id, &caller, event).map(|()| grant)
                }
                Err(reason) => {
                    record_failure_in(audit.as_ref(), &operation_id, &caller, event);
                    Err(match reason {
                        MintFailure::Rejected => VoiceError::KeyRejected,
                        MintFailure::Unreachable => VoiceError::Unreachable,
                    })
                }
            }
        });
        mint.await.unwrap_or(Err(VoiceError::Unreachable))
    }
}

fn record_in(
    audit: &dyn VoiceAudit,
    operation_id: &str,
    caller: &VoiceCaller,
    event: VoiceEvent,
) -> Result<(), VoiceError> {
    audit
        .record(&VoiceRecord {
            operation_id: operation_id.to_owned(),
            caller: caller.clone(),
            event,
        })
        .map_err(|AuditUnavailable| VoiceError::AuditUnavailable)
}

fn record_failure_in(
    audit: &dyn VoiceAudit,
    operation_id: &str,
    caller: &VoiceCaller,
    event: VoiceEvent,
) {
    if record_in(audit, operation_id, caller, event).is_err() {
        tracing::warn!(
            operation_id,
            "a voice failure could not be recorded in the audit"
        );
    }
}
