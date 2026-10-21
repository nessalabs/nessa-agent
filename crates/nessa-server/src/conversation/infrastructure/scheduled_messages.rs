//! The private SQLite dataset of usage-limit continuations
//! (`scheduled-chat-messages.sqlite3`, `scheduled_messages.sql`).
//!
//! Schedule and cancellation audits commit with the status change. The
//! delivery audit commits in the transaction that records the outcome, which
//! runs only after `begin_submit` has moved the row to `submitting`. Cancel,
//! defer, and a model change match `scheduled` and `delivering` only, so they
//! cannot land once that transition has, and the transition fails once they
//! have. If the outcome transaction fails, the row stays `submitting` and
//! keeps its execution id.

use crate::conversation::application::{
    ConversationError, ConversationFuture, ScheduleActor, ScheduleDraft, ScheduleSettlement,
    ScheduledMessage, ScheduledMessageStore,
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
    /// Execution ids whose `submitting` row this process is inside.
    ///
    /// The connection mutex is taken before this one. A restart drops the set,
    /// so a `submitting` row left by a crash can be adopted. Preference off
    /// cancels a `submitting` row only when it is not in this set.
    armed: Arc<Mutex<HashSet<String>>>,
    /// The next delivery-outcome transaction fails before it commits.
    #[cfg(test)]
    fail_outcome_audit: AtomicBool,
    /// The next `begin_submit` fails at commit, before the id is armed.
    #[cfg(test)]
    fail_begin_commit: AtomicBool,
}

impl LocalScheduledMessages {
    /// Open the database at `path`, creating it when absent.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let connection = nessa_local_database::open(path, &Schema::new(DEFINITION)?)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            armed: Arc::new(Mutex::new(HashSet::new())),
            #[cfg(test)]
            fail_outcome_audit: AtomicBool::new(false),
            #[cfg(test)]
            fail_begin_commit: AtomicBool::new(false),
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
    transaction
        .execute(
            "INSERT INTO audit (at_ms, conversation, causal_execution, execution_id, \
             before_status, after_status, cause, initiator_kind, organization_id, \
             principal_id, surface_id, action_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
    })
}

const MESSAGE_COLUMNS: &str = "conversation, causal_execution, execution_id, due_at_ms, status, \
     agent, model, prompt, organization_id, principal_id, surface_id, action_id, \
     initiation, grant_id, policy_revision, tools_checked";

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

