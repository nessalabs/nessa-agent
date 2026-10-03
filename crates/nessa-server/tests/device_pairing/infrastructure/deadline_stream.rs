//! `DeadlineStream` against a recording socket double: reads are ticked and
//! retried, sends are made once and a timed-out send is terminal.
use super::{
    wake::{WakeEndpoint, WakeEndpoints, WAKE_TICK},
    DeadlineStream, NativeSocket, TLS_DEADLINE,
};
use crate::app::ports::Clock;
use std::{
    collections::VecDeque,
    io::{Error, ErrorKind, Read, Result as IoResult, Write},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

struct Elapsed(AtomicU64);
impl Clock for Elapsed {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Calls {
    reads: usize,
    writes: usize,
    flushes: usize,
    read_timeouts: Vec<Duration>,
    write_timeouts: Vec<Duration>,
}
/// Answers reads and writes from scripts and records every call.
struct RecordingSocket {
    calls: Arc<Mutex<Calls>>,
    reads: VecDeque<IoResult<Vec<u8>>>,
    writes: VecDeque<IoResult<usize>>,
}
impl Read for RecordingSocket {
    fn read(&mut self, bytes: &mut [u8]) -> IoResult<usize> {
        self.calls.lock().unwrap().reads += 1;
        let data = self.reads.pop_front().expect("scripted read")?;
        bytes[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}
impl Write for RecordingSocket {
    fn write(&mut self, _: &[u8]) -> IoResult<usize> {
        self.calls.lock().unwrap().writes += 1;
        self.writes.pop_front().expect("scripted write")
    }
    fn flush(&mut self) -> IoResult<()> {
        self.calls.lock().unwrap().flushes += 1;
        Ok(())
    }
}
impl NativeSocket for RecordingSocket {
    fn set_nonblocking(&self, _: bool) -> IoResult<()> {
        Ok(())
    }
    fn set_read_timeout(&self, timeout: Option<Duration>) -> IoResult<()> {
        self.calls
            .lock()
            .unwrap()
            .read_timeouts
            .push(timeout.unwrap());
        Ok(())
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> IoResult<()> {
        self.calls
            .lock()
            .unwrap()
            .write_timeouts
            .push(timeout.unwrap());
        Ok(())
    }
}

fn stream(
    reads: Vec<IoResult<Vec<u8>>>,
    writes: Vec<IoResult<usize>>,
    elapsed_ms: u64,
) -> (
    DeadlineStream<RecordingSocket>,
    Arc<Mutex<Calls>>,
    Arc<Elapsed>,
    Arc<WakeEndpoint>,
) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let clock = Arc::new(Elapsed(AtomicU64::new(elapsed_ms)));
    let wake = WakeEndpoint::new("127.0.0.1:9".parse().unwrap());
    let (stream, _) = DeadlineStream::new(
        RecordingSocket {
            calls: calls.clone(),
            reads: reads.into(),
            writes: writes.into(),
        },
        clock.clone(),
        wake.clone(),
    )
    .unwrap();
    (stream, calls, clock, wake)
}

/// The OS reports a timed-out send as WouldBlock on Unix and TimedOut on
/// Windows; both are terminal.
#[test]
fn timed_out_send_is_terminal_and_never_retried() {
    for elapsed in [ErrorKind::WouldBlock, ErrorKind::TimedOut] {
        let (mut stream, calls, clock, _wake) =
            stream(Vec::new(), vec![Err(Error::from(elapsed))], 0);
        // Four seconds into the ten-second TLS phase.
        clock.0.store(4_000, Ordering::SeqCst);
        assert_eq!(
            stream.write(b"frame").unwrap_err().kind(),
            ErrorKind::TimedOut
        );
        // One send, given the rest of the phase deadline, not the wake tick.
        {
            let calls = calls.lock().unwrap();
            assert_eq!(calls.writes, 1);
            assert_eq!(
                calls.write_timeouts,
                [TLS_DEADLINE - Duration::from_secs(4)]
            );
        }
        // The stream is finished: nothing touches the socket again.
        assert_eq!(
            stream.write(b"frame").unwrap_err().kind(),
            ErrorKind::TimedOut
        );
        assert_eq!(
            stream.read(&mut [0; 8]).unwrap_err().kind(),
            ErrorKind::TimedOut
        );
        assert_eq!(stream.flush().unwrap_err().kind(), ErrorKind::TimedOut);
        let calls = calls.lock().unwrap();
        assert_eq!((calls.writes, calls.reads, calls.flushes), (1, 0, 0));
    }
}

#[test]
fn completed_send_passes_through_once() {
    let (mut stream, calls, _clock, _wake) = stream(Vec::new(), vec![Ok(3), Ok(2)], 0);
    assert_eq!(stream.write(b"abcde").unwrap(), 3);
    assert_eq!(stream.write(b"de").unwrap(), 2);
    stream.flush().unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!((calls.writes, calls.flushes), (2, 1));
    assert_eq!(calls.write_timeouts, [TLS_DEADLINE, TLS_DEADLINE]);
}

/// A timed-out read consumed nothing, so it is retried at the wake tick.
#[test]
fn elapsed_read_waits_are_retried_at_the_tick() {
    let (mut stream, calls, _clock, _wake) = stream(
        vec![
            Err(Error::from(ErrorKind::WouldBlock)),
            Err(Error::from(ErrorKind::TimedOut)),
            Ok(b"data".to_vec()),
        ],
        Vec::new(),
        0,
    );
    let mut bytes = [0; 8];
    assert_eq!(stream.read(&mut bytes).unwrap(), 4);
    assert_eq!(&bytes[..4], b"data");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.reads, 3);
    assert_eq!(calls.read_timeouts, [WAKE_TICK; 3]);
}

#[test]
fn woken_stream_refuses_io_without_touching_the_socket() {
    let (mut stream, calls, _clock, wake) = stream(Vec::new(), Vec::new(), 0);
    let mut owner = WakeEndpoints::new();
    owner.register(&wake);
    owner.close();
    assert_eq!(
        stream.read(&mut [0; 8]).unwrap_err().kind(),
        ErrorKind::ConnectionAborted
    );
    assert_eq!(
        stream.write(b"frame").unwrap_err().kind(),
        ErrorKind::ConnectionAborted
    );
    let calls = calls.lock().unwrap();
    assert_eq!((calls.reads, calls.writes), (0, 0));
}
