use super::{
    Notice, ProductWatchPermit, WatchDeliveries, WatchHandle, WatchPrincipal, WatchRefusal,
    WatchSelector, WatchToken,
};
use crate::product::{
    generated::{
        product_method, ConversationUnwatchParams, ConversationWatchResult,
        MAX_CONNECTION_CHANGE_WATCHES,
    },
    socket::{failure, success, valid_product_request},
    state::ProductRouteState,
};
use crate::product_contract::generated::{
    ChangeWatchEndReason, ChangeWatchErrorCode, SessionCloseReason,
};
use crate::protocol::{OutgoingMessage, RequestFrame};
use futures_util::{
    future::{AbortHandle, Abortable},
    stream::FuturesUnordered,
    StreamExt,
};
use nessa_auth::application::session::AuthenticatedSession;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{
    sync::{oneshot, OwnedSemaphorePermit},
    time::Instant,
};

/// Writer metadata shares the original owner, not a newly acquired permit.
pub(in crate::product) struct WatchAcknowledgement {
    pub deadline: Instant,
    pub completed: oneshot::Sender<()>,
    pub owner: Arc<ProductWatchPermit>,
}

pub(in crate::product) struct WatchReply {
    pub message: OutgoingMessage,
    pub slot: Arc<OwnedSemaphorePermit>,
    pub acknowledgement: Option<WatchAcknowledgement>,
}

struct Target {
    key: u64,
    id: String,
    selector: WatchSelector,
    owner: Arc<ProductWatchPermit>,
    interest: Arc<AtomicBool>,
    source_wait: Option<AbortHandle>,
    source_pending: bool,
    authority_pending: bool,
    accepted: bool,
    // The registration request's own deadline, for its reply; and the timer
    // that closes the connection if admission has not returned by then (R7).
    registration_deadline: Instant,
    registration_bound: Option<AbortHandle>,
    acknowledgements: usize,
}

impl Drop for Target {
    fn drop(&mut self) {
        self.interest.store(false, Ordering::Release);
        if let Some(wait) = self.source_wait.take() {
            wait.abort();
        }
    }
}

struct OwnedSource {
    // Even an unpolled/cancelled future drops registration before its original charge.
    handle: WatchHandle,
    _owner: Arc<ProductWatchPermit>,
}

struct InstalledResult {
    // Field order drops any retained source handle before its original charge.
    result: Result<WatchHandle, ChangeWatchErrorCode>,
    _owner: Arc<ProductWatchPermit>,
}

enum Progress {
    Installed {
        key: u64,
        request: String,
        slot: Arc<OwnedSemaphorePermit>,
        result: Result<WatchHandle, ChangeWatchErrorCode>,
    },
    Authorized {
        key: u64,
        result: Result<(), WatchRefusal>,
    },
    /// The connection's one periodic re-check of all its live watches: one
    /// result per watch, by key.
    Rechecked {
        results: Vec<(u64, Result<(), WatchRefusal>)>,
    },
    /// A registration's deadline passed before its admission returned
    /// (`overdue`), or the timer was cancelled because it returned first.
    RegistrationBound { key: u64, overdue: bool },
    /// The periodic re-check's bound passed (`overdue`), or the check returned
    /// first and the timer was cancelled.
    RecheckBound { generation: u64, overdue: bool },
    Source {
        key: u64,
        result: Option<(OwnedSource, Option<ChangeWatchEndReason>)>,
    },
    Acknowledged {
        key: u64,
        activation: bool,
        result: Result<(), oneshot::error::RecvError>,
    },
}

type PendingProgress = Pin<Box<dyn Future<Output = Progress> + Send>>;

pub(in crate::product) enum WatchOutcome {
    Reply(Box<WatchReply>),
    Close(SessionCloseReason),
    Progress,
}

/// Two target positions and their original owners; this is not a live receiver.
pub(in crate::product) struct ConnectionWatches {
    targets: Vec<Target>,
    pending: FuturesUnordered<PendingProgress>,
    tokens: WatchToken,
    sequence: u64,
    // At most one periodic re-check runs per connection; a tick while it runs
    // is skipped rather than queued.
    recheck_pending: bool,
    // Identifies the running periodic check, so its bound cannot fire for a
    // later one; and cancels that bound when the check returns.
    recheck_generation: u64,
    recheck_bound: Option<AbortHandle>,
    pub deliveries: Arc<WatchDeliveries>,
}

