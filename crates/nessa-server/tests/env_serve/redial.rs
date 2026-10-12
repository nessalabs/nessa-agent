use super::{redial, stop_past_deadline, Backoff};
use crate::env_serve::application::{timer::manual::ManualTimer, Moment, Timer};
use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{watch, Notify};

const BACKOFF: Backoff = Backoff {
    first: Duration::from_secs(1),
    most: Duration::from_secs(8),
};

/// A gateway this side dials: each dial takes the next answer, and when the
/// answers run out it is unreachable. Records when, on its timer, each dial
/// was made.
struct Gateway {
    answers: Mutex<Vec<io::Result<u32>>>,
    dialed: Mutex<Vec<Moment>>,
    timer: Arc<ManualTimer>,
}

impl Gateway {
    fn new(answers: Vec<io::Result<u32>>) -> Arc<Self> {
        let mut answers = answers;
        answers.reverse();
        Arc::new(Self {
            answers: Mutex::new(answers),
            dialed: Mutex::new(Vec::new()),
            timer: Arc::default(),
        })
    }

    async fn dial(&self) -> io::Result<u32> {
        self.dialed.lock().unwrap().push(self.timer.now());
        self.answers
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Err(io::ErrorKind::ConnectionRefused.into()))
    }

    /// The wait before each dial after the first.
    fn waits(&self) -> Vec<Duration> {
        let dialed = self.dialed.lock().unwrap();
        dialed
            .windows(2)
            .map(|pair| pair[1].since(pair[0]))
            .collect()
    }
}

fn refused() -> io::Result<u32> {
    Err(io::ErrorKind::PermissionDenied.into())
}

#[tokio::test]
async fn failed_dials_wait_twice_as_long_each_time_up_to_the_most() {
    let gateway = Gateway::new((0..6).map(|_| refused()).collect());
    let (stop, stopping) = watch::channel(false);
    let dialing = {
        let gateway = gateway.clone();
        tokio::spawn(async move {
            let timer = gateway.timer.clone();
            redial(
                || gateway.dial(),
                |_, _| async {},
                BACKOFF,
                &*timer,
                stopping,
                watch::channel(false).1,
            )
            .await
        })
    };
    for wait in 0..6 {
        gateway.timer.run_out(wait).await;
    }
    // The seventh wait has started: the seventh dial was made.
    gateway.timer.nth(6).await;
    stop.send(true).unwrap();
    assert!(dialing.await.unwrap().is_ok());
    let seconds: Vec<u64> = gateway.waits().iter().map(Duration::as_secs).collect();
    assert_eq!(seconds[..6], [1, 2, 4, 8, 8, 8]);
}

#[tokio::test]
async fn each_connection_is_served_and_a_long_one_starts_the_wait_over() {
    let gateway = Gateway::new(vec![refused(), refused(), refused(), Ok(1), Ok(2)]);
    let served = Arc::new(Mutex::new(Vec::new()));
    let (stop, stopping) = watch::channel(false);
    let dialing = {
        let gateway = gateway.clone();
        let served = served.clone();
        tokio::spawn(async move {
            let timer = gateway.timer.clone();
            redial(
                || gateway.dial(),
                |connection, _| {
                    served.lock().unwrap().push(connection);
                    let timer = timer.clone();
                    async move {
                        // The first connection lasts past the most; the
                        // second ends at once.
                        if connection == 1 {
                            timer.sleep(Duration::from_secs(20)).await;
                        }
                    }
                },
                BACKOFF,
                &*gateway.timer,
                stopping,
                watch::channel(false).1,
            )
            .await
        })
    };
    // Three waits after refusals, connection 1's 20 s, and two waits after
    // connections; the seventh wait has started once the sixth dial was made.
    for wait in 0..6 {
        gateway.timer.run_out(wait).await;
    }
    gateway.timer.nth(6).await;
    stop.send(true).unwrap();
    assert!(dialing.await.unwrap().is_ok());
    assert_eq!(*served.lock().unwrap(), [1, 2]);
    let seconds: Vec<u64> = gateway.waits().iter().map(Duration::as_secs).collect();
    // 1, 2, 4 refused; connection 1 served 20 s, then the wait is 1 again;
    // connection 2 ended at once, so the next wait doubles.
    assert_eq!(seconds[..5], [1, 2, 4, 21, 2]);
}

