//! Native enrollment composition preserves the existing physical worker and
//! purge-before-end policy. Status effects are collected inside its admission.
use super::RetainedOwnerStatus;
use super::{cache_failure, retained_policy, ReadFailure, RetainedCache};
use crate::pairing::{
    owner_recovery, NativeClientError, NativeEnrollmentClient, NativeRetryOutcome,
    OwnerPhonePreparation, OwnerPhoneState,
};
use crate::read_only_sync::application::device::{CachePurges, PurgeBeforeEnd, PurgeReceipt};
use crate::read_only_sync::infrastructure::cache::ReadOnlyCache;
use nessa_auth::{
    adapters::pairing::{CryptoRng, ManualCode, RngCore},
    application::{pairing::ClientPendingStore, ports::Clock as WallClock},
    domain::{pairing::DisclosedConsent, ResourceId},
};
use nessa_protocol::{
    clock::Clock as MonotonicClock,
    pairing::{socket::NativeWakeReport, wire::NativePairingStatus},
};
use std::{net::TcpStream, path::PathBuf, sync::Arc};

/// Original durable receiver purge evidence. Re-reading it does not repeat
/// deletion or establish that credential removal was acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedPurge(PurgeReceipt);
impl RetainedPurge {
    /// The receiver whose retained rows were removed and fenced.
    pub fn receiver(&self) -> &ResourceId {
        &self.0.receiver
    }
    /// Transcript streams removed by the original purge.
    pub fn transcripts(&self) -> u64 {
        self.0.transcripts
    }
    /// Physical records removed by the original purge.
    pub fn records(&self) -> u64 {
        self.0.records
    }
    /// Catalogue entries removed by the original purge.
    pub fn catalogue_entries(&self) -> u64 {
        self.0.catalogue_entries
    }
    /// The original observation time from the injected wall clock, milliseconds.
    pub fn observed_at_ms(&self) -> u64 {
        self.0.observed_at_ms
    }
}

/// One admitted physical status operation's immutable outcome and purge effect.
/// A successful purge and a failed/uncertain credential removal are independent.
/// ```compile_fail
/// use nessa_client_core::{pairing::NativeClientError, retained::RetainedEnrollmentStatus};
/// fn replace(mut status: RetainedEnrollmentStatus) {
///     *status.enrollment() = Err(NativeClientError::NoPending);
/// }
/// ```
/// ```compile_fail
/// use nessa_client_core::retained::RetainedEnrollmentStatus;
/// fn erase(mut status: RetainedEnrollmentStatus) {
///     status.purge = None;
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedEnrollmentStatus {
    enrollment: Result<NativePairingStatus, NativeClientError>,
    purge: Option<Result<RetainedPurge, ReadFailure>>,
}
impl RetainedEnrollmentStatus {
    /// Pinned enrollment result, including a storage error after a successful purge.
    pub fn enrollment(&self) -> &Result<NativePairingStatus, NativeClientError> {
        &self.enrollment
    }
    /// This operation's purge attempt, captured before its physical permit
    /// releases. `None` reports no attempt in this operation, not historical state.
    pub fn purge(&self) -> Option<&Result<RetainedPurge, ReadFailure>> {
        self.purge.as_ref()
    }
}

