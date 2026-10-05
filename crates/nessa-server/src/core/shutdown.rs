//! Typed cleanup evidence preserved by process composition.
//!
//! One [`ShutdownReport`] holds one [`Outcome`] per cleanup owner. The stage
//! reached is derived from the first `Unknown` outcome in the fixed cleanup
//! order (drains, conversations, MCP servers, native), so the order is written
//! once, in [`ShutdownReport::stage`]. The rows this module answers to are the
//! shutdown report table (SR1–SR12) in
//! `docs/design/authorized-record-reads.md`.
use crate::conversation::application::{CatalogueReadError, ConversationError, RecordReadError};
use crate::device_pairing::infrastructure::PairingRuntimeError;
use crate::mcp_servers::application::Unfinished;
use crate::product::WatchTaskFault;
use nessa_auth::application::pairing::PairingWorkerFault;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};

/// What one cleanup owner's drain established. `Unknown` is not success.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Outcome<E> {
    /// Not returned when the report was read: the owner has not reached it,
    /// or ended before it returned. Which of the two is
    /// [`ShutdownReport::stage`].
    #[default]
    Unknown,
    /// The owner returned and reported success.
    Ok,
    /// The owner returned this typed failure.
    Failed(E),
}
impl<E> Outcome<E> {
    /// Whether the owner has returned, with success or a failure.
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown)
    }
    /// Whether the owner returned success. `Unknown` is not.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
    /// The typed failure the owner returned, if it returned one.
    pub fn failed(&self) -> Option<&E> {
        match self {
            Self::Failed(error) => Some(error),
            Self::Unknown | Self::Ok => None,
        }
    }
    /// Records the owner's result. A result is observed once; a second
    /// observation is a composition defect, not new evidence.
    fn observe(&mut self, result: Result<(), E>) {
        assert!(!self.is_known(), "a cleanup outcome is observed once");
        *self = match result {
            Ok(()) => Self::Ok,
            Err(error) => Self::Failed(error),
        };
    }
}

/// The two passive readers' physical drains, and whether the deadline they
/// share with the watch drain passed while either was still unknown.
#[derive(Debug, Default)]
pub struct ReaderDrain {
    record: Outcome<RecordReadError>,
    catalogue: Outcome<CatalogueReadError>,
    deadline_exceeded: bool,
}
impl ReaderDrain {
    /// Record worker outcome; `Unknown` while its physical drain is pending.
    pub fn record(&self) -> &Outcome<RecordReadError> {
        &self.record
    }
    /// Catalogue worker outcome, preserving source and unexpected worker causes.
    pub fn catalogue(&self) -> &Outcome<CatalogueReadError> {
        &self.catalogue
    }
    /// Whether the shutdown deadline passed while either reader was unknown.
    pub fn deadline_exceeded(&self) -> bool {
        self.deadline_exceeded
    }
    /// Whether both readers have returned.
    pub fn complete(&self) -> bool {
        self.record.is_known() && self.catalogue.is_known()
    }
    /// Both readers returned success within the deadline.
    pub fn confirmed(&self) -> bool {
        self.record.is_ok() && self.catalogue.is_ok() && !self.deadline_exceeded
    }
}

/// The watch drain: the first task fault it returned, and whether the shared
/// deadline passed while it was still unknown.
#[derive(Debug, Default)]
pub struct WatchDrain {
    outcome: Outcome<WatchTaskFault>,
    deadline_exceeded: bool,
}
impl WatchDrain {
    /// The drain's outcome; `Unknown` while original watch tasks still run.
    pub fn outcome(&self) -> &Outcome<WatchTaskFault> {
        &self.outcome
    }
    /// The first task fault the drain returned, if any.
    pub fn fault(&self) -> Option<WatchTaskFault> {
        self.outcome.failed().copied()
    }
    /// Whether the shutdown deadline passed before the drain returned.
    pub fn deadline_exceeded(&self) -> bool {
        self.deadline_exceeded
    }
    /// The drain returned without a fault within the deadline.
    pub fn confirmed(&self) -> bool {
        self.outcome.is_ok() && !self.deadline_exceeded
    }
}

/// MCP stop (`composition::mcp_servers::stop`): the servers stop whatever
/// happens, so `Failed` says the drain before it ran out of time — admitted
/// changes or inspections still running, their outcomes perhaps unrecorded —
/// and the report is not confirmed.
pub type ServersOutcome = Outcome<Unfinished>;

/// Native pairing's stop did not confirm. The listener stops admission, wakes
/// and collects its peers and drains its connection owner on its own task; a
/// fault of that task leaves the drain unknown. After the drains, ended
/// enrollments' receivers are settled, and that can fail on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeShutdownFailure {
    /// The listener task ended unexpectedly before its drain was observed.
    ListenerFault(PairingWorkerFault),
    /// Every drain returned, but an ended enrollment's receiver cleanup did
    /// not complete; the registry keeps it pending (design row D6).
    Cleanup(PairingRuntimeError),
}

