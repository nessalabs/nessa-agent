//! One exclusive writer's committed cursor and exact pending bytes.
//! The owner of this value supplies the conversation lease and runtime lifetime.

use super::{snapshot, stream_fact};
use crate::{
    application::agent_execution::sessions::{
        records::{self, FactKind},
        SessionChange, SessionSaveGeneration, SessionSnapshot, StorageError,
    },
    domain::agent_execution::sessions::SessionId,
};
use event_stream::{Cursor, EventRuntime, StreamKey};

struct Pending {
    fact: stream_fact::FramedFact,
    changes: Vec<Vec<u8>>,
    candidate: SessionSnapshot,
}

pub(super) struct RecordWriter {
    id: SessionId,
    stream: StreamKey,
    cursor: Cursor,
    committed: Option<SessionSnapshot>,
    pending: Option<Pending>,
    batch_generation: Option<SessionSaveGeneration>,
    batch_complete: bool,
    inflight_base: Option<Option<SessionSnapshot>>,
    committed_prefix: Vec<Vec<u8>>,
    blocked: bool,
}

impl RecordWriter {
    pub(super) async fn replay<R: EventRuntime>(
        reader: &R,
        id: SessionId,
        stream: StreamKey,
    ) -> Result<Self, StorageError> {
        let bounds = reader.bounds(&stream).await.map_err(store_error)?;
        if bounds.floor.offset != 0 || bounds.floor.stream != stream {
            return Err(corrupt("conversation record prefix is unavailable"));
        }
        let mut writer = Self {
            id,
            stream: stream.clone(),
            cursor: Cursor::new(stream, 0),
            committed: None,
            pending: None,
            batch_generation: None,
            batch_complete: false,
            inflight_base: None,
            committed_prefix: Vec::new(),
            blocked: false,
        };
        loop {
            match stream_fact::read_next_fact(reader, &writer.stream, &writer.cursor)
                .await
                .map_err(fact_error)?
            {
                stream_fact::FactRead::Absent => break,
                stream_fact::FactRead::Partial => {
                    writer.cursor =
                        stream_fact::abort_partial_fact(reader, &writer.stream, &writer.cursor)
                            .await
                            .map_err(fact_error)?;
                }
                stream_fact::FactRead::Aborted { cursor } => {
                    writer.cursor = cursor;
                }
                stream_fact::FactRead::Complete { fact, cursor } => {
                    let changes = snapshot::decode_semantic_batch(
                        &fact.body,
                        fact.key.kind() == FactKind::AtomicTransition,
                        &writer.committed.as_ref().map_or(
                            crate::domain::agent_execution::sessions::ProviderContext::Absent,
                            |state| state.provider_context.clone(),
                        ),
                    )?;
                    let expected = records::key_for_changes(
                        writer.committed.as_ref(),
                        &changes,
                        writer.cursor.offset,
                    )?;
                    if fact.key != expected {
                        return Err(corrupt(
                            "semantic fact identity disagrees with its position",
                        ));
                    }
                    let candidate = records::fold_changes(writer.committed.as_ref(), &changes)?;
                    if candidate.id != writer.id {
                        return Err(StorageError::IdentityMismatch);
                    }
                    writer.committed = Some(candidate);
                    writer.cursor = cursor;
                }
            }
        }
        Ok(writer)
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> Option<&SessionSnapshot> {
        self.committed.as_ref()
    }

    pub(super) fn readable_snapshot(&self) -> Result<Option<SessionSnapshot>, StorageError> {
        if self.blocked {
            return Err(corrupt("conversation fact conflicts with physical history"));
        }
        if self.pending.is_some() || (self.batch_generation.is_some() && !self.batch_complete) {
            return Err(StorageError::Unresolved);
        }
        Ok(self.committed.clone())
    }

    pub(super) fn id(&self) -> &SessionId {
        &self.id
    }

    pub(super) fn stream(&self) -> &StreamKey {
        &self.stream
    }

    pub(super) fn has_unresolved_fact(&self) -> bool {
        self.pending.is_some()
            || (self.batch_generation.is_some() && !self.batch_complete)
            || self.blocked
    }

    pub(super) async fn save<R: EventRuntime>(
        &mut self,
        runtime: &R,
        generation: SessionSaveGeneration,
        observed: &SessionSnapshot,
        changes: &[SessionChange],
    ) -> Result<(), StorageError> {
        if self.blocked {
            return Err(corrupt("conversation fact conflicts with physical history"));
        }
        let next_batch = match self.batch_generation {
            None if generation == SessionSaveGeneration::initial() => true,
            None => return Err(corrupt("first session save generation is not initial")),
            Some(current) if generation == current => false,
            Some(current)
                if self.batch_complete
                    && self.pending.is_none()
                    && generation == current.checked_next()? =>
            {
                true
            }
            Some(_) => return Err(corrupt("session save generation is stale or skipped")),
        };
        let base = if next_batch {
            &self.committed
        } else {
            self.inflight_base.as_ref().unwrap_or(&self.committed)
        };
        records::confirm_candidate(&self.id, base.as_ref(), changes, observed)?;
        let encoded = changes
            .iter()
            .map(snapshot::encode_semantic_change)
            .collect::<Result<Vec<_>, _>>()?;
        if !next_batch && !encoded.starts_with(&self.committed_prefix) {
            return Err(corrupt(
                "committed semantic decision prefix changed before reconciliation",
            ));
        }
        // A definite local size refusal must leave a fresh batch untouched. A
        // later save can then choose a smaller, still atomic decision group.
        let preflight_body = if self.pending.is_none() {
            let start = if next_batch {
                0
            } else {
                self.committed_prefix.len()
            };
            let remaining = &changes[start..];
            if remaining.is_empty() {
                None
            } else {
                let body = snapshot::encode_semantic_batch(remaining)?;
                if body.len() > stream_fact::MAX_BODY_BYTES {
                    return Err(StorageError::TooLarge);
                }
                Some(body)
            }
        } else {
            None
        };
        if next_batch {
            self.batch_generation = Some(generation);
            self.batch_complete = false;
            self.inflight_base = Some(self.committed.clone());
            self.committed_prefix.clear();
        }
        let mut remaining = &changes[self.committed_prefix.len()..];
        if let Some(pending) = self.pending.as_ref() {
            if remaining.len() < pending.changes.len()
                || encoded[self.committed_prefix.len()..].get(..pending.changes.len())
                    != Some(pending.changes.as_slice())
            {
                return Err(corrupt(
                    "pending semantic decision changed before reconciliation",
                ));
            }
            let cursor =
                match stream_fact::commit_fact(runtime, &self.stream, &self.cursor, &pending.fact)
                    .await
                {
                    Ok(cursor) => cursor,
                    Err(error) => {
                        self.blocked = is_invalid_fact(&error);
                        return Err(fact_error(error));
                    }
                };
            let pending = self
                .pending
                .take()
                .expect("pending was retained across commit");
            self.cursor = cursor;
            self.committed = Some(pending.candidate);
            let pending_len = pending.changes.len();
            self.committed_prefix.extend(pending.changes);
            remaining = &remaining[pending_len..];
        }
        if remaining.is_empty() {
            self.batch_complete = true;
            return Ok(());
        }
        let candidate = records::fold_changes(self.committed.as_ref(), remaining)?;
        if candidate != *observed {
            return Err(corrupt("semantic decisions disagree with observed session"));
        }
        let key = records::key_for_changes(self.committed.as_ref(), remaining, self.cursor.offset)?;
        let body = match preflight_body {
            Some(body) => body,
            None => snapshot::encode_semantic_batch(remaining)?,
        };
        if body.len() > stream_fact::MAX_BODY_BYTES {
            return Err(StorageError::TooLarge);
        }
        let fact = stream_fact::FramedFact { key, body };
        let encoded = encoded[self.committed_prefix.len()..].to_vec();
        self.batch_complete = false;
        self.pending = Some(Pending {
            fact,
            changes: encoded,
            candidate,
        });
        let pending = self.pending.as_ref().expect("pending was just installed");
        let cursor = match stream_fact::commit_fact(
            runtime,
            &self.stream,
            &self.cursor,
            &pending.fact,
        )
        .await
        {
            Ok(cursor) => cursor,
            Err(stream_fact::FactCommitError::Conflict) => {
                self.pending = None;
                self.blocked = true;
                return Err(corrupt(
                    "semantic fact identity has different committed bytes",
                ));
            }
            Err(error) => {
                self.blocked = is_invalid_fact(&error);
                return Err(fact_error(error));
            }
        };
        let pending = self.pending.take().expect("pending was just installed");
        self.cursor = cursor;
        self.committed = Some(pending.candidate);
        self.committed_prefix.extend(pending.changes);
        self.batch_complete = true;
        Ok(())
    }
}

fn corrupt(message: &'static str) -> StorageError {
    StorageError::Corrupt(message.into())
}

fn store_error(error: event_stream::Error) -> StorageError {
    match error {
        event_stream::Error::StoreCorrupt(detail) => StorageError::Corrupt(detail),
        event_stream::Error::StreamUnavailable { .. }
        | event_stream::Error::StaleIncarnation { .. }
        | event_stream::Error::StreamNotFound => StorageError::Corrupt(error.to_string()),
        other => StorageError::Io(other.to_string()),
    }
    .bounded()
}

fn fact_error(error: stream_fact::FactCommitError) -> StorageError {
    match error {
        stream_fact::FactCommitError::Stream(error) => store_error(error),
        stream_fact::FactCommitError::Frame(_)
        | stream_fact::FactCommitError::Conflict
        | stream_fact::FactCommitError::InvalidStream => corrupt("conversation fact is invalid"),
    }
}

fn is_invalid_fact(error: &stream_fact::FactCommitError) -> bool {
    matches!(
        error,
        stream_fact::FactCommitError::Conflict
            | stream_fact::FactCommitError::Frame(_)
            | stream_fact::FactCommitError::InvalidStream
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::agent_execution::{
            executions::{ExecutionRequest, SubmissionMode},
            permissions::ActionContext,
            providers::ProviderIdentity,
            sessions::{InvocationRecord, SubmissionAcknowledgement},
        },
        domain::agent_execution::{
            executions::ExecutionId,
            prompts::{PromptText, UserMessage},
            sessions::{ExecutionSessionId, ProviderContext},
        },
    };
    use event_stream::{
        infrastructure::{SqliteOptions, SqliteStore},
        EventConfig, EventReader, EventSink, PersistenceProfile, Runtime, RuntimeConfig, StreamId,
    };
    use std::time::Duration;

    #[tokio::test]
    async fn sqlite_restart_replays_committed_decisions_without_provider_effects() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("records.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let id = SessionId::new("conversation").unwrap();
        let stream_id = StreamId::new("conversation").unwrap();
        let runtime = Runtime::<SqliteStore>::open(options.clone(), config.clone())
            .await
            .unwrap();
        let stream = runtime.create_stream(&stream_id).await.unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        assert!(writer.snapshot().is_none());
        let opened = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let first = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
        writer
            .save(
                &runtime,
                SessionSaveGeneration::initial(),
                &first,
                &[opened],
            )
            .await
            .unwrap();
        let context = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let second = records::fold_changes(Some(&first), std::slice::from_ref(&context)).unwrap();
        writer
            .save(
                &runtime,
                SessionSaveGeneration::initial().checked_next().unwrap(),
                &second,
                &[context],
            )
            .await
            .unwrap();
        assert_eq!(writer.snapshot(), Some(&second));
        drop(writer);
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );

        let reopened = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let same_stream = reopened.create_stream(&stream_id).await.unwrap();
        assert_eq!(same_stream, stream);
        let restored = RecordWriter::replay(&reopened, id, same_stream)
            .await
            .unwrap();
        assert_eq!(restored.snapshot(), Some(&second));
        assert!(
            reopened
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn physical_conflict_fences_live_load_and_later_saves() {
        let directory = tempfile::tempdir().unwrap();
        let options = SqliteOptions::new(directory.path().join("conflict.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let runtime = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let id = SessionId::new("conversation").unwrap();
        let stream = runtime
            .create_stream(&StreamId::new(id.as_str()).unwrap())
            .await
            .unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        let opened = SessionChange::Opened {
            id,
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let initial = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
        writer
            .save(
                &runtime,
                SessionSaveGeneration::initial(),
                &initial,
                &[opened],
            )
            .await
            .unwrap();
        let foreign = stream_fact::FramedFact {
            key: records::FactKey::new(records::FactKind::ProviderContext, None, 1).unwrap(),
            body: b"foreign".to_vec(),
        };
        let frame = stream_fact::frame_fact(&foreign, 2).unwrap().remove(0);
        runtime.append(&stream, frame).await.unwrap();
        let change = SessionChange::ProviderContext {
            before: ProviderContext::Absent,
            after: ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap()),
        };
        let candidate =
            records::fold_changes(Some(&initial), std::slice::from_ref(&change)).unwrap();
        assert!(matches!(
            writer
                .save(
                    &runtime,
                    SessionSaveGeneration::initial().checked_next().unwrap(),
                    &candidate,
                    std::slice::from_ref(&change)
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            writer.readable_snapshot(),
            Err(StorageError::Corrupt(_))
        ));
        assert!(matches!(
            writer
                .save(
                    &runtime,
                    SessionSaveGeneration::initial().checked_next().unwrap(),
                    &candidate,
                    &[change]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }

    #[tokio::test]
    async fn an_unsealed_input_is_aborted_after_restart_and_a_new_attempt_can_commit() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("sessions");
        nessa_local_storage::create_directory(&root).unwrap();
        let options = SqliteOptions::new(root.join("records.sqlite3"));
        let config = RuntimeConfig {
            events: EventConfig {
                max_bytes: 1024 * 1024,
                minimum_persistence: PersistenceProfile::ProcessRestart,
            },
            ..RuntimeConfig::default()
        };
        let id = SessionId::new("conversation").unwrap();
        let stream_id = StreamId::new("conversation").unwrap();
        let runtime = Runtime::<SqliteStore>::open(options.clone(), config.clone())
            .await
            .unwrap();
        let stream = runtime.create_stream(&stream_id).await.unwrap();
        let opened = SessionChange::Opened {
            id: id.clone(),
            provider: ProviderIdentity::new("provider", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        };
        let first = records::fold_changes(None, std::slice::from_ref(&opened)).unwrap();
        let mut writer = RecordWriter::replay(&runtime, id.clone(), stream.clone())
            .await
            .unwrap();
        writer
            .save(
                &runtime,
                SessionSaveGeneration::initial(),
                &first,
                &[opened],
            )
            .await
            .unwrap();
        let execution_id = ExecutionId::new("execution").unwrap();
        let make_input = |text: &str| {
            SessionChange::InputAccepted(Box::new(InvocationRecord {
                target_event_offset: None,
                submission: SubmissionMode::Immediate,
                request: ExecutionRequest {
                    execution_id: execution_id.clone(),
                    user_message: UserMessage::text_only(PromptText::new(text).unwrap()),
                    estimated_input_tokens: 1,
                    reserved_output_tokens: 1,
                },
                actor: ActionContext::new("user", "phone", "send").unwrap(),
                acknowledgement: SubmissionAcknowledgement::Pending,
                events: Vec::new(),
                scheduling: Vec::new(),
                cancellation: None,
                provider_report: None,
                local_cancellation: None,
                local_outcome: None,
                result: None,
            }))
        };
        let input = make_input(&"x".repeat(200_000));
        let fact = stream_fact::FramedFact {
            key: records::key_for_changes(
                Some(&first),
                std::slice::from_ref(&input),
                writer.cursor.offset,
            )
            .unwrap(),
            body: snapshot::encode_semantic_batch(std::slice::from_ref(&input)).unwrap(),
        };
        let frames = stream_fact::frame_fact(&fact, writer.cursor.offset + 1).unwrap();
        assert!(frames.len() > 2);
        runtime.append(&stream, frames[0].clone()).await.unwrap();
        runtime.append(&stream, frames[1].clone()).await.unwrap();
        drop(writer);
        assert!(
            runtime
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );

        let storage = super::super::record::RecordStorage::new(&root).unwrap();
        let lease = crate::application::agent_execution::sessions::SessionStorage::open_existing(
            &storage,
            id.clone(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(lease.load().await.unwrap(), Some(first.clone()));
        drop(lease);
        crate::application::agent_execution::sessions::SessionStorage::shutdown(&storage)
            .await
            .unwrap();
        drop(storage);

        let reopened = Runtime::<SqliteStore>::open(options, config).await.unwrap();
        let same_stream = reopened.create_stream(&stream_id).await.unwrap();
        let mut recovered = RecordWriter::replay(&reopened, id, same_stream.clone())
            .await
            .unwrap();
        assert_eq!(recovered.snapshot(), Some(&first));
        let changed = make_input(&"y".repeat(200_000));
        let changed_state =
            records::fold_changes(Some(&first), std::slice::from_ref(&changed)).unwrap();
        recovered
            .save(
                &reopened,
                SessionSaveGeneration::initial(),
                &changed_state,
                &[changed],
            )
            .await
            .unwrap();
        assert_eq!(recovered.snapshot(), Some(&changed_state));
        let original_state =
            records::fold_changes(Some(&first), std::slice::from_ref(&input)).unwrap();
        assert!(matches!(
            recovered
                .save(
                    &reopened,
                    SessionSaveGeneration::initial(),
                    &original_state,
                    &[input]
                )
                .await,
            Err(StorageError::Corrupt(_))
        ));
        assert!(reopened.bounds(&same_stream).await.unwrap().tail.offset > frames.len() as u64);
        assert!(
            reopened
                .shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .closed
        );
    }
}
