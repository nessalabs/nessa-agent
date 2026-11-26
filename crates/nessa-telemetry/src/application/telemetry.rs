//! The one place events are emitted: [`Telemetry`] hands each to every sink
//! that is turned on and wants its shape.
//!
//! ```text
//! caller ──emit(event)──▶ bounded queue ──worker thread──▶ sink A (wants?) ─▶ emit
//!                              │ full: dropped, counted        └▶ sink B (wants?) ─▶ emit
//! ```
//!
//! Emitting never waits: the queue is bounded, and an event that finds it
//! full is dropped and counted (`a_full_queue_drops_and_counts`). With no
//! sink turned on there is no queue and no thread, and an emit does nothing
//! (`off_does_nothing`), so a caller can emit unconditionally. Telemetry is
//! lossy by design; anything that must not be lost is an audit record, which
//! never goes through here.
use super::Sink;
use crate::domain::Event;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

/// Events held while the worker catches up, by default.
pub const DEFAULT_CAPACITY: usize = 4096;

enum Work {
    Event(Box<Event>),
    /// Flush every sink, then say so.
    Flush(mpsc::Sender<()>),
}

struct Dispatcher {
    queue: SyncSender<Work>,
    dropped: AtomicU64,
    worker: Mutex<Option<JoinHandle<()>>>,
}

/// Emits events to the sinks that are turned on. Cheap to clone; every clone
/// shares one queue and one worker.
#[derive(Clone, Default)]
pub struct Telemetry {
    dispatcher: Option<Arc<Dispatcher>>,
}

impl std::fmt::Debug for Telemetry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Telemetry")
            .field("on", &self.is_on())
            .finish()
    }
}

impl Telemetry {
    /// Telemetry with no sink: every emit does nothing.
    pub fn off() -> Self {
        Self::default()
    }

    /// Send events to `sinks`, holding up to `capacity` (at least one) while
    /// the worker catches up. No sinks is [`Self::off`].
    ///
    /// # Errors
    /// The worker thread could not be started; the caller decides whether to
    /// run without telemetry.
    pub fn new(sinks: Vec<Box<dyn Sink>>, capacity: usize) -> std::io::Result<Self> {
        if sinks.is_empty() {
            return Ok(Self::off());
        }
        let (queue, work) = mpsc::sync_channel(capacity.max(1));
        let worker = std::thread::Builder::new()
            .name("nessa-telemetry".into())
            .spawn(move || run(work, sinks))?;
        Ok(Self {
            dispatcher: Some(Arc::new(Dispatcher {
                queue,
                dropped: AtomicU64::new(0),
                worker: Mutex::new(Some(worker)),
            })),
        })
    }

    /// Whether any sink is turned on. A caller with an event that is costly
    /// to build asks first.
    pub fn is_on(&self) -> bool {
        self.dispatcher.is_some()
    }

    /// Hand `event` to the sinks. Never waits: a full queue drops it.
    pub fn emit(&self, event: Event) {
        let Some(dispatcher) = &self.dispatcher else {
            return;
        };
        match dispatcher.queue.try_send(Work::Event(Box::new(event))) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                dispatcher.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Build an event and emit it, only when telemetry is on.
    pub fn emit_with(&self, event: impl FnOnce() -> Event) {
        if self.is_on() {
            self.emit(event());
        }
    }

    /// How many events were dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dispatcher
            .as_ref()
            .map_or(0, |dispatcher| dispatcher.dropped.load(Ordering::Relaxed))
    }

    /// Wait up to `wait` for every event emitted so far to reach its sinks
    /// and for each sink to flush. Whether it finished in time.
    pub fn flush(&self, wait: Duration) -> bool {
        let Some(dispatcher) = &self.dispatcher else {
            return true;
        };
        let (done, flushed) = mpsc::channel();
        // Waits for room: a flush is the caller asking to wait.
        if dispatcher.queue.send(Work::Flush(done)).is_err() {
            return false;
        }
        flushed.recv_timeout(wait).is_ok()
    }

    /// Flush (see [`Self::flush`]), then stop the worker once every clone is
    /// gone. Further emits are dropped.
    pub fn shutdown(&self, wait: Duration) -> bool {
        let flushed = self.flush(wait);
        if let Some(dispatcher) = &self.dispatcher {
            // The worker ends when the last queue sender is dropped; taking
            // the handle here only stops a second shutdown from waiting.
            let _ = dispatcher.worker.lock().map(|mut worker| worker.take());
        }
        flushed
    }
}

