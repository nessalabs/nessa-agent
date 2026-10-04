//! Bounded synchronous sync-engine reads over the authenticated metadata port.
//! Product catalogue dispatch supplies the authenticated network boundary.
//! This adapter accepts only a scope and caller already chosen by the host,
//! and never opens a writer or provider.

use crate::conversation::application::{
    CatalogueDescriptor, CataloguePageRequest, CatalogueValue, ConversationCaller,
    ConversationCatalogue, ConversationError,
};
use nessa_protocol::conversation::catalogue_metadata::CatalogueMetadata;
use nessa_protocol::conversation::catalogue_payload;
use nessa_protocol::conversation::domain::{
    check_catalogue_scope_identity, conversation_catalogue_schema, conversation_catalogue_stream,
    ConversationId,
};
use nessa_sync::replication::catalogue::{
    validate_catalogue_pass, validate_manifest_request, CataloguePass, CatalogueSource,
    CatalogueSourceError, EntryKey, ManifestEntry, ManifestPage, ManifestRequest, ResolvedEntry,
    MAX_CATALOGUE_ENTRIES, MAX_CATALOGUE_PAYLOAD_BYTES,
};
use nessa_sync::replication::domain::{Id, Scope};
use std::io::Result as IoResult;
use std::sync::mpsc::{Receiver, RecvError, Sender, SyncSender};
use std::sync::{mpsc, Arc, Mutex};
#[cfg(test)]
use std::thread;
use std::thread::{Builder as ThreadBuilder, JoinHandle};
use tokio::runtime::{Builder, Runtime};

const QUEUE_CAPACITY: usize = 32;

enum Command {
    Head(Scope, Sender<Result<u64, CatalogueSourceError>>),
    Manifest(
        ManifestRequest,
        Sender<Result<ManifestPage, CatalogueSourceError>>,
    ),
    Resolve(
        CataloguePass,
        Id,
        usize,
        Sender<Result<ResolvedEntry, CatalogueSourceError>>,
    ),
}

/// Source refusal and an unexpected physical worker exit have distinct meanings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogueWorkerError {
    Source(CatalogueSourceError),
    WorkerPanicked,
}

struct Worker {
    state: Mutex<WorkerState>,
}
struct WorkerState {
    sender: Option<SyncSender<Command>>,
    thread: Option<JoinHandle<()>>,
    completion: Option<Result<(), CatalogueWorkerError>>,
}
impl Worker {
    fn finish(&self) -> Result<(), CatalogueWorkerError> {
        let mut state = self.state.lock().unwrap();
        if let Some(completion) = &state.completion {
            return completion.clone();
        }
        // Closing admission drains the bounded command queue before join returns.
        drop(state.sender.take());
        let completion = match state.thread.take() {
            Some(worker) => worker
                .join()
                .map_err(|_| CatalogueWorkerError::WorkerPanicked),
            _ => Ok(()),
        };
        state.completion = Some(completion.clone());
        completion
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        // The synchronous source owns its physical worker through the last drop.
        // Product callers explicitly drain on their tracked non-entered thread.
        let _ = self.finish();
    }
}

type CatalogueReadiness = Result<(Scope, Option<u64>), CatalogueSourceError>;

enum SourceScope {
    Expected(Scope),
    Discover { receiver: Id, origin: Id, epoch: Id },
}

/// Cloneable synchronous source over one exact metadata owner and scope.
/// Construction, reads and drain can block: use a non-entered blocking thread.
/// The source-owned runtime has its own blocking pool. Last drop closes its
/// queue and joins its worker; explicit drain additionally preserves join faults.
#[derive(Clone)]
pub struct NessaCatalogueSource {
    scope: Scope,
    worker: Arc<Worker>,
}
impl NessaCatalogueSource {
    pub fn new(
        catalogue: Arc<dyn ConversationCatalogue>,
        caller: ConversationCaller,
        scope: Scope,
    ) -> Result<Self, CatalogueWorkerError> {
        Self::start_with_spawn(catalogue, caller, scope, spawn_worker, make_runtime)
    }

