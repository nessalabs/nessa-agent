//! Bounded synchronous sync-engine reads over the authenticated metadata port.
//! #260 will supply the network boundary; this adapter accepts only a scope and
//! caller already chosen by the host, and never opens a writer or provider.

use crate::conversation::{
    application::{
        CatalogueDescriptor, CatalogueKey, CataloguePageRequest, CatalogueValue,
        ConversationCaller, ConversationCatalogue, ConversationError,
    },
    domain::ConversationId,
};
use nessa_sync::replication::{
    catalogue::{
        CataloguePass, CatalogueSource, CatalogueSourceError, EntryKey, ManifestEntry,
        ManifestPage, ManifestRequest, ResolvedEntry, MAX_CATALOGUE_ENTRIES,
        MAX_CATALOGUE_PAYLOAD_BYTES,
    },
    domain::{Id, Scope},
};
use sha2::{Digest, Sha256};
use std::{
    sync::{mpsc, Arc, Mutex},
    thread,
};
use tokio::runtime::Handle;

const SCHEMA: &str = "nessa.conversation-catalogue.v1";
const QUEUE_CAPACITY: usize = 32;

pub fn conversation_catalogue_schema() -> Id {
    Id::new(SCHEMA).expect("fixed schema ID")
}

/// Stable stream identity for one authenticated organization and principal.
/// The length prefix prevents two distinct owner pairs from hashing the same
/// byte sequence; the digest keeps the sync ID within its fixed size bound.
pub fn conversation_catalogue_stream(caller: &ConversationCaller) -> Id {
    let organization = caller.organization_id.as_str().as_bytes();
    let owner = caller.principal_id.as_str().as_bytes();
    let mut hash = Sha256::new();
    hash.update((organization.len() as u64).to_be_bytes());
    hash.update(organization);
    hash.update(owner);
    Id::new(format!("conversation-owner:{:x}", hash.finalize())).expect("digest fits sync ID")
}

enum Command {
    Head(Scope, mpsc::Sender<Result<u64, CatalogueSourceError>>),
    Manifest(
        ManifestRequest,
        mpsc::Sender<Result<ManifestPage, CatalogueSourceError>>,
    ),
    Resolve(
        CataloguePass,
        Id,
        usize,
        mpsc::Sender<Result<ResolvedEntry, CatalogueSourceError>>,
    ),
}

struct Worker {
    sender: mpsc::SyncSender<Command>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the sender drains admitted reads. Never join on a Tokio
        // scheduler thread: the active read may need that very scheduler.
        let (replacement, receiver) = mpsc::sync_channel(0);
        drop(receiver);
        let sender = std::mem::replace(&mut self.sender, replacement);
        drop(sender);
        if Handle::try_current().is_err() {
            if let Some(thread) = self.thread.get_mut().unwrap().take() {
                let _ = thread.join();
            }
        }
    }
}

/// Cloneable source bound to one authenticated caller and one exact sync scope.
/// Call the synchronous trait on a blocking thread. A bounded worker queue and
/// source-owned Tokio runtime form the execution boundary: metadata reads can
/// use `spawn_blocking` even when the caller occupies its only blocking slot.
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
    ) -> Result<Self, CatalogueSourceError> {
        Self::start_with_spawn(
            catalogue,
            caller,
            scope,
            |run| {
                thread::Builder::new()
                    .name("nessa-catalogue-source".into())
                    .spawn(run)
            },
            || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .max_blocking_threads(1)
                    .build()
            },
        )
    }

    fn start_with_spawn(
        catalogue: Arc<dyn ConversationCatalogue>,
        caller: ConversationCaller,
        scope: Scope,
        spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<thread::JoinHandle<()>>,
        make_runtime: impl FnOnce() -> std::io::Result<tokio::runtime::Runtime> + Send + 'static,
    ) -> Result<Self, CatalogueSourceError> {
        if scope.schema() != &conversation_catalogue_schema()
            || scope.stream() != &conversation_catalogue_stream(&caller)
        {
            return Err(CatalogueSourceError::IdentityChanged);
        }
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready, initialized) = mpsc::sync_channel(1);
        let expected = scope.clone();
        let worker = spawn(Box::new(move || {
            let runtime = match make_runtime() {
                Ok(runtime) => runtime,
                Err(_) => {
                    let _ = ready.send(Err(CatalogueSourceError::Unavailable));
                    return;
                }
            };
            if ready.send(Ok(())).is_err() {
                return;
            }
            while let Ok(command) = receiver.recv() {
                match command {
                    Command::Head(scope, reply) => {
                        let result =
                            runtime.block_on(read_head(&*catalogue, &caller, &expected, &scope));
                        let _ = reply.send(result);
                    }
                    Command::Manifest(request, reply) => {
                        let result = runtime.block_on(read_manifest(
                            &*catalogue,
                            &caller,
                            &expected,
                            &request,
                        ));
                        let _ = reply.send(result);
                    }
                    Command::Resolve(pass, id, bound, reply) => {
                        let result = runtime.block_on(read_resolved(
                            &*catalogue,
                            &caller,
                            &expected,
                            &pass,
                            &id,
                            bound,
                        ));
                        let _ = reply.send(result);
                    }
                }
            }
        }))
        .map_err(|_| CatalogueSourceError::Unavailable)?;
        initialized
            .recv()
            .map_err(|_| CatalogueSourceError::Unavailable)??;
        Ok(Self {
            scope,
            worker: Arc::new(Worker {
                sender,
                thread: Mutex::new(Some(worker)),
            }),
        })
    }

    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    fn enqueue(&self, command: Command) -> Result<(), CatalogueSourceError> {
        self.worker
            .sender
            .try_send(command)
            .map_err(|_| CatalogueSourceError::Unavailable)
    }
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
    if request.max_entries == 0
        || request.max_entries > MAX_CATALOGUE_ENTRIES
        || request.pass.boundary <= request.pass.completed
        || request.pass.generation == 0
    {
        return Err(CatalogueSourceError::InvalidRequest);
    }
    let after = request
        .pass
        .cursor
        .as_ref()
        .map(|key| {
            Ok(CatalogueKey {
                creation: key.creation,
                id: ConversationId::new(key.id.as_str())
                    .map_err(|_| CatalogueSourceError::InvalidRequest)?,
            })
        })
        .transpose()?;
    let page = catalogue
        .page(CataloguePageRequest {
            organization: caller.organization_id.clone(),
            owner: caller.principal_id.clone(),
            incarnation: expected.incarnation().as_str().to_owned(),
            completed: request.pass.completed,
            boundary: request.pass.boundary,
            after,
            limit: request.max_entries,
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

fn payload(value: &CatalogueValue) -> Result<Vec<u8>, CatalogueSourceError> {
    let conversation = &value.conversation;
    let summary = value.summary.as_ref();
    serde_json::to_vec(&serde_json::json!({
        "id": conversation.id().to_string(),
        "createdAtMs": conversation.creation_requested_at_ms(),
        "agent": conversation.agent().map(|id| id.name()),
        "model": conversation.model().as_str(),
        "approvalMode": conversation.approval_mode().as_str(),
        "summary": summary.map(|summary| serde_json::json!({
            "title": summary.title().map(|title| title.as_str()),
            "preview": summary.preview().map(|preview| preview.as_str()),
            "updatedAtMs": summary.updated_at_ms(),
            "archived": summary.archived(),
        })),
    }))
    .map_err(|_| CatalogueSourceError::Unavailable)
}

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_source.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests/conversation/catalogue_receiver.rs"]
mod receiver_tests;
