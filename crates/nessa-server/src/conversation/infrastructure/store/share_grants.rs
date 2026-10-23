//! The share grant table (issues 704, 707) in the metadata database, beside
//! the conversations and the catalogue revisions a grant change moves.
//!
//! [`granted`] is the one statement of "this receiver holds a grant on this
//! conversation". `held_grant` reads the capabilities and grant epoch from
//! the same row for one conversation, and the catalogue page and resolve
//! ask it for a paired device's rows (`store.rs`), so a device's list and
//! its reads cannot disagree about what it may see. They read the
//! capabilities from that row too ([`held_capabilities`]), for the access
//! policy to admit each entry they return.
//!
//! A grant's capabilities and constraints are stored as this adapter spells
//! them: the set as [`Capabilities::as_str`], and a drive grant's tool
//! policy as JSON ([`policy_text`]), read back through the domain's
//! constructors.
use super::cached::CachedSql;
use super::{
    cells, failed, next_revision, read, stored_time, time, unreadable, LocalConversationStore,
};
use crate::conversation::application::{
    AccessAction, ConversationError, ConversationFuture, HeldGrant, OwnerAccess, ResourceKind,
    RevisionHold, ShareGrant, ShareGrantChange, ShareGrantList, ShareGrantReceipt,
    ShareGrantTransition, ShareGrants, MAX_DRIVE_GRANTS_PER_CONVERSATION, MAX_POLICY_REVISION,
    MAX_SHARE_GRANTS_PER_CONVERSATION,
};
use crate::conversation::domain::{
    Capabilities, CommandHosts, ShareAccess, ToolChoice, ToolPolicy, ToolPreset, ToolRules,
};
use nessa_auth::domain::{CredentialId, PrincipalId, MAX_IDENTIFIER_BYTES};
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

/// This slice's resource kind. A directory grant uses the other kind under
/// the same `(principal, kind, resource)` lock.
pub(super) const CONVERSATION: &str = ResourceKind::Conversation.as_str();

/// `(resource kind, resource id, principal)` as the map stores it.
type GrantKey = (String, String, String);

/// Per-grant admission. The map holds one mutex per `(kind, resource, principal)`
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
    gate: Option<GrantGate>,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl GrantGates {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            gates: Mutex::new(HashMap::new()),
        })
    }

    /// Lock `(resource kind, resource id, principal)`. A caller that already
    /// holds the metadata connection must not call this: the connection is
    /// taken only after the permit, inside `run`.
    ///
    /// A wait cancelled before the permit is built removes the map entry when
    /// it was the last clone of that gate. The lock future is dropped before
    /// that count, so its `Arc` is not mistaken for another waiter.
    async fn lock(
        self: &Arc<Self>,
        kind: String,
        resource: String,
        principal: String,
    ) -> GrantPermit {
        let key = (kind, resource, principal);
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
                    gate: Some(this.gate.take().expect("a lock wait keeps its gate")),
                    guard: Some(guard),
                })
            }
        }
    }
}

impl Drop for LockWait {
    fn drop(&mut self) {
        // The lock future holds one Arc. Release it before this wait's own
        // clone, so the count is not that future.
        self.waiting.take();
        let Some(gate) = self.gate.take() else {
            return;
        };
        release_gate(&self.gates, std::mem::take(&mut self.key), gate);
    }
}

impl Drop for GrantPermit {
    fn drop(&mut self) {
        // Release waiters before the map lock. A woken waiter takes that
        // lock next.
        self.guard.take();
        let Some(gate) = self.gate.take() else {
            return;
        };
        release_gate(&self.gates, std::mem::take(&mut self.key), gate);
    }
}

/// Drop this caller's gate, then remove `key` when the map's gate is unused.
///
/// The clone is dropped while the map lock is held. A permit and a cancelled
/// wait that both counted before either dropped would each still see the
/// other and leave the entry behind.
fn release_gate(gates: &GrantGates, key: GrantKey, gate: GrantGate) {
    let mut map = gates.gates.lock().unwrap_or_else(PoisonError::into_inner);
    drop(gate);
    let idle = map
        .get(&key)
        .is_some_and(|current| Arc::strong_count(current) == 1);
    if idle {
        map.remove(&key);
    }
    drop(map);
    // After the decision, so a test can hold one releaser here while the
    // other still owns its clone. Dropping the clone only after this point
    // leaves the entry in the map.
    #[cfg(test)]
    if let Some(barrier) = RELEASE_PAUSE.with(|slot| slot.take()) {
        barrier.wait();
    }
}

