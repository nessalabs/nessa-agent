//! The share grant table (issues 704, 707) in the metadata database, beside
//! the conversations and the catalogue revisions a grant change moves.
//!
//! [`granted`] is the one statement of "this receiver holds a grant on this
//! conversation", of any role. `held_grant` reads the role and policy
//! revision from the same row for one conversation, and
//! the catalogue page and resolve ask it for a paired device's rows
//! (`store.rs`), so a device's list and its reads cannot disagree about what
//! it may see. They read the role from that row too ([`held_role`]), for
//! the access policy to admit each entry they return.
//!
//! A grant's role and policy are stored as this adapter spells them: the
//! role as `read`, `comment` or `drive`, and a Drive grant's policy as JSON
//! ([`policy_text`]), read back through the domain's constructors.
use super::{
    cells, failed, next_revision, read, stored_time, time, unreadable, LocalConversationStore,
};
use crate::conversation::application::{
    AccessAction, ConversationError, ConversationFuture, HeldGrant, OwnerAccess, RevisionHold,
    ShareGrant, ShareGrantChange, ShareGrantReceipt, ShareGrantTransition, ShareGrants,
    MAX_DRIVE_GRANTS_PER_CONVERSATION, MAX_SHARE_GRANTS_PER_CONVERSATION,
};
use crate::conversation::domain::{
    CommandHosts, ShareAccess, ShareRole, ToolChoice, ToolPolicy, ToolPreset, ToolRules,
};
use nessa_auth::domain::{CredentialId, MAX_IDENTIFIER_BYTES};
use nessa_local_database::rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use nessa_protocol::conversation::domain::ConversationId;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

/// One admission mutex, shared by every permit for that grant.
type GrantGate = Arc<tokio::sync::Mutex<()>>;

/// `(conversation, receiver)` as the map stores it.
type GrantKey = (String, String);

/// Per-grant admission. The map holds one mutex per `(conversation, receiver)`
/// that a hold or a change is using. The entry is removed when the last permit
/// drops, and when a wait cancelled before it acquired the lock was the last
/// clone of that gate.
pub(super) struct GrantGates {
    gates: Mutex<HashMap<GrantKey, GrantGate>>,
}

/// `lock_owned` for one gate. The future inside holds one `Arc`; drop it
/// before counting.
type OwnedWait = Pin<Box<dyn Future<Output = tokio::sync::OwnedMutexGuard<()>> + Send>>;

/// The permit `hold_revision` keeps until the turn's save has been decided.
struct GrantPermit {
    key: GrantKey,
    gates: Arc<GrantGates>,
    gate: GrantGate,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl GrantGates {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            gates: Mutex::new(HashMap::new()),
        })
    }

    /// Lock `(conversation, receiver)`. A caller that already holds the
    /// metadata connection must not call this: the connection is taken only
    /// after the permit, inside `run`.
    ///
    /// A wait cancelled before the permit is built removes the map entry when
    /// it was the last clone of that gate. The lock future is dropped before
    /// that count, so its `Arc` is not mistaken for another waiter.
    async fn lock(self: &Arc<Self>, conversation: String, receiver: String) -> GrantPermit {
        let key = (conversation, receiver);
        let gate = {
            let mut gates = self.gates.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(existing) = gates.get(&key) {
                Arc::clone(existing)
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(key.clone(), Arc::clone(&gate));
                gate
            }
        };
        LockWait::acquire(Arc::clone(self), key, gate).await
    }
}

/// One `lock_owned` that has not returned a permit yet. Dropped while the
/// wait is still pending, it removes the map entry when this wait is the
/// last clone of that gate.
struct LockWait {
    key: GrantKey,
    gates: Arc<GrantGates>,
    /// `None` once the permit has taken the gate. Drop then leaves the map
    /// to the permit.
    gate: Option<GrantGate>,
    waiting: Option<OwnedWait>,
}

