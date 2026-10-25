//! `channel.create` and `channel.list`: the person's channels on the wire.
//!
//! ```text
//! channel.create ──▶ admitted as `conversation.write` (a paired session is refused)
//!   ──▶ params ──▶ ChannelRequest (each value checked; a bad one is invalid_request)
//!   ──▶ ConversationService::create_channel ──▶ { channel } | its ChannelErrorCode
//! channel.list ──▶ ConversationService::list_channels
//!   ──▶ the default channel first, then the stored ones ──▶ cut to one frame
//! ```
use super::socket::{failure, success};
use super::state::ProductRouteState;
use nessa_auth::application::session::AuthenticatedSession;
use crate::conversation::application::{ChannelError, ChannelListing, FolderRefusal};
use crate::conversation::domain::{
    Channel, ChannelDefaults, ChannelId, ChannelName, ChannelNotes, ChannelPath, ChannelRequest,
    ChannelResources,
};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
use nessa_protocol::product::generated::{
    ApprovalMode, Channel as WireChannel, ChannelCreateParams, ChannelCreateResult,
    ChannelErrorCode, ChannelListResult,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, MAX_PAYLOAD_BYTES};
use serde_json::json;
use std::path::PathBuf;

/// What the default channel is called in a list. It is never stored: its
/// folder is the gateway's configured workspace.
const DEFAULT_NAME: &str = "Conversations";

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(service) = state.conversations.as_ref() else {
        return failure(&frame.id, ChannelErrorCode::ConversationsNotConfigured.as_str());
    };
    match frame.method.as_str() {
        "channel.create" => {
            let Ok(params) = serde_json::from_value::<ChannelCreateParams>(frame.params) else {
                return failure(&frame.id, ChannelErrorCode::InvalidRequest.as_str());
            };
            let Some(request) = channel_request(&params) else {
                return failure(&frame.id, ChannelErrorCode::InvalidRequest.as_str());
            };
            let caller = super::conversation::caller(session, params.request_id.clone());
            match service
                .create_channel(caller, params.request_id, request)
                .await
            {
                Ok(channel) => success(
                    &frame.id,
                    &ChannelCreateResult {
                        channel: wire_channel(&channel),
                    },
                ),
                Err(error) => refused(&frame.id, error),
            }
        }
        "channel.list" => {
            if frame.params != json!({}) && !frame.params.is_null() {
                return failure(&frame.id, ChannelErrorCode::InvalidRequest.as_str());
            }
            let caller = super::conversation::caller(session, frame.id.clone());
            match service.list_channels(&caller).await {
                Ok(listing) => success(&frame.id, &fitted(listed(listing), &frame.id)),
                Err(error) => refused(&frame.id, error),
            }
        }
        _ => failure(&frame.id, "unknown_method"),
    }
}

fn channel_request(params: &ChannelCreateParams) -> Option<ChannelRequest> {
    let path = |text: &str| ChannelPath::new(PathBuf::from(text)).ok();
    Some(ChannelRequest {
        name: ChannelName::new(&params.name).ok()?,
        folder: match &params.folder {
            Some(folder) => Some(path(folder)?),
            None => None,
        },
        defaults: ChannelDefaults {
            agent: AgentId::parse(&params.agent)?,
            model: ConversationModelId::new(params.model.clone()).ok()?,
            mode: match params.approval_mode {
                None | Some(ApprovalMode::Ask) => ConversationApprovalMode::Ask,
                Some(ApprovalMode::Auto) => ConversationApprovalMode::Auto,
                Some(ApprovalMode::Full) => ConversationApprovalMode::Full,
            },
        },
        notes: ChannelNotes::new(params.notes.as_deref().unwrap_or("")).ok()?,
        resources: ChannelResources::new(
            params
                .resources
                .iter()
                .flatten()
                .map(|resource| path(resource))
                .collect::<Option<_>>()?,
        )
        .ok()?,
    })
}

