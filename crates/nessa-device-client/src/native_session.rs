//! Synchronous generated session bindings. UI composition owns worker dispatch
//! and bounded pre-queue admission; this adapter owns no second authority rule.
use crate::native_owner_phone::{
    reservation, NativeOwnerPhonePreparation, NativeOwnerPreparationCallError,
};
use crate::native_product::NativeProductSession;
use crate::{
    failures::{NativeCallError, NativeEnrollmentFailure, NativeReadFailure, NativeSessionFailure},
    native_owner_recovery::NativeOwnerStatus,
    saved::{NativeScope, NativeTranscript},
    session::{
        LocalEnrollment, LocalIssuedEnrollment, NativeSession, SessionCloseObservation,
        SessionFailure, SessionFollow, SessionRead, SessionRefresh, SessionStatus,
    },
};
use nessa_auth::{
    adapters::pairing::ManualCode,
    domain::pairing::{AttemptFailure, AttemptOutcome, ConsentClass, PublicIntent, TerminalCause},
};
use nessa_client_core::{
    pairing::{NativeClientError, NativeRetryOutcome},
    retained::{
        artifact_name_accepted, ArtifactFaultKind, ArtifactFetch, ArtifactFileStep,
        ArtifactFreshness, ArtifactPartCleanup, ArtifactRequest, CataloguePresence, PhoneStamp,
        ReadStop, RetainedCatalogueEntry, RetainedCataloguePage, RetainedPurge,
    },
};
use nessa_protocol::{
    conversation::domain::ConversationId,
    pairing::wire::NativePairingStatus,
    product::{generated::ConversationViewCursor, passive_read::conforming_view_cursor},
};
use nessa_sync::replication::{catalogue::EntryKey, domain::Id};
use std::{net::SocketAddr, path::Path, sync::Arc};
use uuid::Uuid;
use zeroize::Zeroizing;

/// A caller-owned stop latch. Creating/stopping this object performs no enrollment.
#[derive(uniffi::Object)]
pub struct NativeReadStop {
    value: Arc<ReadStop>,
}
#[uniffi::export]
impl NativeReadStop {
    /// Reserve the stop before dispatch so a pre-dispatch stop is retained.
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            value: ReadStop::new(),
        }
    }
    /// Ask the existing core to stop socket reads; OS connect remains bounded by HANDSHAKE.
    pub fn stop(&self) {
        self.value.stop();
    }
    /// Whether this caller-owned latch has been stopped.
    pub fn is_stopped(&self) -> bool {
        self.value.is_stopped()
    }
}
impl Default for NativeReadStop {
    fn default() -> Self {
        Self::new()
    }
}

