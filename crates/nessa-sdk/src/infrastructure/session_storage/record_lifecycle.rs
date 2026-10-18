//! Admission and full storage shutdown share one owner.

use crate::application::agent_execution::caller_wake::contain_caller_wake;
use crate::application::agent_execution::sessions::{StorageError, StorageShutdownFailure};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
    thread,
};
use tokio::sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore};

/// Physical read workers at once; each runs one bounded read step. Further
/// reads wait for one to finish rather than being refused.
const MAX_READS: usize = 4;
type ReadTask = thread::JoinHandle<Result<(), StorageError>>;

#[derive(Default)]
pub(super) struct StorageOwner {
    state: Mutex<State>,
    slots: ReadSlots,
}
/// The read threads' places. Closed at shutdown, which wakes every waiting read.
struct ReadSlots(Arc<Semaphore>);
impl Default for ReadSlots {
    fn default() -> Self {
        Self(Arc::new(Semaphore::new(MAX_READS)))
    }
}
#[derive(Default)]
struct State {
    leases: HashSet<String>,
    // Retained event readers. Not bounded here: each belongs to a follower
    // its host already bounds, so the total grows with the followers.
    event_readers: usize,
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
pub(super) struct EventReadOwner(Arc<StorageOwner>);
impl Drop for EventReadOwner {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .event_readers -= 1;
    }
}
impl StorageOwner {
    pub(super) fn observe(self: &Arc<Self>) -> Result<EventReadOwner, StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.shutdown.is_some() {
            return Err(StorageError::Closed);
        }
        state.event_readers += 1;
        Ok(EventReadOwner(self.clone()))
    }
    /// How many event readers are retained now.
    #[cfg(test)]
    pub(super) fn event_readers(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .event_readers
    }

    pub fn reserve(&self, id: &str) -> Result<(), StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.shutdown.is_some() {
            return Err(StorageError::Closed);
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
    /// Run `job` on a read thread, waiting for a free one. Dropping the
    /// returned receipt does not stop the job; shutdown joins it.
    pub async fn read<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<T, StorageError>>, StorageError> {
        let slot = self.slot().await?;
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
        let (reply, receipt) = oneshot::channel();
        let task = thread::Builder::new()
            .name("nessa-committed-read".into())
            .spawn(move || {
                let _slot = slot;
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
    async fn slot(&self) -> Result<OwnedSemaphorePermit, StorageError> {
        if self.state.lock().map_err(poisoned)?.shutdown.is_some() {
            return Err(StorageError::Closed);
        }
        self.slots
            .0
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| StorageError::Closed)
    }
    pub fn close(&self) -> Result<(Arc<ShutdownCompletion>, Option<ShutdownWork>), StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if let Some(completion) = &state.shutdown {
            return Ok((completion.clone(), None));
        }
        self.slots.0.close();
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
        contain_caller_wake("record storage shutdown", async {
            loop {
                let ready = self.ready.notified();
                tokio::pin!(ready);
                ready.as_mut().enable();
                if let Some(result) = self.outcome.lock().map_err(poisoned)?.clone() {
                    return result;
                }
                ready.await;
            }
        })
        .await
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

    #[tokio::test]
    async fn a_read_past_the_running_ones_waits_for_a_thread_instead_of_failing() {
        let owner = Arc::new(StorageOwner::default());
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
                    .await
                    .unwrap(),
            );
        }
        let waiting = tokio::spawn({
            let owner = owner.clone();
            async move { owner.read(|| Ok(7)).await.unwrap().await.unwrap() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !waiting.is_finished(),
            "a fifth read runs only when a thread is free"
        );
        gates.pop().unwrap().send(()).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), waiting)
                .await
                .unwrap()
                .unwrap(),
            Ok(7)
        );
        // Fill the place the finished reads left, so the next one waits.
        let (release, gate) = mpsc::channel();
        gates.push(release);
        drop(
            owner
                .read(move || {
                    gate.recv().unwrap();
                    Ok(())
                })
                .await
                .unwrap(),
        );
        // A read still waiting when shutdown starts is refused, not left waiting.
        let late = tokio::spawn({
            let owner = owner.clone();
            async move { owner.read(|| Ok(())).await.map(|_| ()) }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (_, work) = owner.close().unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), late)
                .await
                .unwrap()
                .unwrap(),
            Err(StorageError::Closed)
        );
        for gate in gates {
            gate.send(()).unwrap();
        }
        for task in work.unwrap().reads {
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
        assert!(matches!(
            owner.read(|| Ok(())).await,
            Err(StorageError::Closed)
        ));
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
                .await
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
