//! Parent and child lifetimes, spawn reservations, and close settlement.
//! The aggregate decides legal transitions. It does not call storage, a clock, or a provider.
#![deny(missing_docs)]

mod settlement;
pub use settlement::CloseCompletion;

use std::collections::{BTreeMap, BTreeSet};

use super::error::OwnershipError;
use super::values::{
    AbsenceAudit, AbsenceProof, AgentLifetimeId, CloseCompletionRow, CloseEvidenceDetail,
    CloseOperationId, DeliveryState, EvidenceFact, Initiator, KnownMilestone, LifetimeCause,
    LifetimeRow, LifetimeState, OwnershipEvidence, OwnershipMeaning, OwnershipSnapshot,
    PhysicalFact, ReportId, ReportRow, ResourceObservationAudit, SettlementProof, SettlementRow,
    SpawnBinding, SpawnProgress, SpawnRequestId, SpawnRow, MAX_DEPTH, MAX_DIRECT_CHILDREN,
    MAX_READ_PAGE, MAX_RETAINED_REQUESTS,
};
use crate::domain::agent_execution::sessions::SessionId;

#[derive(Clone, Debug)]
struct Lifetime {
    row: LifetimeRow,
}

#[derive(Clone, Debug)]
struct Spawn {
    row: SpawnRow,
}

#[derive(Clone, Debug)]
struct Settlement {
    row: SettlementRow,
}

/// Relationship graph for one ownership store.
/// Dispatch methods refuse when restoration found a contradiction.
#[derive(Clone, Debug)]
pub struct OwnershipGraph {
    lifetimes: BTreeMap<AgentLifetimeId, Lifetime>,
    spawns: BTreeMap<SpawnRequestId, Spawn>,
    child_request: BTreeMap<AgentLifetimeId, SpawnRequestId>,
    settlements: BTreeMap<(AgentLifetimeId, AgentLifetimeId), Settlement>,
    close_completions: BTreeMap<AgentLifetimeId, CloseCompletionRow>,
    reports: BTreeMap<ReportId, ReportRow>,
    refusal: Option<OwnershipError>,
    /// Open descendants joined to a closing ancestor while reloading.
    recovery: Vec<OwnershipEvidence>,
}

/// A spawn the caller wants admitted.
#[derive(Clone, Debug)]
pub struct SpawnAdmission {
    /// Child lifetime minted for this attempt.
    pub child_lifetime: AgentLifetimeId,
    /// Child session minted for this attempt.
    pub child_session: SessionId,
    /// Immutable binding, including the parent and the request id.
    pub binding: SpawnBinding,
    /// Whether the live-conversation owner still has a slot.
    pub live_room: bool,
}

/// What `begin_close` decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseAdmission {
    /// Evidence for the audit port. A join keeps the original cause.
    pub evidence: OwnershipEvidence,
    /// True when this call joined a close that had already started or finished.
    pub joined: bool,
    /// Targets that still need cleanup. Empty when the lifetime is already closed.
    pub targets: Vec<AgentLifetimeId>,
}

/// Correlated physical absence awaiting its real ownership-audit outcome.
/// Produced only for a root; application supplies proof it was never bound.
#[derive(Debug)]
#[must_use = "record the absence evidence and apply its real audit outcome"]
pub struct UnboundRootSettlement {
    root: AgentLifetimeId,
    operation: CloseOperationId,
    evidence: OwnershipEvidence,
}
impl UnboundRootSettlement {
    /// Audit record for confirmed physical absence while the root stays Closing.
    pub fn evidence(&self) -> &OwnershipEvidence {
        &self.evidence
    }
}

/// Whether a prepared child may be dispatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispatch {
    /// The parent and the child are still open.
    Allowed,
    /// The parent is sealing. Do not dispatch.
    Drain,
    /// The child lifetime is closing or closed. Do not dispatch, and do not
    /// report that refusal as the parent closing.
    ChildUnavailable,
}

/// One child on a read page, oldest identity first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildView {
    /// Child lifetime.
    pub lifetime: AgentLifetimeId,
    /// Child session.
    pub session: SessionId,
    /// Parent lifetime.
    pub parent: AgentLifetimeId,
    /// Child lifetime state.
    pub state: LifetimeState,
    /// Spawn progress.
    pub progress: SpawnProgress,
    /// Policy copied at admission.
    pub policy: super::values::ApprovalPolicy,
}

/// A page of children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildPage {
    /// Children after the cursor, in lifetime-id order.
    pub children: Vec<ChildView>,
    /// Lifetime id to pass as the next cursor, when another page remains.
    pub next: Option<AgentLifetimeId>,
}

impl OwnershipGraph {
    /// An empty graph that may admit a root.
    pub fn new() -> Self {
        Self {
            lifetimes: BTreeMap::new(),
            spawns: BTreeMap::new(),
            child_request: BTreeMap::new(),
            settlements: BTreeMap::new(),
            close_completions: BTreeMap::new(),
            reports: BTreeMap::new(),
            refusal: None,
            recovery: Vec::new(),
        }
    }

    /// Why dispatch is refused, when restoration found illegal history.
    pub fn refusal(&self) -> Option<&OwnershipError> {
        self.refusal.as_ref()
    }

    /// Close intents for open descendants joined to a closing ancestor during reload.
    /// Empty when the snapshot was already consistent, or when dispatch is refused.
    pub fn recovery_records(&self) -> &[OwnershipEvidence] {
        &self.recovery
    }

    /// Session bound to this lifetime, when the id is present.
    pub fn session_id(&self, id: &AgentLifetimeId) -> Option<&SessionId> {
        self.lifetimes
            .get(id)
            .map(|lifetime| &lifetime.row.session_id)
    }

    /// Lifetime state, when this id is present.
    pub fn lifetime_state(&self, id: &AgentLifetimeId) -> Option<LifetimeState> {
        self.lifetimes.get(id).map(|lifetime| lifetime.row.state)
    }

