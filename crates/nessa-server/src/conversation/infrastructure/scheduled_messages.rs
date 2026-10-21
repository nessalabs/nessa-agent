//! The private SQLite dataset of usage-limit continuations
//! (`scheduled-chat-messages.sqlite3`, `scheduled_messages.sql`).
//!
//! Schedule and cancellation audits commit with the status change. The
//! delivery audit commits in the transaction that records the outcome, which
//! runs only after `begin_submit` has moved the row to `submitting` and taken
//! a lease. Cancel, defer, and a model change match `scheduled` and
//! `delivering` only, so they cannot land once that transition has, and the
//! transition fails once they have. If the outcome transaction fails, the row
//! stays `submitting` and keeps its execution id. Another process adopts that
//! row only after the lease expires. A preference allow and a preference deny
//! each commit their own audit, with the policy ids, before the command result.

use crate::conversation::application::{
    ConversationError, ConversationFuture, PreferenceReading, PreferenceUpdate, ReconcileRetry,
    RecordedPreference, ScheduleActor, ScheduleDraft, ScheduleSettlement, ScheduledMessage,
    ScheduledMessageStore, FENCE_LEASE_MS,
};
use crate::conversation::domain::ContinuationCause;
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension, Transaction},
    OpenError, Schema,
};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

const DEFINITION: &str = include_str!("scheduled_messages.sql");

/// Scheduled continuations, one open row per conversation.
pub struct LocalScheduledMessages {
    connection: Arc<Mutex<Connection>>,
    /// This process's lease owner. Another open of the same file has another.
    owner: String,
    /// Execution ids whose `submitting` row this process is inside.
    ///
    /// The connection mutex is taken before this one. The set is only this
    /// process's re-entry guard. The durable owner is `fence_owner` until
    /// `fence_until_ms`. A restart drops the set and leaves the lease.
    armed: Arc<Mutex<HashSet<String>>>,
    /// The next delivery-outcome transaction fails before it commits.
    #[cfg(test)]
    fail_outcome_audit: AtomicBool,
    /// The next `begin_submit` fails at commit, before the id is armed.
    #[cfg(test)]
    fail_begin_commit: AtomicBool,
    /// The next preference decision fails before its audit commits.
    #[cfg(test)]
    fail_decision_audit: AtomicBool,
    /// The next submit-retry transaction fails before its audit commits.
    #[cfg(test)]
    fail_defer_audit: AtomicBool,
}

impl LocalScheduledMessages {
    /// Open the database at `path`, creating it when absent.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection = nessa_local_database::open(path, &Schema::new(DEFINITION)?)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            owner: uuid::Uuid::new_v4().to_string(),
            armed: Arc::new(Mutex::new(HashSet::new())),
            #[cfg(test)]
            fail_outcome_audit: AtomicBool::new(false),
            #[cfg(test)]
            fail_begin_commit: AtomicBool::new(false),
            #[cfg(test)]
            fail_decision_audit: AtomicBool::new(false),
            #[cfg(test)]
            fail_defer_audit: AtomicBool::new(false),
        })
    }

    /// The next [`ScheduledMessageStore::record_outcome`] fails and rolls back.
    #[cfg(test)]
    pub fn fail_next_outcome_audit(&self) {
        self.fail_outcome_audit.store(true, Ordering::SeqCst);
    }

    /// The next [`ScheduledMessageStore::begin_submit`] fails at commit and
    /// does not arm the execution id.
    #[cfg(test)]
    pub fn fail_next_begin_commit(&self) {
        self.fail_begin_commit.store(true, Ordering::SeqCst);
    }

    /// The next preference decision rolls back, preference included.
    #[cfg(test)]
    pub fn fail_next_decision_audit(&self) {
        self.fail_decision_audit.store(true, Ordering::SeqCst);
    }

    /// The next [`ScheduledMessageStore::defer_submit`] fails and rolls back.
    #[cfg(test)]
    pub fn fail_next_defer_audit(&self) {
        self.fail_defer_audit.store(true, Ordering::SeqCst);
    }

    /// Initiator of each preference audit, oldest first.
    #[cfg(test)]
    pub fn preference_audit_initiators(&self) -> Vec<String> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut statement = connection
            .prepare("SELECT initiator_kind FROM audit WHERE cause = 'preference' ORDER BY id")
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    /// The outcome audit's clock for `execution_id` at `after_status`.
    #[cfg(test)]
    pub fn audit_instant(&self, execution_id: &str, after_status: &str) -> Option<i64> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        connection
            .query_row(
                "SELECT at_ms FROM audit WHERE execution_id = ?1 AND after_status = ?2 \
                 ORDER BY id DESC LIMIT 1",
                params![execution_id, after_status],
                |row| row.get(0),
            )
            .optional()
            .unwrap()
    }

    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T, ConversationError> + Send + 'static,
    ) -> ConversationFuture<'_, T> {
        let connection = self.connection.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut connection = connection.lock().unwrap_or_else(PoisonError::into_inner);
                work(&mut connection)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}

fn failed(error: nessa_local_database::rusqlite::Error) -> ConversationError {
    tracing::warn!(%error, "the scheduled message store did not answer");
    ConversationError::Metadata
}

fn millis(value: u64) -> Result<i64, ConversationError> {
    i64::try_from(value).map_err(|_| ConversationError::InvalidInput)
}

struct AuditChange<'a> {
    at_ms: i64,
    conversation: &'a str,
    causal_execution: &'a str,
    execution_id: &'a str,
    before_status: Option<&'a str>,
    after_status: &'a str,
    cause: &'a str,
    actor: &'a ScheduleActor,
}

fn audit(transaction: &Transaction<'_>, change: &AuditChange<'_>) -> Result<(), ConversationError> {
    audit_policies(transaction, change, "")
}

fn audit_policies(
    transaction: &Transaction<'_>,
    change: &AuditChange<'_>,
    policies: &str,
) -> Result<(), ConversationError> {
    transaction
        .execute(
            "INSERT INTO audit (at_ms, conversation, causal_execution, execution_id, \
             before_status, after_status, cause, initiator_kind, organization_id, \
             principal_id, surface_id, action_id, policies) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                change.at_ms,
                change.conversation,
                change.causal_execution,
                change.execution_id,
                change.before_status,
                change.after_status,
                change.cause,
                change.actor.kind(),
                change.actor.organization_id,
                change.actor.principal_id,
                change.actor.surface_id,
                change.actor.action_id,
                policies,
            ],
        )
        .map_err(failed)?;
    Ok(())
}

fn read_message(
    row: &nessa_local_database::rusqlite::Row<'_>,
) -> Result<ScheduledMessage, nessa_local_database::rusqlite::Error> {
    let status: String = row.get(4)?;
    Ok(ScheduledMessage {
        conversation: row.get(0)?,
        causal_execution: row.get(1)?,
        execution_id: row.get(2)?,
        due_at_ms: u64::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
        delivering: status == "delivering" || status == "submitting",
        agent: row.get(5)?,
        model: row.get(6)?,
        prompt: row.get(7)?,
        organization_id: row.get(8)?,
        principal_id: row.get(9)?,
        surface_id: row.get(10)?,
        action_id: row.get(11)?,
        initiation: row.get(12)?,
        grant_id: row.get(13)?,
        policy_revision: row.get::<_, Option<i64>>(14)?.map(|value| value as u64),
        tools_checked: row.get::<_, i64>(15)? == 1,
        retry_failures: u32::try_from(row.get::<_, i64>(16)?).unwrap_or(0),
    })
}

const MESSAGE_COLUMNS: &str = "conversation, causal_execution, execution_id, due_at_ms, status, \
     agent, model, prompt, organization_id, principal_id, surface_id, action_id, \
     initiation, grant_id, policy_revision, tools_checked, retry_failures";

fn load_open(
    connection: &Connection,
    conversation: &str,
) -> Result<Option<ScheduledMessage>, ConversationError> {
    connection
        .query_row(
            &format!(
                "SELECT {MESSAGE_COLUMNS} FROM message \
                 WHERE conversation = ?1 AND status IN ('scheduled', 'delivering', 'submitting')"
            ),
            params![conversation],
            read_message,
        )
        .optional()
        .map_err(failed)
}

