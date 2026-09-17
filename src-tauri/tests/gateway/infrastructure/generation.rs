use super::super::control::{
    classify, Health, ManagedRuntime, Registration, RetirementEvidence, ServiceState,
};
use super::{select_generation, service_generation};
use serde_json::{json, Value};
const FINGERPRINT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEW: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
fn base(workspace: &str) -> Value {
    json!({"Label":"service","ProgramArguments":["nessa","server"],"WorkingDirectory":workspace,"EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":FINGERPRINT},"KeepAlive":true})
}
fn installed(base: &Value, generation: &str) -> Value {
    let mut definition = base.clone();
    definition["EnvironmentVariables"]["NESSA_SERVICE_GENERATION"] = json!(generation);
    definition
}
#[test]
fn aborted_update_fence_rotates_current_generation_before_classification() {
    let desired = base("/data");
    let old = installed(&desired, OLD);
    let fence = RetirementEvidence {
        fingerprint: FINGERPRINT.into(),
        generation: OLD.into(),
    };
    let selected =
        select_generation(&desired, Some(&old), Some(&fence), || Ok(NEW.into())).unwrap();
    assert_eq!(selected, NEW);
    let running = ManagedRuntime {
        fingerprint: FINGERPRINT.into(),
        generation: OLD.into(),
        instance: "550e8400-e29b-41d4-a716-446655440000".into(),
        pid: 42,
    };
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            true,
            Some(Health::Managed(running.clone())),
            (FINGERPRINT, &selected),
            true,
            None
        ),
        ServiceState::ManagedStale(running)
    );
}
#[test]
fn reverting_configuration_gets_fresh_generation_and_failed_bootstrap_retry_reuses_it() {
    let configuration_a = base("/first");
    let configuration_b = base("/second");
    let current = installed(&configuration_b, OLD);
    let selected =
        select_generation(&configuration_a, Some(&current), None, || Ok(NEW.into())).unwrap();
    assert_eq!(selected, NEW);
    // Publication happened but bootstrap failed: the next reconciliation must not
    // assign another identity to the same already-published desired definition.
    let published = installed(&configuration_a, &selected);
    assert_eq!(
        select_generation(&configuration_a, Some(&published), None, || panic!(
            "retry must reuse published generation"
        ))
        .unwrap(),
        NEW
    );
}
#[test]
fn every_definition_change_or_invalid_generation_requires_new_identity() {
    let desired = base("/data");
    let current = installed(&desired, OLD);
    for key in desired.as_object().unwrap().keys() {
        let mut changed = desired.clone();
        changed[key] = json!("changed");
        assert_eq!(
            select_generation(&changed, Some(&current), None, || Ok(NEW.into())).unwrap(),
            NEW
        );
    }
    let invalid = installed(&desired, "invalid");
    assert_eq!(
        select_generation(&desired, Some(&invalid), None, || Ok(NEW.into())).unwrap(),
        NEW
    );
    assert!(service_generation(&current, Some(&current), None).is_err());
    let first = service_generation(&desired, None, None).unwrap();
    let second = service_generation(&desired, None, None).unwrap();
    assert_ne!(first, second);
    assert_eq!(first.len(), 64);
    assert!(first
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')));
}