/// Existing enrollment phases; this representation does not grant product access.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeEnrollmentPhase {
    /// Original attempt may claim.
    Pending,
    /// Original attempt failed or was superseded.
    Unclaimed,
    /// Key claimed before explicit approval.
    Claimed,
    /// Owner approved; physical publication may remain.
    Approved,
    /// Durable physical stage exists.
    Staging,
    /// Historical enrollment completion, checked afresh for each product read.
    Active,
    /// Authenticated enrollment ended.
    Terminal,
}
/// Original terminal enrollment cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeTerminalCause {
    /// Published device credential revoked.
    CredentialRevoked,
    /// Owner denied this enrollment.
    Denied,
    /// Owner canceled it.
    Cancelled,
    /// Invitation expired.
    Expired,
    /// Gateway lost the volatile pre-claim setup.
    Restarted,
}
impl From<TerminalCause> for NativeTerminalCause {
    fn from(value: TerminalCause) -> Self {
        match value {
            TerminalCause::CredentialRevoked => Self::CredentialRevoked,
            TerminalCause::Denied => Self::Denied,
            TerminalCause::Cancelled => Self::Cancelled,
            TerminalCause::Expired => Self::Expired,
            TerminalCause::Restarted => Self::Restarted,
        }
    }
}
/// Exact original failed-attempt cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeAttemptFailure {
    /// Crypto refused proof.
    InvalidProof,
    /// Original connection closed.
    ConnectionClosed,
    /// Handshake deadline ended the attempt.
    HandshakeDeadline,
    /// Verifier could not authenticate the claim.
    VerifierUnavailable,
}
impl From<AttemptFailure> for NativeAttemptFailure {
    fn from(value: AttemptFailure) -> Self {
        match value {
            AttemptFailure::InvalidProof => Self::InvalidProof,
            AttemptFailure::ConnectionClosed => Self::ConnectionClosed,
            AttemptFailure::HandshakeDeadline => Self::HandshakeDeadline,
            AttemptFailure::VerifierUnavailable => Self::VerifierUnavailable,
        }
    }
}
/// Original attempt state, separate from invitation terminal cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeAttemptState {
    /// Original attempt remains pending.
    Pending,
    /// Failed with its original cause.
    Failed,
    /// Winning key claim belongs to this attempt.
    Claimed,
    /// Another attempt or terminal invitation superseded it.
    Superseded,
}
/// An immutable status receipt, used for enrollment and retry results as well
/// as completed pinned status. Caller mutation cannot supply Active to the driver.
#[derive(uniffi::Object)]
pub struct NativeEnrollmentReceipt {
    pub(crate) value: NativePairingStatus,
}
#[uniffi::export]
impl NativeEnrollmentReceipt {
    /// Exact historical public correlation; this is not claim or read authority.
    pub fn public_intent(&self) -> Arc<NativePublicIntent> {
        Arc::new(NativePublicIntent {
            value: self.value.public(),
        })
    }
    /// Original phase.
    pub fn phase(&self) -> NativeEnrollmentPhase {
        match self.value {
            NativePairingStatus::Pending(_) => NativeEnrollmentPhase::Pending,
            NativePairingStatus::Unclaimed { .. } => NativeEnrollmentPhase::Unclaimed,
            NativePairingStatus::Claimed(_) => NativeEnrollmentPhase::Claimed,
            NativePairingStatus::Approved(_) => NativeEnrollmentPhase::Approved,
            NativePairingStatus::Staging(_) => NativeEnrollmentPhase::Staging,
            NativePairingStatus::Active { .. } => NativeEnrollmentPhase::Active,
            NativePairingStatus::Terminal { .. } => NativeEnrollmentPhase::Terminal,
        }
    }
    /// Exact opaque intent equality across owned historical receipts.
    pub fn same_intent(&self, other: Arc<Self>) -> bool {
        self.value.public() == other.value.public()
    }
    /// Issued receiver only when Active was reported.
    pub fn receiver(&self) -> Option<String> {
        match &self.value {
            NativePairingStatus::Active { receiver, .. } => Some(receiver.as_str().to_owned()),
            _ => None,
        }
    }
    /// Current epoch in the original Active receipt, preserved as UInt64.
    pub fn access_epoch(&self) -> Option<u64> {
        match self.value {
            NativePairingStatus::Active { access_epoch, .. } => Some(access_epoch),
            _ => None,
        }
    }
    /// Original issued credential identity; not a bearer secret.
    pub fn credential(&self) -> Option<String> {
        match &self.value {
            NativePairingStatus::Active { credential, .. } => Some(credential.as_str().to_owned()),
            _ => None,
        }
    }
    /// Authenticated disclosed gateway audience when that scope was available.
    pub fn audience(&self) -> Option<String> {
        self.value
            .consent()
            .map(|value| value.audience().as_str().to_owned())
    }
    /// Disclosed grant organization when available; no socket-derived scope.
    pub fn organization(&self) -> Option<String> {
        self.value.consent().map(|value| {
            value
                .grant()
                .resource()
                .organization_id()
                .as_str()
                .to_owned()
        })
    }
    /// Disclosed grant resource identity when available; not a local grant.
    pub fn resource(&self) -> Option<String> {
        self.value
            .consent()
            .map(|value| value.grant().resource().id().as_str().to_owned())
    }
    /// Original terminal cause, independently of private-state cleanup success.
    pub fn terminal_cause(&self) -> Option<NativeTerminalCause> {
        match self.value {
            NativePairingStatus::Terminal { cause, .. } => Some(cause.into()),
            NativePairingStatus::Unclaimed { terminal, .. } => terminal.map(Into::into),
            _ => None,
        }
    }
    /// Original Unclaimed attempt outcome, without inferring a terminal cause.
    pub fn attempt_state(&self) -> Option<NativeAttemptState> {
        let NativePairingStatus::Unclaimed { outcome, .. } = self.value else {
            return None;
        };
        Some(match outcome {
            AttemptOutcome::Pending => NativeAttemptState::Pending,
            AttemptOutcome::Failed(_) => NativeAttemptState::Failed,
            AttemptOutcome::Claimed => NativeAttemptState::Claimed,
            AttemptOutcome::Superseded => NativeAttemptState::Superseded,
        })
    }
    /// Original typed failed-attempt payload.
    pub fn attempt_failure(&self) -> Option<NativeAttemptFailure> {
        match self.value {
            NativePairingStatus::Unclaimed {
                outcome: AttemptOutcome::Failed(value),
                ..
            } => Some(value.into()),
            _ => None,
        }
    }
}
/// Existing public consent classes, without manufacturing an enrollment grant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeConsentClass {
    /// One owner's reading device.
    DeviceRead,
    /// A peer gateway's reading identity.
    PeerRead,
    /// A peer environment-serving identity.
    PeerEnvironment,
    /// Explicit first-party owner phone consent.
    OwnerPhoneReadWrite,
}
impl From<ConsentClass> for NativeConsentClass {
    fn from(value: ConsentClass) -> Self {
        match value {
            ConsentClass::DeviceRead => Self::DeviceRead,
            ConsentClass::PeerRead => Self::PeerRead,
            ConsentClass::PeerEnvironment => Self::PeerEnvironment,
            ConsentClass::OwnerPhoneReadWrite => Self::OwnerPhoneReadWrite,
        }
    }
}
/// Immutable public correlation from the original native receipt, not private state.
#[derive(uniffi::Object)]
pub struct NativePublicIntent {
    pub(crate) value: PublicIntent,
}
#[uniffi::export]
impl NativePublicIntent {
    /// Exact public invitation bytes, with the owner's fixed identity width.
    pub fn invitation(&self) -> Vec<u8> {
        self.value.invitation().bytes().to_vec()
    }
    /// Exact admitted attempt bytes; retries may select another attempt.
    pub fn attempt(&self) -> Vec<u8> {
        self.value.attempt().bytes().to_vec()
    }
    /// Exact immutable public consent bytes.
    pub fn consent(&self) -> Vec<u8> {
        self.value.consent().bytes().to_vec()
    }
    /// Original immutable consent generation, without integer narrowing.
    pub fn generation(&self) -> u64 {
        self.value.generation()
    }
    /// Original exclusive invitation expiry in Unix milliseconds.
    pub fn expiry_ms(&self) -> u64 {
        self.value.expiry_ms()
    }
    /// Auth-owned wire class name for receipt/status correlation, delegated to
    /// [`ConsentClass::as_str`] without foreign spelling or normalization policy.
    pub fn consent_class_name(&self) -> String {
        self.value.class().as_str().to_owned()
    }
    /// Original owner-selected public consent class.
    pub fn consent_class(&self) -> NativeConsentClass {
        self.value.class().into()
    }
}
/// Immutable original/retried receipt pair from one completed retry operation.
#[derive(uniffi::Object)]
pub struct NativeRetry {
    value: NativeRetryOutcome,
}
#[uniffi::export]
impl NativeRetry {
    /// Original NOT-CLAIMED receipt, returned only when the whole retry succeeds.
    pub fn original(&self) -> Arc<NativeEnrollmentReceipt> {
        Arc::new(NativeEnrollmentReceipt {
            value: self.value.original().clone(),
        })
    }
    /// New same-enrollment attempt's historical receipt.
    pub fn retried(&self) -> Arc<NativeEnrollmentReceipt> {
        Arc::new(NativeEnrollmentReceipt {
            value: self.value.retried().clone(),
        })
    }
}
/// Original durable receiver purge; historical lookup does not repeat deletion.
#[derive(uniffi::Object)]
pub struct NativePurge {
    pub(crate) value: RetainedPurge,
}
#[uniffi::export]
impl NativePurge {
    /// Receiver originally purged and fenced.
    pub fn receiver(&self) -> String {
        self.value.receiver().as_str().to_owned()
    }
    /// Original transcript count.
    pub fn transcripts(&self) -> u64 {
        self.value.transcripts()
    }
    /// Original physical record count.
    pub fn records(&self) -> u64 {
        self.value.records()
    }
    /// Original catalogue entry count.
    pub fn catalogue_entries(&self) -> u64 {
        self.value.catalogue_entries()
    }
    /// Original injected-clock observation, not the time of this getter call.
    pub fn observed_at_ms(&self) -> u64 {
        self.value.observed_at_ms()
    }
}
/// Immutable compound pinned status, preserving outer completion uncertainty.
#[derive(Debug, uniffi::Object)]
pub struct NativeStatus {
    pub(crate) value: SessionStatus,
}
#[uniffi::export]
impl NativeStatus {
    /// Whether an admitted physical completion captured its purge effect.
    pub fn completion_captured(&self) -> bool {
        self.value.completion_captured()
    }
    /// Original successful pinned receipt; failed private removal does not invent one.
    pub fn receipt(&self) -> Option<Arc<NativeEnrollmentReceipt>> {
        self.value.observation().map(|value| {
            Arc::new(NativeEnrollmentReceipt {
                value: value.clone(),
            })
        })
    }

