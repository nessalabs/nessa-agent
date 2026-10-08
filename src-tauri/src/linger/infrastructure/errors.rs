//! Names a D-Bus or polkit error becomes, before any `Display` string is read.
//!
//! Success is not a name. A name this table does not know is not authorized,
//! and a read this table does not know is unreadable. Neither is a claim that
//! linger is on.

use std::time::Duration;

use crate::linger::domain::{LingerCall, LingerObservation};

/// How long a linger read may take before logind is treated as unreachable.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an interactive `SetUserLinger` may wait for an administrator.
pub(crate) const ENABLE_TIMEOUT: Duration = Duration::from_secs(180);

/// The only arguments `SetUserLinger` is called with: this user, enable, interactive.
///
/// There is no parameter for the enable flag. A call that turns linger off is
/// not representable here.
pub(crate) fn enable_arguments(uid: u32) -> (u32, bool, bool) {
    (uid, true, true)
}

/// What a finished call's error name means. Never [`LingerCall::Succeeded`].
pub(crate) fn classify_call_name(name: &str) -> LingerCall {
    match name {
        "org.freedesktop.PolicyKit1.Error.Cancelled" => LingerCall::Cancelled,
        "org.freedesktop.PolicyKit1.Error.NotAuthorized"
        | "org.freedesktop.PolicyKit1.Error.Failed"
        | "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" => {
            LingerCall::NotAuthorized
        }
        "org.freedesktop.DBus.Error.Timeout"
        | "org.freedesktop.DBus.Error.TimedOut"
        | "org.freedesktop.DBus.Error.NoReply" => LingerCall::TimedOut,
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.PolicyKit1.Error.NotSupported" => LingerCall::Unavailable,
        _ => LingerCall::NotAuthorized,
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

/// The system bus could not be opened. There is no logind to ask.
pub(crate) fn bus_unavailable() -> LingerObservation {
    LingerObservation::Unsupported
}

#[cfg(test)]
mod tests {
    use super::{
        bus_unavailable, classify_call_name, classify_read_name, enable_arguments, ENABLE_TIMEOUT,
        READ_TIMEOUT,
    };
    use crate::linger::domain::{LingerCall, LingerObservation};

    #[test]
    fn the_only_call_enables_interactively() {
        let (uid, enable, interactive) = enable_arguments(1000);
        assert_eq!(uid, 1000);
        assert!(enable);
        assert!(interactive);
        assert!(ENABLE_TIMEOUT >= std::time::Duration::from_secs(60));
        assert!(ENABLE_TIMEOUT > READ_TIMEOUT);
    }

    #[test]
    fn a_call_name_is_never_success() {
        let names = [
            "org.freedesktop.PolicyKit1.Error.Cancelled",
            "org.freedesktop.PolicyKit1.Error.NotAuthorized",
            "org.freedesktop.PolicyKit1.Error.Failed",
            "org.freedesktop.PolicyKit1.Error.NotSupported",
            "org.freedesktop.DBus.Error.AccessDenied",
            "org.freedesktop.DBus.Error.AuthFailed",
            "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
            "org.freedesktop.DBus.Error.Timeout",
            "org.freedesktop.DBus.Error.TimedOut",
            "org.freedesktop.DBus.Error.NoReply",
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.freedesktop.DBus.Error.UnknownMethod",
            "org.freedesktop.DBus.Error.UnknownInterface",
            "",
            "ok",
            "succeeded",
        ];
        for name in names {
            assert_ne!(
                classify_call_name(name),
                LingerCall::Succeeded,
                "{name} is not a successful call"
            );
        }
        assert_eq!(
            classify_call_name("org.freedesktop.PolicyKit1.Error.Cancelled"),
            LingerCall::Cancelled
        );
        assert_eq!(
            classify_call_name("org.freedesktop.DBus.Error.TimedOut"),
            LingerCall::TimedOut
        );
        assert_eq!(
            classify_call_name("org.freedesktop.DBus.Error.ServiceUnknown"),
            LingerCall::Unavailable
        );
        assert_eq!(
            classify_call_name("org.freedesktop.PolicyKit1.Error.NotSupported"),
            LingerCall::Unavailable
        );
        assert_eq!(
            classify_call_name("org.freedesktop.login1.NoSuchUser"),
            LingerCall::NotAuthorized
        );
    }

    #[test]
    fn a_read_name_is_unsupported_or_unreadable_and_never_on() {
        for name in [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.freedesktop.DBus.Error.UnknownMethod",
            "org.freedesktop.DBus.Error.UnknownInterface",
        ] {
            assert_eq!(classify_read_name(name), LingerObservation::Unsupported);
        }
        assert_eq!(
            classify_read_name("org.freedesktop.login1.NoSuchUser"),
            LingerObservation::Unreadable
        );
        assert_eq!(classify_read_name(""), LingerObservation::Unreadable);
        assert_eq!(bus_unavailable(), LingerObservation::Unsupported);
        assert_ne!(bus_unavailable(), LingerObservation::Enabled);
    }
}
