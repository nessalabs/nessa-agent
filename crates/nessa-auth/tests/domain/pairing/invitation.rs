use nessa_auth::domain::{
    pairing::{
        validate_pairing_collection, AttemptFailure, AttemptId, AttemptOutcome, ConsentClass,
        ConsentIntent, ConsentIntentId, DeviceKey, InvitationId, PairingError, PairingEvent,
        PairingInitiator, PairingPhase, PairingPolicy, PairingRecord, TerminalCause,
        MAX_LIVE_PAIRINGS,
    },
    AudienceId, Credential, CredentialId, CredentialLifecycle, CredentialTransition, Initiator,
    IssuanceCause, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId, Supersession,
    TransitionCause,
};

fn record() -> PairingRecord {
    PairingRecord::new(
        InvitationId::new([1; 16]),
        ConsentIntent::new(
            ConsentIntentId::new([2; 16]),
            1,
            AudienceId::new("gateway").unwrap(),
            PrincipalId::new("owner").unwrap(),
            MembershipId::new("membership").unwrap(),
            Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
            ConsentClass::DeviceRead,
        )
        .unwrap(),
        1_000,
        PairingPolicy::new(100, 5).unwrap(),
    )
    .unwrap()
}
fn owner() -> PairingInitiator {
    PairingInitiator::Principal(PrincipalId::new("owner").unwrap())
}
fn device() -> PairingInitiator {
    PairingInitiator::Device(key())
}
fn key() -> DeviceKey {
    DeviceKey::new([3; 32])
}
fn id(n: u8) -> AttemptId {
    AttemptId::new([n; 16])
}
fn reserve(record: &PairingRecord, n: u8) -> PairingRecord {
    record
        .transition(
            PairingEvent::reserve(id(n), key(), [n; 32]),
            device(),
            1_001,
        )
        .unwrap()
        .after()
        .clone()
}
fn claimed() -> PairingRecord {
    reserve(&record(), 1)
        .transition(PairingEvent::claim(id(1), key()), device(), 1_002)
        .unwrap()
        .after()
        .clone()
}
fn staged() -> PairingRecord {
    claimed()
        .transition(PairingEvent::approve(key(), 1), owner(), 1_003)
        .unwrap()
        .after()
        .transition(
            PairingEvent::stage(CredentialId::new("device").unwrap(), id(9), 1),
            owner(),
            1_004,
        )
        .unwrap()
        .after()
        .clone()
}
fn receiver(record: &PairingRecord) -> PairingRecord {
    record
        .transition(
            PairingEvent::receiver(
                CredentialId::new("device").unwrap(),
                id(9),
                ResourceId::new("receiver").unwrap(),
                1,
                1,
            ),
            PairingInitiator::System,
            1_005,
        )
        .unwrap()
        .after()
        .clone()
}

#[test]
fn attempt_retry_is_immutable() {
    let record = reserve(&record(), 1);
    assert_eq!(
        record.reservation_status(id(1), key(), [1; 32]),
        Ok(Some(AttemptOutcome::Pending))
    );
    assert_eq!(
        record.reservation_status(id(1), key(), [2; 32]),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        record.attempt_status(id(1), DeviceKey::new([4; 32])),
        Err(PairingError::WrongActor)
    );
    assert_eq!(record.charged_attempts(), 1);
    assert_eq!(
        record.transition(
            PairingEvent::reserve(id(2), key(), [2; 32]),
            device(),
            1_002
        ),
        Err(PairingError::Capacity)
    );
}

#[test]
fn policy_refuses_each_invalid_bound_and_accepts_finite_endpoints() {
    for (lifetime, attempts) in [(0, 1), (1, 0), (1, 6)] {
        assert_eq!(
            PairingPolicy::new(lifetime, attempts),
            Err(PairingError::Invalid)
        );
    }
    for attempts in [1, 5] {
        let accepted = PairingPolicy::new(1, attempts).unwrap();
        assert_eq!(accepted.lifetime_ms(), 1);
        assert_eq!(accepted.attempts(), attempts);
    }
}

#[test]
fn direct_reservation_refuses_before_creation_and_accepts_creation_time() {
    let original = record();
    assert_eq!(
        original.transition(
            PairingEvent::reserve(id(1), key(), [1; 32]),
            device(),
            original.created_at_ms() - 1,
        ),
        Err(PairingError::Invalid)
    );
    assert_eq!(original.charged_attempts(), 0);
    let accepted = original
        .transition(
            PairingEvent::reserve(id(1), key(), [1; 32]),
            device(),
            original.created_at_ms(),
        )
        .unwrap();
    assert_eq!(accepted.before(), &original);
    assert_eq!(accepted.after().charged_attempts(), 1);
}