    /// Original outer or completed enrollment failure, without discarding purge.
    pub fn failure(&self) -> Option<Arc<NativeEnrollmentFailure>> {
        use nessa_client_core::pairing::OwnerPhoneRecoveryFailure;
        let value = match &self.value {
            SessionStatus::ReadOnly(Err(value)) => Some(*value),
            SessionStatus::ReadOnly(Ok(value)) => value.enrollment().as_ref().err().copied(),
            SessionStatus::Owner(Err(OwnerPhoneRecoveryFailure::Native(value))) => Some(*value),
            SessionStatus::Owner(Ok(value)) => match value.recovery() {
                Err(OwnerPhoneRecoveryFailure::Native(value)) => Some(*value),
                _ => None,
            },
            _ => None,
        };
        value.map(|value| Arc::new(NativeEnrollmentFailure { value }))
    }
    /// Complete owner-specific observations and original independent local failures.
    /// Read-only scopes have no owner aggregate; ordinary failure() cannot flatten it.
    pub fn owner(&self) -> Option<Arc<NativeOwnerStatus>> {
        match &self.value {
            SessionStatus::Owner(value) => Some(Arc::new(NativeOwnerStatus {
                value: value.clone(),
            })),
            SessionStatus::ReadOnly(_) => None,
        }
    }
    /// Whether this captured physical completion attempted a purge. False with
    /// no captured completion does not establish no physical cleanup.
    pub fn purge_attempted(&self) -> bool {
        self.value.purge().is_some()
    }
    /// Original successful purge, independent of enrollment-removal acknowledgement.
    pub fn purge(&self) -> Option<Arc<NativePurge>> {
        self.value.purge()?.as_ref().ok().map(|value| {
            Arc::new(NativePurge {
                value: value.clone(),
            })
        })
    }
    /// Original purge refusal that kept the private enrollment record.
    pub fn purge_failure(&self) -> Option<NativeReadFailure> {
        self.value.purge()?.as_ref().err().copied().map(Into::into)
    }
}
/// One immutable retained read outcome, including withdrawals on failure.
#[derive(uniffi::Object)]
pub struct NativeRead {
    value: SessionRead,
}
#[uniffi::export]
impl NativeRead {
    /// Original failure, absent when the bounded read completed a report.
    pub fn failure(&self) -> Option<NativeReadFailure> {
        self.value.result().as_ref().err().copied().map(Into::into)
    }
    /// Whether actual records/catalogue/withdrawals moved, if a report exists.
    pub fn moved(&self) -> Option<bool> {
        self.value.result().as_ref().ok().map(|value| value.moved)
    }
    /// Whether that bounded pass completed, separately from progress having moved.
    pub fn complete(&self) -> Option<bool> {
        self.value
            .result()
            .as_ref()
            .ok()
            .map(|value| value.complete)
    }
    /// Core's conversation count; checked conversion preserves the integer value.
    pub fn conversations(&self) -> Option<u64> {
        self.value
            .result()
            .as_ref()
            .ok()
            .and_then(|value| u64::try_from(value.conversations).ok())
    }
    /// Actual withdrawal identities from the same admitted read.
    pub fn withdrawn(&self) -> Vec<String> {
        self.value.withdrawn().to_vec()
    }
}
/// Initial status, selection, read and optional refusal recheck stay one owned outcome.
#[derive(uniffi::Object)]
pub struct NativeRefresh {
    value: SessionRefresh,
}
#[uniffi::export]
impl NativeRefresh {
    /// Original initial status and purge.
    pub fn initial(&self) -> Arc<NativeStatus> {
        Arc::new(NativeStatus {
            value: self.value.initial().clone(),
        })
    }
    /// Exact current binding refusal before product/cache selection.
    pub fn selection_failure(&self) -> Option<Arc<NativeEnrollmentFailure>> {
        self.value
            .selection_failure()
            .map(|value| Arc::new(NativeEnrollmentFailure { value }))
    }
    /// Original read report, absent if no reader was selected.
    pub fn read(&self) -> Option<Arc<NativeRead>> {
        self.value.read().map(|value| {
            Arc::new(NativeRead {
                value: value.clone(),
            })
        })
    }
    /// Original refusal recheck, independently of its initial status/purge.
    pub fn recheck(&self) -> Option<Arc<NativeStatus>> {
        self.value.recheck().map(|value| {
            Arc::new(NativeStatus {
                value: value.clone(),
            })
        })
    }
}
/// One admitted `conversation.subscribe` read. The view replaces what the
/// device shows; it does not advance a physical checkpoint.
#[derive(uniffi::Object)]
pub struct NativeFollow {
    value: SessionFollow,
}
#[uniffi::export]
impl NativeFollow {
    /// Original initial status and purge.
    pub fn initial(&self) -> Arc<NativeStatus> {
        Arc::new(NativeStatus {
            value: self.value.initial().clone(),
        })
    }
    /// Exact current binding refusal before product selection.
    pub fn selection_failure(&self) -> Option<Arc<NativeEnrollmentFailure>> {
        self.value
            .selection_failure()
            .map(|value| Arc::new(NativeEnrollmentFailure { value }))
    }
    /// Product view JSON when the first frame was a view.
    pub fn view_json(&self) -> Option<String> {
        self.value
            .followed()
            .and_then(|result| result.as_ref().ok())
            .map(|value| value.view_json().to_owned())
    }
    /// Cursor incarnation on that view.
    pub fn incarnation(&self) -> Option<String> {
        self.value
            .followed()
            .and_then(|result| result.as_ref().ok())
            .map(|value| value.incarnation().to_owned())
    }
    /// Cursor position on that view. A later follow passes it back as `after`.
    pub fn position(&self) -> Option<String> {
        self.value
            .followed()
            .and_then(|result| result.as_ref().ok())
            .map(|value| value.position().to_owned())
    }
    /// Why no view was delivered, when a reader was selected.
    /// [`NativeReadFailure::CursorAhead`] means `after` is past the gateway
    /// head: follow again with no cursor.
    /// [`NativeReadFailure::SubscriptionCapacity`] means this connection's
    /// subscription limit is full: stop following something first. Neither is an ungranted conversation,
    /// and neither asks pinned status.
    pub fn failure(&self) -> Option<NativeReadFailure> {
        self.value
            .followed()
            .and_then(|result| result.as_ref().err().copied())
            .map(Into::into)
    }
    /// Authority-refusal recheck. A conversation this device was not granted does not ask.
    pub fn recheck(&self) -> Option<Arc<NativeStatus>> {
        self.value.recheck().map(|value| {
            Arc::new(NativeStatus {
                value: value.clone(),
            })
        })
    }
}
/// Immutable saved entry cursor. Restoration selects order, not read authority.
#[derive(uniffi::Object)]
pub struct NativeEntryKey {
    value: EntryKey,
}
#[uniffi::export]
impl NativeEntryKey {
    /// Restore one saved ordering selector through the existing Id owner.
    /// Current issued receiver/scope selection still belongs to the native session.
    #[uniffi::constructor]
    pub fn new(creation: u64, id: String) -> Result<Self, NativeCallError> {
        let id = Id::new(id).map_err(|_| NativeCallError::InvalidArgument)?;
        Ok(Self {
            value: EntryKey { creation, id },
        })
    }
    /// Original creation revision.
    pub fn creation(&self) -> u64 {
        self.value.creation
    }
    /// Original stable conversation identity.
    pub fn id(&self) -> String {
        self.value.id.as_str().to_owned()
    }
}
/// One immutable core catalogue row including tombstone and validated metadata.
#[derive(uniffi::Object)]
pub struct NativeCatalogueEntry {
    value: RetainedCatalogueEntry,
}
#[uniffi::export]
impl NativeCatalogueEntry {
    /// Physical key from the same manifest.
    pub fn key(&self) -> Arc<NativeEntryKey> {
        Arc::new(NativeEntryKey {
            value: self.value.manifest().key.clone(),
        })
    }
    /// Original committed revision.
    pub fn revision(&self) -> u64 {
        self.value.manifest().revision
    }
    /// Retained deletion descriptor, independently of metadata absence.
    pub fn deleted(&self) -> bool {
        self.value.manifest().deleted
    }
    /// Validated agent identifier, absent when metadata is unavailable.
    pub fn agent(&self) -> Option<String> {
        self.value
            .metadata()?
            .agent()
            .map(|value| value.name().to_owned())
    }
    /// Validated selected model.
    pub fn model(&self) -> Option<String> {
        self.value
            .metadata()
            .map(|value| value.model().as_str().to_owned())
    }
    /// Validated title from the saved summary.
    pub fn title(&self) -> Option<String> {
        self.value
            .metadata()?
            .summary()?
            .title()
            .map(|value| value.as_str().to_owned())
    }
    /// Validated preview from the saved summary.
    pub fn preview(&self) -> Option<String> {
        self.value
            .metadata()?
            .summary()?
            .preview()
            .map(|value| value.as_str().to_owned())
    }
    /// Actual saved creation timestamp.
    pub fn created_at_ms(&self) -> Option<u64> {
        self.value.metadata().map(|value| value.created_at_ms())
    }
    /// Actual saved summary update timestamp.
    pub fn updated_at_ms(&self) -> Option<u64> {
        self.value
            .metadata()?
            .summary()
            .map(|value| value.updated_at_ms())
    }
    /// Saved archive attribute, independent of deletion/read authority.
    pub fn archived(&self) -> Option<bool> {
        self.value
            .metadata()?
            .summary()
            .map(|value| value.archived())
    }
}
/// One immutable catalogue page from one physical database snapshot.
#[derive(uniffi::Object)]
pub struct NativeCataloguePage {
    value: RetainedCataloguePage,
}
#[uniffi::export]
impl NativeCataloguePage {
    /// Actual saved scope, unrelated to hostname metadata.
    pub fn scope(&self) -> Arc<NativeScope> {
        Arc::new(NativeScope {
            value: self.value.progress().scope.clone(),
        })
    }
    /// Original durable catalogue generation.
    pub fn generation(&self) -> u64 {
        self.value.progress().generation
    }
    /// Original fully completed revision; an active pass can be partial.
    pub fn completed(&self) -> u64 {
        self.value.progress().completed
    }
    /// Captured boundary when a pass remains active.
    pub fn active_boundary(&self) -> Option<u64> {
        self.value
            .progress()
            .active
            .as_ref()
            .map(|value| value.boundary)
    }
    /// Actual retained continuation key of an active pass.
    pub fn active_cursor(&self) -> Option<Arc<NativeEntryKey>> {
        self.value
            .progress()
            .active
            .as_ref()?
            .cursor
            .as_ref()
            .map(|value| {
                Arc::new(NativeEntryKey {
                    value: value.clone(),
                })
            })
    }
    /// Owned rows, including deletion descriptors.
    pub fn entries(&self) -> Vec<Arc<NativeCatalogueEntry>> {
        self.value
            .entries()
            .iter()
            .cloned()
            .map(|value| Arc::new(NativeCatalogueEntry { value }))
            .collect()
    }
    /// Original next page cursor from this database snapshot.
    pub fn next(&self) -> Option<Arc<NativeEntryKey>> {
        self.value
            .next()
            .cloned()
            .map(|value| Arc::new(NativeEntryKey { value }))
    }
}
/// Locally saved issued selectors. This historical pair is not read authority.
#[derive(uniffi::Object)]
pub struct NativeLocalIssuedEnrollment {
    value: LocalIssuedEnrollment,
}
#[uniffi::export]
impl NativeLocalIssuedEnrollment {
    /// Original saved issued credential identity.
    pub fn credential(&self) -> String {
        self.value.credential().as_str().to_owned()
    }
    /// Original saved receiver associated with that credential.
    pub fn receiver(&self) -> String {
        self.value.receiver().as_str().to_owned()
    }
}
/// Locally saved public correlation, to be checked against authenticated owner status.
#[derive(uniffi::Object)]
pub struct NativeLocalEnrollment {
    value: LocalEnrollment,
}
#[uniffi::export]
impl NativeLocalEnrollment {
    /// Saved invitation/attempt/consent metadata, not an approval claim.
    pub fn public_intent(&self) -> Arc<NativePublicIntent> {
        Arc::new(NativePublicIntent {
            value: self.value.intent(),
        })
    }
    /// Public DeviceKey bytes in the existing pairing owner's representation.
    pub fn device_key(&self) -> Vec<u8> {
        self.value.device_key().bytes().to_vec()
    }
    /// Saved gateway SPKI authenticated by enrollment.
    pub fn gateway_pin(&self) -> Vec<u8> {
        self.value.gateway_pin().to_vec()
    }
    /// Issued selectors together, absent while the private record is pending.
    pub fn issued(&self) -> Option<Arc<NativeLocalIssuedEnrollment>> {
        self.value.issued().map(|value| {
            Arc::new(NativeLocalIssuedEnrollment {
                value: value.clone(),
            })
        })
    }
}

