//! Composition of one finite protected native run and its private cache.
//!
//! ```text
//! profile --> private state --> saved credential (key, pin, id, receiver)
//!   --> pinned status --> Active: protected session (credential id) --> driver
//!                     --> Terminal: purge the saved receiver, with receipt
//!   --> an authority refusal during the run --> pinned status again
//! ```
//! `watch` holds the same session for its bounded loop: registration, then
//! one finite records pass per operation, each from the durable checkpoint.
//! Arrows are calls in order (design rows PC1–PC6). No read happens without a
//! fresh Active status, and nothing is purged without a Terminal one.
use super::device::{self, client_failure, pinned, status_json, Device};
use super::profile::{Profile, ProfileError};
use crate::app::dependencies::RuntimeDependencies;
use crate::composition::local_auth::SystemClock;
use crate::conversation::domain::ConversationId;
use crate::product::generated::PASSIVE_MIN_REQUEST_TIMEOUT_MS;
use crate::read_only_sync::application::device::{
    asks_status, CachePurges, PinnedStatus, PurgeBeforeEnd, PurgeReceipt,
};
use crate::read_only_sync::application::driver::{run_catalogue, run_records};
use crate::read_only_sync::application::watch::{
    discover, follow, PassResult, Registered, Wait, WatchPass, WatchSession,
};
use crate::read_only_sync::application::{
    CacheError, CachePolicy, Cancellation, GatewayError, GatewayPolicy,
};
use crate::read_only_sync::entrypoint::watch::WatchLines;
use crate::read_only_sync::entrypoint::{online, Command, CommandError};
use crate::read_only_sync::infrastructure::cache::ReadOnlyCache;
use crate::read_only_sync::infrastructure::gateway::{
    DeviceEvidence, GatewayAuthorizer, GatewayConnection, LocalConnector, RecordGatewaySource,
    Session,
};
use nessa_auth::adapters::pairing::NativeIdentity;
use nessa_auth::application::pairing::ClientPendingStore;
use nessa_sync::replication::domain::Id;
use serde_json::{json, Value};
#[cfg(test)]
use std::cell::RefCell;
use std::io::{Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    // One-shot scheduling witness for actual composition saved-output races.
    static BEFORE_SAVED_REFRESH: RefCell<Option<Box<dyn FnOnce() + Send>>> = const { RefCell::new(None) };
}

struct CommandCancellation;
impl Cancellation for CommandCancellation {
    fn cancelled(&self) -> bool {
        false
    }
}

/// The device's saved enrollment, opened once per command. Its client reads
/// status through `store`, which purges the cache before an ended enrollment's
/// record goes.
struct Paired {
    device: Device,
    store: Arc<PurgeBeforeEnd>,
    identity: NativeIdentity,
    pin: [u8; 44],
    credential: String,
    /// The profile's cache: where this run reads to, and what a purge empties.
    cache: PathBuf,
}

/// `pair PROFILE` and `status PROFILE`.
pub(super) fn execute_device(
    command: &Command,
    policy: CachePolicy,
    input: &mut dyn Read,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let (profile_path, pairing) = match command {
        Command::Pair { profile } => (profile, true),
        Command::Status { profile } => (profile, false),
        _ => return Err(CommandError::Arguments),
    };
    let operation = if pairing { "pair" } else { "status" };
    if !pairing {
        // A device with an issued credential reads status through the purge;
        // one still pending has no receiver, and so no cached data.
        match open_paired(profile_path, policy) {
            Ok(paired) => {
                return match paired.device.status() {
                    Ok(status) => online::write(
                        json!({"operation":operation,"successful":true,"enrollment":status_json(&status),"purge":purge_json(paired.store.purge())}),
                        output,
                    ),
                    Err(error) => {
                        online::write(
                            json!({"operation":operation,"successful":false,"enrollmentFailure":client_failure(error),"purge":purge_json(paired.store.purge())}),
                            output,
                        )?;
                        Err(CommandError::OnlineRefused)
                    }
                };
            }
            Err(OpenError::NotPaired) => {}
            Err(error) => return configuration_failure(operation, error.code(), output),
        }
    }
    let opened = (|| {
        let profile = Profile::load(profile_path)?;
        let state = Arc::new(profile.private_state()?);
        let device = Device::new(&profile, state).map_err(|_| ProfileError::Unavailable)?;
        Ok::<_, ProfileError>(device)
    })();
    let device = match opened {
        Ok(device) => device,
        Err(error) => return configuration_failure(operation, profile_code(error), output),
    };
    let result = if pairing {
        let Some(code) = device::read_code(input) else {
            return configuration_failure(operation, "invalidCode", output);
        };
        device.pair(code)
    } else {
        device.status()
    };
    match result {
        Ok(status) => online::write(
            json!({"operation":operation,"successful":true,"enrollment":status_json(&status)}),
            output,
        ),
        Err(error) => {
            online::write(
                json!({"operation":operation,"successful":false,"enrollmentFailure":client_failure(error)}),
                output,
            )?;
            Err(CommandError::OnlineRefused)
        }
    }
}

