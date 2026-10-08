//! The screen for one logind read and this process's attempt.
//!
//! The table is [ADR 217](../../../../../docs/adr/done/217-linux-linger-at-setup.md).
//! `enabled` is the only claim that the gateway keeps running after logout,
//! and it is returned only for a read whose `Linger` property is true.

/// The account logind was asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoginUserId(u32);

impl LoginUserId {
    pub(crate) fn new(uid: u32) -> Self {
        Self(uid)
    }

    pub(crate) fn get(self) -> u32 {
        self.0
    }
}

/// What the latest logind read said about this user's linger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerObservation {
    /// `org.freedesktop.login1.User.Linger` is true.
    Enabled,
    /// logind answered and `Linger` is false.
    Disabled,
    /// The system bus cannot be reached, or logind has no owner.
    Unsupported,
    /// logind answered and `Linger` could not be read.
    Unreadable,
}

/// What this process has done about the explicit choice.
///
/// A process that replaces one which quit mid-prompt starts at [`NotChosen`].
/// Nothing here is written down for the next process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerAttempt {
    /// No choice yet.
    NotChosen,
    /// The person chose not to enable. No call was made.
    Declined,
    /// `SetUserLinger` has been sent and has not returned.
    InFlight,
    /// The method returned without an error. Not confirmation.
    Succeeded,
    /// The polkit prompt was cancelled or dismissed.
    Cancelled,
    /// polkit did not authorize the change.
    NotAuthorized,
    /// The wait ended with no reply.
    TimedOut,
    /// The call failed because logind was not there to answer it.
    Unavailable,
}

/// How a finished `SetUserLinger` call came back, before the confirming read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerCall {
    Succeeded,
    Cancelled,
    NotAuthorized,
    TimedOut,
    Unavailable,
}

impl LingerCall {
    pub(crate) fn attempt(self) -> LingerAttempt {
        match self {
            Self::Succeeded => LingerAttempt::Succeeded,
            Self::Cancelled => LingerAttempt::Cancelled,
            Self::NotAuthorized => LingerAttempt::NotAuthorized,
            Self::TimedOut => LingerAttempt::TimedOut,
            Self::Unavailable => LingerAttempt::Unavailable,
        }
    }
}

/// What setup shows. `show` never returns the screen for a host with no
/// logind API; that screen exists only on such a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerShown {
    /// This host has no logind API. Setup does not ask.
    #[cfg(any(test, not(target_os = "linux")))]
    NotApplicable,
    /// Linger is off and the person has not chosen.
    Offer,
    /// logind says linger is on.
    Enabled,
    /// The person chose not to enable, and logind still says it is off.
    Declined,
    /// A finished attempt left linger off.
    Refused,
    /// The prompt has not returned, and linger is still off.
    Waiting,
    /// There is no logind.
    Unsupported,
    /// logind did not yield a linger bit. Nothing is claimed.
    Unconfirmed,
}

/// One read: which account, and what logind said about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LingerSnapshot {
    user: LoginUserId,
    observation: LingerObservation,
}

impl LingerSnapshot {
    pub(crate) fn new(user: LoginUserId, observation: LingerObservation) -> Self {
        Self { user, observation }
    }

    pub(crate) fn user(self) -> LoginUserId {
        self.user
    }

    pub(crate) fn observation(self) -> LingerObservation {
        self.observation
    }
}

