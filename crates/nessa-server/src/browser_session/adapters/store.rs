use crate::browser_session::{
    application::SessionStore,
    domain::value_objects::{BrowserSessionOrigin, BrowserSessionState, RemovalReason},
};
use nessa_auth::application::ports::{AccessError, PortFuture};
use nessa_auth::domain::CredentialId;
use nessa_local_storage::{open, OpenMode};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Reason {
    SignIn,
    AbandonedLogin,
    RestoredAfterAbandonedLogin,
    Renewed,
    SignOut,
    IdleExpired,
    FutureRenewal,
    CredentialRevoked,
    CredentialExpired,
    InactiveMembership,
    IdentityMismatch,
    InvalidCredential,
    Replaced,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSession {
    credential_id: String,
    created_at: u64,
    renewed_at: u64,
    origin: String,
    idle_expires_at: u64,
}
impl TryFrom<StoredSession> for BrowserSessionState {
    type Error = AccessError;
    fn try_from(value: StoredSession) -> Result<Self, Self::Error> {
        BrowserSessionState::restore(
            CredentialId::new(value.credential_id).map_err(|_| AccessError::Unavailable)?,
            BrowserSessionOrigin::new(value.origin).ok_or(AccessError::Unavailable)?,
            value.created_at,
            value.renewed_at,
            value.idle_expires_at,
        )
        .ok_or(AccessError::Unavailable)
    }
}
impl From<&BrowserSessionState> for StoredSession {
    fn from(value: &BrowserSessionState) -> Self {
        Self {
            credential_id: value.credential_id().as_str().to_owned(),
            created_at: value.created_at(),
            renewed_at: value.renewed_at(),
            origin: value.origin().to_owned(),
            idle_expires_at: value.idle_expires_at(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredChange {
    id: String,
    before: Option<StoredSession>,
    after: Option<StoredSession>,
    reason: Reason,
    initiator: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    sequence: u64,
    at: u64,
    changes: Vec<StoredChange>,
}
#[derive(Clone, Debug)]
struct Change {
    id: String,
    before: Option<BrowserSessionState>,
    after: Option<BrowserSessionState>,
    reason: Reason,
    initiator: Option<CredentialId>,
}
struct Record {
    sequence: u64,
    at: u64,
    changes: Vec<Change>,
}
impl TryFrom<StoredRecord> for Record {
    type Error = AccessError;
    fn try_from(value: StoredRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            sequence: value.sequence,
            at: value.at,
            changes: value
                .changes
                .into_iter()
                .map(|change| {
                    Ok(Change {
                        id: change.id,
                        before: change.before.map(TryInto::try_into).transpose()?,
                        after: change.after.map(TryInto::try_into).transpose()?,
                        reason: change.reason,
                        initiator: change
                            .initiator
                            .map(CredentialId::new)
                            .transpose()
                            .map_err(|_| AccessError::Unavailable)?,
                    })
                })
                .collect::<Result<_, AccessError>>()?,
        })
    }
}
impl From<&Record> for StoredRecord {
    fn from(value: &Record) -> Self {
        Self {
            sequence: value.sequence,
            at: value.at,
            changes: value
                .changes
                .iter()
                .map(|change| StoredChange {
                    id: change.id.clone(),
                    before: change.before.as_ref().map(StoredSession::from),
                    after: change.after.as_ref().map(StoredSession::from),
                    reason: change.reason.clone(),
                    initiator: change
                        .initiator
                        .as_ref()
                        .map(|value| value.as_str().to_owned()),
                })
                .collect(),
        }
    }
}
/// The journal file, whose advisory lock is given up explicitly.
///
/// An `flock` belongs to the open file description, not to the descriptor. A
/// subprocess forked while this one is open keeps a duplicate of that
/// description until it execs — `O_CLOEXEC` closes the descriptor there, not
/// at the fork — so closing ours alone would leave the journal locked by a
/// child that has no interest in it, for as long as it takes to exec. Only
/// unlocking releases the description itself, which is what every holder of
/// it sees, so it happens here rather than being left to a close.
struct Journal(File);
impl std::ops::Deref for Journal {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}
impl std::ops::DerefMut for Journal {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.0
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        if let Err(error) = self.0.unlock() {
            tracing::error!(%error, "browser session journal unlock failed");
        }
    }
}
struct State {
    sessions: BTreeMap<String, BrowserSessionState>,
    login_replacements: BTreeMap<String, Option<(String, BrowserSessionState)>>,
    sequence: u64,
    file: Option<Journal>,
    max_journal_bytes: u64,
    healthy: bool,
}

/// Private append-only session journal. Each acknowledged write includes its audit
/// evidence and is synced before publication. A write failure fails closed until restart.
pub struct PersistentSessions(Arc<Mutex<State>>, Arc<Semaphore>);
impl PersistentSessions {
    /// Open the journal and reconcile what it claims about time with `now`.
    ///
    /// `now` is read from the same wall clock the store's callers use.
    pub fn open(path: &Path, now: u64) -> Result<Self, AccessError> {
        Self::open_bounded(path, MAX_JOURNAL_BYTES, now)
    }

    fn open_bounded(path: &Path, max_journal_bytes: u64, now: u64) -> Result<Self, AccessError> {
        if max_journal_bytes == 0 {
            return Err(AccessError::Unavailable);
        }
        let file = open(path, OpenMode::OpenOrCreate).map_err(|_| AccessError::Unavailable)?;
        file.try_lock().map_err(|_| AccessError::Unavailable)?;
        // Owned from the instant the lock is taken, so every path out of this
        // function — including the refusals below — releases it.
        let mut file = Journal(file);
        if file.metadata().map_err(|_| AccessError::Unavailable)?.len() > max_journal_bytes {
            return Err(AccessError::Unavailable);
        }
        file.sync_all().map_err(|_| AccessError::Unavailable)?;
        nessa_local_storage::sync_directory(path.parent().ok_or(AccessError::Unavailable)?)
            .map_err(|_| AccessError::Unavailable)?;
        let mut state = State {
            sessions: BTreeMap::new(),
            login_replacements: BTreeMap::new(),
            sequence: 0,
            file: None,
            max_journal_bytes,
            healthy: true,
        };
        let mut reader = BufReader::new(&mut *file);
        let mut line = Vec::new();
        loop {
            line.clear();
            // A record is bounded before allocating untrusted input.
            let count = std::io::Read::take(&mut reader, 1_048_577)
                .read_until(b'\n', &mut line)
                .map_err(|_| AccessError::Unavailable)?;
            if count == 0 {
                break;
            }
            if count > 1_048_576 || line.last() != Some(&b'\n') {
                return Err(AccessError::Unavailable);
            }
            let record: StoredRecord =
                serde_json::from_slice(&line).map_err(|_| AccessError::Unavailable)?;
            state.apply(&record.try_into()?)?;
        }
        file.seek(SeekFrom::End(0))
            .map_err(|_| AccessError::Unavailable)?;
        state.file = Some(file);
        // A record's own instant is untrusted input: nothing in the file
        // constrains it, so a forged or clock-damaged `Renewed` chain can claim
        // any deadline. Bounding the *outcome* against a clock reading here
        // caps that at one idle window past this start, for every rule that
        // reads `record.at`, without the store refusing to open. Refusing would
        // turn a backwards clock step into a lockout with no way out, and
        // keeping a store-wide instant in memory would wedge every later write
        // — both of which is why the ordering guard tried in #49 was reverted.
        // A forged *expiry* remains possible and is not worth that trade: the
        // journal is private and exclusively locked, and whoever can forge one
        // can already destroy sessions by truncating the file.
        let swept: Vec<Change> = state
            .sessions
            .iter()
            .filter(|(_, session)| !session.is_plausible_at(now))
            .map(|(id, session)| Change {
                id: id.clone(),
                before: Some(session.clone()),
                after: None,
                reason: Reason::FutureRenewal,
                initiator: None,
            })
            .collect();
        state.commit(now, swept)?;
        Ok(Self(
            Arc::new(Mutex::new(state)),
            Arc::new(Semaphore::new(1)),
        ))
    }
    fn memory() -> Self {
        Self(
            Arc::new(Mutex::new(State {
                sessions: BTreeMap::new(),
                login_replacements: BTreeMap::new(),
                sequence: 0,
                file: None,
                max_journal_bytes: u64::MAX,
                healthy: true,
            })),
            Arc::new(Semaphore::new(1)),
        )
    }
    fn run<T, F>(&self, operation: F) -> PortFuture<'static, T>
    where
        T: Send + 'static,
        F: FnOnce(&mut State) -> Result<T, AccessError> + Send + 'static,
    {
        let shared = self.0.clone();
        let blocking = self.1.clone();
        Box::pin(async move {
            let permit = blocking
                .acquire_owned()
                .await
                .map_err(|_| AccessError::Unavailable)?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let mut state = shared.lock().map_err(|_| AccessError::Unavailable)?;
                if !state.healthy {
                    return Err(AccessError::Unavailable);
                }
                operation(&mut state)
            })
            .await
            .map_err(|_| AccessError::Unavailable)?
        })
    }
}
impl State {
    fn apply(&mut self, record: &Record) -> Result<(), AccessError> {
        if record.sequence != self.sequence + 1
            || record.changes.is_empty()
            || record.changes.len() > 130
        {
            return Err(AccessError::Unavailable);
        }
        let sign_ins: Vec<_> = record
            .changes
            .iter()
            .filter(|change| matches!(change.reason, Reason::SignIn))
            .collect();
        let replacements: Vec<_> = record
            .changes
            .iter()
            .filter(|change| matches!(change.reason, Reason::Replaced))
            .collect();
        if sign_ins.len() > 1
            || replacements.len() > 1
            || (!replacements.is_empty() && sign_ins.len() != 1)
            || replacements
                .first()
                .zip(sign_ins.first())
                .is_some_and(|(replaced, sign_in)| replaced.id == sign_in.id)
        {
            return Err(AccessError::Unavailable);
        }
        let login_provenance = sign_ins.first().map(|sign_in| {
            (
                sign_in.id.clone(),
                replacements.first().and_then(|replaced| {
                    replaced
                        .before
                        .clone()
                        .map(|prior| (replaced.id.clone(), prior))
                }),
            )
        });
        let mut next = self.sessions.clone();
        let mut next_login_replacements = self.login_replacements.clone();
        for change in &record.changes {
            if change.before.as_ref().is_some_and(|s| {
                record.at < s.renewed_at() && !matches!(change.reason, Reason::FutureRenewal)
            }) || change.id.len() != 64
                || !change.id.bytes().all(|v| v.is_ascii_hexdigit())
                || next.get(&change.id) != change.before.as_ref()
            {
                return Err(AccessError::Unavailable);
            }
            let valid = match (&change.reason, &change.before, &change.after) {
                (Reason::SignIn, None, Some(after)) => {
                    after.created_at() == record.at
                        && after.renewed_at() == record.at
                        && change.initiator.as_ref() == Some(after.credential_id())
                }
                (Reason::Renewed, Some(before), Some(after)) => {
                    before.renewed_for_check(record.at).as_ref() == Some(after)
                        && before != after
                        && change.initiator.as_ref() == Some(before.credential_id())
                }
                (Reason::SignOut, Some(before), None) => {
                    change.initiator.as_ref() == Some(before.credential_id())
                }
                (Reason::AbandonedLogin, Some(before), None) => {
                    let restored = record.changes.iter().find_map(|candidate| {
                        matches!(candidate.reason, Reason::RestoredAfterAbandonedLogin)
                            .then(|| {
                                candidate
                                    .after
                                    .clone()
                                    .map(|session| (candidate.id.clone(), session))
                            })
                            .flatten()
                    });
                    // Provenance and restoration eligibility are separate questions.
                    //
                    // Eligibility is judged against the record's own instant, as
                    // `IdleExpired` and `Renewed` already are. What that instant
                    // may claim is bounded at `open`, which retires any live
                    // state a clock cannot vouch for. A remembered prior needs
                    // no such bound of its own: the record that replaced it
                    // carried it as a `before`, so it could not precede that
                    // record, and this one cannot precede that one either.
                    // The record must name the prior this login actually replaced,
                    // but a prior that is not restorable at this record's instant
                    // — outside its own active window, or whose ID a later
                    // transition already owns — is legitimately left where it is.
                    // Reclaiming the abandoned login must not depend on being
                    // able to resurrect what it replaced.
                    let remembered = self.login_replacements.get(&change.id);
                    let provenance = match (remembered, &restored) {
                        (Some(Some(prior)), Some(restored)) => prior == restored,
                        (Some(remembered), None) => {
                            remembered.as_ref().is_none_or(|(prior_id, prior)| {
                                !prior.is_active_at(record.at)
                                    || self.sessions.contains_key(prior_id)
                            })
                        }
                        _ => false,
                    };
                    change.initiator.is_none()
                        && provenance
                        && ((restored.is_none() && record.changes.len() == 1)
                            || (restored.is_some() && record.changes.len() == 2))
                        && restored
                            .as_ref()
                            .is_none_or(|(_, restored)| restored.origin() == before.origin())
                }
                (Reason::RestoredAfterAbandonedLogin, None, Some(after)) => {
                    let abandoned = record
                        .changes
                        .iter()
                        .find(|candidate| matches!(candidate.reason, Reason::AbandonedLogin));
                    change.initiator.is_none()
                        && record.changes.len() == 2
                        && after.is_active_at(record.at)
                        && abandoned.is_some_and(|abandoned| {
                            self.login_replacements.get(&abandoned.id)
                                == Some(&Some((change.id.clone(), after.clone())))
                        })
                }
                (Reason::IdleExpired, Some(before), None) => {
                    !before.is_active_at(record.at) && change.initiator.is_none()
                }
                // The one transition whose `before` may postdate the record: it
                // exists precisely to retire state no clock could vouch for.
                // The condition is self-contained, so replaying this record
                // later under any clock reaches the same verdict.
                (Reason::FutureRenewal, Some(before), None) => {
                    !before.is_plausible_at(record.at) && change.initiator.is_none()
                }
                (
                    Reason::CredentialRevoked
                    | Reason::CredentialExpired
                    | Reason::InactiveMembership
                    | Reason::IdentityMismatch
                    | Reason::InvalidCredential,
                    Some(_),
                    None,
                ) => change.initiator.is_none(),
                (Reason::Replaced, Some(before), None) => record.changes.iter().any(|candidate| {
                    matches!(candidate.reason, Reason::SignIn)
                        && candidate.after.as_ref().is_some_and(|after| {
                            after.origin() == before.origin()
                                && change.initiator.as_ref() == Some(after.credential_id())
                        })
                }),
                _ => false,
            };
            if !valid {
                return Err(AccessError::Unavailable);
            }
            match &change.after {
                Some(value) => {
                    next.insert(change.id.clone(), value.clone());
                }
                None => {
                    next.remove(&change.id);
                }
            }
            if change.before.is_some() {
                next_login_replacements.remove(&change.id);
            }
        }
        if next.len() > 128 {
            return Err(AccessError::Unavailable);
        }
        self.sessions = next;
        if let Some((id, prior)) = login_provenance {
            next_login_replacements.insert(id, prior);
        }
        self.login_replacements = next_login_replacements;
        self.sequence = record.sequence;
        Ok(())
    }
    fn commit(&mut self, at: u64, changes: Vec<Change>) -> Result<(), AccessError> {
        if changes.is_empty() {
            return Ok(());
        }
        let record = Record {
            sequence: self.sequence + 1,
            at,
            changes,
        };
        // Validate using the same replay rules before touching the durable sink.
        let mut proposed = State {
            sessions: self.sessions.clone(),
            login_replacements: self.login_replacements.clone(),
            sequence: self.sequence,
            file: None,
            max_journal_bytes: self.max_journal_bytes,
            healthy: true,
        };
        proposed.apply(&record)?;
        let mut bytes = serde_json::to_vec(&StoredRecord::from(&record))
            .map_err(|_| AccessError::Unavailable)?;
        bytes.push(b'\n');
        if bytes.len() > 1_048_576 {
            return Err(AccessError::Unavailable);
        }
        if let Some(file) = &mut self.file {
            let projected = file
                .metadata()
                .map_err(|_| AccessError::Unavailable)?
                .len()
                .checked_add(u64::try_from(bytes.len()).map_err(|_| AccessError::Unavailable)?)
                .ok_or(AccessError::Unavailable)?;
            if projected > self.max_journal_bytes {
                return Err(AccessError::Unavailable);
            }
            if file
                .write_all(&bytes)
                .and_then(|()| file.sync_all())
                .is_err()
            {
                self.healthy = false;
                return Err(AccessError::Unavailable);
            }
        }
        self.sessions = proposed.sessions;
        self.login_replacements = proposed.login_replacements;
        self.sequence = proposed.sequence;
        Ok(())
    }
}
impl SessionStore for PersistentSessions {
    fn insert<'a>(
        &'a self,
        id: String,
        session: BrowserSessionState,
        prior: Option<String>,
        now: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSessionState)>> {
        self.run(move |state| {
            if prior
                .as_ref()
                .is_some_and(|prior_id| !state.sessions.contains_key(prior_id))
            {
                return Err(AccessError::InvalidCredential);
            }
            let replaced = prior.as_ref().and_then(|prior_id| {
                state
                    .sessions
                    .get(prior_id)
                    .filter(|session| session.expiration_reason(now).is_none())
                    .cloned()
                    .map(|session| (prior_id.clone(), session))
            });
            let mut changes = Vec::new();
            for (key, value) in &state.sessions {
                if value.expiration_reason(now).is_some() || prior.as_deref() == Some(key.as_str())
                {
                    let expiration = value.expiration_reason(now);
                    changes.push(Change {
                        id: key.clone(),
                        before: Some(value.clone()),
                        after: None,
                        reason: match expiration {
                            Some(_) => Reason::IdleExpired,
                            None => Reason::Replaced,
                        },
                        initiator: if expiration.is_some() {
                            None
                        } else {
                            Some(session.credential_id().clone())
                        },
                    });
                }
            }
            if state.sessions.len() - changes.len() >= 128 || state.sessions.contains_key(&id) {
                return Err(AccessError::Unavailable);
            }
            changes.push(Change {
                id,
                before: None,
                after: Some(session.clone()),
                reason: Reason::SignIn,
                initiator: Some(session.credential_id().clone()),
            });
            state.commit(now, changes)?;
            Ok(replaced)
        })
    }
    fn get<'a>(&'a self, id: String) -> PortFuture<'a, Option<BrowserSessionState>> {
        self.run(move |state| Ok(state.sessions.get(&id).cloned()))
    }
    fn remove<'a>(
        &'a self,
        id: String,
        now: u64,
        reason: RemovalReason,
        initiator: Option<CredentialId>,
    ) -> PortFuture<'a, ()> {
        self.run(move |state| {
            let Some(before) = state.sessions.get(&id).cloned() else {
                return Ok(());
            };
            let expected_initiator =
                (reason == RemovalReason::SignOut).then(|| before.credential_id().clone());
            if initiator != expected_initiator {
                return Err(AccessError::IdentityMismatch);
            }
            let reason = match reason {
                RemovalReason::AbandonedLogin => Reason::AbandonedLogin,
                RemovalReason::SignOut => Reason::SignOut,
                RemovalReason::IdleExpired => Reason::IdleExpired,
                RemovalReason::CredentialRevoked => Reason::CredentialRevoked,
                RemovalReason::CredentialExpired => Reason::CredentialExpired,
                RemovalReason::InactiveMembership => Reason::InactiveMembership,
                RemovalReason::IdentityMismatch => Reason::IdentityMismatch,
                RemovalReason::InvalidCredential => Reason::InvalidCredential,
            };
            state.commit(
                now,
                vec![Change {
                    id,
                    before: Some(before),
                    after: None,
                    reason,
                    initiator,
                }],
            )
        })
    }
    fn renew<'a>(
        &'a self,
        id: String,
        now: u64,
        initiator: CredentialId,
    ) -> PortFuture<'a, BrowserSessionState> {
        self.run(move |state| {
            let before = state
                .sessions
                .get(&id)
                .cloned()
                .ok_or(AccessError::InvalidCredential)?;
            if &initiator != before.credential_id() {
                return Err(AccessError::IdentityMismatch);
            }
            let after = before
                .renewed_for_check(now)
                .ok_or(AccessError::InvalidCredential)?;
            // Renew at most hourly; the returned cookie always uses the stored deadline.
            if after == before {
                return Ok(before);
            }
            state.commit(
                now,
                vec![Change {
                    id,
                    before: Some(before),
                    after: Some(after.clone()),
                    reason: Reason::Renewed,
                    initiator: Some(initiator),
                }],
            )?;
            Ok(after)
        })
    }

    fn abandon_login<'a>(
        &'a self,
        id: String,
        prior: Option<(String, BrowserSessionState)>,
        now: u64,
    ) -> PortFuture<'a, ()> {
        self.run(move |state| {
            let Some(abandoned) = state.sessions.get(&id).cloned() else {
                return Ok(());
            };
            if state.login_replacements.get(&id) != Some(&prior) {
                return Err(AccessError::IdentityMismatch);
            }
            let mut changes = vec![Change {
                id,
                before: Some(abandoned),
                after: None,
                reason: Reason::AbandonedLogin,
                initiator: None,
            }];
            if let Some((prior_id, prior)) = prior.filter(|(_, session)| session.is_active_at(now))
            {
                match state.sessions.get(&prior_id) {
                    Some(existing) if existing == &prior => {}
                    Some(_) => {
                        // A competing transition now owns this ID. The undisclosed
                        // login is still reclaimed without replacing newer state.
                    }
                    None => changes.push(Change {
                        id: prior_id,
                        before: None,
                        after: Some(prior),
                        reason: Reason::RestoredAfterAbandonedLogin,
                        initiator: None,
                    }),
                }
            }
            state.commit(now, changes)
        })
    }
}
/// Isolated adapter for tests; uses the same transition validation as persistence.
pub struct MemorySessions(PersistentSessions);
impl Default for MemorySessions {
    fn default() -> Self {
        Self(PersistentSessions::memory())
    }
}
impl SessionStore for MemorySessions {
    fn insert<'a>(
        &'a self,
        id: String,
        s: BrowserSessionState,
        p: Option<String>,
        now: u64,
    ) -> PortFuture<'a, Option<(String, BrowserSessionState)>> {
        self.0.insert(id, s, p, now)
    }
    fn get<'a>(&'a self, id: String) -> PortFuture<'a, Option<BrowserSessionState>> {
        self.0.get(id)
    }
    fn remove<'a>(
        &'a self,
        id: String,
        now: u64,
        r: RemovalReason,
        initiator: Option<CredentialId>,
    ) -> PortFuture<'a, ()> {
        self.0.remove(id, now, r, initiator)
    }
    fn renew<'a>(
        &'a self,
        id: String,
        now: u64,
        initiator: CredentialId,
    ) -> PortFuture<'a, BrowserSessionState> {
        self.0.renew(id, now, initiator)
    }
    fn abandon_login<'a>(
        &'a self,
        id: String,
        prior: Option<(String, BrowserSessionState)>,
        now: u64,
    ) -> PortFuture<'a, ()> {
        self.0.abandon_login(id, prior, now)
    }
}

#[cfg(test)]
#[path = "../../../tests/browser_session/persistence.rs"]
mod tests;
