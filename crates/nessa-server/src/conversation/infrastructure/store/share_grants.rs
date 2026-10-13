//! The share grant table (issues 704, 707) in the metadata database, beside
//! the conversations and the catalogue revisions a grant change moves.
//!
//! [`granted`] is the one statement of "this receiver holds a grant on this
//! conversation", of any role. `held_grant` reads the role and policy
//! revision from the same row for one conversation, and
//! the catalogue page and resolve ask it for a paired device's rows
//! (`store.rs`), so a device's list and its reads cannot disagree about what
//! it may see.
//!
//! A grant's role and policy are stored as this adapter spells them: the
//! role as `read`, `comment` or `drive`, and a Drive grant's policy as JSON
//! ([`policy_text`]), read back through the domain's constructors.
use super::{
    cells, failed, next_revision, read, stored_time, time, unreadable, LocalConversationStore,
};
use crate::conversation::application::{
    ConversationError, ConversationFuture, HeldGrant, ShareGrant, ShareGrantChange,
    ShareGrantTransition, ShareGrants, MAX_DRIVE_GRANTS_PER_CONVERSATION,
    MAX_SHARE_GRANTS_PER_CONVERSATION,
};
use crate::conversation::domain::{ShareAccess, ShareRole, ToolPolicy, ToolPreset, ToolRules};
use nessa_auth::domain::{CredentialId, MAX_IDENTIFIER_BYTES};
use nessa_local_database::rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use nessa_protocol::conversation::domain::ConversationId;
use serde_json::{json, Map, Value};

/// SQL naming the one grant row the receiver bound to parameter `receiver`
/// holds on the conversation whose id is `conversation`, as `g`. Both are
/// names this adapter writes, never a caller's text. [`granted`] asks whether
/// it exists and `held_grant` reads its role and revision, so the two cannot
/// disagree.
fn grant_row(conversation: &str, receiver: &str) -> String {
    format!(
        "share_grants AS g
         WHERE g.conversation_id = {conversation} AND g.receiver_id = {receiver}"
    )
}

/// SQL that is true when the receiver bound to parameter `receiver` holds a
/// grant, of any role, on the conversation whose id is `conversation`.
pub(super) fn granted(conversation: &str, receiver: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM {})",
        grant_row(conversation, receiver)
    )
}

/// What a party holds on a conversation, as stored: the role, or [`NONE`],
/// and the policy text a Drive grant carries.
type Held = (String, Option<String>);

/// The journal's spelling of "no grant", before a grant or after a revoke.
const NONE: &str = "none";

/// The role's stored spelling.
fn role_name(role: ShareRole) -> &'static str {
    match role {
        ShareRole::Read => "read",
        ShareRole::Comment => "comment",
        ShareRole::Drive => "drive",
    }
}

fn role_named(name: &str) -> Option<ShareRole> {
    [ShareRole::Read, ShareRole::Comment, ShareRole::Drive]
        .into_iter()
        .find(|role| role_name(*role) == name)
}

/// The preset's stored spelling.
fn preset_name(preset: ToolPreset) -> &'static str {
    match preset {
        ToolPreset::ReadOnlyTools => "read_only_tools",
        ToolPreset::NoShell => "no_shell",
        ToolPreset::AskEverything => "ask_everything",
        ToolPreset::All => "all",
    }
}

fn preset_named(name: &str) -> Option<ToolPreset> {
    [
        ToolPreset::ReadOnlyTools,
        ToolPreset::NoShell,
        ToolPreset::AskEverything,
        ToolPreset::All,
    ]
    .into_iter()
    .find(|preset| preset_name(*preset) == name)
}

/// A policy as its stored JSON: `{"preset": …}` or `{"allow": […], "deny":
/// […]}`. One policy has one spelling, so two are the same policy exactly
/// when their texts are equal, which is how a change and a retry compare them.
fn policy_text(policy: &ToolPolicy) -> String {
    match policy {
        ToolPolicy::Preset(preset) => json!({ "preset": preset_name(*preset) }),
        ToolPolicy::Rules(rules) => json!({ "allow": rules.allow(), "deny": rules.deny() }),
    }
    .to_string()
}