#[test]
fn reservation_actor_must_be_the_exact_admitted_device() {
    let original = record();
    for actor in [
        PairingInitiator::Device(DeviceKey::new([4; 32])),
        owner(),
        PairingInitiator::LocalOperator,
        PairingInitiator::System,
    ] {
        assert_eq!(
            original.transition(PairingEvent::reserve(id(1), key(), [1; 32]), actor, 1_001),
            Err(PairingError::WrongActor)
        );
    }
    assert_eq!(original.charged_attempts(), 0);
    assert_eq!(original.reservation_status(id(1), key(), [1; 32]), Ok(None));
    let accepted = original
        .transition(
            PairingEvent::reserve(id(1), key(), [1; 32]),
            device(),
            1_001,
        )
        .unwrap();
    assert_eq!(accepted.after().charged_attempts(), 1);
    assert_eq!(
        accepted.after().attempt_status(id(1), key()),
        Ok(AttemptOutcome::Pending)
    );
}

#[test]
fn last_attempt_remains_eligible() {
    let mut current = record();
    for n in 1..5 {
        current = reserve(&current, n);
        current = current
            .transition(
                PairingEvent::fail(id(n), key(), AttemptFailure::InvalidProof),
                device(),
                1_002,
            )
            .unwrap()
            .after()
            .clone();
    }
    current = reserve(&current, 5);
    assert_eq!(
        current.transition(
            PairingEvent::reserve(id(6), key(), [6; 32]),
            device(),
            1_003
        ),
        Err(PairingError::AttemptsExhausted)
    );
    let claim = current
        .transition(PairingEvent::claim(id(5), key()), device(), 1_099)
        .unwrap();
    assert_eq!(claim.after().claim_binding(), Some((id(5), key())));
    assert_eq!(claim.after().phase(), PairingPhase::Claimed);
    assert_eq!(
        current.transition(PairingEvent::claim(id(5), key()), device(), 1_100),
        Err(PairingError::Expired)
    );
}

#[test]
fn completion_vs_terminal() {
    for cause in [
        TerminalCause::Denied,
        TerminalCause::Cancelled,
        TerminalCause::Expired,
        TerminalCause::Restarted,
    ] {
        let current = reserve(&record(), 1);
        let (actor, at) = match cause {
            TerminalCause::Denied | TerminalCause::Cancelled => (owner(), 1_005),
            TerminalCause::Expired => (PairingInitiator::System, 1_100),
            TerminalCause::Restarted => (PairingInitiator::System, 1_005),
            TerminalCause::CredentialRevoked => {
                panic!("canonical revocation uses its dedicated event")
            }
        };
        let stopped = current
            .transition(PairingEvent::end(cause), actor.clone(), at)
            .unwrap();
        assert_eq!(stopped.after().terminal(), Some((cause, &actor)));
        assert_eq!(
            stopped
                .after()
                .transition(PairingEvent::claim(id(1), key()), device(), at),
            Err(PairingError::Ineligible)
        );
        assert_eq!(
            stopped
                .after()
                .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), at),
            Err(PairingError::Ineligible)
        );
        assert_eq!(
            stopped.after().attempt_status(id(1), key()),
            Ok(AttemptOutcome::Superseded)
        );
        let won = current
            .transition(PairingEvent::claim(id(1), key()), device(), 1_004)
            .unwrap();
        assert_eq!(won.after().phase(), PairingPhase::Claimed);
        assert_eq!(
            won.after().transition(
                PairingEvent::end(TerminalCause::Restarted),
                PairingInitiator::System,
                at
            ),
            Err(PairingError::Invalid)
        );
    }
}

#[test]
fn denied_and_cancelled_require_original_owner() {
    let original = record();
    for cause in [TerminalCause::Denied, TerminalCause::Cancelled] {
        assert_eq!(
            original.transition(PairingEvent::end(cause), PairingInitiator::System, 1_005),
            Err(PairingError::WrongActor)
        );
        let accepted = original
            .transition(PairingEvent::end(cause), owner(), 1_005)
            .unwrap();
        assert_eq!(accepted.after().terminal(), Some((cause, &owner())));
        assert_eq!(original.phase(), PairingPhase::Available);
        assert!(original.terminal().is_none());
    }
}

#[test]
fn approve_exact_claim() {
    let current = claimed();
    assert_eq!(
        current.transition(
            PairingEvent::approve(DeviceKey::new([4; 32]), 1),
            owner(),
            1_003
        ),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        current.transition(PairingEvent::approve(key(), 2), owner(), 1_003),
        Err(PairingError::StaleGeneration)
    );
    assert_eq!(
        current.transition(
            PairingEvent::approve(key(), 1),
            PairingInitiator::System,
            1_003
        ),
        Err(PairingError::WrongActor)
    );
    assert!(current
        .transition(PairingEvent::approve(key(), 1), owner(), 1_003)
        .is_ok());
    assert_eq!(
        current.transition(
            PairingEvent::reserve(id(2), key(), [2; 32]),
            device(),
            1_003
        ),
        Err(PairingError::Ineligible)
    );
    let approved = current
        .transition(PairingEvent::approve(key(), 1), owner(), 1_003)
        .unwrap();
    assert_eq!(
        approved
            .after()
            .transition(PairingEvent::approve(key(), 1), owner(), 1_004),
        Err(PairingError::Conflict)
    );
    assert_eq!(approved.after().claim_binding(), current.claim_binding());
}

