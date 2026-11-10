//! Attribution has no credential class or capability of its own.
use nessa_auth::domain::{
    pairing::*, AudienceId, Credential, CredentialId, Grant, Initiator, MembershipId,
    OrganizationId, PrincipalId, Resource, ResourceId,
};

fn enrolled(choice: bool, published: bool) -> (PairingRecord, Credential) {
    let owner = PrincipalId::new("owner").unwrap();
    let intent = ConsentIntent::new(
        ConsentIntentId::new([2; 16]),
        1,
        AudienceId::new("gateway").unwrap(),
        owner.clone(),
        MembershipId::new("member").unwrap(),
        Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
        ConsentClass::PeerRead,
    )
    .unwrap()
    .with_same_person(choice)
    .unwrap();
    let mut record = PairingRecord::new(
        InvitationId::new([1; 16]),
        intent,
        1000,
        PairingPolicy::initial(),
    )
    .unwrap();
    let key = DeviceKey::new([3; 32]);
    let attempt = AttemptId::new([4; 16]);
    let credential = CredentialId::new("credential").unwrap();
    for (event, actor) in [
        (
            PairingEvent::reserve(attempt, key, [5; 32]),
            PairingInitiator::Device(key),
        ),
        (
            PairingEvent::claim(attempt, key),
            PairingInitiator::Device(key),
        ),
        (
            PairingEvent::approve(key, 1),
            PairingInitiator::Principal(owner.clone()),
        ),
        (
            PairingEvent::stage(credential.clone(), attempt, 1),
            PairingInitiator::Principal(owner.clone()),
        ),
        (
            PairingEvent::receiver(
                credential.clone(),
                attempt,
                ResourceId::new("receiver").unwrap(),
                1,
                1,
            ),
            PairingInitiator::System,
        ),
        (
            PairingEvent::activate(1),
            PairingInitiator::Principal(owner.clone()),
        ),
    ]
    .into_iter()
    .take(if published { 6 } else { 5 })
    {
        record = record
            .transition(event, actor, 1001)
            .unwrap()
            .after()
            .clone();
    }
    let credential = Credential::new(
        credential,
        record.credential_principal().unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        1,
        Some(10),
        record.intent().grants().cloned().collect(),
    )
    .unwrap();
    (record, credential)
}

fn linked_change(
    record: &PairingRecord,
    credential: &Credential,
    revocation: Option<&nessa_auth::domain::CredentialTransition>,
    now: u64,
) -> Result<Option<PersonLinkChange>, PairingError> {
    let owner = nessa_auth::domain::Principal::new(
        record.intent().owner().clone(),
        nessa_auth::domain::PrincipalKind::Human,
    );
    PersonLinkChange::from_record(record, &owner, credential, revocation, now)
}

fn active(choice: bool) -> (PairingRecord, Credential) {
    enrolled(choice, true)
}

#[test]
fn choice_changes_attribution_without_changing_principal_class_or_grants() {
    let (linked, credential) = active(true);
    let (plain, plain_credential) = active(false);
    assert_eq!(linked.intent().class(), plain.intent().class());
    assert_eq!(credential.principal_id(), plain_credential.principal_id());
    assert_eq!(credential.grants(), plain_credential.grants());
    let change = linked_change(&linked, &credential, None, 1)
        .unwrap()
        .unwrap();
    assert!(change.links());
    assert_eq!(change.gateway(), credential.principal_id());
    assert_eq!(change.person().as_str(), "owner");
    assert_eq!(change.credential(), credential.id());
    assert!(PublicIntent::from_record(&linked, AttemptId::new([9; 16]))
        .unwrap()
        .same_person());
    assert_eq!(
        linked_change(&plain, &plain_credential, None, 1).unwrap(),
        None
    );
}

