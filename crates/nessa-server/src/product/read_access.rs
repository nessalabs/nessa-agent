//! Share grants on the socket (issues 704, 707): the paired party a session
//! is, the one question every socket read asks before it reads a
//! conversation or a list, the one admission of every socket request
//! ([`admit_request`]), and the owner's `conversation.share`, `.unshare` and
//! `.shares` commands.
//!
//! What a paired party may do is the conversation access policy's
//! (`ConversationAccessPolicyEvaluator`, ADR 803 step 3): this module names
//! the action a method is ([`paired_action`]), reads the facts the policy is
//! asked with, and maps its answer to a wire code. It also maps a share's
//! role and policy between the wire and the domain. `conversation.read`,
//! `artifact.read` and every view subscription batch
//! (`subscription::authorize_batch`) ask [`admit_conversation`];
//! `conversation.list`, `conversation.observe` and every list batch ask
//! [`admit_list`]. A gateway that pairs no device has no receiver binding, so
//! every session is the owner's and reads exactly as before (row G11).
//!
//! ```text
//! paired session ──▶ binding_of (identity, binding lifecycle)
//!   ──▶ paired_facts: the conversation, the grant its receiver holds
//!   ──▶ PairedAccess::admit(action, conversation, role, request shape)
//!         ──▶ Ok | NotFound | Deleted | Forbidden | Unavailable ──▶ wire code
//! ```
use super::{
    conversation::{caller, conversation_id},
    socket::{admit_action, admit_grant, failure, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    binding_of, error_code, paired_facts, AccessAction, ConversationAccessPolicyEvaluator,
    ConversationError, PairedAccess, PairedParty, PairedRefusal, ReceiverBinding, RequestShape,
    ShareConversation, ShareGrant, MAX_POLICY_REVISION,
};
use crate::conversation::domain::{
    CommandHosts, ShareRole, ToolChoice, ToolPolicy, ToolPreset, ToolRules,
};
use nessa_auth::{application::session::AuthenticatedSession, domain::CredentialId};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::conversation::read_scope::ReadRefusal;
use nessa_protocol::product::generated::{
    action_for_method, ConversationMutationResult, ConversationShare, ConversationShareParams,
    ConversationShareResult, ConversationShareRole, ConversationSharesCursor,
    ConversationSharesParams, ConversationSharesResult, ConversationToolPolicy,
    ConversationToolPreset, ConversationUnshareParams, MAX_SHARE_PAGE,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiation};

/// The paired party `session` is now, by its receiver binding: `None` for
/// the owner's own surface, or when this gateway keeps no receiver bindings.
async fn paired_binding(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<Option<ReceiverBinding>, &'static str> {
    let Some((receivers, _, _)) = state.passive_read.as_ref() else {
        return Ok(None);
    };
    binding_of(receivers.as_ref(), session.context())
        .await
        .map_err(refusal_code)
}

/// The access policy a paired party is asked of. It is bound with the
/// receiver bindings; a route holding one without the other cannot decide.
fn access_policy(
    state: &ProductRouteState,
) -> Result<&dyn ConversationAccessPolicyEvaluator, &'static str> {
    state
        .conversation_access
        .as_deref()
        .ok_or(refusal_code(ReadRefusal::Unverifiable))
}

/// `binding`'s party, signed in as `session`'s principal.
fn party<'a>(binding: &'a ReceiverBinding, session: &'a AuthenticatedSession) -> PairedParty<'a> {
    PairedParty {
        organization: &binding.organization_id,
        signed_in_as: session.context().principal_id(),
        binding_owner: &binding.owner_id,
        receiver: &binding.receiver_id,
    }
}

/// Whether `session` may read conversation `id` (`conversation.read`). An
/// ungranted id answers `conversation_not_found`, as one that does not exist
/// does (row G12).
pub(super) async fn admit_conversation(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    id: &ConversationId,
) -> Result<(), &'static str> {
    let Some(binding) = paired_binding(state, session).await? else {
        return Ok(());
    };
    let Some((_, conversations, grants)) = state.passive_read.as_ref() else {
        return Ok(());
    };
    let access = PairedAccess::new(access_policy(state)?, party(&binding, session));
    let (conversation, held) = paired_facts(
        conversations.as_ref(),
        grants.as_ref(),
        &binding.receiver_id,
        id,
    )
    .await
    .map_err(paired_code)?;
    access
        .admit(
            AccessAction::Read,
            &conversation,
            held.map(|held| held.role),
            RequestShape::default(),
        )
        .map_err(paired_code)
}

