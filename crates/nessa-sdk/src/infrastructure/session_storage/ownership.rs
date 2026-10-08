//! SQLite ownership rows beside the session record database.
//!
//! ```text
//! OwnershipStore::write -> instance admission -> owned blocking job -> ownership.sqlite3
//! OwnershipStore::read  <- blocking query/decode/validation <- ownership.sqlite3
//! ```
//!
//! The arrow is one snapshot replace. A body that does not decode is rejected
//! and left unchanged. This file does not copy transcripts. Shared `nessa-local-storage::physical_operation`
//! holds physical admission through input/state cleanup; `ownership/tests.rs`
//! exercises cancellation, queue lifetime, poison, and watchdog responsiveness.
#![deny(missing_docs)]

mod settlement;
use settlement::{CompletionDto, ProofDto};

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use nessa_local_database::{open, rusqlite, OpenError, Schema};
use nessa_local_storage::physical_operation::Worker;
use serde::{Deserialize, Serialize};

use crate::application::agent_execution::subagents::{OwnershipStore, PortFailure};
use crate::domain::agent_execution::{
    executions::ExecutionId,
    sessions::SessionId,
    subagents::{
        AbsenceAudit, AbsenceProof, AgentLifetimeId, ApprovalPolicy, CloseCompletionRow,
        CloseEvidenceDetail, CloseOperationId, DeliveryState, EvidenceFact, HostActor, Initiator,
        KnownMilestone, LifetimeCause, LifetimeRow, LifetimeState, ModelChoice, OwnershipEvidence,
        OwnershipMeaning, OwnershipSnapshot, PhysicalFact, ReportId, ReportRow,
        ResourceObservationAudit, SettlementProof, SettlementRow, SpawnBinding, SpawnOrigin,
        SpawnProgress, SpawnRequestId, SpawnRow, TaskDigest, TaskReceiptId,
    },
    tools::ToolCallId,
};

#[cfg(test)]
#[path = "../../../../nessa-local-storage/tests/support/physical_operation.rs"]
mod physical_tests;

const DEFINITION: &str = "\
CREATE TABLE ownership_snapshot (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    body TEXT NOT NULL
) STRICT;
PRAGMA user_version = 1;
";

/// One ownership snapshot file. Writers replace the single retained body.
///
/// Each instance admits one physical operation before cloning its input. The
/// `canceled_sqlite_write_excludes_following_io` and queued-worker tests cover
/// caller cancellation: dropping the async wait leaves the admitted job owning
/// its slot through physical I/O and captured-input cleanup. Independent instances
/// have independent slots; this is not cross-process exclusion or a shutdown drain.
/// The first poll requires a Tokio runtime; no runtime returns a typed
/// pre-effect rejection (`no_runtime_first_poll_is_typed`). Admission captures
/// that executor so the Send future can resume on another thread. The originating
/// runtime must remain alive for reliable completion; the ordering and shutdown
/// limitations are documented in `docs/design/bounded-physical-persistence.md`.
/// Interrupted work and a poisoned connection return [`PortFailure::Uncertain`].
/// Read interruption does not become an empty snapshot; only SQLite NoRows does.
pub struct SqliteOwnershipStore {
    state: Arc<OwnershipState>,
    worker: Worker,
    #[cfg(test)]
    probe: Arc<physical_tests::Probe>,
}

struct OwnershipState {
    connection: Mutex<rusqlite::Connection>,
    #[cfg(test)]
    gate: Option<Arc<physical_tests::Gate>>,
}

impl SqliteOwnershipStore {
    /// Open or create `path` at the current ownership schema.
    /// A damaged or differently versioned file is returned as [`OpenError`] and not rewritten.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let schema = Schema::new(DEFINITION).expect("ownership schema states its version");
        Ok(Self {
            state: Arc::new(OwnershipState {
                connection: Mutex::new(open(path, &schema)?),
                #[cfg(test)]
                gate: None,
            }),
            worker: Worker::new(),
            #[cfg(test)]
            probe: Arc::new(physical_tests::Probe::default()),
        })
    }
}

