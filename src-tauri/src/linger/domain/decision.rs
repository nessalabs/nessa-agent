//! Records of an explicit linger choice.
//!
//! An intent exists only to enable linger on an account logind says is off,
//! and it is written before `SetUserLinger`. A decline is the other choice
//! and records no call. The initiator is setup: this is the only place that
//! asks.

use super::show::{LingerCall, LingerObservation, LoginUserId};

/// Who asked. Setup is the only caller this record has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerInitiator {
    Setup,
}

/// Why a record was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerCause {
    /// An accept, recorded before the call.
    Enable,
    Succeeded,
    Cancelled,
    NotAuthorized,
    TimedOut,
    Unavailable,
    Declined,
}

impl LingerCall {
    pub(crate) fn cause(self) -> LingerCause {
        match self {
            Self::Succeeded => LingerCause::Succeeded,
            Self::Cancelled => LingerCause::Cancelled,
            Self::NotAuthorized => LingerCause::NotAuthorized,
            Self::TimedOut => LingerCause::TimedOut,
            Self::Unavailable => LingerCause::Unavailable,
        }
    }
}

/// Identity of one recorded choice, minted by the audit sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LingerCorrelation(String);

impl LingerCorrelation {
    /// Lowercase hex, 32 digits. The sink mints it; a value from anywhere else
    /// is refused rather than trimmed into shape.
    pub(crate) fn parse(value: String) -> Option<Self> {
        if value.len() == 32
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            Some(Self(value))
        } else {
            None
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Written before `SetUserLinger`. Only an account that is off can have one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LingerIntent {
    user: LoginUserId,
    before: LingerObservation,
    initiator: LingerInitiator,
}

impl LingerIntent {
    pub(crate) fn enable(user: LoginUserId, before: LingerObservation) -> Option<Self> {
        if before != LingerObservation::Disabled {
            return None;
        }
        Some(Self {
            user,
            before,
            initiator: LingerInitiator::Setup,
        })
    }

    pub(crate) fn user(&self) -> LoginUserId {
        self.user
    }

    pub(crate) fn before(&self) -> LingerObservation {
        self.before
    }

    pub(crate) fn cause(&self) -> LingerCause {
        LingerCause::Enable
    }

    pub(crate) fn initiator(&self) -> LingerInitiator {
        self.initiator
    }
}

/// Written after the confirming read. `after` is that read, not the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LingerOutcome {
    intent: LingerIntent,
    call: LingerCall,
    after: LingerObservation,
}

impl LingerOutcome {
    pub(crate) fn new(intent: LingerIntent, call: LingerCall, after: LingerObservation) -> Self {
        Self {
            intent,
            call,
            after,
        }
    }

    pub(crate) fn user(&self) -> LoginUserId {
        self.intent.user()
    }

    pub(crate) fn before(&self) -> LingerObservation {
        self.intent.before()
    }

    pub(crate) fn after(&self) -> LingerObservation {
        self.after
    }

    pub(crate) fn cause(&self) -> LingerCause {
        self.call.cause()
    }

    pub(crate) fn initiator(&self) -> LingerInitiator {
        self.intent.initiator()
    }
}

/// The person chose not to enable. No call belongs to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LingerDecline {
    user: LoginUserId,
    observation: LingerObservation,
    initiator: LingerInitiator,
}

impl LingerDecline {
    pub(crate) fn new(user: LoginUserId, observation: LingerObservation) -> Option<Self> {
        if observation != LingerObservation::Disabled {
            return None;
        }
        Some(Self {
            user,
            observation,
            initiator: LingerInitiator::Setup,
        })
    }

    pub(crate) fn user(&self) -> LoginUserId {
        self.user
    }

    pub(crate) fn before(&self) -> LingerObservation {
        self.observation
    }

    pub(crate) fn after(&self) -> LingerObservation {
        self.observation
    }

    pub(crate) fn cause(&self) -> LingerCause {
        LingerCause::Declined
    }

    pub(crate) fn initiator(&self) -> LingerInitiator {
        self.initiator
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LingerCall, LingerCause, LingerCorrelation, LingerDecline, LingerInitiator, LingerIntent,
        LingerObservation, LingerOutcome, LoginUserId,
    };

    #[test]
    fn intent_exists_only_to_enable_a_disabled_account() {
        let user = LoginUserId::new(1000);
        let intent = LingerIntent::enable(user, LingerObservation::Disabled).unwrap();
        assert_eq!(intent.user(), user);
        assert_eq!(intent.before(), LingerObservation::Disabled);
        assert_eq!(intent.cause(), LingerCause::Enable);
        assert_eq!(intent.initiator(), LingerInitiator::Setup);
        for observation in [
            LingerObservation::Enabled,
            LingerObservation::Unsupported,
            LingerObservation::Unreadable,
        ] {
            assert!(LingerIntent::enable(user, observation).is_none());
        }
    }

    #[test]
    fn decline_exists_only_while_linger_is_off() {
        let user = LoginUserId::new(1000);
        let decline = LingerDecline::new(user, LingerObservation::Disabled).unwrap();
        assert_eq!(decline.cause(), LingerCause::Declined);
        assert_eq!(decline.before(), LingerObservation::Disabled);
        assert_eq!(decline.after(), LingerObservation::Disabled);
        assert_eq!(decline.initiator(), LingerInitiator::Setup);
        assert!(LingerDecline::new(user, LingerObservation::Enabled).is_none());
    }

    #[test]
    fn an_outcome_keeps_the_call_and_the_confirming_read_apart() {
        let intent =
            LingerIntent::enable(LoginUserId::new(7), LingerObservation::Disabled).unwrap();
        let outcome = LingerOutcome::new(intent, LingerCall::TimedOut, LingerObservation::Enabled);
        assert_eq!(outcome.user().get(), 7);
        assert_eq!(outcome.before(), LingerObservation::Disabled);
        assert_eq!(outcome.after(), LingerObservation::Enabled);
        assert_eq!(outcome.cause(), LingerCause::TimedOut);
        assert_eq!(outcome.initiator(), LingerInitiator::Setup);
    }

    #[test]
    fn correlation_is_lowercase_hex() {
        assert!(LingerCorrelation::parse("a".repeat(32)).is_some());
        assert!(LingerCorrelation::parse(format!("{:032x}", 4)).is_some());
        assert!(LingerCorrelation::parse("A".repeat(32)).is_none());
        assert!(LingerCorrelation::parse("g".repeat(32)).is_none());
        assert!(LingerCorrelation::parse("ab".repeat(15)).is_none());
    }
}