/// Immutable confirmation from the actual native cleanup owner.
#[derive(uniffi::Object)]
pub struct NativeCloseObservation {
    value: SessionCloseObservation,
}
#[uniffi::export]
impl NativeCloseObservation {
    /// Physical private-scope release, independent of a retained typed failure.
    pub fn released(&self) -> bool {
        self.value.released()
    }
    /// Original cleanup outcome; historical Product failure can coexist with release.
    pub fn failure(&self) -> Option<Arc<NativeSessionFailure>> {
        self.value
            .outcome()
            .as_ref()
            .err()
            .cloned()
            .map(|value| Arc::new(NativeSessionFailure { value }))
    }
}
/// A phone publication stamp. Holding one does not bind the lease.
#[derive(uniffi::Object)]
pub struct NativePhoneStamp {
    value: PhoneStamp,
}
#[uniffi::export]
impl NativePhoneStamp {
    /// Published lease id. Accept also requires the identity the minting session
    /// holds, which this method does not return. A later session mints another id.
    pub fn lease(&self) -> String {
        self.value.lease().to_string()
    }
    /// Generation assigned when this receiver was bound.
    pub fn generation(&self) -> u64 {
        self.value.generation()
    }
    /// Receiver this stamp names.
    pub fn receiver(&self) -> String {
        self.value.receiver().to_owned()
    }
}
impl NativePhoneStamp {
    fn stamp(&self) -> PhoneStamp {
        self.value.clone()
    }
}