    /// First close cause, when sealing has started.
    pub fn close_cause(&self, id: &AgentLifetimeId) -> Option<&LifetimeCause> {
        self.lifetimes
            .get(id)
            .and_then(|lifetime| lifetime.row.cause.as_ref())
    }

    /// First close initiator, when sealing has started.
    pub fn close_initiator(&self, id: &AgentLifetimeId) -> Option<&Initiator> {
        self.lifetimes
            .get(id)
            .and_then(|lifetime| lifetime.row.initiator.as_ref())
    }

    /// Close operation retained for this lifetime, when sealing has started.
    pub fn close_operation(&self, id: &AgentLifetimeId) -> Option<&CloseOperationId> {
        self.lifetimes
            .get(id)
            .and_then(|lifetime| lifetime.row.close_operation.as_ref())
    }

    /// Policy copied onto a spawn, when the request is present.
    pub fn spawn_policy(&self, id: &SpawnRequestId) -> Option<&super::values::ApprovalPolicy> {
        self.spawns.get(id).map(|spawn| &spawn.row.binding.policy)
    }

    /// Ancestor that cascaded this close, when the close was not direct.
    pub fn cascaded_from(&self, id: &AgentLifetimeId) -> Option<&AgentLifetimeId> {
        self.lifetimes
            .get(id)
            .and_then(|lifetime| lifetime.row.cascaded_from.as_ref())
    }

    /// Spawn progress for a request id.
    pub fn spawn_progress(&self, id: &SpawnRequestId) -> Option<&SpawnProgress> {
        self.spawns.get(id).map(|spawn| &spawn.row.progress)
    }

    /// Child lifetime reserved for a request id.
    pub fn child_lifetime(&self, id: &SpawnRequestId) -> Option<&AgentLifetimeId> {
        self.spawns.get(id).map(|spawn| &spawn.row.child_lifetime)
    }

    /// Derive receipt retention for a sealed child whose permission publication is pending.
    /// `eligible` is the application's last acknowledged spawn progress. A
    /// provider-acknowledged task receipt remains factual under Closing/Closed,
    /// represented as Unconfirmed rather than a new affirmative permission.
    /// Existing terminal safety progress and lifetime state are not changed.
    /// Returns `None` when ordinary eligibility selection is sufficient.
    pub fn sealed_spawn_progress(
        &self,
        request: &SpawnRequestId,
        eligible: &SpawnProgress,
    ) -> Option<SpawnProgress> {
        let spawn = self.spawns.get(request)?;
        if self.lifetime_state(&spawn.row.child_lifetime) == Some(LifetimeState::Open) {
            return None;
        }
        let known = spawn.row.progress.known();
        if !matches!(known, KnownMilestone::TaskAdmitted { .. }) || known == eligible.known() {
            return None;
        }
        Some(match &spawn.row.progress {
            SpawnProgress::Ended { .. }
            | SpawnProgress::Draining { .. }
            | SpawnProgress::StartupFailed { .. }
            | SpawnProgress::Unconfirmed { .. } => spawn.row.progress.clone(),
            _ => SpawnProgress::Unconfirmed { known },
        })
    }

    /// Report position.
    pub fn report_state(&self, id: &ReportId) -> Option<DeliveryState> {
        self.reports.get(id).map(|report| report.state)
    }

    /// Physical fact recorded for a target of a close.
    pub fn physical(
        &self,
        close_lifetime: &AgentLifetimeId,
        target: &AgentLifetimeId,
    ) -> Option<PhysicalFact> {
        self.settlements
            .get(&(close_lifetime.clone(), target.clone()))
            .map(|settlement| settlement.row.physical)
    }

    /// Find an active root for a session, excluding child identities and closed history.
    /// This read does not grant dispatch or change the refusal of `open_root`.
    pub fn root_lifetime_for_session(&self, session: &SessionId) -> Option<&AgentLifetimeId> {
        self.lifetimes
            .iter()
            .find(|(id, lifetime)| {
                &lifetime.row.session_id == session
                    && lifetime.row.state != LifetimeState::Closed
                    && !self.child_request.contains_key(*id)
            })
            .map(|(id, _)| id)
    }

    /// Remove one untransferred private root without restoring neighboring history.
    /// The application must establish that this identity never became audit eligible
    /// and never transferred resources. Descendants or close facts refuse deletion.
    ///
    /// # Errors
    /// Returns `StaleOutcome` for a child, non-open root, or root with dependents.
    pub fn discard_private_root(&mut self, id: &AgentLifetimeId) -> Result<(), OwnershipError> {
        if self.child_request.contains_key(id)
            || self.lifetime_state(id) != Some(LifetimeState::Open)
            || self
                .spawns
                .values()
                .any(|s| &s.row.binding.parent_lifetime == id)
            || self
                .settlements
                .keys()
                .any(|(root, target)| root == id || target == id)
            || self
                .reports
                .values()
                .any(|r| &r.parent_lifetime == id || &r.child_lifetime == id)
        {
            return Err(OwnershipError::StaleOutcome);
        }
        self.lifetimes.remove(id);
        Ok(())
    }

    /// Record physical absence on a never-bound root without acknowledging audit.
    /// Application owns proof that no resource transfer occurred. The returned
    /// token correlates the evidence and later audit result to this close.
    ///
    /// # Errors
    /// Returns `StaleOutcome` for child identities or unmatched close operations.
    /// Refused restored history returns `Cycle` or `DispatchRefused`.
    pub fn note_unbound_root(
        &mut self,
        root: &AgentLifetimeId,
        operation: &CloseOperationId,
    ) -> Result<UnboundRootSettlement, OwnershipError> {
        if self.child_request.contains_key(root)
            || self.lifetime_state(root) != Some(LifetimeState::Closing)
        {
            return Err(OwnershipError::StaleOutcome);
        }
        let evidence =
            self.note_absence(root, operation, root, AbsenceProof::NeverTransferredRoot)?;
        Ok(UnboundRootSettlement {
            root: root.clone(),
            operation: operation.clone(),
            evidence,
        })
    }

