use std::{
    fmt,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::reclamation::{ReclamationPersistenceFailure, ReclamationPersistenceStage};
use crate::agent_install::domain::{
    AgentName, ArchiveDigest, InstallAttemptError, InstallEventIdentity, InstallTransition,
    ManagedInstallation, PinnedRelease, PublicationOutcome, PublicationPreparation,
    PublicationSettlement, RuntimeArtifact,
};

/// Why an archive could not be fetched.
///
/// Typed rather than a message because the caller acts on the difference: a
/// user who is offline is told something different from one whose pin points at
/// a release that has been unpublished, and only the first is worth a retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceFailure {
    /// Nothing answered, or the transfer came apart part-way.
    Unreachable(String),
    /// Something answered and said no. Carries the status so a 404 — the pin
    /// naming a release that no longer exists — is distinguishable from a 503.
    Refused(u16),
    /// The response kept coming past the length the pin says the archive is.
    /// Not a rejection of the archive's *contents* — nothing has been hashed
    /// yet — but a refusal to keep writing bytes that already cannot be the
    /// pinned archive, rather than filling a disk first and finding out from
    /// the digest afterwards. Carries the length that was pinned.
    TooLarge(u64),
    /// The bytes arrived and this machine could not keep them: a full disk, or
    /// a file that stopped being writable part-way. Its own variant because
    /// telling somebody the download could not be reached when their disk is
    /// full sends them to look at the wrong thing.
    NotStored(String),
}

impl fmt::Display for SourceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(f, "could not reach the release archive: {detail}"),
            Self::Refused(status) => {
                write!(f, "the release archive was refused with status {status}")
            }
            Self::TooLarge(limit) => {
                write!(f, "the release archive is larger than {limit} bytes")
            }
            Self::NotStored(detail) => {
                write!(f, "could not store the release archive: {detail}")
            }
        }
    }
}

impl std::error::Error for SourceFailure {}

/// Why the machine could not hold, read, or unpack an installed runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreFailure {
    /// A directory could not be created, or a file could not be written.
    Unwritable(String),
    /// A file that should be there could not be read.
    Unreadable(String),
    /// The archive did not contain one of the files the release says it
    /// installs. A pin that is wrong about its own contents, not a machine that
    /// failed — and true of any of them, not only the program that is launched:
    /// Codex's runtime finds its ripgrep and its zsh through the directory it
    /// is installed in, so an archive missing one of those is as unusable as
    /// one missing the program itself. Named for the archive rather than for
    /// the executable because a release stopped being one file: this is raised
    /// for a missing document as readily as for the program, and a reader who
    /// went looking for a missing *executable* would be looking for the wrong
    /// thing. It sits beside [`Self::MalformedArchive`], which is an archive
    /// that could not be read at all rather than one read and found short.
    IncompleteArchive(String),
    /// The archive is not a well-formed gzip tar.
    MalformedArchive(String),
}

impl fmt::Display for StoreFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unwritable(detail) => write!(f, "could not write the agent runtime: {detail}"),
            Self::Unreadable(detail) => write!(f, "could not read the agent runtime: {detail}"),
            Self::IncompleteArchive(path) => {
                write!(f, "the release archive does not contain {path}")
            }
            Self::MalformedArchive(detail) => {
                write!(f, "the release archive could not be unpacked: {detail}")
            }
        }
    }
}

impl std::error::Error for StoreFailure {}

/// The application-level checkpoint at which durable audit acknowledgement failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditFailureStage {
    /// The private journal authority could not be initialized.
    Initialize,
    /// The stable journal lock could not be acquired.
    AcquireLock,
    /// The retained directory or stable lock no longer had its original identity.
    VerifyAuthority,
    /// Existing durable records could not be enumerated or validated.
    ReadJournal,
    /// Existing durable facts conflict with the incoming logical event.
    ReconcileRecord,
    /// The next record could not be encoded or written to its reservation.
    WriteRecord,
    /// The reserved record could not be renamed to its immutable destination.
    PublishRecord,
    /// Publication occurred, but its durability or identity could not be acknowledged.
    AcknowledgeRecord,
}

