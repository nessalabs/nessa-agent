use super::super::{
    AttemptId, ConsentIntent, DeviceKey, InvitationId, PairingError, PairingInitiator,
    PairingPolicy,
};
use crate::domain::{
    CredentialId, CredentialTransition, ResourceId, RevocationCause, TransitionCause,
};

/// The registry publication stage; device possession never implies approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingPhase {
    /// A new bounded PAKE attempt can be reserved.
    Available,
    /// One key has completed PAKE and owns the immutable claim.
    Claimed,
    /// The owner consented to the exact claim and current intent.
    Approved,
    /// Durable receiver dispatch precedes credential publication.
    Staging,
    /// Conditional registry commit published the key credential.
    Active,
    /// The invitation ended; any retained stage still requires cleanup.
    Terminal,
}

/// First cause ending the invitation, retained across cleanup and response loss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalCause {
    /// The linked canonical credential transition revoked the published device binding.
    CredentialRevoked,
    /// An authenticated owner explicitly refused the proposed key.
    Denied,
    /// An authenticated owner cancelled or revoked enrollment.
    Cancelled,
    /// Exclusive invitation expiry preceded publication.
    Expired,
    /// Volatile PAKE setup was lost before a claim was committed.
    Restarted,
}

/// Typed cause settling a charged handshake; diagnostics do not determine it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptFailure {
    /// The selected cryptographic implementation refused peer proof.
    InvalidProof,
    /// Physical original connection closed before claim commitment.
    ConnectionClosed,
    /// The bounded admitted handshake deadline won.
    HandshakeDeadline,
    /// Crypto work failed operationally without authenticating a claim.
    VerifierUnavailable,
}

/// Immutable outcome of an admitted and charged attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The original handshake can still complete.
    Pending,
    /// Failed proof or disconnected original handshake.
    Failed(AttemptFailure),
    /// This attempt owns the winning key claim.
    Claimed,
    /// Another key or a terminal invitation superseded this attempt.
    Superseded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Attempt {
    id: AttemptId,
    key: DeviceKey,
    input: [u8; 32],
    outcome: AttemptOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReceiverStage {
    credential: CredentialId,
    request: AttemptId,
    receiver: Option<ResourceId>,
    epoch: Option<u64>,
    cleaned: bool,
}

/// One validated event; the application establishes proof and authorization.
/// Pure construction here is not a cryptographic authentication verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingEvent(Event);
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    CredentialRevoked(Box<CredentialTransition>),
    Reserve(AttemptId, DeviceKey, [u8; 32]),
    Fail(AttemptId, DeviceKey, AttemptFailure),
    Claim(AttemptId, DeviceKey),
    Approve(DeviceKey, u64),
    Stage(CredentialId, AttemptId, u64),
    Receiver(CredentialId, AttemptId, ResourceId, u64, u64),
    Activate(u64),
    End(TerminalCause),
    Cleanup(CredentialId, AttemptId, ResourceId, u64),
    NoReceiverCleanup(CredentialId, AttemptId, u64),
}
impl PairingEvent {
    /// Consume exact canonical credential revocation evidence, preserving cause and initiator.
    pub fn credential_revoked(transition: CredentialTransition) -> Self {
        Self(Event::CredentialRevoked(Box::new(transition)))
    }
    pub(crate) fn kind(&self) -> &Event {
        &self.0
    }
    /// Admit one immutable attempt fingerprint after transport key possession.
    pub fn reserve(id: AttemptId, key: DeviceKey, input: [u8; 32]) -> Self {
        Self(Event::Reserve(id, key, input))
    }
    /// Settle the admitted handshake after failed proof or physical disconnect.
    pub fn fail(id: AttemptId, key: DeviceKey, cause: AttemptFailure) -> Self {
        Self(Event::Fail(id, key, cause))
    }
    /// Apply an externally verified KE3 result to its earlier reservation.
    pub fn claim(id: AttemptId, key: DeviceKey) -> Self {
        Self(Event::Claim(id, key))
    }
    /// Consent to the exact claimed key and immutable intent generation.
    pub fn approve(key: DeviceKey, generation: u64) -> Self {
        Self(Event::Approve(key, generation))
    }
    /// Save dispatch identity before ensuring a receiver for this reserved credential.
    pub fn stage(credential: CredentialId, request: AttemptId, generation: u64) -> Self {
        Self(Event::Stage(credential, request, generation))
    }
    /// Correlate the canonical receiver outcome to the prior dispatch.
    pub fn receiver(
        credential: CredentialId,
        request: AttemptId,
        receiver: ResourceId,
        epoch: u64,
        generation: u64,
    ) -> Self {
        Self(Event::Receiver(
            credential, request, receiver, epoch, generation,
        ))
    }
    /// Publish only after canonical receiver evidence and fresh revision CAS.
    pub fn activate(generation: u64) -> Self {
        Self(Event::Activate(generation))
    }
    /// End the invitation, preserving the first cause and any physical obligation.
    pub fn end(cause: TerminalCause) -> Self {
        Self(Event::End(cause))
    }
    /// Complete a terminal stage after its physical dispatch owner drained and
    /// the canonical receiver lookup proved no original Pair receipt. Application
    /// admission owns that proof; this pure event checks the exact retained target.
    pub fn no_receiver_cleanup(
        credential: CredentialId,
        request: AttemptId,
        generation: u64,
    ) -> Self {
        Self(Event::NoReceiverCleanup(credential, request, generation))
    }
    /// Record actual fencing of the exact canonical receiver; its epoch advanced.
    pub fn cleanup(
        credential: CredentialId,
        request: AttemptId,
        receiver: ResourceId,
        fenced_epoch: u64,
    ) -> Self {
        Self(Event::Cleanup(credential, request, receiver, fenced_epoch))
    }
}