/// The first owner, in cleanup order, whose outcome is still unknown when the
/// report is read. It does not say whether that owner has not started, is
/// still pending, or ended without reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownStage {
    /// A reader's or the watch drain's outcome is still unknown.
    Drains,
    /// Every drain outcome is known; conversation cleanup's is still unknown.
    Conversations,
    /// Conversation cleanup's outcome is known; MCP stop's is still unknown.
    Servers,
    /// MCP stop's outcome is known; native pairing's drain's is still unknown.
    Native,
    /// Every cleanup owner's outcome is known.
    Complete,
}
impl Display for ShutdownStage {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.write_str(match self {
            Self::Drains => "drains",
            Self::Conversations => "conversations",
            Self::Servers => "MCP stop",
            Self::Native => "native drain",
            Self::Complete => "complete",
        })
    }
}

/// Everything shutdown established, one outcome per cleanup owner.
#[derive(Debug, Default)]
pub struct ShutdownReport {
    readers: ReaderDrain,
    watches: WatchDrain,
    /// Absent conversations are recorded as `Ok`: successful no-work evidence.
    conversations: Outcome<ConversationError>,
    servers: ServersOutcome,
    native: Outcome<NativeShutdownFailure>,
}
impl ShutdownReport {
    /// The two passive readers' drains.
    pub fn readers(&self) -> &ReaderDrain {
        &self.readers
    }
    /// The watch drain.
    pub fn watches(&self) -> &WatchDrain {
        &self.watches
    }
    /// Conversation cleanup.
    pub fn conversations(&self) -> &Outcome<ConversationError> {
        &self.conversations
    }
    /// MCP stop: returned or not.
    pub fn servers(&self) -> ServersOutcome {
        self.servers
    }
    /// Native pairing's drain.
    pub fn native(&self) -> &Outcome<NativeShutdownFailure> {
        &self.native
    }
    /// The first owner, in cleanup order, whose outcome is still unknown.
    pub fn stage(&self) -> ShutdownStage {
        if !self.readers.complete() || !self.watches.outcome.is_known() {
            ShutdownStage::Drains
        } else if !self.conversations.is_known() {
            ShutdownStage::Conversations
        } else if !self.servers.is_known() {
            ShutdownStage::Servers
        } else if !self.native.is_known() {
            ShutdownStage::Native
        } else {
            ShutdownStage::Complete
        }
    }
    /// Every outcome `Ok` and no deadline evidence. An unknown outcome is
    /// never confirmed, so only a `Complete` report can be.
    pub fn confirmed(&self) -> bool {
        self.readers.confirmed()
            && self.watches.confirmed()
            && self.conversations.is_ok()
            && self.servers.is_ok()
            && self.native.is_ok()
    }
    pub(crate) fn observe_record(&mut self, result: Result<(), RecordReadError>) {
        self.readers.record.observe(result);
    }
    pub(crate) fn observe_catalogue(&mut self, result: Result<(), CatalogueReadError>) {
        self.readers.catalogue.observe(result);
    }
    pub(crate) fn observe_watches(&mut self, result: Result<(), WatchTaskFault>) {
        self.watches.outcome.observe(result);
    }
    pub(crate) fn observe_conversations(&mut self, result: Result<(), ConversationError>) {
        self.conversations.observe(result);
    }
    pub(crate) fn observe_servers(&mut self, result: Result<(), Unfinished>) {
        self.servers.observe(result);
    }
    pub(crate) fn observe_native(&mut self, result: Result<(), NativeShutdownFailure>) {
        self.native.observe(result);
    }
    /// The shared drain deadline passed: deadline evidence goes only on the
    /// drains still unknown now, so a drain that already returned is not
    /// relabelled as a timeout.
    pub(crate) fn observe_drain_deadline(&mut self) {
        if !self.readers.complete() {
            self.readers.deadline_exceeded = true;
            tracing::error!("passive reader shutdown exceeded deadline; retaining runtime until physical work ends");
        }
        if !self.watches.outcome.is_known() {
            self.watches.deadline_exceeded = true;
            tracing::error!("watch shutdown exceeded deadline; retaining runtime until original watch tasks end");
        }
    }
    /// `Ok` only for a confirmed report; otherwise the report is the failure.
    pub(crate) fn into_result(self) -> Result<(), ShutdownFailure> {
        if self.confirmed() {
            Ok(())
        } else {
            Err(ShutdownFailure(Box::new(self)))
        }
    }
}

/// One line naming every cleanup owner in cleanup order. An unknown outcome
/// reads `pending` at the stage and `not started` after it; the labels follow
/// position in that order only. Deadline evidence appears only where it was
/// recorded.
impl Display for ShutdownReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let stage = self.stage();
        let at = |owner: ShutdownStage| {
            if owner == stage {
                "pending"
            } else {
                "not started"
            }
        };
        write!(f, "stage reached: {stage}")?;
        write_outcome(f, "record reader", &self.readers.record, "pending")?;
        write_outcome(f, "catalogue reader", &self.readers.catalogue, "pending")?;
        if self.readers.deadline_exceeded {
            f.write_str("; readers exceeded deadline")?;
        }
        write_outcome(f, "watch drain", &self.watches.outcome, "pending")?;
        if self.watches.deadline_exceeded {
            f.write_str("; watch drain exceeded deadline")?;
        }
        write_outcome(
            f,
            "conversations",
            &self.conversations,
            at(ShutdownStage::Conversations),
        )?;
        write_outcome(f, "MCP stop", &self.servers, at(ShutdownStage::Servers))?;
        write_outcome(f, "native drain", &self.native, at(ShutdownStage::Native))
    }
}