    /// Consume the actual audit result for previously confirmed root absence.
    /// Physical release remains true on rejection; closure requires acknowledgment.
    /// This completion is authorized by the evidence carried by the token.
    ///
    /// # Errors
    /// Returns `StaleOutcome` when the close operation or physical absence changed.
    /// Refused restored history returns `Cycle` or `DispatchRefused`.
    pub fn acknowledge_unbound_root(
        &mut self,
        token: UnboundRootSettlement,
        evidence: EvidenceFact,
    ) -> Result<(), OwnershipError> {
        if self.close_operation(&token.root) != Some(&token.operation) {
            return Err(OwnershipError::StaleOutcome);
        }
        self.acknowledge_observation(&token.evidence, evidence)
    }

    /// Open a root lifetime for a session that has no open or closing lifetime.
    pub fn open_root(
        &mut self,
        session: SessionId,
        lifetime: AgentLifetimeId,
        initiator: Initiator,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        if self.lifetimes.values().any(|existing| {
            existing.row.session_id == session && existing.row.state != LifetimeState::Closed
        }) {
            return Err(OwnershipError::LifetimeStillOpen);
        }
        if self.lifetimes.contains_key(&lifetime) {
            return Err(OwnershipError::Contradictory);
        }
        self.lifetimes.insert(
            lifetime.clone(),
            Lifetime {
                row: LifetimeRow {
                    lifetime_id: lifetime.clone(),
                    session_id: session,
                    state: LifetimeState::Open,
                    close_operation: None,
                    cause: None,
                    initiator: None,
                    cascaded_from: None,
                },
            },
        );
        Ok(OwnershipEvidence {
            close_detail: None,
            parent_lifetime: lifetime,
            child_lifetime: None,
            close_operation: None,
            before: OwnershipMeaning::Absent,
            after: OwnershipMeaning::Open,
            cause: None,
            initiator,
        })
    }

    /// Reserve a child or return the original child when the binding matches.
    pub fn admit_spawn(
        &mut self,
        admission: SpawnAdmission,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        if let Some(existing) = self.spawns.get(&admission.binding.request_id) {
            if existing.row.binding != admission.binding {
                return Err(OwnershipError::RequestConflict);
            }
            return Ok(evidence_for_spawn(
                &existing.row,
                meaning_of(&existing.row.progress),
                meaning_of(&existing.row.progress),
                Initiator::Runtime,
            ));
        }
        if self.spawns.len() >= MAX_RETAINED_REQUESTS {
            return Err(OwnershipError::RetainedRequestsExceeded);
        }
        let parent = self
            .lifetimes
            .get(&admission.binding.parent_lifetime)
            .ok_or(OwnershipError::ParentMissing)?;
        if parent.row.session_id != admission.binding.parent_session {
            return Err(OwnershipError::ForeignParent);
        }
        match parent.row.state {
            LifetimeState::Open => {}
            LifetimeState::Closing => return Err(OwnershipError::ParentClosing),
            LifetimeState::Closed => return Err(OwnershipError::ParentClosed),
        }
        if admission.child_lifetime == admission.binding.parent_lifetime {
            return Err(OwnershipError::SelfParent);
        }
        if self.lifetimes.contains_key(&admission.child_lifetime)
            || self.child_request.contains_key(&admission.child_lifetime)
        {
            return Err(OwnershipError::Contradictory);
        }
        if !admission.live_room {
            return Err(OwnershipError::NoRoom);
        }
        let depth = self.depth(&admission.binding.parent_lifetime);
        if depth >= MAX_DEPTH {
            return Err(OwnershipError::DepthExceeded);
        }
        if self.live_children(&admission.binding.parent_lifetime) >= MAX_DIRECT_CHILDREN {
            return Err(OwnershipError::DirectChildrenExceeded);
        }
        let child = admission.child_lifetime.clone();
        self.lifetimes.insert(
            child.clone(),
            Lifetime {
                row: LifetimeRow {
                    lifetime_id: child.clone(),
                    session_id: admission.child_session.clone(),
                    state: LifetimeState::Open,
                    close_operation: None,
                    cause: None,
                    initiator: None,
                    cascaded_from: None,
                },
            },
        );
        let row = SpawnRow {
            child_lifetime: child.clone(),
            child_session: admission.child_session,
            binding: admission.binding,
            progress: SpawnProgress::Reserved,
        };
        let evidence = evidence_for_spawn(
            &row,
            OwnershipMeaning::Absent,
            OwnershipMeaning::Reserved,
            Initiator::Runtime,
        );
        self.child_request
            .insert(child, row.binding.request_id.clone());
        self.spawns
            .insert(row.binding.request_id.clone(), Spawn { row });
        Ok(evidence)
    }

    /// Move a spawn along its chart. Lookup does not invent task admission.
    pub fn advance_spawn(
        &mut self,
        request: &SpawnRequestId,
        next: SpawnProgress,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        let spawn = self
            .spawns
            .get(request)
            .ok_or(OwnershipError::UnknownSpawn)?;
        if !legal_advance(&spawn.row.progress, &next) {
            return Err(OwnershipError::IllegalSpawnProgress);
        }
        let before = meaning_of(&spawn.row.progress);
        let spawn = self.spawns.get_mut(request).expect("spawn exists");
        spawn.row.progress = next;
        let after = meaning_of(&spawn.row.progress);
        Ok(evidence_for_spawn(
            &spawn.row,
            before,
            after,
            Initiator::Runtime,
        ))
    }