fn wire_mode(mode: ConversationApprovalMode) -> ApprovalMode {
    match mode {
        ConversationApprovalMode::Ask => ApprovalMode::Ask,
        ConversationApprovalMode::Auto => ApprovalMode::Auto,
        ConversationApprovalMode::Full => ApprovalMode::Full,
    }
}

fn wire_channel(channel: &Channel) -> WireChannel {
    let defaults = channel.defaults();
    WireChannel {
        channel_id: channel.id().as_str().to_owned(),
        name: channel.name().as_str().to_owned(),
        folder: channel.folder().path.as_str().to_owned(),
        folder_made: channel.folder().made,
        agent: Some(defaults.agent.name().to_owned()),
        model: Some(defaults.model.as_str().to_owned()),
        approval_mode: wire_mode(defaults.mode),
        created_at_ms: channel.created_at_ms(),
    }
}

/// The default channel, then the stored ones. With no workspace configured
/// the default channel has no folder to show, and is left out.
fn listed(listing: ChannelListing) -> ChannelListResult {
    let default = listing.default_folder.map(|folder| WireChannel {
        channel_id: ChannelId::DEFAULT.to_owned(),
        name: DEFAULT_NAME.to_owned(),
        folder,
        folder_made: false,
        agent: None,
        model: None,
        approval_mode: ApprovalMode::Ask,
        created_at_ms: 0,
    });
    ChannelListResult {
        channels: default
            .into_iter()
            .chain(listing.channels.iter().map(wire_channel))
            .collect(),
        complete: true,
    }
}

/// As many channels as one frame holds, saying so when some were left out:
/// a response past the frame closes the socket.
fn fitted(mut list: ChannelListResult, request_id: &str) -> ChannelListResult {
    let empty = ChannelListResult {
        channels: Vec::new(),
        complete: false,
    };
    let Ok(empty) = success(request_id, &empty).to_wire_text() else {
        return list;
    };
    let mut size = empty.len();
    let mut keep = 0;
    for (index, row) in list.channels.iter().enumerate() {
        let Ok(row) = serde_json::to_string(row) else {
            break;
        };
        size += row.len() + usize::from(index > 0);
        if size > MAX_PAYLOAD_BYTES as usize {
            break;
        }
        keep += 1;
    }
    if keep < list.channels.len() {
        list.channels.truncate(keep);
        list.complete = false;
    }
    list
}

fn refused(request_id: &str, error: ChannelError) -> OutgoingMessage {
    let code = match error {
        ChannelError::InvalidInput => ChannelErrorCode::InvalidRequest,
        ChannelError::Forbidden => ChannelErrorCode::Forbidden,
        ChannelError::NotConfigured => ChannelErrorCode::ConversationsNotConfigured,
        ChannelError::Folder(FolderRefusal::NotFound) => ChannelErrorCode::ChannelFolderNotFound,
        ChannelError::Folder(FolderRefusal::NotADirectory) => {
            ChannelErrorCode::ChannelFolderNotADirectory
        }
        ChannelError::Folder(FolderRefusal::Refused) => ChannelErrorCode::ChannelFolderRefused,
        ChannelError::Folder(FolderRefusal::Unavailable) | ChannelError::Storage => {
            ChannelErrorCode::ChannelStorageUnavailable
        }
        ChannelError::ResourceNotFound => ChannelErrorCode::ChannelResourceNotFound,
        ChannelError::LimitReached => ChannelErrorCode::ChannelLimitReached,
        ChannelError::RequestConflict => ChannelErrorCode::ChannelRequestConflict,
        ChannelError::Unavailable => ChannelErrorCode::TemporarilyUnavailable,
    };
    tracing::info!(code = code.as_str(), "channel command refused");
    failure(request_id, code.as_str())
}

#[cfg(test)]
#[path = "../../tests/product/channel.rs"]
mod tests;
