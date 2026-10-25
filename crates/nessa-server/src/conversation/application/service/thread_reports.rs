//! Report a thread's finished turn back to the conversation whose agent sent it.
//!
//! `send_message` from an agent turn registers a report before it submits:
//! saved in the [`ThreadReportStore`] first, so a gateway restart keeps it,
//! then held here. The child's agent runs [`ThreadReportHook`] after every
//! invocation; it only asks the service to look. A withdrawn or closed turn,
//! which runs no invocation, asks for a look too. The look reads the child's
//! saved invocations: a watched execution whose settlement is saved, or that
//! was injected into a turn (its scheduling `target`) whose settlement is
//! saved, is finished, and its report is queued into the parent as an owner
//! turn under the caller the parent's own send was admitted with. Each look
//! checks every waiting caller may still view the thread; one refused, or a
//! thread gone, drops that caller's reports. A report leaves the store once
//! its parent has it, so a restart part way through delivers it again, and
//! its fixed execution identity answers that from the first receipt.
//!
//! ```text
//! send_message ─▶ save + register(child, execution, parent) ─▶ submit to child
//!                                                           └─▶ look(child)
//! child invocation ends / turn withdrawn / thread closed ─▶ look(child)
//! gateway starts ─▶ resume: saved reports ─▶ look(child) each
//! look(child) ─▶ child's history ─▶ finished watches ─▶ submit to parent ─▶ settle
//! ```
//!
//! After a restart no turn left unfinished by the earlier run is run again,
//! so a report saved by that run whose turn has not finished says it was
//! interrupted: [`restored_unfinished`] owns that one decision.
use super::{
    ConversationCaller, ConversationError, ConversationService, Inner, SubmissionMode,
    SubmittedMessage, Writer,
};
use crate::conversation::application::ConversationFuture;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::{
    executions::ExecutionUpdate,
    hooks::{HookError, InvocationContext, InvocationHook},
    sessions::InvocationRecord,
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionOutcome, Initiation, InvocationStage, MessageKind,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, OnceLock, PoisonError, Weak,
    },
    time::Duration,
};

/// A turn's hook runs before its settlement is saved, and a failed turn, or a
/// failed admission, makes the agent cancel the turns queued behind it after
/// that, outside any invocation: no hook follows either. Look again this many
/// times, this far apart, while a watched turn is settling.
const SETTLING_LOOKS: usize = 50;
const SETTLING_LOOK_DELAY: Duration = Duration::from_millis(100);

/// A report its parent cannot take yet (full of open conversations, or its
/// storage not answering) waits this long, doubling up to the most, and tries
/// again until the gateway retires; then it stays saved for the next start.
const DELIVERY_RETRY_FIRST: Duration = Duration::from_millis(100);
const DELIVERY_RETRY_MOST: Duration = Duration::from_secs(60);
/// Threads looked at together when the gateway starts.
const RESUME_LOOKS: usize = 16;

/// Where one finished turn is reported, fixed when its send was admitted.
#[derive(Clone)]
pub struct ThreadReport {
    /// The conversation whose agent sent the message.
    pub parent: ConversationId,
    /// The sender's admitted caller, with the report's own action identity.
    pub caller: ConversationCaller,
    /// The report turn's execution identity in the parent.
    pub execution: String,
}

/// One waiting report as the store keeps it: the turn it waits on, and where
/// it goes.
#[derive(Clone)]
pub struct PendingReport {
    pub child: ConversationId,
    /// The watched execution in `child`.
    pub execution: String,
    pub report: ThreadReport,
}

/// The durable copy of every waiting report. A report is saved before its
/// send is submitted and settled once its parent has it, or once it never
/// will. Keyed by the report's execution identity.
pub trait ThreadReportStore: Send + Sync {
    /// Save one waiting report. An exact repeat leaves the row as it is.
    /// A different binding under its identity returns `RequestConflict`.
    /// Answers whether this save wrote it.
    fn save(&self, report: &PendingReport) -> ConversationFuture<'_, bool>;
    /// Let go of one report belonging to `child`; an absent row is no error.
    /// Another child's binding returns `RequestConflict` and is retained.
    fn settle(&self, child: &ConversationId, report: &str) -> ConversationFuture<'_, ()>;
    /// Let go of every report waiting on, or going to, a deleted conversation.
    fn forget_conversation(&self, id: &ConversationId) -> ConversationFuture<'_, ()>;
    /// Every saved report.
    fn pending(&self) -> ConversationFuture<'_, Vec<PendingReport>>;
}

struct Watch {
    execution: String,
    report: ThreadReport,
    /// Saved by an earlier run of the gateway and loaded at start.
    restored: bool,
    /// Sender calls waiting on this turn in their own turn: while one waits,
    /// the reply is theirs to take and the report holds back.
    awaited: usize,
}

