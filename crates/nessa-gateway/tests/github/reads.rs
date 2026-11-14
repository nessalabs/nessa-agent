//! Read dispatch, cancellation and vault-generation orderings.
use super::*;

#[tokio::test]
async fn repository_reads_use_only_the_current_connected_sign_in() {
    use crate::github::application::GithubReadError;

    let fixture = Fixture::new();
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let connections = fixture.connections().await;
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository, never_stopped())
            .await,
        Err(GithubReadError::NotConnected)
    );
    assert!(fixture.github.reads.lock().unwrap().is_empty());

    let connections = signed_in(&fixture).await;
    *fixture.github.read_answer.lock().unwrap() =
        Ok(serde_json::json!({"full_name":"nessalabs/nessa-agent"}));
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository, never_stopped())
            .await,
        Ok(serde_json::json!({"full_name":"nessalabs/nessa-agent"}))
    );
    assert_eq!(
        *fixture.github.reads.lock().unwrap(),
        [(
            token("ghu_kept"),
            repository.clone(),
            GithubReadKind::Repository
        )]
    );

    connections.disconnect(caller("panel")).await.unwrap();
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository, never_stopped())
            .await,
        Err(GithubReadError::NotConnected)
    );
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn disconnect_waits_for_a_read_and_later_reads_see_signed_out() {
    use crate::github::application::GithubReadError;

    let fixture = Fixture::new();
    let connections = Arc::new(signed_in(&fixture).await);
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let release = Arc::new(Notify::new());
    *fixture.github.hold_reads.lock().unwrap() = Some(release.clone());
    *fixture.github.read_answer.lock().unwrap() = Ok(serde_json::json!({"id":1}));

    let reader = {
        let connections = connections.clone();
        let repository = repository.clone();
        tokio::spawn(async move {
            connections
                .read_repository(&repository, GithubReadKind::Repository, never_stopped())
                .await
        })
    };
    fixture.github.read_started.notified().await;
    let disconnect = {
        let connections = connections.clone();
        tokio::spawn(async move { connections.disconnect(caller("panel")).await })
    };
    tokio::task::yield_now().await;
    assert!(!disconnect.is_finished());
    assert!(fixture.store.held.lock().unwrap().is_some());

    release.notify_one();
    assert_eq!(reader.await.unwrap(), Ok(serde_json::json!({"id":1})));
    assert_eq!(disconnect.await.unwrap(), Ok(signed_out(None)));
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository, never_stopped())
            .await,
        Err(GithubReadError::NotConnected)
    );
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelled_repository_reads_do_not_dispatch() {
    for closed in [false, true] {
        let fixture = Fixture::new();
        let connections = signed_in(&fixture).await;
        let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
        let (stop, signal) = tokio::sync::watch::channel(false);
        if closed {
            drop(stop);
        } else {
            stop.send_replace(true);
        }
        assert_eq!(
            connections
                .read_repository(&repository, GithubReadKind::Repository, signal)
                .await,
            Err(GithubReadError::Cancelled)
        );
        assert!(fixture.github.reads.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn cancelled_queued_read_does_not_delay_disconnect() {
    let fixture = Fixture::new();
    let connections = signed_in(&fixture).await;
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let release = Arc::new(Notify::new());
    *fixture.github.hold_reads.lock().unwrap() = Some(release.clone());
    let first =
        connections.read_repository(&repository, GithubReadKind::Repository, never_stopped());
    tokio::pin!(first);
    assert!(futures_util::poll!(&mut first).is_pending());
    let (stop, signal) = tokio::sync::watch::channel(false);
    let queued = connections.read_repository(&repository, GithubReadKind::Issue(1), signal);
    tokio::pin!(queued);
    assert!(futures_util::poll!(&mut queued).is_pending());
    let disconnect = connections.disconnect(caller("panel"));
    tokio::pin!(disconnect);
    assert!(futures_util::poll!(&mut disconnect).is_pending());
    stop.send_replace(true);
    // It must settle while the first read still owns the gate.
    assert_eq!(
        futures_util::poll!(&mut queued),
        std::task::Poll::Ready(Err(GithubReadError::Cancelled))
    );
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
    release.notify_one();
    let _ = first.await;
    assert_eq!(disconnect.await, Ok(signed_out(None)));
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelled_in_flight_read_releases_disconnect() {
    let fixture = Fixture::new();
    let connections = signed_in(&fixture).await;
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    *fixture.github.hold_reads.lock().unwrap() = Some(Arc::new(Notify::new()));
    let (stop, signal) = tokio::sync::watch::channel(false);
    let read = connections.read_repository(&repository, GithubReadKind::PullRequest(2), signal);
    tokio::pin!(read);
    assert!(futures_util::poll!(&mut read).is_pending());
    let disconnect = connections.disconnect(caller("panel"));
    tokio::pin!(disconnect);
    assert!(futures_util::poll!(&mut disconnect).is_pending());
    stop.send_replace(true);
    assert_eq!(
        futures_util::poll!(&mut read),
        std::task::Poll::Ready(Err(GithubReadError::Cancelled))
    );
    assert_eq!(disconnect.await, Ok(signed_out(None)));
    assert!(fixture.store.held.lock().unwrap().is_none());
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn queued_read_cannot_borrow_a_new_vault_generation() {
    let fixture = Fixture::new();
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("old"),
    });
    let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
    let connections = GithubConnections::load(
        fixture.github.clone(),
        access.clone(),
        fixture.store.clone(),
        fixture.audit.clone(),
        fixture.timer.clone(),
    )
    .await;
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let release = Arc::new(Notify::new());
    *fixture.github.hold_reads.lock().unwrap() = Some(release.clone());
    let first =
        connections.read_repository(&repository, GithubReadKind::Repository, never_stopped());
    tokio::pin!(first);
    assert!(futures_util::poll!(&mut first).is_pending());
    let queued =
        connections.read_repository(&repository, GithubReadKind::Issue(1), never_stopped());
    tokio::pin!(queued);
    assert!(futures_util::poll!(&mut queued).is_pending());
    access.0.store(2, std::sync::atomic::Ordering::SeqCst);
    connections.clear_locked().unwrap();
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("new"),
    });
    connections.restore().await.unwrap();
    release.notify_one();
    let _ = first.await;
    assert_eq!(queued.await, Err(GithubReadError::NotConnected));
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
    assert_eq!(connections.token(), Some(token("new")));
}

#[tokio::test]
async fn old_read_does_not_block_a_new_generation() {
    let fixture = Fixture::new();
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("old"),
    });
    let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
    let connections = GithubConnections::load(
        fixture.github.clone(),
        access.clone(),
        fixture.store.clone(),
        fixture.audit.clone(),
        fixture.timer.clone(),
    )
    .await;
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let release = Arc::new(Notify::new());
    *fixture.github.hold_reads.lock().unwrap() = Some(release.clone());
    let old = connections.read_repository(&repository, GithubReadKind::Repository, never_stopped());
    tokio::pin!(old);
    assert!(futures_util::poll!(&mut old).is_pending());
    access.0.store(2, std::sync::atomic::Ordering::SeqCst);
    connections.clear_locked().unwrap();
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("new"),
    });
    connections.restore().await.unwrap();
    *fixture.github.hold_reads.lock().unwrap() = None;
    *fixture.github.read_answer.lock().unwrap() = Ok(serde_json::json!({"id":42}));
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Issue(1), never_stopped())
            .await,
        Ok(serde_json::json!({"id":42}))
    );
    release.notify_one();
    assert_eq!(old.await, Err(GithubReadError::NotConnected));
    assert_eq!(connections.token(), Some(token("new")));
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cancellation_wins_when_the_adapter_answer_is_also_ready() {
    for kind in [
        GithubReadKind::Repository,
        GithubReadKind::Issue(1),
        GithubReadKind::PullRequest(2),
    ] {
        let fixture = Fixture::new();
        let connections = signed_in(&fixture).await;
        let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
        let release = Arc::new(Notify::new());
        *fixture.github.hold_reads.lock().unwrap() = Some(release.clone());
        *fixture.github.read_answer.lock().unwrap() = Ok(serde_json::json!({"id":1}));
        let (stop, signal) = tokio::sync::watch::channel(false);
        let read = connections.read_repository(&repository, kind, signal);
        tokio::pin!(read);
        assert!(futures_util::poll!(&mut read).is_pending());
        let disconnect = connections.disconnect(caller("panel"));
        tokio::pin!(disconnect);
        assert!(futures_util::poll!(&mut disconnect).is_pending());
        // Both branches can complete at the next poll. The response has not
        // been consumed, so the read-only tool's cancellation rule wins.
        release.notify_one();
        stop.send_replace(true);
        assert_eq!(read.await, Err(GithubReadError::Cancelled));
        assert_eq!(disconnect.await, Ok(signed_out(None)));
        assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
    }
}
