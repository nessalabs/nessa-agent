//! Actual same-owner original binding, enrollment publication and cold correlation.
use super::*;
use crate::{
    application::pairing::{ClientPendingStore, OwnerPhoneOriginalStore, OwnerPhoneReservation},
    domain::{
        pairing::{
            AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, InvitationId,
            OwnerPhoneContext, OwnerPhoneRequestId, OwnerPhoneSource, PublicIntent,
        },
        AudienceId, MembershipId, OrganizationId, PrincipalId, Resource,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    store: FilePairingState,
    request: OwnerPhoneRequest,
    digest: [u8; 32],
    consent: DisclosedConsent,
}
fn seed(value: u8) -> PrivateKeyMaterial {
    PrivateKeyMaterial::new(Zeroizing::new([value; 32]))
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        // Windows validates an ordinary drive path and creates the protected
        // DACL. A canonicalized temp path is a reparse point there, which the
        // private-directory check refuses. Unix resolves the temp alias first.
        #[cfg(windows)]
        let root = temporary.path().join("root");
        #[cfg(not(windows))]
        let root = temporary.path().canonicalize().unwrap().join("root");
        nessa_local_storage::create_directory(&root).unwrap();
        nessa_local_storage::create_directory_beneath(&root, Path::new("private")).unwrap();
        let store = FilePairingState::open(&root, Path::new("private")).unwrap();
        let source = OwnerPhoneSource::new(
            PrincipalId::new("owner").unwrap(),
            MembershipId::new("member").unwrap(),
            AudienceId::new("gateway").unwrap(),
            CredentialId::new("bootstrap").unwrap(),
            Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
        )
        .unwrap();
        let context = OwnerPhoneContext::new(
            source,
            NativeIdentity::restore(seed(8)).unwrap().public_spki(),
            "127.0.0.1:1234".parse().unwrap(),
        )
        .unwrap();
        let reservation =
            OwnerPhoneReservation::new(OwnerPhoneRequestId::new([1; 16]).unwrap(), context);
        store
            .reserve_original(reservation.clone())
            .unwrap()
            .prepare(&seed(9))
            .unwrap();
        let identity = NativeIdentity::restore(seed(9)).unwrap();
        let request = reservation
            .context()
            .request(reservation.id(), identity.device_key())
            .unwrap();
        let signature = identity.sign_owner_phone_request(&request).unwrap();
        let digest = *verify_owner_phone_request(request.clone(), &signature)
            .unwrap()
            .digest();
        let public = PublicIntent::new(
            InvitationId::new([2; 16]),
            AttemptId::new([3; 16]),
            ConsentIntentId::new([4; 16]),
            1,
            600_000,
            ConsentClass::OwnerPhoneReadWrite,
        )
        .unwrap();
        let source = request.source();
        let intent = ConsentIntent::new(
            public.consent(),
            public.generation(),
            source.audience().clone(),
            source.owner().clone(),
            source.membership().clone(),
            source.resource().clone(),
            ConsentClass::OwnerPhoneReadWrite,
        )
        .unwrap();
        let consent = DisclosedConsent::from_intent(public, &intent).unwrap();
        Self {
            _temporary: temporary,
            root,
            store,
            request,
            digest,
            consent,
        }
    }
    fn original(&self) -> PathBuf {
        self.root.join("private/owner-phone-original")
    }
    fn enrollment(&self) -> PathBuf {
        self.root.join("private/client-enrollment")
    }
    fn adopt(&self, issued: bool) -> Result<OwnerPhoneAdoption, OwnerPhoneAdoptionFailure> {
        let credential = CredentialId::new("issued-phone").unwrap();
        let receiver = ResourceId::new("receiver").unwrap();
        self.store.adopt_owner_phone(
            &self.request,
            &self.digest,
            &self.consent,
            if issued {
                Some((&credential, &receiver))
            } else {
                None
            },
        )
    }
}