#[test]
fn exclusive_expiry_and_revocation_remove_only_the_original_credential_link() {
    let (record, mut credential) = active(true);
    assert!(linked_change(&record, &credential, None, 9)
        .unwrap()
        .unwrap()
        .links());
    let expired = linked_change(&record, &credential, None, 10)
        .unwrap()
        .unwrap();
    assert!(!expired.links());
    assert_eq!(expired.cause(), PersonLinkCause::CredentialExpired);
    assert_eq!(expired.initiator(), &PersonLinkInitiator::Expiry);
    let revocation = credential
        .revoke(2, Initiator::Principal(PrincipalId::new("owner").unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(
        linked_change(&record, &credential, None, 3),
        Err(PairingError::Conflict)
    );
    let revoked = linked_change(&record, &credential, Some(&revocation), 3)
        .unwrap()
        .unwrap();
    assert_eq!(revoked.cause(), PersonLinkCause::CredentialRevoked);
    assert_eq!(revoked.credential(), credential.id());
    assert_eq!(
        revoked.initiator(),
        &PersonLinkInitiator::Principal(PrincipalId::new("owner").unwrap())
    );
}

#[test]
fn enrollment_revoke_keeps_its_owner_and_does_not_relabel_the_credential() {
    let (record, mut credential) = active(true);
    let revocation = credential
        .revoke(2, Initiator::Principal(PrincipalId::new("owner").unwrap()))
        .unwrap()
        .unwrap();
    let record = record
        .transition(
            PairingEvent::end(TerminalCause::Cancelled),
            PairingInitiator::Principal(PrincipalId::new("owner").unwrap()),
            2000,
        )
        .unwrap()
        .after()
        .clone();
    let change = linked_change(&record, &credential, Some(&revocation), 2)
        .unwrap()
        .unwrap();
    assert!(!change.links());
    assert_eq!(change.cause(), PersonLinkCause::EnrollmentRevoked);
    assert_eq!(change.gateway(), credential.principal_id());
    assert_eq!(
        change.initiator(),
        &PersonLinkInitiator::Principal(PrincipalId::new("owner").unwrap())
    );
    let (id, request) = record.stage_binding().unwrap();
    let (receiver, epoch) = record.receiver_binding().unwrap();
    let cleaned = record
        .transition(
            PairingEvent::cleanup(id.clone(), request, receiver.clone(), epoch + 1),
            PairingInitiator::System,
            9000,
        )
        .unwrap();
    assert_eq!(cleaned.verify_cancellation_revocation(None), Ok(()));
    assert_eq!(
        linked_change(cleaned.after(), &credential, Some(&revocation), 9).unwrap(),
        Some(change)
    );
}

#[test]
fn unrelated_credential_or_principal_cannot_supply_a_link() {
    let (record, credential) = active(true);
    for (id, principal, organization, audience) in [
        (
            "other",
            credential.principal_id().as_str(),
            "org",
            "gateway",
        ),
        ("credential", "other", "org", "gateway"),
        (
            "credential",
            credential.principal_id().as_str(),
            "other",
            "gateway",
        ),
        (
            "credential",
            credential.principal_id().as_str(),
            "org",
            "other",
        ),
    ] {
        let organization = OrganizationId::new(organization).unwrap();
        // Keep the credential independently valid, including its grants. An
        // empty grant list would hide a missing identity check behind the
        // separate grant-comparison refusal.
        let grants = credential
            .grants()
            .iter()
            .map(|grant| {
                Grant::new(
                    grant.action().clone(),
                    Resource::new(organization.clone(), grant.resource().id().clone()),
                )
            })
            .collect();
        let mismatched = Credential::new(
            CredentialId::new(id).unwrap(),
            PrincipalId::new(principal).unwrap(),
            organization,
            AudienceId::new(audience).unwrap(),
            1,
            Some(10),
            grants,
        )
        .unwrap();
        assert_eq!(
            linked_change(&record, &mismatched, None, 2),
            Err(PairingError::Conflict)
        );
    }
    let no_grants = Credential::new(
        credential.id().clone(),
        credential.principal_id().clone(),
        credential.organization_id().clone(),
        credential.audience_id().clone(),
        credential.issued_at(),
        credential.expires_at(),
        vec![],
    )
    .unwrap();
    assert_eq!(
        linked_change(&record, &no_grants, None, 2),
        Err(PairingError::Conflict)
    );
}

#[test]
fn same_person_cannot_issue_or_relabel_an_owner_device_class() {
    let (record, _) = active(false);
    let prior = record.intent();
    for class in ConsentClass::ALL {
        let intent = ConsentIntent::new(
            prior.id(),
            prior.generation(),
            prior.audience().clone(),
            prior.owner().clone(),
            prior.membership().clone(),
            prior.resource().clone(),
            class,
        )
        .unwrap();
        let changed = intent.clone().with_same_person(true);
        if class.is_peer() {
            let changed = changed.unwrap();
            assert_eq!(changed.class(), intent.class());
            assert!(changed.grants().eq(intent.grants()));
        } else {
            assert_eq!(changed, Err(PairingError::Invalid));
        }
    }
}

#[test]
fn a_retry_after_expiry_and_revoke_keeps_the_first_boundary_and_initiator() {
    for revoked_at in [2, 10, 11] {
        let (record, mut credential) = active(true);
        let revocation = credential
            .revoke(
                revoked_at,
                Initiator::Principal(PrincipalId::new("owner").unwrap()),
            )
            .unwrap()
            .unwrap();
        let change = linked_change(&record, &credential, Some(&revocation), 12)
            .unwrap()
            .unwrap();
        if revoked_at < 10 {
            assert_eq!(change.cause(), PersonLinkCause::CredentialRevoked);
            assert_eq!(
                change.initiator(),
                &PersonLinkInitiator::Principal(PrincipalId::new("owner").unwrap())
            );
        } else {
            assert_eq!(change.cause(), PersonLinkCause::CredentialExpired);
            assert_eq!(change.initiator(), &PersonLinkInitiator::Expiry);
        }
    }
}

fn superseded(
    transition: &nessa_auth::domain::CredentialTransition,
) -> nessa_auth::domain::CredentialTransition {
    nessa_auth::domain::CredentialTransition::new(
        transition.credential_id().clone(),
        transition.before().copied(),
        *transition.after(),
        nessa_auth::domain::TransitionCause::Revoked(
            nessa_auth::domain::RevocationCause::Superseded {
                by: CredentialId::new("replacement").unwrap(),
                kind: nessa_auth::domain::Supersession::Provision,
            },
        ),
        transition.initiator().clone(),
        transition.at(),
    )
    .unwrap()
}

#[test]
fn person_link_terminal_evidence_rejects_a_cancellation_with_another_revocation_time_or_cause() {
    let (record, mut credential) = active(true);
    let owner = PairingInitiator::Principal(PrincipalId::new("owner").unwrap());
    let revocation = credential
        .revoke(2, Initiator::Principal(PrincipalId::new("owner").unwrap()))
        .unwrap()
        .unwrap();
    for (at, revocation) in [(3000, revocation.clone()), (2000, superseded(&revocation))] {
        let cancellation = record
            .transition(
                PairingEvent::end(TerminalCause::Cancelled),
                owner.clone(),
                at,
            )
            .unwrap();
        assert_eq!(
            cancellation.verify_cancellation_revocation(Some(&revocation)),
            Err(PairingError::Conflict)
        );
        assert_eq!(
            linked_change(cancellation.after(), &credential, Some(&revocation), 3),
            Err(PairingError::Conflict)
        );
    }
}

#[test]
fn person_link_terminal_evidence_rejects_replacing_the_original_revocation() {
    let (record, original) = active(true);
    let mut credential = original.clone();
    let revocation = credential
        .revoke(2, Initiator::Principal(PrincipalId::new("owner").unwrap()))
        .unwrap()
        .unwrap();
    let terminal = record
        .transition(
            PairingEvent::credential_revoked(revocation.clone()),
            PairingInitiator::Principal(PrincipalId::new("owner").unwrap()),
            record
                .canonical_revocation_lower_bound_ms(&revocation)
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        linked_change(terminal.after(), &credential, Some(&revocation), 3)
            .unwrap()
            .unwrap()
            .cause(),
        PersonLinkCause::CredentialRevoked
    );
    assert_eq!(
        linked_change(
            terminal.after(),
            &credential,
            Some(&superseded(&revocation)),
            3
        ),
        Err(PairingError::Conflict)
    );
    let mut later = original;
    let later_revocation = later
        .revoke(3, Initiator::Principal(PrincipalId::new("owner").unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(
        linked_change(terminal.after(), &later, Some(&later_revocation), 3),
        Err(PairingError::Conflict)
    );
}

#[test]
fn person_link_terminal_evidence_rejects_an_unpublished_reserved_credential() {
    let (record, mut credential) = enrolled(true, false);
    assert_eq!(record.phase(), PairingPhase::Staging);
    let revocation = credential
        .revoke(2, Initiator::Principal(PrincipalId::new("owner").unwrap()))
        .unwrap()
        .unwrap();
    let cancelled = record
        .transition(
            PairingEvent::end(TerminalCause::Cancelled),
            PairingInitiator::Principal(PrincipalId::new("owner").unwrap()),
            2000,
        )
        .unwrap();
    assert_eq!(
        cancelled.verify_cancellation_revocation(Some(&revocation)),
        Ok(())
    );
    assert_eq!(
        linked_change(cancelled.after(), &credential, Some(&revocation), 2),
        Err(PairingError::Conflict)
    );
}

#[test]
fn same_person_requires_the_exact_authoritative_human_and_false_choice_needs_no_person() {
    use nessa_auth::domain::{Principal, PrincipalKind};
    let (record, credential) = active(true);
    assert_eq!(
        record.intent().validate_same_person_owner(None),
        Err(PairingError::Conflict)
    );
    for kind in [
        PrincipalKind::Integration,
        PrincipalKind::Agent,
        PrincipalKind::Gateway,
    ] {
        let owner = Principal::new(record.intent().owner().clone(), kind);
        assert_eq!(
            PersonLinkChange::from_record(&record, &owner, &credential, None, 2),
            Err(PairingError::Invalid)
        );
    }
    let other = Principal::new(
        PrincipalId::new("another-person").unwrap(),
        PrincipalKind::Human,
    );
    assert_eq!(
        PersonLinkChange::from_record(&record, &other, &credential, None, 2),
        Err(PairingError::Conflict)
    );
    assert!(linked_change(&record, &credential, None, 2)
        .unwrap()
        .unwrap()
        .links());
    let (plain, credential) = active(false);
    assert_eq!(plain.intent().validate_same_person_owner(None), Ok(()));
    let surface = Principal::new(plain.intent().owner().clone(), PrincipalKind::Integration);
    assert_eq!(
        PersonLinkChange::from_record(&plain, &surface, &credential, None, 2),
        Ok(None)
    );
}
