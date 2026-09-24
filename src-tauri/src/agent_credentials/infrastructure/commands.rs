//! Trusted bundled-window command for saving an agent API key.

use std::sync::Arc;

use nessa_agent_credentials::CredentialAgent;
use serde::Serialize;
use tauri::{State, WebviewWindow};

use crate::{
    agent_credentials::{
        application::{
            save_api_key, AgentCredentialStore, CredentialSaveAdmissionFailure,
            CredentialSaveAudit, CredentialSaveIds, CredentialSaveResult, CredentialSaveTargets,
        },
        domain::value_objects::{CredentialSaveCaller, CredentialSaveRefusal},
    },
    composition::HostDependencies,
    panel,
};

struct SaveRequest {
    caller: CredentialSaveCaller,
    agent: CredentialAgent,
    candidate: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentApiKeyResponse {
    status: SaveStatus,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SaveStatus {
    Saved,
    SavedAuditFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveredAuditStatus {
    Recorded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditStatus {
    Recorded,
    Failed,
    Unknown,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum SaveAgentApiKeyFailure {
    UntrustedCaller,
    UnsupportedAgent,
    InvalidCredential {
        #[serde(rename = "auditStatus")]
        audit_status: DeliveredAuditStatus,
    },
    StoreUnavailable,
    AuditUnavailable,
    SaveUncertain {
        #[serde(rename = "auditStatus")]
        audit_status: AuditStatus,
    },
}

fn request(label: &str, agent: &str, key: String) -> Result<SaveRequest, SaveAgentApiKeyFailure> {
    let caller = match label {
        panel::MAIN_WINDOW => CredentialSaveCaller::Main,
        panel::SETUP_WINDOW => CredentialSaveCaller::Setup,
        _ => return Err(SaveAgentApiKeyFailure::UntrustedCaller),
    };
    let agent = match agent {
        "claude" => CredentialAgent::Claude,
        "opencode" => CredentialAgent::Opencode,
        _ => return Err(SaveAgentApiKeyFailure::UnsupportedAgent),
    };
    Ok(SaveRequest {
        caller,
        agent,
        candidate: key.into_bytes(),
    })
}

fn save(
    store: &dyn AgentCredentialStore,
    targets: &dyn CredentialSaveTargets,
    ids: &dyn CredentialSaveIds,
    audit: &dyn CredentialSaveAudit,
    request: SaveRequest,
) -> Result<SaveAgentApiKeyResponse, SaveAgentApiKeyFailure> {
    match save_api_key(
        store,
        targets,
        ids,
        audit,
        request.caller,
        request.agent,
        request.candidate,
    ) {
        Ok(CredentialSaveResult::Saved) => Ok(SaveAgentApiKeyResponse {
            status: SaveStatus::Saved,
        }),
        Ok(CredentialSaveResult::SavedAuditFailed) => Ok(SaveAgentApiKeyResponse {
            status: SaveStatus::SavedAuditFailed,
        }),
        Ok(CredentialSaveResult::Refused {
            failure: CredentialSaveRefusal::Invalid,
            outcome_audit_failed,
        }) => Err(SaveAgentApiKeyFailure::InvalidCredential {
            audit_status: delivered(outcome_audit_failed),
        }),
        Ok(CredentialSaveResult::Uncertain {
            outcome_audit_failed,
            ..
        }) => Err(SaveAgentApiKeyFailure::SaveUncertain {
            audit_status: observed(outcome_audit_failed),
        }),
        Err(
            CredentialSaveAdmissionFailure::TargetUnavailable
            | CredentialSaveAdmissionFailure::TargetMismatch,
        ) => Err(SaveAgentApiKeyFailure::StoreUnavailable),
        Err(CredentialSaveAdmissionFailure::AuditUnavailable) => {
            Err(SaveAgentApiKeyFailure::AuditUnavailable)
        }
    }
}

fn delivered(failed: bool) -> DeliveredAuditStatus {
    match failed {
        true => DeliveredAuditStatus::Failed,
        false => DeliveredAuditStatus::Recorded,
    }
}

fn observed(failed: bool) -> AuditStatus {
    match failed {
        true => AuditStatus::Failed,
        false => AuditStatus::Recorded,
    }
}

fn spawn_save(
    store: Arc<dyn AgentCredentialStore>,
    targets: Arc<dyn CredentialSaveTargets>,
    ids: Arc<dyn CredentialSaveIds>,
    audit: Arc<dyn CredentialSaveAudit>,
    request: SaveRequest,
) -> tauri::async_runtime::JoinHandle<Result<SaveAgentApiKeyResponse, SaveAgentApiKeyFailure>> {
    tauri::async_runtime::spawn_blocking(move || {
        save(
            store.as_ref(),
            targets.as_ref(),
            ids.as_ref(),
            audit.as_ref(),
            request,
        )
    })
}

/// Save one validated API key without exposing a storage name or diagnostic.
#[tauri::command]
pub async fn save_agent_api_key(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    agent: String,
    key: String,
) -> Result<SaveAgentApiKeyResponse, SaveAgentApiKeyFailure> {
    let request = request(window.label(), &agent, key)?;
    let store = deps.agent_credentials.clone();
    let targets = deps.credential_save_targets.clone();
    let ids = deps.credential_save_ids.clone();
    let audit = deps.credential_save_audit.clone();
    spawn_save(store, targets, ids, audit, request)
        .await
        .map_err(|_| SaveAgentApiKeyFailure::SaveUncertain {
            audit_status: AuditStatus::Unknown,
        })?
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{mpsc, Arc, Condvar, Mutex},
        time::Duration,
    };

    use nessa_agent_credentials::{AgentCredential, CredentialNamespace};

    use crate::agent_credentials::{
        application::CredentialSaveAuditFailure,
        domain::value_objects::{
            CredentialSaveCorrelation, CredentialSaveEffect, CredentialSaveIntent,
            CredentialSaveOutcome, CredentialSaveTarget,
        },
    };

    use super::*;
    use crate::agent_credentials::application::CredentialStoreFailure;

    #[derive(Default)]
    struct Store(Mutex<Vec<(CredentialAgent, String)>>);

    impl AgentCredentialStore for Store {
        fn target(
            &self,
            agent: CredentialAgent,
        ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
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
            target: &CredentialSaveTarget,
            credential: &AgentCredential,
        ) -> Result<(), CredentialStoreFailure> {
            self.0
                .lock()
                .unwrap()
                .push((target.agent(), credential.expose().to_owned()));
            Ok(())
        }
    }

    impl CredentialSaveTargets for Store {
        fn target(
            &self,
            agent: CredentialAgent,
        ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
            AgentCredentialStore::target(self, agent)
        }
    }

    struct Ids;

    impl CredentialSaveIds for Ids {
        fn next(&self) -> Result<CredentialSaveCorrelation, CredentialSaveAuditFailure> {
            CredentialSaveCorrelation::parse("550e8400-e29b-41d4-a716-446655440000".into())
                .map_err(|_| CredentialSaveAuditFailure)
        }
    }

    #[derive(Default)]
    struct Audit {
        records: Mutex<Vec<&'static str>>,
        fail_outcome: bool,
    }

    impl CredentialSaveAudit for Audit {
        fn record_intent(
            &self,
            _: &CredentialSaveIntent,
        ) -> Result<(), CredentialSaveAuditFailure> {
            self.records.lock().unwrap().push("intent");
            Ok(())
        }

        fn record_outcome(
            &self,
            _: &CredentialSaveOutcome,
        ) -> Result<(), CredentialSaveAuditFailure> {
            self.records.lock().unwrap().push("outcome");
            match self.fail_outcome {
                true => Err(CredentialSaveAuditFailure),
                false => Ok(()),
            }
        }
    }

    #[test]
    fn only_bundled_surfaces_and_supported_agents_form_requests() {
        assert_eq!(
            request("external", "claude", "secret".into()).err(),
            Some(SaveAgentApiKeyFailure::UntrustedCaller)
        );
        assert_eq!(
            request(panel::SETUP_WINDOW, "codex", "secret".into()).err(),
            Some(SaveAgentApiKeyFailure::UnsupportedAgent)
        );
        assert!(request(panel::MAIN_WINDOW, "claude", "secret".into()).is_ok());
        assert!(request(panel::SETUP_WINDOW, "opencode", "secret".into()).is_ok());
    }

    #[test]
    fn verified_caller_and_selected_target_are_audited_around_the_store() {
        let store = Store::default();
        let audit = Audit::default();
        let candidate = request(panel::SETUP_WINDOW, "claude", "private".into()).unwrap();

        let response = save(&store, &store, &Ids, &audit, candidate).unwrap();

        assert!(matches!(response.status, SaveStatus::Saved));
        assert_eq!(
            store.0.lock().unwrap().as_slice(),
            &[(CredentialAgent::Claude, "private".into())]
        );
        assert_eq!(*audit.records.lock().unwrap(), vec!["intent", "outcome"]);
    }

    #[test]
    fn invalid_candidate_is_a_definite_refusal_with_its_audit_status() {
        let store = Store::default();
        let audit = Audit {
            fail_outcome: true,
            ..Audit::default()
        };
        let candidate = request(panel::SETUP_WINDOW, "opencode", "   ".into()).unwrap();

        assert_eq!(
            save(&store, &store, &Ids, &audit, candidate),
            Err(SaveAgentApiKeyFailure::InvalidCredential {
                audit_status: DeliveredAuditStatus::Failed,
            })
        );
        assert!(store.0.lock().unwrap().is_empty());
        assert_eq!(*audit.records.lock().unwrap(), vec!["intent", "outcome"]);
    }

    #[test]
    fn confirmed_save_with_failed_audit_keeps_its_successful_effect() {
        let store = Store::default();
        let audit = Audit {
            fail_outcome: true,
            ..Audit::default()
        };
        let candidate = request(panel::SETUP_WINDOW, "claude", "private".into()).unwrap();

        assert_eq!(
            save(&store, &store, &Ids, &audit, candidate),
            Ok(SaveAgentApiKeyResponse {
                status: SaveStatus::SavedAuditFailed,
            })
        );
        assert_eq!(store.0.lock().unwrap().len(), 1);
        assert_eq!(*audit.records.lock().unwrap(), vec!["intent", "outcome"]);
    }

    #[test]
    fn command_results_have_one_discriminated_transport_shape() {
        assert_eq!(
            serde_json::to_value(SaveAgentApiKeyFailure::UnsupportedAgent).unwrap(),
            serde_json::json!({ "status": "unsupported-agent" })
        );
        assert_eq!(
            serde_json::to_value(SaveAgentApiKeyResponse {
                status: SaveStatus::SavedAuditFailed,
            })
            .unwrap(),
            serde_json::json!({ "status": "saved-audit-failed" })
        );
        assert_eq!(
            serde_json::to_value(SaveAgentApiKeyFailure::InvalidCredential {
                audit_status: DeliveredAuditStatus::Failed,
            })
            .unwrap(),
            serde_json::from_str::<serde_json::Value>(include_str!(
                "fixtures/invalid-credential-audit-failed.json"
            ))
            .unwrap()
        );
        assert_eq!(
            serde_json::to_value(SaveAgentApiKeyFailure::SaveUncertain {
                audit_status: AuditStatus::Unknown,
            })
            .unwrap(),
            serde_json::json!({ "status": "save-uncertain", "auditStatus": "unknown" })
        );
    }

    struct GatedStore {
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: Mutex<Option<mpsc::Sender<()>>>,
    }

    impl AgentCredentialStore for GatedStore {
        fn target(
            &self,
            agent: CredentialAgent,
        ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
            AgentCredentialStore::target(&Store::default(), agent)
        }

        fn save_api_key(
            &self,
            _: &CredentialSaveTarget,
            _: &AgentCredential,
        ) -> Result<(), CredentialStoreFailure> {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            let (lock, wake) = &*self.gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            Ok(())
        }
    }

    impl CredentialSaveTargets for GatedStore {
        fn target(
            &self,
            agent: CredentialAgent,
        ) -> Result<CredentialSaveTarget, CredentialStoreFailure> {
            AgentCredentialStore::target(self, agent)
        }
    }

    struct ReportingAudit {
        outcome: Mutex<Option<mpsc::Sender<(String, CredentialSaveEffect)>>>,
        fail_outcome: bool,
    }

    impl CredentialSaveAudit for ReportingAudit {
        fn record_intent(
            &self,
            _: &CredentialSaveIntent,
        ) -> Result<(), CredentialSaveAuditFailure> {
            Ok(())
        }

        fn record_outcome(
            &self,
            outcome: &CredentialSaveOutcome,
        ) -> Result<(), CredentialSaveAuditFailure> {
            if let Some(report) = self.outcome.lock().unwrap().take() {
                let _ = report.send((
                    outcome.intent().correlation().as_str().to_owned(),
                    outcome.effect(),
                ));
            }
            match self.fail_outcome {
                true => Err(CredentialSaveAuditFailure),
                false => Ok(()),
            }
        }
    }

    #[test]
    fn production_supervisor_settles_audited_work_after_response_abort() {
        for fail_outcome in [false, true] {
            let gate = Arc::new((Mutex::new(false), Condvar::new()));
            let (entered, admitted) = mpsc::channel();
            let (reported, outcome) = mpsc::channel();
            let store: Arc<dyn AgentCredentialStore> = Arc::new(GatedStore {
                gate: gate.clone(),
                entered: Mutex::new(Some(entered)),
            });
            let targets: Arc<dyn CredentialSaveTargets> = Arc::new(Store::default());
            let audit: Arc<dyn CredentialSaveAudit> = Arc::new(ReportingAudit {
                outcome: Mutex::new(Some(reported)),
                fail_outcome,
            });
            let ids: Arc<dyn CredentialSaveIds> = Arc::new(Ids);
            let request = request(panel::SETUP_WINDOW, "claude", "private".into()).unwrap();

            let response = spawn_save(store, targets, ids, audit, request);
            admitted
                .recv_timeout(Duration::from_secs(5))
                .expect("the production blocking supervisor admitted the store effect");
            response.abort();
            drop(response);
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();

            assert_eq!(
                outcome
                    .recv_timeout(Duration::from_secs(5))
                    .expect("admitted work must attempt its correlated outcome audit"),
                (
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                    CredentialSaveEffect::Confirmed,
                )
            );
        }
    }
}