/// Who a caller is to access: the rest of a caller is attribution.
fn person(caller: &ConversationCaller) -> (&OrganizationId, &PrincipalId) {
    (&caller.organization_id, &caller.principal_id)
}

/// What registering a report did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportRegistration {
    /// Newly waiting: the caller settles it if its send is refused.
    Registered,
    /// An earlier send under the same request already waits; leave it be.
    AlreadyWaiting,
    /// Not saved, or a thread that is its own parent: no report will come.
    Refused,
}
impl ReportRegistration {
    /// Whether a report will come.
    pub fn reports(self) -> bool {
        self != Self::Refused
    }
}

/// Waiting reports by the thread they wait on, and reports being queued.
#[derive(Default)]
pub(crate) struct ThreadReports {
    watches: Mutex<HashMap<ConversationId, Vec<Watch>>>,
    delivering: Arc<AtomicUsize>,
    /// Threads being looked at, and whether another look (after a failure,
    /// when `true`) was asked for meanwhile.
    looking: Mutex<HashMap<ConversationId, Option<bool>>>,
    /// Reports this run saved and has not settled: the start's resume reads
    /// the store while sends save, and leaves these to the run that saved them.
    fresh: Mutex<HashSet<String>>,
    /// Held while a report registers, so a second registration of the same
    /// report sees the first one's watch, never its half-made state.
    registering: tokio::sync::Mutex<()>,
    store: OnceLock<Arc<dyn ThreadReportStore>>,
    /// Reports being queued, by report: so a sender that read the reply in
    /// its own turn meanwhile can still call one off before it is submitted.
    queuing: Arc<Mutex<HashMap<String, Queuing>>>,
}

/// A report taken off its watch and not yet queued into its parent.
struct Queuing {
    child: ConversationId,
    watched: String,
    parent: ConversationId,
    /// The sender read the reply itself: the report is not sent.
    claimed: bool,
}

/// What a look found of one watched turn.
enum Watched {
    Finished(String),
    /// Its result is saved and its settlement is not, or it is queued behind
    /// a failure that will cancel it.
    Settling,
    Waiting,
}

/// Counts one report being queued, and holds its place in `queuing`, until
/// dropped.
struct Delivering {
    count: Arc<AtomicUsize>,
    queuing: Arc<Mutex<HashMap<String, Queuing>>>,
    report: String,
}
impl Delivering {
    fn claimed(&self) -> bool {
        self.queuing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&self.report)
            .is_some_and(|queuing| queuing.claimed)
    }
}
impl Drop for Delivering {
    fn drop(&mut self) {
        self.queuing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.report);
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A report taken off its watch, counted as being queued from the moment it
/// left the registry. Its saved copy stays until it is delivered.
struct Taken {
    child: ConversationId,
    report: ThreadReport,
    text: String,
    delivering: Delivering,
}

impl ThreadReports {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, Vec<Watch>>> {
        self.watches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn store(&self) -> Option<&Arc<dyn ThreadReportStore>> {
        self.store.get()
    }

    fn watched(&self, child: &ConversationId) -> bool {
        self.lock().contains_key(child)
    }

    fn holds(&self, child: &ConversationId, report: &str) -> bool {
        self.lock()
            .get(child)
            .is_some_and(|list| list.iter().any(|watch| watch.report.execution == report))
    }

    /// Hold a watch; `false` when one under the same report already waits.
    fn insert(
        &self,
        child: ConversationId,
        execution: String,
        report: ThreadReport,
        restored: bool,
    ) -> bool {
        let mut watches = self.lock();
        let list = watches.entry(child).or_default();
        if list
            .iter()
            .any(|watch| watch.report.execution == report.execution)
        {
            return false;
        }
        list.push(Watch {
            execution,
            report,
            restored,
            awaited: 0,
        });
        true
    }

    fn forget(&self, child: &ConversationId, report: &str) {
        let mut watches = self.lock();
        if let Some(list) = watches.get_mut(child) {
            list.retain(|watch| watch.report.execution != report);
            if list.is_empty() {
                watches.remove(child);
            }
        }
    }

    /// Whether a report on `execution` in `child` was saved by an earlier run.
    fn restored(&self, child: &ConversationId, execution: &str) -> bool {
        self.lock().get(child).is_some_and(|list| {
            list.iter()
                .any(|watch| watch.execution == execution && watch.restored)
        })
    }

    /// Count one more, or one fewer (`by` -1), sender call waiting on
    /// `execution` in `child` for `parent`.
    fn await_turn(
        &self,
        child: &ConversationId,
        execution: &str,
        parent: &ConversationId,
        by: isize,
    ) {
        let mut watches = self.lock();
        for watch in watches.get_mut(child).into_iter().flatten() {
            if watch.execution == execution && &watch.report.parent == parent {
                watch.awaited = watch.awaited.saturating_add_signed(by);
            }
        }
    }