/// Logical identity of an audit record whose destination rename occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedAuditRecord {
    event: InstallEventIdentity,
    record_id: String,
    sequence: u64,
    destination: String,
}

impl PublishedAuditRecord {
    /// Describe the logical record that reached its immutable destination.
    pub fn new(
        event: InstallEventIdentity,
        record_id: String,
        sequence: u64,
        destination: String,
    ) -> Self {
        Self {
            event,
            record_id,
            sequence,
            destination,
        }
    }

    /// Return the domain-owned stable identity represented by this record.
    pub fn event(&self) -> &InstallEventIdentity {
        &self.event
    }

    /// Return the adapter-owned identity written into the record.
    pub fn record_id(&self) -> &str {
        &self.record_id
    }

    /// Return the durable sequence written into the record and its filename.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Return the immutable single-component destination name.
    pub fn destination(&self) -> &str {
        &self.destination
    }
}

/// Whether durable acknowledgement created a record or re-acknowledged the
/// already-published record for the same logical event and facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAcknowledgement {
    /// A new immutable physical record was durably acknowledged.
    Recorded,
    /// The original physical record for identical semantic facts was re-synced.
    Replayed,
}

/// Record evidence attached to an audit failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditRecordEvidence {
    /// This call renamed the incoming event's record into place.
    IncomingPublished(PublishedAuditRecord),
    /// An existing identical record failed while being re-opened or re-synced.
    ExistingReplay(PublishedAuditRecord),
    /// A different durable record occupied the same logical event identity.
    ExistingConflict(PublishedAuditRecord),
}

/// The durable audit sink did not acknowledge install evidence.
///
/// A destination rename, the primary failure, and reservation cleanup are
/// independent facts. Keeping them separate prevents a caller from blindly
/// retrying a sequence that may already exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditFailure {
    stage: AuditFailureStage,
    context: Box<AuditFailureContext>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditFailureContext {
    detail: String,
    record: Option<AuditRecordEvidence>,
    semantic_conflict: Option<InstallAttemptError>,
    cleanup: Option<String>,
}

impl AuditFailure {
    /// Build a failure while preserving any publication and cleanup evidence.
    pub fn new(
        stage: AuditFailureStage,
        detail: String,
        record: Option<AuditRecordEvidence>,
        cleanup: Option<String>,
    ) -> Self {
        Self {
            stage,
            context: Box::new(AuditFailureContext {
                detail,
                record,
                semantic_conflict: None,
                cleanup,
            }),
        }
    }

    /// Build a typed semantic conflict without reducing its cause to text.
    pub fn semantic_conflict(
        stage: AuditFailureStage,
        detail: String,
        record: Option<AuditRecordEvidence>,
        conflict: InstallAttemptError,
    ) -> Self {
        Self {
            stage,
            context: Box::new(AuditFailureContext {
                detail,
                record,
                semantic_conflict: Some(conflict),
                cleanup: None,
            }),
        }
    }

    /// Return the application checkpoint that failed.
    pub fn stage(&self) -> AuditFailureStage {
        self.stage
    }

    /// Return the primary failure detail.
    pub fn detail(&self) -> &str {
        &self.context.detail
    }

    /// Return publication or conflict evidence for a physical record, if known.
    pub fn record(&self) -> Option<&AuditRecordEvidence> {
        self.context.record.as_ref()
    }

    /// Return the domain admission error when semantic reconciliation failed.
    pub fn semantic_conflict_kind(&self) -> Option<InstallAttemptError> {
        self.context.semantic_conflict
    }

    /// Return an independent reservation-cleanup failure.
    pub fn cleanup(&self) -> Option<&str> {
        self.context.cleanup.as_deref()
    }
}

