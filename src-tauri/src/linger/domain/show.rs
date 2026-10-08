//! The screen for one logind read, and for the call an explicit accept made.
//!
//! The table is [ADR 217](../../../../../docs/adr/done/217-linux-linger-at-setup.md).
//! `enabled` is the only claim that the gateway keeps running after logout,
//! and it is returned only for a read that says linger is on.

/// The account logind was asked about.
#[cfg(any(test, target_os = "linux"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LoginUserId(u32);

#[cfg(any(test, target_os = "linux"))]
impl LoginUserId {
    pub(crate) fn new(uid: u32) -> Self {
        Self(uid)
    }

    pub(crate) fn get(self) -> u32 {
        self.0
    }
}

/// What the latest read said about this user's linger.
#[cfg(any(test, target_os = "linux"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerObservation {
    /// logind's `Linger` property is true, or the linger file is present.
    Enabled,
    /// The read answered and linger is off.
    Disabled,
    /// The system bus cannot be reached, or logind has no owner.
    Unsupported,
    /// logind answered and linger could not be read.
    Unreadable,
}

/// How a finished `SetUserLinger` came back, before the confirming read.
#[cfg(any(test, target_os = "linux"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerCall {
    Succeeded,
    /// polkit refused the change, or the wait ended with no reply.
    Refused,
    /// The call failed for some other reason. Not a refusal.
    Failed,
}

/// What setup shows. `not-applicable` is a host with no logind API.
/// The read-and-call function never returns that variant.
#[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LingerShown {
    /// This host has no logind API. Setup does not ask.
    #[cfg(any(test, not(target_os = "linux")))]
    NotApplicable,
    /// Linger is off and the person has not chosen.
    Offer,
    /// The read says linger is on.
    Enabled,
    /// The call was refused and the read still says linger is off.
    Refused,
    /// The call did not leave linger on, and it was not a refusal.
    Failed,
    /// There is no logind.
    Unsupported,
}

/// One read: which account, and what it said.
#[cfg(any(test, target_os = "linux"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LingerSnapshot {
    user: LoginUserId,
    observation: LingerObservation,
}

#[cfg(any(test, target_os = "linux"))]
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

/// The screen for this read and, if an accept just returned, that call.
///
/// `enabled` is returned only when `observation` is [`LingerObservation::Enabled`].
/// A successful call cannot promote a read that still says linger is off.
#[cfg(any(test, target_os = "linux"))]
#[must_use]
pub(crate) fn show(observation: LingerObservation, call: Option<LingerCall>) -> LingerShown {
    match observation {
        LingerObservation::Enabled => LingerShown::Enabled,
        LingerObservation::Unsupported => LingerShown::Unsupported,
        LingerObservation::Unreadable => LingerShown::Failed,
        LingerObservation::Disabled => match call {
            None => LingerShown::Offer,
            Some(LingerCall::Refused) => LingerShown::Refused,
            Some(LingerCall::Succeeded | LingerCall::Failed) => LingerShown::Failed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{show, LingerCall, LingerObservation, LingerShown};

    const ROWS: &[(LingerObservation, Option<LingerCall>, LingerShown)] = &[
        (Enabled, None, EnabledShown),
        (Enabled, Some(Succeeded), EnabledShown),
        (Enabled, Some(Refused), EnabledShown),
        (Enabled, Some(Failed), EnabledShown),
        (Unsupported, None, UnsupportedShown),
        (Unsupported, Some(Succeeded), UnsupportedShown),
        (Unsupported, Some(Refused), UnsupportedShown),
        (Unsupported, Some(Failed), UnsupportedShown),
        (Unreadable, None, FailedShown),
        (Unreadable, Some(Succeeded), FailedShown),
        (Unreadable, Some(Refused), FailedShown),
        (Unreadable, Some(Failed), FailedShown),
        (Disabled, None, Offer),
        (Disabled, Some(Refused), RefusedShown),
        (Disabled, Some(Succeeded), FailedShown),
        (Disabled, Some(Failed), FailedShown),
    ];

    use LingerCall::{Failed, Refused, Succeeded};
    use LingerObservation::{Disabled, Enabled, Unreadable, Unsupported};
    use LingerShown::{
        Enabled as EnabledShown, Failed as FailedShown, Offer, Refused as RefusedShown,
        Unsupported as UnsupportedShown,
    };

    fn each_pair(mut visit: impl FnMut(LingerObservation, Option<LingerCall>)) {
        for observation in [Enabled, Disabled, Unsupported, Unreadable] {
            match observation {
                Enabled => {}
                Disabled => {}
                Unsupported => {}
                Unreadable => {}
            }
            for call in [None, Some(Succeeded), Some(Refused), Some(Failed)] {
                match call {
                    None => {}
                    Some(Succeeded) => {}
                    Some(Refused) => {}
                    Some(Failed) => {}
                }
                visit(observation, call);
            }
        }
    }

    #[test]
    fn every_read_and_call_shows_the_decided_row() {
        assert_eq!(ROWS.len(), 16, "ADR 217 has one row per read × call");
        each_pair(|observation, call| {
            let shown = ROWS
                .iter()
                .find(|(row_observation, row_call, _)| {
                    *row_observation == observation && *row_call == call
                })
                .map(|(_, _, shown)| *shown)
                .unwrap_or_else(|| panic!("missing row for {observation:?} × {call:?}"));
            assert_eq!(show(observation, call), shown, "{observation:?} × {call:?}");
        });
    }
}