pub(super) fn execute(
    command: &Command,
    policy: CachePolicy,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    let profile_path = match command {
        Command::Records { profile, .. }
        | Command::Catalogue { profile, .. }
        | Command::Watch { profile, .. } => profile,
        _ => return Err(CommandError::Arguments),
    };
    let operation = match command {
        Command::Records { .. } => "records",
        Command::Watch { .. } => "watch",
        _ => "catalogue",
    };
    let paired = match open_paired(profile_path, policy) {
        Ok(paired) => paired,
        Err(error) => return configuration_failure(operation, error.code(), output),
    };
    let cache_path = paired.cache.as_path();
    // Row PC2–PC4: a fresh pinned status decides before any read.
    let status = match paired.device.status() {
        Ok(status) => status,
        Err(error) => {
            online::write(
                json!({"operation":operation,"successful":false,"connectionCheck":"attempted","enrollmentFailure":client_failure(error),"purge":purge_json(paired.store.purge())}),
                output,
            )?;
            return Err(CommandError::OnlineRefused);
        }
    };
    let enrollment = status_json(&status);
    let Some(decision) = pinned(&status) else {
        return refused(
            operation,
            json!({"enrollment":enrollment,"enrollmentFailure":{"code":"wire"},"purge":purge_json(paired.store.purge())}),
            output,
        );
    };
    let (receiver, access_epoch) = match decision {
        PinnedStatus::Active {
            receiver,
            access_epoch,
        } => (receiver, access_epoch),
        // Row PC3: the client ended the record, and the store purged first.
        PinnedStatus::Terminal => {
            let purge = purge_json(paired.store.purge());
            return refused(
                operation,
                json!({"enrollment":enrollment,"purge":purge}),
                output,
            );
        }
        PinnedStatus::NotActive => {
            // A purge here would mean a credential record was ended without a
            // Terminal status; it is reported, never hidden.
            let purge = purge_json(paired.store.purge());
            return refused(
                operation,
                json!({"enrollment":enrollment,"purge":purge}),
                output,
            );
        }
    };
    let connection = match Session::connect(
        paired.device.gateway(),
        DeviceEvidence {
            identity: &paired.identity,
            pin: paired.pin,
            credential: &paired.credential,
        },
        "read-only-example",
        &LocalConnector,
        RuntimeDependencies::default().clock,
        Arc::new(CommandCancellation),
        default_gateway_policy().map_err(CommandError::Gateway)?,
    ) {
        Ok(session) => GatewayConnection::new(session),
        Err(error) => {
            let recheck = recheck(&paired, error);
            online::write(
                json!({"operation":operation,"successful":false,"connectionCheck":"attempted","enrollment":enrollment,"transportFailure":online::gateway_failure(error),"recheck":recheck}),
                output,
            )?;
            return Err(CommandError::OnlineRefused);
        }
    };
    let run = Run {
        paired: &paired,
        cache_path,
        policy,
        connection,
        receiver,
        access_epoch,
        enrollment,
    };
    match command {
        Command::Records {
            conversation,
            pages,
            ..
        } => run.records(conversation.clone(), *pages, output),
        Command::Catalogue { pages, .. } => run.catalogue(*pages, output),
        Command::Watch {
            conversation,
            pages,
            max_passes,
            ..
        } => run.watch(conversation.clone(), *pages, *max_passes, output),
        _ => Err(CommandError::Arguments),
    }
}

