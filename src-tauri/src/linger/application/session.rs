//! One process's linger choice.
//!
//! `status` and `decline` never call logind's enable. `accept` calls it only
//! when the read just taken says linger is off and no call is already in
//! flight, and only after the intent record is stored.

use std::sync::{Arc, Mutex};

use crate::linger::domain::LingerShown;
#[cfg(any(test, target_os = "linux"))]
use crate::linger::domain::{show, LingerAttempt, LingerSnapshot};

#[cfg(any(test, target_os = "linux"))]
use super::{LingerAudit, LingerAuditFailure, LogindLinger};
#[cfg(any(test, target_os = "linux"))]
use crate::linger::domain::{LingerDecline, LingerIntent, LingerObservation, LingerOutcome};

/// Whether the record for the choice the screen is answering was stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuditDelivery {
    /// A read, or a choice that did not change anything.
    NotRequired,
    Recorded,
    Failed,
}

/// What setup renders. `shown` comes from `show`; the audit flag does not change it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LingerView {
    shown: LingerShown,
    audit: AuditDelivery,
}

impl LingerView {
    #[cfg(any(test, not(target_os = "linux")))]
    pub(crate) fn not_this_host() -> Self {
        Self {
            shown: LingerShown::NotApplicable,
            audit: AuditDelivery::NotRequired,
        }
    }

    pub(crate) fn shown(self) -> LingerShown {
        self.shown
    }

    pub(crate) fn audit(self) -> AuditDelivery {
        self.audit
    }

    #[cfg(test)]
    pub(crate) fn from_shown_for_test(shown: LingerShown, audit: AuditDelivery) -> Self {
        Self { shown, audit }
    }

    #[cfg(any(test, target_os = "linux"))]
    fn from_read(snapshot: LingerSnapshot, attempt: LingerAttempt, audit: AuditDelivery) -> Self {
        Self {
            shown: show(snapshot.observation(), attempt),
            audit,
        }
    }
}

/// Setup's linger question for this process.
pub(crate) trait LingerOffer: Send + Sync {
    fn status(&self) -> LingerView;
    fn accept(&self) -> LingerView;
    fn decline(&self) -> LingerView;
}

#[cfg(any(test, target_os = "linux"))]
pub(crate) struct LingerSession {
    logind: Arc<dyn LogindLinger>,
    audit: Arc<dyn LingerAudit>,
    attempt: Mutex<LingerAttempt>,
}

#[cfg(any(test, target_os = "linux"))]
impl LingerSession {
    pub(crate) fn new(logind: Arc<dyn LogindLinger>, audit: Arc<dyn LingerAudit>) -> Self {
        Self {
            logind,
            audit,
            attempt: Mutex::new(LingerAttempt::NotChosen),
        }
    }

    fn view(&self, attempt: LingerAttempt, audit: AuditDelivery) -> LingerView {
        LingerView::from_read(self.logind.read(), attempt, audit)
    }
}

#[cfg(any(test, target_os = "linux"))]
impl LingerOffer for LingerSession {
    fn status(&self) -> LingerView {
        let attempt = *self.attempt.lock().expect("linger attempt");
        self.view(attempt, AuditDelivery::NotRequired)
    }

    fn decline(&self) -> LingerView {
        let mut attempt = self.attempt.lock().expect("linger attempt");
        if *attempt == LingerAttempt::InFlight {
            return self.view(*attempt, AuditDelivery::NotRequired);
        }
        let snapshot = self.logind.read();
        if snapshot.observation() != LingerObservation::Disabled {
            return LingerView::from_read(snapshot, *attempt, AuditDelivery::NotRequired);
        }
        let Some(decline) = LingerDecline::new(snapshot.user(), snapshot.observation()) else {
            return LingerView::from_read(snapshot, *attempt, AuditDelivery::NotRequired);
        };
        *attempt = LingerAttempt::Declined;
        let audit = match self.audit.record_decline(&decline) {
            Ok(()) => AuditDelivery::Recorded,
            Err(LingerAuditFailure) => AuditDelivery::Failed,
        };
        LingerView::from_read(snapshot, LingerAttempt::Declined, audit)
    }