#[test]
fn explicit_pending_and_first_active_adoption_replay_cold_with_the_exact_original() {
    for first_active in [false, true] {
        let fixture = Fixture::new();
        assert_eq!(
            fixture
                .store
                .load_original()
                .unwrap()
                .unwrap()
                .public_intent(),
            None
        );
        let first = fixture.adopt(first_active).unwrap();
        assert_eq!(first.public(), fixture.consent.public());
        assert_eq!(
            first.kind(),
            if first_active {
                OwnerPhoneEnrollmentKind::Issued
            } else {
                OwnerPhoneEnrollmentKind::Pending
            }
        );
        let original = fs::read(fixture.original()).unwrap();
        let enrollment = fs::read(fixture.enrollment()).unwrap();
        assert_eq!(fixture.adopt(first_active).unwrap(), first);
        assert!(fs::read(fixture.original()).unwrap() == original);
        assert!(fs::read(fixture.enrollment()).unwrap() == enrollment);
        if !first_active {
            fixture.adopt(true).unwrap();
        }
        let root = fixture.root.clone();
        drop(fixture.store);
        let cold = FilePairingState::open(&root, Path::new("private")).unwrap();
        let original = cold.load_original().unwrap().unwrap();
        assert_eq!(original.public_intent(), Some(first.public()));
        let issued = cold.load_credential().unwrap().unwrap();
        assert_eq!(issued.intent(), first.public());
        assert_eq!(issued.credential().as_str(), "issued-phone");
        assert_eq!(issued.receiver().as_str(), "receiver");
        assert_eq!(issued.key().expose_bytes(), &[9; 32]);
        assert_eq!(
            issued.gateway_pin(),
            original.reservation().context().gateway_pin()
        );
    }
}

#[test]
fn changed_digest_or_disclosed_scope_refuses_before_binding_or_enrollment() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.original()).unwrap();
    let mut wrong = fixture.digest;
    wrong[0] ^= 1;
    assert_eq!(
        fixture
            .store
            .adopt_owner_phone(&fixture.request, &wrong, &fixture.consent, None),
        Err(OwnerPhoneAdoptionFailure::OriginalRead(
            PrivateStateError::Conflict
        ))
    );
    let consent = DisclosedConsent::with_grants(
        fixture.consent.public(),
        AudienceId::new("another-gateway").unwrap(),
        fixture.consent.grants().cloned().collect(),
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .adopt_owner_phone(&fixture.request, &fixture.digest, &consent, None),
        Err(OwnerPhoneAdoptionFailure::OriginalRead(
            PrivateStateError::Conflict
        ))
    );
    assert!(fs::read(fixture.original()).unwrap() == before);
    assert!(!fixture.enrollment().exists());
    fixture.adopt(false).unwrap();
}

#[test]
fn generic_writers_and_issued_downgrade_or_changed_selectors_preserve_exact_history() {
    let fixture = Fixture::new();
    fixture.adopt(false).unwrap();
    let pending = fs::read(fixture.enrollment()).unwrap();
    assert_eq!(
        fixture.store.save_credential(
            &CredentialId::new("issued-phone").unwrap(),
            &ResourceId::new("receiver").unwrap(),
            fixture.consent.public()
        ),
        Err(PrivateStateError::Conflict)
    );
    assert_eq!(
        fixture.store.save_pending(
            &seed(9),
            fixture.request.gateway_pin(),
            fixture.consent.public(),
            Some(fixture.consent.public())
        ),
        Err(PrivateStateError::Conflict)
    );
    assert!(fs::read(fixture.enrollment()).unwrap() == pending);
    fixture.adopt(true).unwrap();
    let issued = fs::read(fixture.enrollment()).unwrap();
    assert_eq!(
        fixture.adopt(false),
        Err(OwnerPhoneAdoptionFailure::OriginalRead(
            PrivateStateError::Conflict
        ))
    );
    for (credential, receiver) in [
        ("another-issued", "receiver"),
        ("issued-phone", "another-receiver"),
    ] {
        assert_eq!(
            fixture.store.adopt_owner_phone(
                &fixture.request,
                &fixture.digest,
                &fixture.consent,
                Some((
                    &CredentialId::new(credential).unwrap(),
                    &ResourceId::new(receiver).unwrap()
                ))
            ),
            Err(OwnerPhoneAdoptionFailure::OriginalRead(
                PrivateStateError::Conflict
            ))
        );
        assert!(fs::read(fixture.enrollment()).unwrap() == issued);
    }
    fixture.adopt(true).unwrap();
}