#[cfg(test)]
thread_local! {
    static RELEASE_PAUSE: std::cell::Cell<Option<Arc<std::sync::Barrier>>> =
        const { std::cell::Cell::new(None) };
}

/// What one attempt to write a grant change did.
enum ChangeAttempt {
    Applied(ShareGrantReceipt),
    /// The principal this attempt locked is not the row it would write.
    Retry,
}

/// SQL naming the one grant row the receiver bound to parameter `receiver`
/// holds on the conversation whose id is `conversation`, as `g`. Both are
/// names this adapter writes, never a caller's text. [`granted`] asks whether
/// it exists and `held_grant` reads its capabilities and epoch, so the two cannot
/// disagree.
fn grant_row(conversation: &str, receiver: &str) -> String {
    format!(
        "share_grants AS g
         WHERE g.resource_kind = '{kind}' AND g.resource_id = {conversation}
           AND g.receiver_id = {receiver}",
        kind = CONVERSATION,
    )
}

/// SQL that is true when the receiver bound to parameter `receiver` holds a
/// grant on the conversation whose id is `conversation`.
pub(super) fn granted(conversation: &str, receiver: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM {})",
        grant_row(conversation, receiver)
    )
}

/// SQL naming the capabilities the receiver bound to parameter `receiver`
/// holds on the conversation whose id is `conversation`, as stored, or NULL
/// when it holds none: the row [`granted`] asks about. Read back by
/// [`stored_capabilities`].
pub(super) fn held_capabilities(conversation: &str, receiver: &str) -> String {
    format!(
        "(SELECT g.capabilities FROM {})",
        grant_row(conversation, receiver)
    )
}

/// The capabilities [`held_capabilities`] read for row `id`; a spelling this
/// adapter never writes is that row's damage.
pub(super) fn stored_capabilities(
    stored: Option<String>,
    id: &str,
) -> Result<Option<Capabilities>, ConversationError> {
    stored
        .map(|name| Capabilities::parse(&name).ok_or_else(|| unreadable("share_grants", id)))
        .transpose()
}

/// What a party holds on a conversation, as stored: the capabilities, or
/// [`NONE`], and the constraint text a drive grant carries.
type Held = (String, Option<String>);

/// The journal's spelling of "no grant", before a grant or after a revoke.
const NONE: &str = "none";

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
    Some(tools.with_command_hosts(hosts))
}

/// The longest policy text a valid policy writes: every pattern at its
/// longest and every byte of it a quote or backslash, which JSON writes in two
/// (a control character, which it writes in six, is refused by `ToolRules`),
/// every command host at its longest beside those patterns (a host has nothing
/// JSON escapes), and the object around them. A stored text past it is read as
/// damage before it is parsed.
const MAX_POLICY_TEXT_BYTES: usize = 2 * ToolRules::MAX_RULES * (2 * ToolRules::MAX_RULE_BYTES + 3)
    + CommandHosts::MAX_HOSTS * (CommandHosts::MAX_HOST_BYTES + 3)
    + 64;

/// A stored journal sequence, which SQLite numbers from 1.
fn sequence(value: i64) -> Option<u64> {
    u64::try_from(value).ok().filter(|sequence| *sequence > 0)
}

/// What a transition asks the party to hold.
fn wanted(transition: &ShareGrantTransition) -> Held {
    match transition {
        ShareGrantTransition::Grant(access) => (
            access.capabilities().as_str().to_owned(),
            access.policy().map(policy_text),
        ),
        ShareGrantTransition::Revoke => (NONE.to_owned(), None),
    }
}

