//! Pure capability and identity rules; no provider DTOs, runtime or storage.
use super::{AdmissionRefusal, AdmissionRejection, EnvironmentIdentity, ObservedEnvironment};
#[test]
fn refusal_evidence_never_retains_unbounded_or_controlled_provider_text() {
    for name in ["x".repeat(129), "bad\nname".into(), "non-ascii-é".into()] {
        let rejection = AdmissionRejection::new(
            AdmissionRefusal::WrongEnvironment,
            Some(ObservedEnvironment {
                name: Some(name),
                version: Some(2),
            }),
        );
        assert_eq!(rejection.cause(), AdmissionRefusal::InvalidReceipt);
        assert_eq!(rejection.observed(), None);
    }
}

#[test]
fn nil_and_noncanonical_environment_ids_never_become_admission_proof() {
    for invalid in [
        "00000000-0000-0000-0000-000000000000",
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA",
    ] {
        assert!(EnvironmentIdentity::new(
            invalid.into(),
            "nessa".into(),
            "22222222-2222-4222-8222-222222222222".into(),
            2
        )
        .is_err());
        assert!(EnvironmentIdentity::new(
            "11111111-1111-4111-8111-111111111111".into(),
            "nessa".into(),
            invalid.into(),
            2
        )
        .is_err());
    }
}