/// Stored catalogue identity. Absence is not a tombstone.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeCataloguePresence {
    /// A non-deleted row is stored.
    Live,
    /// A deletion fence or receiver purge is stored.
    Tombstone,
    /// No row and no fence.
    Absent,
}
impl From<CataloguePresence> for NativeCataloguePresence {
    fn from(value: CataloguePresence) -> Self {
        match value {
            CataloguePresence::Live => Self::Live,
            CataloguePresence::Tombstone => Self::Tombstone,
            CataloguePresence::Absent => Self::Absent,
        }
    }
}

/// Local file step from the existing held-file fetch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeArtifactFileStep {
    /// Existing same-named fetch step.
    Open,
    /// Existing same-named fetch step.
    Locked,
    /// Existing same-named fetch step.
    Read,
    /// Existing same-named fetch step.
    Restart,
    /// Existing same-named fetch step.
    Write,
    /// Existing same-named fetch step.
    Sync,
    /// Existing same-named fetch step.
    Link,
    /// Existing same-named fetch step.
    SyncDirectory,
    /// Existing same-named fetch step.
    Remove,
    /// Existing same-named fetch step.
    Name,
    /// Existing same-named fetch step.
    Changed,
}
impl From<ArtifactFileStep> for NativeArtifactFileStep {
    fn from(value: ArtifactFileStep) -> Self {
        match value {
            ArtifactFileStep::Open => Self::Open,
            ArtifactFileStep::Locked => Self::Locked,
            ArtifactFileStep::Read => Self::Read,
            ArtifactFileStep::Restart => Self::Restart,
            ArtifactFileStep::Write => Self::Write,
            ArtifactFileStep::Sync => Self::Sync,
            ArtifactFileStep::Link => Self::Link,
            ArtifactFileStep::SyncDirectory => Self::SyncDirectory,
            ArtifactFileStep::Remove => Self::Remove,
            ArtifactFileStep::Name => Self::Name,
            ArtifactFileStep::Changed => Self::Changed,
        }
    }
}