fn cancel_matching(
    transaction: &Transaction<'_>,
    sql_tail: &str,
    params: &[&dyn nessa_local_database::rusqlite::ToSql],
    cause: &str,
    actor: &ScheduleActor,
    at_ms: i64,
) -> Result<usize, ConversationError> {
    let rows = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT conversation, causal_execution, execution_id, status FROM message {sql_tail}"
            ))
            .map_err(failed)?;
        let selected = statement
            .query_map(
                nessa_local_database::rusqlite::params_from_iter(params.iter()),
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(failed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?;
        selected
    };
    let mut cancelled = 0;
    for (conversation, causal, execution, before) in rows {
        let changed = transaction
            .execute(
                "UPDATE message SET status = 'cancelled' \
                 WHERE conversation = ?1 AND causal_execution = ?2 AND status = ?3",
                params![conversation, causal, before],
            )
            .map_err(failed)?;
        if changed == 1 {
            audit(
                transaction,
                &AuditChange {
                    at_ms,
                    conversation: &conversation,
                    causal_execution: &causal,
                    execution_id: &execution,
                    before_status: Some(&before),
                    after_status: "cancelled",
                    cause,
                    actor,
                },
            )?;
            cancelled += 1;
        }
    }
    Ok(cancelled)
}

/// Cancel `submitting` rows whose lease is at or before `now_ms`. A live lease stays.
fn cancel_expired_submitting(
    transaction: &Transaction<'_>,
    now_ms: i64,
    actor: &ScheduleActor,
    at_ms: i64,
    cause: &str,
) -> Result<usize, ConversationError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT conversation, causal_execution, execution_id FROM message \
                 WHERE status = 'submitting' AND fence_until_ms <= ?1",
            )
            .map_err(failed)?;
        let selected = statement
            .query_map(params![now_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(failed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?;
        selected
    };
    let mut cancelled = 0;
    for (conversation, causal, execution) in rows {
        let changed = transaction
            .execute(
                "UPDATE message SET status = 'cancelled', fence_owner = '', fence_until_ms = 0 \
                 WHERE execution_id = ?1 AND status = 'submitting' AND fence_until_ms <= ?2",
                params![execution, now_ms],
            )
            .map_err(failed)?;
        if changed == 1 {
            audit(
                transaction,
                &AuditChange {
                    at_ms,
                    conversation: &conversation,
                    causal_execution: &causal,
                    execution_id: &execution,
                    before_status: Some("submitting"),
                    after_status: "cancelled",
                    cause,
                    actor,
                },
            )?;
            cancelled += 1;
        }
    }
    Ok(cancelled)
}

fn holds(armed: &Mutex<HashSet<String>>) -> std::sync::MutexGuard<'_, HashSet<String>> {
    armed.lock().unwrap_or_else(PoisonError::into_inner)
}

fn cancel_open_preference(
    transaction: &Transaction<'_>,
    actor: &ScheduleActor,
    at_ms: i64,
) -> Result<(), ConversationError> {
    cancel_matching(
        transaction,
        "WHERE status IN ('scheduled', 'delivering')",
        &[],
        ContinuationCause::PreferenceDisabled.as_str(),
        actor,
        at_ms,
    )?;
    cancel_expired_submitting(
        transaction,
        at_ms,
        actor,
        at_ms,
        ContinuationCause::PreferenceDisabled.as_str(),
    )?;
    Ok(())
}

fn lease_until(now_ms: u64) -> Result<i64, ConversationError> {
    millis(now_ms.saturating_add(FENCE_LEASE_MS))
}

struct PreferenceStanding {
    enabled: bool,
    revision: u64,
    settled: bool,
}

fn standing(connection: &Connection) -> Result<PreferenceStanding, ConversationError> {
    let row: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT enabled, revision, settled FROM preference WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(failed)?;
    Ok(match row {
        Some((enabled, revision, settled)) => PreferenceStanding {
            enabled: enabled == 1,
            revision: u64::try_from(revision).unwrap_or(0),
            settled: settled == 1,
        },
        None => PreferenceStanding {
            enabled: false,
            revision: 0,
            settled: true,
        },
    })
}