#[cfg(unix)]
#[test]
fn a_failed_second_acknowledgement_preserves_the_exact_first_lineage_fact() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.adopt(true).unwrap();
    let bytes = fs::read(fixture.enrollment()).unwrap();
    fs::set_permissions(fixture.enrollment(), fs::Permissions::from_mode(0o400)).unwrap();
    let failure = fixture.adopt(true).unwrap_err();
    assert_eq!(
        failure.phase(),
        crate::application::pairing::OwnerPhoneAdoptionPhase::PublishEnrollment
    );
    assert_eq!(failure.error(), PrivateStateError::Uncertain);
    assert_eq!(
        failure.acknowledged_lineage(),
        Some(fixture.consent.public())
    );
    assert!(fs::read(fixture.enrollment()).unwrap() == bytes);
    fs::set_permissions(fixture.enrollment(), fs::Permissions::from_mode(0o600)).unwrap();
    fixture.adopt(true).unwrap();
}

#[cfg(unix)]
#[test]
fn cold_joint_selection_requires_acknowledgement_of_the_actual_original() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.adopt(true).unwrap();
    assert!(fs::read(fixture.original()).is_ok());
    fs::set_permissions(fixture.original(), fs::Permissions::from_mode(0o400)).unwrap();
    assert!(matches!(
        fixture.store.load_credential(),
        Err(PrivateStateError::Uncertain)
    ));
    fs::set_permissions(fixture.original(), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(fixture.store.load_credential().unwrap().is_some());
}

#[test]
fn each_other_valid_public_lineage_refuses_cold_selection_and_terminal_removal() {
    let fixture = Fixture::new();
    fixture.adopt(true).unwrap();
    let before = fs::read(fixture.enrollment()).unwrap();
    // The shared pending codec owns these six public fields; mutate valid values
    // while preserving the exact original seed, pin and credential selectors.
    for offset in [4, 79, 80, 96, 112, 135, 143] {
        let mut changed = before.clone();
        changed[offset] = changed[offset].wrapping_add(1);
        fs::write(fixture.enrollment(), &changed).unwrap();
        assert!(matches!(
            fixture.store.load_credential(),
            Err(PrivateStateError::Conflict)
        ));
        assert_eq!(
            fixture.store.end_enrollment(fixture.consent.public()),
            Err(PrivateStateError::Conflict)
        );
        assert!(fs::read(fixture.enrollment()).unwrap() == changed);
    }
    let mut other_class = before.clone();
    other_class[..4].copy_from_slice(b"NSCA");
    fs::write(fixture.enrollment(), other_class).unwrap();
    assert!(matches!(
        fixture.store.load_credential(),
        Err(PrivateStateError::Conflict)
    ));
    fs::write(fixture.enrollment(), &before).unwrap();
    let original = fs::read(fixture.original()).unwrap();
    fixture
        .store
        .end_enrollment(fixture.consent.public())
        .unwrap();
    assert!(!fixture.enrollment().exists());
    assert!(fs::read(fixture.original()).unwrap() == original);
    fixture.adopt(true).unwrap();
}

#[test]
fn bound_original_does_not_change_to_a_different_valid_public_lineage() {
    let fixture = Fixture::new();
    fixture.adopt(false).unwrap();
    let original = fs::read(fixture.original()).unwrap();
    let enrollment = fs::read(fixture.enrollment()).unwrap();
    let public = fixture
        .consent
        .public()
        .with_attempt(AttemptId::new([99; 16]));
    let consent = DisclosedConsent::with_grants(
        public,
        fixture.consent.audience().clone(),
        fixture.consent.grants().cloned().collect(),
    )
    .unwrap();
    assert_eq!(
        fixture
            .store
            .adopt_owner_phone(&fixture.request, &fixture.digest, &consent, None),
        Err(OwnerPhoneAdoptionFailure::OriginalRead(
            PrivateStateError::Conflict
        ))
    );
    assert!(fs::read(fixture.original()).unwrap() == original);
    assert!(fs::read(fixture.enrollment()).unwrap() == enrollment);
}