/// Cancel `submitting` rows this process is not inside. A held fence stays.
fn cancel_unarmed_submitting(
    transaction: &Transaction<'_>,
    armed: &HashSet<String>,
    actor: &ScheduleActor,
    at_ms: i64,
    cause: &str,
) -> Result<usize, ConversationError> {
    let rows = {
        let mut statement = transaction
            .prepare(
                "SELECT conversation, causal_execution, execution_id FROM message \
                 WHERE status = 'submitting'",
            )
            .map_err(failed)?;
        let selected = statement
            .query_map([], |row| {
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
        if armed.contains(&execution) {
            continue;
        }
        let changed = transaction
            .execute(
                "UPDATE message SET status = 'cancelled' \
                 WHERE execution_id = ?1 AND status = 'submitting'",
                params![execution],
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

impl ScheduledMessageStore for LocalScheduledMessages {
    fn preference(&self) -> ConversationFuture<'_, bool> {
        self.run(|connection| {
            let enabled: Option<i64> = connection
                .query_row("SELECT enabled FROM preference WHERE id = 1", [], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(failed)?;
            Ok(enabled == Some(1))
        })
    }

    fn set_preference(
        &self,
        enabled: bool,
        actor: ScheduleActor,
        at_ms: u64,
    ) -> ConversationFuture<'_, bool> {
        let armed = self.armed.clone();
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let previous: Option<i64> = transaction
                .query_row("SELECT enabled FROM preference WHERE id = 1", [], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(failed)?;
            let was = previous == Some(1);
            if was == enabled {
                transaction.commit().map_err(failed)?;
                return Ok(enabled);
            }
            transaction
                .execute(
                    "INSERT INTO preference (id, enabled) VALUES (1, ?1) \
                     ON CONFLICT (id) DO UPDATE SET enabled = excluded.enabled",
                    params![i64::from(enabled)],
                )
                .map_err(failed)?;
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
                cancel_matching(
                    &transaction,
                    "WHERE status IN ('scheduled', 'delivering')",
                    &[],
                    ContinuationCause::PreferenceDisabled.as_str(),
                    &actor,
                    at_ms,
                )?;
                cancel_unarmed_submitting(
                    &transaction,
                    &holds(&armed),
                    &actor,
                    at_ms,
                    ContinuationCause::PreferenceDisabled.as_str(),
                )?;
            }
            transaction.commit().map_err(failed)?;
            Ok(enabled)
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
                     WHERE status IN ('delivering', 'submitting') \
                     ORDER BY due_at_ms LIMIT ?1"
                ))
                .map_err(failed)?;
            let already = statement
                .query_map(params![limit as i64], read_message)
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
            // row delivering. Either one is already due: wait until the clock
            // so the loop passes again. Only a later scheduled row waits.
            let pending: bool = connection
                .query_row(
                    "SELECT EXISTS( \
                         SELECT 1 FROM message \
                         WHERE status IN ('delivering', 'submitting') \
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
                    "SELECT MIN(due_at_ms) FROM message \
                     WHERE status = 'scheduled' AND due_at_ms > ?1",
                    params![now],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            Ok(due.and_then(|due| u64::try_from(due).ok()))
        })
    }

    fn begin_submit(&self, execution_id: &str) -> ConversationFuture<'_, bool> {
        let execution_id = execution_id.to_owned();
        let armed = self.armed.clone();
        #[cfg(test)]
        let fail_commit = self.fail_begin_commit.swap(false, Ordering::SeqCst);
        self.run(move |connection| {
            let transaction = connection.transaction().map_err(failed)?;
            let status: Option<String> = transaction
                .query_row(
                    "SELECT status FROM message WHERE execution_id = ?1",
                    params![execution_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)?;
            // The connection mutex is held for this whole closure. The arm is
            // taken only after the status commit, so a failed commit cannot
            // leave the id armed.
            let claim = match status.as_deref() {
                Some("delivering") => {
                    transaction
                        .execute(
                            "UPDATE message SET status = 'submitting' \
                             WHERE execution_id = ?1 AND status = 'delivering'",
                            params![execution_id],
                        )
                        .map_err(failed)?
                        == 1
                }
                Some("submitting") => !holds(&armed).contains(&execution_id),
                _ => false,
            };
            if !claim {
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

    fn release_submit(&self, execution_id: &str) -> ConversationFuture<'_, ()> {
        let execution_id = execution_id.to_owned();
        let armed = self.armed.clone();
        self.run(move |_connection| {
            holds(&armed).remove(&execution_id);
            Ok(())
        })
    }

    fn retire_unfenced_submissions(&self, at_ms: u64) -> ConversationFuture<'_, usize> {
        let armed = self.armed.clone();
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
            let cancelled = cancel_unarmed_submitting(
                &transaction,
                &holds(&armed),
                &ScheduleActor::system(),
                at_ms,
                ContinuationCause::PreferenceDisabled.as_str(),
            )?;
            transaction.commit().map_err(failed)?;
            Ok(cancelled)
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
        let armed = self.armed.clone();
        self.run(move |connection| {
            let at_ms = millis(at_ms)?;
            let transaction = connection.transaction().map_err(failed)?;
            let rows = {
                let mut statement = transaction
                    .prepare(
                        "SELECT execution_id, causal_execution, status, prompt, agent, model, \
                         organization_id, principal_id, surface_id, action_id, grant_id \
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
                        })
                    })
                    .map_err(failed)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(failed)?;
                selected
            };
            let armed = holds(&armed);
            for row in rows {
                if row.blank() {
                    continue;
                }
                let cancel = matches!(row.status.as_str(), "scheduled" | "delivering")
                    || (row.status == "submitting" && !armed.contains(&row.execution_id));
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
        }
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
        let store = store().await;
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
        let store = store().await;
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
        assert!(!store.preference().await.unwrap());
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
        let store = store().await;
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
        let store = store().await;
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
        let store = store().await;
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
        assert!(!store.begin_submit("exec-1").await.unwrap());
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
        assert!(held.begin_submit("exec-1").await.unwrap());
        assert!(!held.begin_submit("exec-1").await.unwrap());
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
        let store = store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store
            .defer_claim("conversation", "causal", 80, 2)
            .await
            .unwrap());
        assert!(!store.begin_submit("exec-1").await.unwrap());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("scheduled")
        );

        let changed = open_messages().await;
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
        assert!(!changed.begin_submit("exec-1").await.unwrap());
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
        assert!(store.begin_submit("exec-1").await.unwrap());
        store.release_submit("exec-1").await.unwrap();
        let again = store.claim_due(11, 4).await.unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].execution_id, "exec-1");
        assert!(store.begin_submit("exec-1").await.unwrap());
        // Off while the fence is held leaves the row. Releasing it, with the
        // preference still off, lets the next pass cancel rather than submit.
        assert!(!store.set_preference(false, person(), 3).await.unwrap());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        store.release_submit("exec-1").await.unwrap();
        assert_eq!(store.retire_unfenced_submissions(4).await.unwrap(), 1);
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("cancelled")
        );
        assert!(!store.begin_submit("exec-1").await.unwrap());
    }

    #[tokio::test]
    async fn a_failed_begin_commit_does_not_arm_the_submission() {
        let store = store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        store.fail_next_begin_commit();
        assert!(store.begin_submit("exec-1").await.is_err());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("delivering")
        );
        assert!(store.begin_submit("exec-1").await.unwrap());
        assert_eq!(
            store.status("exec-1").await.unwrap().as_deref(),
            Some("submitting")
        );
        assert!(!store.begin_submit("exec-1").await.unwrap());
    }

    #[tokio::test]
    async fn an_interrupted_submission_is_unconfirmed_and_not_due() {
        let store = store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1").await.unwrap());
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
        assert!(!store.begin_submit("exec-1").await.unwrap());
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
    async fn deleting_a_conversation_blanks_the_prompt_once() {
        let store = store().await;
        store.set_preference(true, person(), 1).await.unwrap();
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 2)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1").await.unwrap());
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
        let store = store().await;
        store
            .accept(draft("conversation", "causal", "exec-1", 10), 1)
            .await
            .unwrap();
        store.claim_due(10, 4).await.unwrap();
        assert!(store.begin_submit("exec-1").await.unwrap());
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
        let store = store().await;
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
}
