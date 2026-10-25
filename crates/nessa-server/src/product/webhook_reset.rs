//! Owner-only checkpoint selection; the native owner holds immutable replay proof.
use super::{
    conversation::conversation_id,
    read_access,
    socket::{admit_grant, admit_now, failure, success},
    state::ProductRouteState,
};
use crate::conversation::application::{
    load_conversation, AccessAction, OwnerAccess, PrincipalRecordScope, RecordReadLease,
    RecordReadValue,
};
use crate::conversation::domain::Conversation;
use crate::webhooks::{
    application::OutboxError,
    infrastructure::{ResetCommand, ResetReceipt},
};
use nessa_auth::{
    application::{
        ports::{AccessAdmission, AccessReader},
        session::AuthenticatedSession,
    },
    domain::CredentialId,
};
use nessa_protocol::{
    product::generated::{
        ConversationRecordsHeadResult, ConversationWebhookCursorParams,
        ConversationWebhookCursorResult, ConversationWebhookResetParams,
        ConversationWebhookResetResult, RecordScope,
    },
    protocol::{OutgoingMessage, RequestFrame},
};
use nessa_sync::replication::domain::Scope;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(runtime) = state.webhooks.clone() else {
        return failure(&frame.id, "conversations_not_configured");
    };
    let requested_head = frame.method == "conversation.webhook.cursor"
        && frame
            .params
            .get("includeSourceHead")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
    let state = state.clone();
    let session = session.clone();
    let id = frame.id.clone();
    let work = runtime.retain_grant(false, async move { Ok(run(&state, &session, frame).await) });
    // Expiring this waiter does not cancel the retained runtime job. Its source
    // lease, authority and reader permit survive until physical work is joined.
    let outcome = if requested_head {
        match tokio::time::timeout(super::passive_read::deadlines::PASSIVE_READ_TIMEOUT, work).await
        {
            Ok(outcome) => outcome,
            Err(_) => return failure(&id, "read_timeout"),
        }
    } else {
        work.await
    };
    match outcome {
        Ok(Ok(message)) => message,
        Ok(Err(code)) => failure(&id, code),
        Err(_) => failure(&id, "temporarily_unavailable"),
    }
}
async fn run(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> Result<OutgoingMessage, &'static str> {
    let runtime = state
        .webhooks
        .as_ref()
        .ok_or("conversations_not_configured")?;
    let params: ConversationWebhookCursorParams;
    let request = if frame.method == "conversation.webhook.reset" {
        let reset: ConversationWebhookResetParams =
            serde_json::from_value(frame.params.clone()).map_err(|_| "invalid_request")?;
        params = ConversationWebhookCursorParams {
            conversation_id: reset.conversation_id,
            credential_id: reset.credential_id,
            include_source_head: None,
        };
        Some(
            Uuid::parse_str(&reset.request_id)
                .ok()
                .filter(|id| id.to_string() == reset.request_id)
                .ok_or("invalid_request")?,
        )
    } else {
        params = serde_json::from_value(frame.params.clone()).map_err(|_| "invalid_request")?;
        None
    };
    let include_source_head = params.include_source_head.unwrap_or(false);
    let canonical = request.is_some() || include_source_head;
    // One Auth hold precedes all metadata/physical/native work. Never reacquire
    // the same fair read fence while retaining it (a writer may already wait).
    let hold = runtime
        .credentials()
        .hold(session.context().credential_id())
        .await
        .map_err(|_| "unauthorized")?;
    let session = admit_now(state, session, &frame.method).await?;
    if canonical {
        admit_grant(state, &session, "conversation.read").await?;
    }
    // Policy consumes the same canonical Auth owner whose physical fence we
    // retain. The outer route reader is not a substitute for that binding.
    for &action in if canonical {
        &["credential.manage", "conversation.read"][..]
    } else {
        &["credential.manage"][..]
    } {
        let decision = nessa_auth::application::authorization::AuthorizeAction {
            access: runtime.credentials(),
            clock: state.clock.as_ref(),
            policy: state.policy.as_ref(),
        }
        .execute(
            &session,
            &nessa_auth::domain::Action::new(action).map_err(|_| "invalid_request")?,
            &state.gateway,
        )
        .await
        .map_err(|_| "unauthorized")?;
        if decision != nessa_auth::application::ports::Decision::Allow {
            return Err("forbidden");
        }
    }

    let conversation = conversation_id(&params.conversation_id).map_err(|_| "invalid_request")?;
    let credential = CredentialId::new(params.credential_id).map_err(|_| "invalid_request")?;
    let (receivers, conversations, _) = state
        .passive_read
        .as_ref()
        .ok_or("conversations_not_configured")?;
    let policy = state
        .conversation_access
        .as_deref()
        .ok_or("temporarily_unavailable")?;
    if let Some(binding) =
        crate::conversation::application::binding_of(receivers.as_ref(), session.context())
            .await
            .map_err(|_| "forbidden")?
    {
        read_access::paired_share(state, &session, &binding, &frame).await?;
    }
    let record = load_conversation(conversations.as_ref(), &conversation)
        .await
        .map_err(|_| "temporarily_unavailable")?
        .ok_or("conversation_not_found")?;
    OwnerAccess::new(policy)
        .admit_to(
            if canonical {
                AccessAction::Act
            } else {
                AccessAction::Shares
            },
            &record,
            session.context().organization_id(),
            session.context().principal_id(),
        )
        .map_err(|_| "forbidden")?;
    let snapshot = runtime
        .store()
        .reset_snapshot(credential.clone(), conversation.clone())
        .await
        .map_err(store_error)?;
    if &snapshot.registration.target.owner != record.owner()
        || &snapshot.registration.target.organization != record.organization()
    {
        return Err("forbidden");
    }
    let target = runtime
        .credentials()
        .read(&credential)
        .await
        .map_err(|_| "temporarily_unavailable")?;
    crate::webhooks::application::validate_target_snapshot(
        &target,
        &snapshot.registration.target,
        &state.gateway,
        state.clock.unix_seconds(),
    )
    .map_err(|_| "forbidden")?;
    if target.origin.cause() != nessa_auth::domain::IssuanceCause::WebhookIssue
        || target.credential.issued_at().checked_mul(1000) != Some(snapshot.issued_at_ms)
    {
        return Err("forbidden");
    }
    if request.is_some() {
        use crate::conversation::application::{
            AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject, RequestShape,
            ShareTarget,
        };
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: session.context().organization_id(),
                principal: session.context().principal_id(),
            },
            action: AccessAction::Share,
            resource: AccessResource::of(&record),
            held: None,
            request: RequestShape::default(),
            target: Some(ShareTarget {
                organization: &snapshot.registration.target.organization,
                binding_owner: &snapshot.registration.target.owner,
                grants: target.credential.grants(),
                gateway: &state.gateway,
            }),
        };
        match policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => {}
            AccessDecision::Deny(AccessDenial::Unavailable) => {
                return Err("temporarily_unavailable")
            }
            AccessDecision::Deny(_) => return Err("forbidden"),
        }
    }
    if !hold
        .snapshot
        .credential
        .is_valid_at(state.clock.unix_seconds())
    {
        return Err("unauthorized");
    }
    let expires = hold
        .snapshot
        .credential
        .expires_at()
        .into_iter()
        .chain(session.expires_at())
        .chain(target.credential.expires_at())
        .min()
        .map(|s| s.saturating_mul(1000));
    let ownership = Arc::new(Mutex::new(hold));
    let Some(request) = request else {
        let response = if include_source_head {
            Some(
                runtime
                    .owner_head(
                        owner_scope(
                            snapshot.registration.id,
                            &snapshot.cursor.scope,
                            &record,
                            &session,
                        )?,
                        RecordReadLease::new(ownership.clone()),
                    )
                    .await
                    .map_err(|_| "temporarily_unavailable")?,
            )
        } else {
            None
        };
        if response.is_some()
            && expires.is_some_and(|expiry| state.clock.unix_milliseconds() >= expiry)
        {
            return Err("unauthorized");
        }
        let observed_source_head = match response.as_ref().map(|r| &r.value) {
            Some(RecordReadValue::Head(head)) => Some(ConversationRecordsHeadResult {
                scope: scope(&head.scope),
                head: head.head.to_string(),
            }),
            None => None,
            _ => return Err("temporarily_unavailable"),
        };
        let message = success(
            &frame.id,
            &ConversationWebhookCursorResult {
                observed_source_head,
                scope: scope(&snapshot.cursor.scope),
                position: snapshot.cursor.position.to_string(),
                reset_required: snapshot.cursor.reset_required,
                request_id: if snapshot.cursor.reset_required {
                    snapshot.cursor.reset.map(|id| id.to_string())
                } else {
                    None
                },
            },
        );
        drop(response);
        return Ok(message);
    };
    let actor = session.context().principal_id().clone();
    if let Some(receipt) = runtime
        .store()
        .reset_receipt(
            request,
            snapshot.registration.id,
            credential.clone(),
            snapshot.issued_at_ms,
            actor.clone(),
            conversation.clone(),
        )
        .await
        .map_err(store_error)?
    {
        // A retained selection is not publication acknowledgement. Run the
        // native replay path so owed evidence is consumed before responding.
        let receipt = runtime
            .store()
            .reset_source(ResetCommand {
                request,
                registration: snapshot.registration.id,
                credential,
                issued_at_ms: snapshot.issued_at_ms,
                actor,
                conversation,
                expected: snapshot.cursor,
                observed: receipt.scope,
                at_ms: state.clock.unix_milliseconds(),
                authority_expires_at_ms: expires,
            })
            .await
            .map_err(store_error)?;
        return Ok(success(&frame.id, &wire(receipt)));
    }
    if !snapshot.cursor.reset_required || snapshot.cursor.reset != Some(request) {
        return Err("invalid_request");
    }
    let selected = owner_scope(
        snapshot.registration.id,
        &snapshot.cursor.scope,
        &record,
        &session,
    )?;
    let response = runtime
        .owner_head(selected, RecordReadLease::new(ownership.clone()))
        .await
        .map_err(|_| "temporarily_unavailable")?;
    let RecordReadValue::Head(head) = &response.value else {
        return Err("temporarily_unavailable");
    };
    let receipt = runtime
        .store()
        .reset_source(ResetCommand {
            request,
            registration: snapshot.registration.id,
            credential,
            issued_at_ms: snapshot.issued_at_ms,
            actor,
            conversation,
            expected: snapshot.cursor,
            observed: head.scope.clone(),
            at_ms: state.clock.unix_milliseconds(),
            authority_expires_at_ms: expires,
        })
        .await
        .map_err(store_error)?;
    drop(response);
    drop(ownership);
    Ok(success(&frame.id, &wire(receipt)))
}
fn store_error(error: OutboxError) -> &'static str {
    match error {
        OutboxError::Conflict | OutboxError::ResetRequired => "invalid_request",
        _ => "temporarily_unavailable",
    }
}
fn scope(value: &Scope) -> RecordScope {
    RecordScope {
        receiver: value.receiver().as_str().to_owned(),
        origin: value.origin().as_str().to_owned(),
        stream: value.stream().as_str().to_owned(),
        incarnation: value.incarnation().as_str().to_owned(),
        schema: value.schema().as_str().to_owned(),
        access_epoch: value.access_epoch().as_str().to_owned(),
    }
}
fn wire(receipt: ResetReceipt) -> ConversationWebhookResetResult {
    ConversationWebhookResetResult {
        request_id: receipt.request.to_string(),
        receipt_id: receipt.receipt.to_string(),
        scope: scope(&receipt.scope),
        position: "0".into(),
        applied_at_ms: receipt.at_ms,
    }
}

fn owner_scope(
    registration: Uuid,
    checkpoint: &Scope,
    record: &Conversation,
    session: &AuthenticatedSession,
) -> Result<PrincipalRecordScope, &'static str> {
    let epoch = checkpoint
        .access_epoch()
        .as_str()
        .strip_prefix("epoch-")
        .and_then(|e| e.parse().ok())
        .ok_or("temporarily_unavailable")?;
    Ok(PrincipalRecordScope::new(
        registration,
        session.context().principal_id().clone(),
        record.organization().clone(),
        record.owner().clone(),
        record.id().clone(),
        epoch,
    ))
}
