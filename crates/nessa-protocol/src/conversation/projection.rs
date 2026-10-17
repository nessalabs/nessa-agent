use super::domain::{ConversationApprovalMode, ConversationId};
use super::tool_uis::{McpToolUis, NoMcpToolUis};
use super::view::{
    ConversationAnswerOption, ConversationApprovalModeChangeView, ConversationArtifact,
    ConversationAsked, ConversationCapabilities, ConversationLeaseCause, ConversationLeaseCleanup,
    ConversationLeaseEnvironment, ConversationLeaseRefusal, ConversationLeaseSandbox,
    ConversationLeaseState, ConversationLeaseView, ConversationLifecycle,
    ConversationLifecyclePhase, ConversationMcpTool, ConversationMessage, ConversationMessageApp,
    ConversationMessageStatus, ConversationNote, ConversationPart, ConversationPending,
    ConversationPendingMode, ConversationPermission, ConversationPermissionAsk,
    ConversationPermissionDeadline, ConversationPermissionOption,
    ConversationPermissionOptionEffect, ConversationPermissionOrigin, ConversationQuestion,
    ConversationReviewEntry, ConversationReviewKind, ConversationRuntime,
    ConversationSelectionView, ConversationTool, ConversationTranscriptState,
    ConversationUnreadable, ConversationView, NativeHooksChoice, MAX_STRUCTURED_CONTENT_BYTES,
};
use nessa_sdk::application::agent_execution::permissions::CancellationOrigin;
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    executions::{ExecutionEvent, ExecutionUpdate, SubmissionMode},
    sessions::{
        CommittedSession, CommittedStatus, CurrentLease, CurrentLeaseState, InvocationRecord,
        LeaseRecord, SessionSnapshot,
    },
};
use nessa_sdk::domain::agent_execution::leases::{
    EnvironmentRef, LeaseCleanup, LeaseEndCause, LeasePhase, LeaseRefusal, LeaseRevision,
    SandboxProfile,
};
use nessa_sdk::domain::agent_execution::tools::McpTool;
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, InvocationStage, MessageKind},
    permissions::{
        PendingReview, PermissionEffect, PermissionReviewDeadline, PermissionScope, RefusalOutcome,
        ReviewDecline, ReviewDeclineId, ReviewDeclineReason, ReviewDeclineStage,
    },
    questions::{AnswerShape, MAX_OPEN_QUESTIONS},
    tools::{ToolContentView, ToolKind, ToolStatus},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

const MAX_MESSAGES: usize = 24;
pub const MAX_TEXT: usize = 8192;
const MAX_TOOLS: usize = 16;
/// Most artifacts the view lists: the newest, in the order they were recorded.
pub const MAX_VIEW_ARTIFACTS: usize = 64;
/// Most notes the view lists: the newest, in the order they were left.
pub const MAX_VIEW_NOTES: usize = 64;
// Application transcript body; product catalog enrichment is serialized afterward.
pub const MAX_VIEW_BYTES: usize = 60_000;
pub use crate::product::generated::{MAX_LISTED_REVIEWS, MAX_REVIEW_ENTRY_TOOL_NAME_BYTES};
/// The most one review entry takes of a view, encoded, with the comma that
/// separates it from the next.
pub const MAX_REVIEW_ENTRY_BYTES: usize = 160;
/// What the most reviews a view can list take of it. A bounded view gives up
/// everything else before any of them, and never gives one up, so this
/// leaves the rest of the view most of its budget.
pub const MAX_REVIEW_LIST_BYTES: usize = MAX_LISTED_REVIEWS * MAX_REVIEW_ENTRY_BYTES;
const _: () = assert!(MAX_REVIEW_LIST_BYTES <= MAX_VIEW_BYTES * 2 / 5);
/// The schema's bound on `ConversationTool.input`, in UTF-8 bytes. An input
/// past it is left off the tool row; its review still offers all of it.
const MAX_TOOL_INPUT_BYTES: usize = 32_768;
const REQUIRED_WORK_FAILURE: &str = "The turn could not complete all required work.";
const PROVIDER_FAILURE_PREFIX: &str = "The agent provider reported an error: ";

/// The conversation's latest lease as the view shows it: what the records
/// fold into, never more. An unreadable lease claims nothing, not even the
/// revision before it, which the fold keeps for itself.
pub fn lease_view(current: &CurrentLease) -> ConversationLeaseView {
    let mut view = ConversationLeaseView {
        state: ConversationLeaseState::Unreadable,
        // Only a lease this build can read says which revision it is; the
        // fold keeps the one before an unreadable lease for itself.
        revision: None,
        environment: None,
        host: None,
        sandbox: None,
        cause: None,
        cleanup: None,
        refusal: None,
        dropped_events: 0,
        commands: None,
    };
    let terms = match current.state() {
        CurrentLeaseState::Unreadable { .. } => return view,
        CurrentLeaseState::Refused { refusal, .. } => {
            view.state = ConversationLeaseState::Refused;
            view.refusal = Some(match refusal {
                LeaseRefusal::SandboxUnavailable => ConversationLeaseRefusal::SandboxUnavailable,
                LeaseRefusal::EnvironmentUnreachable => {
                    ConversationLeaseRefusal::EnvironmentUnreachable
                }
                LeaseRefusal::EnvironmentVersionMismatch => {
                    ConversationLeaseRefusal::EnvironmentVersionMismatch
                }
                LeaseRefusal::EnvironmentBusy => ConversationLeaseRefusal::EnvironmentBusy,
                // Not granted there is, to a person, the environment not
                // running this agent for this gateway: the product contract
                // has no spelling of its own for it.
                LeaseRefusal::AgentUnavailable | LeaseRefusal::EnvironmentNotGranted => {
                    ConversationLeaseRefusal::AgentUnavailable
                }
                LeaseRefusal::EnvironmentPlatformUnsupported => {
                    ConversationLeaseRefusal::EnvironmentPlatformUnsupported
                }
                LeaseRefusal::EnvironmentInstallFailed => {
                    ConversationLeaseRefusal::EnvironmentInstallFailed
                }
            });
            current.records().first().and_then(|record| match record {
                LeaseRecord::Refused { terms, .. } => Some(terms),
                _ => None,
            })
        }
        CurrentLeaseState::Held(lease) => {
            view.dropped_events = lease.dropped_events();
            view.commands = u64::try_from(lease.commands().len())
                .ok()
                .filter(|live| *live > 0);
            let (state, cause, cleanup) = match lease.phase() {
                LeasePhase::Live => (ConversationLeaseState::Live, None, None),
                LeasePhase::Ending { cause } => (ConversationLeaseState::Ending, Some(cause), None),
                LeasePhase::Ended { cause, cleanup } => {
                    (ConversationLeaseState::Ended, Some(cause), Some(cleanup))
                }
                LeasePhase::Interrupted {
                    cause,
                    late_cleanup,
                } => (
                    ConversationLeaseState::Interrupted,
                    Some(cause),
                    late_cleanup,
                ),
            };
            view.state = state;
            view.cause = cause.map(|cause| match cause {
                LeaseEndCause::Stopped => ConversationLeaseCause::Stopped,
                LeaseEndCause::Closed => ConversationLeaseCause::Closed,
                LeaseEndCause::Revoked => ConversationLeaseCause::Revoked,
                LeaseEndCause::Expired => ConversationLeaseCause::Expired,
                LeaseEndCause::Lost => ConversationLeaseCause::Lost,
            });
            view.cleanup = cleanup.map(|cleanup| match cleanup {
                LeaseCleanup::Confirmed { forced: false } => ConversationLeaseCleanup::Confirmed,
                LeaseCleanup::Confirmed { forced: true } => ConversationLeaseCleanup::Forced,
                LeaseCleanup::NotHeld => ConversationLeaseCleanup::NotHeld,
            });
            Some(lease.terms())
        }
    };
    view.revision = current.revision().map(LeaseRevision::get);
    if let Some(terms) = terms {
        view.environment = Some(match &terms.environment {
            EnvironmentRef::Here => ConversationLeaseEnvironment::Here,
            EnvironmentRef::Ssh(host) => {
                view.host = Some(host.as_str().to_owned());
                ConversationLeaseEnvironment::Ssh
            }
            EnvironmentRef::Peer(peer) => {
                view.host = Some(peer.name());
                ConversationLeaseEnvironment::Peer
            }
        });
        view.sandbox = Some(match terms.sandbox {
            SandboxProfile::HarnessDefault => ConversationLeaseSandbox::HarnessDefault,
            SandboxProfile::None => ConversationLeaseSandbox::None,
        });
    }
    view
}

