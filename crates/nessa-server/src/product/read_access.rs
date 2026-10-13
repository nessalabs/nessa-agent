//! Share grants on the socket (issues 704, 707): the reader a session is, the
//! one question every socket read asks before it reads a conversation or a
//! list, the one admission of every socket request ([`admit_request`]), and
//! the owner's `conversation.share`, `.unshare` and `.shares` commands.
//! Every role reads alike; only a Drive command asks a grant's role.
//!
//! The rule is `conversation::application::share_grants`; this module only
//! asks it, and maps a share's role and policy between the wire and the
//! domain. `conversation.read` and every subscription batch
//! (`subscription::authorize_batch`) ask [`admit_conversation`];
//! `conversation.list`, `conversation.observe` and every list batch ask
//! [`admit_list`]. A gateway that pairs no device has no receiver binding, so
//! every session is the owner's and reads exactly as before (row G11).
use super::{
    conversation::{caller, conversation_id},
    socket::{admit_action, admit_grant, failure, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    admit_drive, admit_read, binding_of, error_code, reader_of, ConversationError, DriveRefusal,
    Reader, ShareConversation, ShareGrant,
};
use crate::conversation::domain::{ShareRole, ToolPolicy, ToolPreset, ToolRules};
use nessa_auth::{application::session::AuthenticatedSession, domain::CredentialId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::ReadRefusal;
use nessa_protocol::product::generated::{
    action_for_method, ConversationMutationResult, ConversationShare, ConversationShareParams,
    ConversationShareRole, ConversationSharesParams, ConversationSharesResult,
    ConversationToolPolicy, ConversationToolPreset, ConversationUnshareParams,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};

/// The reader `session` is now: the owner when this gateway keeps no
/// receiver bindings or the credential has none.
async fn reader(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<Reader, &'static str> {
    let Some((receivers, _, _)) = state.passive_read.as_ref() else {
        return Ok(Reader::Owner);
    };
    reader_of(receivers.as_ref(), session.context())
        .await
        .map_err(refusal_code)
}

/// Whether `session` may read conversation `id`. An ungranted id answers
/// `conversation_not_found`, as one that does not exist does (row G12).
pub(super) async fn admit_conversation(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    id: &ConversationId,
) -> Result<(), &'static str> {
    let reader = reader(state, session).await?;
    let Some((_, _, grants)) = state.passive_read.as_ref() else {
        return Ok(());
    };
    admit_read(grants.as_ref(), &reader, id)
        .await
        .map_err(refusal_code)
}

/// Whether `session` may ask for the owner's list. A paired device lists
/// through its catalogue, which the store narrows to its grants; the socket's
/// lists are the owner's own, so a device is refused them (row G12) rather
/// than handed a list filtered after it was read.
pub(super) async fn admit_list(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(), &'static str> {
    match reader(state, session).await? {
        Reader::Owner => Ok(()),
        Reader::PairedDevice { .. } => Err("forbidden"),
    }
}

/// The commands a Drive grant admits on the conversation it names (row J11):
/// start and steer a turn, stop it, answer or cancel its permission request,
/// answer its question, and look up those commands' receipts. Drive never
/// deletes, archives, exports, re-shares or changes the model (issue 707).
const DRIVE_METHODS: [&str; 7] = [
    "conversation.send",
    "conversation.steer",
    "conversation.stop",
    "conversation.answer",
    "conversation.answerQuestion",
    "conversation.cancel",
    "conversation.receipt",
];

/// The methods behind Cedar's `conversation.write` that a paired session
/// reaches only to be admitted again by its grants: the read and the view
/// subscription ask [`admit_conversation`], the lists ask [`admit_list`],
/// and an unsubscribe ends only what was admitted. Rows G12 and G13.
const ADMITTED_BY_ITS_OWN_CHECK: [&str; 6] = [
    "conversation.read",
    "conversation.list",
    "conversation.observe",
    "conversation.subscribe",
    "conversation.subscribeList",
    "conversation.unsubscribe",
];

/// The one admission of a socket request, after its session was found
/// current. The owner's surfaces, and every method outside Cedar's
/// `conversation.write`, are admitted by the method's grant as before.
///
/// A paired session holding `conversation.write` would otherwise act as its
/// owner, so for it every `conversation.write` method is refused unless it
/// is one its grants admit again ([`ADMITTED_BY_ITS_OWN_CHECK`]) or a Drive
/// command ([`DRIVE_METHODS`], row J12). A Drive command asks Cedar for the
/// grant pairing mints, `conversation.read`, and then the share grant on the
/// one conversation the command names, so a Drive grant is what lets the
/// device drive that conversation and no other (row J11). Its tools run
/// unchecked until the grant's policy is enforced (slice J4).
///
/// Admitted, a request is answered with who it is from: a turn it starts is
/// saved with the Drive grant that admitted it, its role and policy revision
/// as read for this command, and every other with the owner (row J15). Both
/// say the turn's tools run unchecked.
pub(super) async fn admit_request(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: &RequestFrame,
) -> Result<Initiation, &'static str> {
    let method = frame.method.as_str();
    let as_owner = || async {
        admit_action(state, session, method).await?;
        Ok(Initiation::owner())
    };
    let writes = action_for_method(method) == Some("conversation.write");
    if !writes || ADMITTED_BY_ITS_OWN_CHECK.contains(&method) {
        return as_owner().await;
    }
    let Some((receivers, _, grants)) = state.passive_read.as_ref() else {
        return as_owner().await;
    };
    let Some(binding) = binding_of(receivers.as_ref(), session.context())
        .await
        .map_err(refusal_code)?
    else {
        return as_owner().await;
    };
    // A device signs in as its owner, so its command is the owner's from
    // the device's surface. A peer gateway signs in as its own principal
    // and forwards its people's commands in a later slice.
    if !DRIVE_METHODS.contains(&method) || binding.owner_id != *session.context().principal_id() {
        return Err("forbidden");
    }
    let reader = Reader::of(Some(&binding));
    admit_grant(state, session, PAIRING_GRANT).await?;
    let id = drive_target(frame)?;
    let held = admit_drive(grants.as_ref(), &reader, &id)
        .await
        .map_err(|refusal| match refusal {
            DriveRefusal::NotGranted => refusal_code(ReadRefusal::WrongOwner),
            DriveRefusal::BelowDrive => "forbidden",
            DriveRefusal::Unverifiable => refusal_code(ReadRefusal::Unverifiable),
        })?
        // Only the owner holds no grant, and this reader is a paired device.
        .ok_or("forbidden")?;
    drives_only_text(frame)?;
    // A receiver id is at most 128 bytes, inside what a record keeps; one of
    // only blanks, which no pairing mints, is refused as unverifiable.
    // The turn names the grant and its revision; what that revision allowed is
    // the grant journal's to say, not a copy on the turn.
    GrantInitiator::new(binding.receiver_id, held.policy_revision)
        .map(Initiation::grant)
        .map_err(|_| refusal_code(ReadRefusal::Unverifiable))
}

