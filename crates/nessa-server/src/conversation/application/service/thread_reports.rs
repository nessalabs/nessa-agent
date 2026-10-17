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
        if watches.values().map(Vec::len).sum::<usize>() >= MAX_THREAD_REPORTS {
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
        _: &Result<ExecutionOutcome, nessa_sdk::application::agent_execution::agents::AgentError>,
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
            runtime.spawn(async move { service.look_for_reports(child).await });
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

    /// Queue every report on `child` whose turn has finished, for each
    /// person who may still view it.
    pub async fn look_for_reports(&self, child: ConversationId) {
        let mut snapshot = None;
        let mut admitted = Vec::new();
        for caller in self.inner.thread_reports.callers(&child) {
            match self.live_now(&child, &caller).await {
                Ok(Some(live)) => {
                    if snapshot.is_none() {
                        snapshot = live.agent.session_manager().snapshot().await;
                    }
                    admitted.push(caller);
                }
                // Refused, or gone for good: this person is told nothing.
                Err(ConversationError::NotFound | ConversationError::Deleted) => {
                    self.inner.thread_reports.drop_person(&child, &caller);
                }
                // Not open, or not readable now: a later look, or the
                // report's lifetime, settles it.
                Ok(None) | Err(_) => {}
            }
        }
        let Some(snapshot) = snapshot else {
            return;
        };
        let taken = self
            .inner
            .thread_reports
            .take(&child, &admitted, |execution| {
                finished(&child, &snapshot, execution)
            });
        for taken in taken {
            self.deliver_report(taken).await;
        }
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

/// The report text for `execution` if its turn has finished.
fn finished(child: &ConversationId, snapshot: &SessionSnapshot, execution: &str) -> Option<String> {
    let record = invocation(snapshot, execution)?;
    let last = record.scheduling.last();
    let stage = last.map(|event| event.stage);
    let (outcome, reply) = match (&record.result, stage) {
        (Some(result), _) => (outcome(result), reply(record)),
        (None, Some(InvocationStage::Cancelled)) => ("cancelled", None),
        // Steering delivered into a turn that has since settled: that
        // turn's outcome and reply are this message's.
        (None, Some(InvocationStage::Injected)) => {
            let host = invocation(snapshot, last?.target.as_ref()?.as_str())?;
            (outcome(host.result.as_ref()?), reply(host))
        }
        _ => return None,
    };
    Some(report_text(child, execution, outcome, reply))
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
