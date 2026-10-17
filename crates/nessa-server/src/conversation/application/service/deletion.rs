//! Conversation deletion: fence, finish, provider erasure and recoverable retries.
use super::{
    initiator_of, supervised, AgentSlot, ConversationCaller, ConversationService, Inner, StopBy,
};
use crate::conversation::application::{
    environment::confirmed_despite,
    mcp_apps::McpAppInitiator,
    metadata_target::load_conversation,
    provider_sessions::ProviderSessionHandler,
    retries::{Claim, DeletionRetries, Waiting},
    session_key::conversation_session,
    AttachmentRelease, AttachmentReleaseCause, ConversationDeletionAuditRecord,
    ConversationDeletionCause, ConversationError, ConversationOwnershipState, DeletionFailures,
    StopFailure, UnfinishedDeletions,
};
use crate::conversation::domain::{
    Conversation, ConversationDeletion, ProviderSessionErasure, ProviderSessionLink,
};
use futures_util::FutureExt;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    permissions::ActionContext,
    sessions::{SessionSnapshot, SessionStorageLease, StorageError},
};
use nessa_sdk::domain::agent_execution::{leases::LeaseEndCause, sessions::ExecutionSessionId};
use std::{
    future::Future,
    mem,
    panic::AssertUnwindSafe,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{watch, OwnedMutexGuard};

/// How many times, when the gateway starts, it tries to finish a deletion
/// that did not finish, and how long it waits before each try after the
/// first, times the number of tries so far. Bounded, so a deletion that
/// cannot finish — its agent will not answer, its history is damaged — is
/// reported and left for the next start rather than retried forever.
const DELETION_ATTEMPTS: u32 = 3;
const DELETION_RETRY_DELAY: Duration = Duration::from_secs(1);

impl ConversationService {
    /// Delete the conversation for good. Returns whether this request is the
    /// one that deleted it: `true` for the deciding request and a repeat of
    /// it, `false` for any other — the domain's rule,
    /// [`ConversationDeletion::is_same_decision`].
    ///
    /// Authorize from the ownership record, as `close` does: a deleted
    /// conversation is still its owner's to finish deleting. Then, one delete
    /// of a conversation at a time, fence it: under the creation lock the
    /// repository writes a tombstone, and from then on each command on the
    /// identity, a creation of it included, is refused with
    /// [`ConversationError::Deleted`], and it is left out of both lists
    /// (`a_deleted_conversation_refuses_every_command_on_it`,
    /// `a_create_racing_a_delete_cannot_republish_it`). The rest is
    /// `finish_deletion`, which a repeat — or the gateway, when it
    /// starts ([`Self::finish_deletions`]) — carries on from wherever the
    /// tombstone says it got to, and which a finished deletion skips.
    pub async fn delete(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
    ) -> Result<bool, ConversationError> {
        let service = self.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let mode = service.inner.mode_changes.lock(&id).await;
            let fenced = service.fence(&id, &caller).await?;
            // The tombstone now rejects new mode changes. Release the mode
            // admission lock before cleanup so repeat deletes preserve their
            // own ordering under the deletion lock.
            drop(mode);
            // The fence is committed, including when another attempt wrote it.
            // Capture does not take admission, and its failure leaves this
            // delete's result alone.
            service.record_inventory().await;
            let (record, applied, _deleting) = match fenced {
                Fence::Written {
                    record,
                    applied,
                    deleting,
                } => (*record, applied, deleting),
                // Whoever erased it has already told the worker it is done.
                Fence::FinishedByAnother { applied } => return Ok(applied),
                Fence::LeftByAnother => {
                    return Err(ConversationError::DeletionIncomplete(Box::new(
                        DeletionFailures {
                            another_attempt: true,
                            ..DeletionFailures::default()
                        },
                    )))
                }
            };
            // Who deleted it, read once, from its tombstone — not this
            // caller, who may be repeating someone else's delete
            // (`c15_a_delete_finished_by_a_repeat_drops_as_the_first_deleter`).
            let deleted_by = deleter(&record);
            let finished = service.finish_deletion(record, &deleted_by).await;
            // Its agent stopped — its apps ended by the deleter — or not:
            // either way nothing names it again, and its apps take no more.
            // A context still held is dropped by the deleter, and reported;
            // the delete answers its own result, not the drop's record
            // (row C15b).
            service.close_apps_for_good(&id, &deleted_by);
            match finished {
                Ok(()) => {
                    // Nothing is left for the worker to carry
                    // (`a_person_s_delete_that_finishes_leaves_nothing_waiting`).
                    service.inner.retries.finished(&id);
                    Ok(applied)
                }
                Err(error) => {
                    service.carry_on_if_it_can_finish(&id, &error);
                    Err(error)
                }
            }
        })
        .await
    }

    /// Reconcile the deletion inventory. A refusal or a failed append is
    /// logged. The tombstone is unchanged, and the caller keeps its result.
    async fn record_inventory(&self) {
        let Some(capture) = self.inner.fence_capture.get() else {
            return;
        };
        if let Err(error) = capture.capture().await {
            tracing::warn!(
                ?error,
                "the deletion inventory was not updated after a committed fence"
            );
        }
    }

    /// Everything a delete does up to and including its own tombstone write:
    /// authorize from the ownership record, take the conversation's delete
    /// lock — waiting for another attempt and answering from the tombstone it
    /// leaves, if one holds it — and write the tombstone.
    ///
    /// Its failures are [`FenceFailure`], so none of them can be answered as one
    /// of the two codes that promise a deletion, whatever a substituted
    /// repository returns (`a_repository_s_error_before_the_fence_is_never_answered_as_a_deletion`).
    pub(super) async fn fence(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Fence, FenceFailure> {
        caller.actor().map_err(|_| FenceFailure::InvalidInput)?;
        let requested_at_ms = self.inner.clock.unix_milliseconds();
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await
            .map_err(RepositoryFailure::from_load)?
            .ok_or(FenceFailure::NotFound)?;
        // The owner deletes their conversation, deleted already or not: a
        // second delete answers from the tombstone the first left.
        match self
            .access()
            .locates(&record, &caller.organization_id, &caller.principal_id)
        {
            Ok(true) => {}
            Ok(false) => return Err(FenceFailure::NotFound),
            Err(_) => return Err(FenceFailure::Repository(RepositoryFailure::Metadata)),
        }
        // Never dated before the conversation it deletes: a clock stepped
        // back since its creation would otherwise propose a tombstone its
        // own record refuses, and every delete of it would be refused as
        // storage trouble. The stored tombstone is still held to that rule
        // when read back (`a_clock_stepped_back_since_creation_still_deletes`).
        let requested_at_ms = requested_at_ms.max(record.creation_requested_at_ms());
        let proposed = ConversationDeletion::new(
            caller.organization_id.clone(),
            caller.principal_id.clone(),
            caller.surface_id.clone(),
            caller.action_id.clone(),
            requested_at_ms,
        )
        .map_err(|_| FenceFailure::InvalidInput)?;
        // A delete that finds another attempt carrying this deletion waits
        // for it and answers from the tombstone it leaves, rather than
        // attempting again: one delete spends at most one attempt
        // (`a_delete_queued_behind_another_attempt_answers_from_its_tombstone`).
        // If that attempt never fenced it, this one does
        // (`a_delete_whose_predecessor_never_fenced_fences_it_itself`).
        let (deleting, waited) = match self.inner.deletions.try_lock(id) {
            Some(guard) => (guard, false),
            None => (self.inner.deletions.lock(id).await, true),
        };
        if waited {
            let current = load_conversation(self.inner.metadata.as_ref(), id)
                .await
                .map_err(RepositoryFailure::from_load)?
                .ok_or(FenceFailure::NotFound)?;
            if let Some(decided) = current.deletion() {
                let applied = decided.is_same_decision(&proposed);
                return Ok(if decided.erased() {
                    Fence::FinishedByAnother { applied }
                } else {
                    Fence::LeftByAnother
                });
            }
        }
        let record = {
            let _creation = self.inner.creation.lock().await;
            self.inner
                .metadata
                .record_deletion(id, proposed.clone())
                .await
                .map_err(RepositoryFailure::from_write)?
        };
        let decided = record.deletion().ok_or(RepositoryFailure::Metadata)?;
        let applied = decided.is_same_decision(&proposed);
        Ok(Fence::Written {
            record: Box::new(record),
            applied,
            deleting,
        })
    }

    /// Finish every deletion a tombstone says did not finish: one that
    /// stopped short of its end — an agent not confirmed stopped, a history
    /// still leased, an agent that could not be asked about its own record, a
    /// deletion record the sink did not take — or that the gateway stopped in
    /// the middle of. Composition starts this once the gateway is listening,
    /// in the background, so it never holds up startup
    /// (`an_unfinished_deletion_is_finished_and_recorded_when_the_gateway_starts`).
    ///
    /// Each gets at most `DELETION_ATTEMPTS` tries, `DELETION_RETRY_DELAY`
    /// apart and growing. What is still unfinished after its tries is
    /// returned with its last typed failure, and logged, and is tried again
    /// at the next start or by a repeated delete. Stops early, returning what
    /// it has, once the service is retired. Tombstones that could not be read
    /// are counted too: a deletion nothing here can see, let alone finish.
    /// Only unfinished deletions are read, not every conversation
    /// (`unfinished_deletions_are_read_by_their_index_and_an_unnamed_one_is_counted`).
    ///
    /// # Errors
    /// The repository's error when the records cannot be enumerated at all.
    pub async fn finish_deletions(&self) -> Result<DeletionsLeft, ConversationError> {
        let UnfinishedDeletions {
            conversations: unfinished,
            unreadable,
        } = self.inner.metadata.unfinished_deletions().await?;
        let mut left = Vec::new();
        for id in unfinished {
            let mut failure = None;
            for attempt in 0..DELETION_ATTEMPTS {
                if attempt > 0 {
                    tokio::time::sleep(DELETION_RETRY_DELAY * attempt).await;
                }
                match self.finish_deletion_in_background(&id).await {
                    // Finished, or being carried by whoever holds it now.
                    Ok(BackgroundTry::Finished | BackgroundTry::HeldElsewhere) => {
                        failure = None;
                        break;
                    }
                    // What this run can see change is carried on by the one
                    // rule for that, not retried here on a clock of its own
                    // (`a_startup_finish_that_finds_every_slot_taken_is_finished_once_one_frees`).
                    Err(error) if self.carry_on_if_it_can_finish(&id, &error) => {
                        failure = None;
                        break;
                    }
                    Err(error) => failure = Some(error),
                }
                if self.inner.retirement.get().is_some() {
                    break;
                }
            }
            if let Some(error) = failure {
                tracing::warn!(
                    conversation_id = %id,
                    %error,
                    "a deleted conversation's erasure is still unfinished; the next start tries again"
                );
                left.push((id, error));
            }
            if self.inner.retirement.get().is_some() {
                break;
            }
        }
        Ok(DeletionsLeft {
            unfinished: left,
            unreadable,
        })
    }

    /// Whether `error` left a deletion unfinished for a reason this run can
    /// see change — every agent slot taken, the conversation's agent still
    /// stopping, the history's lease still held ([`waiting_for`]) — and so one
    /// it carries on itself, rather than leaving it for the next
    /// start; `true` when it does. A cause that cannot change while it runs
    /// (an agent not built, a damaged history, an agent that refused) is left
    /// for the next start, and so is everything once retirement has begun
    /// (`a_delete_turned_away_for_want_of_an_agent_slot_is_finished_once_one_frees`,
    /// `a_deletion_left_for_a_held_lease_is_finished_once_it_is_let_go`,
    /// `nothing_is_carried_on_once_retirement_has_begun`).
    ///
    /// The one rule for what is carried on in-process, for every finisher:
    /// a person's delete, and the finish at start.
    fn carry_on_if_it_can_finish(&self, id: &ConversationId, error: &ConversationError) -> bool {
        let ConversationError::DeletionIncomplete(failures) = error else {
            return false;
        };
        let Some(waiting) = waiting_for(failures) else {
            return false;
        };
        if self.inner.retirement.get().is_some() {
            return false;
        }
        if self.inner.retries.start() {
            let worker = Arc::downgrade(&self.inner);
            let retries = self.inner.retries.clone();
            let retired = self.inner.retired.subscribe();
            drop(tokio::spawn(supervise_carrying_on(
                worker, retries, retired,
            )));
        }
        let generation = self.inner.retries.wait(id, waiting);
        if waiting == Waiting::ForRelease {
            due_again_after(&self.inner.retries, id, generation, 1);
        }
        true
    }

    /// One background try at finishing `id`'s deletion from its tombstone as
    /// it now stands, admitted like a delete.
    ///
    /// Of the repository's error reading the record, only what
    /// [`RepositoryFailure`] keeps is answered, so the one
    /// [`ConversationError::DeletionIncomplete`] a try can answer is
    /// `finish_deletion`'s own
    /// (`a_repository_error_in_a_background_try_is_never_a_reason_to_wait`).
    ///
    /// A background finisher never queues on the conversation's delete lock:
    /// whoever holds it is carrying this deletion, and a finisher waiting its
    /// turn would put a whole attempt between that one and a person's delete
    /// queued behind it (`a_person_s_delete_is_never_queued_behind_a_background_try`).
    async fn finish_deletion_in_background(
        &self,
        id: &ConversationId,
    ) -> Result<BackgroundTry, ConversationError> {
        let service = self.clone();
        let id = id.clone();
        supervised(async move {
            let _admission = service.admit().await?;
            let Some(_deleting) = service.inner.deletions.try_lock(&id) else {
                return Ok(BackgroundTry::HeldElsewhere);
            };
            let record = load_conversation(service.inner.metadata.as_ref(), &id)
                .await
                .map_err(RepositoryFailure::from_load)?
                .ok_or(ConversationError::NotFound)?;
            let deleted_by = deleter(&record);
            service.finish_deletion(record, &deleted_by).await?;
            Ok(BackgroundTry::Finished)
        })
        .await
    }

    /// Carry a fenced deletion to its end, from wherever its tombstone says it
    /// got to, each step in the deciding request's name and each step's
    /// failure kept apart in [`ConversationError::DeletionIncomplete`]. The
    /// caller holds the conversation's deletion lock.
    ///
    /// 1. Stop the live agent as retirement stops one, waiting for a slot that
    ///    is still opening. Unconfirmed, nothing else is done.
    /// 2. Read the provider session from the saved history, under a lease
    ///    this deletion holds and validated as a restoration validates it,
    ///    and keep it in the tombstone.
    /// 3. Ask the conversation's agent, through [`super::ProviderSessionErasers`], to
    ///    delete its own record of that session, and keep what that settled
    ///    in the tombstone. After the stop, so the agent is not running it;
    ///    before the history is erased
    ///    (`the_agents_own_record_is_asked_to_go_after_the_stop_and_before_the_history`).
    ///    A failure leaves the deletion unfinished, to be asked again.
    /// 4. Record [`ConversationDeletionAuditRecord`] from the tombstone, once
    ///    steps 2 and 3 have settled what it says.
    /// 5. Erase: uploads are let go whatever became of the record; the
    ///    history and the summary (under the summary lock) only once the
    ///    record is acknowledged. A history this build cannot read is
    ///    discarded then, without replaying it. A history that opened is
    ///    erased under the same lease.
    /// 6. Mark the tombstone erased, so nothing of it is attempted again.
    ///
    /// Audit stores are not touched
    /// (`deleting_on_the_local_stores_erases_what_it_owns_and_leaves_every_audit_record`).
    ///
    /// Every failure past the fence is a [`ConversationError::DeletionIncomplete`],
    /// since the conversation is deleted whatever else happened: one of a
    /// step's own, a panic in any port it calls
    /// ([`DeletionFailures::interrupted`]), or a tombstone that cannot be read
    /// as a deletion ([`DeletionFailures::tombstone`])
    /// (`a_port_that_panics_after_the_fence_leaves_the_deletion_unfinished`).
    ///
    /// `deleted_by` is who deleted it, read from its tombstone by the caller
    /// ([`deleter`]): its stop ends the apps as theirs.
    async fn finish_deletion(
        &self,
        record: Conversation,
        deleted_by: &McpAppInitiator,
    ) -> Result<(), ConversationError> {
        let id = record.id().clone();
        match AssertUnwindSafe(self.finish_deletion_steps(record, deleted_by))
            .catch_unwind()
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(ConversationError::DeletionIncomplete(failures))) => {
                Err(ConversationError::DeletionIncomplete(failures))
            }
            Ok(Err(error)) => Err(ConversationError::DeletionIncomplete(Box::new(
                DeletionFailures {
                    tombstone: Some(error),
                    ..DeletionFailures::default()
                },
            ))),
            Err(payload) => {
                mem::forget(payload);
                tracing::error!(
                    conversation_id = %id,
                    "a step of a deleted conversation's erasure panicked; it is left unfinished"
                );
                Err(ConversationError::DeletionIncomplete(Box::new(
                    DeletionFailures {
                        interrupted: true,
                        ..DeletionFailures::default()
                    },
                )))
            }
        }
    }
    async fn finish_deletion_steps(
        &self,
        record: Conversation,
        deleted_by: &McpAppInitiator,
    ) -> Result<(), ConversationError> {
        let id = record.id().clone();
        let deletion = record
            .deletion()
            .cloned()
            .ok_or(ConversationError::Metadata)?;
        if deletion.erased() {
            return Ok(());
        }
        let actor = deletion_actor(&deletion)?;
        let slot = self.inner.conversations.lock().await.get(&id).cloned();
        if let Some(slot) = slot {
            // Retirement stops this agent itself, and must not wait here for
            // this delete to do it first. Answered as every step after the
            // tombstone is: the delete happened and did not finish
            // (`a_shutdown_ends_a_delete_waiting_to_stop_or_to_lease_and_it_is_left_unfinished`).
            // Its apps' drops are reported as the stop ends them, before it
            // awaits anything, so retirement winning loses none of them
            // (`c15_a_delete_cut_short_by_retirement_records_each_drop_once`).
            let stopped = tokio::select! {
                stopped = self.stop_slot(
                    &id,
                    slot,
                    StopBy {
                        actor: &actor,
                        ended_by: deleted_by,
                        cause: LeaseEndCause::Closed,
                    },
                ) => stopped,
                () = self.retired() => Err(StopFailure::Failed(AgentError::Closed)),
            };
            match stopped {
                Ok(()) => {}
                // The agent was let go and only the record of it was not
                // saved. That record is in the history this erases, so it is
                // not what the delete waits for
                // (`l7_a_delete_whose_cleanup_record_cannot_be_saved_still_erases_the_history`).
                Err(StopFailure::Failed(error)) if confirmed_despite(&error).is_some() => {
                    tracing::warn!(conversation_id = %id, %error, "a deletion's stop confirmed cleanup whose record was not saved");
                }
                Err(stop) => {
                    return Err(ConversationError::DeletionIncomplete(Box::new(
                        DeletionFailures {
                            stop: Some(stop),
                            ..DeletionFailures::default()
                        },
                    )))
                }
            }
        }
        let mut failures = DeletionFailures::default();
        // `None` when it could not be taken. `Some(None)` when nothing is
        // erased under a lease: the history was never opened, or this build
        // cannot read it. That history is discarded only after the deletion
        // record is acknowledged.
        let leased = tokio::select! {
            leased = self.erasure_lease(&id) => leased.map_err(ConversationError::Storage),
            () = self.retired() => Err(ConversationError::Unavailable),
        };
        let mut lost_unreadable = false;
        let lease = match leased {
            Ok(lease) => Some(lease),
            // Still held after the wait for it: the one place a deletion
            // learns its history is leased elsewhere. `Busy` from reading or
            // erasing under its own lease is only storage failing
            // (`busy_under_the_deletion_s_own_lease_is_left_not_carried_on`).
            Err(ConversationError::Storage(StorageError::Busy)) => {
                failures.history_leased_elsewhere = true;
                None
            }
            // This conversation's saved bytes do not decode, or they name
            // another `schemaVersion`. Settle the provider link as unknown so
            // the delete can be recorded. The bytes stay until that record is
            // acknowledged. A records-database failure is [`StorageError::Store`]
            // and falls through: the tombstone stays unread and the delete retries.
            Err(ConversationError::Storage(
                StorageError::Corrupt(_) | StorageError::AnotherVersion { .. },
            )) => {
                lost_unreadable = true;
                Some(None)
            }
            Err(error) => {
                failures.history = Some(error);
                None
            }
        };
        let mut record = record;
        if lost_unreadable && matches!(deletion.provider_session(), ProviderSessionLink::Unread) {
            match self
                .inner
                .metadata
                .record_deletion(record.id(), deletion.after_losing_history())
                .await
            {
                Ok(kept) => record = kept,
                Err(error) => failures.tombstone = Some(error),
            }
        } else if let (ProviderSessionLink::Unread, Some(lease)) =
            (deletion.provider_session(), &lease)
        {
            // What could not be read is the history's; what was read and
            // could not be kept is the tombstone's
            // (`a_history_read_that_cannot_be_kept_is_the_tombstone_s_failure`).
            match self.read_provider_session(&record, lease.as_deref()).await {
                Ok(read) => match self.inner.metadata.record_deletion(record.id(), read).await {
                    Ok(kept) => record = kept,
                    Err(error) => failures.tombstone = Some(error),
                },
                Err(error) => failures.history = Some(error),
            }
        }
        // The agent is asked only while this deletion holds the history's
        // lease: a lease held elsewhere may be a writer still running the
        // session, and deleting a running session is exactly what the agent
        // must not be asked to do
        // (`the_agent_is_not_asked_while_the_history_is_leased_elsewhere`).
        let record = match &lease {
            Some(_) => self.settle_provider_session(record, &mut failures).await,
            None => record,
        };
        let recorded = match Self::deletion_record(&record) {
            // Unsettled only when the history could not be read or the agent
            // could not be asked, which `failures` already says.
            None => false,
            Some(evidence) => match self.inner.deletion_audit.record(evidence).await {
                Ok(()) => true,
                Err(error) => {
                    failures.audit = Some(error);
                    false
                }
            },
        };
        // No agent is left to read an upload of a stopped, tombstoned
        // conversation, so its uploads go whatever became of the record.
        if let Some(attachments) = &self.inner.attachments {
            if let Err(error) = attachments
                .release(AttachmentRelease {
                    organization_id: record.organization().clone(),
                    conversation_id: id.clone(),
                    cause: AttachmentReleaseCause::ConversationDeleted,
                    initiator_principal_id: deletion.initiator().clone(),
                    initiator_surface_id: deletion.surface().to_owned(),
                    correlation_id: deletion.request().to_owned(),
                })
                .await
            {
                failures.attachments = Some(error);
            }
        }
        if recorded {
            if lost_unreadable {
                match self
                    .inner
                    .storage
                    .discard_unreadable(conversation_session(&id))
                    .await
                {
                    Ok(()) => {}
                    Err(StorageError::Busy) => {
                        failures.history_leased_elsewhere = true;
                    }
                    Err(error) => {
                        failures.history = Some(ConversationError::Storage(error));
                    }
                }
            }
            if let Some(Some(lease)) = &lease {
                if let Err(error) = lease.erase().await {
                    failures.history = Some(ConversationError::Storage(error));
                }
            }
            self.forget_thread_reports_of(&id).await;
            let _writes = self.inner.summary_writes.lock(&id).await;
            if let Err(error) = self.inner.summaries.erase(&id).await {
                failures.summary = Some(error);
            }
            // Where it ran goes with its history; failing to forget it is
            // the summary's kind of failure, retried with the erasure.
            if let Err(error) = self.inner.environment.erase(&id).await {
                failures.summary.get_or_insert(error);
            }
        }
        drop(lease);
        if failures.is_empty() {
            // Finished only if the tombstone can say so: a deletion whose
            // provider session is unsettled has no finished form, so it can
            // never be reported done here, whatever `failures` says.
            match record
                .deletion()
                .and_then(ConversationDeletion::after_erasure)
            {
                Some(finished) => match self.inner.metadata.record_deletion(&id, finished).await {
                    Ok(_) => return Ok(()),
                    Err(error) => failures.tombstone = Some(error),
                },
                None => failures.tombstone = Some(ConversationError::Metadata),
            }
        }
        tracing::error!(
            conversation_id = %id,
            ?failures,
            "a deleted conversation's erasure did not finish"
        );
        Err(ConversationError::DeletionIncomplete(Box::new(failures)))
    }

    /// The delete's own lease on the conversation's saved history, asked for
    /// again while the stopped agent's last handles let go of theirs, for at
    /// most [`super::ConversationDeletionBudgets::history_lease`]. Still held after that is
    /// [`StorageError::Busy`]: somebody else is its writer, so it is not
    /// erased. `None` for a conversation whose history was never opened —
    /// there is nothing to erase, and asking creates nothing
    /// (`deleting_a_conversation_that_never_opened_creates_no_history_lock`).
    async fn erasure_lease(
        &self,
        id: &ConversationId,
    ) -> Result<Option<Box<dyn SessionStorageLease>>, StorageError> {
        let storage = self.inner.storage.clone();
        let session = conversation_session(id);
        self.while_history_busy(|| {
            let storage = storage.clone();
            let session = session.clone();
            async move { storage.open_existing(session).await }
        })
        .await
    }

    /// Read which provider session the saved history names, as the
    /// tombstone that records it; the caller keeps it, before anything erases
    /// that history.
    ///
    /// Read through [`SessionSnapshot::load_saved`], so a snapshot a custom
    /// adapter returns for some other session, or one whose relationships do
    /// not hold, is refused and nothing is kept from it
    /// (`a_history_that_names_another_session_is_refused_and_nothing_is_erased`).
    /// `lease` is `None` when the history was never opened: it named nothing.
    async fn read_provider_session(
        &self,
        record: &Conversation,
        lease: Option<&dyn SessionStorageLease>,
    ) -> Result<ConversationDeletion, ConversationError> {
        let decided = record.deletion().ok_or(ConversationError::Metadata)?;
        let read = match lease {
            Some(lease) => {
                let key = conversation_session(record.id());
                match SessionSnapshot::load_saved(lease, &key)
                    .await
                    .map_err(ConversationError::Storage)?
                {
                    Some(snapshot) => {
                        decided.after_reading(snapshot.provider_context.recorded().cloned())
                    }
                    // A lease with no history behind it: opened, and then its
                    // journal moved aside or never written. Not "named none".
                    None => decided.after_losing_history(),
                }
            }
            None => decided.after_reading(None),
        };
        Ok(read)
    }

    /// Settle what becomes of the agent's own record of the provider session
    /// the tombstone read, if that is not settled yet, and keep it in the
    /// tombstone. A failure is kept in `failures` and leaves it unsettled.
    async fn settle_provider_session(
        &self,
        record: Conversation,
        failures: &mut DeletionFailures,
    ) -> Conversation {
        let Some(deletion) = record.deletion() else {
            return record;
        };
        let ProviderSessionLink::Recorded(session) = deletion.provider_session() else {
            return record;
        };
        if deletion.provider_erasure().is_some() {
            return record;
        }
        let erasure = match self.erase_provider_session(&record, session.clone()).await {
            Ok(erasure) => erasure,
            Err(AskFailure::NoFreeSlot) => {
                failures.no_agent_slot = true;
                return record;
            }
            Err(AskFailure::Failed(error)) => {
                failures.provider = Some(error);
                return record;
            }
        };
        let settled = deletion.after_provider_erasure(erasure);
        match self
            .inner
            .metadata
            .record_deletion(record.id(), settled)
            .await
        {
            Ok(settled) => settled,
            Err(error) => {
                failures.tombstone = Some(error);
                record
            }
        }
    }

    /// Ask the conversation's agent to delete its own record of `session`.
    /// Which agent is asked, and what an answer means, is
    /// [`super::ProviderSessionErasers`]'.
    ///
    /// Retiring this gateway ends the exchange: the ask is abandoned — the
    /// SDK still stops the connection's process and releases what it made —
    /// and answered [`ConversationError::Unavailable`], so the deletion stays
    /// unfinished for the next start, and a retirement is never held waiting
    /// for an agent (`a_shutdown_ends_an_agent_that_is_being_asked_and_a_later_start_finishes`).
    /// A desktop quit's stop pass does not end it.
    ///
    /// A slot is taken only for an agent that will be asked, and nothing is
    /// asked once retirement has begun. With every slot taken the answer is
    /// [`AskFailure::NoFreeSlot`] at once — its own outcome, which no eraser's
    /// error can be taken for
    /// (`an_eraser_answering_capacity_is_a_failure_not_a_slot_wait`); the
    /// deletion is then carried on by this gateway when a slot frees, which
    /// releasing a slot here signals.
    ///
    /// Not held behind the runtime's first-launch preparation: the exchange's
    /// own launch budget is the published one for that scan, and waiting for
    /// preparation first would add a wait with no bound of its own to the
    /// delete.
    async fn erase_provider_session(
        &self,
        record: &Conversation,
        session: ExecutionSessionId,
    ) -> Result<ProviderSessionErasure, AskFailure> {
        // A conversation placed on a host kept its harness's session there,
        // which no handler on this machine can reach: said as no handler,
        // never asked of a local binding.
        if self
            .inner
            .environment
            .placement(record.id())
            .await
            .map_err(AskFailure::Failed)?
            .is_some()
        {
            return Ok(ProviderSessionErasure::NoHandler);
        }
        let eraser = match self
            .inner
            .provider_sessions
            .handler(record.agent())
            .map_err(AskFailure::Failed)?
        {
            ProviderSessionHandler::NoHandler => return Ok(ProviderSessionErasure::NoHandler),
            ProviderSessionHandler::Ask(eraser) => eraser,
        };
        // Nothing is launched once retirement has begun, and a slot is taken
        // only for an agent that will be asked
        // (`nothing_is_asked_once_retirement_has_begun`,
        // `a_conversation_nobody_can_ask_about_takes_no_agent_slot`).
        if self.inner.retirement.get().is_some() {
            return Err(AskFailure::Failed(ConversationError::Unavailable));
        }
        let Ok(permit) = self.inner.agents_asked.clone().try_acquire_owned() else {
            return Err(AskFailure::NoFreeSlot);
        };
        // Held for the ask; letting it go wakes deletions waiting for a slot,
        // however the ask ends — answered, abandoned, or panicking.
        let _slot = AgentSlot {
            permit: Some(permit),
            retries: self.inner.retries.clone(),
        };
        // An ask that panics is a failure of this attempt's ask, like any
        // other: the tombstone is written, so the delete happened and is
        // answered unfinished (`conversation_erasure_incomplete`), not as
        // the command failing (`a_slot_freed_by_an_ask_that_panicked_still_wakes_those_waiting`).
        let asked = async {
            match AssertUnwindSafe(eraser.erase(session)).catch_unwind().await {
                Ok(erased) => erased,
                Err(payload) => {
                    mem::forget(payload);
                    tracing::error!(
                        conversation_id = %record.id(),
                        "the agent's handler panicked while asked to delete its own session"
                    );
                    Err(ConversationError::Unavailable)
                }
            }
        };
        tokio::select! {
            biased;
            () = self.retired() => Err(AskFailure::Failed(ConversationError::Unavailable)),
            erased = asked => erased.map_err(AskFailure::Failed),
        }
    }

    /// The deletion record a tombstone stands for, or `None` while what it
    /// says about the provider session is not settled.
    fn deletion_record(record: &Conversation) -> Option<ConversationDeletionAuditRecord> {
        let deletion = record.deletion()?;
        let provider_erasure = deletion.provider_erasure()?;
        let provider_session_id = match deletion.provider_session() {
            ProviderSessionLink::Recorded(session) => Some(session.clone()),
            ProviderSessionLink::Unread
            | ProviderSessionLink::Absent
            | ProviderSessionLink::Unknown => None,
        };
        Some(ConversationDeletionAuditRecord {
            conversation_id: record.id().clone(),
            organization_id: record.organization().clone(),
            owner_id: record.owner().clone(),
            before: ConversationOwnershipState::Owned,
            after: ConversationOwnershipState::Deleted,
            cause: ConversationDeletionCause::CallerRequested,
            initiator_principal_id: deletion.initiator().clone(),
            initiator_surface_id: deletion.surface().to_owned(),
            correlation_id: deletion.request().to_owned(),
            provider_session_id,
            provider_erasure,
            requested_at_ms: deletion.requested_at_ms(),
        })
    }
}

