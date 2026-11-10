//! How one committed SDK record reads in a conversation view.
//!
//! The view's shapes are the protocol's (`nessa_protocol::conversation::view`);
//! reading an SDK record into one is this crate's, so the protocol does not
//! depend on the SDK. A conversion is [`ViewOf`] when it maps one value to one
//! view value, and a function when it needs more than one record.
use crate::view_of::{IntoView, ViewOf};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::view::*;
use nessa_sdk::{
    application::agent_execution::{
        providers::{
            CompactionReportingCapability, ElicitationForwardingCapability,
            IncomingElicitationCapability, ModelSwitchReportingCapability,
            NativeHookSuppressionCapability, OperationCapabilities, PermissionDeferralCapability,
            PermissionDenialCapability, PolicyCloseSessionCapability, PolicyEndTurnCapability,
            PreToolPolicyCapability, ProviderIdentity,
        },
        sessions::{
            ArtifactRecord, CommittedViewState, HandoffKind, HandoffRecord, NoteDelivery,
            NoteRecord, NoteRetirementReason, NoteState, SessionSnapshot,
        },
    },
    domain::agent_execution::{
        executions::{AgentSource, Initiator},
        permissions::PermissionEffect,
        prompts::{ImageReference, LinkedFile, MessageSender, UploadedFileReference, UserMessage},
    },
};

impl ViewOf<CommittedViewState> for ConversationTranscriptState {
    fn view_of(state: CommittedViewState) -> Self {
        match state {
            CommittedViewState::NotLoaded => Self::NotLoaded,
            CommittedViewState::Partial => Self::Partial,
            CommittedViewState::CompleteEmpty => Self::CompleteEmpty,
            CommittedViewState::Complete => Self::Complete,
            CommittedViewState::Stale => Self::Stale,
            CommittedViewState::Unknown => Self::Unknown,
        }
    }
}

/// The app that wrote `message`, or `None` when the person did.
pub fn message_app(message: &UserMessage) -> Option<ConversationMessageApp> {
    match message.sender() {
        MessageSender::Person | MessageSender::Internal | MessageSender::Agent(_) => None,
        MessageSender::App(app) => Some(ConversationMessageApp {
            execution_id: app.execution_id().as_str().into(),
            tool_id: app.tool_id().as_str().into(),
            server: app.tool().server().into(),
            tool: app.tool().tool().into(),
        }),
    }
}

/// Read an agent message's origin from the SDK's saved sender.
pub fn message_agent(message: &UserMessage) -> Option<ConversationAgentSource> {
    match message.sender() {
        MessageSender::Agent(source) => Some(source.into_view()),
        _ => None,
    }
}

impl ViewOf<&AgentSource> for ConversationAgentSource {
    fn view_of(source: &AgentSource) -> Self {
        Self {
            conversation_id: source.conversation().into(),
            execution_id: source.execution().as_str().into(),
        }
    }
}

impl ViewOf<&ImageReference> for ConversationAttachment {
    fn view_of(image: &ImageReference) -> Self {
        Self {
            digest: image.digest().to_string(),
            mime_type: image.media_type().as_str().into(),
            size: image.size(),
        }
    }
}

impl ViewOf<&LinkedFile> for ConversationLinkedFile {
    fn view_of(file: &LinkedFile) -> Self {
        Self {
            path: file.path().into(),
        }
    }
}

impl ViewOf<PermissionEffect> for ConversationPermissionOptionEffect {
    fn view_of(effect: PermissionEffect) -> Self {
        match effect {
            PermissionEffect::Allow => Self::Allow,
            PermissionEffect::Deny => Self::Deny,
        }
    }
}

