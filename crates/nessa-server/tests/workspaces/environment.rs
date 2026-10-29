//! Pure capability and identity rules; no provider DTOs, runtime or storage.
use super::{
    AdmissionRefusal, AdmissionRejection, EnvironmentConfiguration, EnvironmentIdentity, Failure,
    ObservedEnvironment,
};
fn configuration() -> EnvironmentConfiguration {
    EnvironmentConfiguration {
        safe_for_third_parties: false,
        pass_github: false,
        pass_secrets: false,
        pass_sandbox_credentials: false,
        pass_agents_credentials: true,
        has_contents: false,
        has_secret_files: false,
        has_repositories: false,
        owner_default_profile: true,
        proxy_ai_credentials: false,
    }
}
#[test]
fn only_the_explicit_agent_credential_posture_is_admitted() {
    configuration().admit().unwrap();
    for (label, rejected) in [
        (
            "safe",
            EnvironmentConfiguration {
                safe_for_third_parties: true,
                ..configuration()
            },
        ),
        (
            "github",
            EnvironmentConfiguration {
                pass_github: true,
                ..configuration()
            },
        ),
        (
            "secrets",
            EnvironmentConfiguration {
                pass_secrets: true,
                ..configuration()
            },
        ),
        (
            "sandbox",
            EnvironmentConfiguration {
                pass_sandbox_credentials: true,
                ..configuration()
            },
        ),
        (
            "agents",
            EnvironmentConfiguration {
                pass_agents_credentials: false,
                ..configuration()
            },
        ),
        (
            "contents",
            EnvironmentConfiguration {
                has_contents: true,
                ..configuration()
            },
        ),
        (
            "files",
            EnvironmentConfiguration {
                has_secret_files: true,
                ..configuration()
            },
        ),
        (
            "repositories",
            EnvironmentConfiguration {
                has_repositories: true,
                ..configuration()
            },
        ),
        (
            "profile",
            EnvironmentConfiguration {
                owner_default_profile: false,
                ..configuration()
            },
        ),
        (
            "proxy",
            EnvironmentConfiguration {
                proxy_ai_credentials: true,
                ..configuration()
            },
        ),
    ] {
        assert_eq!(
            rejected.admit(),
            Err(Failure::EnvironmentUnavailable),
            "{label}"
        );
    }
}
#[test]
fn identities_and_observed_receipts_must_name_the_same_immutable_version() {
    let identity = EnvironmentIdentity::new(
        "11111111-1111-4111-8111-111111111111".into(),
        "nessa".into(),
        "22222222-2222-4222-8222-222222222222".into(),
        2,
    )
    .unwrap();
    assert_eq!(
        identity.check(&ObservedEnvironment {
            name: Some("nessa".into()),
            version: Some(2)
        }),
        Ok(())
    );
    for (observed, cause) in [
        (
            ObservedEnvironment {
                name: None,
                version: Some(2),
            },
            AdmissionRefusal::MissingEnvironment,
        ),
        (
            ObservedEnvironment {
                name: Some("other".into()),
                version: Some(2),
            },
            AdmissionRefusal::WrongEnvironment,
        ),
        (
            ObservedEnvironment {
                name: Some("nessa".into()),
                version: None,
            },
            AdmissionRefusal::MissingVersion,
        ),
        (
            ObservedEnvironment {
                name: Some("nessa".into()),
                version: Some(3),
            },
            AdmissionRefusal::WrongVersion,
        ),
    ] {
        assert_eq!(identity.check(&observed), Err(cause));
    }
    for (id, name, version_id, version) in [
        ("bad", "nessa", identity.version_id(), 2),
        (identity.id(), "other", identity.version_id(), 2),
        (identity.id(), "nessa", "bad", 2),
        (identity.id(), "nessa", identity.version_id(), 0),
    ] {
        assert_eq!(
            EnvironmentIdentity::new(id.into(), name.into(), version_id.into(), version),
            Err(Failure::EnvironmentUnavailable)
        );
    }
}
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
