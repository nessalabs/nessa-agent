//! Original issuance is a retained fact, distinct from current revoked state.
use nessa_auth::{
    application::{dto::CredentialTransitionDto, origin::CredentialOrigin},
    domain::*,
};

fn fixture() -> (Credential, CredentialTransitionDto) {
    let credential = Credential::new(
        CredentialId::new("key").unwrap(),
        PrincipalId::new("integration").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        10,
        Some(100),
        vec![],
    )
    .unwrap();
    let issued = credential
        .issued(
            IssuanceCause::WebhookIssue,
            Initiator::Principal(PrincipalId::new("owner").unwrap()),
        )
        .unwrap();
    let record = CredentialTransitionDto::record(&issued, 3, 2, Some("register".into()));
    (credential, record)
}

#[test]
fn original_webhook_origin_survives_revocation_without_synthetic_history() {
    let (mut credential, record) = fixture();
    let origin = CredentialOrigin::restore(record.clone(), &credential).unwrap();
    assert_eq!(origin.cause(), IssuanceCause::WebhookIssue);
    assert_eq!(origin.correlation(), Some("register"));
    assert_eq!((origin.sequence(), origin.revision()), (3, 2));
    assert!(!origin.matches(&credential, 1));
    assert!(origin.matches(&credential, 2));
    credential.restore_revoked_at(20).unwrap();
    let restored = CredentialOrigin::restore(record, &credential).unwrap();
    assert_eq!(restored, origin);
    assert!(restored.matches(&credential, 4));
    assert_eq!(restored.transition().after().revoked_at, None);
}

#[test]
fn malformed_or_cross_credential_origin_is_refused() {
    let (credential, record) = fixture();
    let changes: [fn(&mut CredentialTransitionDto); 10] = [
        |r| r.credential_id = "another".into(),
        |r| r.after.issued_at += 1,
        |r| r.after.expires_at = Some(99),
        |r| r.after.revoked_at = Some(20),
        |r| r.before = Some(r.after),
        |r| r.at += 1,
        |r| r.sequence = 0,
        |r| r.revision = 0,
        |r| r.correlation = Some(" ".into()),
        |r| r.correlation = Some("x".repeat(201)),
    ];
    for change in changes {
        let mut altered = record.clone();
        change(&mut altered);
        assert!(CredentialOrigin::restore(altered, &credential).is_err());
    }
}

#[test]
fn every_issuance_cause_round_trips_without_purpose_inference() {
    let (credential, _) = fixture();
    for cause in [
        IssuanceCause::Bootstrap,
        IssuanceCause::AdminIssue,
        IssuanceCause::WebhookIssue,
        IssuanceCause::DevicePairing,
        IssuanceCause::SurfaceProvision,
        IssuanceCause::OwnerRecovery,
    ] {
        let original = credential
            .issued(
                cause,
                Initiator::Principal(PrincipalId::new("owner").unwrap()),
            )
            .unwrap();
        let dto = CredentialTransitionDto::record(&original, 1, 1, Some("request".into()));
        let json = serde_json::to_vec(&dto).unwrap();
        let decoded = serde_json::from_slice(&json).unwrap();
        let origin = CredentialOrigin::restore(decoded, &credential).unwrap();
        assert_eq!(origin.cause(), cause);
        assert_eq!(origin.transition(), &original);
    }
}
