use super::*;

#[test]
fn runtime_identity_and_correlation_are_immutable_validated_values() {
    let id = "b4a38c5b-cf70-4d90-9059-7d9a3a51c658";
    let request = RetirementRequest::new(
        id.into(),
        "a".repeat(64),
        id.into(),
        "c".repeat(64),
        "d".repeat(64),
    )
    .unwrap();
    assert_eq!(request.id(), id);
    assert_eq!(request.target().as_str(), "a".repeat(64));
    for fingerprint in ["".into(), "a".repeat(63), "g".repeat(64), "A".repeat(64)] {
        assert!(Fingerprint::new(fingerprint).is_err());
    }
    for invalid in ["", "arbitrary", "B4A38C5B-CF70-4D90-9059-7D9A3A51C658"] {
        assert!(RetirementRequest::new(
            invalid.into(),
            "a".repeat(64),
            id.into(),
            "c".repeat(64),
            "d".repeat(64)
        )
        .is_err());
    }
}

#[test]
fn admitted_retirement_evidence_requires_success_or_a_reported_failure() {
    let id = "b4a38c5b-cf70-4d90-9059-7d9a3a51c658";
    let request = RetirementRequest::new(
        id.into(),
        "b".repeat(64),
        id.into(),
        "c".repeat(64),
        "d".repeat(64),
    )
    .unwrap();
    let running = RunningRuntime::new("a".repeat(64), id.into(), 123, "c".repeat(64)).unwrap();
    let cause =
        RetirementCause::new("gateway".into(), "gateway_upgrade".into(), id.into()).unwrap();

    assert!(RetirementFence::new(request.clone(), &running, cause.clone(), true, false,).is_ok());
    assert!(RetirementFence::new(request.clone(), &running, cause.clone(), false, true,).is_ok());
    assert!(RetirementFence::new(request, &running, cause, false, false).is_err());
}

#[test]
fn evidence_cause_agrees_with_admission_and_confirmed_upgrade_authority() {
    let id = "b4a38c5b-cf70-4d90-9059-7d9a3a51c658";
    let request = RetirementRequest::new(
        id.into(),
        "b".repeat(64),
        id.into(),
        "c".repeat(64),
        "d".repeat(64),
    )
    .unwrap();
    let running = RunningRuntime::new("a".repeat(64), id.into(), 123, "c".repeat(64)).unwrap();

    assert!(validate_retirement_evidence(request.clone(), &running, None, false, true,).is_err());

    for cause in [
        RetirementCause::new(
            "system-supervisor".into(),
            "gateway_upgrade".into(),
            id.into(),
        )
        .unwrap(),
        RetirementCause::new("gateway".into(), "server_shutdown".into(), id.into()).unwrap(),
    ] {
        assert!(validate_retirement_evidence(
            request.clone(),
            &running,
            Some(cause.clone()),
            true,
            false,
        )
        .is_err());
        assert!(
            validate_retirement_evidence(request.clone(), &running, Some(cause), false, true,)
                .unwrap()
                .is_some()
        );
    }

    let rejected_request = RetirementRequest::new(
        id.into(),
        "b".repeat(64),
        "550e8400-e29b-41d4-a716-446655440000".into(),
        "c".repeat(64),
        "d".repeat(64),
    )
    .unwrap();
    assert!(
        validate_retirement_evidence(rejected_request, &running, None, false, true,)
            .unwrap()
            .is_none()
    );
}

/// The names this gateway writes are the published ones, the file the desktop
/// host reads too (ADR 221).
#[test]
fn refusal_names_are_the_published_ones() {
    let published: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../protocol/defaults/gateway-retirement-refusals.json"
    ))
    .unwrap();
    let published: Vec<&str> = published["refusals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    assert_eq!(
        [
            RetirementRefusal::DataMissing,
            RetirementRefusal::NotConfirmed
        ]
        .map(RetirementRefusal::as_str)
        .to_vec(),
        published
    );
    assert_eq!(
        RetirementRefusal::of(true, true),
        RetirementRefusal::DataMissing
    );
    assert_eq!(
        RetirementRefusal::of(true, false),
        RetirementRefusal::NotConfirmed
    );
    assert_eq!(
        RetirementRefusal::of(false, true),
        RetirementRefusal::NotConfirmed
    );
}