#[test]
fn stage_before_receiver_effect() {
    let current = claimed();
    assert_eq!(
        current.transition(PairingEvent::activate(1), owner(), 1_003),
        Err(PairingError::Ineligible)
    );
    let stage = staged();
    assert_eq!(stage.phase(), PairingPhase::Staging);
    assert!(stage.receiver_binding().is_none());
    assert_eq!(
        stage.transition(PairingEvent::activate(1), owner(), 1_005),
        Err(PairingError::Ineligible)
    );
    assert_eq!(
        stage.transition(
            PairingEvent::receiver(
                CredentialId::new("wrong").unwrap(),
                id(9),
                ResourceId::new("receiver").unwrap(),
                1,
                1
            ),
            PairingInitiator::System,
            1_005
        ),
        Err(PairingError::Conflict)
    );
    let ready = receiver(&stage);
    let activated = ready
        .transition(PairingEvent::activate(1), owner(), 1_006)
        .unwrap();
    assert_eq!(activated.after().phase(), PairingPhase::Active);
    assert_eq!(activated.after().credential().unwrap().as_str(), "device");
    assert_eq!(
        ready.transition(PairingEvent::activate(1), owner(), 1_100),
        Err(PairingError::Expired)
    );
}

#[test]
fn late_receiver_cleanup() {
    let current = staged()
        .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_005)
        .unwrap()
        .after()
        .clone();
    assert!(current.cleanup_pending());
    let late = receiver(&current);
    assert_eq!(late.phase(), PairingPhase::Terminal);
    assert_eq!(
        late.transition(PairingEvent::activate(1), owner(), 1_006),
        Err(PairingError::Ineligible)
    );
    assert_eq!(
        late.transition(
            PairingEvent::cleanup(
                CredentialId::new("device").unwrap(),
                id(9),
                ResourceId::new("wrong").unwrap(),
                2
            ),
            PairingInitiator::System,
            1_006
        ),
        Err(PairingError::Conflict)
    );
    let clean = late
        .transition(
            PairingEvent::cleanup(
                CredentialId::new("device").unwrap(),
                id(9),
                ResourceId::new("receiver").unwrap(),
                2,
            ),
            PairingInitiator::System,
            1_006,
        )
        .unwrap();
    assert!(!clean.after().cleanup_pending());
    assert_eq!(
        clean.after().terminal(),
        Some((TerminalCause::Cancelled, &owner()))
    );
}

#[test]
fn pairing_history_agrees() {
    let initial = record();
    let reservation = initial
        .transition(
            PairingEvent::reserve(id(1), key(), [1; 32]),
            device(),
            1_001,
        )
        .unwrap();
    let claim = reservation
        .after()
        .transition(PairingEvent::claim(id(1), key()), device(), 1_002)
        .unwrap();
    reservation.verify(&initial).unwrap();
    claim.verify(reservation.after()).unwrap();
    assert_eq!(claim.verify(&initial), Err(PairingError::Conflict));
    assert_eq!(
        reservation.verify(claim.after()),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        initial.transition(PairingEvent::reserve(id(1), key(), [1; 32]), device(), 999),
        Err(PairingError::Invalid)
    );
}

#[test]
fn canonical_revocation_preserves_origin_and_time_resolution() {
    let mut active = PairingRecord::new(
        record().id(),
        record().intent().clone(),
        1001,
        PairingPolicy::new(100, 5).unwrap(),
    )
    .unwrap();
    for (event, actor, time) in [
        (PairingEvent::reserve(id(1), key(), [1; 32]), device(), 1001),
        (PairingEvent::claim(id(1), key()), device(), 1002),
        (PairingEvent::approve(key(), 1), owner(), 1003),
        (
            PairingEvent::stage(CredentialId::new("device").unwrap(), id(9), 1),
            owner(),
            1004,
        ),
        (
            PairingEvent::receiver(
                CredentialId::new("device").unwrap(),
                id(9),
                ResourceId::new("receiver").unwrap(),
                1,
                1,
            ),
            PairingInitiator::System,
            1005,
        ),
        (PairingEvent::activate(1), owner(), 1006),
    ] {
        active = active
            .transition(event, actor, time)
            .unwrap()
            .after()
            .clone();
    }
    let credential = || {
        Credential::new(
            CredentialId::new("device").unwrap(),
            PrincipalId::new("owner").unwrap(),
            OrganizationId::new("org").unwrap(),
            AudienceId::new("gateway").unwrap(),
            1,
            None,
            vec![active.intent().grant().clone()],
        )
        .unwrap()
    };
    for initiator in [
        Initiator::Principal(PrincipalId::new("revoking-admin").unwrap()),
        Initiator::LocalOperator,
    ] {
        let mut credential = credential();
        let canonical = match &initiator {
            Initiator::Principal(_) => credential.revoke(1, initiator.clone()).unwrap().unwrap(),
            Initiator::LocalOperator => credential
                .supersede(
                    1,
                    CredentialId::new("recovered").unwrap(),
                    Supersession::OwnerRecovery,
                    initiator.clone(),
                )
                .unwrap(),
        };
        let actor = PairingInitiator::from_credential(canonical.initiator());
        assert_eq!(canonical.at(), 1);
        assert_eq!(
            active.canonical_revocation_lower_bound_ms(&canonical),
            Ok(1001)
        );
        let event = active
            .transition(
                PairingEvent::credential_revoked(canonical.clone()),
                actor.clone(),
                1001,
            )
            .unwrap();
        assert_eq!(event.ordering_time_ms(), 1001);
        assert_eq!(
            event.after().terminal(),
            Some((TerminalCause::CredentialRevoked, &actor))
        );
        assert!(event.after().cleanup_pending());
        event.verify(&active).unwrap();
        assert_eq!(
            event
                .after()
                .transition(PairingEvent::activate(1), owner(), 1007),
            Err(PairingError::Ineligible)
        );
        assert_eq!(
            active.transition(
                PairingEvent::credential_revoked(canonical),
                PairingInitiator::System,
                1001
            ),
            Err(PairingError::WrongActor)
        );
    }
    let overflow = credential()
        .revoke(u64::MAX, Initiator::LocalOperator)
        .unwrap()
        .unwrap();
    assert_eq!(
        active.canonical_revocation_lower_bound_ms(&overflow),
        Err(PairingError::Invalid)
    );
}

