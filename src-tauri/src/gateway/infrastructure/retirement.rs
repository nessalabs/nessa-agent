//! What a running gateway answered when it was asked to retire, and the one
//! rule for what that answer allows (ADR 221). Both native adapters ask the
//! same question through their own exchange; this is where the answer is
//! decided, so launchd and systemd cannot disagree about it.
use crate::gateway::application::{GatewayReconciliationProgress, ReconciliationHistoryFact};
use crate::gateway::domain::value_objects::RetirementRefusal;

/// Why a retirement request did not end in a retired gateway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::gateway::infrastructure) enum RetirementFailure {
    /// The gateway answered this very request, and refused it (ADR 221).
    Refused {
        refusal: RetirementRefusal,
        message: String,
    },
    /// No answer to act on: the request, the signal, or the wait failed.
    Unanswered(String),
}
impl std::fmt::Display for RetirementFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The refusal's name is part of the text the journal records for the
        // failed retirement step, so the durable record says why, not only the
        // plan the host chose next.
        match self {
            Self::Refused { refusal, message } => {
                write!(output, "{message} (refusal: {})", refusal.name())
            }
            Self::Unanswered(message) => output.write_str(message),
        }
    }
}
impl From<String> for RetirementFailure {
    fn from(message: String) -> Self {
        Self::Unanswered(message)
    }
}
impl From<&str> for RetirementFailure {
    fn from(message: &str) -> Self {
        Self::Unanswered(message.to_owned())
    }
}

/// Why the host may stop the old service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gateway::infrastructure) enum StopAllowedBy {
    /// It retired.
    Retirement,
    /// It refused because every agent it ran was stopped and only the record of
    /// it failed, since its data is gone. Nothing it owns is still running.
    RefusalDataMissing,
}

/// Record why the old service may be stopped, or hand back the failure that
/// ends the attempt with it preserved. Only a retirement, or a refusal naming
/// missing data, allows the stop; the history's fact order then accepts the
/// stop that follows.
pub(in crate::gateway::infrastructure) fn stop_allowed_after(
    progress: &dyn GatewayReconciliationProgress,
    service: &str,
    retired: Result<(), RetirementFailure>,
) -> Result<StopAllowedBy, RetirementFailure> {
    match retired {
        Ok(()) => {
            progress.history_observed(ReconciliationHistoryFact::RetirementAcknowledged);
            Ok(StopAllowedBy::Retirement)
        }
        Err(RetirementFailure::Refused {
            refusal: RetirementRefusal::DataMissing,
            message,
        }) => {
            eprintln!(
                "[nessa] Stopping gateway {service} itself: its agents are stopped but its conversation data is gone, so it cannot retire ({message})"
            );
            progress.history_observed(ReconciliationHistoryFact::RetirementRefusedDataMissing);
            Ok(StopAllowedBy::RefusalDataMissing)
        }
        Err(failure) => Err(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::application::{
        GatewayError, GatewayReconciliationIntent, ReconciliationHistoryFact,
    };
    use crate::gateway::domain::value_objects::{
        AuditDeliveryReceipt, LifecycleCommandResult, LifecycleObservation,
        LifecycleObservationSource, LifecyclePlanStep, ReconciliationIncarnation,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct Facts(Mutex<Vec<ReconciliationHistoryFact>>);

    impl GatewayReconciliationProgress for Facts {
        fn readiness_invalidated(&self) {}
        fn intent_admitted(&self, _: GatewayReconciliationIntent) -> Result<(), GatewayError> {
            Ok(())
        }
        fn history_observed(&self, fact: ReconciliationHistoryFact) {
            self.0.lock().unwrap().push(fact);
        }
        fn effect_planned(
            &self,
            _: &str,
            _: &LifecyclePlanStep,
            _: &[LifecyclePlanStep],
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            unreachable!("the decision plans nothing")
        }
        fn effect_completed(
            &self,
            _: &str,
            _: &str,
            _: &LifecycleCommandResult,
        ) -> Result<AuditDeliveryReceipt, GatewayError> {
            unreachable!("the decision completes nothing")
        }
        fn physical_observed(
            &self,
            _: &LifecycleObservationSource,
            _: Option<ReconciliationIncarnation>,
            _: bool,
        ) -> Result<LifecycleObservation, GatewayError> {
            unreachable!("the decision observes nothing")
        }
    }

    /// ADR 221: only a retirement, or a refusal naming missing data, lets the
    /// host stop the old gateway; every other answer preserves it and records
    /// nothing. The refusal's name is in the text the journal keeps.
    #[test]
    fn only_a_retirement_or_missing_data_allows_the_stop() {
        let answer = |retired| {
            let facts = Facts::default();
            let allowed = stop_allowed_after(&facts, "gui/501/so.nessa.gateway.prod", retired);
            (allowed, facts.0.into_inner().unwrap())
        };
        let refused = |refusal| RetirementFailure::Refused {
            refusal,
            message: "Gateway retirement was not acknowledged".into(),
        };

        assert_eq!(
            answer(Ok(())),
            (
                Ok(StopAllowedBy::Retirement),
                vec![ReconciliationHistoryFact::RetirementAcknowledged]
            )
        );
        assert_eq!(
            answer(Err(refused(RetirementRefusal::DataMissing))),
            (
                Ok(StopAllowedBy::RefusalDataMissing),
                vec![ReconciliationHistoryFact::RetirementRefusedDataMissing]
            )
        );
        for preserved in [
            refused(RetirementRefusal::NotConfirmed),
            RetirementFailure::Unanswered("acknowledgement timed out".into()),
        ] {
            let (allowed, facts) = answer(Err(preserved.clone()));
            assert_eq!(allowed, Err(preserved));
            assert!(facts.is_empty());
        }
        assert!(refused(RetirementRefusal::DataMissing)
            .to_string()
            .ends_with("(refusal: data_missing)"));
    }
}
