//! Instance admission and captured cleanup for physical persistence adapters.
//!
//! Each [`Worker`] owns an independent slot. Adapter state and typed storage
//! outcomes remain with its consumer. The ordering table is documented in
//! `docs/design/bounded-physical-persistence.md`; this capability does not own
//! revision, authorization or settlement decisions.
#![deny(missing_docs)]

use std::{
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};

/// One physical-operation admission slot, independent of other instances.
///
/// Construction needs no runtime. A caller must acquire admission before large
/// input clones, then synchronously submit its owned operation. Dropping the
/// async result wait does not release the physical job's slot.
pub struct Worker {
    slot: Arc<Semaphore>,
}

/// Physical execution could not be admitted or confirmed.
///
/// Consumers map this into their own conservative port outcome. It does not
/// prove absence, rollback, or that an effect did not land.
#[derive(Debug)]
pub struct Interrupted;

/// An acquired instance slot and the originating Tokio execution capability.
///
/// Dropping unused admission releases the empty slot. Submission consumes it,
/// preserving the slot through queued execution and owned capture destruction.
pub struct Admission {
    permit: OwnedSemaphorePermit,
    executor: Handle,
}

impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}

impl Worker {
    /// Construct an independent one-slot worker without acquiring runtime state.
    pub fn new() -> Self {
        Self {
            slot: Arc::new(Semaphore::new(1)),
        }
    }

    /// Capture the originating executor, then asynchronously acquire this slot.
    ///
    /// The first poll requires Tokio context. After it starts waiting, the Send
    /// future may resume on a plain thread (`queued_send_future_uses_origin`).
    /// Reliable progress requires the origin runtime to remain alive. Cancellation
    /// while waiting submits no job and owns no operation capture.
    ///
    /// # Errors
    /// Returns [`Interrupted`] if runtime context or admission is unavailable.
    pub async fn admit(&self) -> Result<Admission, Interrupted> {
        let executor = Handle::try_current().map_err(|_| Interrupted)?;
        let permit = self
            .slot
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Interrupted)?;
        Ok(Admission { permit, executor })
    }
}

impl Admission {
    /// Synchronously hand an owned operation and slot to the captured executor.
    ///
    /// The returned awaitable observes the result; dropping it, even before its
    /// first poll, leaves physical execution and capture cleanup owning the slot
    /// (`unpolled_result_drop_retains_physical_slot`). The operation should capture
    /// actual adapter state and owned inputs. Its ordinary call and capture Drop
    /// faults are contained separately. A Ready result becomes interruption if
    /// capture cleanup faults. Physical-frame double faults can still abort, and
    /// external panic hooks are not suppressed. Returned-output destruction after
    /// Job finishes is outside the slot lifetime.
    ///
    /// # Errors
    /// The awaitable returns [`Interrupted`] for a contained fault or failed join;
    /// the adapter must conservatively translate this without assuming rollback.
    pub fn submit<F, R>(self, operation: F) -> impl Future<Output = Result<R, Interrupted>> + Send
    where
        F: FnMut() -> R + Send + 'static,
        R: Send + 'static,
    {
        let handle = self.spawn(operation);
        async move {
            match handle.await {
                Ok(output) => output,
                Err(error) => {
                    if error.is_panic() {
                        std::mem::forget(error.into_panic());
                    }
                    Err(Interrupted)
                }
            }
        }
    }

    fn spawn<F, R>(self, operation: F) -> JoinHandle<Result<R, Interrupted>>
    where
        F: FnMut() -> R + Send + 'static,
        R: Send + 'static,
    {
        let job = Job {
            operation: Some(operation),
            _permit: self.permit,
        };
        self.executor.spawn_blocking(move || job.run())
    }
}

// Capture destruction precedes automatic permit-field destruction, including
// queued jobs that never start. Both boundaries have unit mutation witnesses.
struct Job<F> {
    operation: Option<F>,
    _permit: OwnedSemaphorePermit,
}

fn contained_drop<T>(value: T) -> bool {
    match catch_unwind(AssertUnwindSafe(|| drop(value))) {
        Ok(()) => true,
        Err(payload) => {
            std::mem::forget(payload);
            false
        }
    }
}

impl<F> Job<F> {
    fn drop_operation(&mut self) -> bool {
        contained_drop(self.operation.take())
    }

    fn run<R>(mut self) -> Result<R, Interrupted>
    where
        F: FnMut() -> R,
    {
        let called = catch_unwind(AssertUnwindSafe(|| {
            self.operation
                .as_mut()
                .expect("owned persistence operation")()
        }));
        let output = called.map_err(|payload| {
            std::mem::forget(payload);
            Interrupted
        });
        let cleaned = self.drop_operation();
        let result = if cleaned {
            output
        } else {
            if let Ok(output) = output {
                let _ = contained_drop(output);
            }
            Err(Interrupted)
        };
        drop(self);
        result
    }
}

impl<F> Drop for Job<F> {
    fn drop(&mut self) {
        let _ = self.drop_operation();
    }
}

#[cfg(test)]
mod tests;
