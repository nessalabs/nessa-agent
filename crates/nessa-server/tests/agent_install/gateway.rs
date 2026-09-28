mod native_installs {
    use super::*;
    use crate::agent_install::{
        application::{
            AgentInstallations, GatewayInstallFailure, InstallFailure, InstallationOffer,
            InstalledRuntime, SourceFailure,
        },
        domain::{AgentName, InstallRequest, ReleaseVersion},
    };
    use std::sync::{atomic::AtomicUsize, mpsc};

    struct Installer {
        calls: AtomicUsize,
        requests: Mutex<Vec<InstallRequest>>,
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        wait: Mutex<Option<mpsc::Receiver<()>>>,
        fail: bool,
    }
    impl Installer {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                started: Mutex::new(None),
                wait: Mutex::new(None),
                fail: false,
            }
        }
    }
    impl AgentInstallations for Installer {
        fn account_id(&self) -> &str {
            "unix:test"
        }
        fn offers(&self) -> Result<Vec<InstallationOffer>, GatewayInstallFailure> {
            Ok(vec![])
        }
        fn install(
            &self,
            _agent: &AgentName,
            request: &InstallRequest,
        ) -> Result<InstalledRuntime, GatewayInstallFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(request.clone());
            if let Some(started) = self.started.lock().unwrap().take() {
                let _ = started.send(());
            }
            if let Some(wait) = self.wait.lock().unwrap().take() {
                wait.recv().unwrap();
            }
            if self.fail {
                return Err(GatewayInstallFailure::Install(Box::new(
                    InstallFailure::Download(SourceFailure::Unreachable("offline".into())),
                )));
            }
            Ok(InstalledRuntime {
                version: ReleaseVersion::parse("1.0.0").unwrap(),
                executable: "/managed/native".into(),
                downloaded: true,
                reclamation_warnings: vec![],
            })
        }
    }

    #[tokio::test]
    async fn install_requires_write_authority_and_records_the_verified_caller() {
        let installer = Arc::new(Installer::new());
        let state = gateway::chat_state().with_agent_installations(installer.clone());
        for credential in ["reader", "foreign"] {
            let session = gateway::chat_session(&state, credential).await;
            let response = gateway::chat_request(
                &state,
                &session,
                "agents.install",
                json!({"agent":"claude", "requestId":"click"}),
            )
            .await;
            assert_eq!(response.error.unwrap().code, "forbidden");
        }
        assert_eq!(installer.calls.load(Ordering::SeqCst), 0);
        let session = gateway::chat_session(&state, "owner-panel").await;
        let response = gateway::chat_request(
            &state,
            &session,
            "agents.install",
            json!({"agent":"claude", "requestId":"click"}),
        )
        .await;
        assert!(response.error.is_none());
        let requests = installer.requests.lock().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(requests[0].request_id()).unwrap(),
            json!([
                "gateway",
                "claude",
                "organization",
                "owner",
                "owner-panel",
                "click"
            ])
        );
        assert_eq!(requests[0].account_id(), "unix:test");
    }

    #[tokio::test]
    async fn lost_install_waiter_keeps_capacity_until_the_worker_finishes() {
        let mut installer = Installer::new();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        *installer.started.get_mut().unwrap() = Some(started);
        let (finish, wait) = mpsc::channel();
        *installer.wait.get_mut().unwrap() = Some(wait);
        let installer = Arc::new(installer);
        let state = gateway::chat_state().with_agent_installations(installer.clone());
        let session = gateway::chat_session(&state, "owner-panel").await;
        let waiting_state = state.clone();
        let waiting_session = session.clone();
        let waiting = tokio::spawn(async move {
            gateway::chat_request(
                &waiting_state,
                &waiting_session,
                "agents.install",
                json!({"agent":"claude", "requestId":"first"}),
            )
            .await
        });
        started_rx.await.unwrap();
        waiting.abort();
        let _ = waiting.await;
        let busy = gateway::chat_request(
            &state,
            &session,
            "agents.install",
            json!({"agent":"codex", "requestId":"second"}),
        )
        .await;
        assert_eq!(busy.error.unwrap().code, "agent_install_busy");
        let health = gateway::chat_request(&state, &session, "server.health", json!({})).await;
        assert!(health.error.is_none());
        finish.send(()).unwrap();
        let permit = timeout(
            Duration::from_secs(5),
            state.installs.clone().acquire_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        drop(permit);
        assert_eq!(installer.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_download_reports_a_typed_failure_and_releases_capacity() {
        let mut installer = Installer::new();
        installer.fail = true;
        let state = gateway::chat_state().with_agent_installations(Arc::new(installer));
        let session = gateway::chat_session(&state, "owner-panel").await;
        let response = gateway::chat_request(
            &state,
            &session,
            "agents.install",
            json!({"agent":"claude", "requestId":"first"}),
        )
        .await;
        assert_eq!(response.error.unwrap().code, "agent_download_failed");
        assert_eq!(state.installs.available_permits(), 1);
    }
    #[tokio::test]
    async fn credential_rotation_changes_attribution_without_changing_installation_owner() {
        let installer = Arc::new(Installer::new());
        let state = gateway::chat_state().with_agent_installations(installer.clone());
        for credential in ["owner-panel", "owner-phone"] {
            let session = gateway::chat_session(&state, credential).await;
            let answer = gateway::chat_request(
                &state,
                &session,
                "agents.install",
                json!({"agent":"claude", "requestId":"same-click"}),
            )
            .await;
            assert!(answer.error.is_none());
        }
        let requests = installer.requests.lock().unwrap();
        assert_eq!(requests[0].account_id(), requests[1].account_id());
        assert_ne!(requests[0].request_id(), requests[1].request_id());
    }

    #[test]
    fn escaped_authenticated_invocations_fit_without_widening_account_identity() {
        let id = "\\".repeat(256);
        let invocation =
            json!(["gateway", "claude", id, id, id, "\u{0000}".repeat(256)]).to_string();
        let request = InstallRequest::new("unix:test", invocation.clone()).unwrap();
        assert_eq!(request.request_id(), invocation);
        assert!(InstallRequest::new("a".repeat(256), "click").is_err());
        assert!(InstallRequest::new("unix:test", "x".repeat(4097)).is_err());
    }
}
