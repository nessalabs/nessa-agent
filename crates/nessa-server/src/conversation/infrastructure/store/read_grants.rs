//! The read grant table (issue 704) in the metadata database, beside the
//! conversations and the catalogue revisions a grant change moves.
//!
//! [`granted`] is the one statement of "this receiver holds a grant on this
//! conversation". `is_granted` asks it for one conversation, and the
//! catalogue page and resolve ask it for a paired device's rows (`store.rs`),
//! so a device's list and its reads cannot disagree about what it may see.
use super::{
    cells, failed, next_revision, read, stored_time, time, unreadable, LocalConversationStore,
};
use crate::conversation::application::{
    ConversationError, ConversationFuture, ReadGrant, ReadGrantChange, ReadGrantTransition,
    ReadGrants, MAX_READ_GRANTS_PER_CONVERSATION,
};
use nessa_auth::domain::{CredentialId, MAX_IDENTIFIER_BYTES};
use nessa_local_database::rusqlite::{params, OptionalExtension, TransactionBehavior};
use nessa_protocol::conversation::domain::ConversationId;

/// SQL that is true when the receiver bound to parameter `receiver` holds a
/// grant on the conversation whose id is `conversation`. Both are names this
/// adapter writes, never a caller's text.
pub(super) fn granted(conversation: &str, receiver: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM read_grants AS g
                 WHERE g.conversation_id = {conversation} AND g.receiver_id = {receiver})"
    )
}

impl ReadGrants for LocalConversationStore {
    fn is_granted<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, bool> {
        let (id, receiver_id) = (id.to_string(), receiver_id.to_owned());
        self.run(move |connection| {
            let found = connection
                .query_row(
                    &format!(
                        "SELECT {} FROM conversations AS c WHERE c.id = ?1",
                        granted("c.id", "?2")
                    ),
                    params![id, receiver_id],
                    |row| row.get::<_, bool>(0),
                )
                .optional()
                .map_err(failed)?;
            Ok(found.unwrap_or(false))
        })
    }

