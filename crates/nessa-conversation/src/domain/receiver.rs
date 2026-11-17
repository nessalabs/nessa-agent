//! One owner for receiver state changes and their replayed evidence.

use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};

/// Durable server-owned binding of one credential to a paired receiver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverBinding {
    pub receiver_id: String,
    pub credential_id: CredentialId,
    pub organization_id: OrganizationId,
    /// The owner whose conversations this receiver reads: the grantor, who
    /// paired it. It is never the reader. For a device the two are the same
    /// principal; for a peer gateway (`docs/design/auth/peer-gateways.md`)
    /// the reader is the session's own `gateway` principal. Admission matches
    /// a binding to its session by credential and organization, never by this
    /// field, and whatever records a read attributes it to the session's
    /// principal.
    pub owner_id: PrincipalId,
    pub access_epoch: u64,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiverInitiator {
    Principal(PrincipalId),
    System,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiverIntent {
    Pair {
        receiver_id: String,
        credential_id: CredentialId,
        organization_id: OrganizationId,
        owner_id: PrincipalId,
    },
    Revoke,
    Regrant(CredentialId),
    PolicyChanged,
}

impl ReceiverIntent {
    pub fn from_cause_name(name: &str, after: &ReceiverBinding) -> Option<Self> {
        match name {
            "paired" => Some(Self::Pair {
                receiver_id: after.receiver_id.clone(),
                credential_id: after.credential_id.clone(),
                organization_id: after.organization_id.clone(),
                owner_id: after.owner_id.clone(),
            }),
            "revoked" => Some(Self::Revoke),
            "regranted" => Some(Self::Regrant(after.credential_id.clone())),
            "policy_changed" => Some(Self::PolicyChanged),
            _ => None,
        }
    }

    pub fn cause_name(&self) -> &'static str {
        match self {
            Self::Pair { .. } => "paired",
            Self::Revoke => "revoked",
            Self::Regrant(_) => "regranted",
            Self::PolicyChanged => "policy_changed",
        }
    }
}

impl ReceiverInitiator {
    pub fn from_evidence_parts(kind: &str, id: Option<String>) -> Option<Self> {
        match (kind, id) {
            ("principal", Some(id)) => PrincipalId::new(id).ok().map(Self::Principal),
            ("system", None) => Some(Self::System),
            _ => None,
        }
    }

