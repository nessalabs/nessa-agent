use super::*;
use std::{path::Path, sync::Mutex};
struct FakeHost {
    registration: Result<ReconciledGateway, GatewayError>,
    stop_result: Result<(), GatewayError>,
    calls: Mutex<Vec<String>>,
}
impl GatewayHost for FakeHost {
    fn register(&self, _: &Path, stage: &str) -> Result<ReconciledGateway, GatewayError> {
        self.calls.lock().unwrap().push(format!("register:{stage}"));
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
        let gateway = Gateway::bootstrap(host.clone(), "/runtime".into(), "ci".into());
        tauri::async_runtime::block_on(gateway.wait_ready()).unwrap();
        assert_eq!(
            gateway.stop_agents(),
            Err(GatewayError::Stop("delivery failed".into()))
        );
        assert_eq!(
            *host.calls.lock().unwrap(),
            ["register:ci".to_string(), format!("stop:{identity}")]
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
    let gateway = Gateway::bootstrap(host.clone(), "/runtime".into(), "ci".into());
    assert!(matches!(
        tauri::async_runtime::block_on(gateway.wait_ready()),
        Err(GatewayError::Registration(_))
    ));
    assert!(tauri::async_runtime::block_on(gateway.wait_ready()).is_err());
    assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));
    assert_eq!(*host.calls.lock().unwrap(), ["register:ci", "register:ci"]);
}

#[test]
fn later_failed_reconciliation_revokes_stop_authority() {
    struct ChangingHost {
        registrations: Mutex<Vec<Result<ReconciledGateway, GatewayError>>>,
        calls: Mutex<Vec<String>>,
    }
    impl GatewayHost for ChangingHost {
        fn register(&self, _: &Path, _: &str) -> Result<ReconciledGateway, GatewayError> {
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
    let gateway = Gateway::bootstrap(host.clone(), "/runtime".into(), "ci".into());
    tauri::async_runtime::block_on(gateway.wait_ready()).unwrap();
    assert!(tauri::async_runtime::block_on(gateway.wait_ready()).is_err());
    assert_eq!(gateway.stop_agents(), Err(GatewayError::NotReconciled));
    assert!(host.calls.lock().unwrap().is_empty());
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