/// Why a device's saved enrollment could not be opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenError {
    Profile(ProfileError),
    /// No issued credential: nothing, or only a pending enrollment.
    NotPaired,
    PrivateState,
    Runtime,
}
impl OpenError {
    fn code(self) -> &'static str {
        match self {
            Self::Profile(error) => profile_code(error),
            Self::NotPaired => "notPaired",
            Self::PrivateState => "privateStateUnavailable",
            Self::Runtime => "runtimeUnavailable",
        }
    }
}

fn open_paired(path: &Path, policy: CachePolicy) -> Result<Paired, OpenError> {
    let profile = Profile::load(path).map_err(OpenError::Profile)?;
    let state = Arc::new(profile.private_state().map_err(OpenError::Profile)?);
    // Row PC1: nothing is read, dialled or opened without an issued credential.
    let saved = state
        .load_credential()
        .map_err(|_| OpenError::PrivateState)?
        .ok_or(OpenError::NotPaired)?;
    let credential = saved.credential().as_str().to_owned();
    let receiver = Id::new(saved.receiver().as_str()).map_err(|_| OpenError::PrivateState)?;
    let (key, pin, _) = saved.into_enrollment().into_parts();
    let identity = NativeIdentity::restore(key).map_err(|_| OpenError::PrivateState)?;
    let cache = profile.cache.clone();
    let store = Arc::new(PurgeBeforeEnd::new(
        state,
        receiver,
        Box::new(move || {
            ReadOnlyCache::open(&cache, policy, Arc::new(SystemClock))
                .map(|cache| Box::new(cache) as Box<dyn CachePurges + Send>)
        }),
    ));
    let device = Device::new(&profile, store.clone()).map_err(|_| OpenError::Runtime)?;
    Ok(Paired {
        device,
        store,
        identity,
        pin,
        credential,
        cache: profile.cache,
    })
}

/// Row PC5: after an authority refusal, ask the pinned status once more. Only
/// a Terminal status ends the record, purging first; Active reports the epoch
/// the next command will use.
fn recheck(paired: &Paired, failure: GatewayError) -> Value {
    if !asks_status(failure) {
        return Value::Null;
    }
    match paired.device.status() {
        Ok(status) => {
            json!({"enrollment":status_json(&status),"purge":purge_json(paired.store.purge())})
        }
        Err(error) => {
            json!({"enrollmentFailure":client_failure(error),"purge":purge_json(paired.store.purge())})
        }
    }
}

/// The purge an ended enrollment ran: its receipt, the cache failure that kept
/// the record, or `null` when none ran.
fn purge_json(purge: Option<Result<PurgeReceipt, CacheError>>) -> Value {
    match purge {
        None => Value::Null,
        Some(Ok(receipt)) => json!({"receiver":receipt.receiver.as_str(),
            "cause":"terminalEnrollment","initiator":"gatewayStatus",
            "transcripts":receipt.transcripts.to_string(),"records":receipt.records.to_string(),
            "catalogueEntries":receipt.catalogue_entries.to_string(),
            "observedAtMs":receipt.observed_at_ms.to_string()}),
        Some(Err(error)) => json!({"failure":online::cache_failure(&error)}),
    }
}

fn refused(operation: &str, fields: Value, output: &mut dyn Write) -> Result<(), CommandError> {
    let mut report =
        json!({"operation":operation,"successful":false,"connectionCheck":"attempted"});
    if let (Some(report), Value::Object(fields)) = (report.as_object_mut(), fields) {
        report.extend(fields);
    }
    online::write(report, output)?;
    Err(CommandError::OnlineRefused)
}

fn configuration_failure(
    operation: &str,
    code: &str,
    output: &mut dyn Write,
) -> Result<(), CommandError> {
    online::write(
        json!({"operation":operation,"successful":false,"connectionCheck":"notPerformed","configurationFailure":code}),
        output,
    )?;
    Err(CommandError::OnlineRefused)
}

