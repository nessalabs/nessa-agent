//! Tracked blocking reads and retained shutdown completion across source adapters.
//!
//! Record and catalogue infrastructure call this owner for admission, physical
//! worker tracking, unexpected faults and join-all drain. Each admitted closure
//! keeps its own source lease; this owner does not allocate socket capacity or
//! decide authorization. Attachment range reads can consume the same lifecycle.
//!
//! ```text
//! source adapter -> ReadWorkers::run -> owned OS thread + result
//! source shutdown -> retained drain -> all actual thread joins
//! ```
//! Arrows are calls. Cancelled observers leave the original physical owners live.

use std::panic;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::thread::{Builder, JoinHandle};
use tokio::sync::watch::Receiver;
use tokio::sync::{oneshot, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadWorkerError {
    Unavailable,
    WorkerPanicked,
}
pub struct ReadWorkers {
    state: Mutex<ReadWorkerState>,
}
struct ReadWorkerState {
    closed: bool,
    joins: Vec<JoinHandle<()>>,
    failure: Option<ReadWorkerError>,
    drain: Option<Receiver<Option<Result<(), ReadWorkerError>>>>,
}
impl ReadWorkerState {
    fn admit(&self) -> Result<(), ReadWorkerError> {
        if self.closed {
            Err(self.failure.unwrap_or(ReadWorkerError::Unavailable))
        } else {
            Ok(())
        }
    }
    fn reap_finished(&mut self) -> Result<(), ReadWorkerError> {
        let mut index = 0;
        while index < self.joins.len() {
            if self.joins[index].is_finished() {
                if self.joins.swap_remove(index).join().is_err() {
                    self.failure = Some(ReadWorkerError::WorkerPanicked);
                    self.closed = true;
                }
            } else {
                index += 1;
            }
        }
        self.admit()
    }
}
impl ReadWorkers {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ReadWorkerState {
                closed: false,
                joins: Vec::new(),
                failure: None,
                drain: None,
            }),
        })
    }
    pub fn admit(&self) -> Result<(), ReadWorkerError> {
        self.state.lock().unwrap().admit()
    }
    /// Whether shutdown has started or a worker fault has fenced new reads.
    /// Running work may read this between its own bounded steps to stop early.
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
    /// Preserve an inner source worker panic in the same lifecycle owner.
    pub fn worker_panicked(&self) {
        let mut state = self.state.lock().unwrap();
        state.failure = Some(ReadWorkerError::WorkerPanicked);
        state.closed = true;
    }
    pub async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        name: &str,
        read: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, ReadWorkerError> {
        let (answer, received) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            state.admit()?;
            state.reap_finished()?;
            let owner = self.clone();
            let join = Builder::new()
                .name(name.into())
                .spawn(move || {
                    // A cancelled caller cannot observe sender loss, and dropping
                    // an undeliverable result can also unwind. Keep both in the owner.
                    let result = panic::catch_unwind(AssertUnwindSafe(move || {
                        let _ = answer.send(read());
                    }));
                    if let Err(failure) = result {
                        owner.worker_panicked();
                        panic::resume_unwind(failure);
                    }
                })
                .map_err(|_| ReadWorkerError::Unavailable)?;
            state.joins.push(join);
        }
        received.await.map_err(|_| {
            // Sender loss observes the panic before the OS handle must be finished.
            self.worker_panicked();
            ReadWorkerError::WorkerPanicked
        })
    }
    pub async fn shutdown(self: &Arc<Self>) -> Result<(), ReadWorkerError> {
        let mut completion = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            match &state.drain {
                Some(completion) => completion.clone(),
                None => {
                    let joins = std::mem::take(&mut state.joins);
                    let (completed, completion) = watch::channel(None);
                    state.drain = Some(completion.clone());
                    let owner = self.clone();
                    tokio::task::spawn_blocking(move || {
                        let mut panicked = false;
                        for join in joins {
                            panicked |= join.join().is_err();
                        }
                        let result = {
                            let mut state = owner.state.lock().unwrap();
                            if panicked {
                                state.failure = Some(ReadWorkerError::WorkerPanicked);
                            }
                            state.failure.map_or(Ok(()), Err)
                        };
                        completed.send_replace(Some(result));
                    });
                    completion
                }
            }
        };
        loop {
            if let Some(result) = *completion.borrow_and_update() {
                return result;
            }
            completion
                .changed()
                .await
                .map_err(|_| ReadWorkerError::WorkerPanicked)?;
        }
    }
}

#[cfg(test)]
mod tests;
