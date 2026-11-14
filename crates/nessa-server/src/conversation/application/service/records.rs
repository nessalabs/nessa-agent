//! What this run keeps about each conversation, apart from its live agent
//! (`Slot`): one record per conversation, in one table behind one lock.
//!
//! A record holds the conversation's gates — the per-conversation locks that
//! put one kind of work on it in turn — and what this run still owes it: a
//! model plan to commit, a failure retry, a fork seed, and its MCP Apps.
//! Keeping them together means one lookup finds all of a conversation's
//! state, and nothing about one conversation can sit in a map another part
//! of the service forgot to clear.
//!
//! The table's lock is a plain mutex held only for a lookup or a small
//! change, never across an await, and nothing done under it reaches back into
//! the table. The gates themselves are async locks, each its own, so taking
//! one never waits behind another kind of work on the same conversation, nor
//! behind any work on another conversation
//! (`summary_writes_of_one_conversation_do_not_wait_for_another`).

use super::{handoffs::ForkSeed, resumption::Retry, AppReviews};
use nessa_protocol::conversation::domain::ConversationId;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex as StdMutex, MutexGuard, PoisonError, Weak},
};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// The fewest records the table holds before it sweeps out empty ones.
const FIRST_SWEEP: usize = 64;

/// One kind of work a conversation takes one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Gate {
    /// One summary change at a time, so a turn's reply and the next message
    /// cannot each read the same summary and have the later write lose the
    /// other's change. An archive and a delete's erasure of the summary take
    /// it too, and each writer asks whether the conversation was deleted
    /// while holding it, so a summary is not written back after its erasure
    /// (`a_late_reply_cannot_write_back_a_deleted_summary`).
    SummaryWrites,
    /// One delete at a time, from its fence to its last erasure, so a second
    /// delete waits for the first and answers from the tombstone it left
    /// (`concurrent_deletes_of_one_conversation_run_one_after_the_other`).
    Deletions,
    /// One submission or mode change at a time.
    ModeChanges,
    /// One create command at a time, from its reopen check to its saved
    /// readiness. Each request keeps its own command history, so without it
    /// two requests for one new conversation could both find it missing and
    /// the second open it as a conflict
    /// (`two_requests_creating_one_conversation_both_open_it`).
    Creations,
    /// The conversation's create, reopen and deletion decisions, in turn.
    /// Per conversation, so creating one never waits behind another's
    /// metadata and audit writes.
    Creation,
}

const GATES: usize = 5;

impl Gate {
    fn index(self) -> usize {
        match self {
            Self::SummaryWrites => 0,
            Self::Deletions => 1,
            Self::ModeChanges => 2,
            Self::Creations => 3,
            Self::Creation => 4,
        }
    }
}

/// What this run keeps about one conversation.
#[derive(Default)]
pub(super) struct ConversationRecord {
    /// Each gate exists while somebody holds it or waits for it. Once nobody
    /// does, its record is forgotten by the next sweep when nothing else is
    /// kept in it (`records_nobody_holds_or_is_owed_are_swept`).
    gates: [Weak<Mutex<()>>; GATES],
    /// An applied model change whose continuation plan has not committed.
    /// [`super::ConversationService::commit_applied_model_plan`] is the only
    /// writer. A crash drops it; catch-up runs the same plan while the
    /// preference is on.
    pub(super) model_reconcile_owed: bool,
    /// The unfinished-turn mark this run has written or read, so a busy
    /// conversation's turns write one mark, not one each, and its failure
    /// retry. Set under the conversation's submission gate, and cleared under
    /// it too, except where nothing can still run there: an agent closed for
    /// good, a conversation deleted.
    pub(super) unfinished: Option<Retry>,
    /// A fork one creation asked for, waiting for that creation's opening.
    pub(super) fork: Option<ForkSeed>,
    /// The conversation's MCP Apps, kept for the conversation rather than its
    /// live agent: a release is kept across openings and before one. Once
    /// made they are kept, deleted ones too, so a release or an opening
    /// racing a delete finds them deleted and cannot build them afresh.
    pub(super) apps: Option<Arc<AppReviews>>,
}

impl ConversationRecord {
    /// Nothing held, nothing waited for, nothing owed: it can be forgotten.
    fn is_empty(&self) -> bool {
        self.gates.iter().all(|gate| gate.strong_count() == 0)
            && !self.model_reconcile_owed
            && self.unfinished.is_none()
            && self.fork.is_none()
            && self.apps.is_none()
    }
}

/// Every conversation's record, behind one lock.
pub(super) struct ConversationRecords {
    table: StdMutex<Table>,
}

