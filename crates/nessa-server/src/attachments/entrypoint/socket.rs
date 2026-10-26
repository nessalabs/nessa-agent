//! `attachment.write`: an upload's bytes in pieces over the product session,
//! for a surface that cannot reach `PUT /attachments`. A phone over its native
//! link is one: the link carries product messages and nothing else.
//!
//! ```text
//! piece at 0     -> SocketUploads (new entry) -> spawn AttachmentService::receive(PieceBody)
//! each piece     -> SocketUploads (entry) -> PieceBody channel -> receive reads it
//! piece `last`   -> close the channel -> receive's answer -> the stored file
//! ```
//!
//! An arrow is a call or a message. As on the HTTP route, the ticket is the
//! whole authority and this module learns nothing else: the service decides
//! what the ticket admits, how long the transfer may take, and what is kept.
//! An entry is keyed by a hash of the ticket, so the secret is not kept here.
//! A refused piece ends its upload, and so does a piece out of order: what
//! the service then answers is no longer the caller's to hear.
use crate::attachments::{
    application::{
        AttachmentService, PortFuture, TicketSecret, UploadBody, UploadError, UploadInterrupted,
    },
    domain::Attachment,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};

/// The most bytes one piece carries; its base64 fits the session's frame.
pub const MAX_PIECE_BYTES: usize = 32 * 1024;
/// Uploads in progress over sessions at once, across the gateway. Each holds
/// one task and one piece; the service's own upload limit is the tighter one.
pub const MAX_SOCKET_UPLOADS: usize = 256;
/// How long a finished upload's answer waits for the piece that asks for it,
/// when the caller went quiet before sending its last piece.
const ANSWER_KEPT: Duration = Duration::from_secs(30);

/// What a piece was answered with.
#[derive(Debug)]
pub enum PieceOutcome {
    /// Taken; the next piece starts here.
    Taken { received: u64 },
    /// The last piece, and the file the conversation now keeps.
    Stored(Attachment),
}

/// Why a piece was refused. Every one of these ends the upload.
#[derive(Debug)]
pub enum PieceError {
    /// The ticket is not one this gateway could have issued.
    TicketInvalid,
    /// Not where the upload stands: a piece after a gap, a repeat, or a
    /// later piece of an upload this gateway is not receiving.
    OutOfOrder,
    /// Too many uploads in progress over sessions; nothing was started.
    Busy,
    /// The service's answer for the upload.
    Upload(UploadError),
}

type Key = [u8; 32];

/// Uploads in progress over product sessions, gateway-wide, so a piece is
/// taken whichever connection it arrives on.
#[derive(Clone, Default)]
pub struct SocketUploads {
    entries: Arc<StdMutex<HashMap<Key, Arc<Upload>>>>,
}

struct Upload {
    progress: Mutex<Progress>,
}
struct Progress {
    received: u64,
    /// Closed by the last piece, or by a piece out of order.
    pieces: Option<mpsc::Sender<Vec<u8>>>,
    /// The service's answer, taken once.
    answer: Option<oneshot::Receiver<Result<Attachment, UploadError>>>,
}

impl SocketUploads {
    /// Take one piece of the upload `ticket` admits.
    pub async fn write(
        &self,
        service: &AttachmentService,
        ticket: &str,
        offset: u64,
        bytes: Vec<u8>,
        last: bool,
    ) -> Result<PieceOutcome, PieceError> {
        if TicketSecret::parse(ticket).is_none() {
            return Err(PieceError::TicketInvalid);
        }
        let key: Key = Sha256::digest(ticket.as_bytes()).into();
        let upload = self.entry(service, ticket, key, offset)?;
        let mut progress = upload.progress.lock().await;
        if offset != progress.received || progress.pieces.is_none() {
            progress.pieces = None;
            self.remove(&key, &upload);
            return Err(PieceError::OutOfOrder);
        }
        let length = bytes.len() as u64;
        if !bytes.is_empty() {
            let sent = match progress.pieces.as_ref() {
                Some(pieces) => pieces.send(bytes).await.is_ok(),
                None => false,
            };
            if !sent {
                // The service stopped reading: it has its answer already.
                return Err(self.finish(&key, &upload, &mut progress).await);
            }
        }
        progress.received += length;
        if last {
            progress.pieces = None;
            return match self.answer(&key, &upload, &mut progress).await {
                Ok(stored) => Ok(PieceOutcome::Stored(stored)),
                Err(error) => Err(error),
            };
        }
        // A refusal the service reached while this piece waited is told now,
        // not at the next piece.
        if let Some(answer) = progress.answer.as_mut() {
            if let Ok(outcome) = answer.try_recv() {
                progress.answer = None;
                progress.pieces = None;
                self.remove(&key, &upload);
                // The service keeps nothing before the pieces close, so an
                // answer here is a refusal.
                return Err(match outcome {
                    Err(error) => PieceError::Upload(error),
                    Ok(_) => PieceError::OutOfOrder,
                });
            }
        }
        Ok(PieceOutcome::Taken {
            received: progress.received,
        })
    }

