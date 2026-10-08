//! Names a D-Bus or polkit error becomes, before any `Display` string is read.
//!
//! Success is not a name. A name this table does not know is a failed call,
//! not a refusal, and a read this table does not know is unreadable. Neither
//! is a claim that linger is on.

use std::time::Duration;

use crate::linger::domain::{LingerCall, LingerObservation};

/// How long a linger read may take before logind is treated as unreachable.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `SetUserLinger` may wait. Stock systemd does not prompt for the
/// caller's own account; this bounds a distro policy that still does.
pub(crate) const ENABLE_TIMEOUT: Duration = Duration::from_secs(25);

/// What a finished call's error name means. Never [`LingerCall::Succeeded`].
pub(crate) fn classify_call_name(name: &str) -> LingerCall {
    match name {
        "org.freedesktop.PolicyKit1.Error.Cancelled"
        | "org.freedesktop.PolicyKit1.Error.NotAuthorized"
        | "org.freedesktop.PolicyKit1.Error.Failed"
        | "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired"
        | "org.freedesktop.DBus.Error.Timeout"
        | "org.freedesktop.DBus.Error.TimedOut"
        | "org.freedesktop.DBus.Error.NoReply" => LingerCall::Refused,
        _ => LingerCall::Failed,
    }
}

/// What a failed linger read's error name means. Never [`LingerObservation::Enabled`].
pub(crate) fn classify_read_name(name: &str) -> LingerObservation {
    match name {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.UnknownInterface" => LingerObservation::Unsupported,
        _ => LingerObservation::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_call_name, classify_read_name, ENABLE_TIMEOUT, READ_TIMEOUT};
    use crate::linger::domain::{LingerCall, LingerObservation};

    #[test]
    fn the_enable_wait_is_not_an_administrator_prompt() {
        assert_eq!(ENABLE_TIMEOUT, std::time::Duration::from_secs(25));
        assert!(ENABLE_TIMEOUT > READ_TIMEOUT);
        assert!(ENABLE_TIMEOUT < std::time::Duration::from_secs(60));
    }

    #[test]
    fn a_refusal_is_polkit_or_a_wait_that_ended_and_anything_else_failed() {
        assert_eq!(
            classify_call_name("org.freedesktop.PolicyKit1.Error.Cancelled"),
            LingerCall::Refused
        );
        assert_eq!(
            classify_call_name("org.freedesktop.DBus.Error.TimedOut"),
            LingerCall::Refused
        );
        assert_eq!(
            classify_call_name("org.freedesktop.login1.NoSuchUser"),
            LingerCall::Failed
        );
        assert_eq!(
            classify_call_name("org.freedesktop.DBus.Error.Failed"),
            LingerCall::Failed
        );
        assert_ne!(classify_call_name(""), LingerCall::Succeeded);
    }

    #[test]
    fn a_missing_logind_is_unsupported_and_any_other_read_is_unreadable() {
        assert_eq!(
            classify_read_name("org.freedesktop.DBus.Error.ServiceUnknown"),
            LingerObservation::Unsupported
        );
        assert_eq!(
            classify_read_name("org.freedesktop.login1.NoSuchUser"),
            LingerObservation::Unreadable
        );
        assert_ne!(classify_read_name(""), LingerObservation::Enabled);
    }
}