struct Table {
    records: HashMap<ConversationId, ConversationRecord>,
    /// The conversations whose record holds an unfinished mark, so the
    /// failure-retry loop walks those alone, not every conversation opened.
    /// Kept by [`Table::settle`] after every change to a record.
    unfinished: HashSet<ConversationId>,
    /// The size at which the next sweep runs: twice what the last sweep
    /// kept, so sweeping costs a constant share of the work that grew it.
    sweep_at: usize,
}

impl Default for ConversationRecords {
    fn default() -> Self {
        Self {
            table: StdMutex::new(Table {
                records: HashMap::new(),
                unfinished: HashSet::new(),
                sweep_at: FIRST_SWEEP,
            }),
        }
    }
}

impl Table {
    /// `id`'s record, made when it has none.
    fn record(&mut self, id: &ConversationId) -> &mut ConversationRecord {
        if !self.records.contains_key(id) && self.records.len() >= self.sweep_at {
            self.records.retain(|_, record| !record.is_empty());
            self.sweep_at = FIRST_SWEEP.max(self.records.len().saturating_mul(2));
        }
        self.records.entry(id.clone()).or_default()
    }

    /// After a change to `id`'s record: note whether it holds an unfinished
    /// mark, and forget the record when it holds nothing any more.
    fn settle(&mut self, id: &ConversationId) {
        match self.records.get(id) {
            Some(record) if record.unfinished.is_some() => {
                self.unfinished.insert(id.clone());
                return;
            }
            Some(record) if record.is_empty() => {
                self.records.remove(id);
            }
            _ => {}
        }
        self.unfinished.remove(id);
    }
}

impl ConversationRecords {
    fn table(&self) -> MutexGuard<'_, Table> {
        // Nothing is awaited while this is held, so a poisoned table is one
        // whose last change finished: taking it back is safe.
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One gate of every conversation.
    pub(super) fn gate(&self, gate: Gate) -> Gates<'_> {
        Gates {
            records: self,
            gate,
        }
    }

    pub(super) fn summary_writes(&self) -> Gates<'_> {
        self.gate(Gate::SummaryWrites)
    }

    pub(super) fn deletions(&self) -> Gates<'_> {
        self.gate(Gate::Deletions)
    }

    pub(super) fn mode_changes(&self) -> Gates<'_> {
        self.gate(Gate::ModeChanges)
    }

    pub(super) fn creations(&self) -> Gates<'_> {
        self.gate(Gate::Creations)
    }

    pub(super) fn creation(&self) -> Gates<'_> {
        self.gate(Gate::Creation)
    }

    /// Read or change `id`'s record. `change` must not reach back into the
    /// records: the table's lock is held while it runs.
    pub(super) fn with<R>(
        &self,
        id: &ConversationId,
        change: impl FnOnce(&mut ConversationRecord) -> R,
    ) -> R {
        let mut table = self.table();
        let answer = change(table.record(id));
        table.settle(id);
        answer
    }

    /// Read `id`'s record, if this run keeps one, without making it.
    pub(super) fn read<R>(
        &self,
        id: &ConversationId,
        read: impl FnOnce(Option<&ConversationRecord>) -> R,
    ) -> R {
        read(self.table().records.get(id))
    }

    /// Change the failure retry of every conversation holding an unfinished
    /// mark. `change` must not reach back into the records: the table's lock
    /// is held while it runs. It changes the retry, never whether there is one.
    pub(super) fn each_unfinished(&self, mut change: impl FnMut(&ConversationId, &mut Retry)) {
        let mut table = self.table();
        let Table {
            records,
            unfinished,
            ..
        } = &mut *table;
        for id in unfinished.iter() {
            if let Some(retry) = records
                .get_mut(id)
                .and_then(|record| record.unfinished.as_mut())
            {
                change(id, retry);
            }
        }
    }

    fn gate_lock(&self, id: &ConversationId, gate: Gate) -> Arc<Mutex<()>> {
        let mut table = self.table();
        let slot = &mut table.record(id).gates[gate.index()];
        match slot.upgrade() {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(Mutex::new(()));
                *slot = Arc::downgrade(&lock);
                lock
            }
        }
    }

    /// How many conversations have a gate somebody holds or waits for.
    #[cfg(test)]
    pub(super) fn in_use(&self) -> usize {
        self.table()
            .records
            .values()
            .filter(|record| record.gates.iter().any(|gate| gate.strong_count() > 0))
            .count()
    }

    /// How many conversations have a fork seed waiting for its opening.
    #[cfg(test)]
    pub(super) fn forks_waiting(&self) -> usize {
        self.table()
            .records
            .values()
            .filter(|record| record.fork.is_some())
            .count()
    }

    /// How many records the table keeps now, empty ones not yet swept too.
    #[cfg(test)]
    pub(super) fn kept(&self) -> usize {
        self.table().records.len()
    }
}