    /// Capture one actual metadata head and construct its matching sync scope.
    /// The supplied receiver, origin and opaque epoch are trusted host facts;
    /// metadata owns the incarnation and revision. No second head is observed.
    pub fn discover(
        catalogue: Arc<dyn ConversationCatalogue>,
        caller: ConversationCaller,
        receiver: Id,
        origin: Id,
        epoch: Id,
    ) -> Result<(Self, u64), CatalogueWorkerError> {
        let (source, head) = Self::start_with_identity(
            catalogue,
            caller,
            SourceScope::Discover {
                receiver,
                origin,
                epoch,
            },
            spawn_worker,
            make_runtime,
        )?;
        Ok((source, head.expect("discovery initializes a captured head")))
    }

    fn start_with_spawn(
        catalogue: Arc<dyn ConversationCatalogue>,
        caller: ConversationCaller,
        scope: Scope,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> IoResult<JoinHandle<()>>,
        make_runtime: impl FnOnce() -> IoResult<Runtime> + Send + 'static,
    ) -> Result<Self, CatalogueWorkerError> {
        Self::start_with_identity(
            catalogue,
            caller,
            SourceScope::Expected(scope),
            spawn,
            make_runtime,
        )
        .map(|(source, _)| source)
    }

    fn start_with_identity(
        catalogue: Arc<dyn ConversationCatalogue>,
        caller: ConversationCaller,
        scope: SourceScope,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> IoResult<JoinHandle<()>>,
        make_runtime: impl FnOnce() -> IoResult<Runtime> + Send + 'static,
    ) -> Result<(Self, Option<u64>), CatalogueWorkerError> {
        Self::start_with_readiness(catalogue, caller, scope, spawn, make_runtime, |ready| {
            ready.recv()
        })
    }

    fn start_with_readiness(
        catalogue: Arc<dyn ConversationCatalogue>,
        caller: ConversationCaller,
        scope: SourceScope,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> IoResult<JoinHandle<()>>,
        make_runtime: impl FnOnce() -> IoResult<Runtime> + Send + 'static,
        receive: impl FnOnce(
            Receiver<CatalogueReadiness>,
        )
            -> Result<Result<(Scope, Option<u64>), CatalogueSourceError>, RecvError>,
    ) -> Result<(Self, Option<u64>), CatalogueWorkerError> {
        if let SourceScope::Expected(scope) = &scope {
            check_catalogue_scope_identity(&caller.organization_id, &caller.principal_id, scope)
                .map_err(CatalogueWorkerError::Source)?;
        }
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready, initialized) = mpsc::sync_channel(1);
        let worker = spawn(Box::new(move || {
            let runtime = match make_runtime() {
                Ok(runtime) => runtime,
                Err(_) => {
                    let _ = ready.send(Err(CatalogueSourceError::Unavailable));
                    return;
                }
            };
            let initialized_scope = match scope {
                SourceScope::Expected(scope) => Ok((scope, None)),
                SourceScope::Discover {
                    receiver,
                    origin,
                    epoch,
                } => runtime.block_on(async {
                    let head = catalogue
                        .head(&caller.organization_id, &caller.principal_id)
                        .await
                        .map_err(map_error)?;
                    let incarnation = Id::new(head.incarnation)
                        .map_err(|_| CatalogueSourceError::IdentityChanged)?;
                    Ok((
                        Scope::new(
                            receiver,
                            origin,
                            conversation_catalogue_stream(
                                &caller.organization_id,
                                &caller.principal_id,
                            ),
                            incarnation,
                            conversation_catalogue_schema(),
                            epoch,
                        ),
                        Some(head.revision),
                    ))
                }),
            };
            let (expected, head) = match initialized_scope {
                Ok(value) => value,
                Err(error) => {
                    let _ = ready.send(Err(error));
                    return;
                }
            };
            if ready.send(Ok((expected.clone(), head))).is_err() {
                return;
            }
            while let Ok(command) = receiver.recv() {
                match command {
                    Command::Head(scope, reply) => {
                        let _ = reply.send(runtime.block_on(read_head(
                            &*catalogue,
                            &caller,
                            &expected,
                            &scope,
                        )));
                    }
                    Command::Manifest(request, reply) => {
                        let _ = reply.send(runtime.block_on(read_manifest(
                            &*catalogue,
                            &caller,
                            &expected,
                            &request,
                        )));
                    }
                    Command::Resolve(pass, id, bound, reply) => {
                        let _ = reply.send(runtime.block_on(read_resolved(
                            &*catalogue,
                            &caller,
                            &expected,
                            &pass,
                            &id,
                            bound,
                        )));
                    }
                }
            }
        }))
        .map_err(|_| CatalogueWorkerError::Source(CatalogueSourceError::Unavailable))?;
        let initialized = receive(initialized);
        match initialized {
            Ok(Ok((scope, head))) => Ok((
                Self {
                    scope,
                    worker: Arc::new(Worker {
                        state: Mutex::new(WorkerState {
                            sender: Some(sender),
                            thread: Some(worker),
                            completion: None,
                        }),
                    }),
                },
                head,
            )),
            result => {
                drop(sender);
                if worker.join().is_err() {
                    return Err(CatalogueWorkerError::WorkerPanicked);
                }
                Err(CatalogueWorkerError::Source(match result {
                    Ok(Err(error)) => error,
                    _ => CatalogueSourceError::Unavailable,
                }))
            }
        }
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    /// Close admission and join the physical worker. Repeated calls share the
    /// completed result; all clones subsequently refuse new commands.
    pub fn finish(&self) -> Result<(), CatalogueWorkerError> {
        self.worker.finish()
    }
    fn enqueue(&self, command: Command) -> Result<(), CatalogueSourceError> {
        self.worker
            .state
            .lock()
            .unwrap()
            .sender
            .as_ref()
            .ok_or(CatalogueSourceError::Unavailable)?
            .try_send(command)
            .map_err(|_| CatalogueSourceError::Unavailable)
    }
}
fn spawn_worker(run: Box<dyn FnOnce() + Send>) -> IoResult<JoinHandle<()>> {
    ThreadBuilder::new()
        .name("nessa-catalogue-source".into())
        .spawn(run)
}
fn make_runtime() -> IoResult<Runtime> {
    Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
}