impl LockWait {
    fn acquire(gates: Arc<GrantGates>, key: GrantKey, gate: GrantGate) -> Self {
        let waiting: OwnedWait = Box::pin(Arc::clone(&gate).lock_owned());
        Self {
            key,
            gates,
            gate: Some(gate),
            waiting: Some(waiting),
        }
    }
}

impl Future for LockWait {
    type Output = GrantPermit;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let ready = {
            let waiting = this
                .waiting
                .as_mut()
                .expect("a finished lock wait is not polled");
            waiting.as_mut().poll(cx)
        };
        match ready {
            Poll::Pending => Poll::Pending,
            Poll::Ready(guard) => {
                this.waiting.take();
                Poll::Ready(GrantPermit {
                    key: this.key.clone(),
                    gates: Arc::clone(&this.gates),
                    gate: this.gate.take().expect("a lock wait keeps its gate"),
                    guard: Some(guard),
                })
            }
        }
    }
}

impl Drop for LockWait {
    fn drop(&mut self) {
        // The lock future holds one Arc. Release it before counting, so a
        // cancelled wait uses the same last-clone rule as a finished permit.
        self.waiting.take();
        let Some(gate) = self.gate.as_ref() else {
            return;
        };
        remove_if_last(&self.gates, &self.key, gate);
    }
}

impl Drop for GrantPermit {
    fn drop(&mut self) {
        // Release the waiters before deciding whether this gate is still in
        // use. The map lock makes that decision atomic with a later lookup.
        self.guard.take();
        remove_if_last(&self.gates, &self.key, &self.gate);
    }
}

/// Remove `key` when `gate` is still the map's gate and the only other owner
/// is this call. Two owners: the map, and `gate`.
fn remove_if_last(gates: &GrantGates, key: &GrantKey, gate: &GrantGate) {
    let mut map = gates.gates.lock().unwrap_or_else(PoisonError::into_inner);
    let last = map
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(current, gate) && Arc::strong_count(gate) == 2);
    if last {
        map.remove(key);
    }
}

/// What one attempt to write a grant change did.
enum ChangeAttempt {
    Applied(ShareGrantReceipt),
    /// The receiver this attempt locked is not the row it would write.
    Retry,
}

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

/// SQL naming the role the receiver bound to parameter `receiver` holds on
/// the conversation whose id is `conversation`, as stored, or NULL when it
/// holds none: the row [`granted`] asks about. Read back by [`stored_role`].
pub(super) fn held_role(conversation: &str, receiver: &str) -> String {
    format!("(SELECT g.role FROM {})", grant_row(conversation, receiver))
}

