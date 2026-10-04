//! One bounded local source slot, using the shared tracked physical worker owner.
use super::{blob_path, corrupt, hold_directory, Files, LocalAttachmentStore};
use crate::{
    attachments::{
        application::{
            ArtifactBytes, ArtifactRange, ArtifactReadError, ArtifactState, AttachmentArtifacts,
            PortFuture,
        },
        domain::ArtifactId,
        infrastructure::hold_record::{HoldRecord, RecordState},
    },
    conversation::domain::ConversationId,
    core::read_workers::{ReadWorkerError, ReadWorkers},
};
use nessa_auth::domain::OrganizationId;
use nessa_local_storage::{open_beneath, sync_directory_beneath, OpenMode};
use nessa_sync::replication::artifacts::MAX_CHUNK_BYTES;
#[cfg(test)]
use std::sync::Mutex;
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::Arc,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
#[cfg(test)]
type ReadProbe = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

pub(super) struct SourceReads {
    pub(super) workers: Arc<ReadWorkers>,
    slot: Arc<Semaphore>,
    #[cfg(test)]
    pub(super) after_open: ReadProbe,
}
// The permit travels through both physical work and undeliverable result drop.
struct Completed<T> {
    value: T,
    _permit: OwnedSemaphorePermit,
}
impl SourceReads {
    pub(super) fn new() -> Self {
        Self {
            workers: ReadWorkers::new(),
            slot: Arc::new(Semaphore::new(1)),
            #[cfg(test)]
            after_open: Arc::new(Mutex::new(None)),
        }
    }
    async fn run<T: Send + 'static>(
        &self,
        files: Arc<Files>,
        work: impl FnOnce(&Files) -> Result<T, ArtifactReadError> + Send + 'static,
    ) -> Result<T, ArtifactReadError> {
        self.workers.admit().map_err(worker_error)?;
        let permit = self
            .slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| ArtifactReadError::Busy)?;
        let completed = self
            .workers
            .run("nessa-artifact-read", move || Completed {
                value: work(&files),
                _permit: permit,
            })
            .await
            .map_err(worker_error)?;
        completed.value
    }
}
fn worker_error(error: ReadWorkerError) -> ArtifactReadError {
    match error {
        ReadWorkerError::Unavailable => ArtifactReadError::Unavailable,
        ReadWorkerError::WorkerPanicked => ArtifactReadError::WorkerPanicked,
    }
}
fn storage_error(error: io::Error) -> ArtifactReadError {
    tracing::error!(%error, "artifact source storage failed");
    ArtifactReadError::Unavailable
}
impl Files {
    fn source_record(
        &self,
        organization: &OrganizationId,
        conversation: &ConversationId,
        id: &ArtifactId,
    ) -> Result<Option<HoldRecord>, ArtifactReadError> {
        let record = self
            .registration(organization, conversation, id)
            .map_err(storage_error)?;
        if record.is_some() {
            #[cfg(test)]
            {
                let probe = self.before_source_sync.lock().unwrap().take();
                if let Some(probe) = probe {
                    probe();
                }
            }
            #[cfg(test)]
            self.fail_publication_at(super::PublicationFault::BeforeManifestSync)
                .map_err(storage_error)?;
            sync_directory_beneath(&self.root, &hold_directory(organization, conversation))
                .map_err(storage_error)?;
        }
        Ok(record)
    }
}
impl AttachmentArtifacts for LocalAttachmentStore {
    fn manifest<'a>(
        &'a self,
        organization: &'a OrganizationId,
        conversation: &'a ConversationId,
        id: &'a ArtifactId,
    ) -> PortFuture<'a, Option<ArtifactState>, ArtifactReadError> {
        let (organization, conversation, id) =
            (organization.clone(), conversation.clone(), id.clone());
        Box::pin(self.source.run(self.files.clone(), move |files| {
            let _changes = files.lock().map_err(storage_error)?;
            let Some(record) = files.source_record(&organization, &conversation, &id)? else {
                return Ok(None);
            };
            match record.state {
                RecordState::Pending => Ok(None),
                RecordState::Retired { .. } => Ok(Some(ArtifactState::Deleted)),
                RecordState::Kept => {
                    if !files
                        .has_bytes(record.hold.stored())
                        .map_err(storage_error)?
                    {
                        return Err(storage_error(corrupt(
                            "registered attachment bytes are unavailable",
                        )));
                    }
                    Ok(Some(ArtifactState::Live(record.hold.stored().clone())))
                }
            }
        }))
    }
    fn chunk<'a>(
        &'a self,
        organization: &'a OrganizationId,
        conversation: &'a ConversationId,
        range: &'a ArtifactRange,
    ) -> PortFuture<'a, ArtifactBytes, ArtifactReadError> {
        #[cfg(test)]
        let after_open = self.source.after_open.clone();
        let (organization, conversation, range) =
            (organization.clone(), conversation.clone(), range.clone());
        Box::pin(self.source.run(self.files.clone(), move |files| {
            if range.revision() == 0
                || range.max_bytes() == 0
                || range.max_bytes() > MAX_CHUNK_BYTES
                || range.offset() >= range.stored().size()
            {
                return Err(ArtifactReadError::InvalidRequest);
            }
            let mut file = {
                let _changes = files.lock().map_err(storage_error)?;
                let record = files
                    .source_record(&organization, &conversation, range.id())?
                    .ok_or(ArtifactReadError::Missing)?;
                match record.state {
                    RecordState::Pending => return Err(ArtifactReadError::Missing),
                    RecordState::Retired { .. } => return Err(ArtifactReadError::Deleted),
                    RecordState::Kept => {}
                }
                if range.revision() != ArtifactState::Live(record.hold.stored().clone()).revision()
                    || record.hold.stored() != range.stored()
                {
                    return Err(ArtifactReadError::Changed);
                }
                let file = open_beneath(
                    &files.root,
                    &blob_path(range.stored().digest()),
                    OpenMode::Read,
                )
                .map_err(storage_error)?;
                if file.metadata().map_err(storage_error)?.len() != range.stored().size() {
                    return Err(storage_error(corrupt(
                        "registered attachment byte length changed",
                    )));
                }
                file
            };
            #[cfg(test)]
            {
                let probe = after_open.lock().unwrap().take();
                if let Some(probe) = probe {
                    probe();
                }
            }
            let length =
                (range.stored().size() - range.offset()).min(range.max_bytes() as u64) as usize;
            let mut bytes = vec![0; length];
            file.seek(SeekFrom::Start(range.offset()))
                .map_err(storage_error)?;
            file.read_exact(&mut bytes).map_err(storage_error)?;
            Ok(ArtifactBytes::new(bytes))
        }))
    }
    fn shutdown(&self) -> PortFuture<'_, (), ArtifactReadError> {
        Box::pin(async { self.source.workers.shutdown().await.map_err(worker_error) })
    }
}