/// The policy revision `principal`'s grant on conversation `id` held once
/// journal row `at` was written: the latest row at or before it that changed
/// the grant's capabilities or constraints, which is the row that stamped the grant then.
/// Read for a retried share, which answers what its first answer did; only
/// asked of a row that left the party a grant, so one such row exists.
fn revision_after(
    transaction: &Transaction<'_>,
    id: &str,
    principal: &str,
    at: i64,
) -> Result<u64, ConversationError> {
    let stamped: Option<i64> = transaction
        .cached_row(
            &format!(
                "SELECT MAX(sequence) FROM share_grant_changes
                 WHERE resource_kind = '{kind}' AND resource_id = ?1
                   AND principal_id = ?2 AND sequence <= ?3
                   AND NOT (before = after AND policy_before IS policy_after)",
                kind = CONVERSATION,
            ),
            params![id, principal, at],
            |row| row.get(0),
        )
        .map_err(failed)?;
    stamped
        .and_then(sequence)
        .ok_or_else(|| unreadable("share_grant_changes", id))
}

fn count(transaction: &Transaction<'_>, sql: &str, id: &str) -> Result<i64, ConversationError> {
    transaction
        .cached_row(sql, params![id], |row| row.get(0))
        .map_err(failed)
}

impl ShareGrants for LocalConversationStore {
    fn hold_principal_revision<'a>(
        &'a self,
        id: &'a ConversationId,
        principal: &'a PrincipalId,
        policy_revision: u64,
    ) -> ConversationFuture<'a, RevisionHold> {
        Box::pin(async move {
            let permit = self
                .grant_gates
                .lock(
                    CONVERSATION.to_owned(),
                    id.to_string(),
                    principal.as_str().to_owned(),
                )
                .await;
            match self.principal_grant(id, principal).await? {
                Some(grant) if grant.policy_revision == policy_revision => {
                    Ok(RevisionHold::held(permit))
                }
                _ => Err(ConversationError::Revoked),
            }
        })
    }

    fn held_grant<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<HeldGrant>> {
        let (id, receiver_id) = (id.to_string(), receiver_id.to_owned());
        self.run(move |connection| {
            let held = connection
                .cached_row(
                    &format!(
                        "SELECT g.capabilities, g.policy_revision FROM conversations AS c, {} AND c.id = ?1",
                        grant_row("c.id", "?2")
                    ),
                    params![id, receiver_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(failed)?;
            held.map(|(name, revision)| {
                Capabilities::parse(&name)
                    .zip(sequence(revision))
                    .map(|(capabilities, policy_revision)| HeldGrant {
                        capabilities,
                        policy_revision,
                    })
                    .ok_or_else(|| unreadable("share_grants", &id))
            })
            .transpose()
        })
    }

    fn change(&self, change: ShareGrantChange) -> ConversationFuture<'_, ShareGrantReceipt> {
        Box::pin(async move {
            // The principal a hold locks is the live row's, which a revoke
            // names only by credential. Look it up, lock that grant, and
            // write. If the row's principal is no longer the one locked, the
            // attempt writes nothing and tries the principal it found.
            for _ in 0..4 {
                let key = self.principal_to_lock(&change).await?;
                let permit = match &key {
                    Some(principal) => Some(
                        self.grant_gates
                            .lock(
                                CONVERSATION.to_owned(),
                                change.conversation_id.to_string(),
                                principal.clone(),
                            )
                            .await,
                    ),
                    None => None,
                };
                // The permit moves into the blocking write. Dropping this
                // future detaches that write; it does not drop the permit
                // while the write can still commit.
                match self.apply_change(change.clone(), key, permit).await? {
                    ChangeAttempt::Applied(receipt) => return Ok(receipt),
                    ChangeAttempt::Retry => continue,
                }
            }
            tracing::error!("a share grant change could not lock the principal it writes");
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
        let conversation = id.to_string();
        let read = self.live_revision(id, receiver_id);
        let principal = self.principal_of(id, receiver_id);
        Box::pin(async move {
            let Some(principal) = principal.await? else {
                return Err(ConversationError::Revoked);
            };
            let permit = gates
                .lock(CONVERSATION.to_owned(), conversation, principal)
                .await;
            match read.await? {
                Some(live) if live == policy_revision => Ok(RevisionHold::held(permit)),
                _ => Err(ConversationError::Revoked),
            }
        })
    }

    fn grants<'a>(&'a self, id: &'a ConversationId) -> ConversationFuture<'a, ShareGrantList> {
        let id = id.clone();
        self.run(move |connection| {
            // One read transaction. This store holds its own connection one
            // call at a time; another opener of the file can still commit
            // between two statements, and the revision would then name a
            // different catalogue than the grants.
            let transaction = connection.transaction().map_err(failed)?;
            let revision: i64 = transaction
                .cached_row(
                    "SELECT change_revision FROM conversations WHERE id = ?1",
                    [id.to_string()],
                    |row| row.get(0),
                )
                .map_err(failed)?;
            let change_revision = time(revision)
                .filter(|value| *value <= MAX_POLICY_REVISION)
                .ok_or_else(|| unreadable("conversations", &id.to_string()))?;
            let grants = {
                let mut statement = transaction
                    .prepare(&format!(
                        "SELECT principal_id, receiver_id, {}, capabilities, {}, policy_revision, granted_at_ms
                         FROM share_grants
                         WHERE resource_kind = '{kind}' AND resource_id = ?1
                         ORDER BY granted_at_ms, credential_id",
                        super::acquisition::text("credential_id", MAX_IDENTIFIER_BYTES, false),
                        super::acquisition::text("policy", MAX_POLICY_TEXT_BYTES, true),
                        kind = CONVERSATION,
                    ))
                    .map_err(failed)?;
                let mut rows = statement.query([id.to_string()]).map_err(failed)?;
                let mut grants = Vec::new();
                while let Some(row) = rows.next().map_err(failed)? {
                    let stored = cells((|| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    })())?;
                    let grant = stored.and_then(
                        |(
                            principal_id,
                            receiver_id,
                            credential,
                            capabilities,
                            policy,
                            policy_revision,
                            granted_at,
                        )| {
                            let policy = match policy {
                                Some(text) => Some(policy_from(&text)?),
                                None => None,
                            };
                            Some(ShareGrant {
                                conversation_id: id.clone(),
                                principal_id: nessa_auth::domain::PrincipalId::new(principal_id).ok()?,
                                receiver_id,
                                credential_id: CredentialId::new(credential).ok()?,
                                access: ShareAccess::new(
                                    Capabilities::parse(&capabilities)?,
                                    policy,
                                )
                                .ok()?,
                                policy_revision: sequence(policy_revision)?,
                                granted_at_ms: time(granted_at)?,
                            })
                        },
                    );
                    grants.push(grant.ok_or_else(|| unreadable("share_grants", &id.to_string()))?);
                }
                grants
            };
            transaction.commit().map_err(failed)?;
            Ok(ShareGrantList {
                grants,
                change_revision,
            })
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
                .cached_row(
                    &format!(
                        "SELECT j.after, {policy} FROM share_grant_changes AS j
                         WHERE j.sequence = ?1 AND j.resource_kind = '{kind}'
                           AND j.resource_id = ?2
                           AND (j.receiver_id = ?3 OR EXISTS (
                               SELECT 1 FROM share_grants AS g
                               WHERE g.resource_kind = j.resource_kind
                                 AND g.resource_id = j.resource_id
                                 AND g.principal_id = j.principal_id
                                 AND g.receiver_id = ?3
                                 AND g.policy_revision = j.sequence))",
                        policy =
                            super::acquisition::text("j.policy_after", MAX_POLICY_TEXT_BYTES, true),
                        kind = CONVERSATION,
                    ),
                    params![sequence, id, receiver_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(failed)?;
            match entry {
                Some((after, Some(text))) if after == Capabilities::drive().as_str() => {
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
    /// The principal `hold_revision` locks for `receiver` on `id`, or `None`
    /// when that receiver holds no grant.
    fn principal_of<'a>(
        &'a self,
        id: &'a ConversationId,
        receiver_id: &'a str,
    ) -> ConversationFuture<'a, Option<String>> {
        let (id, receiver_id) = (id.to_string(), receiver_id.to_owned());
        self.run(move |connection| {
            connection
                .cached_row(
                    &format!(
                        "SELECT principal_id FROM share_grants
                         WHERE resource_kind = '{kind}' AND resource_id = ?1
                           AND receiver_id = ?2",
                        kind = CONVERSATION,
                    ),
                    params![id, receiver_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)
        })
    }

    /// The principal a change locks: the live row's, or the one a new grant
    /// names. `None` when a revoke finds no row and names no principal, so
    /// there is no grant to exclude.
    fn principal_to_lock<'a>(
        &'a self,
        change: &'a ShareGrantChange,
    ) -> ConversationFuture<'a, Option<String>> {
        let id = change.conversation_id.to_string();
        let named = change
            .principal_id
            .as_ref()
            .map(|principal| principal.as_str().to_owned());
        let receiver = change.receiver_id.clone();
        let credential = change.credential_id.as_str().to_owned();
        self.run(move |connection| {
            let found: Option<String> = connection
                .cached_row(
                    &format!(
                        "SELECT principal_id FROM share_grants
                         WHERE resource_kind = '{kind}' AND resource_id = ?1
                           AND ((?2 IS NOT NULL AND principal_id = ?2)
                                OR (?2 IS NULL AND (credential_id = ?3
                                    OR (?4 IS NOT NULL AND receiver_id = ?4))))",
                        kind = CONVERSATION,
                    ),
                    params![id, named.clone(), credential, receiver],
                    |row| row.get(0),
                )
                .optional()
                .map_err(failed)?;
            Ok(found.or(named))
        })
    }

    /// Write `change` while `expected` is the principal the caller has locked.
    /// `Retry` when that is not the row this write would touch; nothing is
    /// committed. The connection is taken here, after the grant lock.
    ///
    /// `permit` is held by this blocking write until the transaction ends,
    /// including when the awaiting caller is cancelled. Dropping the returned
    /// future detaches the write; it does not release the gate first.
    fn apply_change(
        &self,
        change: ShareGrantChange,
        expected: Option<String>,
        permit: Option<GrantPermit>,
    ) -> ConversationFuture<'_, ChangeAttempt> {
        let changes = self.changes.clone();
        let access = self.access.clone();
        let connection = self.connection.clone();
        #[cfg(test)]
        let pause = self
            .grant_write_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                // Held until this task returns, after the transaction commits
                // or rolls back. Dropping the caller's future detaches the
                // task; it does not release the gate while the write can
                // still commit.
                let _permit = permit;
                #[cfg(test)]
                if let Some(pause) = pause {
                    pause.block_until_released();
                }
                let mut connection = connection
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(failed)?;
                let id = change.conversation_id.to_string();
                let conversation = read(&transaction, &change.conversation_id)?
                    .ok_or(ConversationError::NotFound)?;
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
                OwnerAccess::new(access).admit_to(
                    action,
                    &conversation,
                    organization,
                    principal,
                )?;
                let after = wanted(&change.transition);
                // A retried request answers what it answered the first time, so
                // a share whose reply was lost cannot, retried after a later
                // unshare, grant again (row G16). That holds for a request that
                // changed nothing too: it is journaled with before equal to after.
                // The same request naming another principal, device, capabilities
                // or constraints, or the other change is refused (row J6). A retry is answered
                // before the expected revision is compared: its first answer may
                // be what moved the grant past it (row J29).
                let replayed: Option<(String, Option<String>, Held, Held, String, i64)> =
                    transaction
                        .cached_row(
                            &format!(
                                "SELECT credential_id, receiver_id, before, policy_before, after,
                                policy_after, principal_id, sequence
                             FROM share_grant_changes
                             WHERE resource_kind = '{CONVERSATION}' AND resource_id = ?1
                               AND initiator = ?2 AND surface = ?3 AND request = ?4"
                            ),
                            params![
                                id,
                                principal.as_str(),
                                change.initiator.surface_id,
                                change.initiator.action_id
                            ],
                            |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    (row.get(2)?, row.get(3)?),
                                    (row.get(4)?, row.get(5)?),
                                    row.get(6)?,
                                    row.get(7)?,
                                ))
                            },
                        )
                        .optional()
                        .map_err(failed)?;
                if let Some((
                    credential,
                    journaled_receiver,
                    journaled_before,
                    journaled_after,
                    granted,
                    at,
                )) = replayed
                {
                    // The request's identity is the principal, the receiver, and
                    // the credential it first named. A retry that names another
                    // of those, on a grant or a revoke, answers nothing and
                    // writes nothing. Omitting the principal or the receiver
                    // is the lost reply, and answers the first receipt.
                    let other_principal = change
                        .principal_id
                        .as_ref()
                        .is_some_and(|named| named.as_str() != granted);
                    let other_receiver = change.receiver_id.is_some()
                        && change.receiver_id.as_deref() != journaled_receiver.as_deref();
                    if credential != change.credential_id.as_str()
                        || journaled_after != after
                        || other_principal
                        || other_receiver
                    {
                        return Err(ConversationError::InvalidInput);
                    }
                    // A retry answers the first receipt. A switch of receiver
                    // or credential applies even when the capability set does
                    // not change, so the receipt is not that comparison alone.
                    let replaced: Option<(Option<String>, String)> = transaction
                        .cached_row(
                            &format!(
                                "SELECT receiver_id, credential_id FROM share_grant_changes
                                 WHERE resource_kind = '{CONVERSATION}' AND resource_id = ?1
                                   AND principal_id = ?2 AND sequence < ?3
                                 ORDER BY sequence DESC LIMIT 1"
                            ),
                            params![id, granted, at],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()
                        .map_err(failed)?;
                    let party_changed = replaced.is_some_and(|(receiver, previous)| {
                        receiver != journaled_receiver || previous != credential
                    });
                    let policy_revision = if journaled_after.0 == NONE {
                        None
                    } else {
                        Some(revision_after(&transaction, &id, &granted, at)?)
                    };
                    return Ok(ChangeAttempt::Applied(ShareGrantReceipt {
                        applied: journaled_before != journaled_after || party_changed,
                        policy_revision,
                    }));
                }
                let named_principal = change
                    .principal_id
                    .as_ref()
                    .map(|principal| principal.as_str().to_owned());
                // A named principal resolves only its own row. Credential and
                // receiver match when the change names no principal, which is
                // how a revoke finds the grant.
                let existing: Option<(String, Option<String>, String, Held, i64)> = transaction
                    .cached_row(
                        &format!(
                            "SELECT principal_id, receiver_id, credential_id, capabilities, policy,
                                    policy_revision
                             FROM share_grants
                             WHERE resource_kind = '{CONVERSATION}' AND resource_id = ?1
                               AND ((?2 IS NOT NULL AND principal_id = ?2)
                                    OR (?2 IS NULL AND (credential_id = ?3
                                        OR (?4 IS NOT NULL AND receiver_id = ?4))))"
                        ),
                        params![
                            id,
                            named_principal,
                            change.credential_id.as_str(),
                            change.receiver_id,
                        ],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                (row.get(3)?, row.get(4)?),
                                row.get(5)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(failed)?;
                let row_present = existing.is_some();
                let held_revision = existing
                    .as_ref()
                    .map(|(_, _, _, _, stored)| {
                        sequence(*stored).ok_or_else(|| unreadable("share_grants", &id))
                    })
                    .transpose()?;
                let (principal, stored_receiver, stored_credential, before, stored_epoch) =
                    match existing {
                        Some((principal, receiver, credential, held, epoch)) => {
                            (principal, receiver, Some(credential), held, Some(epoch))
                        }
                        None => match &change.transition {
                            ShareGrantTransition::Grant(_) => (
                                named_principal.ok_or(ConversationError::InvalidInput)?,
                                None,
                                None,
                                (NONE.to_owned(), None),
                                None,
                            ),
                            ShareGrantTransition::Revoke => (
                                named_principal.unwrap_or_default(),
                                None,
                                None,
                                (NONE.to_owned(), None),
                                None,
                            ),
                        },
                    };
                // A grant stores the receiver it names, or keeps the one the
                // row already has when this change names none. A revoke
                // journals the receiver the row held.
                let receiver = match &change.transition {
                    ShareGrantTransition::Grant(_) => {
                        change.receiver_id.clone().or(stored_receiver.clone())
                    }
                    ShareGrantTransition::Revoke => {
                        stored_receiver.clone().or(change.receiver_id.clone())
                    }
                };
                // The permit the caller holds is for `expected`. A row that names
                // another principal, or one that appeared after a revoke looked
                // and found nothing, is written on the next attempt under that
                // principal's permit. Nothing is committed here.
                let covered = match &expected {
                    Some(locked) => locked == &principal,
                    None => !row_present,
                };
                if !covered {
                    return Ok(ChangeAttempt::Retry);
                }
                // Compared in the transaction that writes the change, and only
                // once this attempt holds that principal's permit, so no other
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
                // Capabilities and constraints are one change. Replacing the
                // receiver or the credential is another: the row has to name
                // the party that now holds it, and both catalogues have to
                // move. The grant epoch stays when the set and the
                // constraints do not.
                let access_changed = before != after;
                let party_changed = stored_credential.is_some()
                    && matches!(change.transition, ShareGrantTransition::Grant(_))
                    && (stored_receiver.as_deref() != receiver.as_deref()
                        || stored_credential.as_deref() != Some(change.credential_id.as_str()));
                let applied = access_changed || party_changed;
                // The bounds a new grant, and a change to Drive, must fit (rows
                // G17 and J9), checked before anything is written.
                if applied && before.0 == NONE && after.0 != NONE {
                    let held = count(
                        &transaction,
                        &format!(
                            "SELECT COUNT(*) FROM share_grants
                             WHERE resource_kind = '{CONVERSATION}' AND resource_id = ?1"
                        ),
                        &id,
                    )?;
                    if held >= MAX_SHARE_GRANTS_PER_CONVERSATION {
                        return Err(ConversationError::InvalidInput);
                    }
                }
                let drive = Capabilities::drive().as_str();
                if applied && before.0 != drive && after.0 == drive {
                    let driving = count(
                        &transaction,
                        &format!(
                            "SELECT COUNT(*) FROM share_grants
                             WHERE resource_kind = '{CONVERSATION}' AND resource_id = ?1
                               AND capabilities = '{drive}'"
                        ),
                        &id,
                    )?;
                    if driving >= MAX_DRIVE_GRANTS_PER_CONVERSATION {
                        return Err(ConversationError::InvalidInput);
                    }
                }
                // A grant, a revoke, or a replacement of the receiver or
                // credential is a catalogue change: the device's next pass
                // starts past what it completed, so a newly granted row must
                // carry a revision past it (row G5). A change of capabilities
                // or constraints, for the same party, leaves what the device
                // reads as it was, so it keeps the revision it found, as a
                // request that changed nothing does (rows G9, J4).
                let reads_change = (before.0 == NONE) != (after.0 == NONE) || party_changed;
                let revision = if reads_change {
                    let revision = next_revision(&transaction, &conversation, false)?;
                    transaction
                        .cached_execute(
                            "UPDATE conversations SET change_revision = ?1 WHERE id = ?2",
                            params![revision, id],
                        )
                        .map_err(failed)?;
                    revision
                } else {
                    transaction
                        .cached_row(
                            "SELECT change_revision FROM conversations WHERE id = ?1",
                            params![id],
                            |row| row.get(0),
                        )
                        .map_err(failed)?
                };
                let at = stored_time(change.at_ms)?;
                transaction
                    .cached_execute(
                        "INSERT INTO share_grant_changes
                         (resource_kind, resource_id, principal_id, receiver_id, credential_id,
                          before, after, policy_before, policy_after, initiator, surface, request,
                          changed_at_ms, revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                        params![
                            CONVERSATION,
                            id,
                            principal,
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
                // The capabilities and constraints a grant holds are stamped with the journal
                // row that set them, so a later change moves its policy revision
                // and a request that changed nothing leaves it (rows J4, J5).
                let sequence = transaction.last_insert_rowid();
                let policy_revision = if after.0 == NONE {
                    None
                } else if access_changed {
                    Some(
                        self::sequence(sequence)
                            .ok_or_else(|| unreadable("share_grant_changes", &id))?,
                    )
                } else {
                    held_revision
                };
                // The epoch moves only with the capabilities or the
                // constraints. A party switch stamps the epoch the row
                // already held.
                let stamped_epoch = if access_changed {
                    sequence
                } else {
                    // A no-op revoke has no row and no epoch. Nothing is
                    // stamped; the value is unused when the change is not
                    // applied.
                    stored_epoch.unwrap_or(sequence)
                };
                if applied {
                    match (before.0.as_str(), after.0.as_str()) {
                        (_, NONE) => transaction.cached_execute(
                            "DELETE FROM share_grants
                         WHERE resource_kind = ?1 AND resource_id = ?2 AND principal_id = ?3",
                            params![CONVERSATION, id, principal],
                        ),
                        (NONE, _) => transaction.cached_execute(
                            "INSERT INTO share_grants
                             (principal_id, resource_kind, resource_id, receiver_id, credential_id,
                              capabilities, policy, policy_revision, granted_at_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                            params![
                                principal,
                                CONVERSATION,
                                id,
                                receiver,
                                change.credential_id.as_str(),
                                after.0,
                                after.1,
                                stamped_epoch,
                                at
                            ],
                        ),
                        _ => transaction.cached_execute(
                            "UPDATE share_grants
                         SET receiver_id = ?4, credential_id = ?5, capabilities = ?6, policy = ?7,
                             policy_revision = ?8
                         WHERE resource_kind = ?1 AND resource_id = ?2 AND principal_id = ?3",
                            params![
                                CONVERSATION,
                                id,
                                principal,
                                receiver,
                                change.credential_id.as_str(),
                                after.0,
                                after.1,
                                stamped_epoch
                            ],
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
                    capabilities_before = before.0.as_str(),
                    capabilities_after = after.0.as_str(),
                    receiver_before = stored_receiver.as_deref().unwrap_or(""),
                    receiver_after = receiver.as_deref().unwrap_or(""),
                    initiator = change.initiator.principal_id.as_str(),
                    revision,
                    ?policy_revision,
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
            .await
            .map_err(|_| ConversationError::Metadata)?
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
                .cached_row(
                    &format!(
                        "SELECT policy_revision FROM share_grants
                         WHERE resource_kind = '{CONVERSATION}' AND resource_id = ?1
                           AND receiver_id = ?2"
                    ),
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
        let key = (
            "conversation".to_owned(),
            "chat".to_owned(),
            "phone".to_owned(),
        );
        let holder = gates
            .lock(key.0.clone(), key.1.clone(), key.2.clone())
            .await;
        let gates_for_wait = Arc::clone(&gates);
        let kind = key.0.clone();
        let resource = key.1.clone();
        let principal = key.2.clone();
        let waiter = tokio::spawn(async move {
            let _permit = gates_for_wait.lock(kind, resource, principal).await;
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

    /// The permit and a cancelled wait release the same gate together. They
    /// meet after each has finished the map update and before returning, so
    /// a clone dropped only after that meeting is still visible to the other.
    #[test]
    fn a_permit_and_a_cancelled_wait_that_release_together_leave_the_map_empty() {
        let gates = GrantGates::new();
        let key = (
            "conversation".to_owned(),
            "chat".to_owned(),
            "phone".to_owned(),
        );
        let gate = Arc::new(tokio::sync::Mutex::new(()));
        {
            let mut map = gates.gates.lock().unwrap_or_else(PoisonError::into_inner);
            map.insert(key.clone(), Arc::clone(&gate));
        }
        let first = Arc::clone(&gate);
        let second = gate;
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let gates_a = Arc::clone(&gates);
        let gates_b = Arc::clone(&gates);
        let key_a = key.clone();
        let pause_a = Arc::clone(&barrier);
        let pause_b = barrier;
        let left = std::thread::spawn(move || {
            RELEASE_PAUSE.with(|slot| slot.set(Some(pause_a)));
            release_gate(&gates_a, key_a, first);
        });
        let right = std::thread::spawn(move || {
            RELEASE_PAUSE.with(|slot| slot.set(Some(pause_b)));
            release_gate(&gates_b, key, second);
        });
        left.join().expect("one releaser");
        right.join().expect("the other releaser");
        let map = gates.gates.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(
            map.is_empty(),
            "a permit and a cancelled wait left a gate in the map"
        );
    }
}
