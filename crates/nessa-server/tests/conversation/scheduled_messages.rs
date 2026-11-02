//! Scheduler application and durable-adapter recovery regressions.

use super::super::mutation::SubmissionPermit;
use super::fold_catalogue_page;
use crate::conversation::application::ConversationError;
use tokio::sync::Notify;

#[test]
fn enabling_the_preference_reads_past_the_list_bound() {
    let mut cursor = None;
    let mut ids = Vec::new();
    let first: Vec<_> = (0..500).collect();
    let second: Vec<_> = (500..600).collect();
    assert!(!fold_catalogue_page(&mut cursor, false, Some(1_u32), first, &mut ids).unwrap());
    assert!(!fold_catalogue_page(&mut cursor, false, Some(2), second, &mut ids).unwrap());
    assert!(fold_catalogue_page(&mut cursor, true, None, std::iter::empty(), &mut ids).unwrap());
    assert_eq!(ids.len(), 600);
    assert_eq!(ids[0], 0);
    assert_eq!(ids[599], 599);
}

#[test]
fn an_unfinished_catalogue_page_without_a_cursor_fails_the_walk() {
    let mut cursor = None::<u32>;
    let mut ids = Vec::new();
    let error = fold_catalogue_page(&mut cursor, false, None, ["older"], &mut ids);
    assert!(matches!(error, Err(ConversationError::Unavailable)));
}

#[test]
fn a_catalogue_cursor_that_does_not_move_fails_the_walk() {
    let mut cursor = None;
    let mut ids = Vec::new();
    assert!(!fold_catalogue_page(&mut cursor, false, Some(1_u32), ["a"], &mut ids).unwrap());
    let error = fold_catalogue_page(&mut cursor, false, Some(1), ["a"], &mut ids);
    assert!(matches!(error, Err(ConversationError::Unavailable)));
}

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nessa_auth::application::ports::{AccessError, Clock};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_local_database::rusqlite::Connection;
use nessa_sdk::application::agent_execution::commands::{
    CreationFuture, CreationReceipt, CreationStorage, CreationStorageError, CreationStorageLease,
    MutationFailure, MutationOutcome, MutationReceipt, MutationStage,
};
use nessa_sdk::application::agent_execution::sessions::{
    SessionStorage, StorageError, StorageFuture,
};
use nessa_sdk::domain::agent_execution::executions::Initiation;
use nessa_sdk::domain::agent_execution::sessions::SessionId;

use super::{
    renew_while_held, schedule_caller, HistoryGeneration, PlanToken, PreferenceCommandOutcome,
    PreferenceCommandReceipt, PreferenceReading, PreferenceUpdate, RecordedPreference,
    ScheduleActor, ScheduleDraft, ScheduleSettlement, ScheduledCommandEvidence, ScheduledMessage,
    ScheduledMessageStore, SelectionChange, SelectionRequest, FENCE_LEASE_MS, FENCE_RENEW_EVERY_MS,
    FENCE_RENEW_RETRY_MS, SCHEDULE_RETRY_MS,
};
use crate::conversation::application::{
    ConversationCaller, ConversationDependencies, ConversationFuture, ConversationLimits,
    ConversationListing, ConversationModelApplication, ConversationModelRequest,
    ConversationModelRequestState, ConversationModelSelection, ConversationService,
    ListedConversation, ListedConversations, ObservationCursor, ObservedConversations, Reader,
    RequestedConversation, SubmissionMode, SubmittedMessage, UsageLimitPreferenceDecision,
    UsageLimitPreferenceFacts, UsageLimitPreferenceGrant, UsageLimitPreferencePolicyEvaluator,
};
use crate::conversation::domain::ContinuationCause;
use crate::conversation::infrastructure::LocalScheduledMessages;
use crate::conversation_test_support::{
    only, AcceptingCreationAudit, AcceptingDeletionAudit, InMemoryStorage, MemoryRepository,
    MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit, TestClock,
    DELETION_BUDGETS,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationModelId, ConversationSummary,
};
use nessa_protocol::conversation::view::ConversationModelChangeEffect;
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use std::path::Path;

fn caller_as(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "desktop".into(),
        action_id: action.into(),
    }
}

fn caller() -> ConversationCaller {
    caller_as("toggle")
}

fn facts(role: &str) -> UsageLimitPreferenceFacts {
    UsageLimitPreferenceFacts {
        principal_id: "person".into(),
        organization_id: "org".into(),
        role: role.into(),
        active: true,
        grants: vec![UsageLimitPreferenceGrant {
            action: "credential.manage".into(),
            resource_id: "gateway".into(),
            organization_id: "org".into(),
        }],
        gateway_id: "gateway".into(),
        gateway_organization: "org".into(),
    }
}

fn service(listing: Arc<dyn ConversationListing>, clock: Arc<dyn Clock>) -> ConversationService {
    service_over(Arc::new(MemoryRepository::default()), listing, clock)
}

fn service_over(
    metadata: Arc<MemoryRepository>,
    listing: Arc<dyn ConversationListing>,
    clock: Arc<dyn Clock>,
) -> ConversationService {
    service_stored(metadata, listing, clock, Arc::new(InMemoryStorage::new()))
}

fn service_stored(
    metadata: Arc<MemoryRepository>,
    listing: Arc<dyn ConversationListing>,
    clock: Arc<dyn Clock>,
    storage: Arc<dyn SessionStorage>,
) -> ConversationService {
    let provider = Arc::new(ProviderFactory::default());
    ConversationService::new(
        ConversationDependencies {
            input_files: None,
            access_policy: crate::conversation_test_support::access_policy(),
            agents: only(Arc::new(Provider::new(provider))),
            storage,
            metadata,
            mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing,
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: crate::conversation_test_support::claude_erasers(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: crate::conversation_test_support::runtime_message_commit_clock(),
            clock,
            environment: crate::conversation_test_support::in_process_environment(),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap()
}

/// Allows an active admin and refuses every other role. The Cedar bundle is
/// tested beside its policies; this double is what the service is shown.
struct AdminOnly;

impl UsageLimitPreferencePolicyEvaluator for AdminOnly {
    fn evaluate(
        &self,
        facts: &UsageLimitPreferenceFacts,
    ) -> Result<UsageLimitPreferenceDecision, AccessError> {
        if facts.role == "admin" && facts.active {
            Ok(UsageLimitPreferenceDecision::Allow {
                policies: vec!["admin".into()],
            })
        } else {
            Ok(UsageLimitPreferenceDecision::Deny {
                policies: vec!["member".into()],
            })
        }
    }
}

/// Preference changes and outcome timestamps, without a database.
struct RecordingStore {
    enabled: AtomicBool,
    revision: AtomicU64,
    /// False only for an enable this store has not settled or reverted.
    settled: AtomicBool,
    /// `(enabled, person)` for each change. An unchanged write is absent.
    changes: Mutex<Vec<(bool, bool)>>,
    outcomes: Mutex<Vec<(String, u64)>>,
    /// `(allowed, policy ids)` for each Cedar decision that committed.
    decisions: Mutex<Vec<(bool, Vec<String>)>>,
    fail_decision: AtomicBool,
    submits: AtomicUsize,
    opens: AtomicUsize,
    /// Conversation ids passed to `open`, in order.
    opened: Mutex<Vec<String>>,
    /// While set, `open` fails and a delivery or model reconciliation cannot commit.
    fail_open: AtomicBool,
    /// While set, a delivery pass finds nothing due instead of failing the read.
    idle_delivery: AtomicBool,
    /// `preference` fails this many times, then returns the stored boolean.
    preference_failures: AtomicUsize,
    /// When set, delivery claims `due_row` and a submit retry is held until `retry_at_ms`.
    submit_attempts: AtomicBool,
    due_row: Mutex<Option<ScheduledMessage>>,
    retry_at_ms: AtomicU64,
    retry_failures: AtomicU32,
    /// `(execution id, failures, retry_at_ms)` for each committed submit hold.
    defers: Mutex<Vec<(String, u32, u64)>>,
    reconciles: Mutex<HashMap<String, (u32, u64)>>,
    /// The next reconcile-debt write fails. The service keeps the debt in memory.
    fail_reconcile_note: AtomicBool,
    /// Receipts that committed, keyed by request id.
    commands: Mutex<HashMap<String, PreferenceCommandReceipt>>,
    /// The request that installed the current unsettled enable.
    installed_request: Mutex<String>,
    /// The next settlement loses, as a concurrent disable would.
    lose_settle: AtomicBool,
    /// The next revert returns an error and writes nothing.
    fail_revert: AtomicBool,
    /// The next this many outcome writes fail and leave the row due.
    fail_outcome: AtomicUsize,
    /// The next outcome write reports the row gone and writes nothing.
    outcome_missing: AtomicBool,
    /// The next submit hold returns false and writes no retry time.
    fail_defer: AtomicBool,
    /// The next `begin_submit` returns an error and does not claim the row.
    fail_begin: AtomicBool,
    /// `status` returns this. The outer `None` means the read fails.
    /// The inner `None` means the row is gone.
    known_status: Mutex<Option<Option<String>>>,
}

impl RecordingStore {
    fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            revision: AtomicU64::new(0),
            settled: AtomicBool::new(true),
            changes: Mutex::new(Vec::new()),
            outcomes: Mutex::new(Vec::new()),
            decisions: Mutex::new(Vec::new()),
            fail_decision: AtomicBool::new(false),
            submits: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
            opened: Mutex::new(Vec::new()),
            fail_open: AtomicBool::new(true),
            idle_delivery: AtomicBool::new(false),
            preference_failures: AtomicUsize::new(0),
            submit_attempts: AtomicBool::new(false),
            due_row: Mutex::new(None),
            retry_at_ms: AtomicU64::new(0),
            retry_failures: AtomicU32::new(0),
            defers: Mutex::new(Vec::new()),
            reconciles: Mutex::new(HashMap::new()),
            fail_reconcile_note: AtomicBool::new(false),
            commands: Mutex::new(HashMap::new()),
            installed_request: Mutex::new(String::new()),
            lose_settle: AtomicBool::new(false),
            fail_revert: AtomicBool::new(false),
            fail_outcome: AtomicUsize::new(0),
            outcome_missing: AtomicBool::new(false),
            fail_defer: AtomicBool::new(false),
            fail_begin: AtomicBool::new(false),
            known_status: Mutex::new(None),
        }
    }

    /// Settle `revision` when it is still the enabled preference.
    fn settle_revision(&self, revision: u64) -> bool {
        if self.lose_settle.swap(false, Ordering::SeqCst) {
            return false;
        }
        if self.revision.load(Ordering::SeqCst) == revision && self.enabled.load(Ordering::SeqCst) {
            self.settled.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
}

fn refused<T: Send + 'static>() -> ConversationFuture<'static, T> {
    Box::pin(async { Err(ConversationError::Unavailable) })
}

impl ScheduledMessageStore for RecordingStore {
    fn command_evidence(
        &self,
        conversation: &str,
        execution: &str,
    ) -> ConversationFuture<'_, Option<ScheduledCommandEvidence>> {
        let row = self.due_row.lock().unwrap().clone();
        let result = match row
            .filter(|row| row.conversation == conversation && row.execution_id == execution)
        {
            Some(row) => row
                .command_evidence
                .map(Some)
                .ok_or(ConversationError::Unavailable),
            None => Ok(None),
        };
        Box::pin(async move { result })
    }

    fn invalidated_selection(&self, _: &str) -> ConversationFuture<'_, Option<SelectionChange>> {
        Box::pin(async { Ok(None) })
    }

    // This double exercises unrelated store behavior; model invalidation has
    // dedicated tests against LocalScheduledMessages.
    fn invalidate_usage_limit(
        &self,
        _: SelectionRequest<'_>,
        _: &str,
        _: Option<&str>,
        _: &crate::conversation::application::PlanToken,
        _: crate::conversation::application::ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(true) })
    }
    fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
        if self.preference_failures.load(Ordering::SeqCst) > 0 {
            self.preference_failures.fetch_sub(1, Ordering::SeqCst);
            return refused();
        }
        let enabled = self.enabled.load(Ordering::SeqCst);
        let revision = self.revision.load(Ordering::SeqCst);
        Box::pin(async move { Ok(PreferenceReading { enabled, revision }) })
    }
    fn set_preference(
        &self,
        enabled: bool,
        actor: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        let was = self.enabled.swap(enabled, Ordering::SeqCst);
        if was != enabled {
            self.revision.fetch_add(1, Ordering::SeqCst);
            self.settled.store(true, Ordering::SeqCst);
            self.changes.lock().unwrap().push((enabled, actor.person));
        }
        Box::pin(async move { Ok(enabled) })
    }
    fn open(&self, conversation: &str) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.opened.lock().unwrap().push(conversation.to_owned());
        if self.fail_open.load(Ordering::SeqCst) {
            refused()
        } else {
            Box::pin(async { Ok(None) })
        }
    }
    fn accept(&self, _: ScheduleDraft, _: u64) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        refused()
    }
    fn move_due(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        refused()
    }
    fn defer_claim(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        refused()
    }
    fn cancel_open(
        &self,
        _: &str,
        _: ContinuationCause,
        _: ScheduleActor,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        refused()
    }
    fn claim_due(&self, now: u64, _: usize) -> ConversationFuture<'_, Vec<ScheduledMessage>> {
        if self.submit_attempts.load(Ordering::SeqCst) {
            let retry = self.retry_at_ms.load(Ordering::SeqCst);
            if retry > now {
                return Box::pin(async { Ok(Vec::new()) });
            }
            let row = self.due_row.lock().unwrap().clone().map(|mut row| {
                row.retry_failures = self.retry_failures.load(Ordering::SeqCst);
                row
            });
            let held = row.as_ref().is_some_and(|row| {
                self.reconciles
                    .lock()
                    .unwrap()
                    .get(&row.conversation)
                    .is_some_and(|(_, retry_at)| *retry_at > now)
            });
            if held {
                return Box::pin(async { Ok(Vec::new()) });
            }
            return Box::pin(async move { Ok(row.into_iter().collect()) });
        }
        if self.idle_delivery.load(Ordering::SeqCst) {
            Box::pin(async { Ok(Vec::new()) })
        } else {
            refused()
        }
    }
    fn next_due(&self, now: u64) -> ConversationFuture<'_, Option<u64>> {
        if self.submit_attempts.load(Ordering::SeqCst) {
            let retry = self.retry_at_ms.load(Ordering::SeqCst);
            let waiting = self.due_row.lock().unwrap().clone();
            let reconcile_at = waiting.as_ref().and_then(|row| {
                self.reconciles
                    .lock()
                    .unwrap()
                    .get(&row.conversation)
                    .map(|(_, retry_at)| *retry_at)
            });
            return Box::pin(async move {
                if retry > now {
                    Ok(Some(retry))
                } else if let Some(retry_at) = reconcile_at.filter(|retry_at| *retry_at > now) {
                    Ok(Some(retry_at))
                } else if waiting.is_some() {
                    Ok(Some(now))
                } else {
                    Ok(None)
                }
            });
        }
        if self.idle_delivery.load(Ordering::SeqCst) {
            Box::pin(async { Ok(None) })
        } else {
            refused()
        }
    }
    fn begin_submit(&self, _: &str, now: u64) -> ConversationFuture<'_, bool> {
        if self.submit_attempts.load(Ordering::SeqCst) {
            let retry = self.retry_at_ms.load(Ordering::SeqCst);
            if retry > now {
                return Box::pin(async { Ok(false) });
            }
            self.submits.fetch_add(1, Ordering::SeqCst);
            if self.fail_begin.swap(false, Ordering::SeqCst) {
                return Box::pin(async { Err(ConversationError::Metadata) });
            }
            return Box::pin(async { Ok(true) });
        }
        self.submits.fetch_add(1, Ordering::SeqCst);
        refused()
    }
    fn renew_fence(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
    fn release_submit(&self, _: &str) -> ConversationFuture<'_, ()> {
        refused()
    }
    fn preference_command(
        &self,
        request_id: &str,
    ) -> ConversationFuture<'_, Option<PreferenceCommandReceipt>> {
        let saved = self.commands.lock().unwrap().get(request_id).cloned();
        Box::pin(async move { Ok(saved) })
    }
    fn record_preference_decision(
        &self,
        allowed: bool,
        enabled: bool,
        policies: &[String],
        actor: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, RecordedPreference> {
        if self.fail_decision.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        let Some(request_id) = actor.action_id.clone().filter(|value| !value.is_empty()) else {
            return Box::pin(async { Err(ConversationError::InvalidInput) });
        };
        let organization_id = actor.organization_id.clone().unwrap_or_default();
        let principal_id = actor.principal_id.clone().unwrap_or_default();
        let surface_id = actor.surface_id.clone().unwrap_or_default();
        if organization_id.is_empty() || principal_id.is_empty() || surface_id.is_empty() {
            return Box::pin(async { Err(ConversationError::InvalidInput) });
        }
        if let Some(saved) = self.commands.lock().unwrap().get(&request_id).cloned() {
            if saved.organization_id != organization_id
                || saved.principal_id != principal_id
                || saved.surface_id != surface_id
                || saved.enabled != enabled
            {
                return Box::pin(async { Err(ConversationError::RequestConflict) });
            }
            let revision = self.revision.load(Ordering::SeqCst);
            let update = match saved.outcome {
                PreferenceCommandOutcome::Stored(value) => PreferenceUpdate::Stored(value),
                PreferenceCommandOutcome::Refused { policies } => {
                    PreferenceUpdate::Refused { policies }
                }
                PreferenceCommandOutcome::Pending => PreferenceUpdate::Pending,
                PreferenceCommandOutcome::Failed => {
                    return Box::pin(async { Err(ConversationError::Unavailable) });
                }
            };
            return Box::pin(async move {
                Ok(RecordedPreference {
                    update,
                    revision,
                    installed_enable: false,
                    replayed: true,
                })
            });
        }
        self.decisions
            .lock()
            .unwrap()
            .push((allowed, policies.to_vec()));
        let (update, installed_enable) = if !allowed {
            let revision = self.revision.load(Ordering::SeqCst);
            let refused = policies.to_vec();
            self.commands.lock().unwrap().insert(
                request_id,
                PreferenceCommandReceipt {
                    organization_id,
                    principal_id,
                    surface_id,
                    enabled,
                    outcome: PreferenceCommandOutcome::Refused {
                        policies: refused.clone(),
                    },
                },
            );
            return Box::pin(async move {
                Ok(RecordedPreference {
                    update: PreferenceUpdate::Refused { policies: refused },
                    revision,
                    installed_enable: false,
                    replayed: false,
                })
            });
        } else {
            let was = self.enabled.swap(enabled, Ordering::SeqCst);
            let installed_enable = !was && enabled;
            if was != enabled {
                self.revision.fetch_add(1, Ordering::SeqCst);
                self.settled.store(!installed_enable, Ordering::SeqCst);
                self.changes.lock().unwrap().push((enabled, actor.person));
            }
            if installed_enable {
                *self.installed_request.lock().unwrap() = request_id.clone();
            }
            self.commands.lock().unwrap().insert(
                request_id,
                PreferenceCommandReceipt {
                    organization_id,
                    principal_id,
                    surface_id,
                    enabled,
                    outcome: if enabled {
                        PreferenceCommandOutcome::Pending
                    } else {
                        PreferenceCommandOutcome::Stored(false)
                    },
                },
            );
            (PreferenceUpdate::Stored(enabled), installed_enable)
        };
        let revision = self.revision.load(Ordering::SeqCst);
        Box::pin(async move {
            Ok(RecordedPreference {
                update,
                revision,
                installed_enable,
                replayed: false,
            })
        })
    }
    fn settle_installed_preference(&self, revision: u64) -> ConversationFuture<'_, bool> {
        let settled = self.settle_revision(revision);
        Box::pin(async move { Ok(settled) })
    }
    fn revert_installed_preference(
        &self,
        revision: u64,
        actor: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        if self.fail_revert.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        let matches = self.revision.load(Ordering::SeqCst) == revision
            && self.enabled.load(Ordering::SeqCst)
            && !self.settled.load(Ordering::SeqCst);
        if matches {
            self.enabled.store(false, Ordering::SeqCst);
            self.revision.fetch_add(1, Ordering::SeqCst);
            self.settled.store(true, Ordering::SeqCst);
            self.changes.lock().unwrap().push((false, actor.person));
            let request = std::mem::take(&mut *self.installed_request.lock().unwrap());
            let saved = self.commands.lock().unwrap().get(&request).cloned();
            if let Some(saved) = saved {
                self.commands.lock().unwrap().insert(
                    request,
                    PreferenceCommandReceipt {
                        outcome: PreferenceCommandOutcome::Failed,
                        ..saved
                    },
                );
            }
        }
        Box::pin(async move { Ok(matches) })
    }
    fn fail_preference_command(&self, request_id: &str) -> ConversationFuture<'_, ()> {
        let request_id = request_id.to_owned();
        let saved = self.commands.lock().unwrap().get(&request_id).cloned();
        if let Some(saved) = saved {
            if matches!(saved.outcome, PreferenceCommandOutcome::Pending) {
                self.commands.lock().unwrap().insert(
                    request_id,
                    PreferenceCommandReceipt {
                        outcome: PreferenceCommandOutcome::Failed,
                        ..saved
                    },
                );
            }
        }
        Box::pin(async { Ok(()) })
    }
    fn complete_preference_command(&self, request_id: &str) -> ConversationFuture<'_, bool> {
        let request_id = request_id.to_owned();
        let saved = self.commands.lock().unwrap().get(&request_id).cloned();
        let Some(saved) = saved else {
            return Box::pin(async { Ok(false) });
        };
        if !matches!(saved.outcome, PreferenceCommandOutcome::Pending) {
            return Box::pin(async { Ok(false) });
        }
        self.commands.lock().unwrap().insert(
            request_id,
            PreferenceCommandReceipt {
                outcome: PreferenceCommandOutcome::Stored(true),
                ..saved
            },
        );
        Box::pin(async { Ok(true) })
    }
    fn unsettled_enable(&self) -> ConversationFuture<'_, Option<super::UnsettledEnable>> {
        let enabled = self.enabled.load(Ordering::SeqCst);
        let settled = self.settled.load(Ordering::SeqCst);
        let revision = self.revision.load(Ordering::SeqCst);
        let installed_request = self.installed_request.lock().unwrap().clone();
        Box::pin(async move {
            if enabled && !settled {
                Ok(Some(super::UnsettledEnable {
                    revision,
                    installed_request,
                }))
            } else {
                Ok(None)
            }
        })
    }
    fn status(&self, _: &str) -> ConversationFuture<'_, Option<String>> {
        let known = self.known_status.lock().unwrap().clone();
        if let Some(status) = known {
            Box::pin(async move { Ok(status) })
        } else if self.submit_attempts.load(Ordering::SeqCst) {
            Box::pin(async { Ok(Some("submitting".into())) })
        } else {
            Box::pin(async { Ok(Some("delivering".into())) })
        }
    }
    fn record_outcome(
        &self,
        execution_id: &str,
        _: ScheduleSettlement,
        _: &str,
        at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        if self.outcome_missing.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Err(ConversationError::NotFound) });
        }
        if failing(&self.fail_outcome) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        self.outcomes
            .lock()
            .unwrap()
            .push((execution_id.to_owned(), at_ms));
        // A committed outcome is terminal. The row is no longer due.
        self.due_row.lock().unwrap().take();
        Box::pin(async { Ok(()) })
    }
    fn erase_conversation(&self, _: &str, _: u64) -> ConversationFuture<'_, ()> {
        refused()
    }
    fn defer_submit(
        &self,
        execution_id: &str,
        failures: u32,
        retry_at_ms: u64,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        if self.fail_defer.swap(false, Ordering::SeqCst) {
            return Box::pin(async { Ok(false) });
        }
        self.retry_failures.store(failures, Ordering::SeqCst);
        self.retry_at_ms.store(retry_at_ms, Ordering::SeqCst);
        self.defers
            .lock()
            .unwrap()
            .push((execution_id.to_owned(), failures, retry_at_ms));
        Box::pin(async { Ok(true) })
    }
    fn note_reconcile_retry(
        &self,
        conversation: &str,
        failures: u32,
        retry_at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        if self.fail_reconcile_note.swap(false, Ordering::SeqCst) {
            return refused();
        }
        self.reconciles
            .lock()
            .unwrap()
            .insert(conversation.to_owned(), (failures, retry_at_ms));
        Box::pin(async { Ok(()) })
    }
    fn clear_reconcile_retry(&self, conversation: &str) -> ConversationFuture<'_, ()> {
        self.reconciles.lock().unwrap().remove(conversation);
        Box::pin(async { Ok(()) })
    }
    fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<super::ReconcileRetry>> {
        let rows = self
            .reconciles
            .lock()
            .unwrap()
            .iter()
            .map(
                |(conversation, (failures, retry_at_ms))| super::ReconcileRetry {
                    conversation: conversation.clone(),
                    failures: *failures,
                    retry_at_ms: *retry_at_ms,
                },
            )
            .collect();
        Box::pin(async move { Ok(rows) })
    }
}

