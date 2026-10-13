//! `conversation.shares` on the wire (`product::read_access`): row J10 of
//! `docs/design/read-grants.md`.
use super::listed;
use crate::conversation::application::{
    ShareGrant, MAX_DRIVE_GRANTS_PER_CONVERSATION, MAX_SHARE_GRANTS_PER_CONVERSATION,
};
use crate::conversation::domain::{CommandHosts, ShareAccess, ShareRole, ToolPolicy, ToolRules};
use nessa_auth::domain::{CredentialId, MAX_IDENTIFIER_BYTES};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{ConversationSharesResult, MAX_PAYLOAD_BYTES};
use nessa_protocol::protocol::ResponseFrame;

/// Row J10: a `conversation.shares` answer with every grant a conversation
/// may hold, the Drive ones at their bound, every identifier and pattern at
/// its longest and made of the character JSON spells longest, every command
/// host at its longest in place of any pattern, and the longest request id,
/// still fits the frame a client takes, whichever way a policy spends its
/// entries.
#[test]
fn the_largest_shares_answer_fits_one_frame() {
    // A quote is the longest character a JSON string may hold that the
    // domain accepts: control characters are refused, and a quote is
    // written as two bytes.
    let longest = |bytes: usize, n: i64| {
        let tag = format!("{n:02}");
        format!("{tag}{}", "\"".repeat(bytes - tag.len()))
    };
    let pattern = "\"".repeat(ToolRules::MAX_RULE_BYTES);
    let rules = |allow: usize, deny: usize| {
        ToolPolicy::rules(
            ToolRules::new(vec![pattern.clone(); allow], vec![pattern.clone(); deny]).unwrap(),
        )
    };
    let hosts = CommandHosts::new(
        (0..CommandHosts::MAX_HOSTS)
            .map(|n| format!("{n:x}{}", "h".repeat(CommandHosts::MAX_HOST_BYTES - 1)))
            .collect(),
    )
    .unwrap();
    let full = ToolRules::MAX_RULES;
    let policies = [
        rules(full, full),
        rules(full, 0).with_command_hosts(hosts.clone()).unwrap(),
        rules(full / 2, full / 2).with_command_hosts(hosts).unwrap(),
    ];
    let comment = ShareAccess::new(ShareRole::Comment, None).unwrap();
    let id = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    for policy in policies {
        let drive = ShareAccess::new(ShareRole::Drive, Some(policy)).unwrap();
        let items = (0..MAX_SHARE_GRANTS_PER_CONVERSATION)
            .map(|n| {
                let access = if n < MAX_DRIVE_GRANTS_PER_CONVERSATION {
                    drive.clone()
                } else {
                    comment.clone()
                };
                listed(&ShareGrant {
                    conversation_id: id.clone(),
                    receiver_id: format!("receiver-{n}"),
                    credential_id: CredentialId::new(longest(MAX_IDENTIFIER_BYTES, n)).unwrap(),
                    access,
                    policy_revision: u64::MAX >> 11,
                    granted_at_ms: u64::MAX >> 11,
                })
            })
            .collect();
        let request = "\"".repeat(256);
        let frame = ResponseFrame::success(&request, &ConversationSharesResult { items }).unwrap();
        let text = serde_json::to_string(&frame).unwrap();
        assert!(
            text.len() <= MAX_PAYLOAD_BYTES as usize,
            "{} bytes, past {MAX_PAYLOAD_BYTES}",
            text.len()
        );
    }
}

/// The published schema states the bounds the domain enforces, so a client
/// refusing early refuses exactly what the gateway would.
#[test]
fn the_schema_states_the_share_bounds_the_domain_enforces() {
    let schema: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    )))
    .unwrap();
    let definitions = &schema["$defs"];
    let max = ToolRules::MAX_RULES as u64;
    let bytes = ToolRules::MAX_RULE_BYTES as u64;
    // Each list is bounded on its own, as `ToolRules::new` checks it.
    for list in ["allow", "deny"] {
        let property = &definitions["ConversationToolPolicy"]["properties"][list];
        assert_eq!(property["maxItems"].as_u64(), Some(max), "{list}");
        assert_eq!(
            property["items"]["x-utf8MaxBytes"].as_u64(),
            Some(bytes),
            "{list}"
        );
        assert_eq!(
            property["items"]["maxLength"].as_u64(),
            Some(bytes),
            "{list}"
        );
    }
    let hosts = &definitions["ConversationToolPolicy"]["properties"]["commandHosts"];
    assert_eq!(
        hosts["maxItems"].as_u64(),
        Some(CommandHosts::MAX_HOSTS as u64)
    );
    let host_bytes = CommandHosts::MAX_HOST_BYTES as u64;
    assert_eq!(hosts["items"]["maxLength"].as_u64(), Some(host_bytes));
    assert_eq!(hosts["items"]["x-utf8MaxBytes"].as_u64(), Some(host_bytes));
    assert_eq!(
        definitions["ConversationSharesResult"]["properties"]["items"]["maxItems"].as_i64(),
        Some(MAX_SHARE_GRANTS_PER_CONVERSATION)
    );
}