#[async_trait]
impl OwnershipStore for SqliteOwnershipStore {
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        let permit = self
            .worker
            .admit()
            .await
            .map_err(|_| PortFailure::Rejected)?;
        let state = self.state.clone();
        #[cfg(not(test))]
        let snapshot = snapshot.clone();
        #[cfg(test)]
        let snapshot = self.probe.clone_input(|| snapshot.clone());
        let operation = move || state.write(&snapshot);
        #[cfg(test)]
        let operation = self.probe.wrap(operation);
        permit
            .submit(operation)
            .await
            .map_err(|_| PortFailure::Uncertain)?
    }

    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        let permit = self
            .worker
            .admit()
            .await
            .map_err(|_| PortFailure::Rejected)?;
        #[cfg(not(test))]
        let state = self.state.clone();
        #[cfg(test)]
        let state = self.probe.clone_input(|| self.state.clone());
        let operation = move || state.read();
        #[cfg(test)]
        let operation = self.probe.wrap(operation);
        permit
            .submit(operation)
            .await
            .map_err(|_| PortFailure::Uncertain)?
    }
}

impl OwnershipState {
    fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.enter();
        }
        let body = serde_json::to_string(&SnapshotDto::from(snapshot))
            .map_err(|_| PortFailure::Rejected)?;
        let connection = self.connection.lock().map_err(|_| PortFailure::Uncertain)?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| PortFailure::Rejected)?;
        transaction
            .execute("DELETE FROM ownership_snapshot", [])
            .map_err(|_| PortFailure::Rejected)?;
        transaction
            .execute(
                "INSERT INTO ownership_snapshot (id, body) VALUES (1, ?1)",
                [body],
            )
            .map_err(|_| PortFailure::Rejected)?;
        transaction.commit().map_err(|_| PortFailure::Rejected)
    }

    fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.enter();
        }
        let connection = self.connection.lock().map_err(|_| PortFailure::Uncertain)?;
        let body = match connection.query_row(
            "SELECT body FROM ownership_snapshot WHERE id = 1",
            [],
            |row| row.get(0),
        ) {
            Ok(body) => body,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(OwnershipSnapshot::default()),
            Err(_) => return Err(PortFailure::Rejected),
        };
        let body: String = body;
        let dto: SnapshotDto = serde_json::from_str(&body).map_err(|_| PortFailure::Rejected)?;
        dto.try_into().map_err(|_| PortFailure::Rejected)
    }
}

#[derive(Serialize, Deserialize)]
struct SnapshotDto {
    close_completions: Vec<CompletionDto>,
    lifetimes: Vec<LifetimeDto>,
    spawns: Vec<SpawnDto>,
    settlements: Vec<SettlementDto>,
    reports: Vec<ReportDto>,
}

#[derive(Serialize, Deserialize)]
struct LifetimeDto {
    lifetime_id: String,
    session_id: String,
    state: String,
    close_operation: Option<String>,
    cause: Option<String>,
    initiator: Option<InitiatorDto>,
    cascaded_from: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SpawnDto {
    child_lifetime: String,
    child_session: String,
    binding: BindingDto,
    progress: ProgressDto,
}

#[derive(Serialize, Deserialize)]
struct BindingDto {
    parent_lifetime: String,
    parent_session: String,
    request_id: String,
    task_digest: String,
    policy_mode: String,
    policy_offer: String,
    policy_revision: String,
    model_provider: Option<String>,
    model: Option<String>,
    origin: OriginDto,
}

#[derive(Serialize, Deserialize)]
struct OriginDto {
    kind: String,
    principal_id: Option<String>,
    surface_id: Option<String>,
    request_id: Option<String>,
    execution: Option<String>,
    tool: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ProgressDto {
    kind: String,
    receipt: Option<String>,
    known: Option<MilestoneDto>,
}

#[derive(Serialize, Deserialize)]
struct MilestoneDto {
    kind: String,
    receipt: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SettlementDto {
    proof: ProofDto,
    close_lifetime: String,
    target: String,
    physical: String,
    evidence: String,
}

#[derive(Serialize, Deserialize)]
struct ReportDto {
    report_id: String,
    child_lifetime: String,
    parent_lifetime: String,
    state: String,
}

#[derive(Serialize, Deserialize)]
struct InitiatorDto {
    kind: String,
    principal_id: Option<String>,
    surface_id: Option<String>,
    request_id: Option<String>,
}

impl From<&OwnershipSnapshot> for SnapshotDto {
    fn from(snapshot: &OwnershipSnapshot) -> Self {
        Self {
            close_completions: snapshot
                .close_completions
                .iter()
                .map(CompletionDto::from)
                .collect(),
            lifetimes: snapshot.lifetimes.iter().map(LifetimeDto::from).collect(),
            spawns: snapshot.spawns.iter().map(SpawnDto::from).collect(),
            settlements: snapshot
                .settlements
                .iter()
                .map(SettlementDto::from)
                .collect(),
            reports: snapshot.reports.iter().map(ReportDto::from).collect(),
        }
    }
}

impl TryFrom<SnapshotDto> for OwnershipSnapshot {
    type Error = ();