    /// Retain actual transferred factory ownership before its returned future is destroyed.
    /// Existing terminal safety progress stays terminal and gains the known preparation.
    ///
    /// # Errors
    /// Rejects unknown requests or a preparation unrelated to the retained chart.
    pub fn note_prepared_owner(
        &mut self,
        request: &SpawnRequestId,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        let spawn = self
            .spawns
            .get_mut(request)
            .ok_or(OwnershipError::UnknownSpawn)?;
        if spawn.row.progress.known() != KnownMilestone::Reserved {
            return Err(OwnershipError::IllegalSpawnProgress);
        }
        let before = meaning_of(&spawn.row.progress);
        spawn.row.progress = match spawn.row.progress {
            SpawnProgress::Draining { .. } => SpawnProgress::Draining {
                known: KnownMilestone::Prepared,
            },
            SpawnProgress::Unconfirmed { .. } => SpawnProgress::Unconfirmed {
                known: KnownMilestone::Prepared,
            },
            SpawnProgress::Ended { .. } | SpawnProgress::StartupFailed { .. } => {
                return Err(OwnershipError::StaleOutcome)
            }
            _ => SpawnProgress::Prepared,
        };
        Ok(evidence_for_spawn(
            &spawn.row,
            before,
            meaning_of(&spawn.row.progress),
            Initiator::Runtime,
        ))
    }

    /// Install an actually returned task receipt without reopening a sealed lifetime.
    ///
    /// # Errors
    /// Rejects unknown requests, pre-attachment progress or conflicting known receipts.
    pub fn note_task_receipt(
        &mut self,
        request: &SpawnRequestId,
        receipt: super::values::TaskReceiptId,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        let spawn = self
            .spawns
            .get_mut(request)
            .ok_or(OwnershipError::UnknownSpawn)?;
        let before = meaning_of(&spawn.row.progress);
        let known = KnownMilestone::TaskAdmitted {
            receipt: receipt.clone(),
        };
        if let KnownMilestone::TaskAdmitted { receipt: existing } = spawn.row.progress.known() {
            if existing != receipt {
                return Err(OwnershipError::StaleOutcome);
            }
        } else if spawn.row.progress.known() != KnownMilestone::Attached {
            return Err(OwnershipError::IllegalSpawnProgress);
        }
        spawn.row.progress = match spawn.row.progress {
            SpawnProgress::Draining { .. } => SpawnProgress::Draining { known },
            SpawnProgress::Ended { .. } => SpawnProgress::Ended { known },
            SpawnProgress::Unconfirmed { .. } => SpawnProgress::Unconfirmed { known },
            _ => SpawnProgress::TaskAdmitted { receipt },
        };
        Ok(evidence_for_spawn(
            &spawn.row,
            before,
            meaning_of(&spawn.row.progress),
            Initiator::Runtime,
        ))
    }

    /// After the factory returns, say whether the child may be dispatched.
    ///
    /// Both lifetimes must be open. A closing or closed parent returns
    /// [`Dispatch::Drain`]. A child that is already closing or closed, while
    /// its parent is still open, returns [`Dispatch::ChildUnavailable`].
    pub fn dispatch_after_prepare(
        &mut self,
        request: &SpawnRequestId,
    ) -> Result<Dispatch, OwnershipError> {
        self.ensure_dispatch()?;
        let spawn = self
            .spawns
            .get(request)
            .ok_or(OwnershipError::UnknownSpawn)?;
        let parent = spawn.row.binding.parent_lifetime.clone();
        let child = spawn.row.child_lifetime.clone();
        let progress = spawn.row.progress.clone();
        let parent_state = self
            .lifetime_state(&parent)
            .expect("a dispatchable spawn names a recorded parent");
        let child_state = self
            .lifetime_state(&child)
            .expect("a dispatchable spawn names a recorded child");
        if parent_state == LifetimeState::Open && child_state == LifetimeState::Open {
            return Ok(Dispatch::Allowed);
        }
        if !matches!(
            progress,
            SpawnProgress::Draining { .. }
                | SpawnProgress::Ended { .. }
                | SpawnProgress::TaskAdmitted { .. }
                | SpawnProgress::StartupFailed { .. }
        ) {
            let known = progress.known();
            let _evidence = self
                .advance_spawn(request, SpawnProgress::Draining { known })
                .expect("a non-terminal spawn drains to its known milestone");
        }
        if parent_state != LifetimeState::Open {
            return Ok(Dispatch::Drain);
        }
        Ok(Dispatch::ChildUnavailable)
    }

    /// Seal `lifetime` and every open descendant. A repeat joins the first cause.
    pub fn begin_close(
        &mut self,
        lifetime: &AgentLifetimeId,
        operation: CloseOperationId,
        cause: LifetimeCause,
        initiator: Initiator,
    ) -> Result<CloseAdmission, OwnershipError> {
        self.ensure_dispatch()?;
        let current = self
            .lifetimes
            .get(lifetime)
            .ok_or(OwnershipError::ParentMissing)?;
        match current.row.state {
            LifetimeState::Closed => {
                return Ok(CloseAdmission {
                    evidence: close_evidence(current, OwnershipMeaning::Closed),
                    joined: true,
                    targets: Vec::new(),
                });
            }
            LifetimeState::Closing => {
                let targets = self.unsettled_targets(lifetime);
                return Ok(CloseAdmission {
                    evidence: close_evidence(current, OwnershipMeaning::Closing),
                    joined: true,
                    targets,
                });
            }
            LifetimeState::Open => {}
        }
        let _sealed = self.seal(lifetime, operation, cause, initiator, None);
        let targets = self.unsettled_targets(lifetime);
        let current = self.lifetimes.get(lifetime).expect("sealed lifetime");
        Ok(CloseAdmission {
            evidence: close_evidence(current, OwnershipMeaning::Open),
            joined: false,
            targets,
        })
    }