/// `restarted` is an execution that was already unfinished when this
/// projection first accepted history, and that this process has not run.
/// A result is that result even then.
/// `a_restored_turn_takes_its_result_without_being_passed_as_active`.
/// Injected and cancelled are the last stage, before that flag.
/// `an_injected_or_cancelled_turn_keeps_that_status`.
/// A missing result is otherwise still running: the turn was admitted here
/// and is between being saved and being active, or between leaving active
/// and its result being committed.
/// `a_turn_admitted_after_open_stays_running_until_its_result`.
fn record_status(record: &InvocationRecord, restarted: bool) -> ConversationMessageStatus {
    match &record.result {
        Some(Ok(value)) => outcome(*value),
        Some(Err(_))
            if record
                .scheduling
                .last()
                .is_some_and(|event| event.stage == InvocationStage::Cancelled) =>
        {
            ConversationMessageStatus::Cancelled
        }
        Some(Err(_)) => ConversationMessageStatus::Failed,
        None => match record.scheduling.last().map(|event| event.stage) {
            Some(InvocationStage::Injected) => ConversationMessageStatus::Injected,
            Some(InvocationStage::Cancelled) => ConversationMessageStatus::Cancelled,
            _ if restarted => ConversationMessageStatus::Unresolved,
            _ => ConversationMessageStatus::Running,
        },
    }
}

/// Unfinished executions in `snapshot` that this replacement is not running.
fn unfinished_not_live(snapshot: &SessionSnapshot, live_here: &HashSet<String>) -> HashSet<String> {
    snapshot
        .invocations
        .iter()
        .filter(|record| {
            record.result.is_none() && !live_here.contains(record.request.execution_id.as_str())
        })
        .map(|record| record.request.execution_id.as_str().to_owned())
        .collect()
}

/// A view's place in the committed history ([`Projection::committed_cursor`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedCursor {
    /// The stored history's incarnation; positions compare only within one.
    pub incarnation: String,
    /// The committed position the view was folded through.
    pub position: u64,
    /// The history's head as that read observed it.
    pub observed_head: u64,
}

