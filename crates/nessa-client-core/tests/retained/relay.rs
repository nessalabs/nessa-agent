use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

const LONG: Duration = Duration::from_secs(20);

fn never() -> bool {
    false
}

/// A leg that waits until the test drops `hold`, then fails.
fn hanging(hold: mpsc::Receiver<()>) -> impl FnOnce(Duration) -> IoResult<&'static str> {
    move |_| {
        let _ = hold.recv();
        Err(Error::from(ErrorKind::TimedOut))
    }
}

#[test]
fn a_direct_dial_that_connects_first_never_dials_the_relay() {
    let dialed = Arc::new(AtomicBool::new(false));
    let relay = {
        let dialed = dialed.clone();
        move |_| {
            dialed.store(true, Ordering::SeqCst);
            Ok("relay")
        }
    };
    let used = race(|_| Ok("direct"), relay, LONG, LONG, &never).unwrap();
    assert_eq!(used, "direct");
    assert!(!dialed.load(Ordering::SeqCst));
}

#[test]
fn a_direct_dial_still_unconnected_after_the_head_start_is_raced_by_the_relay() {
    let (_hold, hold) = mpsc::channel();
    let used = race(
        hanging(hold),
        |_| Ok("relay"),
        LONG,
        Duration::from_millis(20),
        &never,
    )
    .unwrap();
    assert_eq!(used, "relay");
}

#[test]
fn a_refused_direct_dial_starts_the_relay_without_waiting_out_the_head_start() {
    let started = Instant::now();
    let used = race(
        |_| Err(Error::from(ErrorKind::ConnectionRefused)),
        |_| Ok("relay"),
        LONG,
        LONG,
        &never,
    )
    .unwrap();
    assert_eq!(used, "relay");
    // Far below the head start, so the relay did not wait for it.
    assert!(started.elapsed() < LONG / 2);
}

#[test]
fn a_relay_that_fails_first_leaves_a_slow_direct_dial_to_connect() {
    let (release, hold) = mpsc::channel::<()>();
    let direct = move |_| {
        let _ = hold.recv();
        Ok("direct")
    };
    let relay = move |_| {
        // Fail first, then let the direct dial connect.
        drop(release);
        Err(Error::from(ErrorKind::ConnectionRefused))
    };
    let used = race(direct, relay, LONG, Duration::ZERO, &never).unwrap();
    assert_eq!(used, "direct");
}

#[test]
fn both_dials_failing_returns_the_relays_error() {
    let refused = race::<()>(
        |_| Err(Error::from(ErrorKind::ConnectionRefused)),
        |_| {
            Err(Error::new(
                ErrorKind::ConnectionRefused,
                RelayRefusal::Offline,
            ))
        },
        LONG,
        LONG,
        &never,
    )
    .unwrap_err();
    assert_eq!(RelayRefusal::carried(&refused), Some(RelayRefusal::Offline));
}

#[test]
fn the_race_ends_at_its_budget_with_both_dials_still_running() {
    let (_direct, direct) = mpsc::channel();
    let (_relay, relay) = mpsc::channel();
    let ended = race(
        hanging(direct),
        hanging(relay),
        Duration::from_millis(100),
        Duration::ZERO,
        &never,
    )
    .unwrap_err();
    assert_eq!(ended.kind(), ErrorKind::TimedOut);
}

#[test]
fn a_stop_ends_the_race_while_both_dials_hang() {
    let (_direct, direct) = mpsc::channel();
    let (_relay, relay) = mpsc::channel();
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    let ended = thread::scope(|scope| {
        scope.spawn(|| {
            thread::sleep(Duration::from_millis(50));
            stop.store(true, Ordering::SeqCst);
        });
        race(
            hanging(direct),
            hanging(relay),
            LONG,
            Duration::ZERO,
            &|| stop.load(Ordering::SeqCst),
        )
    })
    .unwrap_err();
    assert_eq!(ended.kind(), ErrorKind::ConnectionAborted);
    assert!(started.elapsed() < LONG / 2);
}

/// A relay that reads one `Connect`, checks its key, and answers `answer`
/// followed by `after` in the same write.
fn scripted_relay(
    answer: RelayFrame,
    after: &'static [u8],
) -> (RelayRoute, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let route = RelayRoute {
        address: RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap(),
        key: [9; 32],
    };
    let serving = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        assert!(matches!(
            read_frame(&mut stream).unwrap(),
            RelayFrame::Connect { key } if key == [9; 32]
        ));
        let mut bytes = answer.encode();
        bytes.extend_from_slice(after);
        stream.write_all(&bytes).unwrap();
        // Hold the connection until the reader has read what it wants.
        let _ = stream.read(&mut [0; 1]);
    });
    (route, serving)
}

#[test]
fn a_joined_relay_leg_leaves_the_gateways_bytes_on_the_stream() {
    let (route, serving) = scripted_relay(RelayFrame::Joined, b"tls");
    let mut stream = relay_leg(&route, LONG).unwrap();
    let mut first = [0; 3];
    stream.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"tls");
    drop(stream);
    serving.join().unwrap();
}

#[test]
fn a_relay_that_does_not_join_refuses_with_its_reason() {
    for (answer, reason) in [
        (RelayFrame::Offline, RelayRefusal::Offline),
        (RelayFrame::NoAnswer, RelayRefusal::NoAnswer),
    ] {
        let (route, serving) = scripted_relay(answer, b"");
        let refused = relay_leg(&route, LONG).unwrap_err();
        assert_eq!(refused.kind(), ErrorKind::ConnectionRefused);
        assert_eq!(RelayRefusal::carried(&refused), Some(reason));
        serving.join().unwrap();
    }
}

#[test]
fn a_relay_answering_a_gateways_frame_is_refused_as_invalid() {
    let (route, serving) = scripted_relay(RelayFrame::Registered, b"");
    let refused = relay_leg(&route, LONG).unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::InvalidData);
    serving.join().unwrap();
}

#[test]
fn a_silent_relay_ends_the_leg_at_its_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let route = RelayRoute {
        address: RelayAddress::try_from(listener.local_addr().unwrap().to_string()).unwrap(),
        key: [9; 32],
    };
    let started = Instant::now();
    let ended = relay_leg(&route, Duration::from_millis(200)).unwrap_err();
    assert!(matches!(
        ended.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut
    ));
    assert!(started.elapsed() < LONG / 2);
    drop(listener);
}