fn policy_from(text: &str) -> Option<ToolPolicy> {
    let Value::Object(fields) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    let patterns = |fields: &Map<String, Value>, name: &str| -> Option<Vec<String>> {
        fields
            .get(name)?
            .as_array()?
            .iter()
            .map(|rule| rule.as_str().map(str::to_owned))
            .collect()
    };
    match fields.len() {
        1 => preset_named(fields.get("preset")?.as_str()?).map(ToolPolicy::Preset),
        2 => ToolRules::new(patterns(&fields, "allow")?, patterns(&fields, "deny")?)
            .ok()
            .map(ToolPolicy::Rules),
        _ => None,
    }
}

/// The longest policy text a valid policy writes: every pattern at its
/// longest and every byte of it a quote or backslash, which JSON writes in two
/// (a control character, which it writes in six, is refused by `ToolRules`),
/// and the object around them. A stored
/// text past it is read as damage before it is parsed.
const MAX_POLICY_TEXT_BYTES: usize =
    2 * ToolRules::MAX_RULES * (2 * ToolRules::MAX_RULE_BYTES + 3) + 32;

/// A stored journal sequence, which SQLite numbers from 1.
fn sequence(value: i64) -> Option<u64> {
    u64::try_from(value).ok().filter(|sequence| *sequence > 0)
}

/// What a transition asks the party to hold.
fn wanted(transition: &ShareGrantTransition) -> Held {
    match transition {
        ShareGrantTransition::Grant(access) => (
            role_name(access.role()).to_owned(),
            access.policy().map(policy_text),
        ),
        ShareGrantTransition::Revoke => (NONE.to_owned(), None),
    }
}

fn count(transaction: &Transaction<'_>, sql: &str, id: &str) -> Result<i64, ConversationError> {
    transaction
        .query_row(sql, params![id], |row| row.get(0))
        .map_err(failed)
}