/// Stored semantic facts are changed through their projection-owned operations.
/// External callers can inspect the immutable view but cannot revise it directly.
///
/// ```compile_fail
/// use nessa_protocol::conversation::projection::Projection;
/// use nessa_protocol::conversation::view::ConversationCapabilities;
/// let mut projection = Projection::new("conversation".into(), ConversationCapabilities::read_only(), None);
/// projection.view.selection = None;
/// ```
pub struct Projection {
    view: ConversationView,
    epoch: Uuid,
    revision: u64,
    committed_position: Option<(String, u64, u64, u64)>,
    resolved_permissions: HashSet<(String, String)>,
    /// Permission ids a request named in this execution, with the deadline
    /// that request published. A deadline notice is shown only when it repeats
    /// that deadline: one whose id is absent, whose request published no
    /// deadline, or whose outcome differs is ignored. A deadline cancellation
    /// must decide as it in full, not only share its outcome.
    requested_permissions: HashMap<(String, String), Option<PermissionReviewDeadline>>,
    /// Reviews cancelled for a reason other than their own deadline. Their
    /// deadline did not settle them, so a later notice is ignored.
    cancelled_otherwise: HashSet<(String, String)>,
    /// Asks that have stopped waiting, so a replayed ask does not reopen one.
    answered_questions: HashSet<(String, String)>,
    terminal_executions: HashSet<String>,
    /// The calls that have a part in their turn, by execution and clipped tool
    /// id, so a call's later updates find it without scanning the turn's
    /// parts. A call goes in only when its part is pushed, and an execution
    /// leaves when its parts do: cleared for a replayed record, or its message
    /// let go.
    tool_parts: HashMap<String, HashSet<String>>,
    /// The hooks choice each execution's tool events were saved with. One
    /// execution runs on one process start, so its events agree. `None`
    /// marks saved events that disagree: no choice can be shown for them.
    execution_hooks: HashMap<String, Option<bool>>,
    /// The execution this replacement was told is active. It is who may be
    /// offered an interaction. It does not, by itself, decide that every
    /// other unfinished record was interrupted.
    live_here: HashSet<String>,
    /// Unfinished executions present when this projection first accepted a
    /// snapshot, except any this process has since run. A record admitted
    /// later is absent, so a read while it is saved but not active — or no
    /// longer active, before its result is committed — stays running.
    /// `a_turn_admitted_after_open_stays_running_until_its_result`.
    unfinished_at_fold: HashSet<String>,
    /// Set once a snapshot has been accepted. Until then the next
    /// replacement is that first acceptance, and its unfinished records are
    /// restarts. `the_first_snapshot_folded_into_an_empty_projection_is_a_restart`.
    history_accepted: bool,
    /// Where an MCP call's UI is looked up when the view is read.
    tool_uis: Arc<dyn McpToolUis>,
}
pub fn clipped(value: &str, bytes: usize) -> String {
    let mut end = value.len().min(bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn outcome(value: ExecutionOutcome) -> ConversationMessageStatus {
    match value {
        ExecutionOutcome::Completed => ConversationMessageStatus::Completed,
        ExecutionOutcome::Cancelled => ConversationMessageStatus::Cancelled,
        ExecutionOutcome::OutputLimit
        | ExecutionOutcome::RequestLimit
        | ExecutionOutcome::Refused => ConversationMessageStatus::Failed,
    }
}
fn permission_deadline(
    deadline: &PermissionReviewDeadline,
    holding: bool,
) -> Option<ConversationPermissionDeadline> {
    // A restored deadline is a record of what was promised, not a promise this
    // process will keep. `holding` is a review this process's worker is still
    // waiting on: storage cannot record that, so the live read says so and
    // the card counts down. Any other unarmed deadline stays off the card.
    // `a_saved_deadline_counts_down_while_this_process_holds_the_review`.
    if !deadline.armed() && !holding {
        return None;
    }
    Some(ConversationPermissionDeadline {
        wait_ms: nessa_sdk::domain::agent_execution::permissions::wait_ms(deadline.wait()),
        due_at_ms: deadline.due_at_ms()?,
        effect: ConversationPermissionOptionEffect::from(deadline.effect()),
        reason: deadline.reason().to_owned(),
        stops_turn: deadline.stops_turn(),
    })
}
fn deadline_notice(effect: PermissionEffect, reason: &str) -> String {
    let verb = match effect {
        PermissionEffect::Allow => "Allowed automatically",
        PermissionEffect::Deny => "Denied automatically",
    };
    format!("{verb}: {reason}")
}
fn decline_notice(decline: &ReviewDecline, delivery: ReviewDeclineStage) -> String {
    let target = decline
        .declared()
        .map(|tool| format!(" labeled {tool}"))
        .unwrap_or_default();
    let reason = match decline.reason() {
        ReviewDeclineReason::ToolNotReviewable => "that tool is not reviewable here",
        ReviewDeclineReason::UnreadableRequest => "the request could not be read safely",
        ReviewDeclineReason::UnusableOptions => "the request offered no usable choices",
    };
    let delivery = match decline.outcome() {
        RefusalOutcome::Continue => match delivery {
            ReviewDeclineStage::Selected => {
                "Nessa has not confirmed writing the refusal to the agent."
            }
            ReviewDeclineStage::WriteConfirmed => "",
            ReviewDeclineStage::WriteUnconfirmed => {
                "Nessa could not confirm writing the refusal to the agent."
            }
            ReviewDeclineStage::WriteNotAttempted => {
                "Nessa did not attempt to write the refusal to the agent."
            }
        },
        // The stop is published once, before the turn finishes. A later
        // delivery stage would be output after that finish, so the write
        // stays in the audit and the panel keeps the reason.
        RefusalOutcome::EndTurn | RefusalOutcome::CloseSession => "",
    };
    let stopped = match decline.outcome() {
        RefusalOutcome::Continue => "",
        RefusalOutcome::EndTurn => " The turn stopped because of that refusal.",
        RefusalOutcome::CloseSession => " The session closed because of that refusal.",
    };
    let mut text = format!("Nessa declined a tool review{target} because {reason}.");
    if !delivery.is_empty() {
        text.push(' ');
        text.push_str(delivery);
    }
    text.push_str(stopped);
    text
}
// A provider report owns dispatched settlement. Before dispatch, the retained
// terminal result owns automatic-attachment failure, including its primary cause
// through queue audit/storage wrappers (startup_authentication tests).
fn authentication_refusal(record: &InvocationRecord) -> Option<&AgentError> {
    let mut error = match record.provider_report.as_ref() {
        Some(report) => report.provider_result()?.as_ref().err()?,
        None => record.result.as_ref()?.as_ref().err()?,
    };
    loop {
        match error {
            AgentError::AuthenticationRequired { .. } => return Some(error),
            AgentError::MultipleOperationFailures { first_error, .. } => error = first_error,
            AgentError::OperationAndCleanupFailure {
                operation_error, ..
            } => error = operation_error,
            AgentError::StorageAfterExecution {
                execution_result, ..
            } => {
                error = execution_result.as_ref().as_ref().err()?;
            }
            _ => return None,
        }
    }
}
/// A typed authentication refusal kept a separate cleanup or storage failure.
/// The view publishes this fact as `required_work_failed`; `error` still
/// carries the person-facing sentence and is not how a surface decides.
fn authentication_independent_failure(record: &InvocationRecord) -> bool {
    let Some(refusal) = authentication_refusal(record) else {
        return false;
    };
    let Some(final_error) = record
        .result
        .as_ref()
        .and_then(|result| result.as_ref().err())
    else {
        return false;
    };
    let report_error = record
        .provider_report
        .as_ref()
        .and_then(|report| report.clone().into_result().err());
    report_error.as_ref().is_some_and(|error| error != refusal) || refusal != final_error
}
fn failure_notice(record: &InvocationRecord) -> Option<String> {
    let final_error = record.result.as_ref()?.as_ref().err()?;
    let report = record.provider_report.as_ref();
    let provider_error = report
        .and_then(|report| report.provider_result())
        .and_then(|result| result.as_ref().err());
    if authentication_refusal(record).is_some() {
        return authentication_independent_failure(record).then(|| REQUIRED_WORK_FAILURE.into());
    }
    let Some(AgentError::Provider {
        diagnostic: Some(diagnostic),
        ..
    }) = provider_error
    else {
        return Some(REQUIRED_WORK_FAILURE.into());
    };
    let diagnostic = diagnostic.as_str().trim();
    if diagnostic.is_empty() {
        return Some(REQUIRED_WORK_FAILURE.into());
    }
    let mut notice = format!("{PROVIDER_FAILURE_PREFIX}{diagnostic}");
    let report_error = report.and_then(|report| report.clone().into_result().err());
    if report_error.as_ref() != provider_error || provider_error != Some(final_error) {
        if !matches!(notice.chars().last(), Some('.' | '!' | '?')) {
            notice.push('.');
        }
        notice.push(' ');
        notice.push_str(REQUIRED_WORK_FAILURE);
    }
    Some(notice)
}
/// Bound a retained snapshot through the same display owner as gateway reads.
///
/// The cache supplies the snapshot and status from one validated fold for `id`.
/// This mapping performs no semantic validation, source contact or dispatch.
/// The injected revision identity is transient presentation evidence; all action
/// capabilities are disabled. Display truncation does not change SDK completeness.
pub fn retained_view(
    id: &ConversationId,
    snapshot: Option<&SessionSnapshot>,
    status: CommittedStatus,
    revision: Uuid,
) -> ConversationView {
    let mut projection = Projection::from_snapshot_with_epoch(
        id.to_string(),
        ConversationCapabilities::read_only(),
        snapshot,
        HashSet::new(),
        revision,
        None,
    );
    projection.transcript_state(status.view_state().into());
    bound_view(projection.read())
}

impl Projection {
    pub fn new(
        id: String,
        capabilities: ConversationCapabilities,
        snapshot: Option<&SessionSnapshot>,
    ) -> Self {
        Self::from_snapshot(id, capabilities, snapshot, HashSet::new(), None)
    }

    fn from_snapshot(
        id: String,
        capabilities: ConversationCapabilities,
        snapshot: Option<&SessionSnapshot>,
        live_here: HashSet<String>,
        carried_unfinished: Option<HashSet<String>>,
    ) -> Self {
        Self::from_snapshot_with_epoch(
            id,
            capabilities,
            snapshot,
            live_here,
            Uuid::new_v4(),
            carried_unfinished,
        )
    }

    fn from_snapshot_with_epoch(
        id: String,
        capabilities: ConversationCapabilities,
        snapshot: Option<&SessionSnapshot>,
        live_here: HashSet<String>,
        epoch: Uuid,
        carried_unfinished: Option<HashSet<String>>,
    ) -> Self {
        let history_accepted = carried_unfinished.is_some() || snapshot.is_some();
        let unfinished_at_fold = match carried_unfinished {
            Some(unfinished) => unfinished,
            None => snapshot
                .map(|snapshot| unfinished_not_live(snapshot, &live_here))
                .unwrap_or_default(),
        };
        let mut projection = Self {
            epoch,
            revision: 0,
            committed_position: None,
            resolved_permissions: HashSet::new(),
            requested_permissions: HashMap::new(),
            cancelled_otherwise: HashSet::new(),
            answered_questions: HashSet::new(),
            terminal_executions: HashSet::new(),
            tool_parts: HashMap::new(),
            execution_hooks: HashMap::new(),
            live_here: live_here.clone(),
            unfinished_at_fold,
            history_accepted,
            tool_uis: Arc::new(NoMcpToolUis),
            view: ConversationView {
                selection: None,
                approval_mode_change: None,
                conversation_id: id,
                revision: String::new(),
                messages: Vec::new(),
                pending: Vec::new(),
                permissions: Vec::new(),
                questions: Vec::new(),
                tools: Vec::new(),
                runtime: None,
                capabilities,
                lifecycle: ConversationLifecycle {
                    phase: ConversationLifecyclePhase::Absent,
                    failure: None,
                    evidence_failure: None,
                },
                truncated: false,
                queue_complete: true,
                transcript_state: ConversationTranscriptState::NotLoaded,
                interaction_view_error: None,
                // Not the projection's: the service fills it from the summary.
                title: None,
                lease: snapshot
                    .and_then(|snapshot| snapshot.lease.as_ref())
                    .map(lease_view),
                artifacts: snapshot.map_or_else(Vec::new, |snapshot| {
                    snapshot.artifacts
                        [snapshot.artifacts.len().saturating_sub(MAX_VIEW_ARTIFACTS)..]
                        .iter()
                        .map(ConversationArtifact::from)
                        .collect()
                }),
                notes: snapshot.map_or_else(Vec::new, |snapshot| {
                    snapshot.notes[snapshot.notes.len().saturating_sub(MAX_VIEW_NOTES)..]
                        .iter()
                        .map(|note| ConversationNote::of(snapshot, note))
                        .collect()
                }),
                unreadable: Vec::new(),
                unreadable_truncated: false,
            },
        };
        if let Some(snapshot) = snapshot {
            projection.view.truncated = snapshot.invocations.len() > MAX_MESSAGES;
            for record in
                &snapshot.invocations[..snapshot.invocations.len().saturating_sub(MAX_MESSAGES)]
            {
                if live_here.contains(record.request.execution_id.as_str()) {
                    projection.active_interactions(record);
                }
            }
            for record in snapshot
                .invocations
                .iter()
                .rev()
                .take(MAX_MESSAGES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                let restarted = projection
                    .unfinished_at_fold
                    .contains(record.request.execution_id.as_str());
                projection.record(record, restarted);
            }
        }
        projection.bump();
        projection
    }

    /// Replace transcript fields from the SDK's committed snapshot. Live
    /// broadcast callbacks may prompt this read, but their provisional text or
    /// terminal status cannot become a replacement view. The SDK owns the
    /// lifecycle fold that produced `snapshot`; this method only bounds display.
    ///
    /// `active` is who may be offered an interaction. An execution admitted
    /// after this projection first accepted history stays running while its
    /// result is uncommitted, including when `active` is absent.
    /// `a_turn_admitted_after_open_stays_running_until_its_result`.
    pub fn replace_committed(
        &mut self,
        committed: &CommittedSession,
        order: &[ExecutionId],
        active: Option<&ExecutionId>,
    ) -> bool {
        self.replace(committed, order, active)
    }

    fn replace(
        &mut self,
        committed: &CommittedSession,
        order: &[ExecutionId],
        active: Option<&ExecutionId>,
    ) -> bool {
        let incarnation = committed.incarnation();
        let position = committed.position();
        let downloaded = committed.downloaded();
        let observed_head = committed.observed_head();
        let snapshot = committed.snapshot();
        if let Some((current_incarnation, current_position, current_downloaded, current_head)) =
            &self.committed_position
        {
            if current_incarnation != incarnation
                || position < *current_position
                || downloaded < *current_downloaded
                || observed_head < *current_head
            {
                return false;
            }
        }
        let active_set = active
            .map(|id| id.as_str().to_owned())
            .into_iter()
            .collect();
        let carried = self.continued_unfinished(active);
        let mut next = Self::from_snapshot(
            self.view.conversation_id.clone(),
            self.view.capabilities.clone(),
            snapshot,
            active_set,
            carried,
        )
        .with_tool_uis(self.tool_uis.clone());
        next.epoch = self.epoch;
        next.revision = self.revision;
        next.committed_position = Some((incarnation.into(), position, downloaded, observed_head));
        next.view.revision = self.view.revision.clone();
        next.view.transcript_state = self.view.transcript_state;
        next.view.selection = self.view.selection.clone();
        next.view.runtime = self.view.runtime.clone();
        next.view.lifecycle = self.view.lifecycle.clone();
        next.view.approval_mode_change = self.view.approval_mode_change.clone();
        next.view.title = self.view.title.clone();
        next.view.unreadable = committed
            .unreadable()
            .iter()
            .map(|part| ConversationUnreadable {
                id: part.id().to_owned(),
                after_execution_id: part.after_execution_id().map(str::to_owned),
            })
            .collect();
        next.view.unreadable_truncated = committed.unreadable_truncated();
        if let Some(snapshot) = snapshot {
            for id in order {
                let Some(record) = snapshot
                    .invocations
                    .iter()
                    .find(|record| record.request.execution_id == *id)
                else {
                    next.view.queue_complete = false;
                    continue;
                };
                let text = record.request.user_message.text_str();
                let mode = match record.submission {
                    SubmissionMode::Steering | SubmissionMode::BoundarySteering => {
                        ConversationPendingMode::Steering
                    }
                    SubmissionMode::Immediate | SubmissionMode::Queued => {
                        ConversationPendingMode::Queued
                    }
                };
                if next.view.pending.len() < 64 {
                    next.view.pending.push(ConversationPending {
                        execution_id: id.as_str().into(),
                        text: clipped(text, MAX_TEXT),
                        attachments: record
                            .request
                            .user_message
                            .images()
                            .iter()
                            .map(Into::into)
                            .collect(),
                        files: record
                            .request
                            .user_message
                            .files()
                            .iter()
                            .map(Into::into)
                            .collect(),
                        app: ConversationMessageApp::of(&record.request.user_message),
                        mode,
                    });
                } else {
                    next.view.queue_complete = false;
                    next.view.truncated = true;
                }
                if let Some(message) = next
                    .view
                    .messages
                    .iter_mut()
                    .find(|message| message.execution_id == id.as_str())
                {
                    message.status = ConversationMessageStatus::Queued;
                }
            }
        }
        let changed = serde_json::to_vec(&next.view).ok() != serde_json::to_vec(&self.view).ok();
        if changed {
            next.bump();
        }
        *self = next;
        true
    }

    /// The restart set for a later replacement. `None` until history has been
    /// accepted, so the first snapshot's unfinished records are restarts.
    /// Once accepted, an execution this process runs leaves the set: the gap
    /// after it stops being active is not a new restart. The other restored
    /// turns stay in the set.
    /// `a_turn_this_process_runs_stays_running_after_it_stops_being_active`,
    /// `running_one_restored_turn_leaves_the_other_unresolved`.
    fn continued_unfinished(&self, active: Option<&ExecutionId>) -> Option<HashSet<String>> {
        if !self.history_accepted {
            return None;
        }
        let mut unfinished = self.unfinished_at_fold.clone();
        if let Some(active) = active {
            unfinished.remove(active.as_str());
        }
        Some(unfinished)
    }

    pub fn transcript_state(&mut self, state: ConversationTranscriptState) {
        if self.view.transcript_state != state {
            self.view.transcript_state = state;
            self.bump();
        }
        if !matches!(
            state,
            ConversationTranscriptState::Complete | ConversationTranscriptState::CompleteEmpty
        ) {
            self.view.queue_complete = false;
            self.view.capabilities.queue = false;
            self.view.capabilities.steer = false;
            self.view.capabilities.permissions = false;
            self.view.permissions.clear();
            self.view.questions.clear();
            self.view.pending.clear();
        }
    }
    /// Look an MCP call's UI up in `tool_uis` when the view is read. A
    /// projection without one shows no UI.
    pub fn with_tool_uis(mut self, tool_uis: Arc<dyn McpToolUis>) -> Self {
        self.tool_uis = tool_uis;
        self
    }
    /// Inspect stored facts without bypassing their replacement/revision owner.
    ///
    /// ```compile_fail
    /// use nessa_protocol::conversation::projection::Projection;
    /// use nessa_protocol::conversation::view::ConversationCapabilities;
    /// let mut projection = Projection::new("conversation".into(), ConversationCapabilities::read_only(), None);
    /// projection.view().selection = None;
    /// ```
    pub fn view(&self) -> &ConversationView {
        &self.view
    }
    /// Replace the coherent durable selection and revise only when it changes.
    /// `selection_and_runtime_replacements_own_revision_without_transcript_output`.
    pub fn selection(&mut self, selection: ConversationSelectionView) {
        let selection = Some(selection);
        if self.view.selection != selection {
            self.view.selection = selection;
            self.bump();
        }
    }
    /// Publish an acknowledged committed mode through the selection owner.
    /// `publishing_mode_without_a_selection_does_not_invent_one`.
    pub fn approval_mode(&mut self, mode: ConversationApprovalMode) {
        if let Some(selection) = self.view.selection.clone() {
            self.selection(ConversationSelectionView {
                approval_mode: mode,
                ..selection
            });
        }
    }
    /// Replace runtime display facts through the same revision owner.
    /// `selection_and_runtime_replacements_own_revision_without_transcript_output`.
    pub fn runtime(&mut self, runtime: ConversationRuntime) {
        let runtime = Some(runtime);
        if self.view.runtime != runtime {
            self.view.runtime = runtime;
            self.bump();
        }
    }
    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.view.revision = format!("{}:{}", self.epoch, self.revision);
    }
    pub fn lifecycle(&mut self, lifecycle: ConversationLifecycle) {
        if self.view.lifecycle != lifecycle {
            self.view.lifecycle = lifecycle;
            self.bump();
        }
    }
    /// Replace the coherent capability snapshot and revise the view only when it changed.
    pub fn capabilities(&mut self, capabilities: ConversationCapabilities) {
        let mut capabilities = capabilities;
        if !matches!(
            self.view.transcript_state,
            ConversationTranscriptState::Complete | ConversationTranscriptState::CompleteEmpty
        ) {
            capabilities.queue = false;
            capabilities.steer = false;
            capabilities.permissions = false;
        }
        if self.view.capabilities != capabilities {
            self.view.capabilities = capabilities;
            self.bump();
        }
    }
    fn ensure_message(&mut self, id: &str) -> usize {
        if let Some(index) = self
            .view
            .messages
            .iter()
            .position(|message| message.execution_id == id)
        {
            return index;
        }
        if self.view.messages.len() == MAX_MESSAGES {
            let evicted = self.view.messages.remove(0);
            self.tool_parts.remove(&evicted.execution_id);
            self.execution_hooks.remove(&evicted.execution_id);
            self.view.truncated = true;
        }
        self.view.messages.push(ConversationMessage {
            authentication_required: None,
            required_work_failed: None,
            parts: Vec::new(),
            steering_offset: None,
            event_count: 0,
            retained_text: 0,
            execution_id: id.into(),
            user_text: String::new(),
            attachments: Vec::new(),
            files: Vec::new(),
            app: None,
            steering_target: None,
            status: ConversationMessageStatus::Running,
            error: None,
        });
        self.view.messages.len() - 1
    }

    pub fn injected(&mut self, id: &str, target: &str) {
        let index = self.ensure_message(id);
        self.view.messages[index].status = ConversationMessageStatus::Injected;
        self.view.messages[index].steering_target = Some(target.into());
        self.view.pending.retain(|value| value.execution_id != id);
        self.bump();
    }
    pub fn resolved_permission(&mut self, execution: &str, id: &str) {
        self.resolved_permissions
            .insert((execution.to_owned(), id.to_owned()));
        self.bump();
    }

    /// Nothing this execution asked or wanted reviewed is waiting. Called
    /// when its message is not running, and when it is running but is not
    /// the execution active here. Left in place, a stale entry still counted
    /// against the open limits and crowded out an ask somebody could answer.
    fn stop_waiting(&mut self, execution: &str) {
        self.live_here.remove(execution);
        self.view
            .questions
            .retain(|question| question.execution_id != execution);
    }

    /// Put one question into the view, unless it has already been answered or
    /// the execution that asked it is over.
    fn observe_question(&mut self, event: &ExecutionEvent) {
        let ExecutionUpdate::QuestionAsked { id, question } = event.update() else {
            return;
        };
        let execution = event.execution_id().as_str();
        if self
            .answered_questions
            .contains(&(execution.to_owned(), id.as_str().to_owned()))
            || self.terminal_executions.contains(execution)
            || self
                .view
                .questions
                .iter()
                .any(|open| open.question_id == id.as_str() && open.execution_id == execution)
        {
            return;
        }
        if self
            .view
            .questions
            .iter()
            .filter(|open| offered(&self.view, &open.execution_id))
            .count()
            >= MAX_OPEN_QUESTIONS
        {
            self.view.truncated = true;
            self.view.interaction_view_error = Some(
                "Some pending interactions exceed this display limit. Use Stop to cancel them."
                    .into(),
            );
            return;
        }
        let value = ConversationQuestion {
            execution_id: execution.to_owned(),
            question_id: id.as_str().to_owned(),
            message: question.message().to_owned(),
            questions: question
                .questions()
                .iter()
                .map(|asked| ConversationAsked {
                    key: asked.key().to_owned(),
                    prompt: asked.prompt().to_owned(),
                    header: asked.header().map(str::to_owned),
                    multi_select: asked.shape() == AnswerShape::Many,
                    free_text: asked.free_text(),
                    required: asked.required(),
                    options: asked
                        .options()
                        .iter()
                        .map(|option| ConversationAnswerOption {
                            value: option.value().to_owned(),
                            label: option.label().to_owned(),
                            description: option.description().map(str::to_owned),
                        })
                        .collect(),
                })
                .collect(),
        };
        self.view.questions.push(value);
    }
    fn observe_permission(&mut self, event: &ExecutionEvent) {
        let ExecutionUpdate::PermissionRequested {
            id: permission,
            tool_id,
            input,
            deadline,
            ..
        } = event.update()
        else {
            return;
        };
        let execution = event.execution_id().as_str();
        // A later notice still names a request this execution saw.
        self.requested_permissions.insert(
            (execution.to_owned(), permission.as_str().to_owned()),
            deadline.clone(),
        );
        let key = (execution.to_owned(), permission.as_str().to_owned());
        if self.resolved_permissions.contains(&key) || self.terminal_executions.contains(execution)
        {
            return;
        }
        // The review itself is not listed from history: whether it still
        // waits, and what it shows, is the live owner's to say
        // (`agent_review`). Its tool's row shows the input it was asked with.
        if let Some(tool) = self
            .view
            .tools
            .iter_mut()
            .find(|tool| tool.execution_id == execution && tool.tool_id == tool_id.as_str())
        {
            tool.input = if input.arguments_json.len() <= MAX_TOOL_INPUT_BYTES {
                input.arguments_json.clone()
            } else {
                String::new()
            };
        }
    }

    // The exact active record can precede the recent message window. Read only
    // its interactions; its prompt, attachments and output are never copied.
    fn active_interactions(&mut self, record: &InvocationRecord) {
        let restarted = self
            .unfinished_at_fold
            .contains(record.request.execution_id.as_str());
        if record_status(record, restarted) != ConversationMessageStatus::Running {
            return;
        }
        for event in &record.events {
            self.observe_interaction(event);
        }
    }
    fn observe_interaction(&mut self, event: &ExecutionEvent) {
        let execution = event.execution_id().as_str();
        match event.update() {
            ExecutionUpdate::PermissionRequested { .. } => self.observe_permission(event),
            ExecutionUpdate::QuestionAsked { .. } => self.observe_question(event),
            ExecutionUpdate::PermissionCancelled(cancellation) => {
                // Only the review's own published deadline settles it by
                // cancelling. An execution timeout shares the reason but has
                // a runtime origin, so its review was settled otherwise.
                let by_deadline = matches!(
                    cancellation.origin(),
                    CancellationOrigin::Deadline(deadline)
                        if self.deadline_settled(
                            execution,
                            cancellation.request().id().as_str(),
                            |published| published.decides_as(deadline),
                        )
                );
                if !by_deadline {
                    self.cancelled_otherwise.insert((
                        execution.to_owned(),
                        cancellation.request().id().as_str().to_owned(),
                    ));
                }
                self.resolved_permission(execution, cancellation.request().id().as_str());
            }
            ExecutionUpdate::DeadlineAnswered { id, effect, reason } => {
                if self.deadline_settled(execution, id.as_str(), |published| {
                    published.effect() == *effect && published.reason() == reason.as_str()
                }) {
                    self.resolved_permission(execution, id.as_str());
                }
            }
            ExecutionUpdate::QuestionClosed { id } => {
                self.answered_questions
                    .insert((execution.to_owned(), id.as_str().to_owned()));
                self.view.questions.retain(|open| {
                    open.execution_id != execution || open.question_id != id.as_str()
                });
            }
            ExecutionUpdate::Finished(_) => {
                self.terminal_executions.insert(execution.to_owned());
                self.stop_waiting(execution);
            }
            _ => {}
        }
    }

    /// Whether `permission` published a deadline that `settled_by` accepts,
    /// and nothing but that deadline ended it. A stale, duplicate, or
    /// contradictory notice in restored history is not one.
    fn deadline_settled(
        &self,
        execution: &str,
        permission: &str,
        settled_by: impl FnOnce(&PermissionReviewDeadline) -> bool,
    ) -> bool {
        let key = (execution.to_owned(), permission.to_owned());
        matches!(
            self.requested_permissions.get(&key),
            Some(Some(published)) if settled_by(published)
        ) && !self.cancelled_otherwise.contains(&key)
    }

    /// Render one observation from a validated committed SDK record.
    fn observe(&mut self, event: &ExecutionEvent) {
        let id = event.execution_id().as_str();
        let index = self.ensure_message(id);
        // Decided once, before this event changes anything: a notice is shown
        // for the deadline that settled its review, and only the first time.
        // The notice part is that record, so a replayed record shows it again.
        // The review's id becomes the notice's, and a notice id is the decimal
        // a review decline has; a saved history may hold any permission id, so
        // one that is not a notice id shows no notice rather than a view the
        // client refuses whole.
        let deadline_notice_shown = match event.update() {
            ExecutionUpdate::DeadlineAnswered {
                id: permission,
                effect,
                reason,
            } => {
                ReviewDeclineId::new(permission.as_str()).is_ok()
                    && self.deadline_settled(id, permission.as_str(), |published| {
                        published.effect() == *effect && published.reason() == reason.as_str()
                    })
                    && !self.view.messages[index].parts.iter().any(|part| {
                        part.kind == "local_notice" && part.notice_id == permission.as_str()
                    })
            }
            _ => false,
        };
        self.view
            .pending
            .retain(|pending| pending.execution_id != id);
        let offset = self.view.messages[index].event_count;
        self.view.messages[index].event_count += 1;
        let retained_text = self.view.messages[index].retained_text;
        let available = (MAX_TEXT * 2).saturating_sub(retained_text);
        if matches!(event.update(), ExecutionUpdate::Message(chunk) if chunk.as_str().len() > available)
        {
            self.view.truncated = true;
        }
        let part = match event.update() {
            ExecutionUpdate::Message(chunk) => Some(ConversationPart {
                message_id: chunk.message_id().map(str::to_owned),
                offset,
                kind: if chunk.kind() == MessageKind::Text {
                    "text"
                } else {
                    "thought"
                }
                .into(),
                text: clipped(chunk.as_str(), available),
                tool_id: String::new(),
                notice_id: String::new(),
            }),
            // One part per tool call, where it was first seen: a call's later
            // updates change its `tools` entry, not the turn's parts.
            ExecutionUpdate::Tool(update) => {
                let tool_id = clipped(update.id().as_str(), 256);
                let seen = self
                    .tool_parts
                    .get(id)
                    .is_some_and(|calls| calls.contains(&tool_id));
                (!seen).then(|| ConversationPart {
                    message_id: None,
                    offset,
                    kind: "tool".into(),
                    text: String::new(),
                    tool_id,
                    notice_id: String::new(),
                })
            }
            ExecutionUpdate::DeadlineAnswered {
                id: permission,
                effect,
                reason,
            } => {
                if !deadline_notice_shown {
                    None
                } else {
                    let text = deadline_notice(*effect, reason.as_str());
                    let bounded = clipped(&text, available);
                    if bounded.len() != text.len() {
                        self.view.truncated = true;
                    }
                    Some(ConversationPart {
                        message_id: None,
                        offset,
                        kind: "local_notice".into(),
                        text: bounded,
                        tool_id: String::new(),
                        notice_id: permission.as_str().into(),
                    })
                }
            }
            ExecutionUpdate::ReviewDeclined(observation) => {
                let notice_id = observation.id().as_str();
                let text = decline_notice(observation.decline(), observation.stage());
                if let Some(position) = self.view.messages[index]
                    .parts
                    .iter()
                    .position(|part| part.kind == "local_notice" && part.notice_id == notice_id)
                {
                    let previous = self.view.messages[index].parts[position].text.len();
                    let budget =
                        (MAX_TEXT * 2).saturating_sub(retained_text.saturating_sub(previous));
                    let bounded = clipped(&text, budget);
                    if bounded.len() != text.len() {
                        self.view.truncated = true;
                    }
                    let next = bounded.len();
                    self.view.messages[index].parts[position].text = bounded;
                    self.view.messages[index].retained_text = retained_text - previous + next;
                    None
                } else {
                    let bounded = clipped(&text, available);
                    if bounded.len() != text.len() {
                        self.view.truncated = true;
                    }
                    Some(ConversationPart {
                        message_id: None,
                        offset,
                        kind: "local_notice".into(),
                        text: bounded,
                        tool_id: String::new(),
                        notice_id: notice_id.into(),
                    })
                }
            }
            _ => None,
        };
        if let Some(part) = part {
            let message = &mut self.view.messages[index];
            if message.parts.len() < 512 && message.retained_text + part.text.len() <= MAX_TEXT * 2
            {
                message.retained_text += part.text.len();
                if part.kind == "tool" {
                    self.tool_parts
                        .entry(id.to_owned())
                        .or_default()
                        .insert(part.tool_id.clone());
                }
                message.parts.push(part);
            } else {
                self.view.truncated = true;
            }
        }
        self.observe_interaction(event);
        match event.update() {
            ExecutionUpdate::Message(_) => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
            }
            ExecutionUpdate::Finished(result) => {
                self.view.messages[index].status = outcome(*result);
            }
            ExecutionUpdate::QuestionAsked { .. } | ExecutionUpdate::PermissionRequested { .. } => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
            }
            ExecutionUpdate::PermissionCancelled(_) | ExecutionUpdate::QuestionClosed { .. } => {}
            ExecutionUpdate::DeadlineAnswered { .. } => {
                if deadline_notice_shown {
                    self.view.messages[index].status = ConversationMessageStatus::Running;
                }
            }
            ExecutionUpdate::ReviewDeclined(observation) => {
                if observation.decline().outcome() == RefusalOutcome::Continue {
                    self.view.messages[index].status = ConversationMessageStatus::Running;
                }
            }
            ExecutionUpdate::Tool(update) => {
                self.view.messages[index].status = ConversationMessageStatus::Running;
                // The choice of the process start that produced the call,
                // saved with its event. Never the choice in effect now.
                let hooks = event.native_hooks().map(|allowed| {
                    let agreed = match self.execution_hooks.get(id) {
                        None => Some(allowed),
                        Some(Some(saved)) if *saved == allowed => Some(allowed),
                        Some(_) => None,
                    };
                    self.execution_hooks.insert(id.to_owned(), agreed);
                    if agreed.is_none() {
                        // Saved events of one execution disagree. Showing
                        // either would be a guess, so none of its calls
                        // shows one.
                        for tool in self
                            .view
                            .tools
                            .iter_mut()
                            .filter(|tool| tool.execution_id == id)
                        {
                            tool.native_hooks = None;
                        }
                    }
                    agreed
                });
                let position = self.view.tools.iter().position(|tool| {
                    tool.execution_id == id && tool.tool_id == update.id().as_str()
                });
                let tool = if let Some(position) = position {
                    &mut self.view.tools[position]
                } else {
                    if self.view.tools.len() == MAX_TOOLS {
                        self.view.tools.remove(0);
                        self.view.truncated = true;
                    }
                    self.view.tools.push(ConversationTool {
                        input: String::new(),
                        details: String::new(),
                        execution_id: id.into(),
                        tool_id: clipped(update.id().as_str(), 256),
                        title: "Tool".into(),
                        kind: String::new(),
                        status: "pending".into(),
                        mcp: None,
                        structured_content: None,
                        native_hooks: None,
                    });
                    self.view.tools.last_mut().unwrap()
                };
                if let Some(agreed) = hooks {
                    tool.native_hooks = agreed.map(|allowed| {
                        if allowed {
                            NativeHooksChoice::On
                        } else {
                            NativeHooksChoice::Off
                        }
                    });
                }
                if let Some(title) = update.title() {
                    tool.title = clipped(title, 512);
                }
                // What a call does is the provider's own word for it. A panel
                // cannot read it off the title — "grep -l" and "Find `**/*`"
                // are both searches and neither says so.
                if let Some(kind) = update.kind() {
                    tool.kind = match kind {
                        ToolKind::Read => "read",
                        ToolKind::Edit => "edit",
                        ToolKind::Search => "search",
                        ToolKind::Fetch => "fetch",
                        ToolKind::Execute => "execute",
                        ToolKind::Think => "think",
                        ToolKind::Delete => "delete",
                        ToolKind::Move => "move",
                        ToolKind::SwitchMode => "switch_mode",
                        ToolKind::Other => "other",
                    }
                    .into();
                }
                if let Some(mcp) = update.mcp_tool() {
                    let carried = update
                        .mcp_arguments()
                        .map(|arguments| arguments.as_str().to_owned());
                    // An update that does not repeat the arguments leaves the
                    // ones already carried, and only for the same server and
                    // tool: a different identity is a different call.
                    let arguments_json = carried.or_else(|| {
                        tool.mcp.as_ref().and_then(|existing| {
                            (existing.server == mcp.server() && existing.tool == mcp.tool())
                                .then(|| existing.arguments_json.clone())
                                .flatten()
                        })
                    });
                    tool.mcp = Some(ConversationMcpTool {
                        server: mcp.server().into(),
                        tool: mcp.tool().into(),
                        // Filled when the view is read, from the tools as
                        // last listed: see `read_with_mode_change`.
                        resource_uri: None,
                        arguments_json,
                    });
                } else if let Some(arguments) = update.mcp_arguments() {
                    // The identity was named on an earlier update. This one
                    // carries the arguments and does not repeat it, so they
                    // join the identity already on this call. With no identity
                    // yet there is no server to show them under.
                    if let Some(existing) = tool.mcp.as_mut() {
                        existing.arguments_json = Some(arguments.as_str().to_owned());
                    }
                }
                if let Some(content) = update.content() {
                    // The structured result is read apart from the text, so a
                    // long text cannot push it out; the last one reported is
                    // the result's. Replaced with the content, as details are.
                    // Past the bound it is left out, never cut: JSON cut short
                    // is not JSON, and its text is in the details either way.
                    let structured = content.iter().rev().find_map(|item| match item.view() {
                        ToolContentView::Structured(json) => Some(json),
                        _ => None,
                    });
                    tool.structured_content = structured
                        .filter(|json| json.len() <= MAX_STRUCTURED_CONTENT_BYTES)
                        .map(str::to_owned);
                    let mut details = String::new();
                    for item in content {
                        let text = match item.view() {
                            ToolContentView::Structured(_) => continue,
                            ToolContentView::Text(text) => clipped(text, 16384),
                            ToolContentView::Diff { path, old, new } => format!(
                                "File: {}\nBefore:\n{}\nAfter:\n{}",
                                path.as_str(),
                                clipped(old.unwrap_or("(not provided)"), 7000),
                                clipped(new, 7000)
                            ),
                        };
                        if !details.is_empty() {
                            details.push('\n');
                        }
                        details.push_str(&text);
                        if details.len() > 16000 {
                            details = clipped(&details, 16000);
                            details.push_str("\n[Output truncated]");
                            break;
                        }
                    }
                    tool.details = details;
                }
                if let Some(status) = update.status() {
                    tool.status = match status {
                        ToolStatus::Pending => "pending",
                        ToolStatus::Running => "running",
                        ToolStatus::Completed => "completed",
                        ToolStatus::Failed => "failed",
                    }
                    .into();
                }
            }
        }
        self.bump();
    }
    fn record(&mut self, record: &InvocationRecord, restarted: bool) {
        let id = record.request.execution_id.as_str();
        let index = self.ensure_message(id);
        self.view.messages[index].steering_target = record
            .scheduling
            .iter()
            .find(|event| event.stage == InvocationStage::Injected)
            .and_then(|event| event.target.as_ref())
            .map(|target| target.as_str().to_owned());
        self.view.messages[index].user_text =
            clipped(record.request.user_message.text_str(), MAX_TEXT);
        self.view.messages[index].attachments = record
            .request
            .user_message
            .images()
            .iter()
            .map(Into::into)
            .collect();
        self.view.messages[index].files = record
            .request
            .user_message
            .files()
            .iter()
            .map(Into::into)
            .collect();
        self.view.messages[index].app = ConversationMessageApp::of(&record.request.user_message);
        self.view.messages[index].parts.clear();
        self.view.messages[index].retained_text = 0;
        self.tool_parts.remove(id);
        self.execution_hooks.remove(id);
        self.view.messages[index].event_count = 0;
        self.view.messages[index].steering_offset = record.target_event_offset;
        self.view.messages[index].status = ConversationMessageStatus::Running;
        self.view.messages[index].error = None;
        for event in &record.events {
            self.observe(event);
        }
        let index = self.ensure_message(id);
        self.view.messages[index].status = record_status(record, restarted);
        if self.view.messages[index].status == ConversationMessageStatus::Injected {
            if let Some(target) = self.view.messages[index].steering_target.clone() {
                self.injected(id, &target);
            }
        }
        self.view.messages[index].error = failure_notice(record);
        self.view.messages[index].authentication_required = (self.view.messages[index].status
            == ConversationMessageStatus::Failed)
            .then_some(record)
            .and_then(authentication_refusal)
            .map(|_| true);
        self.view.messages[index].required_work_failed = (self.view.messages[index].status
            == ConversationMessageStatus::Failed
            && authentication_independent_failure(record))
        .then_some(true);
        // An ask is offered only while its execution is the one running here.
        // A settled or restarted message drops it, and so does a turn that is
        // running but not active: the gap around liveness is not an open ask.
        // `an_ask_whose_closure_never_reached_storage_is_not_offered_after_restart`,
        // `a_turn_admitted_after_open_stays_running_until_its_result`.
        let running_here = self.view.messages[index].status == ConversationMessageStatus::Running
            && self.live_here.contains(id);
        if !running_here {
            self.stop_waiting(id);
        }
        self.view.pending.retain(|value| value.execution_id != id);
    }

    pub fn read(&self) -> ConversationView {
        self.read_with_mode_change(None)
    }
    /// Where in the committed history the view was folded through: the
    /// position this projection last accepted, never a read it refused as
    /// older (`replace_committed`). `None` before any history was accepted.
    pub fn committed_cursor(&self) -> Option<CommittedCursor> {
        self.committed_position
            .as_ref()
            .map(
                |(incarnation, position, _, observed_head)| CommittedCursor {
                    incarnation: incarnation.clone(),
                    position: *position,
                    observed_head: *observed_head,
                },
            )
    }
    pub fn read_with_mode_change(
        &self,
        change: Option<ConversationApprovalModeChangeView>,
    ) -> ConversationView {
        let mut view = self.view.clone();
        if let Some(change) = change {
            let digest = Sha256::digest(
                format!(
                    "{}:{}:{:?}",
                    change.request_id, change.requested_mode, change.status
                )
                .as_bytes(),
            );
            view.revision = format!("{}:mode:{:x}", view.revision, digest);
            view.approval_mode_change = Some(change);
        }
        // Each MCP call's UI, from its server's tools as last listed: a tool's
        // declaration is the server's, not the call's, and no harness passes it
        // through ACP (ADR 344). Filled here, before the byte budget below, and
        // folded into the revision, so a window holding this revision holds
        // these URIs: a list read after the call still reaches the window.
        let mut drawn = Sha256::new();
        let mut any = false;
        let conversation = ConversationId::new(&view.conversation_id).ok();
        for tool in &mut view.tools {
            let Some(mcp) = &mut tool.mcp else { continue };
            mcp.resource_uri = McpTool::new(mcp.server.as_str(), mcp.tool.as_str())
                .ok()
                .zip(conversation.as_ref())
                .and_then(|(call, conversation)| self.tool_uis.resource_uri(conversation, &call))
                .map(|uri| uri.as_str().to_owned());
            if let Some(uri) = &mcp.resource_uri {
                any = true;
                for part in [&tool.execution_id, &tool.tool_id, uri] {
                    drawn.update((part.len() as u64).to_be_bytes());
                    drawn.update(part.as_bytes());
                }
            }
        }
        if any {
            view.revision = format!("{}:ui:{:x}", view.revision, drawn.finalize());
        }
        // A review or an ask is offered only while its execution is running:
        // nothing else is waiting on an answer, and the client refuses a view
        // that says otherwise — the whole conversation would fail to load.
        // Enforced here, where the view is handed out, rather than at each of
        // the many places a message's status changes; any one of them missing
        // it was enough to make a conversation unreadable. An execution whose
        // message is not in the view is left as the client allows: only in a
        // view that says it was truncated.
        let questions = view
            .questions
            .iter()
            .filter(|question| offered(&view, &question.execution_id))
            .cloned()
            .collect();
        view.questions = questions;
        view
    }
}