    /// Let go of the watches on `execution` in `child` reporting to `parent`:
    /// the reports let go. One already being queued is called off, unless its
    /// parent has it already.
    fn claim(
        &self,
        child: &ConversationId,
        execution: &str,
        parent: &ConversationId,
    ) -> Vec<String> {
        let mut watches = self.lock();
        let mut claimed = Vec::new();
        if let Some(list) = watches.get_mut(child) {
            list.retain(|watch| {
                let keep = watch.execution != execution || &watch.report.parent != parent;
                if !keep {
                    claimed.push(watch.report.execution.clone());
                }
                keep
            });
            if list.is_empty() {
                watches.remove(child);
            }
        }
        drop(watches);
        for queuing in self
            .queuing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values_mut()
        {
            if &queuing.child == child && queuing.watched == execution && &queuing.parent == parent
            {
                queuing.claimed = true;
            }
        }
        claimed
    }

    /// Let go of every watch on, or reporting to, `id`, and of the fresh
    /// marks of those reports.
    fn forget_conversation(&self, id: &ConversationId) {
        let mut gone: Vec<String> = Vec::new();
        {
            let mut watches = self.lock();
            if let Some(list) = watches.remove(id) {
                gone.extend(list.into_iter().map(|watch| watch.report.execution));
            }
            watches.retain(|_, list| {
                list.retain(|watch| {
                    let keep = &watch.report.parent != id;
                    if !keep {
                        gone.push(watch.report.execution.clone());
                    }
                    keep
                });
                !list.is_empty()
            });
        }
        let mut fresh = self.fresh();
        for report in gone {
            fresh.remove(&report);
        }
    }

    /// One caller for each person with a report waiting on `child`.
    fn callers(&self, child: &ConversationId) -> Vec<ConversationCaller> {
        let watches = self.lock();
        let mut callers: Vec<ConversationCaller> = Vec::new();
        for watch in watches.get(child).into_iter().flatten() {
            let caller = &watch.report.caller;
            if !callers.iter().any(|seen| person(seen) == person(caller)) {
                callers.push(caller.clone());
            }
        }
        callers
    }

    /// Let go of every report on `child` waiting for `caller`'s person: the
    /// reports let go.
    fn drop_person(&self, child: &ConversationId, caller: &ConversationCaller) -> Vec<String> {
        let mut watches = self.lock();
        let mut dropped = Vec::new();
        if let Some(list) = watches.get_mut(child) {
            list.retain(|watch| {
                let keep = person(&watch.report.caller) != person(caller);
                if !keep {
                    dropped.push(watch.report.execution.clone());
                }
                keep
            });
            if list.is_empty() {
                watches.remove(child);
            }
        }
        dropped
    }

    /// Remove and return every watch of an `admitted` person that `finished`
    /// decides, with its report text.
    fn take(
        &self,
        child: &ConversationId,
        admitted: &[ConversationCaller],
        finished: impl Fn(&str, bool) -> Option<String>,
    ) -> Vec<Taken> {
        let mut watches = self.lock();
        let Some(list) = watches.get_mut(child) else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        list.retain(|watch| {
            let caller = &watch.report.caller;
            if !admitted.iter().any(|seen| person(seen) == person(caller)) {
                return true;
            }
            if watch.awaited > 0 {
                return true;
            }
            let Some(text) = finished(&watch.execution, watch.restored) else {
                return true;
            };
            self.delivering.fetch_add(1, Ordering::SeqCst);
            self.queuing
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    watch.report.execution.clone(),
                    Queuing {
                        child: child.clone(),
                        watched: watch.execution.clone(),
                        parent: watch.report.parent.clone(),
                        claimed: false,
                    },
                );
            taken.push(Taken {
                child: child.clone(),
                report: watch.report.clone(),
                text,
                delivering: Delivering {
                    count: self.delivering.clone(),
                    queuing: self.queuing.clone(),
                    report: watch.report.execution.clone(),
                },
            });
            false
        });
        if list.is_empty() {
            watches.remove(child);
        }
        taken
    }

    /// Take the look at `child`, or leave another for the look under way.
    fn begin_look(&self, child: &ConversationId, failure: bool) -> bool {
        let mut looking = self.looking.lock().unwrap_or_else(PoisonError::into_inner);
        match looking.get_mut(child) {
            Some(again) => {
                *again = Some(again.unwrap_or(false) || failure);
                false
            }
            None => {
                looking.insert(child.clone(), None);
                true
            }
        }
    }

    /// End a look at `child`: the look asked for meanwhile, if any.
    fn end_look(&self, child: &ConversationId) -> Option<bool> {
        let mut looking = self.looking.lock().unwrap_or_else(PoisonError::into_inner);
        match looking.get_mut(child) {
            Some(again @ Some(_)) => again.take(),
            _ => {
                looking.remove(child);
                None
            }
        }
    }