    pub fn evidence_parts(&self) -> (&'static str, Option<&str>) {
        match self {
            Self::Principal(id) => ("principal", Some(id.as_str())),
            Self::System => ("system", None),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverTransitionError {
    Conflict,
    Exhausted,
}

/// Immutable evidence whose complete before/after meaning is checked on replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiverTransition {
    pub before: Option<ReceiverBinding>,
    pub after: ReceiverBinding,
    pub cause: ReceiverIntent,
    pub initiator: ReceiverInitiator,
    pub request_id: String,
    pub observed_at_ms: i64,
}

impl ReceiverTransition {
    pub fn apply(
        before: Option<&ReceiverBinding>,
        cause: ReceiverIntent,
        initiator: ReceiverInitiator,
        request_id: String,
        observed_at_ms: i64,
    ) -> Result<Self, ReceiverTransitionError> {
        if request_id.trim().is_empty() || request_id.len() > 200 || observed_at_ms < 0 {
            return Err(ReceiverTransitionError::Conflict);
        }
        let after = match (&cause, before, &initiator) {
            (
                ReceiverIntent::Pair {
                    receiver_id,
                    credential_id,
                    organization_id,
                    owner_id,
                },
                None,
                ReceiverInitiator::Principal(_),
            ) if !receiver_id.is_empty() && receiver_id.len() <= 128 => ReceiverBinding {
                receiver_id: receiver_id.clone(),
                credential_id: credential_id.clone(),
                organization_id: organization_id.clone(),
                owner_id: owner_id.clone(),
                access_epoch: 1,
                active: true,
            },
            // A principal revokes a receiver; the system fences one whose device
            // enrollment has ended (device pairing cleanup).
            (ReceiverIntent::Revoke, Some(current), _) if current.active => ReceiverBinding {
                access_epoch: next_epoch(current)?,
                active: false,
                ..current.clone()
            },
            (
                ReceiverIntent::Regrant(credential_id),
                Some(current),
                ReceiverInitiator::Principal(_),
            ) if !current.active && credential_id != &current.credential_id => ReceiverBinding {
                credential_id: credential_id.clone(),
                access_epoch: next_epoch(current)?,
                active: true,
                ..current.clone()
            },
            (ReceiverIntent::PolicyChanged, Some(current), ReceiverInitiator::System) => {
                ReceiverBinding {
                    access_epoch: next_epoch(current)?,
                    ..current.clone()
                }
            }
            _ => return Err(ReceiverTransitionError::Conflict),
        };
        Ok(Self {
            before: before.cloned(),
            after,
            cause,
            initiator,
            request_id,
            observed_at_ms,
        })
    }

    /// Replay is the same transition, with every persisted fact compared.
    pub fn verify(&self, before: Option<&ReceiverBinding>) -> Result<(), ReceiverTransitionError> {
        let expected = Self::apply(
            before,
            self.cause.clone(),
            self.initiator.clone(),
            self.request_id.clone(),
            self.observed_at_ms,
        )?;
        if expected == *self {
            Ok(())
        } else {
            Err(ReceiverTransitionError::Conflict)
        }
    }
}

/// The receiver a device enrollment paired, as its stage recorded it: the
/// one rule for whether a receiver still holds that pairing, and the system's
/// fence of it (device pairing design rows P39, P41, P42, P46).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedReceiver {
    pub receiver_id: String,
    pub credential_id: CredentialId,
    pub organization_id: OrganizationId,
    pub owner_id: PrincipalId,
    pub paired_epoch: u64,
}

impl PairedReceiver {
    /// Whether `binding` is this pairing's receiver, credential, organization
    /// and owner at the paired epoch or later, active or not.
    pub fn names(&self, binding: &ReceiverBinding) -> bool {
        binding.receiver_id == self.receiver_id
            && binding.credential_id == self.credential_id
            && binding.organization_id == self.organization_id
            && binding.owner_id == self.owner_id
            && binding.access_epoch >= self.paired_epoch
    }

    /// Whether the current binding still holds this pairing: named and active,
    /// and `revoked_since` (the journal's answer) is false. A revocation of the
    /// credential since the pair ends the pairing even if it was regranted back.
    pub fn admits(&self, current: &ReceiverBinding, revoked_since: bool) -> bool {
        current.active && self.names(current) && !revoked_since
    }

    /// Whether a persisted transition revoked this pairing's credential on this
    /// receiver after it was paired.
    pub fn revoked_by(&self, cause: &ReceiverIntent, before: Option<&ReceiverBinding>) -> bool {
        *cause == ReceiverIntent::Revoke
            && before.is_some_and(|before| before.active && self.names(before))
    }