/// One entry of a view's review list: `key`, who asked, and the start of
/// `tool_name`, clipped at a character so its encoding (without the quotes)
/// takes at most [`MAX_REVIEW_ENTRY_TOOL_NAME_BYTES`]. The entry, encoded
/// with its comma, takes at most [`MAX_REVIEW_ENTRY_BYTES`].
pub fn review_entry(
    key: &str,
    origin: ConversationReviewKind,
    tool_name: &str,
) -> ConversationReviewEntry {
    let mut end = tool_name.len();
    loop {
        while !tool_name.is_char_boundary(end) {
            end -= 1;
        }
        let encoded = serde_json::to_string(&tool_name[..end]).map_or(usize::MAX, |e| e.len());
        if encoded.saturating_sub(2) <= MAX_REVIEW_ENTRY_TOOL_NAME_BYTES {
            break;
        }
        end -= 1;
    }
    ConversationReviewEntry {
        review: key.to_owned(),
        origin,
        tool_name: tool_name[..end].to_owned(),
    }
}

/// What a person is shown of one review the agent's live invocation waits
/// on, from that owner's own record: none when it offers a choice reaching
/// past this one request, which no surface can show as what it is.
pub fn agent_review(review: &PendingReview) -> Option<ConversationPermission> {
    // An option says what it decides (`effect`), not how far it reaches. Live
    // offers decide one request (`once_only`, and the ACP parser admits no
    // persistent kind); one that reached further would read as one that does
    // not, so it is not offered.
    if review
        .options
        .choices()
        .iter()
        .any(|option| *option.decision().scope() != PermissionScope::request())
    {
        return None;
    }
    let content = &review.content;
    Some(ConversationPermission {
        execution_id: review.execution_id.as_str().into(),
        permission_id: review.id.as_str().into(),
        tool_id: review.tool_id.as_str().into(),
        title: clipped(content.title().unwrap_or("Tool permission"), 512),
        tool_name: content.tool_name().into(),
        arguments_json: content.arguments_json().into(),
        origin: ConversationPermissionOrigin::Harness,
        ask: ConversationPermissionAsk::Tool,
        // The live owner holds it, so its deadline is that worker's promise.
        deadline: content
            .deadline()
            .and_then(|deadline| permission_deadline(deadline, true)),
        options: review
            .options
            .choices()
            .iter()
            .map(|option| ConversationPermissionOption {
                id: option.id().as_str().into(),
                label: option.label().into(),
                effect: ConversationPermissionOptionEffect::from(option.decision().effect()),
            })
            .collect(),
    })
}

