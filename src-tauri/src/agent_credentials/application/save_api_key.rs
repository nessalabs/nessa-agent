use std::panic::{catch_unwind, AssertUnwindSafe};

use nessa_agent_credentials::{AgentCredential, AgentCredentialKind, CredentialAgent};

use crate::agent_credentials::domain::value_objects::{
    CredentialSaveCaller, CredentialSaveEffect, CredentialSaveIntent, CredentialSaveOutcome,
    CredentialSaveRefusal, CredentialSaveUncertainty,
};

use super::{AgentCredentialStore, CredentialSaveAudit, CredentialSaveIds, CredentialStoreFailure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSaveResult {
    Saved,
    SavedAuditFailed,
    Refused {
        failure: CredentialSaveRefusal,
        outcome_audit_failed: bool,
    },
    Uncertain {
        reason: CredentialSaveUncertainty,
        outcome_audit_failed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSaveAdmissionFailure {
    TargetUnavailable,
    TargetMismatch,
    AuditUnavailable,
}

pub fn save_api_key(
    store: &dyn AgentCredentialStore,
    ids: &dyn CredentialSaveIds,
    audit: &dyn CredentialSaveAudit,
    caller: CredentialSaveCaller,
    agent: CredentialAgent,
    candidate: Vec<u8>,
) -> Result<CredentialSaveResult, CredentialSaveAdmissionFailure> {
    let target = catch_unwind(AssertUnwindSafe(|| store.target(agent)))
        .map_err(|_| CredentialSaveAdmissionFailure::TargetUnavailable)?
        .map_err(|_| CredentialSaveAdmissionFailure::TargetUnavailable)?;
    if target.agent() != agent {
        return Err(CredentialSaveAdmissionFailure::TargetMismatch);
    }
    let correlation = catch_unwind(AssertUnwindSafe(|| ids.next()))
        .map_err(|_| CredentialSaveAdmissionFailure::AuditUnavailable)?
        .map_err(|_| CredentialSaveAdmissionFailure::AuditUnavailable)?;
    let intent = CredentialSaveIntent::new(correlation, caller, target);
    match catch_unwind(AssertUnwindSafe(|| audit.record_intent(&intent))) {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => return Err(CredentialSaveAdmissionFailure::AuditUnavailable),
    }

    let effect = match AgentCredential::new(AgentCredentialKind::ApiKey, candidate) {
        Err(_) => CredentialSaveEffect::Refused(CredentialSaveRefusal::Invalid),
        Ok(credential) => match catch_unwind(AssertUnwindSafe(|| {
            store.save_api_key(intent.target(), &credential)
        })) {
            Ok(Ok(())) => CredentialSaveEffect::Confirmed,
            Ok(Err(CredentialStoreFailure::Invalid)) => {
                CredentialSaveEffect::Refused(CredentialSaveRefusal::Invalid)
            }
            Ok(Err(CredentialStoreFailure::Unavailable)) => {
                CredentialSaveEffect::Uncertain(CredentialSaveUncertainty::StoreUnavailable)
            }
            Err(_) => CredentialSaveEffect::Uncertain(CredentialSaveUncertainty::StorePanicked),
        },
    };
    let outcome = CredentialSaveOutcome::new(intent, effect);
    let outcome_audit_failed = !matches!(
        catch_unwind(AssertUnwindSafe(|| audit.record_outcome(&outcome))),
        Ok(Ok(()))
    );

    Ok(match effect {
        CredentialSaveEffect::Confirmed if outcome_audit_failed => {
            CredentialSaveResult::SavedAuditFailed
        }
        CredentialSaveEffect::Confirmed => CredentialSaveResult::Saved,
        CredentialSaveEffect::Refused(failure) => CredentialSaveResult::Refused {
            failure,
            outcome_audit_failed,
        },
        CredentialSaveEffect::Uncertain(reason) => CredentialSaveResult::Uncertain {
            reason,
            outcome_audit_failed,
        },
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use nessa_agent_credentials::CredentialNamespace;

    use crate::agent_credentials::{
        application::CredentialSaveAuditFailure,
        domain::value_objects::{CredentialSaveCorrelation, CredentialSaveTarget},
    };

    use super::*;

    struct Ids;

    impl CredentialSaveIds for Ids {
        fn next(&self) -> Result<CredentialSaveCorrelation, CredentialSaveAuditFailure> {
            CredentialSaveCorrelation::parse("550e8400-e29b-41d4-a716-446655440000".into())
                .map_err(|_| CredentialSaveAuditFailure)
        }
    }

    #[derive(Default)]
    struct Store {
        calls: Mutex<usize>,
        failure: Option<CredentialStoreFailure>,
        panic_store: bool,
        resolved_agent: Option<CredentialAgent>,
    }

    impl AgentCredentialStore for Store {
        fn target(
            &self,
            agent: CredentialAgent,
        ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
            let agent = self.resolved_agent.unwrap_or(agent);
            let namespace = CredentialNamespace::new("prod".into(), None).unwrap();
            let account = match agent {
                CredentialAgent::Claude => "prod:claude-api-key",
                CredentialAgent::Opencode => "prod:opencode-api-key",
            };
            CredentialSaveTarget::new(
                agent,
                namespace,
                "so.nessa.agent-credentials".into(),
                account.into(),
            )
            .map_err(|_| CredentialStoreFailure::Invalid)
        }

        fn save_api_key(
            &self,
            _: &CredentialSaveTarget,
            _: &AgentCredential,
        ) -> Result<(), CredentialStoreFailure> {
            *self.calls.lock().unwrap() += 1;
            assert!(!self.panic_store, "substituted store panicked");
            self.failure.map_or(Ok(()), Err)
        }
    }

    #[derive(Default)]
    struct Audit {
        records: Mutex<Vec<&'static str>>,
        intents: Mutex<Vec<(CredentialAgent, String)>>,
        outcomes: Mutex<Vec<CredentialSaveEffect>>,
        fail_intent: bool,
        fail_outcome: bool,
        panic_intent: bool,
        panic_outcome: bool,
    }

    impl CredentialSaveAudit for Audit {
        fn record_intent(
            &self,
            intent: &CredentialSaveIntent,
        ) -> Result<(), CredentialSaveAuditFailure> {
            assert!(!self.panic_intent, "substituted intent audit panicked");
            self.records.lock().unwrap().push("intent");
            self.intents
                .lock()
                .unwrap()
                .push((intent.agent(), intent.target().account().to_owned()));
            if self.fail_intent {
                Err(CredentialSaveAuditFailure)
            } else {
                Ok(())
            }
        }

        fn record_outcome(
            &self,
            outcome: &CredentialSaveOutcome,
        ) -> Result<(), CredentialSaveAuditFailure> {
            assert!(!self.panic_outcome, "substituted outcome audit panicked");
            self.records.lock().unwrap().push("outcome");
            self.outcomes.lock().unwrap().push(outcome.effect());
            if self.fail_outcome {
                Err(CredentialSaveAuditFailure)
            } else {
                Ok(())
            }
        }
    }

    fn run(
        store: &Store,
        audit: &Audit,
    ) -> Result<CredentialSaveResult, CredentialSaveAdmissionFailure> {
        save_api_key(
            store,
            &Ids,
            audit,
            CredentialSaveCaller::Setup,
            CredentialAgent::Claude,
            b"private".to_vec(),
        )
    }

    #[test]
    fn intent_audit_failure_prevents_the_store_effect() {
        let store = Store::default();
        let audit = Audit {
            fail_intent: true,
            ..Audit::default()
        };
        assert_eq!(
            run(&store, &audit),
            Err(CredentialSaveAdmissionFailure::AuditUnavailable)
        );
        assert_eq!(*store.calls.lock().unwrap(), 0);
        assert_eq!(*audit.records.lock().unwrap(), vec!["intent"]);
    }

    #[test]
    fn intent_audit_panic_prevents_the_store_effect() {
        let store = Store::default();
        let audit = Audit {
            panic_intent: true,
            ..Audit::default()
        };

        assert_eq!(
            run(&store, &audit),
            Err(CredentialSaveAdmissionFailure::AuditUnavailable)
        );
        assert_eq!(*store.calls.lock().unwrap(), 0);
        assert!(audit.outcomes.lock().unwrap().is_empty());
    }

    #[test]
    fn requested_agent_must_match_the_resolved_target_before_admission() {
        let store = Store {
            resolved_agent: Some(CredentialAgent::Opencode),
            ..Store::default()
        };
        let audit = Audit::default();

        assert_eq!(
            run(&store, &audit),
            Err(CredentialSaveAdmissionFailure::TargetMismatch)
        );
        assert_eq!(*store.calls.lock().unwrap(), 0);
        assert!(audit.records.lock().unwrap().is_empty());
        assert!(audit.intents.lock().unwrap().is_empty());
    }

    #[test]
    fn each_supported_agent_keeps_its_canonical_target_in_the_intent() {
        let store = Store::default();
        let audit = Audit::default();

        for agent in [CredentialAgent::Claude, CredentialAgent::Opencode] {
            assert_eq!(
                save_api_key(
                    &store,
                    &Ids,
                    &audit,
                    CredentialSaveCaller::Setup,
                    agent,
                    b"private".to_vec(),
                ),
                Ok(CredentialSaveResult::Saved)
            );
        }
        assert_eq!(
            *audit.intents.lock().unwrap(),
            vec![
                (CredentialAgent::Claude, "prod:claude-api-key".into()),
                (CredentialAgent::Opencode, "prod:opencode-api-key".into()),
            ]
        );
    }

    #[test]
    fn invalid_candidates_are_refused_and_audited_without_reaching_the_store() {
        let store = Store::default();
        let audit = Audit::default();
        let result = save_api_key(
            &store,
            &Ids,
            &audit,
            CredentialSaveCaller::Setup,
            CredentialAgent::Claude,
            b"   ".to_vec(),
        );
        assert_eq!(
            result,
            Ok(CredentialSaveResult::Refused {
                failure: CredentialSaveRefusal::Invalid,
                outcome_audit_failed: false,
            })
        );
        assert_eq!(*store.calls.lock().unwrap(), 0);
        assert_eq!(*audit.records.lock().unwrap(), vec!["intent", "outcome"]);
    }

    #[test]
    fn confirmed_effect_survives_outcome_audit_failure() {
        let store = Store::default();
        let audit = Audit {
            fail_outcome: true,
            ..Audit::default()
        };
        assert_eq!(
            run(&store, &audit),
            Ok(CredentialSaveResult::SavedAuditFailed)
        );
        assert_eq!(*store.calls.lock().unwrap(), 1);
    }

    #[test]
    fn confirmed_effect_survives_outcome_audit_panic() {
        let store = Store::default();
        let audit = Audit {
            panic_outcome: true,
            ..Audit::default()
        };

        assert_eq!(
            run(&store, &audit),
            Ok(CredentialSaveResult::SavedAuditFailed)
        );
        assert_eq!(*store.calls.lock().unwrap(), 1);
    }

    #[test]
    fn unavailable_store_effect_is_uncertain_and_audited() {
        let store = Store {
            failure: Some(CredentialStoreFailure::Unavailable),
            ..Store::default()
        };
        let audit = Audit {
            fail_outcome: true,
            ..Audit::default()
        };
        assert_eq!(
            run(&store, &audit),
            Ok(CredentialSaveResult::Uncertain {
                reason: CredentialSaveUncertainty::StoreUnavailable,
                outcome_audit_failed: true,
            })
        );
        assert_eq!(*audit.records.lock().unwrap(), vec!["intent", "outcome"]);
        assert_eq!(
            *audit.outcomes.lock().unwrap(),
            vec![CredentialSaveEffect::Uncertain(
                CredentialSaveUncertainty::StoreUnavailable
            )]
        );
    }

    #[test]
    fn store_panic_is_an_uncertain_effect_with_a_correlated_outcome() {
        let store = Store {
            panic_store: true,
            ..Store::default()
        };
        let audit = Audit::default();

        assert_eq!(
            run(&store, &audit),
            Ok(CredentialSaveResult::Uncertain {
                reason: CredentialSaveUncertainty::StorePanicked,
                outcome_audit_failed: false,
            })
        );
        assert_eq!(*store.calls.lock().unwrap(), 1);
        assert_eq!(
            *audit.outcomes.lock().unwrap(),
            vec![CredentialSaveEffect::Uncertain(
                CredentialSaveUncertainty::StorePanicked
            )]
        );
    }
}
