//! The one GitHub sign-in this gateway holds, and every change to it.
//!
//! All state changes serialize within their captured vault generation. A new
//! generation never waits for a departed generation's provider request. Cached state and storage work carry
//! their immutable vault generation. One compare-and-publish boundary admits
//! cache changes, and terminal operations take ownership before awaiting audit.
//! The ordering tables are in `docs/design/github-connect.md` and ADR 1152.
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::Value;
use tokio::{sync::watch, task::AbortHandle};
use uuid::Uuid;

#[cfg(any(unix, test))]
use super::ports::StoreUnavailable;
use super::ports::{
    ApiFailure, AuditUnavailable, DeviceCode, EndCause, GithubAccess, GithubApi, GithubAudit,
    GithubAuditEvent, GithubAuditRecord, GithubCaller, GithubReadFailure, GithubTokenStore,
    PollAnswer,
};
use crate::github::domain::{
    AccessToken, ConnectEnd, GithubReadKind, GithubRepository, GithubSignIn, GithubStatus,
};
use crate::timer::{within, Moment, Timer};

/// Why a change was not made as asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GithubError {
    /// GitHub could not be reached to ask for a code. Nothing changed.
    Unreachable,
    /// GitHub refused the app. Nothing changed.
    Rejected,
    /// The token could not be deleted; the sign-in is still kept.
    StoreUnavailable,
    /// A record could not be kept. Before a change, nothing changed; after a
    /// sign-out's token was deleted, it is deleted (read the status again).
    AuditUnavailable,
}

/// A repository read could not return a GitHub answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GithubReadError {
    /// Nessa holds no connected GitHub sign-in at this call.
    NotConnected,
    /// The caller stopped the read or dropped its cancellation sender.
    Cancelled,
    /// GitHub or its response refused the bounded read.
    Api(GithubReadFailure),
}

/// The sign-in, and what changes it.
pub struct GithubConnections {
    inner: Arc<Inner>,
}

struct Inner {
    access: Arc<dyn GithubAccess>,
    api: Arc<dyn GithubApi>,
    store: Arc<dyn GithubTokenStore>,
    audit: Arc<dyn GithubAudit>,
    timer: Arc<dyn Timer>,
    changes: Mutex<Option<(u64, Arc<tokio::sync::Mutex<()>>)>>,
    state: Mutex<State>,
}

enum State {
    SignedOut {
        last: Option<ConnectEnd>,
        /// The end named in `last` is not in the audit.
        unrecorded: bool,
    },
    Connecting(Attempt),
    /// Completion owns the terminal operation; this immutable projection keeps
    /// status pending until both durable receipts arrive. It owns no poll task.
    Saving {
        generation: u64,
        user_code: String,
        verification_uri: String,
        expires_at: Moment,
    },
    Connected {
        generation: u64,
        sign_in: GithubSignIn,
        /// A failed sign-out of it is not in the audit.
        unrecorded: bool,
    },
}

impl State {
    fn generation(&self) -> Option<u64> {
        match self {
            Self::SignedOut { .. } => None,
            Self::Connecting(attempt) => Some(attempt.generation),
            Self::Saving { generation, .. } | Self::Connected { generation, .. } => {
                Some(*generation)
            }
        }
    }
    /// Signed out, with the end of the latest attempt (if any) on record.
    fn signed_out(last: Option<ConnectEnd>) -> Self {
        Self::SignedOut {
            last,
            unrecorded: false,
        }
    }
}

struct Attempt {
    generation: u64,
    operation_id: String,
    caller: GithubCaller,
    user_code: String,
    verification_uri: String,
    expires_at: Moment,
    poll: AbortHandle,
}