#[test]
fn available_manual_slot_is_canonical_and_released_only_by_claim_or_terminal() {
    let original = record();
    let replacement = PairingRecord::new(
        InvitationId::new([9; 16]),
        original.intent().clone(),
        1_000,
        PairingPolicy::initial(),
    )
    .unwrap();
    assert_eq!(
        validate_pairing_collection(&[original.clone(), replacement.clone()]),
        Err(PairingError::AvailableSlotOccupied)
    );
    let reserved = reserve(&original, 1);
    assert_eq!(
        validate_pairing_collection(&[reserved, replacement.clone()]),
        Err(PairingError::AvailableSlotOccupied)
    );
    assert_eq!(
        validate_pairing_collection(&[claimed(), replacement.clone()]),
        Ok(())
    );
    let ended = original
        .transition(PairingEvent::end(TerminalCause::Denied), owner(), 1_002)
        .unwrap()
        .after()
        .clone();
    assert_eq!(
        validate_pairing_collection(&[ended, replacement.clone()]),
        Ok(())
    );
    assert_eq!(
        validate_pairing_collection(&[replacement.clone(), replacement]),
        Err(PairingError::Conflict)
    );
}
#[test]
fn collection_live_bound_keeps_claimed_reservations_and_refuses_overflow() {
    let mut records = vec![];
    for n in 0..=MAX_LIVE_PAIRINGS {
        let base = record();
        let value = PairingRecord::new(
            InvitationId::new([n as u8; 16]),
            base.intent().clone(),
            1_000,
            PairingPolicy::initial(),
        )
        .unwrap();
        let value = reserve(&value, 1)
            .transition(PairingEvent::claim(id(1), key()), device(), 1_002)
            .unwrap()
            .after()
            .clone();
        records.push(value);
        assert_eq!(
            validate_pairing_collection(&records),
            if n < MAX_LIVE_PAIRINGS {
                Ok(())
            } else {
                Err(PairingError::Capacity)
            }
        );
    }
}

#[test]
fn no_receiver_cleanup_retains_terminal_cause_and_rejects_contradictory_late_result() {
    assert_eq!(
        staged().transition(
            PairingEvent::no_receiver_cleanup(CredentialId::new("device").unwrap(), id(9), 1),
            PairingInitiator::System,
            1_005
        ),
        Err(PairingError::Conflict)
    );
    let terminal = staged()
        .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_005)
        .unwrap()
        .after()
        .clone();
    assert!(terminal.cleanup_pending());
    for (credential, request, generation, actor, expected) in [
        (
            "other",
            id(9),
            1,
            PairingInitiator::System,
            PairingError::Conflict,
        ),
        (
            "device",
            id(8),
            1,
            PairingInitiator::System,
            PairingError::Conflict,
        ),
        (
            "device",
            id(9),
            2,
            PairingInitiator::System,
            PairingError::StaleGeneration,
        ),
        ("device", id(9), 1, owner(), PairingError::WrongActor),
    ] {
        assert_eq!(
            terminal.transition(
                PairingEvent::no_receiver_cleanup(
                    CredentialId::new(credential).unwrap(),
                    request,
                    generation
                ),
                actor,
                1_006
            ),
            Err(expected)
        );
    }
    let transition = terminal
        .transition(
            PairingEvent::no_receiver_cleanup(CredentialId::new("device").unwrap(), id(9), 1),
            PairingInitiator::System,
            1_006,
        )
        .unwrap();
    transition.verify(&terminal).unwrap();
    let cleaned = transition.after();
    assert!(!cleaned.cleanup_pending());
    assert_eq!(cleaned.terminal(), terminal.terminal());
    assert!(cleaned.receiver_binding().is_none());
    assert_eq!(
        cleaned.transition(
            PairingEvent::no_receiver_cleanup(CredentialId::new("device").unwrap(), id(9), 1),
            PairingInitiator::System,
            1_007
        ),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        cleaned.transition(
            PairingEvent::receiver(
                CredentialId::new("device").unwrap(),
                id(9),
                ResourceId::new("late").unwrap(),
                1,
                1
            ),
            PairingInitiator::System,
            1_007
        ),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        receiver(&staged())
            .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_006)
            .unwrap()
            .after()
            .transition(
                PairingEvent::no_receiver_cleanup(CredentialId::new("device").unwrap(), id(9), 1),
                PairingInitiator::System,
                1_007
            ),
        Err(PairingError::Conflict)
    );
}