fn profile_code(error: ProfileError) -> &'static str {
    match error {
        ProfileError::Unavailable => "unavailable",
        ProfileError::TooLarge => "tooLarge",
        ProfileError::Invalid => "invalid",
        ProfileError::PrivateStateUnavailable => "privateStateUnavailable",
    }
}
// B1: one whole-callback budget, not a renewed allowance for each RPC.
// Finite passes may still exhaust this budget while retaining confirmed pages.
fn default_gateway_policy() -> Result<GatewayPolicy, GatewayError> {
    GatewayPolicy::new(5000, PASSIVE_MIN_REQUEST_TIMEOUT_MS, 16)
}

/// One admitted read: the session, and the receiver and epoch Active named.
struct Run<'a> {
    paired: &'a Paired,
    cache_path: &'a Path,
    policy: CachePolicy,
    connection: GatewayConnection,
    receiver: Id,
    access_epoch: u64,
    enrollment: Value,
}
impl Run<'_> {
    fn records(
        self,
        conversation: ConversationId,
        pages: usize,
        output: &mut dyn Write,
    ) -> Result<(), CommandError> {
        let mut source =
            self.connection
                .records(self.receiver.clone(), self.access_epoch, conversation);
        let discovery = self
            .connection
            .run(|| source.discover())
            .map_err(CommandError::Gateway)?;
        let scope = match discovery.result {
            Some(Ok((scope, _))) => scope,
            _ => return self.discovery_failure(discovery.outcome, output),
        };
        let mut cache = open_cache(self.cache_path, self.policy, output)?;
        let mut authorizer = source.authorizer();
        let attempt = self
            .connection
            .run(|| {
                run_records(
                    &scope,
                    &mut authorizer,
                    &mut source,
                    &mut cache,
                    self.policy.suffix_page(),
                    pages,
                    &SystemClock,
                )
            })
            .map_err(CommandError::Gateway)?;
        let refusal = cache.take_refusal();
        let recheck = attempt
            .outcome
            .failure
            .map(|failure| recheck(self.paired, failure));
        let saved =
            cache.retained_transcript_state(scope.receiver(), scope.origin(), scope.stream());
        online::write_records(
            &attempt,
            saved,
            refusal,
            json!({"enrollment":self.enrollment,"recheck":recheck}),
            output,
        )
    }
    /// Rows W1–W16: discovery and the cache as for `sync-records`, then the
    /// bounded watch loop on this one connection.
    fn watch(
        self,
        conversation: ConversationId,
        pages: usize,
        max_passes: NonZeroUsize,
        output: &mut dyn Write,
    ) -> Result<(), CommandError> {
        let mut source = self.connection.records(
            self.receiver.clone(),
            self.access_epoch,
            conversation.clone(),
        );
        // Row W17 before registration: a preparing discovery is asked again.
        let discovery = discover(|| self.connection.run(|| source.discover()))
            .map_err(CommandError::Gateway)?;
        let scope = match discovery.result {
            Some(Ok((scope, _))) => scope,
            _ => return self.discovery_failure(discovery.outcome, output),
        };
        let cache = open_cache(self.cache_path, self.policy, output)?;
        let authorizer = source.authorizer();
        let mut session = ConnectionWatch {
            run: &self,
            conversation,
            pages,
            scope,
            source,
            authorizer,
            cache,
            rechecked: None,
        };
        let mut lines = WatchLines::new(output, self.enrollment.clone());
        let end = follow(&mut session, &mut lines, max_passes).map_err(|_| CommandError::Output)?;
        // Row PC5: `recheck` asks only after an authority refusal, and once:
        // a pass that already asked for this cause carries the answer.
        let recheck = match (end.cause, session.rechecked.take()) {
            (Some(cause), Some((asked, answer))) if asked == cause => answer,
            (cause, _) => cause.map_or(Value::Null, |cause| recheck(self.paired, cause)),
        };
        lines.end(end, recheck)
    }
    fn catalogue(self, pages: usize, output: &mut dyn Write) -> Result<(), CommandError> {
        let mut source = self
            .connection
            .catalogue(self.receiver.clone(), self.access_epoch);
        let discovery = self
            .connection
            .run(|| source.discover())
            .map_err(CommandError::Gateway)?;
        let scope = match discovery.result {
            Some(Ok((scope, _))) => scope,
            _ => return self.discovery_failure(discovery.outcome, output),
        };
        let mut cache = open_cache(self.cache_path, self.policy, output)?;
        let mut authorizer = source.authorizer();
        let attempt = self
            .connection
            .run(|| run_catalogue(&scope, &mut authorizer, &mut source, &mut cache, pages))
            .map_err(CommandError::Gateway)?;
        let refusal = cache.take_refusal();
        let recheck = attempt
            .outcome
            .failure
            .map(|failure| recheck(self.paired, failure));
        let saved =
            cache.retained_catalogue_progress(scope.receiver(), scope.origin(), scope.stream());
        online::write_catalogue(
            &attempt,
            saved,
            refusal,
            json!({"enrollment":self.enrollment,"recheck":recheck}),
            output,
        )
    }
    fn discovery_failure(
        &self,
        outcome: crate::read_only_sync::application::GatewayOutcome,
        output: &mut dyn Write,
    ) -> Result<(), CommandError> {
        let recheck = outcome.failure.map(|failure| recheck(self.paired, failure));
        online::write(
            json!({"successful":false,"connectionCheck":"performed","connectionOperation":outcome.operation.to_string(),"transportFailure":outcome.failure.map(online::gateway_failure),"discoveryFailure":true,"enrollment":self.enrollment,"recheck":recheck}),
            output,
        )?;
        Err(CommandError::OnlineRefused)
    }
}