/// Whether `session` may ask for the owner's list (`conversation.list` on
/// the catalogue its binding reads). A paired device lists through its
/// catalogue, which the store narrows to its grants; the socket's lists are
/// the owner's own, so the policy refuses them to a device (row G12) rather
/// than it being handed a list filtered after it was read.
pub(super) async fn admit_list(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
) -> Result<(), &'static str> {
    let Some(binding) = paired_binding(state, session).await? else {
        return Ok(());
    };
    PairedAccess::new(access_policy(state)?, party(&binding, session))
        .admit_catalogue(AccessAction::List)
        .map_err(paired_code)
}

/// The action a paired party's `conversation.write` method is: a Drive
/// command starts and steers a turn, stops it, answers or cancels its
/// permission request, answers its question, or looks up those commands'
/// receipts (row J11); a Comment command leaves a note or reads where the
/// caller's own note stands (row J17); any other is `conversation.write`
/// (deleting, archiving, exporting, re-sharing, changing the model, issue
/// 707), asked on the catalogue the binding reads because not every such
/// method names a conversation. Which role allows which, and that none
/// allows the rest, is the policy's.
fn paired_action(method: &str) -> AccessAction {
    match method {
        "conversation.send"
        | "conversation.steer"
        | "conversation.stop"
        | "conversation.answer"
        | "conversation.answerQuestion"
        | "conversation.cancel"
        | "conversation.receipt" => AccessAction::Drive,
        "conversation.message" | "conversation.messageStatus" => AccessAction::Comment,
        _ => AccessAction::Write,
    }
}

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
/// owner, so for it every `conversation.write` method that its grants do
/// not admit again ([`ADMITTED_BY_ITS_OWN_CHECK`]) asks the access policy
/// for its paired action ([`paired_action`], rows J12, J17). A method no
/// role names is asked `conversation.write` on the binding's catalogue, and
/// refused `forbidden` before anything about a conversation is read. A
/// Drive or Comment command asks Cedar for the grant pairing mints,
/// `conversation.read`, and then the access policy for the action on the
/// one conversation the command names, with the role its receiver holds
/// there and what the command carries, so a Drive grant is what lets the
/// device drive that conversation and no other (row J11), a Comment or
/// Drive grant what lets it leave notes there, a driver sends text only
/// (row J14), and a peer gateway does neither. Refused, it is told
/// `forbidden` when it may read the conversation, and otherwise what a read
/// would tell it: a peer gateway learns no more from a refused command than
/// from a refused read.
///
/// Admitted, a request is answered with who it is from: a turn it starts, or
/// a note it leaves, is saved with the grant that admitted it and the
/// grant's policy revision as read for this command, never a copy of its
/// role, and every other with the owner (rows J15, J17). The initiation
/// says a turn's tools run unchecked; a note runs no tools.
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
    let Some((_, conversations, grants)) = state.passive_read.as_ref() else {
        return as_owner().await;
    };
    let Some(binding) = paired_binding(state, session).await? else {
        return as_owner().await;
    };
    // A device signs in as its owner, so its command is the owner's from
    // the device's surface. A peer gateway signs in as its own principal
    // and forwards its people's commands in a later slice.
    let access = PairedAccess::new(access_policy(state)?, party(&binding, session));
    let action = paired_action(method);
    if action == AccessAction::Write {
        access.admit_catalogue(action).map_err(paired_code)?;
        // No policy permits it today; one that did would let the party
        // send this command as the owner's surface does.
        return as_owner().await;
    }
    admit_grant(state, session, PAIRING_GRANT).await?;
    let id = drive_target(frame)?;
    let (conversation, held) = paired_facts(
        conversations.as_ref(),
        grants.as_ref(),
        &binding.receiver_id,
        &id,
    )
    .await
    .map_err(paired_code)?;
    access
        .admit(
            action,
            &conversation,
            held.map(|held| held.role),
            request_shape(frame),
        )
        .map_err(paired_code)?;
    // Every paired permit asks for a grant held, so an admitted command
    // always has one.
    let held = held.ok_or("forbidden")?;
    // A receiver id is at most 128 bytes, inside what a record keeps; one of
    // only blanks, which no pairing mints, is refused as unverifiable.
    // The turn names the grant and its revision; what that revision allowed is
    // the grant journal's to say, not a copy on the turn.
    GrantInitiator::new(binding.receiver_id, held.policy_revision)
        .map(Initiation::grant)
        .map_err(|_| refusal_code(ReadRefusal::Unverifiable))
}

