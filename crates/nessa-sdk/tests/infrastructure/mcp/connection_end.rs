//! ADR392 J23: retained admission snapshot while watch publication waits.
use super::Connection;
use crate::infrastructure::clock::RuntimeClock;
use crate::infrastructure::mcp::McpError;
use std::{
    sync::{
        mpsc::{sync_channel, SyncSender},
        Arc,
    },
    time::Duration,
};
use tokio::{io::duplex, runtime::Handle, sync::oneshot, time::timeout};

const BOUND: Duration = Duration::from_secs(3);

struct ReleasePublication(SyncSender<()>);
impl Drop for ReleasePublication {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[tokio::test]
async fn recorded_end_refuses_admission_while_watch_publication_is_held() {
    let (input, _server_output) = duplex(1024);
    let (output, _server_input) = duplex(1024);
    let connection = Arc::new(Connection::open(
        input,
        output,
        Arc::new(RuntimeClock::new()),
    ));
    let watched = connection.shared.ended.subscribe();
    let (held, holding) = oneshot::channel();
    let (release, resumed) = sync_channel(1);
    let release = ReleasePublication(release);
    let holder = tokio::task::spawn_blocking(move || {
        let value = watched.borrow();
        held.send(()).unwrap();
        resumed.recv().unwrap();
        drop(value);
    });
    timeout(BOUND, holding).await.unwrap().unwrap();
    let ending_connection = connection.clone();
    let ending =
        tokio::task::spawn_blocking(move || ending_connection.shared.end(McpError::Closed));
    timeout(BOUND, async {
        while connection.end_cause().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // subscribe clones/version-loads, without borrowing the held watch value.
    // The retained snapshot must reject before polling that value or ready enqueue.
    let notifying_connection = connection.clone();
    let runtime = Handle::current();
    let mut notifying = tokio::task::spawn_blocking(move || {
        runtime.block_on(notifying_connection.notify("notifications/test", None))
    });
    let notified = timeout(BOUND, &mut notifying).await;
    drop(release);
    holder.await.unwrap();
    ending.await.unwrap();
    if notified.is_err() {
        notifying.await.unwrap().unwrap_err();
    }
    connection.close(McpError::Closed);
    assert!(
        matches!(notified, Ok(Ok(Err(McpError::Closed)))),
        "retained snapshot must return before watch publication: {notified:?}"
    );
}
