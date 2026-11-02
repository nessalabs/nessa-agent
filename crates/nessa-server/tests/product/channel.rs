//! `channel.create` reads only a channel it can keep; `channel.list` puts
//! the default channel first and never answers past one frame; each refusal
//! has its own code.
use super::*;
use crate::conversation_test_support::absolute;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::protocol::ResponseFrame;
use serde_json::json;

fn params(value: serde_json::Value) -> ChannelCreateParams {
    serde_json::from_value(value).unwrap()
}

#[test]
fn a_create_reads_into_a_channel_request_with_ask_by_default() {
    let request = channel_request(&params(json!({
        "requestId": "r1",
        "name": "  design-review ",
        "agent": "codex",
        "model": "gpt-5.6-terra",
        "notes": "Check every export.",
        "resources": [absolute("/work/brand-kit"), absolute("/work/tokens.json")]
    })))
    .unwrap();
    assert_eq!(request.name.as_str(), "design-review");
    assert_eq!(request.folder, None);
    assert_eq!(request.defaults.agent, AgentId::Codex);
    assert_eq!(request.defaults.mode, ConversationApprovalMode::Ask);
    assert_eq!(request.resources.paths().len(), 2);
    let chosen = channel_request(&params(json!({
        "requestId": "r1", "name": "x", "agent": "claude", "model": "m",
        "folder": absolute("/work/repo"), "approvalMode": "full"
    })))
    .unwrap();
    assert_eq!(chosen.folder.unwrap().as_str(), absolute("/work/repo"));
    assert_eq!(chosen.defaults.mode, ConversationApprovalMode::Full);
}

#[test]
fn a_create_that_is_not_a_channel_is_refused_before_the_service() {
    for refused in [
        json!({"requestId": "r", "name": " ", "agent": "codex", "model": "m"}),
        json!({"requestId": "r", "name": "x", "agent": "nobody", "model": "m"}),
        json!({"requestId": "r", "name": "x", "agent": "codex", "model": "m", "folder": "relative"}),
        json!({"requestId": "r", "name": "x", "agent": "codex", "model": "m", "resources": [absolute("/a"), absolute("/a")]}),
        json!({"requestId": "r", "name": "x", "agent": "codex", "model": "m", "notes": "bell\u{7}"}),
    ] {
        assert!(
            channel_request(&params(refused.clone())).is_none(),
            "{refused}"
        );
    }
}

fn stored(n: u8, notes: &str) -> Channel {
    Channel::new(
        ChannelId::from_random([n; 10]),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        crate::conversation::domain::ChannelParts {
            execution: crate::conversation::domain::ChannelExecution::default(),
            name: ChannelName::new(&format!("channel-{n}")).unwrap(),
            folder: crate::conversation::domain::ChannelFolder {
                path: ChannelPath::new(PathBuf::from(absolute(&format!("/data/channels/{n}"))))
                    .unwrap(),
                made: true,
            },
            defaults: ChannelDefaults {
                agent: AgentId::Claude,
                model: ConversationModelId::new("claude-x").unwrap(),
                mode: ConversationApprovalMode::Auto,
            },
            notes: ChannelNotes::new(notes).unwrap(),
            resources: ChannelResources::new(vec![]).unwrap(),
        },
        format!("request-{n}"),
        u64::from(n),
    )
    .unwrap()
}

#[test]
fn a_list_starts_with_the_default_channel_and_lists_no_notes() {
    let list = listed(
        ChannelListing {
            default_folder: Some(absolute("/work/default")),
            channels: vec![stored(1, "private notes")],
        },
        None,
    );
    let wire = serde_json::to_value(&list).unwrap();
    assert_eq!(
        wire["channels"][0],
        json!({
            "channelId": "default", "name": "Conversations", "folder": absolute("/work/default"),
            "folderMade": false, "agent": null, "model": null, "approvalMode": "ask",
            "createdAtMs": 0,
            "coordinatorPlacement": "local", "threadPlacement": "local", "coordinatorConversationId": null
        })
    );
    assert_eq!(wire["channels"][1]["agent"], "claude");
    assert_eq!(wire["channels"][1]["approvalMode"], "auto");
    assert!(!wire.to_string().contains("private notes"));
    assert!(list.complete);
    let without = listed(
        ChannelListing {
            default_folder: None,
            channels: vec![],
        },
        None,
    );
    assert!(without.channels.is_empty());
}