#[test]
fn direct_expiry_requires_due_time() {
    let original = record();
    assert_eq!(
        original.transition(
            PairingEvent::end(TerminalCause::Expired),
            PairingInitiator::System,
            1_005,
        ),
        Err(PairingError::Invalid)
    );
    let accepted = original
        .transition(
            PairingEvent::end(TerminalCause::Expired),
            PairingInitiator::System,
            1_100,
        )
        .unwrap();
    assert_eq!(
        accepted.after().terminal(),
        Some((TerminalCause::Expired, &PairingInitiator::System))
    );
    assert_eq!(original.phase(), PairingPhase::Available);
    assert!(original.terminal().is_none());
}

#[test]
fn conditional_expiry_preserves_valid_current_phase_and_clock() {
    let available = record();
    assert!(available.expire_if_due(999).unwrap().is_none());
    assert!(available.expire_if_due(1099).unwrap().is_none());
    let expired = available.expire_if_due(1100).unwrap().unwrap();
    assert_eq!(
        expired.after().terminal(),
        Some((TerminalCause::Expired, &PairingInitiator::System))
    );
    assert!(expired.after().expire_if_due(1200).unwrap().is_none());
    let denied = claimed()
        .transition(PairingEvent::end(TerminalCause::Denied), owner(), 1005)
        .unwrap();
    assert!(denied.after().expire_if_due(1200).unwrap().is_none());
    assert_eq!(
        denied.after().terminal(),
        Some((TerminalCause::Denied, &owner()))
    );
    let active = receiver(&staged())
        .transition(PairingEvent::activate(1), owner(), 1006)
        .unwrap();
    assert!(active.after().expire_if_due(1200).unwrap().is_none());
    assert_eq!(
        active.after().transition(
            PairingEvent::end(TerminalCause::Expired),
            PairingInitiator::System,
            1200
        ),
        Err(PairingError::Invalid)
    );
    let due = claimed().expire_if_due(1100).unwrap().unwrap();
    assert_eq!(due.after().phase(), PairingPhase::Terminal);
}

#[test]
fn reservation_key_and_transition_replay_keep_the_original_attempt() {
    let pending = reserve(&record(), 1);
    assert_eq!(
        pending.reservation_status(id(1), DeviceKey::new([4; 32]), [1; 32]),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        pending.transition(
            PairingEvent::reserve(id(1), key(), [1; 32]),
            device(),
            1_002
        ),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        pending.reservation_status(id(1), key(), [1; 32]),
        Ok(Some(AttemptOutcome::Pending))
    );
    let failed = pending
        .transition(
            PairingEvent::fail(id(1), key(), AttemptFailure::InvalidProof),
            device(),
            1_002,
        )
        .unwrap();
    assert_eq!(reserve(failed.after(), 2).charged_attempts(), 2);
    assert_eq!(pending.charged_attempts(), 1);
}

#[test]
fn attempt_settlement_requires_the_event_device_actor() {
    let pending = reserve(&record(), 1);
    for event in [
        PairingEvent::fail(id(1), key(), AttemptFailure::InvalidProof),
        PairingEvent::claim(id(1), key()),
    ] {
        assert_eq!(
            pending.transition(event.clone(), owner(), 1_002),
            Err(PairingError::WrongActor)
        );
        let settled = pending.transition(event, device(), 1_002).unwrap();
        assert_eq!(settled.before(), &pending);
        assert_eq!(settled.after().charged_attempts(), 1);
    }
}

#[test]
fn attempt_settlement_requires_an_original_reserved_identity() {
    let pending = reserve(&record(), 1);
    for event in [
        PairingEvent::fail(id(2), key(), AttemptFailure::InvalidProof),
        PairingEvent::claim(id(2), key()),
    ] {
        assert_eq!(
            pending.transition(event, device(), 1_002),
            Err(PairingError::Conflict)
        );
    }
    assert_eq!(
        pending
            .transition(PairingEvent::claim(id(1), key()), device(), 1_002)
            .unwrap()
            .after()
            .attempt_status(id(1), key()),
        Ok(AttemptOutcome::Claimed)
    );
}

#[test]
fn attempt_settlement_requires_the_original_reserved_key() {
    let pending = reserve(&record(), 1);
    let foreign = DeviceKey::new([4; 32]);
    for event in [
        PairingEvent::fail(id(1), foreign, AttemptFailure::InvalidProof),
        PairingEvent::claim(id(1), foreign),
    ] {
        assert_eq!(
            pending.transition(event, PairingInitiator::Device(foreign), 1_002),
            Err(PairingError::WrongActor)
        );
    }
    assert_eq!(
        pending
            .transition(PairingEvent::claim(id(1), key()), device(), 1_002)
            .unwrap()
            .after()
            .claim_binding(),
        Some((id(1), key()))
    );
}

