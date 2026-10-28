//! Owner reset binds one observed source to one immutable frozen capture.
use super::super::{
    codec, cursor, invalid, journal, publication, registry, unavailable, SqlOutbox,
};
use super::{Fact, Selection};
use crate::webhooks::application::{OutboxError, Registration};
use nessa_auth::domain::{CredentialId, PrincipalId};
use nessa_local_database::rusqlite::{params, Connection, OptionalExtension};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sync::replication::domain::Scope;
use uuid::Uuid;

pub(crate) struct ResetSnapshot {
    pub registration: Registration,
    pub issued_at_ms: u64,
    pub cursor: cursor::Cursor,
}
pub(crate) struct ResetReceipt {
    pub request: Uuid,
    pub receipt: Uuid,
    pub scope: Scope,
    pub at_ms: u64,
}
pub(crate) struct ResetCommand {
    pub request: Uuid,
    pub registration: Uuid,
    pub credential: CredentialId,
    pub issued_at_ms: u64,
    pub actor: PrincipalId,
    pub conversation: ConversationId,
    pub expected: cursor::Cursor,
    pub observed: Scope,
    pub at_ms: u64,
    pub authority_expires_at_ms: Option<u64>,
}
fn receipt(fact: &Fact) -> Result<ResetReceipt, OutboxError> {
    let selection = fact.selection.as_ref().ok_or(OutboxError::Invalid)?;
    Ok(ResetReceipt {
        request: super::super::uuid(&selection.request)?,
        receipt: fact.id,
        scope: selection.scope.checked()?,
        at_ms: selection.at_ms,
    })
}
fn selected(connection: &Connection, request: Uuid) -> Result<Option<Fact>, OutboxError> {
    let id: Option<String> = connection.query_row(
        "SELECT id FROM source_reset_facts INDEXED BY owner_reset_request WHERE json_extract(CASE WHEN json_valid(fact) THEN fact ELSE '{}' END,'$.selection.request')=?1 AND json_extract(CASE WHEN json_valid(fact) THEN fact ELSE '{}' END,'$.selection.request') IS NOT NULL",
        [request.to_string()], |r| {codec::check_row_bytes(r, 0..1, 36)?; r.get(0)}).optional().map_err(cursor::row_error)?;
    id.map(|id| super::read(connection, super::super::uuid(&id)?)?.ok_or(OutboxError::Invalid))
        .transpose()
}
fn correlate(
    fact: &Fact,
    registration: Uuid,
    credential: &CredentialId,
    issued_at_ms: u64,
    actor: &PrincipalId,
    conversation: &ConversationId,
) -> Result<(), OutboxError> {
    let selection = fact.selection.as_ref().ok_or(OutboxError::Invalid)?;
    if fact.registration != registration
        || &fact.conversation != conversation
        || selection.credential != credential.as_str()
        || selection.issued_at_ms != issued_at_ms
        || selection.actor != actor.as_str()
    {
        return Err(OutboxError::Conflict);
    }
    Ok(())
}
impl SqlOutbox {
    /// No publication, cursor mutation or physical source read is performed.
    pub(crate) async fn reset_snapshot(
        &self,
        credential: CredentialId,
        conversation: ConversationId,
    ) -> Result<ResetSnapshot, OutboxError> {
        self.read(move |connection| {
            let saved =
                registry::by_credential(connection, &credential)?.ok_or(OutboxError::Conflict)?;
            if saved.lifecycle.phase() != crate::webhooks::domain::RegistrationPhase::Active {
                return Err(OutboxError::Conflict);
            }
            let cursor = cursor::read(connection, saved.registration.id, &conversation)?
                .ok_or(OutboxError::Conflict)?;
            Ok(ResetSnapshot {
                registration: saved.registration,
                issued_at_ms: saved.issued_at_ms,
                cursor,
            })
        })
        .await
    }
    /// Caller freshly authorizes the current owner on every invocation, including replay.
    pub(crate) async fn reset_receipt(
        &self,
        request: Uuid,
        registration: Uuid,
        credential: CredentialId,
        issued_at_ms: u64,
        actor: PrincipalId,
        conversation: ConversationId,
    ) -> Result<Option<ResetReceipt>, OutboxError> {
        self.read(move |connection| {
            let saved = registry::by_id(connection, registration)?;
            if saved.lifecycle.phase() != crate::webhooks::domain::RegistrationPhase::Active
                || saved.registration.target.credential != credential
                || saved.issued_at_ms != issued_at_ms
            {
                return Err(OutboxError::Conflict);
            }
            let Some(fact) = selected(connection, request)? else {
                return Ok(None);
            };
            verify_receipt(connection, &fact)?;
            correlate(
                &fact,
                registration,
                &credential,
                issued_at_ms,
                &actor,
                &conversation,
            )?;
            receipt(&fact).map(Some)
        })
        .await
    }
    /// Native CAS says which observed incarnation was selected, not which source
    /// remains current at commit. Auth is retained by the owning product job.
    pub(crate) async fn reset_source(
        &self,
        command: ResetCommand,
    ) -> Result<ResetReceipt, OutboxError> {
        let clock = self.owner.clock.clone();
        let check = command_check(&command);
        self.run_checked(move |connection| check(connection), move |connection, audit, replay| {
            let now = clock.unix_milliseconds();
            if command.authority_expires_at_ms.is_some_and(|expiry| now >= expiry) { return Err(OutboxError::Conflict); }
            if let Some(fact) = replay { return receipt(&fact); }
            let transaction = connection.transaction().map_err(unavailable)?;
            // Repeat after owed publication: no stale pre-publication cursor is authority.
            if let Some(fact) = command_check(&command)(&transaction)? { return receipt(&fact); }
            let original = super::read(&transaction, command.request)?.ok_or(OutboxError::Conflict)?;
            let fact = Fact { id: Uuid::new_v4(), selection: Some(Selection {
                request: command.request.to_string(), actor: command.actor.as_str().to_owned(), credential: command.credential.as_str().to_owned(),
                issued_at_ms: command.issued_at_ms, at_ms: command.at_ms, scope: codec::SavedScope::of(&command.observed),
            }), ..original };
            super::store::insert(&transaction, &fact)?;
            publication::append(&transaction, super::super::audit::Record::cursor(&fact))?;
            let changed = transaction.execute("UPDATE cursors SET scope=?3,position=0,reset_required=0,source_reset=?4 WHERE registration=?1 AND conversation=?2 AND reset_required=1 AND source_reset=?5",params![command.registration.to_string(), command.conversation.to_string(), codec::scope_text(&command.observed)?, fact.id.to_string(), command.request.to_string()]).map_err(unavailable)?;
            if changed != 1 { return Err(OutboxError::Conflict); }
            if command.authority_expires_at_ms.is_some_and(|expiry| clock.unix_milliseconds() >= expiry) { return Err(OutboxError::Conflict); }
            journal::commit(transaction, Some(command.registration), command.at_ms)?;
            publication::publish(connection, audit)?;
            receipt(&fact)
        }).await
    }
}
fn command_check(
    command: &ResetCommand,
) -> impl Fn(&Connection) -> Result<Option<Fact>, OutboxError> + Send + 'static {
    let request = command.request;
    let registration = command.registration;
    let credential = command.credential.clone();
    let issued_at_ms = command.issued_at_ms;
    let actor = command.actor.clone();
    let conversation = command.conversation.clone();
    let expected = command.expected.clone();
    let observed = command.observed.clone();
    move |connection| {
        let saved = registry::by_id(connection, registration)?;
        if saved.lifecycle.phase() != crate::webhooks::domain::RegistrationPhase::Active
            || saved.registration.target.credential != credential
            || saved.issued_at_ms != issued_at_ms
        {
            return Err(OutboxError::Conflict);
        }
        if let Some(fact) = selected(connection, request)? {
            verify_receipt(connection, &fact)?;
            correlate(
                &fact,
                registration,
                &credential,
                issued_at_ms,
                &actor,
                &conversation,
            )?;
            return Ok(Some(fact));
        }
        cursor::check_identity(
            &registration.to_string(),
            &conversation.to_string(),
            &observed,
        )?;
        if !expected.reset_required
            || expected.reset != Some(request)
            || cursor::read(connection, registration, &conversation)?.as_ref() != Some(&expected)
        {
            return Err(OutboxError::Conflict);
        }
        let original = super::read(connection, request)?.ok_or(OutboxError::Conflict)?;
        if original.selection.is_some()
            || original.registration != registration
            || original.conversation != conversation
        {
            return Err(OutboxError::Conflict);
        }
        // Validating the encoded scope also enforces its existing byte budget.
        codec::scope_text(&observed).map_err(invalid)?;
        Ok(None)
    }
}

/// A selected choice is not acknowledgement without its exact retained audit.
/// The owner receipt uses the selection UUID as its immutable audit identity.
pub(in super::super) fn verify_receipt(
    connection: &Connection,
    fact: &Fact,
) -> Result<(), OutboxError> {
    if fact.selection.is_none() {
        return Ok(());
    }
    let saved = connection
        .query_row(
            "SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE id=?1",
            [fact.id.to_string()],
            publication::SavedAudit::row,
        )
        .optional()
        .map_err(cursor::row_error)?
        .ok_or(OutboxError::Invalid)?;
    let (id, text) = saved.checked()?;
    if id != fact.id || text != super::super::audit::Record::cursor(fact).encode()? {
        return Err(OutboxError::Invalid);
    }
    let record = super::super::audit::Record::decode(&text)?;
    super::super::restoration::contextual(connection, &record, false)
}