fn write_outcome<E: Debug>(
    f: &mut Formatter<'_>,
    owner: &str,
    outcome: &Outcome<E>,
    unknown: &str,
) -> FmtResult {
    match outcome {
        Outcome::Unknown => write!(f, "; {owner} {unknown}"),
        Outcome::Ok => write!(f, "; {owner} ok"),
        Outcome::Failed(error) => write!(f, "; {owner} failed {error:?}"),
    }
}

/// A report that did not confirm: at least one outcome unknown, failed, or
/// past its deadline. Only [`ShutdownReport::into_result`] constructs it, so
/// an all-confirmed failure cannot exist. Boxed: the report is one outcome
/// per cleanup owner, too large to carry inline in every `Result`.
#[derive(Debug)]
pub struct ShutdownFailure(Box<ShutdownReport>);
impl ShutdownFailure {
    /// Everything shutdown established before the report was read.
    pub fn report(&self) -> &ShutdownReport {
        &self.0
    }
}
impl Display for ShutdownFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(&self.0, f)
    }
}
impl Error for ShutdownFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    fn report_at(stage: ShutdownStage) -> ShutdownReport {
        let mut report = ShutdownReport::default();
        if stage == ShutdownStage::Drains {
            report.observe_catalogue(Err(CatalogueReadError::WorkerPanicked));
            report.observe_drain_deadline();
            return report;
        }
        report.observe_record(Ok(()));
        report.observe_catalogue(Err(CatalogueReadError::WorkerPanicked));
        report.observe_watches(Ok(()));
        if stage == ShutdownStage::Conversations {
            return report;
        }
        report.observe_conversations(Err(ConversationError::Audit));
        if stage == ShutdownStage::Servers {
            return report;
        }
        report.observe_servers(Ok(()));
        if stage == ShutdownStage::Native {
            return report;
        }
        report.observe_native(Err(NativeShutdownFailure::ListenerFault(
            PairingWorkerFault::Panic,
        )));
        report
    }

    /// Row SR10: one line names every owner; `pending` only at the stage
    /// reached, `not started` after it, deadline only where recorded.
    #[test]
    fn display_names_every_subsystem_and_the_stage_reached() {
        let cases = [
            (
                ShutdownStage::Drains,
                "stage reached: drains; record reader pending; catalogue reader failed WorkerPanicked; readers exceeded deadline; watch drain pending; watch drain exceeded deadline; conversations not started; MCP stop not started; native drain not started",
            ),
            (
                ShutdownStage::Conversations,
                "stage reached: conversations; record reader ok; catalogue reader failed WorkerPanicked; watch drain ok; conversations pending; MCP stop not started; native drain not started",
            ),
            (
                ShutdownStage::Servers,
                "stage reached: MCP stop; record reader ok; catalogue reader failed WorkerPanicked; watch drain ok; conversations failed Audit; MCP stop pending; native drain not started",
            ),
            (
                ShutdownStage::Native,
                "stage reached: native drain; record reader ok; catalogue reader failed WorkerPanicked; watch drain ok; conversations failed Audit; MCP stop ok; native drain pending",
            ),
            (
                ShutdownStage::Complete,
                "stage reached: complete; record reader ok; catalogue reader failed WorkerPanicked; watch drain ok; conversations failed Audit; MCP stop ok; native drain failed ListenerFault(Panic)",
            ),
        ];
        for (stage, line) in cases {
            let report = report_at(stage);
            assert_eq!(report.stage(), stage);
            let failure = report.into_result().unwrap_err();
            assert_eq!(failure.to_string(), line);
        }
    }

    /// Rows SR1 and SR6: unknown is not success, and only every outcome `Ok`
    /// with no deadline evidence confirms.
    #[test]
    fn only_a_complete_report_with_every_outcome_ok_confirms() {
        let mut report = ShutdownReport::default();
        assert_eq!(report.stage(), ShutdownStage::Drains);
        assert!(!report.confirmed());
        report.observe_record(Ok(()));
        report.observe_catalogue(Ok(()));
        report.observe_watches(Ok(()));
        report.observe_conversations(Ok(()));
        report.observe_servers(Ok(()));
        assert_eq!(report.stage(), ShutdownStage::Native);
        assert!(
            !report.confirmed(),
            "an unknown native drain is not success"
        );
        report.observe_native(Ok(()));
        assert_eq!(report.stage(), ShutdownStage::Complete);
        assert!(report.confirmed());
        assert!(report.into_result().is_ok());
    }

    /// Row SR2: a second observation of the same owner is a composition defect.
    #[test]
    #[should_panic(expected = "observed once")]
    fn an_outcome_is_observed_once() {
        let mut report = ShutdownReport::default();
        report.observe_watches(Ok(()));
        report.observe_watches(Err(WatchTaskFault::Panic));
    }
}