fn bind(service: &ConversationService, store: Arc<RecordingStore>) {
    service.bind_scheduled_messages(store);
    service.bind_usage_limit_preference_policy(Arc::new(AdminOnly));
}

struct TwoOwners {
    seen: Mutex<Vec<String>>,
    fail: bool,
}

impl ConversationListing for TwoOwners {
    fn list(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: bool,
        _: usize,
        _: &Reader,
    ) -> ConversationFuture<'_, ListedConversations> {
        Box::pin(async { Ok(ListedConversations::default()) })
    }
    fn observe(
        &self,
        _: &OrganizationId,
        owner: &PrincipalId,
        _: bool,
        _: Option<ObservationCursor>,
        reader: &Reader,
    ) -> ConversationFuture<'_, ObservedConversations> {
        assert!(matches!(reader, Reader::Owner));
        self.seen.lock().unwrap().push(owner.as_str().to_owned());
        let fail = self.fail;
        Box::pin(async move {
            Ok(ObservedConversations {
                conversations: Vec::new(),
                complete: !fail,
                cursor: None,
            })
        })
    }
    fn owners(&self) -> ConversationFuture<'_, Vec<(OrganizationId, PrincipalId)>> {
        Box::pin(async {
            Ok(vec![
                (
                    OrganizationId::new("org-a").unwrap(),
                    PrincipalId::new("person-a").unwrap(),
                ),
                (
                    OrganizationId::new("org-b").unwrap(),
                    PrincipalId::new("person-b").unwrap(),
                ),
            ])
        })
    }
}

struct NotifyOnDue {
    wakeup: Arc<tokio::sync::Notify>,
    scans: Arc<AtomicUsize>,
}

impl ScheduledMessageStore for NotifyOnDue {
    fn command_evidence(
        &self,
        conversation: &str,
        execution: &str,
    ) -> ConversationFuture<'_, Option<ScheduledCommandEvidence>> {
        let _ = (conversation, execution);
        refused()
    }

    fn invalidated_selection(&self, _: &str) -> ConversationFuture<'_, Option<SelectionChange>> {
        Box::pin(async { Ok(None) })
    }

    // This double exercises unrelated store behavior; model invalidation has
    // dedicated tests against LocalScheduledMessages.
    fn invalidate_usage_limit(
        &self,
        _: SelectionRequest<'_>,
        _: &str,
        _: Option<&str>,
        _: &crate::conversation::application::PlanToken,
        _: crate::conversation::application::ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(true) })
    }
    fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
        Box::pin(async {
            Ok(PreferenceReading {
                enabled: true,
                revision: 0,
            })
        })
    }
    fn set_preference(&self, _: bool, _: ScheduleActor, _: u64) -> ConversationFuture<'_, bool> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn open(&self, _: &str) -> ConversationFuture<'_, Option<super::ScheduledMessage>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn accept(
        &self,
        _: ScheduleDraft,
        _: u64,
    ) -> ConversationFuture<'_, Option<super::ScheduledMessage>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn move_due(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn defer_claim(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn cancel_open(
        &self,
        _: &str,
        _: ContinuationCause,
        _: ScheduleActor,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn claim_due(&self, _: u64, _: usize) -> ConversationFuture<'_, Vec<super::ScheduledMessage>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn next_due(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
        // One accept, during the first read. A later read must not notify:
        // the loop would otherwise stay ready and never park.
        let scan = self.scans.fetch_add(1, Ordering::SeqCst);
        if scan == 0 {
            self.wakeup.notify_waiters();
        }
        Box::pin(async { Ok(None) })
    }
    fn begin_submit(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn renew_fence(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
    fn release_submit(&self, _: &str) -> ConversationFuture<'_, ()> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn record_preference_decision(
        &self,
        _: bool,
        _: bool,
        _: &[String],
        _: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, RecordedPreference> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn settle_installed_preference(&self, _: u64) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
    fn revert_installed_preference(
        &self,
        _: u64,
        _: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
    fn status(&self, _: &str) -> ConversationFuture<'_, Option<String>> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn record_outcome(
        &self,
        _: &str,
        _: ScheduleSettlement,
        _: &str,
        _: u64,
    ) -> ConversationFuture<'_, ()> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn erase_conversation(&self, _: &str, _: u64) -> ConversationFuture<'_, ()> {
        Box::pin(async { Err(ConversationError::Unavailable) })
    }
    fn defer_submit(&self, _: &str, _: u32, _: u64, _: u64) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
    fn note_reconcile_retry(&self, _: &str, _: u32, _: u64) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn clear_reconcile_retry(&self, _: &str) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<super::ReconcileRetry>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

struct ShiftClock(AtomicU64);

impl Clock for ShiftClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct ShiftStorage {
    clock: Arc<ShiftClock>,
}

struct ShiftLease;

impl CreationStorage for ShiftStorage {
    fn open_creation(
        &self,
        _: String,
        _: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        // Read-only receipt recovery does not perform the simulated submit.
        // Advance time only when the command effect opens its writable stream.
        if create {
            self.clock.0.store(9_000, Ordering::SeqCst);
        }
        Box::pin(async { Ok(Some(Arc::new(ShiftLease) as Arc<dyn CreationStorageLease>)) })
    }
}

impl CreationStorageLease for ShiftLease {
    fn load(&self, _: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        Box::pin(async { Ok(None) })
    }
    fn save(&self, _: CreationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn load_mutation(&self, _: &str) -> CreationFuture<'_, Option<MutationReceipt>> {
        Box::pin(async { Ok(None) })
    }
    fn save_mutation(&self, _: MutationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn enabling_the_preference_scans_every_catalogue_owner() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: false,
    });
    let service = service(listing.clone(), Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    let update = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await
        .unwrap();
    assert_eq!(update, PreferenceUpdate::Stored(true));
    assert_eq!(
        listing.seen.lock().unwrap().as_slice(),
        ["person-a", "person-b"]
    );
    assert_eq!(
        store.decisions.lock().unwrap().as_slice(),
        [(true, vec!["admin".into()])]
    );
}

#[tokio::test]
async fn a_failed_catalogue_scan_restores_an_off_preference() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: true,
    });
    let service = service(listing, Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    let error = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(error.is_err());
    assert!(!store.preference().await.unwrap().enabled);
    assert_eq!(
        store.changes.lock().unwrap().as_slice(),
        &[(true, true), (false, false)]
    );
}