#[test]
fn settled_attempt_cannot_be_failed_or_claimed_again() {
    let pending = reserve(&record(), 1);
    for first in [
        PairingEvent::fail(id(1), key(), AttemptFailure::InvalidProof),
        PairingEvent::claim(id(1), key()),
    ] {
        let settled = pending.transition(first, device(), 1_002).unwrap();
        for later in [
            PairingEvent::fail(id(1), key(), AttemptFailure::ConnectionClosed),
            PairingEvent::claim(id(1), key()),
        ] {
            assert_eq!(
                settled.after().transition(later, device(), 1_003),
                Err(PairingError::Ineligible)
            );
        }
    }
    assert_eq!(
        pending.attempt_status(id(1), key()),
        Ok(AttemptOutcome::Pending)
    );
}

#[test]
fn stage_requires_the_original_owner_approval_phase() {
    let claim = claimed();
    let event = PairingEvent::stage(CredentialId::new("device").unwrap(), id(9), 1);
    assert_eq!(
        claim.transition(event.clone(), owner(), 1_004),
        Err(PairingError::Ineligible)
    );
    let approved = claim
        .transition(PairingEvent::approve(key(), 1), owner(), 1_003)
        .unwrap();
    let stage = approved.after().transition(event, owner(), 1_004).unwrap();
    assert_eq!(stage.after().phase(), PairingPhase::Staging);
    assert_eq!(
        stage.after().stage_binding(),
        Some((&CredentialId::new("device").unwrap(), id(9)))
    );
}

#[test]
fn receiver_result_requires_system_and_original_generation() {
    let stage = staged();
    let result = |generation| {
        PairingEvent::receiver(
            CredentialId::new("device").unwrap(),
            id(9),
            ResourceId::new("receiver").unwrap(),
            1,
            generation,
        )
    };
    assert_eq!(
        stage.transition(result(1), owner(), 1_005),
        Err(PairingError::WrongActor)
    );
    assert_eq!(
        stage.transition(result(2), PairingInitiator::System, 1_005),
        Err(PairingError::StaleGeneration)
    );
    assert_eq!(
        stage
            .transition(result(1), PairingInitiator::System, 1_005)
            .unwrap()
            .after()
            .receiver_binding(),
        Some((&ResourceId::new("receiver").unwrap(), 1))
    );
}

#[test]
fn receiver_result_requires_original_stage_request_and_positive_epoch() {
    let stage = staged();
    let result = |request, epoch| {
        PairingEvent::receiver(
            CredentialId::new("device").unwrap(),
            request,
            ResourceId::new("receiver").unwrap(),
            epoch,
            1,
        )
    };
    assert_eq!(
        record().transition(result(id(9), 1), PairingInitiator::System, 1_005),
        Err(PairingError::Ineligible)
    );
    assert_eq!(
        stage.transition(result(id(8), 1), PairingInitiator::System, 1_005),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        stage.transition(result(id(9), 0), PairingInitiator::System, 1_005),
        Err(PairingError::Conflict)
    );
    let retained = stage
        .transition(result(id(9), 1), PairingInitiator::System, 1_005)
        .unwrap();
    assert_eq!(
        retained
            .after()
            .transition(result(id(9), 1), PairingInitiator::System, 1_006),
        Err(PairingError::Conflict)
    );
    assert!(stage.receiver_binding().is_none());
}

#[test]
fn receiver_cleanup_requires_system_and_a_terminal_stage() {
    let ready = receiver(&staged());
    let event = PairingEvent::cleanup(
        CredentialId::new("device").unwrap(),
        id(9),
        ResourceId::new("receiver").unwrap(),
        2,
    );
    let terminal = ready
        .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_006)
        .unwrap();
    assert_eq!(
        terminal.after().transition(event.clone(), owner(), 1_007),
        Err(PairingError::WrongActor)
    );
    assert_eq!(
        ready.transition(event.clone(), PairingInitiator::System, 1_007),
        Err(PairingError::Conflict)
    );
    let clean = terminal
        .after()
        .transition(event, PairingInitiator::System, 1_007)
        .unwrap();
    assert!(!clean.after().cleanup_pending());
    assert_eq!(
        clean.after().terminal(),
        Some((TerminalCause::Cancelled, &owner()))
    );
}