/// What a deletion left unfinished with `failures` is waiting for, when it is
/// something this run can see change: an agent slot; or a release — the
/// conversation's agent confirming a stop that ran past the stop budget, or
/// the history's lease let go. `None` for anything else, which only the next
/// start can finish. The one classification, for a person's delete, the
/// finish at start, and the worker alike.
///
/// A stop past its budget is carried on, not left for the next start: the
/// agent may take longer to stop than a delete waits — an ACP agent's close is
/// bounded by its own shutdown budgets, not by `stopMs` — and a delete the
/// person has already seen succeed must not wait for a restart to finish
/// (`a_delete_whose_agent_stops_after_the_stop_budget_is_finished_once_it_has`).
fn waiting_for(failures: &DeletionFailures) -> Option<Waiting> {
    if failures.no_agent_slot {
        return Some(Waiting::ForSlot);
    }
    let still_stopping = matches!(failures.stop, Some(StopFailure::OverBudget));
    (still_stopping || failures.history_leased_elsewhere).then_some(Waiting::ForRelease)
}

/// Who deleted `record`, by its tombstone: the one source for whom its
/// deletion ends its apps, withdraws their reviews, lets go of their tickets
/// and drops what they held, whoever finishes it. The system when it has
/// none that names a command.
fn deleter(record: &Conversation) -> McpAppInitiator {
    record
        .deletion()
        .and_then(|deletion| deletion_actor(deletion).ok())
        .map_or(McpAppInitiator::System, |actor| initiator_of(&actor))
}