#[tokio::test]
async fn a_failed_revert_stays_pending_until_a_replay_finishes_it() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: true,
    });
    let service = service(listing, Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    store.fail_revert.store(true, Ordering::SeqCst);
    bind(&service, store.clone());
    let error = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(matches!(error, Err(ConversationError::Metadata)));
    assert!(store.preference().await.unwrap().enabled);
    let receipt = store.preference_command("toggle").await.unwrap().unwrap();
    assert!(matches!(receipt.outcome, PreferenceCommandOutcome::Pending));
    assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, true)]);
    let retry = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(matches!(retry, Err(ConversationError::Unavailable)));
    assert!(!store.preference().await.unwrap().enabled);
    let failed = store.preference_command("toggle").await.unwrap().unwrap();
    assert!(matches!(failed.outcome, PreferenceCommandOutcome::Failed));
    assert_eq!(
        store.changes.lock().unwrap().as_slice(),
        &[(true, true), (false, false)]
    );
    let again = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(matches!(again, Err(ConversationError::Unavailable)));
    assert!(!store.preference().await.unwrap().enabled);
    assert_eq!(
        store.changes.lock().unwrap().as_slice(),
        &[(true, true), (false, false)]
    );
    assert_eq!(store.decisions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_scan_leaves_a_preference_that_was_already_on() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: true,
    });
    let service = service(listing, Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    store
        .set_preference(true, ScheduleActor::system(), 1)
        .await
        .unwrap();
    bind(&service, store.clone());
    let error = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(error.is_err());
    assert!(store.preference().await.unwrap().enabled);
    assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, false)]);
    let receipt = store.preference_command("toggle").await.unwrap().unwrap();
    assert!(matches!(receipt.outcome, PreferenceCommandOutcome::Failed));
    let retry = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(matches!(retry, Err(ConversationError::Unavailable)));
    assert!(store.preference().await.unwrap().enabled);
    assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, false)]);
}

#[tokio::test]
async fn a_lost_revision_settlement_fails_the_request_receipt() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: false,
    });
    let service = service(listing, Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    store.lose_settle.store(true, Ordering::SeqCst);
    bind(&service, store.clone());
    let error = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(matches!(error, Err(ConversationError::Unavailable)));
    assert!(store.preference().await.unwrap().enabled);
    let receipt = store.preference_command("toggle").await.unwrap().unwrap();
    assert!(matches!(receipt.outcome, PreferenceCommandOutcome::Failed));
    let retry = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(matches!(retry, Err(ConversationError::Unavailable)));
    assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, true)]);
}

#[tokio::test]
async fn an_active_member_cannot_change_the_preference() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: false,
    });
    let service = service(listing.clone(), Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    let update = service
        .set_usage_limit_auto_continue(true, caller(), facts("member"))
        .await
        .unwrap();
    assert!(matches!(update, PreferenceUpdate::Refused { .. }));
    assert!(!store.preference().await.unwrap().enabled);
    assert!(store.changes.lock().unwrap().is_empty());
    assert!(listing.seen.lock().unwrap().is_empty());
    assert_eq!(
        store.decisions.lock().unwrap().as_slice(),
        [(false, vec!["member".into()])]
    );
}

#[tokio::test]
async fn a_continuation_accepted_during_the_scan_wakes_the_loop() {
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let scans = Arc::new(AtomicUsize::new(0));
    service.bind_scheduled_messages(Arc::new(NotifyOnDue {
        wakeup: service.inner.schedule_wakeup.clone(),
        scans: scans.clone(),
    }));
    let retired = service.inner.retired.clone();
    let task = tokio::spawn(async move {
        service.run_scheduled_messages().await;
    });
    let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while scans.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    retired.send_replace(true);
    let finished = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
    assert!(
        observed.is_ok(),
        "a continuation accepted during the scan did not wake the loop"
    );
    finished.expect("the schedule loop did not stop").unwrap();
}

#[tokio::test]
async fn the_outcome_clock_is_read_after_submission() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(1_000)));
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(false));
    service.bind_scheduled_messages(store.clone());
    service.bind_commands(Arc::new(ShiftStorage { clock }));
    let conversation = "11111111-1111-4111-8111-111111111111";
    let row = ScheduledMessage {
        command_evidence: None,
        conversation: conversation.into(),
        causal_execution: "causal".into(),
        execution_id: "exec-1".into(),
        due_at_ms: 10,
        delivering: true,
        agent: "claude".into(),
        model: "opus".into(),
        prompt: "continue".into(),
        organization_id: "org".into(),
        principal_id: "person".into(),
        surface_id: "desktop".into(),
        action_id: "action".into(),
        initiation: "owner".into(),
        grant_id: None,
        policy_revision: None,
        tools_checked: false,
        retry_failures: 0,
    };
    service
        .finish_fenced_submit(
            store.clone(),
            ConversationId::new(conversation).unwrap(),
            &row,
            u64::MAX,
            Arc::new(SubmissionPermit {
                _threads: service.inner.thread_admission.clone().lock_owned().await,
                _admission: service.inner.admission.clone().read_owned().await,
                _selection: service
                    .inner
                    .mode_changes
                    .lock(&ConversationId::new(conversation).unwrap())
                    .await,
            }),
        )
        .await;
    assert_eq!(
        store.outcomes.lock().unwrap().as_slice(),
        [("exec-1".into(), 9_000)]
    );
}

/// A committed-session read that fails. Absence is `Ok(None)`.
struct UnreadableSessions;

impl SessionStorage for UnreadableSessions {
    fn read_committed(
        &self,
        _: SessionId,
    ) -> StorageFuture<
        '_,
        Option<nessa_sdk::application::agent_execution::sessions::CommittedSession>,
    > {
        Box::pin(async {
            Err(StorageError::Io(
                "the committed session could not be read".into(),
            ))
        })
    }

    fn open(
        &self,
        _: SessionId,
    ) -> StorageFuture<
        '_,
        Box<dyn nessa_sdk::application::agent_execution::sessions::SessionStorageLease>,
    > {
        Box::pin(async {
            Err(StorageError::Io(
                "the committed session could not be read".into(),
            ))
        })
    }
}

#[tokio::test]
async fn a_failed_execution_read_does_not_settle_the_submission() {
    let service = service_stored(
        Arc::new(MemoryRepository::default()),
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
        Arc::new(UnreadableSessions),
    );
    let store = Arc::new(RecordingStore::new(false));
    service.bind_scheduled_messages(store.clone());
    service.bind_commands(Arc::new(ShiftStorage {
        clock: Arc::new(ShiftClock(AtomicU64::new(0))),
    }));
    let conversation = "11111111-1111-4111-8111-111111111111";
    let row = ScheduledMessage {
        command_evidence: None,
        conversation: conversation.into(),
        causal_execution: "causal".into(),
        execution_id: "exec-1".into(),
        due_at_ms: 10,
        delivering: true,
        agent: "claude".into(),
        model: "opus".into(),
        prompt: "continue".into(),
        organization_id: "org".into(),
        principal_id: "person".into(),
        surface_id: "desktop".into(),
        action_id: "action".into(),
        initiation: "owner".into(),
        grant_id: None,
        policy_revision: None,
        tools_checked: false,
        retry_failures: 0,
    };
    let retry = service
        .finish_fenced_submit(
            store.clone(),
            ConversationId::new(conversation).unwrap(),
            &row,
            1_700_000_000_123 + 60_000,
            Arc::new(SubmissionPermit {
                _threads: service.inner.thread_admission.clone().lock_owned().await,
                _admission: service.inner.admission.clone().read_owned().await,
                _selection: service
                    .inner
                    .mode_changes
                    .lock(&ConversationId::new(conversation).unwrap())
                    .await,
            }),
        )
        .await;
    assert!(retry);
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unread_execution_waits_out_the_same_backoff() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let service = service_stored(
        Arc::new(MemoryRepository::default()),
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
        Arc::new(UnreadableSessions),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.submit_attempts.store(true, Ordering::SeqCst);
    *store.due_row.lock().unwrap() = Some(due_row("owner"));
    service.bind_scheduled_messages(store.clone());
    service.bind_commands(Arc::new(ShiftStorage {
        clock: Arc::new(ShiftClock(AtomicU64::new(0))),
    }));

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
    for _ in 0..30 {
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
    }
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 2);
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
}

fn due_row(initiation: &str) -> ScheduledMessage {
    ScheduledMessage {
        command_evidence: None,
        conversation: "11111111-1111-4111-8111-111111111111".into(),
        causal_execution: "causal".into(),
        execution_id: "exec-1".into(),
        due_at_ms: 10,
        delivering: true,
        agent: "claude".into(),
        model: "opus".into(),
        prompt: "continue".into(),
        organization_id: "org".into(),
        principal_id: "person".into(),
        surface_id: "desktop".into(),
        action_id: "action".into(),
        initiation: initiation.into(),
        grant_id: None,
        policy_revision: None,
        tools_checked: false,
        retry_failures: 0,
    }
}

fn delivering(
    clock: &Arc<ShiftClock>,
    store: &Arc<RecordingStore>,
    initiation: &str,
) -> ConversationService {
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    store.submit_attempts.store(true, Ordering::SeqCst);
    *store.due_row.lock().unwrap() = Some(due_row(initiation));
    service.bind_scheduled_messages(store.clone());
    service.bind_commands(Arc::new(ShiftStorage {
        clock: clock.clone(),
    }));
    service
}

#[tokio::test]
async fn a_failed_outcome_waits_out_its_backoff_and_the_replay_records_it() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_outcome.store(2, Ordering::SeqCst);
    let service = delivering(&clock, &store, "grant");

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert_eq!(
        store.defers.lock().unwrap().as_slice(),
        [("exec-1".into(), 1, 5_000 + SCHEDULE_RETRY_MS)]
    );

    // The row is not due yet. A second pass does not take the lease again.
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert_eq!(store.defers.lock().unwrap().len(), 1);

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert_eq!(
        store.defers.lock().unwrap().as_slice(),
        [
            ("exec-1".into(), 1, 5_000 + SCHEDULE_RETRY_MS),
            (
                "exec-1".into(),
                2,
                5_000 + SCHEDULE_RETRY_MS + SCHEDULE_RETRY_MS * 2
            ),
        ]
    );

    clock.0.store(
        5_000 + SCHEDULE_RETRY_MS + SCHEDULE_RETRY_MS * 2,
        Ordering::SeqCst,
    );
    assert_eq!(service.deliver_due_scheduled().await, None);
    assert_eq!(
        store.outcomes.lock().unwrap().as_slice(),
        [(
            "exec-1".into(),
            5_000 + SCHEDULE_RETRY_MS + SCHEDULE_RETRY_MS * 2
        )]
    );
    assert_eq!(store.defers.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_failed_outcome_after_submit_is_not_due_again_immediately() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_outcome.store(1, Ordering::SeqCst);
    let service = delivering(&clock, &store, "owner");

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.defers.lock().unwrap().as_slice(),
        [("exec-1".into(), 1, 9_000 + SCHEDULE_RETRY_MS)]
    );

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert!(store.outcomes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_outcome_whose_retry_does_not_commit_still_waits() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_outcome.store(1, Ordering::SeqCst);
    store.fail_defer.store(true, Ordering::SeqCst);
    let service = delivering(&clock, &store, "grant");

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
    assert_eq!(store.retry_at_ms.load(Ordering::SeqCst), 0);
    for _ in 0..30 {
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
    }
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert!(store.defers.lock().unwrap().is_empty());

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    store.fail_outcome.store(1, Ordering::SeqCst);
    store.fail_defer.store(true, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 2);
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
}

/// A `begin_submit` whose transaction does not commit is the same retry.
/// A missed compare-and-set is not: this error leaves the row `delivering`.
#[tokio::test]
async fn a_begin_submit_error_waits_out_its_backoff() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_begin.store(true, Ordering::SeqCst);
    let service = delivering(&clock, &store, "grant");

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
    assert_eq!(store.retry_at_ms.load(Ordering::SeqCst), 0);
    for _ in 0..30 {
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
    }
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    store.fail_begin.store(true, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 2);
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
}

/// A `begin_submit` error keeps its attempt only while the row is still
/// `delivering`. A cancel, including a preference that is now off, and a
/// missing row drop it before the backoff ends.
#[tokio::test]
async fn a_cancelled_or_missing_row_drops_the_submit_attempt_this_process_kept() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_begin.store(true, Ordering::SeqCst);
    *store.known_status.lock().unwrap() = Some(Some("delivering".into()));
    let service = delivering(&clock, &store, "grant");

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert!(service
        .submit_retries()
        .iter()
        .any(|(id, _)| id == "exec-1"));
    for _ in 0..5 {
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
    }
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);

    store.due_row.lock().unwrap().take();
    store.enabled.store(false, Ordering::SeqCst);
    *store.known_status.lock().unwrap() = Some(Some("cancelled".into()));
    let _ = service.deliver_due_scheduled().await;
    assert!(service.submit_retries().is_empty());

    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_begin.store(true, Ordering::SeqCst);
    *store.known_status.lock().unwrap() = Some(Some("delivering".into()));
    let service = delivering(&clock, &store, "grant");
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    store.due_row.lock().unwrap().take();
    *store.known_status.lock().unwrap() = Some(None);
    assert_eq!(service.deliver_due_scheduled().await, None);
    assert!(service.submit_retries().is_empty());
}

/// A retry the database refused is dropped once the outcome commits.
/// Later passes do not keep walking that execution.
#[tokio::test]
async fn a_committed_outcome_drops_the_submit_attempt_this_process_kept() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_outcome.store(1, Ordering::SeqCst);
    store.fail_defer.store(true, Ordering::SeqCst);
    let service = delivering(&clock, &store, "grant");

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(service
        .submit_retries()
        .iter()
        .any(|(id, at)| { id == "exec-1" && *at == 5_000 + SCHEDULE_RETRY_MS }));

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    assert_eq!(service.deliver_due_scheduled().await, None);
    assert_eq!(store.outcomes.lock().unwrap().len(), 1);
    assert!(service.submit_retries().is_empty());
}

/// A missing row is terminal the same way. The attempt this process kept
/// for the uncommitted retry does not stay behind.
#[tokio::test]
async fn a_missing_row_drops_the_submit_attempt_this_process_kept() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_outcome.store(1, Ordering::SeqCst);
    store.fail_defer.store(true, Ordering::SeqCst);
    let service = delivering(&clock, &store, "grant");

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert!(!service.submit_retries().is_empty());

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    store.outcome_missing.store(true, Ordering::SeqCst);
    let _ = service.deliver_due_scheduled().await;
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(service.submit_retries().is_empty());
}

#[tokio::test]
async fn disabling_the_preference_publishes_every_followed_conversation() {
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(true));
    bind(&service, store.clone());
    let first = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let second = ConversationId::new("22222222-2222-4222-8222-222222222222").unwrap();
    let followed_first = service.inner.live_changes.subscribe(&first);
    let followed_second = service.inner.live_changes.subscribe(&second);
    let update = service
        .set_usage_limit_auto_continue(false, caller(), facts("admin"))
        .await
        .unwrap();
    assert_eq!(update, PreferenceUpdate::Stored(false));
    assert!(followed_first.has_changed().unwrap());
    assert!(followed_second.has_changed().unwrap());
}

#[tokio::test]
async fn a_denied_preference_does_not_publish() {
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let followed = service.inner.live_changes.subscribe(&id);
    let update = service
        .set_usage_limit_auto_continue(true, caller(), facts("member"))
        .await
        .unwrap();
    assert!(matches!(update, PreferenceUpdate::Refused { .. }));
    assert!(!followed.has_changed().unwrap());
}

