//! One explicit linger choice: read logind, and call it only from accept.

#[cfg(any(test, target_os = "linux"))]
use std::sync::Arc;

use crate::linger::domain::LingerShown;
#[cfg(any(test, target_os = "linux"))]
use crate::linger::domain::{show, LingerCall, LingerObservation, LingerSnapshot, LoginUserId};

/// Setup's linger question.
pub(crate) trait LingerOffer: Send + Sync {
    fn status(&self) -> LingerShown;
    fn accept(&self) -> LingerShown;
}

/// A host with no logind API. Nothing here can enable linger.
#[cfg(any(test, not(target_os = "linux")))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct NotApplicableLinger;

#[cfg(any(test, not(target_os = "linux")))]
impl LingerOffer for NotApplicableLinger {
    fn status(&self) -> LingerShown {
        LingerShown::NotApplicable
    }

    fn accept(&self) -> LingerShown {
        LingerShown::NotApplicable
    }
}

/// logind's linger bit, and the one call that turns it on.
///
/// There is no disable. Turning linger off is `loginctl disable-linger`.
#[cfg(any(test, target_os = "linux"))]
pub(crate) trait LogindLinger: Send + Sync {
    fn read(&self) -> LingerSnapshot;
    fn enable(&self, user: LoginUserId) -> LingerCall;
}

#[cfg(any(test, target_os = "linux"))]
pub(crate) struct LingerSession {
    logind: Arc<dyn LogindLinger>,
}

#[cfg(any(test, target_os = "linux"))]
impl LingerSession {
    pub(crate) fn new(logind: Arc<dyn LogindLinger>) -> Self {
        Self { logind }
    }
}

#[cfg(any(test, target_os = "linux"))]
impl LingerOffer for LingerSession {
    fn status(&self) -> LingerShown {
        show(self.logind.read().observation(), None)
    }

    fn accept(&self) -> LingerShown {
        let before = self.logind.read();
        if before.observation() != LingerObservation::Disabled {
            return show(before.observation(), None);
        }
        let call = self.logind.enable(before.user());
        let after = self.logind.read();
        show(after.observation(), Some(call))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::linger::domain::{
        LingerCall, LingerObservation, LingerShown, LingerSnapshot, LoginUserId,
    };

    use super::{LingerOffer, LingerSession, LogindLinger, NotApplicableLinger};

    struct Fake {
        observation: Mutex<LingerObservation>,
        enables: Mutex<Vec<u32>>,
        call: LingerCall,
    }

    impl Fake {
        fn at(observation: LingerObservation, call: LingerCall) -> Self {
            Self {
                observation: Mutex::new(observation),
                enables: Mutex::new(Vec::new()),
                call,
            }
        }
    }

    impl LogindLinger for Fake {
        fn read(&self) -> LingerSnapshot {
            LingerSnapshot::new(
                LoginUserId::new(1000),
                *self.observation.lock().expect("read"),
            )
        }

        fn enable(&self, user: LoginUserId) -> LingerCall {
            self.enables.lock().expect("enable").push(user.get());
            self.call
        }
    }

    fn session(observation: LingerObservation, call: LingerCall) -> (LingerSession, Arc<Fake>) {
        let fake = Arc::new(Fake::at(observation, call));
        (LingerSession::new(fake.clone()), fake)
    }

    #[test]
    fn not_this_host_never_reports_a_logind_claim() {
        let offer = NotApplicableLinger;
        assert_eq!(offer.status(), LingerShown::NotApplicable);
        assert_eq!(offer.accept(), LingerShown::NotApplicable);
    }

    #[test]
    fn status_reads_and_does_not_enable() {
        let (session, fake) = session(LingerObservation::Disabled, LingerCall::Succeeded);
        assert_eq!(session.status(), LingerShown::Offer);
        assert!(fake.enables.lock().expect("enables").is_empty());
    }

    #[test]
    fn accept_calls_only_when_linger_is_off_and_shows_the_confirming_read() {
        let flipping = Flipping::new();
        let session = LingerSession::new(flipping.clone());
        assert_eq!(session.accept(), LingerShown::Enabled);
        assert_eq!(
            flipping.enables.lock().expect("enables").as_slice(),
            &[1000]
        );
    }

    struct Flipping {
        observation: Mutex<LingerObservation>,
        enables: Mutex<Vec<u32>>,
    }

    impl Flipping {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                observation: Mutex::new(LingerObservation::Disabled),
                enables: Mutex::new(Vec::new()),
            })
        }
    }

    impl LogindLinger for Flipping {
        fn read(&self) -> LingerSnapshot {
            LingerSnapshot::new(
                LoginUserId::new(1000),
                *self.observation.lock().expect("read"),
            )
        }

        fn enable(&self, user: LoginUserId) -> LingerCall {
            self.enables.lock().expect("enable").push(user.get());
            *self.observation.lock().expect("read") = LingerObservation::Enabled;
            LingerCall::Succeeded
        }
    }

    #[test]
    fn a_call_that_returns_while_linger_is_still_off_is_not_a_refusal() {
        let (session, fake) = session(LingerObservation::Disabled, LingerCall::Succeeded);
        assert_eq!(session.accept(), LingerShown::Failed);
        assert_eq!(fake.enables.lock().expect("enables").as_slice(), &[1000]);
    }

    #[test]
    fn a_refused_call_stays_refused_when_linger_is_still_off() {
        let (session, fake) = session(LingerObservation::Disabled, LingerCall::Refused);
        assert_eq!(session.accept(), LingerShown::Refused);
        assert_eq!(fake.enables.lock().expect("enables").len(), 1);
    }

    #[test]
    fn already_on_or_unreadable_does_not_call() {
        for observation in [
            LingerObservation::Enabled,
            LingerObservation::Unsupported,
            LingerObservation::Unreadable,
        ] {
            let (session, fake) = session(observation, LingerCall::Succeeded);
            let shown = session.accept();
            assert!(fake.enables.lock().expect("enables").is_empty());
            assert_ne!(shown, LingerShown::Offer);
        }
    }

    #[test]
    fn the_next_read_replaces_whatever_the_last_call_said() {
        let (session, fake) = session(LingerObservation::Disabled, LingerCall::Refused);
        assert_eq!(session.accept(), LingerShown::Refused);
        *fake.observation.lock().expect("read") = LingerObservation::Enabled;
        assert_eq!(session.status(), LingerShown::Enabled);
    }
}