/// The caller recorded on a deletion, as the command that decided it.
fn deletion_actor(deletion: &ConversationDeletion) -> Result<ActionContext, ConversationError> {
    ActionContext::new(
        deletion.initiator().as_str(),
        deletion.surface(),
        deletion.request(),
    )
    .map_err(|_| ConversationError::Metadata)
}

/// What [`ConversationService::finish_deletions`] could not finish.
#[derive(Debug)]
pub struct DeletionsLeft {
    /// Each deletion still unfinished after its tries, with its last typed
    /// failure.
    pub unfinished: Vec<(ConversationId, ConversationError)>,
    /// How many tombstones of unfinished deletions could not be read, so
    /// whose deletion is unfinished cannot be known.
    pub unreadable: usize,
}

/// Why the agent was not asked, or its ask failed.
enum AskFailure {
    /// Every agent slot on this gateway was taken.
    NoFreeSlot,
    /// Anything else: the agent's handler, or the ask itself, failed.
    Failed(ConversationError),
}

/// What a deletion keeps of a repository's failure where that failure is
/// its answer — the delete's fence, and a background try's read of the
/// record: only what [`super::ConversationRepository`]'s contract lets that call
/// answer. Every other error — one a substituted repository returns
/// included — is [`Self::Metadata`], so none there can be answered as a
/// deletion or taken for a reason to wait
/// (`a_repository_s_error_before_the_fence_is_never_answered_as_a_deletion`,
/// `a_repository_error_in_a_background_try_is_never_a_reason_to_wait`).
///
/// Past the fence, a repository's error is kept whole inside
/// [`DeletionFailures`], and only its own fields decide a wait
/// ([`waiting_for`];
/// `a_repository_error_past_the_fence_is_never_a_reason_to_wait`).
#[derive(Debug)]
pub(super) enum RepositoryFailure {
    /// No record to delete.
    NotFound,
    /// A record from before records named their agent.
    AgentUnsupported,
    /// The record could not be read or written:
    /// [`ConversationError::Metadata`].
    Metadata,
}
impl RepositoryFailure {
    /// What [`super::ConversationRepository::load`]'s failure can say: a record
    /// from before agents were named, or storage failing. A missing record is
    /// no error there, so a `NotFound` from it is storage failing too.
    fn from_load(error: ConversationError) -> Self {
        match error {
            ConversationError::AgentUnsupported => Self::AgentUnsupported,
            _ => Self::Metadata,
        }
    }
    /// What [`super::ConversationRepository::record_deletion`]'s failure can say:
    /// no record to delete, a record from before agents were named, or
    /// storage failing.
    fn from_write(error: ConversationError) -> Self {
        match error {
            ConversationError::NotFound => Self::NotFound,
            ConversationError::AgentUnsupported => Self::AgentUnsupported,
            _ => Self::Metadata,
        }
    }
}
impl From<RepositoryFailure> for ConversationError {
    fn from(failure: RepositoryFailure) -> Self {
        match failure {
            RepositoryFailure::NotFound => Self::NotFound,
            RepositoryFailure::AgentUnsupported => Self::AgentUnsupported,
            RepositoryFailure::Metadata => Self::Metadata,
        }
    }
}