#[tokio::test]
async fn a_retried_enable_after_a_later_disable_does_not_turn_the_preference_back_on() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: false,
    });
    let service = service(listing.clone(), Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    let enabled = service
        .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
        .await
        .unwrap();
    assert_eq!(enabled, PreferenceUpdate::Stored(true));
    let seen = listing.seen.lock().unwrap().clone();
    assert_eq!(seen.as_slice(), ["person-a", "person-b"]);
    let disabled = service
        .set_usage_limit_auto_continue(false, caller_as("disable"), facts("admin"))
        .await
        .unwrap();
    assert_eq!(disabled, PreferenceUpdate::Stored(false));
    assert!(!store.preference().await.unwrap().enabled);
    let decisions = store.decisions.lock().unwrap().len();
    let replay = service
        .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
        .await
        .unwrap();
    assert_eq!(replay, PreferenceUpdate::Stored(true));
    assert!(!store.preference().await.unwrap().enabled);
    assert_eq!(store.decisions.lock().unwrap().len(), decisions);
    assert_eq!(listing.seen.lock().unwrap().as_slice(), seen.as_slice());
    assert_eq!(
        store.changes.lock().unwrap().as_slice(),
        &[(true, true), (false, true)]
    );
}

#[tokio::test]
async fn a_preference_request_with_a_different_value_conflicts_and_writes_nothing() {
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    service
        .set_usage_limit_auto_continue(true, caller_as("one"), facts("admin"))
        .await
        .unwrap();
    let conflict = service
        .set_usage_limit_auto_continue(false, caller_as("one"), facts("admin"))
        .await;
    assert!(matches!(conflict, Err(ConversationError::RequestConflict)));
    assert!(store.preference().await.unwrap().enabled);
    assert_eq!(store.decisions.lock().unwrap().len(), 1);
    let mut other = caller_as("one");
    other.principal_id = PrincipalId::new("other").unwrap();
    let mut other_facts = facts("admin");
    other_facts.principal_id = "other".into();
    let conflict = service
        .set_usage_limit_auto_continue(true, other, other_facts)
        .await;
    assert!(matches!(conflict, Err(ConversationError::RequestConflict)));
    assert!(store.preference().await.unwrap().enabled);
    assert_eq!(store.decisions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_blank_or_unreadable_preference_request_is_rejected_before_the_decision() {
    let evaluations = Arc::new(AtomicUsize::new(0));
    struct Counting {
        evaluations: Arc<AtomicUsize>,
    }
    impl UsageLimitPreferencePolicyEvaluator for Counting {
        fn evaluate(
            &self,
            _: &UsageLimitPreferenceFacts,
        ) -> Result<UsageLimitPreferenceDecision, AccessError> {
            self.evaluations.fetch_add(1, Ordering::SeqCst);
            Ok(UsageLimitPreferenceDecision::Allow {
                policies: vec!["admin".into()],
            })
        }
    }
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(false));
    service.bind_scheduled_messages(store.clone());
    service.bind_usage_limit_preference_policy(Arc::new(Counting {
        evaluations: evaluations.clone(),
    }));
    let oversized = "x".repeat(257);
    for action in ["", " ", "toggle\n", "toggle\u{0001}", oversized.as_str()] {
        let error = service
            .set_usage_limit_auto_continue(true, caller_as(action), facts("admin"))
            .await;
        assert!(
            matches!(error, Err(ConversationError::InvalidInput)),
            "{action:?} {error:?}"
        );
    }
    assert_eq!(evaluations.load(Ordering::SeqCst), 0);
    assert!(store.decisions.lock().unwrap().is_empty());
    assert!(store.commands.lock().unwrap().is_empty());
    assert!(!store.preference().await.unwrap().enabled);
}

#[tokio::test]
async fn a_reverted_enable_retries_as_the_same_failure() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: true,
    });
    let service = service(listing.clone(), Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    let error = service
        .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
        .await;
    assert!(error.is_err());
    assert!(!store.preference().await.unwrap().enabled);
    let seen = listing.seen.lock().unwrap().len();
    let decisions = store.decisions.lock().unwrap().len();
    let retry = service
        .set_usage_limit_auto_continue(true, caller_as("enable"), facts("admin"))
        .await;
    assert!(matches!(retry, Err(ConversationError::Unavailable)));
    assert!(!store.preference().await.unwrap().enabled);
    assert_eq!(listing.seen.lock().unwrap().len(), seen);
    assert_eq!(store.decisions.lock().unwrap().len(), decisions);
}

#[tokio::test]
async fn a_failed_preference_audit_leaves_the_preference_and_returns_an_error() {
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(false));
    bind(&service, store.clone());
    store.fail_decision.store(true, Ordering::SeqCst);
    let error = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(error.is_err());
    assert!(!store.preference().await.unwrap().enabled);
    assert!(store.changes.lock().unwrap().is_empty());
    assert!(store.decisions.lock().unwrap().is_empty());

    store.fail_decision.store(true, Ordering::SeqCst);
    let denied = service
        .set_usage_limit_auto_continue(true, caller(), facts("member"))
        .await;
    assert!(denied.is_err());
    assert!(!store.preference().await.unwrap().enabled);
    assert!(store.decisions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_schedule_plan_does_not_submit() {
    let repository = Arc::new(MemoryRepository::default());
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
        crate::conversation::domain::Conversation::restore(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            ConversationModelId::new("opus").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(true));
    service.bind_scheduled_messages(store.clone());
    let row = ScheduledMessage {
        command_evidence: None,
        conversation: id.to_string(),
        causal_execution: "causal".into(),
        execution_id: "exec-1".into(),
        due_at_ms: 10,
        delivering: true,
        agent: "claude".into(),
        model: "opus".into(),
        prompt: "continue".into(),
        organization_id: "org".into(),
        principal_id: "person".into(),
        surface_id: "desktop".into(),
        action_id: "action".into(),
        initiation: "owner".into(),
        grant_id: None,
        policy_revision: None,
        tools_checked: false,
        retry_failures: 0,
    };
    service.deliver_scheduled(store.clone(), row).await;
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.reconciles.lock().unwrap().get(&id.to_string()),
        Some(&(1, TestClock.unix_milliseconds() + SCHEDULE_RETRY_MS))
    );
}

#[tokio::test]
async fn a_failed_delivery_plan_waits_out_its_backoff() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let repository = Arc::new(MemoryRepository::default());
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
        crate::conversation::domain::Conversation::restore(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            ConversationModelId::new("opus").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.submit_attempts.store(true, Ordering::SeqCst);
    // This row has been claimed for delivery but has not crossed begin_submit.
    *store.known_status.lock().unwrap() = Some(Some("delivering".into()));
    store.fail_open.store(true, Ordering::SeqCst);
    *store.due_row.lock().unwrap() = Some(due_row("owner"));
    service.bind_scheduled_messages(store.clone());

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.reconciles.lock().unwrap().get(&id.to_string()),
        Some(&(1, 5_000 + SCHEDULE_RETRY_MS))
    );
    for _ in 0..30 {
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
    }
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    assert_eq!(
        store
            .reconciles
            .lock()
            .unwrap()
            .get(&id.to_string())
            .map(|(failures, _)| *failures),
        Some(2)
    );
}

#[tokio::test]
async fn a_delivery_plan_the_database_refuses_does_not_spin() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let repository = Arc::new(MemoryRepository::default());
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
        crate::conversation::domain::Conversation::restore(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            ConversationModelId::new("opus").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.submit_attempts.store(true, Ordering::SeqCst);
    // This row has been claimed for delivery but has not crossed begin_submit.
    *store.known_status.lock().unwrap() = Some(Some("delivering".into()));
    store.fail_open.store(true, Ordering::SeqCst);
    store.fail_reconcile_note.store(true, Ordering::SeqCst);
    *store.due_row.lock().unwrap() = Some(due_row("owner"));
    service.bind_scheduled_messages(store.clone());

    assert!(service.deliver_due_scheduled().await.unwrap() >= SCHEDULE_RETRY_MS);
    assert!(store.reconciles.lock().unwrap().is_empty());
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    for _ in 0..30 {
        assert!(service.deliver_due_scheduled().await.unwrap() >= SCHEDULE_RETRY_MS);
    }
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_terminal_refusal_publishes_the_conversation() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    let service = delivering(&clock, &store, "grant");
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let followed = service.inner.live_changes.subscribe(&id);
    assert_eq!(service.deliver_due_scheduled().await, None);
    assert_eq!(store.outcomes.lock().unwrap().len(), 1);
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert!(followed.has_changed().unwrap());
}

#[tokio::test]
async fn a_model_request_records_absence_even_without_a_committed_session() {
    let repository = Arc::new(MemoryRepository::default());
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
        crate::conversation::domain::Conversation::restore(
            id.clone(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "desktop".into(),
            "create".into(),
            1,
            Some(nessa_protocol::agents::AgentId::Claude),
            ConversationModelId::new("opus").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("scheduled");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("scheduled-chat-messages.sqlite3");
    service.bind_scheduled_messages(Arc::new(LocalScheduledMessages::open(&path).unwrap()));
    service
        .invalidate_selection_usage_limit(
            SelectionChange::Model,
            &id,
            "model-request",
            ScheduleActor::person(&caller_as("model-request")),
        )
        .await
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    let causal: String = connection.query_row(
        "SELECT causal_execution FROM usage_limit_invalidation WHERE request_id = 'model-request'",
        [], |row| row.get(0),
    ).unwrap();
    assert!(causal.is_empty());
}

#[tokio::test]
async fn an_applied_model_change_is_not_complete_until_reconciliation_commits() {
    let repository = Arc::new(MemoryRepository::default());
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let principal = PrincipalId::new("person").unwrap();
    let prior_model = ConversationModelId::new("opus").unwrap();
    let requested = ConversationModelId::new("sonnet").unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
        crate::conversation::domain::Conversation::restore(
            id.clone(),
            organization.clone(),
            principal.clone(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            prior_model.clone(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    let selection = ConversationModelSelection {
        model: requested.clone(),
        approval_mode: ConversationApprovalMode::Ask,
        effect: ConversationModelChangeEffect::NextTurn,
    };
    repository.model_requests.lock().unwrap().insert(
        (id.clone(), "toggle".into()),
        ConversationModelRequest {
            conversation_id: id.clone(),
            organization_id: organization,
            request_id: "toggle".into(),
            initiator_principal_id: principal.clone(),
            initiator_surface_id: "desktop".into(),
            prior_identity: ProviderIdentity::new("fixture", "opus", "").unwrap(),
            requested_identity: ProviderIdentity::new("fixture", "sonnet", "").unwrap(),
            prior_model,
            prior_approval_mode: ConversationApprovalMode::Ask,
            requested_model: requested.clone(),
            state: ConversationModelRequestState::Applied,
            application: Some(ConversationModelApplication::Applied(selection.clone())),
            requested_at_ms: 1,
        },
    );
    let service = service_over(
        repository.clone(),
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(false));
    service.bind_scheduled_messages(store.clone());
    let caller = caller();
    let failed = service.change_model(&id, &caller, requested.clone()).await;
    assert!(failed.is_err());
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    store.fail_open.store(false, Ordering::SeqCst);
    let applied = service.change_model(&id, &caller, requested).await.unwrap();
    assert_eq!(applied, selection);
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);

    let applied_request = repository
        .model_requests
        .lock()
        .unwrap()
        .get(&(id.clone(), "toggle".into()))
        .unwrap()
        .clone();
    repository.model_requests.lock().unwrap().insert(
        (id.clone(), "refused".into()),
        ConversationModelRequest {
            request_id: "refused".into(),
            state: ConversationModelRequestState::NotApplied,
            application: Some(ConversationModelApplication::Refused),
            ..applied_request
        },
    );
    let opens = store.opens.load(Ordering::SeqCst);
    let mut refused_caller = caller;
    refused_caller.action_id = "refused".into();
    let refused = service
        .change_model(
            &id,
            &refused_caller,
            ConversationModelId::new("sonnet").unwrap(),
        )
        .await;
    assert!(matches!(
        refused,
        Err(ConversationError::ModelChangeNotApplied)
    ));
    assert_eq!(store.opens.load(Ordering::SeqCst), opens);
}

#[tokio::test]
async fn a_failed_model_reconciliation_is_retried_when_nothing_is_pending() {
    let repository = Arc::new(MemoryRepository::default());
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let organization = OrganizationId::new("org").unwrap();
    let principal = PrincipalId::new("person").unwrap();
    let prior_model = ConversationModelId::new("opus").unwrap();
    let requested = ConversationModelId::new("sonnet").unwrap();
    repository.records.lock().unwrap().insert(
        id.clone(),
        crate::conversation::domain::Conversation::restore(
            id.clone(),
            organization.clone(),
            principal.clone(),
            "desktop".into(),
            "create".into(),
            1,
            None,
            prior_model.clone(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    repository.model_requests.lock().unwrap().insert(
        (id.clone(), "toggle".into()),
        ConversationModelRequest {
            conversation_id: id.clone(),
            organization_id: organization,
            request_id: "toggle".into(),
            initiator_principal_id: principal,
            initiator_surface_id: "desktop".into(),
            prior_identity: ProviderIdentity::new("fixture", "opus", "").unwrap(),
            requested_identity: ProviderIdentity::new("fixture", "sonnet", "").unwrap(),
            prior_model,
            prior_approval_mode: ConversationApprovalMode::Ask,
            requested_model: requested.clone(),
            state: ConversationModelRequestState::Applied,
            application: Some(ConversationModelApplication::Applied(
                ConversationModelSelection {
                    model: requested.clone(),
                    approval_mode: ConversationApprovalMode::Ask,
                    effect: ConversationModelChangeEffect::NextTurn,
                },
            )),
            requested_at_ms: 1,
        },
    );
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let store = Arc::new(RecordingStore::new(false));
    service.bind_scheduled_messages(store.clone());
    let caller = caller();
    assert!(service.change_model(&id, &caller, requested).await.is_err());
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert!(service.recover_model_change(&id, &caller).await.is_err());
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);
    store.fail_open.store(false, Ordering::SeqCst);
    service.recover_model_change(&id, &caller).await.unwrap();
    assert_eq!(store.opens.load(Ordering::SeqCst), 3);
    let opens = store.opens.load(Ordering::SeqCst);
    service.recover_model_change(&id, &caller).await.unwrap();
    assert_eq!(store.opens.load(Ordering::SeqCst), opens);
}

/// A catalogue page that settles the enable, then fails the walk.
struct SettleThenFail {
    store: Arc<RecordingStore>,
}

impl ConversationListing for SettleThenFail {
    fn list(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: bool,
        _: usize,
        _: &Reader,
    ) -> ConversationFuture<'_, ListedConversations> {
        Box::pin(async { Ok(ListedConversations::default()) })
    }
    fn observe(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: bool,
        _: Option<ObservationCursor>,
        reader: &Reader,
    ) -> ConversationFuture<'_, ObservedConversations> {
        assert!(matches!(reader, Reader::Owner));
        let revision = self.store.revision.load(Ordering::SeqCst);
        self.store.settle_revision(revision);
        Box::pin(async {
            Ok(ObservedConversations {
                conversations: Vec::new(),
                complete: false,
                cursor: None,
            })
        })
    }
    fn owners(&self) -> ConversationFuture<'_, Vec<(OrganizationId, PrincipalId)>> {
        Box::pin(async {
            Ok(vec![(
                OrganizationId::new("org-a").unwrap(),
                PrincipalId::new("person-a").unwrap(),
            )])
        })
    }
}

#[tokio::test]
async fn a_settled_enable_is_not_rolled_back_by_a_later_scan_failure() {
    let store = Arc::new(RecordingStore::new(false));
    let service = service(
        Arc::new(SettleThenFail {
            store: store.clone(),
        }),
        Arc::new(TestClock),
    );
    bind(&service, store.clone());
    let error = service
        .set_usage_limit_auto_continue(true, caller(), facts("admin"))
        .await;
    assert!(error.is_err());
    assert!(store.preference().await.unwrap().enabled);
    assert_eq!(store.changes.lock().unwrap().as_slice(), &[(true, true)]);
}

/// Schedule-store reads that fail a counted number of times, and a scripted renew.
struct ScriptedSchedule {
    enabled: AtomicBool,
    preference_failures: AtomicUsize,
    claim_failures: AtomicUsize,
    next_due_failures: AtomicUsize,
    renew: Mutex<Vec<Result<bool, ConversationError>>>,
    renew_calls: AtomicUsize,
}

impl ScriptedSchedule {
    fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            preference_failures: AtomicUsize::new(0),
            claim_failures: AtomicUsize::new(0),
            next_due_failures: AtomicUsize::new(0),
            renew: Mutex::new(Vec::new()),
            renew_calls: AtomicUsize::new(0),
        }
    }
}

fn failing(counter: &AtomicUsize) -> bool {
    loop {
        let left = counter.load(Ordering::SeqCst);
        if left == 0 {
            return false;
        }
        if counter
            .compare_exchange(left, left - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return true;
        }
    }
}

fn store_error() -> ConversationFuture<'static, bool> {
    Box::pin(async { Err(ConversationError::Metadata) })
}

