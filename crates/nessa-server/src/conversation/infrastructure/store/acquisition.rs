//! Bounded SQLite text selection before UTF-8 conversion and owned acquisition.
use crate::conversation::domain::Conversation;
use nessa_auth::domain::MAX_IDENTIFIER_BYTES;
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationModelId, ConversationPreview, ConversationTitle,
};
use nessa_sync::replication::domain::MAX_ID_BYTES;

/// SQL names are fixed by this adapter, never supplied by a caller.
pub(super) fn text(column: &str, utf8_max: usize, nullable: bool) -> String {
    let null = if nullable {
        format!("WHEN {column} IS NULL THEN NULL ")
    } else {
        String::new()
    };
    format!(
        "CASE {null}WHEN typeof({column}) = 'text' AND octet_length({column}) <= {} THEN {column} ELSE X'' END",
        utf8_max * 2,
    )
}

pub(super) fn conversation(prefix: &str) -> String {
    let column = |name: &str| format!("{prefix}{name}");
    let agent = column("agent");
    let agent_max = AgentId::ALL
        .iter()
        .map(|agent| agent.name().len())
        .max()
        .unwrap_or(0);
    let mode_max = ConversationApprovalMode::ALL
        .iter()
        .map(|mode| mode.as_str().len())
        .max()
        .unwrap_or(0);
    [
        text(&column("id"), MAX_ID_BYTES, false),
        text(&column("organization"), MAX_IDENTIFIER_BYTES, false),
        text(&column("owner"), MAX_IDENTIFIER_BYTES, false),
        text(&column("creator_surface"), Conversation::MAX_CREATOR_CONTEXT_BYTES, false),
        text(&column("creation_action"), Conversation::MAX_CREATOR_CONTEXT_BYTES, false),
        column("creation_requested_at_ms"),
        // Overlong text cannot name a supported agent. Preserve unsupported
        // semantics without acquiring the original text; wrong types refuse.
        format!("CASE WHEN typeof({agent}) != 'text' THEN X'' WHEN octet_length({agent}) > {} THEN NULL ELSE {agent} END", agent_max * 2),
        text(&column("model"), ConversationModelId::MAX_BYTES, false),
        text(&column("approval_mode"), mode_max, false),
    ].join(", ")
}

pub(super) fn summary(prefix: &str) -> String {
    [
        text(
            &format!("{prefix}title"),
            ConversationTitle::MAX_CHARS * char::MAX.len_utf8(),
            true,
        ),
        text(
            &format!("{prefix}preview"),
            ConversationPreview::MAX_BYTES,
            true,
        ),
        format!("{prefix}updated_at_ms"),
        format!("{prefix}archived"),
    ]
    .join(", ")
}

#[cfg(test)]
#[path = "../../../../tests/conversation/store_acquisition.rs"]
mod tests;