    /// Record one correlated cleanup result. A mismatched operation is ignored as stale.
    pub fn apply_report(
        &mut self,
        close_lifetime: &AgentLifetimeId,
        operation: &CloseOperationId,
        target: &AgentLifetimeId,
        physical: PhysicalFact,
        evidence: EvidenceFact,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        self.validate_target(close_lifetime, operation, target)?;
        let key = (close_lifetime.clone(), target.clone());
        if self.physical(close_lifetime, target) == Some(PhysicalFact::Released)
            && physical != PhysicalFact::Released
        {
            return Err(OwnershipError::StaleOutcome);
        }
        let slot = physical_slot(physical);
        let record = self.observation_record(
            close_lifetime,
            operation,
            target,
            CloseEvidenceDetail::ResourceObservation {
                physical,
                provider_evidence: evidence,
            },
        );
        let settlement = self.settlements.entry(key).or_insert_with(|| Settlement {
            row: SettlementRow {
                close_lifetime: close_lifetime.clone(),
                target: target.clone(),
                physical,
                evidence,
                proof: SettlementProof::Resource(Box::new([None, None, None])),
            },
        });
        let SettlementProof::Resource(slots) = &mut settlement.row.proof else {
            return Err(OwnershipError::StaleOutcome);
        };
        let observation = slots[slot].get_or_insert(ResourceObservationAudit {
            record,
            acknowledgement: EvidenceFact::Pending,
            provider_acknowledged: false,
        });
        observation.provider_acknowledged |= evidence == EvidenceFact::Acknowledged;
        let result = observation.record.clone();
        refresh_summary(&mut settlement.row);
        Ok(result)
    }

    /// Admit or suppress a child result for the parent lifetime.
    pub fn admit_report(
        &mut self,
        report_id: ReportId,
        child: &AgentLifetimeId,
        parent: &AgentLifetimeId,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        if let Some(existing) = self.reports.get(&report_id) {
            if &existing.child_lifetime != child {
                return Err(OwnershipError::ReportConflict);
            }
            return Ok(report_evidence(existing, existing.state, existing.state));
        }
        let spawn_parent = self
            .child_request
            .get(child)
            .and_then(|request| self.spawns.get(request))
            .map(|spawn| spawn.row.binding.parent_lifetime.clone());
        if spawn_parent.as_ref() != Some(parent) {
            return Err(OwnershipError::UnknownChild);
        }
        let parent_state = self
            .lifetime_state(parent)
            .expect("a dispatchable spawn names a recorded parent");
        let state = match parent_state {
            LifetimeState::Open => DeliveryState::Submitted,
            LifetimeState::Closing | LifetimeState::Closed => DeliveryState::Suppressed,
        };
        let row = ReportRow {
            report_id: report_id.clone(),
            child_lifetime: child.clone(),
            parent_lifetime: parent.clone(),
            state,
        };
        let evidence = report_evidence(&row, DeliveryState::Retained, state);
        self.reports.insert(report_id, row);
        Ok(evidence)
    }

    /// Mark a report write uncertain, or confirm the same report by lookup.
    pub fn resolve_report(
        &mut self,
        report_id: &ReportId,
        found: Option<DeliveryState>,
    ) -> Result<OwnershipEvidence, OwnershipError> {
        self.ensure_dispatch()?;
        let row = self
            .reports
            .get(report_id)
            .ok_or(OwnershipError::UnknownChild)?;
        let before = row.state;
        let parent = row.parent_lifetime.clone();
        let next = match (before, found) {
            (DeliveryState::Submitted, Some(_)) => DeliveryState::Submitted,
            (DeliveryState::Submitted, None) => DeliveryState::Unconfirmed,
            (_, Some(DeliveryState::Submitted)) => DeliveryState::Submitted,
            (DeliveryState::Unconfirmed, None) => match self.lifetime_state(&parent) {
                Some(LifetimeState::Open) => DeliveryState::Unconfirmed,
                Some(LifetimeState::Closing | LifetimeState::Closed) | None => {
                    DeliveryState::Suppressed
                }
            },
            (DeliveryState::Suppressed, None) | (DeliveryState::Retained, None) => before,
            (_, Some(other)) => other,
        };
        let row = self.reports.get_mut(report_id).expect("report exists");
        row.state = next;
        Ok(report_evidence(row, before, next))
    }

    /// Read children of `parent` after `cursor`, at most `limit` of them.
    pub fn children_page(
        &self,
        parent: &AgentLifetimeId,
        cursor: Option<&AgentLifetimeId>,
        limit: usize,
    ) -> Result<ChildPage, OwnershipError> {
        if limit == 0 || limit > MAX_READ_PAGE {
            return Err(OwnershipError::PageLimit);
        }
        if !self.lifetimes.contains_key(parent) {
            return Err(OwnershipError::ParentMissing);
        }
        let mut matched = Vec::new();
        for spawn in self.spawns.values() {
            if &spawn.row.binding.parent_lifetime != parent {
                continue;
            }
            if cursor.is_some_and(|cursor| &spawn.row.child_lifetime <= cursor) {
                continue;
            }
            let state = self
                .lifetime_state(&spawn.row.child_lifetime)
                .ok_or(OwnershipError::Contradictory)?;
            matched.push(ChildView {
                lifetime: spawn.row.child_lifetime.clone(),
                session: spawn.row.child_session.clone(),
                parent: parent.clone(),
                state,
                progress: spawn.row.progress.clone(),
                policy: spawn.row.binding.policy.clone(),
            });
        }
        matched.sort_by(|left, right| left.lifetime.cmp(&right.lifetime));
        let more = matched.len() > limit;
        matched.truncate(limit);
        let next = more
            .then(|| matched.last().map(|child| child.lifetime.clone()))
            .flatten();
        Ok(ChildPage {
            children: matched,
            next,
        })
    }

    /// Copy the rows a store can write.
    pub fn snapshot(&self) -> OwnershipSnapshot {
        OwnershipSnapshot {
            close_completions: self.close_completions.values().cloned().collect(),
            lifetimes: self
                .lifetimes
                .values()
                .map(|lifetime| lifetime.row.clone())
                .collect(),
            spawns: self
                .spawns
                .values()
                .map(|spawn| spawn.row.clone())
                .collect(),
            settlements: self
                .settlements
                .values()
                .map(|settlement| settlement.row.clone())
                .collect(),
            reports: self.reports.values().cloned().collect(),
        }
    }