impl GithubConnections {
    /// The sign-in kept from an earlier run, or none. A store that cannot be
    /// read is signed out, and says so. Only the Unix composition runs
    /// agents, so only it builds one.
    #[cfg(any(unix, test))]
    pub(crate) async fn load(
        api: Arc<dyn GithubApi>,
        access: Arc<dyn GithubAccess>,
        store: Arc<dyn GithubTokenStore>,
        audit: Arc<dyn GithubAudit>,
        timer: Arc<dyn Timer>,
    ) -> Self {
        let generation = access.generation();
        // The plaintext file is not the store. Remove it before asking whether
        // the vault is unlocked, and do not read it. A failed delete is the
        // startup result; it is not a clean signed-out.
        let saved = if store.discard_plaintext().await.is_err() {
            tracing::error!("the plaintext GitHub sign-in could not be removed");
            Err(StoreUnavailable)
        } else {
            match generation {
                Some(generation) => store.load(generation).await,
                None => Ok(None),
            }
        };
        let state = match saved {
            Ok(Some(sign_in)) if generation.is_some() && access.generation() == generation => {
                State::Connected {
                    generation: generation.expect("checked above"),
                    sign_in,
                    unrecorded: false,
                }
            }
            Ok(_) => State::signed_out(None),
            Err(_) => {
                tracing::error!("the GitHub sign-in could not be read; signed out this run");
                State::signed_out(Some(ConnectEnd::StoreUnavailable))
            }
        };
        Self {
            inner: Arc::new(Inner {
                access,
                api,
                store,
                audit,
                timer,
                changes: Mutex::new(None),
                state: Mutex::new(state),
            }),
        }
    }

    /// Project current state, retiring a stale cache first. Retirement may perform
    /// one blocking cleanup audit when a pending sign-in belongs to an old vault.
    pub fn status(&self) -> GithubStatus {
        self.inner.status()
    }

    /// Project a token only from the current unlock generation. Like `status`,
    /// this can perform one blocking stale-attempt cleanup audit.
    pub fn token(&self) -> Option<AccessToken> {
        if self.inner.retire_stale().is_err() {
            tracing::error!("GitHub vault cleanup audit failed");
        }
        let state = self.inner.state.lock().unwrap();
        if state.generation() != self.inner.access.generation() {
            return None;
        }
        match &*state {
            State::Connected { sign_in, .. } => Some(sign_in.token.clone()),
            _ => None,
        }
    }