/// The role [`held_role`] read for row `id`; a spelling this adapter never
/// writes is that row's damage.
pub(super) fn stored_role(
    stored: Option<String>,
    id: &str,
) -> Result<Option<ShareRole>, ConversationError> {
    stored
        .map(|name| role_named(&name).ok_or_else(|| unreadable("share_grants", id)))
        .transpose()
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
/// […]}`, with `"commandHosts": […]` beside either when it names any. One
/// policy has one spelling, so two are the same policy exactly when their
/// texts are equal, which is how a change and a retry compare them. A policy
/// stored before command hosts were has no `commandHosts`, and names none.
fn policy_text(policy: &ToolPolicy) -> String {
    let mut text = match policy.tools() {
        ToolChoice::Preset(preset) => json!({ "preset": preset_name(*preset) }),
        ToolChoice::Rules(rules) => json!({ "allow": rules.allow(), "deny": rules.deny() }),
    };
    let hosts = policy.command_hosts();
    if let (false, Value::Object(fields)) = (hosts.is_empty(), &mut text) {
        fields.insert(COMMAND_HOSTS.to_owned(), json!(hosts.names()));
    }
    text.to_string()
}

/// The field a policy's command hosts are stored under.
const COMMAND_HOSTS: &str = "commandHosts";

fn policy_from(text: &str) -> Option<ToolPolicy> {
    let Value::Object(mut fields) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    let strings = |value: &Value| -> Option<Vec<String>> {
        value
            .as_array()?
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect()
    };
    // Stored only when it names a host: an empty list is not its spelling.
    let hosts = match fields.remove(COMMAND_HOSTS) {
        None => CommandHosts::none(),
        Some(value) => {
            Some(CommandHosts::new(strings(&value)?).ok()?).filter(|hosts| !hosts.is_empty())?
        }
    };
    let tools = match fields.len() {
        1 => preset_named(fields.get("preset")?.as_str()?).map(ToolPolicy::preset),
        2 => ToolRules::new(
            strings(fields.get("allow")?)?,
            strings(fields.get("deny")?)?,
        )
        .ok()
        .map(ToolPolicy::rules),
        _ => None,
    }?;
    tools.with_command_hosts(hosts).ok()
}

/// The longest policy text a valid policy writes: every pattern at its
/// longest and every byte of it a quote or backslash, which JSON writes in two
/// (a control character, which it writes in six, is refused by `ToolRules`),
/// a command host in place of any pattern (a host has nothing JSON escapes,
/// so at its longest it is written in no more than a pattern), and the object
/// around them. A stored text past it is read as damage before it is parsed.
const MAX_POLICY_TEXT_BYTES: usize =
    ToolPolicy::MAX_ENTRIES * (2 * ToolRules::MAX_RULE_BYTES + 3) + 64;

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

/// The policy revision `receiver`'s grant on conversation `id` held once
/// journal row `at` was written: the latest row at or before it that changed
/// the grant's role or policy, which is the row that stamped the grant then.
/// Read for a retried share, which answers what its first answer did; only
/// asked of a row that left the party a grant, so one such row exists.
fn revision_after(
    transaction: &Transaction<'_>,
    id: &str,
    receiver: &str,
    at: i64,
) -> Result<u64, ConversationError> {
    let stamped: Option<i64> = transaction
        .query_row(
            "SELECT MAX(sequence) FROM share_grant_changes
             WHERE conversation_id = ?1 AND receiver_id = ?2 AND sequence <= ?3
               AND NOT (before = after AND policy_before IS policy_after)",
            params![id, receiver, at],
            |row| row.get(0),
        )
        .map_err(failed)?;
    stamped
        .and_then(sequence)
        .ok_or_else(|| unreadable("share_grant_changes", id))
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

    fn change(&self, change: ShareGrantChange) -> ConversationFuture<'_, ShareGrantReceipt> {
        Box::pin(async move {
            // The receiver a hold locks is the live row's, which a revoke
            // names only by credential. Look it up, lock that grant, and
            // write. If the row's receiver is no longer the one locked, the
            // attempt writes nothing and tries the receiver it found.
            for _ in 0..4 {
                let key = self.receiver_to_lock(&change).await?;
                let _permit = match &key {
                    Some(receiver) => Some(
                        self.grant_gates
                            .lock(change.conversation_id.to_string(), receiver.clone())
                            .await,
                    ),
                    None => None,
                };
                match self.apply_change(change.clone(), key).await? {
                    ChangeAttempt::Applied(receipt) => return Ok(receipt),
                    ChangeAttempt::Retry => continue,
                }
            }
            tracing::error!("a share grant change could not lock the receiver it writes");
            Err(ConversationError::Metadata)
        })
    }
    fn hold_revision<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
        policy_revision: u64,
    ) -> ConversationFuture<'a, RevisionHold> {
        let gates = Arc::clone(&self.grant_gates);
        let key = (id.to_string(), receiver_id.to_owned());
        let read = self.live_revision(id, receiver_id);
        Box::pin(async move {
            let permit = gates.lock(key.0, key.1).await;
            match read.await? {
                Some(live) if live == policy_revision => Ok(RevisionHold::held(permit)),
                _ => Err(ConversationError::Revoked),
            }
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
                Some((after, Some(text))) if after == role_name(ShareRole::Drive) => {
                    policy_from(&text)
                        .map(Some)
                        .ok_or_else(|| unreadable("share_grant_changes", &id))
                }
                _ => Ok(None),
            }
        })
    }
}