impl ShareGrants for LocalConversationStore {
    fn held_grant<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<HeldGrant>> {
        let (id, receiver_id) = (id.to_string(), receiver_id.to_owned());
        self.run(move |connection| {
            let held = connection
                .query_row(
                    &format!(
                        "SELECT g.role, g.policy_revision FROM conversations AS c, {} AND c.id = ?1",
                        grant_row("c.id", "?2")
                    ),
                    params![id, receiver_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(failed)?;
            held.map(|(name, revision)| {
                role_named(&name)
                    .zip(sequence(revision))
                    .map(|(role, policy_revision)| HeldGrant {
                        role,
                        policy_revision,
                    })
                    .ok_or_else(|| unreadable("share_grants", &id))
            })
            .transpose()
        })
    }

    fn change(&self, change: ShareGrantChange) -> ConversationFuture<'_, bool> {
        let changes = self.changes.clone();
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let id = change.conversation_id.to_string();
            let conversation =
                read(&transaction, &change.conversation_id)?.ok_or(ConversationError::NotFound)?;
            // Asked inside the transaction that writes the grant, so a delete
            // cannot land between the check and the write: a grant needs a
            // conversation that is the initiator's and not deleted, a revoke
            // only that it is the initiator's (row G8).
            let (organization, principal) = (
                &change.initiator.organization_id,
                &change.initiator.principal_id,
            );
            match change.transition {
                ShareGrantTransition::Grant(_) => {
                    conversation.check_access(organization, principal)?
                }
                ShareGrantTransition::Revoke if !conversation.allows(organization, principal) => {
                    return Err(ConversationError::NotFound);
                }
                ShareGrantTransition::Revoke => {}
            }
            let after = wanted(&change.transition);
            // A retried request answers what it answered the first time, so
            // a share whose reply was lost cannot, retried after a later
            // unshare, grant again (row G16). That holds for a request that
            // changed nothing too: it is journaled with before equal to after.
            // The same request naming another device, another role or policy,
            // or the other change is refused (row J6).
            let replayed: Option<(String, Held, Held)> = transaction
                .query_row(
                    "SELECT credential_id, before, policy_before, after, policy_after
                     FROM share_grant_changes
                     WHERE conversation_id = ?1 AND initiator = ?2 AND surface = ?3
                       AND request = ?4",
                    params![
                        id,
                        principal.as_str(),
                        change.initiator.surface_id,
                        change.initiator.action_id
                    ],
                    |row| {
                        Ok((
                            row.get(0)?,
                            (row.get(1)?, row.get(2)?),
                            (row.get(3)?, row.get(4)?),
                        ))
                    },
                )
                .optional()
                .map_err(failed)?;
            if let Some((credential, journaled_before, journaled_after)) = replayed {
                return if credential == change.credential_id.as_str() && journaled_after == after {
                    Ok(journaled_before != journaled_after)
                } else {
                    Err(ConversationError::InvalidInput)
                };
            }
            let existing: Option<(String, Held)> = transaction
                .query_row(
                    "SELECT receiver_id, role, policy FROM share_grants
                     WHERE conversation_id = ?1 AND (receiver_id = ?2 OR credential_id = ?3)",
                    params![
                        id,
                        change.receiver_id.as_deref(),
                        change.credential_id.as_str()
                    ],
                    |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))),
                )
                .optional()
                .map_err(failed)?;
            let (receiver, before) = match existing {
                Some((receiver, held)) => (receiver, held),
                None => match &change.transition {
                    ShareGrantTransition::Grant(_) => (
                        change
                            .receiver_id
                            .clone()
                            .ok_or(ConversationError::InvalidInput)?,
                        (NONE.to_owned(), None),
                    ),
                    ShareGrantTransition::Revoke => (
                        change.receiver_id.clone().unwrap_or_default(),
                        (NONE.to_owned(), None),
                    ),
                },
            };
            let applied = before != after;
            // The bounds a new grant, and a change to Drive, must fit (rows
            // G17 and J9), checked before anything is written.
            if applied && before.0 == NONE && after.0 != NONE {
                let held = count(
                    &transaction,
                    "SELECT COUNT(*) FROM share_grants WHERE conversation_id = ?1",
                    &id,
                )?;
                if held >= MAX_SHARE_GRANTS_PER_CONVERSATION {
                    return Err(ConversationError::InvalidInput);
                }
            }
            let drive = role_name(ShareRole::Drive);
            if applied && before.0 != drive && after.0 == drive {
                let driving = count(
                    &transaction,
                    &format!(
                        "SELECT COUNT(*) FROM share_grants
                         WHERE conversation_id = ?1 AND role = '{drive}'"
                    ),
                    &id,
                )?;
                if driving >= MAX_DRIVE_GRANTS_PER_CONVERSATION {
                    return Err(ConversationError::InvalidInput);
                }
            }
            // A grant or a revoke is a catalogue change: the device's next
            // pass starts past what it completed, so a newly granted row must
            // carry a revision past it (row G5). A change of role or policy
            // leaves what the device reads as it was, so it keeps the
            // revision it found, as a request that changed nothing does (rows
            // G9, J4).
            let reads_change = (before.0 == NONE) != (after.0 == NONE);
            let revision = if reads_change {
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
            let at = stored_time(change.at_ms)?;
            transaction
                .execute(
                    "INSERT INTO share_grant_changes
                         (conversation_id, receiver_id, credential_id, before, after,
                          policy_before, policy_after, initiator, surface, request,
                          changed_at_ms, revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        id,
                        receiver,
                        change.credential_id.as_str(),
                        before.0,
                        after.0,
                        before.1,
                        after.1,
                        change.initiator.principal_id.as_str(),
                        change.initiator.surface_id,
                        change.initiator.action_id,
                        at,
                        revision,
                    ],
                )
                .map_err(failed)?;
            // The role and policy a grant holds are stamped with the journal
            // row that set them, so a later change moves its policy revision
            // and a request that changed nothing leaves it (rows J4, J5).
            let sequence = transaction.last_insert_rowid();
            if applied {
                match (before.0.as_str(), after.0.as_str()) {
                    (_, NONE) => transaction.execute(
                        "DELETE FROM share_grants WHERE conversation_id = ?1 AND receiver_id = ?2",
                        params![id, receiver],
                    ),
                    (NONE, _) => transaction.execute(
                        "INSERT INTO share_grants
                             (conversation_id, receiver_id, credential_id, role, policy,
                              policy_revision, granted_at_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            id,
                            receiver,
                            change.credential_id.as_str(),
                            after.0,
                            after.1,
                            sequence,
                            at
                        ],
                    ),
                    _ => transaction.execute(
                        "UPDATE share_grants SET role = ?3, policy = ?4, policy_revision = ?5
                         WHERE conversation_id = ?1 AND receiver_id = ?2",
                        params![id, receiver, after.0, after.1, sequence],
                    ),
                }
                .map_err(failed)?;
            }
            transaction.commit().map_err(failed)?;
            if !applied {
                return Ok(false);
            }
            tracing::info!(
                conversation_id = %change.conversation_id,
                credential_id = change.credential_id.as_str(),
                change = change.transition.as_str(),
                role_before = before.0.as_str(),
                role_after = after.0.as_str(),
                initiator = change.initiator.principal_id.as_str(),
                revision,
                policy_revision = sequence,
                "conversation share grant changed"
            );
            if reads_change {
                changes.publish(&(
                    conversation.organization().clone(),
                    conversation.owner().clone(),
                ));
            }
            Ok(true)
        })
    }

    fn grants<'a>(&'a self, id: &'a ConversationId) -> ConversationFuture<'a, Vec<ShareGrant>> {
        let id = id.clone();
        self.run(move |connection| {
            let mut statement = connection
                .prepare(&format!(
                    "SELECT receiver_id, {}, role, {}, policy_revision, granted_at_ms
                     FROM share_grants
                     WHERE conversation_id = ?1 ORDER BY granted_at_ms, credential_id",
                    super::acquisition::text("credential_id", MAX_IDENTIFIER_BYTES, false),
                    super::acquisition::text("policy", MAX_POLICY_TEXT_BYTES, true),
                ))
                .map_err(failed)?;
            let mut rows = statement.query([id.to_string()]).map_err(failed)?;
            let mut grants = Vec::new();
            while let Some(row) = rows.next().map_err(failed)? {
                let stored = cells((|| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })())?;
                let grant = stored.and_then(
                    |(receiver_id, credential, role, policy, policy_revision, granted_at)| {
                        let policy = match policy {
                            Some(text) => Some(policy_from(&text)?),
                            None => None,
                        };
                        Some(ShareGrant {
                            conversation_id: id.clone(),
                            receiver_id,
                            credential_id: CredentialId::new(credential).ok()?,
                            access: ShareAccess::new(role_named(&role)?, policy).ok()?,
                            policy_revision: sequence(policy_revision)?,
                            granted_at_ms: time(granted_at)?,
                        })
                    },
                );
                grants.push(grant.ok_or_else(|| unreadable("share_grants", &id.to_string()))?);
            }
            Ok(grants)
        })
    }

    fn policy_at<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
        policy_revision: u64,
    ) -> ConversationFuture<'a, Option<ToolPolicy>> {
        let (id, receiver_id) = (id.to_string(), receiver_id.to_owned());
        self.run(move |connection| {
            // A revision past what SQLite numbers names no entry.
            let Ok(sequence) = i64::try_from(policy_revision) else {
                return Ok(None);
            };
            let entry: Option<(String, Option<String>)> = connection
                .query_row(
                    &format!(
                        "SELECT after, {} FROM share_grant_changes
                         WHERE sequence = ?1 AND conversation_id = ?2 AND receiver_id = ?3",
                        super::acquisition::text("policy_after", MAX_POLICY_TEXT_BYTES, true),
                    ),
                    params![sequence, id, receiver_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(failed)?;
            match entry {
                Some((after, Some(text))) if after == role_name(ShareRole::Drive) => policy_from(
                    &text,
                )
                .map(Some)
                .ok_or_else(|| unreadable("share_grant_changes", &id)),
                _ => Ok(None),
            }
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/share_grant_policy_text.rs"]
mod tests;
