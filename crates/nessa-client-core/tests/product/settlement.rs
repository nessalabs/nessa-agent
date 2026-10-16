//! Failure injection at the batch publication boundary, without a socket race.
use super::{
    lock, Owner, ProductCompletion, ProductFailure, ProductLimits, ProductTicket, State, Ticket,
    TicketState,
};
use crate::product::{ProductWake, ThreadProductWake};
use nessa_protocol::clock::Clock;
use nessa_protocol::protocol::ResponseFrame;
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Condvar, Mutex,
};
use std::time::Duration;

struct Time;
impl Clock for Time {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}
struct Wake {
    calls: AtomicUsize,
    inner: ThreadProductWake,
}
impl ProductWake for Wake {
    fn revision(&self) -> u64 {
        self.inner.revision()
    }
    fn wait(&self, revision: u64, timeout: Duration) {
        self.inner.wait(revision, timeout);
    }
    fn wake(&self) {
        assert_ne!(
            self.calls.fetch_add(1, Ordering::SeqCst),
            1,
            "completion wake"
        );
        self.inner.wake();
    }
}
fn owner(cause: Option<ProductFailure>) -> Arc<Owner> {
    Arc::new(Owner {
        state: Mutex::new(State {
            ready: None,
            failure: cause,
            closing: cause.is_some(),
            drained: false,
            interrupt: None,
            next_id: 1,
            permits: 0,
            queue: VecDeque::new(),
            active: None,
            events: VecDeque::new(),
            event_bytes: 0,
            event_count: 0,
        }),
        clock: Arc::new(Time),
        wake: Arc::new(Wake {
            calls: AtomicUsize::new(0),
            inner: ThreadProductWake::default(),
        }),
        limits: ProductLimits::default(),
    })
}
fn ticket(owner: &Arc<Owner>, id: u64, external: usize) -> ProductTicket {
    lock(&owner.state).permits += 1;
    ProductTicket {
        ticket: Arc::new(Ticket {
            id,
            text: String::new(),
            deadline: 100,
            owner: Arc::downgrade(owner),
            settled: Condvar::new(),
            state: Mutex::new(TicketState {
                external,
                completion: None,
            }),
        }),
    }
}
#[test]
fn completion_wake_failure_settles_the_whole_batch_with_the_effective_cause() {
    for dispatched in [false, true] {
        for observers in [false, true] {
            let owner = owner(None);
            let active = ticket(&owner, 1, 1);
            let queued = ticket(&owner, 2, 1);
            let abandoned = ticket(&owner, 3, 1);
            {
                let mut state = lock(&owner.state);
                state.active = Some((active.ticket.clone(), dispatched));
                state
                    .queue
                    .extend([queued.ticket.clone(), abandoned.ticket.clone()]);
            }
            drop(abandoned);
            let waits = observers.then(|| {
                let active = active.clone();
                let queued = queued.clone();
                std::thread::spawn(move || (active.wait(), queued.wait()))
            });
            owner.stop(); // First wake succeeds; completion wake is the fault.
            assert_eq!(lock(&owner.state).failure, Some(ProductFailure::Cancelled));
            owner.finish(ProductFailure::Cancelled);
            assert_eq!(
                lock(&owner.state).failure,
                Some(ProductFailure::WorkerPanicked)
            );
            let expected = if dispatched {
                ProductCompletion::Uncertain(ProductFailure::WorkerPanicked)
            } else {
                ProductCompletion::NotDispatched(ProductFailure::WorkerPanicked)
            };
            assert_eq!(active.wait().value(), &expected);
            assert_eq!(
                queued.wait().value(),
                &ProductCompletion::NotDispatched(ProductFailure::WorkerPanicked)
            );
            if let Some(waits) = waits {
                let (first, second) = waits.join().unwrap();
                assert_eq!(first.value(), &expected);
                assert_eq!(
                    second.value(),
                    &ProductCompletion::NotDispatched(ProductFailure::WorkerPanicked)
                );
            }
            assert_eq!(lock(&owner.state).permits, 2);
            drop(active);
            drop(queued);
            assert_eq!(lock(&owner.state).permits, 0);
        }
    }
}
#[test]
fn completion_wake_failure_preserves_a_decoded_failure_and_factual_response() {
    for cause in [
        ProductFailure::Cancelled,
        ProductFailure::Protocol,
        ProductFailure::Correlation,
    ] {
        let owner = owner((cause != ProductFailure::Cancelled).then_some(cause));
        let factual = ticket(&owner, 1, 1);
        let completion = ProductCompletion::Response(Box::new(
            ResponseFrame::success("1", &serde_json::json!({})).unwrap(),
        ));
        // A factual reply was committed before this close boundary.
        factual
            .ticket
            .settle(&mut lock(&factual.ticket.state), completion.clone());
        let queued = ticket(&owner, 2, 1);
        lock(&owner.state).queue.push_back(queued.ticket.clone());
        owner.stop();
        owner.finish(cause);
        let effective = if cause == ProductFailure::Cancelled {
            ProductFailure::WorkerPanicked
        } else {
            cause
        };
        assert_eq!(lock(&owner.state).failure, Some(effective));
        assert_eq!(
            queued.wait().value(),
            &ProductCompletion::NotDispatched(effective)
        );
        assert_eq!(factual.wait().value(), &completion);
        drop(factual);
        drop(queued);
        assert_eq!(lock(&owner.state).permits, 0);
    }
}

#[test]
fn observer_does_not_settle_a_batch_already_owned_by_a_finisher() {
    struct Wake {
        stopped: mpsc::Sender<()>,
        inner: ThreadProductWake,
    }
    impl ProductWake for Wake {
        fn revision(&self) -> u64 {
            panic!("waiter revision");
        }
        fn wait(&self, revision: u64, timeout: Duration) {
            self.inner.wait(revision, timeout);
        }
        fn wake(&self) {
            let _ = self.stopped.send(());
            self.inner.wake();
        }
    }
    let (stopped, seen) = mpsc::channel();
    let mut physical = owner(Some(ProductFailure::Cancelled));
    Arc::get_mut(&mut physical).unwrap().wake = Arc::new(Wake {
        stopped,
        inner: ThreadProductWake::default(),
    });
    let ticket = ticket(&physical, 1, 1);
    // The physical finisher was preempted after taking active/queue and
    // before locking its tickets. Its local batch still owns dispatch=true;
    // the shared active/queue are empty and cannot prove non-dispatch.
    let batch = (ticket.ticket.clone(), true);
    lock(&physical.state).drained = true;
    let observer = ticket.clone();
    let (settled, result) = mpsc::channel();
    let waiter = std::thread::spawn(move || settled.send(observer.wait()).unwrap());
    seen.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        result.recv_timeout(Duration::from_millis(50)).is_err(),
        "empty shared state does not transfer settlement authority to the observer"
    );
    assert!(ticket.completion().is_none());
    batch.0.complete(if batch.1 {
        ProductCompletion::Uncertain(ProductFailure::WorkerPanicked)
    } else {
        ProductCompletion::NotDispatched(ProductFailure::WorkerPanicked)
    });
    assert_eq!(
        result.recv_timeout(Duration::from_secs(2)).unwrap().value(),
        &ProductCompletion::Uncertain(ProductFailure::WorkerPanicked)
    );
    waiter.join().unwrap();
    drop(ticket);
    assert_eq!(lock(&physical.state).permits, 0);
}
