//! Instance admission for physical persistence, including queued and canceled jobs.
#[cfg(test)]
use super::physical_tests::Gate;
#[cfg(test)]
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const PHYSICAL_SLOTS: usize = 1;

pub(super) struct Worker {
    slot: Arc<Semaphore>,
    #[cfg(test)]
    pub(super) probe: Arc<Probe>,
}

pub(super) struct Interrupted;

pub(super) struct Admission {
    permit: OwnedSemaphorePermit,
    executor: Handle,
}

// This wrapper names the input whose destruction stays inside Job's admission.
pub(super) struct Input<T> {
    value: T,
    #[cfg(test)]
    gate: Option<Arc<Gate>>,
    #[cfg(test)]
    fault: Option<Arc<Probe>>,
}
impl<T> Input<T> {
    pub(super) fn get(&self) -> &T {
        &self.value
    }
}
#[cfg(test)]
impl<T> Drop for Input<T> {
    fn drop(&mut self) {
        // The real value field is dropped after this destructor returns.
        if let Some(gate) = &self.gate {
            gate.enter();
        }
        if let Some(probe) = &self.fault {
            std::panic::panic_any(Payload(probe.clone()));
        }
    }
}

impl Worker {
    pub(super) fn new() -> Self {
        Self {
            slot: Arc::new(Semaphore::new(PHYSICAL_SLOTS)),
            #[cfg(test)]
            probe: Arc::new(Probe::default()),
        }
    }

    pub(super) async fn admit(&self) -> Result<Admission, Interrupted> {
        // Capture capability before waiting: a Send port future can resume on
        // an ordinary thread (queued_send_future_resumes_on_plain_thread).
        let executor = Handle::try_current().map_err(|_| Interrupted)?;
        let permit = self
            .slot
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| Interrupted)?;
        Ok(Admission { permit, executor })
    }

    pub(super) fn input<T>(&self, value: T) -> Input<T> {
        #[cfg(test)]
        self.probe.inputs.fetch_add(1, Ordering::SeqCst);
        Input {
            value,
            #[cfg(test)]
            gate: self.probe.input_drop.lock().unwrap().take(),
            #[cfg(test)]
            fault: self
                .probe
                .panic_input_drop
                .swap(false, Ordering::SeqCst)
                .then(|| self.probe.clone()),
        }
    }

    pub(super) async fn run<F, R>(
        &self,
        admission: Admission,
        operation: F,
    ) -> Result<R, Interrupted>
    where
        F: FnMut() -> R + Send + 'static,
        R: Send + 'static,
    {
        #[cfg(test)]
        self.probe.submitted.fetch_add(1, Ordering::SeqCst);
        let job = Job {
            operation: Some(operation),
            #[cfg(test)]
            completion: Completion(self.probe.clone()),
            _permit: admission.permit,
        };
        let handle = admission.executor.spawn_blocking(move || job.run());
        #[cfg(test)]
        if self.probe.abort_queued.swap(false, Ordering::SeqCst) {
            handle.abort();
        }
        match handle.await {
            Ok(output) => output,
            Err(error) => {
                if error.is_panic() {
                    // Payloads can contain secrets or have panicking destructors.
                    // As in SDK lifecycle supervision, do not inspect or drop them.
                    std::mem::forget(error.into_panic());
                }
                Err(Interrupted)
            }
        }
    }

    #[cfg(test)]
    pub(super) fn close(&self) {
        self.slot.close();
    }
}

// Job::Drop and drop_operation destroy the capture while admission is owned,
// including a queued closure that never starts. The actual adapter input-drop
// and queued-abort tests exercise this physical cleanup boundary.
struct Job<F> {
    operation: Option<F>,
    #[cfg(test)]
    completion: Completion,
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
        // Keep the capture owned by Job while the call unwinds; drop it only
        // after catching the call fault, so capture Drop is a separate boundary.
        let called = catch_unwind(AssertUnwindSafe(|| self.call_operation()));
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

    fn call_operation<R>(&mut self) -> R
    where
        F: FnMut() -> R,
    {
        #[cfg(test)]
        {
            let probe = &self.completion.0;
            probe.started.fetch_add(1, Ordering::SeqCst);
            let gate = probe.before.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.enter();
            }
            if probe.payload_before.swap(false, Ordering::SeqCst) {
                std::panic::panic_any(Payload(self.completion.0.clone()));
            }
            assert!(
                !probe.panic_before.swap(false, Ordering::SeqCst),
                "physical worker pre-effect fault"
            );
        }
        let output = self
            .operation
            .as_mut()
            .expect("owned persistence operation")();
        #[cfg(test)]
        {
            let probe = &self.completion.0;
            let gate = probe.after.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.enter();
            }
            assert!(
                !probe.panic_after.swap(false, Ordering::SeqCst),
                "physical worker post-effect fault"
            );
        }
        output
    }
}

impl<F> Drop for Job<F> {
    fn drop(&mut self) {
        let _ = self.drop_operation();
        // Automatic field destruction releases admission after this body.
    }
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct Probe {
    pub(super) submitted: AtomicUsize,
    pub(super) inputs: AtomicUsize,
    pub(super) inputs_at_release: AtomicUsize,
    pub(super) started: AtomicUsize,
    pub(super) cleaned: AtomicUsize,
    pub(super) before: Mutex<Option<Arc<Gate>>>,
    pub(super) after: Mutex<Option<Arc<Gate>>>,
    pub(super) input_drop: Mutex<Option<Arc<Gate>>>,
    pub(super) abort_queued: AtomicBool,
    pub(super) payload_before: AtomicBool,
    pub(super) payload_dropped: AtomicUsize,
    pub(super) panic_input_drop: AtomicBool,
    pub(super) panic_before: AtomicBool,
    pub(super) panic_after: AtomicBool,
}
#[cfg(test)]
struct Completion(Arc<Probe>);
#[cfg(test)]
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.cleaned.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
struct Payload(Arc<Probe>);
#[cfg(test)]
impl Drop for Payload {
    fn drop(&mut self) {
        self.0.payload_dropped.fetch_add(1, Ordering::SeqCst);
        panic!("faulting panic payload destructor");
    }
}