impl LocalConversationStore {
    /// The receiver `hold_revision` would lock for `change`: the live row's
    /// receiver, or the one a new grant names. `None` when a revoke finds no
    /// row and names no receiver, so there is no grant to exclude.
    fn receiver_to_lock<'a>(
        &'a self,
        change: &'a ShareGrantChange,
    ) -> ConversationFuture<'a, Option<String>> {
        let id = change.conversation_id.to_string();
        let named = change.receiver_id.clone();
        let credential = change.credential_id.as_str().to_owned();
        self.run(move |connection| {
            let found: Option<String> = connection
                .query_row(
                    "SELECT receiver_id FROM share_grants
                     WHERE conversation_id = ?1 AND (receiver_id = ?2 OR credential_id = ?3)",
                    params![id, named.clone(), credential],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)?;
            Ok(found.or(named))
        })
    }

    /// Write `change` while `expected` is the receiver the caller has locked.
    /// `Retry` when that is not the row this write would touch; nothing is
    /// committed. The connection is taken here, after the grant lock.
    fn apply_change(
        &self,
        change: ShareGrantChange,
        expected: Option<String>,
    ) -> ConversationFuture<'_, ChangeAttempt> {
        let changes = self.changes.clone();
        let access = self.access.clone();
        self.run(move |connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(failed)?;
            let id = change.conversation_id.to_string();
            let conversation =
                read(&transaction, &change.conversation_id)?.ok_or(ConversationError::NotFound)?;
            // Asked inside the transaction that writes the grant, so a delete
            // cannot land between the question and the write (row G8). A
            // grant acts on the conversation: the policy wants it the
            // initiator's and not deleted. The party it grants to was
            // admitted before this transaction, by the caller. A revoke is
            // `conversation.unshare`, which a deleted conversation's owner
            // may still do.
            let (organization, principal) = (
                &change.initiator.organization_id,
                &change.initiator.principal_id,
            );
            let action = match change.transition {
                ShareGrantTransition::Grant(_) => AccessAction::Act,
                ShareGrantTransition::Revoke => AccessAction::Unshare,
            };
            let Some(access) = access.as_deref() else {
                tracing::error!(
                    "the conversation store holds no access policy; the share change is refused"
                );
                return Err(ConversationError::Metadata);
            };
            OwnerAccess::new(access).admit_to(action, &conversation, organization, principal)?;
            let after = wanted(&change.transition);
            // A retried request answers what it answered the first time, so
            // a share whose reply was lost cannot, retried after a later
            // unshare, grant again (row G16). That holds for a request that
            // changed nothing too: it is journaled with before equal to after.
            // The same request naming another device, another role or policy,
            // or the other change is refused (row J6). A retry is answered
            // before the expected revision is compared: its first answer may
            // be what moved the grant past it (row J29).
            let replayed: Option<(String, Held, Held, String, i64)> = transaction
                .query_row(
                    "SELECT credential_id, before, policy_before, after, policy_after,
                            receiver_id, sequence
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
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .optional()
                .map_err(failed)?;
            if let Some((credential, journaled_before, journaled_after, receiver, at)) = replayed {
                if credential != change.credential_id.as_str() || journaled_after != after {
                    return Err(ConversationError::InvalidInput);
                }
                let policy_revision = if journaled_after.0 == NONE {
                    None
                } else {
                    Some(revision_after(&transaction, &id, &receiver, at)?)
                };
                return Ok(ChangeAttempt::Applied(ShareGrantReceipt {
                    applied: journaled_before != journaled_after,
                    policy_revision,
                }));
            }
            let existing: Option<(String, Held, i64)> = transaction
                .query_row(
                    "SELECT receiver_id, role, policy, policy_revision FROM share_grants
                     WHERE conversation_id = ?1 AND (receiver_id = ?2 OR credential_id = ?3)",
                    params![
                        id,
                        change.receiver_id.as_deref(),
                        change.credential_id.as_str()
                    ],
                    |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?), row.get(3)?)),
                )
                .optional()
                .map_err(failed)?;
            let row_present = existing.is_some();
            let held_revision = existing
                .as_ref()
                .map(|(_, _, stored)| {
                    sequence(*stored).ok_or_else(|| unreadable("share_grants", &id))
                })
                .transpose()?;
            let (receiver, before) = match existing {
                Some((receiver, held, _)) => (receiver, held),
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
            // The permit the caller holds is for `expected`. A row that names
            // another receiver, or one that appeared after a revoke looked
            // and found nothing, is written on the next attempt under that
            // receiver's permit. Nothing is committed here.
            let covered = match &expected {
                Some(locked) => locked == &receiver,
                None => !row_present,
            };
            if !covered {
                return Ok(ChangeAttempt::Retry);
            }
            // Compared in the transaction that writes the change, and only
            // once this attempt holds that receiver's permit, so no other
            // change of this grant can land between the comparison and the
            // write (row J30). Not a policy: whether the caller may share was
            // asked above and before; this keeps the caller from overwriting a
            // grant it has not seen. A revision is a journal sequence, which
            // starts at 1, so 0 names no grant held: a first grant built on
            // a list without one is refused once another grant lands.
            if let Some(expected_revision) = change.expected_policy_revision {
                if held_revision.unwrap_or(0) != expected_revision {
                    return Err(ConversationError::ShareGrantChanged);
                }
            }
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
            let policy_revision = match (after.0.as_str(), applied) {
                (NONE, _) => None,
                (_, true) => Some(
                    self::sequence(sequence)
                        .ok_or_else(|| unreadable("share_grant_changes", &id))?,
                ),
                (_, false) => held_revision,
            };
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
            let receipt = ShareGrantReceipt {
                applied,
                policy_revision,
            };
            if !applied {
                return Ok(ChangeAttempt::Applied(receipt));
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
            Ok(ChangeAttempt::Applied(receipt))
        })
    }
}