impl ScheduledMessageStore for LocalScheduledMessages {
    fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
        self.run(|connection| {
            let current = standing(connection)?;
            Ok(PreferenceReading {
                enabled: current.enabled,
                revision: current.revision,
            })
        })
    }

    fn set_preference(
        &self,
        enabled: bool,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let previous = standing(&transaction)?;
            if previous.enabled == enabled {
                transaction.commit().map_err(failed)?;
                return Ok(enabled);
            }
            let changed = transaction
                .execute(
                    "INSERT INTO preference (id, enabled, revision, settled) \
                     VALUES (1, ?1, 1, 1) \
                     ON CONFLICT (id) DO UPDATE SET \
                       enabled = excluded.enabled, \
                       revision = preference.revision + 1, \
                       settled = 1 \
                     WHERE preference.enabled != excluded.enabled",
                    params![i64::from(enabled)],
                )
                .map_err(failed)?;
            if changed != 1 {
                transaction.commit().map_err(failed)?;
                return Ok(previous.enabled);
            }
            let was = previous.enabled;
            audit(
                &transaction,
                &AuditChange {
                    at_ms,
                    conversation: "gateway",
                    causal_execution: "preference",
                    execution_id: actor.action_id.as_deref().unwrap_or("preference"),
                    before_status: Some(if was { "on" } else { "off" }),
                    after_status: if enabled { "on" } else { "off" },
                    cause: "preference",
                    actor: &actor,
                },
            )?;
            if !enabled {
                cancel_open_preference(&transaction, &actor, at_ms)?;
            }
            transaction.commit().map_err(failed)?;
            Ok(enabled)
        })
    }

    fn record_preference_decision(
        &self,
        allowed: bool,
        enabled: bool,
        policies: &[String],
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, RecordedPreference> {
        let policies_text = policies.join("\n");
        let policies = policies.to_vec();
        #[cfg(test)]
        let fail = self.fail_decision_audit.swap(false, Ordering::SeqCst);
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let previous = standing(&transaction)?;
            let mut stored = if allowed { enabled } else { previous.enabled };
            let mut installed_enable = false;
            let mut changed_value = false;
            if allowed && previous.enabled != enabled {
                let changed = if enabled {
                    transaction
                        .execute(
                            "INSERT INTO preference (id, enabled, revision, settled) \
                             VALUES (1, 1, 1, 0) \
                             ON CONFLICT (id) DO UPDATE SET \
                               enabled = 1, \
                               revision = preference.revision + 1, \
                               settled = 0 \
                             WHERE preference.enabled = 0",
                            [],
                        )
                        .map_err(failed)?
                } else {
                    transaction
                        .execute(
                            "UPDATE preference SET enabled = 0, revision = revision + 1, \
                             settled = 1 \
                             WHERE id = 1 AND enabled = 1",
                            [],
                        )
                        .map_err(failed)?
                };
                if changed == 1 {
                    changed_value = true;
                    installed_enable = enabled;
                    stored = enabled;
                    if !enabled {
                        cancel_open_preference(&transaction, &actor, at_ms)?;
                    }
                } else {
                    stored = standing(&transaction)?.enabled;
                }
            }
            let revision = standing(&transaction)?.revision;
            let before = if changed_value {
                if previous.enabled {
                    "on"
                } else {
                    "off"
                }
            } else if stored {
                "on"
            } else {
                "off"
            };
            audit_policies(
                &transaction,
                &AuditChange {
                    at_ms,
                    conversation: "gateway",
                    causal_execution: "preference",
                    execution_id: actor.action_id.as_deref().unwrap_or("preference"),
                    before_status: Some(before),
                    after_status: if stored { "on" } else { "off" },
                    cause: if allowed {
                        "preference_allowed"
                    } else {
                        "preference_denied"
                    },
                    actor: &actor,
                },
                &policies_text,
            )?;
            #[cfg(test)]
            if fail {
                return Err(ConversationError::Metadata);
            }
            transaction.commit().map_err(failed)?;
            let update = if allowed {
                PreferenceUpdate::Stored(stored)
            } else {
                PreferenceUpdate::Refused { policies }
            };
            Ok(RecordedPreference {
                update,
                revision,
                installed_enable,
            })
        })
    }

    fn settle_installed_preference(&self, revision: u64) -> ConversationFuture<'_, bool> {
        self.run(move |connection| {
            let stored_revision =
                i64::try_from(revision).map_err(|_| ConversationError::InvalidInput)?;
            let transaction = connection.transaction().map_err(failed)?;
            let changed = transaction
                .execute(
                    "UPDATE preference SET settled = 1 \
                     WHERE id = 1 AND revision = ?1 AND enabled = 1 AND settled = 0",
                    params![stored_revision],
                )
                .map_err(failed)?;
            let settled = if changed == 1 {
                true
            } else {
                let current = standing(&transaction)?;
                current.enabled && current.settled && current.revision == revision
            };
            transaction.commit().map_err(failed)?;
            Ok(settled)
        })
    }

    fn revert_installed_preference(
        &self,
        revision: u64,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        self.run(move |connection| {
            let revision = i64::try_from(revision).map_err(|_| ConversationError::InvalidInput)?;
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let changed = transaction
                .execute(
                    "UPDATE preference SET enabled = 0, revision = revision + 1, settled = 1 \
                     WHERE id = 1 AND revision = ?1 AND enabled = 1 AND settled = 0",
                    params![revision],
                )
                .map_err(failed)?;
            if changed != 1 {
                transaction.commit().map_err(failed)?;
                return Ok(false);
            }
            audit(
                &transaction,
                &AuditChange {
                    at_ms,
                    conversation: "gateway",
                    causal_execution: "preference",
                    execution_id: actor.action_id.as_deref().unwrap_or("preference"),
                    before_status: Some("on"),
                    after_status: "off",
                    cause: "preference",
                    actor: &actor,
                },
            )?;
            cancel_open_preference(&transaction, &actor, at_ms)?;
            transaction.commit().map_err(failed)?;
            Ok(true)
        })
    }

    fn open(&self, conversation: &str) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        let conversation = conversation.to_owned();
        self.run(move |connection| load_open(connection, &conversation))
    }

    fn accept(
        &self,
        draft: ScheduleDraft,
        at_ms: u64,
    ) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let due = millis(draft.due_at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let current = standing(&transaction)?;
            if !current.enabled || current.revision != draft.preference_revision {
                transaction.commit().map_err(failed)?;
                return Ok(None);
            }
            let existing: Option<(String, String, i64)> = transaction
                .query_row(
                    "SELECT execution_id, status, due_at_ms FROM message \
                     WHERE conversation = ?1 AND causal_execution = ?2",
                    params![draft.conversation, draft.causal_execution],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(failed)?;
            if let Some((execution_id, status, due_at_ms)) = existing {
                if status == "submitting" {
                    transaction.commit().map_err(failed)?;
                    return Ok(None);
                }
                if status != "scheduled" && status != "delivering" {
                    transaction.commit().map_err(failed)?;
                    return Ok(None);
                }
                if status == "scheduled" && due_at_ms != due {
                    let changed = transaction
                        .execute(
                            "UPDATE message SET due_at_ms = ?3 \
                             WHERE conversation = ?1 AND causal_execution = ?2 AND status = 'scheduled'",
                            params![draft.conversation, draft.causal_execution, due],
                        )
                        .map_err(failed)?;
                    if changed == 1 {
                        audit(
                            &transaction,
                            &AuditChange {
                                at_ms,
                                conversation: &draft.conversation,
                                causal_execution: &draft.causal_execution,
                                execution_id: &execution_id,
                                before_status: Some("scheduled"),
                                after_status: "scheduled",
                                cause: ContinuationCause::ResetMoved.as_str(),
                                actor: &ScheduleActor::system(),
                            },
                        )?;
                    }
                } else if status == "delivering" && due > due_at_ms {
                    let changed = transaction
                        .execute(
                            "UPDATE message SET status = 'scheduled', due_at_ms = ?3 \
                             WHERE conversation = ?1 AND causal_execution = ?2 AND status = 'delivering'",
                            params![draft.conversation, draft.causal_execution, due],
                        )
                        .map_err(failed)?;
                    if changed == 1 {
                        audit(
                            &transaction,
                            &AuditChange {
                                at_ms,
                                conversation: &draft.conversation,
                                causal_execution: &draft.causal_execution,
                                execution_id: &execution_id,
                                before_status: Some("delivering"),
                                after_status: "scheduled",
                                cause: ContinuationCause::ResetMoved.as_str(),
                                actor: &ScheduleActor::system(),
                            },
                        )?;
                    }
                }
                transaction.commit().map_err(failed)?;
                return load_open(connection, &draft.conversation);
            }
            let submitting: bool = transaction
                .query_row(
                    "SELECT EXISTS( \
                         SELECT 1 FROM message \
                         WHERE conversation = ?1 AND status = 'submitting' \
                     )",
                    params![draft.conversation],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            if submitting {
                transaction.commit().map_err(failed)?;
                return Ok(None);
            }
            cancel_matching(
                &transaction,
                "WHERE conversation = ?1 AND status IN ('scheduled', 'delivering')",
                &[&draft.conversation],
                ContinuationCause::Superseded.as_str(),
                &draft.actor,
                at_ms,
            )?;
            transaction
                .execute(
                    "INSERT INTO message (conversation, causal_execution, execution_id, due_at_ms, \
                     status, agent, model, prompt, organization_id, principal_id, surface_id, \
                     action_id, initiation, grant_id, policy_revision, tools_checked) \
                     VALUES (?1, ?2, ?3, ?4, 'scheduled', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    params![
                        draft.conversation,
                        draft.causal_execution,
                        draft.execution_id,
                        due,
                        draft.agent,
                        draft.model,
                        draft.prompt,
                        draft.organization_id,
                        draft.principal_id,
                        draft.surface_id,
                        draft.action_id,
                        draft.initiation,
                        draft.grant_id,
                        draft.policy_revision.map(|value| value as i64),
                        i64::from(draft.tools_checked),
                    ],
                )
                .map_err(failed)?;
            audit(
                &transaction,
                &AuditChange {
                    at_ms,
                    conversation: &draft.conversation,
                    causal_execution: &draft.causal_execution,
                    execution_id: &draft.execution_id,
                    before_status: None,
                    after_status: "scheduled",
                    cause: draft.cause.as_str(),
                    actor: &draft.actor,
                },
            )?;
            let conversation = draft.conversation.clone();
            transaction.commit().map_err(failed)?;
            load_open(connection, &conversation)
        })
    }

    fn move_due(
        &self,
        conversation: &str,
        causal_execution: &str,
        due_at_ms: u64,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        let conversation = conversation.to_owned();
        let causal_execution = causal_execution.to_owned();
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let due = millis(due_at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let existing: Option<(String, i64)> = transaction
                .query_row(
                    "SELECT execution_id, due_at_ms FROM message \
                     WHERE conversation = ?1 AND causal_execution = ?2 AND status = 'scheduled'",
                    params![conversation, causal_execution],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(failed)?;
            let Some((execution_id, previous)) = existing else {
                transaction.commit().map_err(failed)?;
                return Ok(false);
            };
            if previous == due {
                transaction.commit().map_err(failed)?;
                return Ok(true);
            }
            transaction
                .execute(
                    "UPDATE message SET due_at_ms = ?3 \
                     WHERE conversation = ?1 AND causal_execution = ?2 AND status = 'scheduled'",
                    params![conversation, causal_execution, due],
                )
                .map_err(failed)?;
            audit(
                &transaction,
                &AuditChange {
                    at_ms,
                    conversation: &conversation,
                    causal_execution: &causal_execution,
                    execution_id: &execution_id,
                    before_status: Some("scheduled"),
                    after_status: "scheduled",
                    cause: ContinuationCause::ResetMoved.as_str(),
                    actor: &ScheduleActor::system(),
                },
            )?;
            transaction.commit().map_err(failed)?;
            Ok(true)
        })
    }

    fn defer_claim(
        &self,
        conversation: &str,
        causal_execution: &str,
        due_at_ms: u64,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        let conversation = conversation.to_owned();
        let causal_execution = causal_execution.to_owned();
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let due = millis(due_at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let execution_id: Option<String> = transaction
                .query_row(
                    "SELECT execution_id FROM message \
                     WHERE conversation = ?1 AND causal_execution = ?2 AND status = 'delivering'",
                    params![conversation, causal_execution],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)?;
            let Some(execution_id) = execution_id else {
                transaction.commit().map_err(failed)?;
                return Ok(false);
            };
            let changed = transaction
                .execute(
                    "UPDATE message SET status = 'scheduled', due_at_ms = ?3 \
                     WHERE conversation = ?1 AND causal_execution = ?2 AND status = 'delivering'",
                    params![conversation, causal_execution, due],
                )
                .map_err(failed)?;
            if changed == 1 {
                audit(
                    &transaction,
                    &AuditChange {
                        at_ms,
                        conversation: &conversation,
                        causal_execution: &causal_execution,
                        execution_id: &execution_id,
                        before_status: Some("delivering"),
                        after_status: "scheduled",
                        cause: ContinuationCause::ResetMoved.as_str(),
                        actor: &ScheduleActor::system(),
                    },
                )?;
            }
            transaction.commit().map_err(failed)?;
            Ok(changed == 1)
        })
    }

    fn cancel_open(
        &self,
        conversation: &str,
        cause: ContinuationCause,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        let conversation = conversation.to_owned();
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let cancelled = cancel_matching(
                &transaction,
                "WHERE conversation = ?1 AND status IN ('scheduled', 'delivering')",
                &[&conversation],
                cause.as_str(),
                &actor,
                at_ms,
            )?;
            transaction.commit().map_err(failed)?;
            Ok(cancelled > 0)
        })
    }

    fn claim_due(
        &self,
        now_ms: u64,
        limit: usize,
    ) -> ConversationFuture<'_, Vec<ScheduledMessage>> {
        self.run(move |connection| {
            let now = millis(now_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let due = {
                let mut statement = transaction
                    .prepare(
                        "SELECT execution_id FROM message \
                         WHERE status = 'scheduled' AND due_at_ms <= ?1 \
                         ORDER BY due_at_ms LIMIT ?2",
                    )
                    .map_err(failed)?;
                let ids = statement
                    .query_map(params![now, limit as i64], |row| row.get::<_, String>(0))
                    .map_err(failed)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(failed)?;
                ids
            };
            for execution_id in &due {
                transaction
                    .execute(
                        "UPDATE message SET status = 'delivering' \
                         WHERE execution_id = ?1 AND status = 'scheduled'",
                        params![execution_id],
                    )
                    .map_err(failed)?;
            }
            transaction.commit().map_err(failed)?;
            let mut claimed = Vec::new();
            for execution_id in &due {
                if let Some(row) = connection
                    .query_row(
                        &format!("SELECT {MESSAGE_COLUMNS} FROM message WHERE execution_id = ?1"),
                        params![execution_id],
                        read_message,
                    )
                    .optional()
                    .map_err(failed)?
                {
                    if row.delivering {
                        claimed.push(row);
                    }
                }
            }
            let mut statement = connection
                .prepare(&format!(
                    "SELECT {MESSAGE_COLUMNS} FROM message \
                     WHERE status = 'delivering' \
                        OR (status = 'submitting' AND retry_at_ms <= ?1) \
                     ORDER BY due_at_ms LIMIT ?2"
                ))
                .map_err(failed)?;
            let already = statement
                .query_map(params![now, limit as i64], read_message)
                .map_err(failed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failed)?;
            for row in already {
                if claimed
                    .iter()
                    .any(|claimed| claimed.execution_id == row.execution_id)
                {
                    continue;
                }
                claimed.push(row);
                if claimed.len() >= limit {
                    break;
                }
            }
            Ok(claimed)
        })
    }

    fn next_due(&self, now_ms: u64) -> ConversationFuture<'_, Option<u64>> {
        self.run(move |connection| {
            let now = millis(now_ms)?;
            // A claim stops at its limit, and a failed outcome audit leaves the
            // row delivering. Either one is already due. A submitting row is
            // due only once its retry time has arrived; a later retry waits.
            let pending: bool = connection
                .query_row(
                    "SELECT EXISTS( \
                         SELECT 1 FROM message \
                         WHERE status = 'delivering' \
                            OR (status = 'submitting' AND retry_at_ms <= ?1) \
                            OR (status = 'scheduled' AND due_at_ms <= ?1) \
                     )",
                    params![now],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            if pending {
                return Ok(Some(now_ms));
            }
            let due: Option<i64> = connection
                .query_row(
                    "SELECT MIN(at_ms) FROM ( \
                         SELECT due_at_ms AS at_ms FROM message \
                          WHERE status = 'scheduled' AND due_at_ms > ?1 \
                         UNION ALL \
                         SELECT retry_at_ms AS at_ms FROM message \
                          WHERE status = 'submitting' AND retry_at_ms > ?1 \
                     )",
                    params![now],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            Ok(due.and_then(|due| u64::try_from(due).ok()))
        })
    }

    fn begin_submit(&self, execution_id: &str, now_ms: u64) -> ConversationFuture<'_, bool> {
        let execution_id = execution_id.to_owned();
        let owner = self.owner.clone();
        let armed = self.armed.clone();
        #[cfg(test)]
        let fail_commit = self.fail_begin_commit.swap(false, Ordering::SeqCst);
        self.run(move |connection| {
            // Same process, still inside the submit. The connection mutex is
            // held for this whole closure, and it is taken before `armed`.
            if holds(&armed).contains(&execution_id) {
                return Ok(false);
            }
            let now = millis(now_ms)?;
            let until = lease_until(now_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            // One update, so two processes cannot both become the owner.
            let claimed = transaction
                .execute(
                    "UPDATE message SET status = 'submitting', fence_owner = ?2, \
                     fence_until_ms = ?3 \
                     WHERE execution_id = ?1 AND status = 'delivering'",
                    params![execution_id, owner, until],
                )
                .map_err(failed)?
                == 1
                || transaction
                    .execute(
                        "UPDATE message SET fence_owner = ?2, fence_until_ms = ?3 \
                         WHERE execution_id = ?1 AND status = 'submitting' \
                           AND (fence_until_ms <= ?4 OR fence_owner = ?2) \
                           AND retry_at_ms <= ?4",
                        params![execution_id, owner, until, now],
                    )
                    .map_err(failed)?
                    == 1;
            if !claimed {
                transaction.commit().map_err(failed)?;
                return Ok(false);
            }
            #[cfg(test)]
            if fail_commit {
                return Err(ConversationError::Metadata);
            }
            transaction.commit().map_err(failed)?;
            Ok(holds(&armed).insert(execution_id))
        })
    }

    fn defer_submit(
        &self,
        execution_id: &str,
        failures: u32,
        retry_at_ms: u64,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        let execution_id = execution_id.to_owned();
        let owner = self.owner.clone();
        #[cfg(test)]
        let fail_audit = self.fail_defer_audit.swap(false, Ordering::SeqCst);
        self.run(move |connection| {
            if failures == 0 {
                return Err(ConversationError::InvalidInput);
            }
            let at_ms = millis(at_ms)?;
            let retry_at = millis(retry_at_ms)?;
            let failures_i = i64::from(failures);
            let previous = failures_i - 1;
            let transaction = connection.transaction().map_err(failed)?;
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT conversation, causal_execution FROM message \
                     WHERE execution_id = ?1 AND status = 'submitting' AND fence_owner = ?2 \
                       AND retry_failures = ?3",
                    params![execution_id, owner, previous],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(failed)?;
            let Some((conversation, causal)) = existing else {
                transaction.commit().map_err(failed)?;
                return Ok(false);
            };
            let changed = transaction
                .execute(
                    "UPDATE message SET retry_at_ms = ?2, retry_failures = ?3 \
                     WHERE execution_id = ?1 AND status = 'submitting' AND fence_owner = ?4 \
                       AND retry_failures = ?5",
                    params![execution_id, retry_at, failures_i, owner, previous],
                )
                .map_err(failed)?;
            if changed != 1 {
                transaction.commit().map_err(failed)?;
                return Ok(false);
            }
            #[cfg(test)]
            if fail_audit {
                return Err(ConversationError::Metadata);
            }
            audit(
                &transaction,
                &AuditChange {
                    at_ms,
                    conversation: &conversation,
                    causal_execution: &causal,
                    execution_id: &execution_id,
                    before_status: Some("submitting"),
                    after_status: "submitting",
                    cause: "submission_retry",
                    actor: &ScheduleActor::system(),
                },
            )?;
            transaction.commit().map_err(failed)?;
            Ok(true)
        })
    }

    fn renew_fence(&self, execution_id: &str, now_ms: u64) -> ConversationFuture<'_, bool> {
        let execution_id = execution_id.to_owned();
        let owner = self.owner.clone();
        self.run(move |connection| {
            let until = lease_until(now_ms)?;
            let changed = connection
                .execute(
                    "UPDATE message SET fence_until_ms = ?1 \
                     WHERE execution_id = ?2 AND status = 'submitting' AND fence_owner = ?3",
                    params![until, execution_id, owner],
                )
                .map_err(failed)?;
            Ok(changed == 1)
        })
    }

    fn release_submit(&self, execution_id: &str) -> ConversationFuture<'_, ()> {
        let execution_id = execution_id.to_owned();
        let armed = self.armed.clone();
        self.run(move |_connection| {
            holds(&armed).remove(&execution_id);
            Ok(())
        })
    }

    fn retire_unfenced_submissions(&self, at_ms: u64) -> ConversationFuture<'_, usize> {
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let enabled: Option<i64> = transaction
                .query_row("SELECT enabled FROM preference WHERE id = 1", [], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(failed)?;
            if enabled == Some(1) {
                transaction.commit().map_err(failed)?;
                return Ok(0);
            }
            let cancelled = cancel_expired_submitting(
                &transaction,
                at_ms,
                &ScheduleActor::system(),
                at_ms,
                ContinuationCause::PreferenceDisabled.as_str(),
            )?;
            transaction.commit().map_err(failed)?;
            Ok(cancelled)
        })
    }

    fn next_live_fence(&self, now_ms: u64) -> ConversationFuture<'_, Option<u64>> {
        self.run(move |connection| {
            let now = millis(now_ms)?;
            let until: Option<i64> = connection
                .query_row(
                    "SELECT MIN(fence_until_ms) FROM message \
                     WHERE status = 'submitting' AND fence_until_ms > ?1",
                    params![now],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            Ok(until.and_then(|until| u64::try_from(until).ok()))
        })
    }

    fn status(&self, execution_id: &str) -> ConversationFuture<'_, Option<String>> {
        let execution_id = execution_id.to_owned();
        self.run(move |connection| {
            connection
                .query_row(
                    "SELECT status FROM message WHERE execution_id = ?1",
                    params![execution_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)
        })
    }

    fn record_outcome(
        &self,
        execution_id: &str,
        outcome: ScheduleSettlement,
        cause: &str,
        at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        let execution_id = execution_id.to_owned();
        let cause = cause.to_owned();
        #[cfg(test)]
        let fail = self.fail_outcome_audit.swap(false, Ordering::SeqCst);
        self.run(move |connection| {
            #[cfg(test)]
            if fail {
                return Err(ConversationError::Metadata);
            }
            let at_ms = millis(at_ms)?;
            let after = outcome.status();
            let transaction = connection.transaction().map_err(failed)?;
            let existing: Option<(String, String, String, String)> = transaction
                .query_row(
                    "SELECT conversation, causal_execution, status, execution_id FROM message \
                     WHERE execution_id = ?1",
                    params![execution_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(failed)?;
            let Some((conversation, causal, status, execution_id)) = existing else {
                transaction.commit().map_err(failed)?;
                return Err(ConversationError::NotFound);
            };
            if status == after {
                transaction.commit().map_err(failed)?;
                return Ok(());
            }
            if status != "delivering" && status != "submitting" {
                transaction.commit().map_err(failed)?;
                return Ok(());
            }
            transaction
                .execute(
                    "UPDATE message SET status = ?2 \
                     WHERE execution_id = ?1 AND status = ?3",
                    params![execution_id, after, status],
                )
                .map_err(failed)?;
            audit(
                &transaction,
                &AuditChange {
                    at_ms,
                    conversation: &conversation,
                    causal_execution: &causal,
                    execution_id: &execution_id,
                    before_status: Some(&status),
                    after_status: after,
                    cause: &cause,
                    actor: &ScheduleActor::system(),
                },
            )?;
            transaction.commit().map_err(failed)?;
            Ok(())
        })
    }

    fn erase_conversation(&self, conversation: &str, at_ms: u64) -> ConversationFuture<'_, ()> {
        let conversation = conversation.to_owned();
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let rows = {
                let mut statement = transaction
                    .prepare(
                        "SELECT execution_id, causal_execution, status, prompt, agent, model, \
                         organization_id, principal_id, surface_id, action_id, grant_id, \
                         fence_until_ms \
                         FROM message WHERE conversation = ?1",
                    )
                    .map_err(failed)?;
                let selected = statement
                    .query_map(params![conversation], |row| {
                        Ok(ScrubRow {
                            execution_id: row.get(0)?,
                            causal_execution: row.get(1)?,
                            status: row.get(2)?,
                            prompt: row.get(3)?,
                            agent: row.get(4)?,
                            model: row.get(5)?,
                            organization_id: row.get(6)?,
                            principal_id: row.get(7)?,
                            surface_id: row.get(8)?,
                            action_id: row.get(9)?,
                            grant_id: row.get(10)?,
                            fence_until_ms: row.get(11)?,
                        })
                    })
                    .map_err(failed)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(failed)?;
                selected
            };
            for row in rows {
                if row.blank() {
                    continue;
                }
                let cancel = matches!(row.status.as_str(), "scheduled" | "delivering")
                    || (row.status == "submitting" && row.fence_until_ms <= at_ms);
                let after = if cancel {
                    "cancelled"
                } else {
                    row.status.as_str()
                };
                let changed = transaction
                    .execute(
                        "UPDATE message SET status = ?2, agent = '', model = '', prompt = '', \
                         organization_id = '', principal_id = '', surface_id = '', action_id = '', \
                         grant_id = NULL \
                         WHERE execution_id = ?1 AND status = ?3",
                        params![row.execution_id, after, row.status],
                    )
                    .map_err(failed)?;
                if changed == 1 {
                    audit(
                        &transaction,
                        &AuditChange {
                            at_ms,
                            conversation: &conversation,
                            causal_execution: &row.causal_execution,
                            execution_id: &row.execution_id,
                            before_status: Some(&row.status),
                            after_status: after,
                            cause: ContinuationCause::ConversationDeleted.as_str(),
                            actor: &ScheduleActor::system(),
                        },
                    )?;
                }
            }
            transaction.commit().map_err(failed)?;
            Ok(())
        })
    }

    fn note_reconcile_retry(
        &self,
        conversation: &str,
        failures: u32,
        retry_at_ms: u64,
    ) -> ConversationFuture<'_, ()> {
        let conversation = conversation.to_owned();
        self.run(move |connection| {
            let retry_at = millis(retry_at_ms)?;
            connection
                .execute(
                    "INSERT INTO reconcile_retry (conversation, failures, retry_at_ms) \
                     VALUES (?1, ?2, ?3) \
                     ON CONFLICT(conversation) DO UPDATE SET \
                       failures = excluded.failures, \
                       retry_at_ms = excluded.retry_at_ms",
                    params![conversation, i64::from(failures), retry_at],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    fn clear_reconcile_retry(&self, conversation: &str) -> ConversationFuture<'_, ()> {
        let conversation = conversation.to_owned();
        self.run(move |connection| {
            connection
                .execute(
                    "DELETE FROM reconcile_retry WHERE conversation = ?1",
                    params![conversation],
                )
                .map_err(failed)?;
            Ok(())
        })
    }

    fn reconcile_retries(&self) -> ConversationFuture<'_, Vec<ReconcileRetry>> {
        self.run(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT conversation, failures, retry_at_ms FROM reconcile_retry \
                     ORDER BY retry_at_ms",
                )
                .map_err(failed)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(ReconcileRetry {
                        conversation: row.get(0)?,
                        failures: u32::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                        retry_at_ms: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                    })
                })
                .map_err(failed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failed)?;
            Ok(rows)
        })
    }
}