/// One held-file fetch. Freshness getters are empty when the fetch faulted.
#[derive(uniffi::Object)]
pub struct NativeArtifactFetch {
    value: ArtifactFetch,
}
#[uniffi::export]
impl NativeArtifactFetch {
    /// Monotonic milliseconds spent in this fetch. Not model-generation latency.
    pub fn elapsed_ms(&self) -> u64 {
        self.value.elapsed_ms()
    }
    /// Bytes already on disk when nothing was asked. Empty unless this is a local match.
    pub fn local_match_bytes(&self) -> Option<u64> {
        match self.value.outcome() {
            Ok(ArtifactFreshness::LocalMatch { bytes }) => Some(bytes),
            _ => None,
        }
    }
    /// Ranges the gateway answered. Empty unless the gateway confirmed the digest.
    pub fn gateway_ranges(&self) -> Option<u64> {
        match self.value.outcome() {
            Ok(ArtifactFreshness::GatewayConfirmed { ranges, .. }) => Some(ranges),
            _ => None,
        }
    }
    /// Bytes resumed from an earlier part. Empty unless the gateway confirmed the digest.
    pub fn gateway_resumed_from(&self) -> Option<u64> {
        match self.value.outcome() {
            Ok(ArtifactFreshness::GatewayConfirmed { resumed_from, .. }) => Some(resumed_from),
            _ => None,
        }
    }
    /// Why the file was not kept. Empty when it was kept.
    pub fn fault(&self) -> Option<NativeArtifactFaultKind> {
        let kind = self.value.outcome().err()?.kind();
        Some(match kind {
            ArtifactFaultKind::Gateway(_) => NativeArtifactFaultKind::Gateway,
            ArtifactFaultKind::WrongRange => NativeArtifactFaultKind::WrongRange,
            ArtifactFaultKind::Integrity { .. } => NativeArtifactFaultKind::Integrity,
            ArtifactFaultKind::OutputExists => NativeArtifactFaultKind::OutputExists,
            ArtifactFaultKind::File(_) => NativeArtifactFaultKind::File,
        })
    }
    /// Gateway refusal, when `fault` is gateway.
    pub fn gateway_failure(&self) -> Option<NativeReadFailure> {
        match self.value.outcome().err()?.kind() {
            ArtifactFaultKind::Gateway(failure) => Some(failure.into()),
            _ => None,
        }
    }
    /// Local file step, when `fault` is file.
    pub fn file_step(&self) -> Option<NativeArtifactFileStep> {
        match self.value.outcome().err()?.kind() {
            ArtifactFaultKind::File(step) => Some(step.into()),
            _ => None,
        }
    }
    /// What happened to a part that was not the named file. Empty unless
    /// `fault` is integrity.
    pub fn part_cleanup(&self) -> Option<NativeArtifactPartCleanup> {
        match self.value.outcome().err()?.kind() {
            ArtifactFaultKind::Integrity { cleanup } => Some(match cleanup {
                ArtifactPartCleanup::Removed => NativeArtifactPartCleanup::Removed,
                ArtifactPartCleanup::Emptied { .. } => NativeArtifactPartCleanup::Emptied,
                ArtifactPartCleanup::Left { .. } => NativeArtifactPartCleanup::Left,
            }),
            _ => None,
        }
    }
    /// The removal step that failed while cleaning an integrity mismatch.
    /// Empty when the part was removed, or the fault is not integrity.
    pub fn cleanup_remove(&self) -> Option<NativeArtifactFileStep> {
        match self.value.outcome().err()?.kind() {
            ArtifactFaultKind::Integrity {
                cleanup: ArtifactPartCleanup::Emptied { remove },
            }
            | ArtifactFaultKind::Integrity {
                cleanup: ArtifactPartCleanup::Left { remove, .. },
            } => Some(remove.into()),
            _ => None,
        }
    }
    /// The emptying step that failed after removal failed. Empty unless the
    /// part was left in place.
    pub fn cleanup_restart(&self) -> Option<NativeArtifactFileStep> {
        match self.value.outcome().err()?.kind() {
            ArtifactFaultKind::Integrity {
                cleanup: ArtifactPartCleanup::Left { restart, .. },
            } => Some(restart.into()),
            _ => None,
        }
    }
    /// Part progress on a fault. Empty when the file was kept.
    pub fn reached(&self) -> Option<u64> {
        self.value.outcome().err().map(|fault| fault.reached())
    }
    /// Bytes already in the part when this run began. Empty when the file was kept.
    pub fn resumed_from(&self) -> Option<u64> {
        self.value.outcome().err().map(|fault| fault.resumed_from())
    }
}

/// The file a phone names and the output path it should land in.
/// Holding one does not read or dial.
#[derive(uniffi::Record)]
pub struct NativeArtifactRequest {
    /// Conversation that holds the file.
    pub conversation: String,
    /// Canonical `sha256:` digest.
    pub digest: String,
    /// Media type the file is held as.
    pub media_type: String,
    /// Size the digest names.
    pub size: u64,
    /// Path where a matching file is kept.
    pub output: String,
}

/// What became of a part that was not the named file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeArtifactPartCleanup {
    /// The part was removed.
    Removed,
    /// Removal failed and the part was emptied.
    Emptied,
    /// Removal and emptying both failed. The part is still there.
    Left,
}

