//! `conversation.shares` on the wire (`product::read_access`): row J10 of
//! `docs/design/read-grants.md`.
use super::share_page;
use crate::conversation::application::{
    ShareGrant, MAX_DRIVE_GRANTS_PER_CONVERSATION, MAX_POLICY_REVISION,
    MAX_SHARE_GRANTS_PER_CONVERSATION,
};
use crate::conversation::domain::{Capabilities, CommandHosts, ShareAccess, ToolPolicy, ToolRules};
use nessa_auth::domain::{CredentialId, MAX_IDENTIFIER_BYTES};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    ConversationSharesCursor, ConversationSharesResult, MAX_PAYLOAD_BYTES, MAX_SHARE_PAGE,
};
use nessa_protocol::protocol::ResponseFrame;

fn grant(n: i64, access: ShareAccess) -> ShareGrant {
    let longest = |bytes: usize, n: i64| {
        let tag = format!("{n:02}");
        format!("{tag}{}", "\"".repeat(bytes - tag.len()))
    };
    let id = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    ShareGrant {
        conversation_id: id,
        receiver_id: format!("receiver-{n}"),
        credential_id: CredentialId::new(longest(MAX_IDENTIFIER_BYTES, n)).unwrap(),
        access,
        policy_revision: u64::MAX >> 11,
        granted_at_ms: u64::MAX >> 11,
    }
}

/// The largest policy a Drive grant can hold: both pattern lists full, every
/// pattern the character JSON spells longest, and every command host at its
/// longest.
fn largest_policy() -> ToolPolicy {
    let pattern = "\"".repeat(ToolRules::MAX_RULE_BYTES);
    let full = ToolRules::MAX_RULES;
    let hosts = CommandHosts::new(
        (0..CommandHosts::MAX_HOSTS)
            .map(|n| format!("{n:x}{}", "h".repeat(CommandHosts::MAX_HOST_BYTES - 1)))
            .collect(),
    )
    .unwrap();
    ToolPolicy::rules(ToolRules::new(vec![pattern.clone(); full], vec![pattern; full]).unwrap())
        .with_command_hosts(hosts)
}

/// Row J10: one `conversation.shares` page of grants at the largest policy,
/// every command host at its longest, every identifier and pattern at its
/// longest, the cursor naming the last of them, the catalogue revision at
/// the largest safe integer, and the longest request id, still fits the
/// frame a client takes. A page is not every grant a
/// conversation may hold.
#[test]
fn a_full_page_of_largest_grants_fits_one_frame() {
    let drive = ShareAccess::new(Capabilities::drive(), Some(largest_policy())).unwrap();
    // One past the page, so the answer carries a cursor: the largest frame.
    let grants: Vec<_> = (0..=MAX_SHARE_PAGE as i64)
        .map(|n| grant(n, drive.clone()))
        .collect();
    let page = share_page(&grants, None, MAX_SHARE_PAGE);
    assert_eq!(page.items.len(), MAX_SHARE_PAGE);
    assert_eq!(
        page.cursor
            .as_ref()
            .map(|cursor| cursor.credential_id.as_str()),
        Some(page.items.last().unwrap().credential_id.as_str()),
        "a full page that is not the last names its last grant"
    );
    let request = "\"".repeat(256);
    let frame = ResponseFrame::success(
        &request,
        &ConversationSharesResult {
            items: page.items,
            cursor: page.cursor,
            change_revision: MAX_POLICY_REVISION,
        },
    )
    .unwrap();
    let text = serde_json::to_string(&frame).unwrap();
    assert!(
        text.len() <= MAX_PAYLOAD_BYTES as usize,
        "{} bytes, past {MAX_PAYLOAD_BYTES}",
        text.len()
    );
}

/// A page resumes after the cursor in oldest-first order, and names no
/// cursor once the list is finished. A cursor that matches no grant is still
/// a position: the grants after that position are the page.
#[test]
fn a_shares_page_resumes_after_its_cursor() {
    let comment = ShareAccess::new(Capabilities::comment(), None).unwrap();
    let mut grants: Vec<_> = (0..5).map(|n| grant(n, comment.clone())).collect();
    // Two share a time, so the credential orders them. `grant` stamps every
    // time at the maximum; set them apart except for that pair.
    for (n, grant) in grants.iter_mut().enumerate() {
        grant.granted_at_ms = if n < 2 { 10 } else { 20 + n as u64 };
    }
    let first = share_page(&grants, None, 2);
    assert_eq!(first.items.len(), 2);
    assert_eq!(
        first
            .cursor
            .as_ref()
            .map(|cursor| cursor.credential_id.as_str()),
        Some(first.items[1].credential_id.as_str())
    );
    let second = share_page(&grants, first.cursor.as_ref(), 2);
    assert_eq!(second.items.len(), 2);
    assert_ne!(second.items[0].credential_id, first.items[0].credential_id);
    let last = share_page(&grants, second.cursor.as_ref(), 2);
    assert_eq!(last.items.len(), 1);
    assert!(last.cursor.is_none());
    let between = ConversationSharesCursor {
        granted_at_ms: 10,
        credential_id: "m".to_owned(),
    };
    let after = share_page(&grants, Some(&between), 8);
    assert!(after
        .items
        .iter()
        .all(|item| item.granted_at_ms > 10 || item.credential_id.as_str() > "m"));
    assert!(!after.items.is_empty());
    // A cursor past every grant is still a position: the page is empty and
    // finished, so the reader does not ask again.
    let past = ConversationSharesCursor {
        granted_at_ms: u64::MAX,
        credential_id: "z".into(),
    };
    let done = share_page(&grants, Some(&past), 8);
    assert!(done.items.is_empty());
    assert!(done.cursor.is_none());
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
    assert!(definitions["ConversationToolPolicy"]
        .get("x-maxEntries")
        .is_none());
    let page = MAX_SHARE_PAGE as u64;
    assert_eq!(
        definitions["ConversationSharesParams"]["properties"]["pageSize"]["maximum"].as_u64(),
        Some(page)
    );
    assert_eq!(
        definitions["ConversationSharesResult"]["properties"]["items"]["maxItems"].as_u64(),
        Some(page)
    );
    assert_eq!(
        definitions["ConversationSharesResult"]["x-maxGrants"].as_i64(),
        Some(MAX_SHARE_GRANTS_PER_CONVERSATION)
    );
    assert_eq!(
        definitions["ConversationSharesResult"]["properties"]["changeRevision"]["maximum"].as_u64(),
        Some(MAX_POLICY_REVISION)
    );
    // The page is what fits the frame. The conversation may hold more grants
    // than one page, Drive ones included.
    const {
        assert!(MAX_SHARE_GRANTS_PER_CONVERSATION > MAX_SHARE_PAGE as i64);
        assert!(MAX_DRIVE_GRANTS_PER_CONVERSATION <= MAX_SHARE_GRANTS_PER_CONVERSATION);
    }
}