impl fmt::Display for AuditFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "audit failed at {:?}: {}",
            self.stage, self.context.detail
        )?;
        if let Some(cleanup) = &self.context.cleanup {
            write!(formatter, "; reservation cleanup also failed: {cleanup}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AuditFailure {}

/// Which state change a successful publication performed while holding the
/// agent's publication lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationChange {
    /// Another install had already published this exact artifact.
    Reused,
    /// No valid runtime record existed before this artifact was published.
    Installed,
    /// This artifact replaced the runtime named by the prior valid record.
    Replaced(RuntimeArtifact),
}

/// Physical result of one admitted superseded-runtime removal attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeReclamationEffect {
    /// Every retained file was removed and the containing directory was synced.
    Removed,
    /// None of the retained files remained when recovery observed them.
    AlreadyAbsent,
    /// The target became the current artifact again and must be preserved.
    DeferredCurrent,
    /// A live launch authority, process generation, or unresolved marker still owns the target.
    DeferredInUse,
    /// Observation found retained files after an interrupted removal admission.
    StillPresent,
    /// Removal failed before durability could be established.
    Failed(StoreFailure),
    /// Files were removed, but the directory update could not be acknowledged as durable.
    SyncUncertain(StoreFailure),
}

/// Keeps the store's per-agent publication authority through the immediate
/// audit attempt. An error return drops the lease; a later bounded redelivery
/// can therefore be observed after another install. Implementations normally
/// own the publication lock handle.
pub trait PublicationLease: Send {
    fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure>;

    fn retain_reclamation(
        &mut self,
        installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure>;

    /// Attempt one bounded removal while retaining the runtime publication lock.
    fn remove_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect;

    fn observe_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect;
}

impl PublicationLease for () {
    fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::Read,
            "runtime reclamation is unavailable from this publication lease".into(),
        ))
    }

    fn retain_reclamation(
        &mut self,
        _installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        Err(ReclamationPersistenceFailure::new(
            stage,
            "runtime reclamation is unavailable from this publication lease".into(),
        ))
    }

    fn remove_superseded(
        &mut self,
        _agent: &AgentName,
        _current: &RuntimeArtifact,
        _superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(
            "runtime reclamation is unavailable from this publication lease".into(),
        ))
    }

    fn observe_superseded(
        &mut self,
        _agent: &AgentName,
        _current: &RuntimeArtifact,
        _superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(
            "runtime reclamation is unavailable from this publication lease".into(),
        ))
    }
}

impl PublicationLease for File {
    fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        Err(ReclamationPersistenceFailure::new(
            ReclamationPersistenceStage::Read,
            "runtime reclamation requires its managed publication authority".into(),
        ))
    }

    fn retain_reclamation(
        &mut self,
        _installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        Err(ReclamationPersistenceFailure::new(
            stage,
            "runtime reclamation requires its managed publication authority".into(),
        ))
    }

    fn remove_superseded(
        &mut self,
        _agent: &AgentName,
        _current: &RuntimeArtifact,
        _superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(
            "runtime reclamation requires its managed publication authority".into(),
        ))
    }

    fn observe_superseded(
        &mut self,
        _agent: &AgentName,
        _current: &RuntimeArtifact,
        _superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        RuntimeReclamationEffect::Failed(StoreFailure::Unwritable(
            "runtime reclamation requires its managed publication authority".into(),
        ))
    }
}

/// A runtime publication and the state it actually changed under the store's
/// publication lock.
pub struct Publication {
    executable: PathBuf,
    change: PublicationChange,
    _lease: Box<dyn PublicationLease>,
}

impl fmt::Debug for Publication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Publication")
            .field("executable", &self.executable)
            .field("change", &self.change)
            .finish_non_exhaustive()
    }
}

impl Publication {
    pub fn new(
        executable: PathBuf,
        change: PublicationChange,
        lease: Box<dyn PublicationLease>,
    ) -> Self {
        Self {
            executable,
            change,
            _lease: lease,
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn change(&self) -> &PublicationChange {
        &self.change
    }

    pub fn remove_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        self._lease.remove_superseded(agent, current, superseded)
    }

    pub fn observe_superseded(
        &mut self,
        agent: &AgentName,
        current: &RuntimeArtifact,
        superseded: &RuntimeArtifact,
    ) -> RuntimeReclamationEffect {
        self._lease.observe_superseded(agent, current, superseded)
    }

    pub fn load_reclamation(
        &mut self,
    ) -> Result<Option<ManagedInstallation>, ReclamationPersistenceFailure> {
        self._lease.load_reclamation()
    }

    pub fn retain_reclamation(
        &mut self,
        installation: &ManagedInstallation,
        stage: ReclamationPersistenceStage,
    ) -> Result<(), ReclamationPersistenceFailure> {
        self._lease.retain_reclamation(installation, stage)
    }
}

/// The state left after the store withdrew a publication that could not be
/// completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RollbackChange {
    Restored(RuntimeArtifact),
    NoInstalledRuntime,
}