/// Why a delete failed before its own tombstone write succeeded. Whether the
/// conversation is fenced is not known — a write that failed may still have
/// landed — so nothing here becomes `conversation_erasure_incomplete` or
/// `audit_unavailable`, the two answers that promise a deletion: of a
/// repository's error, only what [`RepositoryFailure`] keeps.
#[derive(Debug)]
pub(super) enum FenceFailure {
    /// The request's own attribution or deletion could not be made.
    InvalidInput,
    /// No conversation of the caller's by that identity.
    NotFound,
    /// The repository failed.
    Repository(RepositoryFailure),
}
impl From<RepositoryFailure> for FenceFailure {
    fn from(failure: RepositoryFailure) -> Self {
        Self::Repository(failure)
    }
}
impl From<FenceFailure> for ConversationError {
    fn from(failure: FenceFailure) -> Self {
        match failure {
            FenceFailure::InvalidInput => Self::InvalidInput,
            FenceFailure::NotFound => Self::NotFound,
            FenceFailure::Repository(failure) => failure.into(),
        }
    }
}

/// How a delete stood once it was past [`ConversationService::fence`].
pub(super) enum Fence {
    /// This delete wrote its tombstone, or carried the one there: the
    /// conversation as it now stands, whether this request is the deciding
    /// one, and the delete lock it holds to finish it.
    Written {
        record: Box<Conversation>,
        applied: bool,
        deleting: OwnedMutexGuard<()>,
    },
    /// Another attempt, which this delete waited for, finished it.
    FinishedByAnother { applied: bool },
    /// Another attempt, which this delete waited for, fenced it and did not
    /// finish.
    LeftByAnother,
}