impl ScheduledMessageStore for ScriptedSchedule {
    fn command_evidence(
        &self,
        conversation: &str,
        execution: &str,
    ) -> ConversationFuture<'_, Option<ScheduledCommandEvidence>> {
        let _ = (conversation, execution);
        Box::pin(async { Err(ConversationError::Metadata) })
    }

    fn invalidated_selection(&self, _: &str) -> ConversationFuture<'_, Option<SelectionChange>> {
        Box::pin(async { Ok(None) })
    }

    // This double exercises unrelated store behavior; model invalidation has
    // dedicated tests against LocalScheduledMessages.
    fn invalidate_usage_limit(
        &self,
        _: SelectionRequest<'_>,
        _: &str,
        _: Option<&str>,
        _: &crate::conversation::application::PlanToken,
        _: crate::conversation::application::ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(true) })
    }
    fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
        if failing(&self.preference_failures) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        let enabled = self.enabled.load(Ordering::SeqCst);
        Box::pin(async move {
            Ok(PreferenceReading {
                enabled,
                revision: 0,
            })
        })
    }
    fn set_preference(&self, _: bool, _: ScheduleActor, _: u64) -> ConversationFuture<'_, bool> {
        store_error()
    }
    fn open(&self, _: &str) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
    fn accept(&self, _: ScheduleDraft, _: u64) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
    fn move_due(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        store_error()
    }
    fn defer_claim(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        store_error()
    }
    fn cancel_open(
        &self,
        _: &str,
        _: ContinuationCause,
        _: ScheduleActor,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        store_error()
    }
    fn claim_due(&self, _: u64, _: usize) -> ConversationFuture<'_, Vec<ScheduledMessage>> {
        if failing(&self.claim_failures) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        Box::pin(async { Ok(Vec::new()) })
    }
    fn next_due(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
        if failing(&self.next_due_failures) {
            return Box::pin(async { Err(ConversationError::Metadata) });
        }
        Box::pin(async { Ok(None) })
    }
    fn begin_submit(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
        store_error()
    }
    fn renew_fence(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
        self.renew_calls.fetch_add(1, Ordering::SeqCst);
        let next = {
            let mut renew = self.renew.lock().unwrap();
            if renew.is_empty() {
                None
            } else {
                Some(renew.remove(0))
            }
        };
        Box::pin(async move {
            match next {
                Some(result) => result,
                None => Ok(true),
            }
        })
    }
    fn release_submit(&self, _: &str) -> ConversationFuture<'_, ()> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
    fn record_preference_decision(
        &self,
        _: bool,
        _: bool,
        _: &[String],
        _: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, RecordedPreference> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
    fn settle_installed_preference(&self, _: u64) -> ConversationFuture<'_, bool> {
        store_error()
    }
    fn revert_installed_preference(
        &self,
        _: u64,
        _: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        store_error()
    }
    fn status(&self, _: &str) -> ConversationFuture<'_, Option<String>> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
    fn record_outcome(
        &self,
        _: &str,
        _: ScheduleSettlement,
        _: &str,
        _: u64,
    ) -> ConversationFuture<'_, ()> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
    fn erase_conversation(&self, _: &str, _: u64) -> ConversationFuture<'_, ()> {
        Box::pin(async { Err(ConversationError::Metadata) })
    }
    fn defer_submit(&self, _: &str, _: u32, _: u64, _: u64) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(false) })
    }
    fn note_reconcile_retry(&self, _: &str, _: u32, _: u64) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn clear_reconcile_retry(&self, _: &str) -> ConversationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<super::ReconcileRetry>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

#[tokio::test]
async fn a_store_error_retries_delivery_instead_of_parking() {
    let listing = Arc::new(TwoOwners {
        seen: Mutex::new(Vec::new()),
        fail: false,
    });
    let unbound = service(listing.clone(), Arc::new(TestClock));
    assert_eq!(unbound.deliver_due_scheduled().await, None);

    let schedule = Arc::new(ScriptedSchedule::new(false));
    let service = service(listing, Arc::new(TestClock));
    service.bind_scheduled_messages(schedule.clone());

    schedule.preference_failures.store(1, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(service.deliver_due_scheduled().await, None);

    schedule.enabled.store(true, Ordering::SeqCst);
    schedule.claim_failures.store(1, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    schedule.next_due_failures.store(1, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(service.deliver_due_scheduled().await, None);
}

#[tokio::test(start_paused = true)]
async fn a_transient_fence_renewal_is_retried() {
    let schedule = Arc::new(ScriptedSchedule::new(true));
    schedule
        .renew
        .lock()
        .unwrap()
        .push(Err(ConversationError::Metadata));
    schedule.renew.lock().unwrap().push(Ok(true));
    let lost = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn(renew_while_held(
        schedule.clone(),
        "exec-1".into(),
        Arc::new(TestClock),
        lost.clone(),
        TestClock.unix_milliseconds() + FENCE_LEASE_MS,
    ));
    tokio::time::sleep(Duration::from_millis(
        FENCE_RENEW_EVERY_MS + FENCE_RENEW_RETRY_MS + 1,
    ))
    .await;
    assert!(schedule.renew_calls.load(Ordering::SeqCst) >= 2);
    assert!(!lost.load(Ordering::SeqCst));
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_lost_fence_stops_renewal() {
    let schedule = Arc::new(ScriptedSchedule::new(true));
    schedule.renew.lock().unwrap().push(Ok(false));
    let lost = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn(renew_while_held(
        schedule.clone(),
        "exec-1".into(),
        Arc::new(TestClock),
        lost.clone(),
        TestClock.unix_milliseconds() + FENCE_LEASE_MS,
    ));
    tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS + 1)).await;
    assert!(lost.load(Ordering::SeqCst));
    assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), 1);
    tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS)).await;
    assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_renewal_error_after_the_lease_expires_stops() {
    let schedule = Arc::new(ScriptedSchedule::new(true));
    for _ in 0..40 {
        schedule
            .renew
            .lock()
            .unwrap()
            .push(Err(ConversationError::Metadata));
    }
    let clock = Arc::new(PausingClock {
        origin: tokio::time::Instant::now(),
        base: 1_000,
    });
    let lost = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn(renew_while_held(
        schedule.clone(),
        "exec-1".into(),
        clock,
        lost.clone(),
        1_000 + FENCE_LEASE_MS,
    ));
    tokio::time::sleep(Duration::from_millis(FENCE_LEASE_MS + FENCE_RENEW_EVERY_MS)).await;
    assert!(lost.load(Ordering::SeqCst));
    let calls = schedule.renew_calls.load(Ordering::SeqCst);
    assert!(calls >= 1);
    tokio::time::sleep(Duration::from_millis(FENCE_RENEW_EVERY_MS * 3)).await;
    assert_eq!(schedule.renew_calls.load(Ordering::SeqCst), calls);
    task.abort();
}

#[test]
fn a_continuation_prompt_stays_within_the_input_limit() {
    let notice =
        "The usage limit reset. Continue the request that was waiting.\n\nThe request was:\n";
    let request = "please continue the request that was waiting on the limit";
    for max in [1_usize, notice.len() - 1, notice.len(), notice.len() + 4] {
        let prompt = super::continuation_prompt(request, max);
        assert!(prompt.len() <= max, "{prompt:?} exceeds {max}");
        assert!(!prompt.trim().is_empty());
    }
    let fitted = super::continuation_prompt("hello", notice.len() + 5);
    assert!(fitted.starts_with(notice));
    assert!(fitted.ends_with("hello"));
    assert!(fitted.len() <= notice.len() + 5);
}

#[test]
fn catch_up_backoff_doubles_until_it_is_capped() {
    assert_eq!(super::attempt_backoff(1), SCHEDULE_RETRY_MS);
    assert_eq!(super::attempt_backoff(2), SCHEDULE_RETRY_MS * 2);
    assert_eq!(super::attempt_backoff(3), SCHEDULE_RETRY_MS * 4);
    assert_eq!(super::attempt_backoff(30), super::CATCH_UP_RETRY_MAX_MS);
}

/// The first page of `person-a` cannot say where to resume. `person-b`
/// is the owner that page skips.
struct FailThenPlan {
    conversation: crate::conversation::domain::Conversation,
    summary: ConversationSummary,
    seen: Mutex<Vec<String>>,
    failed_pages: AtomicUsize,
}

