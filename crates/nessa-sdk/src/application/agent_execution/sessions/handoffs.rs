//! Handoffs in a conversation's stream: the points after which its agent runs
//! in a native session that has not seen the turns before them (issue #1080,
//! `docs/design/conversation-handoffs.md`).
//!
//! ```text
//! HandoffIntent::Fork   ──prepare, new conversation──▶ Opened + Handoff(Fork)
//! HandoffIntent::Switch ──prepare, other provider────▶ attach: Handoff(Switch)
//!                                                       + ProviderContext, one save
//! Handoff ──first turn admitted after it──▶ taken(turn) ──dispatch──▶ history block
//! ```
//!
//! A fork starts a new conversation from a copy of another's turns, on the
//! same agent. A switch carries the same conversation on under another
//! provider or model, in a new native session of it. Both are one record,
//! [`SessionChange::Handoff`], and from there on they are the same thing.
//!
//! Each fact has one record, as a note's do: the handoff itself, the turn
//! that took it (its [`InvocationRecord::handoff`], chosen at admission and
//! never chosen again on retry), and what that turn is given, which is read
//! from the records when it is dispatched ([`history_before`]) and never
//! stored twice. Only the latest handoff is taken: what an earlier one would
//! carry, the later one carries too.
//!
//! [`SessionChange::Handoff`]: super::SessionChange::Handoff
//! [`InvocationRecord::handoff`]: super::InvocationRecord::handoff
use super::{InvocationRecord, SessionSnapshot, StorageError};
use crate::application::agent_execution::{
    executions::ExecutionUpdate, permissions::ActionContext, providers::ProviderIdentity,
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, MessageKind},
    prompts::{CarriedHistory, CarriedTurn},
    sessions::SessionId,
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// The conversation's own name for one handoff, made when it is recorded.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HandoffId(Box<str>);
impl HandoffId {
    /// Longest id, in bytes. An id of a request is 32 hex digits.
    pub const MAX_BYTES: usize = 64;

    /// The id of the handoff `actor` asks for: the same for a retry of the
    /// same request, so a preparation derived again writes the same record.
    /// The SHA-256 of the principal and the request, as 32 hex digits.
    pub fn of_request(actor: &ActionContext) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"nessa.handoff\0");
        digest.update(actor.principal_id().as_bytes());
        digest.update(b"\0");
        digest.update(actor.request_id().as_bytes());
        let hex: String = digest.finalize()[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self(hex.into_boxed_str())
    }
    /// An id read back: 1 to [`Self::MAX_BYTES`] bytes, not blank.
    ///
    /// # Errors
    ///
    /// [`StorageError::Corrupt`] for anything else.
    pub fn new(value: impl Into<String>) -> Result<Self, StorageError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > Self::MAX_BYTES {
            return Err(corrupt("a handoff id must be 1 to 64 bytes"));
        }
        Ok(Self(value.into_boxed_str()))
    }
    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a handoff is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffKind {
    /// The conversation goes on under another provider or model. The native
    /// session of `before` cannot be resumed under `after`, so a new one is
    /// opened.
    Switch {
        /// The identity the history held.
        before: ProviderIdentity,
        /// The identity it goes on under.
        after: ProviderIdentity,
    },
    /// The conversation began as a copy of another's turns.
    Fork {
        /// The conversation it was copied from.
        source: SessionId,
        /// What was copied: that conversation's turns as a native session is
        /// given them.
        history: CarriedHistory,
    },
}

/// One handoff the conversation recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandoffRecord {
    /// Its id, made when it was recorded.
    pub id: HandoffId,
    /// What it is.
    pub kind: HandoffKind,
    /// Who asked for it, from which surface, under which request: how the
    /// host finds it again ([`SessionSnapshot::handoff`]).
    pub actor: ActionContext,
    /// When it was recorded, in milliseconds since the Unix epoch, on the
    /// conversation's injected wall clock.
    pub at_ms: u64,
    /// The last turn the conversation held when it was recorded, so a
    /// transcript places it after that turn; `None` before any turn.
    pub after_execution: Option<ExecutionId>,
}
impl HandoffRecord {
    /// Most handoffs one conversation keeps. Another is refused.
    pub const MAX_PER_CONVERSATION: usize = 256;
    /// Latest time a handoff may carry: the largest integer a JSON number
    /// holds exactly.
    pub const MAX_AT_MS: u64 = (1 << 53) - 1;

    /// Whether this handoff answers `principal`'s request `request`.
    pub fn answers(&self, principal: &str, request: &str) -> bool {
        self.actor.principal_id() == principal && self.actor.request_id() == request
    }
}