/// What one background try came to, when it did not fail.
enum BackgroundTry {
    /// The deletion is finished.
    Finished,
    /// Another attempt holds it now, and is carrying it.
    HeldElsewhere,
}

/// Mark `id` due a try again after `tries` × [`DELETION_RETRY_DELAY`]: a
/// lease wait's own schedule, or one delay for a claim another attempt held.
fn due_again_after(
    retries: &Arc<DeletionRetries>,
    id: &ConversationId,
    generation: u64,
    tries: u32,
) {
    let retries = retries.clone();
    let id = id.clone();
    drop(tokio::spawn(async move {
        tokio::time::sleep(DELETION_RETRY_DELAY * tries).await;
        retries.due_again(&id, generation);
    }));
}

/// Run the worker, and if it ever ends by panicking, say so loudly and run
/// another after [`DELETION_RETRY_DELAY`] — with every waiting deletion due,
/// since a try it had claimed died with it — rather than leave deletions
/// piling up with nobody to carry them. Ends when the worker ends as told:
/// the service retired or gone.
async fn supervise_carrying_on(
    service: Weak<Inner>,
    retries: Arc<DeletionRetries>,
    retired: watch::Receiver<bool>,
) {
    let alive = service.clone();
    let worker_retries = retries.clone();
    let worker_retired = retired.clone();
    supervise(
        retries,
        retired,
        move || alive.strong_count() > 0,
        move || {
            carry_on(
                service.clone(),
                worker_retries.clone(),
                worker_retired.clone(),
            )
        },
    )
    .await;
}