    /// Read one resource within the vault generation that admitted this call.
    /// The generation's change gate orders local reads before Disconnect.
    /// Cancellation drops the lock wait or HTTP future; a sent GET may already
    /// have reached GitHub. See the read ordering table in `github-connect.md`
    /// and `tests/github/reads.rs` plus `read_http.rs` for forced interleavings.
    pub async fn read_repository(
        &self,
        repository: &GithubRepository,
        kind: GithubReadKind,
        mut stop: watch::Receiver<bool>,
    ) -> Result<Value, GithubReadError> {
        let generation = self
            .inner
            .access
            .generation()
            .ok_or(GithubReadError::NotConnected)?;
        let _change = tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return Err(GithubReadError::Cancelled),
            change = self.inner.change_in(generation) => {
                change.map_err(|_| GithubReadError::NotConnected)?
            }
        };
        // Cancellation can arrive as the gate opens. Inspect it before copying
        // the token, and let it win over a simultaneously ready adapter below.
        if *stop.borrow() || stop.has_changed().is_err() {
            return Err(GithubReadError::Cancelled);
        }
        let token = match &*self.inner.state.lock().unwrap() {
            State::Connected {
                generation: held,
                sign_in,
                ..
            } if *held == generation && self.inner.access.generation() == Some(generation) => {
                sign_in.token.clone()
            }
            _ => return Err(GithubReadError::NotConnected),
        };
        let answer = tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return Err(GithubReadError::Cancelled),
            answer = self.inner.api.read_repository(&token, repository, kind) => answer,
        };
        // A new unlock has an independent change gate. Its progress does not
        // make this departed generation's response available to the agent.
        if self.inner.access.generation() != Some(generation) {
            return Err(GithubReadError::NotConnected);
        }
        answer.map_err(GithubReadError::Api)
    }

    /// Restore encrypted persistence after an explicit vault unlock. No provider call.
    pub async fn restore(&self) -> Result<(), GithubError> {
        if self.inner.store.discard_plaintext().await.is_err() {
            tracing::error!("the plaintext GitHub sign-in could not be removed");
            return Err(GithubError::StoreUnavailable);
        }
        let generation = self
            .inner
            .access
            .generation()
            .ok_or(GithubError::StoreUnavailable)?;
        let _change = self.inner.change_in(generation).await?;
        self.inner
            .retire_stale()
            .map_err(|_| GithubError::AuditUnavailable)?;
        if self.inner.state.lock().unwrap().generation() == Some(generation) {
            return Ok(());
        }
        let saved = match self.inner.store.load(generation).await {
            Ok(saved) => saved,
            Err(_) => {
                self.inner.publish(
                    generation,
                    State::signed_out(Some(ConnectEnd::StoreUnavailable)),
                );
                return Err(GithubError::StoreUnavailable);
            }
        };
        self.inner
            .in_generation(generation, |state| {
                if state.generation() == Some(generation) {
                    return;
                }
                *state = match saved {
                    Some(sign_in) => State::Connected {
                        generation,
                        sign_in,
                        unrecorded: false,
                    },
                    None => State::signed_out(None),
                };
            })
            .ok_or(GithubError::StoreUnavailable)?;
        Ok(())
    }
    /// Vault closure removes cached token/code ownership without deleting persistence.
    pub fn clear_locked(&self) -> Result<(), AuditUnavailable> {
        self.inner.retire_stale()
    }

    /// Start a sign-in, or answer the one already waiting or held: asking
    /// again is safe.
    pub async fn connect(&self, caller: GithubCaller) -> Result<GithubStatus, GithubError> {
        let inner = &self.inner;
        let generation = inner
            .access
            .generation()
            .ok_or(GithubError::StoreUnavailable)?;
        let _change = inner.change_in(generation).await?;
        inner
            .retire_stale()
            .map_err(|_| GithubError::AuditUnavailable)?;
        if !matches!(&*inner.state.lock().unwrap(), State::SignedOut { .. }) {
            return Ok(inner.status());
        }
        let operation_id = Uuid::new_v4().to_string();
        let record = |event| GithubAuditRecord {
            operation_id: operation_id.clone(),
            caller: caller.clone(),
            event,
        };
        inner
            .audit
            .record(&record(GithubAuditEvent::ConnectRequested))
            .await
            .map_err(|_| GithubError::AuditUnavailable)?;
        let authorization = match inner.api.request_device_code().await {
            Ok(authorization) => authorization,
            Err(failure) => {
                let (end, cause, error) = match failure {
                    ApiFailure::Rejected | ApiFailure::Unauthorized => (
                        ConnectEnd::Rejected,
                        EndCause::GithubAnswered,
                        GithubError::Rejected,
                    ),
                    ApiFailure::Unreachable | ApiFailure::Malformed => (
                        ConnectEnd::Unreachable,
                        EndCause::GithubUnreachable,
                        GithubError::Unreachable,
                    ),
                };
                // Nothing started, whether or not the end is on record: the
                // answer says why it did not, and the status that it is not
                // on record.
                let unrecorded = inner
                    .audit
                    .record(&record(GithubAuditEvent::ConnectEnded { end, cause }))
                    .await
                    .is_err();
                if unrecorded {
                    tracing::error!(?end, "the end of a GitHub sign-in could not be recorded");
                }
                inner.publish(
                    generation,
                    State::SignedOut {
                        last: Some(end),
                        unrecorded,
                    },
                );
                return Err(error);
            }
        };
        let published = inner
            .in_generation(generation, |state| {
                let expires_at = inner.timer.now() + authorization.expires_in;
                let poll = tokio::spawn(poll(
                    Arc::downgrade(inner),
                    operation_id.clone(),
                    authorization.device_code,
                    authorization.interval,
                    expires_at,
                ))
                .abort_handle();
                *state = State::Connecting(Attempt {
                    generation,
                    operation_id: operation_id.clone(),
                    caller: caller.clone(),
                    user_code: authorization.user_code,
                    verification_uri: authorization.verification_uri,
                    expires_at,
                    poll,
                });
            })
            .is_some();
        if !published {
            inner
                .audit
                .record(&record(GithubAuditEvent::ConnectEnded {
                    end: ConnectEnd::StoreUnavailable,
                    cause: EndCause::VaultLocked,
                }))
                .await
                .map_err(|_| GithubError::AuditUnavailable)?;
            return Err(GithubError::StoreUnavailable);
        }

        Ok(inner.status())
    }

    /// Stop the sign-in waiting, if one is. A sign-in already held is left
    /// as it is: that is [`Self::disconnect`]'s.
    pub async fn cancel(&self, caller: GithubCaller) -> Result<GithubStatus, GithubError> {
        let generation = self
            .inner
            .access
            .generation()
            .ok_or(GithubError::StoreUnavailable)?;
        let _change = self.inner.change_in(generation).await?;
        self.inner
            .retire_stale()
            .map_err(|_| GithubError::AuditUnavailable)?;
        self.inner.cancel_held(caller).await
    }

    /// Forget the sign-in held, or stop the one waiting. GitHub keeps the
    /// authorization until the person revokes it there; this gateway no
    /// longer holds or gives out its token.
    pub async fn disconnect(&self, caller: GithubCaller) -> Result<GithubStatus, GithubError> {
        let inner = &self.inner;
        let generation = inner
            .access
            .generation()
            .ok_or(GithubError::StoreUnavailable)?;
        let _change = inner.change_in(generation).await?;
        inner
            .retire_stale()
            .map_err(|_| GithubError::AuditUnavailable)?;
        // Read, then let go of the state before answering from it.
        let held = match &*inner.state.lock().unwrap() {
            State::Connected { sign_in, .. } => Held::SignedIn(sign_in.account.clone()),
            State::Connecting(_) | State::Saving { .. } => Held::Waiting,
            State::SignedOut { .. } => Held::Nothing,
        };
        let account = match held {
            Held::SignedIn(account) => account,
            Held::Waiting => return inner.cancel_held(caller).await,
            // Nothing is held, but a token whose save failed after it was
            // written may still be on disk: it goes too, or the next start
            // would hold it.
            Held::Nothing => {
                inner
                    .store
                    .delete(generation)
                    .await
                    .map_err(|_| GithubError::StoreUnavailable)?;
                return Ok(inner.status());
            }
        };
        let operation_id = Uuid::new_v4().to_string();
        let record = |event| GithubAuditRecord {
            operation_id: operation_id.clone(),
            caller: caller.clone(),
            event,
        };
        inner
            .audit
            .record(&record(GithubAuditEvent::DisconnectRequested { account }))
            .await
            .map_err(|_| GithubError::AuditUnavailable)?;
        if inner.store.delete(generation).await.is_err() {
            // Still signed in, and refused for that; a failure the audit
            // will not keep either is said by the status too.
            if inner
                .audit
                .record(&record(GithubAuditEvent::DisconnectFailed))
                .await
                .is_err()
            {
                tracing::error!("a failed GitHub sign-out could not be recorded");
                inner.in_generation(generation, |state| {
                    if let State::Connected { unrecorded, .. } = state {
                        *unrecorded = true;
                    }
                });
            }
            return Err(GithubError::StoreUnavailable);
        }
        inner.publish(generation, State::signed_out(None));
        if inner
            .audit
            .record(&record(GithubAuditEvent::Disconnected))
            .await
            .is_err()
        {
            // Signed out all the same: the token is gone.
            inner.publish(
                generation,
                State::SignedOut {
                    last: None,
                    unrecorded: true,
                },
            );
            return Err(GithubError::AuditUnavailable);
        }
        Ok(inner.status())
    }
}