#[test]
fn receiver_cleanup_requires_original_target_an_advanced_fence_and_no_prior_cleanup() {
    let ready = receiver(&staged());
    let terminal = ready
        .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_006)
        .unwrap();
    let event = |credential, request, epoch| {
        PairingEvent::cleanup(
            credential,
            request,
            ResourceId::new("receiver").unwrap(),
            epoch,
        )
    };
    for (credential, request, epoch) in [
        ("other", id(9), 2),
        ("device", id(8), 2),
        ("device", id(9), 0),
        ("device", id(9), 1),
    ] {
        assert_eq!(
            terminal.after().transition(
                event(CredentialId::new(credential).unwrap(), request, epoch),
                PairingInitiator::System,
                1_007
            ),
            Err(PairingError::Conflict)
        );
    }
    let exact = event(CredentialId::new("device").unwrap(), id(9), 2);
    let clean = terminal
        .after()
        .transition(exact.clone(), PairingInitiator::System, 1_007)
        .unwrap();
    assert_eq!(
        clean.after().transition(
            event(CredentialId::new("device").unwrap(), id(9), 3),
            PairingInitiator::System,
            1_008,
        ),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        clean.after().receiver_binding(),
        Some((&ResourceId::new("receiver").unwrap(), 2))
    );
    assert!(terminal.after().cleanup_pending());
}

#[test]
fn automatic_terminal_causes_require_system_and_denial_refuses_active() {
    for (cause, time) in [
        (TerminalCause::Expired, 1_100),
        (TerminalCause::Restarted, 1_005),
    ] {
        assert_eq!(
            record().transition(PairingEvent::end(cause), owner(), time),
            Err(PairingError::Invalid)
        );
        assert_eq!(
            record()
                .transition(PairingEvent::end(cause), PairingInitiator::System, time)
                .unwrap()
                .after()
                .terminal(),
            Some((cause, &PairingInitiator::System))
        );
    }
    let ready = receiver(&staged());
    let active = ready
        .transition(PairingEvent::activate(1), owner(), 1_006)
        .unwrap();
    assert_eq!(
        active
            .after()
            .transition(PairingEvent::end(TerminalCause::Denied), owner(), 1_007),
        Err(PairingError::Ineligible)
    );
    assert_eq!(
        active
            .after()
            .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_007)
            .unwrap()
            .after()
            .terminal(),
        Some((TerminalCause::Cancelled, &owner()))
    );
}

#[test]
fn terminal_end_preserves_failed_attempt_and_supersedes_only_pending() {
    let first = reserve(&record(), 1);
    let failed = first
        .transition(
            PairingEvent::fail(id(1), key(), AttemptFailure::InvalidProof),
            device(),
            1_002,
        )
        .unwrap();
    let pending = reserve(failed.after(), 2);
    let end = pending
        .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_003)
        .unwrap();
    assert_eq!(
        end.after().attempt_status(id(1), key()),
        Ok(AttemptOutcome::Failed(AttemptFailure::InvalidProof))
    );
    assert_eq!(
        end.after().attempt_status(id(2), key()),
        Ok(AttemptOutcome::Superseded)
    );
    assert_eq!(
        pending.attempt_status(id(2), key()),
        Ok(AttemptOutcome::Pending)
    );
    assert_eq!(
        end.after().terminal(),
        Some((TerminalCause::Cancelled, &owner()))
    );
}

#[test]
fn canonical_revocation_requires_original_active_credential_cause_and_time() {
    let ready = receiver(&staged());
    let active = ready
        .transition(PairingEvent::activate(1), owner(), 1_006)
        .unwrap();
    let credential = |name| {
        Credential::new(
            CredentialId::new(name).unwrap(),
            PrincipalId::new("owner").unwrap(),
            OrganizationId::new("org").unwrap(),
            AudienceId::new("gateway").unwrap(),
            1,
            None,
            vec![active.after().intent().grant().clone()],
        )
        .unwrap()
    };
    let revoked = credential("device")
        .revoke(2, Initiator::LocalOperator)
        .unwrap()
        .unwrap();
    let foreign = credential("other")
        .revoke(2, Initiator::LocalOperator)
        .unwrap()
        .unwrap();
    assert_eq!(
        active.after().transition(
            PairingEvent::credential_revoked(foreign),
            PairingInitiator::LocalOperator,
            2_000
        ),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        ready.transition(
            PairingEvent::credential_revoked(revoked.clone()),
            PairingInitiator::LocalOperator,
            2_000
        ),
        Err(PairingError::Conflict)
    );
    let issued = CredentialTransition::new(
        CredentialId::new("device").unwrap(),
        None,
        CredentialLifecycle {
            issued_at: 1,
            expires_at: None,
            revoked_at: None,
        },
        TransitionCause::Issued(IssuanceCause::DevicePairing),
        Initiator::LocalOperator,
        1,
    )
    .unwrap();
    assert_eq!(
        active.after().transition(
            PairingEvent::credential_revoked(issued),
            PairingInitiator::LocalOperator,
            1_000
        ),
        Err(PairingError::Conflict)
    );
    assert_eq!(
        active.after().transition(
            PairingEvent::credential_revoked(revoked.clone()),
            PairingInitiator::LocalOperator,
            2_001
        ),
        Err(PairingError::Invalid)
    );
    let terminal = active
        .after()
        .transition(
            PairingEvent::credential_revoked(revoked),
            PairingInitiator::LocalOperator,
            2_000,
        )
        .unwrap();
    assert_eq!(
        terminal.after().terminal(),
        Some((
            TerminalCause::CredentialRevoked,
            &PairingInitiator::LocalOperator
        ))
    );
}

