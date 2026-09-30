#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
use tokio::sync::{oneshot, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::conversation::infrastructure) enum ReadWorkerError {
    Unavailable,
    WorkerPanicked,
}
pub(in crate::conversation::infrastructure) struct ReadWorkers {
    pub(in crate::conversation::infrastructure) state: Mutex<ReadWorkerState>,
    #[cfg(test)]
    pub(in crate::conversation::infrastructure) joining: AtomicUsize,
}
pub(in crate::conversation::infrastructure) struct ReadWorkerState {
    pub(in crate::conversation::infrastructure) closed: bool,
    pub(in crate::conversation::infrastructure) joins: Vec<JoinHandle<()>>,
    failure: Option<ReadWorkerError>,
    pub(in crate::conversation::infrastructure) drain:
        Option<watch::Receiver<Option<Result<(), ReadWorkerError>>>>,
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
    pub(in crate::conversation::infrastructure) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ReadWorkerState {
                closed: false,
                joins: Vec::new(),
                failure: None,
                drain: None,
            }),
            #[cfg(test)]
            joining: AtomicUsize::new(0),
        })
    }
    pub(in crate::conversation::infrastructure) fn admit(&self) -> Result<(), ReadWorkerError> {
        self.state.lock().unwrap().admit()
    }
    /// Preserve an inner source worker panic in the same lifecycle owner.
    pub(in crate::conversation::infrastructure) fn worker_panicked(&self) {
        let mut state = self.state.lock().unwrap();
        state.failure = Some(ReadWorkerError::WorkerPanicked);
        state.closed = true;
    }
    pub(in crate::conversation::infrastructure) async fn run<T: Send + 'static>(
        &self,
        name: &str,
        read: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, ReadWorkerError> {
        let (answer, received) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            state.admit()?;
            state.reap_finished()?;
            let join = thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    let _ = answer.send(read());
                })
                .map_err(|_| ReadWorkerError::Unavailable)?;
            state.joins.push(join);
        }
        received.await.map_err(|_| ReadWorkerError::WorkerPanicked)
    }
    pub(in crate::conversation::infrastructure) async fn shutdown(
        self: &Arc<Self>,
    ) -> Result<(), ReadWorkerError> {
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
                            #[cfg(test)]
                            owner.joining.fetch_add(1, Ordering::SeqCst);
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