/// The screen for this read and this process's attempt.
///
/// `enabled` is returned only when `observation` is [`LingerObservation::Enabled`].
/// The attempt cannot promote a read.
#[must_use]
pub(crate) fn show(observation: LingerObservation, attempt: LingerAttempt) -> LingerShown {
    match observation {
        LingerObservation::Enabled => LingerShown::Enabled,
        LingerObservation::Unsupported => LingerShown::Unsupported,
        LingerObservation::Unreadable => LingerShown::Unconfirmed,
        LingerObservation::Disabled => match attempt {
            LingerAttempt::NotChosen => LingerShown::Offer,
            LingerAttempt::Declined => LingerShown::Declined,
            LingerAttempt::InFlight => LingerShown::Waiting,
            LingerAttempt::Succeeded
            | LingerAttempt::Cancelled
            | LingerAttempt::NotAuthorized
            | LingerAttempt::TimedOut
            | LingerAttempt::Unavailable => LingerShown::Refused,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{show, LingerAttempt, LingerObservation, LingerShown};

    /// ADR 217's grid. Each pair is one assertion; the lists below are every
    /// variant, so a new one fails to compile until it has a row.
    const ROWS: &[(LingerObservation, LingerAttempt, LingerShown)] = &[
        (Enabled, NotChosen, EnabledShown),
        (Enabled, Declined, EnabledShown),
        (Enabled, InFlight, EnabledShown),
        (Enabled, Succeeded, EnabledShown),
        (Enabled, Cancelled, EnabledShown),
        (Enabled, NotAuthorized, EnabledShown),
        (Enabled, TimedOut, EnabledShown),
        (Enabled, Unavailable, EnabledShown),
        (Unsupported, NotChosen, UnsupportedShown),
        (Unsupported, Declined, UnsupportedShown),
        (Unsupported, InFlight, UnsupportedShown),
        (Unsupported, Succeeded, UnsupportedShown),
        (Unsupported, Cancelled, UnsupportedShown),
        (Unsupported, NotAuthorized, UnsupportedShown),
        (Unsupported, TimedOut, UnsupportedShown),
        (Unsupported, Unavailable, UnsupportedShown),
        (Unreadable, NotChosen, Unconfirmed),
        (Unreadable, Declined, Unconfirmed),
        (Unreadable, InFlight, Unconfirmed),
        (Unreadable, Succeeded, Unconfirmed),
        (Unreadable, Cancelled, Unconfirmed),
        (Unreadable, NotAuthorized, Unconfirmed),
        (Unreadable, TimedOut, Unconfirmed),
        (Unreadable, Unavailable, Unconfirmed),
        (Disabled, NotChosen, Offer),
        (Disabled, Declined, DeclinedShown),
        (Disabled, InFlight, Waiting),
        (Disabled, Succeeded, Refused),
        (Disabled, Cancelled, Refused),
        (Disabled, NotAuthorized, Refused),
        (Disabled, TimedOut, Refused),
        (Disabled, Unavailable, Refused),
    ];

    use LingerAttempt::{
        Cancelled, Declined, InFlight, NotAuthorized, NotChosen, Succeeded, TimedOut, Unavailable,
    };
    use LingerObservation::{Disabled, Enabled, Unreadable, Unsupported};
    use LingerShown::{
        Declined as DeclinedShown, Enabled as EnabledShown, Offer, Refused, Unconfirmed,
        Unsupported as UnsupportedShown, Waiting,
    };

    fn each_pair(mut visit: impl FnMut(LingerObservation, LingerAttempt)) {
        for observation in [Enabled, Disabled, Unsupported, Unreadable] {
            match observation {
                Enabled => {}
                Disabled => {}
                Unsupported => {}
                Unreadable => {}
            }
            for attempt in [
                NotChosen,
                Declined,
                InFlight,
                Succeeded,
                Cancelled,
                NotAuthorized,
                TimedOut,
                Unavailable,
            ] {
                match attempt {
                    NotChosen => {}
                    Declined => {}
                    InFlight => {}
                    Succeeded => {}
                    Cancelled => {}
                    NotAuthorized => {}
                    TimedOut => {}
                    Unavailable => {}
                }
                visit(observation, attempt);
            }
        }
    }

    #[test]
    fn every_logind_state_and_attempt_shows_the_decided_row() {
        // 4 observations × 8 attempts. A lost reply is the `not-chosen` column
        // on a fresh read: enabled stays a claim, disabled is the offer again,
        // and unsupported / unreadable claim nothing.
        assert_eq!(
            ROWS.len(),
            32,
            "ADR 217 has one row per observation × attempt"
        );
        each_pair(|observation, attempt| {
            let shown = ROWS
                .iter()
                .find(|(row_observation, row_attempt, _)| {
                    *row_observation == observation && *row_attempt == attempt
                })
                .map(|(_, _, shown)| *shown)
                .unwrap_or_else(|| panic!("missing row for {observation:?} × {attempt:?}"));
            assert_ne!(
                shown,
                LingerShown::NotApplicable,
                "not-applicable is a host without logind, not a read"
            );
            assert_eq!(
                show(observation, attempt),
                shown,
                "{observation:?} × {attempt:?}"
            );
        });
    }
}
