use super::super::generation::service_generation;
use super::super::startup::LastExit;
use super::{
    acknowledge, assess, atomic_write, classify, forward_recovery, lock_namespace, parse_health,
    parse_listener_pid, parse_pending_retirement, parse_retirement_evidence, parse_service_process,
    prepare_request, read_acknowledgement, read_pending_retirement, read_retirement_evidence,
    service_status, Health, InstallFailure, ManagedRuntime, Registration, ServiceState,
    ServiceStatus, Step,
};
use nessa_local_storage::OpenMode;
use serde_json::{json, Value};
use std::{
    fs,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
};
const RUNNING_GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TARGET_GENERATION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const INSTANCE: &str = "550e8400-e29b-41d4-a716-446655440000";
fn runtime(fingerprint: &str, pid: u32) -> ManagedRuntime {
    ManagedRuntime {
        fingerprint: fingerprint.into(),
        generation: RUNNING_GENERATION.into(),
        instance: INSTANCE.into(),
        pid,
    }
}
#[test]
fn classification_requires_the_exact_loaded_process() {
    let old = runtime("old", 42);
    assert_eq!(
        classify(
            Registration::Unloaded,
            None,
            true,
            Some(Health::Managed(runtime("new", 42))),
            ("new", RUNNING_GENERATION),
            true,
            None
        ),
        ServiceState::ForeignPort
    );
    assert_eq!(
        classify(
            Registration::Unloaded,
            None,
            false,
            None,
            ("new", RUNNING_GENERATION),
            false,
            None
        ),
        ServiceState::Unloaded
    );
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            true,
            Some(Health::Managed(runtime("new", 42))),
            ("new", RUNNING_GENERATION),
            true,
            None
        ),
        ServiceState::ManagedCurrent(runtime("new", 42))
    );
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            true,
            Some(Health::Managed(old.clone())),
            ("new", RUNNING_GENERATION),
            true,
            None
        ),
        ServiceState::ManagedStale(old)
    );
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            false,
            Some(Health::Managed(runtime("new", 42))),
            ("new", RUNNING_GENERATION),
            true,
            None
        ),
        ServiceState::ManagedStale(runtime("new", 42))
    );
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            true,
            Some(Health::Managed(runtime("new", 42))),
            ("new", TARGET_GENERATION),
            true,
            None
        ),
        ServiceState::ManagedStale(runtime("new", 42))
    );
    for matching in [true, false] {
        assert_eq!(
            classify(
                Registration::Loaded,
                Some(99),
                matching,
                Some(Health::Managed(runtime("new", 42))),
                ("new", RUNNING_GENERATION),
                true,
                None
            ),
            ServiceState::ForeignPort
        );
    }
    assert_eq!(
        classify(
            Registration::Loaded,
            None,
            true,
            None,
            ("new", RUNNING_GENERATION),
            false,
            None
        ),
        ServiceState::UnavailableLoadedService
    );
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            true,
            Some(Health::Legacy),
            ("new", RUNNING_GENERATION),
            true,
            Some(42)
        ),
        ServiceState::LegacyExactService
    );
    for listener in [None, Some(99)] {
        assert_eq!(
            classify(
                Registration::Loaded,
                Some(42),
                true,
                Some(Health::Legacy),
                ("new", RUNNING_GENERATION),
                true,
                listener
            ),
            ServiceState::ForeignPort
        );
    }
}
#[test]
fn diagnostic_pid_parsing_rejects_missing_ambiguous_and_nested_only_values() {
    assert_eq!(
        parse_service_process("gui/501/example = {\n pid = 42\n}"),
        Ok(Some(42))
    );
    for text in [
        "gui/501/example = {\n environment = {\n pid = 99\n }\n pid = 42\n}",
        "gui/501/example = {\n pid = 42\n pid = 43\n}",
        "gui/501/example = {\n pid = invalid\n}",
        "gui/501/example = {\n environment = {\n pid = 42\n }\n}",
        "",
    ] {
        assert_eq!(parse_service_process(text), Err(()));
    }
    assert_eq!(parse_service_process("gui/501/example = {\n}"), Ok(None));
    assert_eq!(
        parse_service_process("gui/501/example = {\n environment = {\n key = value\n }"),
        Err(())
    );
    assert_eq!(parse_listener_pid("42\n42\n"), Some(42));
    for text in ["", "42\n99\n", "invalid", "0"] {
        assert_eq!(parse_listener_pid(text), None);
    }
}
#[test]
fn managed_health_requires_complete_canonical_incarnation_identity() {
    assert_eq!(
        parse_health(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"),
        Some(Health::Legacy)
    );
    let headers = format!("x-nessa-runtime-fingerprint: {}\r\nx-nessa-runtime-instance: {INSTANCE}\r\nx-nessa-service-generation: {RUNNING_GENERATION}\r\nx-nessa-process-id: 42", "a".repeat(64));
    let valid = format!("HTTP/1.1 200 OK\r\n{headers}\r\n\r\n");
    assert_eq!(
        parse_health(valid.as_bytes()),
        Some(Health::Managed(runtime(&"a".repeat(64), 42)))
    );
    for invalid in [
        valid.replace(INSTANCE, "invalid"),
        valid.replace(INSTANCE, &INSTANCE.to_uppercase()),
        valid.replace("process-id: 42", "process-id: 0"),
        valid.replace("process-id: 42", "process-id: -42"),
        valid.replace("200 OK", "503 No"),
        format!("HTTP/1.1 200 OK\r\n{headers}\r\nx-nessa-process-id: 42\r\n\r\n"),
        format!(
            "HTTP/1.1 200 OK\r\nx-nessa-runtime-fingerprint: {}\r\n\r\n",
            "a".repeat(64)
        ),
    ] {
        assert_eq!(parse_health(invalid.as_bytes()), None);
    }
}
#[test]
fn managed_generation_header_must_be_complete_unique_lowercase_sha256() {
    let valid = format!("HTTP/1.1 200 OK\r\nx-nessa-runtime-fingerprint: {}\r\nx-nessa-runtime-instance: {INSTANCE}\r\nx-nessa-process-id: 42\r\nx-nessa-service-generation: {RUNNING_GENERATION}\r\n\r\n", "c".repeat(64));
    for invalid in [
        valid.replace(RUNNING_GENERATION, "short"),
        valid.replace(RUNNING_GENERATION, &RUNNING_GENERATION.to_uppercase()),
        valid.replace(
            &format!("x-nessa-service-generation: {RUNNING_GENERATION}\r\n"),
            "",
        ),
        valid.replace(
            "\r\n\r\n",
            &format!("\r\nx-nessa-service-generation: {RUNNING_GENERATION}\r\n\r\n"),
        ),
    ] {
        assert_eq!(parse_health(invalid.as_bytes()), None);
    }
}
fn successful_result() -> Value {
    json!({"requestId":"fresh","targetFingerprint":"new","runningFingerprint":"old","requestedInstance":INSTANCE,"runningInstance":INSTANCE,"runningGeneration":RUNNING_GENERATION,"requestedRunningGeneration":RUNNING_GENERATION,"targetGeneration":TARGET_GENERATION,"retirementRequestId":INSTANCE,"retirementCause":{"principalId":"gateway","surfaceId":"gateway_upgrade","requestId":INSTANCE},"retired":true,"cleanupError":null,"auditError":null})
}
#[test]
fn retirement_requires_exact_incarnation_and_both_effect_acknowledgements() {
    let valid = successful_result();
    assert_eq!(
        acknowledge(
            valid.to_string().as_bytes(),
            "fresh",
            "new",
            "old",
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(true)
    );
    for field in [
        "requestId",
        "targetFingerprint",
        "runningFingerprint",
        "runningInstance",
        "requestedInstance",
        "runningGeneration",
        "requestedRunningGeneration",
        "targetGeneration",
    ] {
        let mut stale = valid.clone();
        stale[field] = json!("different");
        assert_eq!(
            acknowledge(
                stale.to_string().as_bytes(),
                "fresh",
                "new",
                "old",
                INSTANCE,
                RUNNING_GENERATION,
                TARGET_GENERATION
            ),
            Ok(false)
        );
    }
    for (field, value) in [
        ("retired", json!(false)),
        ("cleanupError", json!("failure")),
        ("auditError", json!("failure")),
    ] {
        let mut failed = valid.clone();
        failed[field] = value;
        assert!(acknowledge(
            failed.to_string().as_bytes(),
            "fresh",
            "new",
            "old",
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        )
        .is_err());
    }
    for value in [
        Value::Null,
        json!("invalid"),
        json!(INSTANCE.to_uppercase()),
    ] {
        let mut invalid = valid.clone();
        invalid["retirementRequestId"] = value;
        assert_eq!(
            acknowledge(
                invalid.to_string().as_bytes(),
                "fresh",
                "new",
                "old",
                INSTANCE,
                RUNNING_GENERATION,
                TARGET_GENERATION
            ),
            Ok(false)
        );
    }
    for field in [
        "retirementRequestId",
        "retirementCause",
        "requestedInstance",
        "runningGeneration",
        "requestedRunningGeneration",
        "targetGeneration",
    ] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert_eq!(
            acknowledge(
                missing.to_string().as_bytes(),
                "fresh",
                "new",
                "old",
                INSTANCE,
                RUNNING_GENERATION,
                TARGET_GENERATION
            ),
            Ok(false)
        );
    }
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("runningInstance");
    assert_eq!(
        acknowledge(
            missing.to_string().as_bytes(),
            "fresh",
            "new",
            "old",
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(false)
    );
    assert_eq!(
        acknowledge(
            b"stale malformed result",
            "fresh",
            "new",
            "old",
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(false)
    );
}
#[test]
fn fresh_requests_preserve_successful_fences_and_ignore_unrelated_results() {
    let directory =
        std::env::temp_dir().join(format!("nessa-upgrade-fence-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    nessa_local_storage::create_directory(&directory).unwrap();
    let _lock = lock_namespace(&directory).unwrap();
    let path = directory.join("result.json");
    atomic_write(&path, b"malformed stale result").unwrap();
    prepare_request(
        &directory,
        "fresh",
        "new",
        INSTANCE,
        RUNNING_GENERATION,
        TARGET_GENERATION,
    )
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"malformed stale result");
    assert_eq!(
        read_acknowledgement(
            &directory,
            "fresh",
            "new",
            "old",
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(false)
    );
    let mut previous = successful_result();
    previous["requestId"] = json!("previous");
    atomic_write(&path, previous.to_string().as_bytes()).unwrap();
    prepare_request(
        &directory,
        "fresh",
        "new",
        INSTANCE,
        RUNNING_GENERATION,
        TARGET_GENERATION,
    )
    .unwrap();
    assert_eq!(fs::read(&path).unwrap(), previous.to_string().as_bytes());
    assert_eq!(
        read_acknowledgement(
            &directory,
            "fresh",
            "new",
            "old",
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(false)
    );
    let request: Value =
        serde_json::from_slice(&fs::read(directory.join("request.json")).unwrap()).unwrap();
    assert_eq!(
        request,
        json!({"requestId":"fresh","targetFingerprint":"new","runningInstance":INSTANCE,"runningGeneration":RUNNING_GENERATION,"targetGeneration":TARGET_GENERATION})
    );
    atomic_write(&path, successful_result().to_string().as_bytes()).unwrap();
    assert_eq!(
        read_acknowledgement(
            &directory,
            "fresh",
            "new",
            "old",
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(true)
    );
    drop(_lock);
    fs::remove_dir_all(directory).unwrap();
}
#[test]
fn installation_failures_preserve_primary_error_and_require_forward_recovery() {
    assert_eq!(forward_recovery(Ok(())), Ok(()));
    for stage in ["publication", "bootstrap", "readiness"] {
        let error = forward_recovery::<()>(Err(stage.into())).unwrap_err();
        assert!(error.starts_with(stage));
        assert!(error.contains("loaded process were preserved for forward recovery"));
    }
}
const EXPECTED_FINGERPRINT: &str =
    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn observed(pid: Option<u32>, identity_known: bool, last_exit: LastExit) -> ServiceStatus {
    ServiceStatus {
        loaded: true,
        pid,
        process_identity_known: identity_known,
        last_exit,
    }
}

#[test]
fn readiness_requires_the_exact_runtime_owned_by_the_loaded_process() {
    let runtime = ManagedRuntime {
        fingerprint: EXPECTED_FINGERPRINT.into(),
        generation: RUNNING_GENERATION.into(),
        instance: INSTANCE.into(),
        pid: 42,
    };
    let expected = (EXPECTED_FINGERPRINT, RUNNING_GENERATION);
    assert_eq!(
        assess(
            Some(&Health::Managed(runtime.clone())),
            &observed(Some(42), true, LastExit::NeverExited),
            expected
        ),
        Step::Ready(runtime.clone())
    );
    // A process launchd did not report, or reported as something else, is not
    // the one that answered, whatever it said about itself.
    for status in [
        observed(Some(43), true, LastExit::NeverExited),
        observed(Some(42), false, LastExit::NeverExited),
        observed(None, true, LastExit::NeverExited),
    ] {
        assert_ne!(
            assess(Some(&Health::Managed(runtime.clone())), &status, expected),
            Step::Ready(runtime.clone())
        );
    }
}

#[test]
fn a_pid_we_could_not_read_is_not_a_process_that_is_gone() {
    // `service_status` keeps "no process" apart from "could not tell", and
    // the rejected PID syntax below is the parser's own. Counting an answer
    // we did not get as death fails a service that is merely restarting,
    // and blames it for whatever its previous process did.
    for diagnostic in [
        "gui/501/service = {\n state = not running\n pid = invalid\n}",
        "gui/501/service = {\n pid = 42\n pid = 43\n}",
    ] {
        let parsed = parse_service_process(diagnostic);
        assert!(parsed.is_err(), "{diagnostic}");
        assert_eq!(
            assess(
                None,
                &observed(parsed.ok().flatten(), false, LastExit::Code(1)),
                (EXPECTED_FINGERPRINT, RUNNING_GENERATION)
            ),
            Step::Waiting
        );
    }
    // Absence established, with a failed exit, is the case this exists for.
    assert_eq!(
        assess(
            None,
            &observed(None, true, LastExit::Code(1)),
            (EXPECTED_FINGERPRINT, RUNNING_GENERATION)
        ),
        Step::Dead
    );
}

#[test]
fn another_gateway_on_the_port_never_answers_for_this_one() {
    // This stage's port is not owned by this label: registration locks are
    // per label and the port is per stage, so a second instance can hold it.
    // Its health response says nothing about the service being started, and
    // must not stop us reading launchd's answer about that service — which
    // used to cost the full deadline and the generic readiness message, the
    // exact outcome this change exists to remove.
    let foreign = Health::Managed(ManagedRuntime {
        fingerprint: "d".repeat(64),
        generation: TARGET_GENERATION.into(),
        instance: "660e8400-e29b-41d4-a716-446655440000".into(),
        pid: 99,
    });
    assert_eq!(
        assess(
            Some(&foreign),
            &observed(None, true, LastExit::Code(23)),
            (EXPECTED_FINGERPRINT, RUNNING_GENERATION)
        ),
        Step::Dead
    );
    // A legacy listener on the same port is no different.
    assert_eq!(
        assess(
            Some(&Health::Legacy),
            &observed(None, true, LastExit::Code(23)),
            (EXPECTED_FINGERPRINT, RUNNING_GENERATION)
        ),
        Step::Dead
    );
}

#[test]
fn a_process_that_is_running_or_has_not_failed_is_still_starting() {
    let expected = (EXPECTED_FINGERPRINT, RUNNING_GENERATION);
    for status in [
        // Something is running, whatever it has yet to say.
        observed(Some(42), true, LastExit::Code(1)),
        // Gone, but nothing says it failed.
        observed(None, true, LastExit::NeverExited),
        observed(None, true, LastExit::Code(0)),
        observed(None, true, LastExit::Unknown),
        // Not loaded at all is not this function's failure to report.
        ServiceStatus {
            loaded: false,
            pid: None,
            process_identity_known: true,
            last_exit: LastExit::Code(1),
        },
    ] {
        assert_eq!(assess(None, &status, expected), Step::Waiting);
    }
}

#[test]
fn an_unloaded_service_reports_no_exit_of_its_own() {
    // The unloaded answer must not carry a stale exit into the judgement
    // above; it is the one `service_status` synthesises, not launchd's.
    let status = service_status("gui/501/so.nessa.absent.invalid").unwrap();
    assert!(!status.loaded);
    assert_eq!(status.pid, None);
    assert!(status.process_identity_known);
    assert_eq!(status.last_exit, LastExit::Unknown);
    assert!(!status.last_exit.is_failure());
}

#[test]
fn a_service_that_will_not_start_keeps_the_sentence_written_for_the_person() {
    // Retrying reconciliation is advice about our own mechanism. It is not what
    // someone whose service exits on startup should be told to do, and the
    // sentence naming the cause is already finished when it arrives here.
    let error = forward_recovery::<()>(Err(InstallFailure::Startup(
        "Nessa's background service is not starting: its credential registry is not one this version of Nessa can read.".into(),
    )))
    .unwrap_err();
    assert_eq!(
        error,
        "Nessa's background service is not starting: its credential registry is not one this version of Nessa can read."
    );
    assert!(!error.contains("forward recovery"));
    assert!(!error.contains("runtime identity"));
}
#[test]
fn private_request_replacement_is_complete_and_exclusively_locked() {
    let directory = std::env::temp_dir().join(format!(
        "nessa-upgrade-exchange-{}-{}",
        std::process::id(),
        service_generation(&json!({}), None, None).unwrap()
    ));
    let _ = fs::remove_dir_all(&directory);
    nessa_local_storage::create_directory(&directory).unwrap();
    let path = directory.join("request.json");
    atomic_write(&path, b"previous request longer than replacement").unwrap();
    atomic_write(&path, b"new").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
    let lock = lock_namespace(&directory).unwrap();
    assert_ne!(
        unsafe { libc::fcntl(lock.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    let competing =
        nessa_local_storage::open(&directory.join("gateway-upgrade.lock"), OpenMode::ReadWrite)
            .unwrap();
    assert_ne!(
        unsafe { libc::flock(competing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    drop(lock);
    assert_eq!(
        unsafe { libc::flock(competing.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    drop(competing);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn durable_fence_requires_strict_success_with_valid_identity() {
    let mut result = successful_result();
    result["requestId"] = json!(INSTANCE);
    result["targetFingerprint"] = json!(TARGET_GENERATION);
    result["runningFingerprint"] = json!(RUNNING_GENERATION);
    let fence = parse_retirement_evidence(result.to_string().as_bytes())
        .unwrap()
        .unwrap();
    assert!(fence.matches(RUNNING_GENERATION, RUNNING_GENERATION));
    assert!(!fence.matches(RUNNING_GENERATION, TARGET_GENERATION));
    for (field, value) in [
        ("auditError", json!("failed")),
        ("cleanupError", json!("failed")),
        ("requestId", json!("invalid")),
        ("requestedInstance", json!("invalid")),
        ("runningGeneration", json!("invalid")),
        ("retirementRequestId", Value::Null),
    ] {
        let mut invalid = result.clone();
        invalid[field] = value;
        assert!(parse_retirement_evidence(invalid.to_string().as_bytes()).is_err());
    }
    assert!(parse_retirement_evidence(b"malformed").is_err());
    result["retired"] = json!(false);
    assert!(parse_retirement_evidence(result.to_string().as_bytes()).is_err());
    result["retirementRequestId"] = Value::Null;
    result["retirementCause"] = Value::Null;
    result["cleanupError"] = json!("runtime identity rejected");
    result["requestedInstance"] = json!("660e8400-e29b-41d4-a716-446655440000");
    assert!(parse_retirement_evidence(result.to_string().as_bytes())
        .unwrap()
        .is_none());
}

#[test]
fn admitted_cleanup_failure_forces_stale_retry_without_authorizing_bootout() {
    let mut failed = successful_result();
    failed["requestId"] = json!(INSTANCE);
    failed["targetFingerprint"] = json!(RUNNING_GENERATION);
    failed["runningFingerprint"] = json!(RUNNING_GENERATION);
    failed["retired"] = json!(false);
    failed["cleanupError"] = json!("cleanup uncertain");
    let evidence = parse_retirement_evidence(failed.to_string().as_bytes())
        .unwrap()
        .unwrap();
    let desired = json!({"WorkingDirectory":"/data","EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":RUNNING_GENERATION}});
    let mut installed = desired.clone();
    installed["EnvironmentVariables"]["NESSA_SERVICE_GENERATION"] = json!(RUNNING_GENERATION);
    let target = service_generation(&desired, Some(&installed), Some(&evidence)).unwrap();
    assert_ne!(target, RUNNING_GENERATION);
    let running = runtime(RUNNING_GENERATION, 42);
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            true,
            Some(Health::Managed(running.clone())),
            (RUNNING_GENERATION, &target),
            true,
            None
        ),
        ServiceState::ManagedStale(running)
    );
    assert!(acknowledge(
        failed.to_string().as_bytes(),
        INSTANCE,
        RUNNING_GENERATION,
        RUNNING_GENERATION,
        INSTANCE,
        RUNNING_GENERATION,
        TARGET_GENERATION
    )
    .is_err());
    // A fresh retry retains the original retirement cause, completes cleanup,
    // and explicitly acknowledges the newly selected target generation.
    failed["requestId"] = json!("660e8400-e29b-41d4-a716-446655440000");
    failed["targetGeneration"] = json!(target);
    failed["retired"] = json!(true);
    failed["cleanupError"] = Value::Null;
    assert_eq!(
        acknowledge(
            failed.to_string().as_bytes(),
            "660e8400-e29b-41d4-a716-446655440000",
            RUNNING_GENERATION,
            RUNNING_GENERATION,
            INSTANCE,
            RUNNING_GENERATION,
            &target
        ),
        Ok(true)
    );
    // Rejections before retirement admission carry no cause and cannot force rotation.
    failed["retired"] = json!(false);
    failed["retirementRequestId"] = Value::Null;
    failed["retirementCause"] = Value::Null;
    failed["cleanupError"] = json!("runtime identity rejected");
    failed["requestedInstance"] = json!("770e8400-e29b-41d4-a716-446655440000");
    assert!(parse_retirement_evidence(failed.to_string().as_bytes())
        .unwrap()
        .is_none());
    assert_eq!(
        service_generation(&desired, Some(&installed), None).unwrap(),
        RUNNING_GENERATION
    );
}

#[test]
fn rejected_requested_generation_does_not_poison_actual_runtime_retry() {
    let data =
        std::env::temp_dir().join(format!("nessa-rejected-generation-{}", std::process::id()));
    let _ = fs::remove_dir_all(&data);
    let directory = data.join("gateway-upgrade");
    nessa_local_storage::create_directory(&directory).unwrap();

    let mut result = successful_result();
    result["requestId"] = json!(INSTANCE);
    result["targetFingerprint"] = json!(TARGET_GENERATION);
    result["runningFingerprint"] = json!(RUNNING_GENERATION);
    result["retired"] = json!(false);
    result["retirementRequestId"] = Value::Null;
    result["retirementCause"] = Value::Null;
    result["cleanupError"] = json!("runtime identity rejected");
    result["requestedRunningGeneration"] = json!(TARGET_GENERATION);
    assert!(parse_retirement_evidence(result.to_string().as_bytes())
        .unwrap()
        .is_none());
    assert_eq!(
        acknowledge(
            result.to_string().as_bytes(),
            INSTANCE,
            TARGET_GENERATION,
            RUNNING_GENERATION,
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(false)
    );
    atomic_write(
        &directory.join("result.json"),
        result.to_string().as_bytes(),
    )
    .unwrap();
    assert!(read_retirement_evidence(&data).unwrap().is_none());
    let desired = json!({"WorkingDirectory":"/data","EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":RUNNING_GENERATION}});
    let mut installed = desired.clone();
    installed["EnvironmentVariables"]["NESSA_SERVICE_GENERATION"] = json!(RUNNING_GENERATION);
    assert_eq!(
        service_generation(&desired, Some(&installed), None).unwrap(),
        RUNNING_GENERATION
    );
    // A retry observing the actual process supplies its actual generation; the
    // rejected request did not establish a prior retirement cause.
    result["requestId"] = json!("660e8400-e29b-41d4-a716-446655440000");
    result["requestedRunningGeneration"] = json!(RUNNING_GENERATION);
    result["retirementRequestId"] = json!("660e8400-e29b-41d4-a716-446655440000");
    result["retirementCause"] = json!({"principalId":"gateway","surfaceId":"gateway_upgrade","requestId":"660e8400-e29b-41d4-a716-446655440000"});
    result["retired"] = json!(true);
    result["cleanupError"] = Value::Null;
    assert!(parse_retirement_evidence(result.to_string().as_bytes())
        .unwrap()
        .unwrap()
        .matches(RUNNING_GENERATION, RUNNING_GENERATION));
    assert_eq!(
        acknowledge(
            result.to_string().as_bytes(),
            "660e8400-e29b-41d4-a716-446655440000",
            TARGET_GENERATION,
            RUNNING_GENERATION,
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION
        ),
        Ok(true)
    );
    atomic_write(
        &directory.join("result.json"),
        result.to_string().as_bytes(),
    )
    .unwrap();
    assert!(read_retirement_evidence(&data)
        .unwrap()
        .unwrap()
        .matches(RUNNING_GENERATION, RUNNING_GENERATION));
    fs::remove_dir_all(data).unwrap();
}
#[test]
fn admitted_failure_evidence_survives_preparing_another_request() {
    let directory =
        std::env::temp_dir().join(format!("nessa-upgrade-admitted-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    nessa_local_storage::create_directory(&directory).unwrap();
    let mut failed = successful_result();
    failed["requestId"] = json!(INSTANCE);
    failed["targetFingerprint"] = json!(TARGET_GENERATION);
    failed["runningFingerprint"] = json!(RUNNING_GENERATION);
    failed["retired"] = json!(false);
    failed["cleanupError"] = json!("still cleaning up");
    let path = directory.join("result.json");
    atomic_write(&path, failed.to_string().as_bytes()).unwrap();
    prepare_request(
        &directory,
        "fresh",
        TARGET_GENERATION,
        INSTANCE,
        RUNNING_GENERATION,
        TARGET_GENERATION,
    )
    .unwrap();
    let evidence = parse_retirement_evidence(&fs::read(&path).unwrap())
        .unwrap()
        .unwrap();
    assert!(evidence.matches(RUNNING_GENERATION, RUNNING_GENERATION));
    // Non-null retirement identity can never be combined with a request for a
    // different generation to silently turn admitted evidence into a rejection.
    failed["requestedRunningGeneration"] = json!(TARGET_GENERATION);
    assert!(parse_retirement_evidence(failed.to_string().as_bytes()).is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn matching_published_request_without_result_forces_reconciliation() {
    let running = runtime(RUNNING_GENERATION, 42);
    let data =
        std::env::temp_dir().join(format!("nessa-pending-retirement-{}", std::process::id()));
    let _ = fs::remove_dir_all(&data);
    let directory = data.join("gateway-upgrade");
    nessa_local_storage::create_directory(&directory).unwrap();
    prepare_request(
        &directory,
        INSTANCE,
        TARGET_GENERATION,
        INSTANCE,
        RUNNING_GENERATION,
        TARGET_GENERATION,
    )
    .unwrap();
    assert!(!directory.join("result.json").exists());
    assert!(read_pending_retirement(&data, &running).unwrap().is_some());
    let request = json!({"requestId":INSTANCE,"targetFingerprint":TARGET_GENERATION,"runningInstance":INSTANCE,"runningGeneration":RUNNING_GENERATION,"targetGeneration":TARGET_GENERATION});
    let pending = parse_pending_retirement(request.to_string().as_bytes(), &running)
        .unwrap()
        .unwrap();
    let desired = json!({"WorkingDirectory":"/data","EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":RUNNING_GENERATION}});
    let mut installed = desired.clone();
    installed["EnvironmentVariables"]["NESSA_SERVICE_GENERATION"] = json!(RUNNING_GENERATION);
    let target = service_generation(&desired, Some(&installed), Some(&pending)).unwrap();
    assert_ne!(target, RUNNING_GENERATION);
    assert_eq!(
        classify(
            Registration::Loaded,
            Some(42),
            true,
            Some(Health::Managed(running.clone())),
            (RUNNING_GENERATION, &target),
            true,
            None
        ),
        ServiceState::ManagedStale(running.clone())
    );
    // Rejected or delayed requests for another live incarnation/generation are
    // not evidence that this process fenced admission.
    for (field, value) in [
        (
            "runningInstance",
            json!("660e8400-e29b-41d4-a716-446655440000"),
        ),
        ("runningGeneration", json!(TARGET_GENERATION)),
        ("targetGeneration", json!(RUNNING_GENERATION)),
    ] {
        let mut stale = request.clone();
        stale[field] = value;
        assert!(
            parse_pending_retirement(stale.to_string().as_bytes(), &running)
                .unwrap()
                .is_none()
        );
    }
    let mut completed = successful_result();
    completed["requestId"] = json!(INSTANCE);
    completed["targetFingerprint"] = json!(TARGET_GENERATION);
    completed["runningFingerprint"] = json!(RUNNING_GENERATION);
    completed["targetGeneration"] = json!(target);
    assert_eq!(
        acknowledge(
            completed.to_string().as_bytes(),
            INSTANCE,
            TARGET_GENERATION,
            RUNNING_GENERATION,
            INSTANCE,
            RUNNING_GENERATION,
            &target
        ),
        Ok(true)
    );
    fs::remove_dir_all(data).unwrap();
}

#[test]
fn safe_looking_disk_definition_cannot_authorize_unloading_an_inactive_service() {
    let desired = json!({"Label":"service","ProgramArguments":["/runtime/nessa","server","--desktop-runtime","/runtime"],"EnvironmentVariables":{"NESSA_RUNTIME_FINGERPRINT":RUNNING_GENERATION}});
    let installed = desired.clone();
    // launchd retains independent configuration. Regardless of the disk match,
    // absent live identity never grants authority to stop/re-register this label.
    for diagnostic in [
        "gui/501/service = {\n state = not running\n program = /other/nessa\n}",
        "gui/501/service = {\n state = not running\n program = /runtime/nessa\n arguments = {\n /runtime/nessa\n server\n --desktop-runtime\n /other/runtime\n }\n}",
        "gui/501/service = {\n state = spawn scheduled\n environment = {\n NESSA_RUNTIME_FINGERPRINT => other\n }\n}",
        "gui/501/service = {\n state = not running\n pid = invalid\n}",
    ] {
        assert_eq!(
            classify(
                Registration::Loaded,
                parse_service_process(diagnostic).ok().flatten(),
                installed == desired,
                None,
                (RUNNING_GENERATION, TARGET_GENERATION),
                false,
                None
            ),
            ServiceState::UnavailableLoadedService
        );
    }
}
#[test]
fn admitted_same_generation_transition_is_corrupt_evidence_not_bootout_authority() {
    let mut result = successful_result();
    result["requestId"] = json!(INSTANCE);
    result["targetFingerprint"] = json!(TARGET_GENERATION);
    result["runningFingerprint"] = json!(RUNNING_GENERATION);
    result["targetGeneration"] = json!(RUNNING_GENERATION);
    for retired in [true, false] {
        result["retired"] = json!(retired);
        assert!(parse_retirement_evidence(result.to_string().as_bytes()).is_err());
        assert_eq!(
            acknowledge(
                result.to_string().as_bytes(),
                INSTANCE,
                TARGET_GENERATION,
                RUNNING_GENERATION,
                INSTANCE,
                RUNNING_GENERATION,
                RUNNING_GENERATION
            ),
            Ok(false)
        );
    }
}

#[test]
fn retirement_cause_must_agree_with_identity_and_confirmed_authority() {
    let mut result = successful_result();
    result["requestId"] = json!(INSTANCE);
    result["targetFingerprint"] = json!(TARGET_GENERATION);
    result["runningFingerprint"] = json!(RUNNING_GENERATION);

    let mut cause_free_admitted = result.clone();
    cause_free_admitted["retired"] = json!(false);
    cause_free_admitted["retirementRequestId"] = Value::Null;
    cause_free_admitted["retirementCause"] = Value::Null;
    cause_free_admitted["cleanupError"] = json!("cleanup failed");
    assert!(parse_retirement_evidence(cause_free_admitted.to_string().as_bytes()).is_err());
    let mut cause_free_invalid_instance = cause_free_admitted.clone();
    cause_free_invalid_instance["requestedInstance"] = json!("not-a-uuid");
    assert!(parse_retirement_evidence(cause_free_invalid_instance.to_string().as_bytes()).is_err());

    for (field, value) in [
        ("principalId", json!("system-supervisor")),
        ("surfaceId", json!("server_shutdown")),
    ] {
        let mut wrong_authority = result.clone();
        wrong_authority["retirementCause"][field] = value;
        assert!(parse_retirement_evidence(wrong_authority.to_string().as_bytes()).is_err());
        assert_eq!(
            acknowledge(
                wrong_authority.to_string().as_bytes(),
                INSTANCE,
                TARGET_GENERATION,
                RUNNING_GENERATION,
                INSTANCE,
                RUNNING_GENERATION,
                TARGET_GENERATION,
            ),
            Ok(false)
        );

        wrong_authority["retired"] = json!(false);
        wrong_authority["cleanupError"] = json!("closed by another lifecycle cause");
        assert!(
            parse_retirement_evidence(wrong_authority.to_string().as_bytes())
                .unwrap()
                .is_some()
        );
        assert!(acknowledge(
            wrong_authority.to_string().as_bytes(),
            INSTANCE,
            TARGET_GENERATION,
            RUNNING_GENERATION,
            INSTANCE,
            RUNNING_GENERATION,
            TARGET_GENERATION,
        )
        .is_err());
    }
}

#[cfg(unix)]
#[test]
fn host_exchange_reads_reject_fifos_without_waiting_for_a_writer() {
    let data = std::env::temp_dir().join(format!(
        "nessa-host-exchange-fifo-{}",
        service_generation(&json!({}), None, None).unwrap()
    ));
    let directory = data.join("gateway-upgrade");
    nessa_local_storage::create_directory(&directory).unwrap();
    let running = runtime(RUNNING_GENERATION, 42);

    let request_path = directory.join("request.json");
    assert!(std::process::Command::new("mkfifo")
        .arg(&request_path)
        .status()
        .unwrap()
        .success());
    assert!(read_pending_retirement(&data, &running).is_err());
    fs::remove_file(request_path).unwrap();

    let result_path = directory.join("result.json");
    assert!(std::process::Command::new("mkfifo")
        .arg(&result_path)
        .status()
        .unwrap()
        .success());
    assert!(read_retirement_evidence(&data).is_err());
    assert!(read_acknowledgement(
        &directory,
        INSTANCE,
        TARGET_GENERATION,
        RUNNING_GENERATION,
        INSTANCE,
        RUNNING_GENERATION,
        TARGET_GENERATION,
    )
    .is_err());
    fs::remove_file(result_path).unwrap();
    fs::remove_dir_all(data).unwrap();
}