/// What `disconnect` found held.
enum Held {
    SignedIn(crate::github::domain::GithubAccount),
    Waiting,
    Nothing,
}

impl Inner {
    /// Same-session IO and publication have one ordering owner. Replacing the
    /// gate for a new vault generation never makes its unlock wait on old IO.
    async fn change_in(
        &self,
        generation: u64,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, GithubError> {
        let gate = {
            let mut changes = self.changes.lock().unwrap();
            if self.access.generation() != Some(generation) {
                return Err(GithubError::StoreUnavailable);
            }
            if changes.as_ref().is_none_or(|(held, _)| *held != generation) {
                *changes = Some((generation, Arc::new(tokio::sync::Mutex::new(()))));
            }
            changes.as_ref().expect("assigned above").1.clone()
        };
        let guard = gate.lock_owned().await;
        if self.access.generation() != Some(generation) {
            return Err(GithubError::StoreUnavailable);
        }
        Ok(guard)
    }
    fn in_generation<T>(&self, generation: u64, apply: impl FnOnce(&mut State) -> T) -> Option<T> {
        let mut state = self.state.lock().unwrap();
        if self.access.generation() != Some(generation) {
            return None;
        }
        Some(apply(&mut state))
    }
    fn publish(&self, generation: u64, next: State) {
        if self
            .in_generation(generation, |state| *state = next)
            .is_none()
        {
            tracing::debug!("discarded a stale GitHub cache publication");
        }
    }

    /// The held generation, not current availability, decides which cache is retired.
    fn retire_stale(&self) -> Result<(), AuditUnavailable> {
        let previous = {
            let mut state = self.state.lock().unwrap();
            if state.generation().is_none() || state.generation() == self.access.generation() {
                return Ok(());
            }
            std::mem::replace(&mut *state, State::signed_out(None))
        };
        if let State::Connecting(attempt) = previous {
            attempt.poll.abort();
            self.audit.record_cleanup(&GithubAuditRecord {
                operation_id: attempt.operation_id,
                caller: attempt.caller,
                event: GithubAuditEvent::ConnectEnded {
                    end: ConnectEnd::StoreUnavailable,
                    cause: EndCause::VaultLocked,
                },
            })?;
        }
        Ok(())
    }
    fn status(&self) -> GithubStatus {
        if self.retire_stale().is_err() {
            tracing::error!("GitHub vault cleanup audit failed");
        }
        let state = self.state.lock().unwrap();
        if state.generation().is_some() && state.generation() != self.access.generation() {
            return GithubStatus::SignedOut {
                last: None,
                unrecorded: false,
            };
        }
        match &*state {
            State::SignedOut { last, unrecorded } => GithubStatus::SignedOut {
                last: *last,
                unrecorded: *unrecorded,
            },
            State::Connecting(attempt) => self.connecting_status(
                &attempt.user_code,
                &attempt.verification_uri,
                attempt.expires_at,
            ),
            State::Saving {
                user_code,
                verification_uri,
                expires_at,
                ..
            } => self.connecting_status(user_code, verification_uri, *expires_at),
            State::Connected {
                sign_in,
                unrecorded,
                ..
            } => GithubStatus::Connected {
                account: sign_in.account.clone(),
                unrecorded: *unrecorded,
            },
        }
    }

    fn connecting_status(
        &self,
        user_code: &str,
        verification_uri: &str,
        expires_at: Moment,
    ) -> GithubStatus {
        GithubStatus::Connecting {
            user_code: user_code.to_owned(),
            verification_uri: verification_uri.to_owned(),
            expires_in_seconds: expires_at.since(self.timer.now()).as_secs(),
        }
    }

    /// Cancel the attempt waiting, with `changes` held.
    async fn cancel_held(&self, by: GithubCaller) -> Result<GithubStatus, GithubError> {
        let attempt = {
            let mut state = self.state.lock().unwrap();
            if !matches!(&*state, State::Connecting(_)) {
                drop(state);
                return Ok(self.status());
            }
            match std::mem::replace(&mut *state, State::signed_out(Some(ConnectEnd::Cancelled))) {
                State::Connecting(attempt) => attempt,
                _ => unreachable!("matched above"),
            }
        };
        let generation = attempt.generation;
        attempt.poll.abort();
        // The cancel stands whether or not its record can be kept: the
        // attempt is not left polling for a code nobody is shown. A missing
        // record is refused now and said by the status after.
        let recorded = self
            .audit
            .record(&GithubAuditRecord {
                operation_id: attempt.operation_id,
                caller: attempt.caller,
                event: GithubAuditEvent::ConnectCancelled { by },
            })
            .await;
        if recorded.is_err() {
            self.publish(
                generation,
                State::SignedOut {
                    last: Some(ConnectEnd::Cancelled),
                    unrecorded: true,
                },
            );
            return Err(GithubError::AuditUnavailable);
        }
        Ok(self.status())
    }

    /// The attempt `operation_id`'s caller, while it is the one waiting.
    fn waiting(&self, operation_id: &str) -> Option<(GithubCaller, u64)> {
        if self.retire_stale().is_err() {
            tracing::error!("GitHub vault cleanup audit failed");
        }
        match &*self.state.lock().unwrap() {
            State::Connecting(attempt) if attempt.operation_id == operation_id => {
                Some((attempt.caller.clone(), attempt.generation))
            }
            _ => None,
        }
    }

    fn take_waiting(
        &self,
        operation_id: &str,
        projection: impl FnOnce(&Attempt) -> State,
    ) -> Option<Attempt> {
        if self.retire_stale().is_err() {
            tracing::error!("GitHub vault cleanup audit failed");
        }
        let mut state = self.state.lock().unwrap();
        if !matches!(&*state,State::Connecting(attempt) if attempt.operation_id==operation_id && self.access.generation()==Some(attempt.generation))
        {
            return None;
        }
        match std::mem::replace(&mut *state, State::signed_out(None)) {
            State::Connecting(attempt) => {
                // Taking terminal ownership and replacing the public projection
                // are one transition; a status read cannot observe the temporary value.
                *state = projection(&attempt);
                Some(attempt)
            }
            _ => unreachable!("matched above"),
        }
    }

    /// End the attempt `operation_id` without a sign-in, if it is still the
    /// one waiting.
    async fn end(&self, operation_id: &str, end: ConnectEnd, cause: EndCause) {
        let Some((_, generation)) = self.waiting(operation_id) else {
            return;
        };
        let Ok(_change) = self.change_in(generation).await else {
            return;
        };
        let Some(attempt) = self.take_waiting(operation_id, |_| State::signed_out(None)) else {
            return;
        };
        let generation = attempt.generation;
        let caller = attempt.caller;
        // The attempt ends either way; a missing record is said, not hidden.
        let unrecorded = self
            .audit
            .record(&GithubAuditRecord {
                operation_id: operation_id.to_owned(),
                caller,
                event: GithubAuditEvent::ConnectEnded { end, cause },
            })
            .await
            .is_err();
        if unrecorded {
            tracing::error!(?end, "the end of a GitHub sign-in could not be recorded");
        }
        self.publish(
            generation,
            State::SignedOut {
                last: Some(end),
                unrecorded,
            },
        );
    }

    /// Keep `sign_in` for the attempt `operation_id`, if it is still the one
    /// waiting: recorded first, then stored, then given to agents.
    async fn complete(&self, operation_id: &str, sign_in: GithubSignIn) {
        let Some((_, generation)) = self.waiting(operation_id) else {
            return;
        };
        let Ok(_change) = self.change_in(generation).await else {
            return;
        };
        let (generation, caller) = {
            let Some(attempt) = self.take_waiting(operation_id, |attempt| State::Saving {
                generation: attempt.generation,
                user_code: attempt.user_code.clone(),
                verification_uri: attempt.verification_uri.clone(),
                expires_at: attempt.expires_at,
            }) else {
                return;
            };
            (attempt.generation, attempt.caller)
        };
        let record = |event| GithubAuditRecord {
            operation_id: operation_id.to_owned(),
            caller: caller.clone(),
            event,
        };
        if self
            .audit
            .record(&record(GithubAuditEvent::Connected {
                account: sign_in.account.clone(),
            }))
            .await
            .is_err()
        {
            tracing::error!("a GitHub sign-in could not be recorded; its token is dropped");
            // The end itself says the record is what failed.
            self.publish(
                generation,
                State::signed_out(Some(ConnectEnd::AuditUnavailable)),
            );
            return;
        }
        if self.store.save(generation, &sign_in).await.is_err() {
            // A save can fail after the file was already written (its
            // directory not synced): delete it, so a dropped token is never
            // held again at the next start. A file that cannot be deleted
            // either is said in the log; disconnecting deletes it later.
            if self.store.delete(generation).await.is_err() {
                tracing::error!("a GitHub sign-in that could not be stored could not be deleted");
            }
            tracing::error!("a GitHub sign-in could not be stored; its token is dropped");
            let unrecorded = self
                .audit
                .record(&record(GithubAuditEvent::ConnectEnded {
                    end: ConnectEnd::StoreUnavailable,
                    cause: if self.access.generation() == Some(generation) {
                        EndCause::StoreFailed
                    } else {
                        EndCause::VaultLocked
                    },
                }))
                .await
                .is_err();
            if unrecorded {
                tracing::error!("the end of a GitHub sign-in could not be recorded");
            }
            self.publish(
                generation,
                State::SignedOut {
                    last: Some(ConnectEnd::StoreUnavailable),
                    unrecorded,
                },
            );
            return;
        }
        self.publish(
            generation,
            State::Connected {
                generation,
                sign_in,
                unrecorded: false,
            },
        );
    }
}

/// Poll GitHub for the attempt `operation_id` every `interval` until it is
/// approved, refused, or `expires_at` passes; then apply what was found. A
/// lost answer is asked again at the next interval. Holds the owner only
/// weakly, so a gateway that has let go of it is not kept polling.
async fn poll(
    owner: Weak<Inner>,
    operation_id: String,
    device_code: DeviceCode,
    mut interval: Duration,
    expires_at: Moment,
) {
    let mut granted: Option<AccessToken> = None;
    loop {
        let Some(inner) = owner.upgrade() else {
            return;
        };
        if inner.waiting(&operation_id).is_none() {
            return;
        }
        let timer = inner.timer.clone();
        let api = inner.api.clone();
        drop(inner);
        let next = timer.now() + interval;
        if next >= expires_at {
            timer.sleep_until(expires_at).await;
            // Approved, but whose it is never came back: not the code's fault.
            let end = if granted.is_some() {
                ConnectEnd::Unreachable
            } else {
                ConnectEnd::Expired
            };
            return finish(&owner, &operation_id, end, EndCause::DeadlinePassed).await;
        }
        timer.sleep_until(next).await;
        if owner
            .upgrade()
            .is_none_or(|inner| inner.waiting(&operation_id).is_none())
        {
            return;
        }
        let token = match &granted {
            Some(token) => token.clone(),
            None => match within(timer.as_ref(), expires_at, api.poll(&device_code)).await {
                Err(_) => {
                    return finish(
                        &owner,
                        &operation_id,
                        ConnectEnd::Expired,
                        EndCause::DeadlinePassed,
                    )
                    .await
                }
                Ok(Ok(PollAnswer::Pending)) => continue,
                Ok(Ok(PollAnswer::SlowDown { interval: slower })) => {
                    interval = slower.max(interval);
                    continue;
                }
                Ok(Ok(PollAnswer::Expired)) => {
                    return finish(
                        &owner,
                        &operation_id,
                        ConnectEnd::Expired,
                        EndCause::GithubAnswered,
                    )
                    .await
                }
                Ok(Ok(PollAnswer::Denied)) => {
                    return finish(
                        &owner,
                        &operation_id,
                        ConnectEnd::Denied,
                        EndCause::GithubAnswered,
                    )
                    .await
                }
                Ok(Err(ApiFailure::Rejected | ApiFailure::Unauthorized)) => {
                    return finish(
                        &owner,
                        &operation_id,
                        ConnectEnd::Rejected,
                        EndCause::GithubAnswered,
                    )
                    .await
                }
                Ok(Err(ApiFailure::Unreachable | ApiFailure::Malformed)) => continue,
                Ok(Ok(PollAnswer::Granted(token))) => {
                    granted = Some(token.clone());
                    token
                }
            },
        };
        if owner
            .upgrade()
            .is_none_or(|inner| inner.waiting(&operation_id).is_none())
        {
            return;
        }
        // Approved: whose it is. A lost answer is asked again until the code
        // would have expired; a refused token is not GitHub's to give.
        match within(timer.as_ref(), expires_at, api.account(&token)).await {
            Ok(Ok(account)) => {
                if let Some(inner) = owner.upgrade() {
                    inner
                        .complete(&operation_id, GithubSignIn { account, token })
                        .await;
                }
                return;
            }
            Ok(Err(ApiFailure::Rejected | ApiFailure::Unauthorized)) => {
                return finish(
                    &owner,
                    &operation_id,
                    ConnectEnd::Rejected,
                    EndCause::GithubAnswered,
                )
                .await
            }
            Ok(Err(ApiFailure::Unreachable | ApiFailure::Malformed)) => continue,
            Err(_) => {
                return finish(
                    &owner,
                    &operation_id,
                    ConnectEnd::Unreachable,
                    EndCause::DeadlinePassed,
                )
                .await
            }
        }
    }
}

async fn finish(owner: &Weak<Inner>, operation_id: &str, end: ConnectEnd, cause: EndCause) {
    if let Some(inner) = owner.upgrade() {
        inner.end(operation_id, end, cause).await;
    }
}