/// Immutable snapshot of one durable pairing consistency boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingRecord {
    id: InvitationId,
    intent: ConsentIntent,
    created_at_ms: u64,
    expires_at_ms: u64,
    policy: PairingPolicy,
    phase: PairingPhase,
    attempts: Box<[Attempt]>,
    claim: Option<(AttemptId, DeviceKey)>,
    stage: Option<ReceiverStage>,
    terminal: Option<(TerminalCause, PairingInitiator)>,
}
impl PairingRecord {
    /// Create a finite invitation using time supplied by the injected application clock.
    pub fn new(
        id: InvitationId,
        intent: ConsentIntent,
        created_at_ms: u64,
        policy: PairingPolicy,
    ) -> Result<Self, PairingError> {
        let expires_at_ms = created_at_ms
            .checked_add(policy.lifetime_ms())
            .ok_or(PairingError::Invalid)?;
        Ok(Self {
            id,
            intent,
            created_at_ms,
            expires_at_ms,
            policy,
            phase: PairingPhase::Available,
            attempts: Box::new([]),
            claim: None,
            stage: None,
            terminal: None,
        })
    }
    /// Durable invitation identity, independent of names or connection handles.
    pub fn id(&self) -> InvitationId {
        self.id
    }
    /// Full immutable consent owned by this invitation.
    pub fn intent(&self) -> &ConsentIntent {
        &self.intent
    }
    /// Current publication phase.
    pub fn phase(&self) -> PairingPhase {
        self.phase
    }
    /// Creation time supplied by the application, in Unix milliseconds.
    pub fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }
    /// Finite policy captured when the invitation began.
    pub fn policy(&self) -> PairingPolicy {
        self.policy
    }
    /// Exclusive expiry in Unix milliseconds.
    pub fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
    /// Number of durably admitted attempts, including failed handshakes.
    pub fn charged_attempts(&self) -> usize {
        self.attempts.len()
    }
    /// Winning claim; no key is published before its attempt is accepted.
    pub fn claim_binding(&self) -> Option<(AttemptId, DeviceKey)> {
        self.claim
    }
    /// Check an immutable reservation replay before consulting admission capacity.
    pub fn reservation_status(
        &self,
        id: AttemptId,
        key: DeviceKey,
        input: [u8; 32],
    ) -> Result<Option<AttemptOutcome>, PairingError> {
        match self.attempts.iter().find(|attempt| attempt.id == id) {
            Some(attempt) if attempt.key == key && attempt.input == input => {
                Ok(Some(attempt.outcome))
            }
            Some(_) => Err(PairingError::Conflict),
            None => Ok(None),
        }
    }
    /// Pending or terminal outcome readable only by the earlier reserved device key.
    pub fn attempt_status(
        &self,
        id: AttemptId,
        key: DeviceKey,
    ) -> Result<AttemptOutcome, PairingError> {
        let attempt = self
            .attempts
            .iter()
            .find(|a| a.id == id)
            .ok_or(PairingError::Conflict)?;
        if attempt.key != key {
            return Err(PairingError::WrongActor);
        }
        Ok(attempt.outcome)
    }
    /// Original terminal cause and attribution; late cleanup does not replace it.
    pub fn terminal(&self) -> Option<(TerminalCause, &PairingInitiator)> {
        self.terminal.as_ref().map(|(cause, actor)| (*cause, actor))
    }
    /// Reserved credential remains unusable until the Active phase.
    pub fn credential(&self) -> Option<&CredentialId> {
        self.stage.as_ref().map(|stage| &stage.credential)
    }
    /// Exact durable receiver dispatch identity retained through cancellation.
    pub fn stage_binding(&self) -> Option<(&CredentialId, AttemptId)> {
        self.stage
            .as_ref()
            .map(|stage| (&stage.credential, stage.request))
    }
    /// Canonical receiver result, when physically confirmed.
    pub fn receiver_binding(&self) -> Option<(&ResourceId, u64)> {
        self.stage
            .as_ref()
            .and_then(|stage| stage.receiver.as_ref().zip(stage.epoch))
    }
    /// A terminal stage needs an exact receiver outcome/fence before cleanup can finish.
    pub fn cleanup_pending(&self) -> bool {
        self.phase == PairingPhase::Terminal
            && self.stage.as_ref().is_some_and(|stage| !stage.cleaned)
    }

    fn expiry_due(&self, at_ms: u64) -> bool {
        matches!(
            self.phase,
            PairingPhase::Available
                | PairingPhase::Claimed
                | PairingPhase::Approved
                | PairingPhase::Staging
        ) && at_ms >= self.expires_at_ms
    }
    /// Expire only an eligible current record at its exclusive deadline.
    /// Active, Terminal and not-due records retain their existing causal evidence.
    pub fn expire_if_due(&self, at_ms: u64) -> Result<Option<PairingTransition>, PairingError> {
        if !self.expiry_due(at_ms) {
            return Ok(None);
        }
        self.transition(
            PairingEvent::end(TerminalCause::Expired),
            PairingInitiator::System,
            at_ms,
        )
        .map(Some)
    }

    /// Order a legal event and return causal evidence without changing this value.
    /// Proof/permission validation and atomic persistence are application responsibilities.
    pub fn transition(
        &self,
        event: PairingEvent,
        actor: PairingInitiator,
        at_ms: u64,
    ) -> Result<PairingTransition, PairingError> {
        if at_ms < self.created_at_ms {
            return Err(PairingError::Invalid);
        }
        let mut next = self.clone();
        match &event.0 {
            Event::CredentialRevoked(transition) => {
                if self.phase != PairingPhase::Active
                    || self.credential() != Some(transition.credential_id())
                    || !matches!(transition.cause(), TransitionCause::Revoked(_))
                {
                    return Err(PairingError::Conflict);
                }
                if actor != PairingInitiator::from_credential(transition.initiator()) {
                    return Err(PairingError::WrongActor);
                }
                if at_ms != self.canonical_revocation_lower_bound_ms(transition)? {
                    return Err(PairingError::Invalid);
                }
                next.phase = PairingPhase::Terminal;
                next.terminal = Some((TerminalCause::CredentialRevoked, actor.clone()));
            }

            Event::Reserve(id, key, input) => {
                if self.reservation_status(*id, *key, *input)?.is_some() {
                    return Err(PairingError::Conflict); // Receipt replay is a read, not another transition.
                }
                self.available(at_ms)?;
                if actor != PairingInitiator::Device(*key) {
                    return Err(PairingError::WrongActor);
                }
                if self.attempts.len() >= usize::from(self.policy.attempts()) {
                    return Err(PairingError::AttemptsExhausted);
                }
                if self
                    .attempts
                    .iter()
                    .any(|a| a.outcome == AttemptOutcome::Pending)
                {
                    return Err(PairingError::Capacity);
                }
                let mut attempts = self.attempts.to_vec();
                attempts.push(Attempt {
                    id: *id,
                    key: *key,
                    input: *input,
                    outcome: AttemptOutcome::Pending,
                });
                next.attempts = attempts.into_boxed_slice();
            }
            Event::Fail(id, key, _) | Event::Claim(id, key) => {
                if actor != PairingInitiator::Device(*key) {
                    return Err(PairingError::WrongActor);
                }
                let index = self
                    .attempts
                    .iter()
                    .position(|a| a.id == *id)
                    .ok_or(PairingError::Conflict)?;
                let prior = &self.attempts[index];
                if prior.key != *key {
                    return Err(PairingError::WrongActor);
                }
                if prior.outcome != AttemptOutcome::Pending {
                    return Err(PairingError::Ineligible);
                }
                let mut attempts = self.attempts.to_vec();
                if matches!(event.0, Event::Claim(..)) {
                    self.available(at_ms)?;
                    attempts[index].outcome = AttemptOutcome::Claimed;
                    next.claim = Some((*id, *key));
                    next.phase = PairingPhase::Claimed;
                } else {
                    let Event::Fail(_, _, cause) = event.0 else {
                        return Err(PairingError::Invalid);
                    };
                    attempts[index].outcome = AttemptOutcome::Failed(cause);
                }
                next.attempts = attempts.into_boxed_slice();
            }
            Event::Approve(key, generation) => {
                self.owner(&actor)?;
                self.generation(*generation)?;
                self.before_expiry(at_ms)?;
                if self.phase != PairingPhase::Claimed
                    || self.claim.is_none_or(|(_, bound)| bound != *key)
                {
                    return Err(PairingError::Conflict);
                }
                next.phase = PairingPhase::Approved;
            }
            Event::Stage(credential, request, generation) => {
                self.owner(&actor)?;
                self.generation(*generation)?;
                self.before_expiry(at_ms)?;
                if self.phase != PairingPhase::Approved {
                    return Err(PairingError::Ineligible);
                }
                next.stage = Some(ReceiverStage {
                    credential: credential.clone(),
                    request: *request,
                    receiver: None,
                    epoch: None,
                    cleaned: false,
                });
                next.phase = PairingPhase::Staging;
            }
            Event::Receiver(credential, request, receiver, epoch, generation) => {
                if actor != PairingInitiator::System {
                    return Err(PairingError::WrongActor);
                }
                self.generation(*generation)?;
                let stage = self.stage.as_ref().ok_or(PairingError::Ineligible)?;
                if stage.cleaned
                    || stage.credential != *credential
                    || stage.request != *request
                    || *epoch == 0
                    || stage.receiver.is_some()
                {
                    return Err(PairingError::Conflict);
                }
                let mut stage = stage.clone();
                stage.receiver = Some(receiver.clone());
                stage.epoch = Some(*epoch);
                next.stage = Some(stage);
            }
            Event::Activate(generation) => {
                self.owner(&actor)?;
                self.generation(*generation)?;
                self.before_expiry(at_ms)?;
                if self.phase != PairingPhase::Staging || self.receiver_binding().is_none() {
                    return Err(PairingError::Ineligible);
                }
                next.phase = PairingPhase::Active;
            }
            Event::End(cause) => {
                if self.phase == PairingPhase::Terminal {
                    return Err(PairingError::Ineligible);
                }
                match cause {
                    TerminalCause::Denied | TerminalCause::Cancelled => self.owner(&actor)?,
                    TerminalCause::Expired
                        if self.expiry_due(at_ms) && actor == PairingInitiator::System => {}
                    TerminalCause::Restarted
                        if self.phase == PairingPhase::Available
                            && actor == PairingInitiator::System => {}
                    _ => return Err(PairingError::Invalid),
                }
                if *cause == TerminalCause::Denied && self.phase == PairingPhase::Active {
                    return Err(PairingError::Ineligible);
                }
                next.phase = PairingPhase::Terminal;
                next.terminal = Some((*cause, actor.clone()));
                next.attempts = self
                    .attempts
                    .iter()
                    .cloned()
                    .map(|mut a| {
                        if a.outcome == AttemptOutcome::Pending {
                            a.outcome = AttemptOutcome::Superseded;
                        }
                        a
                    })
                    .collect();
            }
            Event::NoReceiverCleanup(credential, request, generation) => {
                if actor != PairingInitiator::System {
                    return Err(PairingError::WrongActor);
                }
                self.generation(*generation)?;
                let stage = self.stage.as_ref().ok_or(PairingError::Ineligible)?;
                if self.phase != PairingPhase::Terminal
                    || stage.cleaned
                    || stage.credential != *credential
                    || stage.request != *request
                    || stage.receiver.is_some()
                {
                    return Err(PairingError::Conflict);
                }
                let mut stage = stage.clone();
                stage.cleaned = true;
                next.stage = Some(stage);
            }
            Event::Cleanup(credential, request, receiver, epoch) => {
                if actor != PairingInitiator::System {
                    return Err(PairingError::WrongActor);
                }
                let stage = self.stage.as_ref().ok_or(PairingError::Ineligible)?;
                if self.phase != PairingPhase::Terminal
                    || stage.cleaned
                    || stage.credential != *credential
                    || stage.request != *request
                    || stage.receiver.as_ref() != Some(receiver)
                    || stage.epoch.is_none_or(|previous| *epoch <= previous)
                {
                    return Err(PairingError::Conflict);
                }
                let mut stage = stage.clone();
                stage.cleaned = true;
                stage.epoch = Some(*epoch);
                next.stage = Some(stage);
            }
        }
        Ok(PairingTransition {
            before: self.clone(),
            after: next,
            event,
            actor,
            at_ms,
        })
    }
    /// Derived ordering lower bound for a canonical second-resolution revocation.
    /// The original transition retains exact seconds, cause, actor, and credential identity.
    pub fn canonical_revocation_lower_bound_ms(
        &self,
        transition: &CredentialTransition,
    ) -> Result<u64, PairingError> {
        transition
            .at()
            .checked_mul(1000)
            .map(|lower| lower.max(self.created_at_ms))
            .ok_or(PairingError::Invalid)
    }
    fn before_expiry(&self, at: u64) -> Result<(), PairingError> {
        if at >= self.expires_at_ms {
            Err(PairingError::Expired)
        } else {
            Ok(())
        }
    }
    fn available(&self, at: u64) -> Result<(), PairingError> {
        self.before_expiry(at)?;
        if self.phase != PairingPhase::Available {
            Err(PairingError::Ineligible)
        } else {
            Ok(())
        }
    }
    fn generation(&self, generation: u64) -> Result<(), PairingError> {
        if generation != self.intent.generation() {
            Err(PairingError::StaleGeneration)
        } else {
            Ok(())
        }
    }
    fn owner(&self, actor: &PairingInitiator) -> Result<(), PairingError> {
        if actor != &PairingInitiator::Principal(self.intent.owner().clone()) {
            Err(PairingError::WrongActor)
        } else {
            Ok(())
        }
    }
}

