//! The native session uses the real paired gateway, physical cache and private scope.
#[path = "native_session/clock.rs"]
mod clock;
use super::fixtures::{Gateway, Setup};
use clock::{watched, ManualClock};
use nessa_auth::domain::pairing::ConsentClass;
use nessa_client_core::{
    pairing::NativeClientError,
    retained::{ReadFailure, ReadStop, RetainedTranscript},
};
use nessa_device_client::{NativeSession, SessionFailure};
use nessa_protocol::{conversation::domain::ConversationId, pairing::wire::NativePairingStatus};
use serde_json::Value;
use std::{ffi::OsStr, net::SocketAddr, path::Path, process::Command, sync::Arc};
use uuid::Uuid;

/// Opt-in generated Swift executable, linked against the same shared library.
/// No executable configured means this explicit host probe is ignored by default.
#[test]
#[ignore = "set NESSA_NATIVE_SWIFT_CHECK to the compiled generated-Swift harness"]
fn generated_swift_runs_paired_native_lifecycle() {
    let binary =
        std::env::var_os("NESSA_NATIVE_SWIFT_CHECK").expect("explicit native Swift checker");
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    run_swift(&binary, "first", &profile, &setup);
    drop(gateway);
    run_swift(&binary, "offline", &profile, &setup);
    let restarted = Gateway::start(&root);
    run_swift(&binary, "resumed", &profile, &setup);
    drop(restarted);
    let _revoked = Gateway::start_mode(&root, "revoke");
    run_swift(&binary, "terminal", &profile, &setup);

    // A separate paired scope changes its receiver epoch after a confirmed page.
    // This exercises the native driver's refusal recheck, not the CLI's owner.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _epoch = Gateway::start_mode(&root, "epoch");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    run_swift(&binary, "epoch", &profile, &setup);
}

