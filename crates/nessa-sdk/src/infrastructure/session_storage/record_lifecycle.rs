//! Admission and full storage shutdown share one owner.

use super::creation::CONTROL_STREAM_PREFIX;
use crate::application::agent_execution::{
    commands::MAX_CREATION_OWNERS,
    sessions::{StorageError, StorageShutdownFailure},
};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
    thread,
};
use tokio::sync::{oneshot, Notify};

const MAX_READS: usize = 4;
type ReadTask = thread::JoinHandle<Result<(), StorageError>>;

#[derive(Default)]
pub(super) struct StorageOwner {
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    leases: HashSet<String>,
    reads: Vec<ReadTask>,
    read_failure: Option<StorageError>,
    initialized: bool,
    shutdown: Option<Arc<ShutdownCompletion>>,
}
#[derive(Default)]
pub(super) struct ShutdownCompletion {
    outcome: Mutex<Option<Result<(), StorageError>>>,
    ready: Notify,
}
pub(super) struct ShutdownWork {
    pub completion: Arc<ShutdownCompletion>,
    pub reads: Vec<ReadTask>,
    pub read_failure: Option<StorageError>,
    pub initialized: bool,
}
impl StorageOwner {
    pub fn reserve(&self, id: &str) -> Result<(), StorageError> {
        self.reserve_inner(id, false)
    }
    pub fn reserve_creation(&self, id: &str) -> Result<(), StorageError> {
        self.reserve_inner(id, true)
    }
    fn reserve_inner(&self, id: &str, creation: bool) -> Result<(), StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.shutdown.is_some() {
            return Err(StorageError::Closed);
        }
        if creation
            && state
                .leases
                .iter()
                .filter(|id| id.starts_with(CONTROL_STREAM_PREFIX))
                .count()
                >= MAX_CREATION_OWNERS
        {
            return Err(StorageError::Busy);
        }
        if !state.leases.insert(id.into()) {
            return Err(StorageError::Busy);
        }
        Ok(())
    }
    pub fn release(&self, id: &str) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .leases
            .remove(id);
    }
    pub fn initialize(&self) -> Result<(), StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.shutdown.is_some() {
            return Err(StorageError::Closed);
        }
        state.initialized = true;
        Ok(())
    }
    pub fn read<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<T, StorageError>>, StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.shutdown.is_some() {
            return Err(StorageError::Closed);
        }
        let mut index = 0;
        while index < state.reads.len() {
            if state.reads[index].is_finished() {
                let task = state.reads.swap_remove(index);
                if let Err(error) = join(task) {
                    state.read_failure.get_or_insert(error);
                }
            } else {
                index += 1;
            }
        }
        if state.reads.len() == MAX_READS {
            return Err(StorageError::ReadCapacity);
        }
        let (reply, receipt) = oneshot::channel();
        let task = thread::Builder::new()
            .name("nessa-committed-read".into())
            .spawn(move || {
                let result = job();
                let outcome = if matches!(result, Err(StorageError::ReadWorkerPanicked)) {
                    Err(StorageError::ReadWorkerPanicked)
                } else {
                    Ok(())
                };
                let _ = reply.send(result);
                outcome
            })
            .map_err(|error| StorageError::Io(error.to_string()))?;
        state.reads.push(task);
        Ok(receipt)
    }
    pub fn close(&self) -> Result<(Arc<ShutdownCompletion>, Option<ShutdownWork>), StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if let Some(completion) = &state.shutdown {
            return Ok((completion.clone(), None));
        }
        let completion = Arc::new(ShutdownCompletion::default());
        state.shutdown = Some(completion.clone());
        let work = ShutdownWork {
            completion: completion.clone(),
            reads: std::mem::take(&mut state.reads),
            read_failure: state.read_failure.take(),
            initialized: state.initialized,
        };
        Ok((completion, Some(work)))
    }
}
impl ShutdownCompletion {
    pub fn finish(&self, result: Result<(), StorageError>) {
        *self.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
        self.ready.notify_waiters();
    }
    pub async fn wait(&self) -> Result<(), StorageError> {
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if let Some(result) = self.outcome.lock().map_err(poisoned)?.clone() {
                return result;
            }
            ready.await;
        }
    }
}
pub(super) fn join(task: ReadTask) -> Result<(), StorageError> {
    task.join().map_err(|_| StorageError::ReadWorkerPanicked)?
}
pub(super) fn shutdown_result(
    read: Result<(), StorageError>,
    runtime: Result<(), StorageError>,
) -> Result<(), StorageError> {
    match (read, runtime) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(read), Err(runtime)) => match StorageShutdownFailure::new(
            read.bounded_diagnostic(StorageError::DIAGNOSTIC_BYTES / 2),
            runtime.bounded_diagnostic(StorageError::DIAGNOSTIC_BYTES / 2),
        ) {
            Ok(failure) => Err(StorageError::ShutdownFailures(Box::new(failure))),
            Err(error) => Err(error),
        },
    }
}
fn poisoned<T>(_: PoisonError<T>) -> StorageError {
    StorageError::Io("record admission lock poisoned".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn read_admission_remains_bounded_until_join() {
        let owner = StorageOwner::default();
        let mut gates = Vec::new();
        for _ in 0..MAX_READS {
            let (release, gate) = mpsc::channel();
            gates.push(release);
            // Dropping the async receipt cannot free the owned worker slot.
            drop(
                owner
                    .read(move || {
                        gate.recv().unwrap();
                        Ok(())
                    })
                    .unwrap(),
            );
        }
        assert!(matches!(
            owner.read(|| Ok(())),
            Err(StorageError::ReadCapacity)
        ));
        let (_, work) = owner.close().unwrap();
        let work = work.unwrap();
        assert_eq!(work.reads.len(), MAX_READS);
        for gate in gates {
            gate.send(()).unwrap();
        }
        for task in work.reads {
            join(task).unwrap();
        }
    }

    #[tokio::test]
    async fn shutdown_closes_all_admission_and_shares_completion() {
        let owner = StorageOwner::default();
        owner.initialize().unwrap();
        owner.reserve("held-writer").unwrap();
        let (first, work) = owner.close().unwrap();
        assert!(work.unwrap().initialized);
        assert_eq!(owner.initialize(), Err(StorageError::Closed));
        assert_eq!(owner.reserve("new-writer"), Err(StorageError::Closed));
        assert!(matches!(owner.read(|| Ok(())), Err(StorageError::Closed)));
        let (second, work) = owner.close().unwrap();
        assert!(work.is_none());
        assert!(Arc::ptr_eq(&first, &second));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), first.wait())
                .await
                .is_err()
        );
        // Cancellation of that first waiter leaves the shared task outcome owned.
        let error = StorageError::Unresolved;
        first.finish(Err(error.clone()));
        assert_eq!(second.wait().await, Err(error));
    }

    #[tokio::test]
    async fn cancelled_read_is_drained_before_shutdown_completes() {
        let owner = StorageOwner::default();
        let (release, gate) = mpsc::channel();
        drop(
            owner
                .read(move || {
                    gate.recv().unwrap();
                    Ok(())
                })
                .unwrap(),
        );
        let (completion, work) = owner.close().unwrap();
        let worker_completion = completion.clone();
        let worker = tokio::task::spawn_blocking(move || {
            for task in work.unwrap().reads {
                join(task).unwrap();
            }
            worker_completion.finish(Ok(()));
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(10), completion.wait())
                .await
                .is_err()
        );
        release.send(()).unwrap();
        worker.await.unwrap();
        completion.wait().await.unwrap();
    }

    #[test]
    fn shutdown_preserves_read_and_runtime_failures() {
        assert_eq!(
            shutdown_result(Err(StorageError::ReadWorkerPanicked), Ok(())),
            Err(StorageError::ReadWorkerPanicked)
        );
        assert_eq!(
            shutdown_result(Ok(()), Err(StorageError::Unresolved)),
            Err(StorageError::Unresolved)
        );
        assert_eq!(shutdown_result(Ok(()), Ok(())), Ok(()));
        let result = shutdown_result(
            Err(StorageError::ReadWorkerPanicked),
            Err(StorageError::Unresolved),
        );
        assert!(
            matches!(result, Err(StorageError::ShutdownFailures(failure)) if failure.read() == &StorageError::ReadWorkerPanicked && failure.runtime() == &StorageError::Unresolved)
        );
    }
}
