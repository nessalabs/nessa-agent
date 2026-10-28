//! The one GitHub sign-in this gateway holds, and every change to it.
//!
//! Every change — a connect, a cancel, a disconnect, and an attempt
//! finishing — takes `changes` first, so each sees the state the one before
//! it left. A read (`status`, `token`) takes only the state and never waits
//! on GitHub. An attempt's poll is a task of its own; what it finds is applied
//! only while that same attempt is still the one waiting
//! (`a_cancelled_attempts_late_approval_is_dropped`). The orderings are the
//! table in `docs/design/github-connect.md`.
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde_json::Value;
use tokio::task::AbortHandle;
use uuid::Uuid;

use super::ports::{
    ApiFailure, DeviceCode, EndCause, GithubApi, GithubAudit, GithubAuditEvent, GithubAuditRecord,
    GithubCaller, GithubReadFailure, GithubTokenStore, PollAnswer,
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
    /// GitHub or its response refused the bounded read.
    Api(GithubReadFailure),
}

/// The sign-in, and what changes it.
pub struct GithubConnections {
    inner: Arc<Inner>,
}

struct Inner {
    api: Arc<dyn GithubApi>,
    store: Arc<dyn GithubTokenStore>,
    audit: Arc<dyn GithubAudit>,
    timer: Arc<dyn Timer>,
    changes: tokio::sync::Mutex<()>,
    state: Mutex<State>,
}

enum State {
    SignedOut {
        last: Option<ConnectEnd>,
        /// The end named in `last` is not in the audit.
        unrecorded: bool,
    },
    Connecting(Attempt),
    Connected {
        sign_in: GithubSignIn,
        /// A failed sign-out of it is not in the audit.
        unrecorded: bool,
    },
}

impl State {
    /// Signed out, with the end of the latest attempt (if any) on record.
    fn signed_out(last: Option<ConnectEnd>) -> Self {
        Self::SignedOut {
            last,
            unrecorded: false,
        }
    }
}