/// What the host asks of the next preparation of a conversation, besides
/// opening it: the one way a handoff is recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandoffIntent {
    /// Go on under the provider this preparation is given, whose name or
    /// model differs from the one the history holds. Refused when the
    /// conversation has no history yet.
    Switch {
        /// Who asked, as the host verified.
        actor: ActionContext,
        /// When the host admitted it, in milliseconds since the Unix epoch:
        /// the record's [`HandoffRecord::at_ms`].
        at_ms: u64,
    },
    /// Start this new conversation from `history`, copied from `source`.
    /// Refused when the conversation already exists and was not forked by
    /// this same request.
    Fork {
        /// The conversation the history was copied from.
        source: SessionId,
        /// What was copied ([`SessionSnapshot::carried_history`]).
        history: CarriedHistory,
        /// Who asked, as the host verified.
        actor: ActionContext,
        /// When the host admitted it, as for a switch.
        at_ms: u64,
    },
}
impl HandoffIntent {
    /// Who asked for it.
    pub fn actor(&self) -> &ActionContext {
        match self {
            Self::Switch { actor, .. } | Self::Fork { actor, .. } => actor,
        }
    }
}

impl SessionSnapshot {
    /// The handoff `principal` asked for under request `request`, if one was
    /// recorded: how a host finds its switch or fork again.
    pub fn handoff(&self, principal: &str, request: &str) -> Option<&HandoffRecord> {
        self.handoffs
            .iter()
            .find(|handoff| handoff.answers(principal, request))
    }

    /// Everything this conversation holds, as a native session that never saw
    /// it is given it: what its fork copied, then each of its turns, the
    /// newest that fit. What a fork of it copies.
    pub fn carried_history(&self) -> CarriedHistory {
        history_before(self, self.invocations.len())
    }
}

/// The history the turn at `index` is given: what the conversation's fork
/// copied, if it is a fork, then every turn saved before `index`, the newest
/// that fit [`CarriedHistory`]'s bounds.
pub(crate) fn history_before(snapshot: &SessionSnapshot, index: usize) -> CarriedHistory {
    let (omitted, mut turns) = snapshot
        .handoffs
        .iter()
        .find_map(|handoff| match &handoff.kind {
            HandoffKind::Fork { history, .. } => {
                Some((history.omitted(), history.turns().to_vec()))
            }
            HandoffKind::Switch { .. } => None,
        })
        .unwrap_or_default();
    turns.extend(
        snapshot.invocations[..index.min(snapshot.invocations.len())]
            .iter()
            .filter_map(carried_turn),
    );
    CarriedHistory::newest(omitted, turns)
}

/// One saved turn as it is carried: the person's text, and the text after the
/// turn's last tool step, its final answer.
fn carried_turn(record: &InvocationRecord) -> Option<CarriedTurn> {
    let mut reply = String::new();
    for event in &record.events {
        match event.update() {
            ExecutionUpdate::Message(chunk) if chunk.kind() == MessageKind::Text => {
                // A reply longer than a turn carries keeps its start.
                if reply.len() <= CarriedTurn::MAX_PART_BYTES {
                    reply.push_str(chunk.as_str());
                }
            }
            ExecutionUpdate::Tool(_) => reply.clear(),
            _ => {}
        }
    }
    CarriedTurn::clipped(
        record.request.user_message.text().map(|text| text.as_str()),
        Some(&reply),
    )
}

/// The handoff a turn admitted now takes: the latest, when no turn took it.
pub(super) fn select(snapshot: &SessionSnapshot) -> Option<HandoffId> {
    let latest = snapshot.handoffs.last()?;
    let taken = snapshot
        .invocations
        .iter()
        .any(|record| record.handoff.as_ref() == Some(&latest.id));
    (!taken).then(|| latest.id.clone())
}

/// Whether the handoff a turn names may be folded into `snapshot` with it:
/// the latest, and taken by no turn yet.
pub(super) fn admit_taken(
    snapshot: &SessionSnapshot,
    taken: Option<&HandoffId>,
) -> Result<(), StorageError> {
    let Some(taken) = taken else {
        return Ok(());
    };
    if select(snapshot).as_ref() != Some(taken) {
        return Err(corrupt(
            "a turn takes a handoff that is not the latest one waiting",
        ));
    }
    Ok(())
}

