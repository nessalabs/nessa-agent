//! Report a thread's finished turn back to the conversation whose agent sent it.
//!
//! `send_message` from an agent turn registers a report before it submits. The
//! child's agent runs [`ThreadReportHook`] after every invocation, once its
//! result is saved; it only asks the service to look. The look reads the
//! child's saved invocations: a watched execution that settled, was cancelled,
//! or was injected into a turn (its scheduling `target`) that has since
//! settled is finished, and its report is queued into the parent as an owner
//! turn under the caller the parent's own send was admitted with. Each look
//! checks every waiting caller may still view the thread; one refused, or a
//! thread gone, drops that caller's watches. The
//! report's execution identity is fixed at registration, so a second look or a
//! retried send answers from the same receipt instead of queueing again.
//!
//! ```text
//! send_message ─▶ register(child, execution, parent) ─▶ submit to child
//!                                                   └─▶ look(child)   (already finished?)
//! child invocation ends ─▶ ThreadReportHook ─▶ look(child)
//! look(child) ─▶ child's snapshot ─▶ finished watches ─▶ submit report to parent
//! ```
//!
//! Registrations are process-local and bounded by [`MAX_THREAD_REPORTS`]; a
//! full registry refuses and the send goes on without a report. A
//! gateway restart forgets them: the parent can still `subscribe`.
use super::{
    ConversationCaller, ConversationError, ConversationService, Inner, SubmissionMode,
    SubmittedMessage,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::application::agent_execution::commands::{CreationStorageError, MutationFailure};
use nessa_sdk::application::agent_execution::{
    executions::ExecutionUpdate,
    hooks::{HookError, InvocationContext, InvocationHook},
    sessions::{InvocationRecord, SessionSnapshot, StorageError},
};
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionOutcome, Initiation, InvocationStage, MessageKind,
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, PoisonError, Weak,
    },
    time::{Duration, Instant},
};

/// Reports waiting for their thread's turn to finish, across the gateway.
pub const MAX_THREAD_REPORTS: usize = 256;
/// The child's reply carried in a report; the rest is cut with a marker.
pub const MAX_REPORT_REPLY_BYTES: usize = 8 * 1024;
/// A report whose turn has not finished in this long is let go, so a thread
/// deleted or never run cannot hold a place for good.
const REPORT_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
/// A turn's hook runs before its settlement is saved, and a failed turn, or a
/// failed admission, makes the agent cancel the turns queued behind it after
/// that, outside any invocation: no hook follows either. Look again this many
/// times, this far apart, while a watched turn is settling.
const SETTLING_LOOKS: usize = 50;
const SETTLING_LOOK_DELAY: Duration = Duration::from_millis(100);

/// A report meeting another command of the same person tries again this many
/// times, this far apart.
const DELIVERY_BUSY_TRIES: usize = 50;
const DELIVERY_BUSY_DELAY: Duration = Duration::from_millis(100);

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

struct Watch {
    execution: String,
    report: ThreadReport,
    since: Instant,
}
impl Watch {
    fn expired(&self) -> bool {
        self.since.elapsed() >= REPORT_LIFETIME
    }
}

/// Who a caller is to access: the rest of a caller is attribution.
fn person(caller: &ConversationCaller) -> (&OrganizationId, &PrincipalId) {
    (&caller.organization_id, &caller.principal_id)
}

/// What registering a report did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportRegistration {
    /// Newly waiting: the caller forgets it if its send is refused.
    Registered,
    /// An earlier send under the same request already waits; leave it be.
    AlreadyWaiting,
    /// Full, or a thread that is its own parent: no report will come.
    Refused,
}
impl ReportRegistration {
    /// Whether a report will come.
    pub fn reports(self) -> bool {
        self != Self::Refused
    }
}

/// Registered reports by the thread they wait on, and reports being queued.
#[derive(Default)]
pub(crate) struct ThreadReports {
    watches: Mutex<HashMap<ConversationId, Vec<Watch>>>,
    delivering: Arc<AtomicUsize>,
    /// Threads being looked at, and whether another look (after a failure,
    /// when `true`) was asked for meanwhile.
    looking: Mutex<HashMap<ConversationId, Option<bool>>>,
}

/// What a look found of one watched turn.
enum Watched {
    Finished(String),
    /// Its result is saved and its settlement is not, or it is queued behind
    /// a failure that will cancel it.
    Settling,
    Waiting,
}

/// Counts one report being queued until dropped.
struct Delivering(Arc<AtomicUsize>);

