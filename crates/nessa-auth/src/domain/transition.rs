//! Evidence of one credential lifecycle change: what it targeted, what it looked
//! like before and after, why it happened, and who caused it.
//!
//! A transition is produced by [`Credential`](super::Credential) as the result of
//! a validated change and is never assembled after the fact. Storage adapters
//! persist it in the same commit as the state it describes; application code
//! maps it to a record without altering its meaning. No clock, serde, or
//! persistence here: `at` is the command's own time in Unix seconds.
use super::{CredentialId, DomainError, PrincipalId};

/// The fields of a credential that a lifecycle transition can change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CredentialLifecycle {
    /// Inclusive start of validity, in Unix seconds.
    pub issued_at: u64,
    /// Exclusive end of validity, in Unix seconds.
    pub expires_at: Option<u64>,
    /// Recorded revocation time, or None while still unrevoked.
    pub revoked_at: Option<u64>,
}

/// Which command created a credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IssuanceCause {
    /// First owner credential of a fresh registry, written offline.
    Bootstrap,
    /// `credential.issue` by an authenticated admin.
    AdminIssue,
    /// Offline provisioning of a distinct surface credential.
    SurfaceProvision,
    /// Offline replacement of the owner credential.
    OwnerRecovery,
}

/// Which automatic command replaced a credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Supersession {
    /// A surface was re-provisioned and its earlier credential retired.
    Provision,
    /// The owner recovered and the earlier owner credential was displaced.
    OwnerRecovery,
}

/// Why a credential stopped being valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevocationCause {
    /// `credential.revoke` naming this credential deliberately.
    Explicit,
    /// Replaced automatically by `by` as part of the named command.
    Superseded {
        by: CredentialId,
        kind: Supersession,
    },
}

/// Lifecycle cause of a transition. Expiry is not a cause: nothing happens at
/// expiry, and the expiry instant is already part of the issuance evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransitionCause {
    Issued(IssuanceCause),
    Revoked(RevocationCause),
    /// The credential existed before transitions were recorded. Its real cause
    /// is unknown and is labelled as such rather than guessed.
    PredatesJournal,
}

/// Who caused a transition. Automatic revocations carry the initiator of the
/// command that triggered them; the cause says that they were automatic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Initiator {
    /// Verified principal that issued the command.
    Principal(PrincipalId),
    /// The operating-system owner running an offline command under the registry
    /// lock. There is no authenticated principal to name.
    LocalOperator,
    /// Only valid with [`TransitionCause::PredatesJournal`].
    Unknown,
}

/// One validated lifecycle change of one credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CredentialTransition {
    credential_id: CredentialId,
    before: Option<CredentialLifecycle>,
    after: CredentialLifecycle,
    cause: TransitionCause,
    initiator: Initiator,
    at: u64,
}

impl CredentialTransition {
    /// Assemble a transition and check that its shape matches its cause. This
    /// is the rule storage uses when replaying recorded evidence; live changes
    /// produced by [`Credential`](super::Credential) satisfy it by construction.
    pub fn new(
        credential_id: CredentialId,
        before: Option<CredentialLifecycle>,
        after: CredentialLifecycle,
        cause: TransitionCause,
        initiator: Initiator,
        at: u64,
    ) -> Result<Self, DomainError> {
        let invalid = |reason| Err(DomainError::InvalidTransition { reason });
        if matches!(initiator, Initiator::Unknown)
            != matches!(cause, TransitionCause::PredatesJournal)
        {
            return invalid("unknown initiator is only valid for a pre-journal record");
        }
        match &cause {
            TransitionCause::Issued(_) => {
                if before.is_some() || after.revoked_at.is_some() || at != after.issued_at {
                    return invalid("issuance has no prior state and is not revoked");
                }
            }
            TransitionCause::PredatesJournal => {
                if before.is_some() || at != after.issued_at {
                    return invalid("pre-journal record has no prior state");
                }
            }
            TransitionCause::Revoked(revocation) => {
                let Some(prior) = before else {
                    return invalid("revocation needs a prior state");
                };
                let Some(revoked_at) = after.revoked_at else {
                    return invalid("revocation must record a revocation time");
                };
                if prior.revoked_at.is_some()
                    || prior.issued_at != after.issued_at
                    || prior.expires_at != after.expires_at
                    || revoked_at < after.issued_at
                {
                    return invalid("revocation changes only the revocation time");
                }
                match revocation {
                    RevocationCause::Explicit if at != revoked_at => {
                        return invalid("explicit revocation is recorded at its requested time");
                    }
                    RevocationCause::Superseded { by, .. } if by == &credential_id => {
                        return invalid("a credential cannot supersede itself");
                    }
                    RevocationCause::Superseded { .. } if revoked_at != at.max(after.issued_at) => {
                        return invalid(
                            "supersession is recorded at the later of command and issuance",
                        );
                    }
                    _ => {}
                }
            }
        }
        Ok(Self {
            credential_id,
            before,
            after,
            cause,
            initiator,
            at,
        })
    }

