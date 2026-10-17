//! Report a thread's finished turn back to the conversation whose agent sent it.
//!
//! `send_message` from an agent turn registers a report before it submits. The
//! child's agent runs [`ThreadReportHook`] after every invocation; it only asks
//! the service to look. The look reads the child's saved invocations: a watched
//! execution that settled, was cancelled, or was injected into a turn that has
//! since ended is finished, and its report is queued into the parent as an
//! owner turn under the caller the parent's own send was admitted with. The
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
//! full registry answers `false` and the send goes on without a report. A
//! gateway restart forgets them: the parent can still `subscribe`.
use super::{ConversationCaller, ConversationService, Inner, SubmissionMode, SubmittedMessage};
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
/// A hook can run before the settlement it follows is saved: look again this
/// many times, this far apart, before leaving the report for the next look.
const SETTLEMENT_LOOKS: usize = 20;
const SETTLEMENT_LOOK_DELAY: Duration = Duration::from_millis(100);

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
impl Drop for Delivering {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ThreadReports {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, Vec<Watch>>> {
        self.watches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn delivering(&self) -> Delivering {
        self.delivering.fetch_add(1, Ordering::SeqCst);
        Delivering(self.delivering.clone())
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
            list.retain(|watch| watch.since.elapsed() < REPORT_LIFETIME);
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

    /// Remove and return every watch `finished` decides, with its report text.
    fn take(
        &self,
        child: &ConversationId,
        finished: impl Fn(&str) -> Option<String>,
    ) -> (Vec<(ThreadReport, String)>, usize) {
        let mut watches = self.lock();
        let Some(list) = watches.get_mut(child) else {
            return (Vec::new(), 0);
        };
        let mut taken = Vec::new();
        list.retain(|watch| match finished(&watch.execution) {
            Some(text) => {
                taken.push((watch.report.clone(), text));
                false
            }
            None => true,
        });
        let left = list.len();
        if left == 0 {
            watches.remove(child);
        }
        (taken, left)
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
            runtime.spawn(async move { service.look_for_reports(child, true).await });
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

    /// Queue every report on `child` whose turn has finished. `settling` is a
    /// look after an invocation ended, whose own settlement may not be saved
    /// yet: it looks again while a watch on that invocation still waits.
    pub async fn look_for_reports(&self, child: ConversationId, settling: bool) {
        let looks = if settling { SETTLEMENT_LOOKS } else { 1 };
        for look in 0..looks {
            if look > 0 {
                tokio::time::sleep(SETTLEMENT_LOOK_DELAY).await;
            }
            let Some(snapshot) = self.report_snapshot(&child).await else {
                return;
            };
            let (taken, left) = self
                .inner
                .thread_reports
                .take(&child, |execution| finished(&child, &snapshot, execution));
            for (report, text) in taken {
                self.deliver_report(report, text).await;
            }
            if left == 0 {
                return;
            }
        }
    }

    /// The child's saved invocations, read through any waiting report's
    /// caller: a person who may no longer view the thread is told nothing.
    async fn report_snapshot(&self, child: &ConversationId) -> Option<SessionSnapshot> {
        let caller = {
            let watches = self.inner.thread_reports.lock();
            watches.get(child)?.first()?.report.caller.clone()
        };
        let live = self.live_now(child, &caller).await.ok()??;
        live.agent.session_manager().snapshot().await
    }

    /// Queue one report into its parent. Commands of one person are taken
    /// one at a time with no queue (`CreationStorage`), so a report that meets
    /// the person's own command tries again for a bounded while.
    async fn deliver_report(&self, report: ThreadReport, text: String) {
        let Some(storage) = self.commands() else {
            return;
        };
        let _delivering = self.inner.thread_reports.delivering();
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

/// Whether a turn is running, or work waits, in the snapshot: a later look
/// will follow when it ends.
fn running_or_waiting(snapshot: &SessionSnapshot) -> bool {
    snapshot.invocations.iter().any(|record| {
        record.result.is_none()
            && record.scheduling.last().is_some_and(|event| {
                matches!(
                    event.stage,
                    InvocationStage::Running | InvocationStage::Queued
                )
            })
    })
}

/// The report text for `execution` if its turn has finished.
fn finished(child: &ConversationId, snapshot: &SessionSnapshot, execution: &str) -> Option<String> {
    let index = snapshot
        .invocations
        .iter()
        .position(|record| record.request.execution_id.as_str() == execution)?;
    let record = &snapshot.invocations[index];
    let stage = record.scheduling.last().map(|event| event.stage);
    let (outcome, reply) = match (&record.result, stage) {
        (Some(result), _) => (outcome(result), reply(record)),
        (None, Some(InvocationStage::Cancelled)) => ("cancelled", None),
        // Steering delivered into a turn that has since ended: that turn's
        // outcome and reply are this message's.
        (None, Some(InvocationStage::Injected)) if !running_or_waiting(snapshot) => {
            // The host was running when this was injected: admitted earlier.
            let host = snapshot.invocations[..index]
                .iter()
                .rev()
                .find(|record| record.result.is_some())?;
            (
                outcome(host.result.as_ref().expect("found settled")),
                reply(host),
            )
        }
        _ => return None,
    };
    Some(report_text(child, execution, outcome, reply))
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