    /// The watches still on `child` for an `admitted` person.
    fn waiting_on(
        &self,
        child: &ConversationId,
        admitted: &[ConversationCaller],
    ) -> Vec<(String, bool)> {
        let watches = self.lock();
        watches
            .get(child)
            .into_iter()
            .flatten()
            .filter(|watch| {
                let caller = &watch.report.caller;
                admitted.iter().any(|seen| person(seen) == person(caller))
            })
            .map(|watch| (watch.execution.clone(), watch.restored))
            .collect()
    }

    fn fresh(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.fresh.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Reports registered or being queued.
    #[cfg(all(test, unix))]
    pub(crate) fn waiting(&self) -> usize {
        self.lock().values().map(Vec::len).sum::<usize>() + self.delivering.load(Ordering::SeqCst)
    }
}

/// How a sent turn stands, as its report would tell it.
#[derive(Debug, PartialEq, Eq)]
pub enum ThreadTurn {
    /// Settled for good: its outcome and final reply.
    Ended(&'static str, Option<String>),
    /// Running, waiting, or settling: a saved result can still turn into a
    /// failure while it settles, and the report waits for that too.
    Unfinished,
    /// Its history cannot be read now; ask again.
    Unreadable,
}

/// Holds a turn's report back while a sender call waits on it.
pub struct AwaitedTurn {
    service: ConversationService,
    child: ConversationId,
    execution: String,
    parent: ConversationId,
}
impl Drop for AwaitedTurn {
    fn drop(&mut self) {
        self.service.inner.thread_reports.await_turn(
            &self.child,
            &self.execution,
            &self.parent,
            -1,
        );
        if tokio::runtime::Handle::try_current().is_ok() {
            self.service.wake_thread_reports(&self.child);
        }
    }
}

/// Asks the service to look at a watched thread after each of its invocations.
/// Holds the service weakly: the service owns the agent this hook is on.
pub(crate) struct ThreadReportHook {
    pub(crate) service: Weak<Inner>,
    pub(crate) child: ConversationId,
}

impl InvocationHook for ThreadReportHook {
    fn reviews_permissions(&self) -> bool {
        false
    }

    fn after_invocation(
        &self,
        _: &InvocationContext<'_>,
        result: &Result<
            ExecutionOutcome,
            nessa_sdk::application::agent_execution::agents::AgentError,
        >,
    ) -> Result<(), HookError> {
        let Some(inner) = self.service.upgrade() else {
            return Ok(());
        };
        if !inner.thread_reports.watched(&self.child) {
            return Ok(());
        }
        // Return promptly and never re-enter the session from the hook.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let service = ConversationService { inner };
            let child = self.child.clone();
            let failed = result.is_err();
            runtime.spawn(async move { service.look_for_reports(child, failed).await });
        }
        Ok(())
    }
}

impl ConversationService {
    /// Keep waiting reports in `store`, so a restart keeps them. Without one
    /// they last as long as this process.
    pub fn bind_thread_reports(&self, store: Arc<dyn ThreadReportStore>) {
        let _ = self.inner.thread_reports.store.set(store);
    }

    /// Queue `report` into its parent once `execution` in `child` finishes.
    /// A report that cannot be saved, or a thread that is its own parent,
    /// registers nothing.
    pub async fn report_thread_turn(
        &self,
        child: ConversationId,
        execution: String,
        report: ThreadReport,
    ) -> ReportRegistration {
        if child == report.parent {
            return ReportRegistration::Refused;
        }
        let reports = &self.inner.thread_reports;
        let _registering = reports.registering.lock().await;
        if reports.holds(&child, &report.execution) {
            return ReportRegistration::AlreadyWaiting;
        }
        let newly_fresh = reports.fresh().insert(report.execution.clone());
        // A row already saved, and not watched here, is an earlier run's: a
        // retry of its send, before the start read it, keeps it restored.
        let mut restored = false;
        if let Some(store) = reports.store() {
            let pending = PendingReport {
                child: child.clone(),
                execution: execution.clone(),
                report: report.clone(),
            };
            match store.save(&pending).await {
                Ok(true) => {}
                Ok(false) => {
                    if newly_fresh {
                        reports.fresh().remove(&report.execution);
                    }
                    restored = newly_fresh;
                }
                Err(error) => {
                    tracing::warn!(%child, %error, "a thread report could not be saved; none will come");
                    if newly_fresh {
                        reports.fresh().remove(&report.execution);
                    }
                    return ReportRegistration::Refused;
                }
            }
        }
        if reports.insert(child, execution, report, restored) {
            ReportRegistration::Registered
        } else {
            ReportRegistration::AlreadyWaiting
        }
    }