struct Attempt {
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
        store: Arc<dyn GithubTokenStore>,
        audit: Arc<dyn GithubAudit>,
        timer: Arc<dyn Timer>,
    ) -> Self {
        let state = match store.load().await {
            Ok(Some(sign_in)) => State::Connected {
                sign_in,
                unrecorded: false,
            },
            Ok(None) => State::signed_out(None),
            Err(_) => {
                tracing::error!("the GitHub sign-in could not be read; signed out this run");
                State::signed_out(Some(ConnectEnd::StoreUnavailable))
            }
        };
        Self {
            inner: Arc::new(Inner {
                api,
                store,
                audit,
                timer,
                changes: tokio::sync::Mutex::new(()),
                state: Mutex::new(state),
            }),
        }
    }

    /// Where the sign-in stands now.
    pub fn status(&self) -> GithubStatus {
        self.inner.status()
    }

    /// Inspect the retained token in sign-in state tests only.
    #[cfg(test)]
    pub fn token(&self) -> Option<AccessToken> {
        match &*self.inner.state.lock().unwrap() {
            State::Connected { sign_in, .. } => Some(sign_in.token.clone()),
            _ => None,
        }
    }

    /// Read one repository resource with the current sign-in. The change lock
    /// orders this bounded HTTP request before a concurrent Disconnect, so a
    /// returned Disconnect leaves no request using the old token in flight.
    pub async fn read_repository(
        &self,
        repository: &GithubRepository,
        kind: GithubReadKind,
    ) -> Result<Value, GithubReadError> {
        let _change = self.inner.changes.lock().await;
        let token = match &*self.inner.state.lock().unwrap() {
            State::Connected { sign_in, .. } => sign_in.token.clone(),
            _ => return Err(GithubReadError::NotConnected),
        };
        self.inner
            .api
            .read_repository(&token, repository, kind)
            .await
            .map_err(GithubReadError::Api)
    }

    /// Start a sign-in, or answer the one already waiting or held: asking
    /// again is safe.
    pub async fn connect(&self, caller: GithubCaller) -> Result<GithubStatus, GithubError> {
        let inner = &self.inner;
        let _change = inner.changes.lock().await;
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
                *inner.state.lock().unwrap() = State::SignedOut {
                    last: Some(end),
                    unrecorded,
                };
                return Err(error);
            }
        };
        let expires_at = inner.timer.now() + authorization.expires_in;
        let poll = tokio::spawn(poll(
            Arc::downgrade(inner),
            operation_id.clone(),
            authorization.device_code,
            authorization.interval,
            expires_at,
        ))
        .abort_handle();
        *inner.state.lock().unwrap() = State::Connecting(Attempt {
            operation_id,
            caller,
            user_code: authorization.user_code,
            verification_uri: authorization.verification_uri,
            expires_at,
            poll,
        });
        Ok(inner.status())
    }

    /// Stop the sign-in waiting, if one is. A sign-in already held is left
    /// as it is: that is [`Self::disconnect`]'s.
    pub async fn cancel(&self, caller: GithubCaller) -> Result<GithubStatus, GithubError> {
        let _change = self.inner.changes.lock().await;
        self.inner.cancel_held(caller).await
    }

    /// Forget the sign-in held, or stop the one waiting. GitHub keeps the
    /// authorization until the person revokes it there; this gateway no
    /// longer holds or gives out its token.
    pub async fn disconnect(&self, caller: GithubCaller) -> Result<GithubStatus, GithubError> {
        let inner = &self.inner;
        let _change = inner.changes.lock().await;
        // Read, then let go of the state before answering from it.
        let held = match &*inner.state.lock().unwrap() {
            State::Connected { sign_in, .. } => Held::SignedIn(sign_in.account.clone()),
            State::Connecting(_) => Held::Waiting,
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
                    .delete()
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
        if inner.store.delete().await.is_err() {
            // Still signed in, and refused for that; a failure the audit
            // will not keep either is said by the status too.
            if inner
                .audit
                .record(&record(GithubAuditEvent::DisconnectFailed))
                .await
                .is_err()
            {
                tracing::error!("a failed GitHub sign-out could not be recorded");
                if let State::Connected { unrecorded, .. } = &mut *inner.state.lock().unwrap() {
                    *unrecorded = true;
                }
            }
            return Err(GithubError::StoreUnavailable);
        }
        *inner.state.lock().unwrap() = State::signed_out(None);
        if inner
            .audit
            .record(&record(GithubAuditEvent::Disconnected))
            .await
            .is_err()
        {
            // Signed out all the same: the token is gone.
            *inner.state.lock().unwrap() = State::SignedOut {
                last: None,
                unrecorded: true,
            };
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
    fn status(&self) -> GithubStatus {
        match &*self.state.lock().unwrap() {
            State::SignedOut { last, unrecorded } => GithubStatus::SignedOut {
                last: *last,
                unrecorded: *unrecorded,
            },
            State::Connecting(attempt) => GithubStatus::Connecting {
                user_code: attempt.user_code.clone(),
                verification_uri: attempt.verification_uri.clone(),
                expires_in_seconds: attempt.expires_at.since(self.timer.now()).as_secs(),
            },
            State::Connected {
                sign_in,
                unrecorded,
            } => GithubStatus::Connected {
                account: sign_in.account.clone(),
                unrecorded: *unrecorded,
            },
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
            *self.state.lock().unwrap() = State::SignedOut {
                last: Some(ConnectEnd::Cancelled),
                unrecorded: true,
            };
            return Err(GithubError::AuditUnavailable);
        }
        Ok(self.status())
    }

    /// The attempt `operation_id`'s caller, while it is the one waiting.
    fn waiting(&self, operation_id: &str) -> Option<GithubCaller> {
        match &*self.state.lock().unwrap() {
            State::Connecting(attempt) if attempt.operation_id == operation_id => {
                Some(attempt.caller.clone())
            }
            _ => None,
        }
    }

    /// End the attempt `operation_id` without a sign-in, if it is still the
    /// one waiting.
    async fn end(&self, operation_id: &str, end: ConnectEnd, cause: EndCause) {
        let _change = self.changes.lock().await;
        let Some(caller) = self.waiting(operation_id) else {
            return;
        };
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
        *self.state.lock().unwrap() = State::SignedOut {
            last: Some(end),
            unrecorded,
        };
    }

    /// Keep `sign_in` for the attempt `operation_id`, if it is still the one
    /// waiting: recorded first, then stored, then given to agents.
    async fn complete(&self, operation_id: &str, sign_in: GithubSignIn) {
        let _change = self.changes.lock().await;
        let Some(caller) = self.waiting(operation_id) else {
            return;
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
            *self.state.lock().unwrap() = State::signed_out(Some(ConnectEnd::AuditUnavailable));
            return;
        }
        if self.store.save(&sign_in).await.is_err() {
            // A save can fail after the file was already written (its
            // directory not synced): delete it, so a dropped token is never
            // held again at the next start. A file that cannot be deleted
            // either is said in the log; disconnecting deletes it later.
            if self.store.delete().await.is_err() {
                tracing::error!("a GitHub sign-in that could not be stored could not be deleted");
            }
            tracing::error!("a GitHub sign-in could not be stored; its token is dropped");
            let unrecorded = self
                .audit
                .record(&record(GithubAuditEvent::ConnectEnded {
                    end: ConnectEnd::StoreUnavailable,
                    cause: EndCause::StoreFailed,
                }))
                .await
                .is_err();
            if unrecorded {
                tracing::error!("the end of a GitHub sign-in could not be recorded");
            }
            *self.state.lock().unwrap() = State::SignedOut {
                last: Some(ConnectEnd::StoreUnavailable),
                unrecorded,
            };
            return;
        }
        *self.state.lock().unwrap() = State::Connected {
            sign_in,
            unrecorded: false,
        };
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