struct ScrubRow {
    execution_id: String,
    causal_execution: String,
    status: String,
    prompt: String,
    agent: String,
    model: String,
    organization_id: String,
    principal_id: String,
    surface_id: String,
    action_id: String,
    grant_id: Option<String>,
    fence_until_ms: i64,
}

impl ScrubRow {
    fn blank(&self) -> bool {
        self.prompt.is_empty()
            && self.agent.is_empty()
            && self.model.is_empty()
            && self.organization_id.is_empty()
            && self.principal_id.is_empty()
            && self.surface_id.is_empty()
            && self.action_id.is_empty()
            && self.grant_id.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(conversation: &str, causal: &str, execution: &str, due: u64) -> ScheduleDraft {
        ScheduleDraft {
            conversation: conversation.into(),
            causal_execution: causal.into(),
            execution_id: execution.into(),
            due_at_ms: due,
            cause: ContinuationCause::ProviderLimit,
            actor: ScheduleActor::system(),
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
            preference_revision: 1,
        }
    }

    /// Preference on at revision 1, with no preference audit, so schedule
    /// tests can accept a row the way a planner that just read revision 1 would.
    async fn enabled_store() -> LocalScheduledMessages {
        let store = store().await;
        store
            .connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO preference (id, enabled, revision, settled) VALUES (1, 1, 1, 1)",
                [],
            )
            .unwrap();
        store
    }