/// One long-lived device enrollment worker over one private store/cache scope.
/// Caller composition owns sockets and must drain this owner before replacing
/// its scope: retain the client and await `shutdown()` before replacement.
/// Canceling an `enroll`, `status` or `retry` future, or dropping the final
/// client handle, leaves admitted physical work running with its private store
/// and admission permit until completion. The
/// `canceled_caller_and_shutdown_waiter_keep_physical_work_and_capture` and
/// `last_handle_drop_keeps_private_state_owned_until_physical_completion` tests
/// exercise that ownership. These operations require a Tokio runtime.
/// Private stores retain immutable issued bindings per intent and
/// atomic compare-and-remove; out-of-band store replacement is not supported.
pub struct RetainedEnrollmentClient {
    client: NativeEnrollmentClient,
    store: Arc<PurgeBeforeEnd>,
    original: Option<Arc<dyn OwnerPhoneState>>,
}
impl RetainedEnrollmentClient {
    /// Compose without opening a socket or cache. `path` names this scope's
    /// private cache in an existing private directory. The injected wall clock
    /// timestamps purges; the monotonic clock owns enrollment deadlines.
    /// Cache opening and purge run in the existing physical worker.
    pub fn new(
        state: Arc<dyn ClientPendingStore>,
        path: PathBuf,
        wall: Arc<dyn WallClock>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ReadFailure> {
        let policy = retained_policy().map_err(|error| cache_failure(&error))?;
        let store = Arc::new(PurgeBeforeEnd::new(
            state,
            Box::new(move || {
                ReadOnlyCache::open(&path, policy, wall.clone())
                    .map(|cache| Box::new(cache) as Box<dyn CachePurges + Send>)
            }),
        ));
        Ok(Self {
            client: NativeEnrollmentClient::new(store.clone(), clock),
            store,
            original: None,
        })
    }

    /// Compose original recovery and current read-only reads with one combined private owner.
    /// No separate enrollment/original state can be injected into this scope.
    pub fn owner_phone(
        state: Arc<dyn OwnerPhoneState>,
        path: PathBuf,
        wall: Arc<dyn WallClock>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ReadFailure> {
        let mut value = Self::new(state.clone(), path, wall, clock)?;
        value.original = Some(state);
        Ok(value)
    }

    /// Read only the exact original over its fresh pinned key channel.
    /// This performs no adoption, purge, removal or remote mutation.
    pub async fn recover_owner_phone(
        &self,
        stream: TcpStream,
        expected: Arc<OwnerPhonePreparation>,
        received: Option<DisclosedConsent>,
    ) -> Result<RetainedOwnerStatus, NativeClientError> {
        let state = self.original.clone().ok_or(NativeClientError::NoPending)?;
        self.client
            .owner_recovery_completed(
                stream,
                state,
                expected,
                received,
                || {},
                |result, _, _| match result {
                    Ok(value) => RetainedOwnerStatus::recovered(value, None, None),
                    Err(error) => RetainedOwnerStatus::recovery_failed(error),
                },
            )
            .await
    }

    /// Recover and explicitly reconcile owner enrollment within one physical permit.
    /// Later local failures retain the original remote observation and purge facts.
    pub async fn status_owner_phone(
        &self,
        stream: TcpStream,
        expected: Arc<OwnerPhonePreparation>,
        received: Option<DisclosedConsent>,
    ) -> Result<RetainedOwnerStatus, NativeClientError> {
        let state = self.original.clone().ok_or(NativeClientError::NoPending)?;
        let begin = self.store.clone();
        let finish = self.store.clone();
        self.client
            .owner_recovery_completed(
                stream,
                state,
                expected,
                received,
                move || begin.begin_status(),
                move |result, ending, state| match result {
                    Err(error) => RetainedOwnerStatus::recovery_failed(error),
                    Ok(value) => {
                        let settlement = owner_recovery::settle(&value, state, ending);
                        let purge = finish.finish_status().map(|result| {
                            result
                                .map(RetainedPurge)
                                .map_err(|error| cache_failure(&error))
                        });
                        RetainedOwnerStatus::recovered(value, Some(settlement), purge)
                    }
                },
            )
            .await
    }

    /// Enroll with the caller's socket, code and fallible entropy. The existing
    /// enrollment owner refuses a new attempt when pending or issued state exists.
    pub async fn enroll<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        stream: TcpStream,
        code: ManualCode,
        entropy: R,
    ) -> Result<NativePairingStatus, NativeClientError> {
        self.client.enroll(stream, code, entropy).await
    }

    /// Recover pinned status and this operation's purge effect. Begin/finish
    /// collection runs inside the same physical admission as the exchange.
    /// An outer error means no completion was captured (for example Busy or
    /// worker fault); it does not prove cleanup absent. Recover durable purge
    /// evidence through `RetainedCache::purge_receipt` after uncertain delivery.
    pub async fn status(
        &self,
        stream: TcpStream,
        received: Option<DisclosedConsent>,
    ) -> Result<RetainedEnrollmentStatus, NativeClientError> {
        let begin = self.store.clone();
        let finish = self.store.clone();
        self.client
            .status_completed(
                stream,
                received,
                move || begin.begin_status(),
                move |enrollment| RetainedEnrollmentStatus {
                    enrollment,
                    purge: finish.finish_status().map(|result| {
                        result
                            .map(RetainedPurge)
                            .map_err(|error| cache_failure(&error))
                    }),
                },
            )
            .await
    }

    /// Retry through the existing strict-pin owner. Its original Unclaimed
    /// receipt is returned only when the entire retry succeeds; later failures
    /// preserve private recovery state but do not return that earlier receipt.
    /// A retry error does not authorize a receiver purge.
    pub async fn retry<R: RngCore + CryptoRng + Send + 'static>(
        &self,
        original: TcpStream,
        retry: TcpStream,
        code: ManualCode,
        entropy: R,
    ) -> Result<NativeRetryOutcome, NativeClientError> {
        self.client.retry(original, retry, code, entropy).await
    }

    /// First shutdown's wake result, distinct from physical worker completion.
    pub fn wake_report(&self) -> Option<NativeWakeReport> {
        self.client.wake_report()
    }

    /// Close enrollment admission and wake its current sockets. This is a
    /// signal, not physical completion; await `shutdown()` before replacement.
    pub fn request_shutdown(&self) {
        self.client.request_shutdown();
    }

    /// Exclude new work, wake current sockets and await actual worker drain.
    /// Canceling this wait does not reopen admission or terminate the worker.
    pub async fn shutdown(&self) {
        self.client.shutdown().await
    }
}

impl RetainedCache {
    /// Read original durable purge evidence without dialing or repeating a
    /// purge, even if the enrollment record was already unlinked. This is a
    /// blocking private-cache read; native UI composition runs it off the UI.
    pub fn purge_receipt(&mut self, receiver: &str) -> Result<Option<RetainedPurge>, ReadFailure> {
        let receiver = ResourceId::new(receiver).map_err(|_| ReadFailure::Protocol)?;
        self.cache
            .purge_receipt(&receiver)
            .map(|receipt| receipt.map(RetainedPurge))
            .map_err(|error| cache_failure(&error))
    }
}