/// The grant pairing mints a device (`device_pairing`), which a Drive
/// command or a Comment note asks Cedar for in place of `conversation.write`.
const PAIRING_GRANT: &str = "conversation.read";

/// The conversation a Drive command or a Comment note names. Its params are
/// checked in full by the handler; a command naming none, or no valid id, is
/// refused here as the handler would refuse it.
fn drive_target(frame: &RequestFrame) -> Result<ConversationId, &'static str> {
    frame
        .params
        .get("conversationId")
        .and_then(serde_json::Value::as_str)
        .and_then(|id| conversation_id(id).ok())
        .ok_or(ConversationErrorCode::InvalidRequest.as_str())
}

/// What a command carries, as far as the policy may care: images named by
/// a digest in the owner's attachment store, files linked by a path on the
/// owner's machine, and the operation a receipt lookup names (row J14).
fn request_shape(frame: &RequestFrame) -> RequestShape<'_> {
    let non_empty = |name: &str| {
        frame
            .params
            .get(name)
            .is_some_and(|value| !value.is_null() && *value != serde_json::json!([]))
    };
    RequestShape {
        attachments: non_empty("attachments"),
        files: non_empty("files"),
        receipt_operation: (frame.method == "conversation.receipt")
            .then(|| {
                frame
                    .params
                    .get("operation")
                    .and_then(serde_json::Value::as_str)
            })
            .flatten(),
    }
}

/// A paired party's refusal as the socket tells it: a conversation it may
/// not see is one that does not exist.
fn paired_code(refusal: PairedRefusal) -> &'static str {
    match refusal {
        PairedRefusal::NotFound => ConversationErrorCode::ConversationNotFound.as_str(),
        PairedRefusal::Deleted => ConversationErrorCode::ConversationDeleted.as_str(),
        PairedRefusal::Forbidden => "forbidden",
        PairedRefusal::Unavailable => ConversationErrorCode::TemporarilyUnavailable.as_str(),
    }
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
    let policy = match access_policy(state) {
        Ok(policy) => policy,
        Err(code) => return failure(&frame.id, code),
    };
    let shares = ShareConversation {
        conversations: conversations.as_ref(),
        receivers: receivers.as_ref(),
        grants: grants.as_ref(),
        access: state.access.as_ref(),
        gateway: &state.gateway,
        policy,
    };
    // Sharing is the owner's: a paired device signs in as its owner's
    // principal, so the policy is asked about it as the paired party it is,
    // not only refused by the grant its credential lacks (row G8).
    match paired_binding(state, session).await {
        Ok(None) => {}
        Ok(Some(binding)) => {
            if let Err(code) = paired_share(state, session, &binding, &frame).await {
                return failure(&frame.id, code);
            }
        }
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
                let granted = shares
                    .share(
                        caller,
                        id,
                        credential,
                        role(params.role),
                        policy,
                        expected_revision(params.expected_policy_revision)?,
                        now,
                    )
                    .await?;
                Ok(success(
                    &frame.id,
                    &ConversationShareResult {
                        request_id: params.request_id,
                        applied: granted.applied,
                        policy_revision: granted.policy_revision,
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
                let applied = shares
                    .unshare(
                        caller,
                        id,
                        credential,
                        expected_revision(params.expected_policy_revision)?,
                        now,
                    )
                    .await?;
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
                let page_size = usize::try_from(params.page_size)
                    .ok()
                    .filter(|size| (1..=MAX_SHARE_PAGE).contains(size))
                    .ok_or(ConversationError::InvalidInput)?;
                if let Some(cursor) = &params.cursor {
                    CredentialId::new(cursor.credential_id.clone())
                        .map_err(|_| ConversationError::InvalidInput)?;
                    if cursor.granted_at_ms > MAX_POLICY_REVISION {
                        return Err(ConversationError::InvalidInput);
                    }
                }
                let id = conversation_id(&params.conversation_id)?;
                let grants = shares
                    .shares(&caller(session, frame.id.clone()), &id)
                    .await?;
                let page = share_page(&grants, params.cursor.as_ref(), page_size);
                Ok(success(
                    &frame.id,
                    &ConversationSharesResult {
                        items: page.items,
                        cursor: page.cursor,
                    },
                ))
            }
            _ => Err(ConversationError::InvalidInput),
        }
    }
    .await;
    result.unwrap_or_else(|error| failure(&frame.id, error_code(&error).as_str()))
}