    fn person() -> ScheduleActor {
        ScheduleActor {
            person: true,
            organization_id: Some("org".into()),
            principal_id: Some("person".into()),
            surface_id: Some("desktop".into()),
            action_id: Some("toggle".into()),
        }
    }

    async fn open_messages() -> LocalScheduledMessages {
        store().await
    }

    async fn store() -> LocalScheduledMessages {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("scheduled");
        nessa_local_storage::create_directory(&private).unwrap();
        let opened =
            LocalScheduledMessages::open(&private.join("scheduled-chat-messages.sqlite3")).unwrap();
        // Keep the directory alive for the connection. The file stays while the
        // connection is open; the temp dir would otherwise drop at the end of
        // this function. Leak it for the test process.
        std::mem::forget(directory);
        opened
    }

    fn audits(store: &LocalScheduledMessages) -> Vec<(String, String, String, String)> {
        let connection = store.connection.lock().unwrap();
        let mut statement = connection
            .prepare(
                "SELECT initiator_kind, cause, ifnull(before_status, ''), after_status \
                 FROM audit ORDER BY id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    #[tokio::test]
    async fn a_schedule_keeps_one_execution_and_audits_with_the_change() {
        let store = enabled_store().await;
        let accepted = store
            .accept(draft("conversation", "causal", "exec-1", 50), 10)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(accepted.execution_id, "exec-1");
        assert!(!accepted.delivering);
        let again = store
            .accept(draft("conversation", "causal", "exec-2", 80), 11)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again.execution_id, "exec-1");
        assert_eq!(again.due_at_ms, 80);
        assert_eq!(
            audits(&store),
            vec![
                (
                    "system".into(),
                    "provider_limit".into(),
                    "".into(),
                    "scheduled".into()
                ),
                (
                    "system".into(),
                    "reset_moved".into(),
                    "scheduled".into(),
                    "scheduled".into()
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_claim_is_once_and_a_failed_audit_keeps_the_execution() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        let claimed = store.claim_due(10, 4).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].execution_id, "exec-1");
        assert!(claimed[0].delivering);
        store.fail_next_outcome_audit();
        assert!(store
            .record_outcome("exec-1", ScheduleSettlement::Delivered, "delivered", 12)
            .await
            .is_err());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("delivering")
        );
        let again = store.claim_due(12, 4).await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].execution_id, "exec-1");
        store
            .record_outcome("exec-1", ScheduleSettlement::Delivered, "delivered", 13)
            .await
            .unwrap();
        assert!(store.claim_due(13, 4).await.unwrap().is_empty());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("delivered")
        );
        let delivered = audits(&store)
            .into_iter()
            .filter(|row| row.3 == "delivered")
            .count();
        assert_eq!(delivered, 1);
    }

    #[tokio::test]
    async fn turning_the_preference_off_cancels_open_rows_as_the_person() {
        let store = store().await;
        assert!(!store.preference().await.unwrap().enabled);
        store.set_preference(true, person(), 1).await.unwrap();
        store
            .accept(draft("one", "causal", "exec-1", 50), 2)
            .await
            .unwrap();
        store
            .accept(draft("two", "causal", "exec-2", 50), 2)
            .await
            .unwrap();
        assert!(!store.set_preference(false, person(), 3).await.unwrap());
        assert!(store.open("one").await.unwrap().is_none());
        assert!(store.open("two").await.unwrap().is_none());
        let preference = audits(&store)
            .into_iter()
            .find(|row| row.1 == "preference" && row.3 == "off")
            .unwrap();
        assert_eq!(preference.0, "person");
        assert_eq!(preference.2, "on");
        assert_eq!(preference.3, "off");
        let cancelled = audits(&store)
            .into_iter()
            .filter(|row| row.1 == "preference_disabled" && row.0 == "person")
            .count();
        assert_eq!(cancelled, 2);
        store.set_preference(false, person(), 4).await.unwrap();
        let preference_rows = audits(&store)
            .into_iter()
            .filter(|row| row.1 == "preference")
            .count();
        assert_eq!(preference_rows, 2);
    }

    #[tokio::test]
    async fn a_row_left_scheduled_after_the_claim_limit_is_due_now() {
        let store = enabled_store().await;
        for index in 0..9 {
            store
                .accept(
                    draft(
                        &format!("conversation-{index}"),
                        "causal",
                        &format!("exec-{index}"),
                        10,
                    ),
                    1,
                )
                .await
                .unwrap();
        }
        store
            .accept(draft("later", "causal", "exec-later", 100), 1)
            .await
            .unwrap();
        let claimed = store.claim_due(10, 8).await.unwrap();
        assert_eq!(claimed.len(), 8);
        for row in &claimed {
            store
                .record_outcome(
                    &row.execution_id,
                    ScheduleSettlement::Delivered,
                    "delivered",
                    11,
                )
                .await
                .unwrap();
        }
        assert_eq!(store.next_due(11).await.unwrap(), Some(11));
    }

    #[tokio::test]
    async fn a_delivering_row_with_a_failed_audit_is_due_now() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        assert_eq!(store.claim_due(10, 4).await.unwrap().len(), 1);
        store.fail_next_outcome_audit();
        assert!(store
            .record_outcome("exec-1", ScheduleSettlement::Delivered, "delivered", 12)
            .await
            .is_err());
        assert_eq!(store.next_due(12).await.unwrap(), Some(12));
        store
            .record_outcome("exec-1", ScheduleSettlement::Delivered, "delivered", 13)
            .await
            .unwrap();
        assert_eq!(store.next_due(13).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_future_schedule_waits_until_it_is_due() {
        let store = enabled_store().await;
        assert_eq!(store.next_due(10).await.unwrap(), None);
        store
            .accept(draft("conversation", "causal", "exec-1", 80), 1)
            .await
            .unwrap();
        assert_eq!(store.next_due(10).await.unwrap(), Some(80));
        assert_eq!(store.next_due(80).await.unwrap(), Some(80));
    }

    #[tokio::test]
    async fn a_cancel_before_the_fence_is_not_submitted_and_the_fence_blocks_a_later_cancel() {
        let store = store().await;
        store.set_preference(true, person(), 1).await.unwrap();
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(!store.set_preference(false, person(), 2).await.unwrap());
        assert!(!store.begin_submit("exec-1", 10).await.unwrap());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("cancelled")
        );

        let held = open_messages().await;
        held.set_preference(true, person(), 1).await.unwrap();
        held.accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        held.claim_due(10, 4).await.unwrap();
        assert!(held.begin_submit("exec-1", 10).await.unwrap());
        assert!(!held.begin_submit("exec-1", 10).await.unwrap());
        assert!(!held.set_preference(false, person(), 3).await.unwrap());
        assert_eq!(
            held.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        assert!(!held
            .cancel_open(
                "conversation",
                ContinuationCause::ModelChanged,
                ScheduleActor::system(),
                4
            )
            .await
            .unwrap());
        assert!(!held
            .defer_claim("conversation", "causal", 80, 5)
            .await
            .unwrap());
        assert!(held
            .accept(draft("conversation", "other", "exec-2", 90), 6)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            held.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        held.record_outcome("exec-1", ScheduleSettlement::Delivered, "delivered", 7)
            .await
            .unwrap();
        assert_eq!(
            held.status("exec-1").await.unwrap().as_deref(),
            Some("delivered")
        );
    }

    #[tokio::test]
    async fn a_defer_or_model_change_before_the_fence_refuses_the_submit() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store
            .defer_claim("conversation", "causal", 80, 2)
            .await
            .unwrap());
        assert!(!store.begin_submit("exec-1", 10).await.unwrap());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("scheduled")
        );

        let changed = enabled_store().await;
        changed
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        changed.claim_due(10, 4).await.unwrap();
        assert!(changed
            .cancel_open(
                "conversation",
                ContinuationCause::ModelChanged,
                ScheduleActor::system(),
                2
            )
            .await
            .unwrap());
        assert!(!changed.begin_submit("exec-1", 10).await.unwrap());
        assert_eq!(
            changed.status("exec-1").await.unwrap().as_deref(),
            Some("cancelled")
        );
    }

    #[tokio::test]
    async fn a_released_fence_can_be_adopted_and_preference_off_retires_an_unheld_one() {
        let store = store().await;
        store.set_preference(true, person(), 1).await.unwrap();
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 2)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        store.release_submit("exec-1").await.unwrap();
        let again = store.claim_due(11, 4).await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].execution_id, "exec-1");
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        // Off while the lease is live leaves the row. Releasing the in-memory
        // hold does not expire the lease. The next pass cancels it only once
        // `fence_until_ms` has passed.
        assert!(!store.set_preference(false, person(), 3).await.unwrap());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        store.release_submit("exec-1").await.unwrap();
        assert_eq!(store.retire_unfenced_submissions(4).await.unwrap(), 0);
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        let expired = 10 + super::FENCE_LEASE_MS;
        assert_eq!(store.retire_unfenced_submissions(expired).await.unwrap(), 1);
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("cancelled")
        );
        assert!(!store.begin_submit("exec-1", expired).await.unwrap());
    }

    #[tokio::test]
    async fn a_failed_begin_commit_does_not_arm_the_submission() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        store.fail_next_begin_commit();
        assert!(store.begin_submit("exec-1", 10).await.is_err());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("delivering")
        );
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        assert!(!store.begin_submit("exec-1", 10).await.unwrap());
    }

    #[tokio::test]
    async fn an_interrupted_submission_is_unconfirmed_and_not_due() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        store
            .record_outcome(
                "exec-1",
                ScheduleSettlement::Unconfirmed,
                "submission_interrupted",
                12,
            )
            .await
            .unwrap();
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("unconfirmed")
        );
        assert_eq!(store.next_due(12).await.unwrap(), None);
        assert!(store.open("conversation").await.unwrap().is_none());
        assert!(!store.begin_submit("exec-1", 10).await.unwrap());
        let interrupted = audits(&store)
            .into_iter()
            .filter(|row| row.1 == "submission_interrupted")
            .collect::<Vec<_>>();
        assert_eq!(
            interrupted,
            vec![(
                "system".into(),
                "submission_interrupted".into(),
                "submitting".into(),
                "unconfirmed".into()
            )]
        );
        store
            .record_outcome(
                "exec-1",
                ScheduleSettlement::Unconfirmed,
                "submission_interrupted",
                13,
            )
            .await
            .unwrap();
        assert_eq!(
            audits(&store)
                .into_iter()
                .filter(|row| row.1 == "submission_interrupted")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn a_retryable_submit_is_not_due_again_until_its_retry_time() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        let now = 10;
        let retry_at = now + 1_000;
        assert!(store
            .defer_submit("exec-1", 1, retry_at, now)
            .await
            .unwrap());
        store.release_submit("exec-1").await.unwrap();
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        assert_eq!(store.next_due(now).await.unwrap(), Some(retry_at));
        assert!(store.claim_due(now, 4).await.unwrap().is_empty());
        for _ in 0..20 {
            assert!(!store.begin_submit("exec-1", now).await.unwrap());
        }
        let due = store.claim_due(retry_at, 4).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].retry_failures, 1);
        assert!(store.begin_submit("exec-1", retry_at).await.unwrap());
        let again = retry_at + 2_000;
        assert!(store
            .defer_submit("exec-1", 2, again, retry_at)
            .await
            .unwrap());
        store.release_submit("exec-1").await.unwrap();
        assert_eq!(store.next_due(retry_at).await.unwrap(), Some(again));
        assert!(!store.begin_submit("exec-1", retry_at).await.unwrap());
        let retries = audits(&store)
            .into_iter()
            .filter(|row| row.1 == "submission_retry")
            .collect::<Vec<_>>();
        assert_eq!(
            retries,
            vec![
                (
                    "system".into(),
                    "submission_retry".into(),
                    "submitting".into(),
                    "submitting".into(),
                ),
                (
                    "system".into(),
                    "submission_retry".into(),
                    "submitting".into(),
                    "submitting".into(),
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_submit_retry_audit_leaves_the_row_due() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        store.fail_next_defer_audit();
        assert!(store.defer_submit("exec-1", 1, 1_010, 10).await.is_err());
        assert_eq!(store.next_due(10).await.unwrap(), Some(10));
        assert!(audits(&store)
            .into_iter()
            .all(|row| row.1 != "submission_retry"));
        assert!(store.defer_submit("exec-1", 1, 1_010, 10).await.unwrap());
        assert_eq!(store.next_due(10).await.unwrap(), Some(1_010));
    }

    #[tokio::test]
    async fn a_reconcile_debt_is_kept_until_it_is_cleared() {
        let store = store().await;
        store
            .note_reconcile_retry("conversation", 1, 5_000)
            .await
            .unwrap();
        assert_eq!(store.next_due(1).await.unwrap(), None);
        let debts = store.reconcile_retries().await.unwrap();
        assert_eq!(debts.len(), 1);
        assert_eq!(debts[0].conversation, "conversation");
        assert_eq!(debts[0].failures, 1);
        assert_eq!(debts[0].retry_at_ms, 5_000);
        store
            .note_reconcile_retry("conversation", 2, 9_000)
            .await
            .unwrap();
        assert_eq!(
            store.reconcile_retries().await.unwrap()[0].retry_at_ms,
            9_000
        );
        store.clear_reconcile_retry("conversation").await.unwrap();
        assert!(store.reconcile_retries().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deleting_a_conversation_blanks_the_prompt_once() {
        let store = store().await;
        store.set_preference(true, person(), 1).await.unwrap();
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 2)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        store
            .record_outcome("exec-1", ScheduleSettlement::Delivered, "delivered", 3)
            .await
            .unwrap();
        store
            .accept(draft("conversation", "later", "exec-2", 40), 4)
            .await
            .unwrap();
        store.erase_conversation("conversation", 5).await.unwrap();
        let rows = {
            let connection = store.connection.lock().unwrap();
            let mut statement = connection
                .prepare(
                    "SELECT execution_id, status, prompt, principal_id, organization_id, grant_id \
                     FROM message ORDER BY execution_id",
                )
                .unwrap();
            let rows: Vec<(String, String, String, String, String, Option<String>)> = statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                })
                .unwrap()
                .map(|row| row.unwrap())
                .collect();
            rows
        };
        assert_eq!(
            rows,
            vec![
                (
                    "exec-1".into(),
                    "delivered".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                    None
                ),
                (
                    "exec-2".into(),
                    "cancelled".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                    None
                ),
            ]
        );
        let scrubbed = audits(&store)
            .into_iter()
            .filter(|row| row.1 == "conversation_deleted")
            .count();
        assert_eq!(scrubbed, 2);
        store.erase_conversation("conversation", 6).await.unwrap();
        assert_eq!(
            audits(&store)
                .into_iter()
                .filter(|row| row.1 == "conversation_deleted")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn deleting_an_armed_submission_blanks_the_prompt_and_leaves_it_submitting() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1", 10).await.unwrap());
        store.erase_conversation("conversation", 2).await.unwrap();
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        let connection = store.connection.lock().unwrap();
        let prompt: String = connection
            .query_row(
                "SELECT prompt FROM message WHERE execution_id = 'exec-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);
        assert!(prompt.is_empty());
        let scrubbed = audits(&store)
            .into_iter()
            .filter(|row| row.1 == "conversation_deleted" && row.3 == "submitting")
            .count();
        assert_eq!(scrubbed, 1);
    }

    #[tokio::test]
    async fn an_outcome_records_the_instant_it_is_given() {
        let store = enabled_store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        store
            .record_outcome("exec-1", ScheduleSettlement::Delivered, "delivered", 4242)
            .await
            .unwrap();
        assert_eq!(store.audit_instant("exec-1", "delivered"), Some(4242));
    }

    fn decision_audits(store: &LocalScheduledMessages) -> Vec<(String, String, String, String)> {
        let connection = store.connection.lock().unwrap();
        let mut statement = connection
            .prepare(
                "SELECT cause, ifnull(before_status, ''), after_status, policies \
                 FROM audit WHERE cause IN ('preference_allowed', 'preference_denied') ORDER BY id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect()
    }

    #[tokio::test]
    async fn a_second_process_cannot_submit_a_live_lease() {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("scheduled");
        nessa_local_storage::create_directory(&private).unwrap();
        let path = private.join("scheduled-chat-messages.sqlite3");
        let first = LocalScheduledMessages::open(&path).unwrap();
        let second = LocalScheduledMessages::open(&path).unwrap();
        let now = 1_000;
        first.set_preference(true, person(), 1).await.unwrap();
        first
            .accept(draft("conversation", "causal", "exec-1", now), 2)
            .await
            .unwrap();
        first.claim_due(now, 4).await.unwrap();
        assert!(first.begin_submit("exec-1", now).await.unwrap());
        assert!(!second.begin_submit("exec-1", now).await.unwrap());
        let renewed_at = now + super::FENCE_LEASE_MS;
        assert!(first.renew_fence("exec-1", renewed_at).await.unwrap());
        assert!(!second.begin_submit("exec-1", renewed_at).await.unwrap());
        let expired = renewed_at + super::FENCE_LEASE_MS;
        assert!(second.begin_submit("exec-1", expired).await.unwrap());
        assert!(!first.renew_fence("exec-1", expired).await.unwrap());
        assert!(!first
            .set_preference(false, person(), expired)
            .await
            .unwrap());
        assert_eq!(
            second.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        assert_eq!(
            second.retire_unfenced_submissions(expired).await.unwrap(),
            0
        );
        assert_eq!(
            second.next_live_fence(expired).await.unwrap(),
            Some(expired + super::FENCE_LEASE_MS)
        );
        let later = expired + super::FENCE_LEASE_MS;
        assert_eq!(second.retire_unfenced_submissions(later).await.unwrap(), 1);
        assert_eq!(
            second.status("exec-1").await.unwrap().as_deref(),
            Some("cancelled")
        );
        drop(directory);
    }

    #[tokio::test]
    async fn a_preference_decision_records_the_policies_and_a_failed_audit_changes_nothing() {
        let store = store().await;
        let policies = vec!["usage-limit-preference.member".to_owned()];
        let denied = store
            .record_preference_decision(false, true, &policies, person(), 5)
            .await
            .unwrap();
        assert!(matches!(denied.update, PreferenceUpdate::Refused { .. }));
        assert!(!denied.installed_enable);
        assert!(!store.preference().await.unwrap().enabled);
        assert_eq!(
            decision_audits(&store),
            vec![(
                "preference_denied".into(),
                "off".into(),
                "off".into(),
                "usage-limit-preference.member".into()
            )]
        );

        store.fail_next_decision_audit();
        let failed = store
            .record_preference_decision(
                true,
                true,
                &["usage-limit-preference.admin-grant".into()],
                person(),
                6,
            )
            .await;
        assert!(failed.is_err());
        assert!(!store.preference().await.unwrap().enabled);
        assert_eq!(decision_audits(&store).len(), 1);

        store.fail_next_decision_audit();
        let failed_deny = store
            .record_preference_decision(false, true, &policies, person(), 6)
            .await;
        assert!(failed_deny.is_err());
        assert_eq!(decision_audits(&store).len(), 1);

        let allowed = store
            .record_preference_decision(
                true,
                true,
                &[
                    "usage-limit-preference.admin-grant".into(),
                    "usage-limit-preference.member".into(),
                ],
                person(),
                7,
            )
            .await
            .unwrap();
        assert_eq!(allowed.update, PreferenceUpdate::Stored(true));
        assert!(allowed.installed_enable);
        assert_eq!(allowed.revision, 1);
        assert!(store.preference().await.unwrap().enabled);
        assert_eq!(
            decision_audits(&store).last().unwrap().3,
            "usage-limit-preference.admin-grant\nusage-limit-preference.member"
        );

        let unchanged_allow = store
            .record_preference_decision(
                true,
                true,
                &["usage-limit-preference.admin-grant".into()],
                person(),
                8,
            )
            .await
            .unwrap();
        assert!(!unchanged_allow.installed_enable);
        assert_eq!(unchanged_allow.revision, allowed.revision);
        assert!(store.preference().await.unwrap().enabled);
        let decisions = decision_audits(&store);
        assert_eq!(decisions.len(), 3);
        let unchanged = decisions.last().unwrap();
        assert_eq!(unchanged.1, unchanged.2);
        assert_eq!(unchanged.1, "on");
    }

    #[tokio::test]
    async fn an_installed_enable_reverts_only_while_it_is_unsettled() {
        let store = store().await;
        let installed = store
            .record_preference_decision(
                true,
                true,
                &["usage-limit-preference.admin-grant".into()],
                person(),
                5,
            )
            .await
            .unwrap();
        assert!(installed.installed_enable);
        store
            .accept(draft("one", "causal", "exec-1", 50), 6)
            .await
            .unwrap();
        let second = store
            .record_preference_decision(
                true,
                true,
                &["usage-limit-preference.admin-grant".into()],
                person(),
                6,
            )
            .await
            .unwrap();
        assert!(!second.installed_enable);
        assert_eq!(second.revision, installed.revision);
        assert!(store
            .settle_installed_preference(installed.revision)
            .await
            .unwrap());
        assert!(!store
            .revert_installed_preference(installed.revision, ScheduleActor::system(), 7)
            .await
            .unwrap());
        assert!(store.preference().await.unwrap().enabled);
        assert!(store.open("one").await.unwrap().is_some());

        assert!(!store
            .set_preference(false, ScheduleActor::system(), 8)
            .await
            .unwrap());
        let again = store
            .record_preference_decision(
                true,
                true,
                &["usage-limit-preference.admin-grant".into()],
                person(),
                9,
            )
            .await
            .unwrap();
        assert!(again.installed_enable);
        assert_ne!(again.revision, installed.revision);
        let mut later = draft("two", "causal-2", "exec-2", 50);
        later.preference_revision = again.revision;
        store.accept(later, 9).await.unwrap();
        assert!(store.open("two").await.unwrap().is_some());
        let before = store.preference_audit_initiators().len();
        assert!(store
            .revert_installed_preference(again.revision, ScheduleActor::system(), 10)
            .await
            .unwrap());
        assert!(!store.preference().await.unwrap().enabled);
        assert!(store.open("two").await.unwrap().is_none());
        let initiators = store.preference_audit_initiators();
        assert_eq!(initiators.len(), before + 1);
        assert_eq!(initiators.last().map(String::as_str), Some("system"));
    }

    #[tokio::test]
    async fn a_stale_accept_does_not_insert_after_the_preference_revision_changes() {
        let store = store().await;
        store.set_preference(true, person(), 1).await.unwrap();
        let reading = store.preference().await.unwrap();
        assert!(reading.enabled);
        assert_eq!(reading.revision, 1);
        let accepted = store
            .accept(draft("kept", "causal", "exec-1", 50), 2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(accepted.execution_id, "exec-1");

        assert!(!store.set_preference(false, person(), 3).await.unwrap());
        assert!(store.open("kept").await.unwrap().is_none());

        let stale = store
            .accept(draft("missed", "causal", "exec-2", 50), 4)
            .await
            .unwrap();
        assert!(stale.is_none());
        assert!(store.open("missed").await.unwrap().is_none());
        assert!(!store.preference().await.unwrap().enabled);

        store.set_preference(true, person(), 5).await.unwrap();
        let again = store.preference().await.unwrap();
        assert_ne!(again.revision, reading.revision);
        let mut still_stale = draft("later", "causal", "exec-3", 10);
        still_stale.preference_revision = reading.revision;
        assert!(store.accept(still_stale, 6).await.unwrap().is_none());
        assert!(store.open("later").await.unwrap().is_none());

        let mut current = draft("later", "causal", "exec-4", 80);
        current.preference_revision = again.revision;
        let fresh = store.accept(current, 7).await.unwrap().unwrap();
        assert_eq!(fresh.execution_id, "exec-4");
    }
}