    /// The upload under `key`, started by a piece at offset 0.
    fn entry(
        &self,
        service: &AttachmentService,
        ticket: &str,
        key: Key,
        offset: u64,
    ) -> Result<Arc<Upload>, PieceError> {
        let mut entries = self.entries.lock().expect("socket uploads");
        if let Some(upload) = entries.get(&key) {
            return Ok(upload.clone());
        }
        if offset != 0 {
            return Err(PieceError::OutOfOrder);
        }
        if entries.len() >= MAX_SOCKET_UPLOADS {
            return Err(PieceError::Busy);
        }
        let (pieces, received) = mpsc::channel(1);
        let (answered, answer) = oneshot::channel();
        let upload = Arc::new(Upload {
            progress: Mutex::new(Progress {
                received: 0,
                pieces: Some(pieces),
                answer: Some(answer),
            }),
        });
        entries.insert(key, upload.clone());
        let service = service.clone();
        let ticket = ticket.to_owned();
        let uploads = self.clone();
        let owned = upload.clone();
        tokio::spawn(async move {
            let outcome = service
                .receive(&ticket, None, Box::new(PieceBody(received)))
                .await;
            drop(ticket);
            let _ = answered.send(outcome);
            // Nobody asked for the answer: the caller went quiet. Let it go.
            tokio::time::sleep(ANSWER_KEPT).await;
            uploads.remove(&key, &owned);
        });
        Ok(upload)
    }

    /// The service's answer, once the pieces are closed.
    async fn answer(
        &self,
        key: &Key,
        upload: &Arc<Upload>,
        progress: &mut Progress,
    ) -> Result<Attachment, PieceError> {
        self.remove(key, upload);
        let Some(answer) = progress.answer.take() else {
            return Err(PieceError::OutOfOrder);
        };
        match answer.await {
            Ok(Ok(stored)) => Ok(stored),
            Ok(Err(error)) => Err(PieceError::Upload(error)),
            // The work ended without an answer; the service accounts for it.
            Err(_) => Err(PieceError::OutOfOrder),
        }
    }

    async fn finish(&self, key: &Key, upload: &Arc<Upload>, progress: &mut Progress) -> PieceError {
        progress.pieces = None;
        match self.answer(key, upload, progress).await {
            Err(error) => error,
            Ok(_) => PieceError::OutOfOrder,
        }
    }

    /// Drop the entry under `key` if it is still `upload`, and not a later
    /// upload that reused the key.
    fn remove(&self, key: &Key, upload: &Arc<Upload>) {
        let mut entries = self.entries.lock().expect("socket uploads");
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, upload))
        {
            entries.remove(key);
        }
    }

    #[cfg(test)]
    pub(crate) fn in_progress(&self) -> usize {
        self.entries.lock().expect("socket uploads").len()
    }
}

/// Pieces as the service's chunk source. The channel closing is the end of
/// the transfer; a caller that goes quiet is the service's deadline to end.
struct PieceBody(mpsc::Receiver<Vec<u8>>);
impl UploadBody for PieceBody {
    fn next(&mut self) -> PortFuture<'_, Option<Vec<u8>>, UploadInterrupted> {
        Box::pin(async move { Ok(self.0.recv().await) })
    }
}