/// Every cleanup failure observed after publication failed. The original
/// publication failure remains separate on [`PublishFailure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationCleanupFailure {
    withdrawal: Option<StoreFailure>,
    restoration: Option<StoreFailure>,
    confirmation: Option<StoreFailure>,
}

impl PublicationCleanupFailure {
    pub fn new(
        withdrawal: Option<StoreFailure>,
        restoration: Option<StoreFailure>,
        confirmation: Option<StoreFailure>,
    ) -> Option<Self> {
        (withdrawal.is_some() || restoration.is_some() || confirmation.is_some()).then_some(Self {
            withdrawal,
            restoration,
            confirmation,
        })
    }

    pub fn withdrawal(&self) -> Option<&StoreFailure> {
        self.withdrawal.as_ref()
    }

    pub fn restoration(&self) -> Option<&StoreFailure> {
        self.restoration.as_ref()
    }

    pub fn confirmation(&self) -> Option<&StoreFailure> {
        self.confirmation.as_ref()
    }
}

impl fmt::Display for PublicationCleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let failures = [
            self.withdrawal
                .as_ref()
                .map(|failure| ("withdrawal", failure)),
            self.restoration
                .as_ref()
                .map(|failure| ("restoration", failure)),
            self.confirmation
                .as_ref()
                .map(|failure| ("confirmation", failure)),
        ];
        let mut separator = "";
        for failure in failures.into_iter().flatten() {
            write!(formatter, "{separator}{}: {}", failure.0, failure.1)?;
            separator = "; ";
        }
        Ok(())
    }
}

/// What cleanup after a failed publication established while retaining the
/// publication lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationRecovery {
    /// Publication failed before it changed installed state.
    NotRequired,
    /// The prior installed state was durably restored.
    RolledBack(RollbackChange),
    /// Cleanup failed. `rollback` is present only when the installed state was
    /// nevertheless re-read and confirmed after that failure.
    Incomplete {
        rollback: Option<RollbackChange>,
        cleanup: PublicationCleanupFailure,
    },
}

/// A publication failure, including a rollback the store actually performed.
pub struct PublishFailure {
    failure: StoreFailure,
    recovery: Box<PublicationRecovery>,
    _lease: Box<dyn PublicationLease>,
}

impl PublishFailure {
    pub fn unchanged(failure: StoreFailure, lease: Box<dyn PublicationLease>) -> Self {
        Self {
            failure,
            recovery: Box::new(PublicationRecovery::NotRequired),
            _lease: lease,
        }
    }

    pub fn rolled_back(
        failure: StoreFailure,
        rollback: RollbackChange,
        lease: Box<dyn PublicationLease>,
    ) -> Self {
        Self {
            failure,
            recovery: Box::new(PublicationRecovery::RolledBack(rollback)),
            _lease: lease,
        }
    }

    pub fn incomplete(
        failure: StoreFailure,
        rollback: Option<RollbackChange>,
        cleanup: PublicationCleanupFailure,
        lease: Box<dyn PublicationLease>,
    ) -> Self {
        Self {
            failure,
            recovery: Box::new(PublicationRecovery::Incomplete { rollback, cleanup }),
            _lease: lease,
        }
    }

    pub fn failure(&self) -> &StoreFailure {
        &self.failure
    }

    pub fn recovery(&self) -> &PublicationRecovery {
        self.recovery.as_ref()
    }

    /// Consume the failure into its full-sized store diagnostic, recovery
    /// facts, and publication lease without cloning them.
    ///
    /// The caller must keep the returned lease alive through its immediate
    /// audit attempt, including construction of the bounded domain evidence
    /// projected from the original diagnostic and cleanup failures.
    pub fn into_parts(self) -> (StoreFailure, PublicationRecovery, Box<dyn PublicationLease>) {
        (self.failure, *self.recovery, self._lease)
    }
}