    /// How `execution` in `child` stands, as its report would tell it. A
    /// steering message is answered by the turn it was delivered into.
    pub async fn thread_turn_end(
        &self,
        child: &ConversationId,
        caller: &ConversationCaller,
        execution: &str,
    ) -> ThreadTurn {
        let Some(history) = self.report_history(child, caller).await else {
            return ThreadTurn::Unreadable;
        };
        let found = match invocation(&history, execution) {
            Some(record) => in_record(&history, record, false),
            None => Found::Waiting(None),
        };
        // An earlier run left it unfinished: its report says how it ended,
        // and so does this.
        let restored = self.inner.thread_reports.restored(child, execution);
        match found {
            Found::Finished(outcome, reply) => ThreadTurn::Ended(outcome, reply),
            Found::Settling(saved) | Found::Waiting(saved) if restored => {
                let (outcome, reply) = saved.unwrap_or((INTERRUPTED, None));
                ThreadTurn::Ended(outcome, reply)
            }
            Found::Settling(_) | Found::Waiting(_) => ThreadTurn::Unfinished,
        }
    }

    /// A sender call waits on `execution` in `child` in its own turn: until
    /// the returned guard is dropped, the turn's report holds back, so the
    /// reply goes to the call. Dropping it looks again, so a turn that ended
    /// without the call taking its reply is reported as usual.
    pub fn await_thread_turn(
        &self,
        child: &ConversationId,
        execution: &str,
        parent: &ConversationId,
    ) -> AwaitedTurn {
        self.inner
            .thread_reports
            .await_turn(child, execution, parent, 1);
        AwaitedTurn {
            service: self.clone(),
            child: child.clone(),
            execution: execution.to_owned(),
            parent: parent.clone(),
        }
    }

    /// The sender already has `execution`'s reply, read in its own turn:
    /// its report would only say it again, so it is let go.
    pub async fn claim_thread_report(
        &self,
        child: &ConversationId,
        execution: &str,
        parent: &ConversationId,
    ) {
        for report in self.inner.thread_reports.claim(child, execution, parent) {
            self.settle_saved(child, &report).await;
        }
    }

    /// Drop a report whose turn will never run.
    pub async fn forget_thread_report(&self, child: &ConversationId, report: &str) {
        self.inner.thread_reports.forget(child, report);
        self.settle_saved(child, report).await;
    }

    /// Let go of the reports on, or to, a deleted conversation.
    pub(crate) async fn forget_thread_reports_of(&self, id: &ConversationId) {
        self.inner.thread_reports.forget_conversation(id);
        if let Some(store) = self.inner.thread_reports.store() {
            if let Err(error) = store.forget_conversation(id).await {
                tracing::warn!(conversation_id = %id, %error, "saved thread reports of a deleted conversation were not let go");
            }
        }
    }

    async fn settle_saved(&self, child: &ConversationId, report: &str) {
        if let Some(store) = self.inner.thread_reports.store() {
            if let Err(error) = store.settle(child, report).await {
                tracing::warn!(report, %error, "a settled thread report stays saved; the next start settles it");
                return;
            }
        }
        self.inner.thread_reports.fresh().remove(report);
    }

    /// Load the reports an earlier run saved and look at each thread they
    /// wait on. Run once at start.
    ///
    /// # Errors
    /// The store's error when its reports cannot be read at all.
    pub async fn resume_thread_reports(&self) -> Result<usize, ConversationError> {
        let Some(store) = self.inner.thread_reports.store() else {
            return Ok(0);
        };
        let pending = store.pending().await?;
        let loaded = pending.len();
        let mut children: HashSet<ConversationId> = HashSet::new();
        for saved in pending {
            // Saved by this run, not an earlier one: it is that send's.
            if self
                .inner
                .thread_reports
                .fresh()
                .contains(&saved.report.execution)
            {
                continue;
            }
            children.insert(saved.child.clone());
            self.inner
                .thread_reports
                .insert(saved.child, saved.execution, saved.report, true);
        }
        let looks = Arc::new(tokio::sync::Semaphore::new(RESUME_LOOKS));
        let mut running = tokio::task::JoinSet::new();
        for child in children {
            if self.inner.retirement.get().is_some() {
                break;
            }
            let Ok(permit) = looks.clone().acquire_owned().await else {
                break;
            };
            let service = self.clone();
            running.spawn(async move {
                service.look_for_reports(child, false).await;
                drop(permit);
            });
        }
        while running.join_next().await.is_some() {}
        Ok(loaded)
    }