/// One gate of every conversation: [`ConversationRecords::gate`].
pub(super) struct Gates<'a> {
    records: &'a ConversationRecords,
    gate: Gate,
}

impl Gates<'_> {
    /// Wait for `id`'s gate and hold it until the guard is dropped.
    pub(super) async fn lock(&self, id: &ConversationId) -> OwnedMutexGuard<()> {
        self.entry(id).lock_owned().await
    }

    /// `id`'s gate if nobody holds it now, or `None` without waiting.
    pub(super) fn try_lock(&self, id: &ConversationId) -> Option<OwnedMutexGuard<()>> {
        self.entry(id).try_lock_owned().ok()
    }

    fn entry(&self, id: &ConversationId) -> Arc<Mutex<()>> {
        self.records.gate_lock(id, self.gate)
    }

    /// Retained references to `id`'s gate, guards and unpolled lock futures
    /// included. This does not establish FIFO waiter admission
    /// (`an_unpolled_lock_future_does_not_establish_waiter_order`).
    #[cfg(test)]
    pub(super) fn holders_and_waiters(&self, id: &ConversationId) -> usize {
        self.records.read(id, |record| {
            record.map_or(0, |record| record.gates[self.gate.index()].strong_count())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::{poll_fn, Future},
        task::Poll,
    };

    fn id(n: u32) -> ConversationId {
        ConversationId::new(&format!("00000000-0000-0000-0000-{n:012}")).unwrap()
    }

    #[tokio::test]
    async fn an_unpolled_lock_future_does_not_establish_waiter_order() {
        let records = ConversationRecords::default();
        let locks = records.summary_writes();
        let id = id(651);
        let held = locks.lock(&id).await;
        let mut unpolled = Box::pin(locks.entry(&id).lock_owned());
        assert_eq!(locks.holders_and_waiters(&id), 2);

        let mut first = Box::pin(locks.lock(&id));
        assert!(
            poll_fn(|cx| Poll::Ready(
                std::pin::pin!(tokio::task::unconstrained(first.as_mut()))
                    .poll(cx)
                    .is_pending()
            ))
            .await
        );
        assert_eq!(locks.holders_and_waiters(&id), 3);
        drop(held);
        let acquired_first = tokio::time::timeout(std::time::Duration::from_secs(5), first)
            .await
            .expect("the polled waiter acquires before the unpolled future");
        // The future counted before `first` still has no queue position.
        assert!(
            poll_fn(|cx| Poll::Ready(
                std::pin::pin!(tokio::task::unconstrained(unpolled.as_mut()))
                    .poll(cx)
                    .is_pending()
            ))
            .await
        );
        drop(acquired_first);
        let acquired_second = unpolled.await;
        drop(acquired_second);
        assert_eq!(records.in_use(), 0);
    }

    #[tokio::test]
    async fn one_gate_of_a_conversation_does_not_wait_for_another() {
        let records = ConversationRecords::default();
        let id = id(1);
        let _deleting = records.deletions().lock(&id).await;
        assert!(records.mode_changes().try_lock(&id).is_some());
        assert!(records.summary_writes().try_lock(&id).is_some());
        assert!(records.deletions().try_lock(&id).is_none());
    }

    #[tokio::test]
    async fn records_nobody_holds_or_is_owed_are_swept() {
        let records = ConversationRecords::default();
        for n in 0..(FIRST_SWEEP as u32 * 4) {
            drop(records.creation().lock(&id(n)).await);
        }
        assert_eq!(records.in_use(), 0);
        assert!(records.kept() <= FIRST_SWEEP, "kept {}", records.kept());
    }

    #[test]
    fn only_marked_conversations_are_walked_for_retries() {
        let records = ConversationRecords::default();
        let (marked, other) = (id(3), id(4));
        records.with(&other, |record| record.model_reconcile_owed = true);
        records.with(&marked, |record| record.unfinished = Some(Retry::default()));
        let mut walked = Vec::new();
        records.each_unfinished(|id, _| walked.push(id.clone()));
        assert_eq!(walked, vec![marked.clone()]);
        records.with(&marked, |record| record.unfinished = None);
        let mut walked = 0;
        records.each_unfinished(|_, _| walked += 1);
        assert_eq!(walked, 0);
        assert_eq!(records.kept(), 1);
    }

    #[test]
    fn owed_state_is_kept_until_it_is_cleared() {
        let records = ConversationRecords::default();
        let id = id(2);
        records.with(&id, |record| record.model_reconcile_owed = true);
        assert!(records.read(&id, |record| record.is_some_and(|r| r.model_reconcile_owed)));
        records.with(&id, |record| record.model_reconcile_owed = false);
        assert_eq!(records.kept(), 0);
    }
}