impl ConversationListing for FailThenPlan {
    fn list(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: bool,
        _: usize,
        _: &Reader,
    ) -> ConversationFuture<'_, ListedConversations> {
        Box::pin(async { Ok(ListedConversations::default()) })
    }
    fn observe(
        &self,
        _: &OrganizationId,
        owner: &PrincipalId,
        _: bool,
        _: Option<ObservationCursor>,
        reader: &Reader,
    ) -> ConversationFuture<'_, ObservedConversations> {
        assert!(matches!(reader, Reader::Owner));
        let name = owner.as_str().to_owned();
        self.seen.lock().unwrap().push(name.clone());
        if name == "person-a" && self.failed_pages.fetch_add(1, Ordering::SeqCst) == 0 {
            return Box::pin(async {
                Ok(ObservedConversations {
                    conversations: Vec::new(),
                    complete: false,
                    cursor: None,
                })
            });
        }
        let conversations = if name == "person-b" {
            vec![ListedConversation {
                held: None,
                conversation: self.conversation.clone(),
                summary: self.summary.clone(),
            }]
        } else {
            Vec::new()
        };
        Box::pin(async move {
            Ok(ObservedConversations {
                conversations,
                complete: true,
                cursor: None,
            })
        })
    }
    fn owners(&self) -> ConversationFuture<'_, Vec<(OrganizationId, PrincipalId)>> {
        Box::pin(async {
            Ok(vec![
                (
                    OrganizationId::new("org-a").unwrap(),
                    PrincipalId::new("person-a").unwrap(),
                ),
                (
                    OrganizationId::new("org-b").unwrap(),
                    PrincipalId::new("person-b").unwrap(),
                ),
            ])
        })
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_catch_up_scan_schedules_the_skipped_owner_on_a_later_pass() {
    let id = ConversationId::new("33333333-3333-4333-8333-333333333333").unwrap();
    let conversation = crate::conversation::domain::Conversation::restore(
        id.clone(),
        OrganizationId::new("org-b").unwrap(),
        PrincipalId::new("person-b").unwrap(),
        "desktop".into(),
        "create".into(),
        1,
        None,
        ConversationModelId::new("opus").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    let repository = Arc::new(MemoryRepository::default());
    repository
        .records
        .lock()
        .unwrap()
        .insert(id.clone(), conversation.clone());
    let listing = Arc::new(FailThenPlan {
        conversation,
        summary: ConversationSummary::new(None, None, 1, false).unwrap(),
        seen: Mutex::new(Vec::new()),
        failed_pages: AtomicUsize::new(0),
    });
    let service = service_over(repository, listing.clone(), Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_open.store(false, Ordering::SeqCst);
    store.idle_delivery.store(true, Ordering::SeqCst);
    service.bind_scheduled_messages(store.clone());
    let retired = service.inner.retired.clone();
    let task = tokio::spawn(async move {
        service.run_scheduled_messages().await;
    });

    let saw_failure = tokio::time::timeout(Duration::from_secs(5), async {
        while listing.failed_pages.load(Ordering::SeqCst) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(saw_failure.is_ok(), "the catalogue page did not fail");
    assert!(store.opened.lock().unwrap().is_empty());
    assert_eq!(listing.seen.lock().unwrap().as_slice(), ["person-a"]);

    tokio::time::sleep(Duration::from_millis(SCHEDULE_RETRY_MS - 1)).await;
    tokio::task::yield_now().await;
    assert_eq!(listing.failed_pages.load(Ordering::SeqCst), 1);
    assert!(store.opened.lock().unwrap().is_empty());

    tokio::time::sleep(Duration::from_millis(2)).await;
    let planned = tokio::time::timeout(Duration::from_secs(5), async {
        while !store
            .opened
            .lock()
            .unwrap()
            .iter()
            .any(|opened| opened == &id.to_string())
        {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(planned.is_ok(), "the skipped owner was not planned");
    let seen = listing.seen.lock().unwrap().clone();
    let skipped = seen
        .iter()
        .position(|owner| owner == "person-b")
        .expect("person-b was not visited");
    assert!(
        seen[..skipped]
            .iter()
            .filter(|owner| owner.as_str() == "person-a")
            .count()
            >= 2
    );

    retired.send_replace(true);
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("the schedule loop did not stop")
        .unwrap();
}

/// A preference read that fails must not park the loop when delivery has
/// nothing due. The next pass, after the backoff, traverses the owners.
#[tokio::test(start_paused = true)]
async fn a_failed_preference_read_retries_catch_up_when_nothing_is_due() {
    let id = ConversationId::new("33333333-3333-4333-8333-333333333333").unwrap();
    let conversation = crate::conversation::domain::Conversation::restore(
        id.clone(),
        OrganizationId::new("org-b").unwrap(),
        PrincipalId::new("person-b").unwrap(),
        "desktop".into(),
        "create".into(),
        1,
        None,
        ConversationModelId::new("opus").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    let repository = Arc::new(MemoryRepository::default());
    repository
        .records
        .lock()
        .unwrap()
        .insert(id.clone(), conversation.clone());
    let listing = Arc::new(FailThenPlan {
        conversation,
        summary: ConversationSummary::new(None, None, 1, false).unwrap(),
        seen: Mutex::new(Vec::new()),
        failed_pages: AtomicUsize::new(1),
    });
    let service = service_over(repository, listing.clone(), Arc::new(TestClock));
    let store = Arc::new(RecordingStore::new(true));
    store.fail_open.store(false, Ordering::SeqCst);
    store.idle_delivery.store(true, Ordering::SeqCst);
    store.preference_failures.store(1, Ordering::SeqCst);
    service.bind_scheduled_messages(store.clone());
    let retired = service.inner.retired.clone();
    let task = tokio::spawn(async move {
        service.run_scheduled_messages().await;
    });

    let saw_failure = tokio::time::timeout(Duration::from_secs(5), async {
        while store.preference_failures.load(Ordering::SeqCst) > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(saw_failure.is_ok(), "the preference read did not fail");
    assert!(store.opened.lock().unwrap().is_empty());
    assert!(listing.seen.lock().unwrap().is_empty());

    tokio::time::sleep(Duration::from_millis(SCHEDULE_RETRY_MS - 1)).await;
    tokio::task::yield_now().await;
    assert!(store.opened.lock().unwrap().is_empty());
    assert!(listing.seen.lock().unwrap().is_empty());

    tokio::time::sleep(Duration::from_millis(2)).await;
    let planned = tokio::time::timeout(Duration::from_secs(5), async {
        while !store
            .opened
            .lock()
            .unwrap()
            .iter()
            .any(|opened| opened == &id.to_string())
        {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        planned.is_ok(),
        "catch-up did not run after the preference read failed"
    );

    retired.send_replace(true);
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("the schedule loop did not stop")
        .unwrap();
}

struct PausingClock {
    origin: tokio::time::Instant,
    base: u64,
}

impl Clock for PausingClock {
    fn unix_milliseconds(&self) -> u64 {
        self.base
            .saturating_add(u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(0))
    }
}

fn blocked_conversation(id: &ConversationId) -> crate::conversation::domain::Conversation {
    crate::conversation::domain::Conversation::restore(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "desktop".into(),
        "create".into(),
        1,
        None,
        ConversationModelId::new("opus").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap()
}

/// Catch-up has finished. A settlement whose plan cannot be read is tried
/// again only after the backoff, and not on the tight loop that would
/// otherwise see nothing due.
#[tokio::test(start_paused = true)]
async fn a_failed_settlement_is_retried_after_the_backoff() {
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let repository = Arc::new(MemoryRepository::default());
    repository
        .records
        .lock()
        .unwrap()
        .insert(id.clone(), blocked_conversation(&id));
    let clock = Arc::new(PausingClock {
        origin: tokio::time::Instant::now(),
        base: 5_000,
    });
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.idle_delivery.store(true, Ordering::SeqCst);
    service.bind_scheduled_messages(store.clone());
    service
        .consider_usage_limit(&id, false, ScheduleActor::system())
        .await;
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    let (failures, retry_at) = store
        .reconciles
        .lock()
        .unwrap()
        .get(&id.to_string())
        .copied()
        .expect("the settlement kept a retry");
    assert_eq!(failures, 1);
    assert_eq!(
        retry_at.saturating_sub(clock.unix_milliseconds()),
        SCHEDULE_RETRY_MS
    );

    let retired = service.inner.retired.clone();
    let task = tokio::spawn(async move {
        service.run_scheduled_messages().await;
    });
    tokio::time::sleep(Duration::from_millis(SCHEDULE_RETRY_MS - 1)).await;
    tokio::task::yield_now().await;
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_millis(2)).await;
    let retried = tokio::time::timeout(Duration::from_secs(5), async {
        while store.opens.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(retried.is_ok(), "the settlement was not retried");
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);

    retired.send_replace(true);
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("the schedule loop did not stop")
        .unwrap();
}

#[tokio::test]
async fn a_reconcile_debt_the_database_refuses_is_still_retried() {
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let repository = Arc::new(MemoryRepository::default());
    repository
        .records
        .lock()
        .unwrap()
        .insert(id.clone(), blocked_conversation(&id));
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.idle_delivery.store(true, Ordering::SeqCst);
    store.fail_reconcile_note.store(true, Ordering::SeqCst);
    service.bind_scheduled_messages(store.clone());
    service
        .consider_usage_limit(&id, false, ScheduleActor::system())
        .await;
    assert!(store.reconciles.lock().unwrap().is_empty());
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert!(service.retry_due_reconciles().await.unwrap() >= SCHEDULE_RETRY_MS - 1);
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    clock
        .0
        .store(5_000 + SCHEDULE_RETRY_MS - 1, Ordering::SeqCst);
    service.retry_due_reconciles().await;
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    service.retry_due_reconciles().await;
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_later_reconcile_note_that_does_not_commit_keeps_its_backoff() {
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let repository = Arc::new(MemoryRepository::default());
    repository
        .records
        .lock()
        .unwrap()
        .insert(id.clone(), blocked_conversation(&id));
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let service = service_over(
        repository,
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.idle_delivery.store(true, Ordering::SeqCst);
    service.bind_scheduled_messages(store.clone());
    service
        .consider_usage_limit(&id, false, ScheduleActor::system())
        .await;
    assert_eq!(store.opens.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .reconciles
            .lock()
            .unwrap()
            .get(&id.to_string())
            .copied(),
        Some((1, 5_000 + SCHEDULE_RETRY_MS))
    );

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    store.fail_reconcile_note.store(true, Ordering::SeqCst);
    assert_eq!(
        service.retry_due_reconciles().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);
    assert_eq!(
        store
            .reconciles
            .lock()
            .unwrap()
            .get(&id.to_string())
            .copied(),
        Some((1, 5_000 + SCHEDULE_RETRY_MS))
    );
    for _ in 0..30 {
        assert_eq!(
            service.retry_due_reconciles().await,
            Some(SCHEDULE_RETRY_MS * 2)
        );
    }
    assert_eq!(store.opens.load(Ordering::SeqCst), 2);

    clock.0.store(
        5_000 + SCHEDULE_RETRY_MS + SCHEDULE_RETRY_MS * 2,
        Ordering::SeqCst,
    );
    service.retry_due_reconciles().await;
    assert_eq!(store.opens.load(Ordering::SeqCst), 3);
}

struct BusyCommands;

impl CreationStorage for BusyCommands {
    fn open_creation(
        &self,
        _: String,
        _: String,
        _: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async { Err(CreationStorageError::Storage(StorageError::Busy)) })
    }
}

fn submitting_row(conversation: &str) -> ScheduledMessage {
    ScheduledMessage {
        command_evidence: None,
        conversation: conversation.into(),
        causal_execution: "causal".into(),
        execution_id: "exec-1".into(),
        due_at_ms: 10,
        delivering: true,
        agent: "claude".into(),
        model: "opus".into(),
        prompt: "continue".into(),
        organization_id: "org".into(),
        principal_id: "person".into(),
        surface_id: "desktop".into(),
        action_id: "action".into(),
        initiation: "owner".into(),
        grant_id: None,
        policy_revision: None,
        tools_checked: false,
        retry_failures: 0,
    }
}

/// A retryable submit failure stays retryable, and the same process does
/// not acquire the row again before the backoff.
#[tokio::test]
async fn a_retryable_submit_does_not_spin() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.submit_attempts.store(true, Ordering::SeqCst);
    let conversation = "11111111-1111-4111-8111-111111111111";
    store
        .due_row
        .lock()
        .unwrap()
        .replace(submitting_row(conversation));
    service.bind_scheduled_messages(store.clone());
    service.bind_commands(Arc::new(BusyCommands));

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.defers.lock().unwrap().as_slice(),
        [("exec-1".into(), 1, 5_000 + SCHEDULE_RETRY_MS)]
    );
    for _ in 0..30 {
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
    }
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert!(store.outcomes.lock().unwrap().is_empty());

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 2);
    assert_eq!(
        store.defers.lock().unwrap().as_slice(),
        [
            ("exec-1".into(), 1, 5_000 + SCHEDULE_RETRY_MS),
            ("exec-1".into(), 2, 5_000 + SCHEDULE_RETRY_MS * 3),
        ]
    );
}

#[tokio::test]
async fn a_retryable_submit_whose_retry_does_not_commit_still_waits() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
    );
    let store = Arc::new(RecordingStore::new(true));
    store.submit_attempts.store(true, Ordering::SeqCst);
    store.fail_defer.store(true, Ordering::SeqCst);
    let conversation = "11111111-1111-4111-8111-111111111111";
    store
        .due_row
        .lock()
        .unwrap()
        .replace(submitting_row(conversation));
    service.bind_scheduled_messages(store.clone());
    service.bind_commands(Arc::new(BusyCommands));

    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
    assert!(store.defers.lock().unwrap().is_empty());
    assert_eq!(store.retry_at_ms.load(Ordering::SeqCst), 0);
    for _ in 0..30 {
        assert_eq!(
            service.deliver_due_scheduled().await,
            Some(SCHEDULE_RETRY_MS)
        );
    }
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);

    clock.0.store(5_000 + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    store.fail_defer.store(true, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS * 2)
    );
    assert_eq!(store.submits.load(Ordering::SeqCst), 2);
    assert!(store.outcomes.lock().unwrap().is_empty());
    assert!(store.defers.lock().unwrap().is_empty());
}
/// Real schedule and command stores exercise preference changes after the
/// schedule fence, including a restarted delivery owner.
async fn disabled_recovery_fixture(
    directory: &Path,
    model: &str,
) -> (
    ConversationService,
    Arc<LocalScheduledMessages>,
    Arc<RecordStorage>,
    Arc<ShiftClock>,
    ScheduledMessage,
) {
    let commands = Arc::new(RecordStorage::new(directory.join("records")).unwrap());
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let service = service_stored(
        Arc::new(MemoryRepository::default()),
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        clock.clone(),
        commands.clone(),
    );
    nessa_local_storage::create_directory(&directory.join("scheduled")).unwrap();
    let schedule = Arc::new(
        LocalScheduledMessages::open(&directory.join("scheduled/scheduled.sqlite3")).unwrap(),
    );
    schedule
        .set_preference(true, ScheduleActor::system(), 1)
        .await
        .unwrap();
    let mut row = submitting_row("11111111-1111-4111-8111-111111111111");
    row.model = model.into();
    schedule
        .accept(
            recovery_draft(&row, schedule.preference().await.unwrap().revision),
            2,
        )
        .await
        .unwrap()
        .unwrap();
    schedule.claim_due(5_000, 8).await.unwrap();
    assert!(schedule
        .begin_submit(&row.execution_id, 5_000)
        .await
        .unwrap());
    schedule.release_submit(&row.execution_id).await.unwrap();
    drop(schedule);
    let schedule = Arc::new(
        LocalScheduledMessages::open(&directory.join("scheduled/scheduled.sqlite3")).unwrap(),
    );
    service.bind_scheduled_messages(schedule.clone());
    service.bind_commands(commands.clone());
    (service, schedule, commands, clock, row)
}

fn recovery_draft(row: &ScheduledMessage, preference_revision: u64) -> ScheduleDraft {
    ScheduleDraft {
        conversation: row.conversation.clone(),
        causal_execution: row.causal_execution.clone(),
        execution_id: row.execution_id.clone(),
        due_at_ms: 5_000,
        cause: ContinuationCause::ProviderLimit,
        actor: ScheduleActor::system(),
        agent: row.agent.clone(),
        model: row.model.clone(),
        prompt: row.prompt.clone(),
        organization_id: row.organization_id.clone(),
        principal_id: row.principal_id.clone(),
        surface_id: row.surface_id.clone(),
        action_id: row.action_id.clone(),
        initiation: row.initiation.clone(),
        grant_id: None,
        policy_revision: None,
        tools_checked: false,
        preference_revision,
        history: HistoryGeneration {
            incarnation: "history".into(),
            position: 1,
            generation: 0,
        },
    }
}

#[tokio::test]
async fn preference_off_recovers_command_evidence_before_cancellation() {
    for (stage, outcome) in [
        (None, None),
        (Some(MutationStage::Accepted), None),
        (Some(MutationStage::Attempted), None),
        (
            Some(MutationStage::Settled),
            Some(MutationOutcome::Dispatched),
        ),
        (
            Some(MutationStage::Settled),
            Some(MutationOutcome::NoEffect),
        ),
        (
            Some(MutationStage::Settled),
            Some(MutationOutcome::RefusedAfterSave),
        ),
        (
            Some(MutationStage::Settled),
            Some(MutationOutcome::UnavailableAfterSave),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (service, schedule, commands, clock, row) =
            disabled_recovery_fixture(directory.path(), "opus").await;
        let id = ConversationId::new(&row.conversation).unwrap();
        if stage.is_none() || stage == Some(MutationStage::Accepted) {
            service
                .create(
                    id.clone(),
                    schedule_caller(&row).unwrap(),
                    RequestedConversation::default(),
                )
                .await
                .unwrap();
        }
        let binding = ConversationService::submit_lookup(
            &id,
            &schedule_caller(&row).unwrap(),
            &row.execution_id,
            &SubmittedMessage {
                uploads: Vec::new(),
                text: row.prompt.clone(),
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .unwrap();
        if let Some(stage) = stage {
            let lease = commands
                .open_creation("person".into(), "exec-1".into(), true)
                .await
                .unwrap()
                .unwrap();
            let mut receipt = MutationReceipt::accepted(binding.clone());
            lease.save_mutation(receipt.clone()).await.unwrap();
            if stage != MutationStage::Accepted {
                receipt = receipt.advance(MutationStage::Attempted, None).unwrap();
                lease.save_mutation(receipt.clone()).await.unwrap();
            }
            if stage == MutationStage::Settled {
                receipt = receipt.advance(MutationStage::Settled, outcome).unwrap();
                lease.save_mutation(receipt).await.unwrap();
            }
        }
        let expired = 5_000 + FENCE_LEASE_MS;
        clock.0.store(expired, Ordering::SeqCst);
        schedule
            .set_preference(false, ScheduleActor::system(), expired)
            .await
            .unwrap();
        assert_eq!(
            schedule.status(&row.execution_id).await.unwrap().as_deref(),
            Some("submitting")
        );
        if stage.is_none() {
            schedule.fail_next_outcome_audit();
            assert_eq!(
                service.deliver_due_scheduled().await,
                Some(SCHEDULE_RETRY_MS)
            );
            assert_eq!(
                schedule.status(&row.execution_id).await.unwrap().as_deref(),
                Some("submitting")
            );
            clock.0.store(expired + SCHEDULE_RETRY_MS, Ordering::SeqCst);
            schedule
                .set_preference(true, ScheduleActor::system(), expired + 1)
                .await
                .unwrap();
        }
        assert_eq!(service.deliver_due_scheduled().await, None);
        let expected = match stage {
            Some(MutationStage::Attempted) => "unconfirmed",
            Some(MutationStage::Settled) if outcome == Some(MutationOutcome::Dispatched) => {
                "delivered"
            }
            _ => "refused",
        };
        assert_eq!(
            schedule.status(&row.execution_id).await.unwrap().as_deref(),
            Some(expected),
            "stage {stage:?}"
        );
        let connection =
            Connection::open(directory.path().join("scheduled/scheduled.sqlite3")).unwrap();
        let audit: (String, String, String, String, String, String) = connection.query_row(
            "SELECT conversation, causal_execution, execution_id, before_status, after_status, initiator_kind FROM audit WHERE after_status = ?1 ORDER BY id DESC LIMIT 1",
            [expected], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        ).unwrap();
        assert_eq!(
            audit,
            (
                row.conversation.clone(),
                row.causal_execution.clone(),
                row.execution_id.clone(),
                "submitting".into(),
                expected.into(),
                "system".into()
            )
        );
        let cause: String = connection
            .query_row(
                "SELECT cause FROM audit WHERE after_status = ?1 ORDER BY id DESC LIMIT 1",
                [expected],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            cause,
            match (stage, outcome) {
                (Some(MutationStage::Attempted), _) => "submission_interrupted",
                (_, Some(MutationOutcome::Dispatched)) => "delivered",
                (_, Some(MutationOutcome::NoEffect | MutationOutcome::RefusedAfterSave)) =>
                    "revoked",
                (_, Some(MutationOutcome::UnavailableAfterSave)) => "unavailable",
                _ => "preference_disabled",
            }
        );
        let lease = commands
            .open_creation("person".into(), "exec-1".into(), false)
            .await
            .unwrap()
            .unwrap();
        let receipt = lease.load_mutation("exec-1").await.unwrap().unwrap();
        if stage.is_none() || stage == Some(MutationStage::Accepted) {
            assert_eq!(
                receipt.outcome(),
                Some(MutationOutcome::RefusedBeforeAttempt)
            );
            schedule
                .set_preference(true, ScheduleActor::system(), expired + 1)
                .await
                .unwrap();
            let mut draft = recovery_draft(&row, schedule.preference().await.unwrap().revision);
            draft.history.generation = schedule
                .history_generation(&row.conversation)
                .await
                .unwrap();
            assert!(schedule.accept(draft, expired + 2).await.unwrap().is_none());
            assert!(schedule.open(&row.conversation).await.unwrap().is_none());
        }
        drop(lease);
        if stage.is_none() || stage == Some(MutationStage::Accepted) {
            let delayed = service
                .submit_command(
                    commands.clone(),
                    id.clone(),
                    schedule_caller(&row).unwrap(),
                    Initiation::owner(),
                    false,
                    row.execution_id.clone(),
                    SubmittedMessage {
                        uploads: Vec::new(),
                        text: row.prompt.clone(),
                        ..Default::default()
                    },
                    SubmissionMode::Queue,
                )
                .await;
            assert!(matches!(
                delayed,
                Err(MutationFailure::Target(ConversationError::Revoked))
            ));
            assert!(!service
                .execution_saved(&id, &row.execution_id)
                .await
                .unwrap());
        }
        service.shutdown().await.unwrap();
        drop(service);
        commands.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn preference_off_recovers_a_saved_turn_after_its_delivery_audit_failed() {
    let directory = tempfile::tempdir().unwrap();
    let (service, schedule, commands, clock, row) =
        disabled_recovery_fixture(directory.path(), "opus").await;
    let id = ConversationId::new(&row.conversation).unwrap();
    let caller = schedule_caller(&row).unwrap();
    service
        .create(id.clone(), caller.clone(), RequestedConversation::default())
        .await
        .unwrap();
    service
        .submit_command(
            commands.clone(),
            id.clone(),
            caller,
            Initiation::owner(),
            false,
            row.execution_id.clone(),
            SubmittedMessage {
                uploads: Vec::new(),
                text: row.prompt.clone(),
                ..Default::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    schedule.fail_next_outcome_audit();
    assert!(schedule
        .record_outcome(
            &row.execution_id,
            ScheduleSettlement::Delivered,
            "delivered",
            5_001
        )
        .await
        .is_err());
    // Delete through the real owner: it erases saved history before the
    // schedule payload. The durable Dispatched receipt must still recover.
    assert!(service
        .delete(id.clone(), caller_as("delete"))
        .await
        .unwrap());
    assert!(!service
        .execution_saved(&id, &row.execution_id)
        .await
        .unwrap());
    let expired = 5_000 + FENCE_LEASE_MS;
    clock.0.store(expired, Ordering::SeqCst);
    schedule
        .set_preference(false, ScheduleActor::system(), expired)
        .await
        .unwrap();
    assert_eq!(service.deliver_due_scheduled().await, None);
    assert_eq!(
        schedule.status(&row.execution_id).await.unwrap().as_deref(),
        Some("delivered")
    );
    service.shutdown().await.unwrap();
    drop(service);
    commands.shutdown().await.unwrap();
}
#[tokio::test]
async fn a_scrubbed_live_submission_is_unconfirmed_when_adopted_after_restart() {
    for enabled in [true, false] {
        for accepted_only in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let (service, schedule, commands, clock, row) =
                disabled_recovery_fixture(directory.path(), "opus").await;
            if accepted_only {
                let id = ConversationId::new(&row.conversation).unwrap();
                let binding = super::scheduled_submit_binding(&id, &row).unwrap();
                let lease = commands
                    .open_creation("person".into(), "exec-1".into(), true)
                    .await
                    .unwrap()
                    .unwrap();
                lease
                    .save_mutation(MutationReceipt::accepted(binding))
                    .await
                    .unwrap();
            }
            schedule
                .erase_conversation(&row.conversation, 5_001)
                .await
                .unwrap();
            assert_eq!(
                schedule.status(&row.execution_id).await.unwrap().as_deref(),
                Some("submitting")
            );
            service.shutdown().await.unwrap();
            drop(service);
            commands.shutdown().await.unwrap();
            drop(commands);
            drop(schedule);
            let commands = Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
            let service = service_stored(
                Arc::new(MemoryRepository::default()),
                Arc::new(TwoOwners {
                    seen: Mutex::new(Vec::new()),
                    fail: false,
                }),
                clock.clone(),
                commands.clone(),
            );
            let schedule = Arc::new(
                LocalScheduledMessages::open(&directory.path().join("scheduled/scheduled.sqlite3"))
                    .unwrap(),
            );
            service.bind_scheduled_messages(schedule.clone());
            service.bind_commands(commands.clone());
            let expired = 5_000 + FENCE_LEASE_MS;
            clock.0.store(expired, Ordering::SeqCst);
            schedule
                .set_preference(enabled, ScheduleActor::system(), expired)
                .await
                .unwrap();
            assert_eq!(service.deliver_due_scheduled().await, None);
            assert_eq!(
                schedule.status(&row.execution_id).await.unwrap().as_deref(),
                Some("unconfirmed")
            );
            let journal = commands
                .open_creation("person".into(), "exec-1".into(), false)
                .await
                .unwrap();
            if accepted_only {
                let receipt = journal
                    .as_ref()
                    .unwrap()
                    .load_mutation("exec-1")
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(receipt.stage(), MutationStage::Accepted);
            } else {
                assert!(journal.is_none());
            }
            drop(journal);
            let connection =
                Connection::open(directory.path().join("scheduled/scheduled.sqlite3")).unwrap();
            let audit: (String, String, String) = connection
                .query_row(
                    "SELECT cause, before_status, after_status FROM audit ORDER BY id DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                audit,
                (
                    "conversation_deleted".into(),
                    "submitting".into(),
                    "unconfirmed".into()
                )
            );
            service.shutdown().await.unwrap();
            drop(service);
            commands.shutdown().await.unwrap();
        }
    }
}

#[test]
fn stop_receipts_do_not_prove_a_disabled_submission_was_delivered_or_refused() {
    let row = submitting_row("11111111-1111-4111-8111-111111111111");
    let id = ConversationId::new(&row.conversation).unwrap();
    let binding = ConversationService::submit_lookup(
        &id,
        &schedule_caller(&row).unwrap(),
        &row.execution_id,
        &SubmittedMessage {
            uploads: Vec::new(),
            text: row.prompt.clone(),
            ..Default::default()
        },
        SubmissionMode::Queue,
    )
    .unwrap();
    for outcome in [
        MutationOutcome::Withdrawn,
        MutationOutcome::Cancelled,
        MutationOutcome::AlreadyFinal,
    ] {
        let receipt = MutationReceipt::accepted(binding.clone())
            .advance(MutationStage::Attempted, None)
            .unwrap()
            .advance(MutationStage::Settled, Some(outcome))
            .unwrap();
        assert_eq!(super::submission_receipt_outcome(&receipt), None);
    }
}

#[tokio::test]
async fn deletion_recovers_original_command_outcomes_after_failed_audit_and_restart() {
    for enabled in [false, true] {
        for delete_after_expiry in [false, true] {
            for (outcome, expected, cause) in [
                (Some(MutationOutcome::Dispatched), "delivered", "delivered"),
                (Some(MutationOutcome::NoEffect), "refused", "revoked"),
                (
                    Some(MutationOutcome::RefusedBeforeAttempt),
                    "refused",
                    "preference_disabled",
                ),
                (
                    Some(MutationOutcome::ModelChangedBeforeAttempt),
                    "refused",
                    "model_changed",
                ),
                (
                    Some(MutationOutcome::AgentChangedBeforeAttempt),
                    "refused",
                    "agent_changed",
                ),
                (None, "unconfirmed", "submission_interrupted"),
            ] {
                let directory = tempfile::tempdir().unwrap();
                let (service, schedule, commands, clock, row) =
                    disabled_recovery_fixture(directory.path(), "opus").await;
                let id = ConversationId::new(&row.conversation).unwrap();
                let binding = ConversationService::submit_lookup(
                    &id,
                    &schedule_caller(&row).unwrap(),
                    &row.execution_id,
                    &SubmittedMessage {
                        uploads: Vec::new(),
                        text: row.prompt.clone(),
                        ..Default::default()
                    },
                    SubmissionMode::Queue,
                )
                .unwrap();
                let lease = commands
                    .open_creation("person".into(), "exec-1".into(), true)
                    .await
                    .unwrap()
                    .unwrap();
                let mut receipt = MutationReceipt::accepted(binding.clone());
                lease.save_mutation(receipt.clone()).await.unwrap();
                if !matches!(
                    outcome,
                    Some(
                        MutationOutcome::RefusedBeforeAttempt
                            | MutationOutcome::ModelChangedBeforeAttempt
                            | MutationOutcome::AgentChangedBeforeAttempt
                    )
                ) {
                    receipt = receipt.advance(MutationStage::Attempted, None).unwrap();
                    lease.save_mutation(receipt.clone()).await.unwrap();
                }
                if let Some(outcome) = outcome {
                    lease
                        .save_mutation(
                            receipt
                                .advance(MutationStage::Settled, Some(outcome))
                                .unwrap(),
                        )
                        .await
                        .unwrap();
                }
                drop(lease);
                // Adopt the original owner's expired fence, then lose only the
                // schedule outcome audit. The command journal stays committed.
                let first_adoption = 5_000 + FENCE_LEASE_MS;
                clock.0.store(first_adoption, Ordering::SeqCst);
                schedule.fail_next_outcome_audit();
                assert_eq!(
                    service.deliver_due_scheduled().await,
                    Some(SCHEDULE_RETRY_MS)
                );
                let deleted_at = if delete_after_expiry {
                    first_adoption + FENCE_LEASE_MS
                } else {
                    first_adoption + 1
                };
                schedule
                    .erase_conversation(&row.conversation, deleted_at)
                    .await
                    .unwrap();
                assert_eq!(
                    schedule.status(&row.execution_id).await.unwrap().as_deref(),
                    Some("submitting")
                );
                schedule
                    .set_preference(enabled, ScheduleActor::system(), deleted_at)
                    .await
                    .unwrap();
                service.shutdown().await.unwrap();
                drop(service);
                commands.shutdown().await.unwrap();
                drop(commands);
                drop(schedule);
                let commands =
                    Arc::new(RecordStorage::new(directory.path().join("records")).unwrap());
                let service = service_stored(
                    Arc::new(MemoryRepository::default()),
                    Arc::new(TwoOwners {
                        seen: Mutex::new(Vec::new()),
                        fail: false,
                    }),
                    clock.clone(),
                    commands.clone(),
                );
                let schedule = Arc::new(
                    LocalScheduledMessages::open(
                        &directory.path().join("scheduled/scheduled.sqlite3"),
                    )
                    .unwrap(),
                );
                service.bind_scheduled_messages(schedule.clone());
                service.bind_commands(commands.clone());
                clock
                    .0
                    .store(first_adoption + FENCE_LEASE_MS, Ordering::SeqCst);
                let retained = schedule
                    .claim_due(clock.unix_milliseconds(), 8)
                    .await
                    .unwrap()
                    .pop()
                    .unwrap();
                assert!(retained.prompt.is_empty());
                assert!(retained.principal_id.is_empty());
                assert!(retained.surface_id.is_empty());
                assert!(retained.grant_id.is_none());
                assert_eq!(
                    super::scheduled_submit_binding(&id, &retained).unwrap(),
                    binding
                );
                assert_eq!(service.deliver_due_scheduled().await, None);
                assert_eq!(
                    schedule.status(&row.execution_id).await.unwrap().as_deref(),
                    Some(expected),
                    "{outcome:?}, enabled={enabled}, expired={delete_after_expiry}"
                );
                let connection =
                    Connection::open(directory.path().join("scheduled/scheduled.sqlite3")).unwrap();
                let recorded: (String, String, String) = connection.query_row(
                    "SELECT cause, after_status, initiator_kind FROM audit ORDER BY id DESC LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ).unwrap();
                assert_eq!(recorded, (cause.into(), expected.into(), "system".into()));
                service.shutdown().await.unwrap();
                drop(service);
                commands.shutdown().await.unwrap();
            }
        }
    }
}

#[tokio::test]
async fn deleted_command_evidence_retries_a_busy_journal_without_guessing_an_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let (service, schedule, commands, clock, row) =
        disabled_recovery_fixture(directory.path(), "opus").await;
    let id = ConversationId::new(&row.conversation).unwrap();
    let binding = ConversationService::submit_lookup(
        &id,
        &schedule_caller(&row).unwrap(),
        &row.execution_id,
        &SubmittedMessage {
            uploads: Vec::new(),
            text: row.prompt.clone(),
            ..Default::default()
        },
        SubmissionMode::Queue,
    )
    .unwrap();
    let lease = commands
        .open_creation("person".into(), "exec-1".into(), true)
        .await
        .unwrap()
        .unwrap();
    let accepted = MutationReceipt::accepted(binding);
    lease.save_mutation(accepted.clone()).await.unwrap();
    let attempted = accepted.advance(MutationStage::Attempted, None).unwrap();
    lease.save_mutation(attempted.clone()).await.unwrap();
    lease
        .save_mutation(
            attempted
                .advance(MutationStage::Settled, Some(MutationOutcome::Dispatched))
                .unwrap(),
        )
        .await
        .unwrap();
    schedule
        .erase_conversation(&row.conversation, 5_001)
        .await
        .unwrap();
    clock.0.store(5_000 + FENCE_LEASE_MS, Ordering::SeqCst);
    assert_eq!(
        service.deliver_due_scheduled().await,
        Some(SCHEDULE_RETRY_MS)
    );
    assert_eq!(
        schedule.status(&row.execution_id).await.unwrap().as_deref(),
        Some("submitting")
    );
    drop(lease);
    clock
        .0
        .store(5_000 + FENCE_LEASE_MS + SCHEDULE_RETRY_MS, Ordering::SeqCst);
    assert_eq!(service.deliver_due_scheduled().await, None);
    assert_eq!(
        schedule.status(&row.execution_id).await.unwrap().as_deref(),
        Some("delivered")
    );
    service.shutdown().await.unwrap();
    drop(service);
    commands.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_thread_admission_precedes_the_scheduled_selection_permit() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    let service = delivering(&clock, &store, "owner");
    let row = store.due_row.lock().unwrap().clone().unwrap();
    let id = ConversationId::new(&row.conversation).unwrap();
    let threads = service.inner.thread_admission.clone().lock_owned().await;
    let delivery = service.deliver_due_scheduled();
    tokio::pin!(delivery);
    assert!(futures_util::poll!(&mut delivery).is_pending());
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    let selection = service
        .inner
        .mode_changes
        .try_lock(&id)
        .expect("a delivery waiting for thread admission must not hold selection");
    drop(selection);
    drop(threads);
    tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .unwrap();
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_selection_lock_blocks_the_durable_submit_claim() {
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    let service = delivering(&clock, &store, "owner");
    let row = store.due_row.lock().unwrap().clone().unwrap();
    let id = ConversationId::new(&row.conversation).unwrap();
    let permit = service.inner.mode_changes.lock(&id).await;
    let delivery = service.deliver_due_scheduled();
    tokio::pin!(delivery);
    assert!(futures_util::poll!(&mut delivery).is_pending());
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    drop(permit);
    delivery.await;
    assert_eq!(store.submits.load(Ordering::SeqCst), 1);
}

struct GatedCommandAdmission {
    entered: Notify,
    release: Notify,
}
impl CreationStorage for GatedCommandAdmission {
    fn open_creation(
        &self,
        _: String,
        _: String,
        _: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(None)
        })
    }
}

#[tokio::test]
async fn a_detached_scheduled_command_keeps_its_submission_permit() {
    let service = service(
        Arc::new(TwoOwners {
            seen: Mutex::new(Vec::new()),
            fail: false,
        }),
        Arc::new(TestClock),
    );
    let id = ConversationId::new("11111111-1111-4111-8111-111111111111").unwrap();
    let permit = Arc::new(SubmissionPermit {
        _threads: service.inner.thread_admission.clone().lock_owned().await,
        _admission: service.inner.admission.clone().read_owned().await,
        _selection: service.inner.mode_changes.lock(&id).await,
    });
    let commands = Arc::new(GatedCommandAdmission {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let waiting = service.submit_command_permitted(
        commands.clone(),
        id.clone(),
        caller(),
        Initiation::owner(),
        false,
        "held-command".into(),
        SubmittedMessage {
            uploads: Vec::new(),
            text: "continue".into(),
            ..Default::default()
        },
        SubmissionMode::Queue,
        false,
        Some(permit),
    );
    let mut waiting = Box::pin(waiting);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    commands.entered.notified().await;
    drop(waiting);
    assert!(service.inner.mode_changes.try_lock(&id).is_none());
    assert!(service.inner.admission.try_write().is_err());
    assert!(service.inner.thread_admission.try_lock().is_err());
    commands.release.notify_one();
    let _released =
        tokio::time::timeout(Duration::from_secs(5), service.inner.mode_changes.lock(&id))
            .await
            .unwrap();
    assert!(service.inner.admission.try_write().is_ok());
    assert!(service.inner.thread_admission.try_lock().is_ok());
}

#[tokio::test]
async fn a_retained_schedule_intent_does_not_relabel_a_distinct_person_command() {
    let directory = tempfile::tempdir().unwrap();
    let (service, schedule, commands, _clock, row) =
        disabled_recovery_fixture(directory.path(), "test").await;
    let id = ConversationId::new(&row.conversation).unwrap();
    service
        .create(
            id.clone(),
            caller_as("create-distinct-person"),
            RequestedConversation {
                model: Some(row.model.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(schedule
        .command_evidence(&row.conversation, &row.execution_id)
        .await
        .unwrap()
        .is_some());
    // The schedule crossed its original begin_submit but its process died
    // before SDK admission. This is a different real person action, not an
    // exact retry of the automatic command's original request identity.
    let person = caller_as("distinct-person-request");
    tokio::time::timeout(
        Duration::from_secs(5),
        service.submit_command(
            commands.clone(),
            id.clone(),
            person.clone(),
            Initiation::owner(),
            false,
            row.execution_id.clone(),
            SubmittedMessage {
                text: row.prompt.clone(),
                ..Default::default()
            },
            SubmissionMode::Queue,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let committed = commands
        .read_committed(super::super::conversation_session(&id))
        .await
        .unwrap()
        .unwrap();
    let history = committed.snapshot().unwrap().invocations.clone();
    let people = service.person_turns(&id, &person, &history).await.unwrap();
    service.shutdown().await.unwrap();
    drop(service);
    commands.shutdown().await.unwrap();
    assert_eq!(
        people
            .iter()
            .map(|(_, execution)| execution.as_str())
            .collect::<Vec<_>>(),
        [row.execution_id.as_str()]
    );
}

#[tokio::test]
async fn an_admitted_scheduled_continuation_is_not_an_explicit_person_turn() {
    let directory = tempfile::tempdir().unwrap();
    // Real SQLite schedule accept -> claim_due -> begin_submit -> close/reopen;
    // real RecordStorage command + session store. No fabricated receipt/evidence.
    let (service, schedule, commands, clock, draft_row) =
        disabled_recovery_fixture(directory.path(), "test").await;
    let id = ConversationId::new(&draft_row.conversation).unwrap();
    let automatic_caller = schedule_caller(&draft_row).unwrap();
    service
        .create(
            id.clone(),
            caller_as("create-provenance-conversation"),
            RequestedConversation {
                model: Some(draft_row.model.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // The reopened store must adopt the expired ORIGINAL fence. Obtain the
    // actual row as decoded from SQLite, including its retained non-content
    // command evidence; do not set command_evidence on a struct in the test.
    let adopted_at = 5_000 + FENCE_LEASE_MS;
    clock.0.store(adopted_at, Ordering::SeqCst);
    let rows = schedule.claim_due(adopted_at, 8).await.unwrap();
    let stored = rows
        .into_iter()
        .find(|row| row.execution_id == draft_row.execution_id)
        .expect("the durable submitting row is recoverable");
    assert!(stored.command_evidence.is_some());
    assert_eq!(
        schedule
            .status(&stored.execution_id)
            .await
            .unwrap()
            .as_deref(),
        Some("submitting")
    );
    let original_binding = super::scheduled_submit_binding(&id, &stored).unwrap();

    // Invoke the real scheduler recovery/effect owner, not submit_command with
    // a caller-supplied boolean claiming to be scheduled. It acquires the real
    // thread/admission/selection permit and performs begin_submit adoption.
    tokio::time::timeout(Duration::from_secs(5), service.deliver_due_scheduled())
        .await
        .expect("automatic delivery completes within the existing five-second fixture bound");
    assert_eq!(
        schedule
            .status(&stored.execution_id)
            .await
            .unwrap()
            .as_deref(),
        Some("delivered"),
        "the proposed regression must exercise a genuinely admitted automatic command"
    );
    let receipts = commands.mutations("person".into()).await.unwrap();
    let dispatched = receipts
        .iter()
        .find(|receipt| receipt.binding() == &original_binding)
        .expect("the original retained canonical binding has a real receipt");
    assert_eq!(dispatched.stage(), MutationStage::Settled);
    assert_eq!(dispatched.outcome(), Some(MutationOutcome::Dispatched));

    let committed = commands
        .read_committed(super::super::conversation_session(&id))
        .await
        .unwrap()
        .unwrap();
    let history = committed.snapshot().unwrap().invocations.clone();
    let automatic = history
        .iter()
        .find(|record| record.request.execution_id.as_str() == stored.execution_id)
        .expect("the scheduled command was actually saved in the SDK history");
    assert_eq!(
        automatic.request.user_message.sender(),
        &nessa_sdk::domain::agent_execution::prompts::MessageSender::Person
    );
    let classified_after_automatic = service
        .person_turns(&id, &automatic_caller, &history)
        .await
        .unwrap();
    schedule.fail_next_command_evidence_read();
    let failed_evidence_read = service.person_turns(&id, &automatic_caller, &history).await;

    // Both positives use genuine public person admission. Identical prompt,
    // surface, and a scheduling-looking execution spelling must not substitute
    // for durable origin evidence. A prefix filter would fail this positive.
    let genuine = "explicit-person-after-automatic".to_owned();
    let lookalike = format!("{}-person-lookalike", stored.execution_id);
    for execution in [&genuine, &lookalike] {
        service
            .submit_command(
                commands.clone(),
                id.clone(),
                caller_as(execution),
                Initiation::owner(),
                false,
                execution.clone(),
                SubmittedMessage {
                    text: stored.prompt.clone(),
                    ..Default::default()
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
    }
    let committed = commands
        .read_committed(super::super::conversation_session(&id))
        .await
        .unwrap()
        .unwrap();
    let history = committed.snapshot().unwrap().invocations.clone();
    let classified_after_people = service
        .person_turns(&id, &automatic_caller, &history)
        .await
        .unwrap();

    // Cleanup precedes the intended failing regression assertion, so the
    // negative control does not leave the real process/storage fixture running.
    service.shutdown().await.unwrap();
    drop(service);
    commands.shutdown().await.unwrap();

    assert!(
        classified_after_automatic.is_empty(),
        "an admitted automatic continuation is not an explicit person turn: {classified_after_automatic:?}"
    );
    assert!(matches!(
        failed_evidence_read,
        Err(ConversationError::Metadata)
    ));
    let people: Vec<_> = classified_after_people
        .iter()
        .map(|(_, execution)| execution.as_str())
        .collect();
    assert_eq!(
        people,
        vec![genuine.as_str(), lookalike.as_str()],
        "only real person receipts reauthorize report delivery; spelling and text confer no origin"
    );
}

// One real resolved SDK owner competes at the actual resume/export/delivery
// entry points. The existing scheduled fixture refuses its stale target after
// admission; this does not claim provider dispatch or the historical #1030 cause.

#[derive(Default)]
struct AdmissionOrderMarks(
    Mutex<HashMap<ConversationId, crate::conversation::application::UnfinishedMark>>,
);

impl crate::conversation::application::UnfinishedTurns for AdmissionOrderMarks {
    fn mark(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        self.0.lock().unwrap().insert(
            id.clone(),
            crate::conversation::application::UnfinishedMark {
                conversation_id: id.clone(),
                failures: 0,
                retry_at_ms: None,
            },
        );
        Box::pin(async { Ok(()) })
    }
    fn clear(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        self.0.lock().unwrap().remove(id);
        Box::pin(async { Ok(()) })
    }
    fn defer(
        &self,
        id: &ConversationId,
        failures: u32,
        retry_at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        self.0.lock().unwrap().insert(
            id.clone(),
            crate::conversation::application::UnfinishedMark {
                conversation_id: id.clone(),
                failures,
                retry_at_ms: Some(retry_at_ms),
            },
        );
        Box::pin(async { Ok(()) })
    }
    fn marked(
        &self,
    ) -> ConversationFuture<'_, Vec<crate::conversation::application::UnfinishedMark>> {
        let marks = self.0.lock().unwrap().values().cloned().collect();
        Box::pin(async move { Ok(marks) })
    }
}

#[tokio::test]
async fn recovery_export_and_scheduled_delivery_share_one_admission_order() {
    use futures_util::task::{waker_ref, ArcWake};
    use std::task::Context;

    struct RecoveryWake(AtomicUsize);
    impl ArcWake for RecoveryWake {
        fn wake_by_ref(owner: &Arc<Self>) {
            owner.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let bound = Duration::from_secs(5);
    let clock = Arc::new(ShiftClock(AtomicU64::new(5_000)));
    let store = Arc::new(RecordingStore::new(true));
    let service = delivering(&clock, &store, "owner");
    // These are different conversations: the thread guard is global. The
    // existing delivery fixture's stale target must still complete its normal
    // begin_submit/refusal cleanup once admitted. No nonexistent SDK owner is
    // substituted for recovery's real resolved one.
    let id = ConversationId::new("22222222-2222-4222-8222-222222222222").unwrap();
    service
        .create(
            id.clone(),
            caller_as("create-admission-order"),
            RequestedConversation {
                model: Some("test".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let live = service
        .resolve(&id, &caller_as("resolve-admission-order"))
        .await
        .unwrap();
    tokio::time::timeout(bound, live.join_attachment_owner())
        .await
        .unwrap()
        .unwrap();
    assert!(!live.agent.awaits_close());
    assert!(matches!(
        live.agent.attachment_status().phase(),
        nessa_sdk::application::agent_execution::agents::AttachmentPhase::Attached
            | nessa_sdk::application::agent_execution::agents::AttachmentPhase::Absent
    ));
    // Observe the same full slot precondition without widening the private
    // resumption helper's production visibility for this test.
    {
        let slots = service.inner.conversations.lock().await;
        assert!(slots.get(&id).is_some_and(|slot| {
            !slot.stopping.load(Ordering::SeqCst)
                && matches!(slot.value.get(), Some(Ok(open)) if Arc::ptr_eq(open, &live))
        }));
    }
    assert!(live.agent.is_idle().await);
    let marks = Arc::new(AdmissionOrderMarks::default());
    service.bind_unfinished_turns(
        marks.clone(),
        crate::conversation::application::FailureBackoff::default(),
    );
    service.mark_unfinished(&id).await.unwrap();

    let initial = service.inner.admission.clone().write_owned().await;
    let wake = Arc::new(RecoveryWake(AtomicUsize::new(0)));
    let waker = waker_ref(&wake);
    let mut context = Context::from_waker(&waker);
    let mut recovery = Box::pin(service.resume_owed(&id, live.clone()));
    assert!(std::future::Future::poll(recovery.as_mut(), &mut context).is_pending());
    let recovery_owned_thread_before_delivery = service.inner.thread_admission.try_lock().is_err();
    // Writer FIFO is essential to the reported cycle: queue this writer after
    // recovery's reader and before delivery's reader.
    let mut export = Box::pin(service.hold_for_export());
    assert!(futures_util::poll!(&mut export).is_pending());
    let mut delivery = Box::pin(service.deliver_due_scheduled());
    assert!(futures_util::poll!(&mut delivery).is_pending());
    let thread_owned_after_delivery_poll = service.inner.thread_admission.try_lock().is_err();
    assert_eq!(store.submits.load(Ordering::SeqCst), 0);
    wake.0.store(0, Ordering::SeqCst);
    drop(initial);
    // The release synchronously wakes a queued reader. This distinguishes an
    // actual admission waiter from an earlier attachment-owner/SDK wait.
    let reached_admission = wake.0.load(Ordering::SeqCst) > 0;

    let mut recovery_result = None;
    let mut delivery_done = false;
    let finished = tokio::time::timeout(bound, async {
        futures_util::join!(
            async {
                recovery_result = Some(recovery.as_mut().await);
            },
            async {
                let held = export.as_mut().await.unwrap();
                drop(held); // The export cut ends; do not hold it over join!
            },
            async {
                delivery.as_mut().await;
                delivery_done = true;
            },
        )
    })
    .await;
    let stalled = finished.is_err();
    // Cancellation here belongs only to the test's export waiter. In the
    // negative control it breaks writer preference so real delivery and
    // recovery can finish and release their own actual guards before failure.
    drop(export);
    if stalled {
        tokio::time::timeout(bound, async {
            futures_util::join!(
                async {
                    if recovery_result.is_none() {
                        recovery_result = Some(recovery.as_mut().await);
                    }
                },
                async {
                    if !delivery_done {
                        delivery.as_mut().await;
                        delivery_done = true;
                    }
                },
            )
        })
        .await
        .expect("test-owned writer cancellation permits real cleanup");
    }
    drop(recovery);
    drop(delivery);
    let no_thread_owner = service.inner.thread_admission.try_lock().is_ok();
    let no_admission_owner = service.inner.admission.try_write().is_ok();
    let submitted = store.submits.load(Ordering::SeqCst);
    tokio::time::timeout(bound, service.shutdown())
        .await
        .unwrap()
        .unwrap();
    drop(live);

    assert!(
        reached_admission,
        "recovery must reach admission, not an earlier attachment wait"
    );
    assert_eq!(
        recovery_result,
        Some(true),
        "the real idle marked owner completes reconciliation"
    );
    assert!(
        delivery_done,
        "the actual scheduled delivery future completes"
    );
    assert_eq!(
        submitted, 1,
        "scheduled admission actually crosses begin_submit once"
    );
    assert!(
        no_thread_owner && no_admission_owner,
        "actual owners release their guards"
    );
    assert!(
        thread_owned_after_delivery_poll,
        "one real branch must hold thread admission"
    );
    assert!(!stalled,
        "recovery/export/delivery admission cycle: recovery_owned_thread_before_delivery={recovery_owned_thread_before_delivery}, thread_owned_after_delivery_poll={thread_owned_after_delivery_poll}, reached_admission={reached_admission}");
}
