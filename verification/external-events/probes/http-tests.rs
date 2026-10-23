#[cfg(test)]
mod probe_tests {
    use super::*;
    struct Fixed;
    impl crate::application::ports::Clock for Fixed {
        fn unix_seconds(&self) -> u64 {
            1000
        }
    }
    fn state() -> Arc<Ingress> {
        Arc::new(Ingress {
            binding: EventBinding {
                id: EventId::new("ci".into()).unwrap(),
                clock: Arc::new(Fixed),
                gate: None,
            },
            key: SigningKey::new(vec![42; 32]).unwrap(),
            slots: Arc::new(Semaphore::new(8)),
        })
    }
    fn request(body: &str, timestamp: &str, key: &[u8], binding: &str) -> Request<Body> {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        mac.update(b"nessa-event\n");
        mac.update(binding.as_bytes());
        mac.update(b"\n");
        mac.update(timestamp.as_bytes());
        mac.update(b"\n");
        mac.update(body.as_bytes());
        Request::builder()
            .header("content-type", "application/json")
            .header("x-nessa-timestamp", timestamp)
            .header(
                "x-nessa-signature",
                STANDARD.encode(mac.finalize().into_bytes()),
            )
            .body(Body::from(body.to_owned()))
            .unwrap()
    }
    const VALID: &str = r#"{"id":"event","payload":{}}"#;
    #[tokio::test]
    async fn signatures_and_binding_are_checked() {
        assert_eq!(
            receive(State(state()), request(VALID, "1000", &[42; 32], "ci"))
                .await
                .status(),
            StatusCode::ACCEPTED
        );
        assert_eq!(
            receive(State(state()), request(VALID, "1000", &[43; 32], "ci"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            receive(State(state()), request(VALID, "1000", &[42; 32], "other"))
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    #[tokio::test]
    async fn freshness_is_checked() {
        for time in ["699", "1301", "01000"] {
            assert_eq!(
                receive(State(state()), request(VALID, time, &[42; 32], "ci"))
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        for time in ["700", "1300"] {
            assert_eq!(
                receive(State(state()), request(VALID, time, &[42; 32], "ci"))
                    .await
                    .status(),
                StatusCode::ACCEPTED
            );
        }
    }
    #[tokio::test]
    async fn repeated_headers_are_rejected() {
        let mut req = request(VALID, "1000", &[42; 32], "ci");
        req.headers_mut().append(
            "x-nessa-timestamp",
            axum::http::HeaderValue::from_static("1000"),
        );
        assert_eq!(
            receive(State(state()), req).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    #[tokio::test]
    async fn envelope_shape_is_strict() {
        let body = r#"{"id":"event","payload":{},"actor":"owner"}"#;
        assert_eq!(
            receive(State(state()), request(body, "1000", &[42; 32], "ci"))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    #[tokio::test]
    async fn body_limit_is_enforced() {
        let body = "x".repeat(MAX_EVENT_BYTES + 1);
        assert_eq!(
            receive(State(state()), request(&body, "1000", &[42; 32], "ci"))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
    #[tokio::test(start_paused = true)]
    async fn body_deadline_is_enforced() {
        let mut req = request(VALID, "1000", &[42; 32], "ci");
        *req.body_mut() = Body::from_stream(futures_util::stream::pending::<
            Result<axum::body::Bytes, std::io::Error>,
        >());
        let start = tokio::time::Instant::now();
        assert_eq!(
            receive(State(state()), req).await.status(),
            StatusCode::REQUEST_TIMEOUT
        );
        assert_eq!(start.elapsed(), Duration::from_secs(5));
    }
    #[tokio::test]
    async fn full_capacity_refuses_another_handler() {
        let state = state();
        let _held = state.slots.acquire_many(8).await.unwrap();
        assert_eq!(
            receive(
                State(state.clone()),
                request(VALID, "1000", &[42; 32], "ci")
            )
            .await
            .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    #[tokio::test]
    async fn content_type_is_explicit() {
        let mut req = request(VALID, "1000", &[42; 32], "ci");
        req.headers_mut().insert(
            "content-type",
            axum::http::HeaderValue::from_static("text/plain"),
        );
        assert_eq!(
            receive(State(state()), req).await.status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
    #[tokio::test]
    async fn signature_scope_follows_binding() {
        let mut other = state();
        Arc::get_mut(&mut other).unwrap().binding.id = EventId::new("other".into()).unwrap();
        assert_eq!(
            receive(
                State(other.clone()),
                request(VALID, "1000", &[42; 32], "ci")
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            receive(State(other), request(VALID, "1000", &[42; 32], "other"))
                .await
                .status(),
            StatusCode::ACCEPTED
        );
    }
    #[tokio::test]
    async fn signature_length_bounds_decode_allocation() {
        use std::sync::atomic::Ordering;
        let state = state();
        let mut req = request(VALID, "1000", &[42; 32], "ci");
        req.headers_mut().insert(
            "x-nessa-signature",
            axum::http::HeaderValue::from_str(&"A".repeat(8192)).unwrap(),
        );
        crate::allocation_probe::MAX.store(0, Ordering::SeqCst);
        assert_eq!(
            receive(State(state), req).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let largest = crate::allocation_probe::MAX.load(Ordering::SeqCst);
        assert!(largest <= 2048, "largest allocation {largest}");
    }
    #[tokio::test]
    async fn cancelled_responses_retain_capacity_until_admission_settles() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let gate = Arc::new(crate::external_events::application::Gate {
            entered: AtomicUsize::new(0),
            released: tokio::sync::watch::channel(false).0,
        });
        let mut state = state();
        Arc::get_mut(&mut state).unwrap().binding.gate = Some(gate.clone());
        let mut callers = Vec::new();
        for _ in 0..8 {
            let state = state.clone();
            callers.push(tokio::spawn(receive(
                State(state),
                request(VALID, "1000", &[42; 32], "ci"),
            )));
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            while gate.entered.load(Ordering::SeqCst) != 8 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        for caller in &callers {
            caller.abort();
        }
        for caller in callers {
            assert!(caller.await.unwrap_err().is_cancelled());
        }
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            receive(State(state), request(VALID, "1000", &[42; 32], "ci")),
        )
        .await
        .expect("detached admission remains counted");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(gate.entered.load(Ordering::SeqCst), 8);
        gate.released.send(true).unwrap();
    }
}
