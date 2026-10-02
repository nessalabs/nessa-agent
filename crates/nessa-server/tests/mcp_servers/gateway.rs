/// `GET /mcp-resources` through the real router, over a real connection.
mod mcp_resource_gateway {
    use super::*;
    use crate::conversation::application::ResourceTickets;
    use crate::mcp_servers::infrastructure::ticket_test_support::{app, held, Fixture, CONVERSATION};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const PAGE: &[u8] = b"<!doctype html><title>chart</title>";

    /// One HTTP/1.1 exchange over a real connection to the real router.
    async fn exchange(address: std::net::SocketAddr, head: String) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(head.as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        String::from_utf8_lossy(&reply).into_owned()
    }

    /// Everything traced while it is the default, whatever its level or
    /// source: this crate's, axum's, hyper's.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Self;
        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }

    #[tokio::test]
    async fn a_redeemed_ticket_appears_in_no_log_line_or_trace() {
        let captured = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish();
        // A current-thread runtime: the server's tasks run on this thread,
        // under this default.
        let _traced = tracing::subscriber::set_default(subscriber);
        tracing::info!("capture is live");

        let fixture = Fixture::new();
        let (state, _) = fixture_state();
        let state = state.with_resource_tickets(fixture.store.clone());
        let ticket = fixture
            .store
            .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, crate::server::entrypoint::http::router(state))
                .await
                .unwrap();
        });

        let preflight = exchange(
            address,
            "OPTIONS /mcp-resources HTTP/1.1\r\nHost: gateway\r\nOrigin: tauri://localhost\r\n\
             Access-Control-Request-Method: GET\r\n\
             Access-Control-Request-Headers: x-nessa-resource-ticket\r\nConnection: close\r\n\r\n"
                .into(),
        )
        .await;
        assert!(preflight.starts_with("HTTP/1.1 204"), "{preflight}");
        assert!(preflight.contains("access-control-allow-headers: x-nessa-resource-ticket"));

        let request = format!(
            "GET /mcp-resources HTTP/1.1\r\nHost: gateway\r\nOrigin: tauri://localhost\r\n\
             x-nessa-resource-ticket: {ticket}\r\nConnection: close\r\n\r\n"
        );
        let served = exchange(address, request.clone()).await;
        assert!(served.starts_with("HTTP/1.1 200"), "{served}");
        assert!(served.contains("content-type: text/html;profile=mcp-app"));
        assert!(served.ends_with(std::str::from_utf8(PAGE).unwrap()), "{served}");
        let spent = exchange(address, request).await;
        assert!(spent.starts_with("HTTP/1.1 404"), "{spent}");
        assert!(spent.contains("content-length: 0"), "{spent}");
        // A ticket in the URL is not one: it is not read from there.
        let fresh = fixture
            .store
            .issue(held(CONVERSATION, app("call-1", "mount-1"), PAGE))
            .unwrap();
        let in_url = exchange(
            address,
            format!(
                "GET /mcp-resources?ticket={fresh} HTTP/1.1\r\nHost: gateway\r\n\
                 Connection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(in_url.starts_with("HTTP/1.1 404"), "{in_url}");
        server.abort();

        let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        assert!(logged.contains("capture is live"), "{logged}");
        assert!(!logged.contains(&ticket), "{logged}");
        assert!(!logged.contains(&fresh), "{logged}");
    }

    /// The product route's dependencies, with nothing composed but auth.
    fn fixture_state() -> (ProductRouteState, Arc<Authority>) {
        fixture(MembershipRole::Member)
    }
}