impl CatalogueSource for NessaCatalogueSource {
    fn head(&mut self, scope: &Scope) -> Result<u64, CatalogueSourceError> {
        if scope != &self.scope {
            return Err(CatalogueSourceError::IdentityChanged);
        }
        let (reply, result) = mpsc::channel();
        self.enqueue(Command::Head(scope.clone(), reply))?;
        result
            .recv()
            .map_err(|_| CatalogueSourceError::Unavailable)?
    }

    fn manifest(
        &mut self,
        request: &ManifestRequest,
    ) -> Result<ManifestPage, CatalogueSourceError> {
        if request.pass.scope != self.scope {
            return Err(CatalogueSourceError::IdentityChanged);
        }
        let (reply, result) = mpsc::channel();
        self.enqueue(Command::Manifest(request.clone(), reply))?;
        result
            .recv()
            .map_err(|_| CatalogueSourceError::Unavailable)?
    }

    fn resolve(
        &mut self,
        pass: &CataloguePass,
        id: &Id,
        max_payload_bytes: usize,
    ) -> Result<ResolvedEntry, CatalogueSourceError> {
        if pass.scope != self.scope {
            return Err(CatalogueSourceError::IdentityChanged);
        }
        let (reply, result) = mpsc::channel();
        self.enqueue(Command::Resolve(
            pass.clone(),
            id.clone(),
            max_payload_bytes,
            reply,
        ))?;
        result
            .recv()
            .map_err(|_| CatalogueSourceError::Unavailable)?
    }
}

fn map_error(error: ConversationError) -> CatalogueSourceError {
    match error {
        ConversationError::CatalogueIdentityChanged => CatalogueSourceError::IdentityChanged,
        ConversationError::CatalogueInvalidRequest => CatalogueSourceError::InvalidRequest,
        _ => CatalogueSourceError::Unavailable,
    }
}