/// Complete immutable before/after/event/actor evidence, committed with its state.
#[must_use = "pairing transitions carry audit evidence that must be committed"]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairingTransition {
    before: PairingRecord,
    after: PairingRecord,
    event: PairingEvent,
    actor: PairingInitiator,
    at_ms: u64,
}
impl PairingTransition {
    /// Check the canonical revocation committed by cancellation of an Active pairing.
    /// Other transitions, including cancellation before publication and independent
    /// credential revocation, have no cancellation-owned revocation to correlate.
    ///
    /// # Errors
    /// Returns [`PairingError::Conflict`] when an Active cancellation lacks the
    /// original credential's Explicit revocation, actor or whole-second command time.
    pub fn verify_cancellation_revocation(
        &self,
        revocation: Option<&CredentialTransition>,
    ) -> Result<(), PairingError> {
        if self.before.phase() != PairingPhase::Active
            || self.event.kind() != &Event::End(TerminalCause::Cancelled)
        {
            return Ok(());
        }
        let revocation = revocation.ok_or(PairingError::Conflict)?;
        if self.before.credential() != Some(revocation.credential_id())
            || revocation.cause() != &TransitionCause::Revoked(RevocationCause::Explicit)
            || self.actor != PairingInitiator::from_credential(revocation.initiator())
            || revocation.at() != self.at_ms / 1000
        {
            return Err(PairingError::Conflict);
        }
        Ok(())
    }
    /// Record before the decision.
    pub fn before(&self) -> &PairingRecord {
        &self.before
    }
    /// Replacement snapshot after the decision.
    pub fn after(&self) -> &PairingRecord {
        &self.after
    }
    /// Exact event whose relationship is checked on restoration.
    pub fn event(&self) -> &PairingEvent {
        &self.event
    }
    /// Original actor, rather than a later cleanup caller.
    pub fn actor(&self) -> &PairingInitiator {
        &self.actor
    }
    /// Observed decision milliseconds, or the documented derived lower bound for
    /// a canonical revocation bridge whose original timestamp remains in seconds.
    pub fn ordering_time_ms(&self) -> u64 {
        self.at_ms
    }
    /// Correlate privately minted evidence with the current record before append.
    pub fn verify(&self, previous: &PairingRecord) -> Result<(), PairingError> {
        if &self.before != previous {
            return Err(PairingError::Conflict);
        }
        Ok(())
    }
}
