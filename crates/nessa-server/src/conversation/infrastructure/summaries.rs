use crate::conversation::{
    application::{ConversationError, ConversationFuture, ConversationSummaries},
    domain::{ConversationId, ConversationPreview, ConversationSummary, ConversationTitle},
};
use nessa_local_storage::{self as storage, OpenMode, PrivateTempFile};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Largest summary file read back. A title and a preview at their bounds,
/// with every character JSON escapes, fit well inside it.
const MAX_SUMMARY_BYTES: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSummary {
    id: String,
    title: Option<String>,
    preview: Option<String>,
    updated_at_ms: u64,
    archived: bool,
}

/// One private file per conversation, `<id>.json`, replaced whole on every
/// change, and removed when the conversation is deleted.
///
/// Unlike an ownership record this is rewritten, so it is published by
/// replacing the previous file in one move: a reader sees the old summary or
/// the new one, never part of either.
pub struct LocalConversationSummaries {
    root: PathBuf,
    writes: Arc<Mutex<()>>,
}
impl LocalConversationSummaries {
    /// The composition-selected directory must be private and owned by this OS
    /// user. Taking it releases temporaries an interrupted write left behind.
    pub fn new(root: PathBuf) -> Result<Self, ConversationError> {
        storage::create_directory(&root).map_err(|_| ConversationError::Metadata)?;
        PrivateTempFile::clear_stale(&root).map_err(|error| {
            tracing::error!(
                directory = %root.display(),
                %error,
                "conversation summary temporaries could not be released"
            );
            ConversationError::Metadata
        })?;
        Ok(Self {
            root,
            writes: Arc::new(Mutex::new(())),
        })
    }
}

fn read(
    root: &Path,
    id: &ConversationId,
) -> Result<Option<ConversationSummary>, ConversationError> {
    let path = root.join(format!("{id}.json"));
    let file = match storage::open(&path, OpenMode::Read) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ConversationError::Metadata),
    };
    let mut bytes = Vec::new();
    file.take(MAX_SUMMARY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ConversationError::Metadata)?;
    if bytes.len() > MAX_SUMMARY_BYTES {
        return Err(ConversationError::Metadata);
    }
    let stored: StoredSummary =
        serde_json::from_slice(&bytes).map_err(|_| ConversationError::Metadata)?;
    if ConversationId::new(&stored.id).ok().as_ref() != Some(id) {
        return Err(ConversationError::Metadata);
    }
    let title = stored
        .title
        .map(|title| ConversationTitle::new(&title))
        .transpose()
        .map_err(|_| ConversationError::Metadata)?;
    let preview = stored
        .preview
        .map(|preview| ConversationPreview::new(&preview))
        .transpose()
        .map_err(|_| ConversationError::Metadata)?;
    // Held to the same bounds as one written here: a time a list cannot
    // carry makes this summary unreadable, which costs its row its summary,
    // never the list.
    ConversationSummary::new(title, preview, stored.updated_at_ms, stored.archived)
        .map(Some)
        .map_err(|_| ConversationError::Metadata)
}

fn write(
    root: &Path,
    id: &ConversationId,
    summary: &ConversationSummary,
) -> Result<(), ConversationError> {
    let bytes = serde_json::to_vec(&StoredSummary {
        id: id.to_string(),
        title: summary.title().map(|title| title.as_str().to_owned()),
        preview: summary.preview().map(|preview| preview.as_str().to_owned()),
        updated_at_ms: summary.updated_at_ms(),
        archived: summary.archived(),
    })
    .map_err(|_| ConversationError::Metadata)?;
    if bytes.len() > MAX_SUMMARY_BYTES {
        return Err(ConversationError::Metadata);
    }
    let mut file = PrivateTempFile::new_in(root).map_err(|_| ConversationError::Metadata)?;
    file.as_file_mut()
        .write_all(&bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| ConversationError::Metadata)?;
    file.persist(&root.join(format!("{id}.json")))
        .and_then(|()| storage::sync_directory(root))
        .map_err(|error| {
            tracing::error!(%error, "could not publish a conversation summary");
            ConversationError::Metadata
        })
}

fn erase(root: &Path, id: &ConversationId) -> Result<(), ConversationError> {
    storage::verify_directory(root).map_err(|_| ConversationError::Metadata)?;
    match std::fs::remove_file(root.join(format!("{id}.json"))) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::error!(%error, "could not remove a conversation summary");
            return Err(ConversationError::Metadata);
        }
    }
    storage::sync_directory(root).map_err(|_| ConversationError::Metadata)
}

impl ConversationSummaries for LocalConversationSummaries {
    fn load(&self, id: &ConversationId) -> ConversationFuture<'_, Option<ConversationSummary>> {
        let root = self.root.clone();
        let id = id.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
                read(&root, &id)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
    fn record(
        &self,
        id: &ConversationId,
        summary: ConversationSummary,
    ) -> ConversationFuture<'_, ()> {
        let root = self.root.clone();
        let id = id.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
                write(&root, &id, &summary)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
    fn erase(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        let root = self.root.clone();
        let id = id.clone();
        let writes = self.writes.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let _guard = writes.lock().map_err(|_| ConversationError::Metadata)?;
                erase(&root, &id)
            })
            .await
            .map_err(|_| ConversationError::Metadata)?
        })
    }
}