async fn read_head(
    catalogue: &dyn ConversationCatalogue,
    caller: &ConversationCaller,
    expected: &Scope,
    scope: &Scope,
) -> Result<u64, CatalogueSourceError> {
    if scope != expected {
        return Err(CatalogueSourceError::IdentityChanged);
    }
    let head = catalogue
        .head(&caller.organization_id, &caller.principal_id)
        .await
        .map_err(map_error)?;
    if head.incarnation != scope.incarnation().as_str() {
        return Err(CatalogueSourceError::IdentityChanged);
    }
    Ok(head.revision)
}

async fn read_manifest(
    catalogue: &dyn ConversationCatalogue,
    caller: &ConversationCaller,
    expected: &Scope,
    request: &ManifestRequest,
) -> Result<ManifestPage, CatalogueSourceError> {
    if request.pass.scope != *expected {
        return Err(CatalogueSourceError::IdentityChanged);
    }
    validate_manifest_request(request, MAX_CATALOGUE_ENTRIES)
        .map_err(|_| CatalogueSourceError::InvalidRequest)?;
    let page = catalogue
        .page(CataloguePageRequest {
            organization: caller.organization_id.clone(),
            owner: caller.principal_id.clone(),
            manifest: request.clone(),
        })
        .await
        .map_err(map_error)?;
    Ok(ManifestPage {
        request: request.clone(),
        entries: page.entries.into_iter().map(to_entry).collect(),
        has_more: page.has_more,
    })
}

async fn read_resolved(
    catalogue: &dyn ConversationCatalogue,
    caller: &ConversationCaller,
    expected: &Scope,
    pass: &CataloguePass,
    id: &Id,
    bound: usize,
) -> Result<ResolvedEntry, CatalogueSourceError> {
    if pass.scope != *expected {
        return Err(CatalogueSourceError::IdentityChanged);
    }
    validate_catalogue_pass(pass).map_err(|_| CatalogueSourceError::InvalidRequest)?;
    if bound == 0 || bound > MAX_CATALOGUE_PAYLOAD_BYTES {
        return Err(CatalogueSourceError::InvalidRequest);
    }
    let id = ConversationId::new(id.as_str()).map_err(|_| CatalogueSourceError::InvalidRequest)?;
    let value = catalogue
        .resolve(
            &caller.organization_id,
            &caller.principal_id,
            expected.incarnation().as_str(),
            &id,
        )
        .await
        .map_err(map_error)?
        .ok_or(CatalogueSourceError::Unavailable)?;
    if value.descriptor.key.creation > pass.boundary || value.descriptor.revision <= pass.completed
    {
        return Err(CatalogueSourceError::Unavailable);
    }
    let manifest = to_entry(value.descriptor.clone());
    let payload = if manifest.deleted {
        Vec::new()
    } else {
        payload(&value)?
    };
    if payload.len() > bound {
        return Err(CatalogueSourceError::OversizedEntry);
    }
    Ok(ResolvedEntry { manifest, payload })
}

fn to_entry(descriptor: CatalogueDescriptor) -> ManifestEntry {
    ManifestEntry {
        key: EntryKey {
            creation: descriptor.key.creation,
            id: Id::new(descriptor.key.id.to_string()).expect("UUID fits sync ID"),
        },
        revision: descriptor.revision,
        deleted: descriptor.deleted,
    }
}

/// The entry's payload: its conversation and summary as catalogue metadata,
/// in the one stored representation the device reads back.
fn payload(value: &CatalogueValue) -> Result<Vec<u8>, CatalogueSourceError> {
    let conversation = &value.conversation;
    let metadata = CatalogueMetadata::new(
        conversation.id().clone(),
        conversation.creation_requested_at_ms(),
        conversation.agent(),
        conversation.model().clone(),
        conversation.approval_mode(),
        value.summary.clone(),
    )
    .map_err(|_| CatalogueSourceError::Unavailable)?;
    catalogue_payload::encode(&value.descriptor.key.id.to_string(), &metadata)
        .map_err(|_| CatalogueSourceError::Unavailable)
}

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_source.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_receiver.rs"]
mod receiver_tests;

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_source/discovery.rs"]
mod discovery_tests;