/// The `index`th of many channels, each with a folder near the longest a
/// path may be.
fn long(index: u16) -> Channel {
    let mut bytes = [0; 10];
    bytes[0] = (index & 0x1f) as u8;
    bytes[1] = ((index >> 5) & 0x1f) as u8;
    Channel::new(
        ChannelId::from_random(bytes),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        crate::conversation::domain::ChannelParts {
            execution: crate::conversation::domain::ChannelExecution::default(),
            name: ChannelName::new(&format!("channel-{index}")).unwrap(),
            folder: crate::conversation::domain::ChannelFolder {
                path: ChannelPath::new(PathBuf::from(absolute(&format!("/{}", "f".repeat(4000)))))
                    .unwrap(),
                made: false,
            },
            defaults: ChannelDefaults {
                agent: AgentId::Claude,
                model: ConversationModelId::new("claude-x").unwrap(),
                mode: ConversationApprovalMode::Auto,
            },
            notes: ChannelNotes::new("").unwrap(),
            resources: ChannelResources::new(vec![]).unwrap(),
        },
        format!("request-{index}"),
        u64::from(index),
    )
    .unwrap()
}

#[test]
fn a_list_longer_than_a_frame_is_paged_and_every_channel_is_listed_once() {
    let channels: Vec<Channel> = (0..1000).map(long).collect();
    let listing = || ChannelListing {
        default_folder: Some(absolute("/work/default")),
        channels: channels.clone(),
    };
    let mut seen = Vec::new();
    let mut after: Option<ChannelId> = None;
    let mut pages = 0;
    loop {
        let page = fitted(listed(listing(), after.as_ref()), "request");
        let wire = success("request", &page).to_wire_text().unwrap();
        assert!(wire.len() <= MAX_PAYLOAD_BYTES as usize);
        assert_eq!(page.complete, page.cursor.is_none());
        assert_eq!(
            page.channels.first().map(|c| c.channel_id == "default"),
            Some(pages == 0)
        );
        seen.extend(page.channels.iter().map(|c| c.channel_id.clone()));
        pages += 1;
        match page.cursor {
            Some(cursor) => {
                assert_eq!(Some(&cursor), seen.last());
                after = Some(ChannelId::new(&cursor).unwrap());
            }
            None => break,
        }
    }
    assert!(pages > 1);
    let expected: Vec<String> = std::iter::once("default".to_owned())
        .chain(channels.iter().map(|c| c.id().as_str().to_owned()))
        .collect();
    assert_eq!(seen, expected);
    // After the last channel, nothing more.
    let last = channels.last().unwrap().id().clone();
    let page = fitted(listed(listing(), Some(&last)), "request");
    assert!(page.channels.is_empty() && page.complete);
}

fn code(message: OutgoingMessage) -> String {
    match message {
        OutgoingMessage::Response(ResponseFrame { error, .. }) => error.unwrap().code,
        OutgoingMessage::Event(_) => panic!("expected a response"),
    }
}

#[test]
fn each_refusal_has_its_own_code() {
    for (error, expected) in [
        (
            ChannelError::Folder(FolderRefusal::NotFound),
            "channel_folder_not_found",
        ),
        (
            ChannelError::Folder(FolderRefusal::NotADirectory),
            "channel_folder_not_a_directory",
        ),
        (
            ChannelError::Folder(FolderRefusal::Refused),
            "channel_folder_refused",
        ),
        (
            ChannelError::Folder(FolderRefusal::Unavailable),
            "channel_storage_unavailable",
        ),
        (ChannelError::ResourceNotFound, "channel_resource_not_found"),
        (ChannelError::LimitReached, "channel_limit_reached"),
        (ChannelError::RequestConflict, "channel_request_conflict"),
        (ChannelError::Forbidden, "forbidden"),
        (ChannelError::InvalidInput, "invalid_request"),
        (ChannelError::Unavailable, "temporarily_unavailable"),
    ] {
        assert_eq!(code(refused("r", error)), expected);
    }
}

#[test]
fn a_placement_change_names_a_channel_and_two_known_places() {
    let (params, channel, execution) = placement_request(json!({
        "requestId": "r", "channelId": "ch_aaaaaaaaaa",
        "coordinatorPlacement": "boat", "threadPlacement": "local"
    }))
    .unwrap();
    assert_eq!(params.request_id, "r");
    assert_eq!(channel.as_str(), "ch_aaaaaaaaaa");
    assert_eq!(execution.coordinator, WorkspacePlacement::Boat);
    assert_eq!(execution.threads, WorkspacePlacement::Local);
    for refused in [
        json!({"requestId": "r", "channelId": "nope", "coordinatorPlacement": "boat", "threadPlacement": "boat"}),
        json!({"requestId": "r", "channelId": "ch_aaaaaaaaaa", "coordinatorPlacement": "moon", "threadPlacement": "boat"}),
        json!({"requestId": "r", "channelId": "ch_aaaaaaaaaa", "coordinatorPlacement": "boat"}),
        json!({"requestId": "r", "channelId": "ch_aaaaaaaaaa", "coordinatorPlacement": "boat", "threadPlacement": "Boat"}),
    ] {
        assert!(placement_request(refused.clone()).is_none(), "{refused}");
    }
}

#[test]
fn a_placement_change_asks_to_write_to_conversations() {
    assert_eq!(
        nessa_protocol::product::generated::action_for_method("channel.setPlacement"),
        Some("conversation.write")
    );
}