impl LocalConversationStore {
    /// The revision the live grant row holds, `None` when the row is absent
    /// or its revision is not one this build can read. A row that cannot be
    /// read is skipped, not a failure of the store.
    fn live_revision<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<u64>> {
        let (id, receiver_id) = (id.to_string(), receiver_id.to_owned());
        self.run(move |connection| {
            match connection
                .query_row(
                    "SELECT policy_revision FROM share_grants
                     WHERE conversation_id = ?1 AND receiver_id = ?2",
                    params![id, receiver_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
            {
                Ok(Some(value)) => Ok(sequence(value)),
                Ok(None) => Ok(None),
                Err(error) if super::damaged(&error) => Ok(None),
                Err(error) => Err(failed(error)),
            }
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/share_grant_policy_text.rs"]
mod tests;

#[cfg(test)]
mod cancelled_wait {
    use super::*;
    use std::time::Duration;

    /// A second task blocks inside `lock_owned` while the holder keeps the
    /// permit. Cancelling that wait, then dropping the holder, leaves no
    /// map entry.
    #[tokio::test]
    async fn a_cancelled_contended_wait_leaves_the_map_empty_once_the_holder_drops() {
        let gates = GrantGates::new();
        let key = ("conversation".to_owned(), "phone".to_owned());
        let holder = gates.lock(key.0.clone(), key.1.clone()).await;
        let gates_for_wait = Arc::clone(&gates);
        let conversation = key.0.clone();
        let receiver = key.1.clone();
        let waiter = tokio::spawn(async move {
            let _permit = gates_for_wait.lock(conversation, receiver).await;
        });
        let entered = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let count = {
                    let map = gates.gates.lock().unwrap_or_else(PoisonError::into_inner);
                    map.get(&key).map(Arc::strong_count).unwrap_or(0)
                };
                // The map, the holder's gate, the holder's guard, the waiter's
                // gate, and the lock future. Five means the wait is inside
                // `lock_owned`, not still walking up to it.
                if count >= 5 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            entered.is_ok(),
            "the contended wait did not reach lock_owned"
        );
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        drop(holder);
        let map = gates.gates.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(
            map.is_empty(),
            "a cancelled contended wait left a gate in the map"
        );
    }
}
