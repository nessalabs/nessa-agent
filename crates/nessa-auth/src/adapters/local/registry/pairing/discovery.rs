//! The discovery field on a peer pairing record. It is not a pairing phase
//! and it does not enroll anyone.
use super::super::{LocalCredentialStore, LocalStoreError, Registry};
use crate::domain::pairing::{ConsentClass, DeviceKey, InvitationId, PairingPhase, PublicIntent};
use crate::domain::CredentialId;
/// Largest discovery JSON stored on one pairing record.
const DISCOVERY_JSON_BYTES: usize = 2048;

/// One paired gateway as the granting side knows it, for the announcement.
#[derive(Clone, Debug)]
pub struct GrantorPeer {
    pub key: DeviceKey,
    pub invitation: InvitationId,
    pub intent: PublicIntent,
    pub credential: Option<CredentialId>,
    /// `Active` only. Terminal and earlier phases are false. This read does
    /// not change the phase.
    pub phase_active: bool,
    /// The issued credential is unrevoked and unexpired at `now`.
    pub credential_current: bool,
    /// The discovery JSON, or `None` when the record has no discovery key.
    pub discovery_json: Option<String>,
}

impl LocalCredentialStore {
    /// Peer enrollments and their discovery field. A credential that is not
    /// current leaves `phase_active` as the record has it.
    pub fn peer_discoveries(&self, now_seconds: u64) -> Result<Vec<GrantorPeer>, LocalStoreError> {
        let slot = self.registry()?;
        let registry = slot.as_ref().ok_or(LocalStoreError::NotInitialized)?;
        peers(registry, now_seconds)
    }

    /// Read credential facts only after exact key, enrollment and issued ID match.
    /// Historical records may prove noncurrent; mismatched records prove nothing.
    pub fn peer_discovery(
        &self,
        key: &DeviceKey,
        intent: &PublicIntent,
        credential: &CredentialId,
        now_seconds: u64,
    ) -> Result<Option<GrantorPeer>, LocalStoreError> {
        Ok(self
            .peer_discoveries(now_seconds)?
            .into_iter()
            .rev()
            .find(|row| {
                row.key == *key
                    && row.intent == *intent
                    && row.credential.as_ref() == Some(credential)
            }))
    }

    /// Write the discovery JSON onto the Active peer pairing for `key`.
    /// The phase is not written. `None` clears the field back to absent.
    pub fn write_peer_discovery(
        &self,
        invitation: &InvitationId,
        key: &DeviceKey,
        now_seconds: u64,
        expected_discovery: Option<&str>,
        discovery_json: Option<&str>,
    ) -> Result<(), LocalStoreError> {
        if discovery_json.is_some_and(|json| json.len() > DISCOVERY_JSON_BYTES) {
            return Err(LocalStoreError::Capacity);
        }
        let mut slot = self.mutation_registry()?;
        let current = slot.as_ref().ok_or(LocalStoreError::NotInitialized)?;
        let index = peer_index(current, invitation, key)?.ok_or(LocalStoreError::NotFound)?;
        let record = current.pairings[index]
            .restore(&current.transitions)
            .map_err(|_| LocalStoreError::Corrupt)?;
        if current.pairings[index].discovery_json() != expected_discovery
            || record.phase() != PairingPhase::Active
            || !credential_current(
                current,
                record.credential().map(|id| id.as_str()),
                now_seconds,
            )
        {
            return Err(LocalStoreError::Conflict);
        }
        let mut next = current.clone();
        next.pairings[index].set_discovery_json(discovery_json.map(str::to_owned));
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(LocalStoreError::Capacity)?;
        self.persist(&next)?;
        let revision = next.revision;
        *slot = Some(next.clone());
        self.publish_snapshot(next)?;
        self.publish_revision(revision);
        Ok(())
    }
}

fn peers(registry: &Registry, now_seconds: u64) -> Result<Vec<GrantorPeer>, LocalStoreError> {
    let mut found = Vec::new();
    for stored in &registry.pairings {
        if stored.class() != ConsentClass::PeerRead {
            continue;
        }
        let record = stored
            .restore(&registry.transitions)
            .map_err(|_| LocalStoreError::Corrupt)?;
        let Some((attempt, key)) = record.claim_binding() else {
            continue;
        };
        found.push(GrantorPeer {
            key,
            invitation: record.id(),
            intent: PublicIntent::from_record(&record, attempt)
                .map_err(|_| LocalStoreError::Corrupt)?,
            credential: record.credential().cloned(),
            phase_active: record.phase() == PairingPhase::Active,
            credential_current: credential_current(
                registry,
                record.credential().map(|id| id.as_str()),
                now_seconds,
            ),
            discovery_json: stored.discovery_json().map(str::to_owned),
        });
    }
    Ok(found)
}

fn peer_index(
    registry: &Registry,
    invitation: &InvitationId,
    key: &DeviceKey,
) -> Result<Option<usize>, LocalStoreError> {
    for (index, stored) in registry.pairings.iter().enumerate() {
        if stored.class() != ConsentClass::PeerRead {
            continue;
        }
        let record = stored
            .restore(&registry.transitions)
            .map_err(|_| LocalStoreError::Corrupt)?;
        if record.id() == *invitation
            && record
                .claim_binding()
                .is_some_and(|(_, claimed)| claimed == *key)
        {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn credential_current(registry: &Registry, id: Option<&str>, now_seconds: u64) -> bool {
    let Some(id) = id else {
        return false;
    };
    let Some(credential) = registry
        .credentials
        .iter()
        .find(|credential| credential.metadata.id == id)
    else {
        return false;
    };
    credential.metadata.revoked_at.is_none()
        && credential.metadata.issued_at <= now_seconds
        && credential
            .metadata
            .expires_at
            .is_none_or(|expiry| now_seconds < expiry)
}
