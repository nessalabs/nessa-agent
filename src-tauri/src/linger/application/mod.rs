//! The setup choice: read logind, and call it only after an explicit accept.
//!
//! ```text
//! status ──▶ read ──▶ show
//! decline ─▶ read ──▶ decline record, when linger is off and nothing is in flight
//! accept ──▶ read ──▶ intent record ──▶ SetUserLinger ──▶ read ──▶ outcome record
//! ```
//!
//! The attempt lock is not held across `SetUserLinger`. A second accept sees
//! the call in flight and does not send another. A new session has
//! no attempt, which is what a process that quit mid-prompt comes back as.

#[cfg(any(test, not(target_os = "linux")))]
mod not_applicable;
#[cfg(any(test, target_os = "linux"))]
mod ports;
mod session;

#[cfg(any(test, not(target_os = "linux")))]
pub(crate) use not_applicable::NotApplicableLinger;
#[cfg(any(test, target_os = "linux"))]
pub(crate) use ports::{LingerAudit, LingerAuditFailure, LogindLinger};
#[cfg(any(test, target_os = "linux"))]
pub(crate) use session::LingerSession;
pub(crate) use session::{AuditDelivery, LingerOffer, LingerView};

#[cfg(test)]
mod tests {
    use crate::linger::domain::LingerShown;

    use super::{LingerOffer, NotApplicableLinger};

    #[test]
    fn not_this_host_never_reports_a_logind_claim() {
        let offer = NotApplicableLinger;
        for view in [offer.status(), offer.accept(), offer.decline()] {
            assert_eq!(view.shown(), LingerShown::NotApplicable);
        }
    }
}