    fn try_from(dto: SnapshotDto) -> Result<Self, Self::Error> {
        Ok(Self {
            close_completions: dto
                .close_completions
                .into_iter()
                .map(CompletionDto::try_into)
                .collect::<Result<_, _>>()?,
            lifetimes: dto
                .lifetimes
                .into_iter()
                .map(LifetimeDto::try_into)
                .collect::<Result<_, _>>()?,
            spawns: dto
                .spawns
                .into_iter()
                .map(SpawnDto::try_into)
                .collect::<Result<_, _>>()?,
            settlements: dto
                .settlements
                .into_iter()
                .map(SettlementDto::try_into)
                .collect::<Result<_, _>>()?,
            reports: dto
                .reports
                .into_iter()
                .map(ReportDto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<&LifetimeRow> for LifetimeDto {
    fn from(row: &LifetimeRow) -> Self {
        Self {
            lifetime_id: row.lifetime_id.as_str().to_owned(),
            session_id: row.session_id.as_str().to_owned(),
            state: match row.state {
                LifetimeState::Open => "open",
                LifetimeState::Closing => "closing",
                LifetimeState::Closed => "closed",
            }
            .to_owned(),
            close_operation: row
                .close_operation
                .as_ref()
                .map(|id| id.as_str().to_owned()),
            cause: row.cause.as_ref().map(cause_name).map(str::to_owned),
            initiator: row.initiator.as_ref().map(InitiatorDto::from),
            cascaded_from: row.cascaded_from.as_ref().map(|id| id.as_str().to_owned()),
        }
    }
}

impl TryFrom<LifetimeDto> for LifetimeRow {
    type Error = ();

    fn try_from(dto: LifetimeDto) -> Result<Self, Self::Error> {
        Ok(Self {
            lifetime_id: AgentLifetimeId::new(dto.lifetime_id).map_err(|_| ())?,
            session_id: SessionId::new(dto.session_id).map_err(|_| ())?,
            state: match dto.state.as_str() {
                "open" => LifetimeState::Open,
                "closing" => LifetimeState::Closing,
                "closed" => LifetimeState::Closed,
                _ => return Err(()),
            },
            close_operation: dto
                .close_operation
                .map(CloseOperationId::new)
                .transpose()
                .map_err(|_| ())?,
            cause: dto.cause.as_deref().map(parse_cause).transpose()?,
            initiator: dto.initiator.map(InitiatorDto::try_into).transpose()?,
            cascaded_from: dto
                .cascaded_from
                .map(AgentLifetimeId::new)
                .transpose()
                .map_err(|_| ())?,
        })
    }
}

impl From<&SpawnRow> for SpawnDto {
    fn from(row: &SpawnRow) -> Self {
        Self {
            child_lifetime: row.child_lifetime.as_str().to_owned(),
            child_session: row.child_session.as_str().to_owned(),
            binding: BindingDto::from(&row.binding),
            progress: ProgressDto::from(&row.progress),
        }
    }
}

impl TryFrom<SpawnDto> for SpawnRow {
    type Error = ();

    fn try_from(dto: SpawnDto) -> Result<Self, Self::Error> {
        Ok(Self {
            child_lifetime: AgentLifetimeId::new(dto.child_lifetime).map_err(|_| ())?,
            child_session: SessionId::new(dto.child_session).map_err(|_| ())?,
            binding: dto.binding.try_into()?,
            progress: dto.progress.try_into()?,
        })
    }
}

impl From<&SpawnBinding> for BindingDto {
    fn from(binding: &SpawnBinding) -> Self {
        Self {
            parent_lifetime: binding.parent_lifetime.as_str().to_owned(),
            parent_session: binding.parent_session.as_str().to_owned(),
            request_id: binding.request_id.as_str().to_owned(),
            task_digest: binding.task_digest.as_str().to_owned(),
            policy_mode: binding.policy.mode().to_owned(),
            policy_offer: binding.policy.offer().to_owned(),
            policy_revision: binding.policy.revision().to_owned(),
            model_provider: binding
                .model
                .as_ref()
                .map(|model| model.provider().to_owned()),
            model: binding.model.as_ref().map(|model| model.model().to_owned()),
            origin: OriginDto::from(&binding.origin),
        }
    }
}

impl TryFrom<BindingDto> for SpawnBinding {
    type Error = ();

    fn try_from(dto: BindingDto) -> Result<Self, Self::Error> {
        let model = match (dto.model_provider, dto.model) {
            (Some(provider), Some(model)) => {
                Some(ModelChoice::new(provider, model).map_err(|_| ())?)
            }
            (None, None) => None,
            _ => return Err(()),
        };
        Ok(Self {
            parent_lifetime: AgentLifetimeId::new(dto.parent_lifetime).map_err(|_| ())?,
            parent_session: SessionId::new(dto.parent_session).map_err(|_| ())?,
            request_id: SpawnRequestId::new(dto.request_id).map_err(|_| ())?,
            task_digest: TaskDigest::new(dto.task_digest).map_err(|_| ())?,
            policy: ApprovalPolicy::new(dto.policy_mode, dto.policy_offer, dto.policy_revision)
                .map_err(|_| ())?,
            model,
            origin: dto.origin.try_into()?,
        })
    }
}

impl From<&SpawnOrigin> for OriginDto {
    fn from(origin: &SpawnOrigin) -> Self {
        match origin {
            SpawnOrigin::Host(actor) => Self {
                kind: "host".to_owned(),
                principal_id: Some(actor.principal_id().to_owned()),
                surface_id: Some(actor.surface_id().to_owned()),
                request_id: Some(actor.request_id().to_owned()),
                execution: None,
                tool: None,
            },
            SpawnOrigin::Execution { execution, tool } => Self {
                kind: "execution".to_owned(),
                principal_id: None,
                surface_id: None,
                request_id: None,
                execution: Some(execution.as_str().to_owned()),
                tool: tool.as_ref().map(|tool| tool.as_str().to_owned()),
            },
        }
    }
}

impl TryFrom<OriginDto> for SpawnOrigin {
    type Error = ();

    fn try_from(dto: OriginDto) -> Result<Self, Self::Error> {
        match dto.kind.as_str() {
            "host" => Ok(Self::Host(
                HostActor::new(
                    dto.principal_id.ok_or(())?,
                    dto.surface_id.ok_or(())?,
                    dto.request_id.ok_or(())?,
                )
                .map_err(|_| ())?,
            )),
            "execution" => Ok(Self::Execution {
                execution: ExecutionId::new(dto.execution.ok_or(())?).map_err(|_| ())?,
                tool: dto.tool.map(ToolCallId::new).transpose().map_err(|_| ())?,
            }),
            _ => Err(()),
        }
    }
}

impl From<&SpawnProgress> for ProgressDto {
    fn from(progress: &SpawnProgress) -> Self {
        match progress {
            SpawnProgress::Reserved => Self {
                kind: "reserved".to_owned(),
                receipt: None,
                known: None,
            },
            SpawnProgress::Prepared => Self {
                kind: "prepared".to_owned(),
                receipt: None,
                known: None,
            },
            SpawnProgress::Attached => Self {
                kind: "attached".to_owned(),
                receipt: None,
                known: None,
            },
            SpawnProgress::TaskAdmitted { receipt } => Self {
                kind: "task_admitted".to_owned(),
                receipt: Some(receipt.as_str().to_owned()),
                known: None,
            },
            SpawnProgress::Unconfirmed { known } => Self {
                kind: "unconfirmed".to_owned(),
                receipt: None,
                known: Some(MilestoneDto::from(known)),
            },
            SpawnProgress::Draining { known } => Self {
                kind: "draining".to_owned(),
                receipt: None,
                known: Some(MilestoneDto::from(known)),
            },
            SpawnProgress::StartupFailed { known } => Self {
                kind: "startup_failed".to_owned(),
                receipt: None,
                known: Some(MilestoneDto::from(known)),
            },
            SpawnProgress::Ended { known } => Self {
                kind: "ended".to_owned(),
                receipt: None,
                known: Some(MilestoneDto::from(known)),
            },
        }
    }
}

impl TryFrom<ProgressDto> for SpawnProgress {
    type Error = ();

    fn try_from(dto: ProgressDto) -> Result<Self, Self::Error> {
        match dto.kind.as_str() {
            "reserved" => Ok(Self::Reserved),
            "prepared" => Ok(Self::Prepared),
            "attached" => Ok(Self::Attached),
            "task_admitted" => Ok(Self::TaskAdmitted {
                receipt: TaskReceiptId::new(dto.receipt.ok_or(())?).map_err(|_| ())?,
            }),
            "unconfirmed" => Ok(Self::Unconfirmed {
                known: dto.known.ok_or(())?.try_into()?,
            }),
            "draining" => Ok(Self::Draining {
                known: dto.known.ok_or(())?.try_into()?,
            }),
            "startup_failed" => Ok(Self::StartupFailed {
                known: dto.known.ok_or(())?.try_into()?,
            }),
            "ended" => Ok(Self::Ended {
                known: dto.known.ok_or(())?.try_into()?,
            }),
            _ => Err(()),
        }
    }
}

impl From<&KnownMilestone> for MilestoneDto {
    fn from(milestone: &KnownMilestone) -> Self {
        match milestone {
            KnownMilestone::Reserved => Self {
                kind: "reserved".to_owned(),
                receipt: None,
            },
            KnownMilestone::Prepared => Self {
                kind: "prepared".to_owned(),
                receipt: None,
            },
            KnownMilestone::Attached => Self {
                kind: "attached".to_owned(),
                receipt: None,
            },
            KnownMilestone::TaskAdmitted { receipt } => Self {
                kind: "task_admitted".to_owned(),
                receipt: Some(receipt.as_str().to_owned()),
            },
        }
    }
}

impl TryFrom<MilestoneDto> for KnownMilestone {
    type Error = ();

    fn try_from(dto: MilestoneDto) -> Result<Self, Self::Error> {
        match dto.kind.as_str() {
            "reserved" => Ok(Self::Reserved),
            "prepared" => Ok(Self::Prepared),
            "attached" => Ok(Self::Attached),
            "task_admitted" => Ok(Self::TaskAdmitted {
                receipt: TaskReceiptId::new(dto.receipt.ok_or(())?).map_err(|_| ())?,
            }),
            _ => Err(()),
        }
    }
}

impl From<&SettlementRow> for SettlementDto {
    fn from(row: &SettlementRow) -> Self {
        Self {
            proof: ProofDto::from(&row.proof),
            close_lifetime: row.close_lifetime.as_str().to_owned(),
            target: row.target.as_str().to_owned(),
            physical: match row.physical {
                PhysicalFact::Pending => "pending",
                PhysicalFact::Released => "released",
                PhysicalFact::Failed => "failed",
            }
            .to_owned(),
            evidence: match row.evidence {
                EvidenceFact::Pending => "pending",
                EvidenceFact::Acknowledged => "acknowledged",
                EvidenceFact::Failed => "failed",
            }
            .to_owned(),
        }
    }
}

impl TryFrom<SettlementDto> for SettlementRow {
    type Error = ();

    fn try_from(dto: SettlementDto) -> Result<Self, Self::Error> {
        Ok(Self {
            proof: dto.proof.try_into()?,
            close_lifetime: AgentLifetimeId::new(dto.close_lifetime).map_err(|_| ())?,
            target: AgentLifetimeId::new(dto.target).map_err(|_| ())?,
            physical: match dto.physical.as_str() {
                "pending" => PhysicalFact::Pending,
                "released" => PhysicalFact::Released,
                "failed" => PhysicalFact::Failed,
                _ => return Err(()),
            },
            evidence: match dto.evidence.as_str() {
                "pending" => EvidenceFact::Pending,
                "acknowledged" => EvidenceFact::Acknowledged,
                "failed" => EvidenceFact::Failed,
                _ => return Err(()),
            },
        })
    }
}

impl From<&ReportRow> for ReportDto {
    fn from(row: &ReportRow) -> Self {
        Self {
            report_id: row.report_id.as_str().to_owned(),
            child_lifetime: row.child_lifetime.as_str().to_owned(),
            parent_lifetime: row.parent_lifetime.as_str().to_owned(),
            state: match row.state {
                DeliveryState::Retained => "retained",
                DeliveryState::Submitted => "submitted",
                DeliveryState::Suppressed => "suppressed",
                DeliveryState::Unconfirmed => "unconfirmed",
            }
            .to_owned(),
        }
    }
}

impl TryFrom<ReportDto> for ReportRow {
    type Error = ();

    fn try_from(dto: ReportDto) -> Result<Self, Self::Error> {
        Ok(Self {
            report_id: ReportId::new(dto.report_id).map_err(|_| ())?,
            child_lifetime: AgentLifetimeId::new(dto.child_lifetime).map_err(|_| ())?,
            parent_lifetime: AgentLifetimeId::new(dto.parent_lifetime).map_err(|_| ())?,
            state: match dto.state.as_str() {
                "retained" => DeliveryState::Retained,
                "submitted" => DeliveryState::Submitted,
                "suppressed" => DeliveryState::Suppressed,
                "unconfirmed" => DeliveryState::Unconfirmed,
                _ => return Err(()),
            },
        })
    }
}

impl From<&Initiator> for InitiatorDto {
    fn from(initiator: &Initiator) -> Self {
        match initiator {
            Initiator::Runtime => Self {
                kind: "runtime".to_owned(),
                principal_id: None,
                surface_id: None,
                request_id: None,
            },
            Initiator::Host(actor) => Self {
                kind: "host".to_owned(),
                principal_id: Some(actor.principal_id().to_owned()),
                surface_id: Some(actor.surface_id().to_owned()),
                request_id: Some(actor.request_id().to_owned()),
            },
        }
    }
}

impl TryFrom<InitiatorDto> for Initiator {
    type Error = ();

    fn try_from(dto: InitiatorDto) -> Result<Self, Self::Error> {
        match dto.kind.as_str() {
            "runtime" => Ok(Self::Runtime),
            "host" => Ok(Self::Host(
                HostActor::new(
                    dto.principal_id.ok_or(())?,
                    dto.surface_id.ok_or(())?,
                    dto.request_id.ok_or(())?,
                )
                .map_err(|_| ())?,
            )),
            _ => Err(()),
        }
    }
}

fn cause_name(cause: &LifetimeCause) -> &'static str {
    match cause {
        LifetimeCause::HostClose => "host_close",
        LifetimeCause::TerminalFailure => "terminal_failure",
        LifetimeCause::OwnerDisposed => "owner_disposed",
        LifetimeCause::Deletion => "deletion",
        LifetimeCause::ModeRecovery => "mode_recovery",
        LifetimeCause::GatewayRetirement => "gateway_retirement",
    }
}

fn parse_cause(name: &str) -> Result<LifetimeCause, ()> {
    match name {
        "host_close" => Ok(LifetimeCause::HostClose),
        "terminal_failure" => Ok(LifetimeCause::TerminalFailure),
        "owner_disposed" => Ok(LifetimeCause::OwnerDisposed),
        "deletion" => Ok(LifetimeCause::Deletion),
        "mode_recovery" => Ok(LifetimeCause::ModeRecovery),
        "gateway_retirement" => Ok(LifetimeCause::GatewayRetirement),
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests;
