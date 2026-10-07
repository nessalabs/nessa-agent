//! Audit eligibility for projections of the single live lifecycle graph.
use std::collections::{HashMap, HashSet};

use crate::domain::agent_execution::subagents::{
    AgentLifetimeId, DeliveryState, KnownMilestone, OwnershipGraph, OwnershipSnapshot, ReportId,
    SpawnProgress, SpawnRequestId,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum PublicationTarget {
    Root(AgentLifetimeId),
    Spawn(SpawnRequestId),
    Report(ReportId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PublishedState {
    Root,
    Spawn(SpawnProgress),
    Report(DeliveryState),
}

/// Captured with the graph mutation, before any port await.
#[derive(Clone, Debug)]
pub(super) struct PublicationToken {
    target: PublicationTarget,
    generation: u64,
    proposed: PublishedState,
    previous: Option<PublishedState>,
}

impl PublicationToken {
    pub(super) fn is_safety(&self) -> bool {
        matches!(
            self.proposed,
            PublishedState::Spawn(
                SpawnProgress::Unconfirmed { .. }
                    | SpawnProgress::Draining { .. }
                    | SpawnProgress::StartupFailed { .. }
                    | SpawnProgress::Ended { .. }
            ) | PublishedState::Report(DeliveryState::Suppressed)
        )
    }
}

#[derive(Default)]
pub(super) struct OwnershipPublication {
    roots: HashSet<AgentLifetimeId>,
    spawns: HashMap<SpawnRequestId, SpawnProgress>,
    reports: HashMap<ReportId, DeliveryState>,
    pending: HashMap<PublicationTarget, PublicationToken>,
    generation: u64,
}

impl OwnershipPublication {
    pub(super) fn restored(snapshot: &OwnershipSnapshot) -> Self {
        let children: HashSet<_> = snapshot.spawns.iter().map(|s| &s.child_lifetime).collect();
        Self {
            roots: snapshot
                .lifetimes
                .iter()
                .filter(|r| !children.contains(&r.lifetime_id))
                .map(|r| r.lifetime_id.clone())
                .collect(),
            spawns: snapshot
                .spawns
                .iter()
                .map(|s| (s.binding.request_id.clone(), s.progress.clone()))
                .collect(),
            reports: snapshot
                .reports
                .iter()
                .map(|r| (r.report_id.clone(), r.state))
                .collect(),
            ..Self::default()
        }
    }

    pub(super) fn begin(
        &mut self,
        target: PublicationTarget,
        proposed: PublishedState,
    ) -> PublicationToken {
        if let Some(token) = self.pending.get(&target).filter(|t| t.proposed == proposed) {
            return token.clone();
        }
        self.generation += 1;
        let previous = self.eligible(&target);
        let token = PublicationToken {
            target: target.clone(),
            generation: self.generation,
            proposed,
            previous,
        };
        self.pending.insert(target, token.clone());
        token
    }

    fn eligible(&self, target: &PublicationTarget) -> Option<PublishedState> {
        match target {
            PublicationTarget::Root(id) => self.roots.contains(id).then_some(PublishedState::Root),
            PublicationTarget::Spawn(id) => self.spawns.get(id).cloned().map(PublishedState::Spawn),
            PublicationTarget::Report(id) => {
                self.reports.get(id).copied().map(PublishedState::Report)
            }
        }
    }

    pub(super) fn acknowledge(&mut self, token: &PublicationToken) -> bool {
        if self.eligible(&token.target) != token.previous {
            return false;
        }
        if self.pending.get(&token.target).map(|t| t.generation) != Some(token.generation) {
            return false;
        }
        self.pending.remove(&token.target);
        match (&token.target, &token.proposed) {
            (PublicationTarget::Root(id), PublishedState::Root) => {
                self.roots.insert(id.clone());
            }
            (PublicationTarget::Spawn(id), PublishedState::Spawn(progress)) => {
                self.spawns.insert(id.clone(), progress.clone());
            }
            (PublicationTarget::Report(id), PublishedState::Report(state)) => {
                self.reports.insert(id.clone(), *state);
            }
            _ => unreachable!("publication target and state are captured together"),
        }
        true
    }

    pub(super) fn retain_root(&mut self, id: &AgentLifetimeId) {
        self.roots.insert(id.clone());
    }
    pub(super) fn root_eligible(&self, id: &AgentLifetimeId) -> bool {
        self.roots.contains(id)
    }
    pub(super) fn discard_root(&mut self, id: &AgentLifetimeId) {
        self.pending.remove(&PublicationTarget::Root(id.clone()));
    }
    pub(super) fn spawn_eligible(&self, id: &SpawnRequestId) -> bool {
        self.spawns.contains_key(id)
    }
    pub(super) fn retain_spawn(&mut self, id: &SpawnRequestId) {
        self.spawns
            .entry(id.clone())
            .or_insert(SpawnProgress::Reserved);
    }

    fn eligible_lifetimes(&self, snapshot: &OwnershipSnapshot) -> HashSet<AgentLifetimeId> {
        let mut retained = self.roots.clone();
        // Retain identities only with their retained ancestor chain. Admission
        // uses this same authority; graph Open alone does not grant transfer.
        loop {
            let before = retained.len();
            for row in &snapshot.spawns {
                if self.spawns.contains_key(&row.binding.request_id)
                    && retained.contains(&row.binding.parent_lifetime)
                {
                    retained.insert(row.child_lifetime.clone());
                }
            }
            if retained.len() == before {
                break;
            }
        }
        retained
    }

    pub(super) fn lifetime_eligible(
        &self,
        graph: &OwnershipGraph,
        lifetime: &AgentLifetimeId,
    ) -> bool {
        self.eligible_lifetimes(&graph.snapshot())
            .contains(lifetime)
    }

    pub(super) fn project(&self, graph: &OwnershipGraph) -> OwnershipSnapshot {
        let mut snapshot = graph.snapshot();
        let retained = self.eligible_lifetimes(&snapshot);
        snapshot
            .lifetimes
            .retain(|r| retained.contains(&r.lifetime_id));
        snapshot.spawns.retain(|r| {
            retained.contains(&r.child_lifetime) && retained.contains(&r.binding.parent_lifetime)
        });
        for row in &mut snapshot.spawns {
            let eligible = &self.spawns[&row.binding.request_id];
            if let Some(safety) = graph.sealed_spawn_progress(&row.binding.request_id, eligible) {
                row.progress = safety;
                continue;
            }
            // A returned submit receipt is an observed effect, even when its
            // permission-publication audit failed. Prepared likewise records a
            // transferred factory owner. An unaudited Attached permission is
            // conservatively represented by that prior physical preparation.
            let known = match row.progress.known() {
                milestone @ KnownMilestone::TaskAdmitted { .. } => milestone,
                KnownMilestone::Prepared => KnownMilestone::Prepared,
                KnownMilestone::Attached
                    if eligible.known() == KnownMilestone::Reserved
                        || eligible.known() == KnownMilestone::Prepared =>
                {
                    KnownMilestone::Prepared
                }
                _ => eligible.known(),
            };
            row.progress = match row.progress {
                SpawnProgress::Unconfirmed { .. } => SpawnProgress::Unconfirmed { known },
                SpawnProgress::Draining { .. } => SpawnProgress::Draining { known },
                SpawnProgress::StartupFailed { .. } => SpawnProgress::StartupFailed { known },
                SpawnProgress::Ended { .. } => SpawnProgress::Ended { known },
                _ => eligible.clone(),
            };
        }
        snapshot
            .settlements
            .retain(|r| retained.contains(&r.close_lifetime) && retained.contains(&r.target));
        snapshot.reports.retain(|r| {
            retained.contains(&r.parent_lifetime)
                && retained.contains(&r.child_lifetime)
                && (self.reports.contains_key(&r.report_id) || r.state == DeliveryState::Suppressed)
        });
        for row in &mut snapshot.reports {
            if row.state != DeliveryState::Suppressed {
                row.state = self.reports[&row.report_id];
            }
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_17_tokens_acknowledge_captured_progress_once_and_refuse_stale_generation() {
        let id = SpawnRequestId::new("request").unwrap();
        let mut p = OwnershipPublication::default();
        let old = p.begin(
            PublicationTarget::Spawn(id.clone()),
            PublishedState::Spawn(SpawnProgress::Prepared),
        );
        let current = p.begin(
            PublicationTarget::Spawn(id.clone()),
            PublishedState::Spawn(SpawnProgress::Attached),
        );
        assert!(!p.acknowledge(&old));
        assert!(p.acknowledge(&current));
        assert!(!p.acknowledge(&current));
        assert_eq!(p.spawns[&id], SpawnProgress::Attached);
    }

    #[test]
    fn repeated_pending_report_joins_same_generation() {
        let id = ReportId::new("report").unwrap();
        let mut p = OwnershipPublication::default();
        let first = p.begin(
            PublicationTarget::Report(id.clone()),
            PublishedState::Report(DeliveryState::Submitted),
        );
        let joined = p.begin(
            PublicationTarget::Report(id),
            PublishedState::Report(DeliveryState::Submitted),
        );
        assert_eq!(first.generation, joined.generation);
        assert!(p.acknowledge(&first));
        assert!(!p.acknowledge(&joined));
        let repeated = p.begin(first.target.clone(), first.proposed.clone());
        assert!(p.acknowledge(&repeated));
        assert!(!p.acknowledge(&repeated));
    }
    #[test]
    fn retained_projection_preserves_valid_history_and_referential_closure() {
        use crate::domain::agent_execution::{
            sessions::SessionId,
            subagents::{
                ApprovalPolicy, CloseOperationId, EvidenceFact, Initiator, LifetimeCause,
                OwnershipGraph, PhysicalFact, SpawnAdmission, SpawnBinding, SpawnOrigin,
                TaskDigest,
            },
        };
        let root = AgentLifetimeId::new("root").unwrap();
        let child = AgentLifetimeId::new("child").unwrap();
        let private = AgentLifetimeId::new("private").unwrap();
        let mut graph = OwnershipGraph::new();
        let opened = graph
            .open_root(
                SessionId::new("root-session").unwrap(),
                root.clone(),
                Initiator::Runtime,
            )
            .unwrap();
        assert_eq!(opened.parent_lifetime, root);
        let admitted = graph
            .admit_spawn(SpawnAdmission {
                child_lifetime: child.clone(),
                child_session: SessionId::new("child-session").unwrap(),
                live_room: true,
                binding: SpawnBinding {
                    parent_lifetime: root.clone(),
                    parent_session: SessionId::new("root-session").unwrap(),
                    request_id: SpawnRequestId::new("request").unwrap(),
                    task_digest: TaskDigest::new("0".repeat(64)).unwrap(),
                    policy: ApprovalPolicy::new("read-only", "ask", "rev").unwrap(),
                    model: None,
                    origin: SpawnOrigin::Host(
                        crate::domain::agent_execution::subagents::HostActor::new(
                            "person", "desktop", "request",
                        )
                        .unwrap(),
                    ),
                },
            })
            .unwrap();
        assert_eq!(admitted.child_lifetime, Some(child.clone()));
        // An acknowledged child below an unpublished ancestor cannot become
        // a standalone root after projection/reload, even with stale metadata.
        let mut orphaned = OwnershipPublication::default();
        let token = orphaned.begin(
            PublicationTarget::Spawn(SpawnRequestId::new("request").unwrap()),
            PublishedState::Spawn(SpawnProgress::Reserved),
        );
        assert!(orphaned.acknowledge(&token));
        assert!(!orphaned.lifetime_eligible(&graph, &child));
        let projection = orphaned.project(&graph);
        assert!(projection.lifetimes.is_empty());
        assert!(projection.spawns.is_empty());
        assert!(OwnershipGraph::restore(projection)
            .snapshot()
            .lifetimes
            .is_empty());
        let report = graph
            .admit_report(ReportId::new("report").unwrap(), &child, &root)
            .unwrap();
        assert_eq!(report.child_lifetime, Some(child.clone()));
        let operation = CloseOperationId::new("close").unwrap();
        graph
            .begin_close(
                &root,
                operation.clone(),
                LifetimeCause::HostClose,
                Initiator::Runtime,
            )
            .unwrap();
        for target in [&root, &child] {
            let outcome = graph
                .apply_report(
                    &root,
                    &operation,
                    target,
                    PhysicalFact::Released,
                    EvidenceFact::Acknowledged,
                )
                .unwrap();
            assert_eq!(outcome.child_lifetime.as_ref(), Some(target));
        }
        let acknowledged = graph.snapshot();
        let p = OwnershipPublication::restored(&acknowledged);
        let private_evidence = graph
            .open_root(
                SessionId::new("private-session").unwrap(),
                private,
                Initiator::Runtime,
            )
            .unwrap();
        assert!(private_evidence.child_lifetime.is_none());
        let projected = p.project(&graph);
        assert_eq!(projected, acknowledged);
        assert!(OwnershipGraph::restore(projected).refusal().is_none());
    }
}