    /// Reload rows.
    ///
    /// An open descendant of a closing ancestor joins that close, so it is not
    /// runnable. An open descendant of a closed ancestor is contradictory: those
    /// rows stay as stored and later dispatch is refused. Other illegal history
    /// stays readable and refuses later dispatch.
    pub fn restore(snapshot: OwnershipSnapshot) -> Self {
        let mut graph = Self::new();
        let mut refusal = None;
        for row in snapshot.lifetimes {
            if graph.lifetimes.contains_key(&row.lifetime_id) {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
                continue;
            }
            if row.state == LifetimeState::Open
                && (row.cause.is_some() || row.close_operation.is_some() || row.initiator.is_some())
            {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
            if row.state != LifetimeState::Open
                && (row.cause.is_none() || row.initiator.is_none() || row.close_operation.is_none())
            {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
            graph
                .lifetimes
                .insert(row.lifetime_id.clone(), Lifetime { row });
        }
        for row in snapshot.spawns {
            if row.child_lifetime == row.binding.parent_lifetime {
                note_refusal(&mut refusal, OwnershipError::SelfParent);
            }
            if graph.spawns.contains_key(&row.binding.request_id)
                || graph.child_request.contains_key(&row.child_lifetime)
            {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
                continue;
            }
            match graph.lifetimes.get(&row.binding.parent_lifetime) {
                None => note_refusal(&mut refusal, OwnershipError::ParentMissing),
                Some(parent) if parent.row.session_id != row.binding.parent_session => {
                    note_refusal(&mut refusal, OwnershipError::ForeignParent);
                }
                Some(_) => {}
            }
            if !graph.lifetimes.contains_key(&row.child_lifetime) {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
            graph
                .child_request
                .insert(row.child_lifetime.clone(), row.binding.request_id.clone());
            graph
                .spawns
                .insert(row.binding.request_id.clone(), Spawn { row });
        }
        if graph.has_cycle() {
            note_refusal(&mut refusal, OwnershipError::Cycle);
        }
        // Refused ancestry cannot authorize dependent traversal. Keep every
        // retained row, but preserve the structural refusal before proof checks.
        let structural_refusal = refusal.is_some();
        for row in snapshot.settlements {
            let key = (row.close_lifetime.clone(), row.target.clone());
            if graph.settlements.contains_key(&key)
                || (!structural_refusal && !graph.valid_settlement(&row))
            {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
            graph.settlements.insert(key, Settlement { row });
        }
        for row in snapshot.close_completions {
            let root = row.close_lifetime.clone();
            if graph.close_completions.contains_key(&root)
                || (!structural_refusal
                    && (row.record.close_detail != Some(CloseEvidenceDetail::Completion)
                        || !graph.valid_record(&root, &root, &row.record)
                        || graph.cascaded_from(&root).is_some()
                        || !graph.completion_ready(&root)
                        || (row.acknowledgement == EvidenceFact::Acknowledged
                            && !graph.owned_lifetimes(&root).all(|id| {
                                graph.lifetime_state(id) == Some(LifetimeState::Closed)
                            }))))
            {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
            graph.close_completions.insert(root, row);
        }
        for (id, lifetime) in &graph.lifetimes {
            if lifetime.row.state != LifetimeState::Open && graph.close_owner(id).is_none() {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
            if lifetime.row.state == LifetimeState::Open && lifetime.row.cascaded_from.is_some() {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
            if lifetime.row.state == LifetimeState::Closed
                && !graph
                    .close_owner(id)
                    .and_then(|owner| graph.close_completions.get(&owner))
                    .is_some_and(|row| row.acknowledgement == EvidenceFact::Acknowledged)
            {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
            }
        }
        for row in snapshot.reports {
            if graph.reports.contains_key(&row.report_id) {
                note_refusal(&mut refusal, OwnershipError::Contradictory);
                continue;
            }
            graph.reports.insert(row.report_id.clone(), row);
        }
        // Repair only accepted history. A later proof refusal must not replace
        // the original rows or create recovery effects for a refused snapshot.
        if refusal.is_none() {
            graph.continue_interrupted_cascade();
        }
        graph.refusal = refusal;
        graph
    }

    fn ensure_dispatch(&self) -> Result<(), OwnershipError> {
        match &self.refusal {
            Some(OwnershipError::Cycle) => Err(OwnershipError::Cycle),
            Some(_) => Err(OwnershipError::DispatchRefused),
            None => Ok(()),
        }
    }

    fn depth(&self, lifetime: &AgentLifetimeId) -> u32 {
        let mut current = lifetime.clone();
        let mut depth = 0;
        while let Some(request) = self.child_request.get(&current) {
            current = self
                .spawns
                .get(request)
                .expect("child request maps to a spawn")
                .row
                .binding
                .parent_lifetime
                .clone();
            depth += 1;
            if depth > MAX_DEPTH {
                break;
            }
        }
        depth
    }

    fn live_children(&self, parent: &AgentLifetimeId) -> usize {
        self.spawns
            .values()
            .filter(|spawn| {
                &spawn.row.binding.parent_lifetime == parent
                    && self
                        .lifetime_state(&spawn.row.child_lifetime)
                        .is_some_and(|state| state != LifetimeState::Closed)
            })
            .count()
    }

    fn seal(
        &mut self,
        lifetime: &AgentLifetimeId,
        operation: CloseOperationId,
        cause: LifetimeCause,
        initiator: Initiator,
        cascaded_from: Option<AgentLifetimeId>,
    ) -> Vec<AgentLifetimeId> {
        let descendants: Vec<AgentLifetimeId> = self
            .spawns
            .values()
            .filter(|spawn| &spawn.row.binding.parent_lifetime == lifetime)
            .map(|spawn| spawn.row.child_lifetime.clone())
            .collect();
        let row = self
            .lifetimes
            .get_mut(lifetime)
            .expect("seal names a recorded lifetime");
        row.row.state = LifetimeState::Closing;
        row.row.close_operation = Some(operation.clone());
        row.row.cause = Some(cause.clone());
        row.row.initiator = Some(initiator.clone());
        row.row.cascaded_from = cascaded_from;
        let mut sealed = vec![lifetime.clone()];
        for child in descendants {
            let child_state = self.lifetime_state(&child);
            if child_state == Some(LifetimeState::Open) {
                sealed.extend(self.seal(
                    &child,
                    operation.clone(),
                    cause.clone(),
                    initiator.clone(),
                    Some(lifetime.clone()),
                ));
            }
        }
        sealed
    }

    /// Repair accepted history after structural and Completion validation.
    /// Completion readiness owns refusal of Open descendants of a Closed owner;
    /// see completed_ancestor_refuses_open_descendants_without_repairing_retained_history.
    fn continue_interrupted_cascade(&mut self) {
        let open: Vec<AgentLifetimeId> = self
            .lifetimes
            .iter()
            .filter(|(_, lifetime)| lifetime.row.state == LifetimeState::Open)
            .map(|(id, _)| id.clone())
            .collect();
        let gaps: Vec<AgentLifetimeId> = open
            .into_iter()
            .filter(|id| {
                self.parent_id(id)
                    .and_then(|parent| self.lifetime_state(&parent))
                    == Some(LifetimeState::Closing)
            })
            .collect();
        let mut records = Vec::new();
        for id in gaps {
            let parent = self
                .parent_id(&id)
                .expect("an open child of a closing parent names that parent");
            let operation = self
                .close_operation(&parent)
                .cloned()
                .expect("a closing parent keeps its close operation");
            let cause = self
                .close_cause(&parent)
                .cloned()
                .expect("a closing parent keeps its cause");
            let initiator = self
                .close_initiator(&parent)
                .cloned()
                .expect("a closing parent keeps its initiator");
            let sealed = self.seal(&id, operation, cause, initiator, Some(parent));
            for lifetime in sealed {
                let row = self
                    .lifetimes
                    .get(&lifetime)
                    .expect("sealed lifetime is recorded");
                records.push(OwnershipEvidence {
                    close_detail: None,
                    parent_lifetime: lifetime,
                    child_lifetime: None,
                    close_operation: row.row.close_operation.clone(),
                    before: OwnershipMeaning::Open,
                    after: OwnershipMeaning::Closing,
                    cause: row.row.cause.clone(),
                    initiator: row
                        .row
                        .initiator
                        .clone()
                        .expect("a sealed lifetime keeps its initiator"),
                });
            }
        }
        self.recovery = records;
    }

    fn parent_id(&self, lifetime: &AgentLifetimeId) -> Option<AgentLifetimeId> {
        let request = self.child_request.get(lifetime)?;
        Some(
            self.spawns
                .get(request)
                .expect("child request maps to a spawn")
                .row
                .binding
                .parent_lifetime
                .clone(),
        )
    }

    fn unsettled_targets(&self, root: &AgentLifetimeId) -> Vec<AgentLifetimeId> {
        let mut targets = Vec::new();
        self.collect_targets(root, root, &mut targets);
        targets
    }

    fn collect_targets(
        &self,
        root: &AgentLifetimeId,
        lifetime: &AgentLifetimeId,
        targets: &mut Vec<AgentLifetimeId>,
    ) {
        let state = self.lifetime_state(lifetime);
        if state == Some(LifetimeState::Closing)
            && self.close_owner(lifetime).as_ref() == Some(root)
        {
            let released = self
                .settlements
                .get(&(root.clone(), lifetime.clone()))
                .is_some_and(|settlement| settlement.row.physical == PhysicalFact::Released);
            if !released {
                targets.push(lifetime.clone());
            }
        }
        let children: Vec<AgentLifetimeId> = self
            .spawns
            .values()
            .filter(|spawn| &spawn.row.binding.parent_lifetime == lifetime)
            .map(|spawn| spawn.row.child_lifetime.clone())
            .collect();
        for child in children {
            self.collect_targets(root, &child, targets);
        }
    }

    fn in_subtree(&self, root: &AgentLifetimeId, target: &AgentLifetimeId) -> bool {
        if root == target {
            return true;
        }
        let mut pending = vec![root.clone()];
        while let Some(current) = pending.pop() {
            for spawn in self.spawns.values() {
                if spawn.row.binding.parent_lifetime == current {
                    if &spawn.row.child_lifetime == target {
                        return true;
                    }
                    pending.push(spawn.row.child_lifetime.clone());
                }
            }
        }
        false
    }

    fn has_cycle(&self) -> bool {
        for lifetime in self.lifetimes.keys() {
            let mut seen = BTreeSet::new();
            let mut current = lifetime.clone();
            while let Some(request) = self.child_request.get(&current) {
                if !seen.insert(current.clone()) {
                    return true;
                }
                current = self
                    .spawns
                    .get(request)
                    .expect("child request maps to a spawn")
                    .row
                    .binding
                    .parent_lifetime
                    .clone();
            }
        }
        false
    }
}

impl Default for OwnershipGraph {
    fn default() -> Self {
        Self::new()
    }
}

fn note_refusal(slot: &mut Option<OwnershipError>, error: OwnershipError) {
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn legal_advance(current: &SpawnProgress, next: &SpawnProgress) -> bool {
    match (current, next) {
        (SpawnProgress::Reserved, SpawnProgress::Prepared)
        | (SpawnProgress::Prepared, SpawnProgress::Attached)
        | (SpawnProgress::Attached, SpawnProgress::TaskAdmitted { .. }) => true,
        (
            SpawnProgress::TaskAdmitted { receipt },
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::TaskAdmitted { receipt: known },
            },
        ) => receipt == known,
        (
            SpawnProgress::Reserved | SpawnProgress::Prepared | SpawnProgress::Attached,
            SpawnProgress::Unconfirmed { known },
        ) => known == &current.known(),
        (
            SpawnProgress::Reserved | SpawnProgress::Prepared | SpawnProgress::Attached,
            SpawnProgress::Draining { known },
        ) => known == &current.known(),
        (
            SpawnProgress::Reserved | SpawnProgress::Prepared,
            SpawnProgress::StartupFailed { known },
        ) => known == &current.known(),
        (
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Reserved,
            },
            SpawnProgress::Prepared,
        ) => true,
        (
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Reserved | KnownMilestone::Prepared,
            },
            SpawnProgress::Attached,
        ) => true,
        (
            SpawnProgress::Unconfirmed {
                known:
                    KnownMilestone::Reserved | KnownMilestone::Prepared | KnownMilestone::Attached,
            },
            SpawnProgress::TaskAdmitted { .. },
        ) => true,
        (SpawnProgress::Unconfirmed { known }, SpawnProgress::Draining { known: draining }) => {
            draining == known
        }
        (SpawnProgress::Draining { known }, SpawnProgress::Ended { known: ended }) => {
            ended == known
        }
        (SpawnProgress::StartupFailed { known }, SpawnProgress::Ended { known: ended }) => {
            ended == known
        }
        (
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Reserved,
            },
            SpawnProgress::Reserved,
        ) => true,
        (
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Prepared,
            },
            SpawnProgress::Prepared,
        ) => true,
        (
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::Attached,
            },
            SpawnProgress::Attached,
        ) => true,
        (
            SpawnProgress::Unconfirmed {
                known: KnownMilestone::TaskAdmitted { receipt: known },
            },
            SpawnProgress::TaskAdmitted { receipt },
        ) => receipt == known,
        _ => false,
    }
}

fn meaning_of(progress: &SpawnProgress) -> OwnershipMeaning {
    match progress {
        SpawnProgress::Reserved => OwnershipMeaning::Reserved,
        SpawnProgress::Prepared => OwnershipMeaning::Prepared,
        SpawnProgress::Attached => OwnershipMeaning::Attached,
        SpawnProgress::TaskAdmitted { .. } => OwnershipMeaning::TaskAdmitted,
        SpawnProgress::Unconfirmed { .. } => OwnershipMeaning::Unconfirmed,
        SpawnProgress::Draining { .. } => OwnershipMeaning::Draining,
        SpawnProgress::StartupFailed { .. } => OwnershipMeaning::StartupFailed,
        SpawnProgress::Ended { .. } => OwnershipMeaning::Ended,
    }
}

fn evidence_for_spawn(
    row: &SpawnRow,
    before: OwnershipMeaning,
    after: OwnershipMeaning,
    initiator: Initiator,
) -> OwnershipEvidence {
    OwnershipEvidence {
        close_detail: None,
        parent_lifetime: row.binding.parent_lifetime.clone(),
        child_lifetime: Some(row.child_lifetime.clone()),
        close_operation: None,
        before,
        after,
        cause: None,
        initiator,
    }
}

fn close_evidence(lifetime: &Lifetime, before: OwnershipMeaning) -> OwnershipEvidence {
    OwnershipEvidence {
        close_detail: None,
        parent_lifetime: lifetime.row.lifetime_id.clone(),
        child_lifetime: None,
        close_operation: lifetime.row.close_operation.clone(),
        before,
        after: if lifetime.row.state == LifetimeState::Closed {
            OwnershipMeaning::Closed
        } else {
            OwnershipMeaning::Closing
        },
        cause: lifetime.row.cause.clone(),
        initiator: lifetime.row.initiator.clone().unwrap_or(Initiator::Runtime),
    }
}

fn report_evidence(
    row: &ReportRow,
    before: DeliveryState,
    after: DeliveryState,
) -> OwnershipEvidence {
    OwnershipEvidence {
        close_detail: None,
        parent_lifetime: row.parent_lifetime.clone(),
        child_lifetime: Some(row.child_lifetime.clone()),
        close_operation: None,
        before: delivery_meaning(before),
        after: delivery_meaning(after),
        cause: None,
        initiator: Initiator::Runtime,
    }
}

fn delivery_meaning(state: DeliveryState) -> OwnershipMeaning {
    match state {
        DeliveryState::Retained => OwnershipMeaning::Absent,
        DeliveryState::Submitted => OwnershipMeaning::Submitted,
        DeliveryState::Suppressed => OwnershipMeaning::Suppressed,
        DeliveryState::Unconfirmed => OwnershipMeaning::Unconfirmed,
    }
}

fn physical_slot(physical: PhysicalFact) -> usize {
    match physical {
        PhysicalFact::Pending => 0,
        PhysicalFact::Failed => 1,
        PhysicalFact::Released => 2,
    }
}

/// One derivation owns both the coarse inspection summary and aggregate readiness.
fn settlement_summary(proof: &SettlementProof) -> (PhysicalFact, EvidenceFact) {
    match proof {
        SettlementProof::Absence(absence) => (PhysicalFact::Released, absence.acknowledgement),
        SettlementProof::Resource(slots) => {
            let Some(observation) = slots.iter().rev().flatten().next() else {
                // Refused restored history is retained for inspection. Its
                // missing observation supplies no aggregate readiness.
                return (PhysicalFact::Pending, EvidenceFact::Pending);
            };
            let Some(CloseEvidenceDetail::ResourceObservation {
                physical,
                provider_evidence,
            }) = observation.record.close_detail
            else {
                return (PhysicalFact::Pending, EvidenceFact::Pending);
            };
            let evidence = if slots
                .iter()
                .flatten()
                .any(|a| a.acknowledgement == EvidenceFact::Failed)
            {
                EvidenceFact::Failed
            } else if slots
                .iter()
                .flatten()
                .any(|a| a.acknowledgement != EvidenceFact::Acknowledged)
            {
                EvidenceFact::Pending
            } else if observation.provider_acknowledged {
                EvidenceFact::Acknowledged
            } else {
                provider_evidence
            };
            (physical, evidence)
        }
    }
}

fn refresh_summary(row: &mut SettlementRow) {
    (row.physical, row.evidence) = settlement_summary(&row.proof);
}