/// The watch loop's session port over this run's connection, driver and cache.
struct ConnectionWatch<'a> {
    run: &'a Run<'a>,
    conversation: ConversationId,
    pages: usize,
    scope: nessa_sync::replication::domain::Scope,
    source: RecordGatewaySource,
    authorizer: GatewayAuthorizer,
    cache: ReadOnlyCache,
    /// The last pass's gateway failure and the `recheck` its report carries.
    rechecked: Option<(GatewayError, Value)>,
}
impl WatchSession for ConnectionWatch<'_> {
    type Report = Value;
    fn register(&mut self) -> Result<Registered, GatewayError> {
        self.run.connection.watch_records(
            &self.run.receiver,
            self.run.access_epoch,
            &self.conversation,
        )
    }
    fn wait(&mut self) -> Result<Wait, GatewayError> {
        self.run.connection.wait_hint()
    }
    fn pass(&mut self) -> Result<WatchPass<Value>, GatewayError> {
        let attempt = self.run.connection.run(|| {
            run_records(
                &self.scope,
                &mut self.authorizer,
                &mut self.source,
                &mut self.cache,
                self.run.policy.suffix_page(),
                self.pages,
                &SystemClock,
            )
        })?;
        let refusal = self.cache.take_refusal();
        let saved = self.cache.retained_transcript_state(
            self.scope.receiver(),
            self.scope.origin(),
            self.scope.stream(),
        );
        let recheck = attempt.outcome.failure.map(|failure| {
            let answer = recheck(self.run.paired, failure);
            self.rechecked = Some((failure, answer.clone()));
            answer
        });
        let device = json!({"enrollment":self.run.enrollment,"recheck":recheck});
        let (report, successful) = online::records_report(&attempt, saved, refusal, device);
        let result = match &attempt.result {
            _ if !successful => PassResult::Failed(attempt.outcome.failure),
            Some(Ok(run)) if run.complete => PassResult::Complete,
            _ => PassResult::Incomplete,
        };
        Ok(WatchPass { report, result })
    }
}

#[cfg(test)]
#[path = "../../../tests/composition/read_only_online.rs"]
mod tests;

fn open_cache(
    path: &Path,
    policy: CachePolicy,
    output: &mut dyn Write,
) -> Result<ReadOnlyCache, CommandError> {
    match ReadOnlyCache::open(path, policy, Arc::new(SystemClock)) {
        Ok(cache) => {
            #[cfg(test)]
            let mut cache = cache;
            #[cfg(test)]
            BEFORE_SAVED_REFRESH.with(|hook| {
                if let Some(proceed) = hook.borrow_mut().take() {
                    cache.before_projection_refresh(proceed);
                }
            });
            Ok(cache)
        }
        Err(error) => {
            online::write(
                json!({"successful":false,"connectionCheck":"performed","cacheRefusal":online::cache_failure(&error)}),
                output,
            )?;
            Err(CommandError::OnlineRefused)
        }
    }
}