/// The grant pairing mints a device (`device_pairing`), which a Drive
/// command asks Cedar for in place of `conversation.write`.
const PAIRING_GRANT: &str = "conversation.read";

/// The conversation a Drive command names. Its params are checked in full
/// by the handler; a command naming none, or no valid id, is refused here as
/// the handler would refuse it.
fn drive_target(frame: &RequestFrame) -> Result<ConversationId, &'static str> {
    frame
        .params
        .get("conversationId")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| conversation_id(id).ok())
        .ok_or(ConversationErrorCode::InvalidRequest.as_str())
}

/// A driver sends text. An image is named by a digest in the owner's
/// attachment store and a linked file by a path on the owner's machine, so
/// a driver could name what was never shared with it; both are refused, and
/// a receipt names only a command a driver may make (row J14).
fn drives_only_text(frame: &RequestFrame) -> Result<(), &'static str> {
    let non_empty = |name: &str| {
        frame
            .params
            .get(name)
            .is_some_and(|value| !value.is_null() && *value != serde_json::json!([]))
    };
    if non_empty("attachments") || non_empty("files") {
        return Err("forbidden");
    }
    if frame.method == "conversation.receipt"
        && frame
            .params
            .get("operation")
            .and_then(serde_json::Value::as_str)
            == Some("create")
    {
        return Err("forbidden");
    }
    Ok(())
}

/// `WrongOwner` is how an ungranted id is refused; to the socket that is a
/// conversation it cannot see.
fn refusal_code(refusal: ReadRefusal) -> &'static str {
    match refusal {
        ReadRefusal::WrongOwner => ConversationErrorCode::ConversationNotFound.as_str(),
        ReadRefusal::Unauthorized => "unauthorized",
        ReadRefusal::Unverifiable => ConversationErrorCode::TemporarilyUnavailable.as_str(),
        ReadRefusal::InvalidRequest
        | ReadRefusal::Forbidden
        | ReadRefusal::WrongReceiver
        | ReadRefusal::StaleEpoch => "forbidden",
    }
}