impl ViewOf<OperationCapabilities> for ConversationAgentFeatures {
    fn view_of(value: OperationCapabilities) -> Self {
        Self {
            permission_denial: match value.permission_denial() {
                PermissionDenialCapability::Unknown => PermissionDenialSupport::Unknown,
                PermissionDenialCapability::Unsupported => PermissionDenialSupport::Unsupported,
                PermissionDenialCapability::SupportedForOfferedPermissionReviews => {
                    PermissionDenialSupport::SupportedForOfferedPermissionReviews
                }
            },
            native_hooks: if value.native_hooks_allowed() {
                NativeHooksChoice::On
            } else {
                NativeHooksChoice::Off
            },
            native_hook_suppression: match value.native_hook_suppression() {
                NativeHookSuppressionCapability::Unknown => NativeHookSuppressionSupport::Unknown,
                NativeHookSuppressionCapability::Unsupported => {
                    NativeHookSuppressionSupport::Unsupported
                }
                NativeHookSuppressionCapability::SupportedForUserConfiguredHooks => {
                    NativeHookSuppressionSupport::SupportedForUserConfiguredHooks
                }
            },
            compaction_reporting: match value.compaction_reporting() {
                CompactionReportingCapability::UnsupportedNotImplemented => {
                    CompactionReportingSupport::UnsupportedNotImplemented
                }
                CompactionReportingCapability::SupportedWithInvocationCorrelation => {
                    CompactionReportingSupport::SupportedWithInvocationCorrelation
                }
            },
            model_switch_reporting: match value.model_switch_reporting() {
                ModelSwitchReportingCapability::UnsupportedNotImplemented => {
                    ModelSwitchReportingSupport::UnsupportedNotImplemented
                }
                ModelSwitchReportingCapability::SupportedAfterValidatedSwitch => {
                    ModelSwitchReportingSupport::SupportedAfterValidatedSwitch
                }
            },
            permission_deferral: match value.permission_deferral() {
                PermissionDeferralCapability::UnsupportedNotImplemented => {
                    PermissionDeferralSupport::UnsupportedNotImplemented
                }
                PermissionDeferralCapability::Unsupported => PermissionDeferralSupport::Unsupported,
                PermissionDeferralCapability::SupportedWithNonterminalOutcome => {
                    PermissionDeferralSupport::SupportedWithNonterminalOutcome
                }
            },
            elicitation_forwarding: match value.elicitation_forwarding() {
                ElicitationForwardingCapability::Unknown => ElicitationForwardingSupport::Unknown,
                ElicitationForwardingCapability::Unsupported => {
                    ElicitationForwardingSupport::Unsupported
                }
                ElicitationForwardingCapability::SupportedWithCorrelatedRoundTrip => {
                    ElicitationForwardingSupport::SupportedWithCorrelatedRoundTrip
                }
            },
            pre_tool_policy: match value.pre_tool_policy() {
                PreToolPolicyCapability::Unknown => PreToolPolicySupport::Unknown,
                PreToolPolicyCapability::UnsupportedNotImplemented => {
                    PreToolPolicySupport::UnsupportedNotImplemented
                }
                PreToolPolicyCapability::SupportedAtPermissionGate => {
                    PreToolPolicySupport::SupportedAtPermissionGate
                }
            },
            policy_end_turn: match value.policy_end_turn() {
                PolicyEndTurnCapability::Unknown => PolicyEndTurnSupport::Unknown,
                PolicyEndTurnCapability::Unsupported => PolicyEndTurnSupport::Unsupported,
                PolicyEndTurnCapability::SupportedForCurrentInvocation => {
                    PolicyEndTurnSupport::SupportedForCurrentInvocation
                }
            },
            policy_close_session: match value.policy_close_session() {
                PolicyCloseSessionCapability::Unknown => PolicyCloseSessionSupport::Unknown,
                PolicyCloseSessionCapability::Unsupported => PolicyCloseSessionSupport::Unsupported,
                PolicyCloseSessionCapability::SupportedForSession => {
                    PolicyCloseSessionSupport::SupportedForSession
                }
            },
            incoming_elicitation: match value.incoming_elicitation() {
                IncomingElicitationCapability::Unknown => IncomingElicitationSupport::Unknown,
                IncomingElicitationCapability::Unsupported => {
                    IncomingElicitationSupport::Unsupported
                }
                IncomingElicitationCapability::SupportedWithCorrelatedRoundTrip => {
                    IncomingElicitationSupport::SupportedWithCorrelatedRoundTrip
                }
            },
        }
    }
}