    /// Settle the report of a send that failed: the thread saved no turn
    /// under `execution`, so none will finish and the report goes; or it did,
    /// and the report waits for that turn like any other.
    pub async fn settle_failed_thread_report(
        &self,
        child: ConversationId,
        execution: &str,
        report: &str,
        caller: &ConversationCaller,
    ) {
        let saved = match self.check_view_access(&child, caller).await {
            Ok(()) => match self.report_history(&child, caller).await {
                Some(history) => invocation(&history, execution).is_some(),
                None => return,
            },
            Err(ConversationError::NotFound | ConversationError::Deleted) => false,
            // Not known now: the next look, or the next start, settles it.
            Err(_) => return,
        };
        if !saved {
            self.forget_thread_report(&child, report).await;
            return;
        }
        // One look now; the follow-up a failure may need runs off the send.
        if self.look_once(&child, true).await {
            let service = self.clone();
            tokio::spawn(async move { service.look_for_reports(child, true).await });
        }
    }

    /// Queue every report on `child` whose turn has finished, for each
    /// person who may still view it, looking again while a watched turn
    /// settles. After a `failure` the turns queued behind it are about to be
    /// cancelled: those count as settling too. One look runs per thread at a
    /// time; one asked for meanwhile runs after it.
    pub async fn look_for_reports(&self, child: ConversationId, failure: bool) {
        if !self.inner.thread_reports.begin_look(&child, failure) {
            return;
        }
        let mut failure = failure;
        loop {
            for look in 0..=SETTLING_LOOKS {
                if look > 0 {
                    tokio::time::sleep(SETTLING_LOOK_DELAY).await;
                }
                if !self.look_once(&child, failure).await {
                    break;
                }
            }
            match self.inner.thread_reports.end_look(&child) {
                Some(again) => failure = again,
                None => return,
            }
        }
    }

    /// Look at a thread whose watched turn ended outside any invocation:
    /// withdrawn from its queue, or cancelled by a close.
    pub(crate) fn wake_thread_reports(&self, child: &ConversationId) {
        if !self.inner.thread_reports.watched(child) {
            return;
        }
        let service = self.clone();
        let child = child.clone();
        tokio::spawn(async move { service.look_for_reports(child, false).await });
    }

    /// One look; whether a watched turn is settling.
    async fn look_once(&self, child: &ConversationId, failure: bool) -> bool {
        let mut admitted: Vec<ConversationCaller> = Vec::new();
        for caller in self.inner.thread_reports.callers(child) {
            match self.check_view_access(child, &caller).await {
                Ok(()) => admitted.push(caller),
                // Refused, or gone for good: this person is told nothing.
                Err(ConversationError::NotFound | ConversationError::Deleted) => {
                    for report in self.inner.thread_reports.drop_person(child, &caller) {
                        self.settle_saved(child, &report).await;
                    }
                }
                // Not decidable now: a later look, or the next start, settles it.
                Err(_) => {}
            }
        }
        let Some(first) = admitted.first() else {
            return false;
        };
        let Some(history) = self.report_history(child, first).await else {
            return false;
        };
        let limit = self.inner.limits.max_input_bytes;
        let taken = self
            .inner
            .thread_reports
            .take(child, &admitted, |execution, restored| {
                match watched(child, &history, execution, failure, restored, limit) {
                    Watched::Finished(text) => Some(text),
                    Watched::Settling | Watched::Waiting => None,
                }
            });
        // Each delivery runs on its own, so a parent that cannot take its
        // report yet holds up no other.
        for taken in taken {
            let service = self.clone();
            tokio::spawn(async move { service.deliver_report(taken).await });
        }
        self.inner
            .thread_reports
            .waiting_on(child, &admitted)
            .iter()
            .any(|(execution, restored)| {
                matches!(
                    watched(child, &history, execution, failure, *restored, limit),
                    Watched::Settling
                )
            })
    }

    /// The child's saved invocations: from its agent when it is open, else
    /// from its committed history. `None` when neither can be read now.
    async fn report_history(
        &self,
        child: &ConversationId,
        caller: &ConversationCaller,
    ) -> Option<Vec<InvocationRecord>> {
        if let Ok(Some(live)) = self.live_now(child, caller).await {
            return live
                .agent
                .session_manager()
                .snapshot()
                .await
                .map(|snapshot| snapshot.invocations);
        }
        match self.committed_with_order(child).await {
            Ok((committed, _)) => Some(
                committed
                    .snapshot()
                    .map(|snapshot| snapshot.invocations.clone())
                    .unwrap_or_default(),
            ),
            // A thread with no history yet ran nothing.
            Err(ConversationError::NotFound) => Some(Vec::new()),
            Err(error) => {
                tracing::warn!(%child, %error, "a watched thread's history could not be read");
                None
            }
        }
    }

