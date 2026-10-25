use super::*;
use crate::webhooks::application::observation;
use crate::webhooks::domain::{Delivery, Observation, Phase};
use crate::webhooks::infrastructure::record_message;
use std::error::Error;
use std::io::{Error as IoError, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use rustls::{Error as TlsError, ServerConnection, StreamOwned};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::webhooks::infrastructure::tls_fixture::RecipientTls;

const RECIPIENT_WATCHDOG: Duration = Duration::from_secs(3);

struct Recipient {
    address: SocketAddr,
    request: mpsc::Receiver<Vec<u8>>,
    release: std::sync::mpsc::Sender<()>,
    task: thread::JoinHandle<()>,
    root: Vec<u8>,
}

impl Recipient {
    fn start(response: Vec<u8>, hold_response: bool) -> Self {
        Self::start_with_rejection(response, hold_response, false)
    }

    fn start_with_rejection(
        response: Vec<u8>,
        hold_response: bool,
        expect_rejection: bool,
    ) -> Self {
        install_tls_backend();
        let tls = RecipientTls::generate();
        let root = tls.root.to_vec();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel(1);
        let (release, wait) = std::sync::mpsc::channel();
        let task = thread::spawn(move || {
            // Test-only TLS key and loopback listener. No trust bypass is
            // exposed on the production adapter or Destination constructor.
            let stream = accept(&listener);
            stream.set_read_timeout(Some(RECIPIENT_WATCHDOG)).unwrap();
            stream.set_write_timeout(Some(RECIPIENT_WATCHDOG)).unwrap();
            let mut stream =
                StreamOwned::new(ServerConnection::new(Arc::new(tls.config)).unwrap(), stream);
            let mut bytes = Vec::new();
            loop {
                let mut buf = [0; 1024];
                let len = match stream.read(&mut buf) {
                    Err(_) if expect_rejection => return,
                    result => result.unwrap(),
                };
                assert!(
                    !expect_rejection,
                    "a rejected TLS peer must receive no HTTP bytes"
                );
                assert!(len > 0, "request ended early");
                bytes.extend_from_slice(&buf[..len]);
                assert!(bytes.len() <= crate::webhooks::application::MAX_BODY_BYTES + 8192);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end])
                        .unwrap()
                        .to_ascii_lowercase();
                    let body = headers
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    if bytes.len() == end + 4 + body {
                        break;
                    }
                }
            }
            tx.blocking_send(bytes).unwrap();
            if hold_response {
                let _ = wait.recv_timeout(RECIPIENT_WATCHDOG);
            }
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        });
        Self {
            address,
            request: rx,
            release,
            task,
            root,
        }
    }

    fn client(&self) -> reqwest::Client {
        client_builder("recipient.test", &[self.address])
            .add_root_certificate(reqwest::Certificate::from_der(&self.root).unwrap())
            .build()
            .unwrap()
    }

    fn url(&self) -> url::Url {
        url::Url::parse(&format!(
            "https://recipient.test:{}/events",
            self.address.port()
        ))
        .unwrap()
    }

    fn finish(self) {
        let _ = self.release.send(());
        self.task.join().unwrap();
    }
}

fn accept(listener: &TcpListener) -> TcpStream {
    listener.accept().unwrap().0
}

fn message() -> Message {
    record_message(
        Uuid::from_u128(1),
        0,
        "00000000-0000-0000-0000-00000000000a",
        "i",
        1,
        r#"{"text":"hello"}"#,
    )
    .unwrap()
}

#[tokio::test]
async fn isolated_tls_recipient_verifies_exact_signed_bytes() {
    let mut recipient = Recipient::start(
        b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec(),
        false,
    );
    let key = MacKey::new(std::array::from_fn(|i| i as u8));
    let message = message();
    assert_eq!(
        call(&recipient.client(), &recipient.url(), &key, &message).await,
        Ok(204)
    );
    let request = recipient.request.recv().await.unwrap();
    let split = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let headers = std::str::from_utf8(&request[..split]).unwrap();
    assert!(headers.starts_with("POST /events HTTP/1.1\r\n"));
    let headers = headers.to_ascii_lowercase();
    assert!(headers.contains("content-type: application/json\r\n"));
    assert!(headers.contains("nessa-webhook-timestamp: 0\r\n"));
    assert!(headers.contains("nessa-webhook-id: 00000000-0000-0000-0000-000000000001\r\n"));
    assert!(headers.contains("nessa-webhook-signature: v1=3ba781a84e593504463f3df9cba8d39cb6993768321265a4048b710e60b5ff04\r\n"));
    assert_eq!(&request[split + 4..], message.body());
    recipient.finish();
}