/// `conversation.share`, `.unshare` and `.shares`. The socket has already
/// asked Cedar for `credential.manage`.
pub(super) async fn dispatch_share(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some((receivers, conversations, grants)) = state.passive_read.as_ref() else {
        return failure(
            &frame.id,
            ConversationErrorCode::ConversationsNotConfigured.as_str(),
        );
    };
    let shares = ShareConversation {
        conversations: conversations.as_ref(),
        receivers: receivers.as_ref(),
        grants: grants.as_ref(),
        access: state.access.as_ref(),
        gateway: &state.gateway,
    };
    // Sharing is the owner's: a paired device signs in as its owner's
    // principal, so it is refused here by what it is, not only by the
    // grant its credential lacks (row G8).
    match reader(state, session).await {
        Ok(Reader::Owner) => {}
        Ok(Reader::PairedDevice { .. }) => return failure(&frame.id, "forbidden"),
        Err(code) => return failure(&frame.id, code),
    }
    let now = state.clock.unix_milliseconds();
    let result: Result<OutgoingMessage, ConversationError> = async {
        match frame.method.as_str() {
            "conversation.share" => {
                let params: ConversationShareParams = serde_json::from_value(frame.params)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let id = conversation_id(&params.conversation_id)?;
                let credential = CredentialId::new(params.credential_id)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let policy = params.policy.map(tool_policy).transpose()?;
                let caller = caller(session, params.request_id.clone());
                let applied = shares
                    .share(caller, id, credential, role(params.role), policy, now)
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied,
                    },
                ))
            }
            "conversation.unshare" => {
                let params: ConversationUnshareParams = serde_json::from_value(frame.params)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let id = conversation_id(&params.conversation_id)?;
                let credential = CredentialId::new(params.credential_id)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let caller = caller(session, params.request_id.clone());
                let applied = shares.unshare(caller, id, credential, now).await?;
                Ok(success(
                    &frame.id,
                    &ConversationMutationResult {
                        request_id: params.request_id,
                        applied,
                    },
                ))
            }
            "conversation.shares" => {
                let params: ConversationSharesParams = serde_json::from_value(frame.params)
                    .map_err(|_| ConversationError::InvalidInput)?;
                let id = conversation_id(&params.conversation_id)?;
                let items = shares
                    .shares(&caller(session, frame.id.clone()), &id)
                    .await?
                    .iter()
                    .map(listed)
                    .collect();
                Ok(success(&frame.id, &ConversationSharesResult { items }))
            }
            _ => Err(ConversationError::InvalidInput),
        }
    }
    .await;
    result.unwrap_or_else(|error| failure(&frame.id, error_code(&error).as_str()))
}

fn role(role: ConversationShareRole) -> ShareRole {
    match role {
        ConversationShareRole::Read => ShareRole::Read,
        ConversationShareRole::Comment => ShareRole::Comment,
        ConversationShareRole::Drive => ShareRole::Drive,
    }
}

fn wire_role(role: ShareRole) -> ConversationShareRole {
    match role {
        ShareRole::Read => ConversationShareRole::Read,
        ShareRole::Comment => ConversationShareRole::Comment,
        ShareRole::Drive => ConversationShareRole::Drive,
    }
}

fn preset(preset: ConversationToolPreset) -> ToolPreset {
    match preset {
        ConversationToolPreset::ReadOnlyTools => ToolPreset::ReadOnlyTools,
        ConversationToolPreset::NoShell => ToolPreset::NoShell,
        ConversationToolPreset::AskEverything => ToolPreset::AskEverything,
        ConversationToolPreset::All => ToolPreset::All,
    }
}

fn wire_preset(preset: ToolPreset) -> ConversationToolPreset {
    match preset {
        ToolPreset::ReadOnlyTools => ConversationToolPreset::ReadOnlyTools,
        ToolPreset::NoShell => ConversationToolPreset::NoShell,
        ToolPreset::AskEverything => ConversationToolPreset::AskEverything,
        ToolPreset::All => ConversationToolPreset::All,
    }
}

/// A wire policy as the domain's: `preset` alone, or `allow` and `deny`
/// together, each checked by `ToolRules::new`. Any other shape is
/// `InvalidInput` (row J3).
fn tool_policy(policy: ConversationToolPolicy) -> Result<ToolPolicy, ConversationError> {
    match (policy.preset, policy.allow, policy.deny) {
        (Some(named), None, None) => Ok(ToolPolicy::Preset(preset(named))),
        (None, Some(allow), Some(deny)) => ToolRules::new(allow, deny)
            .map(ToolPolicy::Rules)
            .map_err(|_| ConversationError::InvalidInput),
        _ => Err(ConversationError::InvalidInput),
    }
}

fn wire_policy(policy: &ToolPolicy) -> ConversationToolPolicy {
    match policy {
        ToolPolicy::Preset(named) => ConversationToolPolicy {
            preset: Some(wire_preset(*named)),
            allow: None,
            deny: None,
        },
        ToolPolicy::Rules(rules) => ConversationToolPolicy {
            preset: None,
            allow: Some(rules.allow().to_vec()),
            deny: Some(rules.deny().to_vec()),
        },
    }
}

/// One grant as `conversation.shares` lists it (row J7).
fn listed(grant: &ShareGrant) -> ConversationShare {
    ConversationShare {
        credential_id: grant.credential_id.as_str().to_owned(),
        role: wire_role(grant.access.role()),
        policy: grant.access.policy().map(wire_policy),
        policy_revision: grant.policy_revision,
        granted_at_ms: grant.granted_at_ms,
    }
}

#[cfg(test)]
#[path = "../../tests/product/read_access.rs"]
mod tests;