    /// The system's fence of this pairing's receiver. Only a binding that
    /// still holds the pairing may be fenced; anything else conflicts.
    pub fn fence(
        &self,
        current: &ReceiverBinding,
        revoked_since: bool,
        request_id: String,
        observed_at_ms: i64,
    ) -> Result<ReceiverTransition, ReceiverTransitionError> {
        if !self.admits(current, revoked_since) {
            return Err(ReceiverTransitionError::Conflict);
        }
        ReceiverTransition::apply(
            Some(current),
            ReceiverIntent::Revoke,
            ReceiverInitiator::System,
            request_id,
            observed_at_ms,
        )
    }
}

fn next_epoch(current: &ReceiverBinding) -> Result<u64, ReceiverTransitionError> {
    current
        .access_epoch
        .checked_add(1)
        .filter(|epoch| i64::try_from(*epoch).is_ok())
        .ok_or(ReceiverTransitionError::Exhausted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> ReceiverBinding {
        ReceiverBinding {
            receiver_id: "receiver".into(),
            credential_id: CredentialId::new("first").unwrap(),
            organization_id: OrganizationId::new("org").unwrap(),
            owner_id: PrincipalId::new("owner").unwrap(),
            access_epoch: 1,
            active: true,
        }
    }

    #[test]
    fn one_transition_owner_rejects_illegal_shapes_and_replays_exact_evidence() {
        let actor = ReceiverInitiator::Principal(PrincipalId::new("owner").unwrap());
        let paired = ReceiverTransition::apply(
            None,
            ReceiverIntent::Pair {
                receiver_id: "receiver".into(),
                credential_id: binding().credential_id,
                organization_id: binding().organization_id,
                owner_id: binding().owner_id,
            },
            actor.clone(),
            "pair".into(),
            1,
        )
        .unwrap();
        assert_eq!(paired.after, binding());
        assert_eq!(paired.verify(None), Ok(()));
        assert_eq!(
            paired.verify(Some(&binding())),
            Err(ReceiverTransitionError::Conflict)
        );
        assert_eq!(
            ReceiverTransition::apply(
                Some(&binding()),
                ReceiverIntent::Revoke,
                actor.clone(),
                "  ".into(),
                1
            ),
            Err(ReceiverTransitionError::Conflict)
        );
        let revoked = ReceiverTransition::apply(
            Some(&binding()),
            ReceiverIntent::Revoke,
            actor.clone(),
            "revoke".into(),
            2,
        )
        .unwrap();
        assert_eq!(revoked.verify(Some(&binding())), Ok(()));
        assert_eq!(
            ReceiverTransition::apply(
                Some(&revoked.after),
                ReceiverIntent::Revoke,
                actor.clone(),
                "again".into(),
                3
            ),
            Err(ReceiverTransitionError::Conflict)
        );
        assert_eq!(
            ReceiverTransition::apply(
                Some(&binding()),
                ReceiverIntent::Regrant(CredentialId::new("second").unwrap()),
                actor.clone(),
                "wrong-state".into(),
                3
            ),
            Err(ReceiverTransitionError::Conflict)
        );
        assert_eq!(
            ReceiverTransition::apply(
                Some(&revoked.after),
                ReceiverIntent::Regrant(revoked.after.credential_id.clone()),
                actor.clone(),
                "same".into(),
                3
            ),
            Err(ReceiverTransitionError::Conflict)
        );
        let regranted = ReceiverTransition::apply(
            Some(&revoked.after),
            ReceiverIntent::Regrant(CredentialId::new("second").unwrap()),
            actor.clone(),
            "regrant".into(),
            3,
        )
        .unwrap();
        assert_eq!(regranted.after.access_epoch, 3);
        let policy_active = ReceiverTransition::apply(
            Some(&binding()),
            ReceiverIntent::PolicyChanged,
            ReceiverInitiator::System,
            "policy-a".into(),
            4,
        )
        .unwrap();
        let policy_revoked = ReceiverTransition::apply(
            Some(&revoked.after),
            ReceiverIntent::PolicyChanged,
            ReceiverInitiator::System,
            "policy-r".into(),
            4,
        )
        .unwrap();
        assert!(policy_active.after.active);
        assert!(!policy_revoked.after.active);
        assert_eq!(
            ReceiverTransition::apply(
                Some(&binding()),
                ReceiverIntent::PolicyChanged,
                actor.clone(),
                "wrong-actor".into(),
                4
            ),
            Err(ReceiverTransitionError::Conflict)
        );
        let tampered = ReceiverTransition {
            after: ReceiverBinding {
                active: false,
                ..policy_active.after.clone()
            },
            ..policy_active
        };
        assert_eq!(
            tampered.verify(Some(&binding())),
            Err(ReceiverTransitionError::Conflict)
        );
        let mut exhausted = binding();
        exhausted.access_epoch = i64::MAX as u64;
        assert_eq!(
            ReceiverTransition::apply(
                Some(&exhausted),
                ReceiverIntent::Revoke,
                actor,
                "exhausted".into(),
                5
            ),
            Err(ReceiverTransitionError::Exhausted)
        );
    }

    /// Device pairing cleanup (design row P39): the system fences an active
    /// receiver, and the fence replays; an inactive one cannot be fenced again.
    #[test]
    fn the_system_fences_only_an_active_receiver() {
        let fenced = ReceiverTransition::apply(
            Some(&binding()),
            ReceiverIntent::Revoke,
            ReceiverInitiator::System,
            "fence".into(),
            2,
        )
        .unwrap();
        assert!(!fenced.after.active);
        assert_eq!(fenced.after.access_epoch, 2);
        assert_eq!(fenced.verify(Some(&binding())), Ok(()));
        assert_eq!(
            ReceiverTransition::apply(
                Some(&fenced.after),
                ReceiverIntent::Revoke,
                ReceiverInitiator::System,
                "again".into(),
                3
            ),
            Err(ReceiverTransitionError::Conflict)
        );
        // Pairing and regranting remain a principal's.
        assert_eq!(
            ReceiverTransition::apply(
                Some(&fenced.after),
                ReceiverIntent::Regrant(CredentialId::new("second").unwrap()),
                ReceiverInitiator::System,
                "regrant".into(),
                3
            ),
            Err(ReceiverTransitionError::Conflict)
        );
    }

    /// Rows P41, P42, P46: the pairing rule admits only the exact active
    /// binding with no revocation since; the fence refuses anything else.
    #[test]
    fn a_paired_receiver_admits_and_fences_only_its_exact_binding() {
        let paired = PairedReceiver {
            receiver_id: "receiver".into(),
            credential_id: binding().credential_id,
            organization_id: binding().organization_id,
            owner_id: binding().owner_id,
            paired_epoch: 1,
        };
        assert!(paired.admits(&binding(), false));
        assert!(!paired.admits(&binding(), true), "revoked since the pair");
        for other in [
            ReceiverBinding {
                receiver_id: "other".into(),
                ..binding()
            },
            ReceiverBinding {
                credential_id: CredentialId::new("second").unwrap(),
                ..binding()
            },
            ReceiverBinding {
                organization_id: OrganizationId::new("org-2").unwrap(),
                ..binding()
            },
            ReceiverBinding {
                owner_id: PrincipalId::new("someone").unwrap(),
                ..binding()
            },
            ReceiverBinding {
                active: false,
                ..binding()
            },
        ] {
            assert!(!paired.admits(&other, false), "{other:?}");
            assert_eq!(
                paired.fence(&other, false, "fence".into(), 2),
                Err(ReceiverTransitionError::Conflict)
            );
        }
        let older = PairedReceiver {
            paired_epoch: 2,
            ..paired.clone()
        };
        assert!(
            !older.admits(&binding(), false),
            "an epoch older than the pair"
        );
        assert_eq!(
            paired.fence(&binding(), true, "fence".into(), 2),
            Err(ReceiverTransitionError::Conflict)
        );
        let fenced = paired.fence(&binding(), false, "fence".into(), 2).unwrap();
        assert_eq!(fenced.initiator, ReceiverInitiator::System);
        assert!(!fenced.after.active);
        assert!(paired.revoked_by(&fenced.cause, fenced.before.as_ref()));
        let policy = ReceiverTransition::apply(
            Some(&binding()),
            ReceiverIntent::PolicyChanged,
            ReceiverInitiator::System,
            "policy".into(),
            2,
        )
        .unwrap();
        assert!(!paired.revoked_by(&policy.cause, policy.before.as_ref()));
    }
}
