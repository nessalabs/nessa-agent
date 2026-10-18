//! Admission and full storage shutdown share one owner.

use crate::application::agent_execution::caller_wake::contain_caller_wake;
use crate::application::agent_execution::sessions::{StorageError, StorageShutdownFailure};
use std::{
    collections::HashSet,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{mpsc, Arc, Mutex, PoisonError},
    thread,
};
use tokio::sync::{oneshot, Notify};

/// Physical read workers; each runs one bounded read step at a time. A read
/// past them waits in line for one rather than being refused: every follower
/// keeps at most one read in flight, so the line is as long as the followers
/// reading at that moment.
///
/// A step spends most of its time waiting on the store's one SQLite thread,
/// not on its own CPU, so the workers are many enough to keep that thread's
/// queue full while thousands of followers read: with four, a step waited on
/// three others' round trips before sending its own.
const MAX_READS: usize = 32;
type ReadTask = thread::JoinHandle<Result<(), StorageError>>;
type ReadJob = Box<dyn FnOnce() -> Result<(), StorageError> + Send>;

#[derive(Default)]
pub(super) struct StorageOwner {
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    leases: HashSet<String>,
    // Retained event readers. Not bounded here: each belongs to a follower
    // its host already bounds, so the total grows with the followers.
    event_readers: usize,
    /// The line of reads the workers take from, opened by the first read.
    line: Option<mpsc::Sender<ReadJob>>,
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
    pub fn read<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<T, StorageError>>, StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if state.shutdown.is_some() {
            return Err(StorageError::Closed);
        }
        if state.line.is_none() {
            let (line, jobs) = mpsc::channel::<ReadJob>();
            let jobs = Arc::new(Mutex::new(jobs));
            for _ in 0..MAX_READS {
                let jobs = jobs.clone();
                let worker = thread::Builder::new()
                    .name("nessa-committed-read".into())
                    .spawn(move || work(&jobs))
                    .map_err(|error| StorageError::Io(error.to_string()));
                match worker {
                    Ok(worker) => state.reads.push(worker),
                    // Workers already started still take the line.
                    Err(error) if state.reads.is_empty() => return Err(error),
                    Err(_) => break,
                }
            }
            state.line = Some(line);
        }
        let (reply, receipt) = oneshot::channel();
        let job: ReadJob = Box::new(move || {
            let result = job();
            let outcome = if matches!(result, Err(StorageError::ReadWorkerPanicked)) {
                Err(StorageError::ReadWorkerPanicked)
            } else {
                Ok(())
            };
            let _ = reply.send(result);
            outcome
        });
        state
            .line
            .as_ref()
            .and_then(|line| line.send(job).ok())
            .ok_or(StorageError::ReadWorkerPanicked)?;
        Ok(receipt)
    }
    pub fn close(&self) -> Result<(Arc<ShutdownCompletion>, Option<ShutdownWork>), StorageError> {
        let mut state = self.state.lock().map_err(poisoned)?;
        if let Some(completion) = &state.shutdown {
            return Ok((completion.clone(), None));
        }
        let completion = Arc::new(ShutdownCompletion::default());
        state.shutdown = Some(completion.clone());
        // Closing the line lets each worker finish what is already in it, then
        // stop; shutdown joins them.
        state.line = None;
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
/// One worker: runs reads from the line until it closes, and answers with the
/// first failure it saw. A read that panics drops its reply, which its caller
/// sees as a panicked worker; the worker goes on to the next.
fn work(jobs: &Mutex<mpsc::Receiver<ReadJob>>) -> Result<(), StorageError> {
    let mut failure = None;
    loop {
        let job = match jobs.lock().unwrap_or_else(PoisonError::into_inner).recv() {
            Ok(job) => job,
            Err(mpsc::RecvError) => return failure.map_or(Ok(()), Err),
        };
        let outcome =
            catch_unwind(AssertUnwindSafe(job)).unwrap_or(Err(StorageError::ReadWorkerPanicked));
        if let Err(error) = outcome {
            failure.get_or_insert(error);
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
    fn a_read_past_the_workers_waits_in_line_and_still_runs() {
        let owner = StorageOwner::default();
        let mut gates = Vec::new();
        for _ in 0..MAX_READS {
            let (release, gate) = mpsc::channel();
            gates.push(release);
            // Dropping the async receipt cannot free the owned worker.
            drop(
                owner
                    .read(move || {
                        gate.recv().unwrap();
                        Ok(())
                    })
                    .unwrap(),
            );
        }
        // Every worker is held: the next read is not refused, it waits.
        let (ran, waited) = mpsc::channel();
        let mut queued = owner
            .read(move || {
                ran.send(()).unwrap();
                Ok(7)
            })
            .unwrap();
        assert!(waited.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(queued.try_recv().is_err());
        for gate in gates {
            gate.send(()).unwrap();
        }
        waited.recv_timeout(Duration::from_secs(10)).unwrap();
        let (_, work) = owner.close().unwrap();
        let work = work.unwrap();
        assert_eq!(work.reads.len(), MAX_READS);
        for task in work.reads {
            join(task).unwrap();
        }
        assert_eq!(queued.try_recv().unwrap(), Ok(7));
    }

    #[test]
    fn a_panicking_read_fails_its_caller_and_leaves_the_worker_for_the_next() {
        let owner = StorageOwner::default();
        let panicked = owner.read::<()>(|| panic!("a read step panicked")).unwrap();
        let next = owner.read(|| Ok(1)).unwrap();
        let (_, work) = owner.close().unwrap();
        let failures: Vec<_> = work.unwrap().reads.into_iter().map(join).collect();
        assert!(failures.contains(&Err(StorageError::ReadWorkerPanicked)));
        assert!(panicked.blocking_recv().is_err());
        assert_eq!(next.blocking_recv().unwrap(), Ok(1));
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
