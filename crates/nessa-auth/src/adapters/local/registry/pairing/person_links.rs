//! Bridge the registry's one serialized history to the grant owner's effect.
use super::super::{
    domain_credential, principal_kind, revocation_of, LocalCredentialStore, LocalStoreError,
    Registry,
};
use crate::application::{
    pairing::{PairingLinkEvidence, PairingLinkSource, PairingPersonLinks, PersonLinkError},
    revocation::CredentialRevocation,
};
use crate::domain::{
    pairing::{ConsentIntent, PairingError, PairingRecord},
    Principal, PrincipalId,
};
use std::sync::Arc;

pub(in crate::adapters::local::registry) fn owner_principal(
    registry: &Registry,
    intent: &ConsentIntent,
) -> Result<Option<Principal>, PairingError> {
    let owner = if intent.same_person() {
        registry
            .principals
            .iter()
            .find(|principal| principal.id == intent.owner().as_str())
            .map(|principal| {
                Ok(Principal::new(
                    PrincipalId::new(&principal.id).map_err(|_| PairingError::Invalid)?,
                    principal_kind(principal).map_err(|_| PairingError::Invalid)?,
                ))
            })
            .transpose()?
    } else {
        None
    };
    intent.validate_same_person_owner(owner.as_ref())?;
    Ok(owner)
}

pub(super) fn evidence(registry: &Registry) -> Result<Vec<PairingLinkEvidence>, PersonLinkError> {
    let mut facts = Vec::new();
    for stored in &registry.pairings {
        let record = stored
            .restore(&registry.transitions)
            .map_err(|_| PersonLinkError::Conflict)?;
        if !record.intent().same_person() {
            continue;
        }
        let Some(id) = record.credential() else {
            continue;
        };
        // Staging has a reserved id, but no published credential yet.
        let Some(stored) = registry
            .credentials
            .iter()
            .find(|entry| entry.metadata.id == id.as_str())
        else {
            continue;
        };
        let credential =
            domain_credential(&stored.metadata).map_err(|_| PersonLinkError::Conflict)?;
        let revocation = revocation_of(registry, id.as_str())
            .map(|record| CredentialRevocation::restore(record, &credential))
            .transpose()
            .map_err(|_| PersonLinkError::Conflict)?;
        let owner = owner_principal(registry, record.intent())
            .map_err(|_| PersonLinkError::Conflict)?
            .ok_or(PersonLinkError::Conflict)?;
        facts.push(PairingLinkEvidence {
            record,
            owner,
            credential,
            revocation,
        });
    }
    Ok(facts)
}

impl PairingLinkSource for LocalCredentialStore {
    fn with_current_links(
        &self,
        effect: &mut dyn FnMut(&[PairingLinkEvidence]) -> Result<(), PersonLinkError>,
    ) -> Result<(), PersonLinkError> {
        let slot = self.registry().map_err(|_| PersonLinkError::Unavailable)?;
        let registry = slot.as_ref().ok_or(PersonLinkError::Unavailable)?;
        effect(&evidence(registry)?)
    }
}

impl LocalCredentialStore {
    /// Bind the existing grant owner once at composition and reconcile before
    /// serving. A failed effect remains owed by durable credential history.
    pub fn bind_person_links(
        &self,
        links: Arc<dyn PairingPersonLinks>,
    ) -> Result<(), PersonLinkError> {
        let slot = self.registry().map_err(|_| PersonLinkError::Unavailable)?;
        let current = slot.as_ref().ok_or(PersonLinkError::Unavailable)?;
        let mut bound = self
            .person_links
            .write()
            .map_err(|_| PersonLinkError::Unavailable)?;
        if bound.is_some() {
            return Err(PersonLinkError::Conflict);
        }
        *bound = Some(links.clone());
        links.reconcile(&evidence(current)?)
    }

    pub(in crate::adapters::local::registry) fn reconcile_person_links(
        &self,
        registry: &Registry,
    ) -> Result<(), LocalStoreError> {
        let links = self
            .person_links
            .read()
            .map_err(|_| LocalStoreError::Corrupt)?;
        if let Some(links) = links.as_ref() {
            links
                .reconcile(&evidence(registry).map_err(|_| LocalStoreError::Corrupt)?)
                .map_err(|_| LocalStoreError::PersonLinksUnavailable)?;
        }
        Ok(())
    }

    pub(super) fn require_person_link_absent(
        &self,
        registry: &Registry,
        record: &PairingRecord,
    ) -> Result<(), PersonLinkError> {
        if !record.intent().class().is_peer() {
            return Ok(());
        }
        let links = self
            .person_links
            .read()
            .map_err(|_| PersonLinkError::Unavailable)?;
        match links.as_ref() {
            Some(links) => {
                links.reconcile(&evidence(registry)?)?;
                links.require_absent(record)
            }
            None if record.intent().same_person() => Err(PersonLinkError::Unavailable),
            None => Ok(()),
        }
    }
}