    fn change(&self, change: ReadGrantChange) -> ConversationFuture<'_, bool> {
        let changes = self.changes.clone();
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let id = change.conversation_id.to_string();
            let conversation = read(&transaction, &change.conversation_id)?
                .ok_or(ConversationError::NotFound)?;
            // Asked inside the transaction that writes the grant, so a delete
            // cannot land between the check and the write: a grant needs a
            // conversation that is the initiator's and not deleted, a revoke
            // only that it is the initiator's (row G8).
            let (organization, principal) =
                (&change.initiator.organization_id, &change.initiator.principal_id);
            match change.transition {
                ReadGrantTransition::Grant => conversation.check_access(organization, principal)?,
                ReadGrantTransition::Revoke if !conversation.allows(organization, principal) => {
                    return Err(ConversationError::NotFound);
                }
                ReadGrantTransition::Revoke => {}
            }
            // A retried request answers what it answered the first time, so
            // a share whose reply was lost cannot, retried after a later
            // unshare, grant again (row G16). That holds for a request that
            // changed nothing too: it is journaled with before equal to after.
            // The same request naming another device or the other change is
            // refused.
            let replayed: Option<(String, String, String)> = transaction
                .query_row(
                    "SELECT credential_id, before, after FROM read_grant_changes
                     WHERE conversation_id = ?1 AND initiator = ?2 AND surface = ?3
                       AND request = ?4",
                    params![
                        id,
                        principal.as_str(),
                        change.initiator.surface_id,
                        change.initiator.action_id
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(failed)?;
            if let Some((credential, before, after)) = replayed {
                let target = match change.transition {
                    ReadGrantTransition::Grant => "read",
                    ReadGrantTransition::Revoke => "none",
                };
                return if credential == change.credential_id.as_str() && after == target {
                    Ok(before != after)
                } else {
                    Err(ConversationError::InvalidInput)
                };
            }
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT receiver_id, credential_id FROM read_grants
                     WHERE conversation_id = ?1 AND (receiver_id = ?2 OR credential_id = ?3)",
                    params![
                        id,
                        change.receiver_id.as_deref(),
                        change.credential_id.as_str()
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(failed)?;
            let (receiver, before, after) = match (change.transition, existing) {
                (ReadGrantTransition::Grant, Some((receiver, _))) => (receiver, "read", "read"),
                (ReadGrantTransition::Revoke, None) => {
                    (change.receiver_id.clone().unwrap_or_default(), "none", "none")
                }
                (ReadGrantTransition::Grant, None) => {
                    let receiver = change.receiver_id.clone().ok_or(ConversationError::InvalidInput)?;
                    let held: i64 = transaction
                        .query_row(
                            "SELECT COUNT(*) FROM read_grants WHERE conversation_id = ?1",
                            params![id],
                            |row| row.get(0),
                        )
                        .map_err(failed)?;
                    if held >= MAX_READ_GRANTS_PER_CONVERSATION {
                        return Err(ConversationError::InvalidInput);
                    }
                    transaction
                        .execute(
                            "INSERT INTO read_grants
                                 (conversation_id, receiver_id, credential_id, role, granted_at_ms)
                             VALUES (?1, ?2, ?3, 'read', ?4)",
                            params![
                                id,
                                receiver,
                                change.credential_id.as_str(),
                                stored_time(change.at_ms)?
                            ],
                        )
                        .map_err(failed)?;
                    (receiver, "none", "read")
                }
                (ReadGrantTransition::Revoke, Some((receiver, _))) => {
                    transaction
                        .execute(
                            "DELETE FROM read_grants WHERE conversation_id = ?1 AND receiver_id = ?2",
                            params![id, receiver],
                        )
                        .map_err(failed)?;
                    (receiver, "read", "none")
                }
            };
            let applied = before != after;
            // A grant change is a catalogue change: the device's next pass
            // starts past what it completed, so a newly granted row must
            // carry a revision past it (row G5). A request that changed
            // nothing keeps the revision it found (row G9).
            let revision = if applied {
                let revision = next_revision(&transaction, &conversation, false)?;
                transaction
                    .execute(
                        "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                        params![revision, id],
                    )
                    .map_err(failed)?;
                revision
            } else {
                transaction
                    .query_row(
                        "SELECT change_revision FROM conversations WHERE id = ?1",
                        params![id],
                        |row| row.get(0),
                    )
                    .map_err(failed)?
            };
            transaction
                .execute(
                    "INSERT INTO read_grant_changes
                         (conversation_id, receiver_id, credential_id, before, after,
                          initiator, surface, request, changed_at_ms, revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        id,
                        receiver,
                        change.credential_id.as_str(),
                        before,
                        after,
                        change.initiator.principal_id.as_str(),
                        change.initiator.surface_id,
                        change.initiator.action_id,
                        stored_time(change.at_ms)?,
                        revision,
                    ],
                )
                .map_err(failed)?;
            transaction.commit().map_err(failed)?;
            if !applied {
                return Ok(false);
            }
            tracing::info!(
                conversation_id = %change.conversation_id,
                credential_id = change.credential_id.as_str(),
                change = change.transition.as_str(),
                initiator = change.initiator.principal_id.as_str(),
                revision,
                "conversation read grant changed"
            );
            changes.publish(&(
                conversation.organization().clone(),
                conversation.owner().clone(),
            ));
            Ok(true)
        })
    }

    fn grants<'a>(&'a self, id: &'a ConversationId) -> ConversationFuture<'a, Vec<ReadGrant>> {
        let id = id.clone();
        self.run(move |connection| {
            let mut statement = connection
                .prepare(&format!(
                    "SELECT receiver_id, {}, granted_at_ms FROM read_grants
                     WHERE conversation_id = ?1 ORDER BY granted_at_ms, credential_id",
                    super::acquisition::text("credential_id", MAX_IDENTIFIER_BYTES, false)
                ))
                .map_err(failed)?;
            let mut rows = statement.query([id.to_string()]).map_err(failed)?;
            let mut grants = Vec::new();
            while let Some(row) = rows.next().map_err(failed)? {
                let stored = cells((|| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })())?;
                let grant = stored.and_then(|(receiver_id, credential, granted_at)| {
                    Some(ReadGrant {
                        conversation_id: id.clone(),
                        receiver_id,
                        credential_id: CredentialId::new(credential).ok()?,
                        granted_at_ms: time(granted_at)?,
                    })
                });
                grants.push(grant.ok_or_else(|| unreadable("read_grants", &id.to_string()))?);
            }
            Ok(grants)
        })
    }
}