/// What a view bounded to its size says of the interactions it left out.
const UNSHOWN_INTERACTIONS: &str =
    "Some pending interactions exceed this display limit. Use Stop to cancel them.";

/// Bound the complete service result after its metadata and authority additions.
pub fn bound_view(view: ConversationView) -> ConversationView {
    bound_view_within(view, MAX_VIEW_BYTES, true)
}

/// As [`bound_view`], within `limit` bytes: the view that leaves room for
/// what is added beside it after. Its interactions — the agent's reviews
/// and questions — are given up for that room only when `interactions`
/// says so; otherwise it stops short of them, over `limit` if need be.
pub fn bound_view_within(
    mut view: ConversationView,
    limit: usize,
    interactions: bool,
) -> ConversationView {
    // The binding bounds actual admitted ACP asks. Custom backends can
    // retain other valid histories, so the encoded display owner gives up
    // whole interactions with an explicit notice rather than cutting choices.
    while serde_json::to_vec(&view).map_or(usize::MAX, |bytes| bytes.len()) > limit {
        // The structured result is optional display data. Preserve the history
        // and its text before giving up a message or interaction.
        if let Some(tool) = view
            .tools
            .iter_mut()
            .find(|tool| tool.structured_content.is_some())
        {
            tool.structured_content = None;
            continue;
        }
        // Only the interactions are left, and they are not to be given up:
        // nothing is removed, so the view is not truncated either.
        if !interactions && !gives_up_anything_but_interactions(&view) {
            break;
        }
        // Arguments are what an app is told its call with. They yield once
        // the history has nothing left to give, and before the tool row
        // itself does: a row removed here would leave the desktop on the
        // previous state. Dropping them does not say the view was truncated.
        // The tool and its status stay, as when a structured result is left
        // out.
        if !transcript_can_yield(&view) {
            if let Some(tool) = view.tools.iter_mut().find(|tool| {
                tool.mcp
                    .as_ref()
                    .is_some_and(|mcp| mcp.arguments_json.is_some())
            }) {
                if let Some(mcp) = &mut tool.mcp {
                    mcp.arguments_json = None;
                }
                continue;
            }
        }
        view.truncated = true;
        if view.messages.len() > 1 {
            view.messages.remove(0);
        } else if view
            .messages
            .first()
            .is_some_and(|message| !message.parts.is_empty())
        {
            // The length leaves with the part. A returned view otherwise claims
            // bytes for text it no longer contains.
            // `the_text_budget_keeps_a_running_total`.
            if let Some(removed) = view.messages[0].parts.pop() {
                view.messages[0].retained_text -= removed.text.len();
            }
        } else if let Some(message) = view
            .messages
            .first_mut()
            .filter(|message| !message.user_text.is_empty())
        {
            message.user_text = clipped(&message.user_text, message.user_text.len() / 2);
        } else if let Some(message) = view
            .messages
            .first_mut()
            .filter(|message| !message.files.is_empty() || !message.attachments.is_empty())
        {
            // What the message linked is shown by name, and a path can be
            // long; past the budget the view says it left some out rather
            // than break its bound. Whole interactions yield afterward
            // with an explicit display-limit notice.
            if message.files.pop().is_none() {
                message.attachments.pop();
            }
        } else if !view.notes.is_empty() {
            // The oldest listed note yields once the transcript has nothing
            // left to give: the conversation still holds it.
            view.notes.remove(0);
        } else if !view.artifacts.is_empty() {
            // The oldest listed artifact yields once the transcript has
            // nothing left to give: the conversation still holds it.
            view.artifacts.remove(0);
        } else if !view.tools.is_empty() {
            view.tools.remove(0);
        } else if !view.pending.is_empty() {
            view.pending.remove(0);
            view.queue_complete = false;
        } else if !view.questions.is_empty() {
            // The reviews listed are never given up: each entry is small and
            // of fixed size, and room for the most a view can list is kept
            // (`MAX_REVIEW_LIST_BYTES`).
            view.questions.pop();
            view.interaction_view_error = Some(UNSHOWN_INTERACTIONS.into());
        } else {
            break;
        }
    }
    view
}