fn run(work: Receiver<Work>, sinks: Vec<Box<dyn Sink>>) {
    for item in work {
        match item {
            Work::Event(event) => {
                for sink in &sinks {
                    if sink.wants(event.shape()) {
                        let _ = catch_unwind(AssertUnwindSafe(|| sink.emit(&event)));
                    }
                }
            }
            Work::Flush(done) => {
                for sink in &sinks {
                    let _ = catch_unwind(AssertUnwindSafe(|| sink.flush()));
                }
                let _ = done.send(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{MeasureKind, Shape};
    use std::{sync::Mutex, time::SystemTime};

    #[derive(Clone, Default)]
    struct Kept(Arc<Mutex<Vec<String>>>);
    impl Kept {
        fn names(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }
    struct Recording {
        kept: Kept,
        points_only: bool,
    }
    impl Sink for Recording {
        fn wants(&self, shape: &Shape) -> bool {
            !self.points_only || matches!(shape, Shape::Point { .. })
        }
        fn emit(&self, event: &Event) {
            self.kept.0.lock().unwrap().push(event.name().to_owned());
        }
    }
    struct Panicking;
    impl Sink for Panicking {
        fn emit(&self, event: &Event) {
            if event.name() == "boom" {
                panic!("a sink failed");
            }
        }
    }
    struct Blocking(Arc<Mutex<()>>);
    impl Sink for Blocking {
        fn emit(&self, _: &Event) {
            drop(self.0.lock().unwrap());
        }
    }

    fn point(name: &'static str) -> Event {
        Event::point(name, SystemTime::UNIX_EPOCH)
    }

    #[test]
    fn off_does_nothing() {
        let telemetry = Telemetry::off();
        assert!(!telemetry.is_on());
        let mut built = false;
        telemetry.emit_with(|| {
            built = true;
            point("never")
        });
        telemetry.emit(point("dropped"));
        assert!(!built, "an event is not built while telemetry is off");
        assert_eq!(telemetry.dropped(), 0);
        assert!(telemetry.flush(Duration::from_millis(1)));
        assert!(!Telemetry::new(Vec::new(), 8).unwrap().is_on());
    }

    #[test]
    fn each_sink_gets_the_shapes_it_wants_in_order() {
        let all = Kept::default();
        let points = Kept::default();
        let telemetry = Telemetry::new(
            vec![
                Box::new(Recording {
                    kept: all.clone(),
                    points_only: false,
                }),
                Box::new(Recording {
                    kept: points.clone(),
                    points_only: true,
                }),
            ],
            8,
        )
        .unwrap();
        telemetry.emit(point("first"));
        telemetry.emit(Event::measure(
            "count",
            1.0,
            MeasureKind::Counter,
            SystemTime::UNIX_EPOCH,
        ));
        telemetry.emit(point("second"));
        assert!(telemetry.shutdown(Duration::from_secs(5)));
        assert_eq!(all.names(), ["first", "count", "second"]);
        assert_eq!(points.names(), ["first", "second"]);
    }

    #[test]
    fn a_panicking_sink_loses_one_event_and_keeps_the_rest() {
        let kept = Kept::default();
        let telemetry = Telemetry::new(
            vec![
                Box::new(Panicking),
                Box::new(Recording {
                    kept: kept.clone(),
                    points_only: false,
                }),
            ],
            8,
        )
        .unwrap();
        telemetry.emit(point("boom"));
        telemetry.emit(point("after"));
        assert!(telemetry.flush(Duration::from_secs(5)));
        assert_eq!(kept.names(), ["boom", "after"]);
    }

    #[test]
    fn a_full_queue_drops_and_counts() {
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let telemetry = Telemetry::new(vec![Box::new(Blocking(gate.clone()))], 1).unwrap();
        // The worker takes one and waits on the gate; one more fills the
        // queue; the rest find it full.
        for _ in 0..10 {
            telemetry.emit(point("p"));
        }
        assert!(telemetry.dropped() >= 8, "{}", telemetry.dropped());
        drop(held);
        assert!(telemetry.flush(Duration::from_secs(5)));
    }
}