/// A report taken off its watch, counted as being queued from the moment it
/// left the registry.
struct Taken {
    report: ThreadReport,
    text: String,
    _delivering: Delivering,
}
impl Drop for Delivering {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ThreadReports {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, Vec<Watch>>> {
        self.watches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn watched(&self, child: &ConversationId) -> bool {
        self.lock().contains_key(child)
    }

    fn register(
        &self,
        child: ConversationId,
        execution: String,
        report: ThreadReport,
    ) -> ReportRegistration {
        let mut watches = self.lock();
        let waiting = watches.get(&child).is_some_and(|list| {
            list.iter()
                .any(|watch| watch.report.execution == report.execution)
        });
        if waiting {
            return ReportRegistration::AlreadyWaiting;
        }
        watches.retain(|_, list| {
            list.retain(|watch| !watch.expired());
            !list.is_empty()
        });
        // A report holds its place until it is queued in its parent.
        let held =
            watches.values().map(Vec::len).sum::<usize>() + self.delivering.load(Ordering::SeqCst);
        if held >= MAX_THREAD_REPORTS {
            return ReportRegistration::Refused;
        }
        watches.entry(child).or_default().push(Watch {
            execution,
            report,
            since: Instant::now(),
        });
        ReportRegistration::Registered
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

    /// Let go of every report on `child` waiting for `caller`'s person.
    fn drop_person(&self, child: &ConversationId, caller: &ConversationCaller) {
        let mut watches = self.lock();
        if let Some(list) = watches.get_mut(child) {
            list.retain(|watch| person(&watch.report.caller) != person(caller));
            if list.is_empty() {
                watches.remove(child);
            }
        }
    }

    /// Remove and return every watch of an `admitted` person that `finished`
    /// decides, with its report text; let expired ones go.
    fn take(
        &self,
        child: &ConversationId,
        admitted: &[ConversationCaller],
        finished: impl Fn(&str) -> Option<String>,
    ) -> Vec<Taken> {
        let mut watches = self.lock();
        let Some(list) = watches.get_mut(child) else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        list.retain(|watch| {
            if watch.expired() {
                return false;
            }
            let caller = &watch.report.caller;
            if !admitted.iter().any(|seen| person(seen) == person(caller)) {
                return true;
            }
            let Some(text) = finished(&watch.execution) else {
                return true;
            };
            self.delivering.fetch_add(1, Ordering::SeqCst);
            taken.push(Taken {
                report: watch.report.clone(),
                text,
                _delivering: Delivering(self.delivering.clone()),
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

    /// The executions still watched on `child` for an `admitted` person.
    fn waiting_on(&self, child: &ConversationId, admitted: &[ConversationCaller]) -> Vec<String> {
        let watches = self.lock();
        watches
            .get(child)
            .into_iter()
            .flatten()
            .filter(|watch| {
                let caller = &watch.report.caller;
                admitted.iter().any(|seen| person(seen) == person(caller))
            })
            .map(|watch| watch.execution.clone())
            .collect()
    }

    /// Reports registered or being queued.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.lock().values().map(Vec::len).sum::<usize>() + self.delivering.load(Ordering::SeqCst)
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
    /// Queue `report` into its parent once `execution` in `child` finishes.
    /// A full registry, or a thread that is its own parent, registers nothing.
    pub fn report_thread_turn(
        &self,
        child: ConversationId,
        execution: String,
        report: ThreadReport,
    ) -> ReportRegistration {
        if child == report.parent {
            return ReportRegistration::Refused;
        }
        self.inner.thread_reports.register(child, execution, report)
    }

    /// Drop a registration whose send was refused.
    pub fn forget_thread_report(&self, child: &ConversationId, report: &str) {
        self.inner.thread_reports.forget(child, report);
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
        let saved = match self.live_now(&child, caller).await {
            Ok(Some(live)) => match live.agent.session_manager().snapshot().await {
                Some(snapshot) => invocation(&snapshot, execution).is_some(),
                None => return,
            },
            Err(ConversationError::NotFound | ConversationError::Deleted) => false,
            // Not known now: the report's lifetime settles it.
            Ok(None) | Err(_) => return,
        };
        if !saved {
            self.forget_thread_report(&child, report);
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

    /// Look at a thread whose watched queued turn was withdrawn, outside
    /// any invocation.
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
        let mut live = None;
        let mut admitted = Vec::new();
        for caller in self.inner.thread_reports.callers(child) {
            match self.live_now(child, &caller).await {
                Ok(Some(found)) => {
                    live.get_or_insert(found);
                    admitted.push(caller);
                }
                // Refused, or gone for good: this person is told nothing.
                Err(ConversationError::NotFound | ConversationError::Deleted) => {
                    self.inner.thread_reports.drop_person(child, &caller);
                }
                // Not open, or not readable now: a later look, or the
                // report's lifetime, settles it.
                Ok(None) | Err(_) => {}
            }
        }
        let Some(live) = live else {
            return false;
        };
        let Some(snapshot) = live.agent.session_manager().snapshot().await else {
            return false;
        };
        let taken = self
            .inner
            .thread_reports
            .take(child, &admitted, |execution| {
                match watched(child, &snapshot, execution, failure) {
                    Watched::Finished(text) => Some(text),
                    Watched::Settling | Watched::Waiting => None,
                }
            });
        for taken in taken {
            self.deliver_report(taken).await;
        }
        self.inner
            .thread_reports
            .waiting_on(child, &admitted)
            .iter()
            .any(|execution| {
                matches!(
                    watched(child, &snapshot, execution, failure),
                    Watched::Settling
                )
            })
    }

    /// Queue one report into its parent. Commands of one person are taken
    /// one at a time with no queue (`CreationStorage`), so a report that meets
    /// the person's own command tries again for a bounded while.
    async fn deliver_report(&self, taken: Taken) {
        let Taken { report, text, .. } = &taken;
        let Some(storage) = self.commands() else {
            return;
        };
        let mut tries = 0;
        loop {
            let answer = self
                .submit_command(
                    storage.clone(),
                    report.parent.clone(),
                    report.caller.clone(),
                    Initiation::owner(),
                    false,
                    report.execution.clone(),
                    SubmittedMessage {
                        text: text.clone(),
                        ..Default::default()
                    },
                    SubmissionMode::Queue,
                )
                .await;
            match answer {
                Ok(_) => return,
                Err(MutationFailure::Storage(CreationStorageError::Storage(
                    StorageError::Busy,
                ))) if tries < DELIVERY_BUSY_TRIES => {
                    tries += 1;
                    tokio::time::sleep(DELIVERY_BUSY_DELAY).await;
                }
                Err(error) => {
                    let parent = &report.parent;
                    tracing::warn!(%parent, ?error, "a thread report could not be queued");
                    return;
                }
            }
        }
    }
}

/// What `snapshot` says of the watched `execution`: finished once its
/// settlement is saved, with the report text. After a `failure`, a turn still
/// queued is about to be cancelled.
fn watched(
    child: &ConversationId,
    snapshot: &SessionSnapshot,
    execution: &str,
    failure: bool,
) -> Watched {
    let Some(record) = invocation(snapshot, execution) else {
        return Watched::Waiting;
    };
    let last = record.scheduling.last();
    let (outcome, reply) = match last.map(|event| event.stage) {
        _ if settled(record) => match &record.result {
            Some(result) => (outcome(result), reply(record)),
            None => ("cancelled", None),
        },
        // Steering delivered into a turn: that turn's outcome and reply are
        // this message's, once it has settled.
        Some(InvocationStage::Injected) => {
            let host = last
                .and_then(|event| event.target.as_ref())
                .and_then(|target| invocation(snapshot, target.as_str()));
            let Some(host) = host else {
                return Watched::Waiting;
            };
            match (settled(host), &host.result) {
                (true, Some(result)) => (outcome(result), reply(host)),
                (true, None) => ("cancelled", None),
                (false, Some(_)) => return Watched::Settling,
                (false, None) => return Watched::Waiting,
            }
        }
        _ if record.result.is_some() => return Watched::Settling,
        Some(InvocationStage::Queued) if failure => return Watched::Settling,
        _ => return Watched::Waiting,
    };
    Watched::Finished(report_text(child, execution, outcome, reply))
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

fn invocation<'s>(snapshot: &'s SessionSnapshot, execution: &str) -> Option<&'s InvocationRecord> {
    snapshot
        .invocations
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

fn report_text(
    child: &ConversationId,
    execution: &str,
    outcome: &str,
    reply: Option<String>,
) -> String {
    let mut text = format!(
        "[Thread report] Thread {child} finished the turn you sent it (execution {execution}): {outcome}.\n\n"
    );
    match reply {
        None => text.push_str("It gave no reply text."),
        Some(reply) if reply.len() > MAX_REPORT_REPLY_BYTES => {
            let mut end = MAX_REPORT_REPLY_BYTES;
            while !reply.is_char_boundary(end) {
                end -= 1;
            }
            text.push_str("Its reply:\n");
            text.push_str(&reply[..end]);
            text.push_str("\n[reply cut at 8 KB]");
        }
        Some(reply) => {
            text.push_str("Its reply:\n");
            text.push_str(&reply);
        }
    }
    text
}