/// Whether `view`'s transcript still has a message, part, or linked file a
/// bound may give up.
fn transcript_can_yield(view: &ConversationView) -> bool {
    view.messages.len() > 1
        || view.messages.first().is_some_and(|message| {
            !message.parts.is_empty()
                || !message.user_text.is_empty()
                || !message.files.is_empty()
                || !message.attachments.is_empty()
        })
}

/// Whether `view` still has anything a bound may give up before its
/// interactions: its transcript, notes, artifacts, tool calls or queue.
fn gives_up_anything_but_interactions(view: &ConversationView) -> bool {
    transcript_can_yield(view)
        || !view.notes.is_empty()
        || !view.artifacts.is_empty()
        || !view.tools.is_empty()
        || !view.pending.is_empty()
}

/// Whether `view` may offer an ask or review from `execution`: only while its
/// message is running, or — where the message is no longer in the view — only
/// in a view that says it was truncated. The client's own rule, in one place,
/// used both to decide what is offered and what counts against the limits.
fn offered(view: &ConversationView, execution: &str) -> bool {
    view.messages
        .iter()
        .find(|message| message.execution_id == execution)
        .map_or(view.truncated, |message| {
            message.status == ConversationMessageStatus::Running
        })
}

#[cfg(test)]
#[path = "../../tests/conversation/projection.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/conversation/tool_parts.rs"]
mod tool_part_tests;