#[tokio::test]
async fn isolated_tls_recipient_rejects_untrusted_ca_and_wrong_hostname() {
    for (host, trust_root) in [("recipient.test", false), ("wrong.test", true)] {
        let mut recipient = Recipient::start_with_rejection(Vec::new(), false, true);
        let builder = client_builder(host, &[recipient.address]);
        let builder = if trust_root {
            builder.add_root_certificate(reqwest::Certificate::from_der(&recipient.root).unwrap())
        } else {
            builder
        };
        let client = builder.build().unwrap();
        let mut url = recipient.url();
        url.set_host(Some(host)).unwrap();
        let error = client
            .post(url)
            .body("test-only body")
            .send()
            .await
            .unwrap_err();
        let mut cause: Option<&(dyn Error + 'static)> = Some(&error);
        let mut rejected_certificate = false;
        while let Some(error) = cause {
            if matches!(
                error.downcast_ref::<TlsError>(),
                Some(TlsError::InvalidCertificate(_))
            ) {
                rejected_certificate = true;
            }
            // io::Error::source skips its contained error and asks that
            // error for its source. Inspect get_ref to retain rustls's
            // concrete certificate error rather than accepting a TCP error.
            cause = error
                .downcast_ref::<IoError>()
                .and_then(IoError::get_ref)
                .map(|inner| inner as &(dyn Error + 'static))
                .or_else(|| error.source());
        }
        assert!(
            rejected_certificate,
            "the failure must be certificate verification: {error}"
        );
        assert!(
            recipient.request.recv().await.is_none(),
            "TLS rejection precedes HTTP bytes"
        );
        recipient.finish();
    }
}

#[tokio::test]
async fn redirect_is_a_response_and_receives_no_second_request() {
    let redirect_target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    redirect_target.set_nonblocking(true).unwrap();
    let response = format!("HTTP/1.1 302 Found\r\nLocation: https://recipient.test:{}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", redirect_target.local_addr().unwrap().port());
    let recipient = Recipient::start(response.into_bytes(), false);
    assert_eq!(
        call(
            &recipient.client(),
            &recipient.url(),
            &MacKey::new([42; 32]),
            &message()
        )
        .await,
        Ok(302)
    );
    assert_eq!(
        redirect_target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    recipient.finish();
}

#[tokio::test]
async fn responses_are_bounded_without_retaining_remote_text() {
    let oversized = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        RESPONSE_BYTES + 1
    )
    .into_bytes();
    let chunked = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n", RESPONSE_BYTES + 1, "x".repeat(RESPONSE_BYTES + 1)).into_bytes();
    for (response, expected) in [
        (oversized, PostFailure::ResponseTooLarge),
        (chunked, PostFailure::ResponseTooLarge),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nshort".to_vec(),
            PostFailure::IncompleteResponse,
        ),
    ] {
        let recipient = Recipient::start(response, false);
        assert_eq!(
            call(
                &recipient.client(),
                &recipient.url(),
                &MacKey::new([42; 32]),
                &message()
            )
            .await,
            Err(expected)
        );
        recipient.finish();
    }
    let recipient = Recipient::start(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            RESPONSE_BYTES,
            "x".repeat(RESPONSE_BYTES)
        )
        .into_bytes(),
        false,
    );
    assert_eq!(
        call(
            &recipient.client(),
            &recipient.url(),
            &MacKey::new([42; 32]),
            &message()
        )
        .await,
        Ok(200)
    );
    recipient.finish();
}

#[tokio::test]
async fn lost_or_malformed_headers_do_not_claim_zero_response_bytes() {
    for response in [
        b"".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Len".as_slice(),
        b"HTTP/1.1 garbage\r\n\r\n".as_slice(),
    ] {
        let recipient = Recipient::start(response.to_vec(), false);
        let result = call(
            &recipient.client(),
            &recipient.url(),
            &MacKey::new([42; 32]),
            &message(),
        )
        .await;
        assert_eq!(result, Err(PostFailure::IncompleteResponse));
        assert_eq!(observation(result), Observation::Unknown);
        recipient.finish();
    }
}

#[tokio::test]
async fn oversized_headers_remain_unknown() {
    let headers = format!(
        "HTTP/1.1 200 OK\r\nX-Remote: {}\r\nContent-Length: 0\r\n\r\n",
        "x".repeat(1024 * 1024)
    );
    let recipient = Recipient::start(headers.into_bytes(), false);
    let result = call(
        &recipient.client(),
        &recipient.url(),
        &MacKey::new([42; 32]),
        &message(),
    )
    .await;
    assert_eq!(result, Err(PostFailure::IncompleteResponse));
    recipient.finish();
}

#[tokio::test]
async fn a_hung_response_is_bounded() {
    let mut recipient = Recipient::start(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
        true,
    );
    let deadline = Duration::from_millis(150);
    let client = client_builder("recipient.test", &[recipient.address])
        .add_root_certificate(reqwest::Certificate::from_der(&recipient.root).unwrap())
        .timeout(deadline)
        .build()
        .unwrap();
    // A running blocking task inhibits Tokio's paused-clock auto-advance during
    // real TLS I/O. Host scheduling must not turn this response-timeout case
    // into a timeout before receipt. The existing real fixture watchdog stays.
    let (release_clock, held) = std::sync::mpsc::channel();
    let (started, ready) = tokio::sync::oneshot::channel();
    let keeper = tokio::task::spawn_blocking(move || {
        let _ = started.send(());
        held.recv_timeout(RECIPIENT_WATCHDOG).is_ok()
    });
    ready.await.unwrap();
    tokio::time::pause();
    let start = Instant::now();
    let url = recipient.url();
    let key = MacKey::new([42; 32]);
    let message = message();
    let pending = call(&client, &url, &key, &message);
    tokio::pin!(pending);
    tokio::select! {
        received = recipient.request.recv() => assert!(
            received.is_some(), "the timeout is after actual TLS request receipt"
        ),
        result = &mut pending => panic!("POST ended before TLS request receipt: {result:?}"),
    }
    assert_eq!(start.elapsed(), Duration::ZERO);
    // Tokio's timer driver rounds to millisecond ticks. Cross the first tick
    // after the unchanged 150 ms client deadline while the response stays held.
    let elapsed = deadline + Duration::from_millis(1);
    tokio::time::advance(elapsed).await;
    assert_eq!(pending.await, Err(PostFailure::TimedOut));
    assert_eq!(start.elapsed(), elapsed);
    tokio::time::resume();
    release_clock.send(()).unwrap();
    assert!(keeper.await.unwrap(), "the real fixture watchdog expired");
    recipient.finish();
}

#[test]
fn destination_policy_rejects_nonpublic_answers_and_transition_prefixes() {
    for denied in [
        "0.0.0.0",
        "10.1.2.3",
        "100.64.0.1",
        "100.127.255.254",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.255",
        "192.0.0.1",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.1.1",
        "198.18.0.1",
        "198.19.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::ffff:169.254.169.254",
        "64:ff9b::a00:1",
        "2001::1",
        "2001:20::1",
        "2001:db8::1",
        "2002:7f00:1::",
        "3fff:fff::1",
    ] {
        assert!(
            !public_address(denied.parse().unwrap()),
            "accepted {denied}"
        );
    }
    for permitted in [
        "1.1.1.1",
        "8.8.8.8",
        "100.63.255.255",
        "100.128.0.1",
        "172.15.0.1",
        "172.32.0.1",
        "::ffff:8.8.8.8",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
    ] {
        assert!(
            public_address(permitted.parse().unwrap()),
            "refused {permitted}"
        );
    }
}

struct FixedResolver(Vec<IpAddr>);
#[async_trait]
impl AddressResolver for FixedResolver {
    async fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, PrepareFailure> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
async fn bad_dns_answers_are_refused_before_connect() {
    let destination = Destination::parse("https://recipient.test/events").unwrap();
    for addresses in [
        vec![],
        vec!["127.0.0.1".parse().unwrap()],
        vec![
            "8.8.8.8".parse().unwrap(),
            "169.254.169.254".parse().unwrap(),
        ],
        vec!["::ffff:127.0.0.1".parse().unwrap()],
        vec!["8.8.8.8".parse().unwrap(); MAX_ADDRESSES + 1],
    ] {
        let http = HttpsWebhook::new(Arc::new(FixedResolver(addresses)));
        assert_eq!(
            http.prepare(&destination).await.err(),
            Some(PrepareFailure::Destination)
        );
    }
}

#[tokio::test]
async fn local_backpressure_precedes_sending_and_does_not_consume_an_attempt() {
    let http = HttpsWebhook::new(Arc::new(FixedResolver(vec!["8.8.8.8".parse().unwrap()])));
    let destination = Destination::parse("https://recipient.test/events").unwrap();
    let queued = Delivery::queued(Uuid::from_u128(1), 7);
    let mut prepared = Vec::new();
    for _ in 0..MAX_IN_FLIGHT {
        prepared.push(http.prepare(&destination).await.unwrap());
    }
    assert_eq!(
        http.prepare(&destination).await.err(),
        Some(PrepareFailure::Busy)
    );
    assert_eq!(queued.phase(), Phase::Queued);
    assert_eq!(queued.attempts(), 0);
    drop(prepared.pop());
    // No network effect has occurred. The caller can now prepare, admit and
    // persist Sending, then consume this post handle once under its grant hold.
    let next = http.prepare(&destination).await.unwrap();
    let sending = queued.admit(7, 1).unwrap();
    assert_eq!(sending.phase(), Phase::Sending);
    assert_eq!(sending.attempts(), 1);
    drop(next);
    drop(prepared);
    assert_eq!(http.capacity.available_permits(), MAX_IN_FLIGHT);
}

#[tokio::test]
async fn retained_dns_capacity_refuses_preparation_while_delivery_stays_queued() {
    let resolver = Arc::new(SystemResolver::new());
    let held = resolver
        .capacity
        .clone()
        .acquire_many_owned(MAX_IN_FLIGHT as u32)
        .await
        .unwrap();
    let http = HttpsWebhook::new(resolver);
    let queued = Delivery::queued(Uuid::from_u128(1), 7);
    let destination = Destination::parse("https://recipient.test/events").unwrap();
    assert_eq!(
        http.prepare(&destination).await.err(),
        Some(PrepareFailure::Busy)
    );
    assert_eq!(queued.phase(), Phase::Queued);
    assert_eq!(queued.attempts(), 0);
    assert_eq!(http.capacity.available_permits(), MAX_IN_FLIGHT);
    drop(held);
}

#[tokio::test]
async fn cancelled_dns_waiter_keeps_the_physical_workers_permit() {
    let capacity = Arc::new(Semaphore::new(1));
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let task = lookup_worker(capacity.clone(), move || {
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        Ok(vec![Ipv4Addr::LOCALHOST.into()])
    })
    .unwrap();
    ready.await.unwrap();
    task.abort();
    drop(task);
    assert_eq!(capacity.available_permits(), 0);
    assert_eq!(
        lookup_worker(capacity.clone(), || Ok(vec![])).err(),
        Some(PrepareFailure::Busy)
    );
    release.send(()).unwrap();
    let returned = tokio::time::timeout(Duration::from_secs(3), capacity.clone().acquire_owned())
        .await
        .unwrap()
        .unwrap();
    drop(returned);
    assert_eq!(capacity.available_permits(), 1);
}

#[tokio::test]
async fn expired_preparation_refuses_before_polling_a_request() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let capacity = Arc::new(Semaphore::new(1));
    let prepared = Box::new(PreparedPost {
        client: client_builder("recipient.test", &[address])
            .build()
            .unwrap(),
        url: url::Url::parse(&format!("https://recipient.test:{}/events", address.port())).unwrap(),
        deadline: Instant::now() - Duration::from_millis(1),
        _permit: capacity.clone().try_acquire_owned().unwrap(),
    });
    assert_eq!(
        prepared.post(&MacKey::new([42; 32]), &message()).await,
        Err(PostFailure::TimedOut)
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(capacity.available_permits(), 1);
}

struct HeldResolver {
    entered: mpsc::Sender<()>,
    count: AtomicUsize,
}
#[async_trait]
impl AddressResolver for HeldResolver {
    async fn resolve(&self, _: &str) -> Result<Vec<IpAddr>, PrepareFailure> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.entered.send(()).await.unwrap();
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn dns_deadline_and_concurrency_are_bounded_without_a_waiter_queue() {
    let (entered, mut observe) = mpsc::channel(MAX_IN_FLIGHT);
    let resolver = Arc::new(HeldResolver {
        entered,
        count: AtomicUsize::new(0),
    });
    let http = Arc::new(HttpsWebhook::new(resolver.clone()));
    let destination = Destination::parse("https://recipient.test/events").unwrap();
    let mut tasks = Vec::new();
    for _ in 0..MAX_IN_FLIGHT {
        let http = http.clone();
        let destination = destination.clone();
        tasks.push(tokio::spawn(
            async move { http.prepare(&destination).await },
        ));
    }
    for _ in 0..MAX_IN_FLIGHT {
        observe.recv().await.unwrap();
    }
    assert_eq!(
        http.prepare(&destination).await.err(),
        Some(PrepareFailure::Busy)
    );
    assert_eq!(resolver.count.load(Ordering::SeqCst), MAX_IN_FLIGHT);
    tokio::time::advance(POST_DEADLINE).await;
    for task in tasks {
        assert_eq!(task.await.unwrap().err(), Some(PrepareFailure::TimedOut));
    }
    assert_eq!(http.capacity.available_permits(), MAX_IN_FLIGHT);
}
