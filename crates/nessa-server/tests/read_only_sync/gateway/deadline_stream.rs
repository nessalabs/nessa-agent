use super::super::deadline_stream::DeadlineStream;
use crate::app::ports::Clock;
use crate::read_only_sync::application::{Cancellation, GatewayError, GatewayStream};
use std::io::{ErrorKind, Read, Result as IoResult, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct Time(AtomicU64);
impl Clock for Time {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
struct Cancel(AtomicBool);
impl Cancellation for Cancel {
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
struct Physical {
    clock: Arc<Time>,
    reads: Arc<AtomicUsize>,
    shutdowns: Arc<AtomicUsize>,
    failure: Option<ErrorKind>,
}
impl Read for Physical {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(kind) = self.failure {
            return Err(kind.into());
        }
        bytes[0] = 1;
        self.clock.0.fetch_add(4, Ordering::SeqCst);
        Ok(1)
    }
}
impl Write for Physical {
    fn write(&mut self, bytes: &[u8]) -> IoResult<usize> {
        Ok(bytes.len())
    }
    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}
impl GatewayStream for Physical {
    fn read_timeout(&self, timeout: Duration) -> IoResult<()> {
        assert!(timeout <= Duration::from_millis(10));
        Ok(())
    }
    fn write_timeout(&self, _: Duration) -> IoResult<()> {
        Ok(())
    }
    fn shutdown(&self) -> IoResult<()> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[test]
fn trickled_reads_share_absolute_deadline_and_stop_before_fourth_physical_read() {
    let clock = Arc::new(Time(AtomicU64::new(0)));
    let reads = Arc::new(AtomicUsize::new(0));
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let physical = Physical {
        clock: clock.clone(),
        reads: reads.clone(),
        shutdowns: shutdowns.clone(),
        failure: None,
    };
    let mut stream = DeadlineStream::new(
        Box::new(physical),
        clock,
        Arc::new(Cancel(AtomicBool::new(false))),
        10,
    );
    let mut bytes = [0; 1];
    assert_eq!(stream.read(&mut bytes).unwrap(), 1);
    assert_eq!(stream.read(&mut bytes).unwrap(), 1);
    assert!(stream.read(&mut bytes).is_err());
    assert_eq!(stream.take_failure(), Some(GatewayError::TimedOut));
    assert!(stream.read(&mut bytes).is_err());
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    drop(stream);
    assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
}
#[test]
fn cancellation_and_os_timeout_retain_typed_cause_without_extra_io() {
    for expected in [GatewayError::Cancelled, GatewayError::TimedOut] {
        let clock = Arc::new(Time(AtomicU64::new(0)));
        let reads = Arc::new(AtomicUsize::new(0));
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(Cancel(AtomicBool::new(expected == GatewayError::Cancelled)));
        let physical = Physical {
            clock: clock.clone(),
            reads: reads.clone(),
            shutdowns: shutdowns.clone(),
            failure: (expected == GatewayError::TimedOut).then_some(ErrorKind::TimedOut),
        };
        let mut stream = DeadlineStream::new(Box::new(physical), clock, cancel, 10);
        assert!(stream.read(&mut [0; 1]).is_err());
        assert_eq!(stream.take_failure(), Some(expected));
        assert_eq!(
            reads.load(Ordering::SeqCst),
            usize::from(expected == GatewayError::TimedOut)
        );
        drop(stream);
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn admitted_io_cancellation_and_partial_writes_stop_before_later_effects() {
    struct Admitted {
        clock: Arc<Time>,
        cancel: Arc<Cancel>,
        effects: Arc<AtomicUsize>,
        shutdowns: Arc<AtomicUsize>,
        cancel_after_read: bool,
    }
    impl Read for Admitted {
        fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
            self.effects.fetch_add(1, Ordering::SeqCst);
            bytes[0] = 1;
            self.cancel
                .0
                .store(self.cancel_after_read, Ordering::SeqCst);
            Ok(1)
        }
    }
    impl Write for Admitted {
        fn write(&mut self, _: &[u8]) -> IoResult<usize> {
            self.effects.fetch_add(1, Ordering::SeqCst);
            self.clock.0.fetch_add(4, Ordering::SeqCst);
            Ok(1)
        }
        fn flush(&mut self) -> IoResult<()> {
            Ok(())
        }
    }
    impl GatewayStream for Admitted {
        fn read_timeout(&self, _: Duration) -> IoResult<()> {
            Ok(())
        }
        fn write_timeout(&self, _: Duration) -> IoResult<()> {
            Ok(())
        }
        fn shutdown(&self) -> IoResult<()> {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    for cancel_after_read in [true, false] {
        let clock = Arc::new(Time(AtomicU64::new(0)));
        let cancel = Arc::new(Cancel(AtomicBool::new(false)));
        let effects = Arc::new(AtomicUsize::new(0));
        let shutdowns = Arc::new(AtomicUsize::new(0));
        let mut stream = DeadlineStream::new(
            Box::new(Admitted {
                clock: clock.clone(),
                cancel: cancel.clone(),
                effects: effects.clone(),
                shutdowns: shutdowns.clone(),
                cancel_after_read,
            }),
            clock,
            cancel,
            10,
        );
        if cancel_after_read {
            assert!(stream.read(&mut [0; 1]).is_err());
            assert_eq!(stream.take_failure(), Some(GatewayError::Cancelled));
            assert!(stream.read(&mut [0; 1]).is_err());
            assert_eq!(effects.load(Ordering::SeqCst), 1);
        } else {
            assert!(stream.write_all(&[0; 4]).is_err());
            assert_eq!(stream.take_failure(), Some(GatewayError::TimedOut));
            assert!(stream.write(&[0; 1]).is_err());
            assert_eq!(effects.load(Ordering::SeqCst), 3);
        }
        drop(stream);
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    }
}
/// Row W11: the peer ending the connection is an untyped close however the
/// platform reports it; Windows reports a reset or abort, not end of stream.
#[test]
fn peer_reset_or_abort_is_an_untyped_close_and_other_errors_stay_transport() {
    for (kind, expected) in [
        (ErrorKind::ConnectionReset, GatewayError::Closed(None)),
        (ErrorKind::ConnectionAborted, GatewayError::Closed(None)),
        (ErrorKind::BrokenPipe, GatewayError::Closed(None)),
        (ErrorKind::UnexpectedEof, GatewayError::Closed(None)),
        (ErrorKind::PermissionDenied, GatewayError::Transport),
    ] {
        let clock = Arc::new(Time(AtomicU64::new(0)));
        let physical = Physical {
            clock: clock.clone(),
            reads: Arc::new(AtomicUsize::new(0)),
            shutdowns: Arc::new(AtomicUsize::new(0)),
            failure: Some(kind),
        };
        let mut stream = DeadlineStream::new(
            Box::new(physical),
            clock,
            Arc::new(Cancel(AtomicBool::new(false))),
            10,
        );
        assert!(stream.read(&mut [0; 1]).is_err());
        assert_eq!(stream.take_failure(), Some(expected), "{kind:?}");
    }
}
