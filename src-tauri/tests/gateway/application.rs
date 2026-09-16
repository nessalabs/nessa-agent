use super::*;
use std::{path::Path, sync::Mutex};
struct FakeHost {
    registration: Result<String, GatewayError>,
    stop_result: Result<(), GatewayError>,
    calls: Mutex<Vec<String>>,
}
impl GatewayHost for FakeHost {
    fn register(&self, _: &Path, stage: &str) -> Result<String, GatewayError> {
        self.calls.lock().unwrap().push(format!("register:{stage}"));
        self.registration.clone()
    }
    fn stop_agents(&self, service: &str) -> Result<(), GatewayError> {
        self.calls.lock().unwrap().push(format!("stop:{service}"));
        self.stop_result.clone()
    }
}
#[test]
fn gateway_instances_use_only_their_injected_service_and_surface_stop_failure() {
    for identity in ["first", "second"] {
        let host = Arc::new(FakeHost {
            registration: Ok(identity.into()),
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
fn later_failure_retains_only_the_last_successfully_reconciled_identity() {
    struct ChangingHost {
        registrations: Mutex<Vec<Result<String, GatewayError>>>,
        calls: Mutex<Vec<String>>,
    }
    impl GatewayHost for ChangingHost {
        fn register(&self, _: &Path, _: &str) -> Result<String, GatewayError> {
            self.registrations.lock().unwrap().remove(0)
        }
        fn stop_agents(&self, service: &str) -> Result<(), GatewayError> {
            self.calls.lock().unwrap().push(service.to_owned());
            Ok(())
        }
    }
    let host = Arc::new(ChangingHost {
        registrations: Mutex::new(vec![
            Ok("gui/501/exact-reconciled".into()),
            Err(GatewayError::Registration("foreign service".into())),
        ]),
        calls: Mutex::new(vec![]),
    });
    let gateway = Gateway::bootstrap(host.clone(), "/runtime".into(), "ci".into());
    tauri::async_runtime::block_on(gateway.wait_ready()).unwrap();
    assert!(tauri::async_runtime::block_on(gateway.wait_ready()).is_err());
    gateway.stop_agents().unwrap();
    assert_eq!(*host.calls.lock().unwrap(), ["gui/501/exact-reconciled"]);
}