#[test]
fn contradictory_canonical_originals_refuse_before_either_publication() {
    let fixture = Fixture::new();
    let before = fs::read(fixture.original()).unwrap();
    let same = &fixture.request;
    let other = NativeIdentity::restore(seed(10)).unwrap();
    for (id, pin, endpoint, key) in [
        (
            OwnerPhoneRequestId::new([7; 16]).unwrap(),
            *same.gateway_pin(),
            same.endpoint(),
            same.device_key(),
        ),
        (
            same.id(),
            other.public_spki(),
            same.endpoint(),
            same.device_key(),
        ),
        (
            same.id(),
            *same.gateway_pin(),
            "127.0.0.1:1235".parse().unwrap(),
            same.device_key(),
        ),
        (
            same.id(),
            *same.gateway_pin(),
            same.endpoint(),
            other.device_key(),
        ),
    ] {
        let request =
            OwnerPhoneRequest::new(id, same.source().clone(), pin, endpoint, key).unwrap();
        let identity =
            NativeIdentity::restore(seed(if key == other.device_key() { 10 } else { 9 })).unwrap();
        let signature = identity.sign_owner_phone_request(&request).unwrap();
        let digest = *verify_owner_phone_request(request.clone(), &signature)
            .unwrap()
            .digest();
        assert_eq!(
            fixture
                .store
                .adopt_owner_phone(&request, &digest, &fixture.consent, None),
            Err(OwnerPhoneAdoptionFailure::OriginalRead(
                PrivateStateError::Conflict
            ))
        );
        // A valid request's own digest and the actual original's digest both
        // fail: the expected request itself must name the saved original.
        assert_eq!(
            fixture
                .store
                .adopt_owner_phone(&request, &fixture.digest, &fixture.consent, None,),
            Err(OwnerPhoneAdoptionFailure::OriginalRead(
                PrivateStateError::Conflict
            ))
        );
        assert!(fs::read(fixture.original()).unwrap() == before);
        assert!(!fixture.enrollment().exists());
    }
    fixture.adopt(true).unwrap();
}

#[test]
fn canonical_disclosure_in_another_scope_or_class_does_not_bind_the_original() {
    let fixture = Fixture::new();
    let original = fs::read(fixture.original()).unwrap();
    let public = fixture.consent.public();
    let foreign = Resource::new(
        OrganizationId::new("another-org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let foreign = DisclosedConsent::with_grants(
        public,
        fixture.consent.audience().clone(),
        ConsentClass::OwnerPhoneReadWrite.grants(foreign).unwrap(),
    )
    .unwrap();
    let legacy = PublicIntent::new(
        public.invitation(),
        public.attempt(),
        public.consent(),
        public.generation(),
        public.expiry_ms(),
        ConsentClass::DeviceRead,
    )
    .unwrap();
    let legacy = DisclosedConsent::with_grants(
        legacy,
        fixture.consent.audience().clone(),
        ConsentClass::DeviceRead
            .grants(fixture.request.source().resource().clone())
            .unwrap(),
    )
    .unwrap();
    for consent in [foreign, legacy] {
        assert_eq!(
            fixture
                .store
                .adopt_owner_phone(&fixture.request, &fixture.digest, &consent, None),
            Err(OwnerPhoneAdoptionFailure::OriginalRead(
                PrivateStateError::Conflict
            ))
        );
        assert!(fs::read(fixture.original()).unwrap() == original);
        assert!(!fixture.enrollment().exists());
    }
    fixture.adopt(true).unwrap();
}

#[cfg(unix)]
#[test]
fn semantic_issued_conflict_precedes_any_original_acknowledgement() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.adopt(true).unwrap();
    fs::set_permissions(fixture.original(), fs::Permissions::from_mode(0o400)).unwrap();
    assert_eq!(
        fixture.adopt(false),
        Err(OwnerPhoneAdoptionFailure::OriginalRead(
            PrivateStateError::Conflict
        ))
    );
    assert_eq!(
        fixture.store.adopt_owner_phone(
            &fixture.request,
            &fixture.digest,
            &fixture.consent,
            Some((
                &CredentialId::new("another-issued").unwrap(),
                &ResourceId::new("receiver").unwrap()
            ))
        ),
        Err(OwnerPhoneAdoptionFailure::OriginalRead(
            PrivateStateError::Conflict
        ))
    );
    assert_eq!(
        fixture.adopt(true),
        Err(OwnerPhoneAdoptionFailure::BindLineage(
            PrivateStateError::Uncertain
        ))
    );
    fs::set_permissions(fixture.original(), fs::Permissions::from_mode(0o600)).unwrap();
    fixture.adopt(true).unwrap();
}