/// Whether `record` may follow what `snapshot` already records: within the
/// bound, with an id and a principal's request no earlier handoff has,
/// placed after the conversation's last turn. A switch must leave the
/// identity the history holds, for another provider name or model; a fork
/// must be the first thing a new conversation holds.
pub(super) fn admit_saved(
    snapshot: &SessionSnapshot,
    record: &HandoffRecord,
) -> Result<(), StorageError> {
    if snapshot.handoffs.len() >= HandoffRecord::MAX_PER_CONVERSATION {
        return Err(corrupt("the conversation keeps more handoffs than it may"));
    }
    if record.at_ms > HandoffRecord::MAX_AT_MS {
        return Err(corrupt("a handoff is later than a record can state"));
    }
    if record.after_execution.as_ref()
        != snapshot
            .invocations
            .last()
            .map(|turn| &turn.request.execution_id)
    {
        return Err(corrupt(
            "a handoff does not follow the conversation's last turn",
        ));
    }
    let repeated = snapshot.handoffs.iter().any(|handoff| {
        handoff.id == record.id
            || handoff.answers(record.actor.principal_id(), record.actor.request_id())
    });
    if repeated {
        return Err(corrupt("two handoffs share an id or a request"));
    }
    match &record.kind {
        HandoffKind::Switch { before, after } => {
            if &snapshot.provider != before {
                return Err(StorageError::IdentityMismatch);
            }
            if !switches(before, after) {
                return Err(corrupt(
                    "a switch keeps the provider name and model it leaves",
                ));
            }
        }
        HandoffKind::Fork { source, .. } => {
            if !snapshot.invocations.is_empty() || !snapshot.handoffs.is_empty() {
                return Err(corrupt(
                    "a fork is not the first thing a conversation holds",
                ));
            }
            if source == &snapshot.id {
                return Err(corrupt("a conversation is forked from itself"));
            }
        }
    }
    Ok(())
}

/// Whether going from `before` to `after` is a switch: another provider name
/// or another model. A change of context alone is
/// [`SessionChange::ProviderIdentityChanged`](super::SessionChange::ProviderIdentityChanged).
pub(crate) fn switches(before: &ProviderIdentity, after: &ProviderIdentity) -> bool {
    before.name() != after.name() || before.model_id() != after.model_id()
}

/// Whether the handoffs a saved snapshot holds keep the rules
/// [`admit_saved`] and [`admit_taken`] ask of each, as far as a snapshot,
/// which keeps no order between kinds of records, can show: within the
/// bound, distinct, each after a turn the conversation holds, a fork only
/// first, and each taken at most once by a turn saved after it.
pub(super) fn validate_saved(snapshot: &SessionSnapshot) -> Result<(), StorageError> {
    if snapshot.handoffs.len() > HandoffRecord::MAX_PER_CONVERSATION {
        return Err(corrupt("the conversation keeps more handoffs than it may"));
    }
    let position = |turn: &ExecutionId| {
        snapshot
            .invocations
            .iter()
            .position(|record| &record.request.execution_id == turn)
    };
    let mut ids = HashSet::new();
    let mut requests = HashSet::new();
    let mut follows = Vec::with_capacity(snapshot.handoffs.len());
    for (index, handoff) in snapshot.handoffs.iter().enumerate() {
        if handoff.at_ms > HandoffRecord::MAX_AT_MS {
            return Err(corrupt("a handoff is later than a record can state"));
        }
        if !ids.insert(handoff.id.as_str())
            || !requests.insert((handoff.actor.principal_id(), handoff.actor.request_id()))
        {
            return Err(corrupt("two handoffs share an id or a request"));
        }
        let after = match &handoff.after_execution {
            None => None,
            Some(turn) => Some(position(turn).ok_or_else(|| {
                corrupt("a handoff follows a turn the conversation does not hold")
            })?),
        };
        if let HandoffKind::Fork { source, .. } = &handoff.kind {
            if index != 0 || after.is_some() || source == &snapshot.id {
                return Err(corrupt(
                    "a fork is not the first thing a conversation holds",
                ));
            }
        }
        follows.push(after);
    }
    let mut taken = HashSet::new();
    for (index, record) in snapshot.invocations.iter().enumerate() {
        let Some(id) = &record.handoff else {
            continue;
        };
        let at = snapshot
            .handoffs
            .iter()
            .position(|handoff| &handoff.id == id)
            .ok_or_else(|| corrupt("a turn takes a handoff the conversation does not hold"))?;
        if follows[at].is_some_and(|after| after >= index) || !taken.insert(id) {
            return Err(corrupt("a turn takes a handoff that was not waiting"));
        }
    }
    Ok(())
}

fn corrupt(reason: &str) -> StorageError {
    StorageError::Corrupt(format!("handoff record: {reason}"))
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/handoffs.rs"]
mod tests;