    fn accept(&self) -> LingerView {
        let mut attempt = self.attempt.lock().expect("linger attempt");
        if *attempt == LingerAttempt::InFlight {
            return self.view(*attempt, AuditDelivery::NotRequired);
        }
        let snapshot = self.logind.read();
        if snapshot.observation() != LingerObservation::Disabled {
            return LingerView::from_read(snapshot, *attempt, AuditDelivery::NotRequired);
        }
        let Some(intent) = LingerIntent::enable(snapshot.user(), snapshot.observation()) else {
            return LingerView::from_read(snapshot, *attempt, AuditDelivery::NotRequired);
        };
        let correlation = match self.audit.record_intent(&intent) {
            Ok(correlation) => correlation,
            Err(LingerAuditFailure) => {
                return LingerView::from_read(snapshot, *attempt, AuditDelivery::Failed);
            }
        };
        // Released before the prompt so a second accept sees the call in flight
        // and does not send another. The prompt can outlast this function.
        *attempt = LingerAttempt::InFlight;
        drop(attempt);

        let call = self.logind.enable(snapshot.user());
        let after = self.logind.read();
        let finished = call.attempt();
        let outcome = LingerOutcome::new(intent, call, after.observation());
        let audit = match self.audit.record_outcome(&correlation, &outcome) {
            Ok(()) => AuditDelivery::Recorded,
            Err(LingerAuditFailure) => AuditDelivery::Failed,
        };
        *self.attempt.lock().expect("linger attempt") = finished;
        LingerView::from_read(after, finished, audit)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread;
    use std::time::Duration;

    use crate::linger::domain::{
        LingerAttempt, LingerCall, LingerCause, LingerCorrelation, LingerDecline, LingerInitiator,
        LingerIntent, LingerObservation, LingerOutcome, LingerShown, LingerSnapshot, LoginUserId,
    };

    use super::{
        AuditDelivery, LingerAudit, LingerAuditFailure, LingerOffer, LingerSession, LogindLinger,
    };

    struct Trace(Mutex<Vec<&'static str>>);

    struct Gate {
        started: Mutex<bool>,
        start: Condvar,
        release: Mutex<bool>,
        go: Condvar,
    }

    struct FakeLogind {
        user: LoginUserId,
        observation: Mutex<LingerObservation>,
        enables: Mutex<Vec<LoginUserId>>,
        call: Mutex<LingerCall>,
        flip: Mutex<Option<LingerObservation>>,
        trace: Arc<Trace>,
        gate: Option<Arc<Gate>>,
    }

    struct FakeAudit {
        fail_intent: AtomicBool,
        fail_outcome: AtomicBool,
        fail_decline: AtomicBool,
        next: Mutex<u64>,
        intents: Mutex<Vec<LingerIntent>>,
        outcomes: Mutex<Vec<(String, LingerOutcome)>>,
        declines: Mutex<Vec<LingerDecline>>,
        trace: Arc<Trace>,
    }

    impl Trace {
        fn push(&self, event: &'static str) {
            self.0.lock().expect("trace").push(event);
        }

        fn list(&self) -> Vec<&'static str> {
            self.0.lock().expect("trace").clone()
        }
    }

    impl Gate {
        fn new() -> Self {
            Self {
                started: Mutex::new(false),
                start: Condvar::new(),
                release: Mutex::new(false),
                go: Condvar::new(),
            }
        }

        fn entered(&self) {
            *self.started.lock().expect("start") = true;
            self.start.notify_one();
            let guard = self.release.lock().expect("release");
            let (guard, wait) = self
                .go
                .wait_timeout_while(guard, Duration::from_secs(5), |release| !*release)
                .expect("release");
            assert!(
                !wait.timed_out() && *guard,
                "linger prompt was not released"
            );
        }

        fn until_entered(&self) {
            let guard = self.started.lock().expect("start");
            let (guard, wait) = self
                .start
                .wait_timeout_while(guard, Duration::from_secs(5), |started| !*started)
                .expect("start");
            assert!(!wait.timed_out() && *guard, "linger prompt did not start");
        }

        fn release(&self) {
            *self.release.lock().expect("release") = true;
            self.go.notify_one();
        }
    }

    impl FakeLogind {
        fn new(trace: Arc<Trace>, observation: LingerObservation) -> Self {
            Self {
                user: LoginUserId::new(1000),
                observation: Mutex::new(observation),
                enables: Mutex::new(Vec::new()),
                call: Mutex::new(LingerCall::Succeeded),
                flip: Mutex::new(None),
                trace,
                gate: None,
            }
        }

        fn set(&self, observation: LingerObservation) {
            *self.observation.lock().expect("observation") = observation;
        }

        fn enables(&self) -> Vec<u32> {
            self.enables
                .lock()
                .expect("enables")
                .iter()
                .map(|user| user.get())
                .collect()
        }
    }

    impl LogindLinger for FakeLogind {
        fn read(&self) -> LingerSnapshot {
            LingerSnapshot::new(self.user, *self.observation.lock().expect("observation"))
        }

        fn enable(&self, user: LoginUserId) -> LingerCall {
            self.trace.push("enable");
            self.enables.lock().expect("enables").push(user);
            if let Some(gate) = &self.gate {
                gate.entered();
            }
            if let Some(observation) = *self.flip.lock().expect("flip") {
                self.set(observation);
            }
            *self.call.lock().expect("call")
        }
    }

    impl FakeAudit {
        fn new(trace: Arc<Trace>) -> Self {
            Self {
                fail_intent: AtomicBool::new(false),
                fail_outcome: AtomicBool::new(false),
                fail_decline: AtomicBool::new(false),
                next: Mutex::new(1),
                intents: Mutex::new(Vec::new()),
                outcomes: Mutex::new(Vec::new()),
                declines: Mutex::new(Vec::new()),
                trace,
            }
        }

        fn mint(&self) -> LingerCorrelation {
            let mut next = self.next.lock().expect("id");
            let id = *next;
            *next += 1;
            LingerCorrelation::parse(format!("{id:032x}")).expect("minted correlation")
        }
    }

    impl LingerAudit for FakeAudit {
        fn record_intent(
            &self,
            intent: &LingerIntent,
        ) -> Result<LingerCorrelation, LingerAuditFailure> {
            self.trace.push("intent");
            if self.fail_intent.load(Ordering::Relaxed) {
                return Err(LingerAuditFailure);
            }
            self.intents.lock().expect("intents").push(intent.clone());
            Ok(self.mint())
        }

        fn record_outcome(
            &self,
            correlation: &LingerCorrelation,
            outcome: &LingerOutcome,
        ) -> Result<(), LingerAuditFailure> {
            self.trace.push("outcome");
            if self.fail_outcome.load(Ordering::Relaxed) {
                return Err(LingerAuditFailure);
            }
            self.outcomes
                .lock()
                .expect("outcomes")
                .push((correlation.as_str().to_owned(), outcome.clone()));
            Ok(())
        }

        fn record_decline(&self, decline: &LingerDecline) -> Result<(), LingerAuditFailure> {
            self.trace.push("decline");
            if self.fail_decline.load(Ordering::Relaxed) {
                return Err(LingerAuditFailure);
            }
            self.declines
                .lock()
                .expect("declines")
                .push(decline.clone());
            Ok(())
        }
    }

    struct Rig {
        logind: Arc<FakeLogind>,
        audit: Arc<FakeAudit>,
        trace: Arc<Trace>,
        session: LingerSession,
    }

    impl Rig {
        fn at(observation: LingerObservation) -> Self {
            let trace = Arc::new(Trace(Mutex::new(Vec::new())));
            let logind = Arc::new(FakeLogind::new(trace.clone(), observation));
            let audit = Arc::new(FakeAudit::new(trace.clone()));
            let session = LingerSession::new(logind.clone(), audit.clone());
            Self {
                logind,
                audit,
                trace,
                session,
            }
        }
    }

    #[test]
    fn status_reads_and_does_not_enable() {
        let rig = Rig::at(LingerObservation::Disabled);
        let view = rig.session.status();
        assert_eq!(view.shown(), LingerShown::Offer);
        assert_eq!(view.audit(), AuditDelivery::NotRequired);
        assert!(rig.logind.enables().is_empty());
        assert!(rig.trace.list().is_empty());
    }

    #[test]
    fn decline_does_not_enable() {
        let rig = Rig::at(LingerObservation::Disabled);
        let view = rig.session.decline();
        assert_eq!(view.shown(), LingerShown::Declined);
        assert_eq!(view.audit(), AuditDelivery::Recorded);
        assert!(rig.logind.enables().is_empty());
        let declines = rig.audit.declines.lock().expect("declines");
        assert_eq!(declines.len(), 1);
        assert_eq!(declines[0].user().get(), 1000);
        assert_eq!(declines[0].cause(), LingerCause::Declined);
        assert_eq!(declines[0].initiator(), LingerInitiator::Setup);
        assert_eq!(rig.trace.list(), ["decline"]);
    }

    #[test]
    fn accept_enables_only_when_linger_is_off_and_records_intent_before_the_call() {
        let rig = Rig::at(LingerObservation::Disabled);
        *rig.logind.flip.lock().expect("flip") = Some(LingerObservation::Enabled);
        let view = rig.session.accept();
        assert_eq!(view.shown(), LingerShown::Enabled);
        assert_eq!(view.audit(), AuditDelivery::Recorded);
        assert_eq!(rig.logind.enables(), [1000]);
        assert_eq!(rig.trace.list(), ["intent", "enable", "outcome"]);
        let intents = rig.audit.intents.lock().expect("intents");
        assert_eq!(intents[0].user().get(), 1000);
        assert_eq!(intents[0].before(), LingerObservation::Disabled);
        assert_eq!(intents[0].cause(), LingerCause::Enable);
        assert_eq!(intents[0].initiator(), LingerInitiator::Setup);
        let outcomes = rig.audit.outcomes.lock().expect("outcomes");
        assert_eq!(outcomes[0].1.before(), LingerObservation::Disabled);
        assert_eq!(outcomes[0].1.after(), LingerObservation::Enabled);
        assert_eq!(outcomes[0].1.cause(), LingerCause::Succeeded);
        assert_eq!(outcomes[0].1.initiator(), LingerInitiator::Setup);
    }

    #[test]
    fn a_failed_intent_record_does_not_enable() {
        let rig = Rig::at(LingerObservation::Disabled);
        rig.audit.fail_intent.store(true, Ordering::Relaxed);
        let view = rig.session.accept();
        assert_eq!(view.shown(), LingerShown::Offer);
        assert_eq!(view.audit(), AuditDelivery::Failed);
        assert!(rig.logind.enables().is_empty());
        assert!(rig.audit.intents.lock().expect("intents").is_empty());
        assert_eq!(rig.session.status().shown(), LingerShown::Offer);
    }

    #[test]
    fn a_failed_outcome_record_still_reports_the_confirming_read() {
        let rig = Rig::at(LingerObservation::Disabled);
        rig.audit.fail_outcome.store(true, Ordering::Relaxed);
        *rig.logind.flip.lock().expect("flip") = Some(LingerObservation::Enabled);
        let view = rig.session.accept();
        assert_eq!(view.shown(), LingerShown::Enabled);
        assert_eq!(view.audit(), AuditDelivery::Failed);
        assert_eq!(rig.logind.enables(), [1000]);
        assert!(rig.audit.outcomes.lock().expect("outcomes").is_empty());
        // A later read reports the flag, not the audit of the call that set it.
        assert_eq!(rig.session.status().audit(), AuditDelivery::NotRequired);
        assert_eq!(rig.session.status().shown(), LingerShown::Enabled);
    }

    #[test]
    fn a_finished_call_reports_the_confirming_read_not_the_reply() {
        let cases = [
            (
                LingerCall::Succeeded,
                LingerObservation::Disabled,
                LingerShown::Refused,
            ),
            (
                LingerCall::Succeeded,
                LingerObservation::Enabled,
                LingerShown::Enabled,
            ),
            (
                LingerCall::Cancelled,
                LingerObservation::Disabled,
                LingerShown::Refused,
            ),
            (
                LingerCall::Cancelled,
                LingerObservation::Enabled,
                LingerShown::Enabled,
            ),
            (
                LingerCall::NotAuthorized,
                LingerObservation::Disabled,
                LingerShown::Refused,
            ),
            (
                LingerCall::TimedOut,
                LingerObservation::Disabled,
                LingerShown::Refused,
            ),
            (
                LingerCall::TimedOut,
                LingerObservation::Enabled,
                LingerShown::Enabled,
            ),
            (
                LingerCall::Unavailable,
                LingerObservation::Disabled,
                LingerShown::Refused,
            ),
            (
                LingerCall::Unavailable,
                LingerObservation::Unsupported,
                LingerShown::Unsupported,
            ),
        ];
        for (call, after, shown) in cases {
            let rig = Rig::at(LingerObservation::Disabled);
            *rig.logind.call.lock().expect("call") = call;
            *rig.logind.flip.lock().expect("flip") = Some(after);
            let view = rig.session.accept();
            assert_eq!(view.shown(), shown, "{call:?} then {after:?}");
            let outcomes = rig.audit.outcomes.lock().expect("outcomes");
            assert_eq!(outcomes[0].1.cause(), call.cause());
            assert_eq!(outcomes[0].1.after(), after);
            assert_eq!(rig.logind.enables(), [1000]);
        }
    }

    #[test]
    fn already_on_or_unreadable_does_not_call() {
        for observation in [
            LingerObservation::Enabled,
            LingerObservation::Unsupported,
            LingerObservation::Unreadable,
        ] {
            let rig = Rig::at(observation);
            let view = rig.session.accept();
            assert!(rig.logind.enables().is_empty(), "{observation:?}");
            assert!(rig.audit.intents.lock().expect("intents").is_empty());
            assert_eq!(view.audit(), AuditDelivery::NotRequired);
            assert_eq!(
                view.shown(),
                crate::linger::domain::show(observation, LingerAttempt::NotChosen)
            );
        }
    }

    #[test]
    fn a_second_accept_while_the_prompt_is_open_does_not_call_again() {
        let trace = Arc::new(Trace(Mutex::new(Vec::new())));
        let mut logind = FakeLogind::new(trace.clone(), LingerObservation::Disabled);
        let gate = Arc::new(Gate::new());
        logind.gate = Some(gate.clone());
        *logind.flip.lock().expect("flip") = Some(LingerObservation::Enabled);
        let logind = Arc::new(logind);
        let audit = Arc::new(FakeAudit::new(trace));
        let session = Arc::new(LingerSession::new(logind.clone(), audit));
        let running = Arc::clone(&session);
        let worker = thread::spawn(move || running.accept());
        gate.until_entered();
        let second = session.accept();
        assert_eq!(second.shown(), LingerShown::Waiting);
        assert_eq!(logind.enables(), [1000]);
        let during = session.status();
        assert_eq!(during.shown(), LingerShown::Waiting);
        assert_eq!(logind.enables(), [1000]);
        gate.release();
        let first = worker.join().expect("accept");
        assert_eq!(first.shown(), LingerShown::Enabled);
        assert_eq!(first.audit(), AuditDelivery::Recorded);
        assert_eq!(logind.enables(), [1000]);
    }

    #[test]
    fn decline_during_the_prompt_does_not_record_a_decline() {
        let trace = Arc::new(Trace(Mutex::new(Vec::new())));
        let mut logind = FakeLogind::new(trace.clone(), LingerObservation::Disabled);
        let gate = Arc::new(Gate::new());
        logind.gate = Some(gate.clone());
        *logind.flip.lock().expect("flip") = Some(LingerObservation::Enabled);
        let logind = Arc::new(logind);
        let audit = Arc::new(FakeAudit::new(trace));
        let session = Arc::new(LingerSession::new(logind.clone(), audit.clone()));
        let running = Arc::clone(&session);
        let worker = thread::spawn(move || running.accept());
        gate.until_entered();
        let declined = session.decline();
        assert_eq!(declined.shown(), LingerShown::Waiting);
        assert!(audit.declines.lock().expect("declines").is_empty());
        assert_eq!(logind.enables(), [1000]);
        gate.release();
        assert_eq!(worker.join().expect("accept").shown(), LingerShown::Enabled);
    }

    #[test]
    fn the_next_process_keeps_only_the_fresh_read() {
        let rig = Rig::at(LingerObservation::Disabled);
        *rig.logind.call.lock().expect("call") = LingerCall::Cancelled;
        assert_eq!(rig.session.accept().shown(), LingerShown::Refused);
        let again = LingerSession::new(rig.logind.clone(), rig.audit.clone());
        // The refusal died with the first process. Linger is still off, so the
        // offer is back; the lost or finished prompt is not a stored refusal.
        assert_eq!(again.status().shown(), LingerShown::Offer);
        assert_eq!(rig.logind.enables(), [1000]);

        let landed = Rig::at(LingerObservation::Disabled);
        *landed.logind.flip.lock().expect("flip") = Some(LingerObservation::Enabled);
        assert_eq!(landed.session.accept().shown(), LingerShown::Enabled);
        drop(landed.session);
        let next = LingerSession::new(landed.logind.clone(), landed.audit.clone());
        assert_eq!(next.status().shown(), LingerShown::Enabled);
        assert_eq!(landed.logind.enables(), [1000]);
    }

    #[test]
    fn linger_flipped_outside_replaces_the_claim_on_the_next_read() {
        let already = Rig::at(LingerObservation::Enabled);
        assert_eq!(already.session.status().shown(), LingerShown::Enabled);
        already.logind.set(LingerObservation::Disabled);
        assert_eq!(already.session.status().shown(), LingerShown::Offer);
        assert!(already.logind.enables().is_empty());

        let declined = Rig::at(LingerObservation::Disabled);
        assert_eq!(declined.session.decline().shown(), LingerShown::Declined);
        declined.logind.set(LingerObservation::Enabled);
        assert_eq!(declined.session.status().shown(), LingerShown::Enabled);

        let refused = Rig::at(LingerObservation::Disabled);
        *refused.logind.call.lock().expect("call") = LingerCall::NotAuthorized;
        assert_eq!(refused.session.accept().shown(), LingerShown::Refused);
        refused.logind.set(LingerObservation::Enabled);
        assert_eq!(refused.session.status().shown(), LingerShown::Enabled);
        refused.logind.set(LingerObservation::Disabled);
        assert_eq!(refused.session.status().shown(), LingerShown::Refused);

        let offered = Rig::at(LingerObservation::Disabled);
        offered.logind.set(LingerObservation::Unsupported);
        assert_eq!(offered.session.status().shown(), LingerShown::Unsupported);
        assert_eq!(offered.session.accept().shown(), LingerShown::Unsupported);
        assert!(offered.logind.enables().is_empty());
    }

    #[test]
    fn try_again_after_a_refusal_is_another_explicit_accept() {
        let rig = Rig::at(LingerObservation::Disabled);
        *rig.logind.call.lock().expect("call") = LingerCall::Cancelled;
        assert_eq!(rig.session.accept().shown(), LingerShown::Refused);
        *rig.logind.call.lock().expect("call") = LingerCall::Succeeded;
        *rig.logind.flip.lock().expect("flip") = Some(LingerObservation::Enabled);
        assert_eq!(rig.session.accept().shown(), LingerShown::Enabled);
        assert_eq!(rig.logind.enables(), [1000, 1000]);
    }

    #[test]
    fn a_failed_decline_record_still_does_not_enable() {
        let rig = Rig::at(LingerObservation::Disabled);
        rig.audit.fail_decline.store(true, Ordering::Relaxed);
        let view = rig.session.decline();
        assert_eq!(view.shown(), LingerShown::Declined);
        assert_eq!(view.audit(), AuditDelivery::Failed);
        assert!(rig.logind.enables().is_empty());
    }

    #[test]
    fn decline_does_not_cover_a_read_that_is_already_on() {
        let rig = Rig::at(LingerObservation::Enabled);
        let view = rig.session.decline();
        assert_eq!(view.shown(), LingerShown::Enabled);
        assert!(rig.audit.declines.lock().expect("declines").is_empty());
        assert!(rig.logind.enables().is_empty());
    }
}
