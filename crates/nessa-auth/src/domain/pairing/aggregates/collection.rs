//! Canonical live invitation and Available-slot admission across gateway resources.
use super::super::PairingError;
use super::invitation::{PairingPhase, PairingRecord};
use std::collections::HashSet;
/// Maximum unfinished enrollments; Active and Terminal records are retained receipts.
pub const MAX_LIVE_PAIRINGS: usize = 32;
/// Validate an already bounded registry collection using borrowed gateway identities.
/// Creation and restoration ask this owner. An Available slot remains occupied
/// through charged attempts until canonical claim or terminal state. No clock or
/// password information is consulted and no expired record is implicitly replaced.
pub fn validate_pairing_collection(records: &[PairingRecord]) -> Result<(), PairingError> {
    let mut invitations = HashSet::new();
    let mut available = HashSet::new();
    let mut live = 0;
    for record in records {
        if !invitations.insert(record.id()) {
            return Err(PairingError::Conflict);
        }
        if record.phase() == PairingPhase::Available {
            let resource = record.intent().resource();
            if !available.insert((resource.organization_id(), resource.id())) {
                return Err(PairingError::AvailableSlotOccupied);
            }
        }
        if !matches!(
            record.phase(),
            PairingPhase::Active | PairingPhase::Terminal
        ) {
            live += 1;
            if live > MAX_LIVE_PAIRINGS {
                return Err(PairingError::Capacity);
            }
        }
    }
    Ok(())
}