#[test]
fn cold_bound_original_without_enrollment_replays_only_the_exact_lineage() {
    for issued in [false, true] {
        let fixture = Fixture::new();
        fixture.adopt(false).unwrap();
        let original_path = fixture.original();
        let enrollment_path = fixture.enrollment();
        let original_bytes = fs::read(&original_path).unwrap();
        // Represent the recorded crash boundary after lineage acknowledgement
        // but before any enrollment file is retained (plan ordering row 67).
        fs::remove_file(&enrollment_path).unwrap();
        drop(fixture.store);
        let store = FilePairingState::open(&fixture.root, Path::new("private")).unwrap();
        let original = store.load_original().unwrap().unwrap();
        assert_eq!(original.public_intent(), Some(fixture.consent.public()));
        assert_eq!(original.key().unwrap().expose_bytes(), &[9; 32]);
        assert!(store.load_pending().unwrap().is_none());
        assert!(store.load_credential().unwrap().is_none());
        let changed = DisclosedConsent::with_grants(
            fixture
                .consent
                .public()
                .with_attempt(AttemptId::new([99; 16])),
            fixture.consent.audience().clone(),
            fixture.consent.grants().cloned().collect(),
        )
        .unwrap();
        assert_eq!(
            store.adopt_owner_phone(&fixture.request, &fixture.digest, &changed, None),
            Err(OwnerPhoneAdoptionFailure::OriginalRead(
                PrivateStateError::Conflict
            ))
        );
        assert!(!enrollment_path.exists());
        assert!(fs::read(&original_path).unwrap() == original_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&original_path, fs::Permissions::from_mode(0o400)).unwrap();
            assert_eq!(
                store.adopt_owner_phone(&fixture.request, &fixture.digest, &fixture.consent, None),
                Err(OwnerPhoneAdoptionFailure::BindLineage(
                    PrivateStateError::Uncertain
                ))
            );
            assert!(!enrollment_path.exists());
            assert!(fs::read(&original_path).unwrap() == original_bytes);
            fs::set_permissions(&original_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let credential = CredentialId::new("issued-phone").unwrap();
        let receiver = ResourceId::new("receiver").unwrap();
        let result = store
            .adopt_owner_phone(
                &fixture.request,
                &fixture.digest,
                &fixture.consent,
                if issued {
                    Some((&credential, &receiver))
                } else {
                    None
                },
            )
            .unwrap();
        assert_eq!(result.public(), fixture.consent.public());
        assert!(fs::read(&original_path).unwrap() == original_bytes);
        if issued {
            let saved = store.load_credential().unwrap().unwrap();
            assert_eq!(saved.key().expose_bytes(), &[9; 32]);
            assert_eq!(saved.gateway_pin(), fixture.request.gateway_pin());
            assert_eq!(saved.intent(), fixture.consent.public());
            assert_eq!(saved.credential(), &credential);
            assert_eq!(saved.receiver(), &receiver);
        } else {
            let saved = store.load_pending().unwrap().unwrap();
            assert_eq!(saved.key().expose_bytes(), &[9; 32]);
            assert_eq!(saved.gateway_pin(), fixture.request.gateway_pin());
            assert_eq!(saved.intent(), fixture.consent.public());
        }
    }
}