impl ConnectionWatches {
    pub fn new(state: &ProductRouteState) -> Self {
        Self {
            targets: Vec::with_capacity(MAX_CONNECTION_CHANGE_WATCHES),
            pending: FuturesUnordered::new(),
            tokens: WatchToken::new(state.watch_namespaces.mint()),
            sequence: 0,
            recheck_pending: false,
            recheck_generation: 0,
            recheck_bound: None,
            deliveries: Arc::new(WatchDeliveries::new()),
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Closed generated method identities only; ordinary input admission is unchanged.
    pub fn method(method: &str) -> bool {
        matches!(
            method,
            product_method::CONVERSATION_WATCH_RECORDS
                | product_method::CONVERSATION_WATCH_CATALOGUE
                | product_method::CONVERSATION_UNWATCH
        )
    }

    pub fn begin(
        &mut self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
        frame: RequestFrame,
        slot: Arc<OwnedSemaphorePermit>,
        deadline: Instant,
    ) -> Option<WatchReply> {
        let refused = |code: ChangeWatchErrorCode| WatchReply {
            message: failure(&frame.id, code.as_str()),
            slot: slot.clone(),
            acknowledgement: None,
        };
        if !valid_product_request(&frame) {
            return Some(refused(ChangeWatchErrorCode::InvalidRequest));
        }
        if frame.method == product_method::CONVERSATION_UNWATCH {
            let params: ConversationUnwatchParams =
                match serde_json::from_value(frame.params.clone()) {
                    Ok(params) => params,
                    Err(_) => return Some(refused(ChangeWatchErrorCode::InvalidRequest)),
                };
            if !self.tokens.owns(&params.watch_id) {
                return Some(refused(ChangeWatchErrorCode::InvalidWatch));
            }
            let message = success(
                &frame.id,
                &ConversationWatchResult {
                    watch_id: params.watch_id.clone(),
                },
            );
            let Some(position) = self
                .targets
                .iter()
                .position(|target| target.id == params.watch_id)
            else {
                return Some(WatchReply {
                    message,
                    slot,
                    acknowledgement: None,
                });
            };
            let target = &mut self.targets[position];
            self.deliveries.retire(&target.id);
            target.interest.store(false, Ordering::Release);
            if let Some(wait) = target.source_wait.take() {
                wait.abort();
            }
            let key = target.key;
            // Bounded by this unwatch request's own deadline, like any reply.
            return Some(self.reply(key, message, slot, deadline, false));
        }
        let selector = match WatchSelector::decode(&frame.method, frame.params.clone()) {
            Ok(selector) => selector,
            Err(code) => return Some(refused(code)),
        };
        if self
            .targets
            .iter()
            .any(|target| target.selector == selector)
        {
            return Some(refused(ChangeWatchErrorCode::WatchDuplicate));
        }
        let same_kind = self
            .targets
            .iter()
            .filter(|target| target.selector.same_kind(&selector))
            .count();
        if same_kind >= selector.connection_limit() {
            return Some(refused(ChangeWatchErrorCode::WatchCapacity));
        }
        if !self.tokens.can_accept(
            self.targets
                .iter()
                .filter(|target| !target.accepted)
                .count()
                + 1,
        ) {
            return Some(refused(ChangeWatchErrorCode::WatchCapacity));
        }
        let Some(key) = self.sequence.checked_add(1) else {
            return Some(refused(ChangeWatchErrorCode::WatchCapacity));
        };
        let owner = match state
            .change_watches
            .try_acquire(&WatchPrincipal::of(session))
        {
            Ok(owner) => Arc::new(owner),
            Err(code) => return Some(refused(code)),
        };
        let interest = Arc::new(AtomicBool::new(true));
        let id = format!("registration-{key}");
        if !self.deliveries.reserve(id.clone(), owner.clone()) {
            return Some(refused(ChangeWatchErrorCode::WatchCapacity));
        }
        self.sequence = key;
        self.targets.push(Target {
            key,
            id,
            selector: selector.clone(),
            owner: owner.clone(),
            interest: interest.clone(),
            source_wait: None,
            source_pending: false,
            authority_pending: true,
            accepted: false,
            registration_deadline: deadline,
            registration_bound: None,
            acknowledgements: 0,
        });
        let state = state.clone();
        let session = session.clone();
        let original_task = owner.task();
        let task = tokio::spawn(async move {
            let result = selector.install(&state, &session, &interest).await;
            let result = if interest.load(Ordering::Acquire) {
                result
            } else {
                drop(result);
                Err(ChangeWatchErrorCode::WatchClosed)
            };
            original_task.completed();
            InstalledResult {
                result,
                _owner: owner,
            }
        });
        self.pending.push(Box::pin(async move {
            Progress::Installed {
                key,
                request: frame.id,
                slot,
                result: task
                    .await
                    .map(|output| output.result)
                    .unwrap_or(Err(ChangeWatchErrorCode::TemporarilyUnavailable)),
            }
        }));
        // Admission must answer by the request's deadline (row R7); the task
        // keeps its owner either way until the adapter returns.
        let (abort, cancelled) = AbortHandle::new_pair();
        if let Some(target) = self.targets.iter_mut().find(|target| target.key == key) {
            target.registration_bound = Some(abort);
        }
        self.pending.push(Box::pin(async move {
            let overdue = Abortable::new(tokio::time::sleep_until(deadline), cancelled)
                .await
                .is_ok();
            Progress::RegistrationBound { key, overdue }
        }));
        None
    }

    fn reply(
        &mut self,
        key: u64,
        message: OutgoingMessage,
        slot: Arc<OwnedSemaphorePermit>,
        deadline: Instant,
        activation: bool,
    ) -> WatchReply {
        let target = self
            .targets
            .iter_mut()
            .find(|target| target.key == key)
            .expect("reply retains original target");
        target.acknowledgements += 1;
        let owner = target.owner.clone();
        let (completed, completion) = oneshot::channel();
        let retained_slot = slot.clone();
        self.pending.push(Box::pin(async move {
            let _original_response_slot = retained_slot;
            Progress::Acknowledged {
                key,
                activation,
                result: completion.await,
            }
        }));
        WatchReply {
            message,
            slot,
            acknowledgement: Some(WatchAcknowledgement {
                deadline,
                completed,
                owner,
            }),
        }
    }

    fn source(&mut self, key: u64, handle: WatchHandle) {
        let target = self
            .targets
            .iter_mut()
            .find(|target| target.key == key)
            .expect("source retains original target");
        if self.deliveries.retiring(&target.id) {
            return;
        }
        let (abort, cancelled) = AbortHandle::new_pair();
        target.source_wait = Some(abort);
        target.source_pending = true;
        let mut owned = OwnedSource {
            handle,
            _owner: target.owner.clone(),
        };
        self.pending.push(Box::pin(async move {
            let result = Abortable::new(
                async move {
                    let state = owned.handle.changed().await;
                    (owned, state)
                },
                cancelled,
            )
            .await
            .ok();
            Progress::Source { key, result }
        }));
    }

    /// Re-ask the authority of every live watch on this connection, after the
    /// connection's own refresh confirmed `current` (row A3), so revoked access
    /// ends a watch without a commit. One task for the whole connection, holding
    /// each watch's original owner until it returns; a tick while it runs starts
    /// nothing. If it has not returned within the handshake timeout the
    /// connection closes (row A3b); the task still keeps those owners until its
    /// adapter awaits return.
    pub fn recheck(&mut self, state: &ProductRouteState, current: &AuthenticatedSession) {
        if self.recheck_pending {
            return;
        }
        let live: Vec<_> = self
            .targets
            .iter()
            .filter(|target| target.accepted && !self.deliveries.retiring(&target.id))
            .map(|target| (target.key, target.selector.clone(), target.owner.task()))
            .collect();
        if live.is_empty() {
            return;
        }
        self.recheck_pending = true;
        self.recheck_generation += 1;
        let generation = self.recheck_generation;
        let keys: Vec<u64> = live.iter().map(|(key, _, _)| *key).collect();
        let state_for_task = state.clone();
        let current = current.clone();
        let task = tokio::spawn(async move {
            let (selectors, guards): (Vec<_>, Vec<_>) = live
                .into_iter()
                .map(|(_, selector, guard)| (selector, guard))
                .unzip();
            let results = WatchSelector::recheck(&selectors, &state_for_task, &current).await;
            for guard in guards {
                guard.completed();
            }
            results
        });
        self.pending.push(Box::pin(async move {
            let results = match task.await {
                Ok(results) => keys.into_iter().zip(results).collect(),
                Err(_) => keys
                    .into_iter()
                    .map(|key| (key, Err(WatchRefusal::Unavailable)))
                    .collect(),
            };
            Progress::Rechecked { results }
        }));
        let (abort, cancelled) = AbortHandle::new_pair();
        self.recheck_bound = Some(abort);
        let bound = state.settings.handshake_timeout();
        self.pending.push(Box::pin(async move {
            let overdue = Abortable::new(tokio::time::sleep(bound), cancelled)
                .await
                .is_ok();
            Progress::RecheckBound {
                generation,
                overdue,
            }
        }));
    }

    /// The one decision for a refused authority check, from a notice or the
    /// periodic check: it closes the connection only while its target is still
    /// live; a target unwatched since the check began is ignored (row A5).
    fn refusal_closes(&self, key: u64, refusal: WatchRefusal) -> Option<SessionCloseReason> {
        self.targets
            .iter()
            .find(|target| target.key == key)
            .filter(|target| !self.deliveries.retiring(&target.id))
            .map(|_| refusal.close_reason())
    }

    fn authorize(&mut self, key: u64, state: &ProductRouteState, session: &AuthenticatedSession) {
        let target = self
            .targets
            .iter_mut()
            .find(|target| target.key == key)
            .expect("notice retains original target");
        if target.authority_pending || self.deliveries.retiring(&target.id) {
            return;
        }
        target.authority_pending = true;
        let owner = target.owner.clone();
        let selector = target.selector.clone();
        let state = state.clone();
        let session = session.clone();
        let original_task = owner.task();
        let task = tokio::spawn(async move {
            let result = selector.authorize(&state, &session).await;
            original_task.completed();
            result
        });
        self.pending.push(Box::pin(async move {
            Progress::Authorized {
                key,
                result: task.await.unwrap_or(Err(WatchRefusal::Unavailable)),
            }
        }));
    }

    pub async fn next(
        &mut self,
        state: &ProductRouteState,
        session: &AuthenticatedSession,
    ) -> WatchOutcome {
        let Some(progress) = self.pending.next().await else {
            return WatchOutcome::Progress;
        };
        match progress {
            Progress::Installed {
                key,
                request,
                slot,
                result,
            } => {
                let Some(target) = self.targets.iter_mut().find(|target| target.key == key) else {
                    return WatchOutcome::Progress;
                };
                target.authority_pending = false;
                if let Some(bound) = target.registration_bound.take() {
                    bound.abort();
                }
                let deadline = target.registration_deadline;
                match result {
                    Ok(handle) if !self.deliveries.retiring(&target.id) => {
                        let previous = target.id.clone();
                        let id = self
                            .tokens
                            .next()
                            .expect("frontier exhaustion checked before installation");
                        self.tokens.accepted();
                        target.id = id.clone();
                        target.accepted = true;
                        self.deliveries.installed(&previous, id.clone());
                        self.source(key, handle);
                        let message = success(&request, &ConversationWatchResult { watch_id: id });
                        WatchOutcome::Reply(Box::new(
                            self.reply(key, message, slot, deadline, true),
                        ))
                    }
                    result => {
                        self.deliveries.retire(&target.id);
                        target.interest.store(false, Ordering::Release);
                        let code = match result {
                            Err(code) => code,
                            Ok(_) => ChangeWatchErrorCode::WatchClosed,
                        };
                        WatchOutcome::Reply(Box::new(self.reply(
                            key,
                            failure(&request, code.as_str()),
                            slot,
                            deadline,
                            false,
                        )))
                    }
                }
            }
            Progress::Rechecked { results } => {
                self.recheck_pending = false;
                if let Some(bound) = self.recheck_bound.take() {
                    bound.abort();
                }
                for (key, result) in results {
                    if let Err(refusal) = result {
                        if let Some(reason) = self.refusal_closes(key, refusal) {
                            return WatchOutcome::Close(reason);
                        }
                    }
                }
                WatchOutcome::Progress
            }
            Progress::RecheckBound {
                generation,
                overdue,
            } => {
                // Always closes: while the check is outstanding, no later watch
                // on this connection can be re-checked (A3b).
                if overdue && self.recheck_pending && generation == self.recheck_generation {
                    return WatchOutcome::Close(SessionCloseReason::TemporaryUnavailable);
                }
                WatchOutcome::Progress
            }
            Progress::RegistrationBound { key, overdue } => {
                let awaiting = self.targets.iter().any(|target| {
                    target.key == key && !target.accepted && target.authority_pending
                });
                if overdue && awaiting {
                    return WatchOutcome::Close(SessionCloseReason::TemporaryUnavailable);
                }
                WatchOutcome::Progress
            }
            Progress::Authorized { key, result } => {
                let Some(target) = self.targets.iter_mut().find(|target| target.key == key) else {
                    return WatchOutcome::Progress;
                };
                target.authority_pending = false;
                match result {
                    Err(refusal) => {
                        if let Some(reason) = self.refusal_closes(key, refusal) {
                            return WatchOutcome::Close(reason);
                        }
                    }
                    Ok(()) if !self.deliveries.retiring(&target.id) => {
                        self.deliveries.authorize(&target.id)
                    }
                    Ok(()) => {}
                }
                self.collect_retired();
                WatchOutcome::Progress
            }
            Progress::Source { key, result } => {
                let Some(target) = self.targets.iter_mut().find(|target| target.key == key) else {
                    return WatchOutcome::Progress;
                };
                target.source_wait = None;
                target.source_pending = false;
                let Some((owned, terminal)) = result else {
                    return WatchOutcome::Progress;
                };
                if self.deliveries.retiring(&target.id) {
                    return WatchOutcome::Progress;
                }
                let notice = terminal.map(Notice::Ended).unwrap_or(Notice::Changed);
                let admission = self.deliveries.notice(&target.id, notice);
                if terminal.is_none() {
                    self.source(key, owned.handle);
                }
                if admission {
                    self.authorize(key, state, session);
                }
                WatchOutcome::Progress
            }
            Progress::Acknowledged {
                key,
                activation,
                result,
            } => {
                let Some(target) = self.targets.iter_mut().find(|target| target.key == key) else {
                    return WatchOutcome::Progress;
                };
                target.acknowledgements -= 1;
                if result.is_err() {
                    return WatchOutcome::Close(SessionCloseReason::TemporaryUnavailable);
                }
                if activation && !self.deliveries.retiring(&target.id) {
                    self.deliveries.activate(&target.id);
                }
                self.collect_retired();
                WatchOutcome::Progress
            }
        }
    }

    pub fn collect_retired(&mut self) {
        for target in &self.targets {
            if self.deliveries.retiring(&target.id) {
                target.interest.store(false, Ordering::Release);
            }
        }
        self.targets.retain(|target| {
            let keep = !self.deliveries.retiring(&target.id)
                || target.authority_pending
                || target.source_pending
                || target.acknowledgements > 0
                || self.deliveries.in_flight(&target.id);
            if !keep {
                self.deliveries.remove(&target.id);
            }
            keep
        });
    }
}

impl Drop for ConnectionWatches {
    fn drop(&mut self) {
        for target in &self.targets {
            target.interest.store(false, Ordering::Release);
        }
        self.deliveries.close();
        // Dropping observers detaches admitted JoinHandles; their original owners
        // remain in task bodies until actual adapter awaits complete.
    }
}