#[tokio::test]
async fn a_setup_dialing_again_cannot_change_stops_it() {
    let gateway = Gateway::new(vec![
        refused(),
        Err(io::Error::new(io::ErrorKind::InvalidInput, "not a key")),
    ]);
    let (_stop, stopping) = watch::channel(false);
    let timer = gateway.timer.clone();
    let stopped = timer
        .passing(redial(
            || gateway.dial(),
            |_, _| async {},
            BACKOFF,
            &*timer,
            stopping,
            watch::channel(false).1,
        ))
        .await
        .unwrap_err();
    assert_eq!(stopped.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(gateway.waits(), [Duration::from_secs(1)]);
}

#[tokio::test]
async fn stop_closes_the_connection_being_served_and_dials_no_more() {
    let gateway = Gateway::new(vec![Ok(1)]);
    let serving = Arc::new(Notify::new());
    let (stop, stopping) = watch::channel(false);
    let dialing = {
        let gateway = gateway.clone();
        let serving = serving.clone();
        tokio::spawn(async move {
            redial(
                || gateway.dial(),
                // Serves until stop is asked, as a connection closed then does.
                |_, mut stopping: watch::Receiver<bool>| {
                    serving.notify_one();
                    async move {
                        let _ = stopping.wait_for(|stop| *stop).await;
                    }
                },
                BACKOFF,
                &*gateway.timer,
                stopping,
                watch::channel(false).1,
            )
            .await
        })
    };
    serving.notified().await;
    stop.send(true).unwrap();
    assert!(dialing.await.unwrap().is_ok());
    assert_eq!(gateway.dialed.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn stop_asked_while_waiting_ends_the_wait() {
    let gateway = Gateway::new(Vec::new());
    let (stop, stopping) = watch::channel(false);
    let dialing = {
        let gateway = gateway.clone();
        tokio::spawn(async move {
            let timer = gateway.timer.clone();
            redial(
                || gateway.dial(),
                |_, _| async {},
                BACKOFF,
                &*timer,
                stopping,
                watch::channel(false).1,
            )
            .await
        })
    };
    let wait = gateway.timer.nth(0).await;
    stop.send(true).unwrap();
    let dialed = tokio::time::timeout(Duration::from_secs(5), dialing)
        .await
        .expect("stop ends the wait before its deadline");
    assert!(dialed.unwrap().is_ok());
    assert!(gateway.timer.now() < wait.deadline);
    assert_eq!(gateway.dialed.lock().unwrap().len(), 1);
}

/// Row K1: a drain lets the connection being served end by itself, then
/// dials no more.
#[tokio::test]
async fn a_drain_lets_the_connection_end_by_itself_and_dials_no_more() {
    let gateway = Gateway::new(vec![Ok(1), Ok(2)]);
    let serving = Arc::new(Notify::new());
    let (_stop, stopping) = watch::channel(false);
    let (drain, draining) = watch::channel(false);
    let dialing = {
        let gateway = gateway.clone();
        let serving = serving.clone();
        let drained = draining.clone();
        tokio::spawn(async move {
            redial(
                || gateway.dial(),
                // Serves until drained, as a drained connection ends.
                |_, _| {
                    serving.notify_one();
                    let mut draining = drained.clone();
                    async move {
                        let _ = draining.wait_for(|drain| *drain).await;
                    }
                },
                BACKOFF,
                &*gateway.timer,
                stopping,
                draining,
            )
            .await
        })
    };
    serving.notified().await;
    drain.send(true).unwrap();
    let dialed = tokio::time::timeout(Duration::from_secs(5), dialing)
        .await
        .expect("a drained connection ends dialing");
    assert!(dialed.unwrap().is_ok());
    assert_eq!(gateway.dialed.lock().unwrap().len(), 1);
}

/// A drain asked while waiting to dial again has nothing to drain: the wait
/// ends and nothing is dialed.
#[tokio::test]
async fn a_drain_asked_while_waiting_ends_the_wait() {
    let gateway = Gateway::new(Vec::new());
    let (_stop, stopping) = watch::channel(false);
    let (drain, draining) = watch::channel(false);
    let dialing = {
        let gateway = gateway.clone();
        tokio::spawn(async move {
            let timer = gateway.timer.clone();
            redial(
                || gateway.dial(),
                |_, _| async {},
                BACKOFF,
                &*timer,
                stopping,
                draining,
            )
            .await
        })
    };
    gateway.timer.nth(0).await;
    drain.send(true).unwrap();
    let dialed = tokio::time::timeout(Duration::from_secs(5), dialing)
        .await
        .expect("a drain ends the wait");
    assert!(dialed.unwrap().is_ok());
    assert_eq!(gateway.dialed.lock().unwrap().len(), 1);
}

/// A drain that has not ended by its deadline is cut: stop is asked then,
/// and not a moment before.
#[tokio::test]
async fn a_drain_past_its_deadline_asks_stop() {
    let timer = Arc::new(ManualTimer::default());
    let (stop, mut stopping) = watch::channel(false);
    let (drain, draining) = watch::channel(false);
    let deadline = Duration::from_secs(20);
    let cutting = {
        let timer = timer.clone();
        tokio::spawn(async move { stop_past_deadline(draining, deadline, &*timer, &stop).await })
    };
    drain.send(true).unwrap();
    let wait = timer.nth(0).await;
    assert_eq!(wait.limit(), deadline);
    assert!(!*stopping.borrow_and_update());
    timer.run_out(0).await;
    tokio::time::timeout(Duration::from_secs(5), cutting)
        .await
        .expect("the deadline cuts")
        .unwrap();
    assert!(*stopping.borrow());
}