/// Whether a paired party may take the share action `frame` names on the
/// conversation it names (`conversation.share`, `.unshare`, `.shares`).
/// Refused `forbidden` whatever stopped it: the policy, or a request naming
/// nothing it could be asked about.
async fn paired_share(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    binding: &ReceiverBinding,
    frame: &RequestFrame,
) -> Result<(), &'static str> {
    let action = match frame.method.as_str() {
        "conversation.share" => AccessAction::Share,
        "conversation.unshare" => AccessAction::Unshare,
        "conversation.shares" => AccessAction::Shares,
        _ => return Err("forbidden"),
    };
    let (Some((_, conversations, grants)), Some(id)) = (
        state.passive_read.as_ref(),
        frame
            .params
            .get("conversationId")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| conversation_id(id).ok()),
    ) else {
        return Err("forbidden");
    };
    let access = PairedAccess::new(access_policy(state)?, party(binding, session));
    let Ok((conversation, held)) = paired_facts(
        conversations.as_ref(),
        grants.as_ref(),
        &binding.receiver_id,
        &id,
    )
    .await
    else {
        return Err("forbidden");
    };
    match access.allows(action, &conversation, held.map(|held| held.role)) {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => Err("forbidden"),
    }
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

/// The policy revision a share or revoke was built on, held to the schema's
/// bound; past it is `InvalidInput` (row J30). 0 names no grant held.
fn expected_revision(value: Option<u64>) -> Result<Option<u64>, ConversationError> {
    match value {
        Some(revision) if revision > MAX_POLICY_REVISION => Err(ConversationError::InvalidInput),
        _ => Ok(value),
    }
}

/// A wire policy as the domain's: `preset` alone, or `allow` and `deny`
/// together, each checked by `ToolRules::new`, with `commandHosts` beside
/// either, checked by `CommandHosts::new`; absent or empty, it names none.
/// Hosts are not counted against the patterns. Any other shape is
/// `InvalidInput` (row J3).
fn tool_policy(policy: ConversationToolPolicy) -> Result<ToolPolicy, ConversationError> {
    let hosts = CommandHosts::new(policy.command_hosts.unwrap_or_default())
        .map_err(|_| ConversationError::InvalidInput)?;
    let tools = match (policy.preset, policy.allow, policy.deny) {
        (Some(named), None, None) => ToolPolicy::preset(preset(named)),
        (None, Some(allow), Some(deny)) => ToolRules::new(allow, deny)
            .map(ToolPolicy::rules)
            .map_err(|_| ConversationError::InvalidInput)?,
        _ => return Err(ConversationError::InvalidInput),
    };
    Ok(tools.with_command_hosts(hosts))
}

fn wire_policy(policy: &ToolPolicy) -> ConversationToolPolicy {
    let hosts = policy.command_hosts();
    let command_hosts = (!hosts.is_empty()).then(|| hosts.names().iter().cloned().collect());
    match policy.tools() {
        ToolChoice::Preset(named) => ConversationToolPolicy {
            preset: Some(wire_preset(*named)),
            allow: None,
            deny: None,
            command_hosts,
        },
        ToolChoice::Rules(rules) => ConversationToolPolicy {
            preset: None,
            allow: Some(rules.allow().to_vec()),
            deny: Some(rules.deny().to_vec()),
            command_hosts,
        },
    }
}

struct SharePage {
    items: Vec<ConversationShare>,
    cursor: Option<ConversationSharesCursor>,
}

/// One page of `grants`, already oldest first (`granted_at_ms`, then
/// credential). `cursor` names the last grant the previous page returned;
/// this page is the grants strictly after that pair. The cursor on the
/// result names this page's last grant when further grants remain, and is
/// absent when the page finishes the list.
fn share_page(
    grants: &[ShareGrant],
    cursor: Option<&ConversationSharesCursor>,
    page_size: usize,
) -> SharePage {
    let start = match cursor {
        None => 0,
        Some(cursor) => grants
            .iter()
            .position(|grant| {
                (grant.granted_at_ms, grant.credential_id.as_str())
                    > (cursor.granted_at_ms, cursor.credential_id.as_str())
            })
            .unwrap_or(grants.len()),
    };
    let end = start.saturating_add(page_size).min(grants.len());
    let slice = &grants[start..end];
    let cursor = if end < grants.len() {
        slice.last().map(|grant| ConversationSharesCursor {
            granted_at_ms: grant.granted_at_ms,
            credential_id: grant.credential_id.as_str().to_owned(),
        })
    } else {
        None
    };
    SharePage {
        items: slice.iter().map(listed).collect(),
        cursor,
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
