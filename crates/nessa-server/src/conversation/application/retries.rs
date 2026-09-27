use crate::conversation::domain::ConversationId;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex as StdMutex, MutexGuard, PoisonError,
    },
};
use tokio::sync::Notify;

/// Why a deletion this run carries on is waiting, which decides what lets it
/// be tried again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Waiting {
    /// Every agent slot was taken: tried when one frees, spending no try.
    ForSlot,
    /// Something was still letting go — the conversation's agent had not
    /// confirmed its stop within the stop budget, or the history's lease was
    /// held by a writer: tried when its own timer fires, and only then,
    /// with a bounded number of tries.
    ForRelease,
}

/// One waiting deletion, as the worker claims it for a try. The generation
/// names the reason it was claimed under: what the try finds is recorded only
/// if no newer reason was recorded meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Claim {
    pub(super) id: ConversationId,
    pub(super) waiting: Waiting,
    pub(super) release_tries: u32,
    pub(super) generation: u64,
    /// How many slots had freed when it was claimed.
    pub(super) slots_freed: u64,
}

struct Entry {
    waiting: Waiting,
    release_tries: u32,
    generation: u64,
    due: bool,
}
#[derive(Default)]
struct Table {
    entries: HashMap<ConversationId, Entry>,
    generations: u64,
    /// Every agent slot that has freed, counted, so a try that ends can ask
    /// whether one freed while it ran.
    slots_freed: u64,
}

/// Deletions this run left unfinished for a reason that can change while it
/// runs — every agent slot taken, the conversation's agent still stopping, the
/// history's lease held by a writer still letting go — and that it carries on
/// itself, since the person who asked has
/// already seen the conversation go and cannot ask again.
///
/// Only the bookkeeping lives here: which deletions wait and why, how many
/// timed tries each has had, which are due a try, and the wake that says one
/// is. The tries are `ConversationService::finish_deletion`, the one path every
/// deletion is finished by, run by one worker the service starts when it first
/// has something to carry on. A worker that panics is replaced by its
/// supervisor after a short delay, with every waiting deletion made due; one
/// that ends as told — the service retired or gone — is not.
///
/// The newest reason wins: a failure recorded by [`Self::wait`] starts fresh,
/// and what a try that was claimed earlier finds is dropped if a newer reason
/// was recorded meanwhile (`the_newest_reason_a_deletion_waits_for_wins`).
#[derive(Default)]
pub(super) struct DeletionRetries {
    table: StdMutex<Table>,
    wake: Notify,
    running: AtomicBool,
}
impl DeletionRetries {
    /// Carry `id` on, waiting for `waiting`, starting fresh. Returns the
    /// generation to schedule a release timer under.
    pub(super) fn wait(&self, id: &ConversationId, waiting: Waiting) -> u64 {
        let mut table = self.table();
        table.generations += 1;
        let generation = table.generations;
        table.entries.insert(
            id.clone(),
            Entry {
                waiting,
                release_tries: 0,
                generation,
                due: waiting == Waiting::ForSlot,
            },
        );
        drop(table);
        self.wake.notify_one();
        generation
    }
    /// An agent slot freed: every deletion waiting for one is due a try.
    pub(super) fn slot_freed(&self) {
        let mut table = self.table();
        table.slots_freed += 1;
        for entry in table.entries.values_mut() {
            if entry.waiting == Waiting::ForSlot {
                entry.due = true;
            }
        }
        drop(table);
        self.wake.notify_one();
    }
    /// A timer set for `id` under `generation` fired — its release wait
    /// elapsed, or the attempt that held it when it was last tried has had
    /// time to let go: it is due a try, unless a newer reason has been
    /// recorded since.
    pub(super) fn due_again(&self, id: &ConversationId, generation: u64) {
        if let Some(entry) = self.table().entries.get_mut(id) {
            if entry.generation == generation {
                entry.due = true;
            }
        }
        self.wake.notify_one();
    }
    /// Until something may be due.
    pub(super) async fn woken(&self) {
        self.wake.notified().await;
    }
    /// Claim every deletion due a try now.
    pub(super) fn claim_due(&self) -> Vec<Claim> {
        let mut table = self.table();
        let slots_freed = table.slots_freed;
        table
            .entries
            .iter_mut()
            .filter(|(_, entry)| entry.due)
            .map(|(id, entry)| {
                entry.due = false;
                Claim {
                    id: id.clone(),
                    waiting: entry.waiting,
                    release_tries: entry.release_tries,
                    generation: entry.generation,
                    slots_freed,
                }
            })
            .collect()
    }
    /// After `claim`'s try: wait for `waiting` with `release_tries` spent,
    /// unless a newer reason was recorded meanwhile.
    ///
    /// Whether it is due again is judged by the new reason, whatever it was
    /// claimed under — one rule: due when what it now waits for happened
    /// during the try. Waiting for a slot, it is due if a slot freed since it
    /// was claimed, which a wake judged against its old reason would have
    /// missed; waiting for a release, it is not due until its own timer fires,
    /// whatever freed meanwhile
    /// (`a_slot_freed_during_a_try_is_not_lost_when_it_turns_to_waiting_for_one`,
    /// `a_try_that_turns_to_waiting_on_the_lease_waits_for_its_own_timer`).
    pub(super) fn requeue(&self, claim: &Claim, waiting: Waiting, release_tries: u32) {
        let mut table = self.table();
        let freed_during_the_try = table.slots_freed != claim.slots_freed;
        let Some(entry) = table.entries.get_mut(&claim.id) else {
            return;
        };
        if entry.generation != claim.generation {
            return;
        }
        entry.waiting = waiting;
        entry.release_tries = release_tries;
        entry.due = match waiting {
            Waiting::ForSlot => freed_during_the_try,
            Waiting::ForRelease => false,
        };
        let due = entry.due;
        drop(table);
        if due {
            self.wake.notify_one();
        }
    }
    /// After `claim`'s try: stop carrying it on — finished, or left for the
    /// next start — unless a newer reason was recorded meanwhile.
    pub(super) fn done(&self, claim: &Claim) {
        let mut table = self.table();
        if table
            .entries
            .get(&claim.id)
            .is_some_and(|entry| entry.generation == claim.generation)
        {
            table.entries.remove(&claim.id);
        }
    }
    /// `id` is finished, whoever finished it: nothing is left to carry.
    pub(super) fn finished(&self, id: &ConversationId) {
        self.table().entries.remove(id);
    }
    /// Every waiting deletion is due a try now: a worker that died mid-pass
    /// took its claims with it.
    pub(super) fn all_due(&self) {
        for entry in self.table().entries.values_mut() {
            entry.due = true;
        }
        self.wake.notify_one();
    }
    /// Whether `id` is being carried on, and if so why, with its timed tries.
    #[cfg(test)]
    pub(super) fn waiting_for(&self, id: &ConversationId) -> Option<(Waiting, u32)> {
        self.table()
            .entries
            .get(id)
            .map(|entry| (entry.waiting, entry.release_tries))
    }
    /// Claim the right to run the worker: `true` for exactly one caller until
    /// [`Self::stopped`].
    pub(super) fn start(&self) -> bool {
        !self.running.swap(true, Ordering::SeqCst)
    }
    /// The worker ended; the next [`Self::start`] may run another.
    pub(super) fn stopped(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
    /// Whether a worker is running.
    #[cfg(test)]
    pub(super) fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    fn table(&self) -> MutexGuard<'_, Table> {
        // Nothing is awaited while this is held.
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
