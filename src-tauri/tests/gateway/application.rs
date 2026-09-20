use super::*;
use crate::gateway::{
    application::{testing::FixedLoginShell, LoginShellError},
    domain::value_objects::SearchPathError,
};
use std::{path::Path, sync::Mutex};

fn login_shell(path: &str) -> Arc<FixedLoginShell> {
    Arc::new(FixedLoginShell(Ok(SearchPath::parse(path).unwrap())))
}

struct FakeHost {
    registration: Result<ReconciledGateway, GatewayError>,
    stop_result: Result<(), GatewayError>,
    calls: Mutex<Vec<String>>,
}
impl GatewayHost for FakeHost {
    fn register(
        &self,
        _: &Path,
        stage: &str,
        agent_path: Option<&SearchPath>,
    ) -> Result<ReconciledGateway, GatewayError> {
        self.calls.lock().unwrap().push(format!(
            "register:{stage}:{}",
            agent_path.map_or("-", SearchPath::as_str)
        ));
        self.registration.clone()
    }
    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("stop:{}", gateway.service()));
        self.stop_result.clone()
    }
}
#[test]
fn gateway_instances_use_only_their_injected_service_and_surface_stop_failure() {
    for identity in ["first", "second"] {
        let host = Arc::new(FakeHost {
            registration: Ok(reconciled(identity)),
            stop_result: Err(GatewayError::Stop("delivery failed".into())),
            calls: Mutex::new(vec![]),
        });
        let gateway = Gateway::bootstrap(
            host.clone(),
            login_shell("/opt/homebrew/bin:/usr/bin"),
            "/runtime".into(),
            "ci".into(),
        );
        tauri::async_runtime::block_on(gateway.wait_ready()).unwrap();
        assert_eq!(
            gateway.stop_agents(),
            Err(GatewayError::Stop("delivery failed".into()))
        );
        assert_eq!(
            *host.calls.lock().unwrap(),
            [
                "register:ci:/opt/homebrew/bin:/usr/bin".to_string(),
                format!("stop:{identity}")
            ]
        );
    }
}
#[test]
fn failed_registration_is_retried_and_never_derives_a_service_to_stop() {
    let host = Arc::new(FakeHost {
        registration: Err(GatewayError::Registration("not installed".into())),
        stop_result: Ok(()),
        calls: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/opt/homebrew/bin:/usr/bin"),
        "/runtime".into(),
        "ci".into(),
    );
    assert!(matches!(
        tauri::async_runtime::block_on(gateway.wait_ready()),
        Err(GatewayError::Registration(_))
    ));
    assert!(tauri::async_runtime::block_on(gateway.wait_ready()).is_err());
    assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));
    assert_eq!(
        *host.calls.lock().unwrap(),
        [
            "register:ci:/opt/homebrew/bin:/usr/bin",
            "register:ci:/opt/homebrew/bin:/usr/bin"
        ]
    );
}

#[test]
fn later_failed_reconciliation_revokes_stop_authority() {
    struct ChangingHost {
        registrations: Mutex<Vec<Result<ReconciledGateway, GatewayError>>>,
        calls: Mutex<Vec<String>>,
    }
    impl GatewayHost for ChangingHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
        ) -> Result<ReconciledGateway, GatewayError> {
            self.registrations.lock().unwrap().remove(0)
        }
        fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError> {
            self.calls
                .lock()
                .unwrap()
                .push(gateway.service().to_owned());
            Ok(())
        }
    }
    let host = Arc::new(ChangingHost {
        registrations: Mutex::new(vec![
            Ok(reconciled("gui/501/exact-reconciled")),
            Err(GatewayError::Registration("foreign service".into())),
        ]),
        calls: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(
        host.clone(),
        login_shell("/opt/homebrew/bin:/usr/bin"),
        "/runtime".into(),
        "ci".into(),
    );
    tauri::async_runtime::block_on(gateway.wait_ready()).unwrap();
    assert!(tauri::async_runtime::block_on(gateway.wait_ready()).is_err());
    assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));
    assert!(host.calls.lock().unwrap().is_empty());
}

/// A login shell that hangs, fails or answers with nonsense is not a reason to
/// leave the user without a gateway. The registration still happens; the host is
/// told there is no resolved path this launch, which is a different fact from
/// "the user has nothing on their path".
#[test]
fn a_login_shell_that_cannot_be_read_still_registers_the_service() {
    for failure in [
        LoginShellError::TimedOut,
        LoginShellError::Unavailable("No such file or directory".into()),
        LoginShellError::Rejected(SearchPathError::NoAbsoluteEntry),
    ] {
        let host = Arc::new(FakeHost {
            registration: Ok(reconciled("gui/501/so.nessa.gateway.ci")),
            stop_result: Ok(()),
            calls: Mutex::new(vec![]),
        });
        let gateway = Gateway::bootstrap(
            host.clone(),
            Arc::new(FixedLoginShell(Err(failure.clone()))),
            "/runtime".into(),
            "ci".into(),
        );
        tauri::async_runtime::block_on(gateway.wait_ready()).unwrap();
        assert_eq!(*host.calls.lock().unwrap(), ["register:ci:-"], "{failure}");
    }
}

/// Every failure names itself, because the fallback is only discoverable if it
/// is reported: the symptom otherwise is a tool that works in every terminal
/// and not in Nessa.
#[test]
fn every_login_shell_failure_can_be_reported() {
    for (failure, wording) in [
        (LoginShellError::TimedOut, "deadline"),
        (
            LoginShellError::Unavailable("no such shell".into()),
            "no such shell",
        ),
        (
            LoginShellError::Rejected(SearchPathError::Control),
            "control characters",
        ),
    ] {
        assert!(failure.to_string().contains(wording), "{failure:?}");
    }
}

fn reconciled(service: &str) -> ReconciledGateway {
    ReconciledGateway::new(
        service.into(),
        "a".repeat(64),
        "550e8400-e29b-41d4-a716-446655440000".into(),
        "b".repeat(64),
        42,
        7420,
    )
}
