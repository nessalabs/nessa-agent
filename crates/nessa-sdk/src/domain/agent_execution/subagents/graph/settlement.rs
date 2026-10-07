//! Exact observation debt, first-close scope and explicit aggregate completion.
use super::*;

/// Explicit root-only aggregate decision, separate from observation authority.
#[derive(Clone, Debug)]
#[must_use = "audit the exact completion decision before acknowledging it"]
pub struct CloseCompletion {
    record: OwnershipEvidence,
}
impl CloseCompletion {
    /// Exact immutable decision for the mandatory coordinator audit.
    pub fn evidence(&self) -> &OwnershipEvidence { &self.record }
}

impl OwnershipGraph {
    /// First direct close owner, derived from validated cascade ancestry.
    pub fn close_owner(&self, target: &AgentLifetimeId) -> Option<AgentLifetimeId> {
        let mut current = target.clone();
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(current.clone()) || self.close_operation(&current).is_none() { return None; }
            match self.cascaded_from(&current) {
                Some(parent) if self.close_operation(parent) == self.close_operation(&current)
                    && self.close_cause(parent) == self.close_cause(&current)
                    && self.close_initiator(parent) == self.close_initiator(&current)
                    && self.parent_id(&current).as_ref() == Some(parent) => current = parent.clone(),
                Some(_) => return None,
                None => return Some(current),
            }
        }
    }

    /// Physical work owned by this first close. Independent descendants are delegated.
    pub fn physical_targets(&self, root: &AgentLifetimeId) -> Vec<AgentLifetimeId> {
        self.unsettled_targets(root)
    }

    /// Independently closing descendants whose own Completion this root must join.
    pub fn independent_closes(&self, root: &AgentLifetimeId) -> Vec<AgentLifetimeId> {
        self.lifetimes.keys().filter(|id| *id != root && self.in_subtree(root, id)
            && self.lifetime_state(id) == Some(LifetimeState::Closing)
            && self.close_owner(id).as_ref() == Some(*id)).cloned().collect()
    }

    /// Whether durable live absence excludes new binding/gate transfer for this target.
    pub fn has_absence(&self, target: &AgentLifetimeId) -> bool {
        self.settlements.values().any(|s| &s.row.target == target && matches!(s.row.proof, SettlementProof::Absence(_)))
    }

    /// Record actual live absence with exact request/close correlation.
    /// The application must establish the selected external promise under its admission scope.
    ///
    /// # Errors
    /// Rejects a foreign request, mismatched owner/operation, existing resource fact or refused graph.
    pub fn note_absence(&mut self, root: &AgentLifetimeId, operation: &CloseOperationId,
        target: &AgentLifetimeId, proof: AbsenceProof) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        self.validate_target(root, operation, target)?;
        if !self.valid_absence(target, &proof) { return Err(OwnershipError::StaleOutcome); }
        let key = (root.clone(), target.clone());
        if let Some(existing) = self.settlements.get(&key) {
            return match &existing.row.proof {
                SettlementProof::Absence(absence) if absence.proof == proof => Ok(absence.record.clone()),
                _ => Err(OwnershipError::StaleOutcome),
            };
        }
        let record = self.observation_record(root, operation, target, CloseEvidenceDetail::Absence(proof.clone()));
        self.settlements.insert(key, Settlement { row: SettlementRow {
            close_lifetime: root.clone(), target: target.clone(), physical: PhysicalFact::Released,
            evidence: EvidenceFact::Pending,
            proof: SettlementProof::Absence(AbsenceAudit { proof, record: record.clone(), acknowledgement: EvidenceFact::Pending }),
        }});
        Ok(record)
    }

    /// Exact outstanding observation records, including physically Released targets.
    pub fn pending_close_evidence(&self, root: &AgentLifetimeId) -> Vec<OwnershipEvidence> {
        self.settlements.values().filter(|s| &s.row.close_lifetime == root).flat_map(|s| {
            match &s.row.proof {
                SettlementProof::Absence(a) => if a.acknowledgement != EvidenceFact::Acknowledged { vec![a.record.clone()] } else { Vec::new() },
                SettlementProof::Resource(slots) => slots.iter().flatten().filter(|a| a.acknowledgement != EvidenceFact::Acknowledged)
                    .map(|a| a.record.clone()).collect(),
            }
        }).collect()
    }

    /// Apply actual coordinator acknowledgement to the exact observation, never Completion.
    ///
    /// # Errors
    /// A mismatched record/stage/current operation returns StaleOutcome.
    pub fn acknowledge_observation(&mut self, record: &OwnershipEvidence, outcome: EvidenceFact) -> Result<(), OwnershipError> {
        self.ensure_dispatch()?;
        let target = record.child_lifetime.as_ref().ok_or(OwnershipError::StaleOutcome)?;
        let operation = record.close_operation.as_ref().ok_or(OwnershipError::StaleOutcome)?;
        self.validate_target(&record.parent_lifetime, operation, target)?;
        let row = &mut self.settlements.get_mut(&(record.parent_lifetime.clone(), target.clone()))
            .ok_or(OwnershipError::StaleOutcome)?.row;
        let acknowledgement = match &mut row.proof {
            SettlementProof::Absence(a) if &a.record == record => &mut a.acknowledgement,
            SettlementProof::Resource(slots) => &mut slots.iter_mut().flatten().find(|a| &a.record == record)
                .ok_or(OwnershipError::StaleOutcome)?.acknowledgement,
            _ => return Err(OwnershipError::StaleOutcome),
        };
        if *acknowledgement != EvidenceFact::Acknowledged { *acknowledgement = outcome; }
        refresh_summary(row);
        Ok(())
    }

    /// Prepare explicit Completion after all owned and independent targets are ready.
    /// The application must first acknowledge a Closing safety snapshot through its writer.
    ///
    /// # Errors
    /// Returns StaleOutcome for unresolved proof, debt, independent child or operation.
    pub fn prepare_completion(&mut self, root: &AgentLifetimeId, operation: &CloseOperationId) -> Result<CloseCompletion, OwnershipError> {
        self.ensure_dispatch()?;
        if self.close_owner(root).as_ref() != Some(root) || self.close_operation(root) != Some(operation)
            || !self.completion_ready(root) { return Err(OwnershipError::StaleOutcome); }
        let record = self.observation_record(root, operation, root, CloseEvidenceDetail::Completion);
        let fact = self.close_completions.entry(root.clone()).or_insert(CloseCompletionRow {
            close_lifetime: root.clone(), record, acknowledgement: EvidenceFact::Pending,
        });
        Ok(CloseCompletion { record: fact.record.clone() })
    }

    /// Apply the actual audit result for the explicit aggregate token.
    /// Only acknowledged Completion changes this first operation's owned lifetimes to Closed.
    ///
    /// # Errors
    /// Rejects a stale decision or no longer ready target set.
    pub fn acknowledge_completion(&mut self, token: &CloseCompletion, outcome: EvidenceFact) -> Result<(), OwnershipError> {
        self.ensure_dispatch()?;
        let root = &token.record.parent_lifetime;
        if !self.completion_ready(root) { return Err(OwnershipError::StaleOutcome); }
        let fact = self.close_completions.get_mut(root).ok_or(OwnershipError::StaleOutcome)?;
        if fact.record != token.record { return Err(OwnershipError::StaleOutcome); }
        if fact.acknowledgement != EvidenceFact::Acknowledged { fact.acknowledgement = outcome; }
        if fact.acknowledgement == EvidenceFact::Acknowledged {
            let owned: Vec<_> = self.lifetimes.keys().filter(|id| self.close_owner(id).as_ref() == Some(root)).cloned().collect();
            for id in owned { self.lifetimes.get_mut(&id).expect("owned lifetime").row.state = LifetimeState::Closed; }
        }
        Ok(())
    }

    pub(super) fn validate_target(&self, root: &AgentLifetimeId, operation: &CloseOperationId, target: &AgentLifetimeId) -> Result<(), OwnershipError> {
        if self.lifetime_state(root) == Some(LifetimeState::Open) || self.close_operation(root) != Some(operation)
            || self.close_owner(target).as_ref() != Some(root) || !self.in_subtree(root, target) {
            return Err(OwnershipError::StaleOutcome);
        }
        Ok(())
    }

    pub(super) fn observation_record(&self, root: &AgentLifetimeId, operation: &CloseOperationId, target: &AgentLifetimeId,
        detail: CloseEvidenceDetail) -> OwnershipEvidence {
        OwnershipEvidence { close_detail: Some(detail.clone()), parent_lifetime: root.clone(), child_lifetime: Some(target.clone()),
            close_operation: Some(operation.clone()), before: OwnershipMeaning::Closing,
            after: if detail == CloseEvidenceDetail::Completion { OwnershipMeaning::Closed } else { OwnershipMeaning::Closing },
            cause: self.close_cause(root).cloned(), initiator: self.close_initiator(root).cloned().unwrap_or(Initiator::Runtime) }
    }

    fn valid_absence(&self, target: &AgentLifetimeId, proof: &AbsenceProof) -> bool {
        match proof {
            AbsenceProof::NeverTransferredRoot => !self.child_request.contains_key(target),
            AbsenceProof::PreparationRejectedWithoutOwner(request) | AbsenceProof::AdmissionFailedBeforeFactory(request) =>
                self.child_lifetime(request) == Some(target) && self.spawn_progress(request).is_some_and(|p| p.known() == KnownMilestone::Reserved),
        }
    }

    pub(super) fn valid_record(&self, root: &AgentLifetimeId, target: &AgentLifetimeId, record: &OwnershipEvidence) -> bool {
        record.close_operation.as_ref().is_some_and(|op| self.validate_target(root, op, target).is_ok())
            && &record.parent_lifetime == root && record.child_lifetime.as_ref() == Some(target)
            && record.cause.as_ref() == self.close_cause(root) && Some(&record.initiator) == self.close_initiator(root)
            && record.before == OwnershipMeaning::Closing
            && record.after == if record.close_detail == Some(CloseEvidenceDetail::Completion) { OwnershipMeaning::Closed } else { OwnershipMeaning::Closing }
    }

    pub(super) fn valid_settlement(&self, row: &SettlementRow) -> bool {
        let valid = match &row.proof {
            SettlementProof::Absence(a) => self.valid_absence(&row.target, &a.proof)
                && a.record.close_detail == Some(CloseEvidenceDetail::Absence(a.proof.clone()))
                && self.valid_record(&row.close_lifetime, &row.target, &a.record),
            SettlementProof::Resource(slots) => slots.iter().any(Option::is_some) && slots.iter().enumerate().all(|(slot, a)| a.as_ref().is_none_or(|a| {
                matches!(a.record.close_detail, Some(CloseEvidenceDetail::ResourceObservation { physical, .. }) if physical_slot(physical) == slot)
                    && self.valid_record(&row.close_lifetime, &row.target, &a.record)
            })),
        };
        let mut expected = row.clone(); refresh_summary(&mut expected);
        valid && expected.physical == row.physical && expected.evidence == row.evidence
    }

    pub(super) fn completion_ready(&self, root: &AgentLifetimeId) -> bool {
        self.lifetimes.keys().filter(|id| self.in_subtree(root, id)).all(|id| {
            let Some(owner) = self.close_owner(id) else { return false; };
            if &owner != root { return self.lifetime_state(id) == Some(LifetimeState::Closed); }
            self.settlements.get(&(root.clone(), id.clone())).is_some_and(|s| match &s.row.proof {
                SettlementProof::Absence(a) => a.acknowledgement == EvidenceFact::Acknowledged,
                SettlementProof::Resource(slots) => slots[2].as_ref().is_some_and(|a| a.provider_acknowledged)
                    && slots.iter().flatten().all(|a| a.acknowledgement == EvidenceFact::Acknowledged),
            })
        })
    }
}