/// [`supervise_carrying_on`]'s rule, apart from the worker it runs, so a
/// worker that panics can be tested
/// (`a_worker_that_panics_is_replaced_with_every_waiting_deletion_due`).
async fn supervise<Worker: Future<Output = ()>>(
    retries: Arc<DeletionRetries>,
    retired: watch::Receiver<bool>,
    alive: impl Fn() -> bool,
    mut worker: impl FnMut() -> Worker,
) {
    loop {
        let ended = AssertUnwindSafe(worker()).catch_unwind().await;
        let Err(payload) = ended else {
            break;
        };
        mem::forget(payload);
        tracing::error!(
            "the worker carrying on unfinished deletions panicked; another starts shortly"
        );
        tokio::time::sleep(DELETION_RETRY_DELAY).await;
        if *retired.borrow() || !alive() {
            break;
        }
        retries.all_due();
    }
    retries.stopped();
}

/// The one worker that carries on deletions this run left unfinished for a
/// reason it can see change, by the same path everything finishes a deletion
/// by. It wakes when something may be due and tries each deletion that is:
/// one waiting for a slot when a slot freed, spending no try; one waiting for
/// a release — a stop or a lease — only when its own timer fires — [`DELETION_ATTEMPTS`] tries,
/// [`DELETION_RETRY_DELAY`] apart and growing — never on another deletion's
/// wake (`a_lease_wait_is_not_spent_by_slots_freeing`). A deletion some other
/// attempt holds is left to it. Anything else is left for the next start.
/// Holds nothing of the service while it waits, and ends when the service is
/// retired or gone (`the_worker_carrying_deletions_on_ends_with_retirement`).
async fn carry_on(
    service: Weak<Inner>,
    retries: Arc<DeletionRetries>,
    mut retired: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            () = retries.woken() => {}
            _ = retired.wait_for(|retired| *retired) => return,
        }
        for claim in retries.claim_due() {
            if *retired.borrow() {
                return;
            }
            let Some(inner) = service.upgrade() else {
                return;
            };
            let answer = ConversationService { inner }
                .finish_deletion_in_background(&claim.id)
                .await;
            carried(&retries, &claim, answer);
        }
    }
}

