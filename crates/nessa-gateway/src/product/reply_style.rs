//! `replyStyle.read` and `replyStyle.set`: how agents talk, for the caller's
//! Nessa level or one channel (issue #1382). The method's grant admits the
//! Nessa level; a channel's own choice is admitted by Cedar's
//! channel-settings action first, as the pool's per-channel preference is.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::conversation::{
    application::ConversationError,
    domain::{ChannelId, ReplyStyle, ReplyStyleSettings, RoleStyles, StyleRole},
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::product::generated::{
    ReplyStyleChoice, ReplyStyleEffective, ReplyStyleMode, ReplyStyleReadParams, ReplyStyleRole,
    ReplyStyleRoles, ReplyStyleSelection, ReplyStyleSetParams, ReplyStyles,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(service) = &state.conversation.service else {
        return failure(&frame.id, "reply_style_unavailable");
    };
    // An optional field is left out, never sent as null.
    if ["channelId", "text"].iter().any(|field| {
        frame
            .params
            .get(*field)
            .is_some_and(serde_json::Value::is_null)
    }) {
        return failure(&frame.id, "invalid_request");
    }
    let caller = super::conversation::caller(session, frame.id.clone());
    let channel = |id: Option<String>| match id {
        None => Ok(None),
        Some(id) => ChannelId::new(&id).map(Some).map_err(|_| ()),
    };
    let result = match frame.method.as_str() {
        "replyStyle.read" => {
            let Ok(params) = serde_json::from_value::<ReplyStyleReadParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(channel) = channel(params.channel_id) else {
                return failure(&frame.id, "invalid_request");
            };
            service.reply_styles(&caller, channel).await
        }
        "replyStyle.set" => {
            let Ok(params) = serde_json::from_value::<ReplyStyleSetParams>(frame.params) else {
                return failure(&frame.id, "invalid_request");
            };
            let Ok(channel) = channel(params.channel_id) else {
                return failure(&frame.id, "invalid_request");
            };
            let choice = match (params.mode, params.text) {
                (ReplyStyleSelection::Default, None) => None,
                (ReplyStyleSelection::Plain, None) => Some(ReplyStyle::Plain),
                (ReplyStyleSelection::Off, None) => Some(ReplyStyle::Off),
                (ReplyStyleSelection::Custom, Some(text)) => match ReplyStyle::custom(&text) {
                    Some(style) => Some(style),
                    None => return failure(&frame.id, "invalid_request"),
                },
                _ => return failure(&frame.id, "invalid_request"),
            };
            service
                .set_reply_style(&caller, channel, role(params.role), choice)
                .await
        }
        _ => return failure(&frame.id, "invalid_request"),
    };
    match result {
        Ok(settings) => success(&frame.id, &wire(&settings)),
        Err(ConversationError::ChannelNotFound) => failure(&frame.id, "channel_not_found"),
        Err(_) => failure(&frame.id, "reply_style_unavailable"),
    }
}

fn role(role: ReplyStyleRole) -> StyleRole {
    match role {
        ReplyStyleRole::Coordinator => StyleRole::Coordinator,
        ReplyStyleRole::Thread => StyleRole::Thread,
    }
}

fn choice(style: &ReplyStyle) -> ReplyStyleChoice {
    match style {
        ReplyStyle::Plain => ReplyStyleChoice {
            mode: ReplyStyleMode::Plain,
            text: None,
        },
        ReplyStyle::Off => ReplyStyleChoice {
            mode: ReplyStyleMode::Off,
            text: None,
        },
        ReplyStyle::Custom(text) => ReplyStyleChoice {
            mode: ReplyStyleMode::Custom,
            text: Some(text.as_str().to_owned()),
        },
    }
}

fn roles(styles: &RoleStyles) -> ReplyStyleRoles {
    ReplyStyleRoles {
        coordinator: styles.get(StyleRole::Coordinator).map(choice),
        thread: styles.get(StyleRole::Thread).map(choice),
    }
}

fn wire(settings: &ReplyStyleSettings) -> ReplyStyles {
    let channel = settings.scope().channel();
    ReplyStyles {
        channel_id: channel.map(|channel| channel.as_str().to_owned()),
        nessa: roles(settings.nessa()),
        channel: channel.map(|_| roles(settings.channel())),
        effective: ReplyStyleEffective {
            coordinator: choice(&settings.effective(StyleRole::Coordinator)),
            thread: choice(&settings.effective(StyleRole::Thread)),
        },
    }
}