    /// Queue one report into its parent, then settle its saved copy. The
    /// parent's own history is what says it has the report: one it already
    /// holds is not queued again, and the report's fixed execution identity
    /// makes a second submit answer from the first. A parent that cannot take
    /// it yet is tried again until the gateway retires, which leaves it saved
    /// for the next start. A parent whose agent cannot open in this run, from
    /// its configuration or placement, is not tried again in it: the report
    /// stays saved for the next start. One the parent refuses for good is let
    /// go.
    async fn deliver_report(&self, taken: Taken) {
        let Taken { report, text, .. } = &taken;
        let mut delay = DELIVERY_RETRY_FIRST;
        loop {
            if self.inner.retirement.get().is_some() {
                return;
            }
            let delivered = self
                .report_history(&report.parent, &report.caller)
                .await
                .map(|history| invocation(&history, &report.execution).is_some());
            if delivered == Some(true) {
                break;
            }
            // The sender read the reply in its own turn meanwhile.
            if taken.delivering.claimed() {
                break;
            }
            if delivered == Some(false) {
                let answer = self
                    .submit_as(
                        report.parent.clone(),
                        report.caller.clone(),
                        report.execution.clone(),
                        SubmittedMessage {
                            text: text.clone(),
                            ..Default::default()
                        },
                        SubmissionMode::Queue,
                        Writer::Person(Initiation::owner(), false),
                    )
                    .await;
                match answer.map_err(super::SubmitFailure::into_error) {
                    Ok(_) => break,
                    Err(_) if self.inner.retirement.get().is_some() => return,
                    Err(
                        error @ (ConversationError::NotFound
                        | ConversationError::Deleted
                        | ConversationError::InvalidInput
                        | ConversationError::RequestConflict
                        | ConversationError::AgentUnsupported),
                    ) => {
                        let parent = &report.parent;
                        tracing::warn!(%parent, %error, "a thread report was refused by its parent; it is let go");
                        break;
                    }
                    Err(
                        error @ (ConversationError::LeaseRefused(_)
                        | ConversationError::LeaseUnreadable
                        | ConversationError::PlacementUnreadable
                        | ConversationError::AgentNotConfigured
                        | ConversationError::EnvironmentNotConfigured
                        | ConversationError::ModelUnavailable
                        | ConversationError::ApprovalModeUnavailable),
                    ) => {
                        let parent = &report.parent;
                        tracing::warn!(%parent, %error, "a thread report's parent cannot open in this run; it stays saved for the next start");
                        return;
                    }
                    Err(error) => {
                        let parent = &report.parent;
                        tracing::warn!(%parent, %error, "a thread report waits for its parent");
                    }
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(DELIVERY_RETRY_MOST);
        }
        self.settle_saved(&taken.child, &report.execution).await;
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/thread_report_registry.rs"]
mod tests;

/// What `history` says of the watched `execution`: finished once its
/// settlement is saved, with the report text. After a `failure`, a turn still
/// queued is about to be cancelled. A watch `restored` from an earlier run
/// whose turn has not finished was interrupted.
fn watched(
    child: &ConversationId,
    history: &[InvocationRecord],
    execution: &str,
    failure: bool,
    restored: bool,
    limit: usize,
) -> Watched {
    let found = match invocation(history, execution) {
        Some(record) => in_record(history, record, failure),
        None => Found::Waiting(None),
    };
    match found {
        Found::Finished(outcome, reply) => {
            Watched::Finished(report_text(child, execution, outcome, reply, limit))
        }
        Found::Settling(saved) | Found::Waiting(saved) if restored => {
            restored_unfinished(child, execution, saved, limit)
        }
        Found::Settling(_) => Watched::Settling,
        Found::Waiting(_) => Watched::Waiting,
    }
}

/// A report an earlier run saved, whose turn that run left unfinished. No
/// turn is run again after a restart, so one that saved no result is reported
/// interrupted. One that saved its result, but not its settlement, did end:
/// it is reported with that result and reply.
fn restored_unfinished(
    child: &ConversationId,
    execution: &str,
    saved: Option<Ended>,
    limit: usize,
) -> Watched {
    let (outcome, reply) = saved.unwrap_or((INTERRUPTED, None));
    Watched::Finished(report_text(child, execution, outcome, reply, limit))
}

/// An outcome, and the reply that came with it.
type Ended = (&'static str, Option<String>);

enum Found {
    Finished(&'static str, Option<String>),
    /// Settling, with the result already saved, if one is.
    Settling(Option<Ended>),
    Waiting(Option<Ended>),
}

fn in_record(history: &[InvocationRecord], record: &InvocationRecord, failure: bool) -> Found {
    let last = record.scheduling.last();
    if settled(record) {
        return match &record.result {
            Some(result) => Found::Finished(outcome(result), reply(record)),
            None => Found::Finished("cancelled", None),
        };
    }
    match last.map(|event| event.stage) {
        // Steering delivered into a turn: that turn's outcome and reply are
        // this message's, once it has settled.
        Some(InvocationStage::Injected) => {
            let host = last
                .and_then(|event| event.target.as_ref())
                .and_then(|target| invocation(history, target.as_str()));
            let Some(host) = host else {
                return Found::Waiting(None);
            };
            match (settled(host), &host.result) {
                (true, Some(result)) => Found::Finished(outcome(result), reply(host)),
                (true, None) => Found::Finished("cancelled", None),
                (false, Some(result)) => Found::Settling(Some((outcome(result), reply(host)))),
                (false, None) => Found::Waiting(None),
            }
        }
        _ => match &record.result {
            Some(result) => Found::Settling(Some((outcome(result), reply(record)))),
            None if failure && last.map(|event| event.stage) == Some(InvocationStage::Queued) => {
                Found::Settling(None)
            }
            None => Found::Waiting(None),
        },
    }
}

/// Whether the record's settlement is saved: it ran to an end, or was cancelled.
fn settled(record: &InvocationRecord) -> bool {
    record.scheduling.last().is_some_and(|event| {
        matches!(
            event.stage,
            InvocationStage::Settled | InvocationStage::Cancelled
        )
    })
}

fn invocation<'h>(
    history: &'h [InvocationRecord],
    execution: &str,
) -> Option<&'h InvocationRecord> {
    history
        .iter()
        .find(|record| record.request.execution_id.as_str() == execution)
}

fn outcome(result: &Result<ExecutionOutcome, impl std::fmt::Debug>) -> &'static str {
    match result {
        Ok(ExecutionOutcome::Completed) => "completed",
        Ok(ExecutionOutcome::OutputLimit) => "output_limit",
        Ok(ExecutionOutcome::RequestLimit) => "request_limit",
        Ok(ExecutionOutcome::Refused) => "refused",
        Ok(ExecutionOutcome::Cancelled) => "cancelled",
        Err(_) => "failed",
    }
}

/// The visible text after the turn's last tool step: its final answer.
fn reply(record: &InvocationRecord) -> Option<String> {
    let mut text = String::new();
    for event in &record.events {
        match event.update() {
            ExecutionUpdate::Message(chunk) if chunk.kind() == MessageKind::Text => {
                text.push_str(chunk.as_str());
            }
            ExecutionUpdate::Tool(_) => text.clear(),
            _ => {}
        }
    }
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
pub(crate) fn report_text_for_test(
    child: &ConversationId,
    execution: &str,
    outcome: &str,
    reply: Option<String>,
    limit: usize,
) -> String {
    report_text(child, execution, outcome, reply, limit)
}

/// Every outcome a report can name: each a turn can settle with, and the
/// one a restart leaves (`interrupted`). The desktop reads each of them
/// (`tests/fixtures/thread-report.json`).
#[cfg(test)]
pub(crate) fn report_outcomes_for_test() -> Vec<&'static str> {
    let every = [
        ExecutionOutcome::Completed,
        ExecutionOutcome::OutputLimit,
        ExecutionOutcome::RequestLimit,
        ExecutionOutcome::Refused,
        ExecutionOutcome::Cancelled,
    ];
    // Exhaustive, with no wildcard: an outcome added to the enum does not
    // compile here until it is listed above, and so reaches the shared cases.
    for each in &every {
        match each {
            ExecutionOutcome::Completed
            | ExecutionOutcome::OutputLimit
            | ExecutionOutcome::RequestLimit
            | ExecutionOutcome::Refused
            | ExecutionOutcome::Cancelled => {}
        }
    }
    let mut names: Vec<_> = every
        .into_iter()
        .map(|each| outcome(&Ok::<_, ()>(each)))
        .collect();
    names.push(outcome(&Err::<ExecutionOutcome, ()>(())));
    names.push(INTERRUPTED);
    names
}

/// The outcome a report names when a restart left the turn unfinished.
const INTERRUPTED: &str = "interrupted";

const CUT: &str = "\n[cut to fit]";

/// The report turn's text, whole within `limit` bytes, the parent's input
/// limit: what does not fit is cut, with a marker when the marker fits.
fn report_text(
    child: &ConversationId,
    execution: &str,
    outcome: &str,
    reply: Option<String>,
    limit: usize,
) -> String {
    let mut text = format!(
        "[Thread report] Thread {child} finished the turn you sent it (execution {execution}): {outcome}.\n\n"
    );
    match reply {
        None if outcome == INTERRUPTED => {
            text.push_str("The gateway restarted before the turn finished.");
        }
        None => text.push_str("It gave no reply text."),
        Some(reply) => {
            text.push_str("Its reply:\n");
            text.push_str(&reply);
        }
    }
    if text.len() <= limit {
        return text;
    }
    let marker = if CUT.len() <= limit { CUT } else { "" };
    let mut end = limit - marker.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(marker);
    text
}