#[test]
fn invitation_expiry_refuses_overflow_and_accepts_exact_maximum() {
    let original = record();
    let policy = PairingPolicy::new(1, 1).unwrap();
    assert_eq!(
        PairingRecord::new(original.id(), original.intent().clone(), u64::MAX, policy),
        Err(PairingError::Invalid)
    );
    let finite = PairingRecord::new(
        original.id(),
        original.intent().clone(),
        u64::MAX - 1,
        policy,
    )
    .unwrap();
    assert_eq!(finite.created_at_ms(), u64::MAX - 1);
    assert_eq!(finite.expires_at_ms(), u64::MAX);
}

#[test]
fn collection_completed_receipts_do_not_consume_unfinished_capacity() {
    let base = record();
    let mut active = Vec::new();
    for n in 0..=MAX_LIVE_PAIRINGS {
        let initial = PairingRecord::new(
            InvitationId::new([n as u8; 16]),
            base.intent().clone(),
            base.created_at_ms(),
            base.policy(),
        )
        .unwrap();
        let completed = reserve(&initial, 1)
            .transition(PairingEvent::claim(id(1), key()), device(), 1_002)
            .unwrap()
            .after()
            .transition(PairingEvent::approve(key(), 1), owner(), 1_003)
            .unwrap()
            .after()
            .transition(
                PairingEvent::stage(CredentialId::new("device").unwrap(), id(9), 1),
                owner(),
                1_004,
            )
            .unwrap()
            .after()
            .transition(
                PairingEvent::receiver(
                    CredentialId::new("device").unwrap(),
                    id(9),
                    ResourceId::new("receiver").unwrap(),
                    1,
                    1,
                ),
                PairingInitiator::System,
                1_005,
            )
            .unwrap()
            .after()
            .transition(PairingEvent::activate(1), owner(), 1_006)
            .unwrap()
            .after()
            .clone();
        active.push(completed);
    }
    assert_eq!(validate_pairing_collection(&active), Ok(()));
    let terminal: Vec<_> = active
        .iter()
        .map(|value| {
            value
                .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 1_007)
                .unwrap()
                .after()
                .clone()
        })
        .collect();
    assert_eq!(validate_pairing_collection(&terminal), Ok(()));
}

#[test]
fn cancellation_revocation_matches_original_command() {
    let active = receiver(&staged())
        .transition(PairingEvent::activate(1), owner(), 1_006)
        .unwrap();
    let cancel = active
        .after()
        .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 2_001)
        .unwrap();
    let credential = |name| {
        Credential::new(
            CredentialId::new(name).unwrap(),
            PrincipalId::new("owner").unwrap(),
            OrganizationId::new("org").unwrap(),
            AudienceId::new("gateway").unwrap(),
            1,
            None,
            vec![active.after().intent().grant().clone()],
        )
        .unwrap()
    };
    let principal = Initiator::Principal(PrincipalId::new("owner").unwrap());
    let original = credential("device")
        .revoke(2, principal.clone())
        .unwrap()
        .unwrap();
    assert_eq!(
        cancel.verify_cancellation_revocation(Some(&original)),
        Ok(())
    );
    assert_eq!(
        cancel.verify_cancellation_revocation(None),
        Err(PairingError::Conflict)
    );
    for (label, contradictory) in [
        (
            "credential",
            credential("other")
                .revoke(2, principal.clone())
                .unwrap()
                .unwrap(),
        ),
        (
            "operator",
            credential("device")
                .revoke(2, Initiator::LocalOperator)
                .unwrap()
                .unwrap(),
        ),
        (
            "principal",
            credential("device")
                .revoke(2, Initiator::Principal(PrincipalId::new("other").unwrap()))
                .unwrap()
                .unwrap(),
        ),
        (
            "time",
            credential("device")
                .revoke(3, principal.clone())
                .unwrap()
                .unwrap(),
        ),
        (
            "cause",
            credential("device")
                .supersede(
                    2,
                    CredentialId::new("replacement").unwrap(),
                    Supersession::OwnerRecovery,
                    principal,
                )
                .unwrap(),
        ),
    ] {
        assert_eq!(
            cancel.verify_cancellation_revocation(Some(&contradictory)),
            Err(PairingError::Conflict),
            "{label}"
        );
    }
    let unpublished = staged()
        .transition(PairingEvent::end(TerminalCause::Cancelled), owner(), 2_001)
        .unwrap();
    assert_eq!(unpublished.verify_cancellation_revocation(None), Ok(()));
    let independent = credential("device")
        .revoke(2, Initiator::LocalOperator)
        .unwrap()
        .unwrap();
    let ended = active
        .after()
        .transition(
            PairingEvent::credential_revoked(independent),
            PairingInitiator::LocalOperator,
            2_000,
        )
        .unwrap();
    assert_eq!(ended.verify_cancellation_revocation(None), Ok(()));
    assert_eq!(
        ended.after().terminal(),
        Some((
            TerminalCause::CredentialRevoked,
            &PairingInitiator::LocalOperator
        ))
    );
}
