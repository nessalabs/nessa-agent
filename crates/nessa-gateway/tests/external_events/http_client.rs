//! Loopback fixture transport: the HTTP client owns framing, not socket EOF.
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, CONNECTION, HOST},
    redirect::Policy,
    Client,
};
use rustls::{ClientConfig, RootCertStore};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc, time::Duration};

pub(super) const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) fn client() -> Client {
    // Local provider selection, as in github_relay/transport.rs. No global TLS
    // provider or production HTTPS policy changes for this HTTP-only fixture.
    let tls =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
    Client::builder()
        .tls_backend_preconfigured(tls)
        .no_proxy()
        .redirect(Policy::none())
        .timeout(REPLY_TIMEOUT)
        .build()
        .unwrap()
}

pub(super) fn headers(entries: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in entries {
        headers.append(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}

pub(super) async fn exchange(
    client: &Client,
    address: SocketAddr,
    body: &str,
    headers: HeaderMap,
) -> Result<(u16, Value), reqwest::Error> {
    let response = client
        .post(format!("http://{address}/events"))
        .header(HOST, "localhost")
        .header(CONNECTION, "close")
        .headers(headers)
        .body(body.to_owned())
        .send()
        .await?;
    let status = response.status().as_u16();
    Ok((status, response.json().await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::Request,
        http::StatusCode,
        routing::post,
        Json, Router,
    };
    use serde_json::json;
    use std::{future::Future, task::Poll};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
        task::JoinHandle,
    };

    struct Peer {
        address: SocketAddr,
        accepted: Option<oneshot::Receiver<()>>,
        task: JoinHandle<()>,
    }
    impl Drop for Peer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn peer(reply: &'static [u8], hold_open: bool) -> Peer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted, observed) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket.write_all(reply).await.unwrap();
            let _ = accepted.send(());
            if hold_open {
                std::future::pending::<()>().await;
            }
        });
        Peer {
            address,
            accepted: Some(observed),
            task,
        }
    }

    #[tokio::test]
    async fn a_complete_framed_reply_does_not_wait_for_socket_eof() {
        let peer = peer(
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 11\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}",
            true,
        )
        .await;
        let reply = exchange(&client(), peer.address, "{}", headers(&[]))
            .await
            .unwrap();
        assert_eq!(reply, (202, json!({"ok": true})));
        assert!(
            !peer.task.is_finished(),
            "the peer has not closed the socket"
        );
    }

    #[tokio::test]
    async fn a_truncated_declared_body_is_not_a_receipt() {
        let peer = peer(
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 40\r\nContent-Type: application/json\r\n\r\n{}",
            false,
        )
        .await;
        let error = exchange(&client(), peer.address, "{}", headers(&[]))
            .await
            .unwrap_err();
        assert!(error.is_decode(), "{error:?}");
        let body_error = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<reqwest::Error>())
            .expect("the JSON reader must retain the HTTP body failure");
        assert!(body_error.is_body(), "{error:?}");
    }

    #[tokio::test]
    async fn an_absent_reply_keeps_the_twenty_second_bound() {
        assert_eq!(REPLY_TIMEOUT, Duration::from_secs(20));
        let mut peer = peer(b"", true).await;
        let client = client();
        let request = exchange(&client, peer.address, "{}", headers(&[]));
        tokio::pin!(request);
        tokio::select! {
            result = &mut request => panic!("the peer has sent no reply: {result:?}"),
            accepted = peer.accepted.take().unwrap() => accepted.unwrap(),
        }
        // Pause only after real TCP I/O has established the pending request.
        tokio::time::pause();
        // Sleeping processes the registered timers; advance() alone does not
        // guarantee that the timer driver has notified the pending request.
        tokio::time::sleep(REPLY_TIMEOUT).await;
        let result = std::future::poll_fn(|cx| Poll::Ready(request.as_mut().poll(cx))).await;
        let Poll::Ready(Err(error)) = result else {
            panic!("the original reply bound did not expire: {result:?}");
        };
        assert!(error.is_timeout(), "{error:?}");
    }

    #[tokio::test]
    async fn duplicate_fields_and_the_signed_body_survive_the_real_wire() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let routes = Router::new().route(
            "/events",
            post(|request: Request<Body>| async move {
                let (parts, body) = request.into_parts();
                let values = |name| {
                    parts
                        .headers
                        .get_all(name)
                        .iter()
                        .map(|value| value.to_str().unwrap().to_owned())
                        .collect::<Vec<_>>()
                };
                let timestamps = values("x-nessa-timestamp");
                let signatures = values("x-nessa-signature");
                let body = to_bytes(body, 4096).await.unwrap();
                (
                    StatusCode::ACCEPTED,
                    Json(json!({
                        "timestamps": timestamps,
                        "signatures": signatures,
                        "body": String::from_utf8(body.to_vec()).unwrap(),
                    })),
                )
            }),
        );
        let task = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
        let _peer = Peer {
            address,
            accepted: None,
            task,
        };
        let body = r#"{"id":"é","payload":{"literal":"unchanged"}}"#;
        let (_, observed) = exchange(
            &client(),
            address,
            body,
            headers(&[
                ("Content-Type", "application/json"),
                ("x-nessa-timestamp", "100"),
                ("x-nessa-timestamp", "100"),
                ("x-nessa-signature", "first"),
                ("x-nessa-signature", "second"),
            ]),
        )
        .await
        .unwrap();
        assert_eq!(
            observed,
            json!({
                "timestamps": ["100", "100"],
                "signatures": ["first", "second"],
                "body": body,
            })
        );
    }
}