    /// Credential this transition changed.
    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }
    /// Lifecycle before the change; None for issuance.
    pub fn before(&self) -> Option<&CredentialLifecycle> {
        self.before.as_ref()
    }
    /// Lifecycle after the change.
    pub fn after(&self) -> &CredentialLifecycle {
        &self.after
    }
    /// Why the change happened.
    pub fn cause(&self) -> &TransitionCause {
        &self.cause
    }
    /// Who caused it.
    pub fn initiator(&self) -> &Initiator {
        &self.initiator
    }
    /// The command's own time in Unix seconds, not an observation time.
    pub fn at(&self) -> u64 {
        self.at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> CredentialId {
        CredentialId::new(value).unwrap()
    }
    fn live() -> CredentialLifecycle {
        CredentialLifecycle {
            issued_at: 100,
            expires_at: Some(200),
            revoked_at: None,
        }
    }
    fn revoked(at: u64) -> CredentialLifecycle {
        CredentialLifecycle {
            revoked_at: Some(at),
            ..live()
        }
    }

    #[test]
    fn issuance_needs_no_prior_state_and_matches_its_instant() {
        let cause = TransitionCause::Issued(IssuanceCause::AdminIssue);
        assert!(CredentialTransition::new(
            id("c"),
            None,
            live(),
            cause.clone(),
            Initiator::LocalOperator,
            100
        )
        .is_ok());
        assert!(CredentialTransition::new(
            id("c"),
            Some(live()),
            live(),
            cause.clone(),
            Initiator::LocalOperator,
            100
        )
        .is_err());
        assert!(CredentialTransition::new(
            id("c"),
            None,
            live(),
            cause,
            Initiator::LocalOperator,
            99
        )
        .is_err());
    }

    #[test]
    fn unknown_initiator_is_only_for_pre_journal_records() {
        assert!(CredentialTransition::new(
            id("c"),
            None,
            live(),
            TransitionCause::PredatesJournal,
            Initiator::Unknown,
            100
        )
        .is_ok());
        assert!(CredentialTransition::new(
            id("c"),
            None,
            live(),
            TransitionCause::PredatesJournal,
            Initiator::LocalOperator,
            100
        )
        .is_err());
        assert!(CredentialTransition::new(
            id("c"),
            None,
            live(),
            TransitionCause::Issued(IssuanceCause::Bootstrap),
            Initiator::Unknown,
            100
        )
        .is_err());
    }

    #[test]
    fn revocation_changes_only_the_revocation_time() {
        let explicit = TransitionCause::Revoked(RevocationCause::Explicit);
        let operator = Initiator::LocalOperator;
        assert!(CredentialTransition::new(
            id("c"),
            Some(live()),
            revoked(150),
            explicit.clone(),
            operator.clone(),
            150
        )
        .is_ok());
        // No prior state.
        assert!(CredentialTransition::new(
            id("c"),
            None,
            revoked(150),
            explicit.clone(),
            operator.clone(),
            150
        )
        .is_err());
        // Already revoked before.
        assert!(CredentialTransition::new(
            id("c"),
            Some(revoked(120)),
            revoked(150),
            explicit.clone(),
            operator.clone(),
            150
        )
        .is_err());
        // Explicit time must be the requested time.
        assert!(CredentialTransition::new(
            id("c"),
            Some(live()),
            revoked(150),
            explicit.clone(),
            operator.clone(),
            151
        )
        .is_err());
        // Before issuance.
        assert!(CredentialTransition::new(
            id("c"),
            Some(live()),
            revoked(99),
            explicit,
            operator,
            99
        )
        .is_err());
    }

    #[test]
    fn supersession_is_clamped_forward_and_never_by_itself() {
        let by = |value: &str| {
            TransitionCause::Revoked(RevocationCause::Superseded {
                by: id(value),
                kind: Supersession::Provision,
            })
        };
        let operator = Initiator::LocalOperator;
        assert!(CredentialTransition::new(
            id("c"),
            Some(live()),
            revoked(100),
            by("d"),
            operator.clone(),
            50
        )
        .is_ok());
        assert!(CredentialTransition::new(
            id("c"),
            Some(live()),
            revoked(50),
            by("d"),
            operator.clone(),
            50
        )
        .is_err());
        assert!(CredentialTransition::new(
            id("c"),
            Some(live()),
            revoked(150),
            by("c"),
            operator,
            150
        )
        .is_err());
    }
}
