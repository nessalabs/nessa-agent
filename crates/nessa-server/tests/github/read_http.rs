//! Real loopback HTTP cancellation, with a synthetic token and no provider traffic.
use super::*;
use crate::github::infrastructure::{GithubApp, GithubHttps};
use tokio::io::AsyncReadExt;

#[tokio::test]
async fn cancelled_http_read_and_queued_read_release_disconnect() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = GithubApp::parse(
        &serde_json::to_vec(&serde_json::json!({
            "clientId":"Iv1.disposable", "slug":"disposable", "webUrl":base, "apiUrl":base,
        }))
        .unwrap(),
    )
    .unwrap();
    let fixture = Fixture::new();
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("ghu_synthetic_http"),
    });
    let connections = Arc::new(
        GithubConnections::load(
            Arc::new(GithubHttps::new(app).unwrap()),
            fixture.store.clone(),
            fixture.store.clone(),
            fixture.audit.clone(),
            fixture.timer.clone(),
        )
        .await,
    );
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let (stop, signal) = tokio::sync::watch::channel(false);
    let first = {
        let connections = connections.clone();
        let repository = repository.clone();
        tokio::spawn(async move {
            connections
                .read_repository(&repository, GithubReadKind::Repository, signal)
                .await
        })
    };
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        request.push(
            tokio::time::timeout(Duration::from_secs(5), socket.read_u8())
                .await
                .unwrap()
                .unwrap(),
        );
        assert!(request.len() < 8192);
    }
    let request = String::from_utf8(request).unwrap();
    assert!(request.starts_with("GET /repos/nessalabs/nessa-agent HTTP/1.1\r\n"));
    assert!(request
        .to_ascii_lowercase()
        .contains("authorization: bearer ghu_synthetic_http\r\n"));
    // The server deliberately sends no response. Queue a second call and a
    // Disconnect behind that real, already authenticated GET.
    let (queued_stop, signal) = tokio::sync::watch::channel(false);
    let queued = connections.read_repository(&repository, GithubReadKind::Issue(1), signal);
    tokio::pin!(queued);
    assert!(futures_util::poll!(&mut queued).is_pending());
    let disconnect = connections.disconnect(caller("http-test"));
    tokio::pin!(disconnect);
    assert!(futures_util::poll!(&mut disconnect).is_pending());
    queued_stop.send_replace(true);
    assert_eq!(
        futures_util::poll!(&mut queued),
        std::task::Poll::Ready(Err(GithubReadError::Cancelled))
    );
    stop.send_replace(true);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap(),
        Err(GithubReadError::Cancelled)
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), disconnect)
            .await
            .unwrap(),
        Ok(signed_out(None))
    );
    assert!(fixture.store.held.lock().unwrap().is_none());
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), socket.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert!(futures_util::poll!(Box::pin(listener.accept())).is_pending());
}