/// Record what `claim`'s try came to.
fn carried(
    retries: &Arc<DeletionRetries>,
    claim: &Claim,
    answer: Result<BackgroundTry, ConversationError>,
) {
    let failures = match answer {
        Ok(BackgroundTry::Finished) => return retries.done(claim),
        // Somebody else holds it — an attempt carrying it, or only a delete
        // reading the tombstone to answer — so this one is due again once
        // the holder has had time to let go, without spending a try: a lease
        // wait on its own schedule, a slot wait after one delay. Nothing else
        // may wake a slot wait, since no ask need be running
        // (`a_claim_found_held_is_tried_again_once_its_holder_lets_go`).
        Ok(BackgroundTry::HeldElsewhere) => {
            let tries = match claim.waiting {
                Waiting::ForRelease => claim.release_tries + 1,
                Waiting::ForSlot => 1,
            };
            return due_again_after(retries, &claim.id, claim.generation, tries);
        }
        Err(ConversationError::DeletionIncomplete(failures)) => failures,
        Err(error) => {
            tracing::warn!(conversation_id = %claim.id, %error, "a deletion is left for the next start");
            return retries.done(claim);
        }
    };
    // Only a try its release timer ran spends one of the release's tries: a
    // slot wait that turns to waiting for a release starts that wait where it
    // stood (`a_slot_wait_that_turns_to_a_release_wait_spends_no_release_try`).
    let tries = match claim.waiting {
        Waiting::ForRelease => claim.release_tries + 1,
        Waiting::ForSlot => claim.release_tries,
    };
    match waiting_for(&failures) {
        Some(Waiting::ForSlot) => {
            return retries.requeue(claim, Waiting::ForSlot, claim.release_tries);
        }
        Some(Waiting::ForRelease) if tries < DELETION_ATTEMPTS => {
            retries.requeue(claim, Waiting::ForRelease, tries);
            return due_again_after(retries, &claim.id, claim.generation, tries + 1);
        }
        Some(Waiting::ForRelease) | None => {}
    }
    tracing::warn!(conversation_id = %claim.id, ?failures, "a deletion is left for the next start");
    retries.done(claim);
}

#[cfg(test)]
#[path = "../../../../tests/conversation/deletion.rs"]
mod deletion_tests;