/// Why a held file was not kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum NativeArtifactFaultKind {
    /// The gateway or the read stop refused a range.
    Gateway,
    /// An answer was not the range asked.
    WrongRange,
    /// The part on disk was not the named digest.
    Integrity,
    /// Something else is already at the output.
    OutputExists,
    /// A local file step failed.
    File,
}

/// The generated native session handle. Blocking calls run on the caller's
/// dedicated worker; close must drain there before releasing/replacing scope.
#[derive(uniffi::Object)]
pub struct NativeDeviceSession {
    value: NativeSession,
}
#[uniffi::export]
impl NativeDeviceSession {
    /// Compose numeric address and an existing private directory. No port offset,
    /// hostname-derived origin, socket connect or enrollment is performed.
    #[uniffi::constructor]
    pub fn open(root: String, directory: String, address: String) -> Result<Self, NativeCallError> {
        let address: SocketAddr = address
            .parse()
            .map_err(|_| NativeCallError::InvalidArgument)?;
        NativeSession::open(Path::new(&root), Path::new(&directory), address)
            .map(|value| Self { value })
            .map_err(Into::into)
    }
    /// Pair using the existing code decoder and physical enrollment worker.
    pub fn enroll(&self, code: String) -> Result<Arc<NativeEnrollmentReceipt>, NativeCallError> {
        // Erase the Rust-owned argument copy. Swift may retain its own input.
        let input = Zeroizing::new(code);
        let code = ManualCode::parse(input.as_bytes()).map_err(|value| {
            NativeCallError::from(SessionFailure::Enrollment(NativeClientError::Crypto(value)))
        })?;
        self.value
            .enroll(code)
            .map(|value| Arc::new(NativeEnrollmentReceipt { value }))
            .map_err(Into::into)
    }
    /// Existing exact-operation retry; errors do not claim an original receipt.
    pub fn retry(&self, code: String) -> Result<Arc<NativeRetry>, NativeCallError> {
        let input = Zeroizing::new(code);
        let code = ManualCode::parse(input.as_bytes()).map_err(|value| {
            NativeCallError::from(SessionFailure::Enrollment(NativeClientError::Crypto(value)))
        })?;
        self.value
            .retry(code)
            .map(|value| Arc::new(NativeRetry { value }))
            .map_err(Into::into)
    }
    /// Fresh pinned status retains operation purge and physical worker uncertainty.
    pub fn status(&self) -> Result<Arc<NativeStatus>, NativeCallError> {
        self.value
            .status()
            .map(|value| Arc::new(NativeStatus { value }))
            .map_err(Into::into)
    }
    /// Capture current local public correlation under the same physical admission.
    pub fn local_enrollment(&self) -> Result<Option<Arc<NativeLocalEnrollment>>, NativeCallError> {
        self.value
            .local_enrollment()
            .map(|value| value.map(|value| Arc::new(NativeLocalEnrollment { value })))
            .map_err(Into::into)
    }
    /// Run status, binding, one `conversation.subscribe`, and an authority recheck.
    /// `after_incarnation` and `after_position` are both absent, or both a
    /// published `ConversationViewCursor`: incarnation within its character
    /// bound, position a decimal `u64`. Anything else is
    /// [`NativeCallError::InvalidArgument`] before a follow starts. The first
    /// view is returned and the subscription is closed. Checkpoints stay on
    /// [`Self::refresh`].
    pub fn follow(
        &self,
        conversation: String,
        after_incarnation: Option<String>,
        after_position: Option<String>,
        stop: Arc<NativeReadStop>,
    ) -> Result<Arc<NativeFollow>, NativeCallError> {
        let conversation =
            ConversationId::new(&conversation).map_err(|_| NativeCallError::InvalidArgument)?;
        let after = view_cursor(after_incarnation, after_position)?;
        self.value
            .follow(&conversation, after, stop.value.clone())
            .map(|value| Arc::new(NativeFollow { value }))
            .map_err(Into::into)
    }
    /// Run status, binding, bounded checkpoint read and refusal recheck under one admission.
    pub fn refresh(
        &self,
        stop: Arc<NativeReadStop>,
    ) -> Result<Arc<NativeRefresh>, NativeCallError> {
        self.value
            .refresh(stop.value.clone())
            .map(|value| Arc::new(NativeRefresh { value }))
            .map_err(Into::into)
    }
    /// Bind the publication lease to the saved issued receiver. Nothing is dialed.
    pub fn bind_saved_receiver(&self) -> Result<Arc<NativePhoneStamp>, NativeCallError> {
        self.value
            .bind_saved_receiver()
            .map(|value| Arc::new(NativePhoneStamp { value }))
            .map_err(Into::into)
    }
    /// The stamp currently admitted.
    pub fn publication(&self) -> Result<Arc<NativePhoneStamp>, NativeCallError> {
        self.value
            .publication()
            .map(|value| Arc::new(NativePhoneStamp { value }))
            .map_err(Into::into)
    }
    /// Saved transcript for this stamp. A replaced stamp reads nothing.
    pub fn published_transcript(
        &self,
        stamp: Arc<NativePhoneStamp>,
        conversation: String,
        revision: String,
    ) -> Result<Arc<NativeTranscript>, NativeCallError> {
        let conversation =
            ConversationId::new(&conversation).map_err(|_| NativeCallError::InvalidArgument)?;
        let revision = Uuid::parse_str(&revision).map_err(|_| NativeCallError::InvalidArgument)?;
        self.value
            .published_transcript(&stamp.stamp(), &conversation, revision)
            .map(|value| Arc::new(NativeTranscript { value }))
            .map_err(Into::into)
    }
    /// Saved catalogue page for this stamp. A replaced stamp reads nothing.
    pub fn published_catalogue_page(
        &self,
        stamp: Arc<NativePhoneStamp>,
        after: Option<Arc<NativeEntryKey>>,
        maximum: u32,
    ) -> Result<Option<Arc<NativeCataloguePage>>, NativeCallError> {
        let maximum = usize::try_from(maximum).map_err(|_| NativeCallError::InvalidArgument)?;
        self.value
            .published_catalogue_page(
                &stamp.stamp(),
                after.as_ref().map(|value| &value.value),
                maximum,
            )
            .map(|value| value.map(|value| Arc::new(NativeCataloguePage { value })))
            .map_err(Into::into)
    }
    /// Stored presence of one conversation. Absence is not a tombstone.
    /// `conversation` must be a canonical conversation id.
    pub fn catalogue_presence(
        &self,
        stamp: Arc<NativePhoneStamp>,
        conversation: String,
    ) -> Result<NativeCataloguePresence, NativeCallError> {
        let _parsed =
            ConversationId::new(&conversation).map_err(|_| NativeCallError::InvalidArgument)?;
        self.value
            .catalogue_presence(&stamp.stamp(), &conversation)
            .map(NativeCataloguePresence::from)
            .map_err(Into::into)
    }
    /// Fetch one held file. A replaced stamp does not dial. A local digest match does not dial.
    pub fn fetch_artifact(
        &self,
        stamp: Arc<NativePhoneStamp>,
        request: NativeArtifactRequest,
        stop: Arc<NativeReadStop>,
    ) -> Result<Arc<NativeArtifactFetch>, NativeCallError> {
        let conversation = ConversationId::new(&request.conversation)
            .map_err(|_| NativeCallError::InvalidArgument)?;
        if !artifact_name_accepted(&request.digest, &request.media_type, request.size) {
            return Err(NativeCallError::InvalidArgument);
        }
        let request = ArtifactRequest::new(
            &conversation,
            &request.digest,
            &request.media_type,
            request.size,
            Path::new(&request.output),
        );
        self.value
            .fetch_artifact(&stamp.stamp(), &request, stop.value.clone())
            .map(|value| Arc::new(NativeArtifactFetch { value }))
            .map_err(Into::into)
    }
    /// Historical receipt recovery can select the prior receiver after key removal.
    pub fn purge_receipt(
        &self,
        receiver: String,
    ) -> Result<Option<Arc<NativePurge>>, NativeCallError> {
        self.value
            .purge_receipt(&receiver)
            .map(|value| value.map(|value| Arc::new(NativePurge { value })))
            .map_err(Into::into)
    }
    /// Install the original saved-key product child before its worker can dial.
    /// Returned Ready/settlement/event waits run off-UI on independent waiters.
    pub fn open_product(
        &self,
        instance: String,
    ) -> Result<Arc<NativeProductSession>, NativeCallError> {
        self.value
            .open_product(instance)
            .map(|value| Arc::new(NativeProductSession { value }))
            .map_err(Into::into)
    }
    /// Initial preparation only for a newly allocated application reservation.
    /// Captures the exact original and durable key without network or issuance.
    pub fn prepare_owner_original(
        &self,
        request_id: Vec<u8>,
        discovery_json: String,
    ) -> Result<Arc<NativeOwnerPhonePreparation>, NativeOwnerPreparationCallError> {
        let reservation = reservation(request_id, discovery_json)?;
        self.value
            .prepare_owner_original(reservation)
            .map(|value| Arc::new(NativeOwnerPhonePreparation { value }))
            .map_err(Into::into)
    }
    /// Existing reservations restore only; missing history never creates another key.
    pub fn restore_owner_original(
        &self,
        request_id: Vec<u8>,
        discovery_json: String,
    ) -> Result<Arc<NativeOwnerPhonePreparation>, NativeOwnerPreparationCallError> {
        let reservation = reservation(request_id, discovery_json)?;
        self.value
            .restore_owner_original(&reservation)
            .map(|value| Arc::new(NativeOwnerPhonePreparation { value }))
            .map_err(Into::into)
    }
    /// Read the exact prepared original over pinned TLS without adoption or cache effects.
    /// Blocking work belongs off UI; canceling an observer does not release physical ownership.
    pub fn recover_owner_original(
        &self,
        expected: Arc<NativeOwnerPhonePreparation>,
    ) -> Result<Arc<NativeOwnerStatus>, NativeCallError> {
        self.value
            .recover_owner_original(Arc::new(expected.value.clone()))
            .map(|value| Arc::new(NativeOwnerStatus { value }))
            .map_err(Into::into)
    }
    /// Await actual cleanup and retain its separate physical release observation.
    /// This synchronous method belongs on the same cancellation-independent worker.
    pub fn close_observed(&self) -> Arc<NativeCloseObservation> {
        Arc::new(NativeCloseObservation {
            value: self.value.close_observed(),
        })
    }
    /// Signal closure before queueing drain; stop current core sockets.
    pub fn request_close(&self) -> Result<(), NativeCallError> {
        self.value.request_close().map_err(Into::into)
    }
    /// Await physical work and release private scope on the dedicated worker.
    pub fn close(&self) -> Result<(), NativeCallError> {
        self.value.close().map_err(Into::into)
    }
}

/// A restored or caller-supplied cursor, or none when both parts are absent.
/// [`conforming_view_cursor`] is the check: the published incarnation bound,
/// and a position `decimal_u64` accepts.
fn view_cursor(
    incarnation: Option<String>,
    position: Option<String>,
) -> Result<Option<ConversationViewCursor>, NativeCallError> {
    match (incarnation, position) {
        (None, None) => Ok(None),
        (Some(incarnation), Some(position)) => {
            let cursor = ConversationViewCursor {
                incarnation,
                position,
            };
            if conforming_view_cursor(&cursor) {
                Ok(Some(cursor))
            } else {
                Err(NativeCallError::InvalidArgument)
            }
        }
        _ => Err(NativeCallError::InvalidArgument),
    }
}

#[cfg(test)]
#[path = "../tests/session/correlation.rs"]
mod tests;
