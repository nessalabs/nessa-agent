//! Transition-table evidence for original expiry/credential-stop observations.
use crate::webhooks::domain::{
    ExpiryObserver, LifecycleError, RegistrationLifecycle, RegistrationPhase, StopObservation,
    StopReason,
};
use nessa_auth::domain::PrincipalId;
use uuid::Uuid;

fn owner(id: &str) -> PrincipalId {
    PrincipalId::new(id).unwrap()
}

#[test]
fn restoration_accepts_only_the_closed_phase_fact_relationship() {
    let expired = StopObservation::new(
        StopReason::Expired {
            observed_by: ExpiryObserver::Restart,
        },
        12_000,
    );
    let revoked = StopObservation::new(
        StopReason::Revoked {
            initiator: owner("original-admin"),
        },
        11_000,
    );
    for (phase, fact, valid) in [
        (RegistrationPhase::Active, None, true),
        (RegistrationPhase::Active, Some(expired.clone()), false),
        (RegistrationPhase::Active, Some(revoked.clone()), false),
        (RegistrationPhase::ExpiryObserved, None, false),
        (
            RegistrationPhase::ExpiryObserved,
            Some(expired.clone()),
            true,
        ),
        (
            RegistrationPhase::ExpiryObserved,
            Some(revoked.clone()),
            false,
        ),
        (RegistrationPhase::Stopped, None, false),
        (RegistrationPhase::Stopped, Some(expired), true),
        (RegistrationPhase::Stopped, Some(revoked), true),
    ] {
        assert_eq!(RegistrationLifecycle::restore(phase, fact).is_ok(), valid);
    }
}

#[test]
fn expiry_requires_observation_and_retains_the_first_actual_reader() {
    for reader in [
        ExpiryObserver::Owner(owner("actual-issuer")),
        ExpiryObserver::Delivery(Uuid::new_v4()),
        ExpiryObserver::Restart,
    ] {
        let active = RegistrationLifecycle::active();
        assert_eq!(
            active.stop_observed(),
            Err(LifecycleError::ObservationRequired)
        );
        let (observed, change) = active.observe_expiry(reader.clone(), 12_000);
        let evidence = change.unwrap();
        assert_eq!(active.phase(), RegistrationPhase::Active);
        assert_eq!(
            (evidence.before(), evidence.after()),
            (RegistrationPhase::Active, RegistrationPhase::ExpiryObserved)
        );
        assert_eq!(observed.stopped(), None);
        let original = StopObservation::new(
            StopReason::Expired {
                observed_by: reader,
            },
            12_000,
        );
        assert_eq!(evidence.observation(), &original);
        assert_eq!(observed.observation(), Some(&original));
        let (same, repeated) =
            observed.observe_expiry(ExpiryObserver::Owner(owner("later-reader")), 13_000);
        assert_eq!(same, observed);
        assert_eq!(repeated, None);
        let restored =
            RegistrationLifecycle::restore(observed.phase(), observed.observation().cloned())
                .unwrap();
        let (stopped, change) = restored.stop_observed().unwrap();
        let evidence = change.unwrap();
        assert_eq!(
            (evidence.before(), evidence.after()),
            (
                RegistrationPhase::ExpiryObserved,
                RegistrationPhase::Stopped
            )
        );
        assert_eq!(stopped.stopped(), Some(&original));
        assert_eq!(evidence.observation(), &original);
        let (same, repeated) = stopped.stop_observed().unwrap();
        assert_eq!(same, stopped);
        assert_eq!(repeated, None);
        let (same, repeated) = stopped.stop_revoked(owner("later-admin"), 15_000);
        assert_eq!(same, stopped);
        assert_eq!(repeated, None);
        let (same, repeated) = stopped.observe_expiry(ExpiryObserver::Restart, 16_000);
        assert_eq!(same, stopped);
        assert_eq!(repeated, None);
    }
}

#[test]
fn actual_revocation_stops_directly_and_does_not_replace_an_earlier_expiry_fact() {
    let active = RegistrationLifecycle::active();
    let (revoked, change) = active.stop_revoked(owner("canonical-revoker"), 20_000);
    let fact = StopObservation::new(
        StopReason::Revoked {
            initiator: owner("canonical-revoker"),
        },
        20_000,
    );
    let evidence = change.unwrap();
    assert_eq!(
        (evidence.before(), evidence.after()),
        (RegistrationPhase::Active, RegistrationPhase::Stopped)
    );
    assert_eq!(evidence.observation(), &fact);
    assert_eq!(revoked.stopped(), Some(&fact));
    let (observed, change) =
        active.observe_expiry(ExpiryObserver::Owner(owner("first-reader")), 10_000);
    let original = change.unwrap().observation().clone();
    let (stopped, change) = observed.stop_revoked(owner("later-canonical-revoker"), 21_000);
    let evidence = change.unwrap();
    assert_eq!(
        (evidence.before(), evidence.after()),
        (
            RegistrationPhase::ExpiryObserved,
            RegistrationPhase::Stopped
        )
    );
    assert_eq!(evidence.observation(), &original);
    assert_eq!(stopped.stopped(), Some(&original));
}

#[test]
fn queued_compatibility_is_shared_by_admission_and_restoration() {
    let active = RegistrationLifecycle::active();
    let (observed, _) = active.observe_expiry(ExpiryObserver::Restart, 10_000);
    let (stopped, _) = observed.stop_observed().unwrap();
    assert!(active.retains_queued());
    assert!(observed.retains_queued());
    assert!(!stopped.retains_queued());
}