fn run_swift(binary: &OsStr, mode: &str, profile: &Value, setup: &Setup) {
    let output = Command::new(binary)
        .env(
            "NESSA_NATIVE_EXPECTED_CONSENT_CLASS",
            ConsentClass::DeviceRead.as_str(),
        )
        .env("NESSA_NATIVE_EXPECTED_CREDENTIAL_ID", &setup.credential)
        .env(
            "NESSA_NATIVE_EXPECTED_DEVICE_KEY",
            setup
                .claimed_device_key
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
        .args([
            mode,
            profile["stateRoot"].as_str().unwrap(),
            profile["stateDirectory"].as_str().unwrap(),
            profile["gatewayAddress"].as_str().unwrap(),
            &setup.conversation,
            &setup.receiver,
            &setup.gateway,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Swift {mode}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

#[test]
fn native_refusal_rechecks_pinned_status_without_purging_confirmed_partial_history() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start_mode(&root, "epoch");
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let address: SocketAddr = profile["gatewayAddress"].as_str().unwrap().parse().unwrap();
    let state_root = Path::new(profile["stateRoot"].as_str().unwrap());
    let state_directory = Path::new(profile["stateDirectory"].as_str().unwrap());
    let native = NativeSession::open_with_clock(
        state_root,
        state_directory,
        address,
        Arc::new(ManualClock::default()),
    )
    .unwrap();
    let stop = ReadStop::new();
    let refresh = watched(&native, || native.refresh(stop)).unwrap();
    let initial = refresh.initial();
    assert!(matches!(
        initial.observation(),
        Some(NativePairingStatus::Active { .. })
    ));
    assert!(initial.purge().is_none());
    assert!(refresh.selection_failure().is_none());
    assert_eq!(refresh.read().unwrap().result(), &Err(ReadFailure::Refused));
    let recheck = refresh
        .recheck()
        .expect("read refusal must capture its own pinned status");
    let nessa_device_client::SessionStatus::ReadOnly(Ok(observation)) = recheck else {
        panic!("expected completed current read-only enrollment operation");
    };
    assert_eq!(observation.enrollment(), &Err(NativeClientError::Refused));
    assert!(recheck.purge().is_none());
    assert!(native.purge_receipt(&setup.receiver).unwrap().is_none());
    let page = native.catalogue_page(None, 16).unwrap().unwrap();
    assert!(page.entries().len() >= 2);
    let conversation = ConversationId::new(&setup.conversation).unwrap();
    let RetainedTranscript::Retained(partial) =
        native.transcript(&conversation, Uuid::nil()).unwrap()
    else {
        panic!("confirmed page must remain available after a generic refusal");
    };
    assert!(partial.progress().downloaded() > 0);
    assert!(partial.progress().downloaded() > partial.progress().applied());
    assert!(partial.progress().facts() <= partial.progress().applied());
    native.close().unwrap();
    drop(gateway);
    let offline = NativeSession::open(state_root, state_directory, address).unwrap();
    let RetainedTranscript::Retained(saved) =
        offline.transcript(&conversation, Uuid::nil()).unwrap()
    else {
        panic!("offline reopen lost confirmed partial history");
    };
    assert_eq!(saved.progress(), partial.progress());
    // Startup retires the previous read owner: retained facts stay unchanged,
    // while the passive projection reports that it has not been refreshed.
    let mut expected = serde_json::to_value(partial.view()).unwrap();
    assert_eq!(expected["transcriptState"], "unknown");
    expected["transcriptState"] = Value::from("stale");
    assert_eq!(serde_json::to_value(saved.view()).unwrap(), expected);
    offline.close().unwrap();
}

#[test]
fn native_budget_after_catalogue_progress_is_incomplete_without_a_refusal_recheck() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let _gateway = Gateway::start_mode(&root, "budget-head");
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let address: SocketAddr = profile["gatewayAddress"].as_str().unwrap().parse().unwrap();
    let state_root = Path::new(profile["stateRoot"].as_str().unwrap());
    let state_directory = Path::new(profile["stateDirectory"].as_str().unwrap());
    let clock = Arc::new(ManualClock::default());
    let native =
        NativeSession::open_with_clock(state_root, state_directory, address, clock.clone())
            .unwrap();
    let stop = ReadStop::new();
    let phase = root.join("budget-head-entered");
    let (refresh, advanced) = std::thread::scope(|scope| {
        let advance = scope.spawn(|| clock.expire_at_head(&phase, &stop));
        let refresh = watched(&native, || native.refresh(stop.clone()));
        if refresh.is_err() {
            stop.stop();
        }
        (refresh, advance.join().unwrap())
    });
    assert!(advanced, "actual source head was reached");
    let refresh = refresh.unwrap();
    assert!(matches!(
        refresh.initial().observation(),
        Some(NativePairingStatus::Active { .. })
    ));
    assert!(refresh.initial().purge().is_none());
    assert!(refresh.selection_failure().is_none());
    let report = refresh.read().unwrap().result().as_ref().unwrap();
    assert!(report.moved);
    assert!(!report.complete);
    assert!(report.conversations >= 2);
    assert!(refresh.recheck().is_none());
    assert!(refresh.read().unwrap().withdrawn().is_empty());
    let page = native.catalogue_page(None, 16).unwrap().unwrap();
    assert!(page.entries().len() >= 2);
    assert!(native
        .purge_receipt(page.progress().scope.receiver().as_str())
        .unwrap()
        .is_none());
    native.close().unwrap();
}

#[test]
fn native_session_keeps_paired_records_offline_reopens_and_purges_on_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    let gateway = Gateway::start(&root);
    let setup: Setup =
        serde_json::from_slice(&std::fs::read(root.join("setup.json")).unwrap()).unwrap();
    let profile: Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
    let address: SocketAddr = profile["gatewayAddress"].as_str().unwrap().parse().unwrap();
    let state_root = Path::new(profile["stateRoot"].as_str().unwrap());
    let state_directory = Path::new(profile["stateDirectory"].as_str().unwrap());
    // This lifecycle assertion requires a completed pass, not wall-clock expiry.
    // The separate budget regression advances this same clock explicitly.
    let open = || {
        NativeSession::open_with_clock(
            state_root,
            state_directory,
            address,
            Arc::new(ManualClock::default()),
        )
        .unwrap()
    };
    let native = open();
    let refresh = watched(&native, || native.refresh(ReadStop::new())).unwrap();
    assert!(matches!(
        refresh.initial().observation(),
        Some(NativePairingStatus::Active { .. })
    ));
    assert!(refresh.initial().purge().is_none());
    assert!(refresh.selection_failure().is_none());
    assert!(refresh.read().unwrap().result().as_ref().unwrap().complete);
    assert!(
        refresh
            .read()
            .unwrap()
            .result()
            .as_ref()
            .unwrap()
            .conversations
            >= 2
    );
    assert!(refresh.read().unwrap().withdrawn().is_empty());
    assert!(refresh.recheck().is_none());
    let page = native.catalogue_page(None, 16).unwrap().unwrap();
    assert_eq!(page.progress().scope.receiver().as_str(), setup.receiver);
    assert!(page
        .entries()
        .iter()
        .any(|entry| entry.manifest().key.id.as_str() == setup.conversation));
    let conversation = ConversationId::new(&setup.conversation).unwrap();
    let RetainedTranscript::Retained(original) =
        native.transcript(&conversation, Uuid::nil()).unwrap()
    else {
        panic!("paired canonical records were not projected");
    };
    assert!(original.progress().downloaded() > 0);
    assert_eq!(
        original.progress().downloaded(),
        original.progress().applied()
    );
    assert!(original.progress().facts() > 0);
    let view = serde_json::to_value(original.view()).unwrap();
    native.close().unwrap();
    assert!(matches!(
        native.transcript(&conversation, Uuid::nil()),
        Err(SessionFailure::Closed)
    ));
    drop(gateway);
    let offline = open();
    let RetainedTranscript::Retained(saved) =
        offline.transcript(&conversation, Uuid::nil()).unwrap()
    else {
        panic!("offline session lost saved records");
    };
    assert_eq!(saved.progress(), original.progress());
    assert_eq!(serde_json::to_value(saved.view()).unwrap(), view);
    offline.close().unwrap();
    let restarted = Gateway::start(&root);
    let resumed = open();
    let refresh = watched(&resumed, || resumed.refresh(ReadStop::new())).unwrap();
    assert!(refresh.read().unwrap().result().as_ref().unwrap().complete);
    assert!(!refresh.read().unwrap().result().as_ref().unwrap().moved);
    drop(restarted);
    let _revoked = Gateway::start_mode(&root, "revoke");
    let terminal = watched(&resumed, || resumed.refresh(ReadStop::new())).unwrap();
    let status = terminal.initial();
    assert!(matches!(
        status.observation(),
        Some(NativePairingStatus::Terminal { .. })
    ));
    let purge = status.purge().unwrap().as_ref().unwrap();
    assert!(purge.records() > 0);
    assert!(purge.transcripts() > 0);
    assert!(purge.catalogue_entries() >= 2);
    assert!(terminal.read().is_none());
    assert!(terminal.recheck().is_none());
    assert_eq!(
        resumed.purge_receipt(&setup.receiver).unwrap().as_ref(),
        Some(purge)
    );
    assert!(matches!(
        resumed.transcript(&conversation, Uuid::nil()),
        Err(SessionFailure::NoCredential)
    ));
    // Escaped snapshots remain historical owned evidence after terminal purge.
    assert_eq!(serde_json::to_value(original.view()).unwrap(), view);
    resumed.close().unwrap();
    let recovered = open();
    assert_eq!(
        recovered.purge_receipt(&setup.receiver).unwrap().as_ref(),
        Some(purge)
    );
    recovered.close().unwrap();
}