impl fmt::Debug for PublishFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PublishFailure")
            .field("failure", &self.failure)
            .field("recovery", &self.recovery)
            .finish_non_exhaustive()
    }
}

/// Durable evidence for agent-runtime installation transitions.
pub trait InstallAudit: Send + Sync {
    /// Commit one immutable transition before the install reports its outcome.
    fn record(&self, transition: InstallTransition) -> Result<AuditAcknowledgement, AuditFailure>;

    /// Find the exact completion already retained for a prepared attempt.
    fn completion_for(
        &self,
        _preparation: &PublicationPreparation,
    ) -> Result<Option<InstallTransition>, AuditFailure> {
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallDeliveryFailureStage {
    Initialize,
    AcquireLock,
    ReadState,
    Prepare,
    RetainOutcome,
    Settle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallDeliveryFailure {
    stage: InstallDeliveryFailureStage,
    detail: String,
}

impl InstallDeliveryFailure {
    pub fn new(stage: InstallDeliveryFailureStage, detail: String) -> Self {
        Self { stage, detail }
    }

    pub fn stage(&self) -> InstallDeliveryFailureStage {
        self.stage
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for InstallDeliveryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "installation delivery failed at {:?}: {}",
            self.stage, self.detail
        )
    }
}

impl std::error::Error for InstallDeliveryFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedInstallation {
    record_id: String,
    preparation: PublicationPreparation,
}

impl PreparedInstallation {
    pub fn new(record_id: String, preparation: PublicationPreparation) -> Self {
        Self {
            record_id,
            preparation,
        }
    }

    pub fn record_id(&self) -> &str {
        &self.record_id
    }

    pub fn preparation(&self) -> &PublicationPreparation {
        &self.preparation
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingInstallationDelivery {
    Prepared(PreparedInstallation),
    Outcome {
        prepared: PreparedInstallation,
        outcome: Box<PublicationOutcome>,
    },
}

pub trait InstallationDeliverySession {
    fn pending(&mut self) -> Result<Option<PendingInstallationDelivery>, InstallDeliveryFailure>;

    fn settled(
        &mut self,
        delivery_id: &str,
    ) -> Result<Option<(PreparedInstallation, PublicationSettlement)>, InstallDeliveryFailure>;

    fn prepare(
        &mut self,
        preparation: PublicationPreparation,
    ) -> Result<PreparedInstallation, InstallDeliveryFailure>;

    fn retain_outcome(
        &mut self,
        prepared: &PreparedInstallation,
        outcome: &PublicationOutcome,
    ) -> Result<(), InstallDeliveryFailure>;

    fn settle(
        &mut self,
        prepared: &PreparedInstallation,
        settlement: &PublicationSettlement,
    ) -> Result<(), InstallDeliveryFailure>;
}

pub trait InstallationDelivery: Send + Sync {
    fn session(
        &self,
        account_id: &str,
    ) -> Result<Box<dyn InstallationDeliverySession + '_>, InstallDeliveryFailure>;
}

/// The private file one install downloads its archive into.
///
/// An open handle rather than a path, and that is the whole point of the type.
/// The order this context exists to guarantee is download, measure, accept,
/// unpack: a *path* can name a different file at each of those steps, so a
/// digest taken from one open and an unpack from another prove nothing about
/// each other. One handle, opened once, cannot come apart that way.
///
/// The store creates it exclusively, and where the platform allows it lets go
/// of the name at once. Where it does, what that buys is precise and worth
/// stating precisely: nothing holding only the *name* can reach the file — not
/// to truncate it between the hash and the unpack, not to replace it — and two
/// installs running at the same time stage into two files rather than over one
/// another. It is not unreachable in general: a process running as the same
/// user can still find the open descriptor, and a user who can do that can
/// equally write over the installed runtime afterwards. That is the limit of
/// what this can defend.
///
/// Where the platform does not allow it — today that is Windows, where a file
/// cannot be unlinked while it is open — the name survives, and with it the
/// substitution this type exists to prevent: a same-user process could rewrite
/// the staged file between the digest and the unpack, and the install would
/// then measure one thing and unpack another. Nothing reaches that path at
/// present, because no release is pinned for Windows and the use case refuses a
/// platform the pin does not cover before it stages anything. Pinning one is
/// what would make this real, so it is written down here rather than discovered
/// then.
///
/// The use case never reads or writes it — it only passes it along in order,
/// and hands it back to be discarded.
#[derive(Debug)]
pub struct StagedArchive {
    file: File,
    path: PathBuf,
}

impl StagedArchive {
    /// Take ownership of a file the store has just created for this install.
    ///
    /// For store adapters. The file is expected to be private, empty, and
    /// reachable by no name anything else knows.
    pub fn new(file: File, path: PathBuf) -> Self {
        Self { file, path }
    }

    /// The handle every step reads and writes through.
    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// The name the file was created under.
    ///
    /// Not a name that necessarily still refers to it: a store is expected to
    /// release it as soon as the file exists. It is kept so that a store which
    /// cannot do that has something to remove, and so a diagnostic can say
    /// where the download was.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Where release archives come from.
///
/// A port with one method so the install use case can be tested without a
/// network: every test in this context substitutes an archive that is already
/// on disk, or a failure, and none of them reaches the internet.
///
/// The archive is written into a file rather than returned as bytes because
/// these archives are large — a runtime is on the order of a hundred megabytes —
/// and holding one in memory to hash it and then again to unpack it is a cost
/// with nothing to show for it.
pub trait ArchiveSource: Send + Sync {
    /// Fetch `url` into `staged`, which is empty when this is called, keeping
    /// at most `at_most` bytes of it.
    ///
    /// `at_most` is the length the pin says the archive is, and an
    /// implementation stops with [`SourceFailure::TooLarge`] the moment more
    /// than that has arrived. It is a bound rather than an expectation: a body
    /// that is *shorter* is not this port's business, because a truncated
    /// download is exactly what the digest check afterwards is for. Passed in
    /// rather than chosen here so that the number a fetch is held to is the one
    /// that was measured when the release was pinned, not a constant somebody
    /// guessed.
    ///
    /// A failure may leave bytes in `staged`; the caller discards it either way
    /// and never asks for its digest.
    fn download(
        &self,
        url: &str,
        at_most: u64,
        staged: &mut StagedArchive,
    ) -> Result<(), SourceFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedExecutableUseFailure(String);

impl ManagedExecutableUseFailure {
    pub fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }

    pub fn detail(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ManagedExecutableUseFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ManagedExecutableUseFailure {}

pub trait ManagedExecutableUseGuard: Send {
    fn release(&mut self) -> Result<(), ManagedExecutableUseFailure>;
}

pub trait ManagedExecutableUse: Send + Sync {
    fn admit(&self) -> Result<Box<dyn ManagedExecutableUseGuard>, ManagedExecutableUseFailure>;
}

#[derive(Clone)]
pub struct ManagedLaunchSnapshot {
    executable: PathBuf,
    authority: Arc<dyn ManagedExecutableUse>,
}

impl ManagedLaunchSnapshot {
    pub fn new(executable: PathBuf, authority: Arc<dyn ManagedExecutableUse>) -> Self {
        Self {
            executable,
            authority,
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn admit(&self) -> Result<Box<dyn ManagedExecutableUseGuard>, ManagedExecutableUseFailure> {
        self.authority.admit()
    }

    #[cfg(test)]
    pub(crate) fn unmanaged(executable: PathBuf) -> Self {
        Self::new(executable, Arc::new(TestUnmanagedExecutableUse))
    }
}

#[cfg(test)]
struct TestUnmanagedExecutableUse;

#[cfg(test)]
impl ManagedExecutableUse for TestUnmanagedExecutableUse {
    fn admit(&self) -> Result<Box<dyn ManagedExecutableUseGuard>, ManagedExecutableUseFailure> {
        Ok(Box::new(TestUnmanagedExecutableUseGuard))
    }
}

#[cfg(test)]
struct TestUnmanagedExecutableUseGuard;

#[cfg(test)]
impl ManagedExecutableUseGuard for TestUnmanagedExecutableUseGuard {
    fn release(&mut self) -> Result<(), ManagedExecutableUseFailure> {
        Ok(())
    }
}

impl fmt::Debug for ManagedLaunchSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedLaunchSnapshot")
            .field("executable", &self.executable)
            .finish_non_exhaustive()
    }
}

/// Where installed runtimes live on this machine.
///
/// The filesystem side of installing, behind one port so that the use case
/// below owns the *order* — download, hash, accept, publish — and none of the
/// effects. The methods are deliberately shaped around that order rather than
/// as a general filesystem: there is no "write this file anywhere" here,
/// because the point of the type is that an agent runtime can only be put in
/// the one place Nessa manages.
pub trait RuntimeStore: Send + Sync {
    /// Where `release` is already installed for `agent`, if it really is.
    ///
    /// `None` is a real answer — this release is not installed — and is what
    /// makes a second install of the same pin free.
    ///
    /// Deliberately asked about one release rather than "what is installed":
    /// the only thing a caller can do with the answer is skip a download it
    /// would otherwise start, and a store that answered more generally would be
    /// handing out a launch path for a runtime nobody named.
    ///
    /// The implementation reports `Some` only when what it recorded agrees with
    /// `release` on the version and on every file it installs, and when all of
    /// those files are really on the disk. Answering from the record alone
    /// would let a half-finished install, or one whose executable was since
    /// deleted, be reported as a runtime Nessa can launch — and checking only
    /// the launch would do the same for a runtime whose helper programs are
    /// gone. Keeping those checks here rather than at the call site is what
    /// lets the use case above touch no files.
    fn installed(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<PathBuf>, StoreFailure>;

    /// Verify and retain authority to launch one installed managed runtime.
    ///
    /// Unlike [`Self::installed`], this is not an observation-only readiness
    /// query. A returned snapshot prevents physical reclamation while future
    /// launches remain possible and durably admits each process generation.
    fn managed_launch(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
    ) -> Result<Option<ManagedLaunchSnapshot>, StoreFailure>;

    /// Acquire the publication authority used for reclamation recovery when no
    /// installation publication already owns it.
    fn reclamation_lease(
        &self,
        agent: &AgentName,
    ) -> Result<Box<dyn PublicationLease>, StoreFailure>;

    /// Create a private file this install may download into.
    ///
    /// Owned by the store rather than chosen by the caller so that a partial
    /// download is always somewhere the store knows how to clean up, never
    /// beside the executable that is still in use, and never a name a second
    /// install would pick too.
    fn stage(&self, agent: &AgentName) -> Result<StagedArchive, StoreFailure>;

    /// The SHA-256 of what has been staged.
    fn digest(&self, staged: &mut StagedArchive) -> Result<ArchiveDigest, StoreFailure>;

    /// Unpack every file the release names out of `staged` and make them the
    /// installed runtime for `agent`, returning where the program to launch now
    /// is.
    ///
    /// All of them or none: a runtime whose helper programs are missing starts
    /// and then cannot do its work, so an implementation that cannot finish
    /// takes back what it wrote and reports nothing as installed. One path
    /// comes back rather than a list because one of the files is the launch and
    /// the rest are found relative to it — see
    /// [`ReleaseContents`](crate::agent_install::domain::ReleaseContents).
    ///
    /// Called only after [`PinnedRelease::accept`] has passed, and given the
    /// same open file that was measured, so an implementation may assume it is
    /// unpacking the pinned bytes. It may not assume anything about their
    /// *contents*: a file named by the pin can still be absent, which is
    /// [`StoreFailure::IncompleteArchive`].
    ///
    /// Durable on return: an executable this reports is one a machine that
    /// loses power immediately afterwards still has.
    fn publish(
        &self,
        agent: &AgentName,
        release: &PinnedRelease,
        staged: &mut StagedArchive,
    ) -> Result<Publication, PublishFailure>;

    /// Forget a staged download. Never fails the install: a leftover file in a
    /// directory Nessa owns is survivable, and reporting it would turn a
    /// successful install into a failed one.
    fn discard(&self, staged: StagedArchive);
}
