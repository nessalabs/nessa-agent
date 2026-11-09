//! Evidence of a person's change to a gateway-wide setting made from
//! Settings: no longer always allowing an app, and moving where a channel's
//! agents run.
//!
//! Each change is recorded twice under one operation id: `Requested`,
//! before anything changes (a failure refuses the change), and then what came
//! of it. A change applied whose outcome cannot be recorded answers a failure,
//! so no success goes unrecorded. A change that failed answers its own cause;
//! the outcome record's failure is only logged then, so neither cause is lost.
use super::{ConversationCaller, ConversationFuture};
use crate::conversation::domain::{ChannelExecution, ChannelId};
use nessa_protocol::conversation::domain::ConversationId;

/// What the person asked to change, with what it was before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsChange {
    /// Stop always allowing `server`'s app ("Always allow this app").
    AppUntrust { server: String },
    /// Where `channel`'s agents run, from `before` to `after`, while
    /// `coordinator` was its main conversation.
    ChannelPlacement {
        channel: ChannelId,
        before: ChannelExecution,
        after: ChannelExecution,
        coordinator: ConversationId,
    },
}

/// One step of a settings change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsOutcome {
    /// Asked; recorded before anything changes.
    Requested,
    /// Applied. `changed` is false when there was nothing to change (an app
    /// not always allowed). `coordinator` is a channel's main conversation
    /// afterwards: a fresh one when the move retired the old.
    Applied {
        changed: bool,
        coordinator: Option<ConversationId>,
    },
    /// Nothing changed: the store refused it or could not be written.
    Failed,
}

/// One record: who asked, under which request, for what, and what came of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsAuditRecord {
    /// The gateway's own id for this change, shared by its records: the
    /// caller's request id may come again.
    pub operation_id: String,
    pub caller: ConversationCaller,
    pub change: SettingsChange,
    pub outcome: SettingsOutcome,
}

/// Commits each record before the step it records is reported.
pub trait SettingsAudit: Send + Sync {
    fn record(&self, record: SettingsAuditRecord) -> ConversationFuture<'_, ()>;
}
