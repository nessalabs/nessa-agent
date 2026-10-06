//! One loop for an audit record that outlives the command that caused it.
//!
//! A ticket's unredeemed end and a held context's drop are both that shape:
//! the command returns, the record is still owed, and no command's future —
//! cancellable, budgeted, or unwinding — holds it. Each owner keeps its own
//! channel, its own log line, and this loop.
//!
//! ```text
//!   already queued, stop also ready ──▶ write the record, then look again
//!   stop, and nothing is waiting    ──▶ return
//!   every sender is gone             ──▶ return
//!   the write fails                  ──▶ the owner's log, then the next record
//! ```
//!
//! The first row is `biased`: a record already sent is written before stop
//! is taken. The write's failure is the closure's to log; this loop only
//! waits for it and continues. Callers bound how long the drain may take.
use std::future::Future;
use tokio::sync::{mpsc::UnboundedReceiver, oneshot};

/// Write each record `incoming` receives, in the order they were sent, until
/// every sender is gone or `stop` says the process is stopping. When both are
/// ready, the record is written first.
pub(crate) async fn audit_records<T, F, Fut>(
    mut incoming: UnboundedReceiver<T>,
    mut stop: oneshot::Receiver<()>,
    mut write: F,
) where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = ()> + Send,
{
    loop {
        tokio::select! {
            biased;
            next = incoming.recv() => match next {
                Some(record) => write(record).await,
                None => return,
            },
            _ = &mut stop => return,
        }
    }
}