impl ViewOf<&ArtifactRecord> for ConversationArtifact {
    fn view_of(record: &ArtifactRecord) -> Self {
        Self {
            execution_id: record.turn.as_ref().map(|turn| turn.as_str().into()),
            lease: record.lease.as_str().into(),
            name: record.name.as_str().into(),
            digest: record.file.digest().to_string(),
            mime_type: record.file.media_type().as_str().into(),
            size: record.file.size(),
        }
    }
}

/// `note` as `snapshot`, which holds it, says it stands.
pub fn note(snapshot: &SessionSnapshot, note: &NoteRecord) -> ConversationNote {
    let state = snapshot.note_state(note);
    ConversationNote {
        message_id: note.id.as_str().into(),
        text: note.text.as_str().into(),
        accepted_at_ms: note.accepted_at_ms,
        delivery: match note.delivery {
            NoteDelivery::RecordOnly => ConversationNoteDelivery::RecordOnly,
            NoteDelivery::NextTurn => ConversationNoteDelivery::NextTurn,
        },
        delivery_state: match &state {
            NoteState::Recorded => ConversationNoteState::Recorded,
            NoteState::Pending => ConversationNoteState::Pending,
            NoteState::Assigned(_) => ConversationNoteState::Assigned,
            NoteState::Retired(_) => ConversationNoteState::Retired,
        },
        execution_id: match &state {
            NoteState::Assigned(turn) => Some(turn.as_str().into()),
            _ => None,
        },
        retired_reason: match &state {
            NoteState::Retired(retirement) => Some(match retirement.reason {
                NoteRetirementReason::Expired => ConversationNoteRetiredReason::Expired,
                NoteRetirementReason::PermissionLost => {
                    ConversationNoteRetiredReason::PermissionLost
                }
            }),
            _ => None,
        },
        after_execution_id: note
            .after_execution
            .as_ref()
            .map(|turn| turn.as_str().into()),
        author: ConversationNoteAuthor {
            kind: match note.author {
                Initiator::Owner => ConversationNoteAuthorKind::Owner,
                Initiator::Grant(_) => ConversationNoteAuthorKind::Grant,
                Initiator::Agent(_) => ConversationNoteAuthorKind::Agent,
            },
            surface: note.actor.surface_id().into(),
            agent: match &note.author {
                Initiator::Agent(source) => Some(source.into_view()),
                _ => None,
            },
        },
    }
}

/// `handoff` as a view shows it.
pub fn handoff(handoff: &HandoffRecord) -> ConversationHandoff {
    let agent = |identity: &ProviderIdentity| ConversationHandoffAgent {
        agent: AgentId::of_provider(identity.name()).map(|agent| agent.name().into()),
        model: identity.model_id().into(),
    };
    let (kind, from, to, turns, omitted_turns) = match &handoff.kind {
        HandoffKind::Switch { before, after } => (
            ConversationHandoffKind::Switch,
            Some(agent(before)),
            Some(agent(after)),
            Vec::new(),
            0,
        ),
        HandoffKind::Fork { history, .. } => (
            ConversationHandoffKind::Fork,
            None,
            None,
            history
                .turns()
                .iter()
                .map(|turn| ConversationCarriedTurn {
                    agent: turn.agent_source().map(IntoView::into_view),
                    said: turn.said().map(Into::into),
                    reply: turn.reply().map(Into::into),
                })
                .collect(),
            history.omitted(),
        ),
    };
    ConversationHandoff {
        handoff_id: handoff.id.as_str().into(),
        kind,
        at_ms: handoff.at_ms,
        after_execution_id: handoff
            .after_execution
            .as_ref()
            .map(|turn| turn.as_str().into()),
        from,
        to,
        turns,
        omitted_turns,
    }
}

impl ViewOf<&UploadedFileReference> for ConversationUploadedFile {
    fn view_of(file: &UploadedFileReference) -> Self {
        Self {
            digest: file.digest().to_string(),
            mime_type: file.media_type().as_str().into(),
            size: file.size(),
            name: file.name().into(),
        }
    }
}
